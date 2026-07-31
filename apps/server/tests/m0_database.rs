use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, domain::allocate_conversation_sequence, rate_limit::RateLimiters,
    realtime::RealtimeHub, recovery, seed::dev_seed,
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
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            "/tmp/agent-workspace-test/codex/profiles".to_owned(),
        ),
    ]))
    .expect("development config");

    let first = dev_seed(&pool, &config).await.expect("first seed");
    let second = dev_seed(&pool, &config).await.expect("idempotent seed");
    assert_eq!(first.human_actor_id, second.human_actor_id);
    assert_eq!(first.collaborator_actor_id, second.collaborator_actor_id);
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
    sqlx::query("UPDATE execution_profiles SET account_state = 'authenticated' WHERE id = $1")
        .bind(first.execution_profile_id)
        .execute(&mut *transaction)
        .await
        .expect("authenticate Profile inside rolled-back invariant transaction");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(first.repository_id)
    .execute(&mut *transaction)
    .await
    .expect("prepare Repository inside rolled-back invariant transaction");
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

    let terminal_run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, finished_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'interrupted',
                'recovery test', '{}'::jsonb, now())
        "#,
    )
    .bind(terminal_run_id)
    .bind(first.thread_id)
    .bind(first.execution_worker_id)
    .bind(first.human_actor_id)
    .bind(first.execution_profile_id)
    .bind(first.repository_id)
    .execute(&pool)
    .await
    .expect("insert terminal run");
    let approval_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options
        )
        VALUES ($1, $2, $3, $4, 'approval', 'stale approval', '[]'::jsonb)
        "#,
    )
    .bind(approval_id)
    .bind(terminal_run_id)
    .bind(first.human_actor_id)
    .bind(format!("integration-{approval_id}"))
    .execute(&pool)
    .await
    .expect("insert stale approval");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
    };
    assert_eq!(
        recovery::cancel_terminal_run_approvals(&state)
            .await
            .expect("cancel stale approval"),
        1
    );
    let approval_status =
        sqlx::query_scalar::<_, String>("SELECT status FROM agent_approvals WHERE id = $1")
            .bind(approval_id)
            .fetch_one(&pool)
            .await
            .expect("read canceled approval");
    assert_eq!(approval_status, "canceled");
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
