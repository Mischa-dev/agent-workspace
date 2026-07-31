use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, rate_limit::RateLimiters, realtime::RealtimeHub, recovery,
    seed::dev_seed,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn startup_recovery_idles_the_provider_session_after_interrupting_its_run() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m1_recovery_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated recovery schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let temporary_root = tempfile::tempdir().expect("temporary recovery root");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary_root
                .path()
                .join("codex")
                .to_string_lossy()
                .into_owned(),
        ),
    ]))
    .expect("development config");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed recovery fixture");
    let provider_session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provider_sessions (
            id, conversation_id, execution_profile_id, provider_thread_id, state
        )
        VALUES ($1, $2, $3, $4, 'active')
        "#,
    )
    .bind(provider_session_id)
    .bind(seed.thread_id)
    .bind(seed.execution_profile_id)
    .bind(format!("recovery-thread-{provider_session_id}"))
    .execute(&pool)
    .await
    .expect("insert active provider Session");
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, repository_id, state,
            prompt, authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'working',
            'recovery provider-session test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert non-terminal Agent Run");

    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
    };
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("reconcile Agent Run and provider Session"),
        1
    );

    let run_state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("read recovered Agent Run");
    let provider_session_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM provider_sessions WHERE id = $1")
            .bind(provider_session_id)
            .fetch_one(&pool)
            .await
            .expect("read reconciled provider Session");
    assert_eq!(run_state, "interrupted");
    assert_eq!(provider_session_state, "idle");

    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("recreate the stale terminal-run provider Session");
    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("repair the already-terminal provider Session"),
        0,
        "provider Session repair must not invent another recovered Agent Run"
    );
    let repaired_provider_session_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM provider_sessions WHERE id = $1")
            .bind(provider_session_id)
            .fetch_one(&pool)
            .await
            .expect("read repaired provider Session");
    assert_eq!(repaired_provider_session_state, "idle");

    let reentered_run_id = Uuid::now_v7();
    sqlx::query("UPDATE provider_sessions SET state = 'active' WHERE id = $1")
        .bind(provider_session_id)
        .execute(&pool)
        .await
        .expect("reactivate provider Session for repeat-crash fixture");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, provider_session_id, repository_id, state,
            prompt, authorization_snapshot, started_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, 'recovering',
            'repeat-crash recovery test', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(reentered_run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(provider_session_id)
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("insert Agent Run left recovering by an earlier startup");

    assert_eq!(
        recovery::reconcile_non_terminal_runs(&state)
            .await
            .expect("re-enter recovery after a second crash"),
        1
    );
    let (reentered_run_state, reentered_session_state, recorded_previous_state) =
        sqlx::query_as::<_, (String, String, String)>(
            r#"
            SELECT ar.state, ps.state, are.payload->>'previous_state'
            FROM agent_runs ar
            JOIN provider_sessions ps ON ps.id = ar.provider_session_id
            JOIN agent_run_events are
              ON are.agent_run_id = ar.id
             AND are.kind = 'recovery'
            WHERE ar.id = $1
            "#,
        )
        .bind(reentered_run_id)
        .fetch_one(&pool)
        .await
        .expect("read repeat-crash recovery result");
    assert_eq!(reentered_run_state, "interrupted");
    assert_eq!(reentered_session_state, "idle");
    assert_eq!(recorded_previous_state, "recovering");

    drop(state);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated recovery schema");
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(4)
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
        .expect("connect to isolated recovery schema")
}
