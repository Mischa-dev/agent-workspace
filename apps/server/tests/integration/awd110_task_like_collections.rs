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
    assert_eq!(chore["outcome"], "applied", "{chore}");
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
    assert_eq!(plain["outcome"], "applied", "{plain}");

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
    assert_eq!(reported["outcome"], "applied", "{reported}");
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
    assert_eq!(updated["outcome"], "applied", "{updated}");

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

    // Its records change only through their tasks.
    let direct = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "records": [{"title": "Sneaky"}], "clientRequestId": "sneaky"}),
    )
    .await;
    assert_eq!(direct["outcome"], "conflict", "{direct}");
    // In the database the record owns its title (T1.3), but it can't leave
    // its task: no move to another collection, no delete.
    let sneaky = sqlx::query("DELETE FROM collection_records WHERE id = $1")
        .bind(crash)
        .execute(pool)
        .await;
    assert!(sneaky.is_err(), "a Bugs record deleted directly");

    // The switch stays on while it has records.
    let off = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Bugs", "actsLikeTask": false}),
    )
    .await;
    assert_eq!(off["outcome"], "conflict", "{off}");

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
