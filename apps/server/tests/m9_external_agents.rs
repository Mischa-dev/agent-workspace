use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    mcp::MCP_PATH,
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
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const LOGIN: &str = "m9-owner";
const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn external_agent_onboarding_rotation_disable_and_removal_fail_closed() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m9_external_agents_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M9 schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M9 migrations");

    let temporary = tempfile::tempdir().expect("temporary M9 root");
    let config = test_config(&database_url, temporary.path());
    let seed = dev_seed(&pool, &config).await.expect("seed M9 fixture");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: LOGIN.to_owned(),
            display_name: "M9 Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap M9 owner");
    let application = test_app(pool.clone(), config);
    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let base = format!("/api/v1/projects/{}/external-agents", seed.project_id);

    let created = application
        .clone()
        .oneshot(write_request(
            "POST",
            &base,
            &cookie,
            &csrf,
            json!({
                "connectionName":"Hermes M9",
                "displayName":"Hermes teammate",
                "handle":"Hermes Teammate",
                "role":"member",
                "scopes":["read","write"]
            }),
        ))
        .await
        .expect("create connection response");
    assert_eq!(created.status(), StatusCode::OK);
    let created = response_json(created).await;
    let connection_id = created["connection"]["id"]
        .as_str()
        .expect("connection id")
        .parse::<Uuid>()
        .expect("connection UUID");
    let actor_id = created["connection"]["actorId"]
        .as_str()
        .expect("actor id")
        .parse::<Uuid>()
        .expect("Actor UUID");
    let credential_id = created["oneTimeCredential"]["id"]
        .as_str()
        .expect("credential id")
        .parse::<Uuid>()
        .expect("credential UUID");
    let first_token = created["oneTimeCredential"]["token"]
        .as_str()
        .expect("one-time token")
        .to_owned();
    assert!(first_token.starts_with("aw_ext_"));
    assert_eq!(created["connection"]["handle"], "hermes_teammate");
    assert_eq!(created["connection"]["runtimeVersion"], "0.20.0");
    assert_eq!(created["connection"]["version"], 1);

    let stored = sqlx::query_as::<_, (Vec<u8>, String, Option<Uuid>)>(
        "SELECT token_digest, token_prefix, external_agent_connection_id FROM mcp_client_credentials WHERE id = $1",
    )
    .bind(credential_id)
    .fetch_one(&pool)
    .await
    .expect("stored external credential");
    assert_eq!(stored.0, Sha256::digest(first_token.as_bytes()).as_slice());
    assert!(!String::from_utf8_lossy(&stored.0).contains(&first_token));
    assert_eq!(stored.2, Some(connection_id));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .expect("external Actor kind"),
        "external_agent"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_one(&pool)
            .await
            .expect("external User absence"),
        0
    );

    let list = application
        .clone()
        .oneshot(read_request(&base, &cookie))
        .await
        .expect("list response");
    assert_eq!(list.status(), StatusCode::OK);
    let safe_list = response_json(list).await;
    let safe_serialized = serde_json::to_string(&safe_list).expect("safe list JSON");
    assert!(!safe_serialized.contains(&first_token));
    assert_eq!(safe_list["connections"].as_array().map(Vec::len), Some(1));

    assert_mcp_auth(&application, &first_token, StatusCode::OK).await;
    let connection_uri = format!("{base}/{connection_id}");
    let rotated = application
        .clone()
        .oneshot(write_request(
            "POST",
            &format!("{connection_uri}/credentials/rotate"),
            &cookie,
            &csrf,
            json!({"expectedVersion":1}),
        ))
        .await
        .expect("rotate response");
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated = response_json(rotated).await;
    let second_token = rotated["oneTimeCredential"]["token"]
        .as_str()
        .expect("rotated token")
        .to_owned();
    assert_ne!(first_token, second_token);
    assert_eq!(rotated["connection"]["version"], 2);
    assert_mcp_auth(&application, &first_token, StatusCode::UNAUTHORIZED).await;
    assert_mcp_auth(&application, &second_token, StatusCode::OK).await;

    let disabled = application
        .clone()
        .oneshot(write_request(
            "PATCH",
            &connection_uri,
            &cookie,
            &csrf,
            json!({"expectedVersion":2,"enabled":false}),
        ))
        .await
        .expect("disable response");
    assert_eq!(disabled.status(), StatusCode::OK);
    let disabled = response_json(disabled).await;
    assert_eq!(disabled["connection"]["status"], "disabled");
    assert!(disabled["oneTimeCredential"].is_null());
    assert_mcp_auth(&application, &second_token, StatusCode::UNAUTHORIZED).await;

    let enabled = application
        .clone()
        .oneshot(write_request(
            "PATCH",
            &connection_uri,
            &cookie,
            &csrf,
            json!({"expectedVersion":3,"enabled":true,"role":"viewer"}),
        ))
        .await
        .expect("re-enable response");
    assert_eq!(enabled.status(), StatusCode::OK);
    let enabled = response_json(enabled).await;
    let third_token = enabled["oneTimeCredential"]["token"]
        .as_str()
        .expect("re-enable token")
        .to_owned();
    assert_eq!(enabled["connection"]["version"], 4);
    assert_eq!(enabled["connection"]["role"], "viewer");
    assert_eq!(enabled["connection"]["scopes"], json!(["read"]));
    assert_mcp_auth(&application, &third_token, StatusCode::OK).await;

    let removed = application
        .clone()
        .oneshot(write_request(
            "POST",
            &format!("{connection_uri}/remove"),
            &cookie,
            &csrf,
            json!({"expectedVersion":4}),
        ))
        .await
        .expect("remove response");
    assert_eq!(removed.status(), StatusCode::OK);
    let removed = response_json(removed).await;
    assert_eq!(removed["connection"]["status"], "removed");
    assert_eq!(removed["connection"]["version"], 5);
    assert_mcp_auth(&application, &third_token, StatusCode::UNAUTHORIZED).await;

    let audit_text = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(string_agg(metadata::text, ''), '') FROM audit_events WHERE external_agent_connection_id = $1",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .expect("external audit text");
    for token in [&first_token, &second_token, &third_token] {
        assert!(!audit_text.contains(token));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE external_agent_connection_id = $1",
        )
        .bind(connection_id)
        .fetch_one(&pool)
        .await
        .expect("external audit count"),
        5
    );

    drop(application);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated M9 schema");
}

fn test_config(database_url: &str, root: &std::path::Path) -> AppConfig {
    AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.to_owned(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            root.join("data").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            root.join("data/codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            root.join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M9 test configuration")
}

fn test_app(pool: PgPool, config: AppConfig) -> axum::Router {
    let supervisors = SupervisorRegistry::new(&config).expect("M9 supervisor registry");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    })
}

async fn login(application: &axum::Router) -> String {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"loginName":LOGIN,"password":PASSWORD}))
                        .expect("login JSON"),
                ))
                .expect("login request"),
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
                .expect("cookie")
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
        .find_map(|part| part.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned()
}

fn write_request(method: &str, uri: &str, cookie: &str, csrf: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1")
        .header(header::ORIGIN, "http://127.0.0.1:3100")
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&body).expect("serialize write request"),
        ))
        .expect("write request")
}

fn read_request(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::HOST, "127.0.0.1")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("read request")
}

async fn response_json(response: axum::response::Response) -> Value {
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read response body");
    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "JSON response {status}: {error}; {}",
            String::from_utf8_lossy(&body)
        )
    })
}

async fn assert_mcp_auth(application: &axum::Router, token: &str, status: StatusCode) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(MCP_PATH)
                .header(header::HOST, "127.0.0.1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "jsonrpc":"2.0",
                        "id":1,
                        "method":"initialize",
                        "params":{
                            "protocolVersion":"2025-06-18",
                            "capabilities":{},
                            "clientInfo":{"name":"hermes","version":"0.20.0"}
                        }
                    }))
                    .expect("initialize JSON"),
                ))
                .expect("initialize request"),
        )
        .await
        .expect("initialize response");
    assert_eq!(response.status(), status);
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
        .expect("connect schema-scoped M9 pool")
}
