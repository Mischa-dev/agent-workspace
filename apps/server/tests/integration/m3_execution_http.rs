use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    seed::dev_seed,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const LOGIN_NAME: &str = "m3-execution-owner";
const PASSWORD: &str = "correct horse battery staple";
const SECRET_CAPABILITY_VALUE: &str = "m3-provider-secret-must-not-escape";
const RAW_ERROR_VALUE: &str = "raw provider failure token=m3-secret";

#[tokio::test]
async fn execution_resources_are_project_scoped_and_status_only() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_execution_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M3 execution schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M3 execution migrations");

    let temporary_root = tempfile::Builder::new()
        .prefix("must-not-leak-execution-root-")
        .tempdir()
        .expect("temporary M3 execution root");
    let runtime_root = temporary_root
        .path()
        .canonicalize()
        .expect("canonical M3 execution root");
    let data_root = runtime_root.join("data");
    let codex_home_root = data_root.join("private-codex-homes");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            codex_home_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            runtime_root.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M3 execution test configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed M3 execution fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M3 Execution Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M3 execution owner");

    sqlx::query(
        r#"
        UPDATE execution_profiles
        SET account_state = 'authenticated',
            last_health_at = '2026-07-31T12:00:00Z',
            last_failure_category = 'process_start_failed'
        WHERE id = $1
        "#,
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("set safe Execution Profile status metadata");
    sqlx::query(
        r#"
        UPDATE provider_connections
        SET capabilities = capabilities || $2
        WHERE id = $1
        "#,
    )
    .bind(seed.provider_connection_id)
    .bind(json!({
        "credential": SECRET_CAPABILITY_VALUE,
        "raw_provider_error": RAW_ERROR_VALUE,
        "unknown_private_capability": true
    }))
    .execute(&pool)
    .await
    .expect("add provider-private capability fixture");
    let foreign_project_id = insert_foreign_execution_fixture(&pool, seed.organization_id).await;

    let supervisors = agent_workspace_server::provider::SupervisorRegistry::new(&config)
        .expect("test supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    });
    let cookie = login(&application).await;

    let unauthenticated = application
        .clone()
        .oneshot(get(&format!(
            "/api/v1/projects/{}/execution-workers",
            seed.project_id
        )))
        .await
        .expect("unauthenticated Worker response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let (worker_status, worker_text, workers) = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/execution-workers", seed.project_id),
    )
    .await;
    assert_eq!(worker_status, StatusCode::OK);
    assert_eq!(workers["workers"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        workers["workers"][0]["id"],
        seed.execution_worker_id.to_string()
    );
    assert_eq!(workers["workers"][0]["version"], 1);
    assert_eq!(workers["workers"][0]["actor"]["kind"], "execution_worker");
    assert_eq!(workers["workers"][0]["actor"]["handle"], "agent");
    assert_eq!(workers["workers"][0]["status"], "available");
    assert_eq!(
        workers["workers"][0]["provider"]["binaryVersion"],
        "0.146.0"
    );
    assert_eq!(
        workers["workers"][0]["provider"]["capabilities"]["supportsStreaming"],
        true
    );
    assert_redacted(&worker_text, &runtime_root);

    let (profile_status, profile_text, profiles) = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/execution-profiles", seed.project_id),
    )
    .await;
    assert_eq!(profile_status, StatusCode::OK);
    assert_eq!(profiles["profiles"].as_array().map(Vec::len), Some(1));
    let profile = &profiles["profiles"][0];
    assert_eq!(profile["id"], seed.execution_profile_id.to_string());
    assert_eq!(profile["version"], 1);
    assert_eq!(profile["status"], "available");
    assert_eq!(profile["accountState"], "authenticated");
    assert_eq!(profile["maxActiveTurns"], 1);
    assert_eq!(profile["lastFailureCategory"], "process_start_failed");
    assert_eq!(profile["availableForNewRuns"], true);
    assert_eq!(profile["supervisor"]["state"], "stopped");
    assert_eq!(profile["supervisor"]["consecutiveFailures"], 0);
    assert_eq!(profile["supervisor"]["processStarts"], 0);
    assert_eq!(profile["owner"]["id"], seed.human_actor_id.to_string());
    assert_redacted(&profile_text, &runtime_root);

    let (foreign_status, _, foreign_body) = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{foreign_project_id}/execution-profiles"),
    )
    .await;
    assert_eq!(foreign_status, StatusCode::NOT_FOUND);
    let (missing_status, _, missing_body) = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/execution-profiles", Uuid::now_v7()),
    )
    .await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(
        foreign_body, missing_body,
        "foreign and missing Projects are non-disclosing"
    );

    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("disable requesting Project membership");
    let (disabled_status, _, disabled_body) = get_json(
        &application,
        &cookie,
        &format!("/api/v1/projects/{}/execution-workers", seed.project_id),
    )
    .await;
    assert_eq!(disabled_status, StatusCode::NOT_FOUND);
    assert_eq!(
        disabled_body, missing_body,
        "disabled membership is non-disclosing"
    );

    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M3 execution schema");
}

fn assert_redacted(response: &str, private_runtime_root: &std::path::Path) {
    for forbidden in [
        "codexHomePath",
        "codex_home_path",
        "mirrorPath",
        "worktreePath",
        "credential",
        "raw_provider_error",
        "unknown_private_capability",
        SECRET_CAPABILITY_VALUE,
        RAW_ERROR_VALUE,
        "local_project_host",
    ] {
        assert!(
            !response.contains(forbidden),
            "execution DTO leaked forbidden field or value: {forbidden}"
        );
    }
    assert!(
        !response.contains(private_runtime_root.to_string_lossy().as_ref()),
        "execution DTO leaked a private runtime path"
    );
}

async fn insert_foreign_execution_fixture(pool: &PgPool, organization_id: Uuid) -> Uuid {
    let project_id = Uuid::now_v7();
    let owner_actor_id = Uuid::now_v7();
    let worker_actor_id = Uuid::now_v7();
    let connection_id = Uuid::now_v7();
    let profile_id = Uuid::now_v7();
    let mut transaction = pool.begin().await.expect("foreign execution transaction");
    sqlx::query("INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'FX', 'Foreign execution')")
        .bind(project_id)
        .bind(organization_id)
        .execute(&mut *transaction)
        .await
        .expect("insert foreign Project");
    insert_actor_membership(
        &mut transaction,
        project_id,
        owner_actor_id,
        "human",
        "owner",
        "Foreign Owner",
        "foreign_owner",
    )
    .await;
    insert_actor_membership(
        &mut transaction,
        project_id,
        worker_actor_id,
        "execution_worker",
        "worker",
        "Foreign Agent",
        "foreign_agent",
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO provider_connections (id, project_id, kind, host, capabilities)
        VALUES ($1, $2, 'codex_app_server', 'local_project_host', '{}'::jsonb)
        "#,
    )
    .bind(connection_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert foreign Provider Connection");
    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id,
            name, codex_home_path, status
        )
        VALUES ($1, $2, $3, $4, 'Foreign profile', '/private/foreign-profile', 'available')
        "#,
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(connection_id)
    .bind(owner_actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert foreign Execution Profile");
    sqlx::query(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id, execution_profile_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(connection_id)
    .bind(profile_id)
    .execute(&mut *transaction)
    .await
    .expect("insert foreign Execution Worker");
    transaction
        .commit()
        .await
        .expect("commit foreign execution fixture");
    project_id
}

#[allow(clippy::too_many_arguments)]
async fn insert_actor_membership(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    actor_kind: &str,
    role: &str,
    display_name: &str,
    handle: &str,
) {
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, $2)")
        .bind(actor_id)
        .bind(actor_kind)
        .execute(&mut **transaction)
        .await
        .expect("insert foreign Actor");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .bind(role)
    .bind(display_name)
    .bind(handle)
    .execute(&mut **transaction)
    .await
    .expect("insert foreign Project membership");
}

async fn login(application: &axum::Router) -> String {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "loginName": LOGIN_NAME,
                        "password": PASSWORD
                    }))
                    .expect("serialize login request"),
                ))
                .expect("build login request"),
        )
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
    let _ = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read login response");
    cookies.join("; ")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("build GET request")
}

async fn get_json(
    application: &axum::Router,
    cookie: &str,
    uri: &str,
) -> (StatusCode, String, Value) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("build authenticated GET request"),
        )
        .await
        .expect("execution resource response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read execution resource response");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 execution resource response");
    let value = serde_json::from_str(&text).expect("JSON execution resource response");
    (status, text, value)
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
        .expect("connect schema-scoped M3 execution pool")
}
