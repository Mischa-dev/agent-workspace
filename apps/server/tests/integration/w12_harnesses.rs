//! W12: your harnesses. Only managers make agents and register harnesses;
//! an agent made from the Reviewer template gets its own handle, and
//! tagging it launches a run on its own worker with its instructions; its
//! presence shows the work; turning it off frees the handle and stops tags.
//! A registered harness's secrets never include the server's own keys.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    git_runtime::expected_repository_mirror_path,
    harness_host::ServerHost,
    jobs::{self, WorkerOptions},
    provider::{SupervisorRegistry, host::HarnessHost},
    rate_limit::RateLimiters,
    realtime::{RealtimeHub, publish_ready_outbox_batch},
    router,
    secrets::{self, FileKeyring, SecretValue},
    seed::dev_seed,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::Executor;
use uuid::Uuid;

use crate::{
    m3_scheduler_http::{create_test_mirror, integration_pool, schema_pool, uuid_field},
    w2_harness_platform::{MEMBER_LOGIN, insert_member, login, send, send_get},
};

const OWNER_LOGIN: &str = "w12-harness-owner";
const PASSWORD: &str = "correct horse battery staple";

fn options() -> WorkerOptions {
    WorkerOptions {
        concurrency: 1,
        poll_interval: Duration::from_millis(20),
        lease: Duration::from_secs(30),
        worker_id: "w12-test".to_owned(),
    }
}

#[tokio::test]
async fn agents_are_made_by_managers_tagged_into_runs_and_turned_off() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("w12_harnesses_{}", Uuid::now_v7().simple());
    administrative_pool
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    let keys = tempfile::tempdir().expect("key directory");
    let keyring = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring, &FileKeyring::generate()).expect("keyring");
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
            keyring.to_string_lossy().into_owned(),
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
            display_name: "W12 Owner".to_owned(),
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

    let supervisors = SupervisorRegistry::new(&config).expect("supervisor registry");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    };
    let application = router(state.clone());
    let (owner_cookie, owner_csrf) = login(&application, OWNER_LOGIN).await;
    let (member_cookie, member_csrf) = login(&application, MEMBER_LOGIN).await;
    let owner = (owner_cookie.as_str(), owner_csrf.as_str());
    let member = (member_cookie.as_str(), member_csrf.as_str());
    let project = format!("/api/v1/projects/{}", seed.project_id);

    // A demo account the project's runs use.
    let accounts = format!("{project}/provider-accounts");
    let created = send(
        &application,
        owner,
        "POST",
        &accounts,
        "create-demo",
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(created.0.is_success(), "create account: {}", created.1);
    let account = format!("{accounts}/{}", uuid_field(&created.2, "id"));
    for (path, key, body) in [
        (
            format!("{account}/login/api-key"),
            "demo-sign-in",
            json!({"apiKey": "demo-key-for-tests"}),
        ),
        (format!("{account}/use"), "use-demo", json!({})),
    ] {
        let response = send(&application, owner, "POST", &path, key, body).await;
        assert!(response.0.is_success(), "{path}: {}", response.1);
    }

    // Only managers make agents.
    let profiles = format!("{project}/agent-profiles");
    let refused = send(
        &application,
        member,
        "POST",
        &profiles,
        "member-reviewer",
        json!({"template": "reviewer"}),
    )
    .await;
    assert!(
        matches!(refused.0, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a member made an agent: {}",
        refused.0
    );
    let listed = send_get(&application, member, &profiles).await;
    assert_eq!(listed.0, StatusCode::OK, "{}", listed.1);
    assert_eq!(listed.2["canManage"], false);
    assert_eq!(listed.2["ready"], true);
    assert_eq!(listed.2["templates"].as_array().map(Vec::len), Some(3));

    let reviewer = send(
        &application,
        owner,
        "POST",
        &profiles,
        "reviewer",
        json!({"template": "reviewer"}),
    )
    .await;
    assert_eq!(reviewer.0, StatusCode::OK, "{}", reviewer.1);
    assert_eq!(reviewer.2["handle"], "reviewer");
    assert_eq!(reviewer.2["name"], "Reviewer");
    assert!(
        reviewer.2["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("code reviewer"))
    );
    let profile_id = uuid_field(&reviewer.2, "id");
    let reviewer_actor = uuid_field(&reviewer.2, "actorId");
    let second = send(
        &application,
        owner,
        "POST",
        &profiles,
        "reviewer-again",
        json!({"template": "reviewer"}),
    )
    .await;
    assert_eq!(second.0, StatusCode::OK, "{}", second.1);
    assert_eq!(
        second.2["handle"], "reviewer-2",
        "a taken handle gets a number"
    );
    let unknown = send(
        &application,
        owner,
        "POST",
        &profiles,
        "unknown-harness",
        json!({"name": "Ghost", "harness": "x_not_registered"}),
    )
    .await;
    assert!(
        unknown.0.is_client_error(),
        "an unknown harness was accepted: {}",
        unknown.1
    );

    // Tagging the Reviewer in a channel launches a run on its own worker,
    // in a thread under the message, with its instructions.
    let channel = seed.channel_id;
    let sent = send(
        &application,
        owner,
        "POST",
        &format!("/api/v1/conversations/{channel}/messages"),
        "tag-reviewer",
        json!({"content": {"version": 1, "nodes": [
            {"type": "mention", "actorId": reviewer_actor},
            {"type": "text", "text": " look over the login change"}
        ]}}),
    )
    .await;
    assert_eq!(sent.0, StatusCode::CREATED, "{}", sent.1);
    jobs::run_due(&state, &jobs::default_registry(), &options())
        .await
        .expect("run the tag job");
    let (launch_profile, launch_state, run_id) =
        sqlx::query_as::<_, (Option<Uuid>, String, Option<Uuid>)>(
            "SELECT agent_profile_id, state, agent_run_id FROM launches WHERE source = 'tag'",
        )
        .fetch_one(&pool)
        .await
        .expect("tag launch");
    assert_eq!(launch_profile, Some(profile_id));
    assert_eq!(launch_state, "started");
    let run_id = run_id.expect("started run");
    let (worker_actor, prompt) = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT worker.actor_id, run.prompt FROM agent_runs run
        JOIN execution_workers worker ON worker.id = run.execution_worker_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("run");
    assert_eq!(worker_actor, reviewer_actor, "the run is the Reviewer's");
    assert!(prompt.contains("code reviewer") && prompt.contains("look over the login change"));

    // Presence and the run page show the work, to a member too.
    let presence = send_get(&application, member, &format!("{project}/agents")).await;
    assert_eq!(presence.0, StatusCode::OK, "{}", presence.1);
    let agent = presence.2["agents"]
        .as_array()
        .and_then(|agents| {
            agents
                .iter()
                .find(|agent| agent["actorId"] == json!(reviewer_actor))
        })
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        matches!(agent["state"].as_str(), Some("working" | "waiting")),
        "{agent}"
    );
    assert_eq!(agent["runId"], json!(run_id));
    let run = send_get(
        &application,
        member,
        &format!("/api/v1/agent-runs/{run_id}"),
    )
    .await;
    assert_eq!(run.0, StatusCode::OK, "{}", run.1);
    assert_eq!(run.2["agentProfileId"], json!(profile_id));
    assert_eq!(run.2["agentName"], "Reviewer");
    let runs = send_get(&application, member, "/api/v1/runs?status=active").await;
    assert!(
        runs.2["runs"]
            .as_array()
            .is_some_and(|runs| runs.iter().any(|run| run["id"] == json!(run_id))),
        "{}",
        runs.1
    );
    // Tokens a run used with no provider account are their own row in the
    // usage by account, not counted as someone else's account.
    sqlx::query(
        r#"
        INSERT INTO agent_run_usage (
            agent_run_id, model, organization_id, project_id, provider_account_id,
            adapter_kind, usage_date, input_tokens, output_tokens
        )
        VALUES ($1, 'w12-no-account', $2, $3, NULL, 'codex', current_date, 120, 30)
        "#,
    )
    .bind(run_id)
    .bind(seed.organization_id)
    .bind(seed.project_id)
    .execute(&pool)
    .await
    .expect("usage without an account");
    let usage = send_get(&application, member, "/api/v1/runs/usage?groupBy=account").await;
    assert_eq!(usage.0, StatusCode::OK, "{}", usage.1);
    let rows = usage.2["rows"].as_array().cloned().unwrap_or_default();
    let none = rows
        .iter()
        .find(|row| row["key"] == "none")
        .unwrap_or_else(|| panic!("a no-account row: {}", usage.1));
    assert_eq!(none["label"], "No account");
    assert!(none["inputTokens"].as_i64().unwrap_or(0) >= 120, "{none}");
    assert!(
        rows.iter()
            .all(|row| row["key"] != "others" || row["label"] == "Other people's accounts"),
        "{}",
        usage.1
    );

    // An automation launches as the agent; another project's agent, or one
    // that doesn't exist, is refused when the rule is saved.
    let automations = format!("{project}/automations");
    let unknown = send(
        &application,
        owner,
        "POST",
        &automations,
        "rule-unknown-agent",
        json!({
            "name": "Review new tasks",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "launch", "agentProfileId": Uuid::now_v7()}],
        }),
    )
    .await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND, "{}", unknown.1);
    let rule = send(
        &application,
        owner,
        "POST",
        &automations,
        "rule-reviewer",
        json!({
            "name": "Review new tasks",
            "trigger": {"kind": "task_created"},
            "actions": [{"kind": "launch", "agentProfileId": profile_id}],
        }),
    )
    .await;
    assert_eq!(rule.0, StatusCode::CREATED, "{}", rule.1);
    let task = send(
        &application,
        owner,
        "POST",
        &format!("{project}/work-items"),
        "rule-task",
        json!({"title": "Review the parser"}),
    )
    .await;
    assert_eq!(task.0, StatusCode::CREATED, "{}", task.1);
    for _ in 0..10 {
        let published = publish_ready_outbox_batch(&state.pool, &state.realtime, "w12-test")
            .await
            .expect("publish outbox");
        let ran = jobs::run_due(&state, &jobs::default_registry(), &options())
            .await
            .expect("run jobs");
        if published == 0 && ran == 0 {
            break;
        }
    }
    let (rule_profile, rule_source) = sqlx::query_as::<_, (Option<Uuid>, String)>(
        "SELECT agent_profile_id, source FROM launches WHERE work_item_id = $1",
    )
    .bind(uuid_field(&task.2, "id"))
    .fetch_one(&pool)
    .await
    .expect("the rule launched the task");
    assert_eq!(rule_source, "automation");
    assert_eq!(rule_profile, Some(profile_id));
    let rule_off = send(
        &application,
        owner,
        "PATCH",
        &format!("{automations}/{}", uuid_field(&rule.2, "id")),
        "rule-off",
        json!({"expectedVersion": rule.2["version"], "enabled": false}),
    )
    .await;
    assert_eq!(rule_off.0, StatusCode::OK, "{}", rule_off.1);

    // Turning it off frees the handle and stops tags.
    let off = send(
        &application,
        owner,
        "PATCH",
        &format!("{profiles}/{profile_id}"),
        "reviewer-off",
        json!({"enabled": false, "version": reviewer.2["version"]}),
    )
    .await;
    assert_eq!(off.0, StatusCode::OK, "{}", off.1);
    assert_eq!(off.2["enabled"], false);
    let stale = send(
        &application,
        owner,
        "PATCH",
        &format!("{profiles}/{profile_id}"),
        "reviewer-stale",
        json!({"name": "Old", "version": reviewer.2["version"]}),
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT, "{}", stale.1);
    let renamed = send(
        &application,
        owner,
        "PATCH",
        &format!("{profiles}/{}", uuid_field(&second.2, "id")),
        "second-takes-handle",
        json!({"handle": "reviewer", "version": second.2["version"]}),
    )
    .await;
    assert_eq!(renamed.0, StatusCode::OK, "{}", renamed.1);
    let launches_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM launches")
        .fetch_one(&pool)
        .await
        .expect("launches");
    let sent = send(
        &application,
        owner,
        "POST",
        &format!("/api/v1/conversations/{channel}/messages"),
        "tag-off-reviewer",
        json!({"content": {"version": 1, "nodes": [
            {"type": "mention", "actorId": reviewer_actor},
            {"type": "text", "text": " again"}
        ]}}),
    )
    .await;
    // A turned-off agent can't be mentioned at all, or its tag is ignored.
    if sent.0 == StatusCode::CREATED {
        jobs::run_due(&state, &jobs::default_registry(), &options())
            .await
            .expect("run jobs");
    }
    let launches_after = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM launches")
        .fetch_one(&pool)
        .await
        .expect("launches");
    assert_eq!(
        launches_after, launches_before,
        "a turned-off agent launched"
    );

    // Harness registration is for workspace managers, and a command harness
    // needs the server owner's allowed folders.
    let harnesses = format!("/api/v1/organizations/{}/harnesses", seed.organization_id);
    let refused = send(
        &application,
        member,
        "POST",
        &harnesses,
        "member-harness",
        json!({"name": "Mine", "transport": "command", "command": ["/bin/true"], "manifest": {"models": []}}),
    )
    .await;
    assert!(
        matches!(refused.0, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a member registered a harness: {}",
        refused.0
    );
    let no_dirs = send(
        &application,
        owner,
        "POST",
        &harnesses,
        "owner-harness",
        json!({"name": "Mine", "transport": "command", "command": ["/bin/true"], "manifest": {"models": []}}),
    )
    .await;
    assert_eq!(no_dirs.0, StatusCode::UNPROCESSABLE_ENTITY, "{}", no_dirs.1);
    assert_eq!(no_dirs.2["error"]["code"], "invalid_harness");
    let listed = send_get(&application, owner, &harnesses).await;
    assert_eq!(listed.0, StatusCode::OK, "{}", listed.1);
    assert_eq!(listed.2["commandsAllowed"], false);
    assert_eq!(listed.2["canManage"], true);

    // A registered harness gets only the owner's secrets it names, never a
    // key the server keeps for itself.
    let host = ServerHost::new(pool.clone(), Arc::clone(&state.config));
    let account_id = uuid_field(&created.2, "id");
    let key = SecretValue::new("sk-test-account-key-0123456789");
    host.store_account_secret(account_id, &key)
        .await
        .expect("store account key");
    assert_eq!(
        host.account_secret(account_id)
            .await
            .expect("read account key")
            .map(|value| value.expose().to_owned()),
        Some("sk-test-account-key-0123456789".to_owned())
    );
    let handed = host
        .harness_secrets(
            account_id,
            &[
                format!("provider_account:{account_id}:key"),
                "missing".to_owned(),
            ],
        )
        .await
        .expect("harness secrets");
    assert!(handed.is_empty(), "the server's own key was handed out");
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_run_events WHERE payload::text LIKE '%sk-test-account-key%'",
    )
    .fetch_one(&pool)
    .await
    .expect("events");
    assert_eq!(stored, 0);
    host.delete_account_secret(account_id)
        .await
        .expect("delete account key");
    assert!(
        host.account_secret(account_id)
            .await
            .expect("read account key")
            .is_none()
    );

    // Deleting an agent: only managers may; one that's working is refused;
    // one that never did anything goes for good, and one with runs is
    // hidden so they keep its name.
    let planner = send(
        &application,
        owner,
        "POST",
        &profiles,
        "planner",
        json!({"template": "planner"}),
    )
    .await;
    assert_eq!(planner.0, StatusCode::OK, "{}", planner.1);
    let planner_path = format!("{profiles}/{}", uuid_field(&planner.2, "id"));
    let refused = send(
        &application,
        member,
        "DELETE",
        &planner_path,
        "member-delete",
        json!(null),
    )
    .await;
    assert!(
        matches!(refused.0, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a member deleted an agent: {}",
        refused.0
    );
    let deleted = send(
        &application,
        owner,
        "DELETE",
        &planner_path,
        "planner-delete",
        json!(null),
    )
    .await;
    assert_eq!(deleted.0, StatusCode::OK, "{}", deleted.1);
    assert_eq!(deleted.2["outcome"], "deleted");
    let left = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors WHERE id = $1")
        .bind(uuid_field(&planner.2, "actorId"))
        .fetch_one(&pool)
        .await
        .expect("planner actor");
    assert_eq!(left, 0, "a deleted agent left its actor behind");
    let reviewer_path = format!("{profiles}/{profile_id}");
    let busy = send(
        &application,
        owner,
        "DELETE",
        &reviewer_path,
        "reviewer-busy",
        json!(null),
    )
    .await;
    assert_eq!(busy.0, StatusCode::CONFLICT, "{}", busy.1);
    sqlx::query("UPDATE agent_runs SET state = 'completed', finished_at = now(), updated_at = now() WHERE execution_worker_id = (SELECT execution_worker_id FROM agent_profiles WHERE id = $1)")
        .bind(profile_id)
        .execute(&pool)
        .await
        .expect("finish the reviewer's runs");
    let hidden = send(
        &application,
        owner,
        "DELETE",
        &reviewer_path,
        "reviewer-delete",
        json!(null),
    )
    .await;
    assert_eq!(hidden.0, StatusCode::OK, "{}", hidden.1);
    assert_eq!(hidden.2["outcome"], "hidden");
    let listed = send_get(&application, owner, &profiles).await;
    assert!(
        listed.2["profiles"]
            .as_array()
            .is_some_and(|profiles| profiles
                .iter()
                .all(|profile| profile["id"] != json!(profile_id))),
        "a hidden agent is still listed: {}",
        listed.1
    );
    let run = send_get(
        &application,
        member,
        &format!("/api/v1/agent-runs/{run_id}"),
    )
    .await;
    assert_eq!(run.0, StatusCode::OK, "{}", run.1);
    assert_eq!(
        run.2["agentName"], "Reviewer",
        "a past run lost its agent's name"
    );

    // Deleting an account: one never used goes with its home; the one the
    // project's runs used is signed out and hidden.
    let spare = send(
        &application,
        owner,
        "POST",
        &accounts,
        "create-spare",
        json!({"adapterKind": "demo", "name": "Spare"}),
    )
    .await;
    assert!(spare.0.is_success(), "create spare account: {}", spare.1);
    let spare_id = uuid_field(&spare.2, "id");
    let spare_home =
        sqlx::query_scalar::<_, String>("SELECT home_path FROM provider_accounts WHERE id = $1")
            .bind(spare_id)
            .fetch_one(&pool)
            .await
            .expect("spare home");
    assert!(std::path::Path::new(&spare_home).exists());
    let refused = send(
        &application,
        member,
        "DELETE",
        &format!("{accounts}/{spare_id}"),
        "member-account-delete",
        json!(null),
    )
    .await;
    assert_eq!(refused.0, StatusCode::NOT_FOUND, "{}", refused.1);
    let gone = send(
        &application,
        owner,
        "DELETE",
        &format!("{accounts}/{spare_id}"),
        "spare-delete",
        json!(null),
    )
    .await;
    assert_eq!(gone.0, StatusCode::OK, "{}", gone.1);
    assert_eq!(gone.2["outcome"], "deleted");
    assert!(
        !std::path::Path::new(&spare_home).exists(),
        "a deleted account kept its home"
    );
    sqlx::query("UPDATE agent_runs SET state = 'completed', finished_at = now(), updated_at = now() WHERE state NOT IN ('completed', 'failed', 'cancelled')")
        .execute(&pool)
        .await
        .expect("finish every run");
    let used = send(
        &application,
        owner,
        "DELETE",
        &account,
        "used-delete",
        json!(null),
    )
    .await;
    assert_eq!(used.0, StatusCode::OK, "{}", used.1);
    assert_eq!(used.2["outcome"], "hidden");
    let (status, state) = sqlx::query_as::<_, (String, String)>(
        "SELECT status, account_state FROM provider_accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("hidden account");
    assert_eq!(
        (status.as_str(), state.as_str()),
        ("disabled", "logged_out")
    );
    let listed = send_get(&application, owner, &accounts).await;
    assert!(
        listed.2["accounts"]
            .as_array()
            .is_some_and(|accounts| accounts
                .iter()
                .all(|listed| listed["id"] != json!(account_id))),
        "a hidden account is still listed: {}",
        listed.1
    );
}
