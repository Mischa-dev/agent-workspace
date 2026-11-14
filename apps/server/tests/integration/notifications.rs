//! Notifications and the inbox (roadmap N1, N2). The costly failures are
//! silent: a notification (or its preview) reaching someone who can't see
//! the conversation, a removed message's text surviving in the inbox, one
//! occurrence delivered twice, someone changing another person's
//! notifications, and a comment @mention that reaches nobody.

use agent_workspace_server::realtime::{RealtimeHub, publish_ready_outbox_batch};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    message_edits::{add_person, create_channel, direct_space, get, owner_id, remove, say},
    support::{
        app::{Browser, connect_app, initialize, send, tool},
        world::{Setup, create_project, setup},
    },
};

/// Publishes everything in the outbox, including the events notifications
/// themselves enqueue.
pub(super) async fn drain(setup: &Setup) {
    let hub = RealtimeHub::new(1024);
    for _ in 0..50 {
        let published = publish_ready_outbox_batch(&setup.pool, &hub, "notifications-test")
            .await
            .expect("publish outbox batch");
        if published == 0 {
            return;
        }
    }
    panic!("outbox never drained");
}

pub(super) async fn inbox(setup: &Setup, browser: &Browser, query: &str) -> Value {
    let (status, page) = get(setup, browser, &format!("/api/v1/inbox{query}")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

fn items(page: &Value) -> &Vec<Value> {
    page["notifications"].as_array().expect("notifications")
}

fn mention(actor: Uuid, text: &str) -> Value {
    json!({
        "version": 1,
        "nodes": [
            {"type": "text", "text": text},
            {"type": "mention", "actorId": actor}
        ]
    })
}

async fn update(setup: &Setup, browser: &Browser, body: Value) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request("POST", "/api/v1/inbox/update", None, body),
    )
    .await
}

async fn attention(setup: &Setup, browser: &Browser, project: &str) -> i64 {
    let (status, projects) = get(setup, browser, "/api/v1/projects").await;
    assert_eq!(status, StatusCode::OK, "{projects}");
    projects["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|candidate| candidate["id"] == project)
        .map_or(0, |found| found["attentionCount"].as_i64().expect("count"))
}

#[tokio::test]
async fn mentions_reach_the_person_once_and_follow_the_message() {
    let Some(setup) = setup("notify_mentions").await else {
        return;
    };
    let project = create_project(&setup, "NTF", "Notify").await;
    let general = create_channel(&setup, &project, "general").await;
    let owner = owner_id(&setup.pool).await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let (_ben, ben_browser) = add_person(&setup, &[&project], "ben").await;

    let first = say(
        &setup,
        &setup.browser,
        &general,
        "m1",
        mention(mara, "Look at this palette "),
    )
    .await;
    drain(&setup).await;

    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    let item = &items(&page)[0];
    assert_eq!(item["kind"], "mention");
    assert_eq!(item["title"], "Owner mentioned you in #general");
    // The actor carries its kind, so the inbox can draw an agent as an agent.
    assert_eq!(item["actor"]["kind"], "human", "{item}");
    assert!(
        item["preview"]
            .as_str()
            .expect("preview")
            .contains("palette")
    );
    assert_eq!(item["read"], false);
    assert_eq!(item["count"], 1);
    assert_eq!(
        item["href"],
        format!("/projects/{project}/channels/{general}#message-{first}")
    );
    assert_eq!(page["unreadCount"], 1);
    assert_eq!(attention(&setup, &mara_browser, &project).await, 1);

    // Nobody else hears about it, least of all the author.
    assert!(items(&inbox(&setup, &ben_browser, "").await).is_empty());
    assert!(items(&inbox(&setup, &setup.browser, "").await).is_empty());
    assert_eq!(attention(&setup, &setup.browser, &project).await, 0);

    // A second mention in the same channel collapses into the first.
    let second = say(
        &setup,
        &setup.browser,
        &general,
        "m2",
        mention(mara, "And this one "),
    )
    .await;
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    assert_eq!(items(&page)[0]["count"], 2);
    assert!(
        items(&page)[0]["preview"]
            .as_str()
            .expect("preview")
            .contains("this one")
    );

    // Redelivered events are applied once.
    sqlx::query("UPDATE outbox_events SET published_at = NULL, attempt_count = 0 WHERE event_type = 'conversation.message.created'")
        .execute(&setup.pool)
        .await
        .expect("replay events");
    drain(&setup).await;
    assert_eq!(
        items(&inbox(&setup, &mara_browser, "").await)[0]["count"],
        2
    );
    let rows = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notification_occurrences WHERE recipient_actor_id = $1",
    )
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("occurrences");
    assert_eq!(rows, 2);

    // A removed message drops out, and its text was never copied.
    let (status, removed) = remove(&setup, &setup.browser, &general, &second).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page)[0]["count"], 1, "{page}");
    assert!(
        items(&page)[0]["preview"]
            .as_str()
            .expect("preview")
            .contains("palette")
    );
    for table in ["notifications", "notification_occurrences", "outbox_events"] {
        assert_eq!(
            super::message_edits::copies(&setup.pool, table, "this one").await,
            0,
            "{table} keeps a copy"
        );
    }
    let (status, _) = remove(&setup, &setup.browser, &general, &first).await;
    assert_eq!(status, StatusCode::OK);
    drain(&setup).await;
    assert!(items(&inbox(&setup, &mara_browser, "").await).is_empty());

    // Losing access hides what was delivered.
    say(
        &setup,
        &setup.browser,
        &general,
        "m3",
        mention(mara, "Still there? "),
    )
    .await;
    drain(&setup).await;
    assert_eq!(items(&inbox(&setup, &mara_browser, "").await).len(), 1);
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project.parse::<Uuid>().expect("project"))
    .bind(mara)
    .execute(&setup.pool)
    .await
    .expect("disable membership");
    let page = inbox(&setup, &mara_browser, "").await;
    assert!(items(&page).is_empty(), "{page}");
    assert_eq!(page["unreadCount"], 0);
    let _ = owner;
}

#[tokio::test]
async fn private_conversations_notify_only_people_inside() {
    let Some(setup) = setup("notify_private").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, mara_browser) = add_person(&setup, &[&space], "mara").await;
    let (pat, pat_browser) = add_person(&setup, &[&space], "pat").await;
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("group-one"),
            json!({ "memberIds": [mara, pat] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    say(
        &setup,
        &setup.browser,
        &group,
        "g1",
        mention(mara, "Secret plan "),
    )
    .await;
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    assert!(
        items(&page)[0]["href"]
            .as_str()
            .expect("href")
            .starts_with(&format!("/messages/{group}#message-"))
    );
    assert_eq!(items(&page)[0]["projectName"], "Messages");
    let item = items(&page)[0].clone();
    assert!(items(&inbox(&setup, &pat_browser, "").await).is_empty());

    // Once removed from the group, what was delivered there is hidden.
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/members/{mara}/remove"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page = inbox(&setup, &mara_browser, "").await;
    assert!(items(&page).is_empty(), "{page}");
    assert_eq!(page["unreadCount"], 0);

    // Someone else's notification can't be changed.
    let (status, result) = update(
        &setup,
        &pat_browser,
        json!({"action": "archive", "ids": [item["id"]]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["updated"], json!([]));
}

#[tokio::test]
async fn a_comment_mention_reaches_the_person_and_the_attention_count() {
    let Some(setup) = setup("notify_tasks").await else {
        return;
    };
    let project = create_project(&setup, "TSK", "Tasks").await;
    let (ben, ben_browser) = add_person(&setup, &[&project], "ben").await;
    let base = format!("/api/v1/projects/{project}/work-items");

    // Assigned on creation.
    let (status, created) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &base,
            Some("task-one"),
            json!({"title": "Pick theme colours", "assigneeActorId": ben}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task = created["id"].as_str().expect("task").to_owned();
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=tasks").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    assert_eq!(items(&page)[0]["kind"], "assigned");
    assert_eq!(items(&page)[0]["title"], "Owner assigned you TSK-1");
    assert_eq!(
        items(&page)[0]["href"],
        format!("/projects/{project}/work-items/{task}")
    );

    // Read it, so the comment is what brings the count back.
    let (status, _) = update(&setup, &ben_browser, json!({"action": "read", "all": true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(attention(&setup, &ben_browser, &project).await, 0);

    let (status, comment) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment-one"),
            json!({"content": mention(ben, "Can you check the palette, ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=mentions").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    assert_eq!(items(&page)[0]["kind"], "comment_mention");
    assert_eq!(items(&page)[0]["title"], "Owner mentioned you on TSK-1");
    assert!(
        items(&page)[0]["preview"]
            .as_str()
            .expect("preview")
            .contains("palette")
    );
    assert_eq!(attention(&setup, &ben_browser, &project).await, 1);

    // Ben watches the task he was assigned; Cara is in the project but
    // doesn't watch it. A rename reaches Ben only.
    let (_cara, cara_browser) = add_person(&setup, &[&project], "cara").await;
    let (status, updated) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("{base}/{task}"),
            Some("rename-one"),
            json!({"expectedVersion": created["version"], "title": "Pick the theme colours"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=tasks&view=unread").await;
    let watched = items(&page)
        .iter()
        .find(|item| item["kind"] == "task_changed")
        .unwrap_or_else(|| panic!("{page}"));
    assert_eq!(watched["title"], "Owner updated TSK-1");
    assert_eq!(page["unreadCount"], 2);
    assert!(items(&inbox(&setup, &cara_browser, "").await).is_empty());

    // A plain comment reaches watchers, and repeats collapse into one row.
    let (status, comment) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment-two"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Went with teal"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=tasks").await;
    let watched = items(&page)
        .iter()
        .filter(|item| item["kind"] == "task_changed")
        .collect::<Vec<_>>();
    assert_eq!(watched.len(), 1, "{page}");
    assert_eq!(watched[0]["title"], "Owner commented on TSK-1");
    assert_eq!(watched[0]["preview"], "Went with teal");
    assert_eq!(watched[0]["count"], 2);

    // Once Ben stops watching, changes stop reaching him.
    sqlx::query("DELETE FROM work_item_watchers WHERE work_item_id = $1::uuid AND actor_id = $2")
        .bind(&task)
        .bind(ben)
        .execute(&setup.pool)
        .await
        .expect("stop watching");
    let (status, _) = update(
        &setup,
        &ben_browser,
        json!({"action": "archive", "all": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, comment) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment-three"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "One more"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    drain(&setup).await;
    assert!(items(&inbox(&setup, &ben_browser, "").await).is_empty());
}

#[tokio::test]
async fn people_read_archive_snooze_and_restore_their_own() {
    let Some(setup) = setup("notify_actions").await else {
        return;
    };
    let project = create_project(&setup, "ACT", "Actions").await;
    let general = create_channel(&setup, &project, "general").await;
    let random = create_channel(&setup, &project, "random").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    say(
        &setup,
        &setup.browser,
        &general,
        "a1",
        mention(mara, "One "),
    )
    .await;
    say(&setup, &setup.browser, &random, "a2", mention(mara, "Two ")).await;
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 2);
    assert_eq!(page["unreadCount"], 2);
    let newest = items(&page)[0]["id"].clone();
    let oldest = items(&page)[1]["id"].clone();

    // Pages by cursor.
    let first = inbox(&setup, &mara_browser, "?limit=1").await;
    assert_eq!(items(&first).len(), 1);
    let cursor = first["nextCursor"].as_str().expect("cursor");
    let second = inbox(&setup, &mara_browser, &format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(items(&second)[0]["id"], oldest);
    assert!(second["nextCursor"].is_null());

    let (status, result) = update(
        &setup,
        &mara_browser,
        json!({"action": "read", "ids": [newest]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["unreadCount"], 1);

    let (status, result) = update(
        &setup,
        &mara_browser,
        json!({"action": "archive", "ids": [newest]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(items(&inbox(&setup, &mara_browser, "").await).len(), 1);
    assert_eq!(
        items(&inbox(&setup, &mara_browser, "?view=archived").await).len(),
        1
    );

    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let (status, result) = update(
        &setup,
        &mara_browser,
        json!({"action": "snooze", "ids": [oldest], "snoozeUntil": tomorrow}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["unreadCount"], 0);
    assert!(items(&inbox(&setup, &mara_browser, "").await).is_empty());
    assert_eq!(
        items(&inbox(&setup, &mara_browser, "?view=snoozed").await).len(),
        1
    );

    // A past snooze is refused.
    let (status, _) = update(
        &setup,
        &mara_browser,
        json!({"action": "snooze", "ids": [oldest], "snoozeUntil": "2020-01-01T00:00:00Z"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // When the snooze ends it comes back unread.
    sqlx::query("UPDATE notifications SET snoozed_until = now() - interval '1 minute' WHERE recipient_actor_id = $1")
        .bind(mara)
        .execute(&setup.pool)
        .await
        .expect("end snooze");
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 1);
    assert_eq!(page["unreadCount"], 1);

    let (status, result) = update(
        &setup,
        &mara_browser,
        json!({"action": "restore", "ids": [newest]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(items(&inbox(&setup, &mara_browser, "").await).len(), 2);

    // A new mention in an archived group starts a fresh row.
    let (status, _) = update(
        &setup,
        &mara_browser,
        json!({"action": "archive", "all": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    say(
        &setup,
        &setup.browser,
        &general,
        "a3",
        mention(mara, "Three "),
    )
    .await;
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(items(&page).len(), 1, "{page}");
    assert_eq!(items(&page)[0]["count"], 1);

    // The browser is told about each change.
    let announced = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE event_type = 'notifications.updated' AND resource_id = $1",
    )
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("announcements");
    assert!(announced >= 3, "{announced}");
}

#[tokio::test]
async fn apps_connected_by_a_person_see_their_notifications() {
    let Some(setup) = setup("notify_mcp").await else {
        return;
    };
    let project = create_project(&setup, "MCN", "Mcp").await;
    let general = create_channel(&setup, &project, "general").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    say(
        &setup,
        &setup.browser,
        &general,
        "c1",
        mention(mara, "Ping "),
    )
    .await;
    drain(&setup).await;

    let token = connect_app(&setup.app, &mara_browser, "Mara's app", "full").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let briefing = tool(&setup.app, &token, &session, "briefing", json!({})).await;
    assert_eq!(briefing["outcome"], "ok", "{briefing}");
    assert_eq!(
        briefing["value"]["notifications"][0]["kind"], "mention",
        "{briefing}"
    );

    let listed = tool(&setup.app, &token, &session, "notification_list", json!({})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let id = listed["value"]["notifications"][0]["id"].clone();
    assert_eq!(listed["value"]["unreadCount"], 1);

    let marked = tool(
        &setup.app,
        &token,
        &session,
        "notification_update",
        json!({"ids": [id], "action": "archive"}),
    )
    .await;
    assert_eq!(marked["outcome"], "ok", "{marked}");
    assert_eq!(marked["value"]["unreadCount"], 0);
    assert!(items(&inbox(&setup, &mara_browser, "").await).is_empty());

    // A read-only connection can look but not change.
    let reader = connect_app(&setup.app, &mara_browser, "Reader", "read").await;
    let reader_session = initialize(&setup.app, &reader).await.expect("reader");
    let refused = tool(
        &setup.app,
        &reader,
        &reader_session,
        "notification_update",
        json!({"ids": [id], "action": "restore"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
}

fn text(text: &str) -> Value {
    json!({"version": 1, "nodes": [{"type": "text", "text": text}]})
}

#[tokio::test]
async fn everyone_reaches_the_people_who_can_see_the_conversation() {
    let Some(setup) = setup("notify_everyone").await else {
        return;
    };
    let project = create_project(&setup, "ALL", "Everyone").await;
    let general = create_channel(&setup, &project, "general").await;
    let space = direct_space(&setup).await;
    let (mara, mara_browser) = add_person(&setup, &[&project, &space], "mara").await;
    let (_ben, ben_browser) = add_person(&setup, &[&project], "ben").await;
    let (_pat, pat_browser) = add_person(&setup, &[&space], "pat").await;

    // An email address or a longer word isn't @everyone.
    say(
        &setup,
        &ben_browser,
        &general,
        "e0",
        text("Write to team@everyone.dev or @everyones"),
    )
    .await;
    drain(&setup).await;
    assert!(items(&inbox(&setup, &mara_browser, "").await).is_empty());

    // In a channel, every member who can see it hears, but not the author.
    say(
        &setup,
        &ben_browser,
        &general,
        "e1",
        text("@here standup in 5"),
    )
    .await;
    drain(&setup).await;
    for browser in [&setup.browser, &mara_browser] {
        let page = inbox(&setup, browser, "").await;
        assert_eq!(items(&page).len(), 1, "{page}");
        assert_eq!(items(&page)[0]["kind"], "mention");
    }
    assert!(items(&inbox(&setup, &ben_browser, "").await).is_empty());
    assert!(items(&inbox(&setup, &pat_browser, "").await).is_empty());

    // In a group, only the people inside it, even though others share the
    // Messages space; naming someone as well notifies them once.
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("group-everyone"),
            json!({ "memberIds": [mara] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    say(
        &setup,
        &setup.browser,
        &group,
        "e2",
        json!({"version": 1, "nodes": [
            {"type": "text", "text": "@Everyone, and especially "},
            {"type": "mention", "actorId": mara}
        ]}),
    )
    .await;
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    let in_group = items(&page)
        .iter()
        .filter(|item| {
            item["href"]
                .as_str()
                .is_some_and(|href| href.starts_with(&format!("/messages/{group}")))
        })
        .collect::<Vec<_>>();
    assert_eq!(in_group.len(), 1, "{page}");
    assert_eq!(in_group[0]["count"], 1, "{page}");
    assert!(items(&inbox(&setup, &pat_browser, "").await).is_empty());
}

/// P0.5: a person reading through a connected app reads their mentions too,
/// with mark_read and with conversation_read's markRead, the same as in the
/// browser, so the inbox empties and no push or email goes out later.
#[tokio::test]
async fn reading_through_a_connected_app_reads_the_mentions() {
    let Some(setup) = setup("notify_app_reads").await else {
        return;
    };
    let project = create_project(&setup, "NTA", "Notify apps").await;
    let general = create_channel(&setup, &project, "general").await;
    let random = create_channel(&setup, &project, "random").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    say(
        &setup,
        &setup.browser,
        &general,
        "a1",
        mention(mara, "One "),
    )
    .await;
    say(&setup, &setup.browser, &random, "a2", mention(mara, "Two ")).await;
    drain(&setup).await;
    assert_eq!(inbox(&setup, &mara_browser, "").await["unreadCount"], 2);

    let token = connect_app(&setup.app, &mara_browser, "Mara's app", "full").await;
    let session = initialize(&setup.app, &token).await.expect("app MCP");
    let marked = tool(
        &setup.app,
        &token,
        &session,
        "mark_read",
        json!({"conversationId": general}),
    )
    .await;
    assert_eq!(marked["outcome"], "ok", "{marked}");
    assert_eq!(inbox(&setup, &mara_browser, "").await["unreadCount"], 1);

    let read = tool(
        &setup.app,
        &token,
        &session,
        "conversation_read",
        json!({"conversationId": random, "markRead": true}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(inbox(&setup, &mara_browser, "").await["unreadCount"], 0);
    let unread = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND read_at IS NULL",
    )
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("unread notifications");
    assert_eq!(unread, 0);
}
