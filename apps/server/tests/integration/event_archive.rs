//! K3.25: history is never cut. The silent, costly failures: retention
//! deleting a fact no file holds, an audit trail or activity feed that gets
//! shorter once its facts are archived, and a page version whose body is
//! gone (or whose history entry is) once it went cold.

use std::collections::HashSet;

use agent_workspace_server::{
    events::{self, archive, upkeep},
    files::store::FileStore,
    history::cold,
};
use serde_json::{Value, json};

use super::support::{
    app::{send, tool},
    world::WorldBuilder,
};

#[tokio::test]
async fn archived_facts_and_cold_page_versions_still_answer() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .build("k3_25_cold")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let state = &world.state;
    let workspace = world.organization;
    let rook = &world.agents["rook"];
    let call = |name: &'static str, arguments: Value| async move {
        let result = tool(app, &rook.token, &rook.session, name, arguments).await;
        assert!(
            matches!(result["outcome"].as_str(), Some("ok" | "applied")),
            "{name}: {result}"
        );
        result
    };

    // A task with a few changes, all in the log.
    let created = call(
        "task_create",
        json!({"projectId": world.project("WORK"), "title": "Paint", "clientRequestId": "k3-25-task"}),
    )
    .await;
    let task = created["workItemId"].as_str().expect("task").to_owned();
    for (version, title) in [(1, "Paint the gate"), (2, "Paint the hall")] {
        call(
            "task_update",
            json!({"taskId": task, "title": title, "expectedVersion": version}),
        )
        .await;
    }
    events::catch_up(pool).await.expect("catch up");
    let activity = async |before: Option<i64>| {
        let mut arguments = json!({"thing": task, "limit": 2});
        if let Some(before) = before {
            arguments["before"] = json!(before);
        }
        call("activity_list", arguments).await["value"]["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["position"].as_i64().expect("position"))
            .collect::<Vec<_>>()
    };
    // Every page of it, two at a time.
    let all_activity = async || {
        let mut all = Vec::new();
        loop {
            let page = activity(all.last().copied()).await;
            if page.is_empty() {
                return all;
            }
            all.extend(page);
        }
    };
    let audit_path = format!("/api/v1/organizations/{workspace}/audit-events?limit=100");
    let audit = async || {
        let (status, body) = send(
            app,
            world
                .owner
                .browser
                .request("GET", &audit_path, None, Value::Null),
        )
        .await;
        assert!(status.is_success(), "{status} {body}");
        body["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["id"].as_str().expect("id").to_owned())
            .collect::<Vec<_>>()
    };
    let activity_before = all_activity().await;
    assert!(activity_before.len() >= 3, "{activity_before:?}");
    let audit_before = audit().await;

    // Nothing is deleted before a file holds it.
    assert_eq!(
        upkeep::retain(pool, 36_500, 0).await.expect("retain"),
        0,
        "no fact goes before it is archived"
    );
    assert!(
        archive::archive_once(state, workspace, 0)
            .await
            .expect("archive")
    );
    assert!(
        !archive::archive_once(state, workspace, 0)
            .await
            .expect("archive again"),
        "a second run has nothing new"
    );
    // A fact after the file: retention keeps it.
    call(
        "task_update",
        json!({"taskId": task, "title": "Paint the hall white", "expectedVersion": 3}),
    )
    .await;
    events::catch_up(pool).await.expect("catch up");
    let gone = upkeep::retain(pool, 36_500, 0).await.expect("retain");
    assert!(gone > 0, "the archived facts go");
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM events WHERE workspace_id = $1 AND class = 'fact' \
           AND thing_id = $2::uuid",
    )
    .bind(workspace)
    .bind(&task)
    .fetch_one(pool)
    .await
    .expect("left");
    assert_eq!(left, 1, "only the fact no file holds stays live");

    // The feed and the trail are as long as before: slower, same rows.
    let activity_after = all_activity().await;
    assert_eq!(activity_after.len(), activity_before.len() + 1);
    assert_eq!(&activity_after[1..], &activity_before[..], "same rows");
    let audit_after = audit().await;
    let after = audit_after.iter().collect::<HashSet<_>>();
    assert!(
        audit_before.iter().all(|id| after.contains(id)),
        "nothing missing from the trail: {audit_before:?} {audit_after:?}"
    );
    assert_eq!(audit_after.len(), audit_before.len() + 1, "{audit_after:?}");

    // A page with three versions; the two older ones go cold.
    let page = call(
        "page_write",
        json!({"projectId": world.project("WORK"), "title": "Gate", "body": "Paint it red."}),
    )
    .await;
    let page = page["value"]["id"].as_str().expect("page").to_owned();
    for (version, body) in [(1, "Paint it blue."), (2, "Paint it green.")] {
        call(
            "page_write",
            json!({"pageId": page, "body": body, "expectedVersion": version}),
        )
        .await;
    }
    let history =
        async || call("history_list", json!({"thing": page})).await["value"]["entries"].clone();
    let history_before = history().await;
    let page_id = page.parse().expect("page id");
    assert!(
        cold::archive_page(state, workspace, page_id, 0)
            .await
            .expect("archive page")
    );
    let bodies = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT version, body FROM project_page_revisions WHERE page_id = $1 ORDER BY version",
    )
    .bind(page_id)
    .fetch_all(pool)
    .await
    .expect("bodies");
    assert_eq!(
        bodies,
        vec![
            (1, None),
            (2, None),
            (3, Some("Paint it green.".to_owned()))
        ],
        "the newest stays; no version count goes down"
    );
    assert_eq!(
        history().await,
        history_before,
        "the list still says what changed"
    );
    let shown = call("history_show", json!({"thing": page, "version": 1})).await;
    assert_eq!(
        shown["value"]["content"]["body"], "Paint it red.",
        "{shown}"
    );
    let restored = call(
        "history_restore",
        json!({"thing": page, "version": 2, "expectedVersion": 3}),
    )
    .await;
    assert_eq!(restored["value"]["version"], 4, "{restored}");
    let body = sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1")
        .bind(page_id)
        .fetch_one(pool)
        .await
        .expect("page body");
    assert_eq!(body, "Paint it blue.");

    FileStore::from_config(&state.config)
        .delete_workspace(workspace)
        .await
        .expect("clean up");
    world.finish().await;
}
