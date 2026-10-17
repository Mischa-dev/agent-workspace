use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use sqlx::PgPool;

use crate::error::AppError;

#[derive(Clone)]
pub struct HealthState {
    pub pool: PgPool,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
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

async fn ready(State(state): State<HealthState>) -> Result<Json<HealthResponse>, AppError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;

    Ok(Json(HealthResponse { status: "ready" }))
}
