#![forbid(unsafe_code)]

pub mod account;
pub mod api;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod conversations;
pub mod domain;
pub mod error;
pub mod execution;
pub mod git_runtime;
pub mod github;
pub mod health;
pub mod idempotency;
pub mod mcp;
pub mod message_content;
pub mod migration_preflight;
pub mod outbox;
pub mod policy;
pub mod profile_runtime;
pub mod projects;
pub mod provider;
pub mod rate_limit;
pub mod realtime;
pub mod recovery;
pub mod repositories;
pub mod scheduler;
pub mod seed;
pub mod telemetry;
pub mod work_items;

use std::sync::Arc;

use axum::{Router, routing::any};
use http::{HeaderName, HeaderValue, Request};
use sqlx::PgPool;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing::Span;

use crate::rate_limit::RateLimiters;
use crate::realtime::RealtimeHub;
use crate::{config::AppConfig, health::HealthState};

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub pool: PgPool,
    pub rate_limits: RateLimiters,
    pub realtime: RealtimeHub,
    pub supervisors: provider::SupervisorRegistry,
}

pub fn router(state: AppState) -> Router {
    let request_id = REQUEST_ID_HEADER.clone();
    let web_root = state.config.web_root.clone();
    let index = web_root.join("index.html");

    Router::new()
        .merge(health::router(HealthState {
            pool: state.pool.clone(),
        }))
        .merge(api::router())
        .merge(account::router())
        .merge(conversations::router())
        .merge(repositories::router())
        .merge(github::router())
        .merge(work_items::router())
        .merge(mcp::oauth_router())
        .merge(mcp::router(state.clone()))
        .route("/api/v1", any(api_not_found))
        .route("/api/v1/{*path}", any(api_not_found))
        .fallback_service(
            ServeDir::new(web_root)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(index)),
        )
        .with_state(state)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let request_id = request
                    .headers()
                    .get(&REQUEST_ID_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("missing");

                tracing::info_span!(
                    "http.request",
                    method = %request.method(),
                    uri = %request.uri(),
                    request_id = request_id,
                )
            }),
        )
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; worker-src 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        ))
}

async fn api_not_found() -> crate::error::AppError {
    crate::error::AppError::NotFound("api_route")
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    Span::current().record("shutdown", true);
    tracing::info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
    };
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::{AppState, router};
    use crate::{config::AppConfig, rate_limit::RateLimiters, realtime::RealtimeHub};

    #[tokio::test]
    async fn every_response_receives_the_restrictive_browser_headers() {
        let temporary_web_root = tempfile::tempdir().expect("temporary web root");
        let config = AppConfig::from_values(&BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/database".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
                temporary_web_root.path().to_string_lossy().into_owned(),
            ),
        ]))
        .expect("test config");
        let pool = PgPoolOptions::new()
            .connect_lazy(config.database_url.expose())
            .expect("lazy PostgreSQL pool");
        let supervisors =
            crate::provider::SupervisorRegistry::new(&config).expect("test supervisor registry");
        let application = router(AppState {
            config: Arc::new(config),
            pool,
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(8),
            supervisors,
        });

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");

        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        let csp = headers
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .expect("Content-Security-Policy");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        for (name, expected) in [
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
            ("cross-origin-opener-policy", "same-origin"),
            ("cross-origin-resource-policy", "same-origin"),
        ] {
            assert_eq!(
                headers.get(name).and_then(|value| value.to_str().ok()),
                Some(expected),
                "{name}"
            );
        }
        assert_eq!(
            headers
                .get("permissions-policy")
                .and_then(|value| value.to_str().ok()),
            Some("camera=(), microphone=(), geolocation=(), payment=(), usb=()")
        );
    }

    #[tokio::test]
    async fn unknown_api_routes_and_json_rejections_use_the_error_envelope() {
        let temporary_web_root = tempfile::tempdir().expect("temporary web root");
        let config = AppConfig::from_values(&BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/database".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
                temporary_web_root.path().to_string_lossy().into_owned(),
            ),
        ]))
        .expect("test config");
        let pool = PgPoolOptions::new()
            .connect_lazy(config.database_url.expose())
            .expect("lazy PostgreSQL pool");
        let supervisors =
            crate::provider::SupervisorRegistry::new(&config).expect("test supervisor registry");
        let application = router(AppState {
            config: Arc::new(config),
            pool,
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(8),
            supervisors,
        });

        let unknown = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/does-not-exist")
                    .body(Body::empty())
                    .expect("unknown API request"),
            )
            .await
            .expect("unknown API response");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            unknown
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let unknown_body = to_bytes(unknown.into_body(), 16_384)
            .await
            .expect("unknown API body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&unknown_body)
                .expect("JSON error envelope")["error"]["code"],
            "not_found"
        );

        let malformed = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .expect("malformed JSON request"),
            )
            .await
            .expect("malformed JSON response");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            malformed
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let malformed_body = to_bytes(malformed.into_body(), 16_384)
            .await
            .expect("malformed JSON body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&malformed_body)
                .expect("JSON error envelope")["error"]["code"],
            "invalid_request"
        );
    }
}
