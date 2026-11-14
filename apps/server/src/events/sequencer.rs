//! The sequencer (K3 3.3.1): moves committed events from `event_staging`
//! into `events`, giving them dense positions in commit order.
//!
//! Any process may run it and none needs a lease: each step locks the one
//! `event_position_counter` row first, so steps run one at a time across
//! processes, and a step that can't get the row within two seconds gives up
//! until its next round. Each statement in a step sees what committed
//! before it started, so a writer still open is simply not there yet: its
//! events get later positions when it commits, and a reader that follows
//! positions never skips one. Nothing here is a session lock or setting
//! (ruling R24 rule 3).
//!
//! The `event_staging_wake` trigger notifies at commit; a listener (K5.4)
//! turns that into `wake`. The loop also polls every 250 ms, so it works
//! with no listener at all, as behind a transaction pooler.

use std::{sync::Arc, time::Duration};

use sqlx::PgPool;
use tokio::sync::{Notify, watch};

use crate::error::AppError;

/// How often the loop looks without a wake.
pub const POLL: Duration = Duration::from_millis(250);

/// The most events one step moves.
const BATCH: i64 = 1000;

/// One step: positions for up to a batch of committed events. Returns how
/// many it moved (0 when there was nothing, or another process held the
/// counter).
pub async fn step(pool: &PgPool) -> Result<u64, AppError> {
    // Skip empty rounds without taking the counter row.
    if sqlx::query_scalar::<_, i32>("SELECT 1 FROM event_staging LIMIT 1")
        .fetch_optional(pool)
        .await?
        .is_none()
    {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *transaction)
        .await?;
    let base = match sqlx::query_scalar::<_, i64>(
        "SELECT next_position FROM event_position_counter FOR UPDATE",
    )
    .fetch_one(&mut *transaction)
    .await
    {
        Ok(base) => base,
        // Another process is sequencing: it moves these too.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
            return Ok(0);
        }
        Err(error) => return Err(error.into()),
    };
    let moved = sqlx::query_scalar::<_, i64>(
        r#"
        WITH batch AS (
            DELETE FROM event_staging
            WHERE id IN (SELECT id FROM event_staging ORDER BY ordinal LIMIT $2)
            RETURNING *
        ),
        moved AS (
            INSERT INTO events (
                position, id, workspace_id, space_id, thing_id, thing_kind, access_thing_id,
                thing_version, type, class, changes, audience, conversation_id,
                conversation_restricted, audience_account_id, actor_account_id,
                via_credential_id, requested_by_account_id, run_id, request_id, legacy_type,
                payload, created_at)
            SELECT $1 + row_number() OVER (ORDER BY ordinal) - 1, id, workspace_id, space_id,
                   thing_id, thing_kind, access_thing_id, thing_version, type, class, changes,
                   audience, conversation_id, conversation_restricted, audience_account_id,
                   actor_account_id, via_credential_id, requested_by_account_id, run_id,
                   request_id, legacy_type, payload, created_at
            FROM batch
            RETURNING 1
        )
        SELECT count(*) FROM moved
        "#,
    )
    .bind(base)
    .bind(BATCH)
    .fetch_one(&mut *transaction)
    .await?;
    if moved > 0 {
        sqlx::query("UPDATE event_position_counter SET next_position = $1")
            .bind(base + moved)
            .execute(&mut *transaction)
            .await?;
        // Delivered at commit: the tails (K5.4) wake on it.
        sqlx::query("SELECT pg_notify(realtime_channel('events'), $1::text)")
            .bind(base + moved - 1)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(u64::try_from(moved).unwrap_or(0))
}

/// Run until shutdown: a step on every wake and every poll. A database
/// error is logged and retried; it never stops the server.
pub async fn run(pool: PgPool, wake: Arc<Notify>, mut shutdown: watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        // A full batch means more may be waiting: go again at once.
        let full = match step(&pool).await {
            Ok(moved) => moved >= BATCH as u64,
            Err(error) => {
                tracing::warn!(
                    error_category = error.log_category(),
                    "event sequencer step failed"
                );
                false
            }
        };
        if full {
            continue;
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(POLL) => {}
            _ = shutdown.changed() => {}
        }
    }
}
