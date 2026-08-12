use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    domain::{RunState, transition_run},
    external_events::enqueue_external_approval_requested,
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
                "scopes":["read","write","events","approval_respond","worker_request"]
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(seed.project_id)
        .bind(actor_id)
        .fetch_one(&pool)
        .await
        .expect("initial external membership version"),
        1
    );

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
    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', account_state = 'authenticated' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&pool)
    .await
    .expect("prepare sponsor Execution Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare sponsored repository");

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
    let session_id = initialize_mcp(&application, &first_token).await;
    let conversations = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_list",
        json!({"projectId":seed.project_id.to_string(),"limit":1}),
    )
    .await;
    assert_eq!(conversations["outcome"], "found");
    assert_eq!(
        conversations["value"]["conversations"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(conversations["value"]["nextCursor"].is_string());
    let mentionable = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "mentionable_actor_search",
        json!({"query":"Hermes","limit":10}),
    )
    .await;
    assert_eq!(mentionable["outcome"], "found");
    assert_eq!(
        mentionable["value"]["actors"][0]["id"],
        actor_id.to_string()
    );

    let sent = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_message_send",
        json!({
            "conversationId":seed.thread_id.to_string(),
            "clientRequestId":"m9-external-message-1",
            "content":{"version":1,"nodes":[
                {"type":"text","text":"<script>alert('inert')</script> hello "},
                {"type":"mention","actorId":seed.human_actor_id}
            ]}
        }),
    )
    .await;
    assert_eq!(sent["outcome"], "applied");
    assert_eq!(sent["idempotentReplay"], false);
    assert_eq!(
        sent["result"]["message"]["author"]["id"],
        actor_id.to_string()
    );
    assert_eq!(
        sent["result"]["message"]["author"]["kind"],
        "external_agent"
    );
    let external_message_id = sent["result"]["message"]["id"]
        .as_str()
        .expect("external Message id")
        .to_owned();
    let replayed_message = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_message_send",
        json!({
            "conversationId":seed.thread_id.to_string(),
            "clientRequestId":"m9-external-message-1",
            "content":{"version":1,"nodes":[
                {"type":"text","text":"<script>alert('inert')</script> hello "},
                {"type":"mention","actorId":seed.human_actor_id}
            ]}
        }),
    )
    .await;
    assert_eq!(replayed_message["outcome"], "applied");
    assert_eq!(replayed_message["idempotentReplay"], true);
    assert_eq!(
        replayed_message["result"]["message"]["id"],
        external_message_id
    );
    let conflicting_message = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_message_send",
        json!({
            "conversationId":seed.thread_id.to_string(),
            "clientRequestId":"m9-external-message-1",
            "content":{"version":1,"nodes":[{"type":"text","text":"different"}]}
        }),
    )
    .await;
    assert_eq!(conflicting_message["outcome"], "conflict");

    let timeline = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_timeline_get",
        json!({"conversationId":seed.thread_id.to_string(),"limit":10}),
    )
    .await;
    assert_eq!(timeline["outcome"], "found");
    assert_eq!(
        timeline["value"]["entries"][0]["message"]["id"],
        external_message_id
    );
    let browser_timeline = application
        .clone()
        .oneshot(read_request(
            &format!("/api/v1/conversations/{}/timeline", seed.thread_id),
            &cookie,
        ))
        .await
        .expect("browser timeline response");
    assert_eq!(browser_timeline.status(), StatusCode::OK);
    let browser_timeline = response_json(browser_timeline).await;
    assert_eq!(
        browser_timeline["entries"][0]["message"]["author"]["kind"],
        "external_agent"
    );
    assert_eq!(
        browser_timeline["entries"][0]["message"]["plainText"],
        "<script>alert('inert')</script> hello @mischa"
    );

    let human_mention = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!("/api/v1/threads/{}/messages", seed.thread_id),
            &cookie,
            &csrf,
            "m9-human-mention-1",
            json!({"content":{"version":1,"nodes":[
                {"type":"text","text":"Please inspect this, "},
                {"type":"mention","actorId":actor_id}
            ]}}),
        ))
        .await
        .expect("human mention response");
    assert_eq!(human_mention.status(), StatusCode::CREATED);
    let first_poll = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "external_event_poll",
        json!({"limit":10,"waitSeconds":0}),
    )
    .await;
    assert_eq!(first_poll["outcome"], "events");
    assert_eq!(first_poll["events"][0]["kind"], "connection.created");
    assert_eq!(
        first_poll["events"][0]["resourceId"],
        connection_id.to_string()
    );
    assert_eq!(first_poll["events"][1]["kind"], "message.mentioned");
    assert!(first_poll["events"][1].get("body").is_none());
    let first_cursor = first_poll["nextCursor"]
        .as_str()
        .expect("external event cursor")
        .to_owned();
    let replayed = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "external_event_poll",
        json!({"limit":10,"waitSeconds":0}),
    )
    .await;
    assert_eq!(replayed["events"][0]["id"], first_poll["events"][0]["id"]);
    let empty = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "external_event_poll",
        json!({"afterCursor":first_cursor,"limit":10,"waitSeconds":0}),
    )
    .await;
    assert_eq!(empty["events"], json!([]));

    let created_work_item = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "work_item_create",
        json!({
            "title":"External teammate fixture",
            "description":"Created through the canonical M9 MCP command.",
            "priority":"medium",
            "assigneeActorId":actor_id.to_string(),
            "clientRequestId":"m9-work-item-create-1"
        }),
    )
    .await;
    assert_eq!(created_work_item["outcome"], "applied");
    let external_work_item_id = created_work_item["workItemId"]
        .as_str()
        .expect("created Work Item id")
        .parse::<Uuid>()
        .expect("created Work Item UUID");
    let updated_work_item = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "work_item_update",
        json!({
            "workItemId":external_work_item_id.to_string(),
            "expectedVersion":1,
            "priority":"high",
            "clientRequestId":"m9-work-item-update-1"
        }),
    )
    .await;
    assert_eq!(updated_work_item["outcome"], "applied");
    assert_eq!(updated_work_item["version"], 2);
    let read_work_item = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "work_item_get",
        json!({"identifier":external_work_item_id.to_string()}),
    )
    .await;
    assert_eq!(read_work_item["outcome"], "found");
    assert_eq!(
        read_work_item["value"]["assignee"]["actorId"],
        actor_id.to_string()
    );

    let addressed_run_id = Uuid::now_v7();
    let addressed_approval_id = Uuid::now_v7();
    let mut approval_transaction = pool.begin().await.expect("begin approval fixture");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, project_id, conversation_id, execution_worker_id,
            requesting_actor_id, execution_profile_id, repository_id,
            state, prompt, authorization_snapshot
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'blocked', $8, '{}'::jsonb)
        "#,
    )
    .bind(addressed_run_id)
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind("Addressed approval fixture")
    .execute(&mut *approval_transaction)
    .await
    .expect("insert addressed run");
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        ) VALUES (
            $1, $2, $3, $4, 'approval', $5,
            '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
            'member', false
        )
        "#,
    )
    .bind(addressed_approval_id)
    .bind(addressed_run_id)
    .bind(seed.human_actor_id)
    .bind(format!("m9-addressed:{addressed_approval_id}"))
    .bind("Allow one bounded fixture action?")
    .execute(&mut *approval_transaction)
    .await
    .expect("insert addressed approval");
    enqueue_external_approval_requested(&mut approval_transaction, addressed_approval_id)
        .await
        .expect("enqueue addressed approval");
    approval_transaction
        .commit()
        .await
        .expect("commit approval fixture");

    let addressed = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_get",
        json!({"approvalId":addressed_approval_id.to_string()}),
    )
    .await;
    assert_eq!(addressed["outcome"], "found");
    assert_eq!(addressed["approval"]["version"], 1);
    assert_eq!(addressed["approval"]["canRespond"], true);
    let denied_approval = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_respond",
        json!({
            "approvalId":addressed_approval_id.to_string(),
            "expectedVersion":1,
            "decision":"deny",
            "clientRequestId":"m9-addressed-deny-1"
        }),
    )
    .await;
    assert_eq!(denied_approval["outcome"], "applied");
    assert_eq!(denied_approval["approval"]["version"], 2);
    assert_eq!(denied_approval["approval"]["canRespond"], false);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
            .bind(addressed_run_id)
            .fetch_one(&pool)
            .await
            .expect("addressed run state"),
        "interrupted"
    );
    let stale_approval = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_respond",
        json!({
            "approvalId":addressed_approval_id.to_string(),
            "expectedVersion":1,
            "decision":"deny",
            "clientRequestId":"m9-addressed-deny-stale"
        }),
    )
    .await;
    assert_eq!(stale_approval["outcome"], "conflict");
    assert_eq!(stale_approval["current"]["version"], 2);

    let run_count_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("run count before sponsorship");
    let sponsored = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "external_execution_request_create",
        json!({
            "conversationId":seed.thread_id.to_string(),
            "repositoryId":seed.repository_id.to_string(),
            "sponsorActorId":seed.human_actor_id.to_string(),
            "prompt":"Make one bounded sponsored change.",
            "clientRequestId":"m9-sponsored-run-1"
        }),
    )
    .await;
    assert_eq!(sponsored["outcome"], "requested");
    let sponsored_request_id = sponsored["requestId"]
        .as_str()
        .expect("sponsored request id")
        .parse::<Uuid>()
        .expect("sponsored request UUID");
    let sponsored_approval_id = sponsored["approvalId"]
        .as_str()
        .expect("sponsored approval id")
        .parse::<Uuid>()
        .expect("sponsored approval UUID");
    let human_required = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_get",
        json!({"approvalId":sponsored_approval_id.to_string()}),
    )
    .await;
    assert_eq!(human_required["outcome"], "not_found");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .expect("preapproval run count"),
        run_count_before,
        "requesting sponsorship must not create an Agent Run"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT agent_run_id FROM external_execution_requests WHERE id = $1",
        )
        .bind(sponsored_request_id)
        .fetch_one(&pool)
        .await
        .expect("preapproval request state"),
        None
    );
    let sponsored_list = application
        .clone()
        .oneshot(read_request(
            &format!(
                "/api/v1/projects/{}/external-execution-requests",
                seed.project_id
            ),
            &cookie,
        ))
        .await
        .expect("sponsored request list response");
    assert_eq!(sponsored_list.status(), StatusCode::OK);
    let sponsored_list = response_json(sponsored_list).await;
    assert_eq!(
        sponsored_list["requests"][0]["id"],
        sponsored_request_id.to_string()
    );

    let approval_response = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!("/api/v1/agent-approvals/{sponsored_approval_id}/respond"),
            &cookie,
            &csrf,
            "m9-sponsored-approval-1",
            json!({"decision":"approve"}),
        ))
        .await
        .expect("sponsored approval response");
    assert_eq!(approval_response.status(), StatusCode::OK);
    let approval_response = response_json(approval_response).await;
    let sponsored_run_id = approval_response["agentRunId"]
        .as_str()
        .expect("sponsored Agent Run id")
        .parse::<Uuid>()
        .expect("sponsored Agent Run UUID");
    let run_attribution = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<Uuid>)>(
        "SELECT requesting_actor_id, sponsoring_actor_id, external_execution_request_id FROM agent_runs WHERE id = $1",
    )
    .bind(sponsored_run_id)
    .fetch_one(&pool)
    .await
    .expect("sponsored run attribution");
    assert_eq!(run_attribution.0, actor_id);
    assert_eq!(run_attribution.1, Some(seed.human_actor_id));
    assert_eq!(run_attribution.2, Some(sponsored_request_id));
    let mut terminal_transaction = pool.begin().await.expect("begin terminal transition");
    let sponsored_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1 FOR UPDATE")
            .bind(sponsored_run_id)
            .fetch_one(&mut *terminal_transaction)
            .await
            .expect("lock sponsored run state");
    if !matches!(
        sponsored_state.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        transition_run(
            &mut terminal_transaction,
            sponsored_run_id,
            RunState::Failed,
            Some("M9 terminal event fixture"),
        )
        .await
        .expect("finish sponsored run");
    }
    terminal_transaction
        .commit()
        .await
        .expect("commit terminal run transition");
    let sponsored_events = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "external_event_poll",
        json!({"afterCursor":first_cursor,"limit":10,"waitSeconds":0}),
    )
    .await;
    assert!(
        sponsored_events["events"]
            .as_array()
            .expect("sponsored events")
            .iter()
            .any(|event| event["kind"] == "external_execution.approved")
    );
    let event_kinds = sponsored_events["events"]
        .as_array()
        .expect("M9 event list")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "work_item.assigned",
        "work_item.changed",
        "approval.requested",
        "approval.resolved",
        "agent_run.terminal",
    ] {
        assert!(event_kinds.contains(&expected), "missing event {expected}");
    }
    terminate_mcp(&application, &first_token, &session_id).await;
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(seed.project_id)
        .bind(actor_id)
        .fetch_one(&pool)
        .await
        .expect("membership version after bearer rotation"),
        1,
        "credential rotation must not create an unrelated membership conflict"
    );
    assert_mcp_auth(&application, &first_token, StatusCode::UNAUTHORIZED).await;
    assert_mcp_auth(&application, &second_token, StatusCode::OK).await;
    let second_session = initialize_mcp(&application, &second_token).await;
    let old_cursor_after_rotation = call_mcp_tool(
        &application,
        &second_token,
        &second_session,
        "external_event_poll",
        json!({"afterCursor":first_poll["nextCursor"],"limit":10,"waitSeconds":0}),
    )
    .await;
    assert_eq!(old_cursor_after_rotation["outcome"], "invalid_request");
    terminate_mcp(&application, &second_token, &second_session).await;

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
    assert_eq!(enabled["connection"]["scopes"], json!(["events", "read"]));
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
        11
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

fn write_request_with_idempotency(
    method: &str,
    uri: &str,
    cookie: &str,
    csrf: &str,
    idempotency_key: &str,
    body: Value,
) -> Request<Body> {
    let mut request = write_request(method, uri, cookie, csrf, body);
    request.headers_mut().insert(
        "idempotency-key",
        idempotency_key.parse().expect("Idempotency-Key header"),
    );
    request
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

async fn initialize_mcp(application: &axum::Router, token: &str) -> String {
    let response = mcp_request(
        application,
        token,
        None,
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-06-18",
                "capabilities":{},
                "clientInfo":{"name":"hermes","version":"0.20.0"}
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("MCP session id")
        .to_owned();
    let initialized = mcp_request(
        application,
        token,
        Some(&session_id),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);
    session_id
}

async fn call_mcp_tool(
    application: &axum::Router,
    token: &str,
    session_id: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = mcp_request(
        application,
        token,
        Some(session_id),
        json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }),
    )
    .await;
    let response = protocol_json(response).await;
    response
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or_else(|| panic!("missing MCP structured content: {response}"))
}

async fn terminate_mcp(application: &axum::Router, token: &str, session_id: &str) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(MCP_PATH)
                .header(header::HOST, "127.0.0.1")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("mcp-session-id", session_id)
                .header("mcp-protocol-version", "2025-06-18")
                .body(Body::empty())
                .expect("MCP termination request"),
        )
        .await
        .expect("MCP termination response");
    assert!(
        [StatusCode::OK, StatusCode::ACCEPTED, StatusCode::NO_CONTENT].contains(&response.status()),
        "unexpected MCP termination status {}",
        response.status()
    );
}

async fn mcp_request(
    application: &axum::Router,
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
    application
        .clone()
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&payload).expect("MCP JSON")))
                .expect("MCP request"),
        )
        .await
        .expect("MCP response")
}

async fn protocol_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("read MCP response");
    let text = String::from_utf8(body.to_vec()).expect("MCP UTF-8");
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
        .expect("connect schema-scoped M9 pool")
}
