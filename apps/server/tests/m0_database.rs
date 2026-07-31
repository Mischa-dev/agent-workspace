use std::collections::BTreeMap;

use agent_workspace_server::{
    config::AppConfig, domain::allocate_conversation_sequence, seed::dev_seed,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn walking_skeleton_schema_enforces_core_invariants() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            "/tmp/agent-workspace-test".to_owned(),
        ),
    ]))
    .expect("development config");

    let first = dev_seed(&pool, &config).await.expect("first seed");
    let second = dev_seed(&pool, &config).await.expect("idempotent seed");
    assert_eq!(first.human_actor_id, second.human_actor_id);
    assert_eq!(first.project_id, second.project_id);
    assert_eq!(first.thread_id, second.thread_id);
    assert_eq!(first.repository_id, second.repository_id);

    let mut foreign_key_transaction = pool.begin().await.expect("foreign-key transaction");
    let invalid_membership = sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'member', 'Invalid actor', 'invalid-actor')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(first.project_id)
    .bind(Uuid::now_v7())
    .execute(&mut *foreign_key_transaction)
    .await;
    assert!(
        invalid_membership.is_err(),
        "Actor foreign key must be enforced"
    );
    foreign_key_transaction
        .rollback()
        .await
        .expect("rollback foreign-key transaction");

    let mut transaction = pool.begin().await.expect("invariant transaction");
    let first_sequence = allocate_conversation_sequence(&mut transaction, first.thread_id)
        .await
        .expect("first sequence");
    let second_sequence = allocate_conversation_sequence(&mut transaction, first.thread_id)
        .await
        .expect("second sequence");
    assert_eq!(second_sequence, first_sequence + 1);

    insert_active_run(&mut transaction, &first, Uuid::now_v7())
        .await
        .expect("insert first active run");
    let duplicate = insert_active_run(&mut transaction, &first, Uuid::now_v7()).await;
    assert!(
        duplicate.is_err(),
        "only one non-terminal Agent Run may exist per Thread"
    );
    transaction
        .rollback()
        .await
        .expect("rollback invariant transaction");
}

async fn insert_active_run(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    seed: &agent_workspace_server::seed::DevSeed,
    run_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'working', 'integration test', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
