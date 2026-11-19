//! T1.23: the triage queue. The silent, costly failures: a bug report not
//! landing in triage, rules running out of order or more than one acting,
//! a snooze that never hides or never wakes, and a duplicate that isn't
//! linked to its original.

use axum::{body::Body, http::Request};
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::WorldBuilder,
};

const PRIORITY: &str = "0192f5c0-0000-7000-8000-000000000003";
const URGENT: &str = "0192f5c0-0000-7000-8000-0000000000a1";

#[tokio::test]
async fn triage_lands_orders_rules_and_wakes_snoozes() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .connected_app("laptop")
        .build("t1_triage")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let ana = world.people["ana"].actor;
    let laptop = &world.apps["laptop"];
    let call = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };
    let status_of = |key: String| async move {
        sqlx::query_as::<_, (String, Option<Uuid>, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT status.semantic_category, task.assignee_actor_id, task.snoozed_until \
             FROM work_items task JOIN projects project ON project.id = task.project_id \
             JOIN work_item_statuses status ON status.id = task.status_id \
             WHERE project.project_key || '-' || task.number = $1",
        )
        .bind(key)
        .fetch_one(pool)
        .await
        .expect("task")
    };
    let mut made = 0;
    let mut create = |title: &'static str, priority: &'static str| {
        made += 1;
        let request = format!("triage-{made}");
        async move {
            let created = call(
                "task_create",
                json!({"projectId": "WORK", "title": title, "priority": priority,
                       "status": "Triage", "clientRequestId": request}),
            )
            .await;
            assert_eq!(created["outcome"], "ok", "{created}");
            created["value"]["key"].as_str().expect("key").to_owned()
        }
    };

    // Turning triage on adds the status once.
    let enabled = call("triage_enable", json!({"spaceId": "WORK"})).await;
    assert_eq!(enabled["outcome"], "ok", "{enabled}");
    let again = call("triage_enable", json!({"spaceId": "WORK"})).await;
    assert_eq!(
        again["value"]["statusId"], enabled["value"]["statusId"],
        "{again}"
    );

    // Two rules for urgent tasks: the first in position acts, alone.
    let urgent = json!([{"field": PRIORITY, "op": "is", "values": [URGENT]}]);
    let assign = call(
        "triage_rule_set",
        json!({"spaceId": "WORK", "name": "Urgent to Ana", "filter": urgent,
               "action": {"kind": "assign", "to": ana}}),
    )
    .await;
    assert_eq!(assign["outcome"], "ok", "{assign}");
    let accept = call(
        "triage_rule_set",
        json!({"spaceId": "WORK", "name": "Urgent ships", "filter": urgent,
               "action": {"kind": "accept"}}),
    )
    .await;
    assert_eq!(accept["value"]["rule"]["position"], 2, "{accept}");
    let first = create("Crash on save", "urgent").await;
    assert_eq!(status_of(first.clone()).await.0, "triage");
    assert_eq!(status_of(first.clone()).await.1, Some(ana));
    // Moved first, the accept rule acts instead.
    let moved = call(
        "triage_rule_set",
        json!({"spaceId": "WORK", "rule": accept["value"]["rule"]["id"], "position": 1,
               "expectedVersion": accept["value"]["rule"]["version"]}),
    )
    .await;
    assert_eq!(moved["value"]["rule"]["position"], 1, "{moved}");
    let second = create("Crash on load", "urgent").await;
    assert_eq!(
        status_of(second.clone()).await,
        ("unstarted".to_owned(), None, None)
    );
    // No rule matches: it waits in triage.
    let quiet = create("Typo on the about page", "low").await;
    assert_eq!(status_of(quiet.clone()).await.0, "triage");

    // A bug report lands in triage.
    let (status, link) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/bug-report-links"),
            Some(&Uuid::now_v7().to_string()),
            json!({"name": "Game"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {link}");
    let url = link["url"].as_str().expect("url");
    let path = &url[url.find("/api/v1/bug-reports/").expect("path")..];
    let (status, reported) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "text/plain")
            .body(Body::from("The floor is lava"))
            .expect("report"),
    )
    .await;
    assert!(status.is_success(), "{status} {reported}");
    let report = reported["task"].as_str().expect("task key").to_owned();
    assert_eq!(status_of(report.clone()).await.0, "triage");

    // Accept, decline with a reason, and duplicate.
    let accepted = call(
        "triage_decide",
        json!({"taskIds": [report], "action": "accept"}),
    )
    .await;
    assert_eq!(accepted["outcome"], "ok", "{accepted}");
    assert_eq!(status_of(report.clone()).await.0, "unstarted");
    let no_triage = call(
        "triage_decide",
        json!({"taskIds": [quiet], "action": "accept", "status": "Triage"}),
    )
    .await;
    assert_ne!(no_triage["outcome"], "ok", "{no_triage}");
    let declined = call(
        "triage_decide",
        json!({"taskIds": [quiet], "action": "decline", "reason": "Fixed already."}),
    )
    .await;
    assert_eq!(declined["outcome"], "ok", "{declined}");
    assert_eq!(status_of(quiet.clone()).await.0, "canceled");
    let commented = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_timeline_entries entry \
         JOIN work_items task ON task.id = entry.work_item_id \
         JOIN projects project ON project.id = task.project_id \
         WHERE entry.kind = 'comment' AND project.project_key || '-' || task.number = $1",
    )
    .bind(&quiet)
    .fetch_one(pool)
    .await
    .expect("comments");
    assert_eq!(commented, 1);
    let copy = create("Crash on load again", "low").await;
    let duplicate = call(
        "triage_decide",
        json!({"taskIds": [copy], "action": "duplicate", "duplicateOf": second}),
    )
    .await;
    assert_eq!(duplicate["outcome"], "ok", "{duplicate}");
    assert_eq!(status_of(copy.clone()).await.0, "canceled");
    let linked = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_relations WHERE kind = 'duplicates'",
    )
    .fetch_one(pool)
    .await
    .expect("relations");
    assert_eq!(linked, 1);

    // A snooze hides the task from the Triage view until it wakes.
    let in_queue = |key: String| async move {
        let listed = call("task_list", json!({"projectId": "WORK", "view": "Triage"})).await;
        assert_eq!(listed["outcome"], "ok", "{listed}");
        listed["value"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|task| task["key"] == json!(key))
    };
    assert!(in_queue(first.clone()).await);
    let unsaid = call(
        "triage_decide",
        json!({"taskIds": [first], "action": "snooze"}),
    )
    .await;
    assert_ne!(unsaid["outcome"], "ok", "{unsaid}");
    let until = chrono::Utc::now() + chrono::Duration::hours(1);
    let snoozed = call(
        "triage_decide",
        json!({"taskIds": [first], "action": "snooze", "until": until}),
    )
    .await;
    assert_eq!(snoozed["outcome"], "ok", "{snoozed}");
    assert!(status_of(first.clone()).await.2.is_some());
    assert!(!in_queue(first.clone()).await);
    sqlx::query(
        "UPDATE work_items SET snoozed_until = now() - interval '1 minute' \
         WHERE snoozed_until IS NOT NULL",
    )
    .execute(pool)
    .await
    .expect("snooze due");
    let woken = agent_workspace_server::work_items::triage::wake_snoozed(pool)
        .await
        .expect("wake");
    assert_eq!(woken, 1);
    assert_eq!(status_of(first.clone()).await.2, None);
    assert!(in_queue(first.clone()).await);
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("sequence events");
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM events WHERE type = 'task.snooze_ended' \
         AND audience_account_id = $1",
    )
    .bind(laptop.person)
    .fetch_one(pool)
    .await
    .expect("told");
    assert_eq!(told, 1);

    world.finish().await;
}
