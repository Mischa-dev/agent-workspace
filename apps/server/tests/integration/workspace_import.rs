//! H1.10: a workspace comes back from its archive. The silent, costly
//! failures: an import that stops part way and resumes differently, a
//! copy whose references still point into the original, an archive that
//! writes into another workspace or out of its folder, and an import that
//! notifies people or fires webhooks about history.

use std::io::{Read, Write};

use agent_workspace_server::{
    export::{
        import::{self, ImportError, RunLimits, StartImport},
        transfer,
    },
    files::store::FileStore,
    jobs,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    message_edits::text,
    support::{
        app::send,
        two_workspaces::{TwoWorkspaces, two_workspaces},
    },
};

/// A finished export's archive, written to `into`.
async fn archive_of(fixture: &TwoWorkspaces, export: Uuid, into: &std::path::Path) {
    let (store_id, key) = sqlx::query_as::<_, (String, String)>(
        "SELECT store_id, file_key FROM workspace_transfers WHERE id = $1",
    )
    .bind(export)
    .fetch_one(&fixture.pool)
    .await
    .expect("export row");
    let bytes = FileStore::resolve(&fixture.state, &store_id)
        .await
        .expect("store")
        .read_all(&key, 512 * 1024 * 1024)
        .await
        .expect("archive bytes");
    std::fs::write(into, bytes).expect("write archive");
}

/// How many rows of each table `workspace` holds, by its tenant column.
async fn shape(pool: &PgPool, workspace: Uuid) -> Vec<(String, i64)> {
    let mut counts = Vec::new();
    for declaration in agent_workspace_server::export::registry::DECLARATIONS {
        for kind in declaration.kinds {
            let Some(column) = agent_workspace_server::tenancy::tables::tenant_column(kind.table)
            else {
                continue;
            };
            let count = sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM {} WHERE {column} = $1",
                kind.table
            ))
            .bind(workspace)
            .fetch_one(pool)
            .await
            .expect("count");
            if count > 0 {
                counts.push((kind.table.to_owned(), count));
            }
        }
    }
    counts
}

/// Events, notifications and webhook deliveries about `workspace`, other
/// than the transfer's own and the notifications it carried as history.
async fn news(pool: &PgPool, workspace: Uuid, source: Uuid) -> (i64, i64, i64) {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM events WHERE workspace_id = $1 \
                   AND type <> 'workspace.transfer_changed') \
              + (SELECT count(*) FROM event_staging WHERE workspace_id = $1 \
                   AND type <> 'workspace.transfer_changed'), \
                (SELECT count(*) FROM deliveries WHERE workspace_id = $1), \
                (SELECT count(*) FROM notifications WHERE workspace_id = $1) \
              - (SELECT count(*) FROM notifications WHERE workspace_id = $2)",
    )
    .bind(workspace)
    .bind(source)
    .fetch_one(pool)
    .await
    .expect("news")
}

/// `archive` rewritten through `change`, which sees each entry's name and
/// bytes and returns what to write instead (`None` drops it).
fn rewritten(
    archive: &std::path::Path,
    into: &std::path::Path,
    mut change: impl FnMut(&str, Vec<u8>) -> Option<(String, Vec<u8>)>,
    extra: &[(&str, Vec<u8>)],
) {
    let mut source =
        zip::ZipArchive::new(std::fs::File::open(archive).expect("open")).expect("zip");
    let mut out = zip::ZipWriter::new(std::fs::File::create(into).expect("create"));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    for index in 0..source.len() {
        let mut entry = source.by_index(index).expect("entry");
        let name = entry.name().expect("name").into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read");
        if let Some((name, bytes)) = change(&name, bytes) {
            out.start_file(name, options).expect("start");
            out.write_all(&bytes).expect("write");
        }
    }
    for (name, bytes) in extra {
        out.start_file(*name, options).expect("start");
        out.write_all(bytes).expect("write");
    }
    out.finish().expect("finish");
}

#[tokio::test]
async fn an_import_resumes_copies_and_refuses_what_it_must() {
    let Some(fixture) = two_workspaces("h1_10_import").await else {
        return;
    };
    let (pool, state, a, b) = (&fixture.pool, &fixture.state, &fixture.a, &fixture.b);
    let folder = tempfile::tempdir().expect("folder");
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT requested_by_actor_id FROM workspace_transfers WHERE id = $1",
    )
    .bind(a.export)
    .fetch_one(pool)
    .await
    .expect("owner");

    // A message that names a task by its address: the copy's must name the
    // copy's task.
    let (status, sent) = send(
        &fixture.app,
        a.browser.request(
            "POST",
            &format!("/api/v1/conversations/{}/messages", a.channel),
            Some("h1-10-address"),
            json!({ "content": text(&format!("see /tasks/{} please", a.task)) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, started) = send(
        &fixture.app,
        a.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/workspace_export_start", a.organization),
            Some("h1-10-export"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{started}");
    let export = started["value"]["id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("export");
    transfer::run_export(state, export).await.expect("export");
    let archive = folder.path().join("a.awx");
    archive_of(&fixture, export, &archive).await;

    // Stopped after three batches, then resumed.
    let resumed = import::start_import(
        state,
        StartImport {
            path: &archive,
            owner,
            slug: None,
        },
    )
    .await
    .expect("start");
    let target = resumed.workspace_id;
    assert_ne!(
        target, a.organization,
        "a copy on its own server is a new workspace"
    );
    let mode =
        sqlx::query_scalar::<_, String>("SELECT id_mode FROM workspace_transfers WHERE id = $1")
            .bind(resumed.transfer.id)
            .fetch_one(pool)
            .await
            .expect("mode");
    assert_eq!(mode, "copy");
    let lifecycle = |workspace: Uuid| async move {
        sqlx::query_scalar::<_, String>("SELECT lifecycle FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_one(pool)
            .await
            .expect("lifecycle")
    };
    assert_eq!(lifecycle(target).await, "importing");
    let why = |transfer: Uuid| async move {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT error FROM workspace_transfers WHERE id = $1",
        )
        .bind(transfer)
        .fetch_one(pool)
        .await
        .expect("error")
    };
    // Its job, run while another worker's lease on the transfer still
    // holds (one that stopped without letting go), tries again later
    // rather than finishing and leaving the workspace importing.
    import::enqueue(state, target, resumed.transfer.id)
        .await
        .expect("enqueue");
    sqlx::query(
        "UPDATE workspace_transfers SET state = 'running', \
         lease_until = now() + interval '10 minutes' WHERE id = $1",
    )
    .bind(resumed.transfer.id)
    .execute(pool)
    .await
    .expect("held");
    let mut registry = jobs::JobRegistry::new();
    import::register_jobs(&mut registry);
    let options = jobs::WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "test".to_owned(),
    };
    jobs::run_due(state, &registry, &options)
        .await
        .expect("run");
    let job = sqlx::query_as::<_, (String, i32)>(
        "SELECT state, attempts FROM background_jobs WHERE kind = $1 AND unique_key = $2",
    )
    .bind(import::IMPORT_KIND)
    .bind(format!("{}:{}", import::IMPORT_KIND, resumed.transfer.id))
    .fetch_one(pool)
    .await
    .expect("job");
    assert_eq!(job, ("queued".to_owned(), 1), "tried again later");
    assert_eq!(lifecycle(target).await, "importing");
    // The rest of this test drives the import by hand.
    let canceled = jobs::cancel_queued(
        pool,
        &format!("{}:{}", import::IMPORT_KIND, resumed.transfer.id),
    )
    .await
    .expect("cancel");
    assert!(canceled);
    sqlx::query("UPDATE workspace_transfers SET lease_until = NULL WHERE id = $1")
        .bind(resumed.transfer.id)
        .execute(pool)
        .await
        .expect("let go");

    let finished = match import::run_import(
        state,
        resumed.transfer.id,
        RunLimits {
            max_batches: Some(3),
        },
    )
    .await
    {
        Ok(finished) => finished,
        Err(error) => panic!("first part: {error:?} {:?}", why(resumed.transfer.id).await),
    };
    assert!(!finished, "three batches is not the whole import");
    let finished = match import::run_import(state, resumed.transfer.id, RunLimits::default()).await
    {
        Ok(finished) => finished,
        Err(error) => panic!("the rest: {error:?} {:?}", why(resumed.transfer.id).await),
    };
    assert!(finished);
    assert_eq!(lifecycle(target).await, "active");

    // The same archive in one go gives the same workspace.
    let whole = import::start_import(
        state,
        StartImport {
            path: &archive,
            owner,
            slug: None,
        },
    )
    .await
    .expect("start again");
    assert!(
        import::run_import(state, whole.transfer.id, RunLimits::default())
            .await
            .expect("whole")
    );
    assert_eq!(
        shape(pool, target).await,
        shape(pool, whole.workspace_id).await
    );
    assert!(!shape(pool, target).await.is_empty());

    // Every reference points into the copy, text included.
    let copied_task = sqlx::query_scalar::<_, Uuid>(
        "SELECT copy.id FROM work_items copy, work_items original \
         WHERE copy.workspace_id = $1 AND original.id = $2 \
           AND copy.number = original.number AND copy.created_at = original.created_at",
    )
    .bind(target)
    .bind(a.task)
    .fetch_one(pool)
    .await
    .expect("the copied task");
    assert_ne!(copied_task, a.task);
    let bodies = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(message) FROM messages message WHERE message.workspace_id = $1",
    )
    .bind(target)
    .fetch_all(pool)
    .await
    .expect("messages")
    .into_iter()
    .map(|row| row.to_string())
    .collect::<Vec<_>>();
    assert!(
        bodies
            .iter()
            .any(|body| body.contains(&format!("/tasks/{copied_task}"))),
        "the address names the copy's task"
    );
    // Field values are keyed by their field's id: the copy's keys name the
    // copy's fields, so its records keep their values.
    let (keys, strangers) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*), count(*) FILTER (WHERE NOT EXISTS ( \
             SELECT 1 FROM field_definitions field \
             WHERE field.workspace_id = $1 AND field.id::text = key)) \
         FROM collection_records record, jsonb_object_keys(record.field_values) key \
         WHERE record.workspace_id = $1",
    )
    .bind(target)
    .fetch_one(pool)
    .await
    .expect("field keys");
    assert!(keys > 0, "the fixture's record has field values");
    assert_eq!(
        strangers, 0,
        "every field value names one of the copy's fields"
    );
    let originals = [a.task.to_string(), a.channel.clone(), a.page.clone()];
    for table in ["work_items", "messages", "conversations", "project_pages"] {
        let rows = sqlx::query_scalar::<_, Value>(&format!(
            "SELECT to_jsonb(t) FROM {table} t WHERE t.workspace_id = $1"
        ))
        .bind(target)
        .fetch_all(pool)
        .await
        .unwrap_or_else(|error| panic!("{table}: {error}"));
        for row in rows {
            let text = row.to_string();
            for original in &originals {
                assert!(
                    !text.contains(original.as_str()),
                    "{table} names the original {original}"
                );
            }
        }
    }
    // Nothing about the import was news.
    let kinds = sqlx::query_scalar::<_, String>(
        "SELECT type || ':' || COALESCE(thing_kind, '') FROM events WHERE workspace_id = $1 \
         UNION ALL SELECT 'staged ' || type FROM event_staging WHERE workspace_id = $1",
    )
    .bind(target)
    .fetch_all(pool)
    .await
    .expect("kinds");
    assert_eq!(
        news(pool, target, a.organization).await,
        (0, 0, 0),
        "no event or webhook, and only the notifications the archive carried: {kinds:?}"
    );

    // Refusals change nothing.
    let workspaces = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM organizations")
            .fetch_one(pool)
            .await
            .expect("count")
    };
    let before = workspaces().await;
    let refuse = |path: std::path::PathBuf| async move {
        match import::start_import(
            state,
            StartImport {
                path: &path,
                owner,
                slug: None,
            },
        )
        .await
        {
            Err(ImportError::Refused(reason)) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    };
    let source = a.organization.to_string();
    let foreign = b.organization.to_string();
    let naming = folder.path().join("naming.awx");
    let mut changed = false;
    rewritten(
        &archive,
        &naming,
        |name, bytes| {
            if !changed && name.starts_with("things/tasks/") {
                changed = true;
                let text = String::from_utf8(bytes).expect("text");
                return Some((
                    name.to_owned(),
                    text.replacen(&source, &foreign, 1).into_bytes(),
                ));
            }
            Some((name.to_owned(), bytes))
        },
        &[],
    );
    let reason = refuse(naming).await;
    assert!(reason.contains("another workspace"), "{reason}");
    let climbing = folder.path().join("climbing.awx");
    rewritten(
        &archive,
        &climbing,
        |name, bytes| Some((name.to_owned(), bytes)),
        &[("things/../../escape.jsonl", b"{}\n".to_vec())],
    );
    refuse(climbing).await;
    let bomb = folder.path().join("bomb.awx");
    rewritten(
        &archive,
        &bomb,
        |name, bytes| Some((name.to_owned(), bytes)),
        &[("needs/zeros.jsonl", vec![b'0'; 64 * 1024 * 1024])],
    );
    let reason = refuse(bomb).await;
    assert!(reason.contains("far more than it weighs"), "{reason}");
    assert_eq!(
        workspaces().await,
        before,
        "a refused archive makes nothing"
    );

    fixture.finish().await;
}
