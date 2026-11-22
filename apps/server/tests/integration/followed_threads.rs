//! C2.36: the threads you follow, across spaces. A thread you started or
//! posted in is listed newest reply first with its status and outcome; one
//! you can no longer read (a group you left, a space you're out of) isn't.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{Browser, send},
    world::WorldBuilder,
};

async fn followed(app: &axum::Router, browser: &Browser, workspace: Uuid) -> Vec<Value> {
    followed_with(app, browser, workspace, json!({})).await
}

async fn followed_with(
    app: &axum::Router,
    browser: &Browser,
    workspace: Uuid,
    input: Value,
) -> Vec<Value> {
    let (status, listed) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/thread_followed_list"),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    listed["value"]["threads"]
        .as_array()
        .expect("threads")
        .clone()
}

#[tokio::test]
async fn followed_threads_are_the_ones_you_can_read() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .person("lena", "member")
        .person("nina", "member")
        .private_group("pair", &["lena", "nina"])
        .build("c2_36_followed")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let lena = &world.people["lena"];
    let nina = &world.people["nina"];
    let thread = |browser: &Browser, parent: String, name: &'static str| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/channels/{parent}/threads"),
            Some(&format!("followed-{name}")),
            json!({ "name": name }),
        );
        async move {
            let (status, made) = send(app, request).await;
            assert!(status.is_success(), "{made}");
            made["conversation"]["id"]
                .as_str()
                .expect("thread")
                .to_owned()
        }
    };
    let post = |browser: &Browser, thread: &str, text: &'static str| {
        let request = browser.request(
            "POST",
            &format!("/api/v1/threads/{thread}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": text}]}}),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{sent}");
        }
    };
    let mut channels = Vec::new();
    for (project, name) in [("WORK", "general"), ("OTHER", "plans")] {
        let (status, made) = send(
            app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{}/channels", world.project(project)),
                Some(&format!("followed-channel-{name}")),
                json!({ "name": name }),
            ),
        )
        .await;
        assert!(status.is_success(), "{made}");
        channels.push(
            made["conversation"]["id"]
                .as_str()
                .or_else(|| made["id"].as_str())
                .expect("channel")
                .to_owned(),
        );
    }
    let launch = thread(&lena.browser, channels[0].clone(), "Launch").await;
    let budget = thread(&lena.browser, channels[1].clone(), "Budget").await;
    let private = thread(&lena.browser, world.conversations["pair"].clone(), "Gift").await;
    for id in [&launch, &budget, &private] {
        post(&nina.browser, id, "I'm in").await;
    }
    // Budget gets the newest reply; Launch settles.
    post(&lena.browser, &budget, "Agreed").await;
    let (status, settled) = lena
        .browser
        .op(
            app,
            "thread_update",
            json!({"threadId": launch, "status": "Decided", "outcome": "Ship on Friday"}),
        )
        .await;
    assert!(status.is_success(), "{settled}");

    let listed = followed(app, &nina.browser, workspace).await;
    let ids = listed
        .iter()
        .map(|thread| thread["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids[0], budget, "newest reply first: {listed:?}");
    assert_eq!(ids.len(), 3, "{listed:?}");
    let launch_row = listed
        .iter()
        .find(|row| row["id"] == launch.as_str())
        .expect("launch");
    assert_eq!(launch_row["status"], "Decided");
    assert_eq!(launch_row["outcome"], "Ship on Friday");
    assert_eq!(launch_row["parentName"], "general");
    assert_eq!(listed[0]["unread"], true, "Lena's reply is new to Nina");

    // Open leaves out a settled thread, and a discussion's Resolved settles
    // one too.
    let (status, resolved) = lena
        .browser
        .op(
            app,
            "thread_update",
            json!({"threadId": budget, "status": "Resolved"}),
        )
        .await;
    assert!(status.is_success(), "{resolved}");
    let open = followed_with(app, &nina.browser, workspace, json!({"openOnly": true})).await;
    let open = open
        .iter()
        .map(|thread| thread["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(open, vec![private.clone()], "only Gift is open");

    // Out of OTHER and out of the group, Nina follows only Launch.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(world.project("OTHER"))
    .bind(nina.actor)
    .execute(pool)
    .await
    .expect("nina leaves OTHER");
    sqlx::query(
        "DELETE FROM conversation_members WHERE conversation_id = $1::uuid AND actor_id = $2",
    )
    .bind(&world.conversations["pair"])
    .bind(nina.actor)
    .execute(pool)
    .await
    .expect("nina leaves the group");
    let listed = followed(app, &nina.browser, workspace).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["id"], launch.as_str());

    world.finish().await;
}
