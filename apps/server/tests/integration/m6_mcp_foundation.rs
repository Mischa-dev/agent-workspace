use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
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
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const OAUTH_LOGIN: &str = "m6-oauth-owner";
const OAUTH_PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn bearer_lifecycle_and_streamable_http_are_project_scoped() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace_server=debug")
        .try_init();
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
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OAUTH_LOGIN.to_owned(),
            display_name: "M6 OAuth Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        OAUTH_PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap OAuth resource owner");
    let work_item_number = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE project_counters
        SET next_work_item_number = next_work_item_number + 1,
            updated_at = now()
        WHERE project_id = $1
        RETURNING next_work_item_number - 1
        "#,
    )
    .bind(seed.project_id)
    .fetch_one(&pool)
    .await
    .expect("allocate M6 Work Item number");
    let work_item_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_items (
            id, project_id, number, title, description, type_id, status_id,
            priority, created_by_actor_id, board_rank
        )
        SELECT
            $1, $2, $4, 'M6 read contract', 'Bounded MCP context', type.id,
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
    .bind(work_item_number)
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
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(16),
        supervisors,
    };
    let application = router(state.clone());
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
    assert_eq!(
        unauthorized
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some(
            "Bearer resource_metadata=\"http://127.0.0.1:3100/.well-known/oauth-protected-resource/api/v1/mcp\""
        )
    );

    let mut public_config = (*state.config).clone();
    public_config.public_origin = Some("https://workspace.example.test".to_owned());
    let public_application = router(AppState {
        config: Arc::new(public_config),
        ..state.clone()
    });
    let public_unauthorized = mcp_request(&public_application, None, None, &initialize).await;
    assert_eq!(public_unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        public_unauthorized
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some(
            "Bearer resource_metadata=\"https://workspace.example.test/.well-known/oauth-protected-resource/api/v1/mcp\""
        )
    );
    let public_metadata = json_response(
        public_application
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/api/v1/mcp")
                    .body(Body::empty())
                    .expect("public metadata request"),
            )
            .await
            .expect("public metadata response"),
    )
    .await;
    assert_eq!(
        public_metadata["resource"],
        "https://workspace.example.test/api/v1/mcp"
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
    assert!(instructions.starts_with("Agent Workspace"));
    for required in [
        "Start with briefing",
        "expectedVersion",
        "current membership",
    ] {
        assert!(
            instructions.contains(required),
            "instructions mention {required}"
        );
    }

    let notification = mcp_request(
        &application,
        Some(&token),
        Some(&session_id),
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(notification.status(), StatusCode::ACCEPTED);

    // A session the server dropped (idle, or from before a restart) is picked
    // up again instead of answered with 404, which some clients take as the
    // server going away for good.
    let resumed = protocol_json(
        mcp_request(
            &application,
            Some(&token),
            Some("from-before-a-restart"),
            &json!({"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}}),
        )
        .await,
    )
    .await;
    assert!(
        resumed
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
        "resumed session lists tools: {resumed}"
    );
    let not_an_id = mcp_request(
        &application,
        Some(&token),
        Some("not a session id"),
        &json!({"jsonrpc":"2.0","id":100,"method":"tools/list","params":{}}),
    )
    .await;
    assert_eq!(not_an_id.status(), StatusCode::NOT_FOUND);
    // A session the client ended on purpose stays ended, whether the server
    // started it or resumed it.
    let ended = mcp_request(&application, Some(&token), None, &initialize).await;
    let ended_session = ended
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("MCP session to end")
        .to_owned();
    for session in [ended_session.as_str(), "from-before-a-restart"] {
        let deleted = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(MCP_PATH)
                    .header(header::HOST, "127.0.0.1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("mcp-session-id", session)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::empty())
                    .expect("MCP delete request"),
            )
            .await
            .expect("MCP delete response");
        assert!(
            deleted.status().is_success(),
            "DELETE ends {session}: {}",
            deleted.status()
        );
        let after_delete = mcp_request(
            &application,
            Some(&token),
            Some(session),
            &json!({"jsonrpc":"2.0","id":101,"method":"tools/list","params":{}}),
        )
        .await;
        assert_eq!(
            after_delete.status(),
            StatusCode::NOT_FOUND,
            "a deleted session {session} is not brought back"
        );
    }

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
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "project_open"))
        .expect("project_open tool");
    // No exact tool count, so branches that each add a tool merge in either
    // order. Every tool must instead describe itself and carry valid schemas.
    // See docs/plans/mcp-tools.md for the tool list.
    assert_tools_are_well_formed(
        tools
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tool list"),
    );
    assert_eq!(
        project_tool.pointer("/annotations/readOnlyHint"),
        Some(&json!(true))
    );
    assert_eq!(
        project_tool.pointer("/annotations/openWorldHint"),
        Some(&json!(false))
    );
    // Clients on MCP 2026-07-28 (Claude Code) reject lists without cache hints.
    let prompts = protocol_json(
        mcp_request(
            &application,
            Some(&token),
            Some(&session_id),
            &json!({"jsonrpc":"2.0","id":3,"method":"prompts/list","params":{}}),
        )
        .await,
    )
    .await;
    for list in [&tools, &prompts] {
        assert!(
            list.pointer("/result/ttlMs").is_some_and(Value::is_u64),
            "{list}"
        );
        assert_eq!(list.pointer("/result/cacheScope"), Some(&json!("public")));
    }
    assert_eq!(
        prompts
            .pointer("/result/prompts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(5)
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
                    "name":"project_open",
                    "arguments":{"projectId":seed.project_id.to_string()}
                }
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        project.pointer("/result/structuredContent/outcome"),
        Some(&json!("ok"))
    );
    assert_eq!(
        project.pointer("/result/structuredContent/value/project/id"),
        Some(&json!(seed.project_id.to_string()))
    );

    for (id, name, arguments, expected_outcome) in [
        (4, "briefing", json!({}), "ok"),
        (5, "task_open", json!({"taskId":"AW-1"}), "ok"),
        (
            6,
            "task_list",
            json!({"query":"read contract","limit":10}),
            "ok",
        ),
        (
            7,
            "task_open",
            json!({"taskId":work_item_id.to_string()}),
            "ok",
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
        "task_update",
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
        "task_update",
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
        "task_update",
        update_arguments,
    )
    .await;
    assert_eq!(replay.pointer("/idempotentReplay"), Some(&json!(true)));

    let stale = call_tool(
        &application,
        &token,
        &session_id,
        12,
        "task_update",
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
    assert_eq!(
        stale.pointer("/current/id"),
        Some(&json!(work_item_id.to_string()))
    );
    assert_eq!(stale.pointer("/current/version"), Some(&json!(2)));
    assert_eq!(
        stale.pointer("/current/title"),
        Some(&json!("M6 write contract"))
    );

    let blocked = call_tool(
        &application,
        &token,
        &session_id,
        13,
        "task_mark_blocked",
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
        "task_comment",
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
        "task_create",
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
        "task_update",
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
        "task_update",
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
        "task_mark_ready_for_review",
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
        "task_link_pull_request",
        json!({
            "workItemId":work_item_id.to_string(),
            "expectedVersion":6,
            "pullRequestId":Uuid::now_v7().to_string(),
            "clientRequestId":"m6-pr-1"
        }),
    )
    .await;
    assert_eq!(missing_pr.pointer("/outcome"), Some(&json!("not_found")));

    // M10 characterization: equivalent browser and MCP creates converge on
    // canonical row state and exactly one activity, audit, and outbox event.
    let parity_cookie = login(&application).await;
    let parity_csrf = csrf_from_cookie(&parity_cookie);
    let parity_effects_before = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects before M10 create parity");
    let browser_create_response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{}/work-items", seed.project_id))
                .header(header::COOKIE, &parity_cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header("x-csrf-token", &parity_csrf)
                .header("idempotency-key", "m10-browser-create-parity")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title":"M10 parity item",
                        "description":"Shared command state",
                        "priority":"low"
                    }))
                    .expect("browser parity create JSON"),
                ))
                .expect("browser parity create request"),
        )
        .await
        .expect("browser parity create response");
    assert_eq!(browser_create_response.status(), StatusCode::CREATED);
    let browser_created = json_response(browser_create_response).await;
    let browser_work_item_id = Uuid::parse_str(
        browser_created["id"]
            .as_str()
            .expect("browser parity Work Item id"),
    )
    .expect("browser parity Work Item UUID");
    let after_browser = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects after browser parity create");
    assert_eq!(
        after_browser,
        (
            parity_effects_before.0 + 1,
            parity_effects_before.1 + 1,
            parity_effects_before.2 + 1
        )
    );

    let mcp_created = call_tool(
        &application,
        &token,
        &session_id,
        21,
        "task_create",
        json!({
            "title":"M10 parity item",
            "description":"Shared command state",
            "priority":"low",
            "clientRequestId":"m10-mcp-create-parity"
        }),
    )
    .await;
    assert_eq!(mcp_created["outcome"], "applied");
    let mcp_work_item_id = Uuid::parse_str(
        mcp_created["workItemId"]
            .as_str()
            .expect("MCP parity Work Item id"),
    )
    .expect("MCP parity Work Item UUID");
    let after_mcp = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects after MCP parity create");
    assert_eq!(
        after_mcp,
        (
            after_browser.0 + 1,
            after_browser.1 + 1,
            after_browser.2 + 1
        )
    );
    let browser_state = sqlx::query_as::<_, (String, String, String, i64, Uuid, Uuid)>(
        "SELECT title, description, priority, version, type_id, status_id FROM work_items WHERE id = $1",
    )
    .bind(browser_work_item_id)
    .fetch_one(&pool)
    .await
    .expect("browser parity row");
    let mcp_state = sqlx::query_as::<_, (String, String, String, i64, Uuid, Uuid)>(
        "SELECT title, description, priority, version, type_id, status_id FROM work_items WHERE id = $1",
    )
    .bind(mcp_work_item_id)
    .fetch_one(&pool)
    .await
    .expect("MCP parity row");
    assert_eq!(browser_state, mcp_state);

    let browser_update_response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/projects/{}/work-items/{browser_work_item_id}",
                    seed.project_id
                ))
                .header(header::COOKIE, &parity_cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header("x-csrf-token", &parity_csrf)
                .header("idempotency-key", "m10-browser-update-parity")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "expectedVersion":1,
                        "title":"M10 parity updated",
                        "description":"Shared update state",
                        "priority":"urgent"
                    }))
                    .expect("browser parity update JSON"),
                ))
                .expect("browser parity update request"),
        )
        .await
        .expect("browser parity update response");
    assert_eq!(browser_update_response.status(), StatusCode::OK);
    assert_eq!(
        json_response(browser_update_response).await["version"],
        json!(2)
    );
    let after_browser_update = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects after browser parity update");
    assert_eq!(
        after_browser_update,
        (after_mcp.0 + 1, after_mcp.1 + 1, after_mcp.2 + 1)
    );
    let mcp_updated = call_tool(
        &application,
        &token,
        &session_id,
        22,
        "task_update",
        json!({
            "workItemId":mcp_work_item_id.to_string(),
            "expectedVersion":1,
            "title":"M10 parity updated",
            "description":"Shared update state",
            "priority":"urgent",
            "clientRequestId":"m10-mcp-update-parity"
        }),
    )
    .await;
    assert_eq!(mcp_updated["outcome"], "applied");
    assert_eq!(mcp_updated["version"], 2);
    let after_mcp_update = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM audit_events), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects after MCP parity update");
    assert_eq!(
        after_mcp_update,
        (
            after_browser_update.0 + 1,
            after_browser_update.1 + 1,
            after_browser_update.2 + 1
        )
    );
    let browser_updated_state =
        sqlx::query_as::<_, (String, String, String, i64, Uuid, Uuid)>(
            "SELECT title, description, priority, version, type_id, status_id FROM work_items WHERE id = $1",
        )
        .bind(browser_work_item_id)
        .fetch_one(&pool)
        .await
        .expect("browser parity updated row");
    let mcp_updated_state =
        sqlx::query_as::<_, (String, String, String, i64, Uuid, Uuid)>(
            "SELECT title, description, priority, version, type_id, status_id FROM work_items WHERE id = $1",
        )
        .bind(mcp_work_item_id)
        .fetch_one(&pool)
        .await
        .expect("MCP parity updated row");
    assert_eq!(browser_updated_state, mcp_updated_state);

    let mcp_audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE authentication_class = 'mcp_bearer' AND channel = 'mcp' AND mcp_credential_id = $1",
    )
    .bind(issued.credential_id)
    .fetch_one(&pool)
    .await
    .expect("MCP audit count");
    assert_eq!(
        mcp_audits, 10,
        "exact replay and conflicts create no success audit"
    );
    let viewer_denial_effects_before = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM outbox_events)",
    )
    .fetch_one(&pool)
    .await
    .expect("count effects before Viewer denial");

    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("downgrade MCP Actor to Viewer");
    let viewer_project = protocol_json(
        mcp_request(
            &application,
            Some(&token),
            Some(&session_id),
            &json!({
                "jsonrpc":"2.0",
                "id":30,
                "method":"tools/call",
                "params":{
                    "name":"project_open",
                    "arguments":{"projectId":seed.project_id.to_string()}
                }
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        viewer_project.pointer("/result/structuredContent/outcome"),
        Some(&json!("ok")),
        "a Viewer retains MCP read authority after an immediate role downgrade"
    );
    let denied_viewer_write = call_tool(
        &application,
        &token,
        &session_id,
        31,
        "task_comment",
        json!({
            "workItemId":work_item_id.to_string(),
            "update":"This Viewer write must never be persisted.",
            "clientRequestId":"m7-viewer-denied-1"
        }),
    )
    .await;
    assert_eq!(
        denied_viewer_write.pointer("/outcome"),
        Some(&json!("permission_denied"))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE request_id = 'm7-viewer-denied-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("count denied Viewer audit rows"),
        0,
        "denied Viewer writes produce no success audit"
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT (SELECT count(*) FROM work_item_activity), (SELECT count(*) FROM outbox_events)",
        )
        .fetch_one(&pool)
        .await
        .expect("count effects after Viewer denial"),
        viewer_denial_effects_before,
        "denied Viewer writes create no domain activity or outbox event"
    );
    let viewer_write_credential = mcp::issue_credential(
        &pool,
        IssueCredentialCommand {
            project_id: seed.project_id,
            actor_id: seed.human_actor_id,
            client_name: "m7-viewer-write-denied".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            expires_in_days: 1,
        },
    )
    .await;
    assert!(
        viewer_write_credential.is_err(),
        "a Viewer cannot obtain a new MCP write credential"
    );
    let viewer_read_credential = mcp::issue_credential(
        &pool,
        IssueCredentialCommand {
            project_id: seed.project_id,
            actor_id: seed.human_actor_id,
            client_name: "m7-viewer-read".to_owned(),
            scopes: vec!["read".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("issue Viewer read credential");
    mcp::revoke_credential(&pool, viewer_read_credential.credential_id, 1)
        .await
        .expect("revoke Viewer read credential");
    sqlx::query(
        "UPDATE project_memberships SET role = 'owner' WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("restore MCP Actor role");

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

    let metadata = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/api/v1/mcp")
                    .body(Body::empty())
                    .expect("metadata request"),
            )
            .await
            .expect("metadata response"),
    )
    .await;
    assert_eq!(metadata["resource"], "http://127.0.0.1:3100/api/v1/mcp");
    assert_eq!(metadata["scopes_supported"], json!(["read", "write"]));
    // The setup guide people send to agents names this server's address.
    let guide = application
        .clone()
        .oneshot(
            Request::builder()
                .uri("/connect.md")
                .body(Body::empty())
                .expect("guide request"),
        )
        .await
        .expect("guide response");
    assert_eq!(guide.status(), StatusCode::OK);
    let guide = String::from_utf8(
        to_bytes(guide.into_body(), 100_000)
            .await
            .expect("guide body")
            .to_vec(),
    )
    .expect("guide UTF-8");
    assert!(guide.contains("url = \"http://127.0.0.1:3100/api/v1/mcp\""));
    assert!(guide.contains("${AGENT_WORKSPACE_MCP_TOKEN}"));
    assert!(!guide.contains("{origin}"));

    let redirect_uri = "http://127.0.0.1:45678/callback";
    let registration = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "client_name":"codex-m6-oauth",
                            "redirect_uris":[redirect_uri],
                            "token_endpoint_auth_method":"none",
                            "grant_types":["authorization_code","refresh_token"],
                            "response_types":["code"]
                        }))
                        .expect("registration JSON"),
                    ))
                    .expect("registration request"),
            )
            .await
            .expect("registration response"),
    )
    .await;
    let client_id = registration["client_id"]
        .as_str()
        .unwrap_or_else(|| panic!("OAuth client ID: {registration}"))
        .to_owned();
    assert!(registration.get("client_secret").is_none());
    // Claude Code registers as "Claude Code (<server nickname>)". Clean the
    // name rather than refusing the connection.
    let claude_code = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "client_name":"Claude Code (agent-workspace)",
                            "redirect_uris":["http://localhost:45679/callback"],
                            "token_endpoint_auth_method":"none",
                            "grant_types":["authorization_code","refresh_token"],
                            "response_types":["code"],
                            "scope":"read write"
                        }))
                        .expect("registration JSON"),
                    ))
                    .expect("registration request"),
            )
            .await
            .expect("registration response"),
    )
    .await;
    assert_eq!(claude_code["client_name"], "Claude Code", "{claude_code}");

    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);
    let verifier = "v".repeat(43);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorize_url =
        url::Url::parse("http://127.0.0.1:3100/oauth/authorize").expect("authorize URL");
    authorize_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "read write")
        .append_pair("state", "m6-state")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        // Codex CLI 0.146.0 currently emits the same RFC 8707 resource twice.
        // The authorization endpoint must accept identical duplicates while
        // rejecting conflicting resource targets.
        .append_pair("resource", "http://127.0.0.1:3100/api/v1/mcp")
        .append_pair("resource", "http://127.0.0.1:3100/api/v1/mcp");
    // Signed out: go sign in, then come back to this screen.
    let authorize_path =
        authorize_url.path().to_owned() + "?" + authorize_url.query().expect("query");
    let signed_out = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&authorize_path)
                .body(Body::empty())
                .expect("authorize request"),
        )
        .await
        .expect("authorize response");
    assert_eq!(signed_out.status(), StatusCode::SEE_OTHER);
    let sign_in = url::Url::parse("http://127.0.0.1:3100")
        .and_then(|base| {
            base.join(
                signed_out.headers()[header::LOCATION]
                    .to_str()
                    .expect("location"),
            )
        })
        .expect("sign-in URL");
    assert_eq!(sign_in.path(), "/sign-in");
    assert_eq!(
        sign_in
            .query_pairs()
            .find(|(key, _)| key == "returnTo")
            .map(|(_, value)| value.into_owned()),
        Some(authorize_path.clone())
    );
    assert!(
        signed_out.headers()[header::SET_COOKIE]
            .to_str()
            .expect("cookie")
            .contains("sign_in_return=")
    );

    let consent_page = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(&authorize_path)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .expect("authorize request"),
        )
        .await
        .expect("authorize response");
    assert_eq!(consent_page.status(), StatusCode::OK);
    let consent_html = String::from_utf8(
        to_bytes(consent_page.into_body(), 100_000)
            .await
            .expect("consent body")
            .to_vec(),
    )
    .expect("consent UTF-8");
    assert!(consent_html.contains("Connect codex-m6-oauth"));
    // Browsers send `Origin: null` on form posts under the site-wide
    // `no-referrer` policy, which the consent endpoint rejects.
    assert!(consent_html.contains("<meta name=referrer content=same-origin>"));
    assert!(consent_html.contains("read write"));
    assert!(!consent_html.contains(OAUTH_PASSWORD));
    let consent_form_start = consent_html.find("<form").expect("OAuth consent form");
    let consent_form_end = consent_html
        .find("</form>")
        .expect("OAuth consent form close");
    let consent_form = &consent_html[consent_form_start..consent_form_end];
    assert!(consent_form.contains("action=/connect/confirm"));
    assert!(
        consent_form.contains("name=access value=full") && consent_form.contains("name=show_via"),
        "the access and via-label choices must be submitted by the consent form"
    );
    assert!(consent_form.contains("name=decision"));

    let conflicting_authorize_uri = authorize_url.path().to_owned()
        + "?"
        + authorize_url.query().expect("query")
        + "&resource=http%3A%2F%2F127.0.0.1%3A3100%2Fapi%2Fv1%2Fother";
    let conflicting_resource = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(conflicting_authorize_uri)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .expect("conflicting resource request"),
        )
        .await
        .expect("conflicting resource response");
    assert_eq!(conflicting_resource.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_response(conflicting_resource).await["error"],
        "invalid_request"
    );

    let consent_body = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", redirect_uri),
        ("scope", "read write"),
        ("state", "m6-state"),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("resource", "http://127.0.0.1:3100/api/v1/mcp"),
        ("project_id", &seed.project_id.to_string()),
        ("csrf_token", &csrf),
        ("decision", "approve"),
    ])
    .expect("consent form");
    let consent_response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/connect/confirm")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(consent_body))
                .expect("consent request"),
        )
        .await
        .expect("consent response");
    assert_eq!(consent_response.status(), StatusCode::OK);
    // The browser goes back to the client through a page that forwards
    // itself; a plain redirect is blocked by `form-action 'self'`.
    let continue_page = String::from_utf8(
        to_bytes(consent_response.into_body(), 100_000)
            .await
            .expect("continue page")
            .to_vec(),
    )
    .expect("continue page UTF-8");
    let target = continue_page
        .split("url=")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("forwarding target")
        .replace("&amp;", "&");
    let callback = url::Url::parse(&target).expect("callback URL");
    assert_eq!(
        callback
            .query_pairs()
            .find(|(name, _)| name == "state")
            .map(|(_, value)| value.into_owned()),
        Some("m6-state".to_owned())
    );
    let code = callback
        .query_pairs()
        .find(|(name, _)| name == "code")
        .map(|(_, value)| value.into_owned())
        .expect("authorization code");
    let token_body = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("code", &code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", &verifier),
    ])
    .expect("token form");
    let tokens = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(token_body.clone()))
                    .expect("token request"),
            )
            .await
            .expect("token response"),
    )
    .await;
    let access_token = tokens["access_token"].as_str().expect("access token");
    let refresh_token = tokens["refresh_token"].as_str().expect("refresh token");
    assert_eq!(tokens["scope"], "read write");
    let stored_oauth = serde_json::to_string(&sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT access.token_digest, refresh.token_digest FROM mcp_oauth_access_tokens access JOIN mcp_oauth_refresh_tokens refresh ON refresh.grant_id = access.grant_id LIMIT 1",
    ).fetch_one(&pool).await.expect("stored OAuth digests")).expect("digest JSON");
    assert!(!stored_oauth.contains(access_token));
    assert!(!stored_oauth.contains(refresh_token));

    let oauth_initialized = mcp_request(&application, Some(access_token), None, &initialize).await;
    assert_eq!(oauth_initialized.status(), StatusCode::OK);
    let oauth_session = oauth_initialized
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("OAuth MCP session")
        .to_owned();
    let oauth_update = call_tool(
        &application,
        access_token,
        &oauth_session,
        20,
        "task_comment",
        json!({
            "workItemId":work_item_id.to_string(),
            "update":"OAuth-attributed useful update.",
            "clientRequestId":"m6-oauth-post-1"
        }),
    )
    .await;
    assert_eq!(oauth_update["outcome"], "applied");
    let oauth_audit = sqlx::query_as::<_, (String, String, Option<Uuid>, Option<Uuid>)>(
        "SELECT authentication_class, channel, mcp_credential_id, mcp_oauth_grant_id FROM audit_events WHERE request_id = 'm6-oauth-post-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("OAuth audit");
    assert_eq!(oauth_audit.0, "mcp_oauth");
    assert_eq!(oauth_audit.1, "mcp");
    assert!(oauth_audit.2.is_none());
    assert!(oauth_audit.3.is_some());

    let replayed_code = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(token_body))
                .expect("replayed code request"),
        )
        .await
        .expect("replayed code response");
    assert_eq!(replayed_code.status(), StatusCode::BAD_REQUEST);

    let refresh_body = serde_urlencoded::to_string([
        ("grant_type", "refresh_token"),
        ("client_id", &client_id),
        ("refresh_token", refresh_token),
    ])
    .expect("refresh form");
    let refreshed = json_response(
        application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(refresh_body.clone()))
                    .expect("refresh request"),
            )
            .await
            .expect("refresh response"),
    )
    .await;
    let refreshed_access = refreshed["access_token"]
        .as_str()
        .expect("refreshed access token");
    // Several sessions of one client refresh with the same token at once, and
    // a client whose response got lost retries it. Right after the exchange
    // that still works and leaves every token of the grant working.
    let refresh_again = |body: String| {
        application.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .expect("refresh replay request"),
        )
    };
    let parallel_refresh = json_response(
        refresh_again(refresh_body.clone())
            .await
            .expect("refresh replay response"),
    )
    .await;
    let parallel_access = parallel_refresh["access_token"]
        .as_str()
        .expect("parallel refresh access token");
    assert_ne!(parallel_access, refreshed_access);
    for access in [refreshed_access, parallel_access] {
        let still_working = mcp_request(&application, Some(access), None, &initialize).await;
        assert_eq!(still_working.status(), StatusCode::OK);
    }
    // A replay inside the grace period mustn't become a second branch that
    // lives on its own. Say the parallel refresh was a thief's: the thief
    // rotates their branch, which retires the honest branch, and when the
    // honest client refreshes later, the grant goes with every branch in it.
    let refresh_form = |token: &str| {
        serde_urlencoded::to_string([
            ("grant_type", "refresh_token"),
            ("client_id", &client_id),
            ("refresh_token", token),
        ])
        .expect("refresh form")
    };
    let honest_refresh = refreshed["refresh_token"]
        .as_str()
        .expect("honest refresh token");
    let thief_refresh = parallel_refresh["refresh_token"]
        .as_str()
        .expect("replayed refresh token");
    let thief_rotated = json_response(
        refresh_again(refresh_form(thief_refresh))
            .await
            .expect("thief rotation response"),
    )
    .await;
    let thief_access = thief_rotated["access_token"]
        .as_str()
        .expect("thief access token");
    let thief_next_refresh = thief_rotated["refresh_token"]
        .as_str()
        .expect("thief next refresh token");
    let live_refresh_tokens = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM mcp_oauth_refresh_tokens WHERE grant_id = (SELECT grant_id FROM mcp_oauth_refresh_tokens WHERE token_digest = $1) AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(Sha256::digest(thief_next_refresh.as_bytes()).to_vec())
    .fetch_one(&pool)
    .await
    .expect("live refresh tokens");
    assert_eq!(
        live_refresh_tokens, 1,
        "a rotation leaves one live refresh token in the grant"
    );
    // A retired sibling still works right away, for sessions that hold
    // different branches and refresh together.
    let sibling_refresh = refresh_again(refresh_form(honest_refresh))
        .await
        .expect("sibling refresh response");
    assert_eq!(sibling_refresh.status(), StatusCode::OK);
    // Used again well after it was retired, it looks stolen: the grant goes.
    sqlx::query(
        "UPDATE mcp_oauth_refresh_tokens SET used_at = now() - interval '10 minutes' WHERE token_digest = $1",
    )
    .bind(Sha256::digest(honest_refresh.as_bytes()).to_vec())
    .execute(&pool)
    .await
    .expect("age the retired refresh token");
    let late_refresh = refresh_again(refresh_form(honest_refresh))
        .await
        .expect("late refresh response");
    assert_eq!(late_refresh.status(), StatusCode::BAD_REQUEST);
    for access in [refreshed_access, parallel_access, thief_access] {
        let family_revoked = mcp_request(&application, Some(access), None, &initialize).await;
        assert_eq!(family_revoked.status(), StatusCode::UNAUTHORIZED);
    }
    let thief_continues = refresh_again(refresh_form(thief_next_refresh))
        .await
        .expect("thief refresh after revocation");
    assert_eq!(
        thief_continues.status(),
        StatusCode::BAD_REQUEST,
        "no branch of a revoked grant can refresh"
    );
    let original_replayed = refresh_again(refresh_body)
        .await
        .expect("original refresh replay response");
    assert_eq!(original_replayed.status(), StatusCode::BAD_REQUEST);

    let user_status_credential = mcp::issue_credential(
        &pool,
        IssueCredentialCommand {
            project_id: seed.project_id,
            actor_id: seed.human_actor_id,
            client_name: "m7-user-status-proof".to_owned(),
            scopes: vec!["read".to_owned()],
            expires_in_days: 1,
        },
    )
    .await
    .expect("issue credential before disabling User");
    sqlx::query("UPDATE users SET status = 'disabled' WHERE actor_id = $1")
        .bind(seed.human_actor_id)
        .execute(&pool)
        .await
        .expect("disable MCP resource owner User");
    let disabled_user = mcp_request(
        &application,
        Some(&user_status_credential.token),
        None,
        &initialize,
    )
    .await;
    assert_eq!(
        disabled_user.status(),
        StatusCode::UNAUTHORIZED,
        "an active credential cannot bypass a disabled human User"
    );

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

/// Every tool has a unique snake_case name, a description, an object input
/// schema and, when present, an object output schema, and every schema in
/// them is well formed (see `assert_schema_well_formed`).
fn assert_tools_are_well_formed(tools: &[Value]) {
    assert!(!tools.is_empty(), "no tools listed");
    let mut names = BTreeSet::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        assert!(
            !name.is_empty()
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "tool name {name:?} is not snake_case"
        );
        assert!(names.insert(name.to_owned()), "tool {name} is listed twice");
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "tool {name} has no description"
        );
        for (key, required) in [("inputSchema", true), ("outputSchema", false)] {
            let Some(schema) = tool.get(key) else {
                assert!(!required, "tool {name} has no {key}");
                continue;
            };
            // Result enums (`ToolResult`) are a `oneOf` of object variants.
            assert!(
                schema["type"] == json!("object")
                    || (!required
                        && (schema.get("oneOf").is_some() || schema.get("anyOf").is_some())),
                "{name} {key} is not an object schema: {schema}"
            );
            assert_schema_well_formed(schema, schema, &format!("{name}.{key}"));
        }
    }
}

/// A structural JSON Schema check: known types, properties that are schemas,
/// required names that exist, and `$ref`s that resolve inside the document.
fn assert_schema_well_formed(root: &Value, schema: &Value, at: &str) {
    const TYPES: [&str; 7] = [
        "object", "array", "string", "integer", "number", "boolean", "null",
    ];
    let Some(object) = schema.as_object() else {
        assert!(
            schema.is_boolean(),
            "{at}: a schema must be an object or a boolean"
        );
        return;
    };
    match object.get("type") {
        None => {}
        Some(Value::String(kind)) => assert!(TYPES.contains(&kind.as_str()), "{at}: type {kind}"),
        Some(Value::Array(kinds)) => assert!(
            !kinds.is_empty()
                && kinds
                    .iter()
                    .all(|kind| kind.as_str().is_some_and(|kind| TYPES.contains(&kind))),
            "{at}: types {kinds:?}"
        ),
        Some(other) => panic!("{at}: type {other}"),
    }
    if let Some(reference) = object.get("$ref") {
        let pointer = reference
            .as_str()
            .and_then(|reference| reference.strip_prefix('#'))
            .unwrap_or_else(|| panic!("{at}: $ref {reference} is not local"));
        assert!(
            root.pointer(pointer).is_some(),
            "{at}: $ref {reference} does not resolve"
        );
    }
    let properties = object.get("properties");
    if let Some(properties) = properties {
        let properties = properties
            .as_object()
            .unwrap_or_else(|| panic!("{at}: properties is not an object"));
        for (field, property) in properties {
            assert_schema_well_formed(root, property, &format!("{at}.{field}"));
        }
    }
    if let Some(required) = object.get("required") {
        for field in required
            .as_array()
            .unwrap_or_else(|| panic!("{at}: required is not a list"))
        {
            let field = field
                .as_str()
                .unwrap_or_else(|| panic!("{at}: required {field}"));
            assert!(
                properties
                    .and_then(|properties| properties.get(field))
                    .is_some(),
                "{at}: required field {field} has no property"
            );
        }
    }
    for key in ["items", "additionalProperties", "not"] {
        if let Some(child) = object.get(key) {
            assert_schema_well_formed(root, child, &format!("{at}/{key}"));
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(children) = object.get(key) {
            let children = children
                .as_array()
                .unwrap_or_else(|| panic!("{at}: {key} is not a list"));
            assert!(!children.is_empty(), "{at}: {key} is empty");
            for (index, child) in children.iter().enumerate() {
                assert_schema_well_formed(root, child, &format!("{at}/{key}/{index}"));
            }
        }
    }
    for key in ["$defs", "definitions"] {
        if let Some(definitions) = object.get(key).and_then(Value::as_object) {
            for (definition, child) in definitions {
                assert_schema_well_formed(root, child, &format!("{at}/{key}/{definition}"));
            }
        }
    }
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
                    serde_json::to_vec(&json!({"loginName":OAUTH_LOGIN,"password":OAUTH_PASSWORD}))
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

async fn json_response(response: axum::response::Response) -> Value {
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("JSON body");
    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "JSON response {status}: {error}; {}",
            String::from_utf8_lossy(&body)
        )
    })
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
