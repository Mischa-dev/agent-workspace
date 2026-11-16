//! C2.2: one read path. Reading a conversation through an app acting for a
//! person is the same read as opening it in the web: the inbox's unread
//! count drops and a push queued for it sends nothing. "Mark all read" goes
//! through the same path, and never reads (or leaves a mark in) a
//! conversation the caller can't read.

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
