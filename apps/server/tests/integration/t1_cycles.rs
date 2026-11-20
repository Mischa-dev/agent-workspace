//! T1.24: cycles. The silent, costly failures: a cycle that never closes,
//! unfinished work left behind in an ended cycle (or finished work dragged
//! along), burn-up counts that don't match the tasks, and a task put into a
//! cycle that has already ended.

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn a_cycle_closes_rolls_unfinished_work_over_and_counts_its_days() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("laptop")
        .build("t1_cycles")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let laptop = &world.apps["laptop"];
    let call = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };

    // Turned on, the running cycle and two after it are made at once,
    // starting on the weekday asked for.
    let enabled = call(
        "cycle_set",
        json!({"spaceId": "WORK", "enabled": true, "lengthDays": 14,
               "startsOnWeekday": 1, "upcoming": 2}),
    )
    .await;
    assert_eq!(enabled["outcome"], "ok", "{enabled}");
    let cycles = enabled["value"]["cycles"]
        .as_array()
        .expect("cycles")
        .clone();
    assert_eq!(cycles.len(), 3, "{enabled}");
    assert_eq!(cycles[0]["state"], "running");
    assert_eq!(cycles[1]["state"], "upcoming");
    let date = |value: &Value| {
        NaiveDate::parse_from_str(value.as_str().expect("date"), "%Y-%m-%d").expect("date")
    };
    let starts_on = date(&cycles[0]["startsOn"]);
    let ends_on = date(&cycles[0]["endsOn"]);
    assert_eq!(starts_on.weekday(), chrono::Weekday::Mon);
    assert_eq!((ends_on - starts_on).num_days(), 14);
    assert_eq!(date(&cycles[1]["startsOn"]), ends_on);

    // Six tasks in the running cycle: two done, one started, two ready and
    // one canceled (left out of its scope).
    let mut keys = Vec::new();
    for (index, status) in ["Done", "Done", "In Progress", "Ready", "Ready", "Canceled"]
        .into_iter()
        .enumerate()
    {
        let created = call(
            "task_create",
            json!({"projectId": "WORK", "title": format!("Cycle work {index}"),
                   "status": status, "clientRequestId": format!("cycle-{index}")}),
        )
        .await;
        assert_eq!(created["outcome"], "ok", "{created}");
        let key = created["value"]["key"].as_str().expect("key").to_owned();
        let planned = call(
            "task_update",
            json!({"taskId": key, "expectedVersion": created["value"]["version"],
                   "cycle": "current"}),
        )
        .await;
        assert_eq!(planned["outcome"], "ok", "{planned}");
        keys.push(key);
    }
    let unknown = call(
        "task_update",
        json!({"taskId": keys[0], "expectedVersion": 2, "cycle": "Cycle 40"}),
    )
    .await;
    assert_eq!(unknown["outcome"], "invalid_request", "{unknown}");

    // Named, it's called by its name.
    let named = call(
        "cycle_set",
        json!({"spaceId": "WORK", "cycle": "1", "name": "Launch"}),
    )
    .await;
    assert_eq!(named["value"]["cycles"][0]["title"], "Launch", "{named}");
    let listed = call("cycle_list", json!({"spaceId": "WORK"})).await;
    assert_eq!(listed["value"]["cycles"][0]["scope"], 5, "{listed}");
    assert_eq!(listed["value"]["cycles"][0]["completed"], 2, "{listed}");
    assert_eq!(listed["value"]["cycles"][0]["started"], 1, "{listed}");
    assert_eq!(listed["value"]["settings"]["enabled"], true, "{listed}");

    // The day it ends, the job closes it, counts its last day and rolls the
    // three unfinished tasks into the next.
    let cycle_of = |key: String| async move {
        sqlx::query_scalar::<_, Option<i32>>(
            "SELECT cycle.number FROM work_items task \
             JOIN projects project ON project.id = task.project_id \
             LEFT JOIN cycles cycle ON cycle.id = task.cycle_id \
             WHERE project.project_key || '-' || task.number = $1",
        )
        .bind(key)
        .fetch_one(pool)
        .await
        .expect("task")
    };
    let planned =
        agent_workspace_server::work_items::cycles::run_due(pool, ends_on, Uuid::now_v7())
            .await
            .expect("plan");
    assert_eq!(planned, 1);
    let mut numbers = Vec::new();
    for key in &keys {
        numbers.push(cycle_of(key.clone()).await);
    }
    assert_eq!(
        numbers,
        vec![Some(1), Some(1), Some(2), Some(2), Some(2), Some(1)],
        "done and canceled stay, the rest move on"
    );
    let snapshots = sqlx::query_as::<_, (i32, NaiveDate, i32, i32, i32)>(
        "SELECT cycle.number, snapshot.on_date, snapshot.scope, snapshot.started, \
         snapshot.completed FROM cycle_snapshots snapshot \
         JOIN cycles cycle ON cycle.id = snapshot.cycle_id \
         WHERE cycle.space_id = $1 ORDER BY cycle.number, snapshot.on_date",
    )
    .bind(work)
    .fetch_all(pool)
    .await
    .expect("snapshots");
    let last_day = ends_on - chrono::Duration::days(1);
    assert!(
        snapshots.contains(&(1, last_day, 5, 1, 2)),
        "the closing count: {snapshots:?}"
    );
    assert!(
        snapshots.contains(&(2, ends_on, 3, 1, 0)),
        "the next cycle's first count: {snapshots:?}"
    );
    let open = sqlx::query_as::<_, (i32, bool)>(
        "SELECT number, completed_at IS NOT NULL FROM cycles WHERE space_id = $1 ORDER BY number",
    )
    .bind(work)
    .fetch_all(pool)
    .await
    .expect("cycles");
    assert_eq!(
        open,
        vec![(1, true), (2, false), (3, false), (4, false)],
        "two kept made after the running one"
    );

    // Run again the same day, nothing more happens.
    let again = agent_workspace_server::work_items::cycles::run_due(pool, ends_on, Uuid::now_v7())
        .await
        .expect("plan again");
    assert_eq!(again, 0);

    // An ended cycle takes no more tasks.
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT task.version FROM work_items task JOIN projects project \
         ON project.id = task.project_id WHERE project.project_key || '-' || task.number = $1",
    )
    .bind(&keys[3])
    .fetch_one(pool)
    .await
    .expect("version");
    let late = call(
        "task_update",
        json!({"taskId": keys[3], "expectedVersion": version, "cycle": "Launch"}),
    )
    .await;
    assert_eq!(late["outcome"], "invalid_request", "{late}");

    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("sequence events");
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM events WHERE space_id = $1 \
         AND type IN ('cycle.ended', 'cycle.started')",
    )
    .bind(work)
    .fetch_one(pool)
    .await
    .expect("events");
    assert_eq!(told, 3, "cycle 1 started and ended, cycle 2 started");

    world.finish().await;
}
