//! H1.11: nothing is lost on the way. Workspace A of the shared fixture
//! leaves as an archive and arrives on a fresh server, the way a person
//! moves it at first run; then every exported table is read back on both
//! sides the way the export reads it, ids put through the import's id map,
//! and compared row by row. A column added to an exported table that the
//! import doesn't write back fails here.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

use agent_workspace_server::{
    export::{
        self,
        import::{self, RunLimits, StartImport},
        registry::DECLARATIONS,
    },
    files::store::FileStore,
    setup,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    first_run::post_setup,
    support::{
        app::{app, send},
        db::db,
        two_workspaces::two_workspaces,
    },
};

/// Kinds an archive carries that can't arrive anywhere, each with why. Every
/// one of their rows must be counted in the import's report as left out,
/// never dropped without a word.
const LEFT_OUT: &[(&str, &str)] = &[
    (
        "agent_profiles",
        "an agent's profile names the execution worker it runs on, a machine on the old server that is set up again after import",
    ),
    (
        "agent_runs",
        "a provider run names its execution profile and worker, which never travel; the run's own record (runs) does",
    ),
    (
        "github_pull_requests",
        "a pull request belongs to its GitHub installation, a connection reconnected after import, which brings its pull requests back",
    ),
];

/// Columns an import is declared to write differently, each with why.
const REWRITTEN: &[(&str, &str, &str)] = &[
    (
        "links",
        "created_at",
        "a link read from text is made again from the text as it arrives",
    ),
    (
        "files",
        "storage_key",
        "a file kept by its content (C1.29, shared by the rows holding the same \
         bytes) arrives with the bytes under a key of its own",
    ),
];

/// What the database says about how to compare one table: its primary key,
/// its generated columns (worked out again, never carried), and its
/// nullable references to tables an export never carries (credentials,
/// sessions), which arrive empty.
struct Shape {
    primary: Vec<String>,
    generated: BTreeSet<String>,
    uncarried: BTreeSet<String>,
}

async fn shape(pool: &PgPool, table: &str, carried: &BTreeSet<&str>) -> Shape {
    let primary = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT array(SELECT a.attname::text FROM unnest(i.indkey::int2[]) k \
                      JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k) \
         FROM pg_index i WHERE i.indrelid = $1::regclass AND i.indisprimary",
    )
    .bind(table)
    .fetch_optional(pool)
    .await
    .expect("primary key")
    .unwrap_or_default();
    let generated = sqlx::query_scalar::<_, String>(
        "SELECT attname::text FROM pg_attribute \
         WHERE attrelid = $1::regclass AND attnum > 0 AND attgenerated <> ''",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("generated")
    .into_iter()
    .collect();
    let uncarried = sqlx::query_as::<_, (String, String)>(
        "SELECT c.confrelid::regclass::text, a.attname::text \
         FROM pg_constraint c JOIN pg_attribute a \
           ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey) \
         WHERE c.contype = 'f' AND c.conrelid = $1::regclass AND NOT a.attnotnull",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("references")
    .into_iter()
    .filter(|(target, _)| !carried.contains(target.as_str()))
    .map(|(_, column)| column)
    .collect();
    Shape {
        primary,
        generated,
        uncarried,
    }
}

/// Each exported kind's rows in `workspace` as the export reads them, as
/// JSON objects with sorted keys.
async fn arrived(pool: &PgPool, workspace: Uuid) -> BTreeMap<&'static str, Vec<Value>> {
    let mut connection = pool.acquire().await.expect("connection");
    let mut read = BTreeMap::new();
    for module in DECLARATIONS {
        for kind in module.kinds {
            let lines = export::export_kind(&mut connection, module, kind, workspace)
                .await
                .unwrap_or_else(|error| panic!("read {}: {error}", kind.table));
            read.insert(
                kind.table,
                lines.into_iter().map(|line| line.data).collect(),
            );
        }
    }
    read
}

/// Each kind's lines in the archive, ids put through `map`.
fn archived(
    archive: &std::path::Path,
    map: &BTreeMap<String, String>,
) -> BTreeMap<&'static str, Vec<Value>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).expect("open")).expect("zip");
    let mut read = BTreeMap::new();
    for module in DECLARATIONS {
        for kind in module.kinds {
            let name = format!("things/{}/{}.jsonl", module.module, kind.kind);
            let mut rows = Vec::new();
            if let Ok(mut entry) = zip.by_name(&name) {
                let mut text = String::new();
                entry.read_to_string(&mut text).expect("read lines");
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    let line = serde_json::from_str::<Value>(line).expect("line");
                    let data: Value =
                        serde_json::from_str(&canonical(&line["data"], map)).expect("canonical");
                    rows.push(data);
                }
            }
            read.insert(kind.table, rows);
        }
    }
    read
}

/// A row's identity for matching: its primary key.
fn identity(row: &Value, primary: &[String]) -> String {
    primary
        .iter()
        .map(|column| row.get(column).unwrap_or(&Value::Null).to_string())
        .collect::<Vec<_>>()
        .join("|")
}

/// `value` with object keys sorted and every mapped id swapped.
fn canonical(value: &Value, map: &BTreeMap<String, String>) -> String {
    fn walk(value: &Value, map: &BTreeMap<String, String>) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), walk(value, map)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(|item| walk(item, map)).collect()),
            Value::String(text) => {
                let mut text = text.clone();
                for (source, target) in map {
                    if text.contains(source.as_str()) {
                        text = text.replace(source.as_str(), target);
                    }
                }
                Value::String(text)
            }
            other => other.clone(),
        }
    }
    walk(value, map).to_string()
}

#[tokio::test]
async fn a_workspace_arrives_on_a_fresh_server_with_every_exported_column() {
    let Some(fixture) = two_workspaces("h1_11_source").await else {
        return;
    };
    let Some(fresh) = db("h1_11_target").await else {
        return;
    };
    let a = &fixture.a;

    // An initiative with a milestone under it: the import fills that link
    // in after the records are in, and the registry has to follow.
    for (name, input) in [
        ("initiative_create", json!({"title": "Round trip"})),
        (
            "milestone_create",
            json!({"projectId": "WORK", "name": "Round trip beta", "initiative": "Round trip"}),
        ),
    ] {
        let (status, made) = send(
            &fixture.app,
            a.browser.request(
                "POST",
                &format!("/api/v2/w/{}/ops/{name}", a.organization),
                Some(&format!("h1-11-{name}")),
                input,
            ),
        )
        .await;
        assert!(status.is_success(), "{name}: {made}");
        assert_eq!(made["outcome"], "ok", "{name}: {made}");
    }

    // Out, from the first server.
    let (status, started) = send(
        &fixture.app,
        a.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/workspace_export_start", a.organization),
            Some("h1-11-export"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{started}");
    let export = started["value"]["id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("export");
    crate::support::run_export(&fixture.state, export)
        .await
        .expect("export");
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
    let folder = tempfile::tempdir().expect("folder");
    let archive = folder.path().join("a.awx");
    std::fs::write(&archive, bytes).expect("write archive");

    // In, on a server nobody has signed in to: the first person sets up to
    // import, as at first run.
    let data = tempfile::tempdir().expect("data root");
    let (router, state) = app(&fresh)
        .with(
            "AGENT_WORKSPACE_DATA_ROOT",
            data.path().to_string_lossy().into_owned(),
        )
        .with(
            "AGENT_WORKSPACE_MINIMUM_FREE_BYTES",
            "1073741824".to_owned(),
        )
        .build();
    setup::prepare(&fresh.pool, &state.config)
        .await
        .expect("prepare");
    let code = std::fs::read_to_string(setup::code_path(&state.config)).expect("code");
    let (status, session, _browser) = post_setup(
        &router,
        json!({
            "code": code,
            "person": {
                "displayName": "Mover",
                "loginName": "mover",
                "password": "a long password to move with",
            },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'mover'")
            .fetch_one(&fresh.pool)
            .await
            .expect("owner");
    let started = import::start_import(
        &state,
        StartImport {
            path: &archive,
            owner,
            slug: None,
        },
    )
    .await
    .expect("start the import");
    let mode =
        sqlx::query_scalar::<_, String>("SELECT id_mode FROM workspace_transfers WHERE id = $1")
            .bind(started.transfer.id)
            .fetch_one(&fresh.pool)
            .await
            .expect("mode");
    assert_eq!(mode, "preserve", "a fresh server keeps the archive's ids");
    assert!(
        import::run_import(&state, started.transfer.id, RunLimits::default())
            .await
            .expect("import"),
        "the import finishes"
    );

    // Back, table by table: every row the archive holds is here, column
    // for column, unless the import is declared to change that column.
    let map = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT source_id, target_id FROM workspace_import_ids \
         WHERE transfer_id = $1 AND source_id <> target_id",
    )
    .bind(started.transfer.id)
    .fetch_all(&fresh.pool)
    .await
    .expect("id map")
    .into_iter()
    .map(|(source, target)| (source.to_string(), target.to_string()))
    .collect::<BTreeMap<_, _>>();
    let before = archived(&archive, &map);
    let after = arrived(&fresh.pool, started.workspace_id).await;
    let report =
        sqlx::query_scalar::<_, Value>("SELECT counts FROM workspace_transfers WHERE id = $1")
            .bind(started.transfer.id)
            .fetch_one(&fresh.pool)
            .await
            .expect("report");
    // The registry, which the import's triggers fill: a milestone is under
    // its initiative there, though that column went in last.
    let under = sqlx::query_scalar::<_, bool>(
        "SELECT thing.parent_id IS NOT DISTINCT FROM milestone.initiative_id \
         FROM project_milestones milestone JOIN things thing ON thing.id = milestone.id \
         WHERE milestone.initiative_id IS NOT NULL",
    )
    .fetch_all(&fresh.pool)
    .await
    .expect("milestones under initiatives");
    assert_eq!(under, vec![true], "the milestone's place in the registry");
    let carried = before.keys().copied().collect::<BTreeSet<_>>();
    let mut lost = Vec::new();
    for (table, rows) in &before {
        if LEFT_OUT.iter().any(|(left, _)| left == table) {
            // Declared: every row is reported as left out.
            let reported = report["skipped"][*table].as_u64().unwrap_or(0);
            if reported != rows.len() as u64 {
                lost.push(format!(
                    "{table}: {} rows, {reported} reported left out",
                    rows.len()
                ));
            }
            continue;
        }
        let shape = shape(&fresh.pool, table, &carried).await;
        let here = after
            .get(table)
            .map(|rows| {
                rows.iter()
                    .map(|row| (identity(row, &shape.primary), row))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let mut columns = BTreeSet::new();
        let mut missing = 0;
        for row in rows {
            let Some(arrived) = here.get(&identity(row, &shape.primary)) else {
                missing += 1;
                continue;
            };
            for (column, value) in row.as_object().into_iter().flatten() {
                let now = arrived.get(column);
                let declared = shape.generated.contains(column)
                    || (shape.uncarried.contains(column) && now.is_none_or(Value::is_null))
                    || REWRITTEN
                        .iter()
                        .any(|(kind, name, _)| kind == table && name == column);
                if now != Some(value) && !declared {
                    columns.insert(column.clone());
                }
            }
        }
        if missing > 0 || !columns.is_empty() {
            lost.push(format!(
                "{table}: {missing} of {} rows missing; columns changed: {columns:?}",
                rows.len()
            ));
        }
    }
    assert!(
        before.values().map(Vec::len).sum::<usize>() > 50,
        "the fixture exports a real workspace"
    );
    assert!(
        lost.is_empty(),
        "lost on the way:\n{}\nreport: {report}",
        lost.join("\n")
    );
}
