//! AWD-75: tools that resolve their own targets (scheduled messages, webhooks, saved items, usage batches) check the module there.

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::{add_person, create_channel, text},
    modules::{narrow_in_project, switch},
    w9_capacity_and_push::{demo_account, fixture},
};
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

fn assert_module_off(result: &Value, module: &str) {
    assert_eq!(result["outcome"], "permission_denied", "{result}");
    assert_eq!(result["retryCategory"], "module", "{result}");
    assert!(
        result["message"]
            .as_str()
            .is_some_and(|message| message.contains(module)),
        "{result}"
    );
}

/// A second workspace the owner is also in, with every module on, so a
/// module switched off in the first is still on for them somewhere.
async fn second_workspace(setup: &Setup) -> Uuid {
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(&setup.pool)
        .await
        .expect("second workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(second)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    second
}

/// Later follows Inbox in each saved item's own project, counts included,
/// and a scheduled message follows the module its conversation needs:
/// there the item or message ID names no project the gate could check.
#[tokio::test]
async fn later_items_and_scheduled_messages_follow_their_own_project() {
    let Some(setup) = setup("awd75_ids_later").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let (status, task) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{other}/work-items"),
            Some("awd75-ids-task"),
            json!({"title": "Harbour ledger"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let channel = create_channel(&setup, &other, "harbour").await;
    let scheduled = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO scheduled_messages (
            id, organization_id, project_id, conversation_id, author_actor_id,
            content, plain_text, send_at
        ) VALUES ($1, $2, $3, $4, $5, $6, 'Harbour report', $7)
        "#,
    )
    .bind(scheduled)
    .bind(setup.organization_id)
    .bind(other.parse::<Uuid>().expect("other"))
    .bind(channel.parse::<Uuid>().expect("channel"))
    .bind(bea)
    .bind(text("Harbour report"))
    .bind(Utc::now() + Duration::hours(1))
    .execute(&setup.pool)
    .await
    .expect("scheduled message");

    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let call = |name: &'static str, arguments: Value| {
        let token = bea_app.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let saved = call("later_save", json!({"task": task})).await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    let item = saved["value"]["item"]["id"]
        .as_str()
        .expect("item")
        .to_owned();
    let listed = call("later_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["openCount"], 1, "{listed}");
    assert!(listed.to_string().contains(&item), "{listed}");
    let waiting = call("later_list", json!({"view": "scheduled"})).await;
    assert!(
        waiting.to_string().contains(&scheduled.to_string()),
        "{waiting}"
    );

    // In Other only, Bea's role leaves Inbox and Channels out. Both are
    // still on for her in Work, so her tools stay listed and callable.
    narrow_in_project(&setup, &other, bea, &["inbox", "channels"], "awd75-ids-bea").await;
    let listed = call("later_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["openCount"], 0, "{listed}");
    assert!(!listed.to_string().contains(&item), "{listed}");
    let waiting = call("later_list", json!({"view": "scheduled"})).await;
    assert_eq!(waiting["outcome"], "ok", "{waiting}");
    assert!(
        !waiting.to_string().contains(&scheduled.to_string()),
        "{waiting}"
    );
    let updated = call("later_update", json!({"id": item, "done": true})).await;
    assert_module_off(&updated, "Inbox");
    let again = call("later_save", json!({"task": task})).await;
    assert_module_off(&again, "Inbox");
    let done = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_items WHERE actor_id = $1 AND done_at IS NOT NULL",
    )
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("done");
    assert_eq!(done, 0, "an item was marked done where Inbox is off");

    let canceled = call(
        "scheduled_message_update",
        json!({"id": scheduled.to_string(), "cancel": true}),
    )
    .await;
    assert_module_off(&canceled, "Channels");
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(scheduled)
            .fetch_one(&setup.pool)
            .await
            .expect("status");
    assert_eq!(status, "scheduled");
}

/// Webhook tools follow Automations where each webhook acts: its project,
/// or the workspace a new one across a workspace goes to.
#[tokio::test]
async fn webhook_tools_follow_automations_where_each_webhook_acts() {
    let Some(setup) = setup("awd75_ids_webhooks").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let (bea, bea_browser) = add_person(&setup, &[&work, &other], "bea").await;
    let mut hooks = Vec::new();
    for (project, name) in [(&work, "Work hook"), (&other, "Other hook")] {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO webhook_subscriptions (
                id, organization_id, project_id, owner_actor_id, name, url, event_types
            ) VALUES ($1, $2, $3, $4, $5, 'https://example.com/hook', '{task.updated}')
            "#,
        )
        .bind(id)
        .bind(setup.organization_id)
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(bea)
        .bind(name)
        .execute(&setup.pool)
        .await
        .expect("webhook");
        hooks.push(id.to_string());
    }
    let (in_work, in_other) = (hooks[0].clone(), hooks[1].clone());
    let bea_app = connect_app(app, &bea_browser, "Bea's app", "full").await;
    let session = initialize(app, &bea_app).await.expect("Bea's app");
    let call = |name: &'static str, arguments: Value| {
        let token = bea_app.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let listed = call("webhook_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed.to_string().contains(&in_work), "{listed}");
    assert!(listed.to_string().contains(&in_other), "{listed}");

    // In Other only, Bea's role leaves Automations out.
    narrow_in_project(&setup, &other, bea, &["automations"], "awd75-ids-hooks").await;
    let listed = call("webhook_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed.to_string().contains(&in_work), "{listed}");
    assert!(!listed.to_string().contains(&in_other), "{listed}");
    assert!(
        !listed["value"]["projects"].to_string().contains(&other),
        "{listed}"
    );
    let renamed = call(
        "webhook_write",
        json!({"operation": "update", "webhookId": in_other, "expectedVersion": 1, "name": "Renamed"}),
    )
    .await;
    assert_module_off(&renamed, "Automations");
    let name =
        sqlx::query_scalar::<_, String>("SELECT name FROM webhook_subscriptions WHERE id = $1")
            .bind(in_other.parse::<Uuid>().expect("hook"))
            .fetch_one(&setup.pool)
            .await
            .expect("name");
    assert_eq!(name, "Other hook");
    let log = call("webhook_deliveries", json!({"webhookId": in_other})).await;
    assert_module_off(&log, "Automations");
    let log = call("webhook_deliveries", json!({"webhookId": in_work})).await;
    assert_eq!(log["outcome"], "ok", "{log}");

    // A webhook across a workspace goes to the owner's first workspace: with
    // Automations off there (and on in their second), it's refused.
    second_workspace(&setup).await;
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"automations": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &token).await.expect("owner's app");
    let created = tool(
        app,
        &token,
        &owner_session,
        "webhook_write",
        json!({"operation": "create", "name": "Everything", "url": "https://example.com/all", "eventTypes": ["task.updated"]}),
    )
    .await;
    assert_module_off(&created, "Automations");
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM webhook_subscriptions WHERE name = 'Everything'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(made, 0);
}

/// A person's plain reminder is kept in their home workspace, so with Inbox
/// off there (and on in their second), later_save refuses and saves nothing.
#[tokio::test]
async fn a_plain_reminder_follows_inbox_in_the_home_workspace() {
    let Some(setup) = setup("awd75_later_home").await else {
        return;
    };
    let app = &setup.app;
    second_workspace(&setup).await;
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"inbox": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner's app");
    let at = (Utc::now() + Duration::hours(2)).to_rfc3339();
    let saved = tool(
        app,
        &token,
        &session,
        "later_save",
        json!({"text": "Call the printer", "at": at}),
    )
    .await;
    assert_module_off(&saved, "Inbox");
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_items WHERE note = 'Call the printer'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("count");
    assert_eq!(kept, 0);
}

/// The usage tools suggest, count and launch only where Runs is on: an
/// account's projects are found from the account, not from the call.
#[tokio::test]
async fn usage_tools_follow_runs_in_each_project_the_account_serves() {
    let Some(fixture) = fixture("awd75_ids_usage").await else {
        return;
    };
    let setup = &fixture.setup;
    let alpha = create_project(setup, "ALP", "Alpha").await;
    let account = demo_account(setup, &alpha).await;
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state
        )
        VALUES ($1, $2, 9200000751, 'Fixture', 'awd75', 'https://github.com/Fixture/awd75.git',
                'main', '/nonexistent/awd75.git', true, true, 'pending')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(alpha.parse::<Uuid>().expect("alpha"))
    .execute(&setup.pool)
    .await
    .expect("repository");
    let (status, task) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{alpha}/work-items"),
            Some("awd75-usage-task"),
            json!({"title": "Alpha outage"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let token = connect_app(&setup.app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(&setup.app, &token).await.expect("owner's app");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(&setup.app, &token, &session, name, arguments).await }
    };
    let ready = |windows: &Value| {
        windows["value"]["accounts"]
            .as_array()
            .unwrap_or_else(|| panic!("accounts: {windows}"))
            .iter()
            .find(|line| line["accountId"] == account.to_string())
            .unwrap_or_else(|| panic!("account: {windows}"))["readyTasks"]
            .as_u64()
            .expect("readyTasks")
    };
    let windows = call("usage_windows", json!({})).await;
    assert_eq!(windows["outcome"], "ok", "{windows}");
    assert!(ready(&windows) >= 1, "{windows}");

    // Runs off in this workspace, still on in the owner's second one.
    second_workspace(setup).await;
    let (status, changed) =
        switch(setup, &setup.browser, json!({"switches": {"runs": false}})).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let windows = call("usage_windows", json!({})).await;
    assert_eq!(windows["outcome"], "ok", "{windows}");
    assert_eq!(ready(&windows), 0, "{windows}");
    let batch = call("usage_batch", json!({"accountId": account.to_string()})).await;
    assert_eq!(batch["outcome"], "ok", "{batch}");
    assert!(!batch.to_string().contains("Alpha outage"), "{batch}");
    assert!(
        batch["value"]["skipped"]
            .to_string()
            .contains("Runs is turned off"),
        "{batch}"
    );
    let launched = call(
        "usage_batch_launch",
        json!({"accountId": account.to_string(), "taskIds": [task], "clientRequestId": "awd75-usage-launch"}),
    )
    .await;
    assert_eq!(launched["outcome"], "ok", "{launched}");
    assert_eq!(launched["value"]["launched"], 0, "{launched}");
    assert!(
        launched["value"]["failed"]
            .to_string()
            .contains("Runs is turned off"),
        "{launched}"
    );
    let launches =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM launches WHERE work_item_id = $1")
            .bind(task.parse::<Uuid>().expect("task"))
            .fetch_one(&setup.pool)
            .await
            .expect("launches");
    assert_eq!(launches, 0);
}
