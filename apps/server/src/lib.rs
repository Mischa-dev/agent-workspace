#![forbid(unsafe_code)]

pub mod account;
pub mod account_security;
pub mod agent_docs;
pub mod agent_parity;
pub mod agent_profiles;
pub mod agent_sessions;
pub mod agent_setup_links;
pub mod ai;
pub mod api;
pub mod audit_log;
pub mod auth;
pub mod automations;
pub mod bootstrap;
pub mod bug_reports;
pub mod code_host;
pub mod collection_forms;
pub mod collections;
pub mod config;
pub mod connected_apps;
pub mod conversations;
pub mod direct;
pub mod domain;
pub mod environments;
pub mod error;
pub mod events;
pub mod execution;
pub mod execution_setup;
pub mod external_agents;
pub mod external_events;
pub mod external_execution;
pub mod fields;
pub mod files;
pub mod forwarding;
pub mod git_runtime;
pub mod github;
pub mod harness_host;
pub mod harnesses;
pub mod health;
pub mod home;
pub mod idempotency;
pub mod invitations;
pub mod jobs;
pub mod later;
pub mod launch;
pub mod layouts;
pub mod link_previews;
pub mod mail;
pub mod mcp;
pub mod memberships;
pub mod memory;
pub mod message_content;
pub mod message_edits;
pub mod metrics;
pub mod migration_preflight;
pub mod modules;
pub mod notification_email;
pub mod notification_settings;
pub mod notifications;
pub mod onboarding;
pub mod outbound;
pub mod outbox;
pub mod page_blocks;
pub mod page_docs;
pub mod password_reset;
pub mod policy;
pub mod policy_inventory;
pub mod preflight;
pub mod profile_runtime;
pub mod projects;
pub mod provider;
pub mod public_api;
pub mod push;
pub mod questions;
pub mod rate_limit;
pub mod reactions;
pub mod realtime;
pub mod recovery;
pub mod replies;
pub mod repositories;
pub mod requests;
pub mod retention;
pub mod roles;
pub mod run_log;
pub mod run_views;
pub mod runs;
pub mod scheduled_messages;
pub mod scheduler;
pub mod search;
pub mod secrets;
pub mod seed;
pub mod server_status;
pub mod settings;
pub mod sheets;
pub mod storage_admission;
pub mod telemetry;
pub mod test_support;
pub mod usage;
pub mod webhooks;
pub mod whiteboards;
pub mod work_item_commands;
pub mod work_item_fields;
pub mod work_item_history;
pub mod work_item_labels;
pub mod work_item_move;
pub mod work_item_planning;
pub mod work_item_workflow;
pub mod work_items;
pub mod workspace;
pub mod workspace_space;
pub mod workspace_templates;

use std::sync::Arc;

use axum::{Router, extract::DefaultBodyLimit, routing::any};
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
pub const MAX_APPLICATION_BODY_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub pool: PgPool,
    pub rate_limits: RateLimiters,
    pub realtime: RealtimeHub,
    pub supervisors: provider::SupervisorRegistry,
}

pub fn router(state: AppState) -> Router {
    state
        .supervisors
        .set_host(Arc::new(harness_host::ServerHost::new(
            state.pool.clone(),
            Arc::clone(&state.config),
        )));
    let request_id = REQUEST_ID_HEADER.clone();
    let web_root = state.config.web_root.clone();
    let index = web_root.join("index.html");

    Router::new()
        .merge(health::router(HealthState {
            pool: state.pool.clone(),
        }))
        .merge(api::router())
        .merge(account::router())
        .merge(account_security::router())
        .merge(password_reset::router())
        .merge(conversations::router())
        .merge(direct::router())
        .merge(replies::router())
        .merge(message_edits::router())
        .merge(reactions::router())
        .merge(files::http::router())
        .merge(link_previews::router())
        .merge(notifications::router())
        .merge(usage::router())
        .merge(home::router())
        .merge(push::router())
        .merge(notification_settings::router())
        .merge(later::router())
        .merge(questions::router())
        .merge(scheduled_messages::router())
        .merge(forwarding::router())
        .merge(mail::http::router())
        .merge(external_agents::router())
        .merge(repositories::router())
        .merge(environments::router())
        .merge(harnesses::router())
        .merge(launch::router())
        .merge(run_views::router())
        .merge(agent_profiles::router())
        .merge(agent_sessions::router())
        .merge(github::router())
        .merge(invitations::router())
        .merge(memberships::router())
        .merge(workspace::router())
        .merge(roles::router())
        .merge(workspace_templates::router())
        .merge(server_status::router())
        .merge(onboarding::router())
        .merge(audit_log::router())
        .merge(work_items::router())
        .merge(work_items::comments::router())
        .merge(ai::http::router())
        .merge(memory::router())
        .merge(whiteboards::router())
        .merge(collections::http::router())
        .merge(sheets::http::router())
        .merge(search::router())
        .merge(connected_apps::router())
        .merge(agent_docs::router())
        .merge(agent_setup_links::router())
        .merge(settings::router())
        .merge(layouts::router())
        .merge(modules::router())
        .merge(secrets::router())
        .merge(metrics::router())
        .merge(mcp::oauth_router())
        .merge(mcp::router(state.clone()))
        .merge(public_api::router())
        .merge(webhooks::router())
        .merge(automations::router())
        .merge(bug_reports::router())
        .route("/api/v1", any(api_not_found))
        .route("/api/v1/{*path}", any(api_not_found))
        // A module's routes answer "turned off" where it's off (AWD-97).
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            modules::gate::refuse_off_modules,
        ))
        .fallback_service(
            ServeDir::new(web_root)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(index)),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_APPLICATION_BODY_BYTES))
        .layer(axum::middleware::from_fn(metrics::track_requests))
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
                    path = %logged_path(request),
                    request_id = request_id,
                )
            }),
        )
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        // worker-src allows only the app's own service worker (/sw.js), which
        // makes it installable and caches the built app shell, never API data.
        // img-src allows blob: for images the page makes itself: a whiteboard
        // shrinks a dropped photo through one before saving it.
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; font-src 'self'; worker-src 'self'",
            ),
        ))
        // Browsers only honor this over HTTPS, so it is harmless on loopback.
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000"),
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

/// The path a request's log lines carry: the route's pattern, such as
/// `/api/v1/bug-reports/{token}`, so a secret in a path (a bug report link's
/// token) never reaches the logs. Static files have no route and no secrets,
/// so they log their own path.
fn logged_path<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or_else(|| request.uri().path(), axum::extract::MatchedPath::as_str)
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

    use super::{AppState, MAX_APPLICATION_BODY_BYTES, router};
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
        assert!(csp.contains("worker-src 'self'"));
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
            .clone()
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

        let oversized = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b'x'; MAX_APPLICATION_BODY_BYTES + 1]))
                    .expect("oversized request"),
            )
            .await
            .expect("oversized response");
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let oversized_body = to_bytes(oversized.into_body(), 16_384)
            .await
            .expect("oversized JSON body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&oversized_body)
                .expect("JSON error envelope")["error"]["code"],
            "payload_too_large"
        );
    }
}
