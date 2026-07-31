use std::path::{Path, PathBuf};

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    domain::{RunState, transition_run},
    error::AppError,
    git_runtime::worktree_is_dirty,
    provider::{self, ProviderRecovery},
};

#[derive(Debug, sqlx::FromRow)]
struct RecoverableRun {
    id: Uuid,
    previous_state: String,
    conversation_id: Uuid,
}

/// Reconciles runs that were non-terminal when the host process stopped.
///
/// Provider and Git inspection happen outside database transactions. A stored
/// provider Thread is resumed and read before the application decides whether
/// completion is provable. Any ambiguous state terminates as interrupted while
/// retaining the exact worktree.
pub async fn reconcile_non_terminal_runs(state: &AppState) -> Result<u64, AppError> {
    let runs = sqlx::query_as::<_, RecoverableRun>(
        r#"
        SELECT id, state AS previous_state, conversation_id
        FROM agent_runs
        WHERE state IN ('created', 'queued', 'starting', 'working', 'blocked')
        ORDER BY created_at
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    for run in &runs {
        let mut transition = state.pool.begin().await?;
        transition_run(&mut transition, run.id, RunState::Recovering, None).await?;
        transition.commit().await?;

        let worktree = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT gw.id, gw.path
            FROM agent_runs ar
            JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
            JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
            WHERE ar.id = $1
              AND gw.state IN ('creating', 'ready', 'dirty', 'missing', 'failed')
            "#,
        )
        .bind(run.id)
        .fetch_optional(&state.pool)
        .await?;

        let (worktree_path, worktree_state, dirty) = match worktree {
            Some((worktree_id, path)) => {
                let inspection_path = PathBuf::from(path.clone());
                let (worktree_state, dirty) =
                    tokio::task::spawn_blocking(move || inspect_worktree(&inspection_path))
                        .await
                        .map_err(|_| AppError::Internal("join recovery Git inspection"))?;
                sqlx::query(
                    r#"
                    UPDATE git_worktrees
                    SET state = $2, dirty = $3,
                        last_inspected_at = now(), updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(worktree_id)
                .bind(worktree_state)
                .bind(dirty)
                .execute(&state.pool)
                .await?;
                (Some(PathBuf::from(path)), worktree_state, dirty)
            }
            None => (None, "missing", false),
        };

        let provider_outcome = match &worktree_path {
            Some(path) if worktree_state != "missing" => {
                provider::inspect_provider_for_recovery(state, run.id, path.clone()).await
            }
            _ => ProviderRecovery::Unavailable,
        };

        let recovered_completion = match &provider_outcome {
            ProviderRecovery::Completed { message } => {
                provider::complete_recovered_run(state, run.id, message)
                    .await
                    .is_ok()
            }
            _ => false,
        };
        if recovered_completion {
            let summary = "Service restarted during a run. Provider history proved that the turn completed, so the durable run was reconciled as completed.";
            let mut transaction = state.pool.begin().await?;
            cancel_pending_approvals(&mut transaction, run.id).await?;
            insert_recovery_event(
                &mut transaction,
                run,
                summary,
                "completed",
                worktree_state,
                dirty,
                true,
            )
            .await?;
            transaction.commit().await?;
            continue;
        }

        let provider_resumed = !matches!(&provider_outcome, ProviderRecovery::Unavailable);
        let reason = match &provider_outcome {
            ProviderRecovery::Resumed { latest_turn_status } => {
                let status = known_turn_status(latest_turn_status);
                format!(
                    "Service restarted during a run. The provider Thread resumed, but its latest turn was {status} and could not be safely continued; the worktree was preserved for retry."
                )
            }
            ProviderRecovery::Completed { .. } => {
                "Service restarted during a run. Provider completion was observed but could not be reconciled transactionally; the worktree was preserved for inspection.".to_owned()
            }
            ProviderRecovery::Unavailable if dirty => {
                "Service restarted during a run. Provider resume was unavailable and the dirty worktree was preserved; inspect it before retrying.".to_owned()
            }
            ProviderRecovery::Unavailable => {
                "Service restarted during a run. Provider resume was unavailable, so the worktree was preserved for an explicit retry.".to_owned()
            }
        };

        let mut transaction = state.pool.begin().await?;
        transition_run(
            &mut transaction,
            run.id,
            RunState::Interrupted,
            Some(&reason),
        )
        .await?;
        retain_workspace(&mut transaction, run.id).await?;
        cancel_pending_approvals(&mut transaction, run.id).await?;
        insert_recovery_event(
            &mut transaction,
            run,
            &reason,
            "interrupted",
            worktree_state,
            dirty,
            provider_resumed,
        )
        .await?;
        transaction.commit().await?;
    }

    Ok(runs.len() as u64)
}

pub async fn reconcile_registered_worktrees(state: &AppState) -> Result<u64, AppError> {
    let worktrees = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, path
        FROM git_worktrees
        WHERE state IN ('creating', 'ready', 'dirty', 'missing', 'failed')
        ORDER BY created_at
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    for (worktree_id, path) in &worktrees {
        let inspection_path = PathBuf::from(path);
        let (worktree_state, dirty) =
            tokio::task::spawn_blocking(move || inspect_worktree(&inspection_path))
                .await
                .map_err(|_| AppError::Internal("join registered worktree inspection"))?;
        sqlx::query(
            r#"
            UPDATE git_worktrees
            SET state = $2, dirty = $3,
                last_inspected_at = now(), updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(worktree_id)
        .bind(worktree_state)
        .bind(dirty)
        .execute(&state.pool)
        .await?;
    }
    Ok(worktrees.len() as u64)
}

pub async fn cancel_terminal_run_approvals(state: &AppState) -> Result<u64, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE agent_approvals aa
        SET status = 'canceled'
        FROM agent_runs ar
        WHERE ar.id = aa.agent_run_id
          AND aa.status = 'pending'
          AND ar.state IN ('completed', 'failed', 'canceled', 'interrupted')
        "#,
    )
    .execute(&state.pool)
    .await?;
    Ok(result.rows_affected())
}

fn inspect_worktree(path: &Path) -> (&'static str, bool) {
    if !path.is_dir() {
        return ("missing", false);
    }
    match worktree_is_dirty(path) {
        Ok(false) => ("ready", false),
        Ok(true) => ("dirty", true),
        Err(_) => ("missing", false),
    }
}

fn known_turn_status(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "interrupted" => "interrupted",
        "failed" => "failed",
        "inProgress" => "in progress",
        "missing" => "missing",
        _ => "unknown",
    }
}

async fn retain_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_pending_approvals(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE agent_approvals
        SET status = 'canceled'
        WHERE agent_run_id = $1 AND status = 'pending'
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_recovery_event(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RecoverableRun,
    summary: &str,
    result: &str,
    worktree_state: &str,
    dirty: bool,
    provider_resumed: bool,
) -> Result<(), AppError> {
    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, $3, 'recovery', $4, $5)
        "#,
    )
    .bind(event_id)
    .bind(run.id)
    .bind(format!("startup-recovery:{event_id}"))
    .bind(summary)
    .bind(serde_json::json!({
        "previous_state": run.previous_state,
        "result": result,
        "provider_resumed": provider_resumed,
        "worktree_preserved": true,
        "worktree_state": worktree_state,
        "dirty": dirty
    }))
    .execute(&mut **transaction)
    .await?;
    let sequence =
        crate::domain::allocate_conversation_sequence(transaction, run.conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, agent_run_event_id
        )
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(run.conversation_id)
    .bind(sequence)
    .bind(event_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{inspect_worktree, known_turn_status};
    use std::path::Path;

    #[test]
    fn missing_worktree_is_not_deleted_or_called_clean() {
        assert_eq!(
            inspect_worktree(Path::new("/definitely/not/an/agent-workspace")),
            ("missing", false)
        );
    }

    #[test]
    fn provider_status_is_bounded_to_known_values() {
        assert_eq!(known_turn_status("inProgress"), "in progress");
        assert_eq!(known_turn_status("secret provider text"), "unknown");
    }
}
