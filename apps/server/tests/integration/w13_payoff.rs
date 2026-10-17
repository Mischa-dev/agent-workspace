//! W13: the batch a capacity alert suggests, usage history, Home and the
//! first-run path. The costly failures are silent: a batch that launches on
//! the wrong account or outside the project's environment, one that reaches
//! someone else's account or a project the owner can't open, and Home or
//! onboarding showing what the reader can't see.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::{provider::adapter::UsageReport, usage};

use super::{
    mcp_admin::{Setup, create_project},
    message_edits::{add_person, get},
    notifications::inbox,
    w9_capacity_and_push::{call, demo_account, fixture, weekly},
};

async fn add_repository(setup: &Setup, project: &str, github_id: i64) {
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state
        )
        VALUES ($1, $2, $3, 'Fixture', $4, 'https://github.com/Fixture/batch.git',
                'main', '/nonexistent/batch-' || $3::text || '.git', true, true, 'pending')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(github_id)
    .bind(format!("batch-{github_id}"))
    .execute(&setup.pool)
    .await
    .expect("repository");
}

/// A task an agent can start: a description that says enough and a criterion.
async fn ready_task(setup: &Setup, project: &str, title: &str, priority: &str) -> String {
    let (status, task) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        Some(&format!("task-{}", title.replace(' ', "-"))),
        json!({
            "title": title,
            "priority": priority,
            "description": "Do the whole thing end to end, with tests and a short note in the README.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let id = task["id"].as_str().expect("task id").to_owned();
    let (status, criterion) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items/{id}/acceptance-criteria"),
        Some(&format!("criterion-{}", title.replace(' ', "-"))),
        json!({"expectedVersion": task["version"], "text": "It works"}),
    )
    .await;
    assert!(status.is_success(), "{criterion}");
    id
}

async fn environment(setup: &Setup, project: &str, harness: &str) -> String {
    let (status, created) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{project}/environments"),
        Some(&format!("environment-{project}")),
        json!({"name": "Here", "harness": harness, "setupNotes": "Nothing to set up."}),
    )
    .await;
    assert!(status.is_success(), "{created}");
    created["id"].as_str().expect("environment").to_owned()
}

fn keys(tasks: &Value) -> Vec<String> {
    tasks
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["key"].as_str().expect("key").to_owned())
        .collect()
}

#[tokio::test]
async fn the_alert_launches_its_batch_on_the_account_in_each_projects_environment() {
    let Some(fixture) = fixture("w13_batch").await else {
        return;
    };
    let setup = &fixture.setup;
    let state = &fixture.state;
    let alpha = create_project(setup, "ALP", "Alpha").await;
    let beta = create_project(setup, "BET", "Beta").await;
    let (_mara, mara) = add_person(setup, &[&alpha], "mara").await;
    let account = demo_account(setup, &alpha).await;
    // The same account serves Beta, which has no default environment.
    let (status, body) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{beta}/provider-accounts/{account}/use"),
        Some("use-in-beta"),
        json!({}),
    )
    .await;
    assert!(status.is_success(), "{body}");
    add_repository(setup, &alpha, 9_200_000_001).await;
    add_repository(setup, &beta, 9_200_000_002).await;
    let alpha_environment = environment(setup, &alpha, "demo").await;
    let urgent = ready_task(setup, &alpha, "Fix the outage", "urgent").await;
    ready_task(setup, &alpha, "Tidy the docs", "low").await;
    ready_task(setup, &beta, "Beta work", "high").await;

    // 62% used, resetting in a bit over two hours: the alert fires with the batch.
    let demo = state.supervisors.demo().expect("demo harness");
    demo.set_usage(
        account,
        UsageReport {
            windows: vec![weekly(62, chrono::Duration::minutes(125))],
            reset_credits: None,
            plan: None,
        },
    );
    usage::poll_account(state, account).await.expect("poll");
    let page = inbox(setup, &setup.browser, "").await;
    let alert = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .find(|item| item["kind"] == "capacity_reset_soon")
        .cloned()
        .expect("alert");
    assert_eq!(alert["batch"]["accountId"], account.to_string(), "{alert}");
    assert_eq!(alert["batch"]["size"], 2, "{alert}");
    assert!(
        alert["preview"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Launch 2 ready tasks"),
        "{alert}"
    );
    assert!(
        alert["launchHref"]
            .as_str()
            .unwrap_or_default()
            .contains("batch=1"),
        "{alert}"
    );

    // Ranked by priority; Beta is left out and says why.
    let batch_uri = format!("/api/v1/usage/accounts/{account}/batch");
    let (status, batch) = get(setup, &setup.browser, &batch_uri).await;
    assert_eq!(status, StatusCode::OK, "{batch}");
    assert_eq!(keys(&batch["tasks"]), ["ALP-1", "ALP-2"], "{batch}");
    let skipped = batch["skipped"].as_array().expect("skipped");
    assert_eq!(skipped.len(), 1, "{batch}");
    assert_eq!(skipped[0]["projectName"], "Beta");
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("default environment"),
        "{batch}"
    );

    // Someone else can neither see nor launch the owner's batch.
    let (status, body) = get(setup, &mara, &batch_uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = call(
        setup,
        &mara,
        "POST",
        &batch_uri,
        Some("their-batch"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Review: pick one task, plus one from Beta, which can't run here.
    let (status, picked) = call(
        setup,
        &setup.browser,
        "POST",
        &batch_uri,
        Some("review-batch"),
        json!({"tasks": [
            {"projectId": alpha, "workItemId": urgent},
            {"projectId": beta, "workItemId": Uuid::now_v7()},
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{picked}");
    assert_eq!(picked["launched"], 1, "{picked}");
    assert_eq!(
        picked["failed"].as_array().expect("failed").len(),
        1,
        "{picked}"
    );
    let launch_id = picked["projects"][0]["launches"][0]["id"]
        .as_str()
        .expect("launch")
        .parse::<Uuid>()
        .expect("launch id");
    let (launched_account, launched_environment, launched_state) =
        sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, String)>(
            "SELECT provider_account_id, environment_id, state FROM launches WHERE id = $1",
        )
        .bind(launch_id)
        .fetch_one(&setup.pool)
        .await
        .expect("launch row");
    assert_eq!(launched_account, Some(account), "on the alert's account");
    assert_eq!(
        launched_environment.map(|id| id.to_string()),
        Some(alpha_environment.clone()),
        "in the project's own environment"
    );
    assert_eq!(launched_state, "queued");

    // A queued task isn't suggested again; one tap launches what's left.
    let (_, batch) = get(setup, &setup.browser, &batch_uri).await;
    assert_eq!(keys(&batch["tasks"]), ["ALP-2"], "{batch}");
    let (status, tapped) = call(
        setup,
        &setup.browser,
        "POST",
        &batch_uri,
        Some("one-tap"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tapped}");
    assert_eq!(tapped["launched"], 1, "{tapped}");
    // The same tap again is the same launch, not a second one.
    let (status, again) = call(
        setup,
        &setup.browser,
        "POST",
        &batch_uri,
        Some("one-tap"),
        json!({}),
    )
    .await;
    assert!(
        status.is_success() || status == StatusCode::CONFLICT,
        "{again}"
    );
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM launches WHERE provider_account_id = $1",
    )
    .bind(account)
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(queued, 2);

    // The reading left history: a sample and the current cycle.
    let (status, history) = get(
        setup,
        &setup.browser,
        &format!("/api/v1/usage/accounts/{account}/history"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let window = &history["windows"][0];
    assert_eq!(window["label"], "Weekly", "{history}");
    assert_eq!(window["points"][0]["usedPercent"], 62, "{history}");
    assert_eq!(window["cycles"][0]["current"], true, "{history}");
    assert_eq!(window["cycles"][0]["unusedPercent"], 38, "{history}");
    assert_eq!(history["costTracked"], false);
    let (status, body) = get(
        setup,
        &mara,
        &format!("/api/v1/usage/accounts/{account}/history"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn onboarding_reads_each_step_from_what_exists_for_the_reader() {
    let Some(fixture) = fixture("w13_onboarding").await else {
        return;
    };
    let setup = &fixture.setup;
    let organization = setup.organization_id;
    let uri = format!("/api/v1/organizations/{organization}/onboarding");
    let (status, fresh) = get(setup, &setup.browser, &uri).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    assert_eq!(fresh["nextStep"], "workspace", "{fresh}");
    assert_eq!(fresh["complete"], false);

    let (status, named) = call(
        setup,
        &setup.browser,
        "PUT",
        &uri,
        Some("name-it"),
        json!({"confirm": "workspace"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{named}");
    assert_eq!(named["nextStep"], "project", "{named}");

    let project = create_project(setup, "ONB", "Onboarding").await;
    let (_, after) = get(setup, &setup.browser, &uri).await;
    assert_eq!(after["projectId"], project, "{after}");
    assert_eq!(after["nextStep"], "github", "{after}");
    add_repository(setup, &project, 9_300_000_001).await;
    demo_account(setup, &project).await;
    environment(setup, &project, "demo").await;
    ready_task(setup, &project, "First task", "medium").await;
    let (_, after) = get(setup, &setup.browser, &uri).await;
    let done = |id: &str| {
        after["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .find(|step| step["id"] == id)
            .map(|step| step["done"].as_bool().unwrap_or(false))
            .expect("step")
    };
    assert!(!done("github"), "no GitHub installation yet: {after}");
    for step in ["repository", "account", "environment", "tasks"] {
        assert!(done(step), "{step}: {after}");
    }
    // Skipping the optional agent step; launch is still to do.
    let (status, skipped) = call(
        setup,
        &setup.browser,
        "PUT",
        &uri,
        Some("skip-agent"),
        json!({"skip": "agent"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{skipped}");
    let refused = call(
        setup,
        &setup.browser,
        "PUT",
        &uri,
        Some("skip-launch"),
        json!({"skip": "launch"}),
    )
    .await;
    assert_eq!(refused.0, StatusCode::BAD_REQUEST, "{}", refused.1);

    // Another workspace's onboarding, or a project outside this one, is out of reach.
    let (status, body) = get(
        setup,
        &setup.browser,
        &format!("/api/v1/organizations/{}/onboarding", Uuid::now_v7()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = call(
        setup,
        &setup.browser,
        "PUT",
        &uri,
        Some("foreign-project"),
        json!({"projectId": Uuid::now_v7()}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, home) = get(setup, &setup.browser, "/api/v1/home").await;
    assert_eq!(status, StatusCode::OK, "{home}");
    assert_eq!(home["workingTotal"], 0, "{home}");
}
