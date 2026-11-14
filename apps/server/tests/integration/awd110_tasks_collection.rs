//! AWD-110: tasks as the built-in collection. The silent, costly failures:
//! the Tasks collection showing another project's tasks, a filter or group
//! on a built-in field (status, priority) reading wrong, and a record or
//! field write reaching tasks through the collection.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn a_projects_tasks_read_as_its_built_in_collection() {
    let Some(setup) = setup("awd110_tasks_collection").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    for (project, title) in [
        ("WORK", "Fix the bridge"),
        ("WORK", "Paint the lighthouse"),
        ("WORK", "Tune the engine"),
        ("OTHER", "Someone else's task"),
    ] {
        let made = call(
            "task_create",
            json!({"projectId": project, "title": title, "clientRequestId": title.replace([' ', '\''], "-")}),
        )
        .await;
        assert_eq!(made["outcome"], "applied", "{made}");
    }
    sqlx::query("UPDATE work_items SET priority = 'high' WHERE title = 'Paint the lighthouse'")
        .execute(&setup.pool)
        .await
        .expect("priority");

    // Opening the list makes the Tasks collection, first, counting only
    // this project's tasks.
    let (status, listed) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/collections"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let tasks = &listed["collections"][0];
    assert_eq!(tasks["builtin"], "tasks", "{listed}");
    assert_eq!(tasks["name"], "Tasks");
    assert_eq!(tasks["records"], 3, "{listed}");
    let tasks_id = tasks["id"].as_str().expect("id").to_owned();
    let (_, again) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/collections"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(
        again["collections"]
            .as_array()
            .expect("collections")
            .iter()
            .filter(|collection| collection["builtin"] == "tasks")
            .count(),
        1,
        "{again}"
    );

    // Its fields are the tasks' own; a filter on Priority and a grouping
    // by Status read the tasks.
    let (status, body) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["canEditRecords"], true, "{body}");
    let names = body["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        names.starts_with(&[
            "Status".to_owned(),
            "Assignee".to_owned(),
            "Priority".to_owned()
        ]),
        "{names:?}"
    );
    let high = call(
        "record_query",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "filters": [{"field": "Priority", "op": "is", "values": ["High"]}],
        }),
    )
    .await;
    assert_eq!(high["outcome"], "ok", "{high}");
    assert_eq!(high["value"]["total"], 1, "{high}");
    assert_eq!(high["value"]["records"][0]["title"], "Paint the lighthouse");
    let grouped = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Tasks", "groupBy": "Status"}),
    )
    .await;
    assert_eq!(grouped["outcome"], "ok", "{grouped}");
    let groups = grouped["value"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "{grouped}");
    assert_eq!(groups[0]["count"], 3, "{grouped}");
    assert_ne!(groups[0]["name"], "No value", "{grouped}");
    let (status, by_priority) = send(
        app,
        setup.browser.request(
            "GET",
            &format!(
                "/api/v1/projects/{work}/collections/{tasks_id}/records?groupBy=0192f5c0-0000-7000-8000-000000000003"
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_priority}");
    assert_eq!(
        by_priority["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .map(|group| (group["label"].clone(), group["count"].clone()))
            .collect::<Vec<_>>(),
        vec![(json!("High"), json!(1)), (json!("No value"), json!(2))],
        "{by_priority}"
    );

    // A view of it is saved like any other; its name stays, and agents
    // change its records with the task tools.
    let viewed = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "views": [{"name": "By status", "type": "board", "groupBy": "Status"}],
        }),
    )
    .await;
    assert_eq!(viewed["outcome"], "ok", "{viewed}");
    let renamed = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Tasks", "name": "Chores"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "conflict", "{renamed}");
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "records": [{"title": "Sneaky"}],
            "clientRequestId": "sneaky",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "conflict", "{written}");
    // A field added to Tasks is a task field; a built-in name is refused.
    let (status, mood) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}/fields"),
            Some("field"),
            json!({"name": "Mood", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mood}");
    let kind = sqlx::query_scalar::<_, String>(
        "SELECT record_kind FROM field_definitions WHERE project_id = $1::uuid AND name = 'Mood'",
    )
    .bind(&work)
    .fetch_one(&setup.pool)
    .await
    .expect("Mood");
    assert_eq!(kind, "task");
    let (status, _) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}/fields"),
            Some("field-status"),
            json!({"name": "status", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let agent_field = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Tasks", "fields": [{"name": "Effort", "type": "number"}]}),
    )
    .await;
    assert_eq!(agent_field["outcome"], "ok", "{agent_field}");
    let agent_status = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Tasks", "fields": [{"field": "Status", "name": "State"}]}),
    )
    .await;
    assert_eq!(agent_status["outcome"], "invalid_request", "{agent_status}");
    // Nor does a form add to it.
    let form = call(
        "bug_report_link_write",
        json!({"projectId": "WORK", "name": "Task form", "collection": "Tasks"}),
    )
    .await;
    assert_eq!(form["outcome"], "invalid_request", "{form}");
    let tasks_now =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_items WHERE project_id = $1::uuid")
            .bind(&work)
            .fetch_one(&setup.pool)
            .await
            .expect("count");
    assert_eq!(tasks_now, 3);

    // Another project's Tasks collection is its own, and this one's ID
    // finds nothing there.
    let (status, _) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{other}/collections/{tasks_id}/records"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_tasks_record_changes_as_its_task() {
    let Some(setup) = setup("awd110_tasks_editable").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let made = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the bridge", "clientRequestId": "bridge"}),
    )
    .await;
    assert_eq!(made["outcome"], "applied", "{made}");
    let (_, listed) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/collections"),
            None,
            Value::Null,
        ),
    )
    .await;
    let tasks_id = listed["collections"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let (task, version) = sqlx::query_as::<_, (uuid::Uuid, i64)>(
        "SELECT id, version FROM work_items WHERE project_id = $1::uuid",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("task");
    let next_status = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT status.id FROM work_item_statuses status \
         WHERE status.project_id = $1::uuid AND status.enabled \
           AND status.id <> (SELECT status_id FROM work_items WHERE id = $2) \
         ORDER BY status.position LIMIT 1",
    )
    .bind(&work)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("status");
    let record = format!("/api/v1/projects/{work}/collections/{tasks_id}/records/{task}");

    // Title, status, priority and due date in one edit, as one task change.
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("edit-1"),
            json!({
                "expectedVersion": version,
                "title": "Fix the old bridge",
                "values": {
                    "0192f5c0-0000-7000-8000-000000000001": next_status.to_string(),
                    "0192f5c0-0000-7000-8000-000000000003": "0192f5c0-0000-7000-8000-0000000000a2",
                    "0192f5c0-0000-7000-8000-000000000005": "2026-12-01",
                },
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["record"]["title"], "Fix the old bridge", "{body}");
    let (title, status_id, priority, due, now) =
        sqlx::query_as::<_, (String, uuid::Uuid, String, Option<chrono::NaiveDate>, i64)>(
            "SELECT title, status_id, priority, due_date, version FROM work_items WHERE id = $1",
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("task now");
    assert_eq!(title, "Fix the old bridge");
    assert_eq!(status_id, next_status);
    assert_eq!(priority, "high");
    assert_eq!(due, chrono::NaiveDate::from_ymd_opt(2026, 12, 1));
    assert_eq!(now, version + 1, "one task change");
    let activity = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_activity WHERE work_item_id = $1",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("activity");
    assert!(activity >= 2, "the change is in the task's activity");

    // A label the task holds stays when it's archived since; a newly
    // added one must be active.
    let label = |name: &'static str| {
        let codex = codex.clone();
        let session = session.clone();
        async move {
            let made = tool(
                app,
                &codex,
                &session,
                "label_write",
                json!({"projectId": "WORK", "name": name}),
            )
            .await;
            assert_eq!(made["outcome"], "ok", "{made}");
            made["value"]["label"]["id"]
                .as_str()
                .expect("label id")
                .to_owned()
        }
    };
    let (old, fresh, gone) = (
        label("Old").await,
        label("Fresh").await,
        label("Gone").await,
    );
    let version_now = || async {
        sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
            .bind(task)
            .fetch_one(pool)
            .await
            .expect("version")
    };
    let labels = "0192f5c0-0000-7000-8000-000000000006";
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("labels-1"),
            json!({"expectedVersion": version_now().await, "values": {labels: [old]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query(
        "UPDATE project_labels SET archived_at = now(), archived_by_actor_id = created_by_actor_id \
         WHERE id = ANY($1::uuid[])",
    )
    .bind(vec![old.clone(), gone.clone()])
    .execute(pool)
    .await
    .expect("archive labels");
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("labels-2"),
            json!({"expectedVersion": version_now().await, "values": {labels: [old, fresh]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("labels-3"),
            json!({"expectedVersion": version_now().await, "values": {labels: [old, fresh, gone]}}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an archived label isn't added"
    );
    let now = version_now().await;

    // A stale version is refused; archiving archives the task.
    let (status, _) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("edit-stale"),
            json!({"expectedVersion": version, "title": "Stale"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("edit-archive"),
            json!({"expectedVersion": now, "archived": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["record"]["archived"], true, "{body}");

    // A new record is a new task, with the next key.
    let (status, body) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}/records"),
            Some("new-task"),
            json!({"title": "Paint the lighthouse"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["record"]["number"], 2, "{body}");
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items WHERE project_id = $1::uuid AND title = 'Paint the lighthouse'",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("made");
    assert_eq!(made, 1);
    let (status, _) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}/records"),
            Some("untitled-task"),
            json!({"title": ""}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        app,
        setup.browser.request(
            "PATCH",
            &record,
            Some("blank-title"),
            json!({"expectedVersion": now + 1, "archived": false, "title": " "}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a task keeps a title"
    );
}
