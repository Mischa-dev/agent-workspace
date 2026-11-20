//! W20: files and images on messages, tasks and task comments; C1.33:
//! files of any other thing, such as a picture pasted into a page.
//!
//! A file is uploaded into one place, a conversation or a task, by someone
//! who may post there. A message's files wait, seen only by their uploader,
//! until the message is sent with their IDs (`attach_to_message`). A task
//! comment is a message (C1.4): its files are uploaded to the task and move
//! to the comment's conversation when it is posted
//! (`rehome_comment_uploads`). Files uploaded to a task itself are attached
//! at once. Unsent uploads are cleaned up after a day.
//!
//! Reading a file checks its place every time (`readable_file`): a message's
//! file needs the project, `conversation_visible_to` and the message not
//! removed; a task's file needs the project; another thing's file needs
//! `subject_visible_to` (C1.33). File URLs carry no secret: the
//! browser's session (or the agent's credential, over MCP) is checked on
//! every request, so a link copied out of the app opens for nobody else.
//!
//! The bytes live in a [`FileStore`]; images lose their location data and
//! get a preview first (`image`). Size caps are workspace settings
//! (`files.max_upload_mb`, `files.workspace_quota_mb`), and every upload
//! passes storage admission, counting the files the database knows about.

pub mod http;
pub mod image;
pub mod link;
pub mod links;
pub mod migrate;
pub mod object;
pub mod ops;
pub mod store;
pub mod upload;
pub mod workspace_store;

use std::collections::BTreeMap;

use rmcp::schemars::{self, JsonSchema};
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
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    /// Set for images that show inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// The task comment it belongs to, in a task's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub comment_id: Option<Uuid>,
    /// The start of a text or code file, to show before it is opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[schemars(with = "String")]
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
    pub(crate) preview_text: Option<String>,
    pub(crate) attached_at: Option<String>,
    pub(crate) created_at: String,
    /// Which store holds the bytes (`store::LOCAL`, `store::OBJECT`).
    pub(crate) store_id: String,
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
            preview: self.preview_text.clone(),
            uploaded_by: self.uploaded_by_actor_id,
            created_at: self.created_at.clone(),
        }
    }

    pub(crate) fn is_inline_image(&self) -> bool {
        self.image_width.is_some()
    }
}

const FILE_COLUMNS: &str = r#"
    file.id, COALESCE(file.project_id, direct_scope_id()) AS project_id,
    file.work_item_id, NULL::uuid AS comment_id,
    file.uploaded_by_actor_id, file.name, file.content_type, file.byte_size,
    file.storage_key, file.image_width, file.image_height, file.thumbnail_key,
    file.preview_text,
    file.attached_at::text AS attached_at, file.created_at::text AS created_at,
    file.store_id
"#;

/// Who is reading: a browser session, or an MCP caller (an agent through its
/// connection, or a person through an app they connected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    -- A DM's or group's file (no space) by the conversation's rule alone.
    AND CASE WHEN file.project_id IS NULL
             THEN file.conversation_id IS NOT NULL
                  AND ($3::uuid IS NULL OR agent_connection_can_reach_direct($3))
        WHEN $4 THEN actor_can_access_project($2, $3, file.project_id)
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
        -- Another thing's file: whoever can see the thing (C1.33).
        WHEN file.thing_id IS NOT NULL THEN
            subject_visible_to(file.thing_kind, file.thing_id, $2)
        -- A task's file (its own or a comment's): only while the reader's
        -- role there still lets them see tasks. An app's agent limit is
        -- checked in Rust (`hidden_by_agent_limit`).
        ELSE EXISTS (
            SELECT 1 FROM project_memberships task_membership
            WHERE task_membership.project_id = file.project_id
              AND task_membership.actor_id = $2
              AND task_membership.enabled
              AND role_allows(task_membership.role, task_membership.role_id, 'tasks.view')
        )
    END
"#;

impl Reader {
    fn actor_id(self) -> Uuid {
        match self {
            Self::Browser { actor_id } | Self::Mcp { actor_id, .. } => actor_id,
        }
    }
}

/// Whether the agent limit of an app acting for the reader keeps them from
/// seeing tasks, and so this task file, in its project.
fn hidden_by_agent_limit(reader: Reader, row: &FileRow) -> bool {
    row.work_item_id.is_some()
        && crate::policy::current_agent_limit(
            reader.actor_id(),
            crate::policy::MembershipScope::Project(row.project_id),
        )
        .is_some_and(|limit| !limit.contains(crate::policy::Permission::TasksView))
}

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
    let row = bind_reader(sqlx::query_as::<_, FileRow>(&sql).bind(file_id), reader)
        .fetch_optional(crate::db::scoped(pool))
        .await?
        .filter(|row| !hidden_by_agent_limit(reader, row))
        .ok_or(AppError::NotFound("file"))?;
    // A file in a task's discussion (a comment's, C1.4) keeps the task
    // limit its message has: hidden while the reader, or an app acting for
    // them, can't see tasks there.
    let mut connection = crate::db::scoped_tx(pool).await?;
    let conversation =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT conversation_id FROM files WHERE id = $1")
            .bind(row.id)
            .fetch_one(&mut *connection)
            .await?;
    if let Some(conversation) = conversation
        && direct_beyond_limit(&mut connection, reader.actor_id(), conversation).await?
    {
        return Err(AppError::NotFound("file"));
    }
    if let Some(conversation) = conversation
        && crate::conversations::task_discussion_hidden(
            &mut connection,
            reader.actor_id(),
            conversation,
        )
        .await?
    {
        return Err(AppError::NotFound("file"));
    }
    Ok(row)
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
    let rows = bind_reader(sqlx::query_as::<_, FileRow>(&sql).bind(message_id), reader)
        .fetch_all(crate::db::scoped(pool))
        .await?;
    if rows
        .iter()
        .any(|row| row.project_id == crate::spaces::DIRECT)
    {
        let mut connection = crate::db::scoped_tx(pool).await?;
        let conversation = sqlx::query_scalar::<_, Uuid>(
            "SELECT conversation_id FROM messages \
             WHERE id = $1 AND conversation_visible_to(conversation_id, $2)",
        )
        .bind(message_id)
        .bind(reader.actor_id())
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(conversation) = conversation
            && direct_beyond_limit(&mut connection, reader.actor_id(), conversation).await?
        {
            return Ok(Vec::new());
        }
    }
    Ok(rows)
}

/// Whether `conversation` is a kept DM or group in a workspace the
/// credential `actor` calls with doesn't reach (a token made for another
/// workspace). Channels are checked by their space.
async fn direct_beyond_limit(
    connection: &mut sqlx::PgConnection,
    actor: Uuid,
    conversation: Uuid,
) -> Result<bool, AppError> {
    let Some(allowed) = crate::policy::agent_limit_direct_workspaces(
        actor,
        crate::policy::Permission::ProjectsView,
    ) else {
        return Ok(false);
    };
    let workspace = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT workspace_id FROM conversations
        WHERE id = $1 AND project_id IS NULL AND conversation_visible_to(id, $2)
        "#,
    )
    .bind(conversation)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(workspace.is_some_and(|workspace| !allowed.contains(&workspace)))
}

/// A task's files the reader may see (its own and its comments'), oldest
/// first. The caller has checked the reader may see the task.
pub(crate) async fn task_files(
    pool: &PgPool,
    reader: Reader,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<FileRow>, AppError> {
    // A comment's files are its message's, in the task's comments.
    let columns = FILE_COLUMNS.replace("NULL::uuid AS comment_id", "file.message_id AS comment_id");
    let sql = format!(
        "SELECT {columns} FROM files file
         WHERE file.project_id = $6
           AND (file.work_item_id = $1 OR file.conversation_id = (
               -- visibility: READABLE checks each comment file's conversation
               SELECT place.id FROM conversations place
               WHERE place.kind = 'comments' AND place.subject_kind = 'task'
                 AND place.subject_id = $1))
           AND file.attached_at IS NOT NULL AND {READABLE}
         ORDER BY file.created_at, file.position, file.id"
    );
    Ok(bind_reader(
        sqlx::query_as::<_, FileRow>(&sql).bind(work_item_id),
        reader,
    )
    .bind(project_id)
    .fetch_all(crate::db::scoped(pool))
    .await?
    .into_iter()
    .filter(|row| !hidden_by_agent_limit(reader, row))
    .collect())
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
    /// Another thing's own file, such as a picture in a page (C1.33),
    /// attached at once. The caller has checked the uploader may edit it.
    Thing {
        project_id: Uuid,
        thing: ThingKind,
        thing_id: Uuid,
        purpose: Purpose,
    },
}

impl Destination {
    pub(crate) fn project_id(self) -> Uuid {
        match self {
            Self::Conversation { project_id, .. }
            | Self::Task { project_id, .. }
            | Self::Thing { project_id, .. } => project_id,
        }
    }
}

/// The things other than conversations and tasks that keep files of their
/// own. Board images move here with C1.29.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThingKind {
    Page,
    /// A collection record: its file fields hold its files (T1.20).
    Record,
}

impl ThingKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "page" => Some(Self::Page),
            "record" => Some(Self::Record),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Record => "record",
        }
    }

    /// What uploading to one takes: editing it.
    pub(crate) fn edit_action(self) -> crate::policy::Action {
        match self {
            Self::Page => crate::policy::Action::PageEdit,
            Self::Record => crate::policy::Action::CollectionRecordEdit,
        }
    }
}

/// What a thing's file is for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// Kept with the thing, such as a file added to a page.
    Attachment,
    /// Shown in place, such as a picture in a page.
    Inline,
}

impl Purpose {
    fn name(self) -> &'static str {
        match self {
            Self::Attachment => "attachment",
            Self::Inline => "inline",
        }
    }
}

/// Where an upload to a thing goes, once `editor` may edit it: its space,
/// and the thing still open. A thing the editor can't edit or see is
/// NotFound, so its existence isn't told.
pub(crate) async fn thing_destination(
    pool: &PgPool,
    editor: Uuid,
    thing: ThingKind,
    thing_id: Uuid,
) -> Result<Destination, AppError> {
    match thing {
        ThingKind::Page => {
            let project_id = crate::memory::page_project(pool, thing_id, editor).await?;
            crate::memory::project_access(
                pool,
                editor,
                project_id,
                crate::policy::Action::PageEdit,
            )
            .await
            .map_err(|_| AppError::NotFound("page"))?;
            let open = sqlx::query_scalar::<_, bool>(
                "SELECT archived_at IS NULL FROM project_pages WHERE id = $1",
            )
            .bind(thing_id)
            .fetch_optional(crate::db::scoped(pool))
            .await?
            .ok_or(AppError::NotFound("page"))?;
            if !open {
                return Err(AppError::Conflict("page", "archived"));
            }
            Ok(Destination::Thing {
                project_id,
                thing,
                thing_id,
                purpose: Purpose::Inline,
            })
        }
        ThingKind::Record => {
            let (project_id, archived) = sqlx::query_as::<_, (Uuid, bool)>(
                "SELECT project_id, archived_at IS NOT NULL FROM collection_records WHERE id = $1",
            )
            .bind(thing_id)
            .fetch_optional(crate::db::scoped(pool))
            .await?
            .ok_or(AppError::NotFound("record"))?;
            crate::memory::project_access(
                pool,
                editor,
                project_id,
                crate::policy::Action::CollectionRecordEdit,
            )
            .await
            .map_err(|_| AppError::NotFound("record"))?;
            // A record of a collection that acts like tasks also wants
            // seeing tasks there.
            let visible =
                sqlx::query_scalar::<_, bool>("SELECT subject_visible_to('record', $1, $2)")
                    .bind(thing_id)
                    .bind(editor)
                    .fetch_one(crate::db::scoped(pool))
                    .await?;
            if !visible {
                return Err(AppError::NotFound("record"));
            }
            if archived {
                return Err(AppError::Conflict("record", "archived"));
            }
            Ok(Destination::Thing {
                project_id,
                thing,
                thing_id,
                purpose: Purpose::Attachment,
            })
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
        &mut *connection,
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
    // The plan's largest file, when it is smaller.
    let plan = crate::metering::largest(connection, organization_id, "file_bytes")
        .await?
        .and_then(|bytes| usize::try_from(bytes.max(0)).ok())
        .unwrap_or(usize::MAX);
    Ok(usize::try_from(megabytes)
        .unwrap_or(25)
        .saturating_mul(1024 * 1024)
        .min(MAX_UPLOAD_BYTES)
        .min(plan))
}

pub(crate) async fn organization_of(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    // A place: a space's workspace, or the direct scope for a DM's file.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM projects WHERE id = $1 \
         UNION ALL SELECT id FROM organizations WHERE id = $1 AND id = direct_scope_id()",
    )
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
    let prepared = prepare_upload(upload).await?;
    let staged = stage_prepared(state, destination.project_id(), prepared).await?;
    let mut transaction = match crate::db::scoped_tx(&state.pool).await {
        Ok(transaction) => transaction,
        Err(error) => {
            abandon_staged(state, &staged).await;
            return Err(error.into());
        }
    };
    let row = store_prepared(state, &mut transaction, uploader, destination, staged).await?;
    if let Err(error) = transaction.commit().await {
        discard_stored(state, &row).await;
        return Err(error.into());
    }
    Ok(row)
}

/// An upload ready to store: a picture re-encoded without where it was
/// taken, its type and size worked out. Made before any transaction opens.
pub(crate) struct PreparedUpload {
    name: String,
    bytes: Vec<u8>,
    content_type: String,
    dimensions: Option<(u32, u32)>,
    thumbnail: Option<Vec<u8>>,
    preview: Option<String>,
}

pub(crate) async fn prepare_upload(upload: Upload) -> Result<PreparedUpload, AppError> {
    let name = clean_name(&upload.name);
    let declared = upload.declared_type;
    let processed = if image::sniff(&upload.bytes).is_some() {
        let bytes = upload.bytes;
        let processed = image::one_at_a_time(move || image::process(bytes))
            .await
            .ok_or(AppError::Internal("process an uploaded image"))?;
        // A picture we can't read could still carry where it was taken, so
        // it isn't kept at all rather than kept as it came.
        Ok(processed.ok_or(AppError::Refused {
            field: "file",
            message: "This picture couldn't be read well enough to remove its location data, so it wasn't saved. Save it again as a PNG or JPEG and attach that.",
        })?)
    } else {
        Err(upload.bytes)
    };
    Ok(match processed {
        Ok(image) => PreparedUpload {
            name,
            bytes: image.bytes,
            content_type: image.format.content_type().to_owned(),
            dimensions: Some((image.width, image.height)),
            thumbnail: image.thumbnail.map(|thumbnail| thumbnail.bytes),
            preview: None,
        },
        Err(bytes) => {
            let content_type = file_content_type(&name, declared.as_deref(), &bytes);
            let preview = text_preview(&content_type, &bytes);
            PreparedUpload {
                name,
                bytes,
                content_type,
                dimensions: None,
                thumbnail: None,
                preview,
            }
        }
    })
}

/// A prepared upload's bytes, already in the workspace's store, waiting for
/// [`store_prepared`] to give them a row. Written before any transaction
/// opens (the store may be a bucket across the network), and noted as
/// pending so the sweep takes them away if no row ever claims them.
pub(crate) struct StagedUpload {
    id: Uuid,
    organization_id: Uuid,
    name: String,
    content_type: String,
    dimensions: Option<(u32, u32)>,
    byte_size: u64,
    sha256: String,
    thumbnail_bytes: Option<u64>,
    storage_key: String,
    thumbnail_key: Option<String>,
    store: FileStore,
    preview: Option<String>,
}

impl StagedUpload {
    fn incoming(&self) -> u64 {
        self.byte_size
            .saturating_add(self.thumbnail_bytes.unwrap_or(0))
    }

    fn keys(&self) -> Vec<&str> {
        std::iter::once(self.storage_key.as_str())
            .chain(self.thumbnail_key.as_deref())
            .collect()
    }
}

/// Writes a prepared upload's bytes for a file in `project_id`, outside any
/// transaction: a full workspace or disk refuses first. Pair it with
/// [`store_prepared`], or [`abandon_staged`] when no row will follow.
pub(crate) async fn stage_prepared(
    state: &AppState,
    project_id: Uuid,
    prepared: PreparedUpload,
) -> Result<StagedUpload, AppError> {
    let organization_id = organization_of(crate::db::scoped(&state.pool), project_id).await?;
    let PreparedUpload {
        name,
        bytes,
        content_type,
        dimensions,
        thumbnail,
        preview,
    } = prepared;
    let byte_size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let thumbnail_bytes = thumbnail
        .as_ref()
        .map(|thumbnail| u64::try_from(thumbnail.len()).unwrap_or(u64::MAX));
    let store = {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        require_room(
            state,
            &mut transaction,
            organization_id,
            byte_size.saturating_add(thumbnail_bytes.unwrap_or(0)),
            0,
        )
        .await?;
        FileStore::for_uploads(state, &mut transaction, organization_id).await?
    };
    let id = Uuid::now_v7();
    let storage_key = storage_key(organization_id, id);
    let thumbnail_key = thumbnail.as_ref().map(|_| format!("{storage_key}.thumb"));
    let staged = StagedUpload {
        id,
        organization_id,
        name,
        content_type,
        dimensions,
        byte_size,
        sha256: hex::encode(Sha256::digest(&bytes)),
        thumbnail_bytes,
        storage_key,
        thumbnail_key,
        store,
        preview,
    };
    begin_objects(state, &staged.store, organization_id, &staged.keys()).await?;
    let mut put = staged.store.put(&staged.storage_key, &bytes).await;
    if let (Ok(()), Some(key), Some(thumbnail)) = (&put, &staged.thumbnail_key, &thumbnail) {
        put = staged.store.put(key, thumbnail).await;
    }
    if let Err(error) = put {
        abandon_staged(state, &staged).await;
        return Err(error);
    }
    Ok(staged)
}

/// Takes away staged bytes no row will claim.
pub(crate) async fn abandon_staged(state: &AppState, staged: &StagedUpload) {
    drop_objects(state, &staged.store, &staged.keys()).await;
}

/// Records a staged upload in `transaction`, which the caller commits (with
/// anything else that belongs with it, such as a retry key). If that commit
/// fails, [`discard_stored`] takes the stored bytes away again.
pub(crate) async fn store_prepared(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    uploader: Uuid,
    destination: Destination,
    staged: StagedUpload,
) -> Result<FileRow, AppError> {
    let project_id = destination.project_id();
    let incoming = staged.incoming();
    let recorded = async {
        if organization_of(&mut **transaction, project_id).await? != staged.organization_id {
            return Err(AppError::NotFound("project"));
        }
        hold_room(
            state,
            transaction,
            staged.organization_id,
            incoming,
            incoming,
        )
        .await?;
        insert_file(
            transaction,
            &NewFile {
                id: staged.id,
                organization_id: staged.organization_id,
                project_id,
                destination,
                uploader,
                name: &staged.name,
                content_type: &staged.content_type,
                byte_size: staged.byte_size,
                sha256: &staged.sha256,
                storage_key: &staged.storage_key,
                store_id: staged.store.id(),
                dimensions: staged.dimensions,
                thumbnail_key: staged.thumbnail_key.as_deref(),
                thumbnail_bytes: staged.thumbnail_bytes,
                preview: staged.preview.as_deref(),
            },
        )
        .await
    }
    .await;
    if recorded.is_err() {
        abandon_staged(state, &staged).await;
    }
    recorded
}

/// Stores an upload as its body arrives (K5.11). A picture is read whole,
/// since it is re-encoded without its location data; anything else streams
/// into the store, never held whole, before any transaction opens, and is
/// counted against the workspace's room once its size is known. A refused
/// upload's bytes are deleted again.
pub(crate) async fn store_body(
    state: &AppState,
    uploader: Uuid,
    destination: Destination,
    name: String,
    declared_type: Option<String>,
    body: axum::body::Body,
    limit: usize,
) -> Result<FileRow, AppError> {
    use futures_util::StreamExt;

    let mut chunks = body.into_data_stream();
    let mut head = Vec::new();
    while head.len() < SNIFF_BYTES {
        match chunks.next().await {
            Some(Ok(chunk)) => head.extend_from_slice(&chunk),
            Some(Err(_)) => return Err(AppError::Validation("file")),
            None => break,
        }
        if head.len() > limit {
            return Err(AppError::PayloadTooLarge);
        }
    }
    if head.is_empty() {
        return Err(AppError::Validation("file"));
    }
    if image::sniff(&head).is_some() {
        let mut bytes = head;
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| AppError::Validation("file"))?;
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(AppError::PayloadTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        return store_upload(
            state,
            uploader,
            destination,
            Upload {
                name,
                declared_type,
                bytes,
            },
        )
        .await;
    }

    let name = clean_name(&name);
    let content_type = file_content_type(&name, declared_type.as_deref(), &head);
    // A text file's preview comes from the bytes read so far.
    let preview = text_preview(&content_type, &head);
    let project_id = destination.project_id();
    let organization_id = organization_of(crate::db::scoped(&state.pool), project_id).await?;
    // A full workspace or disk refuses before a byte is written.
    let store = {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        require_room(
            state,
            &mut transaction,
            organization_id,
            u64::try_from(head.len()).unwrap_or(u64::MAX),
            0,
        )
        .await?;
        FileStore::for_uploads(state, &mut transaction, organization_id).await?
    };
    let id = Uuid::now_v7();
    let storage_key = storage_key(organization_id, id);
    begin_objects(state, &store, organization_id, &[&storage_key]).await?;
    let first = futures_util::stream::iter([Ok::<_, axum::Error>(axum::body::Bytes::from(head))]);
    let written = match store
        .put_stream(
            &storage_key,
            first.chain(chunks),
            u64::try_from(limit).unwrap_or(u64::MAX),
        )
        .await
    {
        Ok(written) => written,
        Err(error) => {
            drop_objects(state, &store, &[&storage_key]).await;
            return Err(error);
        }
    };
    let stored = async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        hold_room(
            state,
            &mut transaction,
            organization_id,
            written.length,
            written.length,
        )
        .await?;
        let row = insert_file(
            &mut transaction,
            &NewFile {
                id,
                organization_id,
                project_id,
                destination,
                uploader,
                name: &name,
                content_type: &content_type,
                byte_size: written.length,
                sha256: &written.sha256,
                storage_key: &storage_key,
                store_id: store.id(),
                dimensions: None,
                thumbnail_key: None,
                thumbnail_bytes: None,
                preview: preview.as_deref(),
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(row)
    }
    .await;
    if stored.is_err() {
        drop_objects(state, &store, &[&storage_key]).await;
    }
    stored
}

/// Bytes read to tell a picture from anything else, and to guess a type.
const SNIFF_BYTES: usize = 4096;
/// Bytes written but never claimed by a file row are deleted after this.
const PENDING_OBJECT_MINUTES: i32 = 60;

fn storage_key(organization_id: Uuid, id: Uuid) -> String {
    format!("{}/{}", organization_id.simple(), id.simple())
}

/// One upload at a time per workspace from the room check to the insert, so
/// uploads side by side can't each see room and together overfill it.
async fn hold_room(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    incoming: u64,
    already_written: u64,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("files-quota:{organization_id}"))
        .execute(&mut **transaction)
        .await?;
    require_room(
        state,
        transaction,
        organization_id,
        incoming,
        already_written,
    )
    .await
}

/// Notes keys about to be written, so the sweep deletes their bytes if no
/// file row ever claims them (a crash between the write and the insert).
async fn begin_objects(
    state: &AppState,
    store: &FileStore,
    organization_id: Uuid,
    keys: &[&str],
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO file_objects_pending (storage_key, workspace_id, store_id)
        SELECT key, $2, $3 FROM unnest($1::text[]) AS key
        "#,
    )
    .bind(keys)
    .bind(organization_id)
    .bind(store.id())
    .execute(crate::db::scoped(&state.pool))
    .await?;
    Ok(())
}

async fn delete_objects(store: &FileStore, keys: &[&str]) {
    for key in keys {
        let _ = store.delete(key).await;
    }
}

/// Deletes bytes no file row will claim, and their pending entries.
async fn drop_objects(state: &AppState, store: &FileStore, keys: &[&str]) {
    let mut deleted = Vec::new();
    for key in keys {
        if store.delete(key).await.is_ok() {
            deleted.push(*key);
        }
    }
    // An entry whose bytes couldn't be deleted stays for the sweep.
    let _ = sqlx::query("DELETE FROM file_objects_pending WHERE storage_key = ANY($1)")
        .bind(&deleted)
        .execute(crate::db::scoped(&state.pool))
        .await;
}

/// A file row about to be written.
struct NewFile<'a> {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    destination: Destination,
    uploader: Uuid,
    name: &'a str,
    content_type: &'a str,
    byte_size: u64,
    sha256: &'a str,
    storage_key: &'a str,
    store_id: &'a str,
    dimensions: Option<(u32, u32)>,
    thumbnail_key: Option<&'a str>,
    thumbnail_bytes: Option<u64>,
    preview: Option<&'a str>,
}

/// Writes a file's row and claims its bytes, in the caller's transaction.
async fn insert_file(
    transaction: &mut Transaction<'_, Postgres>,
    file: &NewFile<'_>,
) -> Result<FileRow, AppError> {
    let (conversation_id, work_item_id, attached) = match file.destination {
        Destination::Conversation {
            conversation_id, ..
        } => (Some(conversation_id), None, false),
        Destination::Task {
            work_item_id,
            for_comment,
            ..
        } => (None, Some(work_item_id), !for_comment),
        Destination::Thing { .. } => (None, None, true),
    };
    let (thing_kind, thing_id, purpose) = match file.destination {
        Destination::Thing {
            thing,
            thing_id,
            purpose,
            ..
        } => (Some(thing.name()), Some(thing_id), purpose.name()),
        _ => (None, None, Purpose::Attachment.name()),
    };
    let row = sqlx::query_as::<_, FileRow>(&format!(
        r#"
        INSERT INTO files AS file (
            id, organization_id, project_id, conversation_id, work_item_id,
            uploaded_by_actor_id, name, content_type, byte_size, sha256,
            storage_key, image_width, image_height, thumbnail_key,
            thumbnail_bytes, attached_at, thing_kind, thing_id, purpose, preview_text,
            store_id
        )
        VALUES ($1, $2, NULLIF($3, direct_scope_id()), $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                CASE WHEN $16 THEN now() END, $17, $18, $19, $20, $21)
        RETURNING {FILE_COLUMNS}
        "#
    ))
    .bind(file.id)
    .bind(file.organization_id)
    .bind(file.project_id)
    .bind(conversation_id)
    .bind(work_item_id)
    .bind(file.uploader)
    .bind(file.name)
    .bind(file.content_type)
    .bind(i64::try_from(file.byte_size).unwrap_or(i64::MAX))
    .bind(file.sha256)
    .bind(file.storage_key)
    .bind(
        file.dimensions
            .map(|(width, _)| i32::try_from(width).unwrap_or(i32::MAX)),
    )
    .bind(
        file.dimensions
            .map(|(_, height)| i32::try_from(height).unwrap_or(i32::MAX)),
    )
    .bind(file.thumbnail_key)
    .bind(
        file.thumbnail_bytes
            .map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX)),
    )
    .bind(attached)
    .bind(thing_kind)
    .bind(thing_id)
    .bind(purpose)
    .bind(file.preview)
    .bind(file.store_id)
    .fetch_one(&mut **transaction)
    .await?;
    let keys = std::iter::once(file.storage_key)
        .chain(file.thumbnail_key)
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM file_objects_pending WHERE storage_key = ANY($1)")
        .bind(&keys)
        .execute(&mut **transaction)
        .await?;
    Ok(row)
}

/// Takes away the bytes [`store_prepared`] stored, when the transaction
/// that held its row didn't commit.
pub(crate) async fn discard_stored(state: &AppState, row: &FileRow) {
    let Ok(store) = FileStore::resolve(state, &row.store_id).await else {
        return;
    };
    let keys = std::iter::once(row.storage_key.as_str())
        .chain(row.thumbnail_key.as_deref())
        .collect::<Vec<_>>();
    drop_objects(state, &store, &keys).await;
}

/// Refuses an upload that would take the workspace over its storage limit
/// (metering's: the plan's, `files.workspace_quota_mb` and any spend
/// limit), or the server over its storage watermark. The walk of the data
/// root leaves the file store out (`storage_admission`), so the files the
/// database knows about are added here.
pub(crate) async fn require_room(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    organization_id: Uuid,
    incoming: u64,
    already_written: u64,
) -> Result<(), AppError> {
    let incoming_i64 = i64::try_from(incoming).unwrap_or(i64::MAX);
    crate::metering::admit(
        &mut *connection,
        crate::metering::Ask::new(organization_id, "storage_bytes", incoming_i64),
    )
    .await?;
    // Link preview pictures live in the file store too, so they count.
    let all_bytes = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(sum(bytes), 0)::bigint FROM (
            SELECT byte_size + COALESCE(thumbnail_bytes, 0) AS bytes
            FROM files WHERE blob_deleted_at IS NULL
            UNION ALL
            SELECT image_bytes FROM link_previews WHERE image_key IS NOT NULL
        ) stored
        "#,
    )
    .fetch_one(&mut *connection)
    .await?;
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
        || report
            .available_bytes
            .saturating_sub(incoming.saturating_sub(already_written))
            <= config.minimum_free_bytes
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

/// The most of a text file's start kept to show before it is opened.
const PREVIEW_CHARACTERS: usize = 1_200;
const PREVIEW_LINES: usize = 24;

/// The start of a text or code file, for its preview: up to 24 lines or
/// 1,200 characters of what is valid UTF-8 at its start. None for
/// anything else.
fn text_preview(content_type: &str, bytes: &[u8]) -> Option<String> {
    let texty = content_type.starts_with("text/")
        || content_type.ends_with("+json")
        || content_type.ends_with("+xml")
        || matches!(
            content_type,
            "application/json"
                | "application/xml"
                | "application/yaml"
                | "application/toml"
                | "application/sql"
                | "application/javascript"
                | "application/x-sh"
        );
    if !texty {
        return None;
    }
    let head = &bytes[..bytes.len().min(PREVIEW_CHARACTERS * 4)];
    let valid = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default(),
    };
    let preview = valid
        .lines()
        .take(PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(PREVIEW_CHARACTERS)
        .collect::<String>();
    let preview = preview.trim_end().to_owned();
    (!preview.trim().is_empty()).then_some(preview)
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

/// Moves the author's unsent uploads for a comment on this task (`?for=
/// comment`) into the conversation the comment is posted in, so the send
/// pipeline attaches them as the message's (C1.4). Anything else in the
/// list is left for `attach_to_message` to refuse.
pub(crate) async fn rehome_comment_uploads(
    transaction: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    conversation_id: Uuid,
    author: Uuid,
    file_ids: &[Uuid],
) -> Result<(), AppError> {
    if file_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE files
        SET conversation_id = $2, work_item_id = NULL
        WHERE id = ANY($3) AND work_item_id = $1 AND uploaded_by_actor_id = $4
          AND attached_at IS NULL AND removed_at IS NULL
        "#,
    )
    .bind(work_item_id)
    .bind(conversation_id)
    .bind(file_ids)
    .bind(author)
    .execute(&mut **transaction)
    .await?;
    Ok(())
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

/// Removes a file: an unsent upload by its uploader, a task's own file by
/// anyone who may edit the task (checked by the caller, `may_edit_task`, so
/// an archived task or a viewer can't), or another thing's file by the one
/// who added it (the page keeps showing where it was, as gone). Message
/// files (a task comment's too) go with their message instead.
pub(crate) async fn remove_file(
    transaction: &mut Transaction<'_, Postgres>,
    row: &FileRow,
    actor: Uuid,
    may_edit_task: bool,
) -> Result<(), AppError> {
    // Decided on the row as it is now, locked: an upload sent with a message
    // a moment ago is the message's, not an unsent upload any more.
    let Some((attached, work_item_id, thing, uploader)) =
        sqlx::query_as::<_, (bool, Option<Uuid>, bool, Uuid)>(
            r#"
            SELECT attached_at IS NOT NULL, work_item_id, thing_id IS NOT NULL,
                   uploaded_by_actor_id
            FROM files WHERE id = $1 AND removed_at IS NULL
            FOR UPDATE
            "#,
        )
        .bind(row.id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(());
    };
    let allowed = if thing {
        uploader == actor
    } else if attached {
        work_item_id.is_some() && may_edit_task
    } else {
        uploader == actor
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
    migrate::register_jobs(registry);
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
    .execute(crate::db::scoped(&state.pool))
    .await?;
    let removed = sqlx::query_as::<_, (Uuid, String, Option<String>, String)>(
        r#"
        SELECT id, storage_key, thumbnail_key, store_id FROM files
        WHERE removed_at IS NOT NULL AND blob_deleted_at IS NULL
        ORDER BY removed_at
        LIMIT $1
        "#,
    )
    .bind(CLEANUP_BATCH)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let mut deleted = 0;
    for (id, key, thumbnail_key, store_id) in removed {
        // One file the store can't delete mustn't stop every later one: it
        // stays in the batch and is tried again next time.
        let mut failed = false;
        match FileStore::resolve(state, &store_id).await {
            Ok(store) => {
                for key in std::iter::once(key).chain(thumbnail_key) {
                    if let Err(error) = store.delete(&key).await {
                        tracing::warn!(file_id = %id, category = error.log_category(), "file bytes not deleted");
                        failed = true;
                    }
                }
            }
            Err(_) => {
                tracing::warn!(file_id = %id, "file bytes are in a store this server doesn't have");
                failed = true;
            }
        }
        if failed {
            continue;
        }
        sqlx::query("UPDATE files SET blob_deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(crate::db::scoped(&state.pool))
            .await?;
        deleted += 1;
    }
    for store in FileStore::all(&state.config) {
        deleted += sweep_pending_objects(state, &store).await?;
    }
    // Pending bytes in a workspace's own bucket, by the stores they name.
    let workspace_stores = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT store_id FROM file_objects_pending WHERE store_id LIKE 'ws-%'",
    )
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    for store_id in workspace_stores {
        if let Ok(store) = FileStore::resolve(state, &store_id).await {
            deleted += sweep_pending_objects(state, &store).await?;
        }
    }
    deleted += upload::sweep(state).await?;
    deleted += crate::link_previews::cleanup(state).await?;
    Ok(deleted)
}

/// Deletes bytes written more than an hour ago that no file row claimed:
/// an upload that stopped between writing its bytes and recording them.
pub(crate) async fn sweep_pending_objects(
    state: &AppState,
    store: &FileStore,
) -> Result<u64, AppError> {
    let due = sqlx::query_scalar::<_, String>(
        r#"
        SELECT storage_key FROM file_objects_pending
        WHERE store_id = $1 AND started_at < now() - make_interval(mins => $2)
        ORDER BY started_at
        LIMIT $3
        "#,
    )
    .bind(store.id())
    .bind(PENDING_OBJECT_MINUTES)
    .bind(CLEANUP_BATCH)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let keys = due.iter().map(String::as_str).collect::<Vec<_>>();
    let before = keys.len();
    drop_objects(state, store, &keys).await;
    Ok(u64::try_from(before).unwrap_or(0))
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
            preview_text: None,
            attached_at: None,
            created_at: String::new(),
            store_id: store::LOCAL.to_owned(),
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

    #[test]
    fn text_files_keep_their_first_lines_as_a_preview() {
        let many = (1..=40)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let preview = text_preview("text/plain", many.as_bytes()).expect("a preview");
        assert_eq!(preview.lines().count(), PREVIEW_LINES);
        assert!(preview.starts_with("line 1\nline 2"));
        assert_eq!(
            text_preview("application/json", b"{\"a\": 1}").as_deref(),
            Some("{\"a\": 1}")
        );
        // Cut inside a character: what is whole is kept.
        assert_eq!(
            text_preview("text/plain", "ab\u{e9}".as_bytes()[..3].as_ref()).as_deref(),
            Some("ab")
        );
        assert_eq!(text_preview("application/pdf", b"%PDF-1.7"), None);
        assert_eq!(text_preview("text/plain", b"   \n  "), None);
    }
}
