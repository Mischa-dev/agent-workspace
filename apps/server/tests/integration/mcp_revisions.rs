//! K2.1: what the MCP server does for each protocol revision it serves.
//!
//! Clients connect on 2025-06-18 (most connectors), 2025-11-25 and
//! 2026-07-28 (stateless, `server/discover`, no `initialize`). This pins, for
//! each, the handshake, a tools/list with its cache hints, one read and one
//! write tools/call, and for the older revisions a session the server no
//! longer holds being resumed. The operation layer (K2.3) serves its tools
//! through the same handler, so a change that breaks a revision fails here.

use super::support::long::{run_jobs, second_process};
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
        // A task method names its task there.
        if let Some(name) = payload
            .pointer("/params/name")
            .or_else(|| payload.pointer("/params/taskId"))
            .and_then(Value::as_str)
        {
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
            // The Tasks extension: long operations (K2.21).
            assert!(
                result
                    .pointer("/capabilities/extensions")
                    .is_some_and(|extensions| extensions.to_string().contains("tasks")),
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
        for expected in ["briefing_get", "project_list", "decision_record"] {
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

fn many_tasks(project: &str, count: usize, request_id: &str) -> Value {
    json!({
        "name": "task_create_many",
        "arguments": {
            "projectId": project,
            "clientRequestId": request_id,
            "tasks": (0..count)
                .map(|index| json!({ "title": format!("Long batch task {index}") }))
                .collect::<Vec<_>>(),
        }
    })
}

/// K2.21: a long call is answered with a task (2026-07-28 with the Tasks
/// extension) or `started` (older revisions), a job finishes it, and only
/// the caller can read the answer, from any server process.
#[tokio::test]
async fn long_calls_finish_as_tasks_any_server_can_answer() {
    let Some(setup) = setup("k2_long_tasks").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "LNG", "Long calls").await;
    let token = connect_app(app, &setup.browser, "Long calls", "full").await;
    let (_, other_token) = super::support::app::create_agent(
        app,
        &setup.browser,
        &project,
        "Other agent",
        "other-agent",
    )
    .await;
    let (second_app, second_state) = second_process(&setup.pool);

    // 2026-07-28, declaring the Tasks extension: a task handle.
    let mut call = request(
        10,
        "tools/call",
        many_tasks(&project, 30, "long-a"),
        STATELESS,
    );
    call["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
        json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } });
    let (status, _, started) = post(app, &token, STATELESS, None, call).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(
        started.pointer("/result/resultType"),
        Some(&json!("task")),
        "{started}"
    );
    let task_id = started
        .pointer("/result/taskId")
        .and_then(Value::as_str)
        .expect("task id")
        .to_owned();
    let tasks_get = |id: u64, task: &str, declared: bool| {
        let mut get = request(id, "tasks/get", json!({ "taskId": task }), STATELESS);
        if declared {
            get["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
                json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } });
        }
        get
    };
    let (_, _, working) = post(app, &token, STATELESS, None, tasks_get(11, &task_id, true)).await;
    assert_eq!(
        working.pointer("/result/status"),
        Some(&json!("working")),
        "{working}"
    );

    // The second process runs the job and answers tasks/get.
    assert!(run_jobs(&second_state).await >= 1);
    let (_, _, done) = post(
        &second_app,
        &token,
        STATELESS,
        None,
        tasks_get(12, &task_id, true),
    )
    .await;
    assert_eq!(
        done.pointer("/result/status"),
        Some(&json!("completed")),
        "{done}"
    );
    let envelope = done
        .pointer("/result/result/structuredContent")
        .expect("the call's answer");
    assert_eq!(envelope["outcome"], "ok", "{envelope}");
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE title LIKE 'Long batch task %'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(made, 30);

    // Another account's tasks/get is not found.
    let (_, _, refused) = post(
        app,
        &other_token,
        STATELESS,
        None,
        tasks_get(13, &task_id, true),
    )
    .await;
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(refused.get("result").is_none(), "{refused}");

    // A small batch answers at once.
    let mut small = request(
        14,
        "tools/call",
        many_tasks(&project, 2, "long-small"),
        STATELESS,
    );
    small["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
        json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } });
    let (_, _, answered) = post(app, &token, STATELESS, None, small).await;
    assert_eq!(
        answered.pointer("/result/structuredContent/outcome"),
        Some(&json!("ok")),
        "{answered}"
    );

    // 2025-06-18: `started`, and job_get finishes it.
    let (_, session, _) = post(
        app,
        &token,
        "2025-06-18",
        None,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "long-calls", "version": "1" }
            }
        }),
    )
    .await;
    let session = session.expect("session");
    post(
        app,
        &token,
        "2025-06-18",
        Some(&session),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    let (_, _, started) = post(
        app,
        &token,
        "2025-06-18",
        Some(&session),
        request(
            20,
            "tools/call",
            many_tasks(&project, 26, "long-b"),
            "2025-06-18",
        ),
    )
    .await;
    let envelope = started
        .pointer("/result/structuredContent")
        .expect("envelope");
    assert_eq!(envelope["outcome"], "started", "{started}");
    let job_id = envelope["value"]["jobId"]
        .as_str()
        .expect("job id")
        .to_owned();
    // The same request id finds the same job instead of starting another.
    let (_, _, again) = post(
        app,
        &token,
        "2025-06-18",
        Some(&session),
        request(
            21,
            "tools/call",
            many_tasks(&project, 26, "long-b"),
            "2025-06-18",
        ),
    )
    .await;
    assert_eq!(
        again.pointer("/result/structuredContent/value/jobId"),
        Some(&json!(job_id)),
        "{again}"
    );
    let job_get = |id: u64, job: &str| {
        request(
            id,
            "tools/call",
            json!({ "name": "job_get", "arguments": { "jobId": job } }),
            "2025-06-18",
        )
    };
    let (_, _, working) = post(
        app,
        &token,
        "2025-06-18",
        Some(&session),
        job_get(22, &job_id),
    )
    .await;
    assert_eq!(
        working.pointer("/result/structuredContent/value/status"),
        Some(&json!("working")),
        "{working}"
    );
    assert!(run_jobs(&second_state).await >= 1);
    let (_, _, done) = post(
        app,
        &token,
        "2025-06-18",
        Some(&session),
        job_get(23, &job_id),
    )
    .await;
    let value = done
        .pointer("/result/structuredContent/value")
        .expect("job");
    assert_eq!(value["status"], "completed", "{done}");
    assert_eq!(value["result"]["outcome"], "ok", "{done}");
    // Another account's job_get is not found.
    let (_, _, refused) = post(
        app,
        &other_token,
        STATELESS,
        None,
        request(
            24,
            "tools/call",
            json!({ "name": "job_get", "arguments": { "jobId": job_id } }),
            STATELESS,
        ),
    )
    .await;
    assert_eq!(
        refused.pointer("/result/structuredContent/outcome"),
        Some(&json!("not_found")),
        "{refused}"
    );

    // The same person's read-only app can't make tasks, so it doesn't read
    // the batch's answer either.
    let other_app = connect_app(app, &setup.browser, "Read-only app", "read").await;
    let (_, _, refused) = post(
        app,
        &other_app,
        STATELESS,
        None,
        request(
            25,
            "tools/call",
            json!({ "name": "job_get", "arguments": { "jobId": job_id } }),
            STATELESS,
        ),
    )
    .await;
    assert_eq!(
        refused.pointer("/result/structuredContent/outcome"),
        Some(&json!("not_found")),
        "{refused}"
    );

    // Once the caller may no longer act where it ran (here the space is
    // archived), the kept answer isn't shown, through either route.
    sqlx::query(
        "UPDATE projects SET archived_at = now(), archived_by_actor_id = (SELECT actor_id FROM project_memberships \
         WHERE project_id = $1::uuid LIMIT 1) \
         WHERE id = $1::uuid",
    )
    .bind(&project)
    .execute(&setup.pool)
    .await
    .expect("archive the space");
    let (_, _, hidden) = post(
        app,
        &token,
        STATELESS,
        None,
        request(
            26,
            "tools/call",
            json!({ "name": "job_get", "arguments": { "jobId": job_id } }),
            STATELESS,
        ),
    )
    .await;
    assert_eq!(
        hidden.pointer("/result/structuredContent/outcome"),
        Some(&json!("not_found")),
        "{hidden}"
    );
    let (_, _, hidden) = post(app, &token, STATELESS, None, tasks_get(27, &task_id, true)).await;
    assert!(hidden.get("error").is_some(), "{hidden}");
}
