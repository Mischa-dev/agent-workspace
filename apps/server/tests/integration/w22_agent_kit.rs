//! W22: what makes agents' work easier. The silent, costly failures: a
//! skill or handoff note from a project an agent isn't in reaching it, a
//! skill that was switched off or archived still offered, a handoff lost
//! between one agent letting go and the next one claiming, and a test
//! wakeup that isn't signed like the real thing (so it passes while real
//! wakeups fail).

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
    routing::post,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, test_app_with, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
};

#[tokio::test]
async fn skills_are_offered_only_where_the_agent_works_and_only_while_they_are_skills() {
    let Some(setup) = setup("w22_skills").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "SIDE", "Side").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    // Mara works only in SIDE.
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &other, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");

    let written = call(
        "page_write",
        json!({
            "projectId": "WORK",
            "title": "Deploying the web app",
            "body": "1. Run the checks.\n2. Ship it.",
            "skillDescription": "  Use when deploying\n the web app.  "
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let page = written["value"]["id"].as_str().expect("page").to_owned();
    assert_eq!(
        written["value"]["skillDescription"],
        "Use when deploying the web app."
    );
    call(
        "page_write",
        json!({"projectId": "WORK", "title": "Theming", "body": "Colors."}),
    )
    .await;

    let listed = call("skill_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let skills = listed["value"]["skills"].as_array().expect("skills");
    assert_eq!(skills.len(), 1, "{listed}");
    assert_eq!(skills[0]["name"], "deploying-the-web-app");
    assert_eq!(skills[0]["description"], "Use when deploying the web app.");

    let opened = call("project_open", json!({"projectId": "WORK"})).await;
    assert_eq!(
        opened["value"]["skills"][0]["name"],
        "deploying-the-web-app"
    );
    let titles = opened["value"]["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .map(|page| page["title"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Theming"], "a skill is listed once, as a skill");

    // By name (any spelling that names it the same way) or by page.
    for arguments in [
        json!({"projectId": "WORK", "name": "deploying-the-web-app"}),
        json!({"projectId": "WORK", "name": "Deploying the web app"}),
        json!({"pageId": page}),
    ] {
        let skill = call("skill_get", arguments).await;
        assert_eq!(skill["outcome"], "ok", "{skill}");
        assert_eq!(
            skill["value"]["skillMd"],
            "---\nname: deploying-the-web-app\ndescription: \"Use when deploying the web app.\"\n---\n\n1. Run the checks.\n2. Ship it.\n"
        );
    }

    // Mara isn't in WORK: neither the list nor the skill reaches her.
    let theirs = tool(
        app,
        &mara_token,
        &mara_session,
        "skill_get",
        json!({"pageId": page}),
    )
    .await;
    assert_ne!(theirs["outcome"], "ok", "{theirs}");
    assert!(!theirs.to_string().contains("Ship it"), "{theirs}");
    let theirs = tool(
        app,
        &mara_token,
        &mara_session,
        "skill_list",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(theirs["outcome"], "ok", "{theirs}");

    // Switched off, it's a plain page again; archived, it's gone.
    let off = call(
        "page_write",
        json!({"pageId": page, "skillDescription": "", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(off["value"]["version"], 2, "{off}");
    assert_eq!(off["value"]["history"][0]["note"], "No longer a skill");
    let missing = call("skill_get", json!({"pageId": page})).await;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
    call(
        "page_write",
        json!({"pageId": page, "skillDescription": "Use when deploying.", "archived": true}),
    )
    .await;
    let listed = call("skill_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["value"]["skills"], json!([]), "{listed}");
    let missing = call(
        "skill_get",
        json!({"projectId": "WORK", "name": "deploying-the-web-app"}),
    )
    .await;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
}

#[tokio::test]
async fn a_handoff_note_reaches_whoever_claims_the_task_next() {
    let Some(setup) = setup("w22_handoffs").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let side = create_project(&setup, "SIDE", "Side").await;
    let agents = space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &agents, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let (rook, rook_token) = create_agent(app, &setup.browser, &agents, "Rook", "rook").await;
    add_member(&setup, &side, rook, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let rook_session = initialize(app, &rook_token).await.expect("Rook MCP");
    let created = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the flaky test"}),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");

    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let claimed = mara_call("task_claim", json!({"taskId": "WORK-1"})).await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    assert!(
        claimed["value"].get("handoff").is_none(),
        "nothing left yet: {claimed}"
    );
    let note = "Reproduced on CI only. Branch fix-flaky has a retry; the real cause is the clock.";
    let too_long = mara_call(
        "task_release",
        json!({"taskId": "WORK-1", "handoff": "x".repeat(4_001)}),
    )
    .await;
    assert_eq!(too_long["outcome"], "invalid_request", "{too_long}");
    let released = mara_call(
        "task_release",
        json!({"taskId": "WORK-1", "handoff": format!("  {note}\n")}),
    )
    .await;
    assert_eq!(released["outcome"], "ok", "{released}");
    assert_eq!(released["value"]["released"], true);
    assert_eq!(released["value"]["handoffSaved"], true);
    // Only the claim's holder hands off: a retry (or anyone without the
    // claim) saves nothing more.
    let again = mara_call(
        "task_release",
        json!({"taskId": "WORK-1", "handoff": note}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");

    // Rook isn't in WORK: he can neither leave nor read a note there.
    let refused = tool(
        app,
        &rook_token,
        &rook_session,
        "task_release",
        json!({"taskId": "WORK-1", "handoff": "Not mine to leave."}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let hidden = tool(
        app,
        &rook_token,
        &rook_session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_ne!(hidden["outcome"], "ok", "{hidden}");

    let next = tool(
        app,
        &codex,
        &session,
        "task_claim",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(next["outcome"], "ok", "{next}");
    assert_eq!(next["value"]["handoff"]["by"], "Mara", "{next}");
    assert_eq!(next["value"]["handoff"]["note"], note);
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(opened["value"]["handoffs"][0]["note"], note, "{opened}");
    let handoffs = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_handoffs")
        .fetch_one(&setup.pool)
        .await
        .expect("count");
    assert_eq!(handoffs, 1, "the refused note wasn't saved");
}

#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<(HeaderMap, String)>>>);

#[tokio::test]
async fn a_test_wakeup_is_signed_like_a_real_one_and_reports_the_answer() {
    let Some(setup) = setup("w22_test_wakeup").await else {
        return;
    };
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database");
    // A runner on loopback, which the operator trusts.
    let received = Received::default();
    let runner = Router::new()
        .route(
            "/ok",
            post(
                |axum::extract::State(received): axum::extract::State<Received>,
                 headers: HeaderMap,
                 body: String| async move {
                    received.0.lock().expect("lock").push((headers, body));
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .route("/refuses", post(|| async { StatusCode::UNAUTHORIZED }))
        .with_state(received.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, runner).await.expect("runner");
    });
    let app = test_app_with(
        setup.pool.clone(),
        &database_url,
        &[("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())],
    );
    let agents = space(&setup).await;
    let (status, created) = send(
        &app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{agents}/external-agents"),
            None,
            json!({
                "connectionName": "Runner",
                "displayName": "Runner",
                "handle": "runner",
                "role": "member",
                "scopes": ["read", "write", "events"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let connection = created["connection"]["id"].as_str().expect("connection");
    let test_path = format!("/api/v1/projects/{agents}/external-agents/{connection}/webhook/test");
    let webhook_path = format!("/api/v1/projects/{agents}/external-agents/{connection}/webhook");

    let (status, body) = send(
        &app,
        setup.browser.request("POST", &test_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "no webhook yet: {body}");

    let (status, saved) = send(
        &app,
        setup.browser.request(
            "POST",
            &webhook_path,
            None,
            json!({"url": format!("{origin}/ok")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let secret = saved["webhookSecret"].as_str().expect("secret").to_owned();
    let (status, answer) = send(
        &app,
        setup.browser.request("POST", &test_path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["status"], 204);
    let (headers, body) = received.0.lock().expect("lock").pop().expect("a wakeup");
    let header_value = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(header_value("x-workspace-event"), "ping");
    assert_eq!(header_value("x-workspace-delivery"), answer["deliveryId"]);
    let timestamp = header_value("x-workspace-timestamp");
    assert_eq!(
        header_value("x-workspace-signature"),
        format!(
            "sha256={}",
            agent_workspace_server::replies::sign(&secret, &timestamp, &body).expect("sign")
        ),
        "signed with the webhook's own secret, like every wakeup"
    );
    let ping: Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(ping["event"], "ping");
    assert!(ping.get("replyRequestId").is_none(), "names no request");

    send(
        &app,
        setup.browser.request(
            "POST",
            &webhook_path,
            None,
            json!({"url": format!("{origin}/refuses")}),
        ),
    )
    .await;
    let (_, answer) = send(
        &app,
        setup.browser.request("POST", &test_path, None, Value::Null),
    )
    .await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["status"], 401);
    assert!(
        answer["error"]
            .as_str()
            .is_some_and(|error| error.contains("signature")),
        "{answer}"
    );
}

#[tokio::test]
async fn the_agent_guides_are_public_and_current() {
    let Some(setup) = setup("w22_guides").await else {
        return;
    };
    for (path, needle) in [
        ("/llms.txt", "/mcp-tools.md"),
        ("/mcp-tools.md", "### `skill_get`"),
    ] {
        let response = setup
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/markdown")),
            "{path}"
        );
        let body = to_bytes(response.into_body(), 4_000_000)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("UTF-8");
        assert!(text.contains(needle), "{path} lacks {needle}");
    }
}
