//! The log's upkeep, as jobs on the queue (K3 3.3.1): `events.partitions`
//! adds the next partition of ten million positions once the newest is 80%
//! full, moving anything that landed in the default partition first, and
//! `events.retention` deletes signals after 7 days and facts after the
//! workspace's event retention (400 days by default). These are the log's
//! listed retention jobs (rule 16).

use std::time::Duration;

use sqlx::PgPool;

use crate::{error::AppError, jobs::JobRegistry};

pub const PARTITIONS_KIND: &str = "events.partitions";
pub const RETENTION_KIND: &str = "events.retention";

/// Positions per partition.
pub const PARTITION_SIZE: i64 = 10_000_000;
/// Signals (live saves, read marks) are for live updates only.
pub const SIGNAL_DAYS: i32 = 7;
/// Facts, until H1 makes it a plan setting.
pub const FACT_DAYS: i32 = 400;
/// Rows deleted per statement, so retention never holds a long lock.
const DELETE_BATCH: i64 = 10_000;

/// The upper bound of the newest range partition (exclusive).
async fn newest_bound(pool: &PgPool) -> Result<i64, AppError> {
    let bounds = sqlx::query_scalar::<_, String>(
        r#"
        SELECT pg_get_expr(child.relpartbound, child.oid)
        FROM pg_inherits
        JOIN pg_class child ON child.oid = pg_inherits.inhrelid
        JOIN pg_class parent ON parent.oid = pg_inherits.inhparent
        WHERE parent.relname = 'events'
          AND parent.relnamespace = to_regnamespace(current_schema())
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(bounds
        .iter()
        .filter_map(|bound| {
            // FOR VALUES FROM ('1') TO ('10000001'), or DEFAULT.
            let to = bound.split("TO (").nth(1)?;
            to.trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<i64>()
                .ok()
        })
        .max()
        .unwrap_or(1))
}

/// Add the next partition when the newest is 80% used. Returns whether it
/// added one.
pub async fn ensure_partitions(pool: &PgPool) -> Result<bool, AppError> {
    let upper = newest_bound(pool).await?;
    let next = sqlx::query_scalar::<_, i64>("SELECT next_position FROM event_position_counter")
        .fetch_one(pool)
        .await?;
    let lower = upper - PARTITION_SIZE;
    if next < lower + PARTITION_SIZE * 8 / 10 {
        return Ok(false);
    }
    add_partition(pool, upper).await?;
    Ok(true)
}

/// The partition for `[lower, lower + PARTITION_SIZE)`, taking over any
/// rows the default partition holds in that range.
pub async fn add_partition(pool: &PgPool, lower: i64) -> Result<(), AppError> {
    let upper = lower + PARTITION_SIZE;
    let name = format!("events_p{}", lower / PARTITION_SIZE);
    let mut transaction = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("CREATE TEMP TABLE events_moving (LIKE events) ON COMMIT DROP")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "WITH moved AS (DELETE FROM events_default WHERE position >= $1 AND position < $2 RETURNING *) \
         INSERT INTO events_moving SELECT * FROM moved",
    )
    .bind(lower)
    .bind(upper)
    .execute(&mut *transaction)
    .await?;
    // Bounds and the name are numbers this function made, never input.
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {name} PARTITION OF events FOR VALUES FROM ({lower}) TO ({upper})"
    ))
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO events SELECT * FROM events_moving")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

/// Delete what retention lets go: signals after 7 days, facts after 400.
/// Returns how many rows went.
pub async fn apply_retention(pool: &PgPool) -> Result<u64, AppError> {
    let mut deleted = 0;
    for (class, days) in [("signal", SIGNAL_DAYS), ("fact", FACT_DAYS)] {
        loop {
            let gone = sqlx::query(
                "DELETE FROM events WHERE position IN ( \
                     SELECT position FROM events \
                     WHERE class = $1 AND created_at < now() - make_interval(days => $2) \
                     ORDER BY position LIMIT $3)",
            )
            .bind(class)
            .bind(days)
            .bind(DELETE_BATCH)
            .execute(pool)
            .await?
            .rows_affected();
            deleted += gone;
            if gone < DELETE_BATCH as u64 {
                break;
            }
        }
    }
    Ok(deleted)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(PARTITIONS_KIND, |context| async move {
        ensure_partitions(&context.state.pool).await?;
        Ok(())
    });
    registry.every(PARTITIONS_KIND, Duration::from_secs(60 * 60));
    registry.register(RETENTION_KIND, |context| async move {
        apply_retention(&context.state.pool).await?;
        Ok(())
    });
    registry.every(RETENTION_KIND, Duration::from_secs(6 * 60 * 60));
}
