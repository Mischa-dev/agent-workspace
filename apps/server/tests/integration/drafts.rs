//! C2.28: a draft follows its writer across devices. The newer save wins
//! and the other stays a day as a copy; nobody else reads it, and nobody
//! keeps a draft in a conversation they can't read.

use axum::http::StatusCode;
use serde_json::json;

use super::support::{
    app::{send, tool},
    world::WorldBuilder,
};

#[tokio::test]
async fn drafts_follow_their_writer_and_stay_theirs() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .connected_app("Phone")
        .dm("lena", "mara")
        .build("c2_28_drafts")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK");
    let phone = &world.apps["Phone"];
    let rook = &world.agents["rook"];
    let (status, channel) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("drafts-channel"),
            json!({"name": "drafts"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .to_owned();

    // One device saves; the list has it.
    let saved = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_set",
        json!({"conversationId": channel, "draft": {"text": "Half a thought"}}),
    )
    .await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    assert_eq!(saved["value"]["draft"]["draft"]["text"], "Half a thought");
    assert_eq!(saved["value"]["keptNewer"], false);
    // The writer's other devices hear of it: their id only.
    let announced = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE event_type = 'message_draft.changed' \
         AND payload->>'actorId' = $1",
    )
    .bind(world.owner.actor.to_string())
    .fetch_one(world.pool())
    .await
    .expect("events");
    assert!(announced >= 1, "the draft is announced");

    // A slower device that started before it saves: the newer draft stays,
    // and the slower one is the copy.
    let stale = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_set",
        json!({
            "conversationId": channel,
            "draft": {"text": "An older thought"},
            "savedAt": "2020-01-01T00:00:00Z",
        }),
    )
    .await;
    assert_eq!(stale["value"]["keptNewer"], true, "{stale}");
    assert_eq!(stale["value"]["draft"]["draft"]["text"], "Half a thought");
    assert_eq!(
        stale["value"]["draft"]["previous"]["text"],
        "An older thought"
    );
    // So does a clear from before it (an older device sending its text).
    let stale_clear = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_set",
        json!({"conversationId": channel, "savedAt": "2020-01-01T00:00:01Z"}),
    )
    .await;
    assert_eq!(stale_clear["value"]["keptNewer"], true, "{stale_clear}");
    assert_eq!(
        stale_clear["value"]["draft"]["draft"]["text"],
        "Half a thought"
    );
    let listed = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_list",
        json!({}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(
        listed["value"]["drafts"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );

    // Another account sees none of it, and can't keep a draft in a DM it
    // isn't in.
    let theirs = tool(
        app,
        &rook.token,
        &rook.session,
        "message_draft_list",
        json!({}),
    )
    .await;
    assert_eq!(theirs["value"]["drafts"], json!([]), "{theirs}");
    assert!(!theirs.to_string().contains("thought"), "{theirs}");
    let dm = &world.conversations["lena+mara"];
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "message_draft_set",
        json!({"conversationId": dm, "draft": {"text": "Snooping"}}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    // Clearing it (as sending does) leaves nothing.
    let cleared = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_set",
        json!({"conversationId": channel}),
    )
    .await;
    assert_eq!(cleared["outcome"], "ok", "{cleared}");
    assert!(cleared["value"].get("draft").is_none(), "{cleared}");
    let listed = tool(
        app,
        &phone.token,
        &phone.session,
        "message_draft_list",
        json!({}),
    )
    .await;
    assert_eq!(listed["value"]["drafts"], json!([]), "{listed}");

    world.finish().await;
}
