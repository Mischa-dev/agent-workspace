//! C2.17: profiles. A title, pronouns, a time zone and an avatar, set by
//! the account (or whoever manages the agent), shown to people who share a
//! workspace with it; the avatar is served only to them.

use axum::{body::Body, http::Request, http::StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, Person},
    two_workspaces::tiny_png,
    world::WorldBuilder,
};

async fn op(app: &axum::Router, by: &Person, name: &str, input: Value) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    send(
        app,
        by.browser
            .request("POST", &format!("/api/v2/ops/{name}"), Some(&key), input),
    )
    .await
}

async fn image(
    app: &axum::Router,
    by: &Person,
    path: &str,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let request: Request<Body> = by.browser.request("GET", path, None, Value::Null);
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    (status, kind, bytes.to_vec())
}

#[tokio::test]
async fn profiles_show_to_people_who_share_a_workspace() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .build("c2_17_profiles")
        .await
    else {
        return;
    };
    let app = &world.app;
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let rook = &world.agents["rook"];

    // Lena sets her title, pronouns, time zone and an avatar.
    let (status, set) = op(
        app,
        lena,
        "profile_update",
        json!({
            "title": "Designer",
            "pronouns": "she/her",
            "timeZone": "Europe/Berlin",
            "avatarBase64": STANDARD.encode(tiny_png()),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let url = set["value"]["avatarUrl"]
        .as_str()
        .expect("avatar url")
        .to_owned();
    assert!(url.ends_with("?v=1"), "{url}");

    // Mara reads it, with the space they share, and gets the image.
    let (status, read) = op(app, mara, "profile_get", json!({"account": lena.actor})).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["value"]["title"], "Designer", "{read}");
    assert_eq!(read["value"]["pronouns"], "she/her");
    assert_eq!(read["value"]["timeZone"], "Europe/Berlin");
    assert_eq!(read["value"]["canEdit"], false);
    assert!(
        read["value"]["sharedSpaces"]
            .as_array()
            .expect("spaces")
            .iter()
            .any(|space| space["id"] == world.project("WORK")),
        "{read}"
    );
    let (status, kind, bytes) = image(app, mara, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kind.as_deref(), Some("image/png"));
    assert_eq!(bytes, tiny_png());
    let (status, avatars) = op(app, mara, "profile_avatar_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{avatars}");
    assert!(avatars.to_string().contains(&url), "{avatars}");

    // Not an image, too long, or a zone that isn't one: refused.
    for bad in [
        json!({"avatarBase64": STANDARD.encode(b"<svg onload=alert(1)>")}),
        json!({"title": "x".repeat(81)}),
        json!({"timeZone": "Mars/Olympus"}),
    ] {
        let (status, refused) = op(app, lena, "profile_update", bad.clone()).await;
        assert!(!status.is_success(), "{bad}: {refused}");
    }

    // Mara can't change Lena's; nor can a person who doesn't manage the agent.
    for account in [lena.actor, rook.actor] {
        let (status, refused) = op(
            app,
            mara,
            "profile_update",
            json!({"title": "Intern", "account": account}),
        )
        .await;
        assert!(!status.is_success(), "{refused}");
    }

    // The agent sets its own, through its tools.
    let (_, own) = rook
        .call(
            app,
            Call::Tool {
                name: "profile_update",
                arguments: json!({"title": "Release helper"}),
            },
        )
        .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    assert_eq!(own["value"]["kind"], "agent", "{own}");

    // Removed, the old address is gone; "" clears a field.
    let (status, cleared) = op(
        app,
        lena,
        "profile_update",
        json!({"removeAvatar": true, "pronouns": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert!(cleared["value"]["avatarUrl"].is_null(), "{cleared}");
    assert!(cleared["value"]["pronouns"].is_null(), "{cleared}");
    assert_eq!(cleared["value"]["title"], "Designer");
    let (status, _, _) = image(app, mara, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    world.finish().await;
}
