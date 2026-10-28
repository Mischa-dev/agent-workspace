use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    let fixture = Fixture::new(&database_url, "oauth_agent").await;
    let app = &fixture.app;
    let (_, tokens) = fixture.sign_in_as_agent().await;
    let access = tokens["access_token"].as_str().expect("access").to_owned();

    // The client now acts as Muse: it can read Muse's inbox.
    let session = initialize(app, &access).await.expect("MCP session as Muse");
    let inbox = tool(app, &access, &session, "inbox_list", json!({})).await;
    assert_eq!(inbox["outcome"], "ok", "{inbox}");

    // Disabling Muse cuts off the OAuth grant too.
    let (status, _) = send(
        app,
        browser(
            &fixture.cookie,
            &fixture.csrf,
            "PATCH",
            &format!(
                "/api/v1/projects/{}/external-agents/{}",
                fixture.space, fixture.connection_id
            ),
            json!({ "expectedVersion": fixture.connection_version, "enabled": false }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        initialize(app, &access).await.is_none(),
        "disabled agent is refused"
    );

    fixture.cleanup().await;
}

/// An app running many sessions on one grant refreshes from several of them
/// at the same moment, and each gets tokens that work. Refreshes whose token
/// doesn't check out are charged to whoever sent them, so a flood of junk
/// can't lock real apps out of refreshing (AWD-25).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_refreshes_work_and_junk_cannot_lock_them_out() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let fixture = Fixture::new(&database_url, "oauth_refresh").await;
    let app = &fixture.app;
    let (client_id, tokens) = fixture.sign_in_as_agent().await;
    let refresh = tokens["refresh_token"].as_str().expect("refresh token");

    let sessions = (0..8)
        .map(|_| {
            let app = app.clone();
            let request = token_request(refresh_form(refresh, &client_id), None);
            tokio::spawn(async move { send(&app, request).await })
        })
        .collect::<Vec<_>>();
    let mut accesses = BTreeSet::new();
    let mut refreshes = Vec::new();
    for session in sessions {
        let (status, body) = session.await.expect("refresh task");
        assert_eq!(status, StatusCode::OK, "{body}");
        accesses.insert(body["access_token"].as_str().expect("access").to_owned());
        refreshes.push(body["refresh_token"].as_str().expect("refresh").to_owned());
    }
    assert_eq!(accesses.len(), 8, "each session gets its own tokens");
    for access in &accesses {
        assert!(
            initialize(app, access).await.is_some(),
            "every refreshed token works"
        );
    }

    // One sender's junk is charged to that sender: after its failure budget
    // it waits, while the real app refreshes from anywhere else.
    let junk = format!("aw_refresh_{}", "j".repeat(43));
    for attempt in 0..30 {
        let (status, _) = send(
            app,
            token_request(
                refresh_form(&junk, &Uuid::now_v7().to_string()),
                Some("203.0.113.9"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "junk refresh {attempt}");
    }
    let (status, _) = send(
        app,
        token_request(refresh_form(&junk, &client_id), Some("203.0.113.9")),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, body) = send(
        app,
        token_request(
            refresh_form(&refreshes[0], &client_id),
            Some("198.51.100.7"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let latest = body["refresh_token"].as_str().expect("refresh").to_owned();

    // Junk from many senders at once spends the global failure budget: then
    // an app that got tokens lately still gets through, and nothing else does.
    for sender in 0..20 {
        for _ in 0..30 {
            send(
                app,
                token_request(
                    refresh_form(&junk, &Uuid::now_v7().to_string()),
                    Some(&format!("198.18.{sender}.1")),
                ),
            )
            .await;
        }
    }
    let (status, _) = send(
        app,
        token_request(
            refresh_form(&junk, &Uuid::now_v7().to_string()),
            Some("192.0.2.200"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, body) = send(app, token_request(refresh_form(&latest, &client_id), None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    fixture.cleanup().await;
}

/// A fresh schema with the owner signed in, and an agent, Muse, in the
/// owner's space.
struct Fixture {
    admin: PgPool,
    schema: String,
    app: axum::Router,
    cookie: String,
    csrf: String,
    space: String,
    connection_id: String,
    connection_version: Value,
}

impl Fixture {
    async fn new(database_url: &str, name: &str) -> Self {
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .expect("connect to PostgreSQL");
        let schema = format!("{name}_{}", Uuid::now_v7().simple());
        admin
            .execute(format!("CREATE SCHEMA {schema}").as_str())
            .await
            .expect("create schema");
        let pool = schema_pool(database_url, &schema).await;
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
        let app = test_app(pool.clone(), database_url);
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
        let connection_version = created["connection"]["version"].clone();
        Self {
            admin,
            schema,
            app,
            cookie,
            csrf,
            space,
            connection_id,
            connection_version,
        }
    }

    /// Registers an app and signs it in as Muse, with the owner consenting.
    /// Returns the app's client ID and its first tokens.
    async fn sign_in_as_agent(&self) -> (String, Value) {
        let (app, cookie, csrf) = (&self.app, &self.cookie, &self.csrf);
        let connection_id = &self.connection_id;
        let (status, registration) = send(
            app,
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
                    .header(header::COOKIE, cookie)
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
                    .header(header::COOKIE, cookie)
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
            app,
            token_request(
                serde_urlencoded::to_string([
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id.as_str()),
                    ("code", code.as_str()),
                    ("redirect_uri", REDIRECT),
                    ("code_verifier", verifier.as_str()),
                ])
                .expect("token form"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tokens}");
        (client_id, tokens)
    }

    async fn cleanup(self) {
        agent_workspace_server::test_support::drop_schema(&self.admin, &self.schema)
            .await
            .expect("drop schema");
    }
}

fn refresh_form(refresh_token: &str, client_id: &str) -> String {
    serde_urlencoded::to_string([
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ])
    .expect("refresh form")
}

/// A POST to the token endpoint, from `address` as Cloudflare would report it.
fn token_request(form: String, address: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(address) = address {
        request = request.header("cf-connecting-ip", address);
    }
    request.body(Body::from(form)).expect("token request")
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
