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
pub struct DeliveryRetentionResult {
    pub policy_version: u32,
    pub mode: &'static str,
    pub expired_candidates: u64,
    pub removed_deliveries: u64,
    pub remaining_expired_deliveries: u64,
}

/// How long a finished webhook delivery (delivered, failed or skipped) is
/// kept for its connection's health page. Pending ones are never trimmed.
const DELIVERY_KEEP: &str = "7 days";

/// Trims finished webhook deliveries older than [`DELIVERY_KEEP`], one
/// bounded batch per call. The events themselves stay on the log's own
/// retention.
pub async fn clean_deliveries(
    pool: &PgPool,
    apply: bool,
) -> Result<DeliveryRetentionResult, AppError> {
    // Retention runs over every workspace.
    crate::tenancy::system("retention", clean_deliveries_here(pool, apply)).await
}

async fn clean_deliveries_here(
    pool: &PgPool,
    apply: bool,
) -> Result<DeliveryRetentionResult, AppError> {
    let count = format!(
        "SELECT count(*) FROM deliveries \
         WHERE state <> 'pending' AND created_at < now() - interval '{DELIVERY_KEEP}'"
    );
    let expired_candidates = sqlx::query_scalar::<_, i64>(&count)
        .fetch_one(crate::db::scoped(pool))
        .await?
        .try_into()
        .map_err(|_| AppError::Internal("expired delivery count"))?;
    let removed_deliveries = if apply {
        sqlx::query(&format!(
            r#"
            WITH candidates AS (
                SELECT connection_id, event_position
                FROM deliveries
                WHERE state <> 'pending' AND created_at < now() - interval '{DELIVERY_KEEP}'
                ORDER BY created_at
                LIMIT 10000
                FOR UPDATE SKIP LOCKED
            )
            DELETE FROM deliveries delivery
            USING candidates
            WHERE delivery.connection_id = candidates.connection_id
              AND delivery.event_position = candidates.event_position
            "#
        ))
        .execute(crate::db::scoped(pool))
        .await?
        .rows_affected()
    } else {
        0
    };
    let remaining_expired_deliveries = if apply {
        sqlx::query_scalar::<_, i64>(&count)
            .fetch_one(crate::db::scoped(pool))
            .await?
            .try_into()
            .map_err(|_| AppError::Internal("remaining expired delivery count"))?
    } else {
        expired_candidates
    };
    Ok(DeliveryRetentionResult {
        policy_version: 2,
        mode: if apply { "apply" } else { "dry_run" },
        expired_candidates,
        removed_deliveries,
        remaining_expired_deliveries,
    })
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
    crate::tenancy::system("retention", clean_worktrees_here(pool, data_root, apply)).await
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
    registry.register(DELIVERIES_JOB, |context| async move {
        // Each pass deletes one bounded batch; keep going until nothing
        // expired is left, so a busy server never falls behind for good.
        let started = std::time::Instant::now();
        let mut removed = 0;
        let remaining = loop {
            let result = clean_deliveries(&context.state.pool, true).await?;
            removed += result.removed_deliveries;
            if result.remaining_expired_deliveries == 0
                || result.removed_deliveries == 0
                || started.elapsed() > std::time::Duration::from_secs(8 * 60)
            {
                break result.remaining_expired_deliveries;
            }
        };
        tracing::info!(removed, remaining, "trimmed finished webhook deliveries");
        if remaining > 0 {
            crate::metrics::alert(crate::metrics::Alert {
                key: "retention_behind",
                subject: DELIVERIES_JOB.to_owned(),
                detail: format!(
                    "Delivery retention removed {removed} rows and still has {remaining} expired rows left."
                ),
            });
        }
        Ok(())
    });
    registry.every(DELIVERIES_JOB, std::time::Duration::from_secs(6 * 60 * 60));
    registry.register_with_timeout(
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
    registry.register(DATABASE_WINDOWS_JOB, |context| async move {
        let removed = clean_database_windows(&context.state.pool).await?;
        tracing::info!(removed, "trimmed rows past their retention windows");
        Ok(())
    });
    registry.every(
        DATABASE_WINDOWS_JOB,
        std::time::Duration::from_secs(24 * 60 * 60),
    );
}

pub const DATABASE_WINDOWS_JOB: &str = "retention.database_windows";

/// The fixed database windows of docs/operations/retention.md, the same
/// statements `agent-workspace-host-maintenance retention-cleanup --apply
/// true` runs, under the same advisory lock so the two never overlap.
/// Returns how many rows changed.
pub async fn clean_database_windows(pool: &PgPool) -> Result<i64, AppError> {
    // Retention runs over every workspace.
    crate::tenancy::system("retention", clean_database_windows_here(pool)).await
}

async fn clean_database_windows_here(pool: &PgPool) -> Result<i64, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(470127786, 1)")
        .execute(&mut *transaction)
        .await?;
    let changed = sqlx::query_scalar::<_, i64>(
        r#"
        WITH
        expired_idempotency AS (DELETE FROM idempotency_keys WHERE expires_at <= now() - interval '1 day' RETURNING 1),
        expired_invitations AS (UPDATE project_invitations SET status='revoked', revoked_at=now(), version=version+1, updated_at=now() WHERE status='active' AND expires_at <= now() RETURNING 1),
        oauth_authorization_codes AS (DELETE FROM mcp_oauth_authorization_codes WHERE (used_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '1 day' RETURNING 1),
        oauth_access_tokens AS (DELETE FROM mcp_oauth_access_tokens WHERE (revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '7 days' RETURNING 1),
        refresh_eligible AS (SELECT id FROM mcp_oauth_refresh_tokens WHERE (used_at IS NOT NULL OR revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '30 days' FOR UPDATE),
        refresh_links AS (UPDATE mcp_oauth_refresh_tokens SET rotated_to_token_id=NULL WHERE rotated_to_token_id IN (SELECT id FROM refresh_eligible) RETURNING 1),
        oauth_refresh_tokens AS (DELETE FROM mcp_oauth_refresh_tokens WHERE id IN (SELECT id FROM refresh_eligible) RETURNING 1),
        completed_temporary_jobs AS (DELETE FROM github_jobs WHERE state IN ('completed', 'exhausted') AND updated_at <= now() - interval '30 days' RETURNING 1),
        stopped_instances AS (DELETE FROM server_instances WHERE coalesce(stopped_at, heartbeat_at) <= now() - interval '7 days' RETURNING 1),
        old_ended_mcp_sessions AS (DELETE FROM mcp_ended_sessions WHERE ended_at <= now() - interval '30 days' RETURNING 1),
        old_incoming_hook_deliveries AS (DELETE FROM incoming_hook_deliveries WHERE received_at <= now() - interval '30 days' RETURNING 1)
        SELECT (SELECT count(*) FROM expired_idempotency)
             + (SELECT count(*) FROM expired_invitations)
             + (SELECT count(*) FROM oauth_authorization_codes)
             + (SELECT count(*) FROM oauth_access_tokens)
             + (SELECT count(*) FROM oauth_refresh_tokens)
             + (SELECT count(*) FROM completed_temporary_jobs)
             + (SELECT count(*) FROM stopped_instances)
             + (SELECT count(*) FROM old_ended_mcp_sessions)
             + (SELECT count(*) FROM old_incoming_hook_deliveries)
        "#,
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(changed)
}

pub const DELIVERIES_JOB: &str = "retention.deliveries";
pub const WORKTREES_JOB: &str = "retention.worktrees";
