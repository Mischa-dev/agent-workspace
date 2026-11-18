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
        usage::alerts::evaluate(&state, account, Utc::now())
            .await
            .expect("evaluate"),
        1
    );
    let (status, _) = report(&host, "main", 63).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        usage::alerts::evaluate(&state, account, Utc::now())
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
