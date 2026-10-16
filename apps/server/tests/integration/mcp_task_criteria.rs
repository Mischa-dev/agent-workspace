//! task_criteria and task_open's history (docs/plans/mcp-tools.md, Phase 2).
//! The costly bugs are silent: a batch that half-applies, a retry that
//! applies twice, a position that names the wrong criterion, a web edit that
//! drifts from the MCP one, a criterion from another task changed through
//! this one, and history that names a private thread or another project's
//! task to someone who can't see it.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
};

fn id(value: &Value) -> Uuid {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not an ID: {value}"))
        .parse()
        .expect("UUID")
}

fn message(result: &Value) -> &str {
    result["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no message: {result}"))
}

fn texts(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .map(|row| {
            row["text"]
                .as_str()
                .unwrap_or_else(|| panic!("no text: {row}"))
                .to_owned()
        })
        .collect()
}

fn kinds(entries: &Value) -> Vec<String> {
    entries
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {entries}"))
        .iter()
        .map(|entry| entry["kind"].as_str().expect("kind").to_owned())
        .collect()
}

fn assert_refused(result: &Value, field: &str, says: &str) {
    assert_eq!(result["outcome"], "invalid_request", "{result}");
    assert_eq!(result["field"], field, "{result}");
    assert!(message(result).contains(says), "{result}");
}

/// Activity, audit and outbox rows.
async fn side_effects(setup: &Setup) -> (i64, i64, i64) {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("side effects")
}

/// The task as the web app reads it.
async fn web_item(setup: &Setup, project: &str, task: Uuid) -> Value {
    let (status, item) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/work-items/{task}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    item
}

async fn status_id(setup: &Setup, project: &str, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1::uuid AND name = $2",
    )
    .bind(project)
    .bind(name)
    .fetch_one(&setup.pool)
    .await
    .expect("status")
}

#[tokio::test]
async fn criteria_change_all_together_and_retry_safely() {
    let Some(setup) = setup("mcp_criteria_batch").await else {
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
    let created = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Export archived items"}),
    )
    .await;
    assert_eq!(created["version"], 1, "{created}");
    let task = id(&created["workItemId"]);

    // Three adds are one change: one version, one audit row, one outbox
    // event, and an activity each.
    let first = json!({
        "taskId": "WORK-1",
        "expectedVersion": 1,
        "clientRequestId": "c1",
        "operations": [
            {"op": "add", "text": "Exports include archived items"},
            {"op": "add", "text": "Docs updated"},
            {"op": "add", "text": "Tests pass", "position": 1}
        ]
    });
    let before = side_effects(&setup).await;
    let added = call("task_criteria", first.clone()).await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let value = &added["value"];
    assert_eq!(value["version"], 2);
    assert_eq!(value["key"], "WORK-1");
    assert_eq!(id(&value["taskId"]), task);
    assert!(
        value["url"]
            .as_str()
            .expect("url")
            .ends_with(&format!("/projects/{work}/work-items/{task}")),
        "{value}"
    );
    assert_eq!(
        texts(&value["criteria"]),
        [
            "Tests pass",
            "Exports include archived items",
            "Docs updated"
        ]
    );
    assert_eq!(
        (value["total"].clone(), value["completed"].clone()),
        (json!(3), json!(0))
    );
    let operations = value["operations"].as_array().expect("operations");
    assert_eq!(operations.len(), 3);
    assert!(
        operations
            .iter()
            .all(|operation| operation["changed"] == true && operation["op"] == "add"),
        "{value}"
    );
    assert_eq!(operations[2]["criterionId"], value["criteria"][0]["id"]);
    let after = side_effects(&setup).await;
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        (3, 1, 1)
    );

    // A retry answers the same, and nothing is applied twice.
    let replay = call("task_criteria", first).await;
    assert_eq!(replay["value"]["idempotentReplay"], true, "{replay}");
    assert_eq!(replay["value"]["version"], 2);
    assert_eq!(side_effects(&setup).await, after);
    let item = web_item(&setup, &work, task).await;
    assert_eq!(item["version"], 2);
    assert_eq!(
        item["acceptanceCriteria"]
            .as_array()
            .expect("criteria")
            .len(),
        3
    );
    let reused = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 2, "clientRequestId": "c1",
            "operations": [{"op": "check", "criterion": "1"}]
        }),
    )
    .await;
    assert_refused(&reused, "clientRequestId", "already used");

    // All or nothing: a bad operation leaves the good ones unwritten, and
    // the clientRequestId stays free for the corrected call.
    let half = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 2, "clientRequestId": "c5",
            "operations": [
                {"op": "check", "criterion": "1"},
                {"op": "remove", "criterion": Uuid::now_v7().to_string()}
            ]
        }),
    )
    .await;
    assert_refused(&half, "operations[1].criterion", "has no criterion");
    assert!(message(&half).contains("version 2"), "{half}");
    let item = web_item(&setup, &work, task).await;
    assert_eq!(item["version"], 2);
    assert_eq!(item["acceptanceCriteria"][0]["completed"], false, "{item}");
    assert_eq!(side_effects(&setup).await, after);
    assert_refused(
        &call(
            "task_criteria",
            json!({
                "taskId": "WORK-1", "expectedVersion": 2,
                "operations": [
                    {"op": "remove", "criterion": "2"},
                    {"op": "edit", "criterion": "2", "text": "x"}
                ]
            }),
        )
        .await,
        "operations[1].criterion",
        "operation 1 removed",
    );
    assert_refused(
        &call(
            "task_criteria",
            json!({
                "taskId": "WORK-1", "expectedVersion": 2,
                "operations": [{"op": "move", "criterion": "1", "position": 9}]
            }),
        )
        .await,
        "operations[0].position",
        "Use 1 to 3",
    );
    let stale = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 1,
            "operations": [{"op": "check", "criterion": "1"}]
        }),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert!(message(&stale).contains("now version 2"), "{stale}");

    // Four operations, one version. Positions in `criterion` name the list
    // as it was read; checking a checked criterion changes nothing.
    let before = side_effects(&setup).await;
    let changed = call(
        "task_criteria",
        json!({
            "taskId": task, "expectedVersion": 2, "clientRequestId": "c5",
            "operations": [
                {"op": "check", "criterion": 1},
                {"op": "edit", "criterion": "#2", "text": "Exports include archived and deleted items"},
                {"op": "move", "criterion": "3", "position": 1},
                {"op": "check", "criterion": "1"}
            ]
        }),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let value = &changed["value"];
    assert_eq!(value["version"], 3);
    assert_eq!(
        texts(&value["criteria"]),
        [
            "Docs updated",
            "Tests pass",
            "Exports include archived and deleted items"
        ]
    );
    assert_eq!(value["criteria"][1]["completed"], true);
    assert_eq!(value["criteria"][1]["completedBy"], "Owner");
    assert_eq!(value["criteria"][2]["completed"], false);
    assert!(value["criteria"][2].get("completedBy").is_none(), "{value}");
    assert_eq!(value["completed"], 1);
    let changed_flags = value["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| operation["changed"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        changed_flags,
        [json!(true), json!(true), json!(true), json!(false)]
    );
    let after = side_effects(&setup).await;
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        (3, 1, 1)
    );
    let (status, timeline) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/{task}/timeline?limit=10"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    let newest = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .take(4)
        .map(|entry| entry["activity"]["kind"].as_str().expect("kind").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        newest,
        [
            "criterion_updated",
            "criterion_updated",
            "criterion_completed",
            "criterion_created"
        ]
    );
    // The web timeline shows the app it was done through.
    assert_eq!(timeline["entries"][0]["activity"]["via"], "Codex");
    let docs = id(&value["criteria"][0]["id"]);
    let moved = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM work_item_activity WHERE work_item_id = $1 AND payload->>'criterionId' = $2 AND kind = 'criterion_updated'",
    )
    .bind(task)
    .bind(docs.to_string())
    .fetch_one(&setup.pool)
    .await
    .expect("move payload");
    assert_eq!(
        moved,
        json!({"criterionId": docs, "position": 1, "fields": ["position"]})
    );
    let audit = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE target_id = $1 AND action = 'work_item.mcp_criteria_changed' AND request_id = 'c5'",
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("audit");
    assert_eq!(audit["version"], 3, "{audit}");
    assert_eq!(audit["changes"].as_array().expect("changes").len(), 3);

    // The web app edits criteria through the same command, on top.
    let tests_pass = value["criteria"][1]["id"].as_str().expect("criterion");
    let (status, patched) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/work-items/{task}/acceptance-criteria/{tests_pass}"),
            Some("web-uncheck"),
            json!({"expectedVersion": 3, "completed": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["version"], 4);
    let opened = call("task_open", json!({"taskId": "WORK-1"})).await;
    let criteria = &opened["value"]["task"]["acceptanceCriteria"];
    assert_eq!(criteria[1]["text"], "Tests pass", "{opened}");
    assert_eq!(criteria[1]["completed"], false, "{opened}");

    // Nothing to do changes nothing: no version, no activity.
    let before = side_effects(&setup).await;
    let unchanged = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 4,
            "operations": [{"op": "uncheck", "criterion": "2"}]
        }),
    )
    .await;
    assert_eq!(unchanged["outcome"], "ok", "{unchanged}");
    assert_eq!(unchanged["value"]["version"], 4);
    assert_eq!(unchanged["value"]["operations"][0]["changed"], false);
    let after = side_effects(&setup).await;
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        (0, 1, 0)
    );
}

#[tokio::test]
async fn criteria_follow_roles_tasks_and_archives() {
    let Some(setup) = setup("mcp_criteria_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (vee, vee_token) = create_agent(app, &setup.browser, &space, "Vee", "vee").await;
    add_member(&setup, &work, vee, "viewer").await;
    let (_, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (kai, kai_token) = create_agent(app, &setup.browser, &space, "Kai", "kai").await;
    add_member(&setup, &work, kai, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let codex_session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let vee_session = initialize(app, &vee_token).await.expect("Vee MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let kai_session = initialize(app, &kai_token).await.expect("Kai MCP");
    let created = tool(
        app,
        &codex,
        &codex_session,
        "task_create",
        json!({
            "projectId": "WORK",
            "title": "Guard the criteria",
            "acceptanceCriteria": ["Only members change it"]
        }),
    )
    .await;
    let task = id(&created["workItemId"]);
    let other = tool(
        app,
        &codex,
        &codex_session,
        "task_create",
        json!({
            "projectId": "WORK",
            "title": "Another task",
            "acceptanceCriteria": ["Stays as it is"]
        }),
    )
    .await;
    let other_task = id(&other["workItemId"]);
    let check = |task: Value, version: i64| {
        json!({
            "taskId": task, "expectedVersion": version,
            "operations": [{"op": "check", "criterion": "1"}]
        })
    };

    let read_only = tool(
        app,
        &reader,
        &reader_session,
        "task_criteria",
        check(json!("WORK-1"), 1),
    )
    .await;
    assert_eq!(read_only["outcome"], "permission_denied", "{read_only}");
    let viewer = tool(
        app,
        &vee_token,
        &vee_session,
        "task_criteria",
        check(json!("WORK-1"), 1),
    )
    .await;
    assert_eq!(viewer["outcome"], "permission_denied", "{viewer}");
    assert_eq!(message(&viewer), "You can't change tasks in that project.");
    let outsider = tool(
        app,
        &mara_token,
        &mara_session,
        "task_criteria",
        check(json!("WORK-1"), 1),
    )
    .await;
    assert_eq!(outsider["outcome"], "not_found", "{outsider}");
    let by_id = tool(
        app,
        &mara_token,
        &mara_session,
        "task_criteria",
        check(json!(task), 1),
    )
    .await;
    assert_ne!(by_id["outcome"], "ok", "{by_id}");
    let item = web_item(&setup, &work, task).await;
    assert_eq!(item["version"], 1);
    assert_eq!(item["acceptanceCriteria"][0]["completed"], false);

    // A criterion is found only on its own task: another task's criterion
    // ID is unknown here, and its text never shows.
    let foreign = web_item(&setup, &work, other_task).await["acceptanceCriteria"][0]["id"]
        .as_str()
        .expect("criterion")
        .to_owned();
    let crossed = tool(
        app,
        &kai_token,
        &kai_session,
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 1,
            "operations": [{"op": "check", "criterion": foreign}]
        }),
    )
    .await;
    assert_refused(
        &crossed,
        "operations[0].criterion",
        "WORK-1 has no criterion",
    );
    assert!(!message(&crossed).contains("Stays as it is"), "{crossed}");
    let other_item = web_item(&setup, &work, other_task).await;
    assert_eq!(other_item["version"], 1);
    assert_eq!(other_item["acceptanceCriteria"][0]["completed"], false);

    // An agent that's a member checks it off as itself.
    let checked = tool(
        app,
        &kai_token,
        &kai_session,
        "task_criteria",
        check(json!("WORK-1"), 1),
    )
    .await;
    assert_eq!(checked["outcome"], "ok", "{checked}");
    assert_eq!(checked["value"]["version"], 2);
    assert_eq!(checked["value"]["criteria"][0]["completedBy"], "Kai");
    let completer = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT completed_by_actor_id FROM work_item_acceptance_criteria WHERE work_item_id = $1",
    )
    .bind(task)
    .fetch_one(&setup.pool)
    .await
    .expect("completer");
    assert_eq!(completer, Some(kai));

    // An archived task refuses changes, whatever version is named.
    let (status, archived) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/{task}/archive"),
            Some("criteria-archive"),
            json!({"expectedVersion": 2}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    for version in [3, 2] {
        let refused = tool(
            app,
            &codex,
            &codex_session,
            "task_criteria",
            json!({
                "taskId": task, "expectedVersion": version,
                "operations": [{"op": "uncheck", "criterion": "1"}]
            }),
        )
        .await;
        assert_eq!(refused["outcome"], "conflict", "{refused}");
        assert!(message(&refused).contains("archived"), "{refused}");
    }
    assert_eq!(web_item(&setup, &work, task).await["version"], 3);
}

#[tokio::test]
async fn history_shows_who_did_what_and_hides_private_threads() {
    let Some(setup) = setup("mcp_history").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let created = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Offline mode"}),
    )
    .await;
    let task = id(&created["workItemId"]);
    let ready = call(
        "task_update",
        json!({"taskId": "WORK-1", "expectedVersion": 1, "status": "Ready"}),
    )
    .await;
    assert_eq!(ready["version"], 2, "{ready}");
    let (status, described) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{work}/work-items/{task}"),
            Some("history-describe"),
            json!({"expectedVersion": 2, "description": "Works with no network"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{described}");
    let (status, moved) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/{task}/move"),
            Some("history-move"),
            json!({
                "expectedVersion": 3,
                "targetStatusId": status_id(&setup, &work, "In Progress").await,
                "targetPosition": 1
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let commented = call(
        "task_comment",
        json!({"taskId": "WORK-1", "text": "Repro attached", "clientRequestId": "history-comment"}),
    )
    .await;
    assert_eq!(commented["outcome"], "applied", "{commented}");
    // A criterion added in a call can't be named in that call (its ID isn't
    // known yet), so checking it takes a second call.
    let added = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 4,
            "operations": [{"op": "add", "text": "Mirrors prepare"}]
        }),
    )
    .await;
    assert_eq!(added["value"]["version"], 5, "{added}");
    let checked = call(
        "task_criteria",
        json!({
            "taskId": "WORK-1", "expectedVersion": 5,
            "operations": [{"op": "check", "criterion": "1"}]
        }),
    )
    .await;
    assert_eq!(checked["value"]["version"], 6, "{checked}");

    let opened = call(
        "task_open",
        json!({"taskId": "WORK-1", "include": ["history"]}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let history = &opened["value"]["history"];
    assert_eq!(history["total"], 7, "{history}");
    assert!(history.get("olderCursor").is_none(), "{history}");
    let entries = &history["entries"];
    assert_eq!(
        kinds(entries),
        [
            "work_item_created",
            "work_item_updated",
            "work_item_updated",
            "work_item_updated",
            "comment",
            "criterion_created",
            "criterion_completed"
        ]
    );
    assert_eq!(entries[0]["text"], "Created this task");
    assert_eq!(entries[0]["by"], "Owner");
    assert_eq!(entries[0]["byKind"], "person");
    assert_eq!(entries[0]["via"], "Codex");
    assert_eq!(entries[1]["fromStatus"], "Backlog", "{entries}");
    assert_eq!(entries[1]["toStatus"], "Ready");
    assert_eq!(entries[1]["text"], "Moved from Backlog to Ready");
    assert_eq!(entries[1]["via"], "Codex");
    assert_eq!(entries[2]["changedFields"], json!(["description"]));
    assert_eq!(
        entries[2]["text"],
        "Edited the description:\n+Works with no network"
    );
    assert!(entries[2].get("via").is_none(), "{entries}");
    assert_eq!(entries[3]["fromStatus"], "Ready");
    assert_eq!(entries[3]["toStatus"], "In Progress");
    assert_eq!(
        entries[3]["text"],
        "Moved from Ready to In Progress (position 1)"
    );
    assert_eq!(entries[4]["text"], "Repro attached");
    assert_eq!(entries[4]["via"], "Codex");
    assert!(
        entries[5]["text"]
            .as_str()
            .expect("text")
            .contains("Mirrors prepare"),
        "{entries}"
    );
    assert_eq!(
        entries[5]["criterionId"],
        added["value"]["criteria"][0]["id"]
    );
    assert_eq!(entries[6]["text"], "Checked “Mirrors prepare”");
    // The latest comments stay as they were.
    assert_eq!(opened["value"]["comments"][0]["text"], "Repro attached");

    // Pages go back from the newest, each oldest first.
    let page = call(
        "task_open",
        json!({"taskId": "WORK-1", "include": ["history"], "historyLimit": 2}),
    )
    .await;
    let history = &page["value"]["history"];
    assert_eq!(
        kinds(&history["entries"]),
        ["criterion_created", "criterion_completed"]
    );
    assert_eq!(history["total"], 7);
    let cursor = history["olderCursor"].as_str().expect("cursor").to_owned();
    let earlier = call(
        "task_open",
        json!({
            "taskId": "WORK-1", "include": ["history"], "historyLimit": 2,
            "historyBefore": cursor
        }),
    )
    .await;
    assert_eq!(
        kinds(&earlier["value"]["history"]["entries"]),
        ["work_item_updated", "comment"],
        "{earlier}"
    );
    let second = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Another task"}),
    )
    .await;
    let commented = call(
        "task_comment",
        json!({"taskId": second["workItemId"], "text": "Noted", "clientRequestId": "history-other"}),
    )
    .await;
    assert_eq!(commented["outcome"], "applied", "{commented}");
    let other_page = call(
        "task_open",
        json!({"taskId": "WORK-2", "include": ["history"], "historyLimit": 1}),
    )
    .await;
    let other_cursor = other_page["value"]["history"]["olderCursor"]
        .as_str()
        .expect("other cursor")
        .to_owned();
    for cursor in [other_cursor.as_str(), "not-a-cursor"] {
        assert_refused(
            &call(
                "task_open",
                json!({"taskId": "WORK-1", "include": ["history"], "historyBefore": cursor}),
            )
            .await,
            "historyBefore",
            "olderCursor you were given for this same task",
        );
    }

    // A thread in a channel Mara isn't in is linked, then the channel goes
    // members-only: history names it for the owner but not for Mara.
    let channel = call(
        "channel_create",
        json!({"projectId": "WORK", "name": "offline"}),
    )
    .await;
    let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
    let sent = call(
        "message_send",
        json!({"conversationId": channel_id, "text": "Let's plan offline mode"}),
    )
    .await;
    let thread = call(
        "thread_start",
        json!({
            "conversationId": channel_id,
            "fromMessageId": sent["value"]["messageId"],
            "name": "offline-plan"
        }),
    )
    .await;
    assert_eq!(thread["outcome"], "ok", "{thread}");
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
    let linked = call(
        "task_link",
        json!({"taskId": "WORK-1", "conversationId": thread_id}),
    )
    .await;
    assert_eq!(linked["value"]["changed"], true, "{linked}");
    let thread_name =
        sqlx::query_scalar::<_, String>("SELECT name FROM conversations WHERE id = $1::uuid")
            .bind(&thread_id)
            .fetch_one(&setup.pool)
            .await
            .expect("thread name");
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1::uuid")
        .bind(&channel_id)
        .execute(&setup.pool)
        .await
        .expect("make the channel private");
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = $1::uuid AND role = 'owner'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1::uuid, $2::uuid, $3, $3)",
    )
    .bind(&work)
    .bind(&channel_id)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("the owner stays in the channel");
    let latest = json!({"taskId": "WORK-1", "include": ["history"], "historyLimit": 1});
    let owners = call("task_open", latest.clone()).await;
    let entry = &owners["value"]["history"]["entries"][0];
    assert_eq!(entry["kind"], "thread_linked", "{owners}");
    assert_eq!(entry["text"], format!("Linked thread #{thread_name}"));
    let maras = tool(app, &mara_token, &mara_session, "task_open", latest).await;
    assert_eq!(maras["outcome"], "ok", "{maras}");
    let entry = &maras["value"]["history"]["entries"][0];
    assert_eq!(entry["kind"], "thread_linked", "{maras}");
    assert_eq!(entry["text"], "Linked a thread you can't see");
    assert!(!maras.to_string().contains(&thread_name), "{maras}");
    assert_eq!(entry["byKind"], "person");

    // include takes only history, and the history arguments need it.
    assert_refused(
        &call(
            "task_open",
            json!({"taskId": "WORK-1", "include": ["nope"]}),
        )
        .await,
        "include",
        "Use include: [\"history\"]",
    );
    assert_refused(
        &call(
            "task_open",
            json!({"taskId": "WORK-1", "historyBefore": cursor}),
        )
        .await,
        "historyBefore",
        "Pass include: [\"history\"] with historyBefore",
    );
    assert_refused(
        &call(
            "task_open",
            json!({"taskId": "WORK-1", "include": ["history"], "historyLimit": 0}),
        )
        .await,
        "historyLimit",
        "from 1 to 100",
    );
    let plain = call("task_open", json!({"taskId": "WORK-1"})).await;
    assert_eq!(plain["outcome"], "ok", "{plain}");
    assert!(plain["value"].get("history").is_none(), "{plain}");
}

#[tokio::test]
async fn history_of_a_moved_task_points_back_only_where_the_reader_reaches() {
    let Some(setup) = setup("mcp_history_moved").await else {
        return;
    };
    let app = &setup.app;
    let old = create_project(&setup, "OLD", "Old work").await;
    let new = create_project(&setup, "NEW", "New work").await;
    let space = space(&setup).await;
    // Mara works only in the old project, Nia only in the new one.
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    add_member(&setup, &old, mara, "member").await;
    let (nia, nia_token) = create_agent(app, &setup.browser, &space, "Nia", "nia").await;
    add_member(&setup, &new, nia, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let nia_session = initialize(app, &nia_token).await.expect("Nia MCP");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let created = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "OLD", "title": "Private mirrors"}),
    )
    .await;
    let original = id(&created["workItemId"]);
    let moved = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "OLD-1", "toProjectId": "NEW"}),
    )
    .await;
    assert_eq!(moved["value"]["moved"][0]["to"], "NEW-1", "{moved}");

    let history = |token: &str, session: &str, task: Value| {
        let (token, session) = (token.to_owned(), session.to_owned());
        async move {
            let opened = tool(
                app,
                &token,
                &session,
                "task_open",
                json!({"taskId": task, "include": ["history"]}),
            )
            .await;
            assert_eq!(opened["outcome"], "ok", "{opened}");
            opened["value"]["history"]["entries"].clone()
        }
    };
    let entries = history(&codex, &session, json!("NEW-1")).await;
    assert_eq!(entries[0]["kind"], "work_item_created", "{entries}");
    assert_eq!(entries[0]["text"], "Moved here from OLD-1");
    assert_eq!(id(&entries[0]["fromTaskId"]), original);
    let entries = history(&codex, &session, json!(original)).await;
    let last = entries.as_array().expect("entries").last().expect("entry");
    assert_eq!(last["kind"], "work_item_archived", "{entries}");
    assert_eq!(last["text"], "Moved to NEW-1; this copy was archived");

    // Nia can't reach the old project, so she gets no way back to it.
    let entries = history(&nia_token, &nia_session, json!("NEW-1")).await;
    assert_eq!(entries[0]["text"], "Moved here from OLD-1", "{entries}");
    assert!(entries[0].get("fromTaskId").is_none(), "{entries}");
    // Mara opens the archived original, and isn't told where it went.
    let entries = history(&mara_token, &mara_session, json!("OLD-1")).await;
    let last = entries.as_array().expect("entries").last().expect("entry");
    assert_eq!(
        last["text"], "Moved to another project; this copy was archived",
        "{entries}"
    );
    // (task_move's own comment on the original, which predates history,
    // names where it went; the history line itself doesn't.)
    assert!(!last.to_string().contains("NEW-1"), "{last}");
    assert!(!last.to_string().contains(&new), "{last}");

    // Moved on again, the task in between still points back to the
    // original, though the original's alias now points past it.
    create_project(&setup, "THIRD", "Third work").await;
    let between = id(&moved["value"]["moved"][0]["id"]);
    let moved_on = tool(
        app,
        &codex,
        &session,
        "task_move",
        json!({"taskId": "NEW-1", "toProjectId": "THIRD"}),
    )
    .await;
    assert_eq!(moved_on["value"]["moved"][0]["to"], "THIRD-1", "{moved_on}");
    let entries = history(&codex, &session, json!(between)).await;
    assert_eq!(entries[0]["text"], "Moved here from OLD-1", "{entries}");
    assert_eq!(id(&entries[0]["fromTaskId"]), original);
    let entries = history(&codex, &session, json!("THIRD-1")).await;
    assert_eq!(entries[0]["text"], "Moved here from NEW-1", "{entries}");
    assert_eq!(id(&entries[0]["fromTaskId"]), between);
}
