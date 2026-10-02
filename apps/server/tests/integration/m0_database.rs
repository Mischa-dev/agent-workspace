use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, domain::allocate_conversation_sequence, rate_limit::RateLimiters,
    realtime::RealtimeHub, recovery, seed::dev_seed,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// sqlx accepts two files with the same version at compile time, and the live
/// server runs unmerged branches: a second 0046 would only show up as a
/// startup failure there ("previously applied but has been modified"). Every
/// migration takes the next number, with no duplicates and no gaps.
#[test]
fn migration_versions_are_unique_and_contiguous() {
    let versions: Vec<i64> = sqlx::migrate!("./migrations")
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    let expected: Vec<i64> =
        (1..=i64::try_from(versions.len()).expect("migration count")).collect();
    assert_eq!(
        versions, expected,
        "migration versions must run 1, 2, 3, ... with no duplicates or gaps; check `ls apps/server/migrations` for the next free number"
    );
}

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
    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
        supervisors,
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

    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text, agent_run_id
        )
        VALUES (
            $1, $2, $3, $4, 1,
            '{"version":1,"nodes":[{"type":"text","text":"worker result"}]}'::jsonb,
            'worker result', $5
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(first.project_id)
    .bind(first.thread_id)
    .bind(first.worker_actor_id)
    .bind(terminal_run_id)
    .execute(&pool)
    .await
    .expect("insert consolidated Worker Message");
    let duplicate_worker_message = sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text, agent_run_id
        )
        VALUES (
            $1, $2, $3, $4, 1,
            '{"version":1,"nodes":[{"type":"text","text":"duplicate"}]}'::jsonb,
            'duplicate', $5
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(first.project_id)
    .bind(first.thread_id)
    .bind(first.worker_actor_id)
    .bind(terminal_run_id)
    .execute(&pool)
    .await;
    assert!(
        duplicate_worker_message.is_err(),
        "provider replay cannot create a second Worker Message for one Agent Run"
    );

    let invalid_delivery = sqlx::query(
        "UPDATE agent_approvals SET provider_response_delivered_at = now() WHERE id = $1",
    )
    .bind(approval_id)
    .execute(&pool)
    .await;
    assert!(
        invalid_delivery.is_err(),
        "a delivered provider response must have a durable claim"
    );
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
