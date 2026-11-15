//! File tools: list and read the files on messages and tasks, and upload a
//! file to send with a message (`message_send` / `reply_send` attachmentIds)
//! or to keep on a task, such as a run's output.
//!
//! Every read goes through `files::readable_file` / `message_files` /
//! `task_files` with the caller as an MCP reader, so a file is visible
//! exactly when its message (conversation_visible_to) or task
//! (actor_can_access_project) is.

use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{
    model::{CallToolResult, ContentBlock},
    schemars,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, accessible_project_ids, confined_project, parse_id, parse_message_id},
    talk::{can_write_in, message_actor},
    tasks::resolve_task,
    writes::{
        CreateRetry, REQUEST_ID_MESSAGE, WriteAccess, check_access_for, complete_create,
        parse_request_id, reserve_create,
    },
};
use crate::{
    AppState,
    conversations::{lock_message_conversation, require_active_conversation},
    error::AppError,
    files::{self, Destination, FileRow, FileStore, MAX_ATTACHMENTS, Reader, Upload},
    policy::Action,
};

/// The most an agent uploads or reads in one call. Larger files are for the
/// browser.
const MCP_FILE_BYTES: usize = 4 * 1024 * 1024;

/// A file on a message or task.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileSummary {
    pub file_id: String,
    pub name: String,
    pub content_type: String,
    /// Bytes.
    pub size: i64,
    /// Pixel size, for images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// The task comment it was posted with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// True when this answers a retry of file_upload: the file the first
    /// call stored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

impl From<&FileRow> for FileSummary {
    fn from(row: &FileRow) -> Self {
        Self {
            file_id: row.id.to_string(),
            name: row.name.clone(),
            content_type: row.content_type.clone(),
            size: row.byte_size,
            width: row.image_width,
            height: row.image_height,
            comment_id: row.comment_id.map(|id| id.to_string()),
            idempotent_replay: false,
        }
    }
}

pub(super) fn reader(principal: &McpPrincipal) -> Reader {
    Reader::Mcp {
        actor_id: principal.actor_id,
        connection_id: principal.external_agent_connection_id,
        confined_project: confined_project(principal),
    }
}

/// The IDs in a tool's attachmentIds argument, or why they're refused.
pub(super) fn parse_attachment_ids(ids: Option<Vec<String>>) -> Result<Vec<Uuid>, ()> {
    let ids = ids.unwrap_or_default();
    if ids.len() > MAX_ATTACHMENTS {
        return Err(());
    }
    let mut parsed = Vec::with_capacity(ids.len());
    for id in &ids {
        let id = parse_id(id).ok_or(())?;
        if !parsed.contains(&id) {
            parsed.push(id);
        }
    }
    Ok(parsed)
}

pub(super) fn attachment_ids<T>(ids: Option<Vec<String>>) -> Result<Vec<Uuid>, ToolResult<T>> {
    parse_attachment_ids(ids).map_err(|()| {
        ToolResult::invalid(
            "attachmentIds",
            "Pass up to 10 file IDs returned by file_upload.",
        )
    })
}

/// Message content with no text, for a message that is only files.
pub(super) fn blank_content() -> Value {
    serde_json::json!({ "version": 1, "nodes": [{ "type": "text", "text": "" }] })
}

/// The files on these messages, which the caller can already see.
pub(super) async fn message_attachments(
    state: &AppState,
    message_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<FileSummary>>, AppError> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, AttachmentRow>(
        r#"
        SELECT message_id, id, name, content_type, byte_size, image_width, image_height
        FROM files
        WHERE message_id = ANY($1) AND removed_at IS NULL AND blob_deleted_at IS NULL
        ORDER BY message_id, position, id
        "#,
    )
    .bind(message_ids)
    .fetch_all(&state.pool)
    .await?;
    let mut found: HashMap<Uuid, Vec<FileSummary>> = HashMap::new();
    for row in rows {
        found.entry(row.message_id).or_default().push(FileSummary {
            file_id: row.id.to_string(),
            name: row.name,
            content_type: row.content_type,
            size: row.byte_size,
            width: row.image_width,
            height: row.image_height,
            comment_id: None,
            idempotent_replay: false,
        });
    }
    Ok(found)
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    message_id: Uuid,
    id: Uuid,
    name: String,
    content_type: String,
    byte_size: i64,
    image_width: Option<i32>,
    image_height: Option<i32>,
}

// ---------------------------------------------------------------------------
// file_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileListArguments {
    /// A message ID or link: its files.
    pub message_id: Option<String>,
    /// A task ID, key (such as AW-12) or link: its files and its comments'.
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileListValue {
    pub files: Vec<FileSummary>,
}

pub(super) async fn file_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FileListArguments,
) -> ToolResult<FileListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let listed = match (
        arguments.message_id.as_deref(),
        arguments.task_id.as_deref(),
    ) {
        (Some(message), None) => {
            let Some(message_id) = parse_message_id(message) else {
                return ToolResult::invalid("messageId", "Pass a message ID or link.");
            };
            list_message(state, principal, message_id).await
        }
        (None, Some(task)) => match resolve_task(state, principal, task).await {
            Ok(Some((work_item_id, project_id))) => {
                files::task_files(&state.pool, reader(principal), project_id, work_item_id)
                    .await
                    .map(Some)
            }
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        },
        _ => return ToolResult::invalid("messageId", "Pass either messageId or taskId."),
    };
    match listed {
        Ok(Some(rows)) => ToolResult::ok(FileListValue {
            files: rows.iter().map(FileSummary::from).collect(),
        }),
        Ok(None) => ToolResult::not_found(if arguments.message_id.is_some() {
            "That message"
        } else {
            "That task"
        }),
        Err(error) => ToolResult::from_error(error, "file_list"),
    }
}

/// A visible message's files, or None when the caller can't see it.
async fn list_message(
    state: &AppState,
    principal: &McpPrincipal,
    message_id: Uuid,
) -> Result<Option<Vec<FileRow>>, AppError> {
    let project_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project_id FROM messages
        WHERE id = $1 AND removed_at IS NULL AND conversation_visible_to(conversation_id, $2)
        "#,
    )
    .bind(message_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    if !accessible_project_ids(state, principal)
        .await?
        .contains(&project_id)
    {
        return Ok(None);
    }
    Ok(Some(
        files::message_files(&state.pool, reader(principal), message_id).await?,
    ))
}

// ---------------------------------------------------------------------------
// file_read

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileDeleteArguments {
    /// The file's ID, from file_upload or file_list.
    pub file_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeletedFile {
    pub file_id: String,
    pub removed: bool,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileReadArguments {
    /// The file's ID, from file_list or a message's attachments.
    pub file_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileReadValue {
    #[serde(flatten)]
    pub file: FileSummary,
    /// The file's text, for text files (UTF-8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The bytes, base64, for other files up to 4 MiB. Images come back as
    /// an image instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    /// Why the content isn't included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub(super) async fn file_read(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FileReadArguments,
) -> CallToolResult {
    if !principal.can_read() {
        return read_result(ToolResult::denied("This connection can't read."), None);
    }
    let Some(file_id) = parse_id(&arguments.file_id) else {
        return read_result(
            ToolResult::invalid("fileId", "Pass a file ID from file_list."),
            None,
        );
    };
    let row = match files::readable_file(&state.pool, reader(principal), file_id).await {
        Ok(row) => row,
        Err(AppError::NotFound(_)) => return read_result(ToolResult::not_found("That file"), None),
        Err(error) => return read_result(ToolResult::from_error(error, "file_read"), None),
    };
    let store = FileStore::from_config(&state.config);
    let mut value = FileReadValue {
        file: FileSummary::from(&row),
        text: None,
        data_base64: None,
        note: None,
    };
    // An image is shown to the agent as an image: the original when it's
    // small enough, else its preview.
    if row.is_inline_image() {
        let (key, content_type) = match (&row.thumbnail_key, row.byte_size) {
            (Some(thumbnail), size) if size > MCP_FILE_BYTES as i64 => (
                thumbnail.clone(),
                if row.content_type == "image/jpeg" {
                    "image/jpeg"
                } else {
                    "image/png"
                }
                .to_owned(),
            ),
            _ => (row.storage_key.clone(), row.content_type.clone()),
        };
        return match store.read_all(&key, MCP_FILE_BYTES as u64).await {
            Ok(bytes) => read_result(ToolResult::ok(value), Some((bytes, content_type))),
            Err(_) => {
                value.note = Some("The image is too large to read here.".to_owned());
                read_result(ToolResult::ok(value), None)
            }
        };
    }
    if row.byte_size > MCP_FILE_BYTES as i64 {
        value.note =
            Some("The file is larger than 4 MiB; open it in the app to download it.".to_owned());
        return read_result(ToolResult::ok(value), None);
    }
    match store
        .read_all(&row.storage_key, MCP_FILE_BYTES as u64)
        .await
    {
        Ok(bytes) => {
            if is_text(&row.content_type) {
                match String::from_utf8(bytes) {
                    Ok(text) => value.text = Some(text),
                    Err(error) => value.data_base64 = Some(STANDARD.encode(error.into_bytes())),
                }
            } else {
                value.data_base64 = Some(STANDARD.encode(bytes));
            }
            read_result(ToolResult::ok(value), None)
        }
        Err(error) => read_result(ToolResult::from_error(error, "file_read"), None),
    }
}

fn is_text(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/sql"
        )
        || content_type.ends_with("+json")
        || content_type.ends_with("+xml")
}

fn read_result(
    result: ToolResult<FileReadValue>,
    image: Option<(Vec<u8>, String)>,
) -> CallToolResult {
    let json = serde_json::to_value(&result).unwrap_or(Value::Null);
    let mut content = Vec::new();
    if let Some((bytes, content_type)) = image {
        content.push(ContentBlock::image(STANDARD.encode(bytes), content_type));
    }
    content.push(ContentBlock::text(json.to_string()));
    let mut call = CallToolResult::success(content);
    call.structured_content = Some(json);
    call
}

// ---------------------------------------------------------------------------
// file_upload

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileUploadArguments {
    /// Upload for a message in this conversation (channel, thread, DM or
    /// group). Send it by passing the returned fileId in message_send's (or
    /// reply_send's) attachmentIds; unsent uploads are removed after a day.
    pub conversation_id: Option<String>,
    /// Or keep the file on this task (ID, key or link), shown at once.
    pub task_id: Option<String>,
    /// With taskId: hold the file for a comment instead, attached when you
    /// pass its fileId in task_comment's attachmentIds.
    pub for_comment: Option<bool>,
    /// The file name people see, such as report.md or screenshot.png.
    pub name: String,
    /// The MIME type. Images are recognized from their bytes; otherwise the
    /// name's extension decides when this is left out.
    pub content_type: Option<String>,
    /// The content as text (UTF-8), for text files.
    pub text: Option<String>,
    /// Or the bytes, base64. Up to 4 MiB.
    pub data_base64: Option<String>,
    /// Makes uploading safe to retry: send the same one again and you get
    /// the file the first call stored, not a second copy.
    pub client_request_id: Option<String>,
}

pub(super) async fn file_upload(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FileUploadArguments,
) -> ToolResult<FileSummary> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let retry_key = match arguments.client_request_id.as_deref() {
        Some(value) => match parse_request_id(value) {
            Some(key) => Some(key),
            None => return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE),
        },
        None => None,
    };
    // What the retry key is checked against, before the arguments are used.
    let retried = match serde_json::to_value(&arguments) {
        Ok(retried) => retried,
        Err(_) => return ToolResult::from_error(AppError::Internal("arguments"), "file_upload"),
    };
    let bytes = match (arguments.text, arguments.data_base64) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(data)) => match STANDARD.decode(data.trim()) {
            Ok(bytes) => bytes,
            Err(_) => return ToolResult::invalid("dataBase64", "That isn't valid base64."),
        },
        _ => return ToolResult::invalid("text", "Pass either text or dataBase64."),
    };
    if arguments.name.trim().is_empty() {
        return ToolResult::invalid("name", "Give the file a name.");
    }
    let upload = Upload {
        name: arguments.name,
        declared_type: arguments.content_type,
        bytes,
    };
    let target = match (
        arguments.conversation_id.as_deref(),
        arguments.task_id.as_deref(),
    ) {
        (Some(conversation), None) => {
            let Some(conversation_id) = parse_id(conversation) else {
                return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
            };
            conversation_destination(state, principal, conversation_id).await
        }
        (None, Some(task)) => {
            let for_comment = arguments.for_comment.unwrap_or(false);
            task_destination(state, principal, task, for_comment).await
        }
        _ => {
            return ToolResult::invalid("conversationId", "Pass either conversationId or taskId.");
        }
    };
    let outcome = async {
        let (destination, limit) = target?;
        if upload.bytes.len() > limit.min(MCP_FILE_BYTES) {
            return Err(AppError::PayloadTooLarge);
        }
        let prepared = files::prepare_upload(upload).await?;
        // The retry key, the file and its row in one transaction on one
        // connection: a retry waits for this one and answers with its file,
        // and a crash leaves neither.
        let mut transaction = state.pool.begin().await?;
        let reservation = match reserve_create(
            &mut transaction,
            principal,
            "files",
            retry_key.as_ref(),
            &retried,
        )
        .await?
        {
            CreateRetry::Fresh(reservation) => reservation,
            CreateRetry::Replay(file_id, _) => {
                transaction.commit().await?;
                return Ok((file_id, true, destination));
            }
        };
        let row = files::store_prepared(
            state,
            &mut transaction,
            principal.actor_id,
            destination,
            prepared,
        )
        .await?;
        let kept = complete_create(&mut transaction, reservation.as_ref(), row.id).await;
        let committed = match kept {
            Ok(()) => transaction.commit().await.map_err(AppError::from),
            Err(error) => Err(error),
        };
        if let Err(error) = committed {
            files::discard_stored(state, &row).await;
            return Err(error);
        }
        Ok((row.id, false, destination))
    }
    .await;
    let outcome = match outcome {
        Ok((file_id, replayed, destination)) => {
            if let Destination::Task {
                project_id,
                work_item_id,
                for_comment: false,
            } = destination
                && !replayed
                && let Err(error) =
                    crate::work_items::notify_files_changed(&state.pool, project_id, work_item_id)
                        .await
            {
                return ToolResult::from_error(error, "file_upload");
            }
            // Read back as the caller, so a retry shows the file only while
            // they may still see it.
            files::readable_file(&state.pool, reader(principal), file_id)
                .await
                .map(|row| (row, replayed))
        }
        Err(error) => Err(error),
    };
    match outcome {
        Ok((row, idempotent_replay)) => ToolResult::ok(FileSummary {
            idempotent_replay,
            ..FileSummary::from(&row)
        }),
        Err(AppError::NotFound(_)) => {
            ToolResult::not_found("That conversation or task (or you can't post there)")
        }
        Err(AppError::PayloadTooLarge) => ToolResult::invalid(
            "dataBase64",
            "The file is too large: agents upload up to 4 MiB, within the workspace's limit.",
        ),
        Err(error) => ToolResult::from_error(error, "file_upload"),
    }
}

/// Remove a file, as the web's remove button does: an upload of your own
/// you haven't sent yet, or one of a task's own files (not a comment's) when
/// you may edit the task. Files sent with a message go with the message
/// (message_delete), and a comment's with the comment (task_comment_delete).
pub(super) async fn file_delete(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: FileDeleteArguments,
) -> ToolResult<DeletedFile> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(file_id) = parse_id(&arguments.file_id) else {
        return ToolResult::invalid("fileId", "Pass a file ID.");
    };
    let result = async {
        let row = match files::readable_file(&state.pool, reader(principal), file_id).await {
            Ok(row) => row,
            Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That file")),
            Err(error) => return Err(error),
        };
        let task = row.attached_at.as_ref().and(row.work_item_id);
        let mut transaction = state.pool.begin().await?;
        // Decided under the project lock check_access_for takes, as the web
        // decides it under authorize_task_change's.
        let may_edit_task = match task {
            Some(work_item_id) => {
                let at_file = super::tasks::scoped(principal, row.project_id);
                matches!(
                    check_access_for(&mut transaction, &at_file, Action::WorkItemEdit).await?,
                    WriteAccess::Allowed
                ) && sqlx::query_scalar::<_, bool>(
                    "SELECT archived_at IS NULL FROM task_rows WHERE project_id = $1 AND id = $2",
                )
                .bind(row.project_id)
                .bind(work_item_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false)
            }
            None => false,
        };
        match files::remove_file(&mut transaction, &row, principal.actor_id, may_edit_task).await {
            Ok(()) => {}
            Err(AppError::Forbidden) => {
                return Ok(ToolResult::denied(
                    "You can remove uploads of yours you haven't sent, and a task's own files when you can edit the task. A file sent in a message goes with message_delete; a comment's with task_comment_delete.",
                ));
            }
            Err(error) => return Err(error),
        }
        transaction.commit().await?;
        if let Some(work_item_id) = task {
            crate::work_items::notify_files_changed(&state.pool, row.project_id, work_item_id)
                .await?;
        }
        Ok(ToolResult::ok(DeletedFile {
            file_id: row.id.to_string(),
            removed: true,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "file_delete"))
}

/// Where an upload to a conversation goes, and the most it may be, once
/// the caller may post there.
async fn conversation_destination(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: Uuid,
) -> Result<(Destination, usize), AppError> {
    let actor = message_actor(principal).map_err(|_| AppError::Forbidden)?;
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_message_conversation(
        &mut transaction,
        &actor,
        conversation_id,
        Action::MessageSend,
    )
    .await?;
    require_active_conversation(&conversation)?;
    let limit = files::upload_limit(&mut transaction, conversation.project_id).await?;
    transaction.commit().await?;
    if !accessible_project_ids(state, principal)
        .await?
        .contains(&conversation.project_id)
    {
        return Err(AppError::NotFound("conversation"));
    }
    Ok((
        Destination::Conversation {
            project_id: conversation.project_id,
            conversation_id: conversation.id,
        },
        limit,
    ))
}

/// Where an upload to a task goes, and the most it may be, once the caller
/// may edit the task and it's open.
async fn task_destination(
    state: &AppState,
    principal: &McpPrincipal,
    task: &str,
    for_comment: bool,
) -> Result<(Destination, usize), AppError> {
    let Some((work_item_id, project_id)) = resolve_task(state, principal, task).await? else {
        return Err(AppError::NotFound("work_item"));
    };
    if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
        return Err(AppError::NotFound("work_item"));
    }
    let open = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT item.archived_at IS NULL AND project.archived_at IS NULL
        FROM task_rows item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    if !open {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let mut connection = state.pool.acquire().await?;
    let limit = files::upload_limit(&mut connection, project_id).await?;
    Ok((
        Destination::Task {
            project_id,
            work_item_id,
            for_comment,
        },
        limit,
    ))
}
