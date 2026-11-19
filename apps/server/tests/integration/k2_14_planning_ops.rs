//! K2.14: claims, links and planning as operations. The costly bugs are
//! silent: a person and an agent holding the same task, a handoff note that
//! never reaches whoever picks the task up, a bulk call that half applies,
//! and a plan import that makes a second copy of work the space already
//! has (AWD-168).

use agent_workspace_server::ai::plans::{self, Step};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{connect_app, create_agent, initialize, send, tool},
    world::{Setup, create_project, setup},
};

/// An op through the web's generated route, as the signed-in owner.
async fn web(setup: &Setup, name: &str, key: &str, input: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", setup.organization_id),
            Some(key),
            input,
        ),
    )
    .await
}

fn ok(result: &Value) -> &Value {
    assert_eq!(result["outcome"], "ok", "{result}");
    &result["value"]
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {value}"))
        .iter()
        .map(|item| item.as_str().expect("text").to_owned())
        .collect()
}

#[tokio::test]
async fn a_person_and_an_agent_share_one_claim_and_its_handoff() {
    let Some(setup) = setup("k214_claims").await else {
        return;
    };
    let app = &setup.app;
    let space = create_project(&setup, "CLM", "Claims").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let codex_session = initialize(app, &codex).await.expect("Codex MCP");
    let created = tool(
        app,
        &codex,
        &codex_session,
        "task_create",
        json!({"projectId": "CLM", "title": "Shared work"}),
    )
    .await;
    assert_eq!(created["value"]["key"], "CLM-1", "{created}");
    let (_, agent) = create_agent(app, &setup.browser, &space, "Builder", "builder").await;
    let agent_session = initialize(app, &agent).await.expect("agent MCP");

    // The person claims from the web, through the same operation agents use.
    let (status, claimed) = web(
        &setup,
        "task_claim",
        "claim-1",
        json!({"taskId": "CLM-1", "note": "Looking into it"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let claim = ok(&claimed);
    assert_eq!(claim["claimedBy"], "Owner", "{claim}");
    assert_eq!(claim["note"], "Looking into it");
    assert!(claim.get("handoff").is_none(), "{claim}");

    // The agent can't take it while the person holds it, and is told who.
    let refused = tool(
        app,
        &agent,
        &agent_session,
        "task_claim",
        json!({"taskId": "CLM-1"}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(refused["code"], "claimed", "{refused}");
    assert_eq!(refused["details"]["current"]["claimedBy"], "Owner");
    // Nor release it: it isn't theirs.
    let refused = tool(
        app,
        &agent,
        &agent_session,
        "task_release",
        json!({"taskId": "CLM-1", "handoff": "Not mine"}),
    )
    .await;
    assert_eq!(refused["code"], "not_holder", "{refused}");

    // The person stops with a handoff note; the agent's claim returns it.
    let (status, released) = web(
        &setup,
        "task_release",
        "release-1",
        json!({"taskId": "CLM-1", "handoff": "Got to step 2; the migration is next."}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{released}");
    assert_eq!(ok(&released)["released"], true);
    assert_eq!(ok(&released)["handoffSaved"], true);
    let taken = tool(
        app,
        &agent,
        &agent_session,
        "task_claim",
        json!({"taskId": "CLM-1", "minutes": 30}),
    )
    .await;
    let taken = ok(&taken);
    assert_eq!(taken["claimedBy"], "Builder", "{taken}");
    assert_eq!(
        taken["handoff"]["note"],
        "Got to step 2; the migration is next."
    );
    assert_eq!(taken["handoff"]["by"], "Owner");

    // Ending someone else's claim takes saying so, and managing the space.
    let (_, kept) = web(
        &setup,
        "task_release",
        "release-2",
        json!({"taskId": "CLM-1"}),
    )
    .await;
    assert_eq!(ok(&kept)["released"], false, "{kept}");
    let (status, ended) = web(
        &setup,
        "task_release",
        "release-3",
        json!({"taskId": "CLM-1", "anyone": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert_eq!(ok(&ended)["released"], true);
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_claims WHERE expires_at > now()",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("claims");
    assert_eq!(held, 0);
}

#[tokio::test]
async fn planning_tools_fill_the_gaps_agents_hit() {
    let Some(setup) = setup("k214_planning").await else {
        return;
    };
    let app = &setup.app;
    let space = create_project(&setup, "PLN", "Planning").await;
    let space: Uuid = space.parse().expect("space");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        let pool = setup.pool.clone();
        // A long batch is finished by its job (K2.21).
        async move {
            super::support::long::finished(
                &pool,
                tool(app, &codex, &session, name, arguments).await,
            )
            .await
        }
    };
    let milestone = call(
        "milestone_create",
        json!({"projectId": "PLN", "name": "Beta"}),
    )
    .await;
    ok(&milestone);

    // Thirty tasks in one call, each with its milestone, estimate, dates and
    // the task before it as a blocker; the first five labelled.
    let tasks = (0..30)
        .map(|index| {
            let mut task = json!({
                "ref": format!("t{index}"),
                "title": format!("Step {index}"),
                "milestone": "Beta",
                "estimate": 2,
                "startDate": "2026-12-01",
                "dueDate": "2026-12-20",
            });
            if index > 0 {
                task["blockedByRefs"] = json!([format!("t{}", index - 1)]);
            }
            if index < 5 {
                task["labels"] = json!(["Graph"]);
            }
            task
        })
        .collect::<Vec<_>>();
    let created = call(
        "task_create_many",
        json!({"projectId": "PLN", "tasks": tasks, "createMissingLabels": true,
               "clientRequestId": "k2-14-plan"}),
    )
    .await;
    // Thirty is a long batch: a job makes them (K2.21).
    let created = super::support::long::finished(&setup.pool, created).await;
    let created = ok(&created);
    assert_eq!(created["tasks"].as_array().expect("tasks").len(), 30);
    let (planned, chained) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM task_rows item
                JOIN project_milestones milestone ON milestone.id = item.milestone_id
                WHERE item.project_id = $1 AND milestone.name = 'Beta'
                  AND item.estimate = 2 AND item.start_date = '2026-12-01'
                  AND item.due_date = '2026-12-20'),
               (SELECT count(*) FROM task_dependencies WHERE project_id = $1)
        "#,
    )
    .bind(space)
    .fetch_one(&setup.pool)
    .await
    .expect("planned");
    assert_eq!((planned, chained), (30, 29));

    // Thirty more dependencies in one call, all or nothing.
    let more = call(
        "task_create_many",
        json!({
            "projectId": "PLN",
            "tasks": (0..31).map(|index| json!({"title": format!("Loose {index}")})).collect::<Vec<_>>(),
            "clientRequestId": "k2-14-loose",
        }),
    )
    .await;
    let loose = ok(&more)["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["key"].as_str().expect("key").to_owned())
        .collect::<Vec<_>>();
    let links = (1..31)
        .map(|index| json!({"taskId": loose[index], "blockedBy": [loose[index - 1]]}))
        .collect::<Vec<_>>();
    let linked = call("task_link_many", json!({"links": links})).await;
    assert_eq!(
        ok(&linked)["tasks"].as_array().expect("tasks").len(),
        30,
        "{linked}"
    );
    let looped = call(
        "task_link_many",
        json!({"links": [
            {"taskId": loose[0], "blockedBy": [loose[30]]},
            {"taskId": loose[2], "blockedBy": [loose[0]]},
        ]}),
    )
    .await;
    assert_ne!(looped["outcome"], "ok", "{looped}");
    let dependencies = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_dependencies WHERE project_id = $1",
    )
    .bind(space)
    .fetch_one(&setup.pool)
    .await
    .expect("dependencies");
    assert_eq!(dependencies, 59, "a refused batch changes nothing");

    // A criterion added already checked, and the list's progress counts.
    let first = &loose[0];
    let opened = call("task_open", json!({"taskId": first})).await;
    let version = opened["value"]["version"]
        .as_i64()
        .or_else(|| opened["value"]["task"]["version"].as_i64())
        .unwrap_or_else(|| panic!("version: {opened}"));
    let criteria = call(
        "task_criteria_update",
        json!({
            "taskId": first,
            "expectedVersion": version,
            "operations": [
                {"op": "add", "text": "Already done", "completed": true},
                {"op": "add", "text": "Still to do"},
            ],
        }),
    )
    .await;
    assert!(
        criteria["outcome"] == "ok" || criteria["outcome"] == "applied",
        "{criteria}"
    );
    let listed = call(
        "task_list",
        json!({"projectId": "PLN", "query": first, "limit": 5}),
    )
    .await;
    let row = &ok(&listed)["tasks"][0];
    assert_eq!(row["key"], first.as_str(), "{listed}");
    assert_eq!(
        (row["criteriaDone"].as_i64(), row["criteriaTotal"].as_i64()),
        (Some(1), Some(2))
    );
    let other = &ok(&listed)["tasks"];
    assert!(
        other
            .as_array()
            .expect("rows")
            .iter()
            .skip(1)
            .all(|row| row.get("criteriaTotal").is_none()),
        "{listed}"
    );

    // The graph narrows to a label, keeps what links to it, and names keys.
    let graph = call(
        "dependency_graph",
        json!({"projectId": "PLN", "label": "graph"}),
    )
    .await;
    let graph = ok(&graph);
    let mut keys = graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| node["key"].as_str().expect("key").to_owned())
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| key.trim_start_matches("PLN-").parse::<u32>().unwrap_or(0));
    assert_eq!(keys, ["PLN-1", "PLN-2", "PLN-3", "PLN-4", "PLN-5", "PLN-6"]);
    assert!(
        graph["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .any(|edge| edge["fromKey"] == "PLN-1" && edge["toKey"] == "PLN-2"),
        "{graph}"
    );
    assert_eq!(strings(&graph["frontierKeys"]), ["PLN-1"]);
    assert_eq!(
        strings(&graph["criticalPathKeys"])
            .first()
            .map(String::as_str),
        Some("PLN-1")
    );

    // And to a parent's subtasks.
    let family = call(
        "task_create_many",
        json!({"projectId": "PLN", "tasks": [
            {"ref": "parent", "title": "Parent"},
            {"ref": "a", "parentRef": "parent", "title": "Child a"},
            {"ref": "b", "parentRef": "parent", "title": "Child b", "blockedByRefs": ["a"]},
        ], "clientRequestId": "k2-14-family"}),
    )
    .await;
    let parent = ok(&family)["tasks"][0]["key"]
        .as_str()
        .expect("parent")
        .to_owned();
    let graph = call(
        "dependency_graph",
        json!({"projectId": "PLN", "parentId": parent}),
    )
    .await;
    let titles = ok(&graph)["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| node["title"].as_str().expect("title").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Child a", "Child b"]);
    let unknown = call(
        "dependency_graph",
        json!({"projectId": "PLN", "label": "No such label"}),
    )
    .await;
    assert_eq!(unknown["outcome"], "not_found", "{unknown}");
}

#[tokio::test]
async fn a_plan_import_updates_the_tasks_a_space_already_has() {
    let Some(setup) = setup("k214_plan_match").await else {
        return;
    };
    let app = &setup.app;
    let space = create_project(&setup, "IMP", "Imports").await;
    let space: Uuid = space.parse().expect("space");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        let pool = setup.pool.clone();
        // A long batch is finished by its job (K2.21).
        async move {
            super::support::long::finished(
                &pool,
                tool(app, &codex, &session, name, arguments).await,
            )
            .await
        }
    };
    for title in ["Write the parser", "Old name"] {
        let created = call(
            "task_create",
            json!({"projectId": "IMP", "title": title, "acceptanceCriteria": ["Parses headings"]}),
        )
        .await;
        ok(&created);
    }
    let plan = json!({
        "milestones": [{"ref": "m", "name": "Launch"}],
        "tasks": [
            {"ref": "parser", "title": "write the parser", "description": "Now with tables.",
             "criteria": ["Parses headings", "Parses tables"], "labels": ["Import"], "milestone": "m"},
            {"ref": "renamed", "key": "IMP-2", "title": "New name", "blockedBy": ["parser"]},
            {"ref": "fresh", "title": "Something new", "milestone": "m"},
        ],
    });
    let started = call(
        "plan_import",
        json!({"projectId": "IMP", "title": "Launch plan", "plan": plan.to_string()}),
    )
    .await;
    let started = ok(&started);
    assert_eq!(
        started["updates"],
        json!([{"ref": "parser", "key": "IMP-1"}, {"ref": "renamed", "key": "IMP-2"}]),
        "{started}"
    );
    let import: Uuid = started["plan"]["id"]
        .as_str()
        .expect("plan")
        .parse()
        .expect("uuid");
    let committed = call(
        "plan_import",
        json!({
            "projectId": "IMP",
            "importId": import,
            "commit": true,
            "expectedVersion": started["plan"]["version"],
        }),
    )
    .await;
    ok(&committed);
    for _ in 0..10 {
        if plans::commit_next_step(&setup.pool, import)
            .await
            .expect("step")
            == Step::Done
        {
            break;
        }
    }

    // Three draft tasks, one new task: the other two were updated.
    let rows = sqlx::query_as::<_, (i64, String, String, Option<String>)>(
        r#"
        SELECT item.number, item.title, item.description, milestone.name
        FROM task_rows item
        LEFT JOIN project_milestones milestone ON milestone.id = item.milestone_id
        WHERE item.project_id = $1 ORDER BY item.number
        "#,
    )
    .bind(space)
    .fetch_all(&setup.pool)
    .await
    .expect("tasks");
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0].1, "Write the parser");
    assert_eq!(rows[0].2, "Now with tables.");
    assert_eq!(rows[0].3.as_deref(), Some("Launch"));
    assert_eq!(rows[1].1, "New name");
    assert_eq!(
        (rows[2].1.as_str(), rows[2].3.as_deref()),
        ("Something new", Some("Launch"))
    );
    let (criteria, labels, dependencies) = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM work_item_acceptance_criteria criterion
                JOIN task_rows item ON item.id = criterion.work_item_id
                WHERE item.project_id = $1 AND item.number = 1),
               (SELECT count(*) FROM work_item_active_labels active
                JOIN task_rows item ON item.id = active.work_item_id
                WHERE item.project_id = $1 AND item.number = 1 AND active.name = 'Import'),
               (SELECT count(*) FROM task_dependencies dependency
                JOIN task_rows blocked ON blocked.id = dependency.blocked_work_item_id
                JOIN task_rows blocking ON blocking.id = dependency.blocking_work_item_id
                WHERE dependency.project_id = $1 AND blocked.number = 2 AND blocking.number = 1)
        "#,
    )
    .bind(space)
    .fetch_one(&setup.pool)
    .await
    .expect("details");
    assert_eq!(
        (criteria, labels, dependencies),
        (2, 1, 1),
        "the missing criterion and label were added, the dependency joined the existing tasks"
    );
}
