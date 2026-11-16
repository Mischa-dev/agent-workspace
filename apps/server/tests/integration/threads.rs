//! C1.2: every way a thread is started goes through one function
//! (`conversations::threads::create`). Each entry point here makes a thread
//! with an audit row, the creator's read state and exactly one
//! `conversation.created` event; a thread started from a message without a
//! name goes by the message and loses its words when the message is
//! removed; and nobody starts a thread in a channel they can't see.

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::support::{
    app::{send, tool},
    world::WorldBuilder,
};

fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

/// What every new thread must have, whichever path made it.
async fn assert_made_once(pool: &PgPool, thread: &str, creator: Uuid, path: &str) {
    let thread = thread.parse::<Uuid>().expect("thread id");
    let audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE action = 'conversation.thread_created' AND target_id = $1 AND actor_id = $2",
    )
    .bind(thread)
    .bind(creator)
    .fetch_one(pool)
    .await
    .expect("audit");
    assert_eq!(audits, 1, "{path}: one audit row");
    let read_states = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversation_read_states WHERE conversation_id = $1 AND actor_id = $2",
    )
    .bind(thread)
    .bind(creator)
    .fetch_one(pool)
    .await
    .expect("read state");
    assert_eq!(read_states, 1, "{path}: the creator's read state");
    let events = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE event_type = 'conversation.created' AND resource_id = $1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .expect("events");
    assert_eq!(events, 1, "{path}: one conversation.created");
}

async fn thread_name(pool: &PgPool, thread: &str) -> (String, bool) {
    sqlx::query_as::<_, (String, bool)>(
        "SELECT name, auto_named FROM conversations WHERE id = $1::uuid",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .expect("thread row")
}

#[tokio::test]
async fn every_entry_point_makes_a_thread_the_same_way() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .build("c1_2_threads")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("WORK");
    let lena = &world.people["lena"];
    let rook = &world.agents["rook"];

    let (status, channel) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("threads-general"),
            json!({"name": "general"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let general = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let post = |key: &'static str, content: Value| {
        let general = general.clone();
        async move {
            let (status, sent) = send(
                app,
                lena.browser.request(
                    "POST",
                    &format!("/api/v1/conversations/{general}/messages"),
                    Some(key),
                    json!({ "content": content }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{sent}");
            sent["message"]["id"].as_str().expect("message").to_owned()
        }
    };

    // 1. The web dialog, from a message and without a name.
    let first = post("threads-m1", text("Plan the launch party")).await;
    let (status, web) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/threads"),
            Some("threads-web"),
            json!({"originMessageId": first}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{web}");
    let web_thread = web["conversation"]["id"].as_str().expect("web").to_owned();
    assert_made_once(pool, &web_thread, lena.actor, "web").await;
    assert_eq!(
        thread_name(pool, &web_thread).await,
        ("Plan the launch party".to_owned(), true)
    );

    // 2. thread_start, from a message: named from it until someone names it.
    let secret = format!("SECRET-{}", Uuid::now_v7().simple());
    let second = post("threads-m2", text(&format!("deploy key {secret}"))).await;
    let started = tool(
        app,
        &rook.token,
        &rook.session,
        "thread_start",
        json!({"conversationId": general, "fromMessageId": second}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let mcp_thread = started["value"]["id"].as_str().expect("mcp").to_owned();
    assert_made_once(pool, &mcp_thread, rook.actor, "thread_start").await;
    assert!(thread_name(pool, &mcp_thread).await.1);
    // Starting it again finds the same thread, and makes nothing new.
    let again = tool(
        app,
        &rook.token,
        &rook.session,
        "thread_start",
        json!({"conversationId": general, "fromMessageId": second}),
    )
    .await;
    assert_eq!(again["value"]["id"], mcp_thread.as_str(), "{again}");
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_made_once(pool, &mcp_thread, rook.actor, "thread_start again").await;
    // Removing the message takes its words out of the thread's name.
    let (status, removed) = send(
        app,
        lena.browser.request(
            "DELETE",
            &format!("/api/v1/conversations/{general}/messages/{second}"),
            Some("threads-remove"),
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{removed}");
    let (name, _) = thread_name(pool, &mcp_thread).await;
    assert!(!name.contains(&secret), "renamed after removal: {name}");

    // 3. An agent claiming a channel tag answers in a thread from it.
    let tag = post(
        "threads-m3",
        json!({"version": 1, "nodes": [
            {"type": "mention", "actorId": rook.actor},
            {"type": "text", "text": " can you check the build?"}
        ]}),
    )
    .await;
    let inbox = tool(app, &rook.token, &rook.session, "inbox_list", json!({})).await;
    let request_id = inbox["value"]["requests"]
        .as_array()
        .and_then(|requests| {
            requests
                .iter()
                .find(|request| request["messageId"] == tag.as_str())
        })
        .and_then(|request| request["requestId"].as_str())
        .unwrap_or_else(|| panic!("a request for the tag: {inbox}"))
        .to_owned();
    let claim = tool(
        app,
        &rook.token,
        &rook.session,
        "reply_claim",
        json!({"requestId": request_id}),
    )
    .await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let answer_thread = claim["value"]["request"]["threadId"]
        .as_str()
        .expect("answer thread")
        .to_owned();
    assert_made_once(pool, &answer_thread, rook.actor, "reply_claim").await;
    let reply_audit = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'conversation.thread_created' AND target_id = $1::uuid",
    )
    .bind(&answer_thread)
    .fetch_one(pool)
    .await
    .expect("reply audit");
    assert_eq!(reply_audit["reply_request_id"], request_id.as_str());

    // A thread can't go inside another thread, by any path.
    let (status, nested) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{web_thread}/threads"),
            Some("threads-nested"),
            json!({"name": "nested"}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{nested}");
    let nested = tool(
        app,
        &rook.token,
        &rook.session,
        "thread_start",
        json!({"conversationId": web_thread, "name": "nested"}),
    )
    .await;
    assert_eq!(nested["outcome"], "invalid_request", "{nested}");

    // A private channel the caller isn't in: no thread, by any path.
    let private = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) VALUES ($1, $2::uuid, 'channel', 'private-ops', 'members')",
    )
    .bind(private)
    .bind(project)
    .execute(pool)
    .await
    .expect("private channel");
    let (status, refused) = send(
        app,
        world.people["mara"].browser.request(
            "POST",
            &format!("/api/v1/channels/{private}/threads"),
            Some("threads-private"),
            json!({"name": "peek"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "thread_start",
        json!({"conversationId": private.to_string(), "name": "peek"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let threads = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations WHERE parent_channel_id = $1",
    )
    .bind(private)
    .fetch_one(pool)
    .await
    .expect("threads");
    assert_eq!(threads, 0);

    world.finish().await;
}
