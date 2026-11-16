use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::watch;

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

/// Ready once the database answers and holds no migration newer than this
/// build knows (an old image started against a newer schema), H1.13.
async fn ready(State(state): State<HealthState>) -> Response {
    let applied = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(version) FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(connection)
        .await
    })
    .await;
    match applied {
        Ok(applied) if applied.unwrap_or(0) > newest_known_migration() => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "outdated", "step": "the database is newer than this build"})),
        )
            .into_response(),
        Ok(_) => Json(HealthResponse { status: "ready" }).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "unavailable", "step": "database"})),
        )
            .into_response(),
    }
}

/// The newest migration this build carries.
fn newest_known_migration() -> i64 {
    sqlx::migrate!()
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

/// What answers on the server's address while it starts (H1.13): live,
/// and not ready, saying which step it's on (`migrating`, then
/// `starting`); every other path is 503 too, so nothing reaches a database
/// that is still changing.
pub fn startup_router(step: watch::Receiver<&'static str>) -> Router {
    let starting = move || {
        let step = *step.borrow();
        async move {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::RETRY_AFTER, "5")],
                Json(json!({"status": "starting", "step": step})),
            )
        }
    };
    Router::new()
        .route("/health/live", get(live))
        .route("/health/version", get(version))
        .route("/health/ready", get(starting.clone()))
        .fallback(starting)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn a_starting_server_is_live_but_not_ready() {
        let (step, receiver) = watch::channel("migrating");
        let app = startup_router(receiver);
        let get = |path: &'static str| {
            app.clone()
                .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        };
        assert_eq!(
            get("/health/live").await.expect("live").status(),
            StatusCode::OK
        );
        let ready = get("/health/ready").await.expect("ready");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(ready.into_body(), 1024)
            .await
            .expect("body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("json"),
            json!({"status": "starting", "step": "migrating"})
        );
        step.send("starting").expect("step");
        let other = get("/api/v1/auth/session").await.expect("other");
        assert_eq!(other.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(other.into_body(), 1024)
            .await
            .expect("body");
        assert!(String::from_utf8_lossy(&body).contains("\"starting\""));
    }

    #[test]
    fn this_build_knows_its_newest_migration() {
        assert!(newest_known_migration() >= 20_261_116_203_000);
    }
}
