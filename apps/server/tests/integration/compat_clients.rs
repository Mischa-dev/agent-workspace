//! Contracts with clients already connected to Agent Workspace, which the
//! server can't update: the MCP revisions they speak, the tools and argument
//! names they call, and the URLs they read. The fixture
//! (`tests/fixtures/compat/connected-clients.json`) lists them, and
//! `docs/compatibility/connected-clients.md` says who relies on each and
//! when it may go. A change that breaks one fails here first.

use agent_workspace_server::mcp::{MCP_PATH, tool_catalog};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::create_agent,
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/compat/connected-clients.json"))
        .expect("connected-clients.json")
}

/// One JSON-RPC call to the MCP endpoint at `revision`: the status, the
/// session header and the JSON answer (from the SSE stream when it's one).
async fn rpc(
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
    if let Some(session) = session {
        request = request
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", revision);
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
    let bytes = to_bytes(response.into_body(), 8_000_000)
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

/// Clients pinned to an older MCP revision still connect, list and call,
/// and a session the server no longer holds (after a deploy) still works.
#[tokio::test]
async fn old_mcp_revisions_still_shake_hands() {
    let Some(setup) = setup("compat_revisions").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (agent, token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Rook", "rook").await;
    add_member(&setup, &work, agent, "member").await;
    let fixture = fixture();
    for revision in fixture["mcpRevisions"].as_array().expect("revisions") {
        let revision = revision.as_str().expect("revision");
        let (status, session, initialized) = rpc(
            app,
            &token,
            revision,
            None,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": revision,
                    "capabilities": {},
                    "clientInfo": {"name": "compat", "version": "1"}
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revision}: {initialized}");
        assert_eq!(
            initialized["result"]["protocolVersion"], revision,
            "{initialized}"
        );
        let session = session.unwrap_or_else(|| panic!("{revision}: no mcp-session-id"));
        let (status, _, _) = rpc(
            app,
            &token,
            revision,
            Some(&session),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{revision}");
        let (_, _, listed) = rpc(
            app,
            &token,
            revision,
            Some(&session),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
        assert!(
            listed["result"]["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "briefing")),
            "{revision}: {listed}"
        );
        let call = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "briefing", "arguments": {}}
        });
        let (_, _, called) = rpc(app, &token, revision, Some(&session), call.clone()).await;
        assert_eq!(
            called["result"]["structuredContent"]["outcome"], "ok",
            "{revision}: {called}"
        );
        // A session this server never made (the one before a deploy) resumes.
        let resumed_session = Uuid::now_v7().to_string();
        let (status, _, resumed) = rpc(app, &token, revision, Some(&resumed_session), call).await;
        assert_eq!(status, StatusCode::OK, "{revision}: {resumed}");
        assert_eq!(
            resumed["result"]["structuredContent"]["outcome"], "ok",
            "{revision}: {resumed}"
        );
    }
}

/// Every tool a connected client calls is still there, under its name, with
/// the arguments the client sends, and answers with an outcome.
#[test]
fn connected_clients_find_their_tools() {
    let catalog = tool_catalog();
    let fixture = fixture();
    let mut problems = Vec::new();
    for (client, contract) in fixture["clients"].as_object().expect("clients") {
        for (name, arguments) in contract["tools"].as_object().expect("tools") {
            let Some(tool) = catalog.iter().find(|tool| tool.name == name.as_str()) else {
                problems.push(format!("{client}: {name} is gone"));
                continue;
            };
            let properties = tool.input_schema.get("properties");
            for argument in arguments.as_array().expect("arguments") {
                let argument = argument.as_str().expect("argument");
                if properties.and_then(|found| found.get(argument)).is_none() {
                    problems.push(format!("{client}: {name} no longer takes {argument}"));
                }
            }
            // Every answer (each branch of the schema) names its outcome.
            let outcome = tool.output_schema.as_deref().is_some_and(|schema| {
                serde_json::to_string(schema)
                    .expect("schema")
                    .contains("\"outcome\"")
            });
            if !outcome {
                problems.push(format!("{client}: {name} answers with no outcome"));
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The URLs agents are pointed at answer: the MCP endpoint asks for a token
/// and says where to get one, the rest answer as they are.
#[tokio::test]
async fn agent_facing_urls_answer() {
    let Some(setup) = setup("compat_urls").await else {
        return;
    };
    for url in fixture()["urls"].as_array().expect("urls") {
        let url = url.as_str().expect("url");
        let method = if url == MCP_PATH { "POST" } else { "GET" };
        let response = setup
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(url)
                    .header(header::HOST, "127.0.0.1")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        if url == MCP_PATH {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{url}");
            let challenge = response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            assert!(
                challenge.contains("resource_metadata="),
                "{url}: {challenge}"
            );
        } else {
            assert_eq!(response.status(), StatusCode::OK, "{url}");
        }
    }
}
