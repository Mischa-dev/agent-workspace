//! T1.18: relations both ways, rollups and their recalculation. The silent,
//! costly failures: the two sides of a relation disagreeing; a rollup left
//! stale after a related record changes, or worked out once per change
//! instead of once per record; a loop of rollups that never settles; and a
//! whole-collection recalculation that starts over (or stops) when it's
//! interrupted.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
};
use agent_workspace_server::collections::computed;

/// A record's value for `field`, by title.
fn value_of(page: &Value, title: &str, field: &str) -> Value {
    page["value"]["records"]
        .as_array()
        .unwrap_or_else(|| panic!("no records: {page}"))
        .iter()
        .find(|record| record["title"] == title)
        .unwrap_or_else(|| panic!("no {title}: {page}"))["fields"][field]
        .clone()
}

/// `collection.updated` events naming `record` with `cause`.
async fn events_for(pool: &sqlx::PgPool, record: &str, cause: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {n} outbox WHERE event_type = 'collection.updated' \
         AND payload->>'cause' = $2 AND payload->'recordIds' ? $1",
        n = crate::support::NOTICES
    ))
    .bind(record)
    .bind(cause)
    .fetch_one(pool)
    .await
    .expect("events")
}

#[tokio::test]
async fn relations_stay_paired_and_rollups_follow_related_records() {
    let Some(setup) = setup("t1_relations").await else {
        return;
    };
    let app = &setup.app;
    let project_id = create_project(&setup, "WORK", "Work")
        .await
        .parse::<Uuid>()
        .expect("project");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let query = |collection: &'static str| {
        call(
            "record_query",
            json!({"projectId": "WORK", "collection": collection, "limit": 50}),
        )
    };

    let testers = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Testers",
            "fields": [{"name": "Hours", "type": "number"}],
        }),
    )
    .await;
    assert_eq!(testers["outcome"], "ok", "{testers}");
    let builds = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Builds",
            "fields": [
                {"name": "Tested by", "type": "relation", "target": "record:Testers",
                 "twoWay": true, "pairName": "Builds"},
                {"name": "Hours", "type": "rollup",
                 "rollup": {"relation": "Tested by", "field": "Hours", "aggregate": "sum"}},
                {"name": "Testers", "type": "rollup",
                 "rollup": {"relation": "Tested by", "aggregate": "count"}},
            ],
        }),
    )
    .await;
    assert_eq!(builds["outcome"], "ok", "{builds}");
    // The pair is on Testers, named as asked, pointing back.
    let pair = sqlx::query_scalar::<_, Value>(
        "SELECT field.config FROM field_definitions field \
         JOIN collections collection ON collection.id = field.collection_id \
         WHERE collection.name = 'Testers' AND field.name = 'Builds'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("pair");
    assert!(pair["inverse"].is_string(), "{pair}");

    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Testers",
            "records": [
                {"title": "Ada", "fields": {"Hours": 3}},
                {"title": "Bo", "fields": {"Hours": 4}},
            ],
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "records": [
                {"title": "Build 1", "fields": {"Tested by": ["Ada", "Bo"]}},
                {"title": "Build 2", "fields": {"Tested by": ["Ada"]}},
            ],
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    // Worked out with the write itself.
    let shown = query("Builds").await;
    assert_eq!(value_of(&shown, "Build 1", "Hours"), json!(7), "{shown}");
    assert_eq!(value_of(&shown, "Build 1", "Testers"), json!(2));
    assert_eq!(value_of(&shown, "Build 2", "Hours"), json!(3));
    // And the other side lists both builds for Ada, one for Bo.
    let people = query("Testers").await;
    let mut ada_builds = value_of(&people, "Ada", "Builds")
        .as_array()
        .expect("Ada's builds")
        .clone();
    ada_builds.sort_by_key(|value| value.as_str().unwrap_or_default().to_owned());
    assert_eq!(ada_builds, [json!("Build 1"), json!("Build 2")], "{people}");
    assert_eq!(value_of(&people, "Bo", "Builds"), json!(["Build 1"]));

    let build_one = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_records WHERE project_id = $1 AND title = 'Build 1'",
    )
    .bind(project_id)
    .fetch_one(&setup.pool)
    .await
    .expect("build 1")
    .to_string();

    // A tester's hours change three times; Build 1 is worked out once.
    for (title, hours) in [("Ada", 10), ("Bo", 5), ("Ada", 11)] {
        let changed = call(
            "record_write",
            json!({
                "projectId": "WORK",
                "collection": "Testers",
                "records": [{"record": title_ref(&people, title), "fields": {"Hours": hours}}],
            }),
        )
        .await;
        assert_eq!(changed["outcome"], "ok", "{changed}");
    }
    let stale = query("Builds").await;
    assert_eq!(value_of(&stale, "Build 1", "Hours"), json!(7), "not yet");
    let waiting = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = 'fields.recalculate' \
         AND state = 'queued' AND project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&setup.pool)
    .await
    .expect("jobs");
    assert_eq!(waiting, 1, "one queued job covers every change");
    computed::recalculate_space(&setup.pool, project_id)
        .await
        .expect("recalculate");
    let fresh = query("Builds").await;
    assert_eq!(value_of(&fresh, "Build 1", "Hours"), json!(16), "{fresh}");
    assert_eq!(value_of(&fresh, "Build 2", "Hours"), json!(11));
    assert_eq!(events_for(&setup.pool, &build_one, "computed").await, 1);

    // Taking Bo off Build 1 takes Build 1 off Bo.
    let changed = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "records": [{"record": title_ref(&fresh, "Build 1"), "fields": {"Tested by": ["Ada"]}}],
        }),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let people = query("Testers").await;
    assert_eq!(value_of(&people, "Bo", "Builds"), Value::Null, "{people}");
    // Bo's history has both sides of it, with the values to put back.
    let paired = sqlx::query_as::<_, (Value, Value, Value)>(
        "SELECT history.changes -> 0 -> 'after', history.changes -> 0 -> 'was', \
                history.changes -> 0 -> 'is' \
         FROM collection_record_history history \
         JOIN collection_records record ON record.id = history.record_id \
         WHERE record.project_id = $1 AND record.title = 'Bo' \
           AND history.changes -> 0 ->> 'name' = 'Builds' \
         ORDER BY history.version",
    )
    .bind(project_id)
    .fetch_all(&setup.pool)
    .await
    .expect("Bo's history");
    assert_eq!(
        paired,
        vec![
            (json!(["Build 1"]), Value::Null, json!([build_one])),
            (json!([]), json!([build_one]), Value::Null),
        ]
    );
    assert_eq!(
        value_of(&query("Builds").await, "Build 1", "Hours"),
        json!(11)
    );

    // Once a tester needs a build, taking Ada off her last one from the
    // build's side is refused, as clearing it on Ada would be.
    let required = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Testers",
            "fields": [{"field": "Builds", "rules": {"required": true}}],
        }),
    )
    .await;
    assert_eq!(required["outcome"], "ok", "{required}");
    let builds = query("Builds").await;
    let off = |build: &str| {
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "records": [{"record": title_ref(&builds, build), "fields": {"Tested by": []}}],
        })
    };
    let still_one = call("record_write", off("Build 2")).await;
    assert_eq!(still_one["outcome"], "ok", "{still_one}");
    let last = call("record_write", off("Build 1")).await;
    assert_ne!(last["outcome"], "ok", "{last}");
    assert!(last.to_string().contains("Ada"), "{last}");
    let people = query("Testers").await;
    assert_eq!(
        value_of(&people, "Ada", "Builds"),
        json!(["Build 1"]),
        "{people}"
    );

    // A loop is refused with its path: Testers count their builds, Builds
    // add up those counts, and Testers can't then add up the Builds' sums.
    let counted = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Testers",
            "fields": [{"name": "Build count", "type": "rollup",
                        "rollup": {"relation": "Builds", "aggregate": "count"}}],
        }),
    )
    .await;
    assert_eq!(counted["outcome"], "ok", "{counted}");
    let summed = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Builds",
            "fields": [{"name": "Tester builds", "type": "rollup",
                        "rollup": {"relation": "Tested by", "field": "Build count", "aggregate": "sum"}}],
        }),
    )
    .await;
    assert_eq!(summed["outcome"], "ok", "{summed}");
    let looped = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "collection": "Testers",
            "fields": [{"field": "Build count",
                        "rollup": {"relation": "Builds", "field": "Tester builds", "aggregate": "sum"}}],
        }),
    )
    .await;
    assert_ne!(looped["outcome"], "ok", "{looped}");
    let message = looped.to_string();
    assert!(
        message.contains("read itself") && message.contains("Tester builds"),
        "{looped}"
    );
}

/// The record's ID, from a query's page, by title.
fn title_ref(page: &Value, title: &str) -> String {
    page["value"]["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["title"] == title)
        .and_then(|record| record["id"].as_str())
        .unwrap_or_else(|| panic!("no {title}: {page}"))
        .to_owned()
}

#[tokio::test]
async fn a_whole_collection_recalculation_carries_on_after_a_stop() {
    let Some(setup) = setup("t1_recompute").await else {
        return;
    };
    let app = &setup.app;
    let project_id = create_project(&setup, "WORK", "Work")
        .await
        .parse::<Uuid>()
        .expect("project");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let made = tool(
        app,
        &codex,
        &session,
        "collection_write",
        json!({"projectId": "WORK", "name": "Parts", "fields": [{"name": "Weight", "type": "number"}]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let collection = made["value"]["collection"]["id"]
        .as_str()
        .expect("collection")
        .parse::<Uuid>()
        .expect("collection id");
    let weight = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM field_definitions WHERE collection_id = $1 AND name = 'Weight'",
    )
    .bind(collection)
    .fetch_one(&setup.pool)
    .await
    .expect("weight");
    // 1,200 records written straight in, so the formula below has to be
    // worked out for all of them by the job.
    sqlx::query(
        "INSERT INTO collection_records (id, project_id, collection_id, number, title, field_values, \
             created_by_actor_id, updated_by_actor_id) \
         SELECT gen_random_uuid(), $1, $2, n, 'Part ' || n, jsonb_build_object($3::text, n), \
                c.created_by_actor_id, c.created_by_actor_id \
         FROM generate_series(1, 1200) n, collections c WHERE c.id = $2",
    )
    .bind(project_id)
    .bind(collection)
    .bind(weight.to_string())
    .execute(&setup.pool)
    .await
    .expect("records");
    sqlx::query("UPDATE collections SET next_number = 1201 WHERE id = $1")
        .bind(collection)
        .execute(&setup.pool)
        .await
        .expect("next number");
    let formula = tool(
        app,
        &codex,
        &session,
        "collection_write",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"name": "Double", "type": "formula", "formula": "{Weight} * 2"}]}),
    )
    .await;
    assert_eq!(formula["outcome"], "ok", "{formula}");

    // One batch, then the run "stops": the job for the next batch is queued
    // with where it had got to.
    computed::recompute_collection(&setup.pool, project_id, collection, None)
        .await
        .expect("first batch");
    let done = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM collection_records WHERE collection_id = $1 \
             AND computed_values <> '{}'::jsonb",
        )
        .bind(collection)
        .fetch_one(&setup.pool)
        .await
        .expect("done")
    };
    assert_eq!(done().await, 500);
    // Carry on from the queued jobs, as a worker would after a restart.
    loop {
        let next = sqlx::query_as::<_, (Uuid, Value)>(
            "SELECT id, payload FROM background_jobs WHERE kind = 'collections.recompute_formulas' \
             AND state = 'queued' AND payload ? 'after' ORDER BY created_at LIMIT 1",
        )
        .fetch_optional(&setup.pool)
        .await
        .expect("next job");
        let Some((job, payload)) = next else {
            break;
        };
        sqlx::query(
            "UPDATE background_jobs SET state = 'succeeded', finished_at = now() WHERE id = $1",
        )
        .bind(job)
        .execute(&setup.pool)
        .await
        .expect("take job");
        let after = payload["after"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok());
        computed::recompute_collection(&setup.pool, project_id, collection, after)
            .await
            .expect("next batch");
    }
    assert_eq!(done().await, 1200);
    let doubled = sqlx::query_scalar::<_, Value>(
        "SELECT computed_values FROM collection_records WHERE collection_id = $1 AND number = 1200",
    )
    .bind(collection)
    .fetch_one(&setup.pool)
    .await
    .expect("last");
    assert_eq!(
        doubled
            .as_object()
            .and_then(|values| values.values().next()),
        Some(&json!(2400))
    );
}
