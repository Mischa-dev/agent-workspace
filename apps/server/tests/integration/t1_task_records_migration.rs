//! T1.3 and T1.4d on data: the database as it was before the record took
//! over a task's title, values and archive state (tasks, archived ones,
//! custom values, a title from before titles were cleaned, a task-like
//! collection, a task whose record went missing), then the rest of the
//! migrations. The silent, costly failure: a task that comes out of the
//! move with another title, lost values, a changed archive state or no
//! record at all.

use std::borrow::Cow;

use serde_json::{Value, json};
use sqlx::{Executor, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

use super::support::db::schema_pool;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The first migration of T1.3: everything before it is the old shape.
const FIRST_NEW: i64 = 20261114162000;

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Task {
    number: i64,
    title: String,
    field_values: Value,
    archived: bool,
    collection: String,
}

const TASKS: &str = "SELECT task.number, task.title, task.field_values, \
       task.archived_at IS NOT NULL AS archived, collection.name AS collection \
     FROM {table} task \
     JOIN collection_records record ON record.id = task.id \
     JOIN collections collection ON collection.id = record.collection_id \
     ORDER BY task.number";

#[tokio::test]
async fn tasks_keep_their_titles_values_and_archive_state_through_the_move() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("admin");
    let schema = format!("t1_migration_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("schema");
    let pool = schema_pool(&database_url, &schema).await;

    let before = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version < FIRST_NEW)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.expect("the old shape");

    // A workspace, a person and a project with its workflow.
    let organization = Uuid::now_v7();
    let person = Uuid::now_v7();
    let project = Uuid::now_v7();
    for (sql, binds) in [
        (
            "INSERT INTO organizations (id, name, slug) VALUES ($1, 'Old', 'old')",
            vec![organization],
        ),
        (
            "INSERT INTO actors (id, kind) VALUES ($1, 'human')",
            vec![person],
        ),
        (
            "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) \
             VALUES ($1, $2, 'owner', true)",
            vec![organization, person],
        ),
        (
            "INSERT INTO projects (id, organization_id, project_key, name) \
             VALUES ($1, $2, 'OLD', 'Old')",
            vec![project, organization],
        ),
        (
            "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
             VALUES (gen_random_uuid(), $1, $2, 'owner', 'Owner', 'owner')",
            vec![project, person],
        ),
        (
            "INSERT INTO project_counters (project_id) VALUES ($1)",
            vec![project],
        ),
    ] {
        let mut query = sqlx::query(sql);
        for bind in binds {
            query = query.bind(bind);
        }
        query
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let mut transaction = pool.begin().await.expect("begin");
    agent_workspace_server::work_items::ensure_default_workflow(&mut transaction, project)
        .await
        .expect("workflow");
    transaction.commit().await.expect("commit");

    // Tasks written the old way: straight into work_items, their records
    // made by step 2's triggers.
    let field = Uuid::now_v7().to_string();
    for (number, title, values, archived) in [
        (
            1_i64,
            "Fix the bridge",
            json!({ field.as_str(): "steel" }),
            false,
        ),
        (2_i64, "Old\tand  untidy ", json!({}), false),
        (
            3_i64,
            "Paint the lighthouse",
            json!({ field.as_str(): "white" }),
            true,
        ),
        (
            4_i64,
            "Lost its record",
            json!({ field.as_str(): "gone" }),
            false,
        ),
    ] {
        sqlx::query(
            "INSERT INTO work_items (id, project_id, number, title, type_id, status_id, \
               created_by_actor_id, board_rank, field_values, archived_at, archived_by_actor_id) \
             SELECT gen_random_uuid(), $1, $2, $3, kind.id, status.id, $4, 1024 * $2, $5, \
               CASE WHEN $6 THEN now() END, CASE WHEN $6 THEN $4 END \
             FROM work_item_types kind, work_item_statuses status \
             WHERE kind.project_id = $1 AND kind.is_default \
               AND status.project_id = $1 AND status.is_default",
        )
        .bind(project)
        .bind(number)
        .bind(title)
        .bind(person)
        .bind(&values)
        .bind(archived)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("task {number}: {error}"));
    }
    sqlx::query("UPDATE project_counters SET next_work_item_number = 5 WHERE project_id = $1")
        .bind(project)
        .execute(&pool)
        .await
        .expect("counter");
    // One task lost its record (step 2 ran in another order), and one
    // record went stale; the T1.3 migration repairs both from the task.
    pool.execute("ALTER TABLE collection_records DISABLE TRIGGER USER")
        .await
        .expect("guards off");
    sqlx::query(
        "DELETE FROM collection_records WHERE id = (SELECT id FROM work_items WHERE number = 4)",
    )
    .execute(&pool)
    .await
    .expect("lose a record");
    sqlx::query(
        "UPDATE collection_records SET title = 'Stale' \
         WHERE id = (SELECT id FROM work_items WHERE number = 1)",
    )
    .execute(&pool)
    .await
    .expect("stale record");
    pool.execute("ALTER TABLE collection_records ENABLE TRIGGER USER")
        .await
        .expect("guards on");

    // A collection of records switched to act like a task the old way.
    let bugs = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO collections (id, project_id, name, title_name, created_by_actor_id) \
         VALUES ($1, $2, 'Bugs', 'Title', $3)",
    )
    .bind(bugs)
    .bind(project)
    .bind(person)
    .execute(&pool)
    .await
    .expect("collection");
    sqlx::query(
        "INSERT INTO collection_records (id, project_id, collection_id, number, title, \
           field_values, created_by_actor_id, updated_by_actor_id) \
         VALUES (gen_random_uuid(), $1, $2, 1, 'Crash on save', $4, $3, $3), \
                (gen_random_uuid(), $1, $2, 2, '', '{}'::jsonb, $3, $3)",
    )
    .bind(project)
    .bind(bugs)
    .bind(person)
    .bind(json!({ field.as_str(): "high" }))
    .execute(&pool)
    .await
    .expect("records");
    sqlx::query("SELECT collection_make_task_like($1, $2)")
        .bind(bugs)
        .bind(person)
        .execute(&pool)
        .await
        .expect("acts like a task");

    let expected = vec![
        Task {
            number: 1,
            title: "Fix the bridge".to_owned(),
            field_values: json!({ field.as_str(): "steel" }),
            archived: false,
            collection: "Tasks".to_owned(),
        },
        Task {
            number: 2,
            title: "Old and  untidy".to_owned(),
            field_values: json!({}),
            archived: false,
            collection: "Tasks".to_owned(),
        },
        Task {
            number: 3,
            title: "Paint the lighthouse".to_owned(),
            field_values: json!({ field.as_str(): "white" }),
            archived: true,
            collection: "Tasks".to_owned(),
        },
        Task {
            number: 4,
            title: "Lost its record".to_owned(),
            field_values: json!({ field.as_str(): "gone" }),
            archived: false,
            collection: "Tasks".to_owned(),
        },
        Task {
            number: 5,
            title: "Crash on save".to_owned(),
            field_values: json!({ field.as_str(): "high" }),
            archived: false,
            collection: "Bugs".to_owned(),
        },
        Task {
            number: 6,
            title: "Untitled task".to_owned(),
            field_values: json!({}),
            archived: false,
            collection: "Bugs".to_owned(),
        },
    ];

    // The rest of the migrations: the record takes over, then the copies
    // on work_items go.
    MIGRATOR.run(&pool).await.expect("the new shape");
    let after = sqlx::query_as::<_, Task>(&TASKS.replace("{table}", "task_rows"))
        .fetch_all(&pool)
        .await
        .expect("tasks after");
    assert_eq!(after, expected);
    let copies = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'work_items' \
           AND column_name IN ('title', 'field_values', 'archived_at', 'archived_by_actor_id')",
    )
    .fetch_one(&pool)
    .await
    .expect("columns");
    assert_eq!(copies, 0);
    // A task's words are found through its record and its description.
    let found = sqlx::query_scalar::<_, i64>(
        "SELECT number FROM task_rows WHERE search_vector @@ websearch_to_tsquery('english', 'lighthouse')",
    )
    .fetch_all(&pool)
    .await
    .expect("search");
    assert_eq!(found, vec![3]);

    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");
}
