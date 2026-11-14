//! A1.3: liveness, how recently a run that reports for itself was heard
//! from (A1 3.2). Kept apart from its state: a quiet run is still working
//! as far as anyone knows. A run silent for its report interval is `quiet`;
//! for three intervals, `not_responding`. Any report sets it back to `ok`.
//! Built-in runs are watched by their supervisor instead.

use std::time::Duration;

use sqlx::PgPool;

use super::{
    RunState,
    ops::emit,
    store::{RUN_COLUMNS, RunRow},
};
use crate::{
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry},
};

pub const SWEEP_KIND: &str = "runs.liveness_sweep";
/// The most runs one sweep changes; the next sweep picks up the rest.
const SWEEP_BATCH: i64 = 500;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(SWEEP_KIND, Duration::from_secs(120), |context: JobContext| async move {
        sweep(&context.state.pool)
            .await
            .map_err(|error| JobError::retry(error.log_category()))?;
        Ok(())
    });
    registry.every(SWEEP_KIND, Duration::from_secs(60));
}

/// Move every run whose liveness no longer matches its silence, and say so.
/// Returns how many changed.
pub async fn sweep(pool: &PgPool) -> Result<usize, AppError> {
    let mut transaction = pool.begin().await?;
    let changed = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        WITH due AS (
            SELECT candidate.id, computed.next
            FROM runs candidate
            CROSS JOIN LATERAL (
                SELECT CASE
                         WHEN now() - COALESCE(candidate.last_report_at, candidate.created_at)
                              >= make_interval(secs => candidate.report_interval_seconds * 3)
                           THEN 'not_responding'
                         WHEN now() - COALESCE(candidate.last_report_at, candidate.created_at)
                              >= make_interval(secs => candidate.report_interval_seconds)
                           THEN 'quiet'
                         ELSE 'ok'
                       END AS next
            ) computed
            WHERE candidate.state = ANY($1)
              AND computed.next <> candidate.liveness
              AND NOT EXISTS (SELECT 1 FROM agent_runs builtin WHERE builtin.id = candidate.id)
            ORDER BY candidate.last_report_at NULLS FIRST
            LIMIT $2
            FOR UPDATE OF candidate SKIP LOCKED
        )
        UPDATE runs run
           SET liveness = due.next, version = run.version + 1, updated_at = now()
          FROM due
         WHERE run.id = due.id
        RETURNING {RUN_COLUMNS}
        "#
    ))
    .bind(
        RunState::ALL
            .into_iter()
            .filter(|state| state.expects_reports())
            .map(RunState::as_str)
            .collect::<Vec<_>>(),
    )
    .bind(SWEEP_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    for run in &changed {
        emit(&mut transaction, run, "liveness_changed").await?;
    }
    transaction.commit().await?;
    Ok(changed.len())
}
