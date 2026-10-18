//! W21: save for later, reminders, scheduled send, forwarding and
//! per-conversation notification levels. The costly failures are silent:
//! a saved message from a private conversation still readable after leaving
//! it, a reminder or scheduled send acting for someone who lost access, a
//! retried job reminding or sending twice, a forward posting where its
//! sender can't or quoting to people who can't see the original, and a
//! muted conversation still notifying.

use agent_workspace_server::{
    jobs::{self, WorkerOptions},
    notifications, push,
};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, send},
    mcp_admin::{Setup, create_project},
    message_edits::{add_person, create_channel, direct_space, get, owner_id, say, text},
    notifications::drain,
    w9_capacity_and_push::{Fixture, call, fixture},
};

/// Run every queued job now, including delayed ones.
async fn run_jobs(fixture: &Fixture) {
    sqlx::query("UPDATE background_jobs SET run_at = now() WHERE state = 'queued'")
        .execute(&fixture.setup.pool)
        .await
        .expect("make jobs due");
    jobs::run_due(
        &fixture.state,
        &jobs::default_registry(),
        &WorkerOptions::default(),
    )
    .await
    .expect("run jobs");
}

fn in_an_hour() -> String {
    (Utc::now() + Duration::hours(1)).to_rfc3339()
}

fn mention(actor: Uuid, value: &str) -> Value {
    json!({"version": 1, "nodes": [
        {"type": "mention", "actorId": actor},
        {"type": "text", "text": value}
    ]})
}

async fn notifications_of(pool: &PgPool, recipient: Uuid, kind: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = $2",
    )
    .bind(recipient)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("notifications")
}

async fn messages_by(pool: &PgPool, author: Uuid, plain: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE author_actor_id = $1 AND plain_text = $2",
    )
    .bind(author)
    .bind(plain)
    .fetch_one(pool)
    .await
    .expect("messages")
}

async fn later(setup: &Setup, browser: &Browser, view: &str) -> Value {
    let (status, page) = get(setup, browser, &format!("/api/v1/later?view={view}")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

async fn save(setup: &Setup, browser: &Browser, body: Value) -> (StatusCode, Value) {
    call(setup, browser, "POST", "/api/v1/later", None, body).await
}

async fn group(setup: &Setup, key: &str, members: &[Uuid]) -> String {
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some(key),
            json!({ "memberIds": members }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned()
}

async fn leave_group(setup: &Setup, group: &str, member: Uuid) {
    let (status, body) = call(
        setup,
        &setup.browser,
        "POST",
        &format!("/api/v1/conversations/{group}/members/{member}/remove"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn set_level(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    level: Value,
) -> (StatusCode, Value) {
    call(
        setup,
        browser,
        "PUT",
        &format!("/api/v1/conversations/{conversation}/notification-level"),
        None,
        json!({ "level": level }),
    )
    .await
}

/// What the service worker sends: just the token.
async fn act(setup: &Setup, body: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/notification-actions")
            .header("origin", "http://127.0.0.1:3100")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("body")))
            .expect("request"),
    )
    .await
}

async fn remind_at(pool: &PgPool, item: &str) -> Option<DateTime<Utc>> {
    sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT remind_at FROM saved_items WHERE id = $1",
    )
    .bind(item.parse::<Uuid>().expect("item"))
    .fetch_one(pool)
    .await
    .expect("item")
}

#[tokio::test]
async fn a_saved_private_message_vanishes_and_stays_silent_after_leaving() {
    let Some(fixture) = fixture("w21_private").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let space = direct_space(setup).await;
    let (mara, mara_browser) = add_person(setup, &[&space], "mara").await;
    let (_zoe, zoe_browser) = add_person(setup, &[&space], "zoe").await;
    let group = group(setup, "group-plans", &[mara]).await;
    let message = say(setup, &setup.browser, &group, "g1", text("Secret plan")).await;

    // Only someone who can see the message can save it.
    let (status, body) = save(setup, &zoe_browser, json!({"messageId": message})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, saved) = save(
        setup,
        &mara_browser,
        json!({"messageId": message, "note": "Read this", "remindAt": in_an_hour()}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let item = saved["item"]["id"].as_str().expect("item").to_owned();
    assert_eq!(
        saved["item"]["message"]["preview"], "Secret plan",
        "{saved}"
    );
    // Saving again is the same item.
    let (status, again) = save(setup, &mara_browser, json!({"messageId": message})).await;
    assert!(status.is_success(), "{again}");
    assert_eq!(again["item"]["id"], item.as_str());
    let page = later(setup, &mara_browser, "open").await;
    assert_eq!(page["openCount"], 1, "{page}");
    // It shows as saved on the message, for Mara only.
    let (_, timeline) = get(
        setup,
        &mara_browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert!(timeline.to_string().contains(&item), "{timeline}");
    let (_, timeline) = get(
        setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert!(!timeline.to_string().contains(&item), "{timeline}");

    // Once Mara leaves the group, nothing of it reads back and its reminder
    // stays silent, even though the row is still there.
    leave_group(setup, &group, mara).await;
    for view in ["open", "done"] {
        let page = later(setup, &mara_browser, view).await;
        assert_eq!(page["items"], json!([]), "{page}");
        assert!(!page.to_string().contains("Secret plan"), "{page}");
    }
    assert_eq!(later(setup, &mara_browser, "open").await["openCount"], 0);
    let (status, body) = call(
        setup,
        &mara_browser,
        "PATCH",
        &format!("/api/v1/later/{item}"),
        None,
        json!({"note": "Still mine?"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    run_jobs(&fixture).await;
    assert_eq!(
        notifications_of(pool, mara, notifications::kind::REMINDER).await,
        0
    );
    // Someone else can't touch it either.
    let (status, _) = call(
        setup,
        &zoe_browser,
        "DELETE",
        &format!("/api/v1/later/{item}"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_due_reminder_notifies_once_and_done_or_snooze_work_from_the_push() {
    let Some(fixture) = fixture("w21_remind").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let space = direct_space(setup).await;
    let (mara, mara_browser) = add_person(setup, &[&space], "mara").await;

    // A reminder needs words.
    let (status, _) = save(
        setup,
        &mara_browser,
        json!({"note": "  ", "remindAt": in_an_hour()}),
    )
    .await;
    assert!(status.is_client_error());
    let (status, saved) = save(
        setup,
        &mara_browser,
        json!({"note": "Call the printer", "remindAt": in_an_hour()}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let item = saved["item"]["id"].as_str().expect("item").to_owned();
    assert_eq!(saved["item"]["kind"], "reminder");

    // Due: one notification, however often the job runs.
    run_jobs(&fixture).await;
    run_jobs(&fixture).await;
    assert_eq!(
        notifications_of(pool, mara, notifications::kind::REMINDER).await,
        1
    );
    let notification = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM notifications WHERE recipient_actor_id = $1 AND kind = 'reminder'",
    )
    .bind(mara)
    .fetch_one(pool)
    .await
    .expect("reminder");
    let view = notifications::get_many(pool, mara, &[notification])
        .await
        .expect("notification")
        .into_iter()
        .next()
        .expect("visible reminder");
    assert_eq!(view.title, "Reminder: Call the printer");
    // Nobody else hears about it.
    assert_eq!(
        notifications_of(pool, owner_id(pool).await, notifications::kind::REMINDER).await,
        0
    );

    // Snooze from the phone moves it an hour on; a later run reminds again.
    let minted = push::actions::mint(pool, setup.organization_id, mara, &view)
        .await
        .expect("mint")
        .expect("a reminder offers actions");
    let payload = push::payload_with_actions(&view, Some(&minted));
    let actions = payload["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .map(|action| action["action"].as_str().expect("action"))
        .collect::<Vec<_>>();
    assert_eq!(actions[..2], ["done", "snooze"], "{payload}");
    let before = Utc::now();
    let (status, body) = act(setup, json!({"token": minted.token, "action": "snooze"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "snoozed", "{body}");
    let moved = remind_at(pool, &item).await.expect("snoozed");
    assert!(moved >= before + Duration::minutes(59), "{moved}");
    run_jobs(&fixture).await;
    let reminded = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT reminded_for FROM saved_items WHERE id = $1",
    )
    .bind(item.parse::<Uuid>().expect("item"))
    .fetch_one(pool)
    .await
    .expect("reminded");
    assert_eq!(reminded, Some(moved));

    // Done from the phone finishes it.
    let minted = push::actions::mint(pool, setup.organization_id, mara, &view)
        .await
        .expect("mint")
        .expect("still offers actions");
    let (status, body) = act(setup, json!({"token": minted.token, "action": "done"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "done", "{body}");
    assert_eq!(
        later(setup, &mara_browser, "open").await["items"],
        json!([])
    );
    assert_eq!(
        later(setup, &mara_browser, "done").await["items"][0]["id"],
        item.as_str()
    );
}

#[tokio::test]
async fn a_scheduled_send_posts_once_as_its_author_and_fails_cleanly_after_access_ends() {
    let Some(fixture) = fixture("w21_schedule").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let project = create_project(setup, "SCH", "Schedules").await;
    let space = direct_space(setup).await;
    let channel = create_channel(setup, &project, "updates").await;
    let (mara, mara_browser) = add_person(setup, &[&project, &space], "mara").await;
    let (_zoe, zoe_browser) = add_person(setup, &[&space], "zoe").await;
    let owner = owner_id(pool).await;
    let schedule = |browser: &Browser, content: Value, send_at: String| {
        let browser = Browser {
            cookie: browser.cookie.clone(),
            csrf: browser.csrf.clone(),
        };
        let uri = format!("/api/v1/conversations/{channel}/scheduled-messages");
        async move {
            call(
                setup,
                &browser,
                "POST",
                &uri,
                None,
                json!({"content": content, "sendAt": send_at}),
            )
            .await
        }
    };

    // Only where you can post, and only in the future.
    let (status, _) = schedule(&zoe_browser, text("Not mine"), in_an_hour()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = schedule(
        &mara_browser,
        text("Yesterday"),
        (Utc::now() - Duration::hours(1)).to_rfc3339(),
    )
    .await;
    assert!(status.is_client_error());

    // It sends as Mara once, with its mention resolved then.
    let (status, scheduled) = schedule(
        &mara_browser,
        mention(owner, " morning update"),
        in_an_hour(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{scheduled}");
    let first = scheduled["scheduledMessage"]["id"]
        .as_str()
        .expect("scheduled")
        .to_owned();
    let (status, listed) = get(setup, &mara_browser, "/api/v1/scheduled-messages").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["scheduledMessages"][0]["id"], first.as_str());
    let (_, others) = get(setup, &setup.browser, "/api/v1/scheduled-messages").await;
    assert_eq!(others["scheduledMessages"], json!([]), "{others}");
    // Nobody else can edit or cancel it.
    let (status, _) = call(
        setup,
        &setup.browser,
        "DELETE",
        &format!("/api/v1/scheduled-messages/{first}"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    run_jobs(&fixture).await;
    drain(setup).await;
    run_jobs(&fixture).await;
    let sent = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE author_actor_id = $1 AND conversation_id = $2",
    )
    .bind(mara)
    .bind(channel.parse::<Uuid>().expect("channel"))
    .fetch_one(pool)
    .await
    .expect("sent");
    assert_eq!(sent, 1);
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(first.parse::<Uuid>().expect("id"))
            .fetch_one(pool)
            .await
            .expect("status");
    assert_eq!(status, "sent");
    assert_eq!(
        notifications_of(pool, owner, notifications::kind::MENTION).await,
        1
    );

    // Once Mara loses the project, the next one fails, says so, and posts nothing.
    let (status, scheduled) = schedule(&mara_browser, text("After I leave"), in_an_hour()).await;
    assert_eq!(status, StatusCode::CREATED, "{scheduled}");
    let second = scheduled["scheduledMessage"]["id"]
        .as_str()
        .expect("scheduled")
        .to_owned();
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(mara)
    .execute(pool)
    .await
    .expect("remove Mara");
    run_jobs(&fixture).await;
    assert_eq!(messages_by(pool, mara, "After I leave").await, 0);
    let (status, failure) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, failure FROM scheduled_messages WHERE id = $1",
    )
    .bind(second.parse::<Uuid>().expect("id"))
    .fetch_one(pool)
    .await
    .expect("status");
    assert_eq!(status, "failed");
    assert!(failure.is_some());
    assert_eq!(
        notifications_of(pool, mara, notifications::kind::SCHEDULED_SEND_FAILED).await,
        1
    );
    // The failure names no more than Mara's own words.
    let (_, inbox) = get(setup, &mara_browser, "/api/v1/inbox").await;
    assert!(!inbox.to_string().contains("updates"), "{inbox}");
}

#[tokio::test]
async fn forwarding_posts_only_where_the_sender_can_and_quotes_only_to_who_can_see() {
    let Some(fixture) = fixture("w21_forward").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let project = create_project(setup, "FWD", "Forwards").await;
    let space = direct_space(setup).await;
    let channel = create_channel(setup, &project, "general").await;
    let (mara, mara_browser) = add_person(setup, &[&project, &space], "mara").await;
    let (pat, pat_browser) = add_person(setup, &[&project, &space], "pat").await;
    let group = group(setup, "group-secret", &[mara]).await;
    let secret = say(setup, &setup.browser, &group, "s1", text("Secret plan")).await;
    let open = say(setup, &setup.browser, &channel, "o1", text("Lunch at noon")).await;
    let forward = |browser: &Browser, message: &str, into: &str, key: &str| {
        let browser = Browser {
            cookie: browser.cookie.clone(),
            csrf: browser.csrf.clone(),
        };
        let uri = format!("/api/v1/messages/{message}/forward");
        let body = json!({"conversationId": into, "text": "FYI"});
        let key = key.to_owned();
        async move { call(setup, &browser, "POST", &uri, Some(&key), body).await }
    };

    // Pat can't forward what they can't see, nor into a group they aren't in.
    let (status, _) = forward(&pat_browser, &secret, &channel, "f1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = forward(&pat_browser, &open, &group, "f2").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(messages_by(pool, pat, "FYI").await, 0);

    // Mara forwards the group's message to the channel: the quote is read
    // live, for people who can see the group only.
    let (status, sent) = forward(&mara_browser, &secret, &channel, "f3").await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(
        sent["message"]["forwarded"]["text"], "Secret plan",
        "{sent}"
    );
    let (status, again) = forward(&mara_browser, &secret, &channel, "f3").await;
    assert!(status.is_success(), "{again}");
    assert_eq!(messages_by(pool, mara, "FYI").await, 1);
    let copied = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1 AND plain_text LIKE '%Secret%'",
    )
    .bind(channel.parse::<Uuid>().expect("channel"))
    .fetch_one(pool)
    .await
    .expect("copies");
    assert_eq!(copied, 0, "a forward never copies the original");
    let forwarded = |timeline: &Value| {
        timeline["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .find_map(|entry| entry["message"]["forwarded"].as_object().cloned())
            .expect("the forward")
    };
    let timeline_uri = format!("/api/v1/conversations/{channel}/timeline");
    let (_, owner_view) = get(setup, &setup.browser, &timeline_uri).await;
    assert_eq!(forwarded(&owner_view)["available"], true);
    assert_eq!(forwarded(&owner_view)["text"], "Secret plan");
    let (_, pat_view) = get(setup, &pat_browser, &timeline_uri).await;
    assert_eq!(forwarded(&pat_view)["available"], false, "{pat_view}");
    assert!(!pat_view.to_string().contains("Secret plan"), "{pat_view}");
    // Mara leaving the group hides it from her too.
    leave_group(setup, &group, mara).await;
    let (_, mara_view) = get(setup, &mara_browser, &timeline_uri).await;
    assert_eq!(forwarded(&mara_view)["available"], false);
    assert!(
        !mara_view.to_string().contains("Secret plan"),
        "{mara_view}"
    );

    // A message becomes a task linked back to it, only from what you can see.
    let tasks = format!("/api/v1/projects/{project}/work-items");
    let (status, task) = call(
        setup,
        &pat_browser,
        "POST",
        &tasks,
        Some("task-open"),
        json!({"title": "Book lunch", "sourceMessageId": open}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let links = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_links WHERE target_kind = 'message' AND target_id = $1",
    )
    .bind(open.parse::<Uuid>().expect("message"))
    .fetch_one(pool)
    .await
    .expect("links");
    assert_eq!(links, 1);
    let (status, _) = call(
        setup,
        &pat_browser,
        "POST",
        &tasks,
        Some("task-secret"),
        json!({"title": "Peek", "sourceMessageId": secret}),
    )
    .await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn notification_levels_follow_all_mentions_and_nothing() {
    let Some(fixture) = fixture("w21_levels").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let project = create_project(setup, "LVL", "Levels").await;
    let channel = create_channel(setup, &project, "general").await;
    let (mara, mara_browser) = add_person(setup, &[&project], "mara").await;
    let (pat, pat_browser) = add_person(setup, &[&project], "pat").await;
    let space = direct_space(setup).await;
    let (_zoe, zoe_browser) = add_person(setup, &[&space], "zoe").await;
    let level_uri = format!("/api/v1/conversations/{channel}/notification-level");

    let (status, level) = get(setup, &mara_browser, &level_uri).await;
    assert_eq!(status, StatusCode::OK, "{level}");
    assert_eq!(level["level"], "mentions");
    let (status, _) = set_level(setup, &zoe_browser, &channel, json!("all")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(setup, &zoe_browser, &level_uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // All: every message, except your own.
    let (status, body) = set_level(setup, &mara_browser, &channel, json!("all")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    say(setup, &setup.browser, &channel, "m1", text("Standup moved")).await;
    say(setup, &mara_browser, &channel, "m2", text("Thanks")).await;
    drain(setup).await;
    let message = notifications::kind::CONVERSATION_MESSAGE;
    assert_eq!(notifications_of(pool, mara, message).await, 1);
    assert_eq!(notifications_of(pool, pat, message).await, 0);

    // Nothing: not even a mention.
    let (status, _) = set_level(setup, &pat_browser, &channel, json!("nothing")).await;
    assert_eq!(status, StatusCode::OK);
    say(
        setup,
        &setup.browser,
        &channel,
        "m3",
        mention(pat, " please look"),
    )
    .await;
    drain(setup).await;
    assert_eq!(
        notifications_of(pool, pat, notifications::kind::MENTION).await,
        0
    );

    // Cleared, it's back to mentions only.
    let (status, _) = set_level(setup, &mara_browser, &channel, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    say(setup, &setup.browser, &channel, "m4", text("Another")).await;
    let (status, _) = set_level(setup, &pat_browser, &channel, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    say(setup, &setup.browser, &channel, "m5", mention(pat, " now?")).await;
    drain(setup).await;
    let unread_for_mara = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = $2 AND read_at IS NULL",
    )
    .bind(mara)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("notifications");
    // Repeats collapse into one row per conversation; it must not have
    // picked up "Another" after Mara stopped following.
    assert!(unread_for_mara <= 1);
    let latest = sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM notifications WHERE recipient_actor_id = $1 AND kind = $2 ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(mara)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("latest");
    assert!(!latest.to_string().contains("Another"), "{latest}");
    assert_eq!(
        notifications_of(pool, pat, notifications::kind::MENTION).await,
        1
    );
}
