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
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";
const REDIRECT: &str = "https://agent.example.com/oauth/callback";

/// A hosted client (like Muse) signs in through OAuth as one of the owner's
/// agents, uses the agent tools, and loses access when the agent is disabled.
#[tokio::test]
async fn hosted_client_can_sign_in_as_an_agent() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("oauth_agent_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("OAuth Organization".to_owned()),
            organization_slug: Some(format!("oauth-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let (cookie, csrf) = login(&app).await;

    let space = send(
        &app,
        browser(&cookie, &csrf, "GET", "/api/v1/direct", Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (status, created) = send(
        &app,
        browser(
            &cookie,
            &csrf,
            "POST",
            &format!("/api/v1/projects/{space}/external-agents"),
            json!({
                "connectionName": "Muse",
                "displayName": "Muse",
                "handle": "muse",
                "role": "member",
                "scopes": ["read", "write", "events"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let connection_id = created["connection"]["id"]
        .as_str()
        .expect("connection")
        .to_owned();
    let version = created["connection"]["version"].clone();

    let (status, registration) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "client_name": "Muse",
                    "redirect_uris": [REDIRECT],
                    "token_endpoint_auth_method": "none",
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"]
                }))
                .expect("registration"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{registration}");
    let client_id = registration["client_id"]
        .as_str()
        .expect("client")
        .to_owned();

    let verifier = "m".repeat(43);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let query = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", REDIRECT),
        ("scope", "read write"),
        ("state", "muse-state"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ])
    .expect("query");
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/oauth/authorize?{query}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .expect("authorize"),
        )
        .await
        .expect("authorize response");
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(
        html.contains(&format!("value=\"agent:{connection_id}\"")),
        "the consent page offers the agent"
    );

    let consent = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", REDIRECT),
        ("scope", "read write"),
        ("state", "muse-state"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("connect_as", &format!("agent:{connection_id}")),
        ("csrf_token", csrf.as_str()),
        ("decision", "approve"),
    ])
    .expect("consent");
    let approved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/confirm")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(consent))
                .expect("consent request"),
        )
        .await
        .expect("consent response");
    assert_eq!(
        approved.status(),
        StatusCode::OK,
        "hosted clients get a continue page"
    );
    let continue_page = body_text(approved).await;
    let code = continue_page
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split(['&', '"']).next())
        .expect("code in continue page")
        .to_owned();

    let (status, tokens) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(
                serde_urlencoded::to_string([
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id.as_str()),
                    ("code", code.as_str()),
                    ("redirect_uri", REDIRECT),
                    ("code_verifier", verifier.as_str()),
                ])
                .expect("token form"),
            ))
            .expect("token request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let access = tokens["access_token"].as_str().expect("access").to_owned();

    // The client now acts as Muse: it can read Muse's inbox.
    let session = initialize(&app, &access)
        .await
        .expect("MCP session as Muse");
    let inbox = tool(&app, &access, &session, "inbox_list", json!({})).await;
    assert_eq!(inbox["outcome"], "ok", "{inbox}");

    // Disabling Muse cuts off the OAuth grant too.
    let (status, _) = send(
        &app,
        browser(
            &cookie,
            &csrf,
            "PATCH",
            &format!("/api/v1/projects/{space}/external-agents/{connection_id}"),
            json!({ "expectedVersion": version, "enabled": false }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        initialize(&app, &access).await.is_none(),
        "disabled agent is refused"
    );

    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}

fn browser(cookie: &str, csrf: &str, method: &str, uri: &str, body: Value) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("origin", "http://127.0.0.1:3100")
        .header("cookie", cookie)
        .header("x-csrf-token", csrf);
    if body.is_null() {
        builder.body(Body::empty()).expect("request")
    } else {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("body")))
            .expect("request")
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body")
            .to_vec(),
    )
    .expect("utf-8")
}

async fn initialize(app: &axum::Router, token: &str) -> Option<String> {
    let response = mcp(
        app,
        token,
        None,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "muse", "version": "1" }
            }
        }),
    )
    .await;
    if response.status() != StatusCode::OK {
        return None;
    }
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())?
        .to_owned();
    let initialized = mcp(
        app,
        token,
        Some(&session),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    (initialized.status() == StatusCode::ACCEPTED).then_some(session)
}

async fn tool(
    app: &axum::Router,
    token: &str,
    session: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = mcp(
        app,
        token,
        Some(session),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    let text = body_text(response).await;
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    let value: Value = serde_json::from_str(json).expect("MCP JSON");
    value
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("no structured content: {value}"))
}

async fn mcp(
    app: &axum::Router,
    token: &str,
    session: Option<&str>,
    payload: Value,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        request = request
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-06-18");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&payload).expect("JSON")))
                .expect("MCP request"),
        )
        .await
        .expect("MCP response")
}

async fn login(app: &axum::Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": "owner", "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    (cookies.join("; "), csrf)
}

fn test_app(pool: PgPool, database_url: &str) -> axum::Router {
    let config = AppConfig::from_values(&BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url.to_owned(),
    )]))
    .expect("test configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = Arc::new(schema.to_owned());
    PgPoolOptions::new()
        .max_connections(4)
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
        .expect("connect schema pool")
}
