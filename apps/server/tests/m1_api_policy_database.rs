use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, rate_limit::RateLimiters, realtime::RealtimeHub, router,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn durable_m0_routes_require_a_session_and_exact_project_membership() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m1_api_policy_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated API-policy schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let fixture = insert_fixture(&pool).await;
    let temporary_root = tempfile::tempdir().expect("temporary web root");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary_root.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("development config");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
    });

    let unauthenticated = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/conversations/{}/timeline",
                    fixture.own_conversation_id
                ))
                .body(Body::empty())
                .expect("unauthenticated request"),
        )
        .await
        .expect("unauthenticated response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let own_timeline = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/conversations/{}/timeline",
                    fixture.own_conversation_id
                ))
                .header("cookie", fixture.session_cookie())
                .body(Body::empty())
                .expect("own timeline request"),
        )
        .await
        .expect("own timeline response");
    assert_eq!(own_timeline.status(), StatusCode::OK);
    let own_timeline_body = to_bytes(own_timeline.into_body(), 1_000_000)
        .await
        .expect("own timeline body");
    let own_timeline_json: serde_json::Value =
        serde_json::from_slice(&own_timeline_body).expect("own timeline JSON");
    assert_eq!(
        own_timeline_json["capabilities"]["workerInvoke"],
        serde_json::Value::Bool(false),
        "a Viewer receives an explicit fail-closed Worker capability"
    );

    let foreign_timeline = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/conversations/{}/timeline",
                    fixture.foreign_conversation_id
                ))
                .header("cookie", fixture.session_cookie())
                .body(Body::empty())
                .expect("foreign timeline request"),
        )
        .await
        .expect("foreign timeline response");
    assert_eq!(
        foreign_timeline.status(),
        StatusCode::NOT_FOUND,
        "a cross-Project resource must be indistinguishable from a missing one"
    );

    let missing_csrf = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/threads/{}/agent-runs",
                    fixture.own_conversation_id
                ))
                .header("origin", "http://127.0.0.1:3100")
                .header("cookie", fixture.session_cookie())
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"policy check"}"#))
                .expect("missing CSRF request"),
        )
        .await
        .expect("missing CSRF response");
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let viewer_invocation = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/threads/{}/agent-runs",
                    fixture.own_conversation_id
                ))
                .header("origin", "http://127.0.0.1:3100")
                .header("cookie", fixture.full_cookie_header())
                .header("x-csrf-token", &fixture.csrf_token)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"policy check"}"#))
                .expect("Viewer invocation request"),
        )
        .await
        .expect("Viewer invocation response");
    assert_eq!(
        viewer_invocation.status(),
        StatusCode::FORBIDDEN,
        "a Viewer may read the timeline but cannot invoke a Worker"
    );
    let viewer_switch = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{}/switch",
                    fixture.own_project_id
                ))
                .header("origin", "http://127.0.0.1:3100")
                .header("cookie", fixture.full_cookie_header())
                .header("x-csrf-token", &fixture.csrf_token)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"expectedVersion":0}"#))
                .expect("Viewer Project switch request"),
        )
        .await
        .expect("Viewer Project switch response");
    assert_eq!(
        viewer_switch.status(),
        StatusCode::FORBIDDEN,
        "Project switching advances navigation state and requires NavigationWrite"
    );

    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(fixture.own_project_id)
    .bind(fixture.actor_id)
    .execute(&pool)
    .await
    .expect("disable membership");
    let disabled_membership = application
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/conversations/{}/timeline",
                    fixture.own_conversation_id
                ))
                .header("cookie", fixture.session_cookie())
                .body(Body::empty())
                .expect("disabled membership request"),
        )
        .await
        .expect("disabled membership response");
    assert_eq!(
        disabled_membership.status(),
        StatusCode::NOT_FOUND,
        "M2 makes missing and unauthorized Conversations indistinguishable"
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated API-policy schema");
}

struct Fixture {
    actor_id: Uuid,
    own_project_id: Uuid,
    own_conversation_id: Uuid,
    foreign_conversation_id: Uuid,
    session_token: String,
    csrf_token: String,
}

impl Fixture {
    fn session_cookie(&self) -> String {
        format!("agent_workspace_session={}", self.session_token)
    }

    fn full_cookie_header(&self) -> String {
        format!(
            "agent_workspace_session={}; agent_workspace_csrf={}",
            self.session_token, self.csrf_token
        )
    }
}

async fn insert_fixture(pool: &PgPool) -> Fixture {
    let actor_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let own_project_id = Uuid::now_v7();
    let foreign_project_id = Uuid::now_v7();
    let own_channel_id = Uuid::now_v7();
    let own_conversation_id = Uuid::now_v7();
    let foreign_conversation_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let suffix = actor_id.simple().to_string();
    let session_token = format!("session-{suffix}");
    let csrf_token = format!("csrf-{suffix}");

    let mut transaction = pool.begin().await.expect("fixture transaction");
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&mut *transaction)
        .await
        .expect("insert Actor");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, 'M1 API Viewer')",
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(format!("m1api{}", &suffix[..20]))
    .execute(&mut *transaction)
    .await
    .expect("insert User");
    sqlx::query(
        "INSERT INTO organizations (id, name, slug) VALUES ($1, 'M1 API organization', $2)",
    )
    .bind(organization_id)
    .bind(format!("m1-api-{}", &suffix[..20]))
    .execute(&mut *transaction)
    .await
    .expect("insert Organization");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES
            ($1, $2, 'PA', 'Policy Project'),
            ($3, $2, 'PB', 'Foreign Project')
        "#,
    )
    .bind(own_project_id)
    .bind(organization_id)
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Projects");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'viewer', 'M1 API Viewer', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(own_project_id)
    .bind(actor_id)
    .bind(format!("api-{}", &suffix[..16]))
    .execute(&mut *transaction)
    .await
    .expect("insert Project membership");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES
            ($1, $2, 'channel', NULL, 'Policy channel'),
            ($3, $2, 'thread', $1, 'Policy Thread'),
            ($4, $5, 'channel', NULL, 'Foreign channel')
        "#,
    )
    .bind(own_channel_id)
    .bind(own_project_id)
    .bind(own_conversation_id)
    .bind(foreign_conversation_id)
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert Conversations");
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, $3, $4, 1,
            now() + interval '1 hour', now() + interval '1 day'
        )
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(Sha256::digest(session_token.as_bytes()).to_vec())
    .bind(Sha256::digest(csrf_token.as_bytes()).to_vec())
    .execute(&mut *transaction)
    .await
    .expect("insert session");
    transaction.commit().await.expect("commit fixture");

    Fixture {
        actor_id,
        own_project_id,
        own_conversation_id,
        foreign_conversation_id,
        session_token,
        csrf_token,
    }
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
        .expect("connect to isolated API-policy schema")
}
