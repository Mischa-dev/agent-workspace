use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    domain::{RunState, transition_run},
    error::AppError,
};

#[derive(Debug, sqlx::FromRow)]
struct RecoverableRun {
    id: Uuid,
    previous_state: String,
    conversation_id: Uuid,
}

/// Reconciles runs that were non-terminal when the host process stopped.
///
/// M0 cannot safely resume a provider thread until its real Codex profile is
/// authenticated. It therefore records recovery, inspects and preserves the
/// worktree, and terminates the old run with an actionable explanation.
pub async fn reconcile_non_terminal_runs(pool: &PgPool) -> Result<u64, AppError> {
    let mut transaction = pool.begin().await?;
    let runs = sqlx::query_as::<_, RecoverableRun>(
        r#"
        SELECT id, state AS previous_state, conversation_id
        FROM agent_runs
        WHERE state IN ('created', 'queued', 'starting', 'working', 'blocked')
        FOR UPDATE
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;

    for run in &runs {
        transition_run(&mut transaction, run.id, RunState::Recovering, None).await?;

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
        .fetch_optional(&mut *transaction)
        .await?;

        let reason = match worktree {
            Some((worktree_id, path)) => {
                let (state, dirty) = inspect_worktree(&path);
                sqlx::query(
                    r#"
                    UPDATE git_worktrees
                    SET state = $2, dirty = $3, last_inspected_at = now(), updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(worktree_id)
                .bind(state)
                .bind(dirty)
                .execute(&mut *transaction)
                .await?;
                if dirty {
                    "Service restarted during a run. The dirty worktree was preserved; inspect it before retrying."
                } else {
                    "Service restarted during a run. Provider resume was not provable, so the clean worktree was preserved."
                }
            }
            None => {
                "Service restarted during a run. No live worktree could be proven; inspect the retained workspace."
            }
        };

        transition_run(
            &mut transaction,
            run.id,
            RunState::Interrupted,
            Some(reason),
        )
        .await?;
        sqlx::query(
            r#"
            UPDATE repository_workspaces rw
            SET state = 'retained', updated_at = now()
            FROM agent_runs ar
            WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
            "#,
        )
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
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
        .bind(reason)
        .bind(serde_json::json!({
            "previous_state": run.previous_state,
            "result": "interrupted",
            "worktree_preserved": true
        }))
        .execute(&mut *transaction)
        .await?;
        let sequence =
            crate::domain::allocate_conversation_sequence(&mut transaction, run.conversation_id)
                .await?;
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
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;
    Ok(runs.len() as u64)
}

fn inspect_worktree(path: &str) -> (&'static str, bool) {
    let worktree = std::path::Path::new(path);
    if !worktree.is_dir() {
        return ("missing", false);
    }
    let output = std::process::Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "status", "--porcelain=v1"])
        .current_dir(worktree)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() && output.stdout.is_empty() => ("ready", false),
        Ok(output) if output.status.success() => ("dirty", true),
        _ => ("missing", false),
    }
}

#[cfg(test)]
mod tests {
    use super::inspect_worktree;

    #[test]
    fn missing_worktree_is_not_deleted_or_called_clean() {
        assert_eq!(
            inspect_worktree("/definitely/not/an/agent-workspace"),
            ("missing", false)
        );
    }
}
