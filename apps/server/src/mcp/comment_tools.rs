//! task_comment_edit and task_comment_delete: the web's comment commands
//! (`crate::work_items::comments`) for agents and connected apps. The author
//! edits a comment; the author, or a project owner or admin who is a person,
//! deletes it.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
    writes::{WriteAccess, check_write_access, mcp_actor, normalize_comment},
};
use crate::{
    AppState,
    error::AppError,
    message_content::MessageContent,
    work_items::comments::{
        CommentRefusal, apply_comment_edit, apply_comment_removal, is_comment_moderator,
        lock_comment, resolve_comment_content,
    },
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
    pub text: String,
    /// The revision you read (this tool returns it; a comment never edited
    /// is 1). Leave it out to edit whatever is there.
    pub expected_revision: Option<i32>,
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
pub(super) struct EditedComment {
    pub task_id: String,
    pub comment_id: String,
    /// Pass as expectedRevision next time.
    pub revision: i32,
    /// False when the comment already said exactly this; nothing changed.
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeletedComment {
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
            .fetch_optional(&state.pool)
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
            "You can delete your own comments. Deleting someone else's takes a project owner or admin.",
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
        let Some(text) = normalize_comment(arguments.text.clone()) else {
            return Ok(ToolResult::invalid("text", TEXT_MESSAGE));
        };
        let Ok(validated) = MessageContent::from_plain_text(text) else {
            return Ok(ToolResult::invalid("text", TEXT_MESSAGE));
        };
        let Some(task_id) = task_in_scope(state, principal, &arguments.task_id).await? else {
            return Ok(ToolResult::invalid("taskId", TASK_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = access_result(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(result);
        }
        let Some(locked) =
            lock_comment(&mut transaction, principal.project_id, task_id, comment_id).await?
        else {
            return Ok(ToolResult::not_found("That comment"));
        };
        let resolved = match resolve_comment_content(
            &mut transaction,
            principal.project_id,
            validated.content().clone(),
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(AppError::Validation(_)) => return Ok(ToolResult::invalid("text", TEXT_MESSAGE)),
            Err(error) => return Err(error),
        };
        let actor = mcp_actor(principal, String::new());
        let edit = match apply_comment_edit(
            &mut transaction,
            &actor,
            &locked,
            resolved,
            arguments.expected_revision,
        )
        .await?
        {
            Ok(edit) => edit,
            Err(refusal) => return Ok(refusal_result(refusal)),
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
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = access_result(check_write_access(&mut transaction, principal).await?)
        {
            return Ok(result);
        }
        let Some(locked) =
            lock_comment(&mut transaction, principal.project_id, task_id, comment_id).await?
        else {
            return Ok(ToolResult::not_found("That comment"));
        };
        let moderator = is_comment_moderator(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
        )
        .await?;
        let actor = mcp_actor(principal, String::new());
        let already_deleted =
            match apply_comment_removal(&mut transaction, &actor, moderator, &locked).await? {
                Ok(already) => already,
                Err(refusal) => return Ok(refusal_result(refusal)),
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
