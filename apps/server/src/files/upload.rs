//! Direct upload (K5.20, ruling R24 rule 2): when the store can sign, a
//! client uploads straight to it and the server never carries the bytes.
//!
//! - `start` checks the destination and the workspace's room for the
//!   declared size, notes the upload (`file_uploads`) and hands out a
//!   signed `PUT` link for 15 minutes, fixed to that size, type and
//!   SHA-256, so the store refuses anything else.
//! - `finish` asks the store what arrived: a wrong size (or checksum) is
//!   refused and the bytes deleted. A picture is read back, loses its
//!   location data and is written as a new object, so only the stripped copy
//!   is ever served; anything else becomes a file where it lies.
//! - Nothing serves an upload's bytes before its finish: no file row names
//!   them. One never finished is swept with its bytes after a day.
//!
//! On disk there is nothing to sign, so `start` answers that the upload goes
//! through the server (the upload route, or `file_upload` for agents).

use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    Destination, FileRow, FileStore, NewFile, Upload, abandon_staged, clean_name, delete_objects,
    file_content_type, hold_room, image, insert_file, organization_of, prepare_upload,
    stage_prepared, store_prepared,
};
use crate::{
    AppState,
    conversations::{MessageCommandActor, lock_message_conversation, require_active_conversation},
    error::AppError,
    policy::{Action, Decision},
    work_item_commands::{CommandActor, authorize_mutation, lock_membership},
};

/// How long a signed upload link lasts.
pub const LINK_TTL: Duration = Duration::from_secs(15 * 60);
/// An upload not finished within this long is swept with its bytes.
pub const UNFINISHED_HOURS: i32 = 24;
/// The largest picture the server reads back to strip (as on the upload
/// route, a picture is re-encoded whole).
pub const IMAGE_BYTES: u64 = 25 * 1024 * 1024;
/// Bytes read to tell a picture from anything else.
const SNIFF_BYTES: u64 = 4096;
/// Unfinished uploads swept per cleanup run.
const SWEEP_BATCH: i64 = 200;

/// Where an upload goes, as the caller named it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Place {
    Conversation(Uuid),
    Task { id: Uuid, for_comment: bool },
}

/// Checks the caller may put a file in `place` now, and returns where it
/// goes and the most it may be. In the caller's transaction; a place the
/// caller can't post in is `not_found`, as on the upload routes.
pub(crate) async fn destination(
    transaction: &mut Transaction<'_, Postgres>,
    message_actor: &MessageCommandActor,
    command_actor: &CommandActor,
    place: Place,
) -> Result<(Destination, usize), AppError> {
    let destination = match place {
        Place::Conversation(conversation_id) => {
            let conversation = lock_message_conversation(
                transaction,
                message_actor,
                conversation_id,
                Action::MessageSend,
            )
            .await?;
            require_active_conversation(&conversation)?;
            Destination::Conversation {
                project_id: conversation.place(),
                conversation_id: conversation.id,
            }
        }
        Place::Task { id, for_comment } => {
            let project_id =
                sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM task_rows WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut **transaction)
                    .await?
                    .ok_or(AppError::NotFound("work_item"))?;
            let membership = lock_membership(transaction, command_actor.actor_id, project_id)
                .await?
                .ok_or(AppError::NotFound("work_item"))?;
            if authorize_mutation(command_actor, &membership, Action::WorkItemEdit, Some(id))?
                != Decision::Allow
            {
                return Err(AppError::NotFound("work_item"));
            }
            if membership.project_archived_at.is_some() {
                return Err(AppError::ProjectArchived);
            }
            let open = sqlx::query_scalar::<_, bool>(
                "SELECT archived_at IS NULL FROM task_rows WHERE project_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(id)
            .fetch_one(&mut **transaction)
            .await?;
            if !open {
                return Err(AppError::Conflict("work_item", "archived"));
            }
            Destination::Task {
                project_id,
                work_item_id: id,
                for_comment,
            }
        }
    };
    let limit = super::upload_limit(transaction, destination.project_id()).await?;
    Ok((destination, limit))
}

/// What an upload declared.
pub(crate) struct Declared {
    pub name: String,
    pub content_type: Option<String>,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// A started upload: where to send the bytes.
pub(crate) struct Started {
    pub upload_id: Uuid,
    pub url: url::Url,
    /// Headers the client sends with the bytes, as they are (the signature
    /// covers them).
    pub headers: Vec<(String, String)>,
    pub expires_at: DateTime<Utc>,
}

/// Why an upload can't go straight to the store.
pub(crate) enum NotDirect {
    /// The store is this server's disk: upload through the server.
    ThroughServer,
    Refused(AppError),
}

impl From<AppError> for NotDirect {
    fn from(error: AppError) -> Self {
        Self::Refused(error)
    }
}

/// Starts a direct upload into `destination`, which the caller checked.
pub(crate) async fn start(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    uploader: Uuid,
    destination: Destination,
    limit: usize,
    declared: Declared,
) -> Result<Started, NotDirect> {
    let name = clean_name(&declared.name);
    let sha256 = parse_sha256(&declared.sha256).ok_or(AppError::Validation("sha256"))?;
    if declared.size == 0 {
        return Err(AppError::Validation("size").into());
    }
    if declared.size > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(AppError::PayloadTooLarge.into());
    }
    let project_id = destination.project_id();
    let organization_id = organization_of(&mut **transaction, project_id).await?;
    let store = FileStore::for_uploads(state, transaction, organization_id).await?;
    hold_room(state, transaction, organization_id, declared.size, 0).await?;
    let content_type = file_content_type(&name, declared.content_type.as_deref(), &[]);
    let id = Uuid::now_v7();
    let storage_key = upload_key(organization_id, id);
    let Some(link) = store.presign_put(
        &storage_key,
        LINK_TTL,
        &content_type,
        declared.size,
        &sha256,
    )?
    else {
        return Err(NotDirect::ThroughServer);
    };
    let (conversation_id, work_item_id, for_comment) = places(destination);
    let expires_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        INSERT INTO file_uploads (
            id, workspace_id, space_id, conversation_id, work_item_id, for_comment,
            uploaded_by_actor_id, name, content_type, byte_size, sha256, storage_key, store_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING created_at + make_interval(secs => $14)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(crate::spaces::space_of(project_id))
    .bind(conversation_id)
    .bind(work_item_id)
    .bind(for_comment)
    .bind(uploader)
    .bind(&name)
    .bind(&content_type)
    .bind(i64::try_from(declared.size).unwrap_or(i64::MAX))
    .bind(hex::encode(sha256))
    .bind(&storage_key)
    .bind(store.id())
    .bind(LINK_TTL.as_secs_f64())
    .fetch_one(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    Ok(Started {
        upload_id: id,
        url: link.url,
        headers: link.headers,
        expires_at,
    })
}

#[derive(Debug, FromRow)]
pub(crate) struct UploadRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// None for a DM or group's file, which is in no space.
    pub space_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    pub for_comment: bool,
    pub uploaded_by_actor_id: Uuid,
    name: String,
    content_type: String,
    byte_size: i64,
    sha256: String,
    storage_key: String,
    store_id: String,
    pub file_id: Option<Uuid>,
}

impl UploadRow {
    /// Where the upload was going, to check again at its finish.
    pub(crate) fn place(&self) -> Place {
        match (self.conversation_id, self.work_item_id) {
            (Some(conversation), _) => Place::Conversation(conversation),
            (None, Some(id)) => Place::Task {
                id,
                for_comment: self.for_comment,
            },
            (None, None) => Place::Conversation(Uuid::nil()),
        }
    }
}

/// The caller's own upload, unfinished within a day or finished.
pub(crate) async fn upload_of(
    connection: &mut PgConnection,
    uploader: Uuid,
    upload_id: Uuid,
) -> Result<UploadRow, AppError> {
    sqlx::query_as::<_, UploadRow>(
        r#"
        SELECT id, workspace_id, space_id, conversation_id, work_item_id, for_comment,
               uploaded_by_actor_id, name, content_type, byte_size, sha256, storage_key,
               store_id, file_id
        FROM file_uploads
        WHERE id = $1 AND uploaded_by_actor_id = $2
          AND (finished_at IS NOT NULL
               OR created_at > now() - make_interval(hours => $3))
        "#,
    )
    .bind(upload_id)
    .bind(uploader)
    .bind(UNFINISHED_HOURS)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("upload"))
}

/// Why a finish was refused.
pub(crate) enum Unfinished {
    /// Nothing is stored yet: the client should send the bytes first.
    NotArrived,
    /// What arrived isn't what was declared; it was deleted.
    Mismatch(&'static str),
    /// A picture too large to strip, or unreadable; it was deleted.
    Picture(AppError),
    Failed(AppError),
}

impl From<AppError> for Unfinished {
    fn from(error: AppError) -> Self {
        Self::Failed(error)
    }
}

/// Makes a file of what arrived for `upload`, whose destination the caller
/// checked again (`destination`). A finished upload answers with its file.
pub(crate) async fn finish(
    state: &AppState,
    upload: &UploadRow,
    destination: Destination,
) -> Result<Uuid, Unfinished> {
    if let Some(file_id) = upload.file_id {
        return Ok(file_id);
    }
    let store = FileStore::resolve(state, &upload.store_id).await?;
    let Some(head) = store.head(&upload.storage_key).await? else {
        return Err(Unfinished::NotArrived);
    };
    let declared = u64::try_from(upload.byte_size).unwrap_or(0);
    let checksum = hex::decode(&upload.sha256)
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_default();
    let mismatch = if head.length != declared {
        Some("size")
    } else if head
        .sha256
        .as_ref()
        .is_some_and(|stored| *stored != checksum)
    {
        Some("sha256")
    } else {
        None
    };
    if let Some(field) = mismatch {
        abandon(state, &store, upload).await;
        return Err(Unfinished::Mismatch(field));
    }
    let start = store.read_start(&upload.storage_key, SNIFF_BYTES).await?;
    if image::sniff(&start).is_some() {
        return finish_picture(state, &store, upload, destination, declared).await;
    }
    let content_type = file_content_type(&upload.name, Some(&upload.content_type), &start);
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(AppError::from)?;
    if let Some(file_id) = claim(&mut transaction, upload.id).await? {
        return Ok(file_id);
    }
    hold_room(
        state,
        &mut transaction,
        upload.workspace_id,
        declared,
        declared,
    )
    .await?;
    let row = insert_file(
        &mut transaction,
        &NewFile {
            id: upload.id,
            organization_id: upload.workspace_id,
            project_id: crate::spaces::place(upload.space_id),
            destination,
            uploader: upload.uploaded_by_actor_id,
            name: &upload.name,
            content_type: &content_type,
            byte_size: declared,
            sha256: &upload.sha256,
            storage_key: &upload.storage_key,
            store_id: &upload.store_id,
            dimensions: None,
            thumbnail_key: None,
            thumbnail_bytes: None,
            preview: None,
        },
    )
    .await?;
    finished(&mut transaction, upload.id, row.id).await?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok(row.id)
}

/// A picture is read back, re-encoded without where it was taken, and
/// stored as a new object; the upload's own bytes are deleted.
async fn finish_picture(
    state: &AppState,
    store: &FileStore,
    upload: &UploadRow,
    destination: Destination,
    declared: u64,
) -> Result<Uuid, Unfinished> {
    if declared > IMAGE_BYTES {
        abandon(state, store, upload).await;
        return Err(Unfinished::Picture(AppError::PayloadTooLarge));
    }
    let bytes = store.read_all(&upload.storage_key, IMAGE_BYTES).await?;
    let prepared = match prepare_upload(Upload {
        name: upload.name.clone(),
        declared_type: Some(upload.content_type.clone()),
        bytes,
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            abandon(state, store, upload).await;
            return Err(Unfinished::Picture(error));
        }
    };
    // The stripped picture goes into the store before the transaction that
    // records it opens.
    let staged = stage_prepared(state, destination.project_id(), prepared).await?;
    let claimed = async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let claimed = claim(&mut transaction, upload.id).await?;
        Ok::<_, AppError>((transaction, claimed))
    }
    .await;
    let mut transaction = match claimed {
        Ok((transaction, Some(file_id))) => {
            drop(transaction);
            abandon_staged(state, &staged).await;
            return Ok(file_id);
        }
        Ok((transaction, None)) => transaction,
        Err(error) => {
            abandon_staged(state, &staged).await;
            return Err(error.into());
        }
    };
    let row = store_prepared(
        state,
        &mut transaction,
        upload.uploaded_by_actor_id,
        destination,
        staged,
    )
    .await?;
    let committed = async {
        finished(&mut transaction, upload.id, row.id).await?;
        transaction.commit().await.map_err(AppError::from)
    }
    .await;
    if let Err(error) = committed {
        super::discard_stored(state, &row).await;
        return Err(error.into());
    }
    delete_objects(store, &[upload.storage_key.as_str()]).await;
    Ok(row.id)
}

/// Locks the upload row; the file it already made, if a finish beat this one.
async fn claim(
    transaction: &mut Transaction<'_, Postgres>,
    upload: Uuid,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT file_id FROM file_uploads WHERE id = $1 FOR UPDATE",
    )
    .bind(upload)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("upload"))
}

async fn finished(
    transaction: &mut Transaction<'_, Postgres>,
    upload: Uuid,
    file: Uuid,
) -> Result<(), AppError> {
    sqlx::query("UPDATE file_uploads SET file_id = $2, finished_at = now() WHERE id = $1")
        .bind(upload)
        .bind(file)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// Deletes a refused upload's bytes and the upload itself.
async fn abandon(state: &AppState, store: &FileStore, upload: &UploadRow) {
    if store.delete(&upload.storage_key).await.is_ok() {
        let _ = sqlx::query("DELETE FROM file_uploads WHERE id = $1 AND file_id IS NULL")
            .bind(upload.id)
            .execute(crate::db::scoped(&state.pool))
            .await;
    }
}

/// Deletes uploads nobody finished within a day, and their bytes. One whose
/// bytes can't be deleted stays for the next run.
pub(crate) async fn sweep(state: &AppState) -> Result<u64, AppError> {
    let due = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT id, storage_key, store_id FROM file_uploads
        WHERE finished_at IS NULL AND created_at < now() - make_interval(hours => $1)
        ORDER BY created_at
        LIMIT $2
        "#,
    )
    .bind(UNFINISHED_HOURS)
    .bind(SWEEP_BATCH)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let mut swept = 0;
    for (id, key, store_id) in due {
        let Ok(store) = FileStore::resolve(state, &store_id).await else {
            continue;
        };
        if store.delete(&key).await.is_err() {
            continue;
        }
        sqlx::query("DELETE FROM file_uploads WHERE id = $1 AND finished_at IS NULL")
            .bind(id)
            .execute(crate::db::scoped(&state.pool))
            .await?;
        swept += 1;
    }
    Ok(swept)
}

fn places(destination: Destination) -> (Option<Uuid>, Option<Uuid>, bool) {
    match destination {
        Destination::Conversation {
            conversation_id, ..
        } => (Some(conversation_id), None, false),
        Destination::Task {
            work_item_id,
            for_comment,
            ..
        } => (None, Some(work_item_id), for_comment),
        // Resumable uploads go only to messages and tasks (`Place`); a row
        // with neither finds no place at its finish and is refused.
        Destination::Thing { .. } => (None, None, false),
    }
}

/// An upload's bytes: where a file of the upload's own id keeps them, so
/// anything but a picture becomes a file where it lies. Nothing serves the
/// key before the finish records that file.
fn upload_key(organization_id: Uuid, id: Uuid) -> String {
    super::storage_key(organization_id, id)
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    let value = value.trim().to_ascii_lowercase();
    let bytes = hex::decode(value).ok()?;
    bytes.try_into().ok()
}

/// The file a finished upload made, as its uploader reads it.
pub(crate) async fn file_of(
    state: &AppState,
    reader: super::Reader,
    file_id: Uuid,
) -> Result<FileRow, AppError> {
    super::readable_file(&state.pool, reader, file_id).await
}
