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
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";

/// The rules for who answers: a DM asks for a required reply that can only be
/// answered once; group messages ask everyone optionally; agent back-and-forth
/// stops when the turn allowance runs out until a person says Continue.
#[tokio::test]
async fn agents_are_asked_claim_and_answer_exactly_once() {
    let Some(Fixture {
        admin,
        schema,
        pool,
        app,
        owner,
        space,
    }) = fixture("replies").await
    else {
        return;
    };
    let mara = owner.add_agent(&app, &space, "Mara", "mara").await;
    let rook = owner.add_agent(&app, &space, "Rook", "rook").await;
    let mara_mcp = mara.connect(&app).await;
    let rook_mcp = rook.connect(&app).await;

    // DM: a required request, answered exactly once.
    let (_, dm) = owner
        .call(
            &app,
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": mara.actor_id }),
        )
        .await;
    let dm_id = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    owner
        .say(&app, &dm_id, "dm-1", "Mara, are you there?")
        .await;
    owner
        .say(&app, &dm_id, "dm-2", "Also: what's the plan today?")
        .await;
    let inbox = mara_mcp.tool(&app, "inbox_list", json!({})).await;
    let requests = inbox["value"]["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 1, "the second message supersedes the first");
    assert_eq!(requests[0]["required"], true);
    let request_id = requests[0]["requestId"].as_str().expect("id").to_owned();

    let passed = mara_mcp
        .tool(&app, "reply_pass", json!({ "requestId": request_id }))
        .await;
    assert_eq!(passed["outcome"], "conflict", "a DM can't be passed");

    let claim = mara_mcp
        .tool(&app, "reply_claim", json!({ "requestId": request_id }))
        .await;
    assert_eq!(claim["outcome"], "ok");
    let generation = claim["value"]["request"]["generation"]
        .as_i64()
        .expect("generation");
    assert_eq!(
        claim["value"]["recentMessages"]
            .as_array()
            .expect("context")
            .len(),
        2
    );
    let renewed = mara_mcp
        .tool(&app, "reply_claim", json!({ "requestId": request_id }))
        .await;
    assert_eq!(
        renewed["value"]["request"]["generation"].as_i64(),
        Some(generation)
    );

    let first = mara_mcp
        .tool(
            &app,
            "reply_send",
            json!({ "requestId": request_id, "generation": generation, "text": "Here. Plan: ship DMs." }),
        )
        .await;
    assert_eq!(first["outcome"], "ok");
    let second = mara_mcp
        .tool(
            &app,
            "reply_send",
            json!({ "requestId": request_id, "generation": generation, "text": "A different answer", "clientRequestId": "another-key" }),
        )
        .await;
    assert_eq!(second["outcome"], "conflict", "only one reply per request");
    assert_eq!(agent_messages(&pool, &dm_id).await, 1);

    // Group: everyone is asked; passes post nothing; replies ask the others.
    let (_, group) = owner
        .call(
            &app,
            "POST",
            "/api/v1/direct/groups",
            Some("group-1"),
            json!({ "memberIds": [mara.actor_id, rook.actor_id] }),
        )
        .await;
    let group_id = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    owner
        .say(&app, &group_id, "group-1-msg", "Thoughts, anyone?")
        .await;
    let rook_request = only_request(&rook_mcp, &app).await;
    assert_eq!(rook_request["required"], false);
    let passed = rook_mcp
        .tool(
            &app,
            "reply_pass",
            json!({ "requestId": rook_request["requestId"] }),
        )
        .await;
    assert_eq!(passed["outcome"], "ok");
    answer(&mara_mcp, &app, "I think we should start with the inbox.").await;
    let rook_request = only_request(&rook_mcp, &app).await;
    assert_eq!(
        rook_request["messageAuthor"], "Mara",
        "Mara's reply asks Rook"
    );

    // With one turn left, Rook's reply uses it up and Mara is not asked.
    sqlx::query("UPDATE conversations SET agent_turns_remaining = 1 WHERE id = $1::uuid")
        .bind(&group_id)
        .execute(&pool)
        .await
        .expect("shrink allowance");
    answer(&rook_mcp, &app, "Agreed, inbox first.").await;
    assert_eq!(inbox_len(&mara_mcp, &app).await, 0, "allowance used up");
    let (status, timeline) = owner
        .call(
            &app,
            "GET",
            &format!("/api/v1/conversations/{group_id}/timeline"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(timeline["agentTurnsRemaining"], 0);

    let (status, _) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/conversations/{group_id}/continue"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let mara_request = only_request(&mara_mcp, &app).await;
    assert_eq!(
        mara_request["messageAuthor"], "Rook",
        "Continue asks Mara about Rook's reply"
    );

    // A second Continue (a double click, or a retry) keeps that request:
    // asking about the same message never supersedes its own request.
    let (status, _) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/conversations/{group_id}/continue"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let still = only_request(&mara_mcp, &app).await;
    assert_eq!(still["requestId"], mara_request["requestId"], "{still}");
    assert_eq!(still["status"], "pending", "{still}");

    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");
}

/// An agent taken out of a group can't see it any more, so every open
/// request it had there (pending, claimed, expired or released, in the group
/// and its threads) is withdrawn and none can be answered or retried. A
/// request left open for an agent who can't see the conversation (as an older
/// server left them) can't be retried either: it would sit pending forever.
#[tokio::test]
async fn requests_of_agents_removed_from_a_group_are_withdrawn() {
    let Some(Fixture {
        admin,
        schema,
        pool,
        app,
        owner,
        space,
    }) = fixture("withdrawn").await
    else {
        return;
    };
    let mara = owner.add_agent(&app, &space, "Mara", "mara").await;
    let rook = owner.add_agent(&app, &space, "Rook", "rook").await;
    let mara_mcp = mara.connect(&app).await;
    let rook_mcp = rook.connect(&app).await;
    let (_, group) = owner
        .call(
            &app,
            "POST",
            "/api/v1/direct/groups",
            Some("withdraw-group"),
            json!({ "memberIds": [mara.actor_id, rook.actor_id] }),
        )
        .await;
    let group_id = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();

    // Rook releases his first request, a person retries it (Rook can still
    // see the group, so that works), and Rook releases it again.
    owner
        .say(&app, &group_id, "withdraw-1", "First question")
        .await;
    let rook_first = only_request(&rook_mcp, &app).await["requestId"].clone();
    let released = rook_mcp
        .tool(
            &app,
            "reply_release",
            json!({ "requestId": rook_first, "reason": "Busy" }),
        )
        .await;
    assert_eq!(released["outcome"], "ok", "{released}");
    let rook_first = rook_first.as_str().expect("request").to_owned();
    let (status, retried) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/reply-requests/{rook_first}/retry"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    let released = rook_mcp
        .tool(
            &app,
            "reply_release",
            json!({ "requestId": rook_first, "reason": "Still busy" }),
        )
        .await;
    assert_eq!(released["outcome"], "ok", "{released}");

    // Mara claims hers.
    let mara_first = only_request(&mara_mcp, &app).await["requestId"].clone();
    let claim = mara_mcp
        .tool(&app, "reply_claim", json!({ "requestId": mara_first }))
        .await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let generation = claim["value"]["request"]["generation"].clone();
    let mara_first = mara_first.as_str().expect("request").to_owned();

    // A second message asks both again; Rook's lease lapses on that one.
    owner
        .say(&app, &group_id, "withdraw-2", "Second question")
        .await;
    let rook_second = request_id(&pool, &group_id, &rook.actor_id, "pending").await;
    sqlx::query(
        "UPDATE reply_requests SET status = 'expired', reason = 'Lapsed.' WHERE id = $1::uuid",
    )
    .bind(&rook_second)
    .execute(&pool)
    .await
    .expect("lapse a claim");
    let mara_second = request_id(&pool, &group_id, &mara.actor_id, "pending").await;

    // And a thread under the group asks both too.
    let (status, thread) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/conversations/{group_id}/threads"),
            Some("withdraw-thread"),
            json!({ "name": "Side topic" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{thread}");
    let thread_id = thread["conversation"]["id"]
        .as_str()
        .expect("thread")
        .to_owned();
    owner
        .say(&app, &thread_id, "withdraw-3", "A side question")
        .await;
    let mara_thread = request_id(&pool, &thread_id, &mara.actor_id, "pending").await;
    let rook_thread = request_id(&pool, &thread_id, &rook.actor_id, "pending").await;
    assert_eq!(
        statuses(&pool, &[&mara_first, &mara_second, &mara_thread]).await,
        ["claimed", "pending", "pending"]
    );
    assert_eq!(
        statuses(&pool, &[&rook_first, &rook_second, &rook_thread]).await,
        ["released", "expired", "pending"]
    );

    // Taking Mara out withdraws hers, in the group and the thread, and
    // nobody else's.
    let (status, members) = owner
        .call(
            &app,
            "POST",
            &format!(
                "/api/v1/conversations/{group_id}/members/{}/remove",
                mara.actor_id
            ),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    assert_eq!(
        statuses(&pool, &[&mara_first, &mara_second, &mara_thread]).await,
        ["superseded", "superseded", "superseded"]
    );
    assert_eq!(
        reasons(&pool, &[&mara_first, &mara_second, &mara_thread]).await,
        ["Removed from the group."; 3]
    );
    assert_eq!(
        statuses(&pool, &[&rook_first, &rook_second, &rook_thread]).await,
        ["released", "expired", "pending"]
    );
    let sent = mara_mcp
        .tool(
            &app,
            "reply_send",
            json!({ "requestId": mara_first, "generation": generation, "text": "Late answer" }),
        )
        .await;
    assert_ne!(sent["outcome"], "ok", "{sent}");
    assert_eq!(agent_messages(&pool, &group_id).await, 0);
    assert_eq!(inbox_len(&mara_mcp, &app).await, 0);
    let (status, timeline) = owner
        .call(
            &app,
            "GET",
            &format!("/api/v1/conversations/{group_id}/timeline"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    let listed = timeline["replyRequests"]
        .as_array()
        .expect("reply requests")
        .iter()
        .map(|request| {
            request["agent"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        [rook.actor_id.clone(), rook.actor_id.clone()],
        "{timeline}"
    );

    // A request an older server left open for Mara can't be put back in an
    // inbox she can no longer see.
    sqlx::query(
        "UPDATE reply_requests SET status = 'released', reason = 'Busy' WHERE id = $1::uuid",
    )
    .bind(&mara_second)
    .execute(&pool)
    .await
    .expect("simulate a request left open");
    let (status, refused) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/reply-requests/{mara_second}/retry"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(statuses(&pool, &[&mara_second]).await, ["released"]);

    // Taking Rook out withdraws his released, expired and pending ones, and
    // a withdrawn request can't be retried.
    let (status, members) = owner
        .call(
            &app,
            "POST",
            &format!(
                "/api/v1/conversations/{group_id}/members/{}/remove",
                rook.actor_id
            ),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    assert_eq!(
        statuses(&pool, &[&rook_first, &rook_second, &rook_thread]).await,
        ["superseded", "superseded", "superseded"]
    );
    let (status, refused) = owner
        .call(
            &app,
            "POST",
            &format!("/api/v1/reply-requests/{rook_second}/retry"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");
}

async fn request_id(pool: &PgPool, conversation_id: &str, agent: &str, status: &str) -> String {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT id::text FROM reply_requests
        WHERE conversation_id = $1::uuid AND agent_actor_id = $2::uuid AND status = $3
        "#,
    )
    .bind(conversation_id)
    .bind(agent)
    .bind(status)
    .fetch_one(pool)
    .await
    .expect("exactly one request")
}

async fn statuses(pool: &PgPool, requests: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for request in requests {
        found.push(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM reply_requests WHERE id = $1::uuid",
            )
            .bind(request)
            .fetch_one(pool)
            .await
            .expect("request status"),
        );
    }
    found
}

async fn reasons(pool: &PgPool, requests: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for request in requests {
        found.push(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT reason FROM reply_requests WHERE id = $1::uuid",
            )
            .bind(request)
            .fetch_one(pool)
            .await
            .expect("request reason")
            .unwrap_or_default(),
        );
    }
    found
}

async fn answer(agent: &AgentSession, app: &axum::Router, text: &str) {
    let request = only_request(agent, app).await;
    let claim = agent
        .tool(
            app,
            "reply_claim",
            json!({ "requestId": request["requestId"] }),
        )
        .await;
    let generation = claim["value"]["request"]["generation"].clone();
    let sent = agent
        .tool(
            app,
            "reply_send",
            json!({ "requestId": request["requestId"], "generation": generation, "text": text }),
        )
        .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
}

async fn only_request(agent: &AgentSession, app: &axum::Router) -> Value {
    let inbox = agent.tool(app, "inbox_list", json!({})).await;
    let requests = inbox["value"]["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 1, "{inbox}");
    requests[0].clone()
}

async fn inbox_len(agent: &AgentSession, app: &axum::Router) -> usize {
    agent.tool(app, "inbox_list", json!({})).await["value"]["requests"]
        .as_array()
        .expect("requests")
        .len()
}

async fn agent_messages(pool: &PgPool, conversation_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM messages message
        JOIN actors actor ON actor.id = message.author_actor_id
        WHERE message.conversation_id = $1::uuid AND actor.kind = 'external_agent'
        "#,
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await
    .expect("count agent messages")
}

struct Fixture {
    admin: PgPool,
    schema: String,
    pool: PgPool,
    app: axum::Router,
    owner: Owner,
    /// The organization's direct-messages space, where groups live.
    space: String,
}

/// A fresh schema with the owner logged in, or None without PostgreSQL.
async fn fixture(prefix: &str) -> Option<Fixture> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
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
            organization_name: Some("Reply Organization".to_owned()),
            organization_slug: Some(format!("replies-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let owner = Owner::login(&app).await;
    let (_, overview) = owner
        .call(&app, "GET", "/api/v1/direct", None, Value::Null)
        .await;
    let space = overview["projectId"].as_str().expect("space").to_owned();
    Some(Fixture {
        admin,
        schema,
        pool,
        app,
        owner,
        space,
    })
}

struct Owner {
    cookie: String,
    csrf: String,
}

struct Agent {
    actor_id: String,
    token: String,
}

struct AgentSession {
    token: String,
    session_id: String,
}

impl Owner {
    async fn login(app: &axum::Router) -> Self {
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
        Self {
            cookie: cookies.join("; "),
            csrf,
        }
    }

    async fn call(
        &self,
        app: &axum::Router,
        method: &str,
        uri: &str,
        idempotency_key: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", "http://127.0.0.1:3100")
            .header("cookie", &self.cookie)
            .header("x-csrf-token", &self.csrf);
        if let Some(key) = idempotency_key {
            builder = builder.header("idempotency-key", key);
        }
        let body = if body.is_null() {
            Body::empty()
        } else {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).expect("body"))
        };
        let response = app
            .clone()
            .oneshot(builder.body(body).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn say(&self, app: &axum::Router, conversation_id: &str, key: &str, text: &str) {
        let (status, body) = self
            .call(
                app,
                "POST",
                &format!("/api/v1/conversations/{conversation_id}/messages"),
                Some(key),
                json!({ "content": { "version": 1, "nodes": [{ "type": "text", "text": text }] } }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    async fn add_agent(&self, app: &axum::Router, space: &str, name: &str, handle: &str) -> Agent {
        let (status, created) = self
            .call(
                app,
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
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        Agent {
            actor_id: created["connection"]["actorId"]
                .as_str()
                .expect("actor")
                .to_owned(),
            token: created["oneTimeCredential"]["token"]
                .as_str()
                .expect("token")
                .to_owned(),
        }
    }
}

impl Agent {
    async fn connect(&self, app: &axum::Router) -> AgentSession {
        let response = mcp(
            app,
            &self.token,
            None,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test-agent", "version": "1" }
                }
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("session")
            .to_owned();
        let initialized = mcp(
            app,
            &self.token,
            Some(&session_id),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        assert_eq!(initialized.status(), StatusCode::ACCEPTED);
        AgentSession {
            token: self.token.clone(),
            session_id,
        }
    }
}

impl AgentSession {
    async fn tool(&self, app: &axum::Router, name: &str, arguments: Value) -> Value {
        let response = mcp(
            app,
            &self.token,
            Some(&self.session_id),
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
        )
        .await;
        let bytes = to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body");
        let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
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
}

async fn mcp(
    app: &axum::Router,
    token: &str,
    session_id: Option<&str>,
    payload: Value,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session_id) = session_id {
        request = request
            .header("mcp-session-id", session_id)
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
