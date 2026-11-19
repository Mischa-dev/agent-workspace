//! The tick (K5.18): `POST /internal/work/tick` runs one bounded round of
//! a `work` instance's queue work, so an outside timer can drive it with
//! `AGENT_WORKSPACE_WORK_LOOP=off` and nothing has to stay awake (ruling
//! R24 rule 4). A round gives the log's staged events their positions and
//! runs its consumers, expires lapsed reply claims and delivers agents'
//! wake-ups, runs GitHub's jobs, then the due jobs (scheduled messages,
//! retention and the rest), for at most 25 seconds. It answers with what
//! it did and whether more is due; the timer calls again then.
//!
//! The bearer token is read from `AGENT_WORKSPACE_WORK_TICK_SECRET_PATH`
//! on each call and compared in constant time; without the file, or with
//! the wrong token, the route answers as if it weren't there. It is never
//! part of the public API.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Serialize;
use subtle::ConstantTimeEq;

use crate::{AppState, error::AppError, jobs::JobRegistry};

/// The longest one tick works.
pub const BUDGET: Duration = Duration::from_secs(25);

/// What one tick did.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticked {
    /// Events the log's consumers handled.
    pub events: usize,
    /// Jobs run.
    pub jobs: usize,
    /// More was due when the round stopped: tick again.
    pub more: bool,
    pub elapsed_ms: u64,
}

/// One round of work, for at most `budget`.
pub async fn round(
    state: &AppState,
    registry: &JobRegistry,
    budget: Duration,
) -> Result<Ticked, AppError> {
    let started = Instant::now();
    let deadline = started + budget;
    let log = crate::events::log_round(&state.pool, Some(deadline)).await?;
    crate::replies::round(state).await;
    if let Err(error) = crate::github::drain_jobs(state).await {
        tracing::warn!(
            error_category = error.log_category(),
            "GitHub job pass failed"
        );
    }
    let queue =
        crate::jobs::run_round(state, registry, state.config.job_concurrency, deadline).await?;
    // What the jobs wrote reaches the log's consumers in the same tick.
    let after = crate::events::log_round(&state.pool, Some(deadline)).await?;
    Ok(Ticked {
        events: log.handled + after.handled,
        jobs: queue.ran,
        more: after.more || queue.more,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// The tick's route, on a `work` instance.
pub fn router(state: AppState, registry: Arc<JobRegistry>) -> Router {
    Router::new()
        .route("/internal/work/tick", post(tick))
        .with_state((state, registry))
}

async fn tick(
    State((state, registry)): State<(AppState, Arc<JobRegistry>)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return AppError::NotFound("route").into_response();
    }
    match round(&state, &registry, BUDGET).await {
        Ok(ticked) => Json(ticked).into_response(),
        Err(error) => error.into_response(),
    }
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(path) = &state.config.work_tick_secret_path else {
        return false;
    };
    let Ok(expected) = std::fs::read_to_string(path) else {
        return false;
    };
    let expected = expected.trim();
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    !expected.is_empty() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}
