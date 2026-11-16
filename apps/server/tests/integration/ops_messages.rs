//! K2.7: messages as operations. The silent failures: a retried send that
//! posts twice, a send without a request id that claims to be retry-safe,
//! and reading through an app (by the operation's own name) that leaves the
//! mentions unread.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, drain_outbox},
    world::{Workspace, WorldBuilder},
};

async fn channel(world: &Workspace, name: &str) -> String {
    let (status, channel) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/channels", world.project("WORK")),
            Some(&format!("channel-{name}")),
            json!({ "name": name }),
        ),
    )
    .await;
    assert!(status.is_success(), "{channel}");
    channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .to_owned()
}

async fn agent_tool(world: &Workspace, name: &'static str, arguments: Value) -> Value {
    world.agents["rook"]
        .call(&world.app, Call::Tool { name, arguments })
        .await
        .1
}

async fn messages_in(world: &Workspace, conversation: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid",
    )
    .bind(conversation)
    .fetch_one(world.pool())
    .await
    .expect("messages")
}

#[tokio::test]
async fn a_retried_send_posts_once_and_an_unkeyed_one_says_so() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("k27_send_once")
        .await
    else {
        return;
    };
    let general = channel(&world, "ops-general").await;
    let before = messages_in(&world, &general).await;

    let hello = json!({"conversationId": general, "text": "Hello once",
                       "clientRequestId": "hello-1"});
    let first = agent_tool(&world, "message_send", hello.clone()).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    assert_eq!(first["replaySafe"], true, "{first}");
    let again = agent_tool(&world, "message_send", hello).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["value"]["messageId"], first["value"]["messageId"]);
    assert_eq!(messages_in(&world, &general).await, before + 1);

    // The same id for different words is refused, not sent.
    let other = agent_tool(
        &world,
        "message_send",
        json!({"conversationId": general, "text": "Something else",
               "clientRequestId": "hello-1"}),
    )
    .await;
    assert_eq!(other["outcome"], "conflict", "{other}");
    assert_eq!(messages_in(&world, &general).await, before + 1);

    // Without one it runs once and says a retry would send again.
    let unkeyed = agent_tool(
        &world,
        "message_send",
        json!({"conversationId": general, "text": "No key"}),
    )
    .await;
    assert_eq!(unkeyed["outcome"], "ok", "{unkeyed}");
    assert_eq!(unkeyed["replaySafe"], false, "{unkeyed}");
    assert_eq!(messages_in(&world, &general).await, before + 2);
    world.finish().await;
}

#[tokio::test]
async fn marking_read_through_an_app_reads_the_mentions() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("desk")
        .build("k27_mark_read")
        .await
    else {
        return;
    };
    let general = channel(&world, "ops-mentions").await;
    let owner = world.owner.actor;
    let sent = agent_tool(
        &world,
        "message_send",
        json!({"conversationId": general, "clientRequestId": "mention-1",
               "content": {"version": 1, "nodes": [
                   {"type": "text", "text": "Have a look "},
                   {"type": "mention", "actorId": owner}
               ]}}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    drain_outbox(&world.state).await;
    let unread = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE recipient_actor_id = $1 AND conversation_id = $2::uuid AND read_at IS NULL",
        )
        .bind(owner)
        .bind(&general)
        .fetch_one(world.pool())
        .await
        .expect("notifications")
    };
    assert!(unread().await > 0, "the mention reached the owner");

    let marked = world.apps["desk"]
        .call(
            &world.app,
            Call::Tool {
                name: "conversation_mark_read",
                arguments: json!({"conversationId": general}),
            },
        )
        .await
        .1;
    assert_eq!(marked["outcome"], "ok", "{marked}");
    assert_eq!(marked["value"]["unreadCount"], 0, "{marked}");
    assert_eq!(unread().await, 0);
    // It was the person who read it, through their app.
    let reader = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM audit_events WHERE action = 'conversation_mark_read' \
         ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(world.pool())
    .await
    .expect("audit");
    assert_eq!(reader, owner);
    world.finish().await;
}
