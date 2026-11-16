//! The link store (K3 3.2): one `links` table for every link between two
//! things. K3.12 fills it: triggers mirror each of today's link stores into
//! it, and `links.backfill` copies what they held once the registry backfill
//! has given every end a `things` row. Nothing reads it yet (K3.14).

use sqlx::PgPool;

use crate::{
    error::AppError,
    jobs::{JobRegistry, NewJob},
};

pub const BACKFILL_KIND: &str = "links.backfill";

/// Copy every old store into `links`, skipping what is there; how many
/// links were added. Safe to run again.
pub async fn copy_all(pool: &PgPool) -> Result<i64, AppError> {
    Ok(crate::db::scoped_one(pool, async |connection| {
        sqlx::query_scalar::<_, i64>("SELECT links_copy_all()")
            .fetch_one(&mut *connection)
            .await
    })
    .await?)
}

/// The copy, queued once (the registry backfill queues it when it ends).
pub fn backfill_job() -> NewJob {
    NewJob::new(BACKFILL_KIND).unique(BACKFILL_KIND)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(BACKFILL_KIND, |context| async move {
        let added = copy_all(&context.state.pool).await?;
        tracing::info!(added, "links copied from the old stores");
        Ok(())
    });
}
