//! The workspace column backfill (H1.4 to H1.7, H1 3.3).
//!
//! A table given its `workspace_id` column by `tenancy_expand` (migration
//! `20261116183000`) gets a `tenancy.backfill` job when it already had rows.
//! The job fills the column a few pages at a time through the database's
//! `tenancy_backfill`, which takes it from the row's project or parent the
//! same way the table's fill trigger does, and queues the next batch with the
//! page it got to, so a stopped job resumes where it left off. The switch
//! migration finishes whatever the job hasn't reached, so it never waits for
//! one.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    db,
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    tenancy::tables::{TENANT_TABLES, WORKSPACE_ID},
};

/// Fills a table's workspace column, a batch at a time.
pub const BACKFILL_KIND: &str = "tenancy.backfill";
/// Pages per batch: 64 pages of 8 kB, a few thousand rows.
const BACKFILL_PAGES: i32 = 64;

/// Where a table's backfill got to.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillPayload {
    pub table: String,
    #[serde(default)]
    pub after_page: i64,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        BACKFILL_KIND,
        Duration::from_secs(300),
        |context: JobContext| async move {
            let payload: BackfillPayload = context.job.payload_as()?;
            let Some(table) = backfilled_table(&payload.table) else {
                return Err(JobError::permanent(
                    "The table has no workspace column to fill.",
                ));
            };
            backfill_batch(&context.state.pool, table, payload.after_page)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
}

/// The listed table a payload names: only a tenant table whose column is
/// `workspace_id` (the ones H1.4 to H1.7 add) is backfilled.
fn backfilled_table(name: &str) -> Option<&'static str> {
    TENANT_TABLES
        .iter()
        .find(|(table, column)| *table == name && *column == WORKSPACE_ID)
        .map(|(table, _)| *table)
}

/// Fills one batch of `table` from `from_page`, and queues the next batch
/// when there is more. Returns the page the next batch starts at, or None
/// when the table is done. Safe to run twice: filled rows are skipped.
pub async fn backfill_batch(
    pool: &sqlx::PgPool,
    table: &'static str,
    from_page: i64,
) -> Result<Option<i64>, AppError> {
    let mut transaction = db::scoped_tx(pool).await?;
    let required = sqlx::query_scalar::<_, bool>(
        "SELECT attnotnull FROM pg_attribute WHERE attrelid = $1::regclass AND attname = 'workspace_id'",
    )
    .bind(table)
    .fetch_optional(&mut *transaction)
    .await?
    .unwrap_or(true);
    if required {
        // The switch migration already finished it.
        transaction.commit().await?;
        return Ok(None);
    }
    let next =
        sqlx::query_scalar::<_, Option<i64>>("SELECT tenancy_backfill($1::regclass, $2, $3)")
            .bind(table)
            .bind(from_page)
            .bind(BACKFILL_PAGES)
            .fetch_one(&mut *transaction)
            .await?;
    if let Some(next) = next {
        crate::jobs::enqueue(
            &mut *transaction,
            NewJob::new(BACKFILL_KIND)
                .payload(BackfillPayload {
                    table: table.to_owned(),
                    after_page: next,
                })
                .unique(format!("{BACKFILL_KIND}:{table}:{next}"))
                .max_attempts(50),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tables_given_the_column_here_are_backfilled() {
        assert_eq!(backfilled_table("messages"), Some("messages"));
        assert_eq!(backfilled_table("projects"), None, "organization_id");
        assert_eq!(backfilled_table("messages; DROP TABLE x"), None);
    }
}
