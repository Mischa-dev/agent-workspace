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

/// The built-in host's lease a sweep holds while it removes worktrees: each
/// removal checks it is still held, and keeps it from being taken until
/// that removal is recorded. A sweep run by hand (the command line) has
/// none.
pub type Holding = Option<(crate::cluster::Cluster, crate::cluster::Lease)>;

pub async fn clean_worktrees(
    pool: &PgPool,
    data_root: &Path,
    apply: bool,
    holding: Holding,
) -> Result<WorktreeRetentionResult, AppError> {
    // Retention runs over every workspace.
    crate::tenancy::system(
        "retention",
        Box::pin(clean_worktrees_here(pool, data_root, apply, holding)),
    )
    .await
}

async fn clean_worktrees_here(
    pool: &PgPool,
    data_root: &Path,
    apply: bool,
    holding: Holding,
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
        // The mirror is wherever this host keeps it; the worktree must be
        // the one the server recorded.
        let Some(mirror) = candidate_mirror(data_root, &candidate) else {
            continue;
        };
        if !apply {
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

        // Another holder works on this disk now: this sweep stops here.
        // Checked in a transaction of its own: holding the lease row while
        // the removal waits on Git would stall the lease's renewal.
        if let Some((cluster, lease)) = holding.as_ref() {
            let mut check = crate::db::scoped_tx(pool).await?;
            cluster.fence(&mut check, lease).await?;
            check.commit().await?;
        }
        let mut transaction = crate::db::scoped_tx(pool).await?;
        lock_repository_retention(&mut transaction, candidate.repository_id).await?;
        if !candidate_is_current(&mut transaction, &candidate).await? {
            transaction.rollback().await?;
            continue;
        }
        let worktree = PathBuf::from(&candidate.worktree_path);
        let branch_name = candidate.branch_name.clone();
        let published_commit = candidate.published_commit.clone();
        if holding
            .as_ref()
            .is_some_and(|(_, lease)| lease.lost.is_cancelled())
        {
            transaction.rollback().await?;
            return Err(AppError::LeaseLost);
        }
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
           repository.id AS repository_id,
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

/// The mirror this host keeps for a candidate's repository, when the
/// candidate's worktree is where this host keeps it. The stored mirror path
/// is the registering server's and isn't read.
fn candidate_mirror(data_root: &Path, candidate: &WorktreeCandidate) -> Option<PathBuf> {
    let mirror = crate::hosts::builtin::git::expected_repository_mirror_path(
        data_root,
        candidate.repository_id,
    )
    .ok()?;
    let expected_worktree = crate::hosts::builtin::git::managed_worktree_path(
        data_root,
        candidate.repository_id,
        candidate.workspace_id,
    )
    .ok()?;
    (Path::new(&candidate.worktree_path) == expected_worktree).then_some(mirror)
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
            let holding = match context.state.supervisors.cluster() {
                // A state without a cluster (a test's) is its own host.
                None => None,
                Some(cluster) => {
                    match cluster.held_lease(&crate::cluster::LeaseName::BuiltinHost) {
                        Some(lease) => Some((cluster.clone(), lease)),
                        // The host is elsewhere now: its holder sweeps.
                        None => return Ok(()),
                    }
                }
            };
            let result =
                tokio::spawn(
                    async move { clean_worktrees(&pool, &data_root, true, holding).await },
                )
                .await
                .map_err(|_| AppError::Internal("join worktree retention"))?;
            let result = match result {
                // The lease went mid-sweep: the next holder sweeps the rest.
                Err(AppError::LeaseLost) => {
                    tracing::info!("worktree retention stopped: the built-in host moved");
                    return Ok(());
                }
                other => other?,
            };
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
