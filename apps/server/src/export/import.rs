//! A workspace arriving from an archive (H1.10): the other half of
//! [`super::transfer`].
//!
//! [`start_import`] reads the whole archive once before anything is
//! written ([`inspect`]): its entry names and sizes (no `../`, no absolute
//! paths, no links, nothing that unpacks to far more than it weighs), its
//! manifest, and every line, which must be a declared kind at its module's
//! version and must name no workspace but its own. Then, in one
//! transaction, it makes the new workspace (marked `importing`, so it
//! refuses writes), the transfer, and the id map:
//!
//! - **Ids are kept** unless one already exists here (the archive came
//!   from this server): then every id the archive carries gets a fresh one
//!   (**copy mode**), and every reference, including ids inside text and
//!   addresses, is rewritten through `workspace_import_ids`.
//! - **Accounts** (R19): a person is matched by a verified email address,
//!   or is made as an unclaimed account to claim later; an agent or app
//!   already here is matched by its id, otherwise it arrives disabled,
//!   "needs connecting", with no credential.
//!
//! The archive is copied into the new workspace's store, and the
//! `workspace.import` job fills the workspace in batches, each batch and
//! the cursor saying where it got to committed together, so an import that
//! stops resumes where it was and gives the same result. Tables go in the
//! order their foreign keys need (read from the database, not assumed);
//! where two tables point at each other, the nullable side is filled in
//! afterwards. Rows the new workspace made for itself (its own space, its
//! built-in roles) are matched by their unique keys instead of copied.
//! Each batch sets `aw.importing`, so the triggers that stage events stage
//! none: nothing an import writes notifies anyone or fires a webhook.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;
use zip::ZipArchive;

use super::{
    ExportDecl, KindDecl, Line, identifier,
    registry::DECLARATIONS,
    transfer::{self, FORMAT, FORMAT_VERSION, TransferView, VIEW_COLUMNS},
};
use crate::{
    AppState,
    accounts::{NewPerson, create_person},
    error::AppError,
    events,
    files::store::FileStore,
    jobs::{JobError, JobRegistry, NewJob},
    tenancy::tables,
    workspace::lifecycle::{self, NewWorkspace},
};

pub const IMPORT_KIND: &str = "workspace.import";
/// Lines per batch.
const BATCH: usize = 500;
/// How long one process holds a transfer before another may take it over.
const LEASE_SECONDS: i32 = 600;
/// Entries one archive may hold.
const MAX_ENTRIES: usize = 4_000_000;
/// The most one entry may unpack to.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// The most a whole archive may unpack to.
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
/// How much more an entry over a megabyte may unpack to than it weighs.
const MAX_RATIO: u64 = 400;
/// The longest line read.
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Why an archive was refused, said to whoever imports it.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("the archive was refused: {0}")]
    Refused(String),
    #[error(transparent)]
    App(#[from] AppError),
}

impl From<sqlx::Error> for ImportError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<ImportError> for AppError {
    fn from(error: ImportError) -> Self {
        match error {
            ImportError::Refused(_) => AppError::Validation("archive"),
            ImportError::App(error) => error,
        }
    }
}

fn refused(reason: impl Into<String>) -> ImportError {
    ImportError::Refused(reason.into())
}

fn io<E>(_: E) -> ImportError {
    ImportError::App(AppError::Internal("the archive could not be read"))
}

/// A declared kind, by its module and name.
fn declared(module: &str, kind: &str) -> Option<(&'static ExportDecl, &'static KindDecl)> {
    DECLARATIONS
        .iter()
        .filter(|declaration| declaration.module == module)
        .find_map(|declaration| {
            declaration
                .kinds
                .iter()
                .find(|declared| declared.kind == kind)
                .map(|declared| (declaration, declared))
        })
}

/// What a member account was in the archive's workspace.
#[derive(Clone, Debug)]
struct Stub {
    id: Uuid,
    kind: String,
    display_name: Option<String>,
    emails: Vec<(String, bool)>,
}

/// One kind the archive carries.
#[derive(Clone, Debug)]
struct Present {
    table: &'static str,
    ids: Vec<Uuid>,
}

/// What [`inspect`] found in an archive that may be imported.
#[derive(Debug)]
pub struct Inspected {
    pub source: Uuid,
    pub name: String,
    pub slug: Option<String>,
    present: Vec<Present>,
    stubs: Vec<Stub>,
}

impl Inspected {
    /// How many lines of things it holds.
    pub fn lines(&self) -> usize {
        self.present.iter().map(|present| present.ids.len()).sum()
    }
}

fn allowed_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
        })
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && (name == "manifest.json"
            || name == "workspace/workspace.json"
            || name == "files/index.jsonl"
            || ["accounts/", "things/", "files/blobs/", "needs/"]
                .iter()
                .any(|prefix| name.starts_with(prefix)))
}

/// Every entry's name and size, before any is unpacked: refuses paths
/// that climb out, links, encrypted entries and size bombs.
fn check_entries(archive: &mut ZipArchive<File>) -> Result<(), ImportError> {
    if archive.len() > MAX_ENTRIES {
        return Err(refused("it holds too many entries"));
    }
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| refused("an entry is damaged"))?;
        let name = entry
            .name()
            .map_err(|_| refused("an entry's name is not text"))?
            .into_owned();
        if !allowed_name(&name) || entry.enclosed_name().is_none() {
            return Err(refused(format!("it holds an entry named {name:?}")));
        }
        if entry.is_symlink() || entry.is_dir() || entry.encrypted() {
            return Err(refused(format!("{name} is not a plain file")));
        }
        let size = entry.size();
        let compressed = entry.compressed_size().max(1);
        if size > MAX_ENTRY_BYTES || (size > 1024 * 1024 && size / compressed > MAX_RATIO) {
            return Err(refused(format!(
                "{name} unpacks to far more than it weighs"
            )));
        }
        total = total.saturating_add(size);
        if total > MAX_TOTAL_BYTES {
            return Err(refused("it unpacks to more than this server takes"));
        }
    }
    Ok(())
}

/// One entry's bytes, read no further than its declared size: an entry
/// that unpacks to more than it says is a bomb.
fn bounded<R: Read>(reader: R, declared: u64) -> impl Read {
    reader.take(declared.saturating_add(1))
}

fn entry_names(archive: &ZipArchive<File>) -> Result<Vec<String>, ImportError> {
    archive
        .file_names()
        .map(|name| {
            name.map(std::borrow::Cow::into_owned)
                .map_err(|_| refused("an entry's name is not text"))
        })
        .collect()
}

/// Reads one line of at most [`MAX_LINE_BYTES`]; `None` at the end.
fn next_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<Option<usize>, ImportError> {
    line.clear();
    let read = reader
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', line)
        .map_err(io)?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_LINE_BYTES {
        return Err(refused("a line is too long"));
    }
    Ok(Some(read))
}

fn read_json(archive: &mut ZipArchive<File>, name: &str) -> Result<Value, ImportError> {
    let entry = archive
        .by_name(name)
        .map_err(|_| refused(format!("it has no {name}")))?;
    let size = entry.size();
    let mut bytes = Vec::new();
    bounded(entry, size).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > size || bytes.len() > MAX_LINE_BYTES {
        return Err(refused(format!("{name} is larger than it says")));
    }
    serde_json::from_slice(&bytes).map_err(|_| refused(format!("{name} is not JSON")))
}

/// Reads the whole archive once, writing nothing: see the module notes.
pub fn inspect(path: &Path) -> Result<Inspected, ImportError> {
    let file = File::open(path).map_err(io)?;
    let mut archive = ZipArchive::new(file).map_err(|_| refused("it is not a zip file"))?;
    check_entries(&mut archive)?;
    let manifest = read_json(&mut archive, "manifest.json")?;
    if manifest["format"] != FORMAT {
        return Err(refused("it is not a workspace export"));
    }
    if manifest["formatVersion"] != FORMAT_VERSION {
        return Err(refused(format!(
            "it is format version {}; this server reads version {FORMAT_VERSION}",
            manifest["formatVersion"]
        )));
    }
    let header = read_json(&mut archive, "workspace/workspace.json")?;
    let source = header["id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .ok_or_else(|| refused("workspace/workspace.json has no id"))?;
    if manifest["workspace"]["id"] != header["id"] {
        return Err(refused(
            "the manifest and workspace.json name different workspaces",
        ));
    }
    let name = header["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| refused("workspace/workspace.json has no name"))?
        .to_owned();
    let slug = header["slug"].as_str().map(ToOwned::to_owned);
    let source_text = source.to_string();

    let names = entry_names(&archive)?;
    let mut present = Vec::new();
    let mut stubs = Vec::new();
    let mut seen = HashSet::new();
    let mut line = Vec::new();
    for name in &names {
        if let Some(rest) = name.strip_prefix("files/blobs/") {
            if rest.len() != 64 || !rest.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(refused(format!("{name} is not named by its digest")));
            }
            continue;
        }
        let accounts = name.starts_with("accounts/") && name.ends_with(".jsonl");
        let thing = name
            .strip_prefix("things/")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
            .and_then(|rest| rest.split_once('/'));
        if !accounts && thing.is_none() {
            continue;
        }
        let declaration = match thing {
            Some((module, kind)) => Some(
                declared(module, kind)
                    .ok_or_else(|| refused(format!("{name} is not a kind this server knows")))?,
            ),
            None => None,
        };
        let entry = archive
            .by_name(name)
            .map_err(|_| refused("an entry is damaged"))?;
        let size = entry.size();
        let mut reader = BufReader::new(bounded(entry, size));
        let mut read = 0_u64;
        let mut ids = Vec::new();
        while let Some(bytes) = next_line(&mut reader, &mut line)? {
            read += bytes as u64;
            if read > size {
                return Err(refused(format!("{name} is larger than it says")));
            }
            let parsed: Line = serde_json::from_slice(&line)
                .map_err(|_| refused(format!("a line of {name} is not a line of a thing")))?;
            let id = parsed.id.as_deref().and_then(|id| id.parse::<Uuid>().ok());
            if let Some((module, kind)) = declaration {
                if parsed.kind != kind.kind {
                    return Err(refused(format!(
                        "{name} holds a line of kind {}",
                        parsed.kind
                    )));
                }
                if parsed.v != module.version {
                    return Err(refused(format!(
                        "{} is version {}; this server reads version {}",
                        parsed.kind, parsed.v, module.version
                    )));
                }
                // Every line is this workspace's: one naming another is
                // refused before anything is written.
                let mut columns = vec!["workspace_id", "organization_id"];
                columns.extend(tables::tenant_column(kind.table));
                for column in columns {
                    match parsed.data.get(column) {
                        None | Some(Value::Null) => {}
                        Some(Value::String(named)) if *named == source_text => {}
                        Some(_) => {
                            return Err(refused(format!(
                                "a line of {name} names another workspace"
                            )));
                        }
                    }
                }
                if let Some(id) = id {
                    // One row per id in a table; the same id in two tables
                    // is one thing in two parts (a run and its details).
                    if !seen.insert((kind.table, id)) {
                        return Err(refused(format!("{id} appears twice")));
                    }
                    ids.push(id);
                }
            } else {
                let Some(id) = id else {
                    return Err(refused(format!("a line of {name} has no id")));
                };
                stubs.push(Stub {
                    id,
                    kind: parsed.data["kind"].as_str().unwrap_or("person").to_owned(),
                    display_name: parsed.data["displayName"].as_str().map(ToOwned::to_owned),
                    emails: parsed.data["emails"]
                        .as_array()
                        .map(|emails| {
                            emails
                                .iter()
                                .filter_map(|email| {
                                    Some((
                                        email["address"].as_str()?.to_owned(),
                                        email["verified"].as_bool().unwrap_or(false),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
        }
        if let Some((module, kind)) = declaration {
            let _ = module;
            present.push(Present {
                table: kind.table,
                ids,
            });
        }
    }
    Ok(Inspected {
        source,
        name,
        slug,
        present,
        stubs,
    })
}

/// What [`start_import`] needs.
#[derive(Clone, Debug)]
pub struct StartImport<'a> {
    /// The archive, on this server's disk.
    pub path: &'a Path,
    /// Who imports it: the new workspace's owner.
    pub owner: Uuid,
    /// The new workspace's address; left out, the archive's when free.
    pub slug: Option<String>,
}

/// What [`start_import`] made.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub workspace_id: Uuid,
    pub transfer: TransferView,
}

/// Checks an archive and makes its workspace, transfer and id map; the
/// `workspace.import` job does the rest. See the module notes.
pub async fn start_import(
    state: &AppState,
    input: StartImport<'_>,
) -> Result<Started, ImportError> {
    let path = input.path.to_owned();
    let inspected = tokio::task::spawn_blocking(move || inspect(&path))
        .await
        .map_err(|_| ImportError::App(AppError::Internal("the archive check stopped")))??;
    crate::tenancy::system("workspace.import", async {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        // The new workspace and its accounts are part of the import: news
        // of them is the transfer's, not each row's.
        sqlx::query("SELECT set_config('aw.importing', 'on', true)")
            .execute(&mut *connection)
            .await?;
        let copy = collides(&mut connection, &inspected).await?;
        let slug = match input.slug.clone() {
            Some(slug) => Some(crate::workspace::slug::check_new(&slug)?),
            None => match &inspected.slug {
                Some(slug) if slug_free(&mut connection, slug).await? => Some(slug.clone()),
                _ => None,
            },
        };
        let workspace = lifecycle::create(
            &mut connection,
            input.owner,
            NewWorkspace {
                name: inspected.name.clone(),
                slug,
                id: (!copy).then_some(inspected.source),
            },
        )
        .await?;
        lifecycle::begin_import(&mut connection, workspace).await?;
        let id = Uuid::now_v7();
        let view = sqlx::query_as::<_, TransferView>(&format!(
            "INSERT INTO workspace_transfers \
             (id, workspace_id, direction, state, requested_by_actor_id, id_mode, cursor) \
             VALUES ($1, $2, 'import', 'queued', $3, $4, '{{}}'::jsonb) RETURNING {VIEW_COLUMNS}"
        ))
        .bind(id)
        .bind(workspace)
        .bind(input.owner)
        .bind(if copy { "copy" } else { "preserve" })
        .fetch_one(&mut *connection)
        .await?;
        let mut map = vec![(inspected.source, workspace)];
        if copy {
            let mut fresh = HashSet::from([inspected.source]);
            for present in &inspected.present {
                for source in &present.ids {
                    if fresh.insert(*source) {
                        map.push((*source, Uuid::now_v7()));
                    }
                }
            }
        }
        map.extend(map_accounts(&mut connection, workspace, input.owner, &inspected.stubs).await?);
        record_map(&mut connection, id, workspace, &map).await?;
        connection.commit().await?;

        // The archive goes into the new workspace's store, so any process
        // can pick the import up.
        let store = {
            let mut connection = crate::db::scoped_tx(&state.pool).await?;
            FileStore::for_uploads(state, &mut connection, workspace).await?
        };
        let key = format!("{}/imports/{}.awx", workspace.simple(), id.simple());
        let file = tokio::fs::File::open(input.path).await.map_err(io)?;
        store
            .put_stream(&key, tokio_util::io::ReaderStream::new(file), u64::MAX)
            .await?;
        crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query(
                "UPDATE workspace_transfers SET store_id = $2, file_key = $3 WHERE id = $1",
            )
            .bind(id)
            .bind(store.id())
            .bind(&key)
            .execute(&mut *connection)
            .await?;
            events::emit(connection, transfer::changed(workspace, id, input.owner)).await?;
            Ok::<_, AppError>(())
        })
        .await?;
        Ok(Started {
            workspace_id: workspace,
            transfer: view,
        })
    })
    .await
}

/// The account signed in as `login`, for the server command.
pub async fn account_by_login(pool: &sqlx::PgPool, login: &str) -> Result<Uuid, AppError> {
    crate::tenancy::system("workspace.import", async {
        crate::db::scoped_one(pool, async |connection| {
            sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = $1")
                .bind(login.trim().to_ascii_lowercase())
                .fetch_optional(&mut *connection)
                .await
        })
        .await?
        .ok_or(AppError::NotFound("account"))
    })
    .await
}

/// How an import is going, for the account that started it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    #[serde(flatten)]
    pub transfer: TransferView,
    pub workspace_id: Uuid,
    /// The workspace's address, to open it once the import is done.
    pub slug: String,
}

/// `requester`'s import `transfer`, wherever it is.
pub async fn progress(
    pool: &sqlx::PgPool,
    transfer: Uuid,
    requester: Uuid,
) -> Result<Option<Progress>, AppError> {
    crate::tenancy::system("workspace.import", async {
        let found = crate::db::scoped_one(pool, async |connection| {
            sqlx::query_as::<_, (Uuid, String)>(
                "SELECT transfer.workspace_id, workspace.slug \
                 FROM workspace_transfers transfer \
                 JOIN organizations workspace ON workspace.id = transfer.workspace_id \
                 WHERE transfer.id = $1 AND transfer.requested_by_actor_id = $2 \
                   AND transfer.direction = 'import'",
            )
            .bind(transfer)
            .bind(requester)
            .fetch_optional(&mut *connection)
            .await
        })
        .await?;
        let Some((workspace_id, slug)) = found else {
            return Ok(None);
        };
        let mut connection = crate::db::scoped_tx(pool).await?;
        let view =
            transfer::transfer_of(&mut connection, workspace_id, requester, transfer).await?;
        Ok(view.map(|transfer| Progress {
            transfer,
            workspace_id,
            slug,
        }))
    })
    .await
}

/// Queues the job that fills an imported workspace.
pub async fn enqueue(state: &AppState, workspace: Uuid, transfer: Uuid) -> Result<(), AppError> {
    crate::db::scoped_one(&state.pool, async |connection| {
        crate::jobs::enqueue(
            &mut *connection,
            NewJob::new(IMPORT_KIND)
                .payload(ImportPayload { transfer })
                .workspace(workspace)
                .unique(format!("{IMPORT_KIND}:{transfer}")),
        )
        .await?;
        Ok::<_, AppError>(())
    })
    .await
}

async fn slug_free(connection: &mut PgConnection, slug: &str) -> Result<bool, ImportError> {
    if crate::workspace::slug::check_new(slug).is_err() {
        return Ok(false);
    }
    Ok(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organizations WHERE slug = $1)",
    )
    .bind(slug)
    .fetch_one(&mut *connection)
    .await?)
}

/// Whether any id the archive carries is already here (copy mode).
async fn collides(
    connection: &mut PgConnection,
    inspected: &Inspected,
) -> Result<bool, ImportError> {
    let workspace_here =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1)")
            .bind(inspected.source)
            .fetch_one(&mut *connection)
            .await?;
    if workspace_here {
        return Ok(true);
    }
    for present in &inspected.present {
        if present.ids.is_empty() {
            continue;
        }
        let table = identifier(present.table);
        for chunk in present.ids.chunks(10_000) {
            let found = sqlx::query_scalar::<_, bool>(&format!(
                "SELECT EXISTS (SELECT 1 FROM {table} WHERE id = ANY($1))"
            ))
            .bind(chunk)
            .fetch_one(&mut *connection)
            .await?;
            if found {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Matches or makes each member account (R19); returns the ones whose id
/// here differs from the archive's.
async fn map_accounts(
    connection: &mut PgConnection,
    workspace: Uuid,
    importer: Uuid,
    stubs: &[Stub],
) -> Result<Vec<(Uuid, Uuid)>, ImportError> {
    let mut map = Vec::new();
    for stub in stubs {
        let exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM actors WHERE id = $1)")
                .bind(stub.id)
                .fetch_one(&mut *connection)
                .await?;
        if stub.kind == "person" {
            let mut matched = None;
            for (address, verified) in &stub.emails {
                if !verified {
                    continue;
                }
                matched = sqlx::query_scalar::<_, Uuid>(
                    "SELECT person.actor_id FROM user_email_addresses email \
                     JOIN users person ON person.id = email.user_id \
                     WHERE lower(email.address) = lower($1) AND email.verified_at IS NOT NULL",
                )
                .bind(address)
                .fetch_optional(&mut *connection)
                .await?;
                if matched.is_some() {
                    break;
                }
            }
            let target = match matched {
                Some(found) => found,
                None if exists => stub.id,
                None => unclaimed_person(connection, workspace, importer, stub).await?,
            };
            if target != stub.id {
                map.push((stub.id, target));
            }
        } else if !exists {
            // An agent or app arrives with no credential: disabled until
            // its managers connect it again.
            let kind = if stub.kind == "app" {
                "integration"
            } else {
                "external_agent"
            };
            let account_kind = if stub.kind == "app" { "app" } else { "agent" };
            sqlx::query(
                "INSERT INTO actors (id, kind, account_kind, status, owner_workspace_id, \
                 created_by_actor_id) VALUES ($1, $2, $3, 'disabled', $4, $5)",
            )
            .bind(stub.id)
            .bind(kind)
            .bind(account_kind)
            .bind(workspace)
            .bind(importer)
            .execute(&mut *connection)
            .await?;
        }
    }
    Ok(map)
}

/// An account for someone who hasn't signed in here yet, claimed later.
async fn unclaimed_person(
    connection: &mut PgConnection,
    workspace: Uuid,
    importer: Uuid,
    stub: &Stub,
) -> Result<Uuid, ImportError> {
    let login = format!("imported-{}", &Uuid::now_v7().simple().to_string()[20..]);
    let display = stub
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(120).collect::<String>())
        .unwrap_or_else(|| "Imported person".to_owned());
    let person = create_person(
        connection,
        NewPerson {
            login_name: &login,
            display_name: &display,
            default_workspace: Some(workspace),
            created_by: Some(importer),
            claimed: false,
        },
    )
    .await?;
    if let Some((address, _)) = stub.emails.first() {
        sqlx::query(
            "INSERT INTO user_email_addresses (user_id, address) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(person.user_id)
        .bind(address)
        .execute(&mut *connection)
        .await?;
    }
    Ok(person.account_id)
}

async fn record_map(
    connection: &mut PgConnection,
    transfer: Uuid,
    workspace: Uuid,
    map: &[(Uuid, Uuid)],
) -> Result<(), ImportError> {
    for chunk in map.chunks(10_000) {
        let (sources, targets): (Vec<Uuid>, Vec<Uuid>) = chunk.iter().copied().unzip();
        sqlx::query(
            "INSERT INTO workspace_import_ids (transfer_id, workspace_id, source_id, target_id) \
             SELECT $1, $2, source, target FROM unnest($3::uuid[], $4::uuid[]) AS m(source, target) \
             ON CONFLICT (transfer_id, source_id) DO UPDATE SET target_id = EXCLUDED.target_id",
        )
        .bind(transfer)
        .bind(workspace)
        .bind(&sources)
        .bind(&targets)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct ImportPayload {
    transfer: Uuid,
}

/// Where an import got to, saved with each batch.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
struct Cursor {
    /// `rows`, then `fixups`; absent before the first batch.
    #[serde(default)]
    phase: String,
    /// Which step of the plan.
    #[serde(default)]
    step: usize,
    /// Byte offset into that step's lines.
    #[serde(default)]
    offset: u64,
    /// Lines written so far, by table.
    #[serde(default)]
    written: BTreeMap<String, u64>,
    /// Lines left out, by table: a row whose server-wide unique key
    /// (a custom harness's kind) another workspace here already holds.
    #[serde(default)]
    skipped: BTreeMap<String, u64>,
}

/// Limits for one run, for tests that stop an import part way.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunLimits {
    pub max_batches: Option<usize>,
}

/// Fills an imported workspace, from where its cursor says. Returns whether
/// it finished.
pub async fn run_import(
    state: &AppState,
    transfer: Uuid,
    limits: RunLimits,
) -> Result<bool, AppError> {
    crate::tenancy::system("workspace.import", run(state, transfer, limits)).await
}

struct Claimed {
    workspace: Uuid,
    requester: Uuid,
    store_id: String,
    key: String,
    cursor: Cursor,
}

async fn claim(state: &AppState, transfer: Uuid) -> Result<Option<Claimed>, AppError> {
    let row = crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query_as::<_, (Uuid, Uuid, Option<String>, Option<String>, Value)>(
            "UPDATE workspace_transfers SET state = 'running', \
                 lease_until = now() + make_interval(secs => $2) \
             WHERE id = $1 AND direction = 'import' AND state IN ('queued', 'running') \
               AND (lease_until IS NULL OR lease_until < now()) \
             RETURNING workspace_id, requested_by_actor_id, store_id, file_key, cursor",
        )
        .bind(transfer)
        .bind(f64::from(LEASE_SECONDS))
        .fetch_optional(&mut *connection)
        .await
    })
    .await?;
    let Some((workspace, requester, store_id, key, cursor)) = row else {
        return Ok(None);
    };
    let (Some(store_id), Some(key)) = (store_id, key) else {
        return Err(AppError::Internal("the import's archive was never stored"));
    };
    Ok(Some(Claimed {
        workspace,
        requester,
        store_id,
        key,
        cursor: serde_json::from_value(cursor).unwrap_or_default(),
    }))
}

fn work_folder(state: &AppState, transfer: Uuid) -> PathBuf {
    state
        .config
        .data_root
        .join("tmp")
        .join(format!("import-{}", transfer.simple()))
}

async fn run(state: &AppState, transfer: Uuid, limits: RunLimits) -> Result<bool, AppError> {
    let Some(claimed) = claim(state, transfer).await? else {
        return Ok(false);
    };
    let folder = work_folder(state, transfer);
    let outcome = fill(state, transfer, &claimed, &folder, limits).await;
    match outcome {
        Ok(true) => {
            let _ = tokio::fs::remove_dir_all(&folder).await;
            let store = FileStore::resolve(state, &claimed.store_id).await?;
            store.delete(&claimed.key).await?;
            Ok(true)
        }
        Ok(false) => {
            release(state, transfer).await?;
            Ok(false)
        }
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&folder).await;
            let message = match &error {
                ImportError::Refused(reason) => format!("The archive was refused: {reason}."),
                ImportError::App(error) => {
                    format!("The import stopped ({}).", error.log_category())
                }
            };
            crate::db::scoped_one(&state.pool, async |connection| {
                sqlx::query(
                    "UPDATE workspace_transfers SET state = 'failed', error = $2, \
                     finished_at = now(), lease_until = NULL WHERE id = $1",
                )
                .bind(transfer)
                .bind(message.chars().take(1000).collect::<String>())
                .execute(&mut *connection)
                .await?;
                events::emit(
                    connection,
                    transfer::changed(claimed.workspace, transfer, claimed.requester),
                )
                .await?;
                Ok::<_, AppError>(())
            })
            .await?;
            Err(error.into())
        }
    }
}

async fn release(state: &AppState, transfer: Uuid) -> Result<(), AppError> {
    crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query("UPDATE workspace_transfers SET lease_until = NULL WHERE id = $1")
            .bind(transfer)
            .execute(&mut *connection)
            .await?;
        Ok::<_, AppError>(())
    })
    .await
}

/// The archive on this process's disk, unpacked into one file per kind.
async fn unpack(
    state: &AppState,
    claimed: &Claimed,
    folder: &Path,
) -> Result<PathBuf, ImportError> {
    let archive = folder.join("archive.awx");
    let ready = folder.join("ready");
    if tokio::fs::try_exists(&ready).await.unwrap_or(false) {
        return Ok(archive);
    }
    tokio::fs::create_dir_all(folder).await.map_err(io)?;
    let store = FileStore::resolve(state, &claimed.store_id).await?;
    let stored = store.get(&claimed.key).await?;
    let mut file = tokio::fs::File::create(&archive).await.map_err(io)?;
    let mut stream = stored.body.into_data_stream();
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk.map_err(io)?).await.map_err(io)?;
    }
    file.flush().await.map_err(io)?;
    drop(file);
    let (source, target) = (archive.clone(), folder.to_owned());
    tokio::task::spawn_blocking(move || unpack_kinds(&source, &target))
        .await
        .map_err(|_| ImportError::App(AppError::Internal("unpacking stopped")))??;
    tokio::fs::write(&ready, b"").await.map_err(io)?;
    Ok(archive)
}

fn kind_file(folder: &Path, module: &str, kind: &str) -> PathBuf {
    folder.join(format!("{module}.{kind}.jsonl"))
}

fn unpack_kinds(archive: &Path, folder: &Path) -> Result<(), ImportError> {
    let mut zip = ZipArchive::new(File::open(archive).map_err(io)?)
        .map_err(|_| refused("it is not a zip file"))?;
    check_entries(&mut zip)?;
    let names = entry_names(&zip)?;
    for name in names {
        let Some((module, kind)) = name
            .strip_prefix("things/")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let entry = zip
            .by_name(&name)
            .map_err(|_| refused("an entry is damaged"))?;
        let size = entry.size();
        let mut out = File::create(kind_file(folder, module, kind)).map_err(io)?;
        let copied = std::io::copy(&mut bounded(entry, size), &mut out).map_err(io)?;
        if copied > size {
            return Err(refused(format!("{name} is larger than it says")));
        }
    }
    Ok(())
}

/// Copies one stored file's bytes out of the archive.
fn unpack_blob(archive: &Path, sha256: &str, into: &Path) -> Result<bool, ImportError> {
    let mut zip = ZipArchive::new(File::open(archive).map_err(io)?)
        .map_err(|_| refused("it is not a zip file"))?;
    let Ok(entry) = zip.by_name(&format!("files/blobs/{sha256}")) else {
        return Ok(false);
    };
    let size = entry.size();
    let mut out = File::create(into).map_err(io)?;
    let copied = std::io::copy(&mut bounded(entry, size), &mut out).map_err(io)?;
    if copied > size {
        return Err(refused("a file is larger than it says"));
    }
    Ok(true)
}

/// What the database says about one table.
#[derive(Debug)]
struct TableInfo {
    columns: HashSet<String>,
    has_id: bool,
    tenant: &'static str,
    keys: Vec<Vec<String>>,
    fks: Vec<Fk>,
    /// A unique constraint checked at commit, which `ON CONFLICT` can't
    /// arbitrate: only the id may then.
    deferred_unique: bool,
}

/// One foreign key: the table it points at, its columns, the nullable ones
/// among them, and the columns they point at.
#[derive(Debug)]
struct Fk {
    target: String,
    columns: Vec<String>,
    nullable: Vec<String>,
    referenced: Vec<String>,
}

async fn table_info(
    connection: &mut PgConnection,
    table: &'static str,
) -> Result<TableInfo, ImportError> {
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT attname::text FROM pg_attribute \
         WHERE attrelid = $1::regclass AND attnum > 0 AND NOT attisdropped AND attgenerated = ''",
    )
    .bind(table)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let keys = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT array(SELECT a.attname::text FROM unnest(i.indkey::int2[]) k \
                      JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k) \
         FROM pg_index i \
         WHERE i.indrelid = $1::regclass AND i.indisunique AND NOT i.indisprimary \
           AND i.indexprs IS NULL",
    )
    .bind(table)
    .fetch_all(&mut *connection)
    .await?;
    let fks = sqlx::query_as::<_, (String, Vec<String>, Vec<String>, Vec<String>)>(
        "SELECT c.confrelid::regclass::text, \
                array(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(n, o) \
                      JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.n \
                      ORDER BY k.o), \
                array(SELECT a.attname::text FROM unnest(c.conkey) k \
                      JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k \
                      WHERE NOT a.attnotnull), \
                array(SELECT a.attname::text FROM unnest(c.confkey) WITH ORDINALITY k(n, o) \
                      JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.n \
                      ORDER BY k.o) \
         FROM pg_constraint c WHERE c.contype = 'f' AND c.conrelid = $1::regclass",
    )
    .bind(table)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(target, columns, nullable, referenced)| Fk {
        target,
        columns,
        nullable,
        referenced,
    })
    .collect();
    let deferred_unique = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = $1::regclass \
                        AND contype IN ('u', 'x') AND condeferrable)",
    )
    .bind(table)
    .fetch_one(&mut *connection)
    .await?;
    Ok(TableInfo {
        deferred_unique,
        has_id: columns.contains("id"),
        columns,
        tenant: tables::tenant_column(table)
            .ok_or_else(|| refused(format!("{table} has no workspace column")))?,
        keys,
        fks,
    })
}

/// The registry of things, which triggers fill as things go in.
const DERIVED: &str = "things";

/// Orders no foreign key shows: (table, the table it goes after). A Tasks
/// record shares its task's id and is checked against it at commit.
const AFTER: &[(&str, &str)] = &[("collection_records", "work_items")];

/// Rows that go in together, in one transaction: (table, its companion).
/// A Tasks record goes in first, then its task, which checks for it.
const COMPANIONS: &[(&str, &str)] = &[("work_items", "collection_records")];

/// One step of an import: one kind, and the columns it fills in last.
#[derive(Debug)]
struct Step {
    module: &'static str,
    kind: &'static str,
    table: &'static str,
    deferred: Vec<String>,
}

/// The kinds the archive holds, in the order their foreign keys need.
/// Where tables point at each other, the nullable side of each such
/// reference is deferred to the fixups.
fn plan(
    present: &[(&'static str, &'static str, &'static str)],
    info: &HashMap<&str, TableInfo>,
) -> Result<Vec<Step>, ImportError> {
    let tables = present
        .iter()
        .map(|(_, _, table)| *table)
        .collect::<BTreeSet<_>>();
    // Edges: table -> (target, nullable columns of that reference).
    let mut edges: BTreeMap<&str, Vec<(&str, Vec<String>)>> = BTreeMap::new();
    for table in &tables {
        for fk in &info[table].fks {
            if let Some(target) = tables.get(fk.target.as_str())
                && target != table
            {
                edges
                    .entry(table)
                    .or_default()
                    .push((target, fk.nullable.clone()));
            }
        }
    }
    // Tables that reach each other (strongly connected components).
    let reaches = |from: &str| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(next) = stack.pop() {
            for (target, _) in edges.get(next).map(Vec::as_slice).unwrap_or_default() {
                if seen.insert(*target) {
                    stack.push(target);
                }
            }
        }
        seen
    };
    let reach = tables
        .iter()
        .map(|table| (*table, reaches(table)))
        .collect::<BTreeMap<_, _>>();
    let mut deferred: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut hard: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (table, after) in AFTER {
        if let (Some(table), Some(after)) = (tables.get(table), tables.get(after)) {
            hard.entry(table).or_default().insert(after);
        }
    }
    for (table, targets) in &edges {
        for (target, nullable) in targets {
            let cycle = reach[target].contains(table);
            if cycle && !nullable.is_empty() {
                deferred
                    .entry(table)
                    .or_default()
                    .extend(nullable.iter().cloned());
            } else {
                hard.entry(table).or_default().insert(target);
            }
        }
    }
    // `things` is filled by triggers as each kind of thing goes in, so a
    // table that points into it goes after every other.
    let last = tables
        .iter()
        .filter(|table| info[*table].fks.iter().any(|fk| fk.target == DERIVED))
        .copied()
        .collect::<BTreeSet<_>>();
    let mut order = Vec::new();
    let mut placed = BTreeSet::new();
    while placed.len() < tables.len() {
        let others_placed = tables
            .iter()
            .all(|table| last.contains(table) || placed.contains(table));
        let ready = tables
            .iter()
            .filter(|table| !placed.contains(*table))
            .filter(|table| others_placed || !last.contains(*table))
            .filter(|table| {
                hard.get(*table)
                    .is_none_or(|targets| targets.iter().all(|target| placed.contains(target)))
            })
            .copied()
            .collect::<Vec<_>>();
        if ready.is_empty() {
            let left = tables
                .iter()
                .filter(|table| !placed.contains(*table))
                .copied()
                .collect::<Vec<_>>();
            return Err(ImportError::App(AppError::Internal(Box::leak(
                format!("these tables need each other: {left:?}").into_boxed_str(),
            ))));
        }
        for table in ready {
            placed.insert(table);
            order.push(table);
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|table| {
            present
                .iter()
                .find(|(_, _, present)| *present == table)
                .map(|(module, kind, table)| Step {
                    module,
                    kind,
                    table,
                    deferred: deferred
                        .get(table)
                        .map(|columns| columns.iter().cloned().collect())
                        .unwrap_or_default(),
                })
        })
        .collect())
}

/// Reads up to [`BATCH`] lines from `offset`; returns them and the offset
/// after them.
fn read_batch(path: &Path, offset: u64) -> Result<(Vec<Line>, u64), ImportError> {
    let mut file = File::open(path).map_err(io)?;
    file.seek(SeekFrom::Start(offset)).map_err(io)?;
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut at = offset;
    let mut buffer = Vec::new();
    while lines.len() < BATCH {
        let Some(read) = next_line(&mut reader, &mut buffer)? else {
            break;
        };
        at += read as u64;
        lines.push(serde_json::from_slice(&buffer).map_err(|_| refused("a line changed"))?);
    }
    Ok((lines, at))
}

/// Every id written as text in `value`.
fn ids_in(value: &Value, into: &mut HashSet<Uuid>) {
    match value {
        Value::String(text) => into.extend(uuids(text).map(|(_, id)| id)),
        Value::Array(items) => items.iter().for_each(|item| ids_in(item, into)),
        Value::Object(fields) => fields.values().for_each(|item| ids_in(item, into)),
        _ => {}
    }
}

/// The ids in `text` with where each starts: 36 characters, hyphens at
/// 8, 13, 18 and 23, hex digits elsewhere.
fn uuids(text: &str) -> impl Iterator<Item = (usize, Uuid)> + '_ {
    let bytes = text.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        while at + 36 <= bytes.len() {
            let window = &bytes[at..at + 36];
            let shaped = window.iter().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            });
            let start = at;
            if shaped && let Ok(id) = Uuid::parse_str(&text[start..start + 36]) {
                at += 36;
                return Some((start, id));
            }
            at += 1;
        }
        None
    })
}

/// `value` with every mapped id rewritten, in fields and inside text.
fn rewrite(value: &mut Value, map: &HashMap<Uuid, Uuid>) {
    match value {
        Value::String(text) => {
            let found = uuids(text)
                .filter_map(|(at, id)| map.get(&id).map(|target| (at, *target)))
                .collect::<Vec<_>>();
            if found.is_empty() {
                return;
            }
            let mut out = String::with_capacity(text.len());
            let mut last = 0;
            for (at, target) in found {
                out.push_str(&text[last..at]);
                // Keep the case the text used.
                let written = target.to_string();
                if text[at..at + 36].chars().any(|c| c.is_ascii_uppercase()) {
                    out.push_str(&written.to_ascii_uppercase());
                } else {
                    out.push_str(&written);
                }
                last = at + 36;
            }
            out.push_str(&text[last..]);
            *text = out;
        }
        Value::Array(items) => items.iter_mut().for_each(|item| rewrite(item, map)),
        Value::Object(fields) => fields.values_mut().for_each(|item| rewrite(item, map)),
        _ => {}
    }
}

async fn load_map(
    connection: &mut PgConnection,
    transfer: Uuid,
    lines: &[Line],
) -> Result<HashMap<Uuid, Uuid>, ImportError> {
    let mut found = HashSet::new();
    for line in lines {
        ids_in(&line.data, &mut found);
    }
    if found.is_empty() {
        return Ok(HashMap::new());
    }
    let found = found.into_iter().collect::<Vec<_>>();
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT source_id, target_id FROM workspace_import_ids \
         WHERE transfer_id = $1 AND source_id = ANY($2)",
    )
    .bind(transfer)
    .bind(&found)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect())
}

/// Accounts a batch names that this server has never seen (someone who
/// left before the export): kept as disabled accounts under their own id,
/// so what they did keeps its author.
async fn keep_authors(
    connection: &mut PgConnection,
    info: &TableInfo,
    rows: &[Value],
) -> Result<(), ImportError> {
    let mut named = HashSet::new();
    for fk in &info.fks {
        if fk.target != "actors" || fk.columns.len() != 1 {
            continue;
        }
        for row in rows {
            if let Some(id) = row
                .get(&fk.columns[0])
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
            {
                named.insert(id);
            }
        }
    }
    if named.is_empty() {
        return Ok(());
    }
    let named = named.into_iter().collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO actors (id, kind, account_kind, status) \
         SELECT id, 'human', 'person', 'disabled' FROM unnest($1::uuid[]) AS named(id) \
         WHERE NOT EXISTS (SELECT 1 FROM actors WHERE actors.id = named.id)",
    )
    .bind(&named)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// References to things the archive doesn't carry and this server doesn't
/// have (a code host install, a connection to set up again): a nullable
/// reference is emptied; a row that can't be without it is left out.
/// Returns the rows to write and how many were left out.
async fn drop_dangling(
    connection: &mut PgConnection,
    table: &'static str,
    info: &TableInfo,
    carried: &BTreeSet<&str>,
    rows: Vec<Value>,
    lines: Vec<Line>,
) -> Result<(Vec<Value>, Vec<Line>, u64), ImportError> {
    let table = identifier(table);
    let mut rows = rows;
    let mut leave = BTreeSet::new();
    for fk in &info.fks {
        if carried.contains(fk.target.as_str()) || fk.target == DERIVED || fk.target == table {
            continue;
        }
        let target = plain(&fk.target)?;
        let present = fk
            .columns
            .iter()
            .map(|column| plain(column).map(|column| format!("r.{column} IS NOT NULL")))
            .collect::<Result<Vec<_>, _>>()?
            .join(" AND ");
        let matches = fk
            .columns
            .iter()
            .zip(&fk.referenced)
            .map(|(column, referenced)| {
                Ok::<_, ImportError>(format!("t.{} = r.{}", plain(referenced)?, plain(column)?))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" AND ");
        let missing = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT e.ordinality - 1 FROM jsonb_array_elements($1) WITH ORDINALITY e(row, ordinality), \
                 LATERAL jsonb_populate_record(NULL::{table}, e.row) r \
             WHERE {present} AND NOT EXISTS (SELECT 1 FROM {target} t WHERE {matches})"
        ))
        .bind(Value::Array(rows.clone()))
        .fetch_all(&mut *connection)
        .await?;
        for index in missing {
            let index = usize::try_from(index).unwrap_or(usize::MAX);
            let Some(Value::Object(fields)) = rows.get_mut(index) else {
                continue;
            };
            if fk.nullable.is_empty() {
                leave.insert(index);
            } else {
                for column in &fk.nullable {
                    fields.insert(column.clone(), Value::Null);
                }
            }
        }
    }
    let left = leave.len() as u64;
    let (rows, lines) = rows
        .into_iter()
        .zip(lines)
        .enumerate()
        .filter(|(index, _)| !leave.contains(index))
        .map(|(_, pair)| pair)
        .unzip();
    Ok((rows, lines, left))
}

async fn fill(
    state: &AppState,
    transfer: Uuid,
    claimed: &Claimed,
    folder: &Path,
    limits: RunLimits,
) -> Result<bool, ImportError> {
    let archive = unpack(state, claimed, folder).await?;
    let workspace = claimed.workspace;
    let present = DECLARATIONS
        .iter()
        .flat_map(|declaration| {
            declaration
                .kinds
                .iter()
                .map(move |kind| (declaration.module, kind.kind, kind.table))
        })
        .filter(|(module, kind, _)| kind_file(folder, module, kind).exists())
        .collect::<Vec<_>>();
    let mut info = HashMap::new();
    {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        for (_, _, table) in &present {
            info.insert(*table, table_info(&mut connection, table).await?);
        }
        connection.commit().await?;
    }
    let steps = plan(&present, &info)?;
    let carried = present
        .iter()
        .map(|(_, _, table)| *table)
        .collect::<BTreeSet<_>>();
    let store = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        FileStore::for_uploads(state, &mut connection, workspace).await?
    };
    let mut cursor = claimed.cursor.clone();
    if cursor.phase.is_empty() {
        cursor.phase = "rows".to_owned();
    }
    let mut batches = 0;
    let mut companion_index: Option<HashMap<Uuid, u64>> = None;
    loop {
        if limits.max_batches.is_some_and(|most| batches >= most) {
            return Ok(false);
        }
        if cursor.phase == "rows" && cursor.step >= steps.len() {
            cursor = Cursor {
                phase: "fixups".to_owned(),
                step: 0,
                offset: 0,
                ..cursor
            };
        }
        if cursor.phase == "fixups" && cursor.step >= steps.len() {
            break;
        }
        let step = &steps[cursor.step];
        if cursor.phase == "fixups" && step.deferred.is_empty() {
            cursor.step += 1;
            cursor.offset = 0;
            continue;
        }
        let path = kind_file(folder, step.module, step.kind);
        let (lines, next) = read_batch(&path, cursor.offset)?;
        if lines.is_empty() {
            cursor.step += 1;
            cursor.offset = 0;
            continue;
        }
        // Files' bytes go into the new store before their rows, outside any
        // transaction; a key already there (a resumed batch) is kept.
        let mut blobs_missing = HashSet::new();
        if cursor.phase == "rows" && step.table == "files" {
            let mut connection = crate::db::scoped_tx(&state.pool).await?;
            let map = load_map(&mut connection, transfer, &lines).await?;
            connection.commit().await?;
            for line in &lines {
                let source = line.data["id"]
                    .as_str()
                    .and_then(|id| id.parse::<Uuid>().ok());
                let sha = line.data["sha256"].as_str().unwrap_or_default().to_owned();
                let Some(source) = source else { continue };
                let id = map.get(&source).copied().unwrap_or(source);
                let key = format!("{}/{}", workspace.simple(), id.simple());
                if store.head(&key).await?.is_some() {
                    continue;
                }
                let staged = folder.join(format!("blob-{}", id.simple()));
                let (from, into, digest) = (archive.clone(), staged.clone(), sha.clone());
                let found = tokio::task::spawn_blocking(move || {
                    if digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()) {
                        unpack_blob(&from, &digest, &into)
                    } else {
                        Ok(false)
                    }
                })
                .await
                .map_err(|_| ImportError::App(AppError::Internal("unpacking stopped")))??;
                if !found {
                    blobs_missing.insert(source);
                    continue;
                }
                let file = tokio::fs::File::open(&staged).await.map_err(io)?;
                store
                    .put_stream(&key, tokio_util::io::ReaderStream::new(file), u64::MAX)
                    .await?;
                let _ = tokio::fs::remove_file(&staged).await;
            }
        }

        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        sqlx::query("SELECT set_config('aw.importing', 'on', true)")
            .execute(&mut *connection)
            .await?;
        let context = Batch {
            transfer,
            workspace,
            store_id: store.id(),
            info: &info,
            carried: &carried,
        };
        if cursor.phase == "rows" {
            // A row that must go in with its companion's (a task and its
            // Tasks record) brings it along, in the same transaction.
            if let Some((_, companion)) = COMPANIONS.iter().find(|(table, _)| *table == step.table)
                && let Some(partner) = steps.iter().find(|other| other.table == *companion)
            {
                let path = kind_file(folder, partner.module, partner.kind);
                if companion_index.is_none() {
                    let indexed = path.clone();
                    companion_index = Some(
                        tokio::task::spawn_blocking(move || index_lines(&indexed))
                            .await
                            .map_err(|_| {
                                ImportError::App(AppError::Internal("indexing stopped"))
                            })??,
                    );
                }
                let offsets = lines
                    .iter()
                    .filter_map(|line| line.id.as_deref()?.parse::<Uuid>().ok())
                    .filter_map(|id| companion_index.as_ref()?.get(&id).copied())
                    .collect::<Vec<_>>();
                if !offsets.is_empty() {
                    let partners = read_at(&path, &offsets)?;
                    let (written, skipped) = write_batch(
                        &mut connection,
                        &context,
                        partner,
                        partners,
                        &HashSet::new(),
                    )
                    .await?;
                    count(&mut cursor, partner.table, written, skipped);
                }
            }
            let (written, skipped) =
                write_batch(&mut connection, &context, step, lines, &blobs_missing).await?;
            count(&mut cursor, step.table, written, skipped);
        } else {
            let rows = prepare(&mut connection, &context, step, &lines, &blobs_missing).await?;
            fix_up(&mut connection, step, rows).await?;
        }
        cursor.offset = next;
        sqlx::query(
            "UPDATE workspace_transfers SET cursor = $2, \
                 lease_until = now() + make_interval(secs => $3) \
             WHERE id = $1",
        )
        .bind(transfer)
        .bind(serde_json::to_value(&cursor).unwrap_or_default())
        .bind(f64::from(LEASE_SECONDS))
        .execute(&mut *connection)
        .await?;
        connection.commit().await?;
        batches += 1;
    }

    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    lifecycle::finish_import(&mut connection, workspace).await?;
    sqlx::query(
        "UPDATE workspace_transfers SET state = 'succeeded', counts = $2, finished_at = now(), \
             lease_until = NULL, cursor = $3 \
         WHERE id = $1",
    )
    .bind(transfer)
    .bind(json!({ "written": cursor.written, "skipped": cursor.skipped }))
    .bind(serde_json::to_value(&cursor).unwrap_or_default())
    .execute(&mut *connection)
    .await?;
    events::emit(
        &mut connection,
        transfer::changed(workspace, transfer, claimed.requester),
    )
    .await?;
    connection.commit().await?;
    Ok(true)
}

/// What every batch of one import shares.
struct Batch<'a> {
    transfer: Uuid,
    workspace: Uuid,
    store_id: &'a str,
    info: &'a HashMap<&'static str, TableInfo>,
    carried: &'a BTreeSet<&'static str>,
}

fn count(cursor: &mut Cursor, table: &str, written: u64, skipped: u64) {
    *cursor.written.entry(table.to_owned()).or_default() += written;
    if skipped > 0 {
        *cursor.skipped.entry(table.to_owned()).or_default() += skipped;
    }
}

/// The batch's rows as this server writes them: ids rewritten through the
/// map, columns this server lacks left out, and a file's place in the new
/// store.
async fn prepare(
    connection: &mut PgConnection,
    context: &Batch<'_>,
    step: &Step,
    lines: &[Line],
    blobs_missing: &HashSet<Uuid>,
) -> Result<Vec<Value>, ImportError> {
    let info = &context.info[step.table];
    let map = load_map(connection, context.transfer, lines).await?;
    let mut rows = Vec::with_capacity(lines.len());
    for line in lines {
        let mut data = line.data.clone();
        rewrite(&mut data, &map);
        if let Value::Object(fields) = &mut data {
            fields.retain(|column, _| info.columns.contains(column));
            if step.table == "files" {
                let id = fields
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .replace('-', "");
                fields.insert(
                    "storage_key".to_owned(),
                    json!(format!("{}/{id}", context.workspace.simple())),
                );
                fields.insert("store_id".to_owned(), json!(context.store_id));
                fields.insert("thumbnail_key".to_owned(), Value::Null);
                fields.insert("thumbnail_bytes".to_owned(), Value::Null);
                fields.insert("moved_from_store_id".to_owned(), Value::Null);
                let source = line.data["id"].as_str().and_then(|id| id.parse().ok());
                if source.is_some_and(|source| blobs_missing.contains(&source))
                    && fields.get("blob_deleted_at").is_none_or(Value::is_null)
                {
                    fields.insert("blob_deleted_at".to_owned(), json!(chrono::Utc::now()));
                }
            }
        }
        rows.push(data);
    }
    Ok(rows)
}

/// Prepares and writes one batch of one kind. Returns how many rows went
/// in and how many were left out.
async fn write_batch(
    connection: &mut PgConnection,
    context: &Batch<'_>,
    step: &Step,
    lines: Vec<Line>,
    blobs_missing: &HashSet<Uuid>,
) -> Result<(u64, u64), ImportError> {
    let info = &context.info[step.table];
    let rows = prepare(connection, context, step, &lines, blobs_missing).await?;
    keep_authors(connection, info, &rows).await?;
    let (rows, kept, dangling) =
        drop_dangling(connection, step.table, info, context.carried, rows, lines).await?;
    let (written, skipped) = insert_rows(
        connection,
        context.transfer,
        context.workspace,
        step,
        info,
        &kept,
        rows,
    )
    .await?;
    Ok((written, skipped + dangling))
}

/// Where each line of a kind's file starts, by its id.
fn index_lines(path: &Path) -> Result<HashMap<Uuid, u64>, ImportError> {
    let mut reader = BufReader::new(File::open(path).map_err(io)?);
    let mut index = HashMap::new();
    let mut at = 0_u64;
    let mut buffer = Vec::new();
    while let Some(read) = next_line(&mut reader, &mut buffer)? {
        let line: Line = serde_json::from_slice(&buffer).map_err(|_| refused("a line changed"))?;
        if let Some(id) = line.id.as_deref().and_then(|id| id.parse::<Uuid>().ok()) {
            index.insert(id, at);
        }
        at += read as u64;
    }
    Ok(index)
}

/// The lines starting at `offsets`.
fn read_at(path: &Path, offsets: &[u64]) -> Result<Vec<Line>, ImportError> {
    let mut file = File::open(path).map_err(io)?;
    let mut lines = Vec::with_capacity(offsets.len());
    let mut buffer = Vec::new();
    for offset in offsets {
        file.seek(SeekFrom::Start(*offset)).map_err(io)?;
        let mut reader = BufReader::new(&mut file);
        if next_line(&mut reader, &mut buffer)?.is_some() {
            lines.push(serde_json::from_slice(&buffer).map_err(|_| refused("a line changed"))?);
        }
    }
    Ok(lines)
}

/// Inserts one batch; a row that clashes with one the new workspace made
/// for itself (a unique key it already holds) is matched to it instead,
/// and one whose server-wide unique key another workspace holds is left
/// out and counted. Returns how many rows went in and how many were left
/// out.
async fn insert_rows(
    connection: &mut PgConnection,
    transfer: Uuid,
    workspace: Uuid,
    step: &Step,
    info: &TableInfo,
    lines: &[Line],
    mut rows: Vec<Value>,
) -> Result<(u64, u64), ImportError> {
    let table = identifier(step.table);
    let full = rows.clone();
    for row in &mut rows {
        if let Value::Object(fields) = row {
            for column in &step.deferred {
                fields.remove(column);
            }
        }
    }
    let mut columns = BTreeSet::new();
    for row in &rows {
        if let Value::Object(fields) = row {
            columns.extend(fields.keys().cloned());
        }
    }
    let list = columns
        .iter()
        .map(|column| plain(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    if list.is_empty() {
        return Ok((0, 0));
    }
    let returning = if info.has_id { " RETURNING id" } else { "" };
    let conflict = match (info.deferred_unique, info.has_id) {
        (false, _) => " ON CONFLICT DO NOTHING",
        (true, true) => " ON CONFLICT (id) DO NOTHING",
        (true, false) => "",
    };
    let sql = format!(
        "INSERT INTO {table} ({list}) SELECT {list} FROM jsonb_populate_recordset(NULL::{table}, $1)\
         {conflict}{returning}"
    );
    if !info.has_id {
        let done = sqlx::query(&sql)
            .bind(Value::Array(rows))
            .execute(&mut *connection)
            .await?;
        return Ok((done.rows_affected(), 0));
    }
    let inserted = sqlx::query_scalar::<_, Uuid>(&sql)
        .bind(Value::Array(rows.clone()))
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();
    let tenant = identifier(info.tenant);
    let mut matched = Vec::new();
    let mut skipped = 0;
    for ((line, row), whole) in lines.iter().zip(&rows).zip(&full) {
        let Some(id) = row["id"].as_str().and_then(|id| id.parse::<Uuid>().ok()) else {
            continue;
        };
        if inserted.contains(&id) {
            continue;
        }
        // Already here, as this workspace's: a batch done before.
        let owner = sqlx::query_scalar::<_, Option<Uuid>>(&format!(
            "SELECT {tenant} FROM {table} WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(owner) = owner {
            if owner == Some(workspace) {
                continue;
            }
            return Err(refused(format!(
                "{} {id} belongs to another workspace",
                step.kind
            )));
        }
        let mut found = None;
        for key in &info.keys {
            if key
                .iter()
                .any(|column| whole.get(column).is_none_or(Value::is_null))
            {
                continue;
            }
            let condition = key
                .iter()
                .map(|column| plain(column).map(|column| format!("t.{column} = r.{column}")))
                .collect::<Result<Vec<_>, _>>()?
                .join(" AND ");
            found = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(&format!(
                "SELECT t.id, t.{tenant} FROM {table} t, \
                 jsonb_populate_record(NULL::{table}, $1) r WHERE {condition} LIMIT 1"
            ))
            .bind(whole)
            .fetch_optional(&mut *connection)
            .await?;
            if found.is_some() {
                break;
            }
        }
        match found {
            Some((existing, Some(owner))) if owner == workspace => {
                if let Some(source) = line.id.as_deref().and_then(|id| id.parse::<Uuid>().ok()) {
                    matched.push((source, existing));
                }
            }
            Some(_) => skipped += 1,
            None => {
                return Err(ImportError::App(AppError::Internal(
                    "an imported row could not be written",
                )));
            }
        }
    }
    record_map(connection, transfer, workspace, &matched).await?;
    Ok((inserted.len() as u64, skipped))
}

/// Fills in the columns [`plan`] deferred.
async fn fix_up(
    connection: &mut PgConnection,
    step: &Step,
    rows: Vec<Value>,
) -> Result<(), ImportError> {
    let table = identifier(step.table);
    let set = step
        .deferred
        .iter()
        .map(|column| plain(column).map(|column| format!("{column} = r.{column}")))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    sqlx::query(&format!(
        "UPDATE {table} t SET {set} FROM jsonb_populate_recordset(NULL::{table}, $1) r \
         WHERE t.id = r.id"
    ))
    .bind(Value::Array(rows))
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn plain(name: &str) -> Result<&str, ImportError> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Ok(name)
    } else {
        Err(refused(format!("a column is named {name:?}")))
    }
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(IMPORT_KIND, |context| async move {
        let payload = context.job.payload_as::<ImportPayload>()?;
        run_import(&context.state, payload.transfer, RunLimits::default())
            .await
            .map_err(JobError::from)?;
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_climb_out_or_hide_are_refused() {
        for name in [
            "../manifest.json",
            "things/../../etc/passwd",
            "/manifest.json",
            "things/tasks/./work_items.jsonl",
            "Things/tasks/work_items.jsonl",
            "other/file",
            "things//x.jsonl",
        ] {
            assert!(!allowed_name(name), "{name}");
        }
        for name in [
            "manifest.json",
            "things/tasks/work_items.jsonl",
            "files/blobs/ab12",
            "accounts/people.jsonl",
        ] {
            assert!(allowed_name(name), "{name}");
        }
    }

    #[test]
    fn ids_in_text_are_rewritten_and_keep_their_case() {
        let from = Uuid::now_v7();
        let to = Uuid::now_v7();
        let map = HashMap::from([(from, to)]);
        let mut value = json!({
            "id": from.to_string(),
            "body": format!("see /tasks/{from}#x and {}", from.to_string().to_uppercase()),
            "other": Uuid::now_v7().to_string(),
        });
        let other = value["other"].clone();
        rewrite(&mut value, &map);
        assert_eq!(value["id"], to.to_string());
        assert_eq!(
            value["body"],
            format!("see /tasks/{to}#x and {}", to.to_string().to_uppercase())
        );
        assert_eq!(value["other"], other);
    }
}
