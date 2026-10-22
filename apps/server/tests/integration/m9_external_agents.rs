use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    domain::{RunState, transition_run},
    external_events::{enqueue_external_approval_requested, enqueue_external_event},
    mcp::MCP_PATH,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    retention, router,
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
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace_server=debug")
        .try_init();
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
    assert_eq!(created["connection"]["runtimeKind"], "mcp");
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
    let tools = protocol_json(
        mcp_request(
            &application,
            &first_token,
            Some(&session_id),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await,
    )
    .await;
    let message_tool = tools["result"]["tools"]
        .as_array()
        .expect("MCP tools")
        .iter()
        .find(|tool| tool["name"] == "message_send")
        .expect("message_send tool");
    assert!(message_tool["inputSchema"]["properties"]["text"].is_object());
    assert!(message_tool["inputSchema"]["properties"]["content"].is_object());
    assert_eq!(
        message_tool["inputSchema"]["$defs"]["MessageContent"]["type"],
        "object"
    );
    assert_eq!(
        message_tool["inputSchema"]["$defs"]["MessageContent"]["properties"]["nodes"]["type"],
        "array"
    );
    let conversations = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "conversation_list",
        json!({"projectId":seed.project_id.to_string(),"limit":1}),
    )
    .await;
    assert_eq!(conversations["outcome"], "ok");
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
        "people_search",
        json!({"query":"Hermes","limit":10}),
    )
    .await;
    assert_eq!(mentionable["outcome"], "ok");
    assert_eq!(
        mentionable["value"]["people"][0]["id"],
        actor_id.to_string()
    );

    let sent = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "message_send",
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
    assert_eq!(sent["outcome"], "ok", "{sent}");
    assert_eq!(sent["value"]["idempotentReplay"], false);
    let external_message_id = sent["value"]["messageId"]
        .as_str()
        .expect("external Message id")
        .to_owned();
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>("SELECT author_actor_id FROM messages WHERE id = $1")
            .bind(external_message_id.parse::<Uuid>().expect("message UUID"))
            .fetch_one(&pool)
            .await
            .expect("external message author"),
        actor_id
    );
    let replayed_message = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "message_send",
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
    assert_eq!(replayed_message["outcome"], "ok");
    assert_eq!(replayed_message["value"]["idempotentReplay"], true);
    assert_eq!(replayed_message["value"]["messageId"], external_message_id);
    let conflicting_message = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "message_send",
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
        "conversation_read",
        json!({"conversationId":seed.thread_id.to_string(),"limit":10}),
    )
    .await;
    assert_eq!(timeline["outcome"], "ok");
    assert_eq!(
        timeline["value"]["messages"][0]["messageId"],
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
        "task_create",
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
    let mismatched_create = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "task_create",
        json!({
            "title":"External teammate fixture",
            "description":"A different description must not replay.",
            "priority":"medium",
            "assigneeActorId":actor_id.to_string(),
            "clientRequestId":"m9-work-item-create-1"
        }),
    )
    .await;
    assert_eq!(mismatched_create["outcome"], "conflict");
    assert_eq!(mismatched_create["retryCategory"], "use_new_request_id");
    let created_subtask = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "task_create",
        json!({
            "parentWorkItemId":external_work_item_id.to_string(),
            "title":"Idempotency subtask fixture",
            "description":"Original subtask description",
            "clientRequestId":"m9-subtask-create-1"
        }),
    )
    .await;
    assert_eq!(created_subtask["outcome"], "applied");
    let mismatched_subtask = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "task_create",
        json!({
            "parentWorkItemId":external_work_item_id.to_string(),
            "title":"Idempotency subtask fixture",
            "description":"Different subtask description",
            "clientRequestId":"m9-subtask-create-1"
        }),
    )
    .await;
    assert_eq!(mismatched_subtask["outcome"], "conflict");
    assert_eq!(mismatched_subtask["retryCategory"], "use_new_request_id");
    let updated_work_item = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "task_update",
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
        "task_open",
        json!({"taskId":external_work_item_id.to_string()}),
    )
    .await;
    assert_eq!(read_work_item["outcome"], "ok");
    assert_eq!(
        read_work_item["value"]["task"]["assignee"]["actorId"],
        actor_id.to_string()
    );

    let browser_created = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!("/api/v1/projects/{}/work-items", seed.project_id),
            &cookie,
            &csrf,
            "m9-browser-work-item-create",
            json!({
                "title":"Browser external event fixture",
                "description":"Created by a human and assigned to an external teammate.",
                "typeId":null,
                "statusId":null,
                "priority":"medium",
                "assigneeActorId":actor_id
            }),
        ))
        .await
        .expect("browser Work Item create response");
    assert_eq!(browser_created.status(), StatusCode::CREATED);
    let browser_created = response_json(browser_created).await;
    let browser_work_item_id = browser_created["id"]
        .as_str()
        .expect("browser Work Item id")
        .parse::<Uuid>()
        .expect("browser Work Item UUID");
    let browser_updated = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "PATCH",
            &format!(
                "/api/v1/projects/{}/work-items/{browser_work_item_id}",
                seed.project_id
            ),
            &cookie,
            &csrf,
            "m9-browser-work-item-update",
            json!({"expectedVersion":1,"priority":"high"}),
        ))
        .await
        .expect("browser Work Item update response");
    assert_eq!(browser_updated.status(), StatusCode::OK);
    let browser_comment = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!(
                "/api/v1/projects/{}/work-items/{browser_work_item_id}/comments",
                seed.project_id
            ),
            &cookie,
            &csrf,
            "m9-browser-work-item-comment",
            json!({"content":{"version":1,"nodes":[
                {"type":"text","text":"Browser-authored update fixture"}
            ]}}),
        ))
        .await
        .expect("browser Work Item comment response");
    assert_eq!(browser_comment.status(), StatusCode::CREATED);
    let browser_event_kinds = sqlx::query_scalar::<_, String>(
        r#"
        SELECT string_agg(kind, ',' ORDER BY sequence)
        FROM external_agent_events
        WHERE connection_id = $1 AND resource_id = $2
        "#,
    )
    .bind(connection_id)
    .bind(browser_work_item_id)
    .fetch_one(&pool)
    .await
    .expect("browser Work Item external events");
    assert_eq!(
        browser_event_kinds,
        "work_item.assigned,work_item.changed,work_item.changed"
    );

    let second_created = application
        .clone()
        .oneshot(write_request(
            "POST",
            &base,
            &cookie,
            &csrf,
            json!({
                "connectionName":"Hermes M9 second",
                "displayName":"Second external teammate",
                "handle":"second-external-teammate",
                "role":"member",
                "scopes":["read","write","events","approval_respond"]
            }),
        ))
        .await
        .expect("create second connection response");
    assert_eq!(second_created.status(), StatusCode::OK);
    let second_created = response_json(second_created).await;
    let second_actor_id = second_created["connection"]["actorId"]
        .as_str()
        .expect("second connection actor id")
        .parse::<Uuid>()
        .expect("second connection Actor UUID");
    let second_token = second_created["oneTimeCredential"]["token"]
        .as_str()
        .expect("second connection token")
        .to_owned();
    let second_session_id = initialize_mcp(&application, &second_token).await;
    let second_initial_events = call_mcp_tool(
        &application,
        &second_token,
        &second_session_id,
        "external_event_poll",
        json!({"limit":10,"waitSeconds":0}),
    )
    .await;
    let second_initial_cursor = second_initial_events["nextCursor"]
        .as_str()
        .expect("second connection initial cursor")
        .to_owned();

    let self_response_run_id = Uuid::now_v7();
    let self_response_approval_id = Uuid::now_v7();
    let mut self_response_transaction = pool.begin().await.expect("begin self-response fixture");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, project_id, conversation_id, execution_worker_id,
            requesting_actor_id, execution_profile_id, repository_id,
            state, prompt, authorization_snapshot
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'blocked', $8, '{}'::jsonb)
        "#,
    )
    .bind(self_response_run_id)
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind("External self-response authorization fixture")
    .execute(&mut *self_response_transaction)
    .await
    .expect("insert self-response run");
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        ) VALUES (
            $1, $2, $3, $4, 'question', $5, '[]'::jsonb,
            'any_authorized', false
        )
        "#,
    )
    .bind(self_response_approval_id)
    .bind(self_response_run_id)
    .bind(actor_id)
    .bind(format!("m9-self-response:{self_response_approval_id}"))
    .bind("Which authorized teammate should answer this question?")
    .execute(&mut *self_response_transaction)
    .await
    .expect("insert self-response approval");
    enqueue_external_approval_requested(&mut self_response_transaction, self_response_approval_id)
        .await
        .expect("enqueue self-response approval request");
    self_response_transaction
        .commit()
        .await
        .expect("commit self-response fixture");

    let requester_self_response = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_respond",
        json!({
            "approvalId":self_response_approval_id.to_string(),
            "expectedVersion":1,
            "answer":"I should not be able to answer my own question.",
            "clientRequestId":"m9-self-response-requester"
        }),
    )
    .await;
    assert_eq!(requester_self_response["outcome"], "not_found");
    assert_eq!(
        sqlx::query_as::<_, (String, Option<Uuid>)>(
            "SELECT status, responded_by_actor_id FROM agent_approvals WHERE id = $1",
        )
        .bind(self_response_approval_id)
        .fetch_one(&pool)
        .await
        .expect("self-response approval remains pending"),
        ("pending".to_owned(), None)
    );

    let teammate_response = call_mcp_tool(
        &application,
        &second_token,
        &second_session_id,
        "approval_respond",
        json!({
            "approvalId":self_response_approval_id.to_string(),
            "expectedVersion":1,
            "answer":"Another authorized teammate can answer.",
            "clientRequestId":"m9-self-response-teammate"
        }),
    )
    .await;
    assert_eq!(teammate_response["outcome"], "applied");
    assert_eq!(
        sqlx::query_as::<_, (String, Option<Uuid>)>(
            "SELECT status, responded_by_actor_id FROM agent_approvals WHERE id = $1",
        )
        .bind(self_response_approval_id)
        .fetch_one(&pool)
        .await
        .expect("teammate resolved self-response approval"),
        ("resolved".to_owned(), Some(second_actor_id))
    );
    let mut self_response_terminal = pool
        .begin()
        .await
        .expect("begin self-response terminal transition");
    transition_run(
        &mut self_response_terminal,
        self_response_run_id,
        RunState::Completed,
        None,
    )
    .await
    .expect("complete self-response fixture run");
    self_response_terminal
        .commit()
        .await
        .expect("commit self-response terminal transition");

    let addressed_run_id = Uuid::now_v7();
    let addressed_approval_id = Uuid::now_v7();
    let human_required_approval_id = Uuid::now_v7();
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
            'any_authorized', false
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
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, eligible_actor_id,
            provider_request_id, kind, summary, options,
            eligible_role, human_required
        ) VALUES (
            $1, $2, $3, $4, $5, 'approval', $6,
            '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
            'member', true
        )
        "#,
    )
    .bind(human_required_approval_id)
    .bind(addressed_run_id)
    .bind(seed.human_actor_id)
    .bind(actor_id)
    .bind(format!("m9-human-required:{human_required_approval_id}"))
    .bind("Human-required approval audit fixture")
    .execute(&mut *approval_transaction)
    .await
    .expect("insert human-required approval");
    enqueue_external_approval_requested(&mut approval_transaction, addressed_approval_id)
        .await
        .expect("enqueue addressed approval");
    approval_transaction
        .commit()
        .await
        .expect("commit approval fixture");

    let hidden_human_required = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_respond",
        json!({
            "approvalId":human_required_approval_id.to_string(),
            "expectedVersion":1,
            "decision":"approve",
            "clientRequestId":"m9-human-required-denial-1"
        }),
    )
    .await;
    assert_eq!(hidden_human_required["outcome"], "not_found");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*) FROM audit_events
            WHERE external_agent_connection_id = $1
              AND target_id = $2
              AND action = 'approval.external_response_rejected'
              AND outcome = 'denied'
              AND metadata->>'reason' = 'human_required'
            "#,
        )
        .bind(connection_id)
        .bind(human_required_approval_id)
        .fetch_one(&pool)
        .await
        .expect("human-required denial audit count"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM agent_approvals WHERE id = $1")
            .bind(human_required_approval_id)
            .fetch_one(&pool)
            .await
            .expect("human-required approval status"),
        "pending"
    );
    sqlx::query("UPDATE agent_approvals SET status = 'canceled' WHERE id = $1")
        .bind(human_required_approval_id)
        .execute(&pool)
        .await
        .expect("clean up human-required approval fixture");

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
    let invalid_approval = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "approval_respond",
        json!({
            "approvalId":addressed_approval_id.to_string(),
            "expectedVersion":1,
            "decision":"later",
            "clientRequestId":"m9-addressed-invalid-1"
        }),
    )
    .await;
    assert_eq!(invalid_approval["outcome"], "invalid_request");
    assert_eq!(invalid_approval["field"], "approval decision");
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

    let second_addressed_events = call_mcp_tool(
        &application,
        &second_token,
        &second_session_id,
        "external_event_poll",
        json!({
            "afterCursor":second_initial_cursor,
            "limit":10,
            "waitSeconds":0
        }),
    )
    .await;
    let second_addressed_kinds = second_addressed_events["events"]
        .as_array()
        .expect("second connection addressed events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(second_addressed_kinds.contains(&"approval.requested"));
    assert!(second_addressed_kinds.contains(&"approval.resolved"));
    let second_addressed_cursor = second_addressed_events["nextCursor"]
        .as_str()
        .expect("second connection addressed cursor")
        .to_owned();

    let browser_resolution_run_id = Uuid::now_v7();
    let browser_resolution_approval_id = Uuid::now_v7();
    let mut browser_resolution_transaction = pool
        .begin()
        .await
        .expect("begin browser resolution fixture");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, project_id, conversation_id, execution_worker_id,
            requesting_actor_id, execution_profile_id, repository_id,
            state, prompt, authorization_snapshot
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'blocked', $8, '{}'::jsonb)
        "#,
    )
    .bind(browser_resolution_run_id)
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .bind("Browser approval resolution fixture")
    .execute(&mut *browser_resolution_transaction)
    .await
    .expect("insert browser resolution run");
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        ) VALUES (
            $1, $2, $3, $4, 'question', $5, '[]'::jsonb,
            'any_authorized', false
        )
        "#,
    )
    .bind(browser_resolution_approval_id)
    .bind(browser_resolution_run_id)
    .bind(actor_id)
    .bind(format!(
        "m9-browser-resolution:{browser_resolution_approval_id}"
    ))
    .bind("Provide one bounded fixture answer.")
    .execute(&mut *browser_resolution_transaction)
    .await
    .expect("insert browser resolution approval");
    enqueue_external_approval_requested(
        &mut browser_resolution_transaction,
        browser_resolution_approval_id,
    )
    .await
    .expect("enqueue browser resolution request");
    browser_resolution_transaction
        .commit()
        .await
        .expect("commit browser resolution fixture");
    let browser_projection = application
        .clone()
        .oneshot(read_request(
            &format!("/api/v1/conversations/{}/timeline", seed.thread_id),
            &cookie,
        ))
        .await
        .expect("browser approval projection response");
    let browser_projection_status = browser_projection.status();
    let browser_projection = response_json(browser_projection).await;
    assert_eq!(
        browser_projection_status,
        StatusCode::OK,
        "browser approval projection: {browser_projection}"
    );
    assert!(
        browser_projection["approvals"]
            .as_array()
            .expect("browser approval projection")
            .iter()
            .any(|approval| {
                approval["id"] == browser_resolution_approval_id.to_string()
                    && approval["capabilities"]["respond"] == true
            })
    );
    let browser_resolution = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!("/api/v1/agent-approvals/{browser_resolution_approval_id}/respond"),
            &cookie,
            &csrf,
            "m9-browser-approval-resolution",
            json!({"answer":"Bounded browser answer"}),
        ))
        .await
        .expect("browser approval resolution response");
    let browser_resolution_status = browser_resolution.status();
    let browser_resolution_body = response_json(browser_resolution).await;
    assert_eq!(
        browser_resolution_status,
        StatusCode::OK,
        "browser approval resolution: {browser_resolution_body}"
    );
    let second_browser_resolution_events = call_mcp_tool(
        &application,
        &second_token,
        &second_session_id,
        "external_event_poll",
        json!({
            "afterCursor":second_addressed_cursor,
            "limit":10,
            "waitSeconds":0
        }),
    )
    .await;
    let second_browser_resolution_kinds = second_browser_resolution_events["events"]
        .as_array()
        .expect("second connection browser resolution events")
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(second_browser_resolution_kinds.contains(&"approval.requested"));
    assert!(second_browser_resolution_kinds.contains(&"approval.resolved"));
    let mut browser_resolution_terminal = pool
        .begin()
        .await
        .expect("begin browser resolution terminal transition");
    transition_run(
        &mut browser_resolution_terminal,
        browser_resolution_run_id,
        RunState::Completed,
        None,
    )
    .await
    .expect("complete browser resolution run");
    browser_resolution_terminal
        .commit()
        .await
        .expect("commit browser resolution terminal transition");

    let run_count_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .expect("run count before sponsorship");
    let sponsored = call_mcp_tool(
        &application,
        &first_token,
        &session_id,
        "run_request",
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
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT config <> '{}'::jsonb FROM agent_runs WHERE id = $1")
            .bind(sponsored_run_id)
            .fetch_one(&pool)
            .await
            .expect("sponsored run settings"),
        "a sponsored run's settings freeze when it is created"
    );
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
        json!({"afterCursor":first_cursor,"limit":100,"waitSeconds":0}),
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
    terminate_mcp(&application, &second_token, &second_session_id).await;
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

    let expired_resource_id = Uuid::now_v7();
    let retained_resource_id = Uuid::now_v7();
    let mut event_transaction = pool.begin().await.expect("begin retention fixture");
    for resource_id in [expired_resource_id, retained_resource_id] {
        enqueue_external_event(
            &mut event_transaction,
            connection_id,
            seed.project_id,
            "connection.test",
            "Connector retention fixture changed",
            "external_agent_connection",
            resource_id,
            &format!("/projects/{}/settings", seed.project_id),
        )
        .await
        .expect("enqueue retention fixture");
    }
    event_transaction
        .commit()
        .await
        .expect("commit retention fixture");
    sqlx::query(
        "UPDATE external_agent_events SET created_at=now()-interval '40 days', expires_at=now()-interval '10 days' WHERE resource_id=$1",
    )
    .bind(expired_resource_id)
    .execute(&pool)
    .await
    .expect("expire retention fixture");
    let dry_run = retention::clean_external_events(&pool, false)
        .await
        .expect("dry-run external event retention");
    assert_eq!(dry_run.mode, "dry_run");
    assert_eq!(dry_run.expired_candidates, 1);
    assert_eq!(dry_run.removed_events, 0);
    let applied = retention::clean_external_events(&pool, true)
        .await
        .expect("apply external event retention");
    assert_eq!(applied.mode, "apply");
    assert_eq!(applied.removed_events, 1);
    assert_eq!(applied.remaining_expired_events, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM external_agent_events WHERE resource_id=$1",
        )
        .bind(retained_resource_id)
        .fetch_one(&pool)
        .await
        .expect("retained external event count"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM external_agent_connections WHERE id=$1",
        )
        .bind(connection_id)
        .fetch_one(&pool)
        .await
        .expect("retained connection count"),
        1
    );

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
        13
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

#[tokio::test]
async fn existing_agent_joins_another_project_without_another_identity_or_token() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("member_accounts_{}", Uuid::now_v7().simple());
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

    let created = application.clone().oneshot(write_request("POST", &base, &cookie, &csrf,
        json!({"connectionName":"Shared agent","displayName":"Shared agent","handle":"shared_agent","role":"member","scopes":["read","write","events"]}))).await.expect("account fixture operation");
    assert_eq!(created.status(), StatusCode::OK);
    let created = response_json(created).await;
    let actor = created["connection"]["actorId"]
        .as_str()
        .expect("account fixture operation");
    let token = created["oneTimeCredential"]["token"]
        .as_str()
        .expect("account fixture operation");
    let second = Uuid::now_v7();
    let other_org = Uuid::now_v7();
    let other_project = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations(id,name,slug) VALUES ($1,'Other organization',$2)")
        .bind(other_org)
        .bind(format!("other-{}", other_org.simple()))
        .execute(&pool)
        .await
        .expect("other organization");
    sqlx::query("INSERT INTO organization_memberships(organization_id,actor_id,role) VALUES ($1,$2,'owner')").bind(other_org).bind(seed.human_actor_id).execute(&pool).await.expect("other owner");
    sqlx::query("INSERT INTO projects(id,organization_id,project_key,name) VALUES ($1,$2,'OTHER','Other project')").bind(other_project).bind(other_org).execute(&pool).await.expect("other project");
    sqlx::query("INSERT INTO project_memberships(id,project_id,actor_id,role,display_name,handle) SELECT $1,$2,actor_id,role,display_name,handle FROM project_memberships WHERE project_id=$3 AND actor_id=$4").bind(Uuid::now_v7()).bind(other_project).bind(seed.project_id).bind(seed.human_actor_id).execute(&pool).await.expect("other project owner");
    let cross_org = application
        .clone()
        .oneshot(write_request(
            "POST",
            &format!("/api/v1/projects/{other_project}/members"),
            &cookie,
            &csrf,
            json!({"actorId":actor,"role":"member"}),
        ))
        .await
        .expect("cross organization request");
    assert_eq!(
        cross_org.status(),
        StatusCode::NOT_FOUND,
        "a project owner cannot import another organization's agent"
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT agent_connection_can_access($1,$2)")
            .bind(
                created["connection"]["id"]
                    .as_str()
                    .expect("connection")
                    .parse::<Uuid>()
                    .expect("connection UUID")
            )
            .bind(other_project)
            .fetch_one(&pool)
            .await
            .expect("cross organization authorization")
    );

    sqlx::query("INSERT INTO projects(id,organization_id,project_key,name) VALUES ($1,$2,'SECOND','Second project')").bind(second).bind(seed.organization_id).execute(&pool).await.expect("account fixture operation");
    sqlx::query("INSERT INTO project_memberships(id,project_id,actor_id,role,display_name,handle) SELECT $1,$2,actor_id,role,display_name,handle FROM project_memberships WHERE project_id=$3 AND actor_id=$4")
        .bind(Uuid::now_v7()).bind(second).bind(seed.project_id).bind(seed.human_actor_id).execute(&pool).await.expect("account fixture operation");
    let session = initialize_mcp(&application, token).await;
    let denied = mcp_request(&application,token,Some(&session),json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"conversation_list","arguments":{"projectId":second}}})).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let candidates = response_json(
        application
            .clone()
            .oneshot(read_request(
                &format!("/api/v1/projects/{second}/member-candidates"),
                &cookie,
            ))
            .await
            .expect("account fixture operation"),
    )
    .await;
    assert!(
        candidates
            .as_array()
            .expect("account fixture operation")
            .iter()
            .any(|v| v["actorId"] == actor)
    );
    let add = application
        .clone()
        .oneshot(write_request(
            "POST",
            &format!("/api/v1/projects/{second}/members"),
            &cookie,
            &csrf,
            json!({"actorId":actor,"role":"member"}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(add.status(), StatusCode::OK);
    let added = response_json(add).await;
    assert_eq!(added["actorKind"], "external_agent");
    assert_eq!(added["handle"], "shared_agent");
    let mentions = application
        .clone()
        .oneshot(read_request(
            &format!("/api/v1/projects/{second}/mentionable-actors?q=shared_agent"),
            &cookie,
        ))
        .await
        .expect("search project mention picker");
    assert_eq!(mentions.status(), StatusCode::OK);
    let mentions = response_json(mentions).await;
    assert_eq!(mentions["actors"][0]["id"], actor);
    assert_eq!(mentions["actors"][0]["kind"], "external_agent");
    // An agent that cannot answer is not offered: a viewer cannot send, and a
    // read-only connection cannot claim the reply.
    let connection_id = created["connection"]["id"]
        .as_str()
        .expect("connection")
        .parse::<Uuid>()
        .expect("connection UUID");
    let actor_id = actor.parse::<Uuid>().expect("actor UUID");
    let original_scopes = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT scopes FROM external_agent_connections WHERE id=$1",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .expect("connection scopes");
    for (role, scopes) in [
        ("viewer", vec!["read", "write", "events"]),
        ("member", vec!["read", "events"]),
    ] {
        sqlx::query("UPDATE project_memberships SET role=$3 WHERE project_id=$1 AND actor_id=$2")
            .bind(second)
            .bind(actor_id)
            .bind(role)
            .execute(&pool)
            .await
            .expect("set agent role");
        sqlx::query("UPDATE external_agent_connections SET scopes=$2 WHERE id=$1")
            .bind(connection_id)
            .bind(&scopes)
            .execute(&pool)
            .await
            .expect("set connection scopes");
        let hidden = response_json(
            application
                .clone()
                .oneshot(read_request(
                    &format!("/api/v1/projects/{second}/mentionable-actors?q=shared_agent"),
                    &cookie,
                ))
                .await
                .expect("search project mention picker"),
        )
        .await;
        assert_eq!(
            hidden["actors"].as_array().map(Vec::len),
            Some(0),
            "{role} agent with {scopes:?} must not be mentionable"
        );
    }
    sqlx::query("UPDATE project_memberships SET role='member' WHERE project_id=$1 AND actor_id=$2")
        .bind(second)
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("restore agent role");
    sqlx::query("UPDATE external_agent_connections SET scopes=$2 WHERE id=$1")
        .bind(connection_id)
        .bind(&original_scopes)
        .execute(&pool)
        .await
        .expect("restore connection scopes");
    let allowed = call_mcp_tool(
        &application,
        token,
        &session,
        "conversation_list",
        json!({"projectId":second}),
    )
    .await;
    assert_eq!(allowed["outcome"], "ok");
    let channel = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "POST",
            &format!("/api/v1/projects/{second}/channels"),
            &cookie,
            &csrf,
            "second-channel",
            json!({"name":"shared"}),
        ))
        .await
        .expect("create channel");
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel = response_json(channel).await;
    let conversation = channel["conversation"]["id"].as_str().expect("channel ID");
    let sent = call_mcp_tool(&application,token,&session,"message_send",json!({"conversationId":conversation,"content":{"version":1,"nodes":[{"type":"text","text":"Same agent in a second project"}]},"clientRequestId":"second-project-message"})).await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT author_actor_id FROM messages WHERE conversation_id=$1"
        )
        .bind(conversation.parse::<Uuid>().expect("conversation UUID"))
        .fetch_one(&pool)
        .await
        .expect("authorship"),
        actor.parse::<Uuid>().expect("actor UUID")
    );
    let mention = application.clone().oneshot(write_request_with_idempotency("POST",&format!("/api/v1/channels/{conversation}/messages"),&cookie,&csrf,"ask-in-project",json!({"content":{"version":1,"nodes":[{"type":"mention","actorId":actor},{"type":"text","text":" please reply here"}]}}))).await.expect("mention shared agent");
    assert_eq!(mention.status(), StatusCode::CREATED);
    let inbox = call_mcp_tool(&application, token, &session, "inbox_list", json!({})).await;
    assert_eq!(inbox["outcome"], "ok");
    let request = &inbox["value"]["requests"][0];
    assert_eq!(request["projectId"], second.to_string());
    assert_eq!(request["conversationId"], conversation);
    let request_id = request["requestId"]
        .as_str()
        .expect("cross-project reply request");
    let claim = call_mcp_tool(
        &application,
        token,
        &session,
        "reply_claim",
        json!({"requestId":request_id}),
    )
    .await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let reply = call_mcp_tool(&application,token,&session,"reply_send",json!({"requestId":request_id,"generation":claim["value"]["request"]["generation"],"text":"Replying with my existing connection"})).await;
    assert_eq!(reply["outcome"], "ok", "{reply}");
    // One MCP session can alternate projects without leaking a selected project.
    let home = call_mcp_tool(
        &application,
        token,
        &session,
        "conversation_list",
        json!({"projectId":seed.project_id}),
    )
    .await;
    assert_eq!(home["outcome"], "ok");
    let elevated = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "PATCH",
            &format!("/api/v1/projects/{second}/members/{actor}"),
            &cookie,
            &csrf,
            "no-agent-admin",
            json!({"expectedVersion":1,"role":"admin"}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(elevated.status(), StatusCode::FORBIDDEN);
    let removed = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "PATCH",
            &format!("/api/v1/projects/{second}/members/{actor}"),
            &cookie,
            &csrf,
            "remove-agent",
            json!({"expectedVersion":1,"enabled":false}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(removed.status(), StatusCode::OK);
    let denied = mcp_request(&application,token,Some(&session),json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"conversation_list","arguments":{"projectId":second}}})).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let home = call_mcp_tool(
        &application,
        token,
        &session,
        "conversation_list",
        json!({"projectId":seed.project_id}),
    )
    .await;
    assert_eq!(home["outcome"], "ok");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors WHERE kind='external_agent'")
            .fetch_one(&pool)
            .await
            .expect("account fixture operation"),
        1
    );
    terminate_mcp(&application, token, &session).await;

    // An approval in an agent's additional project can be read and answered
    // with the home-project credential, and only while the agent belongs there.
    let approver = response_json(
        application
            .clone()
            .oneshot(write_request(
                "POST",
                &format!("/api/v1/projects/{second}/external-agents"),
                &cookie,
                &csrf,
                json!({"connectionName":"Approver agent","displayName":"Approver agent","handle":"approver_agent","role":"member","scopes":["read","write","approval_respond"]}),
            ))
            .await
            .expect("create approver agent"),
    )
    .await;
    let approver_actor = approver["connection"]["actorId"]
        .as_str()
        .expect("approver actor")
        .to_owned();
    let approver_token = approver["oneTimeCredential"]["token"]
        .as_str()
        .expect("approver token")
        .to_owned();
    let joined = application
        .clone()
        .oneshot(write_request(
            "POST",
            &format!("/api/v1/projects/{}/members", seed.project_id),
            &cookie,
            &csrf,
            json!({"actorId":approver_actor,"role":"member"}),
        ))
        .await
        .expect("add approver to seed project");
    assert_eq!(joined.status(), StatusCode::OK);
    let run_id = Uuid::now_v7();
    let approval_id = Uuid::now_v7();
    let mut fixture = pool.begin().await.expect("begin approval fixture");
    sqlx::query(
        "UPDATE execution_profiles SET status = 'available', account_state = 'authenticated' WHERE id = $1",
    )
    .bind(seed.execution_profile_id)
    .execute(&mut *fixture)
    .await
    .expect("prepare Execution Profile");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&mut *fixture)
    .await
    .expect("prepare repository");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, project_id, conversation_id, execution_worker_id,
            requesting_actor_id, execution_profile_id, repository_id,
            state, prompt, authorization_snapshot
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'blocked', 'Cross-project approval', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(seed.project_id)
    .bind(seed.thread_id)
    .bind(seed.execution_worker_id)
    .bind(seed.human_actor_id)
    .bind(seed.execution_profile_id)
    .bind(seed.repository_id)
    .execute(&mut *fixture)
    .await
    .expect("insert approval run");
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        ) VALUES ($1, $2, $3, $4, 'question', 'Which branch?', '[]'::jsonb, 'any_authorized', false)
        "#,
    )
    .bind(approval_id)
    .bind(run_id)
    .bind(seed.human_actor_id)
    .bind(format!("cross-project:{approval_id}"))
    .execute(&mut *fixture)
    .await
    .expect("insert approval");
    fixture.commit().await.expect("commit approval fixture");
    let approver_session = initialize_mcp(&application, &approver_token).await;
    let fetched = call_mcp_tool(
        &application,
        &approver_token,
        &approver_session,
        "approval_get",
        json!({"approvalId":approval_id.to_string()}),
    )
    .await;
    assert_eq!(fetched["outcome"], "found", "{fetched}");
    let member_version = response_json(
        application
            .clone()
            .oneshot(read_request(
                &format!("/api/v1/projects/{}/members", seed.project_id),
                &cookie,
            ))
            .await
            .expect("list seed members"),
    )
    .await["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|member| member["actorId"] == approver_actor.as_str())
        .expect("approver membership")["version"]
        .clone();
    let removed = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "PATCH",
            &format!(
                "/api/v1/projects/{}/members/{approver_actor}",
                seed.project_id
            ),
            &cookie,
            &csrf,
            "remove-approver",
            json!({"expectedVersion":member_version,"enabled":false}),
        ))
        .await
        .expect("remove approver from seed project");
    assert_eq!(removed.status(), StatusCode::OK);
    let denied = mcp_request(&application,&approver_token,Some(&approver_session),json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"approval_respond","arguments":{"approvalId":approval_id.to_string(),"expectedVersion":1,"answer":"main","clientRequestId":"cross-project-denied"}}})).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let rejoined = application
        .clone()
        .oneshot(write_request_with_idempotency(
            "PATCH",
            &format!(
                "/api/v1/projects/{}/members/{approver_actor}",
                seed.project_id
            ),
            &cookie,
            &csrf,
            "restore-approver",
            json!({"expectedVersion":member_version.as_i64().expect("version") + 1,"enabled":true}),
        ))
        .await
        .expect("restore approver in seed project");
    assert_eq!(rejoined.status(), StatusCode::OK);
    let answered = call_mcp_tool(
        &application,
        &approver_token,
        &approver_session,
        "approval_respond",
        json!({"approvalId":approval_id.to_string(),"expectedVersion":1,"answer":"main","clientRequestId":"cross-project-answer"}),
    )
    .await;
    assert_eq!(answered["outcome"], "applied", "{answered}");
    terminate_mcp(&application, &approver_token, &approver_session).await;
    drop(application);
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("account fixture operation");
}

#[tokio::test]
async fn workos_link_start_requires_csrf_and_callback_is_bound_to_the_browser() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("workos_browser_{}", Uuid::now_v7().simple());
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
    let mut config = test_config(&database_url, temporary.path());
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
    config.workos = Some(agent_workspace_server::config::WorkOsConfig {
        client_id: "client_test".into(),
        api_key: agent_workspace_server::config::SecretString::new("sk_test"),
        allow_signup: false,
        authkit_domain: None,
        google_enabled: true,
        agent_registration_enabled: false,
        api_base: agent_workspace_server::config::WORKOS_API_BASE.into(),
    });
    let application = test_app(pool.clone(), config);
    let cookie = login(&application).await;
    let csrf = csrf_from_cookie(&cookie);

    let no_csrf = application
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/auth/workos/start",
            &cookie,
            "wrong",
            json!({"link":true,"provider":"GoogleOAuth"}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
    let started = application
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/auth/workos/start",
            &cookie,
            &csrf,
            json!({"link":true,"provider":"GoogleOAuth"}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(started.status(), StatusCode::OK);
    let login_cookie = started
        .headers()
        .get(header::SET_COOKIE)
        .expect("account fixture operation")
        .to_str()
        .expect("account fixture operation")
        .to_owned();
    assert!(login_cookie.contains("HttpOnly") && login_cookie.contains("SameSite=Lax"));
    let started = response_json(started).await;
    let url = url::Url::parse(started["url"].as_str().expect("account fixture operation"))
        .expect("account fixture operation");
    assert_eq!(url.host_str(), Some("api.workos.com"));
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["provider"], "GoogleOAuth");
    assert!(!query.contains_key("client_secret"));
    let callback = format!(
        "/auth/workos/callback?code=test-code&state={}",
        query["state"]
    );
    let wrong_browser = application
        .clone()
        .oneshot(read_request(&callback, &cookie))
        .await
        .expect("account fixture operation");
    assert_eq!(wrong_browser.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        wrong_browser.headers()[header::LOCATION],
        "/sign-in?workosError=1"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workos_login_attempts WHERE link_session_id IS NOT NULL"
        )
        .fetch_one(&pool)
        .await
        .expect("account fixture operation"),
        1,
        "another browser must not consume this login"
    );
    sqlx::query("UPDATE workos_login_attempts SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .expect("account fixture operation");
    let expired = application
        .clone()
        .oneshot(read_request(
            &callback,
            login_cookie
                .split(';')
                .next()
                .expect("account fixture operation"),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(expired.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        expired.headers()[header::LOCATION],
        "/sign-in?workosError=1"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workos_identities")
            .fetch_one(&pool)
            .await
            .expect("account fixture operation"),
        0
    );
    // Cancelling at WorkOS brings an error and no code; a link still returns
    // to Account settings, where the browser is signed in.
    let cancelled = application
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/auth/workos/start",
            &cookie,
            &csrf,
            json!({"link":true}),
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled_cookie = cancelled
        .headers()
        .get(header::SET_COOKIE)
        .expect("account fixture operation")
        .to_str()
        .expect("account fixture operation")
        .split(';')
        .next()
        .expect("account fixture operation")
        .to_owned();
    let cancelled = response_json(cancelled).await;
    let cancelled_url = url::Url::parse(
        cancelled["url"]
            .as_str()
            .expect("account fixture operation"),
    )
    .expect("account fixture operation");
    let cancelled_state = cancelled_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .expect("account fixture operation")
        .1
        .into_owned();
    let denied = application
        .clone()
        .oneshot(read_request(
            &format!("/auth/workos/callback?error=access_denied&state={cancelled_state}"),
            &cancelled_cookie,
        ))
        .await
        .expect("account fixture operation");
    assert_eq!(denied.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        denied.headers()[header::LOCATION],
        "/settings/account/profile?workosError=1"
    );
    drop(application);
    pool.close().await;
    administrative_pool
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("account fixture operation");
}
