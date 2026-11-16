//! H1.4 to H1.7: every workspace-owned table carries `workspace_id`. The
//! silent failures: a row written by a writer that doesn't know the column
//! landing with no workspace (so row rules, export and deletion skip it), a
//! backfill that loses its place and stops short, and a switch migration
//! that waits on a job that never ran.

use sqlx::PgPool;
use uuid::Uuid;

use super::tenancy_tables::migrated_schema;

async fn workspace_and_project(pool: &PgPool) -> (Uuid, Uuid) {
    let workspace = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Fill', $2)")
        .bind(workspace)
        .bind(format!("fill-{}", workspace.simple()))
        .execute(pool)
        .await
        .expect("workspace");
    let project = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, $3, 'Fill')",
    )
    .bind(project)
    .bind(workspace)
    .bind(format!("F{}", &project.simple().to_string()[24..]).to_uppercase())
    .execute(pool)
    .await
    .expect("project");
    (workspace, project)
}

async fn nulls(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {table} WHERE workspace_id IS NULL"
    ))
    .fetch_one(pool)
    .await
    .expect("count rows without a workspace")
}

/// A probe table shaped like the ones H1.4 to H1.7 expand: a project, no
/// workspace column yet, and enough rows over enough pages to batch.
async fn probe(pool: &PgPool, name: &str, project: Option<Uuid>, rows: i32) {
    sqlx::query(&format!(
        "CREATE TABLE {name} (id BIGSERIAL PRIMARY KEY, project_id UUID REFERENCES projects(id), padding TEXT) WITH (fillfactor = 10)"
    ))
    .execute(pool)
    .await
    .expect("probe table");
    sqlx::query(&format!(
        "INSERT INTO {name} (project_id, padding) SELECT $1, repeat('x', 400) FROM generate_series(1, $2)"
    ))
    .bind(project)
    .bind(rows)
    .execute(pool)
    .await
    .expect("probe rows");
}

#[tokio::test]
async fn rows_get_their_workspace_on_insert_by_backfill_and_by_the_switch() {
    let Some((pool, schema)) = migrated_schema("tenancy_backfill").await else {
        return;
    };
    let (workspace, project) = workspace_and_project(&pool).await;

    // Every table H1.4 to H1.7 expanded requires the column now, and a
    // writer that doesn't name it still gets it: from the project...
    let missing = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM information_schema.columns c
        JOIN pg_trigger t ON t.tgrelid = format('%I.%I', c.table_schema, c.table_name)::regclass
         AND t.tgname = c.table_name || '_fill_workspace'
        WHERE c.table_schema = $1 AND c.column_name = 'workspace_id' AND c.is_nullable = 'YES'
        "#,
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("nullable workspace columns");
    assert_eq!(missing, 0, "a workspace column left nullable");
    let filled = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_trigger WHERE tgname LIKE '%\\_fill\\_workspace'",
    )
    .fetch_one(&pool)
    .await
    .expect("fill triggers");
    assert!(filled >= 115, "{filled}");
    let actor = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor)
        .execute(&pool)
        .await
        .expect("actor");
    let counter = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO project_counters (project_id) VALUES ($1)
        ON CONFLICT (project_id) DO UPDATE SET updated_at = now()
        RETURNING workspace_id
        "#,
    )
    .bind(project)
    .fetch_one(&pool)
    .await
    .expect("a counter written without its workspace");
    assert_eq!(counter, workspace);
    // ...and from the parent row, for a table without a project.
    let whiteboard = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO whiteboards (id, organization_id, project_id, title, created_by_actor_id, updated_by_actor_id) VALUES ($1, $2, $3, 'Fill', $4, $4)",
    )
    .bind(whiteboard)
    .bind(workspace)
    .bind(project)
    .bind(actor)
    .execute(&pool)
    .await
    .expect("whiteboard");
    let element = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO whiteboard_elements
            (whiteboard_id, element_id, element_type, version, version_nonce, data, seq)
        VALUES ($1, 'fill', 'rectangle', 1, 1, '{}'::jsonb, 1)
        RETURNING workspace_id
        "#,
    )
    .bind(whiteboard)
    .fetch_one(&pool)
    .await
    .expect("an element written without its workspace");
    assert_eq!(element, workspace);

    // The backfill job resumes from its cursor: expanding a table that has
    // rows queues the first batch; one batch fills some pages and names the
    // next; picking up from there (after a stop) finishes it.
    probe(&pool, "backfill_probe", Some(project), 600).await;
    sqlx::query("SELECT tenancy_expand('backfill_probe')")
        .execute(&pool)
        .await
        .expect("expand");
    let queued = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload FROM background_jobs WHERE kind = 'tenancy.backfill' AND state = 'queued'",
    )
    .fetch_all(&pool)
    .await
    .expect("queued backfill");
    assert_eq!(
        queued,
        vec![serde_json::json!({"table": "backfill_probe", "afterPage": 0})]
    );
    let before = nulls(&pool, "backfill_probe").await;
    assert_eq!(before, 600);
    let next = sqlx::query_scalar::<_, Option<i64>>("SELECT tenancy_backfill('backfill_probe', 0, 2)")
        .fetch_one(&pool)
        .await
        .expect("first batch");
    assert_eq!(next, Some(2));
    let after_one = nulls(&pool, "backfill_probe").await;
    assert!(after_one > 0 && after_one < before, "{after_one}");
    // The job stops here; a later run starts from its cursor.
    let mut cursor = next;
    let mut batches = 0;
    while let Some(page) = cursor {
        cursor = sqlx::query_scalar::<_, Option<i64>>("SELECT tenancy_backfill('backfill_probe', $1, 2)")
            .bind(page)
            .fetch_one(&pool)
            .await
            .expect("next batch");
        batches += 1;
        assert!(batches < 1000, "the backfill never finished");
    }
    assert_eq!(nulls(&pool, "backfill_probe").await, 0);
    let filled = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM backfill_probe WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(&pool)
    .await
    .expect("filled rows");
    assert_eq!(filled, 600);

    // The switch completes when the job never ran: it backfills inline,
    // makes the column required and indexes it.
    probe(&pool, "switch_probe", Some(project), 300).await;
    sqlx::query("SELECT tenancy_expand('switch_probe')")
        .execute(&pool)
        .await
        .expect("expand");
    sqlx::query("SELECT tenancy_switch('switch_probe')")
        .execute(&pool)
        .await
        .expect("switch without the job");
    assert_eq!(nulls(&pool, "switch_probe").await, 0);
    let (required, indexed, validated) = sqlx::query_as::<_, (bool, bool, bool)>(
        r#"
        SELECT
            (SELECT attnotnull FROM pg_attribute
             WHERE attrelid = 'switch_probe'::regclass AND attname = 'workspace_id'),
            EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = $1 AND indexname = 'switch_probe_workspace'),
            (SELECT convalidated FROM pg_constraint
             WHERE conrelid = 'switch_probe'::regclass AND conname = 'switch_probe_workspace_id_fkey')
        "#,
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("switched table");
    assert!(required && indexed && validated);
    // A second switch is a no-op.
    sqlx::query("SELECT tenancy_switch('switch_probe')")
        .execute(&pool)
        .await
        .expect("switch again");

    // A row with nothing to take a workspace from stops the switch rather
    // than leaving it unowned.
    probe(&pool, "orphan_probe", None, 3).await;
    sqlx::query("SELECT tenancy_expand('orphan_probe')")
        .execute(&pool)
        .await
        .expect("expand");
    let refused = sqlx::query("SELECT tenancy_switch('orphan_probe')")
        .execute(&pool)
        .await
        .expect_err("rows with no workspace");
    assert!(refused.to_string().contains("orphan_probe"), "{refused}");

    agent_workspace_server::test_support::drop_schema(&pool, &schema)
        .await
        .expect("drop the test schema");
    pool.close().await;
}
