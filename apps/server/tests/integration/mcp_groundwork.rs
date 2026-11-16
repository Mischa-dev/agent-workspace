//! The shared task code every MCP Phase 2 tool builds on: what a write tells
//! an agent (keys, links, the way through the workflow), where a task lands
//! when its status changes, and the archive rules the web app and MCP now
//! share. The costly bugs here are silent: a reservation that outlives a
//! refused write, a claim left on an archived task, or a moved task's
//! original coming back as a live duplicate.
//!
//! Phase 3 groundwork: a copied message link names the message, not the
//! conversation its address starts with.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

fn names(statuses: &Value) -> Vec<String> {
    statuses
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {statuses}"))
        .iter()
        .map(|status| match status {
            Value::String(name) => name.clone(),
            status => status["name"].as_str().expect("status name").to_owned(),
        })
        .collect()
}

#[tokio::test]
async fn refused_status_changes_explain_the_way_and_writes_name_the_task() {
    let Some(setup) = setup("mcp_ground_flow").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");

    let created = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Offline mode", "clientRequestId": "ground-create-1"}),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let task = created["workItemId"].as_str().expect("task").to_owned();
    assert_eq!(created["key"], "WORK-1", "{created}");
    assert!(
        created["url"]
            .as_str()
            .expect("url")
            .ends_with(&format!("/projects/{project}/work-items/{task}")),
        "{created}"
    );
    let replay = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Offline mode", "clientRequestId": "ground-create-1"}),
    )
    .await;
    assert_eq!(replay["idempotentReplay"], true, "{replay}");
    assert_eq!(
        replay["key"], "WORK-1",
        "a replay carries the key: {replay}"
    );

    // Backlog can't go straight to Done: the answer names where it can go
    // and the way there, and nothing is written.
    let refused = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 1, "status": "Done", "clientRequestId": "ground-done"}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_transition", "{refused}");
    assert_eq!(
        refused["retryCategory"],
        "ask_user_or_choose_allowed_transition"
    );
    assert!(refused["currentStatusId"].is_string(), "{refused}");
    assert_eq!(refused["currentStatus"], "Backlog", "{refused}");
    assert_eq!(refused["requestedStatus"], "Done", "{refused}");
    assert_eq!(names(&refused["allowedStatuses"]), ["Ready", "Canceled"]);
    assert_eq!(
        names(&refused["path"]),
        ["Ready", "In Progress", "Review", "Done"]
    );
    assert!(
        refused["message"]
            .as_str()
            .expect("message")
            .contains("Backlog can't move straight to Done"),
        "{refused}"
    );
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task.parse::<Uuid>().expect("task UUID"))
        .fetch_one(&setup.pool)
        .await
        .expect("version");
    assert_eq!(version, 1);

    // The refused call's reservation rolled back, so a corrected retry can
    // reuse its clientRequestId.
    let ready = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 1, "status": "Ready", "clientRequestId": "ground-done"}),
    )
    .await;
    assert_eq!(ready["outcome"], "applied", "{ready}");
    assert_eq!(ready["version"], 2);
    assert_eq!(ready["key"], "WORK-1");

    let review = tool(
        app,
        &codex,
        &session,
        "task_mark_ready_for_review",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "clientRequestId": "ground-review"}),
    )
    .await;
    assert_eq!(review["outcome"], "invalid_transition", "{review}");
    assert_eq!(names(&review["path"]), ["In Progress", "Review"]);

    let stale = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 1, "title": "Lost", "clientRequestId": "ground-stale"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert_eq!(stale["retryCategory"], "refetch_then_decide");
    assert_eq!(
        stale["message"],
        "Someone changed WORK-1 first (now version 2). Open it again with task_open, then retry with expectedVersion 2."
    );

    let unknown = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "status": "Doing"}),
    )
    .await;
    assert_eq!(unknown["outcome"], "invalid_request", "{unknown}");
    assert_eq!(unknown["field"], "status");
    assert_eq!(unknown["retryCategory"], "correct_input");
    assert!(
        unknown["message"]
            .as_str()
            .expect("message")
            .starts_with("No status called \"Doing\" in WORK. Statuses: Backlog, Ready"),
        "{unknown}"
    );

    // The audit log keeps request IDs up to 128 characters; a longer one is
    // refused up front instead of failing the write.
    let too_long = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "title": "Renamed", "clientRequestId": "r".repeat(129)}),
    )
    .await;
    assert_eq!(too_long["outcome"], "invalid_request", "{too_long}");
    assert_eq!(too_long["field"], "clientRequestId");
    let longest = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "title": "Renamed", "clientRequestId": "r".repeat(128)}),
    )
    .await;
    assert_eq!(longest["outcome"], "applied", "{longest}");
    assert_eq!(longest["version"], 3);

    let comment = tool(
        app,
        &codex,
        &session,
        "task_comment",
        json!({"taskId": "WORK-1", "text": "Picked this up.", "clientRequestId": "ground-comment"}),
    )
    .await;
    assert_eq!(comment["outcome"], "applied", "{comment}");
    assert_eq!(comment["key"], "WORK-1");
    assert_eq!(comment["url"], created["url"]);
}

#[tokio::test]
async fn a_task_whose_status_changes_goes_to_the_bottom_of_its_new_column() {
    let Some(setup) = setup("mcp_ground_board").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    // Created first, so it has the lowest rank in Backlog.
    for title in ["Last into Ready", "First into Ready", "Second into Ready"] {
        let created = tool(
            app,
            &codex,
            &session,
            "task_create",
            json!({"projectId": "WORK", "title": title}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    for key in ["WORK-2", "WORK-3", "WORK-1"] {
        let moved = tool(
            app,
            &codex,
            &session,
            "task_update",
            json!({"taskId": key, "expectedVersion": 1, "status": "Ready"}),
        )
        .await;
        assert_eq!(moved["outcome"], "applied", "{moved}");
        assert_eq!(moved["changedFields"], json!(["status"]), "{moved}");
    }
    let column = tool(
        app,
        &codex,
        &session,
        "task_list",
        json!({"projectId": "WORK", "status": "Ready"}),
    )
    .await;
    let order = column["value"]["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["key"].as_str().expect("key").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(order, ["WORK-2", "WORK-3", "WORK-1"], "{column}");

    // History can tell where it came from.
    let payload = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT activity.payload
        FROM work_item_activity activity
        JOIN task_rows item ON item.id = activity.work_item_id
        WHERE item.number = 1 AND activity.kind = 'work_item_updated'
        ORDER BY activity.created_at DESC
        LIMIT 1
        "#,
    )
    .fetch_one(&setup.pool)
    .await
    .expect("activity");
    assert_eq!(payload["changedFields"], json!(["status"]), "{payload}");
    assert!(payload["previousStatusId"].is_string(), "{payload}");
    assert!(payload["statusId"].is_string(), "{payload}");
}

#[tokio::test]
async fn archiving_releases_claims_and_moved_originals_stay_archived() {
    let Some(setup) = setup("mcp_ground_archive").await else {
        return;
    };
    let app = &setup.app;
    let old = create_project(&setup, "OLD", "Old work").await;
    create_project(&setup, "NEW", "New work").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    // Mara works only in the old project, so she can't follow a moved task.
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (status, added) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{old}/members"),
            None,
            json!({"actorId": mara, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");

    let created = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "OLD", "title": "Mirror private repositories"}),
    )
    .await;
    let task = created["workItemId"].as_str().expect("task").to_owned();
    let claimed = tool(
        app,
        &codex,
        &session,
        "task_claim",
        json!({"taskId": "OLD-1"}),
    )
    .await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");

    // Archiving in the web app lets go of the claim.
    let base = format!("/api/v1/projects/{old}/work-items/{task}");
    let (status, archived) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("{base}/archive"),
            Some("ground-archive"),
            json!({"expectedVersion": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["version"], 2);
    let claims = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_claims")
        .fetch_one(&setup.pool)
        .await
        .expect("claims");
    assert_eq!(claims, 0);
    let (status, again) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("{base}/archive"),
            Some("ground-archive-again"),
            json!({"expectedVersion": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "already archived: {again}");
    assert_eq!(again["version"], 2);
    let (status, restored) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("{base}/restore"),
            Some("ground-restore"),
            json!({"expectedVersion": 2}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["version"], 3);

    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "OLD-1", "toProjectId": "NEW"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let new_task = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("new task")
        .to_owned();

    // The original stays archived: restoring it would make a live copy
    // whose old key opens the moved task.
    let original_version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
            .bind(task.parse::<Uuid>().expect("task UUID"))
            .fetch_one(&setup.pool)
            .await
            .expect("original version");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("{base}/restore"),
            Some("ground-restore-moved"),
            json!({"expectedVersion": original_version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["code"], "conflict", "{refused}");
    let still_archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE id = $1",
    )
    .bind(task.parse::<Uuid>().expect("task UUID"))
    .fetch_one(&setup.pool)
    .await
    .expect("archived");
    assert!(still_archived);

    // Task lists the middleware doesn't rewrite still follow the old key to
    // the moved task for someone who can reach it...
    let reordered = tool(
        app,
        &codex,
        &session,
        "task_reorder",
        json!({"taskIds": ["OLD-1"], "position": "top"}),
    )
    .await;
    assert_eq!(reordered["outcome"], "ok", "{reordered}");
    assert_eq!(reordered["value"]["column"][0], "NEW-1", "{reordered}");
    // ...and to the archived original for someone who can't.
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara_claim = tool(
        app,
        &mara_token,
        &mara_session,
        "task_claim",
        json!({"taskId": "OLD-1"}),
    )
    .await;
    assert_eq!(mara_claim["outcome"], "conflict", "{mara_claim}");
    assert!(
        mara_claim["message"]
            .as_str()
            .expect("message")
            .contains("archived"),
        "{mara_claim}"
    );
    let direct = tool(
        app,
        &mara_token,
        &mara_session,
        "task_claim",
        json!({"taskId": new_task}),
    )
    .await;
    assert_ne!(direct["outcome"], "ok", "{direct}");
}

/// The web app links a message as `<conversation address>#message-<id>`. Every
/// tool that takes a message reads such a link as the message, and so does
/// the middleware that picks the project from it: read as an ordinary link it
/// would name the conversation, and an account-wide app would get "not found"
/// or be asked which project it meant.
#[tokio::test]
async fn message_links_name_the_message_everywhere() {
    let Some(setup) = setup("mcp_ground_links").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let channel = call(
        "channel_create",
        json!({"projectId": "WORK", "name": "design"}),
    )
    .await;
    let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
    let sent = call(
        "message_send",
        json!({"conversationId": channel_id, "text": "Let's use design tokens."}),
    )
    .await;
    let message_id = sent["value"]["messageId"]
        .as_str()
        .expect("message")
        .to_owned();
    let link = format!(
        "https://workspace.example/projects/{project}/channels/{channel_id}#message-{message_id}"
    );

    // Only the link says where: the project comes from the message.
    let decided = call(
        "decision_record",
        json!({"title": "Use design tokens", "fromMessageId": link}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    assert_eq!(decided["value"]["projectId"], project.as_str(), "{decided}");
    let source = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT source_message_id FROM project_decisions WHERE id = $1::uuid",
    )
    .bind(decided["value"]["id"].as_str().expect("decision"))
    .fetch_one(&setup.pool)
    .await
    .expect("decision source");
    assert_eq!(source.map(|id| id.to_string()), Some(message_id.clone()));

    let pinned = call(
        "pin_set",
        json!({"conversationId": channel_id, "messageId": link}),
    )
    .await;
    assert_eq!(pinned["outcome"], "ok", "{pinned}");
    assert_eq!(pinned["value"]["id"], message_id.as_str(), "{pinned}");

    let around = call(
        "conversation_read",
        json!({"conversationId": channel_id, "aroundMessageId": link}),
    )
    .await;
    assert_eq!(around["outcome"], "ok", "{around}");
    assert!(
        around["value"]["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .any(|message| message["messageId"] == message_id.as_str()),
        "{around}"
    );

    let thread = call(
        "thread_start",
        json!({"conversationId": channel_id, "fromMessageId": link}),
    )
    .await;
    assert_eq!(thread["outcome"], "ok", "{thread}");
    assert_eq!(thread["value"]["created"], true, "{thread}");
    let again = call(
        "thread_start",
        json!({"conversationId": channel_id, "fromMessageId": message_id}),
    )
    .await;
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(again["value"]["id"], thread["value"]["id"], "{again}");

    let created = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Adopt design tokens"}),
    )
    .await;
    assert_eq!(created["outcome"], "applied", "{created}");
    let linked = call("task_link", json!({"taskId": "WORK-1", "messageId": link})).await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    assert_eq!(linked["value"]["kind"], "message", "{linked}");
    assert_eq!(linked["value"]["targetId"], message_id.as_str(), "{linked}");

    // A link whose message part is broken is refused, never read as the
    // conversation.
    let broken = format!("/projects/{project}/channels/{channel_id}#message-nope");
    let refused = call(
        "pin_set",
        json!({"conversationId": channel_id, "messageId": broken}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
}
