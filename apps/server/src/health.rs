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
        .with_state(state)
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(state): State<HealthState>) -> Result<Json<HealthResponse>, AppError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;

    Ok(Json(HealthResponse { status: "ready" }))
}
