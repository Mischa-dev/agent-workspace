use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    provider::SupervisorRegistry,
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
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const LOGIN_NAME: &str = "m3-account-owner";
const PASSWORD: &str = "correct horse battery staple";
const API_KEY: &str = "sk-test-value-that-must-never-escape";

#[cfg(unix)]
#[tokio::test]
async fn account_flow_is_session_bound_csrf_safe_idempotent_and_redacted() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m3_account_http_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M3 account schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M3 account migrations");

    let temporary = tempfile::tempdir().expect("temporary M3 account root");
    let data_root = temporary.path().join("data");
    let codex_home_root = data_root.join("profiles");
    fs::create_dir_all(&data_root).expect("create data root");
    let fake_codex = temporary.path().join("fake-codex");
    fs::write(&fake_codex, fake_codex_script()).expect("write fake Codex binary");
    fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
        .expect("make fake Codex executable");
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
            codex_home_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            fake_codex.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
            "2".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS".to_owned(),
            "1".to_owned(),
        ),
    ]))
    .expect("M3 account test configuration");
    let seed = dev_seed(&pool, &config)
        .await
        .expect("seed account fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN_NAME.to_owned(),
            display_name: "M3 Account Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap account owner");
    sqlx::query(
        "UPDATE execution_profiles SET status = 'authentication_required', account_state = 'authentication_required' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("require profile authentication");

    let supervisors = SupervisorRegistry::new(&config).expect("account supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors: supervisors.clone(),
    });
    let base = format!(
        "/api/v1/projects/{}/execution-profiles/{}/account",
        seed.project_id, seed.execution_profile_id
    );

    let unauthenticated = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&base)
                .body(Body::empty())
                .expect("build unauthenticated account request"),
        )
        .await
        .expect("unauthenticated account response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let (status, initial_text, initial) = get_json(&application, &cookie, &base).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["accountState"], "authentication_required");
    assert_eq!(initial["providerAvailable"], true);
    assert_redacted(&initial_text, temporary.path());

    let missing_csrf = post_json(
        &application,
        &cookie,
        None,
        Some("device-key"),
        &format!("{base}/login/device-code"),
        json!({}),
    )
    .await;
    assert_eq!(missing_csrf.0, StatusCode::FORBIDDEN);

    let (device_status, device_text, device) = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("device-key"),
        &format!("{base}/login/device-code"),
        json!({}),
    )
    .await;
    assert_eq!(device_status, StatusCode::OK);
    assert_eq!(device["kind"], "device_code");
    assert_eq!(device["state"], "pending");
    assert_eq!(device["verificationUrl"], "https://example.com/device");
    assert_eq!(device["userCode"], "ABCD-EFGH");
    assert_redacted(&device_text, temporary.path());

    let (_, replay_text, replay) = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("device-key"),
        &format!("{base}/login/device-code"),
        json!({}),
    )
    .await;
    assert_eq!(replay["ceremonyId"], device["ceremonyId"]);
    assert_redacted(&replay_text, temporary.path());

    let concurrent = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("other-key"),
        &format!("{base}/login/browser"),
        json!({}),
    )
    .await;
    assert_eq!(concurrent.0, StatusCode::CONFLICT);

    let canceled = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("cancel-key"),
        &format!("{base}/login/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(canceled.0, StatusCode::OK);
    assert!(canceled.2["ceremony"].is_null());

    let canceled_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("cancel-key"),
        &format!("{base}/login/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(canceled_replay.0, StatusCode::OK);
    assert!(canceled_replay.2["ceremony"].is_null());

    let (api_key_status, api_key_text, api_key_login) = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("api-key-login"),
        &format!("{base}/login/api-key"),
        json!({"apiKey": API_KEY}),
    )
    .await;
    assert_eq!(api_key_status, StatusCode::OK);
    assert_eq!(api_key_login["kind"], "api_key");
    assert_eq!(api_key_login["state"], "completed");
    assert!(!api_key_text.contains(API_KEY));

    let (account_status, account_text, account) = get_json(&application, &cookie, &base).await;
    assert_eq!(account_status, StatusCode::OK);
    assert_eq!(account["accountState"], "authenticated");
    assert_eq!(account["account"]["accountType"], "chatgpt");
    assert_eq!(account["rateLimits"]["primary"]["usedPercent"], 42);
    assert!(account["ceremony"].is_null());
    assert_redacted(&account_text, temporary.path());

    let (logout_status, logout_text, logout) = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("logout-key"),
        &format!("{base}/logout"),
        json!({}),
    )
    .await;
    assert_eq!(logout_status, StatusCode::OK);
    assert_eq!(logout["accountState"], "logged_out");
    assert!(!logout_text.contains(API_KEY));

    let logout_replay = post_json(
        &application,
        &cookie,
        Some(&csrf),
        Some("logout-key"),
        &format!("{base}/logout"),
        json!({}),
    )
    .await;
    assert_eq!(logout_replay.0, StatusCode::OK);
    assert_eq!(logout_replay.2["accountState"], "logged_out");

    let stored_state = sqlx::query_as::<_, (String, String)>(
        "SELECT account_state, status FROM execution_profiles WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .fetch_one(&pool)
    .await
    .expect("read stored account state");
    assert_eq!(
        stored_state,
        (
            "logged_out".to_owned(),
            "authentication_required".to_owned()
        )
    );
    supervisors.shutdown_all().await;
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop isolated M3 account schema");
}

fn fake_codex_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
authenticated=false
while IFS= read -r line; do
  if [[ "$line" != *'"id"'* ]]; then continue; fi
  request_id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  if [[ "$line" == *'"method":"initialize"'* ]]; then
    printf '{"id":%s,"result":{"server":"fake"}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/read"'* ]]; then
    if [[ "$authenticated" == true ]]; then
      printf '{"id":%s,"result":{"account":{"type":"chatgpt","email":"owner@example.com","planType":"plus"},"requiresOpenaiAuth":true}}\n' "$request_id"
    else
      printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$request_id"
    fi
  elif [[ "$line" == *'"method":"account/rateLimits/read"'* ]]; then
    printf '{"id":%s,"result":{"rateLimits":{"primary":{"usedPercent":42,"resetsAt":2000000000,"windowDurationMins":300},"secondary":null}}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/login/start"'* && "$line" == *'"type":"chatgptDeviceCode"'* ]]; then
    printf '{"id":%s,"result":{"type":"chatgptDeviceCode","loginId":"device-login","verificationUrl":"https://example.com/device","userCode":"ABCD-EFGH"}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/login/start"'* && "$line" == *'"type":"chatgpt"'* ]]; then
    printf '{"id":%s,"result":{"type":"chatgpt","loginId":"browser-login","authUrl":"https://example.com/login"}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/login/start"'* && "$line" == *'"type":"apiKey"'* ]]; then
    authenticated=true
    printf '{"id":%s,"result":{"type":"apiKey"}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/login/cancel"'* ]]; then
    printf '{"id":%s,"result":{}}\n' "$request_id"
  elif [[ "$line" == *'"method":"account/logout"'* ]]; then
    authenticated=false
    printf '{"id":%s,"result":{}}\n' "$request_id"
  else
    printf '{"id":%s,"result":{}}\n' "$request_id"
  fi
done
"#
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
    response
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
        .collect::<Vec<_>>()
        .join("; ")
}

fn csrf_from_cookie(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|pair| pair.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned()
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
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("build account GET request"),
        )
        .await
        .expect("account GET response");
    response_json(response).await
}

async fn post_json(
    application: &axum::Router,
    cookie: &str,
    csrf: Option<&str>,
    idempotency_key: Option<&str>,
    uri: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(csrf) = csrf {
        request = request
            .header("origin", "http://127.0.0.1:3100")
            .header("x-csrf-token", csrf);
    }
    if let Some(idempotency_key) = idempotency_key {
        request = request.header("idempotency-key", idempotency_key);
    }
    let response = application
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    serde_json::to_vec(&body).expect("serialize account request"),
                ))
                .expect("build account POST request"),
        )
        .await
        .expect("account POST response");
    response_json(response).await
}

async fn response_json(response: axum::response::Response) -> (StatusCode, String, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read account response");
    let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 account response");
    let value = serde_json::from_str(&text).expect("JSON account response");
    (status, text, value)
}

fn assert_redacted(response: &str, runtime_root: &std::path::Path) {
    for forbidden in [
        API_KEY,
        "codexHomePath",
        "codex_home_path",
        "provider_thread_id",
        "stderr",
    ] {
        assert!(!response.contains(forbidden), "response leaked {forbidden}");
    }
    assert!(!response.contains(runtime_root.to_string_lossy().as_ref()));
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
        .expect("connect schema-scoped M3 account pool")
}
