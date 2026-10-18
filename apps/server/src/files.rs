//! W20: files and images on messages, tasks and task comments.
//!
//! A file is uploaded into one place, a conversation or a task, by someone
//! who may post there. A message's files wait, seen only by their uploader,
//! until the message is sent with their IDs (`attach_to_message`); a
//! comment's the same way (`attach_to_comment`). Files uploaded to a task
//! itself are attached at once. Unsent uploads are cleaned up after a day.
//!
//! Reading a file checks its place every time (`readable_file`): a message's
//! file needs the project, `conversation_visible_to` and the message not
//! removed; a task's file needs the project. File URLs carry no secret: the
//! browser's session (or the agent's credential, over MCP) is checked on
//! every request, so a link copied out of the app opens for nobody else.
//!
//! The bytes live in a [`FileStore`]; images lose their location data and
//! get a preview first (`image`). Size caps are workspace settings
//! (`files.max_upload_mb`, `files.workspace_quota_mb`), and every upload
//! passes storage admission, counting the files the database knows about.

pub mod http;
pub mod image;
pub mod store;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

pub use store::FileStore;

use crate::{
    AppState,
    error::AppError,
    jobs::JobRegistry,
    settings::{self, SettingsContext},
    storage_admission,
};

/// Most files on one message or comment.
pub const MAX_ATTACHMENTS: usize = 10;
/// Uploads nobody sent are removed after this long.
const UNSENT_HOURS: i32 = 24;
/// The upload route never reads more than this, whatever the setting says.
pub const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;
/// Files removed per cleanup run.
const CLEANUP_BATCH: i64 = 200;
const CLEANUP_KIND: &str = "files.cleanup";

/// A file as messages, tasks and agents see it. Clients build its URLs from
/// the ID: `/api/v1/files/{id}/content` (add `?download=1` to save it) and,
/// for images, `/api/v1/files/{id}/thumbnail`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    /// Set for images that show inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// The task comment it belongs to, for a task's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<Uuid>,
    pub uploaded_by: Uuid,
    pub created_at: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct FileRow {
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) work_item_id: Option<Uuid>,
    pub(crate) comment_id: Option<Uuid>,
    pub(crate) uploaded_by_actor_id: Uuid,
    pub(crate) name: String,
    pub(crate) content_type: String,
    pub(crate) byte_size: i64,
    pub(crate) storage_key: String,
    pub(crate) image_width: Option<i32>,
    pub(crate) image_height: Option<i32>,
    pub(crate) thumbnail_key: Option<String>,
    pub(crate) attached_at: Option<String>,
    pub(crate) created_at: String,
}

impl FileRow {
    pub(crate) fn view(&self) -> FileView {
        FileView {
            id: self.id,
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            size: self.byte_size,
            width: self.image_width,
            height: self.image_height,
            comment_id: self.comment_id,
            uploaded_by: self.uploaded_by_actor_id,
            created_at: self.created_at.clone(),
        }
    }

    pub(crate) fn is_inline_image(&self) -> bool {
        self.image_width.is_some()
    }
}

const FILE_COLUMNS: &str = r#"
    file.id, file.project_id, file.work_item_id, file.comment_id,
    file.uploaded_by_actor_id, file.name, file.content_type, file.byte_size,
    file.storage_key, file.image_width, file.image_height, file.thumbnail_key,
    file.attached_at::text AS attached_at, file.created_at::text AS created_at
"#;

/// Who is reading: a browser session, or an MCP caller (an agent through its
/// connection, or a person through an app they connected).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reader {
    Browser {
        actor_id: Uuid,
    },
    Mcp {
        actor_id: Uuid,
        connection_id: Option<Uuid>,
        /// An older project-scoped credential reaches only this project.
        confined_project: Option<Uuid>,
    },
}

/// The condition a reader must meet to see `file` (a `files` row): $2 is the
/// reader, $3 their agent connection, $4 true for MCP, $5 the one project an
/// older MCP credential is confined to.
const READABLE: &str = r#"
    file.removed_at IS NULL
    AND file.blob_deleted_at IS NULL
    AND ($5::uuid IS NULL OR file.project_id = $5)
    AND CASE WHEN $4 THEN actor_can_access_project($2, $3, file.project_id)
        ELSE EXISTS (
            SELECT 1
            FROM project_memberships membership
            JOIN organization_memberships organization_membership
              ON organization_membership.organization_id = file.organization_id
             AND organization_membership.actor_id = $2
             AND organization_membership.enabled
            WHERE membership.project_id = file.project_id
              AND membership.actor_id = $2
              AND membership.enabled
        )
    END
    AND CASE
        WHEN file.attached_at IS NULL THEN file.uploaded_by_actor_id = $2
        WHEN file.conversation_id IS NOT NULL THEN
            EXISTS (
                SELECT 1 FROM messages message
                WHERE message.id = file.message_id AND message.removed_at IS NULL
            )
            AND conversation_visible_to(file.conversation_id, $2)
        ELSE true
    END
"#;

fn bind_reader<'q, O>(
    query: sqlx::query::QueryAs<'q, Postgres, O, sqlx::postgres::PgArguments>,
    reader: Reader,
) -> sqlx::query::QueryAs<'q, Postgres, O, sqlx::postgres::PgArguments> {
    match reader {
        Reader::Browser { actor_id } => query
            .bind(actor_id)
            .bind(None::<Uuid>)
            .bind(false)
            .bind(None::<Uuid>),
        Reader::Mcp {
            actor_id,
            connection_id,
            confined_project,
        } => query
            .bind(actor_id)
            .bind(connection_id)
            .bind(true)
            .bind(confined_project),
    }
}

/// One file the reader may see right now, or NotFound.
pub(crate) async fn readable_file(
    pool: &PgPool,
    reader: Reader,
    file_id: Uuid,
) -> Result<FileRow, AppError> {
    let sql = format!("SELECT {FILE_COLUMNS} FROM files file WHERE file.id = $1 AND {READABLE}");
    bind_reader(sqlx::query_as::<_, FileRow>(&sql).bind(file_id), reader)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("file"))
}

/// The files of one message the reader may see, in order.
pub(crate) async fn message_files(
    pool: &PgPool,
    reader: Reader,
    message_id: Uuid,
) -> Result<Vec<FileRow>, AppError> {
    let sql = format!(
        "SELECT {FILE_COLUMNS} FROM files file WHERE file.message_id = $1 AND {READABLE}
         ORDER BY file.position, file.id"
    );
    Ok(
        bind_reader(sqlx::query_as::<_, FileRow>(&sql).bind(message_id), reader)
            .fetch_all(pool)
            .await?,
    )
}

/// A task's files the reader may see (its own and its comments'), oldest
/// first. The caller has checked the reader may see the task.
pub(crate) async fn task_files(
    pool: &PgPool,
    reader: Reader,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<FileRow>, AppError> {
    let sql = format!(
        "SELECT {FILE_COLUMNS} FROM files file
         WHERE file.project_id = $6 AND file.work_item_id = $1
           AND file.attached_at IS NOT NULL AND {READABLE}
         ORDER BY file.created_at, file.position, file.id"
    );
    Ok(bind_reader(
        sqlx::query_as::<_, FileRow>(&sql).bind(work_item_id),
        reader,
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// Uploading

/// Where an upload goes. The caller has checked the uploader may post there.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Destination {
    /// For a message in this conversation; attached when the message is sent.
    Conversation {
        project_id: Uuid,
        conversation_id: Uuid,
    },
    /// A task's own file, attached at once, or (`for_comment`) for a comment
    /// on it, attached when the comment is posted.
    Task {
        project_id: Uuid,
        work_item_id: Uuid,
        for_comment: bool,
    },
}

impl Destination {
    fn project_id(self) -> Uuid {
        match self {
            Self::Conversation { project_id, .. } | Self::Task { project_id, .. } => project_id,
        }
    }
}

/// An upload as it arrived.
pub(crate) struct Upload {
    pub(crate) name: String,
    /// What the sender said it is. Images are recognized from their bytes;
    /// for anything else this (or the name's extension) is kept.
    pub(crate) declared_type: Option<String>,
    pub(crate) bytes: Vec<u8>,
}

/// The largest upload this project's workspace takes, in bytes.
pub(crate) async fn upload_limit(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<usize, AppError> {
    let organization_id = organization_of(&mut *connection, project_id).await?;
    let effective = settings::resolve(
        connection,
        SettingsContext {
            organization_id,
            project_id: None,
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    let megabytes = effective
        .value::<i64>("files.max_upload_mb")
        .unwrap_or(25)
        .clamp(1, 100);
    Ok(usize::try_from(megabytes)
        .unwrap_or(25)
        .saturating_mul(1024 * 1024)
        .min(MAX_UPLOAD_BYTES))
}

async fn organization_of(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("project"))
}

/// Stores an upload in `destination` for `uploader` and returns it. The
/// caller has checked the uploader may post there and that the bytes are
/// within [`upload_limit`].
pub(crate) async fn store_upload(
    state: &AppState,
    uploader: Uuid,
    destination: Destination,
    upload: Upload,
) -> Result<FileRow, AppError> {
    let project_id = destination.project_id();
    let organization_id = organization_of(&state.pool, project_id).await?;
    let name = clean_name(&upload.name);
    let declared = upload.declared_type;
    let processed = {
        let bytes = upload.bytes;
        tokio::task::spawn_blocking(move || {
            if image::sniff(&bytes).is_some() {
                match image::process(bytes.clone()) {
                    Some(processed) => Ok(processed),
                    None => Err(bytes),
                }
            } else {
                Err(bytes)
            }
        })
        .await
        .map_err(|_| AppError::Internal("process an uploaded image"))?
    };
    let (bytes, content_type, dimensions, thumbnail) = match processed {
        Ok(image) => (
            image.bytes,
            image.format.content_type().to_owned(),
            Some((image.width, image.height)),
            image.thumbnail,
        ),
        Err(bytes) => {
            let content_type = file_content_type(&name, declared.as_deref(), &bytes);
            (bytes, content_type, None, None)
        }
    };
    let incoming = u64::try_from(bytes.len())
        .unwrap_or(u64::MAX)
        .saturating_add(thumbnail.as_ref().map_or(0, |thumbnail| {
            u64::try_from(thumbnail.bytes.len()).unwrap_or(u64::MAX)
        }));
    require_room(state, organization_id, incoming).await?;

    let id = Uuid::now_v7();
    let storage_key = format!("{}/{}", organization_id.simple(), id.simple());
    let thumbnail_key = thumbnail.as_ref().map(|_| format!("{storage_key}.thumb"));
    let store = FileStore::from_config(&state.config);
    store.put(&storage_key, &bytes).await?;
    if let (Some(key), Some(thumbnail)) = (&thumbnail_key, &thumbnail) {
        if let Err(error) = store.put(key, &thumbnail.bytes).await {
            let _ = store.delete(&storage_key).await;
            return Err(error);
        }
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let (conversation_id, work_item_id, attached) = match destination {
        Destination::Conversation {
            conversation_id, ..
        } => (Some(conversation_id), None, false),
        Destination::Task {
            work_item_id,
            for_comment,
            ..
        } => (None, Some(work_item_id), !for_comment),
    };
    let inserted = sqlx::query_as::<_, FileRow>(&format!(
        r#"
        INSERT INTO files AS file (
            id, organization_id, project_id, conversation_id, work_item_id,
            uploaded_by_actor_id, name, content_type, byte_size, sha256,
            storage_key, image_width, image_height, thumbnail_key,
            thumbnail_bytes, attached_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                CASE WHEN $16 THEN now() END)
        RETURNING {FILE_COLUMNS}
        "#
    ))
    .bind(id)
    .bind(organization_id)
    .bind(project_id)
    .bind(conversation_id)
    .bind(work_item_id)
    .bind(uploader)
    .bind(&name)
    .bind(&content_type)
    .bind(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
    .bind(&sha256)
    .bind(&storage_key)
    .bind(dimensions.map(|(width, _)| i32::try_from(width).unwrap_or(i32::MAX)))
    .bind(dimensions.map(|(_, height)| i32::try_from(height).unwrap_or(i32::MAX)))
    .bind(&thumbnail_key)
    .bind(
        thumbnail
            .as_ref()
            .map(|thumbnail| i64::try_from(thumbnail.bytes.len()).unwrap_or(i64::MAX)),
    )
    .bind(attached)
    .fetch_one(&state.pool)
    .await;
    match inserted {
        Ok(row) => Ok(row),
        Err(error) => {
            let _ = store.delete(&storage_key).await;
            if let Some(key) = &thumbnail_key {
                let _ = store.delete(key).await;
            }
            Err(error.into())
        }
    }
}

/// Refuses an upload that would take the workspace over its file quota, or
/// the server over its storage watermark. The walk of the data root leaves
/// the file store out (`storage_admission`), so the files the database knows
/// about are added here.
async fn require_room(
    state: &AppState,
    organization_id: Uuid,
    incoming: u64,
) -> Result<(), AppError> {
    let mut connection = state.pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id: None,
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    let quota_mb = effective
        .value::<i64>("files.workspace_quota_mb")
        .unwrap_or(10_240)
        .max(0);
    let (workspace_bytes, all_bytes) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
            COALESCE(sum(byte_size + COALESCE(thumbnail_bytes, 0))
                FILTER (WHERE organization_id = $1), 0)::bigint,
            COALESCE(sum(byte_size + COALESCE(thumbnail_bytes, 0)), 0)::bigint
        FROM files
        WHERE blob_deleted_at IS NULL
        "#,
    )
    .bind(organization_id)
    .fetch_one(&mut *connection)
    .await?;
    drop(connection);
    let incoming_i64 = i64::try_from(incoming).unwrap_or(i64::MAX);
    if quota_mb > 0
        && workspace_bytes.saturating_add(incoming_i64) > quota_mb.saturating_mul(1024 * 1024)
    {
        return Err(AppError::Conflict(
            "storage",
            "this workspace's storage for files is full",
        ));
    }
    let config = &state.config;
    // A fresh install may not have made its data folder yet.
    tokio::fs::create_dir_all(&config.data_root)
        .await
        .map_err(|_| AppError::Internal("create the data folder"))?;
    let report = storage_admission::inspect(
        &config.data_root,
        config.data_quota_bytes,
        config.minimum_free_bytes,
    )?;
    let allocated = report
        .allocated_bytes
        .saturating_add(u64::try_from(all_bytes).unwrap_or(0))
        .saturating_add(incoming);
    if allocated >= config.data_quota_bytes
        || report.available_bytes.saturating_sub(incoming) <= config.minimum_free_bytes
    {
        return Err(AppError::Conflict(
            "storage",
            "disk growth is blocked by the configured watermark",
        ));
    }
    Ok(())
}

/// A file name safe to store and show: no folders, no control characters,
/// at most 255 characters, never empty.
pub(crate) fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned = base
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let trimmed = cleaned.trim().trim_start_matches('.');
    let mut name = trimmed.chars().take(255).collect::<String>();
    if name.trim().is_empty() {
        name = "file".to_owned();
    }
    name
}

/// The type a non-image file is stored with: what the sender said when it is
/// well formed and specific, else a guess from the name, else plain text for
/// UTF-8 that looks like text, else octet-stream.
fn file_content_type(name: &str, declared: Option<&str>, bytes: &[u8]) -> String {
    let declared = declared
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| valid_content_type(value) && value != "application/octet-stream");
    if let Some(declared) = declared {
        return declared;
    }
    if let Some(guess) = mime_guess::from_path(name).first() {
        let guess = guess.essence_str().to_ascii_lowercase();
        if valid_content_type(&guess) {
            return guess;
        }
    }
    let sample = &bytes[..bytes.len().min(4096)];
    if !sample.is_empty() && !sample.contains(&0) && std::str::from_utf8(sample).is_ok() {
        "text/plain".to_owned()
    } else {
        "application/octet-stream".to_owned()
    }
}

fn valid_content_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    let part = |part: &str| {
        (1..=64).contains(&part.len())
            && part
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    part(kind) && part(subtype)
}

/// Types a browser would run or render as a page. They are always served as
/// downloads of `application/octet-stream`.
pub(crate) fn served_content_type(row: &FileRow) -> &str {
    let content_type = row.content_type.as_str();
    let active = content_type.contains("html")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("ecmascript")
        || content_type == "application/pdf"
        || content_type.starts_with("multipart/");
    if active {
        "application/octet-stream"
    } else {
        content_type
    }
}

// ---------------------------------------------------------------------------
// Attaching

/// Attaches the sender's unsent uploads in this conversation to a new
/// message, in the order given. Any ID that isn't one of them refuses the
/// whole message.
pub(crate) async fn attach_to_message(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    message_id: Uuid,
    sender: Uuid,
    file_ids: &[Uuid],
) -> Result<Vec<FileView>, AppError> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }
    check_attachment_ids(file_ids)?;
    let rows = sqlx::query_as::<_, FileRow>(&format!(
        r#"
        UPDATE files AS file
        SET message_id = $1, attached_at = now(), position = chosen.position - 1
        FROM unnest($2::uuid[]) WITH ORDINALITY AS chosen(id, position)
        WHERE file.id = chosen.id
          AND file.conversation_id = $3
          AND file.uploaded_by_actor_id = $4
          AND file.attached_at IS NULL
          AND file.removed_at IS NULL
        RETURNING {FILE_COLUMNS}
        "#
    ))
    .bind(message_id)
    .bind(file_ids)
    .bind(conversation_id)
    .bind(sender)
    .fetch_all(&mut **transaction)
    .await?;
    in_order(rows, file_ids)
}

/// Attaches the author's unsent uploads on this task to a new comment.
pub(crate) async fn attach_to_comment(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
    author: Uuid,
    file_ids: &[Uuid],
) -> Result<Vec<FileView>, AppError> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }
    check_attachment_ids(file_ids)?;
    let rows = sqlx::query_as::<_, FileRow>(&format!(
        r#"
        UPDATE files AS file
        SET comment_id = $1, attached_at = now(), position = chosen.position - 1
        FROM unnest($2::uuid[]) WITH ORDINALITY AS chosen(id, position)
        WHERE file.id = chosen.id
          AND file.project_id = $3
          AND file.work_item_id = $4
          AND file.uploaded_by_actor_id = $5
          AND file.attached_at IS NULL
          AND file.removed_at IS NULL
        RETURNING {FILE_COLUMNS}
        "#
    ))
    .bind(comment_id)
    .bind(file_ids)
    .bind(project_id)
    .bind(work_item_id)
    .bind(author)
    .fetch_all(&mut **transaction)
    .await?;
    in_order(rows, file_ids)
}

fn check_attachment_ids(file_ids: &[Uuid]) -> Result<(), AppError> {
    let mut unique = file_ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if file_ids.len() > MAX_ATTACHMENTS || unique.len() != file_ids.len() {
        return Err(AppError::Validation("attachments"));
    }
    Ok(())
}

fn in_order(rows: Vec<FileRow>, file_ids: &[Uuid]) -> Result<Vec<FileView>, AppError> {
    if rows.len() != file_ids.len() {
        return Err(AppError::Validation("attachments"));
    }
    let mut views = rows.iter().map(FileRow::view).collect::<Vec<_>>();
    views.sort_by_key(|view| file_ids.iter().position(|id| *id == view.id));
    Ok(views)
}

/// A removed message's files stop showing at once (and its link previews
/// are forgotten); their bytes are deleted by the next cleanup.
pub(crate) async fn forget_message_files(
    transaction: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
    removed_by: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE files SET removed_at = now(), removed_by_actor_id = $2
        WHERE message_id = $1 AND removed_at IS NULL
        "#,
    )
    .bind(message_id)
    .bind(removed_by)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM message_link_previews WHERE message_id = $1")
        .bind(message_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// Removes a file: an unsent upload by its uploader, or a task's own file by
/// its uploader or anyone who may edit the task (checked by the caller,
/// `may_edit_task`). Message and comment files go with their message or
/// comment instead.
pub(crate) async fn remove_file(
    transaction: &mut Transaction<'_, Postgres>,
    row: &FileRow,
    actor: Uuid,
    may_edit_task: bool,
) -> Result<(), AppError> {
    let allowed = if row.attached_at.is_none() {
        row.uploaded_by_actor_id == actor
    } else {
        row.work_item_id.is_some()
            && row.comment_id.is_none()
            && (row.uploaded_by_actor_id == actor || may_edit_task)
    };
    if !allowed {
        return Err(AppError::Forbidden);
    }
    sqlx::query(
        "UPDATE files SET removed_at = now(), removed_by_actor_id = $2 WHERE id = $1 AND removed_at IS NULL",
    )
    .bind(row.id)
    .bind(actor)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cleanup

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(CLEANUP_KIND, |context| async move {
        cleanup(&context.state).await?;
        Ok(())
    });
    registry.every(CLEANUP_KIND, std::time::Duration::from_secs(30 * 60));
}

/// Removes uploads nobody sent within a day, then deletes the bytes of
/// removed files (and link preview images nothing shows any more).
pub async fn cleanup(state: &AppState) -> Result<u64, AppError> {
    sqlx::query(
        r#"
        UPDATE files SET removed_at = now(), removed_by_actor_id = uploaded_by_actor_id
        WHERE attached_at IS NULL AND removed_at IS NULL
          AND created_at < now() - make_interval(hours => $1)
        "#,
    )
    .bind(UNSENT_HOURS)
    .execute(&state.pool)
    .await?;
    let removed = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        r#"
        SELECT id, storage_key, thumbnail_key FROM files
        WHERE removed_at IS NOT NULL AND blob_deleted_at IS NULL
        ORDER BY removed_at
        LIMIT $1
        "#,
    )
    .bind(CLEANUP_BATCH)
    .fetch_all(&state.pool)
    .await?;
    let store = FileStore::from_config(&state.config);
    let mut deleted = 0;
    for (id, key, thumbnail_key) in removed {
        store.delete(&key).await?;
        if let Some(key) = thumbnail_key {
            store.delete(&key).await?;
        }
        sqlx::query("UPDATE files SET blob_deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&state.pool)
            .await?;
        deleted += 1;
    }
    deleted += crate::link_previews::cleanup(state, &store).await?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_folders_and_control_characters() {
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\Users\\me\\report.pdf"), "report.pdf");
        assert_eq!(clean_name("a\u{0}b\nc.txt"), "abc.txt");
        assert_eq!(clean_name("   "), "file");
        assert_eq!(clean_name(".hidden"), "hidden");
        assert_eq!(clean_name(&"x".repeat(300)).chars().count(), 255);
    }

    #[test]
    fn content_types_come_from_the_sender_or_the_name() {
        assert_eq!(
            file_content_type("notes.txt", Some("text/markdown; charset=utf-8"), b"# hi"),
            "text/markdown"
        );
        assert_eq!(
            file_content_type("report.pdf", Some("application/octet-stream"), b"%PDF"),
            "application/pdf"
        );
        assert_eq!(file_content_type("run", None, b"plain words"), "text/plain");
        assert_eq!(
            file_content_type("blob", Some("not a type"), &[0, 1, 2]),
            "application/octet-stream"
        );
    }

    #[test]
    fn active_types_are_served_as_downloads() {
        let row = |content_type: &str| FileRow {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            work_item_id: None,
            comment_id: None,
            uploaded_by_actor_id: Uuid::nil(),
            name: "x".to_owned(),
            content_type: content_type.to_owned(),
            byte_size: 0,
            storage_key: "a/b".to_owned(),
            image_width: None,
            image_height: None,
            thumbnail_key: None,
            attached_at: None,
            created_at: String::new(),
        };
        for active in [
            "text/html",
            "image/svg+xml",
            "application/xhtml+xml",
            "text/javascript",
        ] {
            assert_eq!(
                served_content_type(&row(active)),
                "application/octet-stream"
            );
        }
        assert_eq!(served_content_type(&row("text/plain")), "text/plain");
    }
}
