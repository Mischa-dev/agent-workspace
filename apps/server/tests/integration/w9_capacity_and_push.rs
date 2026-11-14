//! W9: usage windows, capacity alerts, banked resets, Web Push and
//! notification settings. The costly failures are silent: an alert or a
//! push reaching someone who doesn't own the account (or can no longer see
//! the notification), one alert firing every poll, a banked reset spent
//! twice or without asking, and a push endpoint's secrets stored in the
//! clear.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
    config::AppConfig,
    jobs::{self, JobRegistry, WorkerOptions},
    notification_settings,
    provider::{
        SupervisorRegistry,
        adapter::{ResetCredit, ResetCredits, UsageReport, UsageWindow},
    },
    push,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    secrets::{self, FileKeyring},
    usage,
};
use axum::http::StatusCode;
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Executor, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, PASSWORD, login, schema_pool, send},
    mcp_admin::{Setup, create_project},
    message_edits::{add_person, copies, create_channel, get, owner_id, say},
    notifications::{drain, inbox},
};

pub(super) struct Fixture {
    pub(super) setup: Setup,
    pub(super) state: AppState,
    _keys: (tempfile::TempDir, tempfile::TempDir),
}

pub(super) async fn fixture(prefix: &str) -> Option<Fixture> {
    fixture_with(prefix, |_| {}).await
}

/// [`fixture`] with its configuration changed by `configure` first.
pub(super) async fn fixture_with(
    prefix: &str,
    configure: impl FnOnce(&mut AppConfig),
) -> Option<Fixture> {
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
    let owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Capacity".to_owned()),
            organization_slug: Some(format!("capacity-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let keys = tempfile::tempdir().expect("key directory");
    let keyring_path = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring_path, &FileKeyring::generate()).expect("keyring");
    let roots = tempfile::tempdir().expect("data root");
    let data_root = roots.path().join("data");
    let mut config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
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
            roots.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_DEMO_EXECUTOR".to_owned(),
            "true".to_owned(),
        ),
    ]))
    .expect("config");
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    configure(&mut config);
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    };
    let app = router(state.clone());
    let browser = login(&app).await;
    Some(Fixture {
        setup: Setup {
            app,
            browser,
            organization_id: owner.organization_id,
            pool,
            cleanup: None,
        },
        state,
        _keys: (keys, roots),
    })
}

pub(super) async fn call(
    setup: &Setup,
    browser: &Browser,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    send(&setup.app, browser.request(method, uri, key, body)).await
}

/// A signed-in demo account of the owner's, used in `project`.
pub(super) async fn demo_account(setup: &Setup, project: &str) -> Uuid {
    let accounts = format!("/api/v1/projects/{project}/provider-accounts");
    let (status, created) = call(
        setup,
        &setup.browser,
        "POST",
        &accounts,
        Some("create-demo"),
        json!({"adapterKind": "demo"}),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let account = created["id"].as_str().expect("account").to_owned();
    for (path, key, body) in [
        (
            "login/api-key",
            "demo-sign-in",
            json!({"apiKey": "demo-key-for-tests"}),
        ),
        ("use", "use-demo", json!({})),
    ] {
        let (status, body) = call(
            setup,
            &setup.browser,
            "POST",
            &format!("{accounts}/{account}/{path}"),
            Some(key),
            body,
        )
        .await;
        assert!(status.is_success(), "{path}: {body}");
    }
    account.parse().expect("account id")
}

pub(super) fn weekly(used_percent: u16, resets_in: chrono::Duration) -> UsageWindow {
    UsageWindow {
        key: "demo.weekly".to_owned(),
        limit_name: None,
        used_percent,
        resets_at: Some((Utc::now() + resets_in).timestamp()),
        duration_minutes: Some(7 * 24 * 60),
        limit_reached: false,
    }
}

fn device_keys() -> Value {
    let device = aws_lc_rs::agreement::PrivateKey::generate(&aws_lc_rs::agreement::ECDH_P256)
        .expect("device key");
    let public = device.compute_public_key().expect("device public key");
    let b64 = |bytes: &[u8]| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    };
    json!({"p256dh": b64(public.as_ref()), "auth": b64(&[9_u8; 16])})
}

fn push_jobs() -> JobRegistry {
    let mut registry = JobRegistry::new();
    push::register_jobs(&mut registry);
    registry
}

fn options() -> WorkerOptions {
    WorkerOptions {
        concurrency: 1,
        poll_interval: Duration::from_millis(20),
        lease: Duration::from_secs(30),
        worker_id: "w9-test".to_owned(),
    }
}

#[tokio::test]
async fn a_plan_with_only_a_weekly_window_alerts_its_owner_once_with_ready_tasks() {
    let Some(fixture) = fixture("w9_alerts").await else {
        return;
    };
    let setup = &fixture.setup;
    let state = &fixture.state;
    let project = create_project(setup, "CAP", "Capacity").await;
    let (_mara, mara) = add_person(setup, &[&project], "mara").await;
    let account = demo_account(setup, &project).await;

    // One ready task: open, unblocked, in a project with a repository.
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state
        )
        VALUES ($1, $2, 9100000001, 'Fixture', 'capacity', 'https://github.com/Fixture/capacity.git',
                'main', '/nonexistent/capacity.git', true, true, 'pending')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project.parse::<Uuid>().expect("project"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    let (status, task) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        Some("ready-task"),
        json!({"title": "Wire the importer"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");

    // The harness reports a plan with only a weekly window.
    let demo = state.supervisors.demo().expect("demo harness");
    demo.set_usage(
        account,
        UsageReport {
            windows: vec![weekly(62, chrono::Duration::minutes(125))],
            reset_credits: None,
            plan: Some("pro".to_owned()),
        },
    );
    usage::poll_account(state, account).await.expect("poll");
    let (status, page) = get(setup, &setup.browser, "/api/v1/usage").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let listed = page["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|candidate| candidate["accountId"] == account.to_string())
        .expect("account listed")
        .clone();
    let windows = listed["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 1, "{listed}");
    assert_eq!(windows[0]["label"], "Weekly");
    assert_eq!(windows[0]["unusedPercent"], 38);
    assert!(windows[0]["resetsAt"].is_string());

    // Reading it fired the alert; later reads this cycle don't repeat it.
    usage::poll_account(state, account)
        .await
        .expect("poll again");
    assert_eq!(
        usage::alerts::evaluate(state, account, Utc::now())
            .await
            .expect("evaluate again"),
        0,
        "an alert fires once a cycle"
    );
    let page = inbox(setup, &setup.browser, "").await;
    let alerts = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .filter(|item| item["kind"] == "capacity_reset_soon")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(alerts.len(), 1, "{page}");
    let title = alerts[0]["title"].as_str().expect("title");
    assert!(
        title.ends_with("weekly limit resets in 2 h · 38% unused"),
        "{title}"
    );
    assert!(
        alerts[0]["preview"]
            .as_str()
            .unwrap_or_default()
            .contains('1'),
        "the preview counts ready tasks: {}",
        alerts[0]
    );
    let launch = alerts[0]["launchHref"].as_str().expect("launch");
    assert!(
        launch.starts_with("/settings/account/usage?account="),
        "{launch}"
    );
    let (status, ready) = get(
        setup,
        &setup.browser,
        &format!("/api/v1/usage/accounts/{account}/ready-tasks"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(
        ready["tasks"].as_array().expect("tasks").len(),
        1,
        "{ready}"
    );

    // Someone else in the project sees no alert and can't reach the account.
    let theirs = inbox(setup, &mara, "").await;
    assert!(
        !theirs.to_string().contains("capacity_"),
        "only the owner hears about their account: {theirs}"
    );
    for uri in [
        format!("/api/v1/usage/accounts/{account}"),
        format!("/api/v1/usage/accounts/{account}/ready-tasks"),
    ] {
        let (status, body) = get(setup, &mara, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    let (status, body) = call(
        setup,
        &mara,
        "POST",
        &format!("/api/v1/usage/accounts/{account}/reset-credits/spend"),
        Some("their-spend"),
        json!({"confirm": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, page) = get(setup, &mara, "/api/v1/usage").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!page.to_string().contains(&account.to_string()), "{page}");

    // A banked reset is spent only when confirmed, once per attempt.
    demo.set_usage(
        account,
        UsageReport {
            windows: vec![weekly(100, chrono::Duration::hours(30))],
            reset_credits: Some(ResetCredits {
                available: 1,
                credits: vec![ResetCredit {
                    id: "credit-1".to_owned(),
                    title: Some("Weekly reset".to_owned()),
                    expires_at: None,
                    granted_at: None,
                }],
            }),
            plan: Some("pro".to_owned()),
        },
    );
    usage::poll_account(state, account).await.expect("poll");
    let spend = format!("/api/v1/usage/accounts/{account}/reset-credits/spend");
    let (status, body) = call(
        setup,
        &setup.browser,
        "POST",
        &spend,
        Some("spend-unconfirmed"),
        json!({"confirm": false}),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    let (status, body) = call(
        setup,
        &setup.browser,
        "POST",
        &spend,
        None,
        json!({"confirm": true}),
    )
    .await;
    assert!(
        status.is_client_error(),
        "no idempotency key: {status} {body}"
    );
    let (status, spent) = call(
        setup,
        &setup.browser,
        "POST",
        &spend,
        Some("spend-1"),
        json!({"confirm": true, "creditId": "credit-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{spent}");
    assert_eq!(spent["outcome"], "reset");
    assert_eq!(spent["account"]["windows"][0]["usedPercent"], 0, "{spent}");
    let (status, again) = call(
        setup,
        &setup.browser,
        "POST",
        &spend,
        Some("spend-2"),
        json!({"confirm": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["outcome"], "no_credit");
}

#[tokio::test]
async fn push_devices_stay_encrypted_follow_settings_and_lose_access_with_the_project() {
    let Some(fixture) = fixture("w9_push").await else {
        return;
    };
    let setup = &fixture.setup;
    let state = &fixture.state;
    let project = create_project(setup, "PSH", "Push").await;
    let general = create_channel(setup, &project, "general").await;
    let (mara_id, mara) = add_person(setup, &[&project], "mara").await;
    let organization = setup.organization_id;

    // The public key is per workspace; the private key never leaves the server.
    let (status, config) = get(setup, &mara, "/api/v1/push/config").await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert!(
        config["publicKey"]
            .as_str()
            .is_some_and(|key| key.len() > 80)
    );
    assert!(!config.to_string().to_lowercase().contains("private"));

    // Endpoints outside the browsers' push services are refused.
    for endpoint in [
        "https://push.example.com/send/abc",
        "http://fcm.googleapis.com/fcm/send/abc",
        "https://user@fcm.googleapis.com/fcm/send/abc",
        "https://fcm.googleapis.com:8443/fcm/send/abc",
        "https://127.0.0.1/fcm/send/abc",
    ] {
        let (status, body) = call(
            setup,
            &mara,
            "POST",
            "/api/v1/push/subscriptions",
            Some("bad-endpoint"),
            json!({"endpoint": endpoint, "keys": device_keys(), "organizationId": organization}),
        )
        .await;
        assert!(status.is_client_error(), "{endpoint}: {status} {body}");
    }

    let endpoint = "https://fcm.googleapis.com/fcm/send/w9-secret-endpoint-token";
    let keys = device_keys();
    let (status, device) = call(
        setup,
        &mara,
        "POST",
        "/api/v1/push/subscriptions",
        Some("mara-phone"),
        json!({"endpoint": endpoint, "keys": keys, "organizationId": organization, "label": "Phone"}),
    )
    .await;
    assert!(status.is_success(), "{device}");
    let (status, devices) = get(setup, &mara, "/api/v1/push/subscriptions").await;
    assert_eq!(status, StatusCode::OK, "{devices}");
    for text in [device.to_string(), devices.to_string()] {
        assert!(!text.contains("w9-secret-endpoint-token"), "{text}");
        assert!(
            !text.contains(keys["auth"].as_str().expect("auth")),
            "{text}"
        );
    }
    // Stored only through the encrypted secrets store.
    for table in ["push_subscriptions", "secrets"] {
        assert_eq!(
            copies(&setup.pool, table, "w9-secret-endpoint-token").await,
            0,
            "{table}"
        );
        assert_eq!(
            copies(&setup.pool, table, keys["p256dh"].as_str().expect("p256dh")).await,
            0,
            "{table}"
        );
    }
    // Nobody else sees the device.
    let (_, owner_devices) = get(setup, &setup.browser, "/api/v1/push/subscriptions").await;
    assert!(
        !owner_devices
            .to_string()
            .contains(device["id"].as_str().expect("id")),
        "{owner_devices}"
    );

    // Mute the project's runs but keep its mentions (N4).
    let (status, body) = call(
        setup,
        &mara,
        "PUT",
        "/api/v1/notification-settings/mutes",
        Some("mute-runs"),
        json!({"projectId": project, "group": "runs", "muted": true}),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let project_id = project.parse::<Uuid>().expect("project");
    let mut connection = setup.pool.acquire().await.expect("connection");
    let run = notification_settings::decide(&mut connection, mara_id, project_id, "run_completed")
        .await
        .expect("decide run");
    assert!(!run.in_app && !run.push, "{run:?}");
    let mention = notification_settings::decide(&mut connection, mara_id, project_id, "mention")
        .await
        .expect("decide mention");
    assert!(mention.in_app && mention.push, "{mention:?}");
    drop(connection);
    // Someone else can't mute a project they aren't in.
    let other = create_project(setup, "OTH", "Other").await;
    let (status, body) = call(
        setup,
        &mara,
        "PUT",
        "/api/v1/notification-settings/mutes",
        Some("mute-other"),
        json!({"projectId": other, "group": "all", "muted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A mention queues one push for Mara's device.
    let owner = owner_id(&setup.pool).await;
    assert_ne!(owner, mara_id);
    say(
        setup,
        &setup.browser,
        &general,
        "push-mention",
        json!({
            "version": 1,
            "nodes": [
                {"type": "text", "text": "Can you look "},
                {"type": "mention", "actorId": mara_id}
            ]
        }),
    )
    .await;
    drain(setup).await;
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = $1 AND state = 'queued'",
    )
    .bind(push::DELIVER_KIND)
    .fetch_one(&setup.pool)
    .await
    .expect("queued pushes");
    assert_eq!(queued, 1);
    assert_eq!(
        copies(&setup.pool, "background_jobs", "w9-secret-endpoint-token").await,
        0,
        "the job carries no endpoint"
    );

    // Mara leaves the project before it sends: nothing goes out.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(mara_id)
    .execute(&setup.pool)
    .await
    .expect("remove Mara");
    let ran = jobs::run_due(state, &push_jobs(), &options())
        .await
        .expect("run pushes");
    assert!(ran >= 1);
    let (failures, last_sent) = sqlx::query_as::<_, (i32, Option<chrono::DateTime<Utc>>)>(
        "SELECT failure_count, last_sent_at FROM push_subscriptions WHERE actor_id = $1",
    )
    .bind(mara_id)
    .fetch_one(&setup.pool)
    .await
    .expect("device");
    assert_eq!((failures, last_sent), (0, None), "no send was attempted");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM push_deliveries")
            .fetch_one(&setup.pool)
            .await
            .expect("deliveries"),
        0
    );

    // Removing the device removes its stored endpoint too.
    let (status, body) = call(
        setup,
        &mara,
        "DELETE",
        &format!(
            "/api/v1/push/subscriptions/{}",
            device["id"].as_str().expect("id")
        ),
        Some("remove-phone"),
        Value::Null,
    )
    .await;
    assert!(status.is_success(), "{body}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM secrets WHERE name LIKE 'push:%' AND owner_actor_id = $1"
        )
        .bind(mara_id)
        .fetch_one(&setup.pool)
        .await
        .expect("secrets"),
        0
    );
}
