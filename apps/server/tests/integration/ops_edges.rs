//! K2.3: the generated edges. The kernel's test operations, called through
//! the v2 route (a browser session with CSRF, and a bearer), MCP and an
//! automation rule, give the same envelope and leave one audit row each
//! saying which edge they came by. The route's workspace must be where the
//! target is; a caller reaching two workspaces names one, by argument or by
//! the MCP alias path, and the two can't disagree; a viewer's tool list
//! leaves out what they can't do; a request with both a cookie and a bearer
//! is refused.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use agent_workspace_server::ops::registry::register_for_tests;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
    ops_kernel::{add_echo, scratch_table},
};

fn bearer(token: &str, method: &str, uri: &str, key: Option<&str>, body: &Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(Body::from(serde_json::to_vec(body).expect("body")))
        .expect("request")
}

/// One MCP request at `path`, answered as JSON (status, body).
async fn rpc(
    app: &axum::Router,
    token: &str,
    path: &str,
    session: &str,
    payload: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::HOST, "127.0.0.1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", "2025-06-18")
                .body(Body::from(serde_json::to_vec(&payload).expect("JSON")))
                .expect("MCP request"),
        )
        .await
        .expect("MCP response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    (status, serde_json::from_str(json).unwrap_or(Value::Null))
}

async fn call_at(
    app: &axum::Router,
    token: &str,
    path: &str,
    session: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let (status, reply) = rpc(
        app,
        token,
        path,
        session,
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("no structured content: {reply}"))
}

async fn tool_names(app: &axum::Router, token: &str) -> Vec<String> {
    let session = initialize(app, token).await.expect("MCP session");
    let (status, reply) = rpc(
        app,
        token,
        "/api/v1/mcp",
        &session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn every_edge_runs_the_same_operation() {
    let Some(setup) = setup("k2_ops_edges").await else {
        return;
    };
    register_for_tests(add_echo);
    scratch_table(&setup.pool).await;
    let app = &setup.app;
    let space = create_project(&setup, "EDG", "Edges").await;
    let workspace = setup.organization_id;
    let slug: String = sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(workspace)
        .fetch_one(&setup.pool)
        .await
        .expect("slug");
    let route = format!("/api/v2/w/{workspace}/ops/echo_create");

    // The web app: a session with CSRF, by the workspace's id or slug.
    let (status, web) = send(
        app,
        setup.browser.request(
            "POST",
            &route,
            Some("web-1"),
            json!({ "space": space, "text": "web" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{web}");
    assert_eq!(web["outcome"], "ok", "{web}");
    assert_eq!(web["value"]["text"], "web");
    let (status, by_slug) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v2/w/{slug}/ops/echo_create"),
            Some("web-2"),
            json!({ "space": space, "text": "web by slug" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_slug}");
    // A web write carries its request id.
    let (status, unkeyed) = send(
        app,
        setup.browser.request(
            "POST",
            &route,
            None,
            json!({ "space": space, "text": "no key" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unkeyed}");
    assert_eq!(unkeyed["field"], "Idempotency-Key");

    // A script: the same route with a bearer.
    let token = connect_app(app, &setup.browser, "Edge script", "full").await;
    let (status, api) = send(
        app,
        bearer(
            &token,
            "POST",
            &route,
            Some("api-1"),
            &json!({ "space": space, "text": "api" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{api}");
    assert_eq!(api["value"]["text"], "api");

    // A cookie and a bearer together are refused.
    let mut both = setup.browser.request(
        "POST",
        &route,
        Some("both-1"),
        json!({ "space": space, "text": "both" }),
    );
    both.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("header"),
    );
    let (status, refused) = send(app, both).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["field"], "authorization");

    // An agent's tool: the same operation over MCP, its answer matching the
    // tool's output schema.
    let session = initialize(app, &token).await.expect("MCP session");
    let mcp = tool(
        app,
        &token,
        &session,
        "echo_create",
        json!({ "space": space, "text": "mcp", "clientRequestId": "mcp-1" }),
    )
    .await;
    assert_eq!(mcp["outcome"], "ok", "{mcp}");
    assert_eq!(mcp["value"]["text"], "mcp");
    assert_eq!(mcp["requestId"], "mcp-1");

    // An automation rule that runs it, run by hand.
    let (status, rule) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/automations"),
            None,
            json!({
                "name": "Echo",
                "trigger": {"kind": "task_created"},
                "actions": [{
                    "kind": "run_operation",
                    "operation": "echo_create",
                    "input": { "space": space, "text": "rule {{rule.name}}" }
                }],
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {rule}");
    let rule_id = rule["id"].as_str().expect("rule");
    let (status, run) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/automations/{rule_id}/run"),
            None,
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {run}");
    let automated: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ops_kernel_echo WHERE text = 'rule Echo'")
            .fetch_one(&setup.pool)
            .await
            .expect("automated row");
    assert_eq!(automated, 1, "{run}");
    // An operation no rule may run is refused when the rule is saved.
    let (status, unknown) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/automations"),
            None,
            json!({
                "name": "Nothing",
                "trigger": {"kind": "task_created"},
                "actions": [{ "kind": "run_operation", "operation": "no_such_op", "input": {} }],
            }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {unknown}");

    // One audit row per write, each naming its edge.
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT channel, request_id FROM audit_events WHERE action = 'echo_create' \
         ORDER BY occurred_at, id",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("audit rows");
    let channels = rows
        .iter()
        .map(|(channel, _)| channel.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        channels,
        ["browser", "browser", "api", "mcp", "automation"],
        "{rows:?}"
    );
    assert_eq!(rows[0].1.as_deref(), Some("web-1"));
    assert_eq!(rows[3].1.as_deref(), Some("mcp-1"));
    assert!(
        rows[4]
            .1
            .as_deref()
            .is_some_and(|id| id.starts_with("automation-")),
        "{rows:?}"
    );

    // A second workspace the owner is in.
    let other = Uuid::now_v7();
    let other_slug = format!("other-{}", other.simple());
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Other', $2)")
        .bind(other)
        .bind(&other_slug)
        .execute(&setup.pool)
        .await
        .expect("other workspace");
    let owner: Uuid = sqlx::query_scalar(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(workspace)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(other)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("owner joins");

    // The route's workspace must be where the target is.
    let (status, mismatch) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v2/w/{other}/ops/echo_create"),
            Some("web-3"),
            json!({ "space": space, "text": "elsewhere" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{mismatch}");
    // An unknown workspace is not found either.
    let (status, _) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/w/no-such-workspace/ops/echo_create",
            Some("web-4"),
            json!({ "space": space, "text": "nowhere" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Reaching two workspaces, a call whose target doesn't say which must
    // name one.
    let unnamed = call_at(
        app,
        &token,
        "/api/v1/mcp",
        &session,
        "echo_note",
        json!({ "text": "where?" }),
    )
    .await;
    assert_eq!(unnamed["outcome"], "invalid_request", "{unnamed}");
    assert_eq!(unnamed["field"], "workspace");
    let named = call_at(
        app,
        &token,
        "/api/v1/mcp",
        &session,
        "echo_note",
        json!({ "text": "here", "workspace": slug }),
    )
    .await;
    assert_eq!(named["outcome"], "ok", "{named}");
    let aliased = call_at(
        app,
        &token,
        &format!("/api/v1/w/{other_slug}/mcp"),
        &session,
        "echo_note",
        json!({ "text": "there" }),
    )
    .await;
    assert_eq!(aliased["outcome"], "ok", "{aliased}");
    let workspaces: Vec<Uuid> = sqlx::query_scalar(
        "SELECT organization_id FROM audit_events WHERE action = 'echo_note' ORDER BY occurred_at, id",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("note rows");
    assert_eq!(workspaces, [workspace, other]);
    // Through the other workspace's path, a hand-written tool doesn't reach
    // this workspace's space, for a write or a read; through its own path
    // it does.
    let (status, refused) = rpc(
        app,
        &token,
        &format!("/api/v1/w/{other_slug}/mcp"),
        &session,
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "task_create", "arguments": { "projectId": space, "title": "Elsewhere" } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM task_rows WHERE title = 'Elsewhere'")
        .fetch_one(&setup.pool)
        .await
        .expect("tasks");
    assert_eq!(made, 0);
    let (status, refused) = rpc(
        app,
        &token,
        &format!("/api/v1/w/{other_slug}/mcp"),
        &session,
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "project_open", "arguments": { "projectId": "EDG" } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let (status, opened) = rpc(
        app,
        &token,
        &format!("/api/v1/w/{slug}/mcp"),
        &session,
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "project_open", "arguments": { "projectId": "EDG" } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_ne!(opened["result"]["isError"], json!(true), "{opened}");
    // The alias path and the argument disagreeing is refused.
    let disagree = call_at(
        app,
        &token,
        &format!("/api/v1/w/{other_slug}/mcp"),
        &session,
        "echo_note",
        json!({ "text": "which?", "workspace": slug }),
    )
    .await;
    assert_eq!(disagree["outcome"], "invalid_request", "{disagree}");
    assert_eq!(disagree["field"], "workspace");
    // An alias for a workspace the credential doesn't reach is not found.
    let stranger = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Stranger', $2)")
        .bind(stranger)
        .bind(format!("stranger-{}", stranger.simple()))
        .execute(&setup.pool)
        .await
        .expect("stranger workspace");
    let (status, _) = rpc(
        app,
        &token,
        &format!("/api/v1/w/{stranger}/mcp"),
        &session,
        json!({
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": { "name": "echo_note", "arguments": { "text": "x" } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A viewer's tool list leaves out the write they can't make.
    assert!(
        tool_names(app, &token)
            .await
            .contains(&"echo_create".to_owned())
    );
    let (status, created) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/external-agents"),
            None,
            json!({
                "connectionName": "Watcher",
                "displayName": "Watcher",
                "handle": "watcher",
                "role": "member",
                "scopes": ["read", "write", "events"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let viewer = created["oneTimeCredential"]["token"]
        .as_str()
        .expect("viewer token");
    assert!(
        tool_names(app, viewer)
            .await
            .contains(&"echo_create".to_owned())
    );
    // Its role becomes viewer; its credential can still write.
    let watcher: Uuid = created["connection"]["actorId"]
        .as_str()
        .expect("watcher")
        .parse()
        .expect("watcher id");
    let (status, readers) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{workspace}/roles"),
            None,
            json!({"name": "Readers", "baseRole": "member", "permissions": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{readers}");
    let readers: Uuid = readers["id"]
        .as_str()
        .expect("role")
        .parse()
        .expect("role id");
    sqlx::query("UPDATE organization_memberships SET role_id = $2 WHERE actor_id = $1")
        .bind(watcher)
        .bind(readers)
        .execute(&setup.pool)
        .await
        .expect("workspace role");
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer', role_id = NULL WHERE actor_id = $1",
    )
    .bind(watcher)
    .execute(&setup.pool)
    .await
    .expect("viewer role");
    let listed = tool_names(app, viewer).await;
    assert!(!listed.is_empty());
    assert!(!listed.contains(&"echo_create".to_owned()), "{listed:?}");
}

/// K2.6: an old tool name runs its operation with its old arguments,
/// answers in its old shape byte for byte, says what to use instead, and is
/// counted once per call.
#[tokio::test]
async fn an_old_name_answers_in_its_old_shape_and_is_counted() {
    use agent_workspace_server::ops::aliases::{Alias, register_alias_for_tests};

    let Some(setup) = setup("k2_ops_alias").await else {
        return;
    };
    register_for_tests(add_echo);
    register_alias_for_tests(Alias {
        name: "echo_make",
        canonical: |_| "echo_create",
        runs: &["echo_create"],
        adapt_in: |mut arguments| {
            if let Some(words) = arguments.remove("words") {
                arguments.insert("text".into(), words);
            }
            arguments
        },
        adapt_out: |_, value| json!({ "echo": { "words": value["text"] } }),
        sunset: Some("2027-06-01"),
    });
    scratch_table(&setup.pool).await;
    let app = &setup.app;
    let space = create_project(&setup, "ALI", "Aliases").await;
    let token = connect_app(app, &setup.browser, "Old client", "full").await;
    let session = initialize(app, &token).await.expect("MCP session");

    let (status, reply) = rpc(
        app,
        &token,
        "/api/v1/mcp",
        &session,
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {
                "name": "echo_make",
                "arguments": { "space": space, "words": "kept", "clientRequestId": "old-1" }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let answered = reply
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("no structured content: {reply}"));
    assert_eq!(
        serde_json::to_string(&answered).expect("JSON"),
        r#"{"echo":{"words":"kept"}}"#
    );
    assert_eq!(
        reply.pointer("/result/_meta/deprecated"),
        Some(&json!(
            "use echo_create; echo_make stops working after 2027-06-01"
        )),
        "{reply}"
    );
    // The old name isn't listed; the canonical one is.
    let names = tool_names(app, &token).await;
    assert!(names.iter().any(|name| name == "echo_create"), "{names:?}");
    assert!(!names.iter().any(|name| name == "echo_make"), "{names:?}");

    // The public API's v1 route: the old shape, with Deprecation and Sunset.
    let response = app
        .clone()
        .oneshot(bearer(
            &token,
            "POST",
            "/api/public/v1/echo_make",
            None,
            &json!({ "space": space, "words": "again", "clientRequestId": "old-2" }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("deprecation")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    assert_eq!(
        response
            .headers()
            .get("sunset")
            .and_then(|value| value.to_str().ok()),
        Some("Tue, 01 Jun 2027 00:00:00 GMT")
    );
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    assert_eq!(
        std::str::from_utf8(&bytes).expect("utf-8"),
        r#"{"echo":{"words":"again"}}"#
    );

    // Each call ran the operation once and was counted once.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ops_kernel_echo WHERE text IN ('kept', 'again')")
            .fetch_one(&setup.pool)
            .await
            .expect("rows");
    assert_eq!(rows, 2);
    let calls: i64 = sqlx::query_scalar(
        "SELECT calls FROM operation_alias_usage WHERE workspace_id = $1 AND alias = 'echo_make'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("usage");
    assert_eq!(calls, 2);
    // A retry with the same request id replays without counting again.
    let (_, again) = rpc(
        app,
        &token,
        "/api/v1/mcp",
        &session,
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "tools/call",
            "params": {
                "name": "echo_make",
                "arguments": { "space": space, "words": "kept", "clientRequestId": "old-1" }
            }
        }),
    )
    .await;
    assert_eq!(
        again.pointer("/result/structuredContent"),
        Some(&json!({ "echo": { "words": "kept" } })),
        "{again}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM ops_kernel_echo WHERE text = 'kept'")
        .fetch_one(&setup.pool)
        .await
        .expect("rows");
    assert_eq!(rows, 1);
}
