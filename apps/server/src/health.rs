use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{cluster::Roles, error::AppError, realtime::tail};

#[derive(Clone)]
pub struct HealthState {
    pub pool: PgPool,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// How far behind the head the tail may be while the instance says it is
/// ready, where it feeds this instance's sockets.
pub const READY_TAIL_LAG: Duration = Duration::from_secs(10);

/// This process as its health checks see it (K5.7): which instance, which
/// roles, whether it is draining, and how far its tail is behind. The
/// server sets it on its routes; a router built without a server (tests of
/// the routes alone) answers readiness from the database only.
#[derive(Clone)]
pub struct Process {
    inner: Arc<ProcessInner>,
}

struct ProcessInner {
    instance: Uuid,
    roles: Roles,
    draining: AtomicBool,
    /// The tail feeding this instance's sockets, where it has one.
    tail: Option<tail::Progress>,
}

impl Process {
    pub fn new(instance: Uuid, roles: Roles, tail: Option<tail::Progress>) -> Self {
        Self {
            inner: Arc::new(ProcessInner {
                instance,
                roles,
                draining: AtomicBool::new(false),
                tail,
            }),
        }
    }

    /// From now on readiness answers 503, so the balancer moves off.
    pub fn drain(&self) {
        self.inner.draining.store(true, Ordering::SeqCst);
    }

    pub fn is_draining(&self) -> bool {
        self.inner.draining.load(Ordering::SeqCst)
    }

    fn tail_lag(&self) -> Option<Duration> {
        self.inner.tail.as_ref().map(tail::Progress::behind)
    }
}

pub fn router<S>(state: HealthState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/health/version", get(version))
        .with_state(state)
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
    commit: &'static str,
}

/// Which build is running, so an operator can tell what a deploy left live.
/// `deploy/redhat/deploy-release.sh` stamps the commit; other builds say "dev".
pub const BUILD_COMMIT: &str = match option_env!("AGENT_WORKSPACE_BUILD_COMMIT") {
    Some(commit) => commit,
    None => "dev",
};

async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION"),
        commit: BUILD_COMMIT,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    /// `ready`, or why not: `draining`, `schema_behind`, `tail_behind`.
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    roles: Option<Vec<String>>,
    draining: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tail_lag_ms: Option<u64>,
}

/// 200 only when the database answers, its schema is at least the version
/// this release expects, the instance isn't draining, and the tail feeding
/// its sockets is less than `READY_TAIL_LAG` behind. A wake listener that is
/// down doesn't matter: the polls keep everything correct.
async fn ready(
    State(state): State<HealthState>,
    process: Option<Extension<Process>>,
) -> Result<Response, AppError> {
    let applied = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(version) FROM _sqlx_migrations WHERE success",
    )
    .fetch_one(crate::db::scoped(&state.pool))
    .await?
    .unwrap_or(0);
    let process = process.map(|Extension(process)| process);
    let draining = process.as_ref().is_some_and(Process::is_draining);
    let tail_lag = process.as_ref().and_then(Process::tail_lag);
    let status = readiness(
        draining,
        applied,
        crate::preflight::expected_schema_version(),
        tail_lag,
    );
    let body = Readiness {
        status,
        instance: process.as_ref().map(|process| process.inner.instance),
        roles: process.as_ref().map(|process| process.inner.roles.names()),
        draining,
        tail_lag_ms: tail_lag.map(|lag| u64::try_from(lag.as_millis()).unwrap_or(u64::MAX)),
    };
    let code = if status == "ready" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    Ok((code, Json(body)).into_response())
}

fn readiness(
    draining: bool,
    applied: i64,
    expected: i64,
    tail_lag: Option<Duration>,
) -> &'static str {
    if draining {
        "draining"
    } else if applied < expected {
        "schema_behind"
    } else if tail_lag.is_some_and(|lag| lag >= READY_TAIL_LAG) {
        "tail_behind"
    } else {
        "ready"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_only_when_nothing_holds_it_back() {
        assert_eq!(readiness(false, 5, 5, None), "ready");
        assert_eq!(
            readiness(false, 6, 5, Some(Duration::from_secs(1))),
            "ready"
        );
        assert_eq!(readiness(true, 5, 5, None), "draining");
        assert_eq!(readiness(false, 4, 5, None), "schema_behind");
        assert_eq!(readiness(false, 5, 5, Some(READY_TAIL_LAG)), "tail_behind");
    }

    #[test]
    fn draining_is_seen_at_once() {
        let process = Process::new(Uuid::now_v7(), Roles::ALL, None);
        assert!(!process.is_draining());
        process.drain();
        assert!(process.is_draining());
        assert_eq!(process.tail_lag(), None);
    }
}
