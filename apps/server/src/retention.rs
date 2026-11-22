use serde::Serialize;
use sqlx::PgPool;

use crate::error::AppError;

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
    crate::tenancy::system("retention", Box::pin(clean_deliveries_here(pool, apply))).await
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
    crate::tenancy::system("retention", Box::pin(clean_database_windows_here(pool))).await
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
        old_incoming_hook_deliveries AS (DELETE FROM incoming_hook_deliveries WHERE received_at <= now() - interval '30 days' RETURNING hook_id, octet_length(body) AS body_bytes),
        released_hook_holds AS (UPDATE incoming_hooks hook SET held_deliveries = greatest(hook.held_deliveries - cleared.deliveries, 0), held_bytes = greatest(hook.held_bytes - cleared.bytes, 0) FROM (SELECT hook_id, count(body_bytes)::integer AS deliveries, COALESCE(sum(body_bytes), 0)::bigint AS bytes FROM old_incoming_hook_deliveries GROUP BY hook_id) cleared WHERE hook.id = cleared.hook_id RETURNING 1),
        finished_operation_tasks AS (DELETE FROM operation_tasks WHERE finished_at <= now() - interval '7 days' RETURNING 1)
        SELECT (SELECT count(*) FROM expired_idempotency)
             + (SELECT count(*) FROM expired_invitations)
             + (SELECT count(*) FROM oauth_authorization_codes)
             + (SELECT count(*) FROM oauth_access_tokens)
             + (SELECT count(*) FROM oauth_refresh_tokens)
             + (SELECT count(*) FROM completed_temporary_jobs)
             + (SELECT count(*) FROM stopped_instances)
             + (SELECT count(*) FROM old_ended_mcp_sessions)
             + (SELECT count(*) FROM old_incoming_hook_deliveries)
             + (SELECT count(*) FROM finished_operation_tasks)
        "#,
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(changed)
}

pub const DELIVERIES_JOB: &str = "retention.deliveries";
