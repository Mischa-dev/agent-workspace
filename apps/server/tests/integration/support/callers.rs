//! The kinds of caller that reach the app (a person in a browser, an agent,
//! an app a person connected) behind one `Caller` trait, so a test can run
//! the same operation as each and compare: "no difference by account kind".

use agent_workspace_server::{
    AppState,
    realtime::{outbox_worker_id, publish_ready_outbox_batch},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use super::app::{Browser, send, tool};

/// One operation, as a person (HTTP), an MCP tool, or the public API (the
/// same tools over plain HTTPS).
pub(crate) enum Call {
    Http {
        method: &'static str,
        path: String,
        body: Value,
    },
    Tool {
        name: &'static str,
        arguments: Value,
    },
    PublicApi {
        tool: &'static str,
        arguments: Value,
    },
}

pub(crate) trait Caller {
    /// The status and JSON answer. A tool call answers 200 with its
    /// structured result (`outcome`, `value`).
    async fn call(&self, app: &axum::Router, call: Call) -> (StatusCode, Value);
}

/// A person signed in through the browser.
pub(crate) struct Person {
    pub(crate) actor: Uuid,
    pub(crate) browser: Browser,
}

/// An agent: its own actor, its connection and bearer token, and an MCP
/// session opened with it.
pub(crate) struct Agent {
    pub(crate) actor: Uuid,
    pub(crate) connection: Uuid,
    pub(crate) token: String,
    pub(crate) session: String,
}

/// An app a person connected through OAuth: it acts as that person.
pub(crate) struct ConnectedApp {
    pub(crate) person: Uuid,
    pub(crate) token: String,
    pub(crate) session: String,
}

impl Caller for Person {
    async fn call(&self, app: &axum::Router, call: Call) -> (StatusCode, Value) {
        match call {
            Call::Http { method, path, body } => {
                send(app, self.browser.request(method, &path, None, body)).await
            }
            Call::Tool { .. } | Call::PublicApi { .. } => {
                panic!("a person calls tools through a ConnectedApp")
            }
        }
    }
}

impl Caller for Agent {
    async fn call(&self, app: &axum::Router, call: Call) -> (StatusCode, Value) {
        bearer_call(app, &self.token, &self.session, call).await
    }
}

impl Caller for ConnectedApp {
    async fn call(&self, app: &axum::Router, call: Call) -> (StatusCode, Value) {
        bearer_call(app, &self.token, &self.session, call).await
    }
}

async fn bearer_call(
    app: &axum::Router,
    token: &str,
    session: &str,
    call: Call,
) -> (StatusCode, Value) {
    match call {
        Call::Tool { name, arguments } => (
            StatusCode::OK,
            tool(app, token, session, name, arguments).await,
        ),
        Call::PublicApi { tool, arguments } => {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/public/v1/{tool}"))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&arguments).expect("JSON")))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 8_000_000)
                .await
                .expect("body");
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }
        Call::Http { method, path, body } => {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).expect("JSON")))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 8_000_000)
                .await
                .expect("body");
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }
    }
}

/// Publishes the outbox until it's empty, so the events a write queued
/// (realtime, notifications, webhooks) have gone out.
pub(crate) async fn drain_outbox(state: &AppState) {
    let worker = outbox_worker_id();
    for _ in 0..100 {
        let published = publish_ready_outbox_batch(&state.pool, &state.realtime, &worker)
            .await
            .expect("publish the outbox");
        if published == 0 {
            return;
        }
    }
    panic!("the outbox didn't drain in 100 batches");
}

/// Fails when `body` carries any of `markers` (text or IDs that mustn't
/// reach this caller), naming the first one found.
pub(crate) fn assert_absent(body: &Value, markers: &[&str]) {
    let text = body.to_string().to_ascii_lowercase();
    for marker in markers {
        assert!(
            !text.contains(&marker.to_ascii_lowercase()),
            "{marker:?} reached this caller: {body}"
        );
    }
}
