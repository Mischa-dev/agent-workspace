//! A1.24: plan windows over the host protocol. The costly failures are
//! silent: a host writing windows for an account it never described (or
//! another host's), an alert firing on every report, and a host's account
//! offered to the built-in harness as if it had a home here.

use agent_workspace_server::usage;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    notifications::{drain, inbox},
    support::{
        app::{send, state_on},
        test_host::TestHost,
        world::{create_project, setup},
    },
};

async fn describe_accounts(host: &TestHost, accounts: Value) -> (StatusCode, Value) {
    host.call(
        "POST",
        "/api/v1/host/describe",
        Some(json!({"software": {"name": "test-host", "version": "1.0"},
                    "reportIntervalSeconds": 60, "agents": [], "accounts": accounts})),
    )
    .await
}

async fn report(host: &TestHost, reference: &str, used: u16) -> (StatusCode, Value) {
    let resets_at = (Utc::now() + Duration::hours(2)).to_rfc3339();
    host.call(
        "POST",
        "/api/v1/host/windows",
        Some(json!({"accounts": [{
            "ref": reference,
            "plan": "Team",
            "windows": [{"key": "weekly", "label": "Weekly", "usedPercent": used,
                         "resetsAt": resets_at, "durationMinutes": 10080}],
        }]})),
    )
    .await
}

#[tokio::test]
async fn a_host_reports_the_windows_of_accounts_it_described_and_alerts_fire_once() {
    let Some(setup) = setup("host_windows").await else {
        return;
    };
    let host = TestHost::enrol(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "Team host",
        json!({"connection": "dial_out"}),
    )
    .await;

    // Nothing described yet: a report is refused and nothing is stored.
    let (status, refused) = report(&host, "main", 62).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    let (status, described) = describe_accounts(
        &host,
        json!([{"ref": "main", "name": "Main plan"}, {"ref": "spare", "name": "Spare"}]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{described}");
    let (status, _) = describe_accounts(
        &host,
        json!([{"ref": "a", "name": "A"}, {"ref": "a", "name": "B"}]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "refs are unique");

    let account: Uuid = sqlx::query_scalar(
        "SELECT id FROM provider_accounts WHERE host_id = $1 AND host_account_ref = 'main'",
    )
    .bind(host.id)
    .fetch_one(&setup.pool)
    .await
    .expect("described account");

    // A key the server can't keep is refused whole.
    let (status, _) = host
        .call(
            "POST",
            "/api/v1/host/windows",
            Some(json!({"accounts": [{"ref": "main",
                "windows": [{"key": "Not A Key", "usedPercent": 10}]}]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, stored) = report(&host, "main", 62).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["accounts"], 1);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs WHERE kind = 'usage.evaluate' AND unique_key = $1",
    )
    .bind(format!("usage.evaluate:{account}"))
    .fetch_one(&setup.pool)
    .await
    .expect("evaluation queued");
    assert_eq!(queued, 1, "the alert rules run after a report");

    // The rules fire once this cycle, however often the host reports.
    let state = state_on(&setup.pool);
    assert_eq!(
        crate::support::as_job(
            setup.organization_id,
            usage::alerts::evaluate(&state, account, Utc::now()),
        )
        .await
        .expect("evaluate"),
        1
    );
    let (status, _) = report(&host, "main", 63).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        crate::support::as_job(
            setup.organization_id,
            usage::alerts::evaluate(&state, account, Utc::now()),
        )
        .await
        .expect("evaluate again"),
        0,
        "an alert fires once a cycle"
    );
    drain(&setup).await;
    let page = inbox(&setup, &setup.browser, "").await;
    let alerts = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .filter(|item| item["kind"] == "capacity_reset_soon")
        .count();
    assert_eq!(alerts, 1, "{page}");

    // The usage page shows it, named by its host and connected while the
    // host is.
    let (status, usage) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/usage?organizationId={}", setup.organization_id),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{usage}");
    let listed = usage["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|candidate| candidate["accountId"] == account.to_string())
        .expect("host account listed")
        .clone();
    assert_eq!(listed["harnessName"], "Team host", "{listed}");
    assert_eq!(listed["connected"], true);
    assert_eq!(listed["plan"], "Team");
    assert_eq!(listed["windows"][0]["unusedPercent"], 37, "{listed}");

    // It is not a harness account: never listed to sign in or run with.
    let project = create_project(&setup, "HW", "Host windows").await;
    let (status, accounts) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/provider-accounts"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accounts}");
    assert!(
        !accounts.to_string().contains(&account.to_string()),
        "{accounts}"
    );

    // Another host can't write to it, even knowing its ref.
    let other = TestHost::enrol(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "Other host",
        json!({"connection": "dial_out"}),
    )
    .await;
    let (status, refused) = report(&other, "main", 1).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // An account no longer described is hidden and takes no reports.
    let (status, _) = describe_accounts(&host, json!([{"ref": "spare", "name": "Spare"}])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = report(&host, "main", 70).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let hidden: bool =
        sqlx::query_scalar("SELECT hidden_at IS NOT NULL FROM provider_accounts WHERE id = $1")
            .bind(account)
            .fetch_one(&setup.pool)
            .await
            .expect("hidden");
    assert!(hidden);
    let windows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_windows WHERE account_id = $1")
            .bind(account)
            .fetch_one(&setup.pool)
            .await
            .expect("windows");
    assert_eq!(windows, 1, "its windows stay with it");
}

/// A1.17: the built-in host reports its own account homes' windows on the
/// same endpoint, by account id; a partial report (heard mid-run) keeps
/// the windows it doesn't list. Only the built-in host can name a home
/// account, and it can't name another host's.
#[tokio::test]
async fn the_built_in_host_reports_its_accounts_windows_through_the_protocol() {
    use agent_workspace_builtin_host::harness::adapter::{UsageReport, UsageWindow};
    use agent_workspace_builtin_host::runtime::windows;

    let Some(setup) = setup("builtin_windows").await else {
        return;
    };
    let owner: Uuid = sqlx::query_scalar(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 LIMIT 1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let home = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO provider_accounts (id, organization_id, owner_actor_id, adapter_kind, name, home_path) \
         VALUES ($1, $2, $3, 'demo', 'Home', '/tmp/a117-' || $1::text)",
    )
    .bind(home)
    .bind(setup.organization_id)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("home account");

    let state = state_on(&setup.pool);
    let client = agent_workspace_server::hosts::builtin::transport::client(&state);
    let window = |key: &str, used: u16| UsageWindow {
        key: key.to_owned(),
        limit_name: None,
        used_percent: used,
        resets_at: Some((Utc::now() + Duration::hours(3)).timestamp()),
        duration_minutes: Some(300),
        limit_reached: false,
    };
    let keys = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (String, i16)>(
            "SELECT window_key, used_percent FROM usage_windows WHERE account_id = $1 ORDER BY window_key",
        )
        .bind(home)
        .fetch_all(&pool)
        .await
        .expect("windows")
    };

    let full = UsageReport {
        windows: vec![window("session", 20), window("weekly", 40)],
        reset_credits: None,
        plan: Some("pro".to_owned()),
    };
    windows::report(&client, home, &full, false)
        .await
        .expect("full report taken");
    assert_eq!(
        keys(setup.pool.clone()).await,
        vec![("session".to_owned(), 20), ("weekly".to_owned(), 40)]
    );

    // Mid-run: only the session window moved; the weekly one stays.
    let partial = UsageReport {
        windows: vec![window("session", 55)],
        reset_credits: None,
        plan: None,
    };
    windows::report(&client, home, &partial, true)
        .await
        .expect("partial report taken");
    assert_eq!(
        keys(setup.pool.clone()).await,
        vec![("session".to_owned(), 55), ("weekly".to_owned(), 40)]
    );
    let plan: Option<String> =
        sqlx::query_scalar("SELECT plan FROM usage_accounts WHERE account_id = $1")
            .bind(home)
            .fetch_one(&setup.pool)
            .await
            .expect("plan");
    assert_eq!(
        plan.as_deref(),
        Some("pro"),
        "a partial report keeps the plan"
    );

    // The next full read drops what the harness no longer lists.
    windows::report(&client, home, &partial, false)
        .await
        .expect("full report taken");
    assert_eq!(
        keys(setup.pool.clone()).await,
        vec![("session".to_owned(), 55)]
    );

    // An outside host can't name a home account, even by its id...
    let host = TestHost::enrol(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "Team host",
        json!({"connection": "dial_out"}),
    )
    .await;
    let (status, refused) = report(&host, &home.to_string(), 99).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // ...and the built-in host can't name another host's account.
    let (status, _) = describe_accounts(&host, json!([{"ref": "main", "name": "Main"}])).await;
    assert_eq!(status, StatusCode::OK);
    let described: Uuid = sqlx::query_scalar(
        "SELECT id FROM provider_accounts WHERE host_id = $1 AND host_account_ref = 'main'",
    )
    .bind(host.id)
    .fetch_one(&setup.pool)
    .await
    .expect("described account");
    assert!(
        windows::report(&client, described, &partial, false)
            .await
            .is_err()
    );
    assert_eq!(
        keys(setup.pool.clone()).await,
        vec![("session".to_owned(), 55)]
    );
}
