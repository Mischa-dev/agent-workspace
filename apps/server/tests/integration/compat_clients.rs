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

/// K5.7: a session its client ended with DELETE stays ended on every
/// server, while a session a server simply doesn't hold (after a restart,
/// or held by another server) is still picked up.
#[tokio::test]
async fn an_ended_session_stays_ended_on_another_server() {
    let Some(setup) = setup("compat_ended_session").await else {
        return;
    };
    let url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    // A second server on the same database, with its own sessions.
    let other = crate::support::app::test_app(setup.pool.clone(), &url);
    let (_, token) = create_agent(
        &setup.app,
        &setup.browser,
        &space(&setup).await,
        "Rook",
        "rook",
    )
    .await;
    let revision = "2025-06-18";
    let (status, session, initialized) = rpc(
        &setup.app,
        &token,
        revision,
        None,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": revision,
                "capabilities": {},
                "clientInfo": {"name": "ended", "version": "1"}
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{initialized}");
    let session = session.expect("mcp-session-id");
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});

    // The other server picks the live session up.
    let (status, _, _) = rpc(&other, &token, revision, Some(&session), list.clone()).await;
    assert_eq!(status, StatusCode::OK);

    let ended = setup
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(MCP_PATH)
                .header(header::HOST, "127.0.0.1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", revision)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(ended.status().is_success(), "{}", ended.status());

    for (name, app) in [("this", &setup.app), ("the other", &other)] {
        let (status, _, _) = rpc(app, &token, revision, Some(&session), list.clone()).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{name} server resumed an ended session"
        );
    }
    let (status, _, _) = rpc(
        &other,
        &token,
        revision,
        Some("held-by-a-server-that-restarted"),
        list,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
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
                .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "briefing_get")),
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
            // An old name that became an alias (K2) still answers: its
            // arguments are checked against the operation it runs.
            let alias = agent_workspace_server::ops::aliases::active(name);
            let named = alias.map_or(name.as_str(), |alias| alias.runs[0]);
            let Some(tool) = catalog.iter().find(|tool| tool.name == named) else {
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
