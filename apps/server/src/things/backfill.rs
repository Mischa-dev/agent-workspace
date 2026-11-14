//! `things.backfill`: copies rows written before the registry existed into
//! `things` (K3 4.1 step 2). The triggers exist first, so rows written since
//! are already there. Each source is copied in id order, a batch per
//! transaction, and `things_backfill` remembers the last id, so a stopped
//! run resumes where it was. When every source is done, parents and access
//! parents the copy couldn't set yet (their own row came later) are set.
//!
//! A recurring job: once everything is done, a run is one cheap query.

use std::time::{Duration, Instant};

use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::AppError, jobs::JobRegistry};

pub const KIND: &str = "things.backfill";

/// The sources in the order they're copied: containers before what they
/// hold, and tasks before records, so a task's record is a `task` at once.
pub const SOURCES: &[&str] = &[
    "projects",
    "conversations",
    "collections",
    "project_pages",
    "project_decisions",
    "whiteboards",
    "sheets",
    "work_items",
    "collection_records",
];

const BATCH: i64 = 5_000;

/// How far one call got.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub copied: u64,
    pub done: bool,
}

/// Copy up to `max_batches` batches of `batch` rows, then return; `done` once
/// every source is copied and the parents are set.
pub async fn run(pool: &PgPool, batch: i64, max_batches: usize) -> Result<Progress, AppError> {
    let mut progress = Progress::default();
    let mut batches = 0;
    for source in SOURCES {
        loop {
            if batches >= max_batches {
                return Ok(progress);
            }
            let Some(copied) = copy_batch(pool, source, batch).await? else {
                break;
            };
            batches += 1;
            progress.copied += copied;
        }
    }
    set_parents(pool).await?;
    progress.done = true;
    Ok(progress)
}

/// One batch of `table`. `None` when the source was already done.
async fn copy_batch(pool: &PgPool, table: &str, batch: i64) -> Result<Option<u64>, AppError> {
    let source = format!("thing_source_{table}");
    let mut transaction = pool.begin().await?;
    sqlx::query("INSERT INTO things_backfill (source) VALUES ($1) ON CONFLICT (source) DO NOTHING")
        .bind(&source)
        .execute(&mut *transaction)
        .await?;
    let (last_id, done) = sqlx::query_as::<_, (Option<Uuid>, bool)>(
        "SELECT last_id, done_at IS NOT NULL FROM things_backfill WHERE source = $1 FOR UPDATE",
    )
    .bind(&source)
    .fetch_one(&mut *transaction)
    .await?;
    if done {
        return Ok(None);
    }
    // The table name comes from SOURCES, never from input.
    let ids = sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT id FROM {table} WHERE $1::uuid IS NULL OR id > $1 ORDER BY id LIMIT $2"
    ))
    .bind(last_id)
    .bind(batch)
    .fetch_all(&mut *transaction)
    .await?;
    sqlx::query("SELECT things_upsert_from($1, $2)")
        .bind(&source)
        .bind(&ids)
        .execute(&mut *transaction)
        .await?;
    let finished = i64::try_from(ids.len()).unwrap_or(i64::MAX) < batch;
    sqlx::query(
        "UPDATE things_backfill SET last_id = COALESCE($2, last_id), \
         done_at = CASE WHEN $3 THEN now() END, updated_at = now() WHERE source = $1",
    )
    .bind(&source)
    .bind(ids.last())
    .bind(finished)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(ids.len() as u64))
}

/// Parents and access parents whose own rows came after them.
async fn set_parents(pool: &PgPool) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    for table in [
        "conversations",
        "project_decisions",
        "work_items",
        "collection_records",
    ] {
        sqlx::query(&format!(
            r#"
            UPDATE things thing
            SET parent_id = source.parent_id, access_parent_id = source.access_parent_id
            FROM thing_source_{table} source
            WHERE source.id = thing.id AND source.kind = thing.kind
              AND (thing.parent_id IS DISTINCT FROM source.parent_id
                   OR thing.access_parent_id IS DISTINCT FROM source.access_parent_id)
            "#
        ))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Whether every source has been copied.
pub async fn is_done(pool: &PgPool) -> Result<bool, AppError> {
    let done = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM things_backfill WHERE done_at IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(usize::try_from(done).unwrap_or(0) == SOURCES.len())
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(KIND, |context| async move {
        if is_done(&context.state.pool).await? {
            return Ok(());
        }
        // A few minutes at a time; the next run carries on.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(240) {
            if run(&context.state.pool, BATCH, 10).await?.done {
                break;
            }
        }
        Ok(())
    });
    registry.every(KIND, Duration::from_secs(10 * 60));
}
