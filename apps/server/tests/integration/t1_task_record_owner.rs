//! T1.3 and T1.4d: a task's record owns its title, field values and archive
//! state, and since T1.4d it holds the only copy (readers use `task_rows`).
//! The silent, costly failures: a task write that never reaches the record,
//! an archive (by the task tool or a project move) that never reaches it, a
//! copy left behind on `work_items` that could drift, and versions bumped
//! twice, which would turn an agent's next `expectedVersion` into a
//! conflict.

use serde_json::{Value, json};
use uuid::Uuid;

use axum::http::StatusCode;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Both {
    record_title: String,
    task_title: String,
    record_values: Value,
    task_values: Value,
    record_archived: bool,
    task_archived: bool,
    record_version: i64,
    task_version: i64,
    record_writer: Uuid,
}

async fn both(pool: &sqlx::PgPool, task: Uuid) -> Both {
    sqlx::query_as::<_, Both>(
        r#"
        SELECT record.title AS record_title, task.title AS task_title,
               record.field_values AS record_values, task.field_values AS task_values,
               record.archived_at IS NOT NULL AS record_archived,
               task.archived_at IS NOT NULL AS task_archived,
               record.version AS record_version, task.version AS task_version,
               record.updated_by_actor_id AS record_writer
        FROM collection_records record JOIN task_rows task ON task.id = record.id
        WHERE record.id = $1
        "#,
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("record and task")
}

#[tokio::test]
async fn the_record_owns_title_values_and_archive_state() {
    let Some(setup) = setup("t1_task_record_owner").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    create_project(&setup, "WORK", "Work").await;
    create_project(&setup, "DEST", "Dest").await;
    let token = connect_app(app, &setup.browser, "Tool", "full").await;
    let session = initialize(app, &token).await.expect("MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let field = call(
        "field_write",
        json!({"projectId": "WORK", "name": "Notes", "type": "text"}),
    )
    .await;
    assert_eq!(field["outcome"], "ok", "{field}");
    let field = field["value"]["field"]["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| field["value"]["id"].as_str().map(str::to_owned))
        .expect("field id");

    // A new task: its record is written first and the task copies it.
    let made = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the bridge", "clientRequestId": "bridge"}),
    )
    .await;
    assert_eq!(made["outcome"], "applied", "{made}");
    let task = sqlx::query_scalar::<_, Uuid>(
        "SELECT record.id FROM collection_records record \
         JOIN collections collection ON collection.id = record.collection_id \
         WHERE collection.builtin = 'tasks' AND record.title = 'Fix the bridge'",
    )
    .fetch_one(pool)
    .await
    .expect("task record");
    let start = both(pool, task).await;
    assert_eq!(start.task_title, "Fix the bridge");
    assert_eq!((start.record_version, start.task_version), (1, 1));

    // A title change through task_update: written on the record, copied to
    // the task, each version bumped once.
    let renamed = call(
        "task_update",
        json!({"taskId": "WORK-1", "title": "Fix the old bridge", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(renamed["outcome"], "applied", "{renamed}");
    let after = both(pool, task).await;
    assert_eq!(after.record_title, "Fix the old bridge");
    assert_eq!(after.task_title, after.record_title);
    assert_eq!((after.record_version, after.task_version), (2, 2));
    assert_ne!(after.record_writer, Uuid::nil());

    // Title and a field value at once: still one bump each.
    let both_changed = call(
        "task_update",
        json!({
            "taskId": "WORK-1", "title": "Fix the new bridge",
            "fields": {"Notes": "steel"}, "expectedVersion": 2,
        }),
    )
    .await;
    assert_eq!(both_changed["outcome"], "applied", "{both_changed}");
    let after = both(pool, task).await;
    assert_eq!(after.record_title, "Fix the new bridge");
    assert_eq!(after.record_values, json!({ field.as_str(): "steel" }));
    assert_eq!(after.task_values, after.record_values);
    assert_eq!((after.record_version, after.task_version), (3, 3));

    // A change to the task's own columns leaves the record alone.
    let prioritised = call(
        "task_update",
        json!({"taskId": "WORK-1", "priority": "high", "expectedVersion": 3}),
    )
    .await;
    assert_eq!(prioritised["outcome"], "applied", "{prioritised}");
    let after = both(pool, task).await;
    assert_eq!((after.record_version, after.task_version), (3, 4));

    // work_items keeps no copy of them (T1.4d).
    let copies = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'work_items' \
           AND column_name IN ('title', 'field_values', 'archived_at', 'archived_by_actor_id')",
    )
    .fetch_one(pool)
    .await
    .expect("columns");
    assert_eq!(copies, 0);
    assert_eq!(both(pool, task).await, after);

    // Archiving through task_update lands on the record; so does restoring.
    let archived = call(
        "task_update",
        json!({"taskId": "WORK-1", "archived": true, "expectedVersion": 4}),
    )
    .await;
    assert_eq!(archived["outcome"], "applied", "{archived}");
    let after = both(pool, task).await;
    assert!(after.record_archived && after.task_archived, "{after:?}");
    assert_eq!((after.record_version, after.task_version), (4, 5));
    let restored = call(
        "task_update",
        json!({"taskId": "WORK-1", "archived": false, "expectedVersion": 5}),
    )
    .await;
    assert_eq!(restored["outcome"], "applied", "{restored}");
    let after = both(pool, task).await;
    assert!(!after.record_archived && !after.task_archived, "{after:?}");
    assert_eq!((after.record_version, after.task_version), (5, 6));

    // A move to another project archives the original on its record, and
    // the copy's record is made with its title.
    let previewed = call(
        "task_move",
        json!({"taskId": "WORK-1", "toProjectId": "DEST", "preview": true}),
    )
    .await;
    assert_eq!(previewed["outcome"], "ok", "{previewed}");
    let snapshot = previewed["value"]["preview"]["snapshot"].clone();
    let resolutions = previewed["value"]["preview"]["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|issue| json!({"issue": issue["id"], "action": "leave"}))
        .collect::<Vec<_>>();
    let moved = call(
        "task_move",
        json!({
            "taskId": "WORK-1", "toProjectId": "DEST", "snapshot": snapshot,
            "resolutions": resolutions, "clientRequestId": "move-bridge",
        }),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let after = both(pool, task).await;
    assert!(after.record_archived && after.task_archived, "{after:?}");
    assert_eq!((after.record_version, after.task_version), (6, 7));
    let copy = moved["value"]["moved"][0]["id"]
        .as_str()
        .expect("copy")
        .parse::<Uuid>()
        .expect("copy id");
    let copy = both(pool, copy).await;
    assert_eq!(copy.record_title, "Fix the new bridge");
    assert_eq!(copy.task_title, copy.record_title);
    assert!(!copy.record_archived && !copy.task_archived);
}

/// Ruling R53: a task has one public revision, `work_items.version`, that
/// every change advances exactly once, whichever side writes it: the task
/// tools or the task's record. A stale version from either side conflicts,
/// and the task read and its events show the one revision.
#[tokio::test]
async fn a_task_has_one_revision_whichever_side_writes_it() {
    let Some(setup) = setup("t1_task_revision").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let token = connect_app(app, &setup.browser, "Tool", "full").await;
    let session = initialize(app, &token).await.expect("MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let field = call(
        "field_write",
        json!({"projectId": "WORK", "name": "Notes", "type": "text"}),
    )
    .await;
    assert_eq!(field["outcome"], "ok", "{field}");
    let made = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the bridge", "clientRequestId": "bridge"}),
    )
    .await;
    assert_eq!(made["outcome"], "applied", "{made}");
    let (task, tasks) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT task.id, task.record_collection_id FROM task_rows task \
         WHERE task.project_id = $1::uuid",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("task");
    let record = format!("/api/v1/projects/{work}/collections/{tasks}/records/{task}");
    let edit_record = |key: &'static str, body: Value| {
        let request = setup.browser.request("PATCH", &record, Some(key), body);
        async move { send(app, request).await }
    };
    let revision = || async {
        let opened = tool(
            app,
            &token,
            &session,
            "task_open",
            json!({"taskId": "WORK-1"}),
        )
        .await;
        let read = opened["value"]["task"]["version"]
            .as_i64()
            .expect("version");
        let event = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max((payload ->> 'version')::bigint) FROM outbox_events \
             WHERE payload ->> 'workItemId' = $1::text",
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("events");
        (read, event)
    };
    assert_eq!(revision().await, (1, Some(1)));

    // The record side renames it: revision 2.
    let (status, body) = edit_record(
        "rename",
        json!({"expectedVersion": 1, "title": "Fix the old bridge"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["record"]["version"], 2, "{body}");
    assert_eq!(revision().await, (2, Some(2)));

    // The task side, still at 1, conflicts; at 2 it moves the status: 3.
    let stale = call(
        "task_update",
        json!({"taskId": "WORK-1", "status": "Ready", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let moved = call(
        "task_update",
        json!({"taskId": "WORK-1", "status": "Ready", "expectedVersion": 2}),
    )
    .await;
    assert_eq!(moved["outcome"], "applied", "{moved}");
    assert_eq!(revision().await, (3, Some(3)));

    // The record side, still at 2, conflicts; at 3 it sets a value: 4.
    let (status, body) = edit_record(
        "stale-value",
        json!({"expectedVersion": 2, "values": {"Notes": "steel"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = edit_record(
        "value",
        json!({"expectedVersion": 3, "values": {"Notes": "steel"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["record"]["version"], 4, "{body}");
    assert_eq!(revision().await, (4, Some(4)));

    // The task as a record reads the same revision.
    let (status, listed) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/collections/{tasks}/records"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["records"][0]["version"], 4, "{listed}");
}
