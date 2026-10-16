//! Planning through MCP (roadmap W3, T1 to T8): dependencies across projects
//! but never across workspaces or into projects the caller isn't in, who may
//! change a workflow, retired statuses moving their tasks with history, and
//! history that reads a change's before and after.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

fn keys(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .map(|row| row["key"].as_str().expect("key").to_owned())
        .collect()
}

fn history_texts(opened: &Value) -> Vec<String> {
    opened["value"]["history"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("no history: {opened}"))
        .iter()
        .map(|entry| entry["text"].as_str().expect("text").to_owned())
        .collect()
}

/// An agent who belongs to `project` only, with its MCP token.
async fn agent_in(setup: &Setup, project: &str, name: &str, handle: &str) -> (Uuid, String) {
    let app = &setup.app;
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
    let (actor, token) = create_agent(app, &setup.browser, &space, name, handle).await;
    let (status, added) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/members"),
            None,
            json!({"actorId": actor, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    (actor, token)
}

async fn create_task(
    setup: &Setup,
    token: &str,
    session: &str,
    project: &str,
    arguments: Value,
) -> String {
    let mut arguments = arguments;
    arguments["projectId"] = json!(project);
    let created = tool(&setup.app, token, session, "task_create", arguments).await;
    assert_eq!(created["outcome"], "applied", "{created}");
    created["key"].as_str().expect("key").to_owned()
}

#[tokio::test]
async fn dependencies_cross_projects_but_never_workspaces_or_private_projects() {
    let Some(setup) = setup("mcp_planning_links").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let side = create_project(&setup, "SIDE", "Side").await;
    let secret = create_project(&setup, "SECRET", "Secret").await;

    // The owner also runs a second workspace, with a project of its own.
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(&setup.pool)
        .await
        .expect("second organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(elsewhere)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    let (status, other) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some("create-OTHER"),
            json!({"organizationId": elsewhere, "key": "OTHER", "name": "Other"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let other = other["id"].as_str().expect("other").to_owned();

    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let (mara_id, mara) = agent_in(&setup, &work, "Mara", "mara").await;
    let mara_session = initialize(app, &mara).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let as_mara = |name: &'static str, arguments: Value| {
        let mara = mara.clone();
        let session = mara_session.clone();
        async move { tool(app, &mara, &session, name, arguments).await }
    };
    for (project, title) in [
        (&work, "Design"),
        (&work, "Build"),
        (&work, "Ship"),
        (&side, "Docs"),
        (&secret, "Private plan"),
        (&other, "Their work"),
    ] {
        create_task(&setup, &codex, &session, project, json!({"title": title})).await;
    }

    // WORK-1 → WORK-2 → WORK-3 → SIDE-1: closing the loop either way is
    // refused with the path, keys and all.
    for (task, blocker) in [
        ("WORK-2", "WORK-1"),
        ("WORK-3", "WORK-2"),
        ("SIDE-1", "WORK-3"),
    ] {
        let added = call(
            "task_depend",
            json!({"taskId": task, "blockedBy": [blocker]}),
        )
        .await;
        assert_eq!(added["outcome"], "ok", "{added}");
    }
    let cycle = call(
        "task_depend",
        json!({"taskId": "WORK-1", "blockedBy": ["SIDE-1"]}),
    )
    .await;
    assert_eq!(cycle["outcome"], "conflict", "{cycle}");
    assert!(
        cycle["message"]
            .as_str()
            .is_some_and(|text| text.contains("WORK-1 blocks WORK-2 blocks WORK-3 blocks SIDE-1")),
        "{cycle}"
    );
    let docs = call("task_open", json!({"taskId": "SIDE-1"})).await;
    assert_eq!(keys(&docs["value"]["blockedBy"]), ["WORK-3"], "{docs}");
    assert_eq!(docs["value"]["ready"], false);

    // The other workspace's task is reachable on its own, but never as a
    // link: it reads as not found, and nothing is written.
    let theirs = call("task_open", json!({"taskId": "OTHER-1"})).await;
    assert_eq!(theirs["outcome"], "ok", "{theirs}");
    let their_id = theirs["value"]["task"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    for reference in ["OTHER-1", their_id.as_str()] {
        let refused = call(
            "task_depend",
            json!({"taskId": "WORK-1", "blocks": [reference]}),
        )
        .await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
        let related = call(
            "task_relate",
            json!({"taskId": "WORK-1", "otherTaskId": reference}),
        )
        .await;
        assert_eq!(related["outcome"], "not_found", "{related}");
    }
    let crossing = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_dependencies WHERE project_id = $1 OR blocking_project_id = $1",
    )
    .bind(other.parse::<Uuid>().expect("other id"))
    .fetch_one(&setup.pool)
    .await
    .expect("crossing links");
    assert_eq!(crossing, 0);

    // A private project's task blocks WORK-3. Mara, who isn't in SECRET,
    // sees that something unfinished blocks it, never what.
    let hidden = call(
        "task_depend",
        json!({"taskId": "WORK-3", "blockedBy": ["SECRET-1"]}),
    )
    .await;
    assert_eq!(
        keys(&hidden["value"]["blockedBy"]),
        ["SECRET-1", "WORK-2"],
        "{hidden}"
    );
    let secret_id = hidden["value"]["blockedBy"][0]["id"]
        .as_str()
        .expect("secret id")
        .to_owned();
    let seen = as_mara(
        "task_open",
        json!({"taskId": "WORK-3", "include": ["history"]}),
    )
    .await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert_eq!(keys(&seen["value"]["blockedBy"]), ["WORK-2"], "{seen}");
    assert_eq!(seen["value"]["blockedBy"][0]["status"], "Backlog");
    // SIDE-1 waits on it, but Mara isn't in SIDE either.
    assert!(seen["value"].get("blocks").is_none(), "{seen}");
    assert_eq!(seen["value"]["ready"], false);
    assert!(!seen.to_string().contains("SECRET"), "{seen}");
    assert!(!seen.to_string().contains("Private plan"), "{seen}");
    let texts = history_texts(&seen);
    assert!(texts.contains(&"Blocked by WORK-2".to_owned()), "{texts:?}");
    assert!(
        texts.contains(&"Blocked by a task you can't see".to_owned()),
        "{texts:?}"
    );
    for reference in ["SECRET-1", secret_id.as_str()] {
        let refused = as_mara(
            "task_depend",
            json!({"taskId": "WORK-1", "blocks": [reference]}),
        )
        .await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
        let opened = as_mara("task_open", json!({"taskId": reference})).await;
        assert_ne!(opened["outcome"], "ok", "{opened}");
        assert!(!opened.to_string().contains("Private plan"), "{opened}");
    }
    let listed = as_mara("task_list", json!({"projectId": "WORK", "blocked": true})).await;
    assert_eq!(
        keys(&listed["value"]["tasks"]),
        ["WORK-2", "WORK-3"],
        "{listed}"
    );
    assert_eq!(
        listed["value"]["tasks"][1]["openBlockerCount"], 2,
        "{listed}"
    );
    let ready = as_mara("task_list", json!({"projectId": "WORK", "ready": true})).await;
    assert_eq!(keys(&ready["value"]["tasks"]), ["WORK-1"], "{ready}");

    // Mara watches WORK-3; a duplicate closes and stops blocking.
    let watched = as_mara("task_watch", json!({"taskId": "WORK-3"})).await;
    assert_eq!(watched["outcome"], "ok", "{watched}");
    assert!(
        watched["value"]["watchers"]
            .as_array()
            .expect("watchers")
            .iter()
            .any(|watcher| watcher["handle"] == "mara"),
        "{watched}"
    );
    // A viewer may watch what they can see, as in the app.
    let set_role = |role: &'static str| {
        sqlx::query(
            "UPDATE project_memberships SET role = $3 WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(work.parse::<Uuid>().expect("work id"))
        .bind(mara_id)
        .bind(role)
        .execute(&setup.pool)
    };
    set_role("viewer").await.expect("make Mara a viewer");
    let unwatched = as_mara("task_watch", json!({"taskId": "WORK-3", "watching": false})).await;
    assert_eq!(unwatched["outcome"], "ok", "{unwatched}");
    let rewatched = as_mara("task_watch", json!({"taskId": "WORK-3"})).await;
    assert_eq!(rewatched["value"]["watching"], true, "{rewatched}");
    set_role("member").await.expect("make Mara a member again");
    let duplicate = call(
        "task_relate",
        json!({"taskId": "WORK-2", "otherTaskId": "WORK-1", "kind": "duplicates", "close": true}),
    )
    .await;
    assert_eq!(duplicate["outcome"], "ok", "{duplicate}");
    assert_eq!(duplicate["value"]["relations"][0]["kind"], "duplicates");
    assert_eq!(duplicate["value"]["task"]["statusGroup"], "canceled");
    let original = as_mara("task_open", json!({"taskId": "WORK-1"})).await;
    assert_eq!(
        original["value"]["relations"][0]["kind"], "duplicated_by",
        "{original}"
    );
    let after = as_mara("task_open", json!({"taskId": "WORK-3"})).await;
    assert_eq!(after["value"]["watchers"][0]["handle"], "mara", "{after}");
    assert_eq!(after["value"]["ready"], false, "SECRET-1 still blocks it");
}

#[tokio::test]
async fn workflow_update_is_for_admins_and_retired_statuses_move_their_tasks() {
    let Some(setup) = setup("mcp_planning_workflow").await else {
        return;
    };
    let app = &setup.app;
    let flow = create_project(&setup, "FLOW", "Flow").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let (_, mara) = agent_in(&setup, &flow, "Mara", "mara").await;
    let mara_session = initialize(app, &mara).await.expect("Mara MCP");
    for title in ["One", "Two"] {
        create_task(&setup, &codex, &session, &flow, json!({"title": title})).await;
    }
    let current = tool(
        app,
        &codex,
        &session,
        "workflow_get",
        json!({"projectId": "FLOW"}),
    )
    .await;
    assert_eq!(current["outcome"], "ok", "{current}");
    let version = current["value"]["version"].as_i64().expect("version");
    // Everything but Backlog stays; its open tasks go to Ready.
    let statuses = current["value"]["statuses"]
        .as_array()
        .expect("statuses")
        .iter()
        .filter(|status| status["name"] != "Backlog")
        .enumerate()
        .map(|(index, status)| {
            json!({
                "id": status["id"], "name": status["name"],
                "category": status["category"], "isDefault": index == 0
            })
        })
        .collect::<Vec<_>>();
    let types = current["value"]["types"]
        .as_array()
        .expect("types")
        .iter()
        .map(|kind| json!({"id": kind["id"], "name": kind["name"], "isDefault": kind["isDefault"]}))
        .collect::<Vec<_>>();
    let document = json!({
        "projectId": flow, "expectedVersion": version,
        "statuses": statuses, "types": types,
        "replacements": {"Backlog": "Ready"}
    });

    for (token, session) in [(&mara, &mara_session), (&reader, &reader_session)] {
        let refused = tool(app, token, session, "workflow_update", document.clone()).await;
        assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    }
    let mut unplaced = document.clone();
    unplaced["replacements"] = json!({});
    let unplaced = tool(app, &codex, &session, "workflow_update", unplaced).await;
    assert_eq!(unplaced["outcome"], "invalid_request", "{unplaced}");
    assert!(
        unplaced["message"]
            .as_str()
            .is_some_and(|text| text.contains("2 open tasks in Backlog")),
        "{unplaced}"
    );

    // Mara watches FLOW-1, so the move reaches her as an event.
    let watched = tool(
        app,
        &mara,
        &mara_session,
        "task_watch",
        json!({"taskId": "FLOW-1"}),
    )
    .await;
    assert_eq!(watched["outcome"], "ok", "{watched}");
    let mara_events = || {
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM external_agent_events event
            JOIN external_agent_connections connection ON connection.id = event.connection_id
            JOIN actors actor ON actor.id = connection.actor_id
            WHERE actor.kind = 'external_agent' AND event.kind = 'work_item.changed'
              AND connection.actor_id IN (
                  SELECT actor_id FROM work_item_watchers watcher
                  JOIN work_items item ON item.id = watcher.work_item_id
                  WHERE item.project_id = $1 AND item.number = 1
              )
            "#,
        )
        .bind(flow.parse::<Uuid>().expect("flow id"))
        .fetch_one(&setup.pool)
    };
    let before = mara_events().await.expect("events before");

    let updated = tool(app, &codex, &session, "workflow_update", document.clone()).await;
    assert_eq!(updated["outcome"], "ok", "{updated}");
    assert_eq!(
        mara_events().await.expect("events after"),
        before + 1,
        "the watcher hears about the move"
    );
    assert_eq!(updated["value"]["statusesRetired"], 1);
    assert_eq!(updated["value"]["tasksMoved"], 2);
    assert_eq!(updated["value"]["workflow"]["version"], version + 1);
    assert!(
        !updated["value"]["workflow"]["statuses"]
            .to_string()
            .contains("Backlog"),
        "{updated}"
    );
    let moved = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "FLOW-1", "include": ["history"]}),
    )
    .await;
    assert_eq!(moved["value"]["task"]["status"]["name"], "Ready", "{moved}");
    let texts = history_texts(&moved);
    assert_eq!(
        texts.last().map(String::as_str),
        Some("Moved from Backlog to Ready"),
        "{texts:?}"
    );
    let stale = tool(app, &codex, &session, "workflow_update", document.clone()).await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // The role is read on every call: an owner made a member can't anymore.
    sqlx::query(
        "UPDATE project_memberships SET role = 'member' WHERE project_id = $1 AND actor_id = (SELECT actor_id FROM users WHERE login_name = 'owner')",
    )
    .bind(flow.parse::<Uuid>().expect("flow id"))
    .execute(&setup.pool)
    .await
    .expect("demote owner");
    let mut again = document;
    again["expectedVersion"] = json!(version + 1);
    let demoted = tool(app, &codex, &session, "workflow_update", again).await;
    assert_eq!(demoted["outcome"], "permission_denied", "{demoted}");
}

#[tokio::test]
async fn task_fields_filter_move_and_read_as_before_and_after() {
    let Some(setup) = setup("mcp_planning_fields").await else {
        return;
    };
    let app = &setup.app;
    let plan = create_project(&setup, "PLAN", "Plan").await;
    let next = create_project(&setup, "NEXT", "Next").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let milestone = call(
        "milestone_write",
        json!({"projectId": "PLAN", "name": "v1", "targetDate": "2026-12-01"}),
    )
    .await;
    assert_eq!(milestone["outcome"], "ok", "{milestone}");

    create_task(
        &setup,
        &codex,
        &session,
        &plan,
        json!({"title": "Launch", "priority": "high", "milestone": "V1",
               "startDate": "2019-12-01", "dueDate": "2020-01-01", "estimate": 3}),
    )
    .await;
    create_task(
        &setup,
        &codex,
        &session,
        &plan,
        json!({"title": "Polish", "dueDate": "2030-01-01"}),
    )
    .await;
    create_task(&setup, &codex, &session, &plan, json!({"title": "Someday"})).await;
    let refused = call(
        "task_create",
        json!({"projectId": plan, "title": "Backwards", "startDate": "2026-02-01", "dueDate": "2026-01-01"}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert_eq!(refused["field"], "dueDate");
    let unknown = call(
        "task_create",
        json!({"projectId": plan, "title": "Nowhere", "milestone": "v9"}),
    )
    .await;
    assert_eq!(unknown["field"], "milestone", "{unknown}");

    let opened = call("task_open", json!({"taskId": "PLAN-1"})).await;
    let value = &opened["value"];
    assert_eq!(value["milestone"]["name"], "v1", "{opened}");
    assert_eq!(value["startDate"], "2019-12-01");
    assert_eq!(value["dueDate"], "2020-01-01");
    assert_eq!(value["overdue"], true);
    assert_eq!(value["estimate"], 3.0);
    assert_eq!(value["estimateUnit"], "points");
    assert_eq!(value["ready"], true);

    // Filters: overdue, due before, milestone (or none).
    let list = |arguments: Value| {
        let call = &call;
        async move {
            let mut arguments = arguments;
            arguments["projectId"] = json!("PLAN");
            let listed = call("task_list", arguments).await;
            assert_eq!(listed["outcome"], "ok", "{listed}");
            listed["value"].clone()
        }
    };
    assert_eq!(
        keys(&list(json!({"overdue": true})).await["tasks"]),
        ["PLAN-1"]
    );
    assert_eq!(
        keys(&list(json!({"dueBefore": "2030-01-01"})).await["tasks"]),
        ["PLAN-1", "PLAN-2"]
    );
    let in_v1 = list(json!({"milestone": "v1"})).await;
    assert_eq!(keys(&in_v1["tasks"]), ["PLAN-1"], "{in_v1}");
    assert_eq!(in_v1["tasks"][0]["milestone"], "v1");
    assert_eq!(in_v1["tasks"][0]["dueDate"], "2020-01-01");
    assert_eq!(
        keys(&list(json!({"milestone": "none"})).await["tasks"]),
        ["PLAN-2", "PLAN-3"]
    );
    let missing = call("task_list", json!({"projectId": "PLAN", "milestone": "v9"})).await;
    assert_eq!(missing["outcome"], "invalid_request", "{missing}");

    // A change reads as its before and after.
    let updated = call(
        "task_update",
        json!({"taskId": "PLAN-1", "expectedVersion": opened["value"]["task"]["version"],
               "priority": "urgent", "dueDate": "2026-10-20", "clearEstimate": true}),
    )
    .await;
    assert_eq!(updated["outcome"], "applied", "{updated}");
    // A due date before a task's start is found before anything is applied.
    let early = call(
        "task_update_many",
        json!({"taskIds": ["PLAN-2", "PLAN-1"], "dueDate": "2019-01-01"}),
    )
    .await;
    assert_ne!(early["outcome"], "ok", "{early}");
    assert_eq!(early["problems"][0]["field"], "dueDate", "{early}");
    assert!(
        early
            .to_string()
            .contains("PLAN-1 starts after that due date"),
        "{early}"
    );
    let bulk = call(
        "task_update_many",
        json!({"taskIds": ["PLAN-2", "PLAN-3"], "milestone": "v1", "dueDate": ""}),
    )
    .await;
    assert_eq!(bulk["outcome"], "ok", "{bulk}");
    assert_eq!(bulk["value"]["changedCount"], 2, "{bulk}");
    let history = call(
        "task_open",
        json!({"taskId": "PLAN-1", "include": ["history"]}),
    )
    .await;
    let texts = history_texts(&history);
    assert_eq!(
        texts.last().map(String::as_str),
        Some("Priority: High → Urgent; Due: 2020-01-01 → 2026-10-20; Estimate: 3 → none"),
        "{texts:?}"
    );
    let polish = call(
        "task_open",
        json!({"taskId": "PLAN-2", "include": ["history"]}),
    )
    .await;
    assert_eq!(
        history_texts(&polish).last().map(String::as_str),
        Some("Milestone: none → v1; Due: 2030-01-01 → none"),
        "{polish}"
    );

    // milestone_write moves write history too.
    let out = call(
        "milestone_write",
        json!({"projectId": "PLAN", "milestoneId": milestone["value"]["id"], "removeTasks": ["PLAN-3"]}),
    )
    .await;
    assert_eq!(out["value"]["total"], 2, "{out}");
    // Archiving and adding at once is refused, and nothing is archived.
    let refused = call(
        "milestone_write",
        json!({"projectId": "PLAN", "milestoneId": milestone["value"]["id"],
               "archived": true, "addTasks": ["PLAN-3"]}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let listed = call("milestone_list", json!({"projectId": "PLAN"})).await;
    assert_eq!(
        listed["value"]["milestones"][0]["archived"], false,
        "{listed}"
    );
    let someday = call(
        "task_open",
        json!({"taskId": "PLAN-3", "include": ["history"]}),
    )
    .await;
    assert_eq!(
        history_texts(&someday).last().map(String::as_str),
        Some("Milestone: v1 → none"),
        "{someday}"
    );

    // A move carries the dates; the estimate only between projects that
    // estimate in the same unit.
    sqlx::query(
        "INSERT INTO project_task_settings (project_id, estimate_unit) VALUES ($1, 'hours')",
    )
    .bind(next.parse::<Uuid>().expect("project id"))
    .execute(&setup.pool)
    .await
    .expect("estimate unit");
    let current = call("task_open", json!({"taskId": "PLAN-1"})).await;
    let sized = call(
        "task_update",
        json!({"taskId": "PLAN-1", "expectedVersion": current["value"]["task"]["version"], "estimate": 8}),
    )
    .await;
    assert_eq!(sized["outcome"], "applied", "{sized}");
    let moved = call(
        "task_move",
        json!({"taskId": "PLAN-1", "toProjectId": "NEXT"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let carried = call("task_open", json!({"taskId": "NEXT-1"})).await;
    assert_eq!(carried["value"]["dueDate"], "2026-10-20", "{carried}");
    assert_eq!(carried["value"]["startDate"], "2019-12-01");
    assert_eq!(carried["value"]["estimate"], Value::Null, "{carried}");
    assert!(
        moved["value"]["notes"]
            .to_string()
            .contains("estimates in hours, not points"),
        "{moved}"
    );
}
