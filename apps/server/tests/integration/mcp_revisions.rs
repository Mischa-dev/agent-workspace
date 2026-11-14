//! K2.1: what the MCP server does for each protocol revision it serves.
//!
//! Clients connect on 2025-06-18 (most connectors), 2025-11-25 and
//! 2026-07-28 (stateless, `server/discover`, no `initialize`). This pins, for
//! each, the handshake, a tools/list with its cache hints, one read and one
//! write tools/call, and for the older revisions a session the server no
//! longer holds being resumed. The operation layer (K2.3) serves its tools
//! through the same handler, so a change that breaks a revision fails here.

use super::{
    agent_interface::{connect_app, send},
    mcp_admin::{create_project, setup},
};
use agent_workspace_server::mcp::MCP_PATH;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const LEGACY: [&str; 2] = ["2025-06-18", "2025-11-25"];
const STATELESS: &str = "2026-07-28";

/// One MCP POST; returns the status, the session header and the JSON-RPC
/// message (from a JSON body or the first SSE data line).
async fn post(
    app: &axum::Router,
    token: &str,
    revision: &str,
    session: Option<&str>,
    payload: Value,
) -> (StatusCode, Option<String>, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    // An `initialize` names its revision in the body; everything after it
    // carries the header.
    if payload["method"] != "initialize" {
        request = request.header("mcp-protocol-version", revision);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    // From 2026-07-28 the method (and a tool call's name) ride in headers
    // too, so a gateway can route without reading the body.
    if revision == STATELESS {
        if let Some(method) = payload["method"].as_str() {
            request = request.header("mcp-method", method);
        }
        if let Some(name) = payload.pointer("/params/name").and_then(Value::as_str) {
            request = request.header("mcp-name", name);
        }
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&payload).expect("JSON")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    (
        status,
        session,
        serde_json::from_str(json).unwrap_or(Value::Null),
    )
}

/// The `_meta` a 2026-07-28 request carries instead of a session.
fn stateless_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": STATELESS,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

fn request(id: u64, method: &str, mut params: Value, revision: &str) -> Value {
    if revision == STATELESS {
        params["_meta"] = stateless_meta();
    }
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn tool_names(listed: &Value) -> Vec<String> {
    listed
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_served_revision_lists_reads_writes_and_resumes() {
    let Some(setup) = setup("k2_mcp_revisions").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "REV", "Revisions").await;
    let token = connect_app(app, &setup.browser, "Revision matrix", "full").await;

    for revision in LEGACY.into_iter().chain([STATELESS]) {
        // The handshake: `initialize` before 2026-07-28, `server/discover`
        // from it on (no session).
        let session = if revision == STATELESS {
            let (status, session, discovered) = post(
                app,
                &token,
                revision,
                None,
                request(1, "server/discover", json!({}), revision),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{revision} discover: {discovered}");
            assert!(session.is_none(), "{revision} is stateless: {session:?}");
            let result = &discovered["result"];
            assert!(
                result.pointer("/capabilities/tools").is_some(),
                "{revision} discover: {discovered}"
            );
            // This revision names the server in `_meta`, not `serverInfo`.
            assert_eq!(
                result
                    .pointer("/_meta/io.modelcontextprotocol~1serverInfo/name")
                    .or_else(|| result.pointer("/serverInfo/name")),
                Some(&json!("agent-workspace")),
                "{discovered}"
            );
            assert!(
                result["supportedVersions"]
                    .as_array()
                    .is_some_and(|versions| LEGACY
                        .iter()
                        .chain([&STATELESS])
                        .all(|revision| versions.contains(&json!(revision)))),
                "{discovered}"
            );
            // The Tasks extension (long operations, K2.21) is something
            // rmcp can serve; this server doesn't advertise it yet.
            assert!(
                result
                    .pointer("/capabilities/extensions")
                    .is_none_or(|extensions| { !extensions.to_string().contains("tasks") }),
                "{discovered}"
            );
            None
        } else {
            let (status, session, initialized) = post(
                app,
                &token,
                revision,
                None,
                json!({
                    "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {
                        "protocolVersion": revision,
                        "capabilities": {},
                        "clientInfo": { "name": "revision-matrix", "version": "1" }
                    }
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{revision}: {initialized}");
            assert_eq!(
                initialized.pointer("/result/protocolVersion"),
                Some(&json!(revision)),
                "{revision}: {initialized}"
            );
            let session = session.unwrap_or_else(|| panic!("{revision} gives a session"));
            let (status, _, _) = post(
                app,
                &token,
                revision,
                Some(&session),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED, "{revision} initialized");
            Some(session)
        };

        // tools/list: filtered to the caller, with the cache hints 2026-07-28
        // clients require (and older ones ignore).
        let (status, _, listed) = post(
            app,
            &token,
            revision,
            session.as_deref(),
            request(2, "tools/list", json!({}), revision),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revision} list: {listed}");
        let names = tool_names(&listed);
        for expected in ["briefing", "projects_list", "decision_record"] {
            assert!(
                names.contains(&expected.to_owned()),
                "{revision}: {expected}"
            );
        }
        assert!(
            listed.pointer("/result/ttlMs").is_some()
                || listed.pointer("/result/ttl").is_some()
                || listed.pointer("/result/_meta").is_some(),
            "{revision} list carries its cache hints: {}",
            listed["result"]
                .as_object()
                .map(|result| result.keys().cloned().collect::<Vec<_>>().join(","))
                .unwrap_or_default()
        );

        // One read and one write.
        let (status, _, read) = post(
            app,
            &token,
            revision,
            session.as_deref(),
            request(
                3,
                "tools/call",
                json!({ "name": "projects_list", "arguments": {} }),
                revision,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revision} read: {read}");
        assert_eq!(
            read.pointer("/result/structuredContent/outcome"),
            Some(&json!("ok")),
            "{revision} read: {read}"
        );
        let (status, _, wrote) = post(
            app,
            &token,
            revision,
            session.as_deref(),
            request(
                4,
                "tools/call",
                json!({
                    "name": "decision_record",
                    "arguments": {
                        "projectId": project,
                        "title": format!("Serve {revision}"),
                    }
                }),
                revision,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revision} write: {wrote}");
        assert_eq!(
            wrote.pointer("/result/structuredContent/outcome"),
            Some(&json!("ok")),
            "{revision} write: {wrote}"
        );

        // A session the server no longer holds (a restart, five idle
        // minutes) is picked up again instead of answering 404.
        if session.is_some() {
            let forgotten = uuid::Uuid::now_v7().to_string();
            let (status, _, listed) = post(
                app,
                &token,
                revision,
                Some(&forgotten),
                request(5, "tools/list", json!({}), revision),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{revision} resumed: {listed}");
            assert!(!tool_names(&listed).is_empty(), "{revision} resumed");
        }
    }

    // Both writes landed once each.
    let (status, decisions) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/decisions"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{decisions}");
    assert_eq!(
        decisions["decisions"].as_array().map(Vec::len),
        Some(3),
        "{decisions}"
    );
}
