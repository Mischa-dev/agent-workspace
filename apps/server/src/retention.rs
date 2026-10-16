use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, git_runtime};

const WORKTREE_RETENTION_DAYS: i32 = 7;

#[derive(Debug, sqlx::FromRow)]
struct WorktreeCandidate {
    worktree_id: Uuid,
    workspace_id: Uuid,
    repository_id: Uuid,
    mirror_path: String,
    worktree_path: String,
    branch_name: String,
    base_commit: String,
    published_commit: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRetentionResult {
    pub policy_version: u32,
    pub mode: &'static str,
    pub database_candidates: u64,
    pub eligible_clean_worktrees: u64,
    pub removed_worktrees: u64,
    pub preserved_worktrees: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEventRetentionResult {
    pub policy_version: u32,
    pub mode: &'static str,
    pub expired_candidates: u64,
    pub removed_events: u64,
    pub remaining_expired_events: u64,
}

pub async fn clean_external_events(
    pool: &PgPool,
    apply: bool,
) -> Result<ExternalEventRetentionResult, AppError> {
    let expired_candidates = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM external_agent_events WHERE expires_at <= now()",
    )
    .fetch_one(pool)
    .await?
    .try_into()
    .map_err(|_| AppError::Internal("expired external event count"))?;
    let removed_events = if apply {
        sqlx::query(
            r#"
            WITH candidates AS (
                SELECT id
                FROM external_agent_events
                WHERE expires_at <= now()
                ORDER BY expires_at, id
                LIMIT 10000
                FOR UPDATE SKIP LOCKED
            )
            DELETE FROM external_agent_events event
            USING candidates
            WHERE event.id = candidates.id
            "#,
        )
        .execute(pool)
        .await?
        .rows_affected()
    } else {
        0
    };
    let remaining_expired_events = if apply {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM external_agent_events WHERE expires_at <= now()",
        )
        .fetch_one(pool)
        .await?
        .try_into()
        .map_err(|_| AppError::Internal("remaining expired external event count"))?
    } else {
        expired_candidates
    };
    Ok(ExternalEventRetentionResult {
        policy_version: 1,
        mode: if apply { "apply" } else { "dry_run" },
        expired_candidates,
        removed_events,
        remaining_expired_events,
    })
}

pub async fn clean_worktrees(
    pool: &PgPool,
    data_root: &Path,
    apply: bool,
) -> Result<WorktreeRetentionResult, AppError> {
    let live_worktrees: u64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM git_worktrees WHERE state IN ('creating','ready','dirty','missing','failed')",
    )
    .fetch_one(pool)
    .await?
    .try_into()
    .map_err(|_| AppError::Internal("live worktree count"))?;
    let candidates = load_candidates(pool).await?;
    let database_candidates = candidates.len() as u64;
    let mut eligible = 0_u64;
    let mut removed = 0_u64;
    for candidate in candidates {
        if !candidate_paths_match(data_root, &candidate) {
            continue;
        }
        if !apply {
            let mirror = PathBuf::from(&candidate.mirror_path);
            let worktree = PathBuf::from(&candidate.worktree_path);
            let inspection = git_runtime::inspect_worktree(&mirror, &worktree);
            if inspection.is_ok_and(|value| !value.dirty && value.branch == candidate.branch_name)
                && git_runtime::inspect_publishable_worktree(
                    &mirror,
                    &worktree,
                    &candidate.branch_name,
                    &candidate.base_commit,
                )
                .is_ok_and(|value| value.head_commit == candidate.published_commit)
            {
                eligible = eligible.saturating_add(1);
            }
            continue;
        }

        let mut transaction = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("repository-retention:{}", candidate.repository_id))
            .execute(&mut *transaction)
            .await?;
        if !candidate_is_current(&mut transaction, &candidate).await? {
            transaction.rollback().await?;
            continue;
        }
        let mirror = PathBuf::from(&candidate.mirror_path);
        let worktree = PathBuf::from(&candidate.worktree_path);
        let branch_name = candidate.branch_name.clone();
        let published_commit = candidate.published_commit.clone();
        let removal = tokio::task::spawn_blocking(move || {
            git_runtime::remove_published_worktree(
                &mirror,
                &worktree,
                &branch_name,
                &published_commit,
            )
        })
        .await
        .map_err(|_| AppError::Internal("join worktree retention"))?;
        if removal.is_err() {
            transaction.rollback().await?;
            continue;
        }
        eligible = eligible.saturating_add(1);
        sqlx::query(
            "UPDATE git_worktrees SET state='removed', dirty=false, last_inspected_at=now(), updated_at=now() WHERE id=$1 AND state='ready'",
        )
        .bind(candidate.worktree_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE repository_workspaces SET state='cleaned', updated_at=now() WHERE id=$1 AND state='retained'",
        )
        .bind(candidate.workspace_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        removed = removed.saturating_add(1);
    }
    Ok(WorktreeRetentionResult {
        policy_version: 1,
        mode: if apply { "apply" } else { "dry_run" },
        database_candidates,
        eligible_clean_worktrees: eligible,
        removed_worktrees: removed,
        preserved_worktrees: live_worktrees.saturating_sub(removed),
    })
}

async fn load_candidates(pool: &PgPool) -> Result<Vec<WorktreeCandidate>, AppError> {
    Ok(sqlx::query_as::<_, WorktreeCandidate>(candidate_query())
        .bind(WORKTREE_RETENTION_DAYS)
        .fetch_all(pool)
        .await?)
}

async fn candidate_is_current(
    transaction: &mut Transaction<'_, Postgres>,
    candidate: &WorktreeCandidate,
) -> Result<bool, AppError> {
    Ok(sqlx::query_as::<_, WorktreeCandidate>(&format!(
        "{} AND worktree.id=$2 AND workspace.id=$3 FOR UPDATE OF worktree, workspace",
        candidate_query()
    ))
    .bind(WORKTREE_RETENTION_DAYS)
    .bind(candidate.worktree_id)
    .bind(candidate.workspace_id)
    .fetch_optional(&mut **transaction)
    .await?
    .is_some())
}

fn candidate_query() -> &'static str {
    r#"
    SELECT worktree.id AS worktree_id, workspace.id AS workspace_id,
           repository.id AS repository_id, repository.mirror_path,
           worktree.path AS worktree_path, workspace.branch_name, workspace.base_commit,
           workspace.published_commit
    FROM git_worktrees worktree
    JOIN repository_workspaces workspace
      ON workspace.id = worktree.repository_workspace_id
    JOIN repositories repository ON repository.id = workspace.repository_id
    WHERE worktree.state = 'ready'
      AND workspace.state = 'retained'
      AND workspace.published_commit IS NOT NULL
      AND workspace.updated_at <= now() - make_interval(days => $1)
      AND NOT EXISTS (
        SELECT 1 FROM agent_runs run
        WHERE run.repository_workspace_id = workspace.id
          AND run.state IN ('created','queued','starting','working','blocked','recovering')
      )
      AND NOT EXISTS (
        SELECT 1 FROM provider_sessions session
        WHERE session.conversation_id = workspace.conversation_id
          AND session.repository_id = workspace.repository_id
          AND session.state IN ('new','active')
      )
    "#
}

fn candidate_paths_match(data_root: &Path, candidate: &WorktreeCandidate) -> bool {
    let Ok(expected_mirror) =
        git_runtime::expected_repository_mirror_path(data_root, candidate.repository_id)
    else {
        return false;
    };
    let Ok(expected_worktree) = git_runtime::managed_worktree_path(
        data_root,
        candidate.repository_id,
        candidate.workspace_id,
    ) else {
        return false;
    };
    Path::new(&candidate.mirror_path) == expected_mirror
        && Path::new(&candidate.worktree_path) == expected_worktree
}

/// O3: the retention commands, run on a schedule as background jobs
/// instead of by hand. The host commands still work for a dry run.
pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(EXTERNAL_EVENTS_JOB, |context| async move {
        let result = clean_external_events(&context.state.pool, true).await?;
        tracing::info!(
            removed = result.removed_events,
            remaining = result.remaining_expired_events,
            "trimmed expired agent events"
        );
        Ok(())
    });
    registry.every(
        EXTERNAL_EVENTS_JOB,
        std::time::Duration::from_secs(6 * 60 * 60),
    );
    registry.register_with_timeout(
        WORKTREES_JOB,
        std::time::Duration::from_secs(30 * 60),
        |context| async move {
            let result =
                clean_worktrees(&context.state.pool, &context.state.config.data_root, true).await?;
            tracing::info!(
                removed = result.removed_worktrees,
                preserved = result.preserved_worktrees,
                "removed retained worktrees past their retention"
            );
            Ok(())
        },
    );
    registry.every(WORKTREES_JOB, std::time::Duration::from_secs(24 * 60 * 60));
}

pub const EXTERNAL_EVENTS_JOB: &str = "retention.external_events";
pub const WORKTREES_JOB: &str = "retention.worktrees";
