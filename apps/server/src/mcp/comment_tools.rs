//! task_comment_edit and task_comment_delete: the web's comment commands
//! (`crate::work_items::comments`, over the message commands) for agents and
//! connected apps. The author edits a comment; the author, or a person whose
//! role moderates the space, deletes it.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
    writes::{WriteAccess, check_access_for, normalize_comment},
};
use crate::{
    AppState,
    error::AppError,
    message_content::MessageContent,
    policy::Action,
    work_items::comments::{self, CommentRefusal},
};

const TASK_ID_MESSAGE: &str = "Pass the task's ID, key (such as AWD-12) or link.";
const COMMENT_ID_MESSAGE: &str =
    "Pass the comment's ID; task_open lists a task's comments with theirs.";
const TEXT_MESSAGE: &str =
    "Write the comment as text, 1 to 20,000 characters. Line breaks and Markdown are kept.";
const DELETED_NOTE: &str = "Deleted for everyone, with every earlier version of its text. If it held a secret, rotate the secret: anyone who already read it may have a copy.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCommentEditArguments {
    /// The task's ID, key (such as AWD-12) or link.
    pub task_id: String,
    /// The comment's ID. task_open lists the task's recent comments with
    /// their IDs.
    pub comment_id: String,
    /// The new text, 1 to 20,000 characters. Line breaks and Markdown are
    /// kept.
    pub text: Option<String>,
    /// The revision you read (this tool returns it; a comment never edited
    /// is 1). Leave it out to edit whatever is there.
    pub expected_revision: Option<i32>,
    /// New structured content instead of text, as the web composer writes
    /// it.
    #[serde(default)]
    pub content: Option<MessageContent>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCommentDeleteArguments {
    /// The task's ID, key (such as AWD-12) or link.
    pub task_id: String,
    /// The comment's ID. task_open lists the task's recent comments with
    /// their IDs.
    pub comment_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditedComment {
    pub task_id: String,
    pub comment_id: String,
    /// Pass as expectedRevision next time.
    pub revision: i32,
    /// False when the comment already said exactly this; nothing changed.
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeletedComment {
    pub task_id: String,
    pub comment_id: String,
    /// True when it was already deleted; nothing changed.
    pub already_deleted: bool,
    pub note: String,
}

/// The task in the caller's project the reference names.
async fn task_in_scope(
    state: &AppState,
    principal: &McpPrincipal,
    reference: &str,
) -> Result<Option<Uuid>, AppError> {
    let found = match parse_id(reference) {
        Some(id) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE id = $1 AND project_id = $2",
            )
            .bind(id)
            .bind(principal.project_id)
            .fetch_optional(crate::db::scoped(&state.pool))
            .await?
        }
        None => super::tasks::resolve_task(state, principal, reference)
            .await?
            .filter(|(_, project_id)| *project_id == principal.project_id)
            .map(|(id, _)| id),
    };
    Ok(found)
}

fn refusal_result<T>(refusal: CommentRefusal) -> ToolResult<T> {
    match refusal {
        CommentRefusal::NotAuthor => ToolResult::denied(
            "Only its author can edit a comment. Add a comment of your own instead.",
        ),
        CommentRefusal::NotModerator => ToolResult::denied(
            "You can delete your own comments. Deleting someone else's takes a role that moderates this space.",
        ),
        CommentRefusal::Removed => ToolResult::conflict("That comment was deleted."),
        CommentRefusal::RevisionMoved(current) => ToolResult::conflict(format!(
            "The comment changed since you read it; it's at revision {current} now. Read it again with task_open, then retry with expectedRevision {current}."
        )),
        CommentRefusal::TooManyEdits => ToolResult::conflict(
            "That comment has been edited too many times. Add a new comment instead.",
        ),
        CommentRefusal::TaskArchived => ToolResult::conflict(
            "The task is archived, so its comments can't change. Restore it first.",
        ),
    }
}

fn access_result<T>(access: WriteAccess) -> Option<ToolResult<T>> {
    match access {
        WriteAccess::Allowed => None,
        WriteAccess::ReadOnly => Some(ToolResult::read_only()),
        WriteAccess::NotFound => Some(ToolResult::not_found("That task")),
        WriteAccess::ProjectArchived => Some(ToolResult::conflict(
            "The project is archived, so nothing in it can change.",
        )),
        WriteAccess::Denied => Some(ToolResult::denied(
            "Your role in this project doesn't allow commenting on its tasks.",
        )),
    }
}

pub(super) async fn task_comment_edit(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCommentEditArguments,
) -> ToolResult<EditedComment> {
    let result = async {
        let Some(comment_id) = parse_id(&arguments.comment_id) else {
            return Ok(ToolResult::invalid("commentId", COMMENT_ID_MESSAGE));
        };
        let validated = match (arguments.text.clone(), arguments.content.clone()) {
            (Some(text), None) => {
                let Some(text) = normalize_comment(text) else {
                    return Ok(ToolResult::invalid("text", TEXT_MESSAGE));
                };
                let content =
                    super::common::comment_content_from_text(state, principal.project_id, &text)
                        .await?;
                let Ok(validated) = MessageContent::from_json(content) else {
                    return Ok(ToolResult::invalid("text", TEXT_MESSAGE));
                };
                validated
            }
            (None, Some(content)) => {
                if content.validate_structure().is_err() {
                    return Ok(ToolResult::invalid("content", "The content isn't valid."));
                }
                content
            }
            _ => return Ok(ToolResult::invalid("text", "Pass either text or content.")),
        };
        let Some(task_id) = task_in_scope(state, principal, &arguments.task_id).await? else {
            return Ok(ToolResult::invalid("taskId", TASK_ID_MESSAGE));
        };
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        // Editing or deleting a comment needs "comment on tasks", as
        // writing one does; whose comment it is is checked below.
        if let Some(result) = access_result(
            check_access_for(&mut transaction, principal, Action::WorkItemComment).await?,
        ) {
            return Ok(result);
        }
        let actor = match super::talk::message_actor(principal) {
            Ok(actor) => actor,
            Err(message) => return Ok(ToolResult::denied(message)),
        };
        let edit = match comments::edit(
            &mut transaction,
            &actor,
            task_id,
            comment_id,
            validated,
            arguments.expected_revision,
        )
        .await
        {
            Ok(Ok(edit)) => edit,
            Ok(Err(refusal)) => return Ok(refusal_result(refusal)),
            Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That comment")),
            Err(AppError::Validation(_)) => return Ok(ToolResult::invalid("text", TEXT_MESSAGE)),
            Err(error) => return Err(error),
        };
        transaction.commit().await?;
        Ok(ToolResult::ok(EditedComment {
            task_id: task_id.to_string(),
            comment_id: comment_id.to_string(),
            revision: edit.revision,
            changed: edit.changed,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_comment_edit"))
}

pub(super) async fn task_comment_delete(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCommentDeleteArguments,
) -> ToolResult<DeletedComment> {
    let result = async {
        let Some(comment_id) = parse_id(&arguments.comment_id) else {
            return Ok(ToolResult::invalid("commentId", COMMENT_ID_MESSAGE));
        };
        let Some(task_id) = task_in_scope(state, principal, &arguments.task_id).await? else {
            return Ok(ToolResult::invalid("taskId", TASK_ID_MESSAGE));
        };
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        // Editing or deleting a comment needs "comment on tasks", as
        // writing one does; whose comment it is is checked below.
        if let Some(result) = access_result(
            check_access_for(&mut transaction, principal, Action::WorkItemComment).await?,
        ) {
            return Ok(result);
        }
        let actor = match super::talk::message_actor(principal) {
            Ok(actor) => actor,
            Err(message) => return Ok(ToolResult::denied(message)),
        };
        let already_deleted =
            match comments::remove(&mut transaction, &actor, task_id, comment_id).await {
                Ok(Ok(already)) => already,
                Ok(Err(refusal)) => return Ok(refusal_result(refusal)),
                Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That comment")),
                Err(error) => return Err(error),
            };
        transaction.commit().await?;
        Ok(ToolResult::ok(DeletedComment {
            task_id: task_id.to_string(),
            comment_id: comment_id.to_string(),
            already_deleted,
            note: DELETED_NOTE.to_owned(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_comment_delete"))
}
