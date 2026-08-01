use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, domain::allocate_conversation_sequence,
    provider::SupervisorRegistry, rate_limit::RateLimiters, realtime::RealtimeHub, router,
    seed::dev_seed,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode, header},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3100";

#[tokio::test]
async fn failed_run_acknowledgement_is_private_durable_and_actor_scoped() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_attention_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated attention schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run attention migrations");

    let temporary = tempfile::tempdir().expect("temporary attention root");
    let data_root = temporary.path().join("data");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root.join("codex").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("attention configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed attention fixture");
    let actor_a = insert_session(&pool, seed.human_actor_id, "attention-a").await;
    let actor_b = insert_session(&pool, seed.collaborator_actor_id, "attention-b").await;

    let run_id = Uuid::now_v7();
    let failure_event_id = Uuid::now_v7();
    let mut transaction = pool.begin().await.expect("attention transaction");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, failure_reason, finished_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'failed',
            'two Actor attention proof', '{}'::jsonb,
            'A bounded synthetic failure.', now()
        )
        "#,
    )
    .bind(run_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .execute(&mut *transaction)
    .await
    .expect("insert failed attention run");
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, 'attention:failed', 'run_failed',
                'The synthetic run failed.', '{}'::jsonb)
        "#,
    )
    .bind(failure_event_id)
    .bind(run_id)
    .execute(&mut *transaction)
    .await
    .expect("insert durable failure event");
    let failure_sequence = allocate_conversation_sequence(&mut transaction, seed.thread_id)
        .await
        .expect("allocate failure sequence");
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, agent_run_event_id
        )
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(seed.thread_id)
    .bind(failure_sequence)
    .bind(failure_event_id)
    .execute(&mut *transaction)
    .await
    .expect("attach durable failure event");
    transaction
        .commit()
        .await
        .expect("commit attention fixture");

    let supervisors = SupervisorRegistry::new(&config).expect("attention supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    });

    assert_eq!(
        mark_seen(&application, &actor_a, seed.thread_id, failure_sequence).await,
        StatusCode::OK
    );
    assert_eq!(
        mark_seen(&application, &actor_b, seed.thread_id, failure_sequence).await,
        StatusCode::OK
    );
    assert_eq!(
        thread_attention(&sidebar(&application, &actor_a, seed.project_id, seed.channel_id).await),
        "failed"
    );
    assert_eq!(
        thread_attention(&sidebar(&application, &actor_b, seed.project_id, seed.channel_id).await),
        "failed"
    );

    assert_eq!(
        acknowledge_failure(
            &application,
            &actor_a,
            run_id,
            failure_event_id,
            "actor-a-first"
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        acknowledge_failure(
            &application,
            &actor_a,
            run_id,
            failure_event_id,
            "actor-a-retry"
        )
        .await,
        StatusCode::OK
    );

    assert_eq!(
        thread_attention(&sidebar(&application, &actor_a, seed.project_id, seed.channel_id).await),
        "settled"
    );
    assert_eq!(
        thread_attention(&sidebar(&application, &actor_b, seed.project_id, seed.channel_id).await),
        "failed"
    );
    assert_eq!(
        timeline(&application, &actor_a, seed.thread_id).await["latestTerminalRun"]["failureAcknowledged"],
        true
    );
    assert_eq!(
        timeline(&application, &actor_b, seed.thread_id).await["latestTerminalRun"]["failureAcknowledged"],
        false
    );
    let acknowledgement_actors = sqlx::query_scalar::<_, Vec<Uuid>>(
        "SELECT array_agg(actor_id ORDER BY actor_id) FROM agent_run_acknowledgements WHERE agent_run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("read isolated acknowledgement Actors");
    assert_eq!(acknowledgement_actors, vec![seed.human_actor_id]);

    assert_eq!(
        acknowledge_failure(
            &application,
            &actor_b,
            run_id,
            failure_event_id,
            "actor-b-first"
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        thread_attention(&sidebar(&application, &actor_b, seed.project_id, seed.channel_id).await),
        "settled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM agent_run_acknowledgements WHERE agent_run_id = $1",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count independent acknowledgements"),
        2
    );

    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated attention schema");
}

#[derive(Clone)]
struct ActorSession {
    cookie: String,
    csrf: String,
}

async fn insert_session(pool: &PgPool, actor_id: Uuid, label: &str) -> ActorSession {
    let user_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let session_token = format!("{label}-{}-session-token", Uuid::now_v7());
    let csrf = format!("{label}-{}-csrf-token", Uuid::now_v7());
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(label)
    .bind(label)
    .execute(pool)
    .await
    .expect("insert attention User");
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES ($1, $2, $3, $4, 1,
                now() + interval '1 hour', now() + interval '1 day')
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(Sha256::digest(session_token.as_bytes()).to_vec())
    .bind(Sha256::digest(csrf.as_bytes()).to_vec())
    .execute(pool)
    .await
    .expect("insert attention Session");
    ActorSession {
        cookie: format!("agent_workspace_session={session_token}; agent_workspace_csrf={csrf}"),
        csrf,
    }
}

async fn mark_seen(
    application: &axum::Router,
    actor: &ActorSession,
    conversation_id: Uuid,
    sequence: i64,
) -> StatusCode {
    application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/conversations/{conversation_id}/mark-seen"))
                .header("origin", ORIGIN)
                .header("cookie", &actor.cookie)
                .header("x-csrf-token", &actor.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"highestSeenSequence": sequence}))
                        .expect("serialize mark-seen request"),
                ))
                .expect("build mark-seen request"),
        )
        .await
        .expect("mark-seen response")
        .status()
}

async fn acknowledge_failure(
    application: &axum::Router,
    actor: &ActorSession,
    run_id: Uuid,
    event_id: Uuid,
    idempotency_key: &str,
) -> StatusCode {
    application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/agent-runs/{run_id}/acknowledge-failure"))
                .header("origin", ORIGIN)
                .header("cookie", &actor.cookie)
                .header("x-csrf-token", &actor.csrf)
                .header("idempotency-key", idempotency_key)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"eventId": event_id}))
                        .expect("serialize acknowledgement request"),
                ))
                .expect("build acknowledgement request"),
        )
        .await
        .expect("acknowledgement response")
        .status()
}

async fn sidebar(
    application: &axum::Router,
    actor: &ActorSession,
    project_id: Uuid,
    channel_id: Uuid,
) -> Value {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/projects/{project_id}/conversation-sidebar?selectedChannelId={channel_id}"
                    ))
                    .header("cookie", &actor.cookie)
                    .body(Body::empty())
                    .expect("build attention sidebar request"),
            )
            .await
            .expect("attention sidebar response"),
    )
    .await
}

fn thread_attention(sidebar: &Value) -> &str {
    sidebar["channels"][0]["threads"][0]["attention"]["kind"]
        .as_str()
        .expect("accessible attention kind")
}

async fn timeline(
    application: &axum::Router,
    actor: &ActorSession,
    conversation_id: Uuid,
) -> Value {
    response_json(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/conversations/{conversation_id}/timeline"))
                    .header("cookie", &actor.cookie)
                    .body(Body::empty())
                    .expect("build attention timeline request"),
            )
            .await
            .expect("attention timeline response"),
    )
    .await
}

async fn response_json(response: Response<Body>) -> Value {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024)
        .await
        .expect("read attention response body");
    let value: Value = serde_json::from_slice(&bytes).expect("decode attention JSON response");
    assert_eq!(status, StatusCode::OK, "attention response: {value}");
    value
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
        .expect("connect to attention PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let search_path = format!("SET search_path TO {schema}");
            Box::pin(async move {
                sqlx::query(&search_path).execute(connection).await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect to isolated attention schema")
}
