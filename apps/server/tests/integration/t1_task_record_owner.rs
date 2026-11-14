//! T1.3: a task's record owns its title, field values and archive state;
//! the task keeps a copy for readers that haven't moved yet. The silent,
//! costly failures: a task write that changes the copy but not the record
//! (or the other way round), a direct write that makes the two disagree, an
//! archive (by the task tool or a project move) that never reaches the
//! record, and versions bumped twice, which would turn an agent's next
//! `expectedVersion` into a conflict.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
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
        FROM collection_records record JOIN work_items task ON task.id = record.id
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

    // The cached columns can't be written directly.
    for write in [
        "UPDATE work_items SET title = 'Sneaky' WHERE id = $1",
        "UPDATE work_items SET field_values = '{}'::jsonb WHERE id = $1",
        "UPDATE work_items SET archived_at = now(), archived_by_actor_id = created_by_actor_id WHERE id = $1",
    ] {
        let refused = sqlx::query(write).bind(task).execute(pool).await;
        assert!(refused.is_err(), "{write}");
    }
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

    // Every task agrees with its record.
    let unmatched = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_items task JOIN collection_records record ON record.id = task.id \
         WHERE record.title <> task.title OR record.field_values <> task.field_values \
            OR record.archived_at IS DISTINCT FROM task.archived_at \
            OR record.archived_by_actor_id IS DISTINCT FROM task.archived_by_actor_id",
    )
    .fetch_one(pool)
    .await
    .expect("unmatched");
    assert_eq!(unmatched, 0);
}
