//! A1.17: the built-in host's own upkeep of the worktrees it made: a
//! retained worktree whose branch was published and whose run is long over
//! is removed from this machine's disk. A host job (K5.3), so it runs where
//! the worktrees are, on whichever instance or host program holds the
//! built-in host's lease, never on a server that only serves the app.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

pub const WORKTREES_JOB: &str = "retention.worktrees";

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

/// Held while a retained worktree is removed, and by the scheduler while it
/// claims one for a retry, so a cleanup never deletes a worktree a retry is
/// taking over.
pub async fn lock_repository_retention(
    transaction: &mut Transaction<'_, Postgres>,
    repository_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("repository-retention:{repository_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub async fn clean_worktrees(
    pool: &PgPool,
    data_root: &Path,
    apply: bool,
) -> Result<WorktreeRetentionResult, AppError> {
    // Retention runs over every workspace.
    crate::tenancy::system(
        "retention",
        Box::pin(clean_worktrees_here(pool, data_root, apply)),
    )
    .await
}

async fn clean_worktrees_here(
    pool: &PgPool,
    data_root: &Path,
    apply: bool,
) -> Result<WorktreeRetentionResult, AppError> {
    let live_worktrees: u64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM git_worktrees WHERE state IN ('creating','ready','dirty','missing','failed')",
    )
    .fetch_one(crate::db::scoped(pool))
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
            let inspection = crate::hosts::builtin::git::inspect_worktree(&mirror, &worktree);
            if inspection.is_ok_and(|value| !value.dirty && value.branch == candidate.branch_name)
                && crate::hosts::builtin::git::inspect_publishable_worktree(
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

        let mut transaction = crate::db::scoped_tx(pool).await?;
        lock_repository_retention(&mut transaction, candidate.repository_id).await?;
        if !candidate_is_current(&mut transaction, &candidate).await? {
            transaction.rollback().await?;
            continue;
        }
        let mirror = PathBuf::from(&candidate.mirror_path);
        let worktree = PathBuf::from(&candidate.worktree_path);
        let branch_name = candidate.branch_name.clone();
        let published_commit = candidate.published_commit.clone();
        let removal = tokio::task::spawn_blocking(move || {
            crate::hosts::builtin::git::remove_published_worktree(
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
    Ok(sqlx::query_as::<_, WorktreeCandidate>(&candidate_query())
        .bind(WORKTREE_RETENTION_DAYS)
        .fetch_all(crate::db::scoped(pool))
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

fn candidate_query() -> String {
    format!(
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
      AND NOT {active_run}
      AND NOT EXISTS (
        SELECT 1 FROM provider_sessions session
        WHERE session.conversation_id = workspace.conversation_id
          AND session.repository_id = workspace.repository_id
          AND session.state IN ('new','active')
      )
    "#,
        active_run = crate::runs::sql::workspace_has_active_run("workspace.id"),
    )
}

fn candidate_paths_match(data_root: &Path, candidate: &WorktreeCandidate) -> bool {
    let Ok(expected_mirror) = crate::hosts::builtin::git::expected_repository_mirror_path(
        data_root,
        candidate.repository_id,
    ) else {
        return false;
    };
    let Ok(expected_worktree) = crate::hosts::builtin::git::managed_worktree_path(
        data_root,
        candidate.repository_id,
        candidate.workspace_id,
    ) else {
        return false;
    };
    Path::new(&candidate.mirror_path) == expected_mirror
        && Path::new(&candidate.worktree_path) == expected_worktree
}

/// Removes retained worktrees past their retention, daily.
pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register_host_with_timeout(
        WORKTREES_JOB,
        std::time::Duration::from_secs(30 * 60),
        |context| async move {
            // Run the cleanup as its own task: if the job times out, only
            // this wait is dropped. A removal in progress keeps its
            // transaction and locks until git is done, so a retry waits
            // for it instead of racing it.
            let pool = context.state.pool.clone();
            let data_root = context.state.config.data_root.clone();
            let result =
                tokio::spawn(async move { clean_worktrees(&pool, &data_root, true).await })
                    .await
                    .map_err(|_| AppError::Internal("join worktree retention"))??;
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
