//! W9: usage windows, capacity alerts, banked resets, Web Push and
//! notification settings. The costly failures are silent: an alert or a
//! push reaching someone who doesn't own the account (or can no longer see
//! the notification), one alert firing every poll, a banked reset spent
//! twice or without asking, and a push endpoint's secrets stored in the
//! clear.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use crate::support::legacy::router;
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
            "AGENT_WORKSPACE_PROVIDER_HOME_ROOT".to_owned(),
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
    // The task writes moved to operations (K2.22).
    if crate::support::tasks::op_for(method, uri, &body).is_some() {
        return crate::support::tasks::send(&setup.app, browser, method, uri, key, body).await;
    }
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

pub(super) fn device_keys() -> Value {
    let device = aws_lc_rs::agreement::PrivateKey::generate(&aws_lc_rs::agreement::ECDH_P256)
        .expect("device key");
    let public = device.compute_public_key().expect("device public key");
    let b64 = |bytes: &[u8]| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    };
    json!({"p256dh": b64(public.as_ref()), "auth": b64(&[9_u8; 16])})
}

pub(super) fn push_jobs() -> JobRegistry {
    let mut registry = JobRegistry::new();
    push::register_jobs(&mut registry);
    registry
}

pub(super) fn options() -> WorkerOptions {
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
    crate::support::as_job(setup.organization_id, usage::poll_account(state, account))
        .await
        .expect("poll");
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
    crate::support::as_job(setup.organization_id, usage::poll_account(state, account))
        .await
        .expect("poll again");
    assert_eq!(
        crate::support::as_job(
            setup.organization_id,
            usage::alerts::evaluate(state, account, Utc::now()),
        )
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
    crate::support::as_job(setup.organization_id, usage::poll_account(state, account))
        .await
        .expect("poll");
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

    // C2.34: a push's receipt, sent back by the service worker without a
    // session, marks that push shown and opened, and the device list counts
    // them; an unknown receipt changes nothing and says so to nobody.
    let device_id = device["id"]
        .as_str()
        .expect("id")
        .parse::<Uuid>()
        .expect("device id");
    sqlx::query(
        r#"
        INSERT INTO push_receipts (receipt_hash, workspace_id, subscription_id, notification_id)
        SELECT sha256(convert_to('cmVjZWlwdC1mb3ItbWFyYQ', 'UTF8')), $2, $1, id
        FROM notifications WHERE recipient_actor_id = $3 LIMIT 1
        "#,
    )
    .bind(device_id)
    .bind(organization)
    .bind(mara_id)
    .execute(&setup.pool)
    .await
    .expect("receipt");
    let receipt = |token: &'static str, event: &'static str| {
        super::agent_interface::send(
            &setup.app,
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/push-receipts")
                .header("origin", "http://127.0.0.1:3100")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({"receipt": token, "event": event})).expect("body"),
                ))
                .expect("request"),
        )
    };
    for (token, event) in [
        ("cmVjZWlwdC1mb3ItbWFyYQ", "shown"),
        ("cmVjZWlwdC1mb3ItbWFyYQ", "opened"),
        ("bm90LWEtcmVjZWlwdA", "opened"),
    ] {
        let (status, body) = receipt(token, event).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let (_, devices) = get(setup, &mara, "/api/v1/push/subscriptions").await;
    let mine = &devices["devices"][0];
    assert_eq!(mine["transport"], "web_push", "{devices}");
    assert_eq!(
        (
            &mine["recentSent"],
            &mine["recentShown"],
            &mine["recentOpened"]
        ),
        (&json!(1), &json!(1), &json!(1)),
        "{devices}"
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

/// A read that had to skip a notification (its row was being written)
/// leaves it unread until a job reads it. A push for that message must not
/// go out in between, whichever job runs first.
#[tokio::test]
async fn a_push_for_a_message_already_seen_sends_nothing() {
    let Some(fixture) = fixture("w9_push_seen").await else {
        return;
    };
    let setup = &fixture.setup;
    let state = &fixture.state;
    let project = create_project(setup, "PSN", "Push seen").await;
    let general = create_channel(setup, &project, "general").await;
    let (mara_id, mara) = add_person(setup, &[&project], "mara").await;
    let (status, device) = call(
        setup,
        &mara,
        "POST",
        "/api/v1/push/subscriptions",
        Some("mara-phone"),
        json!({
            "endpoint": "https://fcm.googleapis.com/fcm/send/w9-seen-endpoint",
            "keys": device_keys(),
            "organizationId": setup.organization_id,
            "label": "Phone"
        }),
    )
    .await;
    assert!(status.is_success(), "{device}");
    say(
        setup,
        &setup.browser,
        &general,
        "seen-mention",
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
    let sequence = sqlx::query_scalar::<_, i64>(
        "SELECT next_entry_sequence - 1 FROM conversations WHERE id = $1::uuid",
    )
    .bind(&general)
    .fetch_one(&setup.pool)
    .await
    .expect("sequence");

    // Mara reads the channel while her notification is held, as a fan-out
    // writing to it would hold it: the read leaves the row for a job.
    let mut writer = setup.pool.begin().await.expect("writer");
    let held = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications WHERE recipient_actor_id = $1 AND read_at IS NULL FOR UPDATE",
    )
    .bind(mara_id)
    .fetch_all(&mut *writer)
    .await
    .expect("hold the notification");
    assert_eq!(held.len(), 1);
    let (status, seen) = call(
        setup,
        &mara,
        "POST",
        "/api/v2/ops/conversation_mark_read",
        Some("seen-read"),
        json!({"conversationId": general, "upToSequence": sequence}),
    )
    .await;
    assert!(status.is_success(), "{seen}");
    writer.commit().await.expect("writer done");
    assert_eq!(inbox(setup, &mara, "").await["unreadCount"], 1);

    // The push runs before the job that reads the row, and sends nothing.
    let ran = jobs::run_due(state, &push_jobs(), &options())
        .await
        .expect("run pushes");
    assert_eq!(ran, 1);
    let finished =
        sqlx::query_scalar::<_, String>("SELECT state::text FROM background_jobs WHERE kind = $1")
            .bind(push::DELIVER_KIND)
            .fetch_all(&setup.pool)
            .await
            .expect("push job");
    assert_eq!(finished, ["succeeded"], "the job decided, it didn't fail");
    let (failures, last_sent) = sqlx::query_as::<_, (i32, Option<chrono::DateTime<Utc>>)>(
        "SELECT failure_count, last_sent_at FROM push_subscriptions WHERE actor_id = $1",
    )
    .bind(mara_id)
    .fetch_one(&setup.pool)
    .await
    .expect("device");
    assert_eq!((failures, last_sent), (0, None), "no send was attempted");
    // A send mints its receipt first, so none means it never got that far.
    for table in ["push_receipts", "push_deliveries"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&setup.pool)
                .await
                .expect("count"),
            0,
            "{table}"
        );
    }
}
