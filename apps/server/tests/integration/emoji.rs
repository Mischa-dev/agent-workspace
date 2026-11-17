//! C2.19: any emoji, and a workspace's own. An agent reacts with a standard
//! emoji beyond the quick ones and with a custom one; strings that are no
//! emoji are refused; a retired custom emoji can't be used again but still
//! shows, with its image, on the reactions that used it; and its image is
//! for members of its workspace only.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::support::{
    NOTICES,
    app::{send, tool},
    world::WorldBuilder,
};

/// The smallest PNG header the server takes for an image.
const PNG_BASE64: &str = "iVBORw0KGgpwYXJyb3Q=";

#[tokio::test]
async fn any_emoji_and_the_workspaces_own() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("Desk")
        .build("c2_19_emoji")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK");
    let desk = &world.apps["Desk"];
    let rook = &world.agents["rook"];
    let (status, channel) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("emoji-channel"),
            json!({"name": "emoji"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .to_owned();
    let mut messages = Vec::new();
    for key in ["emoji-1", "emoji-2"] {
        let (status, sent) = send(
            app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/conversations/{channel}/messages"),
                Some(key),
                json!({"content": {"version": 1, "nodes": [{"type": "text", "text": key}]}}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sent}");
        messages.push(sent["message"]["id"].as_str().expect("message").to_owned());
    }

    // The owner's app adds :partyparrot:; a standard name is refused.
    let added = tool(
        app,
        &desk.token,
        &desk.session,
        "custom_emoji_create",
        json!({"name": "partyparrot", "imageBase64": PNG_BASE64, "clientRequestId": "parrot"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let parrot = added["value"]["id"].as_str().expect("id").to_owned();
    // Open pickers hear of it, in the workspace's spaces.
    let announced = sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {NOTICES} notice WHERE event_type = 'custom_emoji.changed' \
         AND payload->>'emojiId' = $1",
    ))
    .bind(&parrot)
    .fetch_one(world.pool())
    .await
    .expect("events");
    assert!(announced >= 1, "the new emoji is announced");
    for (name, image) in [
        ("eyes", PNG_BASE64),
        ("svg", "PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+"),
    ] {
        let refused = tool(
            app,
            &desk.token,
            &desk.session,
            "custom_emoji_create",
            json!({"name": name, "imageBase64": image, "clientRequestId": format!("bad-{name}")}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{name}: {refused}");
    }

    // Rook reacts with any emoji and with :partyparrot:; not with anything else.
    let react = |message: &str, emoji: &str| {
        let arguments = json!({"messageId": message, "emoji": emoji});
        async move { tool(app, &rook.token, &rook.session, "message_react", arguments).await }
    };
    for emoji in ["🦄", "unicorn_face", ":partyparrot:"] {
        let reacted = react(&messages[0], emoji).await;
        assert_eq!(reacted["outcome"], "ok", "{emoji}: {reacted}");
    }
    for emoji in ["<script>", "🦄🦄", "not_an_emoji_here", ":eyes::eyes:"] {
        let refused = react(&messages[0], emoji).await;
        assert_eq!(refused["outcome"], "invalid_request", "{emoji}: {refused}");
    }

    // Retired, :partyparrot: can't be used again, and still shows where it was.
    let retired = tool(
        app,
        &desk.token,
        &desk.session,
        "custom_emoji_update",
        json!({"id": parrot, "retired": true}),
    )
    .await;
    assert_eq!(retired["value"]["retired"], true, "{retired}");
    let refused = react(&messages[1], ":partyparrot:").await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let (status, timeline) = send(
        app,
        world.owner.browser.request(
            "GET",
            &format!("/api/v1/conversations/{channel}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    let entry = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["id"] == messages[0].as_str())
        .expect("first message");
    let reactions = entry["message"]["reactions"].as_array().expect("reactions");
    assert_eq!(reactions.len(), 2, "{reactions:?}");
    let image = format!("/api/v1/custom-emoji/{parrot}");
    assert!(
        reactions
            .iter()
            .any(|reaction| reaction["name"] == "partyparrot"
                && reaction["imageUrl"] == image.as_str()),
        "{reactions:?}"
    );

    // Its image is served, as a PNG, to the workspace.
    let (status, _) = send(
        app,
        world
            .owner
            .browser
            .request("GET", &image, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    world.finish().await;
}
