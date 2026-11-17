//! K2.15: task and space reads as operations. The silent failures: the web
//! list and agents seeing different tasks for the same filter, and the old tool names answering in another shape.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn tool(world: &Workspace, who: &str, name: &'static str, arguments: Value) -> Value {
    let call = Call::Tool { name, arguments };
    match world.agents.get(who) {
        Some(agent) => agent.call(&world.app, call).await.1,
        None => world.apps[who].call(&world.app, call).await.1,
    }
}

fn ids(rows: &Value, key: &str) -> Vec<String> {
    let mut ids = rows
        .as_array()
        .unwrap_or_else(|| panic!("rows in {rows}"))
        .iter()
        .map(|row| row[key].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[tokio::test]
async fn the_web_and_agents_list_the_same_tasks_for_the_same_filter() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .build("k2_15_same_tasks")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    for (title, priority) in [
        ("Fix the login loop", "high"),
        ("Write release notes", "low"),
        ("Shrink the bundle", "high"),
        ("Answer the survey", "medium"),
    ] {
        let made = tool(
            &world,
            "desk",
            "task_create",
            json!({"projectId": work, "title": title, "priority": priority,
                   "clientRequestId": title.replace(' ', "-")}),
        )
        .await;
        assert_eq!(made["outcome"], "ok", "{made}");
    }

    for (web, agent) in [
        ("priority=high", json!({"priority": ["high"]})),
        ("notPriority=high", json!({"notPriority": ["high"]})),
        ("q=bundle", json!({"query": "bundle"})),
    ] {
        let (status, page) = send(
            &world.app,
            world.owner.browser.request(
                "GET",
                &format!("/api/v1/projects/{work}/work-items?{web}"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let mut arguments = agent.clone();
        arguments["projectId"] = json!(work);
        let listed = tool(&world, "desk", "task_list", arguments).await;
        assert_eq!(listed["outcome"], "ok", "{listed}");
        assert_eq!(
            ids(&page["items"], "id"),
            ids(&listed["value"]["tasks"], "id"),
            "{web}: {page} / {listed}"
        );
        assert_eq!(page["total"], listed["value"]["total"], "{web}");
    }

    // The old names answer as they did.
    let opened = tool(
        &world,
        "desk",
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["task"]["key"], "WORK-1", "{opened}");
    let spaces = tool(&world, "desk", "projects_list", json!({})).await;
    assert_eq!(spaces["outcome"], "ok", "{spaces}");
    let briefed = tool(&world, "desk", "briefing", json!({})).await;
    assert_eq!(briefed["outcome"], "ok", "{briefed}");
    world.finish().await;
}
