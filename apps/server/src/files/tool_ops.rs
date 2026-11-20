//! A message's, task's or record's files as operations (K2.11): listing
//! them, uploading one through the call itself (up to 4 MiB; larger files
//! go by file_upload_start) and removing one. Each runs the command its
//! hand-written tool ran (`ops::wrapped`) as the caller the dispatcher
//! checked. `file_read` stays a hand-written tool: it answers an image as
//! an image, which an operation's value can't carry yet.

use serde_json::json;
use uuid::Uuid;

use crate::{
    events::{Audience, NewEvent, catalog},
    mcp::{file_tools, task_commands::command_request_id},
    ops::{
        OpCall, OpError, Registry, Target, Targeted,
        refs::{parse_id, parse_message_id},
        wrapped::{run_tool, tool_op},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<FileList>()
        .add::<FileUpload>()
        .add::<FileDelete>();
}

/// A task named by id or link (the dispatcher turns a key into the id).
fn task(task_id: &str) -> Target {
    Target::thing("task", parse_id(task_id).unwrap_or_else(Uuid::nil))
}

impl Targeted for file_tools::FileListArguments {
    fn target(&self) -> Target {
        match (self.message_id.as_deref(), self.task_id.as_deref()) {
            (Some(message), _) => Target::thing(
                "message",
                parse_message_id(message).unwrap_or_else(Uuid::nil),
            ),
            (None, Some(task_id)) => task(task_id),
            (None, None) => Target::Workspace(None),
        }
    }
}

impl Targeted for file_tools::FileUploadArguments {
    fn target(&self) -> Target {
        match (
            self.conversation_id.as_deref(),
            self.task_id.as_deref(),
            self.thing.as_ref(),
        ) {
            (Some(conversation), _, _) => Target::thing(
                "conversation",
                parse_id(conversation).unwrap_or_else(Uuid::nil),
            ),
            (None, Some(task_id), _) => task(task_id),
            // A record keeps the file; a page is named here too, so a guard
            // on uploads reviews it as the page's (page_file_upload keeps
            // a page's files).
            (None, None, Some(thing)) if matches!(thing.kind.as_str(), "record" | "page") => {
                let kind = if thing.kind == "page" {
                    "page"
                } else {
                    "record"
                };
                Target::thing(kind, parse_id(&thing.id).unwrap_or_else(Uuid::nil))
            }
            _ => Target::Workspace(None),
        }
    }
}

impl Targeted for file_tools::FileDeleteArguments {
    fn target(&self) -> Target {
        Target::thing("file", parse_id(&self.file_id).unwrap_or_else(Uuid::nil))
    }
}

/// Tells the file's space a task's own file came or went: ids only.
async fn file_event(
    call: &mut OpCall<'_>,
    ty: &'static crate::events::EventType,
    space: Uuid,
    file: Uuid,
) -> Result<(), OpError> {
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    crate::events::emit(
        tx,
        NewEvent {
            id: Uuid::now_v7(),
            ty,
            workspace_id: Some(workspace),
            space_id: Some(space),
            thing: Some((file, "file")),
            access_thing: None,
            version: None,
            changes: &[],
            audience: Audience::Space,
            payload: json!({}),
            actor: None,
            legacy_type: None,
        },
    )
    .await
    .map(drop)
    .map_err(OpError::from)
}

tool_op!(FileList {
    name: "file_list",
    module: "workspace",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List files",
    description: "List the files on a message (messageId) or on a task and its comments \
        (taskId): name, type, size and fileId for file_read.",
    emits: [],
    input: file_tools::FileListArguments,
    output: file_tools::FileListValue,
    run: |call, input| { run_tool!(call, as_tool, file_tools::file_list, input) }
});

tool_op!(FileUpload {
    name: "file_upload",
    module: "workspace",
    kind: Create,
    permission: None,
    idempotency: Command,
    title: "Upload file",
    description: "Upload a file (text or base64, up to 4 MiB) to a conversation, then send it by \
        passing the returned fileId in message_send's or reply_send's attachmentIds; or keep it \
        on a task (taskId; forComment: true holds it for task_comment's attachmentIds), such as \
        a run's output, report or screenshot, or on a record you can edit (thing). Larger files \
        go by file_upload_start.",
    emits: ["file.attached"],
    input: file_tools::FileUploadArguments,
    output: file_tools::FileSummary,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        let on_task = input.task_id.is_some() && input.for_comment != Some(true);
        let mut file = run_tool!(call, as_tool, file_tools::file_upload, input)?;
        // The envelope says it was a retry; the value is the first answer's.
        call.mark_replayed(file.idempotent_replay);
        let replay = std::mem::take(&mut file.idempotent_replay);
        if on_task
            && !replay
            && let (Some(space), Some(id)) = (call.ctx.space_id, parse_id(&file.file_id))
        {
            file_event(call, &catalog::FILE_ATTACHED, space, id).await?;
        }
        Ok(file)
    }
});

tool_op!(FileDelete {
    name: "file_delete",
    module: "workspace",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    destructive: true,
    title: "Remove file",
    description: "Remove a file: an upload of yours you haven't sent yet, or one of a task's own \
        files when you can edit the task. A file sent in a message goes with the message \
        (message_delete), and a comment's with the comment (task_comment_delete).",
    emits: ["file.removed"],
    input: file_tools::FileDeleteArguments,
    output: file_tools::DeletedFile,
    run: |call, input| {
        let file_id = parse_id(&input.file_id);
        // Only a task's own file is news to its space; an unsent upload
        // was only ever its uploader's.
        let task_space = match file_id {
            Some(id) => {
                let tx = call.tx().await?;
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT project_id FROM files \
                     WHERE id = $1 AND work_item_id IS NOT NULL AND attached_at IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|error| OpError::logged("file_delete", &error.into()))?
            }
            None => None,
        };
        let removed = run_tool!(call, as_tool, file_tools::file_delete, input)?;
        if removed.removed
            && let (Some(space), Some(id)) = (task_space, file_id)
        {
            file_event(call, &catalog::FILE_REMOVED, space, id).await?;
        }
        Ok(removed)
    }
});
