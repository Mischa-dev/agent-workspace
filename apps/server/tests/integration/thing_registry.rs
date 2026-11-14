//! K3.10: the thing registry. The silent, costly failures: a `things` row
//! that drifts from its kind's row (an old title, a missed archive, a
//! deleted thing still live), a backfill that stops halfway and never ends
//! or copies a row twice, and a task's record showing as a `record`.

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use agent_workspace_server::things::backfill;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

#[derive(Debug, sqlx::FromRow)]
struct Row {
    kind: String,
    title: Option<String>,
    status: Option<String>,
    space_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    version: i64,
    archived: bool,
    deleted: bool,
}

async fn row(pool: &PgPool, id: &str) -> Row {
    sqlx::query_as::<_, Row>(
        "SELECT kind, title, status, space_id, parent_id, version, \
                archived_at IS NOT NULL AS archived, deleted_at IS NOT NULL AS deleted \
         FROM things WHERE id = $1::uuid",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .expect("things row")
    .unwrap_or_else(|| panic!("no things row for {id}"))
}

/// The id a tool's result names, wherever that tool puts it.
fn id_of(result: &Value) -> String {
    if let Some(id) = result["workItemId"].as_str() {
        return id.to_owned();
    }
    let value = &result["value"];
    for path in [
        "/id",
        "/task/id",
        "/whiteboard/id",
        "/page/id",
        "/decision/id",
        "/collection/id",
        "/project/id",
    ] {
        if let Some(id) = value.pointer(path).and_then(Value::as_str) {
            return id.to_owned();
        }
    }
    if let Some(id) = value.pointer("/records/0/id").and_then(Value::as_str) {
        return id.to_owned();
    }
    panic!("no id in {result}")
}

/// Every kind table's rows against `things`: one row each, same kind.
async fn counts_match(pool: &PgPool) {
    for (table, kinds) in [
        ("projects", "{project}"),
        ("conversations", "{channel,thread,conversation}"),
        ("work_items", "{task}"),
        ("project_pages", "{page}"),
        ("project_decisions", "{decision}"),
        ("whiteboards", "{whiteboard}"),
        ("sheets", "{sheet}"),
        ("collections", "{collection}"),
    ] {
        let missing = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} source \
             WHERE NOT EXISTS (SELECT 1 FROM things WHERE id = source.id AND kind = ANY($1::text[]))"
        ))
        .bind(kinds)
        .fetch_one(pool)
        .await
        .expect("count");
        assert_eq!(missing, 0, "{table} rows missing from things");
    }
    // Records: their own kind, or the task they are.
    let missing = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records record \
         WHERE NOT EXISTS (SELECT 1 FROM things WHERE id = record.id AND kind IN ('record', 'task'))",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(missing, 0, "records missing from things");
    let tasks_as_records = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM things JOIN work_items USING (id) WHERE things.kind <> 'task'",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(tasks_as_records, 0, "a task shows as another kind");
}

#[tokio::test]
async fn every_kind_keeps_its_row_in_step_and_the_backfill_resumes() {
    let Some(setup) = setup("k3_10_things").await else {
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
        async move {
            let result = tool(app, &codex, &session, name, arguments).await;
            assert!(
                matches!(result["outcome"].as_str(), Some("ok" | "applied")),
                "{name}: {result}"
            );
            result
        }
    };

    // Each kind, made through its tool, has its row at once.
    let project = row(pool, &work).await;
    assert_eq!(
        (project.kind.as_str(), project.space_id),
        ("project", Some(work.parse().expect("id")))
    );
    let task = id_of(
        &call(
            "task_create",
            json!({"projectId": "WORK", "title": "Fix the gate"}),
        )
        .await,
    );
    let subtask = id_of(
        &call(
            "task_create",
            json!({"projectId": "WORK", "title": "Oil the hinge", "parentId": task}),
        )
        .await,
    );
    let page = id_of(
        &call(
            "page_write",
            json!({"projectId": "WORK", "title": "Gate notes", "body": "Hinges."}),
        )
        .await,
    );
    let decision = id_of(
        &call("decision_record", json!({"projectId": "WORK", "title": "Paint it green", "clientRequestId": "k3-10-decision"})).await,
    );
    let board = id_of(
        &call(
            "whiteboard_create",
            json!({"projectId": "WORK", "title": "Gate sketch"}),
        )
        .await,
    );
    let sheet = id_of(
        &call("sheet_write", json!({"projectId": "WORK", "name": "Gate costs", "cells": [{"cell": "A1", "input": "Paint"}], "clientRequestId": "k3-10-sheet"})).await,
    );
    let collection = id_of(
        &call("collection_write", json!({"projectId": "WORK", "name": "Suppliers", "fields": [{"name": "Notes", "type": "text"}]})).await,
    );
    let record = id_of(
        &call("record_write", json!({"projectId": "WORK", "collection": "Suppliers", "records": [{"title": "Hinge Co"}], "clientRequestId": "k3-10-record"})).await,
    );
    let channel = id_of(
        &call(
            "channel_create",
            json!({"projectId": "WORK", "name": "gate"}),
        )
        .await,
    );
    let said = call(
        "message_send",
        json!({"conversationId": channel, "text": "Who has the key?"}),
    )
    .await;
    let message = said["value"]["id"]
        .as_str()
        .or_else(|| said["value"]["messageId"].as_str())
        .unwrap_or_else(|| panic!("message id in {said}"))
        .to_owned();
    let thread = id_of(
        &call(
            "thread_start",
            json!({"conversationId": channel, "fromMessageId": message}),
        )
        .await,
    );

    for (id, kind, title) in [
        (&task, "task", "Fix the gate"),
        (&page, "page", "Gate notes"),
        (&decision, "decision", "Paint it green"),
        (&board, "whiteboard", "Gate sketch"),
        (&sheet, "sheet", "Gate costs"),
        (&collection, "collection", "Suppliers"),
        (&record, "record", "Hinge Co"),
        (&channel, "channel", "gate"),
    ] {
        let found = row(pool, id).await;
        assert_eq!(found.kind, kind, "{id}");
        assert_eq!(found.title.as_deref(), Some(title), "{kind}");
        assert_eq!(found.space_id, Some(work.parse().expect("id")), "{kind}");
        assert!(!found.archived && !found.deleted, "{kind}");
    }
    assert!(
        row(pool, &task).await.status.is_some(),
        "a task's status category"
    );
    assert_eq!(row(pool, &decision).await.status.as_deref(), Some("active"));
    assert_eq!(
        row(pool, &subtask).await.parent_id,
        Some(task.parse().expect("id"))
    );
    assert_eq!(
        row(pool, &record).await.parent_id,
        Some(collection.parse().expect("id"))
    );
    let thread_row = row(pool, &thread).await;
    assert_eq!(thread_row.kind, "thread");
    assert_eq!(thread_row.parent_id, Some(channel.parse().expect("id")));

    // Renames, a new status, archives and deletes follow.
    let version = row(pool, &task).await.version;
    call(
        "task_update",
        json!({"taskId": task, "expectedVersion": version, "title": "Fix the north gate"}),
    )
    .await;
    let renamed = row(pool, &task).await;
    assert_eq!(renamed.title.as_deref(), Some("Fix the north gate"));
    assert!(renamed.version > version);
    call(
        "whiteboard_update",
        json!({"whiteboardId": board, "title": "North gate sketch"}),
    )
    .await;
    assert_eq!(
        row(pool, &board).await.title.as_deref(),
        Some("North gate sketch")
    );
    call(
        "whiteboard_update",
        json!({"whiteboardId": board, "deleted": true}),
    )
    .await;
    assert!(row(pool, &board).await.deleted);
    call(
        "project_update",
        json!({"projectId": work, "name": "Gates"}),
    )
    .await;
    assert_eq!(row(pool, &work).await.title.as_deref(), Some("Gates"));
    let sheet_version = row(pool, &sheet).await.version;
    call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "cells": [{"cell": "A2", "input": "Oil"}], "clientRequestId": "k3-10-sheet-2"}),
    )
    .await;
    assert!(row(pool, &sheet).await.version > sheet_version);
    // Writes no tool makes yet go through the same trigger.
    sqlx::query("UPDATE project_pages SET archived_at = now() WHERE id = $1::uuid")
        .bind(&page)
        .execute(pool)
        .await
        .expect("archive page");
    assert!(row(pool, &page).await.archived);
    sqlx::query("UPDATE conversations SET thread_status = 'Resolved' WHERE id = $1::uuid")
        .bind(&thread)
        .execute(pool)
        .await
        .expect("resolve thread");
    assert_eq!(row(pool, &thread).await.status.as_deref(), Some("resolved"));
    sqlx::query("DELETE FROM project_decisions WHERE id = $1::uuid")
        .bind(&decision)
        .execute(pool)
        .await
        .expect("delete decision");
    let tombstone = row(pool, &decision).await;
    assert!(tombstone.deleted, "a deleted row leaves a tombstone");

    // The registry opens any of them by id alone.
    let (status, opened) = send(
        app,
        setup
            .browser
            .request("GET", &format!("/api/v1/things/{sheet}"), None, Value::Null),
    )
    .await;
    assert!(status.is_success(), "{opened}");
    assert_eq!(opened["thing"]["kind"], "sheet");
    assert_eq!(opened["thing"]["spaceId"], work);
    let by_id = call("thing_open", json!({"id": format!("thing:{subtask}")})).await;
    assert_eq!(by_id["value"]["thing"]["kind"], "task", "{by_id}");
    assert_eq!(by_id["value"]["thing"]["parentId"], task, "{by_id}");
    let (status, _) = send(
        app,
        setup
            .browser
            .request("GET", &format!("/api/v1/things/{board}"), None, Value::Null),
    )
    .await;
    assert_eq!(status.as_u16(), 404, "a deleted board doesn't open");

    // A task and its record share one `task` row whichever is written first.
    sqlx::query("DELETE FROM things WHERE id = $1::uuid")
        .bind(&subtask)
        .execute(pool)
        .await
        .expect("forget the subtask");
    // (A Tasks record changes only through its task, so the record's
    // trigger is run as it would run.)
    let record_trigger =
        "SELECT things_upsert_from('thing_source_collection_records', ARRAY[$1::uuid])";
    sqlx::query(record_trigger)
        .bind(&subtask)
        .execute(pool)
        .await
        .expect("the record first");
    assert_eq!(row(pool, &subtask).await.kind, "record");
    sqlx::query("UPDATE work_items SET updated_at = now() WHERE id = $1::uuid")
        .bind(&subtask)
        .execute(pool)
        .await
        .expect("then the task");
    assert_eq!(row(pool, &subtask).await.kind, "task");
    sqlx::query(record_trigger)
        .bind(&subtask)
        .execute(pool)
        .await
        .expect("the record again");
    assert_eq!(
        row(pool, &subtask).await.kind,
        "task",
        "a record write never takes a task's row"
    );

    // The backfill: forget everything, as before the registry, then copy it
    // back a little at a time, stopping halfway.
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM things")
        .fetch_one(pool)
        .await
        .expect("count");
    sqlx::query("DELETE FROM things")
        .execute(pool)
        .await
        .expect("forget");
    let first = backfill::run(pool, 2, 3).await.expect("first part");
    assert!(!first.done);
    assert!(first.copied > 0 && first.copied <= 6, "{first:?}");
    assert!(!backfill::is_done(pool).await.expect("done?"));
    let rest = backfill::run(pool, 2, usize::MAX).await.expect("the rest");
    assert!(rest.done);
    assert!(backfill::is_done(pool).await.expect("done?"));
    let after = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM things")
        .fetch_one(pool)
        .await
        .expect("count");
    // The decision was hard-deleted: its tombstone isn't copied back.
    assert_eq!(after, before - 1);
    counts_match(pool).await;
    assert_eq!(
        row(pool, &thread).await.parent_id,
        Some(channel.parse().expect("id"))
    );
    assert_eq!(
        row(pool, &subtask).await.parent_id,
        Some(task.parse().expect("id"))
    );
    assert_eq!(row(pool, &subtask).await.kind, "task");
    // Done is done: another run copies nothing.
    let again = backfill::run(pool, 2, usize::MAX).await.expect("again");
    assert_eq!(
        again,
        backfill::Progress {
            copied: 0,
            done: true
        }
    );
}
