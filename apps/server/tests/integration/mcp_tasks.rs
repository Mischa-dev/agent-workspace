//! task_create and task_update (docs/plans/mcp-tools.md, Phase 2): an agent
//! lays out and reorganizes a board without the web app. The costly bugs are
//! silent: a field dropped on its way to the shared command, a refused call
//! that still used up a task number or its clientRequestId, a task nested
//! under its own subtask, a label change that bumps the version twice, an
//! error that names a project the caller can't reach, a claim left on an
//! archived task, or a moved task's original restored as a live duplicate.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

fn id(value: &Value) -> Uuid {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not an ID: {value}"))
        .parse()
        .expect("UUID")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {value}"))
        .iter()
        .map(|entry| match entry {
            Value::String(text) => text.clone(),
            entry => entry["text"]
                .as_str()
                .or_else(|| entry["name"].as_str())
                .or_else(|| entry["key"].as_str())
                .unwrap_or_else(|| panic!("no text: {entry}"))
                .to_owned(),
        })
        .collect()
}

fn message(result: &Value) -> &str {
    result["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no message: {result}"))
}

fn assert_refused(result: &Value, field: &str, says: &str) {
    assert_eq!(result["outcome"], "invalid_request", "{result}");
    assert_eq!(result["field"], field, "{result}");
    assert_eq!(result["retryCategory"], "correct_input", "{result}");
    assert!(message(result).contains(says), "{result}");
}

async fn scalar(setup: &Setup, query: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(query)
        .fetch_one(&setup.pool)
        .await
        .unwrap_or_else(|error| panic!("{query}: {error}"))
}

async fn version(setup: &Setup, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("version")
}

async fn parent(setup: &Setup, task: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT parent_work_item_id FROM work_items WHERE id = $1",
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("parent")
}

/// Activity, audit and outbox rows: one of each per change.
async fn side_effects(setup: &Setup) -> (i64, i64, i64) {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("side effects")
}

/// A status column's task keys, top first.
async fn column(app: &axum::Router, token: &str, session: &str, status: &str) -> Vec<String> {
    let listed = tool(
        app,
        token,
        session,
        "task_list",
        json!({"projectId": "WORK", "status": status}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    strings(&listed["value"]["tasks"])
}

/// The Messages space every owner has, where agents are created.
pub(super) async fn space(setup: &Setup) -> String {
    send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned()
}

pub(super) async fn add_member(setup: &Setup, project: &str, actor: Uuid, role: &str) {
    let (status, added) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/members"),
            None,
            json!({"actorId": actor, "role": role}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
}

#[tokio::test]
async fn an_agent_lays_out_a_plan_and_reorganizes_the_board() {
    let Some(setup) = setup("mcp_tasks_plan").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for name in ["Mobile", "Sync"] {
        let label = call("label_write", json!({"projectId": "WORK", "name": name})).await;
        assert_eq!(label["outcome"], "ok", "{label}");
    }
    for (title, status) in [
        ("First ready", Some("Ready")),
        ("Second ready", Some("Ready")),
        ("Waiting", None),
    ] {
        let created = call(
            "task_create",
            json!({"projectId": "WORK", "title": title, "status": status}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }

    // One call sets everything, as one change: version 1 and one activity,
    // audit row and outbox event.
    let plan = json!({
        "projectId": "WORK",
        "title": "Offline mode",
        "type": "bug",
        "status": "ready",
        "labels": ["mobile", "SYNC"],
        "acceptanceCriteria": [
            "Works with no network",
            "  Syncs within 5s of reconnecting\n(measured on 3G) "
        ],
        "position": "top",
        "priority": "high",
        "assignee": "me",
        "clientRequestId": "plan-offline"
    });
    let before = side_effects(&setup).await;
    let created = call("task_create", plan.clone()).await;
    assert_eq!(created["outcome"], "applied", "{created}");
    assert_eq!(created["version"], 1);
    assert_eq!(created["key"], "WORK-4");
    assert_eq!(created["changedFields"], json!(["work_item"]));
    assert_eq!(created["labels"], json!(["Mobile", "Sync"]), "{created}");
    let offline = id(&created["workItemId"]);
    assert!(
        created["url"]
            .as_str()
            .expect("url")
            .ends_with(&format!("/projects/{work}/work-items/{offline}")),
        "{created}"
    );
    let after = side_effects(&setup).await;
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        (1, 1, 1)
    );
    let opened = call("task_open", json!({"taskId": "WORK-4"})).await;
    let task = &opened["value"]["task"];
    assert_eq!(task["status"]["name"], "Ready", "{opened}");
    assert_eq!(task["itemType"]["name"], "Bug");
    assert_eq!(task["priority"], "high");
    assert!(task["assignee"].is_object(), "{opened}");
    assert_eq!(strings(&task["labels"]), ["Mobile", "Sync"]);
    assert_eq!(
        strings(&task["acceptanceCriteria"]),
        [
            "Works with no network",
            "Syncs within 5s of reconnecting\n(measured on 3G)"
        ]
    );
    assert_eq!(
        column(app, &codex, &session, "Ready").await,
        ["WORK-4", "WORK-1", "WORK-2"]
    );
    let replay = call("task_create", plan).await;
    assert_eq!(replay["idempotentReplay"], true, "{replay}");
    assert_eq!(replay["key"], "WORK-4");
    assert_eq!(replay["labels"], json!(["Mobile", "Sync"]));
    assert_eq!(side_effects(&setup).await, after);

    // Refused creates write nothing: no task, no task number, and the
    // clientRequestId stays free for the corrected call.
    let numbers = "SELECT next_work_item_number FROM project_counters";
    let (next_number, tasks) = (
        scalar(&setup, numbers).await,
        scalar(&setup, "SELECT count(*) FROM work_items").await,
    );
    let refuse = |extra: Value| {
        let mut arguments = json!({"projectId": "WORK", "title": "Refused"});
        for (key, value) in extra.as_object().expect("object") {
            arguments[key] = value.clone();
        }
        call("task_create", arguments)
    };
    assert_refused(
        &refuse(json!({"status": "Doing"})).await,
        "status",
        "No status called \"Doing\" in WORK. Statuses: Backlog, Ready",
    );
    assert_refused(
        &refuse(json!({"type": "Epic"})).await,
        "type",
        "No type called \"Epic\" in WORK. Types: Feature, Bug, Task, Request, Research, Improvement.",
    );
    let unknown = refuse(json!({"labels": ["nope"], "clientRequestId": "plan-retry"})).await;
    assert_refused(&unknown, "labels", "No label called \"nope\" in WORK");
    assert!(
        message(&unknown).contains("Mobile") && message(&unknown).contains("label_write"),
        "{unknown}"
    );
    assert_refused(
        &refuse(json!({"afterTaskId": "WORK-999"})).await,
        "afterTaskId",
        "No task WORK-999",
    );
    assert_refused(
        &refuse(json!({"status": "Ready", "afterTaskId": "WORK-3"})).await,
        "afterTaskId",
        "WORK-3 is in Backlog, but this task goes in Ready.",
    );
    assert_refused(
        &refuse(json!({"status": "Ready", "position": 9})).await,
        "position",
        "Ready has 3 other open tasks, so use a position from 1 to 4 (or top or bottom).",
    );
    assert_refused(
        &refuse(json!({"position": "middle"})).await,
        "position",
        "Use top, bottom, or a position number",
    );
    assert_refused(
        &refuse(json!({"position": "top", "beforeTaskId": "WORK-1"})).await,
        "position",
        "not several",
    );
    assert_refused(
        &refuse(json!({"acceptanceCriteria": ["Fine", "   "]})).await,
        "acceptanceCriteria",
        "Acceptance criterion 2 is empty",
    );
    assert_eq!(scalar(&setup, numbers).await, next_number);
    assert_eq!(
        scalar(&setup, "SELECT count(*) FROM work_items").await,
        tasks
    );
    let corrected = refuse(json!({"labels": ["Mobile"], "clientRequestId": "plan-retry"})).await;
    assert_eq!(corrected["outcome"], "applied", "{corrected}");
    assert_eq!(corrected["key"], "WORK-5");

    // A subtask keeps everything it's given (it used to lose its priority
    // and assignee on the way).
    let subtask = call(
        "task_create",
        json!({
            "parentId": "WORK-4",
            "title": "Sync queue",
            "priority": "high",
            "assignee": "me",
            "status": "Ready",
            "type": "Task",
            "labels": ["Sync"],
            "acceptanceCriteria": ["Queue survives a restart"],
            "position": "bottom",
            "clientRequestId": "plan-subtask"
        }),
    )
    .await;
    assert_eq!(subtask["outcome"], "applied", "{subtask}");
    assert_eq!(subtask["changedFields"], json!(["subtask"]));
    assert_eq!(subtask["key"], "WORK-6");
    let opened = call("task_open", json!({"taskId": "WORK-6"})).await;
    let task = &opened["value"]["task"];
    assert_eq!(id(&task["parentWorkItemId"]), offline, "{opened}");
    assert_eq!(task["priority"], "high");
    assert!(task["assignee"].is_object(), "{opened}");
    assert_eq!(task["status"]["name"], "Ready");
    assert_eq!(task["itemType"]["name"], "Task");
    assert_eq!(strings(&task["labels"]), ["Sync"]);
    assert_eq!(
        strings(&task["acceptanceCriteria"]),
        ["Queue survives a restart"]
    );
    assert_eq!(
        column(app, &codex, &session, "Ready").await,
        ["WORK-4", "WORK-1", "WORK-2", "WORK-6"]
    );

    // Reordering one task is a versioned change of its position.
    let moved = call(
        "task_update",
        json!({"taskId": "WORK-2", "expectedVersion": 1, "position": 1}),
    )
    .await;
    assert_eq!(moved["outcome"], "applied", "{moved}");
    assert_eq!(moved["changedFields"], json!(["position"]));
    assert_eq!(moved["version"], 2);
    let moved = call(
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 1, "beforeTaskId": "WORK-4"}),
    )
    .await;
    assert_eq!(moved["changedFields"], json!(["position"]), "{moved}");
    assert_eq!(
        column(app, &codex, &session, "Ready").await,
        ["WORK-2", "WORK-1", "WORK-4", "WORK-6"]
    );
    let stays = call(
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "afterTaskId": "WORK-2"}),
    )
    .await;
    assert_eq!(stays["outcome"], "applied", "{stays}");
    assert_eq!(stays["changedFields"], json!([]), "{stays}");
    assert_eq!(stays["version"], 2);
    assert_eq!(
        stays["key"], "WORK-1",
        "a no-op names the task too: {stays}"
    );
    assert!(stays["url"].is_string(), "{stays}");
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-2", "expectedVersion": 2, "position": "9"}),
        )
        .await,
        "position",
        "Ready has 3 other open tasks, so use a position from 1 to 4",
    );
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-2", "expectedVersion": 2, "afterTaskId": "WORK-2"}),
        )
        .await,
        "afterTaskId",
        "A task can't go before or after itself.",
    );

    // A status change carries the other changes with it, as one change
    // (a status call with anything else must not take the status-only
    // path, which would drop the rest).
    let waiting = sqlx::query_scalar::<_, Uuid>("SELECT id FROM work_items WHERE number = 3")
        .fetch_one(&setup.pool)
        .await
        .expect("WORK-3");
    let promoted = call(
        "task_update",
        json!({
            "taskId": "WORK-3",
            "expectedVersion": 1,
            "status": "Ready",
            "position": 2,
            "type": "Feature",
            "addLabels": ["Mobile"]
        }),
    )
    .await;
    assert_eq!(promoted["outcome"], "applied", "{promoted}");
    assert_eq!(promoted["version"], 2);
    for field in ["status", "position", "type", "labels"] {
        assert!(
            strings(&promoted["changedFields"]).contains(&field.to_owned()),
            "{field}: {promoted}"
        );
    }
    assert_eq!(promoted["labels"], json!(["Mobile"]));
    assert_eq!(
        column(app, &codex, &session, "Ready").await,
        ["WORK-2", "WORK-3", "WORK-1", "WORK-4", "WORK-6"]
    );
    let payloads = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 AND kind = 'work_item_updated'",
    )
    .bind(waiting)
    .fetch_all(&setup.pool)
    .await
    .expect("activity");
    let [payload] = payloads.as_slice() else {
        panic!("one change is one activity: {payloads:?}");
    };
    assert_eq!(payload["position"], 2, "{payload}");
    assert!(payload["statusId"].is_string(), "{payload}");
    assert_eq!(payload["labelsAdded"].as_array().map(Vec::len), Some(1));

    // The workflow still decides where a status can go.
    let skipped = call(
        "task_update",
        json!({"taskId": "WORK-5", "expectedVersion": 1, "status": "Done", "position": "top"}),
    )
    .await;
    assert_eq!(skipped["outcome"], "invalid_transition", "{skipped}");
    let nested = call(
        "task_update",
        json!({"taskId": "WORK-5", "expectedVersion": 1, "status": "Ready", "parentId": "WORK-4"}),
    )
    .await;
    assert_eq!(nested["outcome"], "applied", "{nested}");
    let changed = strings(&nested["changedFields"]);
    assert!(
        changed.contains(&"status".to_owned()) && changed.contains(&"parent".to_owned()),
        "{nested}"
    );
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-5", "expectedVersion": 2, "addLabels": []}),
        )
        .await,
        "update",
        "Pass at least one change",
    );
    for (key, arguments, also) in [
        (
            "WORK-5",
            json!({"expectedVersion": 2, "status": "In Progress", "clearParent": true}),
            "parent",
        ),
        (
            "WORK-1",
            json!({"expectedVersion": 2, "status": "In Progress", "beforeTaskId": "WORK-5"}),
            "position",
        ),
    ] {
        let mut arguments = arguments;
        arguments["taskId"] = json!(key);
        let started = call("task_update", arguments).await;
        assert_eq!(started["outcome"], "applied", "{started}");
        assert_eq!(started["version"], 3, "{started}");
        let changed = strings(&started["changedFields"]);
        assert!(
            changed.contains(&"status".to_owned()) && changed.contains(&also.to_owned()),
            "{started}"
        );
    }
    assert_eq!(
        column(app, &codex, &session, "In Progress").await,
        ["WORK-1", "WORK-5"]
    );
}

#[tokio::test]
async fn reparenting_refuses_cycles_and_never_names_projects_out_of_reach() {
    let Some(setup) = setup("mcp_tasks_parent").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "OTHER", "Other").await;
    create_project(&setup, "SECRET", "Secret").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let create = |arguments: Value| {
        let call = &call;
        async move {
            let created = call("task_create", arguments).await;
            assert_eq!(created["outcome"], "applied", "{created}");
            id(&created["workItemId"])
        }
    };
    let a = create(json!({"projectId": "WORK", "title": "A"})).await;
    let b = create(json!({"parentId": "WORK-1", "title": "B"})).await;
    let c = create(json!({"parentId": "WORK-2", "title": "C"})).await;
    create(json!({"projectId": "OTHER", "title": "Elsewhere"})).await;
    let secret = create(json!({"projectId": "SECRET", "title": "Codename"})).await;

    // A can't go under its own grandchild; nothing changes.
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-1", "expectedVersion": 1, "parentId": "WORK-3"}),
        )
        .await,
        "parentId",
        "WORK-3 is already under WORK-1 (WORK-1 › WORK-2 › WORK-3), so WORK-1 can't go under it. Move WORK-3 out first, or pick another parent.",
    );
    assert_eq!(version(&setup, a).await, 1);
    assert_eq!(parent(&setup, a).await, None);
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-3", "expectedVersion": 1, "parentId": "WORK-3"}),
        )
        .await,
        "parentId",
        "A task can't be its own parent.",
    );

    // A parent in another project the caller can reach is named, so the
    // agent can fix the call. The task's own project decides where a
    // task_update acts, never the parent's.
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-3", "expectedVersion": 1, "parentId": "OTHER-1"}),
        )
        .await,
        "parentId",
        "OTHER-1 is in Other, but this task is in Work.",
    );
    assert_refused(
        &call(
            "task_create",
            json!({"projectId": "WORK", "parentId": "OTHER-1", "title": "Stray"}),
        )
        .await,
        "parentWorkItemId",
        "The parent OTHER-1 is in Other, not Work.",
    );
    assert_eq!(parent(&setup, c).await, Some(b));

    // One the caller can't reach is never named, by ID or by key.
    for reference in [secret.to_string(), "SECRET-1".to_owned()] {
        let refused = tool(
            app,
            &mara_token,
            &mara_session,
            "task_update",
            json!({"taskId": c, "expectedVersion": 1, "parentId": reference}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
        assert_eq!(refused["field"], "parentId");
        assert!(!refused.to_string().contains("Secret"), "{refused}");
        assert!(!refused.to_string().contains("SECRET-1") || reference == "SECRET-1");
        let created = tool(
            app,
            &mara_token,
            &mara_session,
            "task_create",
            json!({"projectId": work, "parentId": secret, "title": "Stray"}),
        )
        .await;
        assert_ne!(created["outcome"], "applied", "{created}");
        assert!(!created.to_string().contains("Secret"), "{created}");
    }
    assert_eq!(version(&setup, c).await, 1);

    // An archived parent, and both ways of setting a parent at once.
    let archived = create(json!({"projectId": "WORK", "title": "Shelved"})).await;
    let shelved = call(
        "task_update",
        json!({"taskId": archived, "expectedVersion": 1, "archived": true}),
    )
    .await;
    assert_eq!(shelved["outcome"], "applied", "{shelved}");
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-3", "expectedVersion": 1, "parentId": archived}),
        )
        .await,
        "parentId",
        "WORK-4 is archived. Restore it first, or pick another parent.",
    );
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-3", "expectedVersion": 1, "parentId": "WORK-1", "clearParent": true}),
        )
        .await,
        "parentId",
        "Pass parentId or clearParent, not both.",
    );

    let cleared = call(
        "task_update",
        json!({"taskId": "WORK-3", "expectedVersion": 1, "clearParent": true}),
    )
    .await;
    assert_eq!(cleared["outcome"], "applied", "{cleared}");
    assert_eq!(cleared["changedFields"], json!(["parent"]));
    assert_eq!(cleared["version"], 2);
    let opened = call("task_open", json!({"taskId": "WORK-3"})).await;
    assert!(
        opened["value"]["task"]["parentWorkItemId"].is_null(),
        "{opened}"
    );
    let payload = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 AND kind = 'work_item_updated'",
    )
    .bind(c)
    .fetch_one(&setup.pool)
    .await
    .expect("activity");
    assert_eq!(
        payload["parent"],
        json!({"from": b, "to": null}),
        "{payload}"
    );
    let moved = call(
        "task_update",
        json!({"taskId": "WORK-3", "expectedVersion": 2, "parentId": "WORK-1"}),
    )
    .await;
    assert_eq!(moved["changedFields"], json!(["parent"]), "{moved}");
    assert_eq!(parent(&setup, c).await, Some(a));

    // The database refuses a cycle from any writer, not only through MCP.
    let refused = sqlx::query("UPDATE work_items SET parent_work_item_id = $1 WHERE id = $2")
        .bind(c)
        .bind(a)
        .execute(&setup.pool)
        .await
        .expect_err("a cycle");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514"),
        "{refused}"
    );

    // Trees stop at 20 levels, counting the top-level task as the first:
    // creating a 21st level and moving a task under the 20th are refused.
    let mut level = create(json!({"projectId": "WORK", "title": "Level 1"})).await;
    let top = level;
    let mut nineteenth = level;
    for depth in 2..=20 {
        if depth == 20 {
            nineteenth = level;
        }
        level = create(json!({"parentId": level, "title": format!("Level {depth}")})).await;
    }
    let too_deep = call(
        "task_create",
        json!({"parentId": level, "title": "Level 21"}),
    )
    .await;
    assert_refused(&too_deep, "parentWorkItemId", "more than 20 levels deep");
    create(json!({"parentId": nineteenth, "title": "Also level 20"})).await;
    let loose = create(json!({"projectId": "WORK", "title": "Loose"})).await;
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": loose, "expectedVersion": 1, "parentId": level}),
        )
        .await,
        "parentId",
        "more than 20 levels deep",
    );
    // The whole subtree counts: twenty levels can't go under anything.
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": top, "expectedVersion": 1, "parentId": "WORK-1"}),
        )
        .await,
        "parentId",
        "more than 20 levels deep",
    );

    // A parent's link works as its ID or key does, in task_create as in
    // task_update, and so does the task's own link.
    let shelf = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Shelf"}),
    )
    .await;
    assert_eq!(shelf["outcome"], "applied", "{shelf}");
    let book = call(
        "task_create",
        json!({"parentId": shelf["url"], "title": "Book"}),
    )
    .await;
    assert_eq!(book["outcome"], "applied", "{book}");
    assert_eq!(
        parent(&setup, id(&book["workItemId"])).await,
        Some(id(&shelf["workItemId"]))
    );
    let renamed = call(
        "task_update",
        json!({"taskId": book["url"], "expectedVersion": 1, "parentId": shelf["url"], "title": "A book"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "applied", "{renamed}");
    assert_eq!(renamed["changedFields"], json!(["title"]), "{renamed}");
}

#[tokio::test]
async fn labels_change_in_one_version_and_only_members_change_them() {
    let Some(setup) = setup("mcp_tasks_labels").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (vic, vic_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Vic", "vic").await;
    add_member(&setup, &work, vic, "viewer").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let vic_session = initialize(app, &vic_token).await.expect("Vic MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let mut gamma = Uuid::nil();
    for name in ["Alpha", "Beta", "Gamma"] {
        let label = call("label_write", json!({"projectId": "WORK", "name": name})).await;
        if name == "Gamma" {
            gamma = id(&label["value"]["label"]["id"]);
        }
    }
    let created = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Relabel me", "labels": ["Gamma"]}),
    )
    .await;
    assert_eq!(created["labels"], json!(["Gamma"]), "{created}");
    let task = id(&created["workItemId"]);

    let relabel = json!({
        "taskId": "WORK-1",
        "expectedVersion": 1,
        "addLabels": ["alpha", "BETA"],
        "removeLabels": ["gamma"],
        "clientRequestId": "relabel-1"
    });
    let relabeled = call("task_update", relabel.clone()).await;
    assert_eq!(relabeled["outcome"], "applied", "{relabeled}");
    assert_eq!(relabeled["version"], 2);
    assert_eq!(relabeled["changedFields"], json!(["labels"]));
    assert_eq!(relabeled["labels"], json!(["Alpha", "Beta"]));
    let payloads = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 AND kind = 'work_item_updated'",
    )
    .bind(task)
    .fetch_all(&setup.pool)
    .await
    .expect("activity");
    let [payload] = payloads.as_slice() else {
        panic!("one change is one activity: {payloads:?}");
    };
    assert_eq!(payload["labelsAdded"].as_array().map(Vec::len), Some(2));
    assert_eq!(payload["labelsRemoved"], json!([gamma]), "{payload}");
    let replay = call("task_update", relabel).await;
    assert_eq!(replay["idempotentReplay"], true, "{replay}");
    assert_eq!(replay["version"], 2);
    assert_eq!(replay["labels"], json!(["Alpha", "Beta"]));

    let again = call(
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "addLabels": ["Alpha"]}),
    )
    .await;
    assert_eq!(again["outcome"], "applied", "{again}");
    assert_eq!(again["changedFields"], json!([]));
    assert_eq!(again["version"], 2);
    assert_eq!(again["labels"], json!(["Alpha", "Beta"]));
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-1", "expectedVersion": 2, "addLabels": ["Gamma"], "removeLabels": ["gamma"]}),
        )
        .await,
        "removeLabels",
        "\"Gamma\" is in both addLabels and removeLabels. Pick one.",
    );
    let archived = call(
        "label_write",
        json!({"projectId": "WORK", "label": "Gamma", "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-1", "expectedVersion": 2, "removeLabels": ["Gamma"]}),
        )
        .await,
        "removeLabels",
        "archived",
    );
    let many = (1..=21)
        .map(|number| format!("L{number}"))
        .collect::<Vec<_>>();
    assert_refused(
        &call(
            "task_update",
            json!({"taskId": "WORK-1", "expectedVersion": 2, "addLabels": many}),
        )
        .await,
        "addLabels",
        "at most 20",
    );

    // Viewers and read-only apps change nothing.
    let viewer = tool(
        app,
        &vic_token,
        &vic_session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "removeLabels": ["Alpha"]}),
    )
    .await;
    assert_eq!(viewer["outcome"], "permission_denied", "{viewer}");
    assert_eq!(viewer["retryCategory"], "ask_project_owner");
    let viewer_create = tool(
        app,
        &vic_token,
        &vic_session,
        "task_create",
        json!({"projectId": work, "title": "Viewer task", "labels": ["Alpha"]}),
    )
    .await;
    assert_eq!(
        viewer_create["outcome"], "permission_denied",
        "{viewer_create}"
    );
    let read_only = tool(
        app,
        &reader,
        &reader_session,
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "removeLabels": ["Alpha"]}),
    )
    .await;
    assert_eq!(read_only["outcome"], "permission_denied", "{read_only}");
    assert_eq!(read_only["retryCategory"], "request_write_scope");
    assert_eq!(version(&setup, task).await, 2);

    let moved = call(
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 2, "status": "Ready", "removeLabels": ["Beta"]}),
    )
    .await;
    assert_eq!(moved["outcome"], "applied", "{moved}");
    assert_eq!(moved["version"], 3);
    assert_eq!(moved["labels"], json!(["Alpha"]));
    let changed = strings(&moved["changedFields"]);
    assert!(
        changed.contains(&"status".to_owned()) && changed.contains(&"labels".to_owned()),
        "{moved}"
    );
}

#[tokio::test]
async fn archiving_through_task_update_releases_claims_and_keeps_moved_originals() {
    let Some(setup) = setup("mcp_tasks_archive").await else {
        return;
    };
    let app = &setup.app;
    let old = create_project(&setup, "OLD", "Old work").await;
    create_project(&setup, "NEW", "New work").await;
    // Mara works only in the old project, so she can't follow a moved task.
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &old, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    call(
        "task_create",
        json!({"projectId": "OLD", "title": "Parent"}),
    )
    .await;
    call(
        "task_create",
        json!({"parentId": "OLD-1", "title": "First part"}),
    )
    .await;
    let second = call(
        "task_create",
        json!({"parentId": "OLD-1", "title": "Second part"}),
    )
    .await;
    let original = id(&second["workItemId"]);
    let claimed = call("task_claim", json!({"taskId": "OLD-1"})).await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");

    for extra in [json!({"title": "Renamed"}), json!({"status": "Ready"})] {
        let mut arguments = json!({"taskId": "OLD-1", "expectedVersion": 1, "archived": true});
        for (key, value) in extra.as_object().expect("object") {
            arguments[key] = value.clone();
        }
        assert_refused(
            &call("task_update", arguments).await,
            "archived",
            "Pass archived on its own.",
        );
    }

    let archive = json!({"taskId": "OLD-1", "expectedVersion": 1, "archived": true, "clientRequestId": "archive-parent"});
    let archived = call("task_update", archive.clone()).await;
    assert_eq!(archived["outcome"], "applied", "{archived}");
    assert_eq!(archived["changedFields"], json!(["archived"]));
    assert_eq!(archived["version"], 2);
    assert_eq!(archived["key"], "OLD-1");
    assert_eq!(
        archived["message"],
        "Archived. Its subtasks OLD-2 and OLD-3 are still open."
    );
    assert_eq!(
        scalar(&setup, "SELECT count(*) FROM work_item_claims").await,
        0,
        "archiving lets go of the claim"
    );
    let listed = call("task_list", json!({"projectId": "OLD"})).await;
    assert!(
        !strings(&listed["value"]["tasks"]).contains(&"OLD-1".to_owned()),
        "{listed}"
    );
    let replay = call("task_update", archive).await;
    assert_eq!(replay["idempotentReplay"], true, "{replay}");
    assert_eq!(replay["version"], 2);
    let already = call(
        "task_update",
        json!({"taskId": "OLD-1", "expectedVersion": 2, "archived": true}),
    )
    .await;
    assert_eq!(already["outcome"], "applied", "{already}");
    assert_eq!(already["changedFields"], json!([]));
    assert_eq!(already["version"], 2);

    // An archived task refuses every change but a restore.
    let renamed = call(
        "task_update",
        json!({"taskId": "OLD-1", "expectedVersion": 2, "title": "Renamed"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "conflict", "{renamed}");
    assert_eq!(renamed["changedFields"], json!(["archived"]));
    assert_eq!(renamed["retryCategory"], "do_not_retry");
    assert!(message(&renamed).contains("archived"), "{renamed}");
    let stale = call(
        "task_update",
        json!({"taskId": "OLD-1", "expectedVersion": 1, "archived": false}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert_eq!(stale["retryCategory"], "refetch_then_decide");
    let restored = call(
        "task_update",
        json!({"taskId": "OLD-1", "expectedVersion": 2, "archived": false}),
    )
    .await;
    assert_eq!(restored["outcome"], "applied", "{restored}");
    assert_eq!(restored["changedFields"], json!(["archived"]));
    assert_eq!(restored["version"], 3);
    let listed = call("task_list", json!({"projectId": "OLD"})).await;
    assert!(
        strings(&listed["value"]["tasks"]).contains(&"OLD-1".to_owned()),
        "{listed}"
    );

    // Agents with a member role archive and restore too.
    for (archived, expected_version) in [(true, 1), (false, 2)] {
        let changed = tool(
            app,
            &mara_token,
            &mara_session,
            "task_update",
            json!({"taskId": "OLD-2", "expectedVersion": expected_version, "archived": archived}),
        )
        .await;
        assert_eq!(changed["outcome"], "applied", "{changed}");
        assert_eq!(changed["version"], expected_version + 1);
    }

    // A moved task's original stays archived: restoring it would make a
    // live copy whose old key opens the moved task. Only someone who can
    // reach the new project is told where it went.
    let moved = call(
        "task_move",
        json!({"taskId": "OLD-3", "toProjectId": "NEW"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["moved"][0]["to"], "NEW-1", "{moved}");
    let current = version(&setup, original).await;
    let refused = call(
        "task_update",
        json!({"taskId": original, "expectedVersion": current, "archived": false}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(refused["retryCategory"], "do_not_retry");
    assert_eq!(
        refused["message"],
        "OLD-3 was moved to NEW-1. Work on NEW-1; this original stays archived as its history."
    );
    let mara_refused = tool(
        app,
        &mara_token,
        &mara_session,
        "task_update",
        json!({"taskId": original, "expectedVersion": current, "archived": false}),
    )
    .await;
    assert_eq!(mara_refused["outcome"], "conflict", "{mara_refused}");
    assert_eq!(
        mara_refused["message"],
        "OLD-3 was moved to another project; this original stays archived as its history."
    );
    let still_archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE id = $1",
    )
    .bind(original)
    .fetch_one(&setup.pool)
    .await
    .expect("archived");
    assert!(still_archived);
    assert_eq!(version(&setup, original).await, current);
}

/// P0.4: an agent's task comment names people and tasks the way a person's
/// does: "@rook" notifies Rook and "WORK-2" is a backlink on WORK-2, and an
/// edit moves the mention.
#[tokio::test]
async fn an_agents_task_comment_mentions_people_and_references_tasks() {
    use super::{message_edits::add_person, notifications::drain};

    let Some(setup) = setup("mcp_task_comment_mentions").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (rook, _) = add_person(&setup, &[&work], "rook").await;
    let messages = space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &messages, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let session = initialize(app, &mara_token).await.expect("agent MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let mut tasks = Vec::new();
    for title in ["Ship it", "Write the notes"] {
        let created = call("task_create", json!({"projectId": "WORK", "title": title})).await;
        assert_eq!(created["outcome"], "applied", "{created}");
        tasks.push(id(&created["workItemId"]));
    }
    let comment = json!({
        "taskId": tasks[0],
        "text": "@rook see WORK-2 (and not WORK-99 or XWORK-2)",
        "clientRequestId": "comment-1"
    });
    let posted = call("task_comment", comment.clone()).await;
    assert_eq!(posted["outcome"], "applied", "{posted}");
    let comment_id = id(&posted["activityId"]);
    let mentioned = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM work_item_comment_mentions WHERE comment_id = $1",
    )
    .bind(comment_id)
    .fetch_all(&setup.pool)
    .await
    .expect("mentions");
    assert_eq!(mentioned, [rook]);
    let referenced = sqlx::query_scalar::<_, Uuid>(
        "SELECT referenced_work_item_id FROM work_item_comment_references WHERE comment_id = $1",
    )
    .bind(comment_id)
    .fetch_all(&setup.pool)
    .await
    .expect("references");
    assert_eq!(referenced, [tasks[1]]);
    let stored =
        sqlx::query_scalar::<_, String>("SELECT plain_text FROM work_item_comments WHERE id = $1")
            .bind(comment_id)
            .fetch_one(&setup.pool)
            .await
            .expect("plain text");
    assert!(
        stored.contains("@rook") && stored.contains("WORK-2"),
        "{stored}"
    );

    drain(&setup).await;
    let unread_mentions = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'comment_mention' AND read_at IS NULL",
        )
        .bind(rook)
        .fetch_one(&setup.pool)
        .await
        .expect("notifications")
    };
    assert_eq!(unread_mentions().await, 1);

    // The same request again is a replay, and tells nobody twice.
    let replayed = call("task_comment", comment).await;
    assert_eq!(replayed["activityId"], posted["activityId"], "{replayed}");
    drain(&setup).await;
    assert_eq!(unread_mentions().await, 1);

    // Editing it to name the owner moves the mention.
    let owner = super::message_edits::owner_id(&setup.pool).await;
    let edited = call(
        "task_comment_edit",
        json!({"taskId": tasks[0], "commentId": comment_id, "text": "@owner over to you"}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let mentioned = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM work_item_comment_mentions WHERE comment_id = $1",
    )
    .bind(comment_id)
    .fetch_all(&setup.pool)
    .await
    .expect("mentions");
    assert_eq!(mentioned, [owner]);
}
