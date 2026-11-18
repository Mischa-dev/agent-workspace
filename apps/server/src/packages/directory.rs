//! The directory (M1 section 3.9, slice M1.11): what can be installed.
//!
//! - **Built in**: `modules/packages/` in the release (the templates and
//!   the shapes), compiled in and always listed.
//! - **Server-wide**: a folder (`AGENT_WORKSPACE_DIRECTORY_DIR`) and URL
//!   lists (`AGENT_WORKSPACE_DIRECTORY_URLS`) the operator sets; synced into
//!   `directory_sources` and read by the refresh job at start-up and
//!   hourly.
//! - **A workspace's own** URL lists (`directory_source_write`), where the
//!   server allows them.
//! - **Saved workspace templates**, read as packages (`legacy_template`).
//!
//! The refresh job reads each source into `directory_entries` with the
//! inline document, so an install never fetches anything. A file or entry
//! that doesn't read is kept with its error and listed: never dropped
//! silently. A URL list is fetched through `outbound.rs`.

use std::{path::Path, time::Duration};

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::{builtin, format::Package, install, zipped};
use crate::{config::AppConfig, error::AppError, jobs::JobRegistry};

/// The job that reads the sources.
pub const REFRESH_KIND: &str = "directory_refresh";
/// How often a source is read again.
const REFRESH_EVERY: Duration = Duration::from_secs(60 * 60);
/// The most entries one source lists.
const MAX_ENTRIES: usize = 200;
/// The format a URL list's index names.
pub const INDEX_FORMAT: &str = "agent-workspace-directory/1";

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(REFRESH_KIND, |context| async move {
        refresh(&context.state.pool, &context.state.config).await?;
        Ok(())
    });
    // Due sources are read: one an hour old, or new since the last run.
    registry.every(REFRESH_KIND, Duration::from_secs(15 * 60));
}

// ---------------------------------------------------------------- reading

/// One package a source offers, read or not.
#[derive(Debug)]
struct Read {
    key: String,
    version: String,
    name: String,
    description: String,
    location: String,
    result: Result<(Value, String), String>,
}

/// A document as an entry: the package it is, or why it isn't one.
fn entry_of(location: &str, fallback_key: &str, document: Result<Value, String>) -> Read {
    let checked = document.and_then(|document| {
        let package = install::read(&document).map_err(|error| error.message().into_owned())?;
        let package = package.sealed();
        let digest = package.digest.clone().unwrap_or_default();
        let document = serde_json::to_value(&package).map_err(|error| error.to_string())?;
        Ok((package, document, digest))
    });
    match checked {
        Ok((package, document, digest)) => Read {
            key: package.id.clone(),
            version: package.version.clone(),
            name: package.name.chars().take(200).collect(),
            description: package.description.chars().take(2000).collect(),
            location: location.to_owned(),
            result: Ok((document, digest)),
        },
        Err(error) => Read {
            key: fallback_key.chars().take(300).collect(),
            version: String::new(),
            name: fallback_key.chars().take(200).collect(),
            description: String::new(),
            location: location.to_owned(),
            result: Err(error.chars().take(2000).collect()),
        },
    }
}

/// A package's bytes as its inline document: a zip, or JSON.
pub fn document_of(bytes: &[u8]) -> Result<Value, String> {
    if zipped::is_zip(bytes) {
        return zipped::inline(bytes);
    }
    if bytes.len() > install::MAX_DOCUMENT_BYTES {
        return Err("A package file is at most 1 MB (5 MB zipped).".to_owned());
    }
    serde_json::from_slice::<Value>(bytes).map_err(|error| format!("This isn't JSON: {error}"))
}

/// Reads a file of at most `limit` bytes.
async fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "The file can't be read.".to_owned())?;
    if metadata.len() > limit as u64 {
        return Err("The file is too large for a package.".to_owned());
    }
    tokio::fs::read(path)
        .await
        .map_err(|_| "The file can't be read.".to_owned())
}

/// The packages in a folder: `*.json` and `*.zip` files, and folders
/// holding a `package.json`.
async fn read_folder(folder: &Path) -> Result<Vec<Read>, String> {
    let mut found = Vec::new();
    let mut entries = tokio::fs::read_dir(folder)
        .await
        .map_err(|_| "The folder can't be read.".to_owned())?;
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths.into_iter().take(MAX_ENTRIES) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let file = if path.is_dir() {
            let manifest = path.join("package.json");
            if !manifest.is_file() {
                continue;
            }
            manifest
        } else if name.ends_with(".json") || name.ends_with(".zip") {
            path.clone()
        } else {
            continue;
        };
        let location = path.to_string_lossy().into_owned();
        let stem = name
            .trim_end_matches(".json")
            .trim_end_matches(".zip")
            .to_owned();
        let document = match read_file(&file, zipped::MAX_ZIP_BYTES).await {
            Ok(bytes) if path.is_dir() => folder_package(&path, &bytes).await,
            Ok(bytes) => document_of(&bytes),
            Err(error) => Err(error),
        };
        found.push(entry_of(&location, &stem, document));
    }
    Ok(found)
}

/// A package kept as a folder: its `package.json`, with each part's
/// `path` read from beside it.
async fn folder_package(folder: &Path, manifest: &[u8]) -> Result<Value, String> {
    let mut document = serde_json::from_slice::<Value>(manifest)
        .map_err(|error| format!("package.json isn't JSON: {error}"))?;
    let mut files = Vec::new();
    if let Some(contents) = document["contents"].as_array() {
        for part in contents {
            if let Some(path) = part["path"].as_str() {
                if path.starts_with('/') || path.split('/').any(|step| step == "..") {
                    return Err(format!("{path}: a part's path stays inside the package."));
                }
                let bytes = read_file(&folder.join(path), install::MAX_DOCUMENT_BYTES)
                    .await
                    .map_err(|error| format!("{path}: {error}"))?;
                files.push((
                    path.to_owned(),
                    String::from_utf8(bytes).map_err(|_| format!("{path} isn't UTF-8 text."))?,
                ));
            }
        }
    }
    if files.is_empty() {
        return Ok(document);
    }
    // Read the same way as a zip: pack it and unpack it.
    let mut listed = vec![("package.json".to_owned(), document.to_string())];
    listed.extend(files);
    let packed = pack(&listed)?;
    document = zipped::inline(&packed)?;
    Ok(document)
}

fn pack(files: &[(String, String)]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (path, text) in files {
            writer
                .start_file(path.as_str(), zip::write::SimpleFileOptions::default())
                .map_err(|_| "The package can't be read.".to_owned())?;
            writer
                .write_all(text.as_bytes())
                .map_err(|_| "The package can't be read.".to_owned())?;
        }
        writer
            .finish()
            .map_err(|_| "The package can't be read.".to_owned())?;
    }
    Ok(buffer.into_inner())
}

/// Fetches `url` (at most `limit` bytes) through `outbound.rs`.
async fn fetch(url: &str, allow_private: bool, limit: usize) -> Result<Vec<u8>, String> {
    let parsed = url::Url::parse(url).map_err(|_| "This isn't a URL.".to_owned())?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err("A URL list is an http or https address.".to_owned());
    }
    let client = crate::outbound::client_for(
        &parsed,
        allow_private,
        crate::outbound::Timeouts {
            connect: Duration::from_secs(5),
            total: Some(Duration::from_secs(20)),
        },
        Some("agent-workspace-directory"),
    )
    .await
    .map_err(|_| "The address can't be reached from this server.".to_owned())?;
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|_| "The address didn't answer.".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("The address answered {}.", response.status()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The answer was cut off.".to_owned())?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > limit {
            return Err("The answer is too large.".to_owned());
        }
    }
    Ok(bytes)
}

/// The packages a URL list names: `{"format": "agent-workspace-directory/1",
/// "entries": [{"url"} or {"package"}]}`, each an inline package or a zip.
async fn read_url_list(url: &str, allow_private: bool) -> Result<Vec<Read>, String> {
    let index = fetch(url, allow_private, install::MAX_DOCUMENT_BYTES).await?;
    let index = serde_json::from_slice::<Value>(&index)
        .map_err(|error| format!("The list isn't JSON: {error}"))?;
    if index["format"] != INDEX_FORMAT {
        return Err(format!(
            "The list's format is \"{INDEX_FORMAT}\", with its entries."
        ));
    }
    let mut found = Vec::new();
    for (position, entry) in index["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(MAX_ENTRIES)
        .enumerate()
    {
        let fallback = format!("entry_{}", position + 1);
        if let Some(document) = entry.get("package") {
            found.push(entry_of(url, &fallback, Ok(document.clone())));
        } else if let Some(location) = entry["url"].as_str() {
            let document = match fetch(location, allow_private, zipped::MAX_ZIP_BYTES).await {
                Ok(bytes) => document_of(&bytes),
                Err(error) => Err(error),
            };
            found.push(entry_of(location, &fallback, document));
        } else {
            found.push(entry_of(
                url,
                &fallback,
                Err("An entry names its package (url or package).".to_owned()),
            ));
        }
    }
    Ok(found)
}

// --------------------------------------------------------------- the sources

/// Makes `directory_sources` match the operator's settings: one row per
/// configured folder and URL list, the rest switched off.
pub async fn sync_configured(pool: &PgPool, config: &AppConfig) -> Result<(), AppError> {
    let mut wanted = Vec::new();
    if let Some(folder) = &config.directory_dir {
        wanted.push(("folder", folder.to_string_lossy().into_owned()));
    }
    for url in &config.directory_urls {
        wanted.push(("url_list", url.clone()));
    }
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let mut kept = Vec::new();
    for (kind, location) in &wanted {
        let id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO directory_sources (id, workspace_id, kind, location)
            VALUES ($1, NULL, $2, $3)
            ON CONFLICT (kind, location) WHERE workspace_id IS NULL DO UPDATE
            SET enabled = true,
                fetched_at = CASE WHEN directory_sources.enabled
                                  THEN directory_sources.fetched_at END
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(location)
        .fetch_one(&mut *transaction)
        .await?;
        kept.push(id);
    }
    sqlx::query(
        "UPDATE directory_sources SET enabled = false \
         WHERE workspace_id IS NULL AND enabled AND NOT (id = ANY($1))",
    )
    .bind(&kept)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Reads every enabled source that is due (never read, or read an hour
/// ago or more). Returns how many it read.
pub async fn refresh(pool: &PgPool, config: &AppConfig) -> Result<usize, AppError> {
    sync_configured(pool, config).await?;
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let due = sqlx::query_as::<_, (Uuid, Option<Uuid>, String, String)>(
        r#"
        SELECT id, workspace_id, kind, location FROM directory_sources
        WHERE enabled AND kind IN ('folder', 'url_list')
          AND (fetched_at IS NULL OR fetched_at <= now() - make_interval(secs => $1))
        ORDER BY fetched_at NULLS FIRST, id
        LIMIT 50
        "#,
    )
    .bind(REFRESH_EVERY.as_secs_f64())
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    for (id, workspace_id, kind, location) in &due {
        refresh_source(pool, config, *id, *workspace_id, kind, location).await?;
    }
    Ok(due.len())
}

/// Reads one source now and replaces its entries.
pub async fn refresh_source(
    pool: &PgPool,
    config: &AppConfig,
    id: Uuid,
    workspace_id: Option<Uuid>,
    kind: &str,
    location: &str,
) -> Result<(), AppError> {
    // The operator's own lists may be on their network; a workspace's
    // follow the server's rule for addresses a workspace sets.
    let allow_private = workspace_id.is_none() || config.allow_private_addresses;
    // Read before any transaction: a URL list is a network call.
    let read = match kind {
        "folder" => read_folder(Path::new(location)).await,
        _ => read_url_list(location, allow_private).await,
    };
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let error = match read {
        Ok(entries) => {
            sqlx::query("DELETE FROM directory_entries WHERE source_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await?;
            for entry in entries {
                let (document, digest, error) = match entry.result {
                    Ok((document, digest)) => (Some(document), Some(digest), None),
                    Err(error) => (None, None, Some(error)),
                };
                sqlx::query(
                    r#"
                    INSERT INTO directory_entries (
                        source_id, workspace_id, entry_kind, entry_key, version, name,
                        description, location, digest, document, error
                    ) VALUES ($1, $2, 'package', $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (source_id, entry_kind, entry_key, version) DO NOTHING
                    "#,
                )
                .bind(id)
                .bind(workspace_id)
                .bind(&entry.key)
                .bind(&entry.version)
                .bind(&entry.name)
                .bind(&entry.description)
                .bind(entry.location.chars().take(2000).collect::<String>())
                .bind(digest)
                .bind(document)
                .bind(error)
                .execute(&mut *transaction)
                .await?;
            }
            None
        }
        Err(error) => Some(error),
    };
    sqlx::query("UPDATE directory_sources SET fetched_at = now(), error = $2 WHERE id = $1")
        .bind(id)
        .bind(error)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

// ----------------------------------------------------------------- listing

/// One thing the directory offers.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    /// What to pass as package_preview's and package_install's `entry`:
    /// the package's id, or "id@version" where several versions are
    /// listed.
    pub entry: String,
    /// The package's id, such as "software_team" or "studio.game_studio"
    /// (a broken entry: its file or position).
    pub id: String,
    pub version: String,
    /// package (connected modules come later).
    pub kind: String,
    pub name: String,
    pub description: String,
    /// workspace or space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Where it comes from: built_in, server, workspace or saved_template.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub source_id: Option<Uuid>,
    /// A shape: what a workspace is for, chosen at first run.
    pub shape: bool,
    /// How many parts it has.
    pub parts: usize,
    /// Why it can't be installed, for an entry that didn't read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn view_of(package: &Package, source: &str, source_id: Option<Uuid>, shape: bool) -> EntryView {
    EntryView {
        entry: package.id.clone(),
        id: package.id.clone(),
        version: package.version.clone(),
        kind: "package".to_owned(),
        name: package.name.clone(),
        description: package.description.clone(),
        target: Some(
            match package.target {
                super::format::PackageTarget::Workspace => "workspace",
                super::format::PackageTarget::Space => "space",
            }
            .to_owned(),
        ),
        source: source.to_owned(),
        source_id,
        shape,
        parts: package.contents.len(),
        error: None,
    }
}

#[derive(sqlx::FromRow)]
struct EntryRow {
    source_id: Uuid,
    workspace_id: Option<Uuid>,
    entry_key: String,
    version: String,
    name: String,
    description: String,
    document: Option<Value>,
    error: Option<String>,
}

/// A source's broken read, as a listed entry.
#[derive(sqlx::FromRow)]
struct SourceError {
    id: Uuid,
    workspace_id: Option<Uuid>,
    location: String,
    error: String,
}

/// What `workspace_id` can install: built in first (the templates, then
/// the shapes), then the server's sources and the workspace's own, then
/// its saved templates. `query` matches names, ids and descriptions.
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    query: Option<&str>,
) -> Result<Vec<EntryView>, AppError> {
    let mut found = builtin::all()
        .iter()
        .map(|builtin| view_of(&builtin.package, "built_in", None, builtin.shape))
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, EntryRow>(
        r#"
        SELECT entry.source_id, entry.workspace_id, entry.entry_key, entry.version, entry.name,
               entry.description, entry.document, entry.error
        FROM directory_entries entry
        JOIN directory_sources source ON source.id = entry.source_id AND source.enabled
        WHERE entry.entry_kind = 'package'
          AND (entry.workspace_id IS NULL OR entry.workspace_id = $1)
        ORDER BY entry.workspace_id NULLS FIRST, lower(entry.name), entry.entry_key,
                 entry.version DESC
        LIMIT 1000
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        let source = if row.workspace_id.is_some() {
            "workspace"
        } else {
            "server"
        };
        let package = row
            .document
            .as_ref()
            .and_then(|document| serde_json::from_value::<Package>(document.clone()).ok());
        found.push(match (package, row.error) {
            (Some(package), None) => view_of(&package, source, Some(row.source_id), false),
            (_, error) => EntryView {
                entry: row.entry_key.clone(),
                id: row.entry_key,
                version: row.version,
                kind: "package".to_owned(),
                name: row.name,
                description: row.description,
                target: None,
                source: source.to_owned(),
                source_id: Some(row.source_id),
                shape: false,
                parts: 0,
                error: Some(error.unwrap_or_else(|| "It doesn't read as a package.".to_owned())),
            },
        });
    }
    // A source that couldn't be read at all is listed too.
    let broken = sqlx::query_as::<_, SourceError>(
        "SELECT id, workspace_id, location, error FROM directory_sources \
         WHERE enabled AND error IS NOT NULL AND (workspace_id IS NULL OR workspace_id = $1) \
         ORDER BY created_at, id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    for source in broken {
        found.push(EntryView {
            entry: String::new(),
            id: source.location.clone(),
            version: String::new(),
            kind: "source".to_owned(),
            name: source.location,
            description: String::new(),
            target: None,
            source: if source.workspace_id.is_some() {
                "workspace"
            } else {
                "server"
            }
            .to_owned(),
            source_id: Some(source.id),
            shape: false,
            parts: 0,
            error: Some(source.error),
        });
    }
    // Saved workspace templates, as packages.
    for template in crate::workspace_templates::list(connection, workspace_id).await? {
        if template.built_in {
            continue;
        }
        let package = super::format::from_template(
            &template.id,
            &template.name,
            &template.description,
            &template.definition,
        );
        let mut view = view_of(&package, "saved_template", None, false);
        view.entry = format!("template:{}", template.id);
        found.push(view);
    }
    // Several versions of one id: name each by its version.
    let ids = found
        .iter()
        .filter(|view| view.error.is_none())
        .map(|view| view.id.clone())
        .collect::<Vec<_>>();
    for view in found.iter_mut().filter(|view| view.error.is_none()) {
        if ids.iter().filter(|id| **id == view.id).count() > 1 && !view.entry.contains(':') {
            view.entry = format!("{}@{}", view.id, view.version);
        }
    }
    if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
        let query = query.to_lowercase();
        found.retain(|view| {
            view.name.to_lowercase().contains(&query)
                || view.id.to_lowercase().contains(&query)
                || view.description.to_lowercase().contains(&query)
        });
    }
    Ok(found)
}

/// A package the directory offers, as an install reads it.
pub struct Found {
    pub package: Package,
    pub document: Value,
    /// `packages.source`.
    pub source: &'static str,
    pub source_ref: String,
    /// A saved template's id and name.
    pub template: Option<(String, String)>,
}

/// The entry `entry` names for `workspace_id` (as directory_list gives it:
/// an id, "id@version", or "template:<id>"), or why it can't be installed.
pub async fn find(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    entry: &str,
) -> Result<Result<Found, String>, AppError> {
    let entry = entry.trim();
    if let Some(template) = entry.strip_prefix("template:") {
        let Some(view) = crate::workspace_templates::find(connection, workspace_id, template)
            .await?
            .filter(|view| !view.built_in)
        else {
            return Ok(Err("No saved template has that id or name.".to_owned()));
        };
        let package =
            super::format::from_template(&view.id, &view.name, &view.description, &view.definition);
        let document = serde_json::to_value(&package).unwrap_or(Value::Null);
        return Ok(Ok(Found {
            package,
            document,
            source: "legacy_template",
            source_ref: format!("template:{}", view.id),
            template: Some((view.id, view.name)),
        }));
    }
    let (id, version) = match entry.split_once('@') {
        Some((id, version)) => (id, Some(version)),
        None => (entry, None),
    };
    if let Some(builtin) = builtin::find(id)
        && version.is_none_or(|version| version == builtin.package.version)
    {
        return Ok(Ok(Found {
            package: builtin.package.clone(),
            document: builtin.document.clone(),
            source: "builtin",
            source_ref: format!("builtin:{}", builtin.folder),
            template: None,
        }));
    }
    let row = sqlx::query_as::<_, EntryRow>(
        r#"
        SELECT entry.source_id, entry.workspace_id, entry.entry_key, entry.version, entry.name,
               entry.description, entry.document, entry.error
        FROM directory_entries entry
        JOIN directory_sources source ON source.id = entry.source_id AND source.enabled
        WHERE entry.entry_kind = 'package' AND entry.entry_key = $2
          AND ($3::text IS NULL OR entry.version = $3)
          AND (entry.workspace_id IS NULL OR entry.workspace_id = $1)
        ORDER BY entry.error IS NOT NULL, entry.workspace_id NULLS FIRST,
                 entry.fetched_at DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(id)
    .bind(version)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(Err(
            "The directory has no such package: pick one from directory_list.".to_owned(),
        ));
    };
    if let Some(error) = row.error {
        return Ok(Err(format!("That entry can't be installed: {error}")));
    }
    let document = row.document.unwrap_or(Value::Null);
    let package = match install::read(&document) {
        Ok(package) => package,
        Err(error) => return Ok(Err(error.message().into_owned())),
    };
    Ok(Ok(Found {
        package,
        document,
        source: "directory",
        source_ref: format!("{}:{}@{}", row.source_id, row.entry_key, row.version),
        template: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_folder_lists_its_packages_and_its_broken_files() {
        let folder = tempfile::tempdir().expect("folder");
        let good = builtin::find("personal")
            .expect("personal")
            .document
            .clone();
        let mut renamed = good.clone();
        renamed["id"] = Value::String("studio.personal_plus".into());
        renamed["name"] = Value::String("Personal plus".into());
        renamed.as_object_mut().expect("object").remove("digest");
        std::fs::write(folder.path().join("plus.json"), renamed.to_string()).expect("write");
        std::fs::write(folder.path().join("broken.json"), "{ not json").expect("write");
        std::fs::write(
            folder.path().join("zipped.zip"),
            zipped::zip_of(&[("package.json", &good.to_string())]),
        )
        .expect("write");
        std::fs::write(folder.path().join("notes.txt"), "ignored").expect("write");
        let read = read_folder(folder.path()).await.expect("reads");
        assert_eq!(read.len(), 3, "{read:?}");
        let broken = read
            .iter()
            .find(|entry| entry.key == "broken")
            .expect("broken");
        assert!(
            broken
                .result
                .as_ref()
                .is_err_and(|error| error.contains("JSON"))
        );
        assert!(
            read.iter()
                .any(|entry| entry.key == "studio.personal_plus" && entry.result.is_ok())
        );
        assert!(
            read.iter()
                .any(|entry| entry.key == "personal" && entry.result.is_ok())
        );
    }
}
