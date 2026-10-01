use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const LOGIN_NAME: &str = "m2-concurrent-owner";
const PASSWORD: &str = "correct horse battery staple";

#[derive(Debug)]
struct ThreadSideEffectCounts {
    conversations: i64,
    read_states: i64,
    created_outbox_events: i64,
    creation_audits: i64,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_thread_create_is_linearizable_and_exactly_once() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let administrative_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("m2_concurrent_commands_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated concurrent-command schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let bootstrapped = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M2 Concurrent Owner".to_owned(),
            organization_name: Some("M2 Concurrent Organization".to_owned()),
            organization_slug: Some(format!("m2-concurrent-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap concurrent-command owner");
    let app = test_app(pool.clone(), &database_url);
    let (cookie, csrf) = login(&app).await;

    let project_response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/projects",
            &write_headers(&cookie, &csrf, Some("m2-concurrent-project")),
            json!({
                "organizationId": bootstrapped.organization_id,
                "key": "m2c",
                "name": "M2 Concurrent Commands"
            }),
        ))
        .await
        .expect("create Project");
    assert_eq!(project_response.status(), StatusCode::CREATED);
    let project_id = uuid_field(&response_json(project_response).await, "id");

    let channel_response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/projects/{project_id}/channels"),
            &write_headers(&cookie, &csrf, Some("m2-concurrent-channel")),
            json!({"name": "Concurrent command proofs"}),
        ))
        .await
        .expect("create Channel");
    assert_eq!(channel_response.status(), StatusCode::CREATED);
    let channel_id = uuid_field(&response_json(channel_response).await["conversation"], "id");

    let identical_before = thread_side_effect_counts(&pool, project_id, channel_id).await;
    let (identical_first, identical_second) = tokio::join!(
        create_thread(
            &app,
            &cookie,
            &csrf,
            channel_id,
            "concurrent-identical-thread",
            "Canonical concurrent Thread",
        ),
        create_thread(
            &app,
            &cookie,
            &csrf,
            channel_id,
            "concurrent-identical-thread",
            "Canonical concurrent Thread",
        )
    );
    assert_eq!(identical_first.0, StatusCode::CREATED);
    assert_eq!(identical_second.0, StatusCode::CREATED);
    assert_eq!(
        identical_first.1, identical_second.1,
        "an identical concurrent retry must replay the canonical response"
    );
    let identical_thread_id = uuid_field(&identical_first.1["conversation"], "id");
    assert_eq!(
        identical_first.1["conversation"]["name"],
        "Canonical concurrent Thread"
    );
    let identical_after = thread_side_effect_counts(&pool, project_id, channel_id).await;
    assert_incremented_once(
        &identical_before,
        &identical_after,
        "identical concurrent create",
    );
    assert_thread_result(
        &pool,
        project_id,
        channel_id,
        bootstrapped.actor_id,
        "concurrent-identical-thread",
        identical_thread_id,
    )
    .await;

    let mismatch_before = thread_side_effect_counts(&pool, project_id, channel_id).await;
    let (mismatch_first, mismatch_second) = tokio::join!(
        create_thread(
            &app,
            &cookie,
            &csrf,
            channel_id,
            "concurrent-mismatched-thread",
            "Concurrent contender Alpha",
        ),
        create_thread(
            &app,
            &cookie,
            &csrf,
            channel_id,
            "concurrent-mismatched-thread",
            "Concurrent contender Beta",
        )
    );
    let mismatch_responses = [mismatch_first, mismatch_second];
    assert_eq!(
        mismatch_responses
            .iter()
            .filter(|(status, _)| *status == StatusCode::CREATED)
            .count(),
        1,
        "one mismatched contender must create the canonical Thread"
    );
    assert_eq!(
        mismatch_responses
            .iter()
            .filter(|(status, _)| *status == StatusCode::CONFLICT)
            .count(),
        1,
        "the other mismatched contender must conflict"
    );
    let (_, conflict_body) = mismatch_responses
        .iter()
        .find(|(status, _)| *status == StatusCode::CONFLICT)
        .expect("conflict response");
    assert_eq!(conflict_body["error"]["code"], "conflict");
    let (_, created_body) = mismatch_responses
        .iter()
        .find(|(status, _)| *status == StatusCode::CREATED)
        .expect("created response");
    let mismatch_thread_id = uuid_field(&created_body["conversation"], "id");

    let contenders = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, name
        FROM conversations
        WHERE project_id = $1
          AND parent_channel_id = $2
          AND kind = 'thread'
          AND name IN ($3, $4)
        "#,
    )
    .bind(project_id)
    .bind(channel_id)
    .bind("Concurrent contender Alpha")
    .bind("Concurrent contender Beta")
    .fetch_all(&pool)
    .await
    .expect("load mismatched Thread contenders");
    assert_eq!(
        contenders.len(),
        1,
        "the losing payload must not leave a Conversation"
    );
    assert_eq!(contenders[0].0, mismatch_thread_id);
    assert_eq!(created_body["conversation"]["name"], contenders[0].1);

    let mismatch_after = thread_side_effect_counts(&pool, project_id, channel_id).await;
    assert_incremented_once(
        &mismatch_before,
        &mismatch_after,
        "mismatched concurrent create",
    );
    assert_thread_result(
        &pool,
        project_id,
        channel_id,
        bootstrapped.actor_id,
        "concurrent-mismatched-thread",
        mismatch_thread_id,
    )
    .await;

    drop(app);
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated concurrent-command schema");
}

async fn create_thread(
    app: &axum::Router,
    cookie: &str,
    csrf: &str,
    channel_id: Uuid,
    idempotency_key: &str,
    name: &str,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/channels/{channel_id}/threads"),
            &write_headers(cookie, csrf, Some(idempotency_key)),
            json!({"name": name}),
        ))
        .await
        .expect("create Thread response");
    let status = response.status();
    (status, response_json(response).await)
}

async fn thread_side_effect_counts(
    pool: &PgPool,
    project_id: Uuid,
    channel_id: Uuid,
) -> ThreadSideEffectCounts {
    let (conversations, read_states, created_outbox_events, creation_audits) =
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
            SELECT
                (
                    SELECT count(*)
                    FROM conversations
                    WHERE project_id = $1
                      AND parent_channel_id = $2
                      AND kind = 'thread'
                ),
                (
                    SELECT count(*)
                    FROM conversation_read_states read_state
                    JOIN conversations conversation
                      ON conversation.project_id = read_state.project_id
                     AND conversation.id = read_state.conversation_id
                    WHERE conversation.project_id = $1
                      AND conversation.parent_channel_id = $2
                      AND conversation.kind = 'thread'
                ),
                (
                    SELECT count(*)
                    FROM outbox_events
                    WHERE project_id = $1
                      AND event_type = 'conversation.created'
                ),
                (
                    SELECT count(*)
                    FROM audit_events
                    WHERE project_id = $1
                      AND action = 'conversation.thread_created'
                )
            "#,
        )
        .bind(project_id)
        .bind(channel_id)
        .fetch_one(pool)
        .await
        .expect("count Thread command side effects");
    ThreadSideEffectCounts {
        conversations,
        read_states,
        created_outbox_events,
        creation_audits,
    }
}

fn assert_incremented_once(
    before: &ThreadSideEffectCounts,
    after: &ThreadSideEffectCounts,
    context: &str,
) {
    assert_eq!(
        after.conversations,
        before.conversations + 1,
        "{context} must insert exactly one Conversation"
    );
    assert_eq!(
        after.read_states,
        before.read_states + 1,
        "{context} must insert exactly one read state"
    );
    assert_eq!(
        after.created_outbox_events,
        before.created_outbox_events + 1,
        "{context} must insert exactly one creation outbox event"
    );
    assert_eq!(
        after.creation_audits,
        before.creation_audits + 1,
        "{context} must insert exactly one creation audit"
    );
}

async fn assert_thread_result(
    pool: &PgPool,
    project_id: Uuid,
    channel_id: Uuid,
    actor_id: Uuid,
    idempotency_key: &str,
    thread_id: Uuid,
) {
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM conversations
            WHERE id = $1
              AND project_id = $2
              AND parent_channel_id = $3
              AND kind = 'thread'
            "#,
        )
        .bind(thread_id)
        .bind(project_id)
        .bind(channel_id)
        .fetch_one(pool)
        .await
        .expect("count canonical Thread"),
        1
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT highest_seen_sequence, version
            FROM conversation_read_states
            WHERE project_id = $1
              AND conversation_id = $2
              AND actor_id = $3
            "#,
        )
        .bind(project_id)
        .bind(thread_id)
        .bind(actor_id)
        .fetch_all(pool)
        .await
        .expect("load canonical Thread read state"),
        vec![(0, 1)]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM outbox_events
            WHERE project_id = $1
              AND conversation_id = $2
              AND resource_kind = 'conversation'
              AND resource_id = $2
              AND event_type = 'conversation.created'
            "#,
        )
        .bind(project_id)
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count canonical Thread outbox event"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM audit_events
            WHERE actor_id = $1
              AND project_id = $2
              AND action = 'conversation.thread_created'
              AND target_kind = 'conversation'
              AND target_id = $3
              AND outcome = 'succeeded'
            "#,
        )
        .bind(actor_id)
        .bind(project_id)
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count canonical Thread audit"),
        1
    );

    let scope = format!("channel.{}.thread_create", channel_id.simple());
    let idempotency_rows = sqlx::query_as::<_, (Option<i32>, Option<Value>)>(
        r#"
        SELECT response_status, response_body
        FROM idempotency_keys
        WHERE actor_id = $1
          AND scope = $2
          AND key = $3
        "#,
    )
    .bind(actor_id)
    .bind(scope)
    .bind(idempotency_key)
    .fetch_all(pool)
    .await
    .expect("load canonical idempotency result");
    assert_eq!(
        idempotency_rows.len(),
        1,
        "one completed idempotency result must remain"
    );
    assert_eq!(idempotency_rows[0].0, Some(201));
    let response_body = idempotency_rows[0]
        .1
        .as_ref()
        .expect("completed idempotency response body");
    assert_eq!(uuid_field(&response_body["conversation"], "id"), thread_id);
}

async fn login(app: &axum::Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/auth/login",
            &[("origin", "http://127.0.0.1:3100")],
            json!({
                "loginName": LOGIN_NAME,
                "password": PASSWORD
            }),
        ))
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("cookie header")
                .split(';')
                .next()
                .expect("cookie pair")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    let cookie = cookies.join("; ");
    let _ = response_json(response).await;
    (cookie, csrf)
}

fn uuid_field(value: &Value, field: &str) -> Uuid {
    Uuid::parse_str(value[field].as_str().expect("UUID field")).expect("valid UUID")
}

fn test_app(pool: PgPool, database_url: &str) -> axum::Router {
    let values = BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url.to_owned(),
    )]);
    let config = AppConfig::from_values(&values).expect("test configuration");
    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

fn write_headers<'a>(
    cookie: &'a str,
    csrf: &'a str,
    idempotency_key: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut headers = vec![
        ("origin", "http://127.0.0.1:3100"),
        ("cookie", cookie),
        ("x-csrf-token", csrf),
    ];
    if let Some(key) = idempotency_key {
        headers.push(("idempotency-key", key));
    }
    headers
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: Value) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize request body"))
    };
    builder.body(body).expect("build request")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("read response body"),
    )
    .expect("parse response JSON")
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _metadata| {
            let schema = Arc::clone(&schema);
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema.as_str())
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect schema-scoped concurrent-command pool")
}
