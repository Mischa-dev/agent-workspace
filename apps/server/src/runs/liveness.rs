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
    registry.register_with_timeout(
        SWEEP_KIND,
        Duration::from_secs(120),
        |context: JobContext| async move {
            sweep(&context.state.pool)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            settle(&context.state.pool)
                .await
                .map_err(|error| JobError::retry(error.log_category()))?;
            Ok(())
        },
    );
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

/// How long a stopped run's agent has to confirm before the run ends
/// cancelled anyway: a runner that only uses the reply tools never does.
pub const STOP_GRACE_SECONDS: i64 = 600;

/// End the runs nothing else will end (A1.4):
///
/// - a launched run whose request ended without the runner saying so on the
///   run: answered (done), passed or withdrawn (cancelled), stopped
///   (cancelled), released or its lease ran out (failed, with the reason);
/// - a run asked to stop whose agent didn't confirm within
///   [`STOP_GRACE_SECONDS`] (cancelled).
///
/// Returns how many ended.
pub async fn settle(pool: &PgPool) -> Result<usize, AppError> {
    let mut transaction = pool.begin().await?;
    let ended_requests = sqlx::query_as::<_, (uuid::Uuid, String, Option<String>)>(
        r#"
        SELECT run.id, latest.status, latest.reason
        FROM runs run
        CROSS JOIN LATERAL (
            SELECT request.status, request.reason
            FROM reply_requests request
            WHERE request.message_id = run.cause_message_id
              AND request.agent_actor_id = run.account_id
            ORDER BY request.updated_at DESC
            LIMIT 1
        ) latest
        WHERE run.launch_id IS NOT NULL AND run.state = ANY($1)
          AND NOT EXISTS (SELECT 1 FROM agent_runs builtin WHERE builtin.id = run.id)
          AND latest.status IN ('replied', 'passed', 'released', 'expired', 'superseded', 'cancelled')
        ORDER BY run.id
        LIMIT $2
        FOR UPDATE OF run SKIP LOCKED
        "#,
    )
    .bind(RunState::active_keys())
    .bind(SWEEP_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    let mut ended = 0;
    for (run_id, status, reason) in ended_requests {
        let (next, line) = match status.as_str() {
            "replied" => (RunState::Done, "Answered.".to_owned()),
            "passed" => (RunState::Cancelled, "Passed.".to_owned()),
            "superseded" => (
                RunState::Cancelled,
                "The request it answered was withdrawn.".to_owned(),
            ),
            "cancelled" => (RunState::Cancelled, "Stopped.".to_owned()),
            "released" => (
                RunState::Failed,
                reason.unwrap_or_else(|| "Released.".to_owned()),
            ),
            _ => (
                RunState::Failed,
                reason
                    .unwrap_or_else(|| "The agent stopped checking in before replying.".to_owned()),
            ),
        };
        if super::ops::end_by_service(&mut transaction, run_id, next, &line)
            .await?
            .is_some()
        {
            ended += 1;
        }
    }
    let unconfirmed = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"
        SELECT run.id FROM runs run
        WHERE run.state = $1
          AND run.cancel_requested_at < now() - make_interval(secs => $2)
          AND NOT EXISTS (SELECT 1 FROM agent_runs builtin WHERE builtin.id = run.id)
        ORDER BY run.cancel_requested_at
        LIMIT $3
        FOR UPDATE OF run SKIP LOCKED
        "#,
    )
    .bind(RunState::CancelRequested.as_str())
    .bind(STOP_GRACE_SECONDS as f64)
    .bind(SWEEP_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    for run_id in unconfirmed {
        if super::ops::end_by_service(
            &mut transaction,
            run_id,
            RunState::Cancelled,
            "Stopped; the agent didn't confirm in time.",
        )
        .await?
        .is_some()
        {
            ended += 1;
        }
    }
    transaction.commit().await?;
    Ok(ended)
}

/// A host's heartbeat listed these runs: those that are the host's own and
/// still expect reports are heard from now (A1.11). Returns how many.
pub async fn renew_for_host(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    host_id: uuid::Uuid,
    run_ids: &[uuid::Uuid],
) -> Result<u64, AppError> {
    if run_ids.is_empty() {
        return Ok(0);
    }
    #[derive(sqlx::FromRow)]
    struct Renewed {
        #[sqlx(flatten)]
        run: RunRow,
        was: String,
    }
    let renewed = sqlx::query_as::<_, Renewed>(&format!(
        r#"
        WITH listed AS (
            SELECT candidate.id, candidate.liveness AS was
            FROM runs candidate
            WHERE candidate.id = ANY($1) AND candidate.host_id = $2 AND candidate.state = ANY($3)
            FOR UPDATE
        )
        UPDATE runs run
           SET last_report_at = now(),
               liveness = 'ok',
               version = CASE WHEN listed.was <> 'ok' THEN run.version + 1 ELSE run.version END,
               updated_at = now()
          FROM listed
         WHERE run.id = listed.id
        RETURNING {RUN_COLUMNS}, listed.was
        "#
    ))
    .bind(run_ids)
    .bind(host_id)
    .bind(
        RunState::ALL
            .into_iter()
            .filter(|state| state.expects_reports())
            .map(RunState::as_str)
            .collect::<Vec<_>>(),
    )
    .fetch_all(&mut **transaction)
    .await?;
    for renewed in &renewed {
        if renewed.was != "ok" {
            emit(transaction, &renewed.run, "liveness_changed").await?;
        }
    }
    Ok(renewed.len() as u64)
}
