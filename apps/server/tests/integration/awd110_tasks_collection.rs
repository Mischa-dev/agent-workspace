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
    assert_eq!(body["canEditRecords"], false, "{body}");
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

    // A view of it is saved like any other; its records, fields and name
    // change only as tasks.
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
    let (status, _) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{tasks_id}/fields"),
            Some("field"),
            json!({"name": "Mood", "type": "text"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
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
