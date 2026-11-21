//! Project admin through MCP (docs/plans/mcp-tools.md, Phase 1): the costly
//! parts are who may change a project, and that an archived project can be
//! brought back.

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::support::app::{
    connect_app, create_agent, creates_once, initialize, send, test_app_with, tool,
};

pub(super) use super::support::world::{Setup, create_project, setup};

#[tokio::test]
async fn creating_a_project_again_creates_it_once() {
    let Some(setup) = setup("mcp_project_retry").await else {
        return;
    };
    let app = &setup.app;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    creates_once(
        app,
        &setup.pool,
        &codex,
        &session,
        "space_create",
        json!({"key": "WEB", "name": "Website", "clientRequestId": "project-retry-1"}),
        json!({"key": "APP", "name": "App", "clientRequestId": "project-retry-1"}),
        "SELECT count(*) FROM projects",
    )
    .await;
}

#[tokio::test]
async fn only_owners_change_projects_and_archived_ones_come_back() {
    let Some(setup) = setup("mcp_admin").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let old = create_project(&setup, "OLD", "Old work").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["workspaceSpaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (status, added) = setup
        .browser
        .op(
            app,
            "space_member_add",
            json!({"spaceId": work, "account": mara, "role": "member"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");

    // A project key works wherever a project ID does.
    let workflow = tool(
        app,
        &codex,
        &session,
        "workflow_get",
        json!({"projectId": "work"}),
    )
    .await;
    assert_eq!(workflow["outcome"], "ok", "{workflow}");
    let statuses = workflow["value"]["statuses"].as_array().expect("statuses");
    assert!(
        statuses
            .iter()
            .any(|status| status["category"] == "completed"),
        "{workflow}"
    );
    assert!(
        !workflow["value"]["transitions"]
            .as_array()
            .expect("transitions")
            .is_empty()
    );

    // Owners see members and roles; agents are pointed at people_search.
    let members = tool(
        app,
        &codex,
        &session,
        "project_members",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(members["outcome"], "ok", "{members}");
    assert!(
        members["value"]["members"]
            .as_array()
            .expect("members")
            .iter()
            .any(|member| member["handle"] == "mara" && member["kind"] == "agent"),
        "{members}"
    );
    let hidden = tool(
        app,
        &mara_token,
        &mara_session,
        "project_members",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(hidden["outcome"], "permission_denied", "{hidden}");

    // Renaming: the owner can, an agent and a read-only app can't.
    for (token, session) in [(&mara_token, &mara_session), (&reader, &reader_session)] {
        let refused = tool(
            app,
            token,
            session,
            "space_update",
            json!({"spaceId": work, "name": "Taken over"}),
        )
        .await;
        assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    }
    let renamed = tool(
        app,
        &codex,
        &session,
        "space_update",
        json!({"spaceId": "WORK", "name": "Agent Workspace"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    assert_eq!(renamed["value"]["name"], "Agent Workspace");
    assert_eq!(renamed["value"]["key"], "WORK");
    let stale = tool(
        app,
        &codex,
        &session,
        "space_update",
        json!({"spaceId": work, "name": "Again", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Archiving: agents and read-only apps can't; the owner can, and can
    // bring it back by id even though archived projects are hidden.
    let refused = tool(
        app,
        &mara_token,
        &mara_session,
        "space_archive",
        json!({"spaceId": work}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "space_archive",
        json!({"spaceId": old}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let archived = tool(
        app,
        &codex,
        &session,
        "space_archive",
        json!({"spaceId": "OLD"}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["archived"], true);
    let listed = tool(app, &codex, &session, "projects_list", json!({})).await;
    assert!(
        !listed["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str()),
        "{listed}"
    );
    let with_archived = tool(
        app,
        &codex,
        &session,
        "projects_list",
        json!({"includeArchived": true}),
    )
    .await;
    assert!(
        with_archived["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str() && project["archived"] == true),
        "{with_archived}"
    );
    // Archived, it is out of reach of everything but space_restore.
    let again = tool(
        app,
        &codex,
        &session,
        "space_archive",
        json!({"spaceId": old}),
    )
    .await;
    assert_eq!(again["outcome"], "not_found", "{again}");
    let restored = tool(
        app,
        &codex,
        &session,
        "space_restore",
        json!({"spaceId": old}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["archived"], false);
    let listed = tool(app, &codex, &session, "projects_list", json!({})).await;
    assert!(
        listed["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str()),
        "{listed}"
    );
}

#[tokio::test]
async fn channels_threads_pins_and_decisions_can_be_shaped() {
    let Some(setup) = setup("mcp_channels").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");

    // Creating a channel twice returns the same one.
    let design = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": "WORK", "name": "design"}),
    )
    .await;
    assert_eq!(design["outcome"], "ok", "{design}");
    assert_eq!(design["value"]["created"], true);
    let design_id = design["value"]["id"].as_str().expect("channel").to_owned();
    let again = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": work, "name": "Design"}),
    )
    .await;
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(again["value"]["id"], design_id.as_str());
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "channel_create",
        json!({"projectId": work, "name": "lurking"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A message, a thread from it, and the thread's status and outcome.
    let sent = tool(
        app,
        &codex,
        &session,
        "message_send",
        json!({"conversationId": design_id, "text": "Let's use design tokens."}),
    )
    .await;
    let message_id = sent["value"]["messageId"]
        .as_str()
        .expect("message")
        .to_owned();
    let thread = tool(
        app,
        &codex,
        &session,
        "thread_start",
        json!({"conversationId": design_id, "fromMessageId": message_id}),
    )
    .await;
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
    let decided = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": thread_id, "status": "decided", "outcome": "Tokens it is"}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    assert_eq!(decided["value"]["status"], "Decided");
    assert_eq!(decided["value"]["outcome"], "Tokens it is");
    let wrong_kind = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "status": "Done"}),
    )
    .await;
    assert_eq!(wrong_kind["outcome"], "invalid_request", "{wrong_kind}");

    // Renaming, and a name that's taken.
    let renamed = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "name": "design-system"}),
    )
    .await;
    assert_eq!(renamed["value"]["name"], "design-system", "{renamed}");
    let other = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": work, "name": "other"}),
    )
    .await;
    let other_id = other["value"]["id"].as_str().expect("other").to_owned();
    let taken = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": other_id, "name": "design-system"}),
    )
    .await;
    assert_eq!(taken["outcome"], "conflict", "{taken}");

    // Archive a channel, see it leave the list, and restore it.
    let archived = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "archived": true}),
    )
    .await;
    assert_eq!(archived["value"]["archived"], true, "{archived}");
    let channels = tool(
        app,
        &codex,
        &session,
        "conversation_list",
        json!({"projectId": work, "kind": "channel"}),
    )
    .await;
    assert!(
        !channels["value"]["conversations"]
            .as_array()
            .expect("conversations")
            .iter()
            .any(|conversation| conversation["id"] == design_id.as_str()),
        "{channels}"
    );
    let restored = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "archived": false, "name": "design"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["archived"], false);
    assert_eq!(restored["value"]["name"], "design");

    // Pins: pin the message, then unpin it.
    let pinned = tool(
        app,
        &codex,
        &session,
        "pin_set",
        json!({"conversationId": design_id, "messageId": message_id}),
    )
    .await;
    assert_eq!(pinned["outcome"], "ok", "{pinned}");
    assert_eq!(pinned["value"]["pins"].as_array().expect("pins").len(), 1);
    let unpinned = tool(
        app,
        &codex,
        &session,
        "pin_set",
        json!({"conversationId": design_id, "messageId": message_id, "pinned": false}),
    )
    .await;
    assert_eq!(
        unpinned["value"]["pins"].as_array().expect("pins").len(),
        0,
        "{unpinned}"
    );

    // Decisions: a superseded one drops out unless asked for.
    let first = tool(
        app,
        &codex,
        &session,
        "decision_record",
        json!({"projectId": work, "title": "Use CSS variables", "conversationId": design_id}),
    )
    .await;
    let first_id = first["value"]["id"].as_str().expect("decision").to_owned();
    let second = tool(
        app,
        &codex,
        &session,
        "decision_record",
        json!({"projectId": work, "title": "Use design tokens", "supersedes": first_id}),
    )
    .await;
    assert_eq!(second["outcome"], "ok", "{second}");
    let current = tool(
        app,
        &codex,
        &session,
        "decision_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    let titles = current["value"]["decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .map(|decision| decision["title"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Use design tokens"], "{current}");
    let everything = tool(
        app,
        &codex,
        &session,
        "decision_list",
        json!({"projectId": work, "includeSuperseded": true}),
    )
    .await;
    let superseded = everything["value"]["decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .find(|decision| decision["id"] == first_id.as_str())
        .cloned()
        .expect("superseded decision");
    assert_eq!(
        superseded["supersededByDecisionId"], second["value"]["id"],
        "{everything}"
    );
}

#[tokio::test]
async fn owners_add_change_and_remove_members() {
    let Some(setup) = setup("mcp_members").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["workspaceSpaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (_, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");

    let set = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let added = set(
        "space_member_add",
        json!({"spaceId": work, "account": "@mara"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(added["value"]["role"], "member");
    let again = set(
        "space_member_add",
        json!({"spaceId": work, "account": "mara"}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "adding twice is harmless: {again}");
    // An agent holds any role a person can (K1.14a).
    let admin = set(
        "space_member_update",
        json!({"spaceId": work, "account": "@mara", "role": "admin"}),
    )
    .await;
    assert_eq!(admin["value"]["role"], "admin", "{admin}");
    let viewer = set(
        "space_member_update",
        json!({"spaceId": work, "account": "@mara", "role": "viewer"}),
    )
    .await;
    assert_eq!(viewer["value"]["role"], "viewer", "{viewer}");
    let own = set(
        "space_member_remove",
        json!({"spaceId": work, "account": "@owner"}),
    )
    .await;
    assert_ne!(own["outcome"], "ok", "{own}");

    // Mara can't manage members, and loses access the moment she's removed.
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let hers = tool(
        app,
        &mara_token,
        &mara_session,
        "space_member_update",
        json!({"spaceId": work, "account": "@owner", "role": "viewer"}),
    )
    .await;
    assert_eq!(hers["outcome"], "permission_denied", "{hers}");
    let before = tool(
        app,
        &mara_token,
        &mara_session,
        "workflow_get",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(before["outcome"], "ok", "{before}");
    let removed = set(
        "space_member_remove",
        json!({"spaceId": work, "account": "@mara"}),
    )
    .await;
    assert_eq!(removed["value"]["active"], false, "{removed}");
    let after = tool(
        app,
        &mara_token,
        &mara_session,
        "workflow_get",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(after["outcome"], "ok", "{after}");

    // Giving her a role again brings her back.
    let back = set(
        "space_member_update",
        json!({"spaceId": work, "account": "@mara", "role": "member"}),
    )
    .await;
    assert_eq!(back["value"]["active"], true, "{back}");
    assert_eq!(back["value"]["role"], "member");
}

#[tokio::test]
async fn moved_tasks_carry_over_and_old_keys_follow_them() {
    let Some(setup) = setup("mcp_move").await else {
        return;
    };
    let app = &setup.app;
    let old = create_project(&setup, "OLD", "Old work").await;
    let new = create_project(&setup, "NEW", "New work").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["workspaceSpaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    // Mara works only in the old project, so she can't follow the task.
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (status, added) = setup
        .browser
        .op(
            app,
            "space_member_add",
            json!({"spaceId": old, "account": mara, "role": "member"}),
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
        json!({
            "projectId": "OLD",
            "title": "Private repositories can't prepare mirrors",
            "description": "Steps:\n1. Add a private repo\n2. Watch it fail",
            "priority": "urgent",
            "assignee": "@mara"
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let task = created["value"]["workItemId"]
        .as_str()
        .expect("task")
        .to_owned();
    let subtask = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"parentId": task, "title": "Write a failing test"}),
    )
    .await;
    assert_eq!(subtask["outcome"], "ok", "{subtask}");
    let commented = tool(
        app,
        &codex,
        &session,
        "task_comment",
        json!({"taskId": "OLD-1", "text": "Repro: the clone needs a token.", "clientRequestId": "c1"}),
    )
    .await;
    assert_eq!(
        commented["outcome"], "applied",
        "keys work in task_comment: {commented}"
    );
    let started = tool(
        app,
        &codex,
        &session,
        "task_update",
        json!({"taskId": task, "expectedVersion": 1, "status": "Ready"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let (status, criterion) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/task_criteria_update",
            Some("criterion-1"),
            json!({
                "taskId": task,
                "expectedVersion": started["value"]["version"],
                "operations": [{"op": "add", "text": "Mirrors prepare with a token"}]
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{criterion}");

    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "OLD-1", "toProjectId": "NEW"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let moved_tasks = moved["value"]["moved"].as_array().expect("moved");
    assert_eq!(moved_tasks.len(), 2, "the subtask moves too: {moved}");
    assert_eq!(moved_tasks[0]["from"], "OLD-1");
    assert_eq!(moved_tasks[0]["to"], "NEW-1");
    assert_eq!(moved_tasks[1]["from"], "OLD-2");
    assert!(
        moved["value"]["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().unwrap_or_default().contains("Mara")),
        "{moved}"
    );
    let new_task = moved_tasks[0]["id"].as_str().expect("new task").to_owned();

    // The old key opens the new task, with everything carried over.
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "OLD-1"}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let carried = &opened["value"]["task"];
    assert_eq!(carried["id"], new_task.as_str(), "{opened}");
    assert_eq!(carried["projectId"], new.as_str());
    assert_eq!(
        carried["description"],
        "Steps:\n1. Add a private repo\n2. Watch it fail"
    );
    assert_eq!(carried["priority"], "urgent");
    assert_eq!(carried["status"]["name"], "Ready", "{opened}");
    assert!(carried["assignee"].is_null(), "{opened}");
    assert_eq!(
        carried["acceptanceCriteria"][0]["text"], "Mirrors prepare with a token",
        "{opened}"
    );
    assert_eq!(
        opened["value"]["subtasks"].as_array().map(Vec::len),
        Some(1),
        "{opened}"
    );
    let comments = opened["value"]["comments"].to_string();
    assert!(comments.contains("Moved from OLD-1"), "{comments}");
    assert!(
        comments.contains("Repro: the clone needs a token."),
        "{comments}"
    );

    // The original is archived and gone from the old project's board.
    let board = tool(
        app,
        &codex,
        &session,
        "task_list",
        json!({"projectId": old}),
    )
    .await;
    assert_eq!(
        board["value"]["tasks"].as_array().map(Vec::len),
        Some(0),
        "{board}"
    );

    // Moving again returns where it went instead of copying it twice.
    let again = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": task, "toProjectId": new}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["created"], false);
    assert_eq!(again["value"]["moved"][0]["id"], new_task.as_str());
    assert_eq!(
        again["value"]["moved"], moved["value"]["moved"],
        "the replay lists the subtask too: {again}"
    );

    // Mara isn't in the new project, so the old key opens what she could
    // always see: the archived original, pointing at where it went.
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let original = tool(
        app,
        &mara_token,
        &mara_session,
        "task_open",
        json!({"taskId": "OLD-1"}),
    )
    .await;
    assert_eq!(original["value"]["task"]["id"], task.as_str(), "{original}");
    assert_eq!(original["value"]["task"]["archived"], true);
    let direct = tool(
        app,
        &mara_token,
        &mara_session,
        "task_open",
        json!({"taskId": new_task}),
    )
    .await;
    assert_ne!(direct["outcome"], "ok", "{direct}");
}

#[tokio::test]
async fn dependencies_refuse_cycles_and_milestones_track_progress() {
    let Some(setup) = setup("mcp_planning").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "PLAN", "Plan").await;
    let other = create_project(&setup, "ELSE", "Elsewhere").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for (project, title) in [
        (&work, "Design"),
        (&work, "Build"),
        (&work, "Ship"),
        (&other, "Elsewhere"),
    ] {
        let created = call("task_create", json!({"projectId": project, "title": title})).await;
        assert_eq!(created["outcome"], "ok", "{created}");
    }

    // Ship waits on Build, Build on Design.
    let shipped = call(
        "task_depend",
        json!({"taskId": "PLAN-3", "blockedBy": ["PLAN-2"]}),
    )
    .await;
    assert_eq!(shipped["outcome"], "ok", "{shipped}");
    assert_eq!(shipped["value"]["ready"], false);
    let built = call(
        "task_depend",
        json!({"taskId": "PLAN-2", "blockedBy": ["PLAN-1"]}),
    )
    .await;
    assert_eq!(built["value"]["blocks"][0]["key"], "PLAN-3", "{built}");
    // Design waiting on Ship would close a cycle.
    let cycle = call(
        "task_depend",
        json!({"taskId": "PLAN-1", "blockedBy": ["PLAN-3"]}),
    )
    .await;
    assert_eq!(cycle["outcome"], "conflict", "{cycle}");
    assert!(
        cycle["message"]
            .as_str()
            .is_some_and(|text| text.contains("PLAN-1 blocks PLAN-2 blocks PLAN-3")),
        "{cycle}"
    );
    // A blocker may be in another project of the same workspace.
    let elsewhere = call(
        "task_depend",
        json!({"taskId": "PLAN-1", "blocks": ["ELSE-1"]}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "ok", "{elsewhere}");
    assert_eq!(
        elsewhere["value"]["blocks"][0]["key"], "ELSE-1",
        "{elsewhere}"
    );
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "task_depend",
        json!({"taskId": "PLAN-1", "blocks": ["PLAN-3"]}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let removed = call(
        "task_depend",
        json!({"taskId": "PLAN-3", "blockedBy": ["PLAN-2"], "remove": true}),
    )
    .await;
    assert_eq!(removed["value"]["ready"], true, "{removed}");

    // A milestone with a target date holds the tasks and counts progress.
    let milestone = call(
        "milestone_write",
        json!({"projectId": "PLAN", "name": "v1", "targetDate": "2020-01-01", "addTasks": ["PLAN-1", "PLAN-2", "PLAN-3"]}),
    )
    .await;
    assert_eq!(milestone["outcome"], "ok", "{milestone}");
    assert_eq!(milestone["value"]["total"], 3);
    assert_eq!(milestone["value"]["overdue"], true);
    let again = call("milestone_write", json!({"projectId": work, "name": "V1"})).await;
    assert_eq!(again["value"]["id"], milestone["value"]["id"], "{again}");
    let wrong = call(
        "milestone_write",
        json!({"projectId": work, "name": "v2", "addTasks": ["ELSE-1"]}),
    )
    .await;
    assert_eq!(wrong["outcome"], "invalid_request", "{wrong}");
    let trimmed = call(
        "milestone_write",
        json!({"projectId": work, "milestoneId": milestone["value"]["id"], "removeTasks": ["PLAN-3"], "targetDate": ""}),
    )
    .await;
    assert_eq!(trimmed["value"]["total"], 2, "{trimmed}");
    assert_eq!(trimmed["value"]["overdue"], false);
    let listed = call("milestone_list", json!({"projectId": "PLAN"})).await;
    assert_eq!(listed["value"]["milestones"][0]["name"], "v1", "{listed}");

    // A canceled task leaves the milestone's scope instead of counting as
    // done: one done and one canceled reads "1 of 1 done", not "2 of 2".
    for (key, category) in [(1_i64, "completed"), (2_i64, "canceled")] {
        sqlx::query(
            "UPDATE work_items item SET status_id = status.id \
             FROM work_item_statuses status \
             WHERE item.project_id = $1 AND item.number = $2 \
               AND status.project_id = item.project_id AND status.semantic_category = $3",
        )
        .bind(work.parse::<Uuid>().expect("project id"))
        .bind(key)
        .bind(category)
        .execute(&setup.pool)
        .await
        .expect("set the status");
    }
    let progress = call("milestone_list", json!({"projectId": "PLAN"})).await;
    let v1 = &progress["value"]["milestones"][0];
    assert_eq!(v1["total"], 1, "{progress}");
    assert_eq!(v1["finished"], 1, "{progress}");
    assert_eq!(v1["canceled"], 1, "{progress}");
    assert_eq!(v1["open"], 0, "{progress}");
    assert_eq!(v1["progress"], 1.0, "{progress}");
}

#[tokio::test]
async fn moving_an_archived_original_does_not_reveal_its_hidden_destination() {
    let Some(setup) = setup("mcp_move_hidden").await else {
        return;
    };
    let source = create_project(&setup, "SOURCE", "Source").await;
    let secret = create_project(&setup, "SECRET", "Secret").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (_, reader) = super::message_edits::add_person(&setup, &[&source, &other], "reader").await;
    let owner_token = connect_app(&setup.app, &setup.browser, "Owner app", "full").await;
    let owner_session = initialize(&setup.app, &owner_token)
        .await
        .expect("owner session");
    let created = tool(
        &setup.app,
        &owner_token,
        &owner_session,
        "task_create",
        json!({"projectId": source, "title": "Move me"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let original = created["value"]["workItemId"].clone();
    let moved = tool(
        &setup.app,
        &owner_token,
        &owner_session,
        "task_move",
        json!({"taskId": original, "toProjectId": secret}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let destination = moved["value"]["moved"][0]["id"].clone();
    let reader_token = connect_app(&setup.app, &reader, "Reader app", "full").await;
    let reader_session = initialize(&setup.app, &reader_token)
        .await
        .expect("reader session");
    let result = tool(
        &setup.app,
        &reader_token,
        &reader_session,
        "task_move",
        json!({"taskId": original, "toProjectId": other}),
    )
    .await;
    assert_ne!(
        result["outcome"], "ok",
        "hidden destination leaked: {result}"
    );
    assert!(
        !result
            .to_string()
            .contains(destination.as_str().expect("destination")),
        "{result}"
    );
    assert!(!result.to_string().contains("SECRET-"), "{result}");
}

#[tokio::test]
async fn a_project_scoped_credential_cannot_move_tasks_outside_its_project() {
    use agent_workspace_server::mcp::{IssueCredentialCommand, issue_credential};
    let Some(setup) = setup("mcp_move_confined").await else {
        return;
    };
    let source = create_project(&setup, "SOURCE", "Source").await;
    let target = create_project(&setup, "TARGET", "Target").await;
    let actor =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let issued = issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            actor_id: actor,
            project_id: source.parse().expect("source"),
            client_name: "Confined".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("credential");
    let session = initialize(&setup.app, &issued.token)
        .await
        .expect("session");
    let created = tool(
        &setup.app,
        &issued.token,
        &session,
        "task_create",
        json!({"title": "Stay here"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let result = tool(
        &setup.app,
        &issued.token,
        &session,
        "task_move",
        json!({"taskId": created["value"]["workItemId"], "toProjectId": target}),
    )
    .await;
    assert_ne!(
        result["outcome"], "ok",
        "credential escaped its project: {result}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM work_items WHERE project_id = $1")
        .bind(target.parse::<Uuid>().expect("target"))
        .fetch_one(&setup.pool)
        .await
        .expect("target tasks");
    assert_eq!(count, 0);

    // Nor does it change the workspace through a package: not a role, and
    // not a package that lands outside its project.
    let package = json!({
        "format": "agent-workspace-package/1",
        "id": "escape",
        "version": "1.0.0",
        "name": "Escape",
        "target": "workspace",
        "contents": [{"kind": "role", "key": "boss", "body": {
            "name": "Boss", "baseRole": "admin", "permissions": ["tasks.view"]}}],
    });
    let preview = tool(
        &setup.app,
        &issued.token,
        &session,
        "package_preview",
        json!({"package": package}),
    )
    .await;
    let installed = tool(
        &setup.app,
        &issued.token,
        &session,
        "package_install",
        json!({"package": package, "planHash": preview["value"]["planHash"],
               "clientRequestId": "escape-1"}),
    )
    .await;
    assert_eq!(installed["outcome"], "permission_denied", "{installed}");
    let roles: i64 = sqlx::query_scalar("SELECT count(*) FROM workspace_roles WHERE name = 'Boss'")
        .fetch_one(&setup.pool)
        .await
        .expect("roles");
    assert_eq!(roles, 0);

    // Nor reaches into the other space through a workspace-wide module: the
    // owner is in TARGET, but this connection isn't.
    let elsewhere = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO collections (id, project_id, name, description, title_name, position, \
         created_by_actor_id) VALUES ($1, $2::uuid, 'Target list', '', 'Name', 0, $3)",
    )
    .bind(elsewhere)
    .bind(&target)
    .bind(actor)
    .execute(&setup.pool)
    .await
    .expect("collection in TARGET");
    let wide = tool(
        &setup.app,
        &issued.token,
        &session,
        "module_create",
        json!({"key": "wide", "name": "Wide", "clientRequestId": "wide-1"}),
    )
    .await;
    assert_eq!(wide["outcome"], "permission_denied", "{wide}");
    let module = tool(
        &setup.app,
        &issued.token,
        &session,
        "module_create",
        json!({"key": "reach", "name": "Reach", "spaceId": source,
               "clientRequestId": "reach-1"}),
    )
    .await;
    assert_eq!(module["outcome"], "ok", "{module}");
    let linked = tool(
        &setup.app,
        &issued.token,
        &session,
        "module_part_link",
        json!({"moduleId": module["value"]["id"],
                   "part": {"kind": "collection", "id": elsewhere.to_string()}}),
    )
    .await;
    assert_eq!(linked["outcome"], "not_found", "{linked}");
    let parts: i64 = sqlx::query_scalar("SELECT count(*) FROM module_parts WHERE object_id = $1")
        .bind(elsewhere)
        .fetch_one(&setup.pool)
        .await
        .expect("parts");
    assert_eq!(parts, 0);

    // Nor switches, changes or archives a workspace-wide module, which
    // reaches every space.
    let full = connect_app(&setup.app, &setup.browser, "Owner app", "full").await;
    let owner_session = initialize(&setup.app, &full).await.expect("session");
    let shared = tool(
        &setup.app,
        &full,
        &owner_session,
        "module_create",
        json!({"key": "shared", "name": "Shared", "clientRequestId": "shared-1"}),
    )
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let shared_id = shared["value"]["id"].clone();
    for (name, arguments) in [
        ("module_set", json!({"moduleId": shared_id, "on": false})),
        (
            "module_update",
            json!({"moduleId": shared_id, "name": "Mine",
                   "expectedRevision": shared["value"]["revision"]}),
        ),
        (
            "module_archive",
            json!({"moduleId": shared_id, "parts": "archive"}),
        ),
    ] {
        let result = tool(&setup.app, &issued.token, &session, name, arguments).await;
        assert_eq!(result["outcome"], "permission_denied", "{name}: {result}");
    }
    let untouched: (String, bool) =
        sqlx::query_as("SELECT name, archived_at IS NULL FROM modules WHERE id = $1::text::uuid")
            .bind(shared_id.as_str().expect("id"))
            .fetch_one(&setup.pool)
            .await
            .expect("module");
    assert_eq!(untouched, ("Shared".to_owned(), true));

    // Nor reads what lives in the other space, though the account is a
    // member there: its templates, its installs, or a module's part there.
    sqlx::query(
        "INSERT INTO agent_templates (id, workspace_id, space_id, template_key, name, definition) \
         SELECT $1, organization_id, id, 'target_helper', 'Target helper', \
                '{\"instructions\": \"TARGET-ONLY\"}'::jsonb \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&target)
    .execute(&setup.pool)
    .await
    .expect("template in TARGET");
    let package_row = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO packages (id, workspace_id, package_key, version, name, document, digest, source) \
         SELECT $1, organization_id, 'target_kit', '1.0.0', 'Target kit', '{}'::jsonb, 'd', 'upload' \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(package_row)
    .bind(&target)
    .execute(&setup.pool)
    .await
    .expect("package");
    sqlx::query(
        "INSERT INTO package_installs (id, workspace_id, space_id, package_id, state, plan, \
             plan_hash, summary, installed_by_actor_id) \
         SELECT $1, organization_id, id, $3, 'installed', '{}'::jsonb, 'h', \
                ARRAY['Made TARGET-ONLY'], $4 \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&target)
    .bind(package_row)
    .bind(actor)
    .execute(&setup.pool)
    .await
    .expect("install in TARGET");
    let linked = tool(
        &setup.app,
        &full,
        &owner_session,
        "module_part_link",
        json!({"moduleId": shared_id, "part": {"kind": "collection", "id": elsewhere.to_string()}}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    for (name, arguments) in [
        ("agent_template_list", json!({})),
        ("package_list", json!({})),
        ("module_open", json!({"moduleId": shared_id})),
    ] {
        let result = tool(&setup.app, &issued.token, &session, name, arguments).await;
        assert_eq!(result["outcome"], "ok", "{name}: {result}");
        let text = result.to_string();
        assert!(
            !text.contains("TARGET-ONLY")
                && !text.contains("Target list")
                && !text.contains(&elsewhere.to_string()),
            "{name} reached past its space: {result}"
        );
    }

    // A board made with no space named lands in the held space, as before.
    let board = tool(
        &setup.app,
        &issued.token,
        &session,
        "whiteboard_create",
        json!({"title": "Held board", "clientRequestId": "held-board"}),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let space: Option<Uuid> =
        sqlx::query_scalar("SELECT project_id FROM whiteboards WHERE title = 'Held board'")
            .fetch_one(&setup.pool)
            .await
            .expect("board");
    assert_eq!(space, Some(source.parse::<Uuid>().expect("source")));
}

#[tokio::test]
async fn dependency_cycles_longer_than_two_hundred_tasks_are_refused() {
    let Some(setup) = setup("mcp_long_cycle").await else {
        return;
    };
    let project = create_project(&setup, "PLAN", "Plan").await;
    let token = connect_app(&setup.app, &setup.browser, "Planner", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    let mut tasks = Vec::new();
    for batch in 0..3 {
        let count = if batch == 2 { 1 } else { 100 };
        let result = tool(&setup.app, &token, &session, "task_create_many", json!({
            "projectId": project, "clientRequestId": format!("chain-{batch}"),
            "tasks": (0..count).map(|i| json!({"title": format!("Task {}", batch * 100 + i)})).collect::<Vec<_>>()
        })).await;
        let result = super::support::long::finished(&setup.pool, result).await;
        assert_eq!(result["outcome"], "ok", "{result}");
        tasks.extend(
            result["value"]["tasks"]
                .as_array()
                .expect("tasks")
                .iter()
                .map(|task| {
                    task["id"]
                        .as_str()
                        .expect("id")
                        .parse::<Uuid>()
                        .expect("uuid")
                }),
        );
    }
    let project_id = project.parse::<Uuid>().expect("project");
    sqlx::query("INSERT INTO links (workspace_id, from_thing_id, to_thing_id, kind, origin, created_by_account_id) SELECT thing.workspace_id, chain.id, chain.next, 'blocks', 'explicit', (SELECT actor_id FROM users WHERE login_name = 'owner') FROM (SELECT id, lead(id) OVER (ORDER BY n) AS next FROM unnest($1::uuid[]) WITH ORDINALITY AS t(id, n)) chain JOIN things thing ON thing.id = chain.id WHERE chain.next IS NOT NULL")
        .bind(&tasks).execute(&setup.pool).await.expect("chain");
    let result = tool(
        &setup.app,
        &token,
        &session,
        "task_depend",
        json!({"taskId": tasks[200], "blocks": [tasks[0]]}),
    )
    .await;
    assert_eq!(
        result["outcome"], "conflict",
        "long cycle accepted: {result}"
    );
    let edges: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_dependencies WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&setup.pool)
            .await
            .expect("edges");
    assert_eq!(edges, 200);
}

#[tokio::test]
async fn task_moves_lock_projects_in_id_order() {
    let Some(setup) = setup("mcp_move_locks").await else {
        return;
    };
    let low = create_project(&setup, "LOW", "Low").await;
    let high = create_project(&setup, "HIGH", "High").await;
    assert!(low < high);
    let token = connect_app(&setup.app, &setup.browser, "Mover", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    let created = tool(
        &setup.app,
        &token,
        &session,
        "task_create",
        json!({"projectId": high, "title": "Move down"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let mut blocker = setup.pool.begin().await.expect("blocker");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("pid");
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(low.parse::<Uuid>().expect("low"))
        .execute(&mut *blocker)
        .await
        .expect("lock low");
    let app = setup.app.clone();
    let moving = tokio::spawn(async move {
        tool(
            &app,
            &token,
            &session,
            "task_move",
            json!({"taskId": created["value"]["workItemId"], "toProjectId": low}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&setup.pool).await.expect("waiter");
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("move waits for low project");
    let mut observer = setup.pool.begin().await.expect("observer");
    let high_free = sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(high.parse::<Uuid>().expect("high"))
        .execute(&mut *observer)
        .await
        .is_ok();
    observer.rollback().await.expect("release observer");
    blocker.rollback().await.expect("release blocker");
    let result = moving.await.expect("move");
    assert_eq!(result["outcome"], "ok", "{result}");
    assert!(
        high_free,
        "the move locked HIGH before LOW; opposite moves can deadlock"
    );
}

#[tokio::test]
async fn project_admin_rechecks_roles_after_waiting_for_the_project_lock() {
    let Some(setup) = setup("admin_revocation").await else {
        return;
    };
    let project = create_project(&setup, "RACE", "Original name")
        .await
        .parse::<Uuid>()
        .expect("project");
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    for tool_name in ["space_update", "space_archive"] {
        sqlx::query("UPDATE project_memberships SET role = 'owner' WHERE project_id = $1")
            .bind(project)
            .execute(&setup.pool)
            .await
            .expect("restore role");
        let mut revocation = setup.pool.begin().await.expect("revocation");
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *revocation)
            .await
            .expect("pid");
        sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
            .bind(project)
            .execute(&mut *revocation)
            .await
            .expect("lock project");
        sqlx::query("UPDATE project_memberships SET role = 'member' WHERE project_id = $1")
            .bind(project)
            .execute(&mut *revocation)
            .await
            .expect("demote");
        let app = setup.app.clone();
        let token = token.clone();
        let session = session.clone();
        let pending = tokio::spawn(async move {
            let args = if tool_name == "space_update" {
                json!({"spaceId": project, "name": "Forbidden rename"})
            } else {
                json!({"spaceId": project})
            };
            tool(&app, &token, &session, tool_name, args).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))")
                    .bind(pid).fetch_one(&setup.pool).await.expect("waiter");
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("admin command waits for revocation");
        revocation.commit().await.expect("revoked");
        let result = pending.await.expect("command result");
        assert_eq!(
            result["outcome"], "permission_denied",
            "{tool_name} used obsolete authority: {result}"
        );
        let unchanged: bool = sqlx::query_scalar("SELECT name = 'Original name' AND archived_at IS NULL AND version = 1 FROM projects WHERE id = $1")
            .bind(project).fetch_one(&setup.pool).await.expect("project");
        assert!(unchanged, "{tool_name} changed the project after demotion");
    }
}

#[tokio::test]
async fn space_archive_checks_expected_version() {
    let Some(setup) = setup("archive_version").await else {
        return;
    };
    let project = create_project(&setup, "VERS", "Versioned").await;
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let session = initialize(&setup.app, &token).await.expect("session");
    let result = tool(
        &setup.app,
        &token,
        &session,
        "space_archive",
        json!({"spaceId": project, "expectedVersion": 999}),
    )
    .await;
    assert_eq!(result["outcome"], "conflict", "{result}");
}

/// AWD-11: a fresh project, made through the web app with no seed data,
/// becomes execution-ready when its first person connects their Codex
/// account; doing it again, or by a second person, never duplicates setup.
#[tokio::test]
async fn a_fresh_project_gets_its_worker_when_someone_connects_codex() {
    let Some(setup) = setup("fresh_execution").await else {
        return;
    };
    let project = create_project(&setup, "FRESH", "Fresh").await;
    let temporary = tempfile::tempdir().expect("temporary data root");
    let data_root = temporary.path().join("data");
    let app = test_app_with(
        setup.pool.clone(),
        &std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database"),
        &[
            (
                "AGENT_WORKSPACE_DATA_ROOT",
                data_root.to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_PROVIDER_HOME_ROOT",
                data_root
                    .join("codex/profiles")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ],
    );
    let uri = format!("/api/v1/projects/{project}/execution-profiles");
    let (status, created) = send(
        &app,
        setup
            .browser
            .request("POST", &uri, Some("connect-1"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, again) = send(
        &app,
        setup
            .browser
            .request("POST", &uri, Some("connect-2"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], created["id"]);
    let project_id = project.parse::<Uuid>().expect("project");
    let counts = |pool: PgPool| async move {
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
            SELECT (SELECT count(*) FROM provider_connections WHERE project_id = $1),
                   (SELECT count(*) FROM execution_workers WHERE project_id = $1),
                   (SELECT count(*) FROM execution_workers WHERE project_id = $1 AND enabled),
                   (SELECT count(*) FROM project_memberships
                     WHERE project_id = $1 AND role = 'worker' AND lower(handle) = 'agent')
            "#,
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .expect("setup counts")
    };
    assert_eq!(
        counts(setup.pool.clone()).await,
        (1, 1, 0, 1),
        "one provider and one @agent worker, waiting for a signed-in profile"
    );
    // Signing in to Codex makes the profile available and turns the worker on.
    let profile_id = created["id"]
        .as_str()
        .expect("profile")
        .parse::<Uuid>()
        .expect("id");
    sqlx::query("UPDATE execution_profiles SET status = 'available', account_state = 'authenticated', version = version + 1 WHERE id = $1")
        .bind(profile_id)
        .execute(&setup.pool)
        .await
        .expect("sign in");
    crate::support::as_job(
        setup.organization_id,
        agent_workspace_server::execution_setup::activate_pending_worker(&setup.pool, profile_id),
    )
    .await
    .expect("activate");
    assert_eq!(counts(setup.pool.clone()).await, (1, 1, 1, 1));
    let (status, workers) = send(
        &app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/execution-workers"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workers}");
    assert_eq!(
        workers["workers"].as_array().map(Vec::len),
        Some(1),
        "{workers}"
    );
}

/// project_create goes through the same command as the web app: the person
/// who asks owns the new project, retries return it, and agents and
/// read-only apps can't create projects.
#[tokio::test]
async fn people_create_projects_through_their_apps() {
    let Some(setup) = setup("mcp_project_create").await else {
        return;
    };
    let app = &setup.app;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["workspaceSpaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (_mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");

    for (token, session) in [(&mara_token, &mara_session), (&reader, &reader_session)] {
        let refused = tool(
            app,
            token,
            session,
            "space_create",
            json!({"key": "NOPE", "name": "Not allowed"}),
        )
        .await;
        assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    }
    let bad_key = tool(
        app,
        &codex,
        &session,
        "space_create",
        json!({"key": "1x", "name": "Bad key"}),
    )
    .await;
    assert_eq!(bad_key["outcome"], "invalid_request", "{bad_key}");

    let created = tool(
        app,
        &codex,
        &session,
        "space_create",
        json!({"key": "web", "name": "Website", "clientRequestId": "make-web"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    assert_eq!(created["value"]["key"], "WEB");
    assert_eq!(created["value"]["name"], "Website");
    let project_id = created["value"]["id"].as_str().expect("project").to_owned();

    // A retry returns the same project; another with that key is refused.
    let retried = tool(
        app,
        &codex,
        &session,
        "space_create",
        json!({"key": "web", "name": "Website", "clientRequestId": "make-web"}),
    )
    .await;
    assert_eq!(retried["value"]["id"], project_id.as_str(), "{retried}");
    let taken = tool(
        app,
        &codex,
        &session,
        "space_create",
        json!({"key": "WEB", "name": "Something else"}),
    )
    .await;
    assert_eq!(taken["outcome"], "conflict", "{taken}");

    // It lands in the caller's own workspace, the caller owns it, and it has
    // the default workflow like a web-made one.
    let organization: Uuid =
        sqlx::query_scalar("SELECT organization_id FROM projects WHERE id = $1")
            .bind(Uuid::parse_str(&project_id).expect("id"))
            .fetch_one(&setup.pool)
            .await
            .expect("project");
    assert_eq!(organization, setup.organization_id);
    let role: String = sqlx::query_scalar(
        "SELECT membership.role FROM project_memberships membership JOIN users u ON u.actor_id = membership.actor_id WHERE membership.project_id = $1 AND u.login_name = 'owner'",
    )
    .bind(Uuid::parse_str(&project_id).expect("id"))
    .fetch_one(&setup.pool)
    .await
    .expect("owner membership");
    assert_eq!(role, "owner");
    let workflow = tool(
        app,
        &codex,
        &session,
        "workflow_get",
        json!({"projectId": "WEB"}),
    )
    .await;
    assert_eq!(workflow["outcome"], "ok", "{workflow}");
}

/// K1.13: changing a project is decided by the role, never the kind of
/// account. An agent holding Member is refused, as a person holding Member
/// is; the same agent given an admin role (in the workspace and the
/// project) renames the project.
#[tokio::test]
async fn an_agent_with_a_managing_role_changes_a_project() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("k113_agent_manages")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let rook = &world.agents["rook"];
    let refused = tool(
        &world.app,
        &rook.token,
        &rook.session,
        "space_update",
        json!({"spaceId": work, "name": "Renamed by a member"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    sqlx::query("UPDATE organization_memberships SET role = 'admin', role_explicit = true WHERE actor_id = $1")
        .bind(rook.actor)
        .execute(world.pool())
        .await
        .expect("workspace admin");
    sqlx::query("UPDATE project_memberships SET role = 'admin' WHERE actor_id = $1 AND project_id = $2::uuid")
        .bind(rook.actor)
        .bind(&work)
        .execute(world.pool())
        .await
        .expect("project admin");
    let renamed = tool(
        &world.app,
        &rook.token,
        &rook.session,
        "space_update",
        json!({"spaceId": work, "name": "Renamed by an admin"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    world.finish().await;
}

/// A credential confined to one space acts at workspace level only within
/// what it holds there, and never with a permission that reaches the whole
/// workspace: an owner's credential held to SOURCE can't make an agent,
/// add a person or make an invitation link, though it still works in SOURCE.
#[tokio::test]
async fn a_space_confined_credential_does_nothing_workspace_wide() {
    use agent_workspace_server::mcp::{IssueCredentialCommand, issue_credential};
    let Some(setup) = setup("mcp_confined_workspace").await else {
        return;
    };
    let source = create_project(&setup, "SOURCE", "Source").await;
    let actor =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let issued = issue_credential(
        &setup.pool,
        IssueCredentialCommand {
            actor_id: actor,
            project_id: source.parse().expect("source"),
            client_name: "Confined".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("credential");
    let session = initialize(&setup.app, &issued.token)
        .await
        .expect("session");
    for (name, arguments) in [
        (
            "agent_create",
            json!({"displayName": "Ivy", "handle": "ivy", "clientRequestId": "ivy"}),
        ),
        (
            "person_create",
            json!({"displayName": "Nia", "handle": "nia", "clientRequestId": "nia"}),
        ),
        ("invite_link_create", json!({"clientRequestId": "link"})),
    ] {
        let refused = tool(&setup.app, &issued.token, &session, name, arguments).await;
        assert_ne!(
            refused["outcome"], "ok",
            "{name}: a confined credential acted on the whole workspace: {refused}"
        );
    }
    let made: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM actors WHERE kind = 'agent') \
              + (SELECT count(*) FROM users WHERE login_name IS NULL OR claimed_at IS NULL) \
              + (SELECT count(*) FROM invite_tokens)",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(made, 0);
    let created = tool(
        &setup.app,
        &issued.token,
        &session,
        "task_create",
        json!({"title": "Still works here"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
}
