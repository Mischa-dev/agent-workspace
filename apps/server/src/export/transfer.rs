//! A whole workspace leaving as one archive (H1.9), and the transfers that
//! track it. `workspace_export` records a transfer and queues the
//! `workspace.export` job, which writes the archive (format version 1,
//! a ZIP64 file with the `.awx` extension) to the workspace's store and
//! keeps it 7 days; the requester downloads it with their own sign-in.
//!
//! What goes in: the workspace, a profile stub for each member account
//! (never their other workspaces), every declared kind of thing
//! ([`super::registry`]) with its excluded columns left out, every stored
//! file once by content, and the secrets and connections to set up again,
//! named but never their values. What never does: DMs and groups (they
//! belong to the people in them, R20, not to a workspace), credentials,
//! sessions, secret values, the event log and the search index.
//!
//! The database part is read in one read-only snapshot; the network parts
//! (old page bodies in cold storage, file bytes) are fetched before and
//! after it, never inside it.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    path::Path,
    time::Duration,
};

use axum::{Router, extract::State, response::Response, routing::get};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, TryStreamExt};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{ExportDecl, KindDecl, Line, identifier, registry::DECLARATIONS};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
    events::{self, Audience, NewEvent, catalog},
    files::store::{FileStore, Readers},
    jobs::{JobError, JobRegistry, NewJob},
    policy::Permission,
    spaces::SpaceKind,
    tenancy::tables,
};

pub const EXPORT_KIND: &str = "workspace.export";
pub const SWEEP_KIND: &str = "workspace.transfers_sweep";
pub const FORMAT: &str = "agent-workspace-export";
pub const FORMAT_VERSION: u32 = 1;
/// How long an export's archive is kept for its requester.
pub const KEEP_DAYS: i64 = 7;

/// What leaves out of every archive, said in its manifest.
pub const EXCLUDED: &[&str] = &[
    "credentials, tokens and sessions",
    "secret values (needs/secrets.jsonl names what to enter again)",
    "DMs and groups (they belong to the people in them)",
    "the event log and the search index",
    "delivery, job and rate-limit state",
];

/// One export or import, as its requester sees it.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// `export` or `import`.
    pub direction: String,
    /// `queued`, `running`, `succeeded`, `failed`, or `expired` once an
    /// export's archive is gone.
    pub state: String,
    #[schemars(with = "String")]
    pub requested_by_actor_id: Uuid,
    /// The archive's size, once written.
    pub byte_size: Option<i64>,
    /// How many of each kind it holds.
    pub counts: Value,
    pub error: Option<String>,
    #[schemars(with = "String")]
    pub created_at: DateTime<Utc>,
    #[schemars(with = "Option<String>")]
    pub finished_at: Option<DateTime<Utc>>,
    /// When an export's archive goes.
    #[schemars(with = "Option<String>")]
    pub expires_at: Option<DateTime<Utc>>,
}

pub(crate) const VIEW_COLUMNS: &str = "id, direction, state, requested_by_actor_id, byte_size, \
    counts, error, created_at, finished_at, expires_at";

#[derive(Debug, Deserialize, Serialize)]
struct ExportPayload {
    transfer: Uuid,
}

/// Records an export and queues its job, in the caller's transaction.
pub async fn start_export(
    connection: &mut PgConnection,
    workspace: Uuid,
    by: Uuid,
) -> Result<TransferView, AppError> {
    let id = Uuid::now_v7();
    let view = sqlx::query_as::<_, TransferView>(&format!(
        "INSERT INTO workspace_transfers (id, workspace_id, direction, state, requested_by_actor_id) \
         VALUES ($1, $2, 'export', 'queued', $3) RETURNING {VIEW_COLUMNS}"
    ))
    .bind(id)
    .bind(workspace)
    .bind(by)
    .fetch_one(&mut *connection)
    .await?;
    crate::jobs::enqueue(
        &mut *connection,
        NewJob::new(EXPORT_KIND)
            .payload(ExportPayload { transfer: id })
            .workspace(workspace)
            .unique(format!("{EXPORT_KIND}:{id}")),
    )
    .await?;
    events::emit(&mut *connection, changed(workspace, id, by)).await?;
    Ok(view)
}

/// The fact that a transfer moved on, for its requester only.
pub(crate) fn changed(workspace: Uuid, transfer: Uuid, requester: Uuid) -> NewEvent<'static> {
    NewEvent {
        id: Uuid::now_v7(),
        ty: &catalog::WORKSPACE_TRANSFER_CHANGED,
        workspace_id: Some(workspace),
        space_id: None,
        thing: Some((transfer, "workspace_transfer")),
        access_thing: None,
        version: None,
        changes: &["state"],
        audience: Audience::Account(requester),
        payload: json!({}),
        actor: None,
        legacy_type: None,
    }
}

/// One transfer of `workspace`, when `reader` asked for it.
pub async fn transfer_of(
    connection: &mut PgConnection,
    workspace: Uuid,
    reader: Uuid,
    id: Uuid,
) -> Result<Option<TransferView>, AppError> {
    Ok(sqlx::query_as::<_, TransferView>(&format!(
        "SELECT {VIEW_COLUMNS} FROM workspace_transfers \
         WHERE id = $1 AND workspace_id = $2 AND requested_by_actor_id = $3"
    ))
    .bind(id)
    .bind(workspace)
    .bind(reader)
    .fetch_optional(&mut *connection)
    .await?)
}

/// `reader`'s transfers of `workspace`, newest first.
pub async fn transfers_of(
    connection: &mut PgConnection,
    workspace: Uuid,
    reader: Uuid,
) -> Result<Vec<TransferView>, AppError> {
    Ok(sqlx::query_as::<_, TransferView>(&format!(
        "SELECT {VIEW_COLUMNS} FROM workspace_transfers \
         WHERE workspace_id = $1 AND requested_by_actor_id = $2 \
         ORDER BY created_at DESC LIMIT 50"
    ))
    .bind(workspace)
    .bind(reader)
    .fetch_all(&mut *connection)
    .await?)
}

/// Where a finished export's archive is, for its requester.
pub async fn archive_of(
    connection: &mut PgConnection,
    workspace: Uuid,
    reader: Uuid,
    id: Uuid,
) -> Result<Option<(String, String)>, AppError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT store_id, file_key FROM workspace_transfers \
         WHERE id = $1 AND workspace_id = $2 AND requested_by_actor_id = $3 \
           AND direction = 'export' AND state = 'succeeded' AND expires_at > now() \
           AND store_id IS NOT NULL AND file_key IS NOT NULL",
    )
    .bind(id)
    .bind(workspace)
    .bind(reader)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The address an export's archive downloads from.
pub fn download_path(workspace: Uuid, transfer: Uuid) -> String {
    format!("/api/v1/organizations/{workspace}/transfers/{transfer}/archive")
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/organizations/{organization_id}/transfers/{transfer_id}/archive",
        get(download),
    )
}

/// The archive, to the account that asked for it while it still may
/// export the workspace; anyone else gets not found.
async fn download(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((workspace, transfer)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let found = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let may = crate::roles::standing(&mut connection, workspace, principal.actor_id)
            .await
            .ok()
            .is_some_and(|standing| standing.permissions.contains(Permission::WorkspaceExport));
        let found = if may {
            archive_of(&mut connection, workspace, principal.actor_id, transfer).await?
        } else {
            None
        };
        connection.commit().await?;
        found
    };
    let (store_id, key) = found.ok_or(AppError::NotFound("transfer"))?;
    crate::files::http::serve(
        &state,
        &store_id,
        &key,
        "application/zip",
        &format!("workspace-{}.awx", transfer.simple()),
        true,
    )
    .await
}

/// Write the archive for one export transfer.
pub async fn run_export(state: &AppState, transfer: Uuid) -> Result<(), AppError> {
    let Some((workspace, requester)) = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "UPDATE workspace_transfers SET state = 'running' \
             WHERE id = $1 AND direction = 'export' AND state IN ('queued', 'running') \
             RETURNING workspace_id, requested_by_actor_id",
        )
        .bind(transfer)
        .fetch_optional(&mut *connection)
        .await
    })
    .await?
    else {
        return Ok(());
    };
    let folder = state.config.data_root.join("tmp");
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|_| AppError::Internal("the export folder could not be made"))?;
    let path = folder.join(format!("export-{}.awx", transfer.simple()));
    let outcome = write_archive(state, workspace, &path).await;
    let finish = match outcome {
        Ok(written) => store_archive(state, workspace, transfer, &path, written).await,
        Err(error) => Err(error),
    };
    let _ = tokio::fs::remove_file(&path).await;
    if let Err(error) = &finish {
        let reason = error.log_category();
        crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query(
                "UPDATE workspace_transfers SET state = 'failed', error = $2, finished_at = now() \
                 WHERE id = $1",
            )
            .bind(transfer)
            .bind(format!("The export stopped ({reason}). Start another."))
            .execute(&mut *connection)
            .await?;
            events::emit(connection, changed(workspace, transfer, requester)).await?;
            Ok::<_, AppError>(())
        })
        .await?;
    }
    finish
}

async fn store_archive(
    state: &AppState,
    workspace: Uuid,
    transfer: Uuid,
    path: &Path,
    written: Written,
) -> Result<(), AppError> {
    let store = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        FileStore::for_uploads(state, &mut connection, workspace, Readers::Members).await?
    };
    let key = format!("{}/exports/{}.awx", workspace.simple(), transfer.simple());
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::Internal("the export could not be read back"))?;
    let chunks = tokio_util::io::ReaderStream::new(file);
    store.put_stream(&key, chunks, u64::MAX).await?;
    crate::db::scoped_one(&state.pool, async |connection| {
        let requester = sqlx::query_scalar::<_, Uuid>(
            "UPDATE workspace_transfers SET state = 'succeeded', store_id = $2, file_key = $3, \
             byte_size = $4, counts = $5, finished_at = now(), \
             expires_at = now() + make_interval(days => $6) \
             WHERE id = $1 RETURNING requested_by_actor_id",
        )
        .bind(transfer)
        .bind(store.id())
        .bind(&key)
        .bind(i64::try_from(written.bytes).unwrap_or(i64::MAX))
        .bind(Value::Object(written.counts.into_iter().collect()))
        .bind(KEEP_DAYS as i32)
        .fetch_one(&mut *connection)
        .await?;
        events::emit(connection, changed(workspace, transfer, requester)).await?;
        Ok::<_, AppError>(())
    })
    .await
}

/// What the writer wrote.
struct Written {
    bytes: u64,
    counts: BTreeMap<String, Value>,
}

/// A stored file to copy into the archive once.
struct Blob {
    sha256: String,
    size: i64,
    store_id: String,
    key: String,
}

/// How long a file's store may go quiet mid-file before the export
/// gives up on it.
const BLOB_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

fn options(compress: bool) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(if compress {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        })
        .large_file(true)
}

fn zip_error<E>(_: E) -> AppError {
    AppError::Internal("the export archive could not be written")
}

/// Whether the snapshot holds a page version kept in cold storage whose
/// body `cold` lacks.
async fn cold_missing(
    snapshot: &mut sqlx::PgConnection,
    workspace: Uuid,
    cold: &HashMap<(Uuid, i64), String>,
) -> Result<bool, AppError> {
    let kept = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT page_id, version::bigint FROM project_page_revisions \
         WHERE workspace_id = $1 AND body IS NULL",
    )
    .bind(workspace)
    .fetch_all(&mut *snapshot)
    .await?;
    Ok(kept.iter().any(|key| !cold.contains_key(key)))
}

/// Writes the whole archive to `path`.
async fn write_archive(
    state: &AppState,
    workspace: Uuid,
    path: &Path,
) -> Result<Written, AppError> {
    // Before the snapshot: bodies of page versions kept in cold storage
    // (K3.25), so every version leaves whole. A version archived after they
    // were read and before the snapshot would leave without its body: the
    // snapshot checks it has every one, and they are read again if not.
    let (cold, mut snapshot) = {
        let mut tries = 0;
        loop {
            let cold = crate::history::cold::cold_page_bodies(state, workspace).await?;
            let mut snapshot = crate::db::scoped_snapshot(&state.pool).await?;
            if !cold_missing(&mut snapshot, workspace, &cold).await? {
                break (cold, snapshot);
            }
            snapshot.rollback().await?;
            tries += 1;
            if tries == 3 {
                return Err(AppError::Conflict(
                    "export",
                    "page history kept moving to cold storage; try again",
                ));
            }
        }
    };
    let file = std::fs::File::create(path)
        .map_err(|_| AppError::Internal("the export file could not be made"))?;
    let mut zip = ZipWriter::new(file);
    let mut counts = BTreeMap::new();
    let (blobs, slug) = {
        let hidden = not_spaces(&mut snapshot, workspace).await?;
        let left_out = left_out(&mut snapshot, workspace, &hidden).await?;
        let header = workspace_entry(&mut snapshot, workspace).await?;
        write_json(&mut zip, "workspace/workspace.json", &header)?;
        let slug = header["slug"].clone();
        for (entry, rows) in accounts(&mut snapshot, workspace).await? {
            counts.insert(entry.to_owned(), json!(rows.len()));
            write_lines(&mut zip, entry, rows.iter())?;
        }
        let mut skipped = Vec::new();
        for declaration in DECLARATIONS {
            for kind in declaration.kinds {
                let Some(column) = tables::tenant_column(kind.table) else {
                    skipped.push(kind.table);
                    continue;
                };
                let written = write_kind(
                    &mut snapshot,
                    &mut zip,
                    Kind {
                        declaration,
                        kind,
                        column,
                    },
                    workspace,
                    &hidden,
                    &left_out,
                    &cold,
                )
                .await?;
                if written > 0 {
                    counts.insert(
                        format!("{}/{}", declaration.module, kind.kind),
                        json!(written),
                    );
                }
            }
        }
        if !skipped.is_empty() {
            counts.insert("skipped".to_owned(), json!(skipped));
        }
        let (index, blobs) = files_index(&mut snapshot, workspace, &hidden, &left_out).await?;
        counts.insert("files".to_owned(), json!(index.len()));
        write_lines(&mut zip, "files/index.jsonl", index.iter())?;
        let (secrets, connections) = needs(&mut snapshot, workspace).await?;
        write_lines(&mut zip, "needs/secrets.jsonl", secrets.iter())?;
        write_lines(&mut zip, "needs/connections.jsonl", connections.iter())?;
        snapshot.commit().await?;
        (blobs, slug)
    };
    // After the snapshot: the files' bytes, once per content.
    let mut bytes = 0_u64;
    for blob in &blobs {
        let store = FileStore::resolve(state, &blob.store_id).await?;
        let stored = store.get(&blob.key).await?;
        zip.start_file(format!("files/blobs/{}", blob.sha256), options(false))
            .map_err(zip_error)?;
        // Taken only as recorded: at most the file's size, its digest
        // checked, and a store that goes quiet is given up on. A store a
        // workspace set can serve anything; the archive gets only the
        // bytes the file row vouches for.
        let size = u64::try_from(blob.size).unwrap_or(0);
        let mut read = 0_u64;
        let mut digest = Sha256::new();
        let mut stream = stored.body.into_data_stream();
        loop {
            let next = tokio::time::timeout(BLOB_IDLE, stream.next())
                .await
                .map_err(|_| AppError::Internal("a file's store stopped sending"))?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|_| AppError::Internal("a file could not be read"))?;
            read += chunk.len() as u64;
            if read > size {
                return Err(AppError::Internal(
                    "a stored file is bigger than its record",
                ));
            }
            digest.update(&chunk);
            zip.write_all(&chunk).map_err(zip_error)?;
        }
        if read != size || hex::encode(digest.finalize()) != blob.sha256 {
            return Err(AppError::Internal("a stored file doesn't match its record"));
        }
        bytes += read;
    }
    let manifest = json!({
        "format": FORMAT,
        "formatVersion": FORMAT_VERSION,
        "exportedAt": Utc::now(),
        "source": {
            "serverVersion": env!("CARGO_PKG_VERSION"),
            "origin": state.config.public_origin.as_deref(),
        },
        "workspace": {
            "id": workspace,
            "slug": slug,
        },
        "idMode": "preserve",
        "modules": modules_of(&counts),
        "files": { "count": blobs.len(), "bytes": bytes },
        "excluded": EXCLUDED,
    });
    write_json(&mut zip, "manifest.json", &manifest)?;
    let file = zip.finish().map_err(zip_error)?;
    let size = file
        .metadata()
        .map_err(|_| AppError::Internal("the export file could not be read"))?
        .len();
    Ok(Written {
        bytes: size,
        counts,
    })
}

/// `modules` of the manifest: each module's export version and the kinds
/// it wrote, with how many.
fn modules_of(counts: &BTreeMap<String, Value>) -> Value {
    let mut modules = serde_json::Map::new();
    for declaration in DECLARATIONS {
        let kinds = declaration
            .kinds
            .iter()
            .filter_map(|kind| {
                counts
                    .get(&format!("{}/{}", declaration.module, kind.kind))
                    .map(|count| (kind.kind.to_owned(), count.clone()))
            })
            .collect::<serde_json::Map<_, _>>();
        modules.insert(
            declaration.module.to_owned(),
            json!({ "version": declaration.version, "kinds": kinds }),
        );
    }
    Value::Object(modules)
}

fn write_json(
    zip: &mut ZipWriter<std::fs::File>,
    name: &str,
    value: &Value,
) -> Result<(), AppError> {
    zip.start_file(name, options(true)).map_err(zip_error)?;
    serde_json::to_writer_pretty(&mut *zip, value).map_err(zip_error)?;
    Ok(())
}

fn write_lines<'a>(
    zip: &mut ZipWriter<std::fs::File>,
    name: &str,
    lines: impl Iterator<Item = &'a Value>,
) -> Result<(), AppError> {
    zip.start_file(name, options(true)).map_err(zip_error)?;
    for line in lines {
        serde_json::to_writer(&mut *zip, line).map_err(zip_error)?;
        zip.write_all(b"\n").map_err(zip_error)?;
    }
    Ok(())
}

/// The workspace's direct project, which is not a space: nothing in it is
/// the workspace's to export (R20).
async fn not_spaces(connection: &mut PgConnection, workspace: Uuid) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, kind FROM projects WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .filter(|(_, kind)| SpaceKind::from_stored(kind).is_none())
    .map(|(id, _)| id)
    .collect())
}

/// `hidden`: DMs and groups in workspace `$1` (and threads in them), their
/// messages, and the projects that are not spaces (the array parameter
/// [`with_hidden`] names). No line of the
/// archive may name one (R20). A read-only snapshot can't fill a table, so
/// each query that needs it carries it.
const HIDDEN: &str = r#"
    -- visibility: these rows are left out of an export, never shown
    RECURSIVE direct AS (
        SELECT id FROM conversations
        WHERE project_id = ANY(NOT_SPACES)
           OR (kind IN ('dm', 'group')
               AND (workspace_id = $1
                    OR project_id IN (SELECT id FROM projects WHERE organization_id = $1)
                    -- in no workspace: one with a member of this workspace,
                    -- whose rows here may name it
                    OR (workspace_id = direct_scope_id()
                        AND EXISTS (
                            SELECT 1 FROM conversation_members member
                            JOIN organization_memberships workspace_member
                              ON workspace_member.actor_id = member.actor_id
                             AND workspace_member.organization_id = $1
                            WHERE member.conversation_id = conversations.id))))
        UNION
        SELECT child.id FROM conversations child
        JOIN direct ON child.parent_channel_id = direct.id
    ),
    hidden AS (
        SELECT id FROM direct
        UNION
        SELECT message.id FROM messages message JOIN direct ON message.conversation_id = direct.id
        UNION
        SELECT unnest(NOT_SPACES::uuid[])
    )
"#;

/// `WITH` and [`HIDDEN`], its project array at parameter `$<parameter>`.
fn with_hidden(parameter: u8) -> String {
    format!(
        "WITH {} ",
        HIDDEN.replace("NOT_SPACES", &format!("${parameter}"))
    )
}

/// `workspace/workspace.json`: name, address and settings.
async fn workspace_entry(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Value, AppError> {
    let (name, slug) =
        sqlx::query_as::<_, (String, String)>("SELECT name, slug FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_one(&mut *connection)
            .await?;
    Ok(json!({ "id": workspace, "name": name, "slug": slug }))
}

/// Profile stubs for every account in the workspace (R19): what this
/// workspace knows of them, never their other workspaces, credentials or
/// sign-ins.
async fn accounts(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<(&'static str, Vec<Value>)>, AppError> {
    let rows = sqlx::query_as::<_, (String, Value)>(
        r#"
        SELECT actor.account_kind,
               jsonb_strip_nulls(jsonb_build_object(
                   'id', actor.id,
                   'kind', actor.account_kind,
                   -- an agent that runs on a profile keeps its memberships' worker role
                   'profile', actor.kind = 'execution_worker', -- // kind-check: mechanical (how the account runs)
                   'displayName', COALESCE(profile.display_name, person.display_name),
                   'handle', profile.handle,
                   'ownedByThisWorkspace', actor.owner_workspace_id = $1,
                   'emails', (SELECT jsonb_agg(jsonb_build_object(
                                     'address', email.address,
                                     'verified', email.verified_at IS NOT NULL)
                                 ORDER BY email.address)
                              FROM user_email_addresses email
                              WHERE email.user_id = person.id)))
        FROM actors actor
        LEFT JOIN users person ON person.actor_id = actor.id
        LEFT JOIN organization_actor_profiles profile
          ON profile.organization_id = $1 AND profile.actor_id = actor.id
        WHERE actor.id IN (
            SELECT actor_id FROM organization_memberships WHERE organization_id = $1
            -- a space's worker, unlisted in the workspace (K1.5), travels
            -- with its seat; its machine is set up again after import
            UNION
            SELECT seat.actor_id FROM project_memberships seat
            JOIN projects space ON space.id = seat.project_id
            JOIN actors worker ON worker.id = seat.actor_id
            WHERE space.organization_id = $1
              AND worker.kind = 'execution_worker') -- // kind-check: mechanical (how the account runs)
        ORDER BY actor.id
        "#,
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    let mut people = Vec::new();
    let mut agents = Vec::new();
    let mut apps = Vec::new();
    for (kind, stub) in rows {
        let line = json!({ "kind": "account", "v": 1, "id": stub["id"], "data": stub });
        match kind.as_str() {
            "agent" => agents.push(line),
            "app" => apps.push(line),
            _ => people.push(line),
        }
    }
    Ok(vec![
        ("accounts/people.jsonl", people),
        ("accounts/agents.jsonl", agents),
        ("accounts/apps.jsonl", apps),
    ])
}

/// The tables whose rows may be hidden (tables, not modules).
const MAY_HIDE: [&str; 3] = ["conversations", "messages", "projects"];

/// Columns of `kind`'s table that name a conversation, a message or a
/// space (any of which may be hidden).
async fn naming_columns(
    connection: &mut PgConnection,
    kind: &KindDecl,
) -> Result<Vec<String>, AppError> {
    let mut columns = kind
        .references
        .iter()
        .filter(|(_, target)| MAY_HIDE.contains(target))
        .map(|(column, _)| (*column).to_owned())
        .collect::<Vec<_>>();
    if MAY_HIDE.contains(&kind.table) {
        columns.push("id".to_owned());
    }
    // A column that names one without being declared still counts.
    let found = sqlx::query_scalar::<_, String>(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = $1 AND data_type = 'uuid' \
           AND (column_name LIKE '%conversation_id' OR column_name LIKE '%message_id' \
                OR column_name IN ('thread_id', 'project_id', 'space_id'))",
    )
    .bind(kind.table)
    .fetch_all(&mut *connection)
    .await?;
    for column in found {
        if !columns.contains(&column) {
            columns.push(column);
        }
    }
    Ok(columns)
}

/// Rows left out of an export because a row they point at is (R20, to
/// any depth): a run in a DM is left out by its conversation, and its
/// activities, usage and requests by their run. Worked out once per
/// snapshot, from every exported table's single-column foreign keys.
struct LeftOut {
    /// Every left-out row's id, the hidden conversations and messages
    /// included.
    ids: Vec<Uuid>,
    /// Each table's foreign key columns that point at a table rows were
    /// left out of.
    pointing: HashMap<String, Vec<String>>,
    /// Exported tables with an `id` column.
    with_id: HashSet<String>,
}

impl LeftOut {
    /// ` AND (...) IS NOT TRUE` for a table: none of its rows' foreign keys may
    /// name a left-out row, nor may its own id be one. The ids are at
    /// parameter `$<parameter>`.
    fn filter(&self, table: &str, parameter: u8) -> String {
        let mut tests = Vec::new();
        if self.with_id.contains(table) {
            tests.push(format!("t.id = ANY(${parameter})"));
        }
        for column in self.pointing.get(table).into_iter().flatten() {
            tests.push(format!("t.{} = ANY(${parameter})", plain(column)));
        }
        if tests.is_empty() {
            String::new()
        } else {
            // A null foreign key names nothing: `IS NOT TRUE` keeps the
            // row, where `NOT (NULL)` would drop it.
            format!(" AND ({}) IS NOT TRUE", tests.join(" OR "))
        }
    }
}

/// Works out [`LeftOut`]: starting from the hidden conversations and
/// messages, any row whose foreign key names a left-out row is left out
/// too, until nothing more is.
async fn left_out(
    connection: &mut PgConnection,
    workspace: Uuid,
    not_spaces: &[Uuid],
) -> Result<LeftOut, AppError> {
    let mut ids =
        sqlx::query_scalar::<_, Uuid>(&format!("{}SELECT id FROM hidden", with_hidden(2)))
            .bind(workspace)
            .bind(not_spaces)
            .fetch_all(&mut *connection)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
    // Each foreign key's column that names its target's id (a key with
    // more columns, such as a run's space and conversation, by the one
    // pointing at the id), so each step can ask one table at a time.
    let keys = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT DISTINCT child.relname::text, column_name.attname::text, parent.relname::text
        FROM pg_constraint constraint_row
        CROSS JOIN LATERAL unnest(constraint_row.conkey, constraint_row.confkey)
            AS pair(child_column, parent_column)
        JOIN pg_class child ON child.oid = constraint_row.conrelid
        JOIN pg_class parent ON parent.oid = constraint_row.confrelid
        JOIN pg_namespace space ON space.oid = child.relnamespace
        JOIN pg_attribute column_name
          ON column_name.attrelid = child.oid AND column_name.attnum = pair.child_column
        JOIN pg_attribute target
          ON target.attrelid = parent.oid AND target.attnum = pair.parent_column
        WHERE constraint_row.contype = 'f' AND target.attname = 'id'
          AND space.nspname = current_schema()
          AND column_name.atttypid = 'uuid'::regtype
        ORDER BY 1, 2, 3
        "#,
    )
    .fetch_all(&mut *connection)
    .await?;
    let exported = DECLARATIONS
        .iter()
        .flat_map(|declaration| declaration.kinds.iter())
        .filter_map(|kind| tables::tenant_column(kind.table).map(|column| (kind.table, column)))
        .collect::<HashMap<_, _>>();
    let mut tables_left_out: HashSet<String> = ["conversations", "messages", "projects"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    let with_id = sqlx::query_scalar::<_, String>(
        "SELECT table_name::text FROM information_schema.columns \
         WHERE table_schema = current_schema() AND column_name = 'id' AND data_type = 'uuid'",
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .filter(|table| exported.contains_key(table.as_str()))
    .collect::<HashSet<_>>();
    // Each round reaches one level further; the chain from a DM down is
    // short, and a round that finds nothing new ends it.
    for _ in 0..8 {
        let mut found = false;
        for (child, column, parent) in &keys {
            if !tables_left_out.contains(parent) {
                continue;
            }
            let Some(tenant) = exported.get(child.as_str()) else {
                continue;
            };
            if !with_id.contains(child) {
                continue;
            }
            let current = ids.iter().copied().collect::<Vec<_>>();
            let reached = sqlx::query_scalar::<_, Uuid>(&format!(
                "SELECT t.id FROM {} t WHERE t.{} = $1 AND t.{} = ANY($2)",
                plain(child),
                identifier(tenant),
                plain(column)
            ))
            .bind(workspace)
            .bind(&current)
            .fetch_all(&mut *connection)
            .await?;
            for id in reached {
                if ids.insert(id) {
                    found = true;
                    tables_left_out.insert(child.clone());
                }
            }
        }
        if !found {
            break;
        }
    }
    let mut pointing: HashMap<String, Vec<String>> = HashMap::new();
    for (child, column, parent) in &keys {
        if tables_left_out.contains(parent) && exported.contains_key(child.as_str()) {
            pointing
                .entry(child.clone())
                .or_default()
                .push(column.clone());
        }
    }
    Ok(LeftOut {
        ids: ids.into_iter().collect(),
        pointing,
        with_id,
    })
}

/// One declared kind and the column that names its workspace.
struct Kind<'a> {
    declaration: &'a ExportDecl,
    kind: &'a KindDecl,
    column: &'static str,
}

/// One kind's rows as lines in `things/<module>/<kind>.jsonl`, oldest
/// first, so a row comes after the rows of its own kind it points at (a
/// reply after its parent). Returns how many.
async fn write_kind(
    connection: &mut PgConnection,
    zip: &mut ZipWriter<std::fs::File>,
    Kind {
        declaration,
        kind,
        column,
    }: Kind<'_>,
    workspace: Uuid,
    not_spaces: &[Uuid],
    left_out: &LeftOut,
    cold: &HashMap<(Uuid, i64), String>,
) -> Result<usize, AppError> {
    let table = identifier(kind.table);
    let column = identifier(column);
    let naming = naming_columns(connection, kind).await?;
    let parents = left_out.filter(kind.table, 4);
    let (with, hidden) = if naming.is_empty() {
        (String::new(), String::new())
    } else {
        let names = naming
            .iter()
            .map(|name| format!("t.{}", plain(name)))
            .collect::<Vec<_>>()
            .join(", ");
        (
            with_hidden(3),
            format!(" AND NOT EXISTS (SELECT 1 FROM hidden h WHERE h.id IN ({names}))"),
        )
    };
    let excluded = kind
        .exclude
        .iter()
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    let sql = format!(
        "{with}SELECT to_jsonb(t) - $2::text[] FROM {table} t WHERE t.{column} = $1{hidden}{parents} \
         ORDER BY to_jsonb(t)->>'created_at', to_jsonb(t)->>'id'"
    );
    let mut rows = sqlx::query_scalar::<_, Value>(&sql)
        .bind(workspace)
        .bind(&excluded)
        .bind(not_spaces)
        .bind(&left_out.ids)
        .fetch(&mut *connection);
    let name = format!("things/{}/{}.jsonl", declaration.module, kind.kind);
    let mut started = false;
    let mut count = 0;
    while let Some(mut data) = rows.try_next().await? {
        if kind.table == "project_page_revisions" && data["body"].is_null() {
            let key = (
                data["page_id"]
                    .as_str()
                    .and_then(|id| id.parse().ok())
                    .unwrap_or_default(),
                data["version"].as_i64().unwrap_or_default(),
            );
            // `write_archive` checked the snapshot has every cold body.
            let body = cold
                .get(&key)
                .ok_or(AppError::Internal("a cold page version has no body"))?;
            data["body"] = Value::String(body.clone());
        }
        if !started {
            zip.start_file(&name, options(true)).map_err(zip_error)?;
            started = true;
        }
        let line = Line {
            kind: kind.kind.to_owned(),
            v: declaration.version,
            id: data
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            data,
        };
        serde_json::to_writer(&mut *zip, &line).map_err(zip_error)?;
        zip.write_all(b"\n").map_err(zip_error)?;
        count += 1;
    }
    Ok(count)
}

fn plain(name: &str) -> &str {
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "column names are plain: {name}"
    );
    name
}

/// `files/index.jsonl` and the bytes to copy, once per content.
async fn files_index(
    connection: &mut PgConnection,
    workspace: Uuid,
    not_spaces: &[Uuid],
    left_out: &LeftOut,
) -> Result<(Vec<Value>, Vec<Blob>), AppError> {
    let parents = left_out.filter("files", 3);
    let rows = sqlx::query_as::<_, (Uuid, String, i64, String, String, String)>(&format!(
        r#"
        {}
        SELECT file.id, file.sha256, file.byte_size, file.content_type, file.store_id,
               file.storage_key
        FROM files file
        WHERE file.organization_id = $1
          AND file.blob_deleted_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM hidden h
                          WHERE h.id IN (file.conversation_id, file.message_id, file.project_id))
          {}
        ORDER BY file.id
        "#,
        with_hidden(2),
        parents.replace("t.", "file.")
    ))
    .bind(workspace)
    .bind(not_spaces)
    .bind(&left_out.ids)
    .fetch_all(&mut *connection)
    .await?;
    let mut seen = HashSet::new();
    let mut index = Vec::new();
    let mut blobs = Vec::new();
    for (id, sha256, size, content_type, store_id, key) in rows {
        index.push(json!({
            "kind": "file", "v": 1, "id": id,
            "data": { "id": id, "sha256": sha256, "size": size, "contentType": content_type },
        }));
        if seen.insert(sha256.clone()) {
            blobs.push(Blob {
                sha256,
                size,
                store_id,
                key,
            });
        }
    }
    Ok((index, blobs))
}

/// What the new home must set up again: secrets by name (never a value)
/// and connections (a bucket, agent connections, code host installs).
async fn needs(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<(Vec<Value>, Vec<Value>), AppError> {
    let secrets = sqlx::query_scalar::<_, Value>(
        "SELECT jsonb_strip_nulls(jsonb_build_object('kind', 'secret', 'v', 1, 'id', id, \
         'data', jsonb_build_object('id', id, 'name', name, 'projectId', project_id, \
         'ownerAccountId', owner_actor_id, 'systemManaged', system_managed))) \
         FROM secrets WHERE organization_id = $1 ORDER BY name",
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    let connections = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object('kind', 'connection', 'v', 1, 'id', id, 'data', data)
        FROM (
            SELECT id, jsonb_build_object('type', 'file_store', 'endpoint', endpoint,
                                          'bucket', bucket, 'region', region) AS data
            FROM workspace_stores WHERE workspace_id = $1 AND retired_at IS NULL
            UNION ALL
            SELECT id, jsonb_build_object('type', 'agent_connection', 'accountId', actor_id,
                                          'name', connection_name, 'runtime', runtime_kind)
            FROM external_agent_connections
            WHERE workspace_id = $1 AND removed_at IS NULL
            UNION ALL
            SELECT id, jsonb_build_object('type', 'code_host_install', 'projectId', project_id)
            FROM github_installations WHERE workspace_id = $1
        ) connection
        ORDER BY id
        "#,
    )
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    Ok((secrets, connections))
}

/// Archives past their 7 days go, with their bytes (a listed retention
/// job, rule 16: an archive is a copy, never the record).
pub async fn sweep(state: &AppState) -> Result<usize, AppError> {
    // A server-wide job: it runs in the jobs' own server scope.
    let expired = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String)>(
            "SELECT id, workspace_id, requested_by_actor_id, store_id, file_key \
             FROM workspace_transfers \
             WHERE state = 'succeeded' AND expires_at <= now() \
               AND store_id IS NOT NULL AND file_key IS NOT NULL \
             LIMIT 100",
        )
        .fetch_all(&mut *connection)
        .await
    })
    .await?;
    let count = expired.len();
    for (id, workspace, requester, store_id, key) in expired {
        FileStore::resolve(state, &store_id)
            .await?
            .delete(&key)
            .await?;
        crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query("UPDATE workspace_transfers SET state = 'expired' WHERE id = $1")
                .bind(id)
                .execute(&mut *connection)
                .await?;
            events::emit(connection, changed(workspace, id, requester)).await?;
            Ok::<_, AppError>(())
        })
        .await?;
    }
    Ok(count)
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(EXPORT_KIND, |context| async move {
        let payload = context.job.payload_as::<ExportPayload>()?;
        run_export(&context.state, payload.transfer)
            .await
            .map_err(JobError::from)
    });
    registry.register(SWEEP_KIND, |context| async move {
        sweep(&context.state).await.map_err(JobError::from)?;
        Ok(())
    });
    registry.every(SWEEP_KIND, Duration::from_secs(6 * 60 * 60));
}
