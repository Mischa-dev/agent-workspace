//! task_create_many and task_update_many (docs/plans/mcp-tools.md, Phase 2):
//! an agent imports a plan with a preview, then reorganizes the board,
//! without the web app. The costly bugs are silent: a preview that leaves
//! rows or uses up task numbers, a batch that half applies, a retry that
//! creates everything twice, a task list that reaches into a project the
//! caller can't, a status change that skips the workflow, an archived or
//! moved task changed through the back door, or a claim left on an archived
//! task.

use std::time::Instant;

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, tool},
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

/// The keys of a list of tasks, or the strings of a list.
fn keys(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .map(|row| match row {
            Value::String(text) => text.clone(),
            row => row["key"]
                .as_str()
                .or_else(|| row["text"].as_str())
                .unwrap_or_else(|| panic!("no key: {row}"))
                .to_owned(),
        })
        .collect()
}

/// Where each problem is and what it's about.
fn problems(result: &Value) -> Vec<(u64, String)> {
    assert_eq!(result["outcome"], "rejected", "{result}");
    result["problems"]
        .as_array()
        .unwrap_or_else(|| panic!("no problems: {result}"))
        .iter()
        .map(|problem| {
            (
                problem["index"].as_u64().expect("index"),
                problem["field"].as_str().expect("field").to_owned(),
            )
        })
        .collect()
}

fn assert_invalid(result: &Value, field: &str, says: &str) {
    assert_eq!(result["outcome"], "invalid_request", "{result}");
    assert_eq!(result["field"], field, "{result}");
    assert!(message(result).contains(says), "{result}");
}

/// Every row a batch could leave behind: tasks, activity, audit, outbox,
/// labels, criteria, stored replays and task numbers.
async fn footprint(setup: &Setup) -> Vec<i64> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM work_items),
               (SELECT count(*) FROM work_item_activity),
               (SELECT count(*) FROM audit_events),
               (SELECT count(*) FROM outbox_events),
               (SELECT count(*) FROM project_labels),
               (SELECT count(*) FROM work_item_acceptance_criteria),
               (SELECT count(*) FROM idempotency_keys),
               (SELECT COALESCE(sum(next_work_item_number), 0)::bigint FROM project_counters)
        "#,
    )
    .fetch_one(&setup.pool)
    .await
    .expect("footprint");
    vec![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7]
}

async fn version(setup: &Setup, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("version")
}

async fn priority(setup: &Setup, task: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT priority FROM work_items WHERE id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("priority")
}

async fn activities(setup: &Setup, task: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_activity WHERE work_item_id = $1")
        .bind(task)
        .fetch_one(&setup.pool)
        .await
        .expect("activities")
}

/// A status column's task keys in board order, top first.
async fn column(
    app: &axum::Router,
    token: &str,
    session: &str,
    project: &str,
    status: &str,
) -> Vec<String> {
    let listed = tool(
        app,
        token,
        session,
        "task_list",
        json!({"projectId": project, "status": status, "includeDone": true, "limit": 200}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    keys(&listed["value"]["tasks"])
}

#[tokio::test]
async fn plan_import_previews_creates_once_and_rejects_as_a_whole() {
    let Some(setup) = setup("mcp_batches_import").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "PLAN", "Plan").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let existing = call(
        "task_create",
        json!({"projectId": "PLAN", "title": "Existing parent"}),
    )
    .await;
    assert_eq!(existing["key"], "PLAN-1", "{existing}");
    let existing = id(&existing["workItemId"]);
    let label = call(
        "label_write",
        json!({"projectId": "PLAN", "name": "Import"}),
    )
    .await;
    assert_eq!(label["outcome"], "ok", "{label}");

    let plan = json!({
        "projectId": "PLAN",
        "tasks": [
            {
                "ref": "epic", "title": "Import plans", "type": "Feature", "status": "Ready",
                "priority": "high", "assignee": "me", "labels": ["import"],
                "acceptanceCriteria": ["Every plan is a task", "  Keys are listed "]
            },
            {
                "ref": "a", "parentRef": "epic", "title": "Parse Markdown",
                "description": "Headings become tasks.\n\n- Lists become subtasks"
            },
            {"parentRef": "a", "title": "Handle nested lists", "status": "Done"},
            {"parentId": "PLAN-1", "title": "Child of existing"}
        ]
    });

    // A preview checks everything for real and leaves nothing behind: no
    // task, activity, audit or outbox row, and no task number used up.
    let before = footprint(&setup).await;
    let mut preview = plan.clone();
    preview["preview"] = json!(true);
    let previewed = call("task_create_many", preview).await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    let value = &previewed["value"];
    assert_eq!(value["preview"], true);
    assert_eq!(value["projectKey"], "PLAN");
    assert_eq!(
        keys(&value["tasks"]),
        ["PLAN-2", "PLAN-3", "PLAN-4", "PLAN-5"]
    );
    let epic = &value["tasks"][0];
    assert!(
        epic.get("id").is_none() && epic.get("url").is_none(),
        "{epic}"
    );
    assert_eq!(epic["ref"], "epic");
    assert_eq!(epic["status"], "Ready");
    assert_eq!(epic["type"], "Feature");
    assert_eq!(epic["assignee"], "Owner");
    assert_eq!(epic["labels"], json!(["Import"]));
    assert_eq!(epic["criteriaCount"], 2);
    assert_eq!(value["tasks"][1]["status"], "Backlog");
    assert_eq!(value["tasks"][1]["parentKey"], "PLAN-2");
    assert_eq!(value["tasks"][2]["parentKey"], "PLAN-3");
    assert_eq!(value["tasks"][3]["parentKey"], "PLAN-1");
    assert_eq!(footprint(&setup).await, before);

    // The real call creates the same tasks, once.
    let mut real = plan.clone();
    real["clientRequestId"] = json!("plan-import-1");
    let created = call("task_create_many", real.clone()).await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let value = &created["value"];
    assert_eq!(value["preview"], false);
    assert_eq!(
        keys(&value["tasks"]),
        ["PLAN-2", "PLAN-3", "PLAN-4", "PLAN-5"]
    );
    for task in value["tasks"].as_array().expect("tasks") {
        assert_eq!(task["version"], 1, "{task}");
        assert!(
            task["url"]
                .as_str()
                .is_some_and(|url| url.contains("/work-items/")),
            "{task}"
        );
    }
    let ids = value["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| id(&task["id"]))
        .collect::<Vec<_>>();
    let after = footprint(&setup).await;
    // Four tasks, each with one activity, audit row and outbox event, plus
    // one audit row for the batch; two criteria; one stored replay.
    assert_eq!(after[0] - before[0], 4);
    assert_eq!(after[1] - before[1], 4);
    assert_eq!(after[2] - before[2], 5);
    assert_eq!(after[3] - before[3], 4);
    assert_eq!(after[5] - before[5], 2);
    assert_eq!(after[6] - before[6], 1);

    let opened = call("task_open", json!({"taskId": "PLAN-2"})).await;
    let task = &opened["value"]["task"];
    assert_eq!(task["itemType"]["name"], "Feature", "{opened}");
    assert_eq!(task["status"]["name"], "Ready");
    assert_eq!(task["priority"], "high");
    assert_eq!(task["assignee"]["displayName"], "Owner");
    assert_eq!(task["labels"], json!(["Import"]));
    assert_eq!(
        keys(&task["acceptanceCriteria"]),
        ["Every plan is a task", "Keys are listed"]
    );
    assert_eq!(keys(&opened["value"]["subtasks"]), ["PLAN-3"]);
    let nested = call("task_open", json!({"taskId": "PLAN-4"})).await;
    assert_eq!(nested["value"]["task"]["status"]["name"], "Done");
    assert_eq!(id(&nested["value"]["task"]["parentWorkItemId"]), ids[1]);
    let child = call("task_open", json!({"taskId": "PLAN-5"})).await;
    assert_eq!(id(&child["value"]["task"]["parentWorkItemId"]), existing);
    // Each new task went to the bottom of its column, in list order.
    assert_eq!(
        column(app, &codex, &session, "PLAN", "Backlog").await,
        ["PLAN-1", "PLAN-3", "PLAN-5"]
    );
    assert_eq!(
        column(app, &codex, &session, "PLAN", "Ready").await,
        ["PLAN-2"]
    );

    // A retry returns the first result and creates nothing.
    let replay = call("task_create_many", real.clone()).await;
    assert_eq!(replay["outcome"], "ok", "{replay}");
    assert_eq!(replay["value"]["idempotentReplay"], true);
    assert_eq!(
        replay["value"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|task| id(&task["id"]))
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(footprint(&setup).await, after);
    let mut changed = real.clone();
    changed["tasks"][0]["title"] = json!("Import every plan");
    let reused = call("task_create_many", changed).await;
    assert_eq!(reused["outcome"], "conflict", "{reused}");
    assert!(message(&reused).contains("clientRequestId"), "{reused}");

    // A batch with problems creates nothing and lists every problem at once.
    let bad = json!({
        "projectId": "PLAN",
        "clientRequestId": "plan-import-2",
        "tasks": [
            {"title": "fine"},
            {"title": "x", "status": "Doing"},
            {"title": "y", "assignee": "@nobody"},
            {"ref": "c", "parentRef": "later", "title": "z"},
            {"ref": "later", "title": "w"}
        ]
    });
    let rejected = call("task_create_many", bad).await;
    assert_eq!(
        problems(&rejected),
        [
            (1, "status".to_owned()),
            (2, "assignee".to_owned()),
            (3, "parentRef".to_owned())
        ]
    );
    let listed = rejected["problems"].as_array().expect("problems");
    assert!(
        message(&listed[0]).contains("Statuses: Backlog, Ready"),
        "{rejected}"
    );
    assert!(message(&listed[1]).contains("@nobody"), "{rejected}");
    assert!(message(&listed[2]).contains("earlier"), "{rejected}");
    assert_eq!(listed[2]["ref"], "c");
    assert!(
        message(&rejected).starts_with("Nothing was created: 3 of 5 tasks have problems"),
        "{rejected}"
    );
    assert_eq!(footprint(&setup).await, after);
    let next = call("task_create", json!({"projectId": "PLAN", "title": "Next"})).await;
    assert_eq!(next["key"], "PLAN-6", "no numbers were used up: {next}");
    // Refused batches aren't stored: the corrected one reuses the ID.
    let fixed = call(
        "task_create_many",
        json!({
            "projectId": "PLAN",
            "clientRequestId": "plan-import-2",
            "tasks": [
                {"title": "fine"},
                {"title": "x", "status": "In Progress"},
                {"title": "y", "assignee": "me"},
                {"ref": "later", "title": "w"},
                {"ref": "c", "parentRef": "later", "title": "z"}
            ]
        }),
    )
    .await;
    assert_eq!(fixed["outcome"], "ok", "{fixed}");
    assert_eq!(
        keys(&fixed["value"]["tasks"]),
        ["PLAN-7", "PLAN-8", "PLAN-9", "PLAN-10", "PLAN-11"]
    );
    assert_eq!(fixed["value"]["tasks"][4]["parentKey"], "PLAN-10");
    assert!(fixed["value"].get("notes").is_none(), "{fixed}");

    // Labels the project doesn't have are a problem, unless the call asks
    // for them to be created; a preview shows which. Titles already on the
    // board are pointed out, not refused.
    let unlabeled = json!({
        "projectId": "PLAN",
        "preview": true,
        "tasks": [{"title": "Phone app", "labels": ["Mobile", "Import"]}, {"title": "Next"}]
    });
    let refused = call("task_create_many", unlabeled.clone()).await;
    assert_eq!(problems(&refused), [(0, "labels".to_owned())]);
    let says = message(&refused["problems"][0]);
    assert!(
        says.contains("No label called \"Mobile\"") && says.contains("createMissingLabels"),
        "{refused}"
    );
    let mut creating = unlabeled.clone();
    creating["createMissingLabels"] = json!(true);
    let labels_before = footprint(&setup).await[4];
    let previewed = call("task_create_many", creating.clone()).await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    assert_eq!(previewed["value"]["labelsCreated"], json!(["Mobile"]));
    assert_eq!(
        previewed["value"]["tasks"][0]["labels"],
        json!(["Import", "Mobile"])
    );
    assert!(
        keys(&previewed["value"]["notes"])
            .iter()
            .any(|note| note == "PLAN-6 already has the title \"Next\"."),
        "{previewed}"
    );
    assert_eq!(footprint(&setup).await[4], labels_before);
    creating["preview"] = json!(false);
    creating["clientRequestId"] = json!("plan-labels");
    let labeled = call("task_create_many", creating).await;
    assert_eq!(labeled["outcome"], "ok", "{labeled}");
    assert_eq!(footprint(&setup).await[4], labels_before + 1);
    // The new label has its own audit row, as label_write's and task_move's
    // do, so its history starts where it was made.
    let label_audits = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT audit.metadata
        FROM audit_events audit
        JOIN project_labels label
          ON label.id = audit.target_id AND label.project_id = audit.project_id
        JOIN projects project ON project.id = label.project_id
        WHERE audit.action = 'work_item_label.mcp_created'
          AND audit.target_kind = 'work_item_label'
          AND project.project_key = 'PLAN' AND label.name = 'Mobile'
        "#,
    )
    .fetch_all(&setup.pool)
    .await
    .expect("label audit");
    assert_eq!(
        label_audits,
        [json!({"changedFields": ["label"], "name": "Mobile", "batch": "task_create_many"})]
    );

    // Too deep: the 21st level is refused, and only that task is named.
    let chain = (1..=21)
        .map(|level| {
            let mut task = json!({"ref": format!("l{level}"), "title": format!("Level {level}")});
            if level > 1 {
                task["parentRef"] = json!(format!("l{}", level - 1));
            }
            task
        })
        .collect::<Vec<_>>();
    let deep = call(
        "task_create_many",
        json!({"projectId": "PLAN", "preview": true, "tasks": chain}),
    )
    .await;
    assert_eq!(problems(&deep), [(20, "parentRef".to_owned())]);
    assert!(
        message(&deep["problems"][0]).contains("20 levels"),
        "{deep}"
    );

    // An archived parent is refused; restore it first.
    let archived = call(
        "task_update",
        json!({"taskId": "PLAN-1", "expectedVersion": 1, "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "applied", "{archived}");
    let under_archived = call(
        "task_create_many",
        json!({"projectId": "PLAN", "preview": true, "tasks": [{"title": "Orphan", "parentId": "PLAN-1"}]}),
    )
    .await;
    assert_eq!(problems(&under_archived), [(0, "parentId".to_owned())]);
    assert!(
        message(&under_archived["problems"][0]).contains("PLAN-1 is archived"),
        "{under_archived}"
    );

    // Request checks come before anything else.
    assert_invalid(
        &call(
            "task_create_many",
            json!({"projectId": "PLAN", "tasks": [{"title": "No ID"}]}),
        )
        .await,
        "clientRequestId",
        "Pass a clientRequestId",
    );
    assert_invalid(
        &call(
            "task_create_many",
            json!({"projectId": "PLAN", "tasks": [], "preview": true}),
        )
        .await,
        "tasks",
        "1 to 100",
    );
    assert_invalid(
        &call(
            "task_create_many",
            json!({"projectId": "PLAN", "tasks": [{"title": "Long"}], "clientRequestId": "a".repeat(129)}),
        )
        .await,
        "clientRequestId",
        "128",
    );
}

#[tokio::test]
async fn bulk_status_change_follows_the_workflow_or_changes_nothing() {
    let Some(setup) = setup("mcp_batches_update").await else {
        return;
    };
    let app = &setup.app;
    let flow = create_project(&setup, "FLOW", "Flow").await;
    create_project(&setup, "DEST", "Destination").await;
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &flow, mara, "member").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let created = call(
        "task_create_many",
        json!({
            "projectId": "FLOW",
            "clientRequestId": "flow-setup",
            "tasks": [
                {"title": "Review one", "status": "Review"},
                {"title": "Review two", "status": "Review"},
                {"title": "Backlog one"}
            ]
        }),
    )
    .await;
    assert_eq!(
        keys(&created["value"]["tasks"]),
        ["FLOW-1", "FLOW-2", "FLOW-3"]
    );
    let task = |index: usize| id(&created["value"]["tasks"][index]["id"]);
    let (r1, r2, b1) = (task(0), task(1), task(2));

    // One task can't go straight to Done, so none do, and the answer says
    // the way there.
    let all_done = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", "FLOW-2", "FLOW-3"], "status": "Done"}),
    )
    .await;
    assert_eq!(problems(&all_done), [(2, "status".to_owned())]);
    assert_eq!(all_done["problems"][0]["task"], "FLOW-3");
    assert!(
        message(&all_done["problems"][0])
            .contains("To reach Done, go Ready, In Progress, Review, Done"),
        "{all_done}"
    );
    assert!(
        message(&all_done).contains(
            "FLOW-3 is in Backlog. The shortest way to Done is Backlog → Ready → In Progress → Review → Done"
        ),
        "{all_done}"
    );
    assert_eq!(version(&setup, r1).await, 1);

    // A preview shows the result and changes nothing.
    let done = json!({
        "taskIds": ["FLOW-1", r2],
        "status": "Done",
        "priority": "urgent",
        "clientRequestId": "done-1"
    });
    let before = footprint(&setup).await;
    let mut preview = done.clone();
    preview["preview"] = json!(true);
    let previewed = call("task_update_many", preview).await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    assert_eq!(previewed["value"]["preview"], true);
    assert_eq!(previewed["value"]["changedCount"], 2);
    assert_eq!(previewed["value"]["tasks"][0]["version"], 2);
    assert_eq!(previewed["value"]["tasks"][0]["status"], "Done");
    assert_eq!(footprint(&setup).await, before);
    assert_eq!(version(&setup, r1).await, 1);

    let applied = call("task_update_many", done.clone()).await;
    assert_eq!(applied["outcome"], "ok", "{applied}");
    let value = &applied["value"];
    assert_eq!(value["preview"], false);
    assert_eq!(
        (
            value["changedCount"].as_u64(),
            value["unchangedCount"].as_u64()
        ),
        (Some(2), Some(0))
    );
    for (index, key) in ["FLOW-1", "FLOW-2"].iter().enumerate() {
        let updated = &value["tasks"][index];
        assert_eq!(updated["key"], *key, "{value}");
        assert_eq!(updated["version"], 2);
        assert_eq!(updated["changedFields"], json!(["status", "priority"]));
        assert_eq!(updated["status"], "Done");
    }
    // One activity, audit row and outbox event per task, plus the batch's
    // audit row.
    let after = footprint(&setup).await;
    assert_eq!(
        (
            after[1] - before[1],
            after[2] - before[2],
            after[3] - before[3]
        ),
        (2, 3, 2)
    );
    let column_done = column(app, &codex, &session, "FLOW", "Done").await;
    assert_eq!(column_done[column_done.len() - 2..], ["FLOW-1", "FLOW-2"]);
    let replay = call("task_update_many", done).await;
    assert_eq!(replay["value"]["idempotentReplay"], true, "{replay}");
    assert_eq!(version(&setup, r1).await, 2);

    // A task changed since it was read stops the whole batch.
    let stale = call(
        "task_update_many",
        json!({
            "taskIds": ["FLOW-1", "FLOW-2"],
            "expectedVersions": {"FLOW-1": 2, r2.to_string(): 1},
            "priority": "low"
        }),
    )
    .await;
    assert_eq!(problems(&stale), [(1, "expectedVersions".to_owned())]);
    assert_eq!(stale["problems"][0]["currentVersion"], 2);
    assert!(
        message(&stale["problems"][0]).contains("Someone changed FLOW-2 first (now version 2)"),
        "{stale}"
    );
    assert_eq!(priority(&setup, r1).await, "urgent");
    assert_invalid(
        &call(
            "task_update_many",
            json!({"taskIds": ["FLOW-1"], "expectedVersions": {"FLOW-3": 1}, "priority": "low"}),
        )
        .await,
        "expectedVersions",
        "expectedVersions names FLOW-3, which isn't in taskIds.",
    );

    // Request checks.
    assert_invalid(
        &call("task_update_many", json!({"taskIds": ["FLOW-1"]})).await,
        "status",
        "Say what to change",
    );
    assert_invalid(
        &call(
            "task_update_many",
            json!({"taskIds": ["FLOW-1"], "archived": true, "priority": "low"}),
        )
        .await,
        "archived",
        "on its own",
    );
    assert_invalid(
        &call(
            "task_update_many",
            json!({"taskIds": ["FLOW-1"], "status": "Doing"}),
        )
        .await,
        "status",
        "No status called \"Doing\" in FLOW",
    );
    let twice = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", r1], "priority": "low"}),
    )
    .await;
    assert_eq!(problems(&twice), [(1, "taskIds".to_owned())]);
    assert!(
        message(&twice["problems"][0]).contains("listed twice (also at 0)"),
        "{twice}"
    );

    // Labels: added in one version per task; an unknown name is refused,
    // never created; the label cap stops the batch.
    let label = call(
        "label_write",
        json!({"projectId": "FLOW", "name": "Mobile"}),
    )
    .await;
    assert_eq!(label["outcome"], "ok", "{label}");
    let activity_before = activities(&setup, b1).await;
    let labeled = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", "FLOW-3"], "addLabels": ["mobile"]}),
    )
    .await;
    assert_eq!(labeled["outcome"], "ok", "{labeled}");
    assert_eq!(
        labeled["value"]["tasks"][1]["changedFields"],
        json!(["labels"])
    );
    assert_eq!(labeled["value"]["tasks"][1]["version"], 2);
    assert_eq!(activities(&setup, b1).await, activity_before + 1);
    assert_invalid(
        &call(
            "task_update_many",
            json!({"taskIds": ["FLOW-1"], "addLabels": ["Nope"]}),
        )
        .await,
        "addLabels",
        "No label called \"Nope\" in FLOW",
    );
    let many = (1..=19)
        .map(|number| format!("L{number}"))
        .collect::<Vec<_>>();
    for name in many.iter().chain(std::iter::once(&"Extra".to_owned())) {
        let made = call("label_write", json!({"projectId": "FLOW", "name": name})).await;
        assert_eq!(made["outcome"], "ok", "{made}");
    }
    let full = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-3"], "addLabels": many}),
    )
    .await;
    assert_eq!(full["outcome"], "ok", "{full}");
    let capped = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", "FLOW-3"], "addLabels": ["Extra"]}),
    )
    .await;
    assert_eq!(problems(&capped), [(1, "addLabels".to_owned())]);
    assert!(
        message(&capped["problems"][0]).contains("at most 20 labels"),
        "{capped}"
    );
    let opened = call("task_open", json!({"taskId": "FLOW-1"})).await;
    assert_eq!(
        opened["value"]["task"]["labels"],
        json!(["Mobile"]),
        "{opened}"
    );

    // Archiving releases claims and says so; an archived task refuses every
    // change but a restore, which goes on its own.
    let claimed = tool(
        app,
        &mara_token,
        &mara_session,
        "task_claim",
        json!({"taskId": "FLOW-2"}),
    )
    .await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    let archived = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", "FLOW-2"], "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(
        archived["value"]["tasks"][0]["changedFields"],
        json!(["archived"])
    );
    assert_eq!(archived["value"]["tasks"][0]["archived"], true);
    assert!(
        keys(&archived["value"]["notes"])
            .iter()
            .any(|note| note == "FLOW-2 was claimed by Mara; archiving released the claim."),
        "{archived}"
    );
    let claims = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_claims WHERE work_item_id = $1",
    )
    .bind(r2)
    .fetch_one(&setup.pool)
    .await
    .expect("claims");
    assert_eq!(claims, 0);
    let listed = column(app, &codex, &session, "FLOW", "Done").await;
    assert!(!listed.contains(&"FLOW-1".to_owned()), "{listed:?}");
    // Archiving again changes nothing, even with the versions read before.
    let again = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1", "FLOW-2"], "archived": true, "expectedVersions": {"FLOW-1": 3}}),
    )
    .await;
    assert_eq!(again["value"]["changedCount"], 0, "{again}");
    let refused = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1"], "priority": "high"}),
    )
    .await;
    assert_eq!(problems(&refused), [(0, "archived".to_owned())]);
    assert!(
        message(&refused["problems"][0]).contains("archived: false"),
        "{refused}"
    );
    let restored = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1"], "archived": false}),
    )
    .await;
    assert_eq!(
        restored["value"]["tasks"][0]["changedFields"],
        json!(["archived"]),
        "{restored}"
    );
    let changed = call(
        "task_update_many",
        json!({"taskIds": ["FLOW-1"], "priority": "high"}),
    )
    .await;
    assert_eq!(
        changed["value"]["tasks"][0]["changedFields"],
        json!(["priority"]),
        "{changed}"
    );

    // The archived original of a moved task stays archived.
    let moved = call(
        "task_move",
        json!({"taskId": "FLOW-3", "toProjectId": "DEST"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let restore_original = call(
        "task_update_many",
        json!({"taskIds": [b1], "archived": false}),
    )
    .await;
    assert_eq!(problems(&restore_original), [(0, "archived".to_owned())]);
    assert!(
        message(&restore_original["problems"][0]).contains("FLOW-3 was moved to DEST-1"),
        "{restore_original}"
    );
}

#[tokio::test]
async fn bulk_tools_check_access_for_every_task() {
    let Some(setup) = setup("mcp_batches_access").await else {
        return;
    };
    let app = &setup.app;
    let plan = create_project(&setup, "PLAN", "Plan").await;
    let other = create_project(&setup, "OTHER", "Other work").await;
    let space = space(&setup).await;
    let (nyx, nyx_token) = create_agent(app, &setup.browser, &space, "Nyx", "nyx").await;
    add_member(&setup, &plan, nyx, "member").await;
    let (vee, vee_token) = create_agent(app, &setup.browser, &space, "Vee", "vee").await;
    add_member(&setup, &plan, vee, "viewer").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let nyx_session = initialize(app, &nyx_token).await.expect("Nyx MCP");
    let vee_session = initialize(app, &vee_token).await.expect("Vee MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for project in ["PLAN", "OTHER"] {
        let created = call(
            "task_create",
            json!({"projectId": project, "title": format!("{project} task")}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
    }
    let plan_task =
        id(&call("task_open", json!({"taskId": "PLAN-1"})).await["value"]["task"]["id"]);

    // Nyx belongs to PLAN only: a task in OTHER is just not found, and
    // nothing in PLAN changes either.
    let nyx_call = |name: &'static str, arguments: Value| {
        let token = nyx_token.clone();
        let session = nyx_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let across = nyx_call(
        "task_update_many",
        json!({"taskIds": ["PLAN-1", "OTHER-1"], "priority": "high"}),
    )
    .await;
    assert_eq!(problems(&across), [(1, "taskIds".to_owned())]);
    let says = message(&across["problems"][0]);
    assert!(
        says.contains("wasn't found, or you can't see it"),
        "{across}"
    );
    assert!(
        !says.contains("Other work") && !message(&across).contains(" is in "),
        "{across}"
    );
    assert_eq!(version(&setup, plan_task).await, 1);
    let tasks_before = footprint(&setup).await[0];
    let foreign_parent = nyx_call(
        "task_create_many",
        json!({"projectId": "PLAN", "clientRequestId": "n1", "tasks": [{"title": "x", "parentId": "OTHER-1"}]}),
    )
    .await;
    assert_eq!(problems(&foreign_parent), [(0, "parentId".to_owned())]);
    assert!(
        message(&foreign_parent["problems"][0]).contains("Parent OTHER-1 wasn't found in PLAN"),
        "{foreign_parent}"
    );
    assert_eq!(footprint(&setup).await[0], tasks_before);
    let elsewhere = nyx_call(
        "task_create_many",
        json!({"projectId": other, "clientRequestId": "n2", "tasks": [{"title": "x"}]}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "forbidden", "{elsewhere}");
    // An agent's own home is the Messages space, which holds no tasks.
    assert_invalid(
        &nyx_call(
            "task_create_many",
            json!({"clientRequestId": "n3", "tasks": [{"title": "x"}]}),
        )
        .await,
        "projectId",
        "Say which project",
    );

    // Viewers and read-only apps change nothing, previews included.
    for (token, session) in [(&vee_token, &vee_session), (&reader, &reader_session)] {
        for (name, arguments) in [
            (
                "task_update_many",
                json!({"taskIds": ["PLAN-1"], "priority": "high"}),
            ),
            (
                "task_update_many",
                json!({"taskIds": ["PLAN-1"], "priority": "high", "preview": true}),
            ),
            (
                "task_create_many",
                json!({"projectId": "PLAN", "preview": true, "tasks": [{"title": "v"}]}),
            ),
        ] {
            let denied = tool(app, token, session, name, arguments).await;
            assert_eq!(denied["outcome"], "permission_denied", "{name}: {denied}");
        }
    }
    assert_eq!(version(&setup, plan_task).await, 1);

    // A person connected for their whole account names the project, and a
    // batch stays in one project.
    assert_invalid(
        &call(
            "task_create_many",
            json!({"clientRequestId": "c1", "tasks": [{"title": "x"}]}),
        )
        .await,
        "projectId",
        "Say which project",
    );
    let mixed = call(
        "task_update_many",
        json!({"taskIds": ["PLAN-1", "OTHER-1"], "priority": "high"}),
    )
    .await;
    assert_eq!(problems(&mixed), [(1, "taskIds".to_owned())]);
    assert!(
        message(&mixed["problems"][0])
            .contains("All tasks must be in one project; OTHER-1 is in OTHER."),
        "{mixed}"
    );
    let named = call(
        "task_update_many",
        json!({"projectId": "PLAN", "taskIds": ["OTHER-1"], "priority": "high"}),
    )
    .await;
    assert_eq!(problems(&named), [(0, "taskIds".to_owned())]);
    assert!(
        message(&named["problems"][0]).contains("OTHER-1 isn't in PLAN."),
        "{named}"
    );

    // A moved task's old key follows it for those who can reach where it
    // went; Nyx, who can't, gets the archived original and changes nothing.
    let moved = call(
        "task_move",
        json!({"taskId": "PLAN-1", "toProjectId": "OTHER"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let followed = call(
        "task_update_many",
        json!({"taskIds": ["PLAN-1"], "priority": "low"}),
    )
    .await;
    assert_eq!(followed["outcome"], "ok", "{followed}");
    assert_eq!(followed["value"]["tasks"][0]["key"], "OTHER-2");
    assert_eq!(
        followed["value"]["tasks"][0]["changedFields"],
        json!(["priority"])
    );
    let original = nyx_call(
        "task_update_many",
        json!({"taskIds": ["PLAN-1"], "priority": "low"}),
    )
    .await;
    assert_eq!(problems(&original), [(0, "archived".to_owned())]);
    assert!(!message(&original).contains("OTHER"), "{original}");
}

/// Phase 2's done-when: an agent imports a Markdown plan with a preview,
/// then reorganizes the board, without the web app.
#[tokio::test]
async fn an_agent_imports_a_plan_and_reorganizes_the_board() {
    let Some(setup) = setup("mcp_batches_flow").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "SHIP", "Ship it").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let label = call(
        "label_write",
        json!({"projectId": "SHIP", "name": "Mobile"}),
    )
    .await;
    assert_eq!(label["outcome"], "ok", "{label}");
    // ## Offline mode
    // - Queue writes while offline
    // - Sync on reconnect
    // - Show sync state
    // ## Release notes
    // ## Crash reporting
    let plan = json!({
        "projectId": "SHIP",
        "createMissingLabels": true,
        "tasks": [
            {
                "ref": "offline", "title": "Offline mode", "type": "Feature",
                "labels": ["Mobile", "Sync"],
                "acceptanceCriteria": ["Works with no network", "Syncs within 5s of reconnecting"]
            },
            {"ref": "queue", "parentRef": "offline", "title": "Queue writes while offline"},
            {"parentRef": "offline", "title": "Sync on reconnect", "labels": ["sync"]},
            {"parentRef": "offline", "title": "Show sync state", "type": "Task"},
            {"title": "Release notes", "type": "Task"},
            {"title": "Crash reporting", "priority": "high"}
        ]
    });
    let mut preview = plan.clone();
    preview["preview"] = json!(true);
    let previewed = call("task_create_many", preview).await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    assert_eq!(
        keys(&previewed["value"]["tasks"]),
        ["SHIP-1", "SHIP-2", "SHIP-3", "SHIP-4", "SHIP-5", "SHIP-6"]
    );
    assert_eq!(previewed["value"]["labelsCreated"], json!(["Sync"]));
    let mut import = plan.clone();
    import["clientRequestId"] = json!("ship-plan-1");
    let imported = call("task_create_many", import).await;
    assert_eq!(imported["outcome"], "ok", "{imported}");
    assert_eq!(
        keys(&imported["value"]["tasks"]),
        keys(&previewed["value"]["tasks"])
    );

    // Pick up the three subtasks: Backlog → Ready, checking nobody touched
    // them since the import.
    let ready = call(
        "task_update_many",
        json!({
            "taskIds": ["SHIP-2", "SHIP-3", "SHIP-4"],
            "expectedVersions": {"SHIP-2": 1, "SHIP-3": 1, "SHIP-4": 1},
            "status": "Ready",
            "assignee": "me",
            "clientRequestId": "ship-ready"
        }),
    )
    .await;
    assert_eq!(ready["outcome"], "ok", "{ready}");
    assert_eq!(ready["value"]["changedCount"], 3);
    assert_eq!(
        column(app, &codex, &session, "SHIP", "Ready").await,
        ["SHIP-2", "SHIP-3", "SHIP-4"]
    );

    // Showing sync state comes first.
    let reordered = call(
        "task_reorder",
        json!({"taskIds": ["SHIP-4"], "beforeTaskId": "SHIP-2"}),
    )
    .await;
    assert_eq!(reordered["outcome"], "ok", "{reordered}");
    assert_eq!(
        column(app, &codex, &session, "SHIP", "Ready").await,
        ["SHIP-4", "SHIP-2", "SHIP-3"]
    );

    // Crash reporting belongs under offline mode too, and syncs.
    let regrouped = call(
        "task_update",
        json!({
            "taskId": "SHIP-6", "expectedVersion": 1, "parentId": "SHIP-1",
            "addLabels": ["sync"], "clientRequestId": "ship-regroup"
        }),
    )
    .await;
    assert_eq!(regrouped["outcome"], "applied", "{regrouped}");
    assert_eq!(regrouped["changedFields"], json!(["parent", "labels"]));
    let parent = call("task_open", json!({"taskId": "SHIP-1"})).await;
    assert_eq!(
        keys(&parent["value"]["subtasks"]),
        ["SHIP-2", "SHIP-3", "SHIP-4", "SHIP-6"],
        "{parent}"
    );
    assert_eq!(parent["value"]["task"]["labels"], json!(["Mobile", "Sync"]));

    // Its history tells the story, one entry per change.
    let opened = call(
        "task_open",
        json!({"taskId": "SHIP-4", "include": ["history"]}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let entries = opened["value"]["history"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["text"].as_str().expect("text").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 3, "{opened}");
    assert_eq!(entries[0], "Created this task as a subtask of SHIP-1");
    assert!(
        entries[1].contains("from Backlog to Ready") && entries[1].contains("Assignee: none → "),
        "{entries:?}"
    );
    assert!(entries[2].contains("position 1"), "{entries:?}");
}

/// A 100-task plan with real descriptions fits one call (over the old
/// 256 KiB body limit), and a 200-task change goes through in one
/// transaction. The timings are printed to compare hosts.
#[tokio::test]
async fn big_batches_fit_one_call() {
    let Some(setup) = setup("mcp_batches_big").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "BIG", "Big plan").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let description = "Context for this step of the plan, as an agent would write it.\n".repeat(50);
    for (batch, request) in [(0, "big-1"), (1, "big-2")] {
        let tasks = (1..=100)
            .map(|number| {
                json!({
                    "title": format!("Step {}", batch * 100 + number),
                    "description": description,
                    "labels": ["Bulk"],
                    "acceptanceCriteria": ["The step is done", "Someone checked it"]
                })
            })
            .collect::<Vec<_>>();
        let arguments = json!({
            "projectId": "BIG",
            "createMissingLabels": true,
            "clientRequestId": request,
            "tasks": tasks
        });
        assert!(
            serde_json::to_vec(&arguments).expect("JSON").len() > 256 * 1024,
            "the plan is bigger than the old limit"
        );
        let started = Instant::now();
        let created = call("task_create_many", arguments).await;
        eprintln!("task_create_many of 100 tasks took {:?}", started.elapsed());
        assert_eq!(created["outcome"], "ok", "{created}");
        assert_eq!(
            created["value"]["tasks"].as_array().map(Vec::len),
            Some(100)
        );
    }
    let all = (1..=200)
        .map(|number| format!("BIG-{number}"))
        .collect::<Vec<_>>();
    let started = Instant::now();
    let updated = call(
        "task_update_many",
        json!({"taskIds": all, "status": "Ready", "priority": "high", "clientRequestId": "big-ready"}),
    )
    .await;
    eprintln!("task_update_many of 200 tasks took {:?}", started.elapsed());
    assert_eq!(updated["outcome"], "ok", "{updated}");
    assert_eq!(updated["value"]["changedCount"], 200);
    assert_eq!(column(app, &codex, &session, "BIG", "Ready").await, all);
}
