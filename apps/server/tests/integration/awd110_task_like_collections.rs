//! AWD-110, step 6: a collection that acts like a task. Its records become
//! tasks with the same IDs and keys from the project's sequence, the task
//! tools make and list them by `collection`, and their `fields` are the
//! collection's own. The silent, costly failures: a record left without its
//! task (or two things sharing a key), fields resolved against the wrong
//! collection, a record changed behind its task's back, and a switch that
//! turns off and strands tasks.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn a_collection_that_acts_like_a_task_holds_tasks() {
    let Some(setup) = setup("awd110_task_like").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    // In a project with no tasks yet (so no built-in Tasks collection), a
    // collection made to act like a task reads its tasks.
    let made = call(
        "collection_write",
        json!({"projectId": "WORK", "name": "Chores", "actsLikeTask": true}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let chore = call(
        "task_create",
        json!({"projectId": "WORK", "collection": "Chores", "title": "Water the plants", "clientRequestId": "chore"}),
    )
    .await;
    assert_eq!(chore["outcome"], "ok", "{chore}");
    let chores = call(
        "record_query",
        json!({"projectId": "WORK", "collection": "Chores"}),
    )
    .await;
    assert_eq!(chores["value"]["total"], 1, "{chores}");
    assert_eq!(chores["value"]["records"][0]["title"], "Water the plants");

    // A plain task next, so the project's task numbers are ahead of the
    // records' own.
    let plain = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Plain task", "clientRequestId": "plain"}),
    )
    .await;
    assert_eq!(plain["outcome"], "ok", "{plain}");

    let made = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Bugs",
            "fields": [{"name": "Severity", "type": "single_choice", "options": [{"name": "High"}, {"name": "Low"}]}],
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Bugs",
            "records": [
                {"title": "Crash on save", "fields": {"Severity": "High"}},
                {"title": "Typo in footer"},
            ],
            "clientRequestId": "bugs",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");

    // A collection with its own "Status" can't switch on until it's renamed.
    let clash = call(
        "collection_write",
        json!({"projectId": "WORK", "name": "Clash", "fields": [{"name": "Status", "type": "text"}]}),
    )
    .await;
    assert_eq!(clash["outcome"], "ok", "{clash}");
    let refused = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Clash", "actsLikeTask": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");

    // Switching on: each record gets a task with its ID, a key from the
    // project's sequence and its values.
    let switched = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "actsLikeTask": true}),
    )
    .await;
    assert_eq!(switched["outcome"], "ok", "{switched}");
    let bugs = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections WHERE project_id = $1::uuid AND name = 'Bugs'",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("bugs");
    let severity = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM field_definitions WHERE collection_id = $1 AND name = 'Severity'",
    )
    .bind(bugs)
    .fetch_one(pool)
    .await
    .expect("severity");
    let rows = sqlx::query_as::<_, (Uuid, String, i64, i64, Value, Option<Uuid>)>(
        "SELECT record.id, task.title, task.number, record.number, task.field_values, task.collection_id \
         FROM collection_records record JOIN task_rows task ON task.id = record.id \
         WHERE record.collection_id = $1 ORDER BY task.number",
    )
    .bind(bugs)
    .fetch_all(pool)
    .await
    .expect("bug tasks");
    assert_eq!(rows.len(), 2, "every record has its task");
    assert_eq!(rows[0].1, "Crash on save");
    assert_eq!(
        rows[0].2, 3,
        "after the chore and the plain task, in the project's sequence"
    );
    assert_eq!(rows[1].2, 4);
    for row in &rows {
        assert_eq!(row.2, row.3, "the record takes its task's number");
        assert_eq!(row.5, Some(bugs));
    }
    assert!(
        rows[0].4.get(severity.to_string()).is_some(),
        "{:?}",
        rows[0].4
    );
    let crash = rows[0].0;

    // task_create with collection makes a Bugs task, its fields Bugs' own.
    let reported = call(
        "task_create",
        json!({
            "projectId": "WORK",
            "collection": "Bugs",
            "title": "Login loops",
            "fields": {"Severity": "Low"},
            "clientRequestId": "loops",
        }),
    )
    .await;
    assert_eq!(reported["outcome"], "ok", "{reported}");
    let (loops, loops_collection, loops_values) = sqlx::query_as::<_, (Uuid, Option<Uuid>, Value)>(
        "SELECT id, collection_id, field_values FROM task_rows \
             WHERE project_id = $1::uuid AND title = 'Login loops'",
    )
    .bind(&work)
    .fetch_one(pool)
    .await
    .expect("loops");
    assert_eq!(loops_collection, Some(bugs));
    assert!(loops_values.get(severity.to_string()).is_some());
    let in_bugs = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM collection_records WHERE id = $1 AND collection_id = $2)",
    )
    .bind(loops)
    .bind(bugs)
    .fetch_one(pool)
    .await
    .expect("record");
    assert!(in_bugs, "its record is in Bugs, not Tasks");
    // A plain task has no Severity.
    let plain_severity = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Nope", "fields": {"Severity": "Low"}, "clientRequestId": "nope"}),
    )
    .await;
    assert_eq!(
        plain_severity["outcome"], "invalid_request",
        "{plain_severity}"
    );
    let nowhere = call(
        "task_create",
        json!({"projectId": "WORK", "collection": "Clash", "title": "Nope", "clientRequestId": "nowhere"}),
    )
    .await;
    assert_eq!(nowhere["outcome"], "invalid_request", "{nowhere}");

    // task_list by collection.
    let listed = |collection: &'static str| {
        let call = &call;
        async move {
            let found = call(
                "task_list",
                json!({"projectId": "WORK", "collection": collection}),
            )
            .await;
            assert_eq!(found["outcome"], "ok", "{found}");
            let mut titles = found["value"]["tasks"]
                .as_array()
                .expect("tasks")
                .iter()
                .map(|task| task["title"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>();
            titles.sort();
            titles
        }
    };
    assert_eq!(
        listed("Bugs").await,
        vec!["Crash on save", "Login loops", "Typo in footer"]
    );
    assert_eq!(listed("Tasks").await, vec!["Plain task"]);
    // The project's task list and board show every task, whatever
    // collection it was made in.
    let (status, page) = super::support::app::send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items?limit=50"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{page}");
    let mut titles = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|task| task["title"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    titles.sort();
    for title in ["Crash on save", "Plain task", "Water the plants"] {
        assert!(
            titles.iter().any(|seen| seen == title),
            "{title}: {titles:?}"
        );
    }

    // task_open names the collection and its fields; task_update sets them.
    let opened = call("task_open", json!({"taskId": crash.to_string()})).await;
    assert_eq!(opened["value"]["collection"], "Bugs", "{opened}");
    let fields = opened["value"]["fields"].as_array().expect("fields");
    assert_eq!(fields.len(), 1, "{opened}");
    assert_eq!(fields[0]["name"], "Severity");
    assert_eq!(fields[0]["value"], "High");
    let version = opened["value"]["task"]["version"]
        .as_i64()
        .expect("version");
    let updated = call(
        "task_update",
        json!({"taskId": crash.to_string(), "fields": {"Severity": "Low"}, "expectedVersion": version}),
    )
    .await;
    assert_eq!(updated["outcome"], "ok", "{updated}");

    // Many at once: each task takes its own collection's fields, and a
    // field another collection has is refused.
    let many = call(
        "task_update_many",
        json!({"projectId": "WORK", "taskIds": [crash.to_string(), loops.to_string()],
               "fields": {"Severity": "High"}, "clientRequestId": "many"}),
    )
    .await;
    assert_eq!(many["outcome"], "ok", "{many}");
    let mixed = call(
        "task_update_many",
        json!({"projectId": "WORK", "taskIds": [crash.to_string(), "WORK-2"],
               "fields": {"Severity": "Low"}, "clientRequestId": "mixed"}),
    )
    .await;
    assert_eq!(mixed["outcome"], "rejected", "{mixed}");

    // project_open lists the collection.
    let project = call("project_open", json!({"projectId": "WORK"})).await;
    assert_eq!(
        project["value"]["taskCollections"],
        json!(["Chores", "Bugs"]),
        "{project}"
    );

    // A record written there is a task, made through the task rules (T1.11).
    let direct = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "records": [{"title": "Written as a record"}], "clientRequestId": "as-record"}),
    )
    .await;
    assert_eq!(direct["outcome"], "ok", "{direct}");
    let as_task = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE title = 'Written as a record' AND collection_id IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(as_task, 1);
    // In the database the record owns its title (T1.3), but it can't leave
    // its task: no move to another collection, no delete.
    let sneaky = sqlx::query("DELETE FROM collection_records WHERE id = $1")
        .bind(crash)
        .execute(pool)
        .await;
    assert!(sneaky.is_err(), "a Bugs record deleted directly");

    // The switch stays on while one of its tasks is claimed (T1.25).
    let key_of = |id: Uuid| async move {
        sqlx::query_scalar::<_, i64>("SELECT number FROM work_items WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("number")
    };
    let crash_number = key_of(crash).await;
    let claimed = call("task_claim", json!({"taskId": crash.to_string()})).await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    let off = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "actsLikeTask": false}),
    )
    .await;
    assert_eq!(off["outcome"], "conflict", "{off}");
    let released = call(
        "task_release",
        json!({"taskId": crash.to_string(), "handoff": "Switching Bugs off"}),
    )
    .await;
    assert_eq!(released["outcome"], "ok", "{released}");

    // Switched off, its tasks are hidden and its records are plain records.
    let off = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "actsLikeTask": false}),
    )
    .await;
    assert_eq!(off["outcome"], "ok", "{off}");
    let visible =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM task_rows WHERE collection_id = $1")
            .bind(bugs)
            .fetch_one(pool)
            .await
            .expect("visible");
    assert_eq!(visible, 0, "Bugs' tasks are hidden");
    let all = call("task_list", json!({"projectId": "WORK"})).await;
    assert!(
        all["value"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .all(|task| task["title"] != "Crash on save"),
        "{all}"
    );
    let version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM collection_records WHERE id = $1")
            .bind(crash)
            .fetch_one(pool)
            .await
            .expect("version");
    let renamed = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "clientRequestId": "bugs-off-1",
               "records": [{"record": crash.to_string(), "title": "Crash on save, again",
                            "expectedVersion": version},
                           {"title": "Added while off"}]}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");

    // On again, nothing is lost: the old tasks come back with their keys,
    // and the record added meanwhile gets a task and a new key.
    let on = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "actsLikeTask": true}),
    )
    .await;
    assert_eq!(on["outcome"], "ok", "{on}");
    assert_eq!(key_of(crash).await, crash_number);
    let back = sqlx::query_as::<_, (String, i64)>(
        "SELECT title, number FROM task_rows WHERE collection_id = $1 ORDER BY number",
    )
    .bind(bugs)
    .fetch_all(pool)
    .await
    .expect("back");
    assert_eq!(back.len(), 5, "{back:?}");
    assert!(
        back.contains(&("Crash on save, again".to_owned(), crash_number)),
        "{back:?}"
    );
    let added = back
        .iter()
        .find(|(title, _)| title == "Added while off")
        .expect("added");
    assert!(back.iter().filter(|(_, number)| *number == added.1).count() == 1);

    // Bugs uses only some of the space's statuses and starts its tasks in
    // one of them; plain tasks keep the whole workflow.
    let statuses = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, is_default FROM work_item_statuses \
             WHERE project_id = $1::uuid AND enabled ORDER BY position",
    )
    .bind(&work)
    .fetch_all(pool)
    .await
    .expect("statuses");
    let others: Vec<&(Uuid, String, bool)> =
        statuses.iter().filter(|(_, _, default)| !default).collect();
    assert!(others.len() >= 2, "{statuses:?}");
    let (start, start_name, _) = others[0];
    let (kept, kept_name, _) = others[1];
    let unknown = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "taskStatuses": ["Nope"]}),
    )
    .await;
    assert_eq!(unknown["outcome"], "invalid_request", "{unknown}");
    let outside = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs",
               "taskStatuses": [kept_name], "startStatus": start_name}),
    )
    .await;
    assert_eq!(outside["outcome"], "conflict", "{outside}");
    let narrowed = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs",
               "taskStatuses": [start_name, kept_name], "startStatus": start_name}),
    )
    .await;
    assert_eq!(narrowed["outcome"], "ok", "{narrowed}");
    let (own, own_start) = sqlx::query_as::<_, (Option<Vec<Uuid>>, Option<Uuid>)>(
        "SELECT task_statuses, start_status_id FROM collections WHERE id = $1",
    )
    .bind(bugs)
    .fetch_one(pool)
    .await
    .expect("workflow");
    assert_eq!(own, Some(vec![*start, *kept]));
    assert_eq!(own_start, Some(*start));
    let started = call(
        "task_create",
        json!({"projectId": "WORK", "collection": "Bugs", "title": "Starts in its own status",
               "clientRequestId": "own-start"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let (started_id, started_status) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, status_id FROM task_rows WHERE title = 'Starts in its own status'",
    )
    .fetch_one(pool)
    .await
    .expect("started");
    assert_eq!(started_status, *start);
    let default_status = statuses
        .iter()
        .find(|(_, _, default)| *default)
        .expect("default");
    let not_used = call(
        "task_create",
        json!({"projectId": "WORK", "collection": "Bugs", "title": "Wrong status",
               "status": default_status.1, "clientRequestId": "wrong-status"}),
    )
    .await;
    assert_eq!(not_used["outcome"], "invalid_request", "{not_used}");
    let opened = call("task_open", json!({"taskId": started_id.to_string()})).await;
    let version = opened["value"]["task"]["version"].clone();
    let moved = call(
        "task_update",
        json!({"taskId": started_id.to_string(), "expectedVersion": version,
               "status": default_status.1}),
    )
    .await;
    assert_eq!(moved["outcome"], "invalid_request", "{moved}");
    let plain_task = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Plain default", "clientRequestId": "plain-default"}),
    )
    .await;
    assert_eq!(plain_task["outcome"], "ok", "{plain_task}");
    let plain_status = sqlx::query_scalar::<_, Uuid>(
        "SELECT status_id FROM task_rows WHERE title = 'Plain default'",
    )
    .fetch_one(pool)
    .await
    .expect("plain");
    assert_eq!(plain_status, default_status.0);
    // [] uses them all again.
    let widened = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "taskStatuses": [], "startStatus": ""}),
    )
    .await;
    assert_eq!(widened["outcome"], "ok", "{widened}");

    // Every task still has exactly one record, and keys stay unique.
    let unmatched = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM task_rows task
        LEFT JOIN collection_records record ON record.id = task.id
        LEFT JOIN collections collection
          ON collection.id = record.collection_id AND collection.acts_like_task
        WHERE collection.id IS NULL OR record.number <> task.number
           OR record.field_values <> task.field_values
           OR (task.collection_id IS NULL AND collection.builtin IS NULL)
           OR task.collection_id <> collection.id
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("unmatched");
    assert_eq!(unmatched, 0);
}
