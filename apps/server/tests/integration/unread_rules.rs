//! C2.2: one read path. Reading a conversation through an app acting for a
//! person is the same read as opening it in the web: the inbox's unread
//! count drops and a push queued for it sends nothing. "Mark all read" goes
//! through the same path, and never reads (or leaves a mark in) a
//! conversation the caller can't read.
//!
//! C2.3: the unread rules (C2 plan 3.3). Only messages count, your own never
//! unread a conversation, removal reads it again, muted stays unread but
//! uncounted, archived never counts, DM counts cap at 99, and an agent's or
//! app's briefing says what the web says.

use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::{jobs, push};

use super::{
    message_edits::{add_person, create_channel, say},
    notifications::{drain, inbox},
    support::{
        app::{Browser, connect_app, initialize, send, tool},
        callers::drain_outbox,
        world::WorldBuilder,
    },
    w9_capacity_and_push::{call, device_keys, fixture, options, push_jobs},
};

fn mention(actor: Uuid, text: &str) -> Value {
    json!({
        "version": 1,
        "nodes": [{"type": "text", "text": text}, {"type": "mention", "actorId": actor}]
    })
}

fn words(text: &str) -> Value {
    json!({"version": 1, "nodes": [{"type": "text", "text": text}]})
}

#[tokio::test]
async fn reading_through_an_app_reads_notifications() {
    let Some(fixture) = fixture("c2_2_read").await else {
        return;
    };
    let setup = &fixture.setup;
    let state = &fixture.state;
    let project = super::mcp_admin::create_project(setup, "RD", "Reading").await;
    let general = create_channel(setup, &project, "general").await;
    let (mara_id, mara) = add_person(setup, &[&project], "mara").await;

    // Mara has a phone, and is mentioned.
    let (status, device) = call(
        setup,
        &mara,
        "POST",
        "/api/v1/push/subscriptions",
        Some("mara-phone"),
        json!({
            "endpoint": "https://fcm.googleapis.com/fcm/send/c2-2-read",
            "keys": device_keys(),
            "organizationId": setup.organization_id,
        }),
    )
    .await;
    assert!(status.is_success(), "{device}");
    say(
        setup,
        &setup.browser,
        &general,
        "read-mention",
        mention(mara_id, "Look at this "),
    )
    .await;
    drain(setup).await;
    assert_eq!(inbox(setup, &mara, "").await["unreadCount"], 1);
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = $1 AND state = 'queued'",
    )
    .bind(push::DELIVER_KIND)
    .fetch_one(&setup.pool)
    .await
    .expect("queued pushes");
    assert_eq!(queued, 1);

    // Her app reads the channel for her.
    let token = connect_app(&setup.app, &mara, "Mara's app", "full").await;
    let session = initialize(&setup.app, &token).await.expect("app MCP");
    let read = tool(
        &setup.app,
        &token,
        &session,
        "mark_read",
        json!({"conversationId": general}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["unreadCount"], 0, "{read}");
    assert_eq!(inbox(setup, &mara, "").await["unreadCount"], 0);

    // The push queued before the read sends nothing.
    jobs::run_due(state, &push_jobs(), &options())
        .await
        .expect("run pushes");
    let last_sent = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT last_sent_at FROM push_subscriptions WHERE actor_id = $1",
    )
    .bind(mara_id)
    .fetch_one(&setup.pool)
    .await
    .expect("device");
    assert_eq!(last_sent, None, "no send was attempted");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM push_deliveries")
            .fetch_one(&setup.pool)
            .await
            .expect("deliveries"),
        0
    );
}

#[tokio::test]
async fn mark_all_read_reads_only_what_you_may_read() {
    let Some(world) = WorldBuilder::default()
        .project("ALL")
        .person("mara", "member")
        .person("nina", "member")
        .person("lena", "member")
        .dm("lena", "nina")
        .build("c2_2_all")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("ALL");
    let mara = &world.people["mara"];
    let lena = &world.people["lena"];
    let nina = &world.people["nina"];
    let dm = world.conversations["lena+nina"].clone();
    let post = |browser: &Browser, conversation: String, key: &'static str, content: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(key),
            json!({"content": content}),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{key}: {sent}");
        }
    };
    let mut channels = Vec::new();
    for name in ["general", "random"] {
        let (status, channel) = send(
            app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{project}/channels"),
                Some(&format!("all-{name}")),
                json!({"name": name}),
            ),
        )
        .await;
        assert!(status.is_success(), "{channel}");
        channels.push(
            channel["conversation"]["id"]
                .as_str()
                .or_else(|| channel["id"].as_str())
                .expect("channel")
                .to_owned(),
        );
    }
    // A DM between Lena and Nina: Mara is in its space but not in it.
    post(&lena.browser, dm.clone(), "all-dm", words("Only for Nina")).await;
    post(
        &world.owner.browser,
        channels[0].clone(),
        "all-general",
        mention(mara.actor, "First "),
    )
    .await;
    post(
        &world.owner.browser,
        channels[1].clone(),
        "all-random",
        words("Second"),
    )
    .await;
    drain_outbox(&world.state).await;
    let unread = |browser: &Browser| {
        let request = browser.request("GET", "/api/v1/inbox", None, Value::Null);
        async move { send(app, request).await.1["unreadCount"].clone() }
    };
    assert_eq!(unread(&mara.browser).await, 1);

    let token = connect_app(app, &mara.browser, "Mara's app", "full").await;
    let session = initialize(app, &token).await.expect("app MCP");
    let marked = tool(
        app,
        &token,
        &session,
        "conversations_mark_read",
        json!({"clientRequestId": "mark-all-1"}),
    )
    .await;
    assert_eq!(marked["outcome"], "ok", "{marked}");
    assert_eq!(marked["value"]["marked"], 2, "{marked}");
    assert_eq!(marked["value"]["more"], false, "{marked}");
    assert_eq!(unread(&mara.browser).await, 0);

    // The DM she isn't in has no read mark of hers, and Nina's is untouched.
    for (actor, who) in [(mara.actor, "Mara"), (nina.actor, "Nina")] {
        let marks = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_read_states WHERE conversation_id = $1::uuid AND actor_id = $2 AND highest_seen_sequence > 0",
        )
        .bind(&dm)
        .bind(actor)
        .fetch_one(pool)
        .await
        .expect("marks");
        assert_eq!(marks, 0, "{who}'s mark in the DM");
    }

    // Nothing is left to read, in the workspace or in the space.
    let again = tool(
        app,
        &token,
        &session,
        "conversations_mark_read",
        json!({"spaceId": project, "clientRequestId": "mark-all-2"}),
    )
    .await;
    assert_eq!(again["value"]["marked"], 0, "{again}");

    world.finish().await;
}

#[tokio::test]
async fn unread_follows_the_rules() {
    let Some(world) = WorldBuilder::default()
        .project("UN")
        .person("lena", "member")
        .person("mara", "member")
        .dm("lena", "mara")
        .build("c2_3_unread")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("UN").to_owned();
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let dm = world.conversations["lena+mara"].clone();
    let post = |browser: &Browser, conversation: &str, key: String, text: &str| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&key),
            json!({"content": words(text)}),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{key}: {sent}");
            sent["message"]["id"]
                .as_str()
                .or_else(|| sent["id"].as_str())
                .expect("message id")
                .to_owned()
        }
    };
    let get = |browser: &Browser, path: String| {
        let request = browser.request("GET", &path, None, Value::Null);
        async move {
            let (status, body) = send(app, request).await;
            assert!(status.is_success(), "{path}: {body}");
            body
        }
    };
    let find = |list: &Value, id: &str| {
        list.as_array()
            .expect("list")
            .iter()
            .find(|item| item["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not in {list}"))
    };
    let channel_of = |browser: &'static str, id: String| {
        let who = &world.people[browser];
        let path = format!("/api/v1/projects/{project}/channels");
        async move { find(&get(&who.browser, path).await["channels"], &id) }
    };
    let dm_of = |browser: &'static str| {
        let who = &world.people[browser];
        let dm = dm.clone();
        async move {
            find(
                &get(&who.browser, "/api/v1/direct".to_owned()).await["conversations"],
                &dm,
            )
        }
    };

    let (status, created) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("unread-general"),
            json!({"name": "general"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let general = created["conversation"]["id"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .expect("channel")
        .to_owned();

    // R1: an entry that isn't a message (a worker's or the system's) never
    // makes it unread.
    sqlx::query(
        r#"
        WITH next AS (
            UPDATE conversations SET next_entry_sequence = next_entry_sequence + 1
            WHERE id = $1 RETURNING next_entry_sequence - 1 AS sequence
        )
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind)
        SELECT $2, $1, sequence, 'system' FROM next
        "#,
    )
    .bind(Uuid::parse_str(&general).expect("id"))
    .bind(Uuid::now_v7())
    .execute(pool)
    .await
    .expect("system entry");
    let item = channel_of("mara", general.clone()).await;
    assert_eq!(item["unread"], false, "{item}");

    // Someone else's message does, and counts.
    let only = post(&lena.browser, &general, "unread-1".into(), "Hello").await;
    let item = channel_of("mara", general.clone()).await;
    assert_eq!(item["unread"], true, "{item}");
    assert_eq!(item["unreadMessages"], 1, "{item}");
    assert_eq!(item["muted"], false, "{item}");

    // R4: removing the only unread message reads it.
    let (status, removed) = send(
        app,
        lena.browser.request(
            "DELETE",
            &format!("/api/v1/conversations/{general}/messages/{only}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{removed}");
    let item = channel_of("mara", general.clone()).await;
    assert_eq!(item["unread"], false, "{item}");
    assert_eq!(item["unreadMessages"], 0, "{item}");

    // R2: her own message, sent by her app, leaves it read, and so does a
    // forward.
    let token = connect_app(app, &mara.browser, "Mara's app", "full").await;
    let session = initialize(app, &token).await.expect("app MCP");
    let original = post(&lena.browser, &general, "unread-2".into(), "Worth sharing").await;
    let sent = tool(
        app,
        &token,
        &session,
        "message_send",
        json!({"conversationId": general, "text": "Seen it", "clientRequestId": "unread-own"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let item = channel_of("mara", general.clone()).await;
    assert_eq!(item["unread"], false, "{item}");
    let forwarded = tool(
        app,
        &token,
        &session,
        "message_forward",
        json!({"messageId": original, "conversationId": dm, "clientRequestId": "unread-fwd"}),
    )
    .await;
    assert_eq!(forwarded["outcome"], "ok", "{forwarded}");
    let mine = dm_of("mara").await;
    assert_eq!(mine["unread"], false, "{mine}");
    let hers = dm_of("lena").await;
    assert_eq!(hers["unread"], true, "{hers}");
    assert_eq!(hers["unreadMessages"], 1, "{hers}");

    // A DM shows its number: 3 after 3 messages, and never more than 99.
    for index in 0..3 {
        post(&lena.browser, &dm, format!("unread-dm-{index}"), "Ping").await;
    }
    assert_eq!(dm_of("mara").await["unreadMessages"], 3);
    for index in 3..101 {
        post(&lena.browser, &dm, format!("unread-dm-{index}"), "Ping").await;
    }
    let mine = dm_of("mara").await;
    assert_eq!(mine["unread"], true, "{mine}");
    assert_eq!(mine["unreadMessages"], 99, "{mine}");

    // R5: muted stays unread but says so; an app's briefing leaves it out.
    let (status, level) = send(
        app,
        mara.browser.request(
            "PUT",
            &format!("/api/v1/conversations/{general}/notification-level"),
            Some("unread-mute"),
            json!({"level": "nothing"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{level}");
    post(&lena.browser, &general, "unread-3".into(), "Quietly").await;
    let item = channel_of("mara", general.clone()).await;
    assert_eq!(item["unread"], true, "{item}");
    assert_eq!(item["muted"], true, "{item}");

    // R10: the briefing agrees with the web, conversation by conversation.
    let briefing = tool(app, &token, &session, "briefing", json!({})).await;
    assert_eq!(briefing["outcome"], "ok", "{briefing}");
    let unread = briefing["value"]["unread"].as_array().expect("unread");
    assert!(
        unread.iter().all(|item| item["id"] != general.as_str()),
        "a muted channel isn't in the briefing: {briefing}"
    );
    let in_briefing = unread
        .iter()
        .find(|item| item["id"] == dm.as_str())
        .unwrap_or_else(|| panic!("the DM is unread: {briefing}"));
    assert_eq!(in_briefing["unreadCount"], 99, "{briefing}");

    // R7: an archived conversation never counts.
    let (status, archived) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/archive"),
            None,
            json!({"expectedVersion": created["conversation"]["version"].as_i64().unwrap_or(1)}),
        ),
    )
    .await;
    assert!(status.is_success(), "{archived}");
    let sidebar = get(
        &mara.browser,
        format!("/api/v1/projects/{project}/conversation-sidebar?includeArchived=true"),
    )
    .await;
    let item = find(&sidebar["channels"], &general);
    assert_eq!(item["unread"], false, "{item}");
    assert_eq!(item["unreadMessages"], 0, "{item}");

    world.finish().await;
}

/// C2.4: the badge counts the places that need you, once each, on the
/// server: five mentions in one channel and two DMs are three, and a run
/// finishing (for your information) adds nothing. Each notification says
/// which it is.
#[tokio::test]
async fn the_badge_counts_what_needs_you() {
    let Some(world) = WorldBuilder::default()
        .project("BD")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .dm("lena", "mara")
        .dm("nina", "mara")
        .build("c2_4_badge")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("BD").to_owned();
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let nina = &world.people["nina"];
    let (status, created) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("badge-general"),
            json!({"name": "general"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let general = created["conversation"]["id"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .expect("channel")
        .to_owned();
    let post = |browser: &Browser, conversation: String, key: String, content: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&key),
            json!({ "content": content }),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{key}: {sent}");
        }
    };
    for n in 0..5 {
        post(
            &lena.browser,
            general.clone(),
            format!("badge-mention-{n}"),
            mention(mara.actor, "Look "),
        )
        .await;
    }
    post(
        &lena.browser,
        world.conversations["lena+mara"].clone(),
        "badge-dm-lena".to_owned(),
        words("Hi"),
    )
    .await;
    post(
        &nina.browser,
        world.conversations["nina+mara"].clone(),
        "badge-dm-nina".to_owned(),
        words("Hello"),
    )
    .await;
    drain_outbox(&world.state).await;
    // A run finishing is for your information.
    sqlx::query(
        "INSERT INTO notifications (id, project_id, recipient_actor_id, kind, subject_kind, \
         subject_id, group_key) VALUES ($1, $2::uuid, $3, 'run_completed', 'agent_run', $1, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(&project)
    .bind(mara.actor)
    .bind(format!("run:{}", Uuid::now_v7()))
    .execute(pool)
    .await
    .expect("a finished run");

    let (status, summary) = send(
        app,
        mara.browser
            .request("GET", "/api/v1/inbox/summary", None, Value::Null),
    )
    .await;
    assert!(status.is_success(), "{summary}");
    assert_eq!(summary["needsYouConversations"], 3, "{summary}");
    assert_eq!(summary["fyiUnread"], 1, "{summary}");
    let (status, listed) = send(
        app,
        mara.browser
            .request("GET", "/api/v1/inbox?limit=50", None, Value::Null),
    )
    .await;
    assert!(status.is_success(), "{listed}");
    for notification in listed["notifications"].as_array().expect("notifications") {
        let expected = if notification["kind"] == "run_completed" {
            "fyi"
        } else {
            "needs_you"
        };
        assert_eq!(notification["attention"], expected, "{notification}");
    }

    world.finish().await;
}
