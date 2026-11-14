//! AWD-110, step 2 of moving tasks into collection records: every task has
//! a record in its project's Tasks collection, with the same ID, kept in step
//! by triggers (since T1.3 the record owns the shared columns and the task
//! keeps a copy). The silent, costly failures: a task without its record (or
//! with a stale one), a record deleted behind its task's back, a project's
//! own "Tasks" collection taken over, and tasks found twice by search.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
};

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Mirror {
    collection_name: String,
    builtin: Option<String>,
    number: i64,
    title: String,
    field_values: Value,
    archived: bool,
}

async fn mirror(pool: &sqlx::PgPool, task: Uuid) -> Option<Mirror> {
    sqlx::query_as::<_, Mirror>(
        r#"
        SELECT collection.name AS collection_name, collection.builtin, record.number,
               record.title, record.field_values, record.archived_at IS NOT NULL AS archived
        FROM collection_records record
        JOIN collections collection ON collection.id = record.collection_id
        JOIN work_items task ON task.id = record.id AND task.project_id = record.project_id
        WHERE record.id = $1
        "#,
    )
    .bind(task)
    .fetch_optional(pool)
    .await
    .expect("mirror")
}

#[tokio::test]
async fn every_task_has_its_record_in_the_tasks_collection() {
    let Some(setup) = setup("awd110_task_records").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    // The project already has its own collection called Tasks: it keeps
    // its name, and the first task makes the built-in one beside it.
    let own = call(
        "collection_write",
        json!({"projectId": "WORK", "name": "Tasks", "clientRequestId": "own-tasks"}),
    )
    .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    let made = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the bridge", "clientRequestId": "bridge"}),
    )
    .await;
    assert_eq!(made["outcome"], "applied", "{made}");
    let task = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_items WHERE project_id = $1::uuid AND title = 'Fix the bridge'",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("task");
    let record = mirror(pool, task).await.expect("the task has its record");
    assert_eq!(
        record,
        Mirror {
            collection_name: "Tasks (built in)".to_owned(),
            builtin: Some("tasks".to_owned()),
            number: 1,
            title: "Fix the bridge".to_owned(),
            field_values: json!({}),
            archived: false,
        }
    );
    let views = sqlx::query_scalar::<_, String>(
        "SELECT view.name FROM collection_views view \
         JOIN collections collection ON collection.id = view.collection_id \
         WHERE collection.project_id = $1::uuid AND collection.builtin = 'tasks'",
    )
    .bind(&work)
    .fetch_all(pool)
    .await
    .expect("views");
    assert_eq!(views, vec!["All tasks".to_owned()]);

    // A task's changes reach its record: the title through the task tool.
    // Since T1.3 the record owns the title, field values and archive state,
    // and the task keeps a copy, which can't be written directly.
    let opened = call("task_open", json!({"taskId": task.to_string()})).await;
    let version = opened["value"]["task"]["version"]
        .as_i64()
        .expect("version");
    let renamed = call(
        "task_update",
        json!({"taskId": task.to_string(), "title": "Fix the old bridge", "expectedVersion": version}),
    )
    .await;
    assert_ne!(renamed["outcome"], "conflict", "{renamed}");
    assert_eq!(
        mirror(pool, task).await.expect("record").title,
        "Fix the old bridge"
    );
    let field = Uuid::now_v7().to_string();
    sqlx::query(
        "UPDATE collection_records SET title = 'Fix the bridge', field_values = jsonb_build_object($2::text, 'steel'), \
           archived_at = now(), archived_by_actor_id = created_by_actor_id \
         WHERE id = $1",
    )
    .bind(task)
    .bind(&field)
    .execute(pool)
    .await
    .expect("record write");
    let record = mirror(pool, task).await.expect("record");
    assert_eq!(record.title, "Fix the bridge");
    assert_eq!(record.field_values, json!({ field.as_str(): "steel" }));
    assert!(record.archived);
    let cached = sqlx::query_as::<_, (String, Value, bool)>(
        "SELECT title, field_values, archived_at IS NOT NULL FROM work_items WHERE id = $1",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("cache");
    assert_eq!(
        cached,
        ("Fix the bridge".to_owned(), json!({ field.as_str(): "steel" }), true)
    );
    let direct = sqlx::query("UPDATE work_items SET title = 'Sneaky' WHERE id = $1")
        .bind(task)
        .execute(pool)
        .await;
    assert!(direct.is_err(), "a direct write to a task's cached title");

    // A Tasks record is deleted, moved or added only through its task.
    let deleted = sqlx::query("DELETE FROM collection_records WHERE id = $1")
        .bind(task)
        .execute(pool)
        .await;
    assert!(deleted.is_err(), "a Tasks record deleted directly");
    let built_in = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections WHERE project_id = $1::uuid AND builtin = 'tasks'",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("built in");
    let inserted = sqlx::query(
        "INSERT INTO collection_records \
           (id, project_id, collection_id, number, title, created_by_actor_id, updated_by_actor_id) \
         SELECT $1, project_id, id, 99, 'Sneaky', created_by_actor_id, created_by_actor_id \
         FROM collections WHERE id = $2",
    )
    .bind(Uuid::now_v7())
    .bind(built_in)
    .execute(pool)
    .await;
    assert!(inserted.is_err(), "a record added to Tasks directly");
    // Records of the project's own collection are untouched by the guard.
    let notes = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Tasks",
            "records": [{"title": "My own record"}],
            "clientRequestId": "own-record",
        }),
    )
    .await;
    assert_eq!(notes["outcome"], "ok", "{notes}");

    // Search finds a task once, as the task.
    sqlx::query(
        "UPDATE collection_records SET title = 'Paint the zeppelin', archived_at = NULL, \
           archived_by_actor_id = NULL WHERE id = $1",
    )
    .bind(task)
    .execute(pool)
    .await
    .expect("unarchive");
    let found = call("search", json!({"query": "zeppelin"})).await;
    assert_eq!(found["outcome"], "ok", "{found}");
    let kinds = found["value"]["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|result| result["kind"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["task".to_owned()], "{found}");

    // Every task in the database has exactly one matching record.
    let unmatched = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM work_items task
        LEFT JOIN collection_records record ON record.id = task.id
        LEFT JOIN collections collection
          ON collection.id = record.collection_id AND collection.builtin = 'tasks'
        WHERE collection.id IS NULL OR record.number <> task.number
           OR record.field_values <> task.field_values
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("unmatched");
    assert_eq!(unmatched, 0);
}
