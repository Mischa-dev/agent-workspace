use std::{sync::Arc, time::Duration};

use agent_workspace_server::domain::allocate_conversation_sequence;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use uuid::Uuid;

#[tokio::test]
async fn concurrent_m2_timeline_writers_allocate_gap_free_unique_sequences() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_sequence_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated sequence schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_every_migration(&pool).await;

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let channel_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("insert sequence Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'M2 sequence', $2)")
        .bind(organization_id)
        .bind(format!("m2-sequence-{}", organization_id.simple()))
        .execute(&pool)
        .await
        .expect("insert sequence Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'SQ', 'Sequence Project')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert sequence Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Sequence owner', 'sequence-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert sequence membership");
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name) VALUES ($1, $2, 'channel', 'sequence')",
    )
    .bind(channel_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("insert sequence Channel");

    let barrier = Arc::new(Barrier::new(3));
    let mut writers = Vec::new();
    for _ in 0..2 {
        let writer_pool = pool.clone();
        let writer_barrier = Arc::clone(&barrier);
        writers.push(tokio::spawn(async move {
            writer_barrier.wait().await;
            let mut allocated = Vec::new();
            for _ in 0..20 {
                let mut transaction = writer_pool.begin().await.expect("begin timeline append");
                let sequence = allocate_conversation_sequence(&mut transaction, channel_id)
                    .await
                    .expect("allocate timeline sequence");
                sqlx::query(
                    r#"
                    INSERT INTO conversation_entries (
                        id, conversation_id, sequence, kind
                    )
                    VALUES ($1, $2, $3, 'system')
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(channel_id)
                .bind(sequence)
                .execute(&mut *transaction)
                .await
                .expect("insert allocated timeline entry");
                transaction.commit().await.expect("commit timeline append");
                allocated.push(sequence);
            }
            allocated
        }));
    }
    barrier.wait().await;
    let mut allocated = Vec::new();
    for writer in writers {
        allocated.extend(writer.await.expect("join timeline writer"));
    }
    allocated.sort_unstable();
    assert_eq!(allocated, (1_i64..=40).collect::<Vec<_>>());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_entries WHERE conversation_id = $1",
        )
        .bind(channel_id)
        .fetch_one(&pool)
        .await
        .expect("count concurrent timeline entries"),
        40
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated sequence schema");
}

#[tokio::test]
async fn archive_updates_serialize_with_thread_and_navigation_validation() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_archive_race_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated archive-race schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_every_migration(&pool).await;

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let navigation_channel_id = Uuid::now_v7();
    let existing_thread_id = Uuid::now_v7();
    let creation_channel_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("insert archive-race Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Archive race', $2)")
        .bind(organization_id)
        .bind(format!("archive-race-{}", organization_id.simple()))
        .execute(&pool)
        .await
        .expect("insert archive-race Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'RACE', 'Archive Race')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert archive-race Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Archive owner', 'archive-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert archive-race membership");
    for (conversation_id, name) in [
        (navigation_channel_id, "Navigation parent"),
        (creation_channel_id, "Creation parent"),
    ] {
        sqlx::query(
            "INSERT INTO conversations (id, project_id, kind, name) VALUES ($1, $2, 'channel', $3)",
        )
        .bind(conversation_id)
        .bind(project_id)
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert archive-race Channel");
    }
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'Existing child')
        "#,
    )
    .bind(existing_thread_id)
    .bind(project_id)
    .bind(navigation_channel_id)
    .execute(&pool)
    .await
    .expect("insert existing child Thread");
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (project_id, actor_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert overview navigation state");

    let mut archive_navigation_parent = pool.begin().await.expect("begin parent archive");
    sqlx::query(
        "UPDATE conversations SET archived_at = now(), archived_by_actor_id = $1 WHERE id = $2",
    )
    .bind(actor_id)
    .bind(navigation_channel_id)
    .execute(&mut *archive_navigation_parent)
    .await
    .expect("stage navigation parent archive");
    let navigation_pool = pool.clone();
    let navigation_write = tokio::spawn(async move {
        sqlx::query(
            r#"
            UPDATE project_navigation_states
               SET location_kind = 'thread', conversation_id = $1
             WHERE project_id = $2 AND actor_id = $3
            "#,
        )
        .bind(existing_thread_id)
        .bind(project_id)
        .bind(actor_id)
        .execute(&navigation_pool)
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !navigation_write.is_finished(),
        "navigation validation must wait for the parent archive decision"
    );
    archive_navigation_parent
        .commit()
        .await
        .expect("commit navigation parent archive");
    let navigation_result = tokio::time::timeout(Duration::from_secs(3), navigation_write)
        .await
        .expect("navigation race completion deadline")
        .expect("join navigation writer");
    assert!(
        navigation_result.is_err(),
        "navigation validated after the archive commit must reject the stale child route"
    );

    let mut archive_creation_parent = pool.begin().await.expect("begin creation parent archive");
    sqlx::query(
        "UPDATE conversations SET archived_at = now(), archived_by_actor_id = $1 WHERE id = $2",
    )
    .bind(actor_id)
    .bind(creation_channel_id)
    .execute(&mut *archive_creation_parent)
    .await
    .expect("stage creation parent archive");
    let creation_pool = pool.clone();
    let new_thread_id = Uuid::now_v7();
    let thread_create = tokio::spawn(async move {
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
            VALUES ($1, $2, 'thread', $3, 'Racing child')
            "#,
        )
        .bind(new_thread_id)
        .bind(project_id)
        .bind(creation_channel_id)
        .execute(&creation_pool)
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !thread_create.is_finished(),
        "Thread validation must wait for the parent archive decision"
    );
    archive_creation_parent
        .commit()
        .await
        .expect("commit creation parent archive");
    let create_result = tokio::time::timeout(Duration::from_secs(3), thread_create)
        .await
        .expect("Thread race completion deadline")
        .expect("join Thread creator");
    assert!(
        create_result.is_err(),
        "Thread validated after the archive commit must reject the archived parent"
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated archive-race schema");
}

/// Today's schema: every migration, as the server runs them. Race tests use
/// it so they test the code that runs now, not an early version of it.
async fn apply_every_migration(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .expect("apply every migration");
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect to isolated M2 schema")
}
