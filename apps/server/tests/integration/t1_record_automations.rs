//! T1.38: automations on records. The silent, costly failures: a rule that
//! fires on every change while a record matches (a flood of tasks), one that
//! misses the change it watches for, and one that keeps acting for someone
//! who can no longer edit records there.

use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn a_record_becoming_high_makes_one_task_and_a_maker_without_edit_rights_stops_it() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=error")
        .try_init();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_record_automations")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };
    let state = world.state.clone();
    let pump = || async {
        for _ in 0..10 {
            let published = agent_workspace_server::events::catch_up(&state.pool)
                .await
                .expect("publish outbox");
            let before = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM background_jobs WHERE state = 'succeeded'",
            )
            .fetch_one(&state.pool)
            .await
            .expect("jobs");
            super::support::run_due_jobs(&state).await;
            let after = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM background_jobs WHERE state = 'succeeded'",
            )
            .fetch_one(&state.pool)
            .await
            .expect("jobs");
            if published == 0 && before == after {
                return;
            }
        }
    };

    let (_, made) = laptop
        .call(
            app,
            tool(
                "collection_write",
                json!({
                    "projectId": "WORK",
                    "name": "Playtest",
                    "fields": [
                        {"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]},
                        {"name": "Notes", "type": "text"},
                    ],
                }),
            ),
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let collection = made["value"]["collection"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{made}"))
        .to_owned();

    // A field that isn't there, and a match with nothing to match, are refused.
    for trigger in [
        json!({"kind": "field_changed", "collectionId": collection, "field": "Nope"}),
        json!({"kind": "record_matches", "collectionId": collection}),
    ] {
        let (_, refused) = rook
            .call(
                app,
                tool(
                    "automation_write",
                    json!({"projectId": "WORK", "operation": "create", "name": "Bad",
                           "trigger": trigger,
                           "actions": [{"kind": "notify", "recipients": "owner", "message": "x"}]}),
                ),
            )
            .await;
        assert_ne!(refused["outcome"], "ok", "{refused}");
    }

    // Rook's rule: when Severity becomes High, a task.
    let (_, rule) = rook
        .call(
            app,
            tool(
                "automation_write",
                json!({
                    "projectId": "WORK",
                    "operation": "create",
                    "name": "High severity makes a task",
                    "trigger": {"kind": "field_changed", "collectionId": collection,
                                "field": "Severity", "to": ["High"]},
                    "actions": [{"kind": "run_operation", "operation": "task_create",
                                 "input": {"title": "Fix {{record.title}}"}}],
                }),
            ),
        )
        .await;
    assert_eq!(rule["outcome"], "ok", "{rule}");
    let rule_text = rule.to_string();
    assert!(!rule_text.contains("\"Severity\""), "stored by id: {rule}");
    // The laptop's: told when a record starts matching High.
    let (_, matching) = laptop
        .call(
            app,
            tool(
                "automation_write",
                json!({
                    "projectId": "WORK",
                    "operation": "create",
                    "name": "High records",
                    "trigger": {"kind": "record_matches", "collectionId": collection,
                                "filters": [{"field": "Severity", "op": "is", "values": ["High"]}]},
                    "actions": [{"kind": "notify", "recipients": "owner",
                                 "message": "{{record.title}} is high"}],
                }),
            ),
        )
        .await;
    assert_eq!(matching["outcome"], "ok", "{matching}");

    let tasks = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM task_rows WHERE project_id = $1::uuid AND title = 'Fix Boss fight'",
        )
        .bind(world.project("WORK"))
        .fetch_one(pool)
        .await
        .expect("tasks")
    };
    let told = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE kind = 'automation'")
            .fetch_one(pool)
            .await
            .expect("notifications")
    };
    let write = |fields: Value, version: Option<i64>, record: Option<String>, key: &'static str| {
        let mut entry = json!({"fields": fields});
        match record {
            Some(id) => {
                entry["record"] = json!(id);
                entry["expectedVersion"] = json!(version);
            }
            None => entry["title"] = json!("Boss fight"),
        }
        tool(
            "record_write",
            json!({"projectId": "WORK", "collection": "Playtest", "records": [entry],
                   "clientRequestId": key}),
        )
    };

    let (_, written) = rook
        .call(app, write(json!({"Severity": "Low"}), None, None, "w1"))
        .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let record = written["value"]["records"][0]["id"]
        .as_str()
        .expect("record")
        .to_owned();
    let mut version = written["value"]["records"][0]["version"]
        .as_i64()
        .expect("version");
    pump().await;
    assert_eq!((tasks().await, told().await), (0, 0));

    let step = |fields: Value, current: i64, key: &'static str| {
        let record = record.clone();
        async move {
            let (_, changed) = rook
                .call(app, write(fields, Some(current), Some(record), key))
                .await;
            assert_eq!(changed["outcome"], "ok", "{changed}");
            changed["value"]["records"][0]["version"]
                .as_i64()
                .expect("version")
        }
    };
    version = step(json!({"Severity": "High"}), version, "w2").await;
    pump().await;
    assert_eq!((tasks().await, told().await), (1, 1), "becoming High");

    // Another change while it's High: neither fires again.
    version = step(json!({"Notes": "Still bad"}), version, "w3").await;
    pump().await;
    assert_eq!((tasks().await, told().await), (1, 1), "still High");

    // Low, then High again: both fire again.
    version = step(json!({"Severity": "Low"}), version, "w4").await;
    pump().await;
    version = step(json!({"Severity": "High"}), version, "w5").await;
    pump().await;
    assert_eq!((tasks().await, told().await), (2, 2), "High again");

    // Rook becomes a viewer: its rule stops and says why, and makes nothing.
    let (_, demoted) = laptop
        .call(
            app,
            tool(
                "project_member_set",
                json!({"projectId": "WORK", "person": "@rook", "role": "viewer"}),
            ),
        )
        .await;
    assert_eq!(demoted["outcome"], "ok", "{demoted}");
    let (_, again) = laptop
        .call(
            app,
            write(
                json!({"Severity": "Low"}),
                Some(version),
                Some(record.clone()),
                "w6",
            ),
        )
        .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let version = again["value"]["records"][0]["version"]
        .as_i64()
        .expect("version");
    pump().await;
    let (_, high) = laptop
        .call(
            app,
            write(
                json!({"Severity": "High"}),
                Some(version),
                Some(record),
                "w7",
            ),
        )
        .await;
    assert_eq!(high["outcome"], "ok", "{high}");
    pump().await;
    assert_eq!(tasks().await, 2, "a viewer's rule makes nothing");
    let (enabled, reason) = sqlx::query_as::<_, (bool, Option<String>)>(
        "SELECT enabled, disabled_reason FROM automation_rules WHERE name = 'High severity makes a task'",
    )
    .fetch_one(pool)
    .await
    .expect("rule");
    assert!(!enabled);
    assert!(
        reason
            .as_deref()
            .is_some_and(|reason| reason.contains("can no longer make changes")),
        "{reason:?}"
    );

    world.finish().await;
}
