use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    mcp::{self, IssueCredentialCommand, MCP_PATH},
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

#[tokio::test]
async fn bearer_lifecycle_and_streamable_http_are_project_scoped() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m6_mcp_foundation_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create isolated M6 schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run M6 migrations");

    let temporary = tempfile::tempdir().expect("temporary M6 root");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            temporary.path().join("data").to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            temporary
                .path()
                .join("data/codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            "/usr/bin/false".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().join("web").to_string_lossy().into_owned(),
        ),
    ]))
    .expect("M6 configuration");
    let seed = dev_seed(&pool, &config).await.expect("seed M6 fixture");
    let issued = mcp::issue_credential(
        &pool,
        IssueCredentialCommand {
            project_id: seed.project_id,
            actor_id: seed.human_actor_id,
            client_name: "codex-m6-test".to_owned(),
            scopes: vec!["read".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("issue test credential");
    let token = issued.token;

    let stored_digest = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT token_digest FROM mcp_client_credentials WHERE id = $1",
    )
    .bind(issued.credential_id)
    .fetch_one(&pool)
    .await
    .expect("stored token digest");
    assert_eq!(stored_digest.len(), 32);
    assert!(!String::from_utf8_lossy(&stored_digest).contains(&token));
    let safe_list = serde_json::to_string(
        &mcp::list_credentials(&pool, seed.project_id)
            .await
            .expect("list safe credential metadata"),
    )
    .expect("serialize credential metadata");
    assert!(!safe_list.contains(&token));

    let supervisors = SupervisorRegistry::new(&config).expect("M6 supervisor registry");
    let application = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    });
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "codex", "version": "0.146.0"}
        }
    });

    let unauthorized = mcp_request(&application, None, None, &initialize).await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert!(
        unauthorized
            .headers()
            .contains_key(header::WWW_AUTHENTICATE)
    );

    let initialized = mcp_request(&application, Some(&token), None, &initialize).await;
    assert_eq!(initialized.status(), StatusCode::OK);
    let session_id = initialized
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("MCP session id")
        .to_owned();
    let initialize_body = protocol_json(initialized).await;
    assert_eq!(
        initialize_body.pointer("/result/protocolVersion"),
        Some(&json!("2025-06-18"))
    );
    let instructions = initialize_body
        .pointer("/result/instructions")
        .and_then(Value::as_str)
        .expect("server instructions");
    assert!(instructions[..instructions.len().min(512)].contains("expectedVersion"));

    let notification = mcp_request(
        &application,
        Some(&token),
        Some(&session_id),
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(notification.status(), StatusCode::ACCEPTED);

    let tools = protocol_json(
        mcp_request(
            &application,
            Some(&token),
            Some(&session_id),
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )
        .await,
    )
    .await;
    let project_tool = tools
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "project_get"))
        .expect("project_get tool");
    assert_eq!(
        project_tool.pointer("/annotations/readOnlyHint"),
        Some(&json!(true))
    );
    assert_eq!(
        project_tool.pointer("/annotations/openWorldHint"),
        Some(&json!(false))
    );

    let project = protocol_json(
        mcp_request(
            &application,
            Some(&token),
            Some(&session_id),
            &json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{
                    "name":"project_get",
                    "arguments":{"projectId":seed.project_id.to_string()}
                }
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        project.pointer("/result/structuredContent/outcome"),
        Some(&json!("found"))
    );
    assert_eq!(
        project.pointer("/result/structuredContent/project/id"),
        Some(&json!(seed.project_id.to_string()))
    );

    let before_conversations = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversations")
        .fetch_one(&pool)
        .await
        .expect("conversation count before revocation");
    mcp::revoke_credential(&pool, issued.credential_id, 1)
        .await
        .expect("revoke credential");
    let revoked = mcp_request(
        &application,
        Some(&token),
        Some(&session_id),
        &json!({"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    let after_conversations = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversations")
        .fetch_one(&pool)
        .await
        .expect("conversation count after MCP calls");
    assert_eq!(before_conversations, after_conversations);

    drop(application);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&administrative_pool)
        .await
        .expect("drop isolated M6 schema");
}

async fn mcp_request(
    application: &axum::Router,
    token: Option<&str>,
    session_id: Option<&str>,
    payload: &Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header(header::HOST, "127.0.0.1")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(session_id) = session_id {
        builder = builder
            .header("mcp-session-id", session_id)
            .header("mcp-protocol-version", "2025-06-18");
    }
    application
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    serde_json::to_vec(payload).expect("serialize MCP request"),
                ))
                .expect("build MCP request"),
        )
        .await
        .expect("MCP response")
}

async fn protocol_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read MCP response");
    let text = String::from_utf8(body.to_vec()).expect("MCP response UTF-8");
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&text);
    serde_json::from_str(json).expect("MCP response JSON")
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
        .expect("connect schema-scoped M6 pool")
}
