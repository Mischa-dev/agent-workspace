//! W14: a script with a personal token does what the app does through the
//! public API; an outgoing webhook hears finished runs and messages, signed,
//! with one id across redeliveries, and never a message its owner can't
//! see; "when a PR's checks fail, launch an agent on it" runs unattended;
//! and a rule or webhook whose maker lost access turns itself off instead of
//! acting for them.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    domain::RunState,
    git_runtime::expected_repository_mirror_path,
    jobs::{self, WorkerOptions},
    launch,
    outbox::{self, AgentRunUpdatedPayload, OutboxEventPayload},
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::{RealtimeHub, publish_ready_outbox_batch},
    router,
    secrets::{self, FileKeyring},
    seed::dev_seed,
};
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Executor, PgPool};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    m3_scheduler_http::{
        create_test_mirror, integration_pool, response_json, schema_pool, uuid_field,
    },
    w2_harness_platform::{MEMBER_LOGIN, insert_member, login, send, send_get},
};

const OWNER_LOGIN: &str = "w14-owner";
const PASSWORD: &str = "correct horse battery staple";

type Received = Arc<Mutex<Vec<(HeaderMap, String)>>>;

/// A loopback receiver that records every POST.
async fn receiver() -> (String, Received) {
    let received: Received = Arc::default();
    let store = received.clone();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |headers: HeaderMap, body: String| {
            let store = store.clone();
            async move {
                store.lock().expect("receiver lock").push((headers, body));
                StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind receiver");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("receiver address")
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve receiver");
    });
    (origin, received)
}

/// Publishes the outbox and runs due jobs until both are quiet.
async fn pump(state: &AppState) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "w14-test".to_owned(),
    };
    for _ in 0..10 {
        let published = publish_ready_outbox_batch(&state.pool, &state.realtime, "w14-test")
            .await
            .expect("publish outbox");
        let ran = jobs::run_due(state, &registry, &options)
            .await
            .expect("run jobs");
        if published == 0 && ran == 0 {
            return;
        }
    }
}

async fn bearer(
    application: &axum::Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    response_json(
        application
            .clone()
            .oneshot(
                request
                    .body(if method == "GET" {
                        Body::empty()
                    } else {
                        Body::from(serde_json::to_vec(&body).expect("serialize body"))
                    })
                    .expect("build request"),
            )
            .await
            .expect("response"),
    )
    .await
}

fn signature_ok(secret: &str, headers: &HeaderMap, body: &str) -> bool {
    let timestamp = headers["x-workspace-timestamp"]
        .to_str()
        .expect("timestamp");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    headers["x-workspace-signature"]
        .to_str()
        .expect("signature")
        == expected
}

#[tokio::test]
async fn api_webhooks_and_automations_act_for_their_maker_only() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("w14_automations_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let (receiver_origin, received) = receiver().await;
    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    let keys = tempfile::tempdir().expect("key directory");
    let keyring_path = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring_path, &FileKeyring::generate()).expect("keyring");
    let mut config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root
                .join("codex/profiles")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS".to_owned(),
            receiver_origin.clone(),
        ),
    ]))
    .expect("configuration");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let seed = dev_seed(&pool, &config).await.expect("seed");
    sqlx::query(
        "UPDATE repositories SET preparation_state = 'ready', prepared_at = now() WHERE id = $1",
    )
    .bind(seed.repository_id)
    .execute(&pool)
    .await
    .expect("prepare repository");
    let mirror =
        expected_repository_mirror_path(&data_root, seed.repository_id).expect("mirror path");
    create_test_mirror(temporary.path(), &mirror);
    bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: OWNER_LOGIN.to_owned(),
            display_name: "W14 Owner".to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(seed.human_actor_id),
            organization_id: Some(seed.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    insert_member(&pool, seed.organization_id, seed.project_id).await;
    let member_actor = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE handle = 'w2-member'",
    )
    .fetch_one(&pool)
    .await
    .expect("member actor");

    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(64),
        supervisors,
    };
    let application = router(state.clone());
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN).await;
    let (member_cookie, member_csrf) = login(&application, MEMBER_LOGIN).await;
    let owner = (owner_cookie.as_str(), owner_csrf.as_str());
    let member = (member_cookie.as_str(), member_csrf.as_str());
    let project = format!("/api/v1/projects/{}", seed.project_id);

    // A demo account, so launches can start.
    let accounts = format!("{project}/provider-accounts");
    let created = send(
        &application,
        owner,
        "POST",
        &accounts,
        "demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(created.0.is_success(), "account: {}", created.1);
    let account = format!("{accounts}/{}", uuid_field(&created.2, "id"));
    for (path, key, body) in [
        (
            format!("{account}/login/api-key"),
            "demo-login",
            json!({"apiKey": "demo-key-for-tests"}),
        ),
        (format!("{account}/use"), "demo-use", json!({})),
    ] {
        let done = send(&application, owner, "POST", &path, key, body).await;
        assert!(done.0.is_success(), "{path}: {}", done.1);
    }
    let environment = send(
        &application,
        owner,
        "POST",
        &format!("{project}/environments"),
        "environment",
        json!({"name": "Demo here", "harness": "demo"}),
    )
    .await;
    assert!(environment.0.is_success(), "environment: {}", environment.1);

    // ---------------------------------------------------------------------
    // The public API: a personal token does what the app does.
    let issued = send(
        &application,
        owner,
        "POST",
        "/api/v1/account/api-tokens",
        "token",
        json!({"projectId": seed.project_id, "name": "script", "scopes": ["read", "write"], "expiresInDays": 30}),
    )
    .await;
    assert_eq!(issued.0, StatusCode::CREATED, "{}", issued.1);
    let token = issued.2["token"].as_str().expect("token").to_owned();

    let document = bearer(
        &application,
        None,
        "GET",
        "/api/public/v1/openapi.json",
        json!(null),
    )
    .await;
    assert_eq!(document.0, StatusCode::OK);
    for operation in [
        "/task_create",
        "/task_list",
        "/webhook_write",
        "/automation_write",
    ] {
        assert!(
            document.2["paths"][operation]["post"].is_object(),
            "the document has {operation}"
        );
    }
    let refused = bearer(
        &application,
        None,
        "POST",
        "/api/public/v1/task_create",
        json!({"projectId": seed.project_id, "title": "No token"}),
    )
    .await;
    assert_eq!(refused.0, StatusCode::UNAUTHORIZED, "{}", refused.1);
    let unknown = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/no_such_tool",
        json!({}),
    )
    .await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND, "{}", unknown.1);
    let made = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/task_create",
        json!({"projectId": seed.project_id, "title": "Made by a script"}),
    )
    .await;
    assert_eq!(made.0, StatusCode::OK, "{}", made.1);
    assert_eq!(made.2["outcome"], "applied", "{}", made.1);
    let listed = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/task_list",
        json!({"projectId": seed.project_id}),
    )
    .await;
    assert_eq!(listed.0, StatusCode::OK, "{}", listed.1);
    assert!(listed.1.contains("Made by a script"), "{}", listed.1);

    // ---------------------------------------------------------------------
    // Webhooks: the member hears messages and finished runs they can see.
    let hook = send(
        &application,
        member,
        "POST",
        "/api/v1/webhooks",
        "hook",
        json!({
            "name": "Member's hook",
            "url": format!("{receiver_origin}/hook"),
            "eventTypes": ["message.created", "run.finished"],
        }),
    )
    .await;
    assert_eq!(hook.0, StatusCode::CREATED, "{}", hook.1);
    let secret = hook.2["secret"].as_str().expect("secret").to_owned();
    assert!(secret.starts_with("whsec_"));
    let webhook_id = uuid_field(&hook.2["webhook"], "id");
    let listing = send_get(&application, member, "/api/v1/webhooks").await;
    assert!(!listing.1.contains(&secret), "the secret is shown once");
    // Nothing stores the secret in the clear.
    let stored_in_clear = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (SELECT 1 FROM webhook_subscriptions WHERE row_to_json(webhook_subscriptions)::text LIKE '%' || $1 || '%')
            OR EXISTS (SELECT 1 FROM secrets WHERE row_to_json(secrets)::text LIKE '%' || $1 || '%')
        "#,
    )
    .bind(&secret)
    .fetch_one(&pool)
    .await
    .expect("look for the secret");
    assert!(!stored_in_clear);

    let say = |conversation: Uuid, key: &'static str, words: &'static str| {
        let application = application.clone();
        async move {
            let sent = send(
                &application,
                owner,
                "POST",
                &format!("/api/v1/conversations/{conversation}/messages"),
                key,
                json!({"content": {"version": 1, "nodes": [{"type": "text", "text": words}]}}),
            )
            .await;
            assert_eq!(sent.0, StatusCode::CREATED, "{}", sent.1);
        }
    };
    say(seed.channel_id, "public-hello", "Public hello").await;
    // A members-only channel the member isn't in.
    let private = send(
        &application,
        owner,
        "POST",
        &format!("{project}/channels"),
        "private-channel",
        json!({"name": "leadership"}),
    )
    .await;
    assert!(private.0.is_success(), "{}", private.1);
    let private_id = private.2["conversation"]["id"]
        .as_str()
        .or_else(|| private.2["id"].as_str())
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("channel id");
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(private_id)
        .execute(&pool)
        .await
        .expect("make private");
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) VALUES ($1, $2, $3, $3) ON CONFLICT DO NOTHING",
    )
    .bind(seed.project_id)
    .bind(private_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("owner in private channel");
    say(private_id, "secret-plan", "Secret plan").await;

    // A run that finishes.
    let task = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "run-task",
        json!({"title": "Run me"}),
    )
    .await;
    assert_eq!(task.0, StatusCode::CREATED, "{}", task.1);
    let run_task = uuid_field(&task.2, "id");
    let launched = send(
        &application,
        owner,
        "POST",
        &format!("{project}/launches"),
        "launch-run-task",
        json!({"workItemIds": [run_task]}),
    )
    .await;
    assert_eq!(launched.0, StatusCode::CREATED, "{}", launched.1);
    launch::drain(&state).await.expect("drain");
    let (run_id, conversation_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT run.id, run.conversation_id FROM launches launch
        JOIN agent_runs run ON run.id = launch.agent_run_id
        WHERE launch.work_item_id = $1
        "#,
    )
    .bind(run_task)
    .fetch_one(&pool)
    .await
    .expect("the run");
    let mut transaction = pool.begin().await.expect("finish run");
    sqlx::query("UPDATE agent_runs SET state = 'completed', finished_at = now() WHERE id = $1")
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        .expect("complete run");
    outbox::enqueue_typed(
        &mut transaction,
        seed.project_id,
        OutboxEventPayload::AgentRunUpdated(AgentRunUpdatedPayload {
            conversation_id,
            agent_run_id: run_id,
            state: Some(RunState::Completed),
        }),
    )
    .await
    .expect("run event");
    transaction.commit().await.expect("commit run");
    pump(&state).await;

    let deliveries = received.lock().expect("received").clone();
    assert!(
        deliveries
            .iter()
            .all(|(_, body)| !body.contains("Secret plan")),
        "a message its owner can't see was delivered"
    );
    let (message_headers, message_body) = deliveries
        .iter()
        .find(|(headers, _)| headers["x-workspace-event"] == "message.created")
        .expect("the public message was delivered");
    assert!(message_body.contains("Public hello"), "{message_body}");
    assert!(signature_ok(&secret, message_headers, message_body));
    let finished = deliveries
        .iter()
        .find(|(headers, _)| headers["x-workspace-event"] == "run.finished")
        .expect("the finished run was delivered");
    assert!(finished.1.contains(&run_id.to_string()), "{}", finished.1);
    assert!(signature_ok(&secret, &finished.0, &finished.1));
    // Redelivering keeps the delivery's id.
    let delivery_id = message_headers["x-workspace-delivery"]
        .to_str()
        .expect("delivery id")
        .to_owned();
    let log = send_get(
        &application,
        member,
        &format!("/api/v1/webhooks/{webhook_id}/deliveries"),
    )
    .await;
    assert_eq!(log.0, StatusCode::OK, "{}", log.1);
    assert!(log.1.contains(&delivery_id), "{}", log.1);
    assert!(!log.1.contains("Secret plan"));
    let again = send(
        &application,
        member,
        "POST",
        &format!("/api/v1/webhooks/{webhook_id}/deliveries/{delivery_id}/redeliver"),
        "redeliver",
        json!({}),
    )
    .await;
    assert!(again.0.is_success(), "{}", again.1);
    pump(&state).await;
    let same_id = received
        .lock()
        .expect("received")
        .iter()
        .filter(|(headers, _)| headers["x-workspace-delivery"] == delivery_id.as_str())
        .count();
    assert_eq!(same_id, 2, "the redelivery carries the same id");

    // ---------------------------------------------------------------------
    // "When a PR's checks fail, launch an agent on it", made by a script.
    let rule = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/automation_write",
        json!({
            "projectId": seed.project_id,
            "operation": "create",
            "name": "Fix failing checks",
            "trigger": {"kind": "pull_request_checks_failed"},
            "actions": [{"kind": "launch"}],
        }),
    )
    .await;
    assert_eq!(rule.0, StatusCode::OK, "{}", rule.1);
    assert_eq!(rule.2["outcome"], "ok", "{}", rule.1);
    let rule_id = rule.2["value"]["id"].as_str().expect("rule id").to_owned();
    let fix = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "fix-task",
        json!({"title": "Offline mode"}),
    )
    .await;
    assert_eq!(fix.0, StatusCode::CREATED, "{}", fix.1);
    let fix_task = uuid_field(&fix.2, "id");
    seed_failing_pull_request(
        &pool,
        seed.project_id,
        seed.repository_id,
        fix_task,
        seed.human_actor_id,
    )
    .await;
    // The checks result reaches the task as a change, as GitHub sync does.
    let touched = send(
        &application,
        owner,
        "PATCH",
        &format!("{project}/work-items/{fix_task}"),
        "touch-fix",
        json!({"expectedVersion": fix.2["version"], "priority": "high"}),
    )
    .await;
    assert_eq!(touched.0, StatusCode::OK, "{}", touched.1);
    pump(&state).await;
    let (source, launch_state) = sqlx::query_as::<_, (String, String)>(
        "SELECT source, state FROM launches WHERE work_item_id = $1",
    )
    .bind(fix_task)
    .fetch_one(&pool)
    .await
    .expect("the automation launched the task");
    assert_eq!(source, "automation");
    assert!(
        matches!(launch_state.as_str(), "queued" | "started"),
        "{launch_state}"
    );
    let history = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/automation_runs",
        json!({"projectId": seed.project_id, "ruleId": rule_id}),
    )
    .await;
    assert_eq!(history.2["outcome"], "ok", "{}", history.1);
    assert_eq!(
        history.2["value"]["runs"][0]["state"], "succeeded",
        "{}",
        history.1
    );
    launch::drain(&state).await.expect("drain");
    let prompt = sqlx::query_scalar::<_, String>(
        r#"
        SELECT run.prompt FROM launches launch JOIN agent_runs run ON run.id = launch.agent_run_id
        WHERE launch.work_item_id = $1
        "#,
    )
    .bind(fix_task)
    .fetch_one(&pool)
    .await
    .expect("the launched run");
    assert!(
        prompt.contains("Checks failed on pull request #7"),
        "{prompt}"
    );
    // The same failure seen again doesn't launch twice.
    let touched_again = send(
        &application,
        owner,
        "PATCH",
        &format!("{project}/work-items/{fix_task}"),
        "touch-fix-again",
        json!({"expectedVersion": touched.2["version"], "priority": "urgent"}),
    )
    .await;
    assert_eq!(touched_again.0, StatusCode::OK, "{}", touched_again.1);
    pump(&state).await;
    let launches =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM launches WHERE work_item_id = $1")
            .bind(fix_task)
            .fetch_one(&pool)
            .await
            .expect("count launches");
    assert_eq!(launches, 1);

    // ---------------------------------------------------------------------
    // A rule made by a script keeps the script's agent limit when it fires
    // later (AWD-74): narrowing what the owner's apps may do turns it off
    // rather than letting it act with the owner's full role.
    let limited_rule = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/automation_write",
        json!({
            "projectId": seed.project_id,
            "operation": "create",
            "name": "Raise everything",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "set_field", "field": "priority", "value": "urgent"}],
        }),
    )
    .await;
    assert_eq!(limited_rule.2["outcome"], "ok", "{}", limited_rule.1);
    let limited_rule_id = uuid_field(&limited_rule.2["value"], "id");
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.view'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("narrow the owner's apps");
    let watched = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "limited-task",
        json!({"title": "Made in the browser"}),
    )
    .await;
    assert_eq!(watched.0, StatusCode::CREATED, "{}", watched.1);
    pump(&state).await;
    let priority = sqlx::query_scalar::<_, String>("SELECT priority FROM work_items WHERE id = $1")
        .bind(uuid_field(&watched.2, "id"))
        .fetch_one(&pool)
        .await
        .expect("watched task");
    assert_ne!(priority, "urgent", "the rule acted beyond its app's limit");
    let (enabled, reason) = sqlx::query_as::<_, (bool, Option<String>)>(
        "SELECT enabled, disabled_reason FROM automation_rules WHERE id = $1",
    )
    .bind(limited_rule_id)
    .fetch_one(&pool)
    .await
    .expect("limited rule");
    assert!(!enabled, "{reason:?}");
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit");

    // ---------------------------------------------------------------------
    // A webhook made by a script delivers only what the script's agent
    // limit lets it see when the event happens, and a message it scheduled
    // goes out only if the limit still lets it post (AWD-74).
    let limited_hook = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/webhook_write",
        json!({
            "operation": "create",
            "name": "Script's task hook",
            "url": format!("{receiver_origin}/script-hook"),
            "eventTypes": ["task.created"],
            "projectId": seed.project_id,
        }),
    )
    .await;
    assert_eq!(limited_hook.2["outcome"], "ok", "{}", limited_hook.1);
    let limited_hook_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM webhook_subscriptions WHERE name = 'Script''s task hook'",
    )
    .fetch_one(&pool)
    .await
    .expect("script's hook");
    let hook_deliveries = move |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webhook_deliveries WHERE subscription_id = $1",
        )
        .bind(limited_hook_id)
        .fetch_one(&pool)
        .await
        .expect("count deliveries")
    };
    let seen = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "hook-seen-task",
        json!({"title": "Seen by the script"}),
    )
    .await;
    assert_eq!(seen.0, StatusCode::CREATED, "{}", seen.1);
    pump(&state).await;
    let before = hook_deliveries(pool.clone()).await;
    assert!(before >= 1, "the script's hook heard a task it may see");
    let scheduled = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/message_schedule",
        json!({
            "conversationId": seed.channel_id,
            "text": "Posted by the script later",
            "sendAt": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        }),
    )
    .await;
    assert_eq!(scheduled.2["outcome"], "ok", "{}", scheduled.1);
    let scheduled_id = scheduled.2["value"]["scheduled"]["id"]
        .as_str()
        .expect("scheduled id")
        .to_owned();
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['runs.launch'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("the owner's apps no longer see tasks or post");
    let hidden = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "hook-hidden-task",
        json!({"title": "Hidden from the script"}),
    )
    .await;
    assert_eq!(hidden.0, StatusCode::CREATED, "{}", hidden.1);
    pump(&state).await;
    assert_eq!(
        hook_deliveries(pool.clone()).await,
        before,
        "the script's hook heard a task its limit hides"
    );
    let sent = send(
        &application,
        owner,
        "POST",
        &format!("/api/v1/scheduled-messages/{scheduled_id}/send-now"),
        "script-message-now",
        json!({}),
    )
    .await;
    assert!(sent.0.is_success(), "{}", sent.1);
    let (status, message) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT status, message_id FROM scheduled_messages WHERE id = $1::uuid",
    )
    .bind(&scheduled_id)
    .fetch_one(&pool)
    .await
    .expect("scheduled message");
    assert_eq!(status, "failed", "it went out beyond its app's limit");
    assert!(message.is_none());
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit again");

    // A launch an app-made automation queued starts only within the app's
    // limit as it is when the queue drains (AWD-74).
    let launch_rule = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/automation_write",
        json!({
            "projectId": seed.project_id,
            "operation": "create",
            "name": "Launch new tasks",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "launch"}],
        }),
    )
    .await;
    assert_eq!(launch_rule.2["outcome"], "ok", "{}", launch_rule.1);
    let launch_rule_id = uuid_field(&launch_rule.2["value"], "id");
    let launched = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "rule-launched-task",
        json!({"title": "Launched by the script's rule"}),
    )
    .await;
    assert_eq!(launched.0, StatusCode::CREATED, "{}", launched.1);
    pump(&state).await;
    let marked = sqlx::query_scalar::<_, bool>(
        "SELECT made_through_app FROM launches WHERE work_item_id = $1",
    )
    .bind(uuid_field(&launched.2, "id"))
    .fetch_one(&pool)
    .await
    .expect("the rule's launch");
    assert!(marked, "the app-made rule's launch keeps the app's limit");
    sqlx::query("UPDATE automation_rules SET enabled = false WHERE id = $1")
        .bind(launch_rule_id)
        .execute(&pool)
        .await
        .expect("turn the rule off");
    // One such launch waiting in the queue when the limit narrows.
    let waiting_task = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "rule-queued-task",
        json!({"title": "Queued by the script's rule"}),
    )
    .await;
    assert_eq!(waiting_task.0, StatusCode::CREATED, "{}", waiting_task.1);
    let waiting = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO launches (
            id, organization_id, project_id, work_item_id, requested_by_actor_id,
            source, start_when, position, made_through_app
        ) VALUES ($1, $2, $3, $4, $5, 'automation', 'now',
                  (SELECT COALESCE(max(position), 0) + 1 FROM launches WHERE project_id = $3),
                  true)
        "#,
    )
    .bind(waiting)
    .bind(seed.organization_id)
    .bind(seed.project_id)
    .bind(uuid_field(&waiting_task.2, "id"))
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("queued launch");
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.view', 'tasks.edit'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("the owner's apps no longer launch runs");
    launch::drain(&state).await.expect("drain");
    let launch_state = sqlx::query_scalar::<_, String>("SELECT state FROM launches WHERE id = $1")
        .bind(waiting)
        .fetch_one(&pool)
        .await
        .expect("queued launch");
    assert_eq!(
        launch_state, "cancelled",
        "it started beyond its app's limit"
    );
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit a third time");

    // @everyone in a channel of more than 50 people reaches everyone only
    // if its author may moderate, the script's agent limit included, as it
    // stood when the message was sent (AWD-74).
    for index in 0..52 {
        let person = Uuid::now_v7();
        let handle = format!("crowd-{index}");
        sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
            .bind(person)
            .execute(&pool)
            .await
            .expect("crowd actor");
        // A person needs an active user account to be reached at all
        // (actor_can_access_project).
        sqlx::query(
            "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(person)
        .bind(&handle)
        .execute(&pool)
        .await
        .expect("crowd user");
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(seed.organization_id)
        .bind(person)
        .execute(&pool)
        .await
        .expect("crowd workspace membership");
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $4)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(seed.project_id)
        .bind(person)
        .bind(&handle)
        .execute(&pool)
        .await
        .expect("crowd project membership");
    }
    let notified = |pool: PgPool, message: String| async move {
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*) FROM notifications
            WHERE message_id = $1::uuid AND recipient_actor_id IN (
                SELECT actor_id FROM project_memberships WHERE handle LIKE 'crowd-%'
            )
            "#,
        )
        .bind(message)
        .fetch_one(&pool)
        .await
        .expect("count notifications")
    };
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['channels.post', 'tasks.view'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("the owner's apps no longer moderate");
    let limited = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/message_send",
        json!({"conversationId": seed.channel_id, "text": "@everyone from the script, limited", "clientRequestId": "shout-limited"}),
    )
    .await;
    assert_eq!(limited.2["outcome"], "ok", "{}", limited.1);
    let limited_id = limited.2["value"]["messageId"]
        .as_str()
        .expect("message id")
        .to_owned();
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit a fourth time");
    // Lifting the limit afterwards doesn't widen a message already sent.
    pump(&state).await;
    assert_eq!(
        notified(pool.clone(), limited_id).await,
        0,
        "@everyone reached everyone beyond the script's limit"
    );
    let open = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/message_send",
        json!({"conversationId": seed.channel_id, "text": "@everyone from the script, unlimited", "clientRequestId": "shout-open"}),
    )
    .await;
    assert_eq!(open.2["outcome"], "ok", "{}", open.1);
    let open_id = open.2["value"]["messageId"]
        .as_str()
        .expect("message id")
        .to_owned();
    pump(&state).await;
    assert!(
        notified(pool.clone(), open_id).await == 52,
        "the owner's @everyone reaches everyone"
    );

    // A rule a script made skips what the script's current limit hides: it
    // doesn't post a task's title once the limit leaves out "see tasks".
    let title_rule = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/automation_write",
        json!({
            "projectId": seed.project_id,
            "operation": "create",
            "name": "Announce new tasks",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "post_message", "conversationId": seed.channel_id, "text": "New: {{task.title}}"}],
        }),
    )
    .await;
    assert_eq!(title_rule.2["outcome"], "ok", "{}", title_rule.1);
    let title_rule_id = uuid_field(&title_rule.2["value"], "id");
    let posted = move |pool: PgPool, title: &'static str| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE conversation_id = $1 AND plain_text LIKE $2",
        )
        .bind(seed.channel_id)
        .bind(format!("%{title}%"))
        .fetch_one(&pool)
        .await
        .expect("count messages")
    };
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.edit', 'channels.post'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("the owner's apps no longer see tasks");
    let hidden_task = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "title-hidden-task",
        json!({"title": "Quiet launch plan"}),
    )
    .await;
    assert_eq!(hidden_task.0, StatusCode::CREATED, "{}", hidden_task.1);
    pump(&state).await;
    assert_eq!(
        posted(pool.clone(), "Quiet launch plan").await,
        0,
        "the script's rule posted a title its limit hides"
    );
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit a fifth time");
    let shown_task = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "title-shown-task",
        json!({"title": "Open roadmap"}),
    )
    .await;
    assert_eq!(shown_task.0, StatusCode::CREATED, "{}", shown_task.1);
    pump(&state).await;
    assert_eq!(
        posted(pool.clone(), "Open roadmap").await,
        1,
        "the script's rule posts the title it may see"
    );
    sqlx::query("UPDATE automation_rules SET enabled = false WHERE id = $1")
        .bind(title_rule_id)
        .execute(&pool)
        .await
        .expect("turn the rule off");

    // Several tasks a script launches go through the queue (source "queue")
    // and still keep the script's limit when the queue drains (AWD-74).
    let blocker = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "queue-blocker",
        json!({"title": "Blocks the batch"}),
    )
    .await;
    assert_eq!(blocker.0, StatusCode::CREATED, "{}", blocker.1);
    let mut queued_tasks = Vec::new();
    for title in ["Queued one", "Queued two"] {
        let made = send(
            &application,
            owner,
            "POST",
            &format!("{project}/work-items"),
            &format!("queue-{}", title.replace(' ', "-")),
            json!({"title": title}),
        )
        .await;
        assert_eq!(made.0, StatusCode::CREATED, "{}", made.1);
        let task = uuid_field(&made.2, "id");
        // An open blocker keeps it waiting in the queue after a drain.
        sqlx::query(
            r#"
            INSERT INTO work_item_dependencies (
                project_id, blocked_work_item_id, blocking_project_id, blocking_work_item_id,
                created_by_actor_id
            ) VALUES ($1, $2, $1, $3, $4)
            "#,
        )
        .bind(seed.project_id)
        .bind(task)
        .bind(uuid_field(&blocker.2, "id"))
        .bind(seed.human_actor_id)
        .execute(&pool)
        .await
        .expect("blocked by the blocker");
        queued_tasks.push(task);
    }
    let queued = bearer(
        &application,
        Some(&token),
        "POST",
        "/api/public/v1/task_launch",
        json!({"projectId": seed.project_id, "taskIds": queued_tasks}),
    )
    .await;
    assert_eq!(queued.2["outcome"], "ok", "{}", queued.1);
    pump(&state).await;
    let launches = sqlx::query_as::<_, (String, String, bool)>(
        "SELECT source, state, made_through_app FROM launches WHERE work_item_id = ANY($1)",
    )
    .bind(&queued_tasks)
    .fetch_all(&pool)
    .await
    .expect("queued launches");
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches
            .iter()
            .all(|(_, state, marked)| state == "queued" && *marked),
        "{launches:?}"
    );
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = ARRAY['tasks.view', 'tasks.edit'] WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("the owner's apps no longer launch runs");
    launch::drain(&state).await.expect("drain");
    let states =
        sqlx::query_scalar::<_, String>("SELECT state FROM launches WHERE work_item_id = ANY($1)")
            .bind(&queued_tasks)
            .fetch_all(&pool)
            .await
            .expect("drained launches");
    assert!(
        states.iter().all(|state| state == "cancelled"),
        "they started beyond the script's limit: {states:?}"
    );
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = NULL WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(seed.organization_id)
    .bind(seed.human_actor_id)
    .execute(&pool)
    .await
    .expect("lift the limit a sixth time");

    // ---------------------------------------------------------------------
    // A rule whose maker lost the project turns off instead of acting.
    let members_rule = send(
        &application,
        member,
        "POST",
        &format!("{project}/automations"),
        "members-rule",
        json!({
            "name": "Raise new tasks",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "set_field", "field": "priority", "value": "urgent"}],
        }),
    )
    .await;
    assert_eq!(members_rule.0, StatusCode::CREATED, "{}", members_rule.1);
    let members_rule_id = uuid_field(&members_rule.2, "id");
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(seed.project_id)
    .bind(member_actor)
    .execute(&pool)
    .await
    .expect("remove member from the project");
    let later = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "later-task",
        json!({"title": "After they left"}),
    )
    .await;
    assert_eq!(later.0, StatusCode::CREATED, "{}", later.1);
    let later_task = uuid_field(&later.2, "id");
    pump(&state).await;
    let (enabled, reason) = sqlx::query_as::<_, (bool, Option<String>)>(
        "SELECT enabled, disabled_reason FROM automation_rules WHERE id = $1",
    )
    .bind(members_rule_id)
    .fetch_one(&pool)
    .await
    .expect("member's rule");
    assert!(!enabled, "a rule whose maker left kept running");
    assert!(
        reason
            .as_deref()
            .unwrap_or("")
            .contains("can no longer work in this project"),
        "{reason:?}"
    );
    let priority = sqlx::query_scalar::<_, String>("SELECT priority FROM work_items WHERE id = $1")
        .bind(later_task)
        .fetch_one(&pool)
        .await
        .expect("later task");
    assert_ne!(priority, "urgent", "the rule acted for someone who left");
    // The maker hears why, in their own space.
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'automation_disabled'",
    )
    .bind(member_actor)
    .fetch_one(&pool)
    .await
    .expect("count notices");
    assert_eq!(told, 1);

    // Their webhook stops hearing the project, and turns off when they
    // leave the workspace.
    let before = received.lock().expect("received").len();
    say(seed.channel_id, "after-leaving", "After leaving").await;
    pump(&state).await;
    assert_eq!(received.lock().expect("received").len(), before);
    sqlx::query("UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2")
        .bind(seed.organization_id)
        .bind(member_actor)
        .execute(&pool)
        .await
        .expect("remove member from the workspace");
    say(seed.channel_id, "after-workspace", "After the workspace").await;
    pump(&state).await;
    let (hook_state, hook_reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, disabled_reason FROM webhook_subscriptions WHERE id = $1",
    )
    .bind(webhook_id)
    .fetch_one(&pool)
    .await
    .expect("member's webhook");
    assert_eq!(hook_state, "disabled");
    assert!(
        hook_reason
            .unwrap_or_default()
            .contains("no longer in this workspace")
    );
    assert_eq!(received.lock().expect("received").len(), before);

    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop schema");
}

/// A pull request linked to the task, with failing checks.
async fn seed_failing_pull_request(
    pool: &PgPool,
    project_id: Uuid,
    repository_id: Uuid,
    work_item_id: Uuid,
    linked_by: Uuid,
) -> Uuid {
    let integration = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(integration)
        .execute(pool)
        .await
        .expect("integration actor");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) VALUES ($1, $2, $3, 'member', 'GitHub', 'github_integration')",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(integration)
    .execute(pool)
    .await
    .expect("integration membership");
    let installation = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO github_installations (
            id, project_id, integration_actor_id, app_id, installation_id,
            account_id, account_login, target_type, repository_selection,
            metadata_permission, contents_permission, pull_requests_permission,
            checks_permission, subscribed_events, state, enabled, last_success_at
        ) VALUES (
            $1, $2, $3, 42, 4242, 777, 'Mischa-dev', 'Organization', 'selected',
            'read', 'write', 'write', 'read',
            ARRAY['push', 'pull_request', 'pull_request_review', 'check_suite', 'check_run', 'repository'],
            'available', true, now()
        )
        "#,
    )
    .bind(installation)
    .bind(project_id)
    .bind(integration)
    .execute(pool)
    .await
    .expect("installation");
    let (owner, name, github_repository_id) = sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT owner, name, github_repository_id FROM repositories WHERE id = $1",
    )
    .bind(repository_id)
    .fetch_one(pool)
    .await
    .expect("repository");
    let pull_request = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id, github_repository_id,
            pull_request_number, node_id, html_url, head_ref, head_sha, base_ref, title,
            author_login, draft, state, external_created_at, external_updated_at, checks_state
        ) VALUES (
            $1, $2, $3, $4, $5, 7, $6, $7, 'offline-mode', repeat('b', 40), 'main',
            'Offline mode', 'mischa', false, 'open', now(), now(), 'failure'
        )
        "#,
    )
    .bind(pull_request)
    .bind(project_id)
    .bind(repository_id)
    .bind(installation)
    .bind(github_repository_id.unwrap_or(99))
    .bind(format!("PR_{pull_request}"))
    .bind(format!("https://github.com/{owner}/{name}/pull/7"))
    .execute(pool)
    .await
    .expect("pull request");
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_work_item_links (
            project_id, github_pull_request_id, work_item_id, source, confirmed, created_by_actor_id
        ) VALUES ($1, $2, $3, 'explicit', true, $4)
        "#,
    )
    .bind(project_id)
    .bind(pull_request)
    .bind(work_item_id)
    .bind(linked_by)
    .execute(pool)
    .await
    .expect("link pull request");
    pull_request
}
