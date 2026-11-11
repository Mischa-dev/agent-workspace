//! The bug sweep of 2026-10-18: one case per silent failure it fixed. An
//! agent's scheduled message or reminder that was saved but read back as
//! "not found" (so it couldn't be cancelled and a retry sent it twice); a
//! failed scheduled message that went out the moment its text was fixed;
//! a reset email sent through a mail server another workspace's owner runs;
//! a reset link that outlived a password change; agent sessions shown to
//! someone no longer in the project; and a link preview left showing after
//! its link was edited out.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::{mail, password_reset};

use super::{
    agent_interface::{create_agent, initialize, tool},
    mcp_admin::create_project,
    mcp_tasks::{add_member, space},
    message_edits::{add_person, create_channel, edit, get, say, text},
    w9_capacity_and_push::{call, fixture},
};

fn in_an_hour() -> String {
    (Utc::now() + Duration::hours(1)).to_rfc3339()
}

#[tokio::test]
async fn an_agent_can_read_back_cancel_and_be_reminded_of_what_it_scheduled() {
    let Some(fixture) = fixture("sweep_agent_later").await else {
        return;
    };
    let setup = &fixture.setup;
    let app = &setup.app;
    let work = create_project(setup, "WORK", "Work").await;
    let channel = create_channel(setup, &work, "updates").await;
    let agents = space(setup).await;
    let (rook, token) = create_agent(app, &setup.browser, &agents, "Rook", "rook").await;
    add_member(setup, &work, rook, "member").await;
    let session = initialize(app, &token).await.expect("Rook MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let scheduled = call(
        "message_schedule",
        json!({"conversationId": channel, "text": "Nightly report", "sendAt": in_an_hour()}),
    )
    .await;
    assert_eq!(scheduled["outcome"], "ok", "{scheduled}");
    let id = scheduled["value"]["scheduled"]["id"]
        .as_str()
        .expect("scheduled id")
        .to_owned();
    let listed = call("later_list", json!({"view": "scheduled"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed.to_string().contains(&id), "{listed}");
    let canceled = call(
        "scheduled_message_update",
        json!({"id": id, "cancel": true}),
    )
    .await;
    assert_eq!(canceled["outcome"], "ok", "{canceled}");
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(id.parse::<Uuid>().expect("id"))
            .fetch_one(&setup.pool)
            .await
            .expect("status");
    assert_eq!(status, "canceled");

    let reminder = call(
        "later_save",
        json!({"text": "Check the nightly report", "at": in_an_hour()}),
    )
    .await;
    assert_eq!(reminder["outcome"], "ok", "{reminder}");
    let open = call("later_list", json!({})).await;
    assert!(
        open.to_string().contains("Check the nightly report"),
        "{open}"
    );
}

#[tokio::test]
async fn fixing_a_failed_scheduled_message_keeps_it_until_it_gets_a_new_time() {
    let Some(fixture) = fixture("sweep_failed_edit").await else {
        return;
    };
    let setup = &fixture.setup;
    let project = create_project(setup, "FIX", "Fix").await;
    let channel = create_channel(setup, &project, "updates").await;
    let (status, scheduled) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/conversations/{channel}/scheduled-messages"),
        None,
        json!({"content": text("Typo hree"), "sendAt": in_an_hour()}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{scheduled}");
    let id = scheduled["scheduledMessage"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    // As after its send job ran and failed: no job left for it.
    sqlx::query("DELETE FROM background_jobs WHERE payload::text LIKE '%' || $1 || '%'")
        .bind(&id)
        .execute(&setup.pool)
        .await
        .expect("job ran");
    sqlx::query(
        "UPDATE scheduled_messages SET status = 'failed', failure = 'x', send_at = now() - interval '1 hour' WHERE id = $1",
    )
    .bind(id.parse::<Uuid>().expect("id"))
    .execute(&setup.pool)
    .await
    .expect("fail it");
    let (status, fixed) = call(
        setup,
        &setup.browser,
        "PATCH",
        &format!("/api/v1/scheduled-messages/{id}"),
        None,
        json!({"content": text("Typo here")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fixed}");
    assert_eq!(fixed["scheduledMessage"]["status"], "failed", "{fixed}");
    assert_eq!(
        fixed["scheduledMessage"]["plainText"], "Typo here",
        "{fixed}"
    );
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE state = 'queued' AND payload::text LIKE '%' || $1 || '%'",
    )
    .bind(&id)
    .fetch_one(&setup.pool)
    .await
    .expect("jobs");
    assert_eq!(queued, 0, "nothing is queued to send it");
}

#[tokio::test]
async fn reset_mail_goes_only_through_a_workspace_the_person_chose_or_owns() {
    let Some(fixture) = fixture("sweep_reset_mail").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    // The test workspace sends mail; its owner runs the mail server.
    sqlx::query(
        r#"
        INSERT INTO workspace_mail_settings (
            organization_id, provider, provider_config, from_address, enabled, updated_by_actor_id
        )
        SELECT $1, 'smtp',
               '{"host":"127.0.0.1","port":2525,"security":"none","username":null}'::jsonb,
               'workspace@example.com', true, actor_id
        FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1
        "#,
    )
    .bind(setup.organization_id)
    .execute(pool)
    .await
    .expect("mail settings");
    let project = create_project(setup, "RST", "Reset").await;
    let (mara, _) = add_person(setup, &[&project], "mara").await;
    let state = &fixture.state;
    let transport = |actor: Uuid| async move {
        mail::transport_for_actor(state, actor)
            .await
            .expect("transport")
            .map(|transport| transport.organization_id)
    };
    // It's the only workspace Mara is in, so it carries her mail.
    assert_eq!(transport(mara).await, Some(setup.organization_id));
    // She joins a second workspace (that sends no mail). The first is now
    // her default only because she joined it first, which isn't a choice:
    // its owner's mail server no longer carries her reset links.
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(pool)
        .await
        .expect("second organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(second)
    .bind(mara)
    .execute(pool)
    .await
    .expect("second membership");
    assert_eq!(transport(mara).await, None);
    // Once she chooses it as her default (or joins it from a workspace
    // invitation, which sets it), resets for her use it.
    sqlx::query("UPDATE users SET default_organization_id = $2 WHERE actor_id = $1")
        .bind(mara)
        .bind(setup.organization_id)
        .execute(pool)
        .await
        .expect("chosen default");
    assert_eq!(transport(mara).await, Some(setup.organization_id));
    // The owner's own workspace sends theirs.
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(setup.organization_id)
    .fetch_one(pool)
    .await
    .expect("owner");
    assert_eq!(transport(owner).await, Some(setup.organization_id));
    // Mara makes a workspace of her own (no mail) her default: the other
    // owner's mail server no longer carries her reset links.
    let own = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Mara', $2)")
        .bind(own)
        .bind(format!("mara-{}", own.simple()))
        .execute(pool)
        .await
        .expect("organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(own)
    .bind(mara)
    .execute(pool)
    .await
    .expect("membership");
    sqlx::query("UPDATE users SET default_organization_id = $2 WHERE actor_id = $1")
        .bind(mara)
        .bind(own)
        .execute(pool)
        .await
        .expect("default");
    assert_eq!(transport(mara).await, None);
    // When her own workspace sends mail, it carries hers, default or not.
    sqlx::query(
        r#"
        INSERT INTO workspace_mail_settings (
            organization_id, provider, provider_config, from_address, enabled, updated_by_actor_id
        ) VALUES ($1, 'smtp',
               '{"host":"127.0.0.1","port":2525,"security":"none","username":null}'::jsonb,
               'mara@example.com', true, $2)
        "#,
    )
    .bind(own)
    .bind(mara)
    .execute(pool)
    .await
    .expect("Mara's mail settings");
    assert_eq!(transport(mara).await, Some(own));
    sqlx::query("UPDATE users SET default_organization_id = $2 WHERE actor_id = $1")
        .bind(mara)
        .bind(setup.organization_id)
        .execute(pool)
        .await
        .expect("default back");
    assert_eq!(
        transport(mara).await,
        Some(setup.organization_id),
        "a chosen default comes first"
    );
    // Mail turned off in the chosen one: her own still works.
    sqlx::query("UPDATE workspace_mail_settings SET enabled = false WHERE organization_id = $1")
        .bind(setup.organization_id)
        .execute(pool)
        .await
        .expect("mail off");
    assert_eq!(transport(mara).await, Some(own));
    assert_eq!(transport(owner).await, None);
}

#[tokio::test]
async fn changing_your_password_ends_reset_links() {
    let Some(fixture) = fixture("sweep_reset_password").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let (user_id, epoch) = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, authentication_epoch FROM users WHERE login_name = 'owner'",
    )
    .fetch_one(pool)
    .await
    .expect("owner");
    let token = format!("aw_reset_{}", Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO password_reset_tokens (id, user_id, token_digest, authentication_epoch, expires_at) \
         VALUES ($1, $2, $3, $4, now() + interval '30 minutes')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(password_reset::digest(&token).to_vec())
    .bind(epoch)
    .execute(pool)
    .await
    .expect("plant reset");
    let (status, body) = call(
        setup,
        &setup.browser,
        "POST",
        "/api/v1/account/password",
        None,
        json!({
            "currentPassword": super::agent_interface::PASSWORD,
            "newPassword": "a new password chosen by hand",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let live = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM password_reset_tokens WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("links");
    assert_eq!(live, 0);
}

#[tokio::test]
async fn agent_sessions_need_the_project_and_previews_follow_edits() {
    let Some(fixture) = fixture("sweep_sessions").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let project = create_project(setup, "SES", "Sessions").await;
    let channel = create_channel(setup, &project, "general").await;
    let (mara, mara_browser) = add_person(setup, &[&project], "mara").await;
    let uri = format!("/api/v1/conversations/{channel}/agent-sessions");
    let (status, _) = get(setup, &mara_browser, &uri).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(mara)
    .execute(pool)
    .await
    .expect("remove Mara");
    let (status, _) = get(setup, &mara_browser, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A link edited out of a message takes its preview with it.
    let message = say(
        setup,
        &setup.browser,
        &channel,
        "sweep-preview",
        text("See https://example.com/private-doc"),
    )
    .await;
    let shown = |message: String| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM message_link_previews WHERE message_id = $1",
        )
        .bind(message.parse::<Uuid>().expect("message"))
        .fetch_one(pool)
        .await
        .expect("previews")
    };
    assert_eq!(shown(message.clone()).await, 1);
    let (status, body) = edit(
        setup,
        &setup.browser,
        &channel,
        &message,
        "sweep-edit-1",
        json!({"content": text("See the doc"), "expectedRevision": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(shown(message.clone()).await, 0);
    let (status, body) = edit(
        setup,
        &setup.browser,
        &channel,
        &message,
        "sweep-edit-2",
        json!({"content": text("See https://example.com/public"), "expectedRevision": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(shown(message).await, 1);
}
