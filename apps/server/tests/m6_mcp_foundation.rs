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
    let work_item_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_items (
            id, project_id, number, title, description, type_id, status_id,
            priority, created_by_actor_id, board_rank
        )
        SELECT
            $1, $2, 1, 'M6 read contract', 'Bounded MCP context', type.id,
            status.id, 'high', $3, 1024
        FROM work_item_types type
        CROSS JOIN work_item_statuses status
        WHERE type.project_id = $2 AND type.is_default
          AND status.project_id = $2 AND status.is_default
        "#,
    )
    .bind(work_item_id)
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("insert M6 Work Item fixture");
    sqlx::query(
        "INSERT INTO work_item_repository_links (project_id, work_item_id, repository_id, role, created_by_actor_id) VALUES ($1, $2, $3, 'primary', $4)",
    )
    .bind(seed.project_id)
    .bind(work_item_id)
    .bind(seed.repository_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("link M6 Repository fixture");
    let issued = mcp::issue_credential(
        &pool,
        IssueCredentialCommand {
            project_id: seed.project_id,
            actor_id: seed.human_actor_id,
            client_name: "codex-m6-test".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
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
        tools
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(14)
    );
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

    for (id, name, arguments, expected_outcome) in [
        (4, "project_instructions_get", json!({}), "found"),
        (5, "work_item_get", json!({"identifier":"AW-1"}), "found"),
        (
            6,
            "work_item_search",
            json!({"query":"read contract","limit":10}),
            "found",
        ),
        (
            7,
            "work_item_context_get",
            json!({"identifier":work_item_id.to_string()}),
            "found",
        ),
        (
            8,
            "repository_get",
            json!({"repositoryId":seed.repository_id.to_string()}),
            "found",
        ),
        (
            9,
            "pull_request_get",
            json!({"identifier":Uuid::now_v7().to_string()}),
            "not_found",
        ),
    ] {
        let result = protocol_json(
            mcp_request(
                &application,
                Some(&token),
                Some(&session_id),
                &json!({
                    "jsonrpc":"2.0",
                    "id":id,
                    "method":"tools/call",
                    "params":{"name":name,"arguments":arguments}
                }),
            )
            .await,
        )
        .await;
        assert_eq!(
            result.pointer("/result/structuredContent/outcome"),
            Some(&json!(expected_outcome)),
            "unexpected outcome for {name}: {result}"
        );
    }

    let unchanged = call_tool(
        &application,
        &token,
        &session_id,
        20,
        "work_item_set_status",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":1,
            "status":"Backlog",
            "clientRequestId":"m6-status-unchanged-1"
        }),
    )
    .await;
    assert_eq!(unchanged.pointer("/outcome"), Some(&json!("applied")));
    assert_eq!(unchanged.pointer("/version"), Some(&json!(1)));
    assert_eq!(unchanged.pointer("/changedFields"), Some(&json!([])));
    assert!(unchanged.pointer("/resultId").is_some());

    let update_arguments = json!({
        "workItemId":work_item_id.to_string(),
        "expectedVersion":1,
        "title":"M6 write contract",
        "clientRequestId":"m6-update-1"
    });
    let updated = call_tool(
        &application,
        &token,
        &session_id,
        10,
        "work_item_update",
        update_arguments.clone(),
    )
    .await;
    assert_eq!(updated.pointer("/outcome"), Some(&json!("applied")));
    assert_eq!(updated.pointer("/version"), Some(&json!(2)));
    assert_eq!(updated.pointer("/idempotentReplay"), Some(&json!(false)));
    let replay = call_tool(
        &application,
        &token,
        &session_id,
        11,
        "work_item_update",
        update_arguments,
    )
    .await;
    assert_eq!(replay.pointer("/idempotentReplay"), Some(&json!(true)));

    let stale = call_tool(
        &application,
        &token,
        &session_id,
        12,
        "work_item_set_status",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":1,
            "status":"Ready",
            "clientRequestId":"m6-stale-1"
        }),
    )
    .await;
    assert_eq!(stale.pointer("/outcome"), Some(&json!("conflict")));
    assert_eq!(stale.pointer("/currentVersion"), Some(&json!(2)));

    let blocked = call_tool(
        &application,
        &token,
        &session_id,
        13,
        "work_item_mark_blocked",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":2,
            "reason":"Waiting for a bounded dependency",
            "clientRequestId":"m6-blocked-1"
        }),
    )
    .await;
    assert_eq!(blocked.pointer("/version"), Some(&json!(3)));

    let posted = call_tool(
        &application,
        &token,
        &session_id,
        14,
        "work_item_post_update",
        json!({
            "workItemId":work_item_id.to_string(),
            "update":"Useful bounded progress update.",
            "clientRequestId":"m6-post-1"
        }),
    )
    .await;
    assert_eq!(posted.pointer("/version"), Some(&json!(3)));

    let subtask = call_tool(
        &application,
        &token,
        &session_id,
        15,
        "work_item_create_subtask",
        json!({
            "parentWorkItemId":work_item_id.to_string(),
            "title":"Verify MCP audit",
            "clientRequestId":"m6-subtask-1"
        }),
    )
    .await;
    assert_eq!(subtask.pointer("/outcome"), Some(&json!("applied")));

    let ready = call_tool(
        &application,
        &token,
        &session_id,
        16,
        "work_item_set_status",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":3,
            "status":"Ready",
            "clientRequestId":"m6-ready-1"
        }),
    )
    .await;
    assert_eq!(ready.pointer("/version"), Some(&json!(4)));
    let started = call_tool(
        &application,
        &token,
        &session_id,
        17,
        "work_item_set_status",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":4,
            "status":"In Progress",
            "clientRequestId":"m6-started-1"
        }),
    )
    .await;
    assert_eq!(started.pointer("/version"), Some(&json!(5)));
    let review = call_tool(
        &application,
        &token,
        &session_id,
        18,
        "work_item_mark_ready_for_review",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":5,
            "clientRequestId":"m6-review-1"
        }),
    )
    .await;
    assert_eq!(review.pointer("/version"), Some(&json!(6)));

    let missing_pr = call_tool(
        &application,
        &token,
        &session_id,
        19,
        "work_item_link_pull_request",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":6,
            "pullRequestId":Uuid::now_v7().to_string(),
            "clientRequestId":"m6-pr-1"
        }),
    )
    .await;
    assert_eq!(missing_pr.pointer("/outcome"), Some(&json!("not_found")));

    let mcp_audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE authentication_class = 'mcp_bearer' AND channel = 'mcp' AND mcp_credential_id = $1",
    )
    .bind(issued.credential_id)
    .fetch_one(&pool)
    .await
    .expect("MCP audit count");
    assert_eq!(
        mcp_audits, 8,
        "exact replay and conflicts create no success audit"
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

async fn call_tool(
    application: &axum::Router,
    token: &str,
    session_id: &str,
    id: i64,
    name: &str,
    arguments: Value,
) -> Value {
    let response = protocol_json(
        mcp_request(
            application,
            Some(token),
            Some(session_id),
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }),
        )
        .await,
    )
    .await;
    response
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("missing structured result for {name}: {response}"))
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
