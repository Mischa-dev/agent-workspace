//! AWD-186: a space that turns a module off stops its own events reaching
//! anyone, and only its own. Event routing once read the workspace's
//! switches alone, so a space's narrowing still sent its events out; every
//! transport (webhook, poll, stream) reads through `delivery::read` now, so
//! the poll shows what a webhook would be sent.

use axum::http::StatusCode;
use serde_json::json;

use super::support::{
    app::send,
    callers::{Call, Caller, drain_outbox},
    world::WorldBuilder,
};

#[tokio::test]
async fn a_module_off_in_one_space_stops_its_events_only() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .agent("rook", "member")
        .build("awd186_space_events")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    // A connection of the owner's that hears new tasks.
    let (_, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &op("connection_create"),
            Some("awd186-tasks"),
            json!({"name": "Tasks", "reachesAll": false,
                   "subscriptions": [{"kinds": ["task.created"], "minReason": "visible"}]}),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let connection = made["value"]["connection"]["id"].clone();

    for key in ["WORK", "OTHER"] {
        let (_, made) = rook
            .call(
                &world.app,
                Call::Tool {
                    name: "task_create",
                    arguments: json!({"projectId": world.project(key), "title": "Hooked",
                                      "clientRequestId": format!("hooked-{key}")}),
                },
            )
            .await;
        assert!(
            matches!(made["outcome"].as_str(), Some("ok" | "applied")),
            "{made}"
        );
    }

    // Tasks off in OTHER only, before anything is read.
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/workspace_update", world.organization),
            Some("tasks-off-in-other"),
            json!({"projectId": world.project("OTHER"), "modules": {"tasks": false}, "inProjectOnly": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    drain_outbox(&world.state).await;
    let (_, page) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &op("event_list"),
            Some("awd186-read"),
            json!({"connectionId": connection}),
        ),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let spaces = page["value"]["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["type"] == "task.created")
        .map(|event| {
            event["data"]["spaceId"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        spaces,
        vec![world.project("WORK").to_string()],
        "WORK's task is heard; OTHER turned Tasks off, so its task isn't"
    );
    world.finish().await;
}
