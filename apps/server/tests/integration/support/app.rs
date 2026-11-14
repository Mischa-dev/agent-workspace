//! The app under test and how tests reach it: a browser session, MCP calls
//! as an agent or a connected app, and the plain HTTP helpers. Moved here
//! unchanged from `agent_interface.rs`, which re-exports them.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState, config::AppConfig, mcp::MCP_PATH, provider::SupervisorRegistry,
    rate_limit::RateLimiters, realtime::RealtimeHub, router,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

pub(crate) const PASSWORD: &str = "correct horse battery staple";
const REDIRECT: &str = "http://127.0.0.1:49152/callback";

pub(crate) struct Browser {
    pub(crate) cookie: String,
    pub(crate) csrf: String,
}

impl Browser {
    pub(crate) fn request(
        &self,
        method: &str,
        uri: &str,
        key: Option<&str>,
        body: Value,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", "http://127.0.0.1:3100")
            .header("cookie", &self.cookie)
            .header("x-csrf-token", &self.csrf);
        if let Some(key) = key {
            builder = builder.header("idempotency-key", key);
        }
        if body.is_null() {
            builder.body(Body::empty()).expect("request")
        } else {
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("body")))
                .expect("request")
        }
    }
}

pub(crate) async fn create_agent(
    app: &axum::Router,
    browser: &Browser,
    space: &str,
    name: &str,
    handle: &str,
) -> (Uuid, String) {
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/external-agents"),
            None,
            json!({
                "connectionName": name,
                "displayName": name,
                "handle": handle,
                "role": "member",
                "scopes": ["read", "write", "events"]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let actor = created["connection"]["actorId"]
        .as_str()
        .expect("agent actor")
        .parse::<Uuid>()
        .expect("agent UUID");
    let token = created["oneTimeCredential"]["token"]
        .as_str()
        .expect("agent token")
        .to_owned();
    (actor, token)
}

/// Run the OAuth flow for a local app and return its access token.
pub(crate) async fn connect_app(
    app: &axum::Router,
    browser: &Browser,
    name: &str,
    access: &str,
) -> String {
    let (status, registration) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/oauth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "client_name": name,
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
    let verifier = "v".repeat(43);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let form = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", REDIRECT),
        ("scope", "read write"),
        ("state", "state"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("connect_as", "self"),
        ("access", access),
        ("show_via", "on"),
        ("via_choice", "1"),
        ("csrf_token", browser.csrf.as_str()),
        ("decision", "approve"),
    ])
    .expect("consent");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/confirm")
                .header(header::COOKIE, &browser.cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .expect("consent request"),
        )
        .await
        .expect("consent response");
    assert_eq!(response.status(), StatusCode::OK);
    let page = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .expect("continue page")
            .to_vec(),
    )
    .expect("utf-8");
    let location = page
        .split("url=")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("forwarding target")
        .replace("&amp;", "&");
    let code = url::Url::parse(&location)
        .expect("callback")
        .query_pairs()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.into_owned())
        .expect("code");
    let (status, tokens) = send(
        app,
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
    tokens["access_token"].as_str().expect("access").to_owned()
}

pub(crate) async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub(crate) async fn initialize(app: &axum::Router, token: &str) -> Option<String> {
    let response = mcp(
        app,
        token,
        None,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "interface-test", "version": "1" }
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

pub(crate) async fn tool(
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
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    if status == StatusCode::FORBIDDEN {
        return json!({ "outcome": "forbidden" });
    }
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    let value: Value = serde_json::from_str(json).unwrap_or_else(|_| panic!("MCP JSON: {text}"));
    let content = value
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("no structured content for {name}: {value}"));
    // Clients such as Claude Code reject a result that doesn't match the
    // tool's declared outputSchema, so every call here checks it.
    let schema = output_schema(app, token, session, name).await;
    if let Err(path) = conforms(&schema, &schema, &content, "") {
        panic!("{name} result doesn't match its outputSchema at {path}: {content}");
    }
    content
}

/// A create that takes a `clientRequestId` makes one thing however often
/// it's retried: the same call twice answers the first call's thing again,
/// marked `idempotentReplay`, and another call with the same id is refused.
/// `count` counts the things made. Returns the first answer.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn creates_once(
    app: &axum::Router,
    pool: &PgPool,
    token: &str,
    session: &str,
    name: &str,
    arguments: Value,
    different: Value,
    count: &str,
) -> Value {
    let counted = || async {
        sqlx::query_scalar::<_, i64>(count)
            .fetch_one(pool)
            .await
            .expect("count")
    };
    let before = counted().await;
    let first = tool(app, token, session, name, arguments.clone()).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    assert!(first["value"].get("idempotentReplay").is_none(), "{first}");
    let again = tool(app, token, session, name, arguments).await;
    let mut expected = first.clone();
    expected["value"]["idempotentReplay"] = json!(true);
    assert_eq!(again, expected, "a retry answers the first call");
    assert_eq!(counted().await, before + 1, "{name} made one thing");
    let refused = tool(app, token, session, name, different).await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(counted().await, before + 1, "{name} made nothing more");
    first
}

async fn output_schema(app: &axum::Router, token: &str, session: &str, name: &str) -> Value {
    let response = mcp(
        app,
        token,
        Some(session),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .await;
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    let value: Value = serde_json::from_str(json).expect("tools/list JSON");
    value
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
        .map(|tool| {
            tool.get("outputSchema")
                .cloned()
                .unwrap_or_else(|| panic!("no outputSchema for {name}"))
        })
        // A tool the caller's roles don't let them use isn't in their list
        // (AWD-74); its refusal has nothing to check against.
        .unwrap_or_else(|| json!({}))
}

/// Checks the parts of JSON Schema our generated schemas use: `$ref`,
/// `required`, `properties`, `items`, `const`, `enum`, `type`, and the
/// `oneOf`/`anyOf`/`allOf` combinators. Returns the path of the first mismatch.
fn conforms(root: &Value, schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
            .unwrap_or_else(|| panic!("unresolved $ref {reference}"));
        conforms(root, target, value, path)?;
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(branches) = schema.get(key).and_then(Value::as_array) {
            let results = branches
                .iter()
                .map(|branch| conforms(root, branch, value, path))
                .collect::<Vec<_>>();
            let matches = results.iter().filter(|result| result.is_ok()).count();
            if matches == 0 {
                // The branch that got furthest is the one the value meant.
                return Err(results
                    .into_iter()
                    .filter_map(Result::err)
                    .max_by_key(String::len)
                    .unwrap_or_else(|| format!("{path} (no {key} branches)")));
            }
            if key == "oneOf" && matches > 1 {
                return Err(format!("{path} (oneOf matched {matches})"));
            }
        }
    }
    if let Some(branches) = schema.get("allOf").and_then(Value::as_array) {
        for branch in branches {
            conforms(root, branch, value, path)?;
        }
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return Err(format!("{path} (expected {expected})"));
    }
    // A unit enum's undocumented variants come as one `enum` branch beside
    // its documented ones (AWD-75's LinkKind).
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(format!("{path} (not one of {allowed:?})"));
    }
    if let Some(types) = schema.get("type") {
        let types = types
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![types.clone()]);
        let matches = types.iter().any(|kind| match kind.as_str() {
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            Some("integer") => value.is_i64() || value.is_u64(),
            Some("number") => value.is_number(),
            _ => true,
        });
        if !matches {
            return Err(format!("{path} (not {types:?})"));
        }
    }
    if let Some(object) = value.as_object() {
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                return Err(format!("{path}/{required} (missing)"));
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, property) in properties {
                if let Some(field) = object.get(key) {
                    conforms(root, property, field, &format!("{path}/{key}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            conforms(root, items, item, &format!("{path}/{index}"))?;
        }
    }
    Ok(())
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

pub(crate) async fn login(app: &axum::Router) -> Browser {
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
    Browser {
        cookie: cookies.join("; "),
        csrf,
    }
}

pub(crate) fn test_app(pool: PgPool, database_url: &str) -> axum::Router {
    test_app_with(pool, database_url, &[])
}

/// The app with extra configuration, such as a temporary data root.
pub(crate) fn test_app_with(
    pool: PgPool,
    database_url: &str,
    extra: &[(&str, String)],
) -> axum::Router {
    let mut values = BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url.to_owned(),
    )]);
    for (key, value) in extra {
        values.insert((*key).to_owned(), value.clone());
    }
    let config = AppConfig::from_values(&values).expect("test configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    router(AppState {
        config: Arc::new(config),
        pool,
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    })
}

/// Builds the app (and its state) on a test database, with any extra
/// configuration: the one place a test assembles an `AppState`.
pub(crate) struct AppBuilder {
    pool: PgPool,
    url: String,
    extra: Vec<(String, String)>,
}

pub(crate) fn app(db: &super::db::Db) -> AppBuilder {
    AppBuilder {
        pool: db.pool.clone(),
        url: db.url.clone(),
        extra: Vec::new(),
    }
}

impl AppBuilder {
    /// One more configuration value, such as a data root.
    pub(crate) fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.extra.push((key.to_owned(), value.into()));
        self
    }

    /// The state the router serves, for driving workers (the outbox, jobs).
    pub(crate) fn state(&self) -> AppState {
        let mut values =
            BTreeMap::from([("AGENT_WORKSPACE_DATABASE_URL".to_owned(), self.url.clone())]);
        for (key, value) in &self.extra {
            values.insert(key.clone(), value.clone());
        }
        let config = AppConfig::from_values(&values).expect("test configuration");
        let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
        AppState {
            config: Arc::new(config),
            pool: self.pool.clone(),
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(32),
            supervisors,
        }
    }

    pub(crate) fn build(self) -> (axum::Router, AppState) {
        let state = self.state();
        (router(state.clone()), state)
    }
}
