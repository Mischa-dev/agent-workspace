//! The task commands as operations run them (K2.13). Each one is the
//! command the hand-written tool ran, now called from `work_items::ops`
//! with the caller the dispatcher checked: the same checks inside, the
//! same idempotency scopes, the same audit rows. What comes back is the
//! operation's result or its error.

use std::borrow::Cow;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    McpPrincipal,
    comment_tools::{self, TaskCommentDeleteArguments, TaskCommentEditArguments},
    common::ToolResult,
    criteria::{self, TaskCriteriaArguments},
    messaging::principal_of,
    tasks::{self, TaskCreateArguments, TaskUpdateArguments},
    writes::{self, MarkBlockedArguments, MarkReadyForReviewArguments, PostUpdateArguments},
};
pub use super::{
    comment_tools::{DeletedComment, EditedComment},
    criteria::{CriteriaValue, CriterionOperationArgument},
};
use crate::{
    AppState,
    ops::{ConflictReason, DenyReason, OpContext, OpError},
};

/// What a task write did.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskWritten {
    /// The task's id.
    pub work_item_id: String,
    /// The task's version now; pass it as expectedVersion next time.
    pub version: i64,
    /// What the call changed, such as ["status", "position"].
    pub changed_fields: Vec<String>,
    /// The activity entry it made, when it made one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
    /// The change's own id (a comment's id, for a comment).
    pub result_id: String,
    /// True when this answers a retry of a call that already applied.
    pub idempotent_replay: bool,
    /// The task's key, such as AWD-12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Where people open the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Anything else worth knowing, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The task's labels after the change, when the call changed them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// When the caller's role only suggests on tasks: the proposal the
    /// change became, for someone who edits the task to accept or reject
    /// (proposal_decide). The task itself is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    /// The parts the proposal sets, each with how it read before and after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed: Vec<Value>,
}

impl TaskWritten {
    /// The task it wrote, for the audit row.
    pub fn task_id(&self) -> Option<Uuid> {
        self.work_item_id.parse().ok()
    }
}

/// The caller as the commands read it, acting in `space` when the call
/// didn't locate one (a credential held to its own space).
fn principal_in(ctx: &OpContext, space: Option<Uuid>) -> McpPrincipal {
    let mut principal = principal_of(ctx);
    if let Some(space) = space {
        principal.project_id = space;
    }
    principal
}

/// The request id a command scopes its retry to: the caller's, or a fresh
/// one when the call came without (it runs once, R52).
fn request_id(ctx: &OpContext) -> String {
    ctx.request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string())
}

/// An operation's input as the command's arguments, which name the same
/// fields, plus the request id.
fn arguments<A: DeserializeOwned>(
    input: &impl Serialize,
    client_request_id: Option<String>,
) -> Result<A, OpError> {
    let mut value = serde_json::to_value(input).map_err(|_| OpError::Unavailable)?;
    if let (Some(object), Some(id)) = (value.as_object_mut(), client_request_id) {
        object.insert("clientRequestId".to_owned(), Value::String(id));
    }
    serde_json::from_value(value).map_err(|error| OpError::Invalid {
        field: Cow::Borrowed("input"),
        message: Cow::Owned(error.to_string()),
    })
}

fn owned(message: Option<String>, otherwise: &'static str) -> Cow<'static, str> {
    message.map_or(Cow::Borrowed(otherwise), Cow::Owned)
}

/// A task command's answer as an operation's.
fn written(result: writes::WriteResult) -> Result<TaskWritten, OpError> {
    use writes::WriteResult as Result;
    match result {
        Result::Applied {
            work_item_id,
            version,
            changed_fields,
            activity_id,
            result_id,
            idempotent_replay,
            key,
            url,
            message,
            labels,
        } => Ok(TaskWritten {
            work_item_id,
            version,
            changed_fields,
            activity_id,
            result_id,
            idempotent_replay,
            key,
            url,
            message,
            labels,
            proposal_id: None,
            proposed: Vec::new(),
        }),
        Result::Conflict {
            retry_category,
            message,
            ..
        } if retry_category == "use_new_request_id" => Err(OpError::Conflict {
            reason: ConflictReason::RequestIdReused,
            message: owned(
                message,
                "That clientRequestId was already used for a different request.",
            ),
            current: None,
        }),
        Result::Conflict {
            changed_fields,
            message,
            ..
        } if changed_fields == ["archived"] => Err(OpError::Conflict {
            reason: ConflictReason::Archived,
            message: owned(message, "That task is archived: restore it first."),
            current: None,
        }),
        Result::Conflict {
            current_version,
            current,
            changed_fields,
            message,
            ..
        } => Err(OpError::Conflict {
            reason: ConflictReason::Version { current_version },
            message: owned(
                message,
                "The task changed since you read it. Open it again and retry with its version.",
            ),
            current: Some(json!({ "task": current, "changedFields": changed_fields })),
        }),
        Result::NotFound { .. } => Err(OpError::not_found("That task")),
        Result::PermissionDenied {
            retry_category,
            message,
        } => Err(OpError::Denied {
            reason: if retry_category == "module" {
                DenyReason::ModuleOff {
                    module: "tasks".to_owned(),
                }
            } else {
                DenyReason::Rule
            },
            message: owned(message, "You can't change that task."),
        }),
        Result::InvalidRequest { field, message, .. } => Err(OpError::Invalid {
            field: Cow::Owned(field),
            message: owned(message, "Check this field and try again."),
        }),
        Result::InvalidTransition {
            current_status_id,
            requested_status_id,
            current_status,
            requested_status,
            allowed_statuses,
            path,
            message,
            ..
        } => Err(OpError::Conflict {
            reason: ConflictReason::Transition,
            message: owned(
                message,
                "The task can't move to that status directly from where it is.",
            ),
            current: Some(json!({
                "currentStatusId": current_status_id,
                "requestedStatusId": requested_status_id,
                "currentStatus": current_status,
                "requestedStatus": requested_status,
                "allowedStatuses": allowed_statuses,
                "path": path,
            })),
        }),
        Result::TemporarilyUnavailable { .. } => Err(OpError::Unavailable),
    }
}

/// A tool-shaped answer as an operation's.
fn answer<T>(result: ToolResult<T>) -> Result<T, OpError> {
    match result {
        ToolResult::Ok { value } => Ok(value),
        ToolResult::NotFound { message } if !message.is_empty() => Err(OpError::Missing {
            message: Cow::Owned(message),
        }),
        ToolResult::NotFound { .. } => Err(OpError::not_found("That task")),
        ToolResult::InvalidRequest { field, message } => Err(OpError::Invalid {
            field: Cow::Owned(field),
            message: Cow::Owned(message),
        }),
        ToolResult::PermissionDenied {
            message,
            retry_category,
        } => Err(OpError::Denied {
            reason: if retry_category.as_deref() == Some("module") {
                DenyReason::ModuleOff {
                    module: "tasks".to_owned(),
                }
            } else {
                DenyReason::Rule
            },
            message: Cow::Owned(message),
        }),
        ToolResult::Conflict { message } => Err(OpError::Conflict {
            reason: if message.contains("clientRequestId") {
                ConflictReason::RequestIdReused
            } else {
                ConflictReason::State { code: "state" }
            },
            message: Cow::Owned(message),
            current: None,
        }),
        ToolResult::TemporarilyUnavailable { .. } => Err(OpError::Unavailable),
    }
}

pub(crate) async fn create(
    state: &AppState,
    ctx: &OpContext,
    space: Option<Uuid>,
    input: &impl Serialize,
) -> Result<TaskWritten, OpError> {
    let principal = principal_in(ctx, space);
    let arguments: TaskCreateArguments = arguments(input, Some(request_id(ctx)))?;
    written(tasks::task_create(state, &principal, arguments).await)
}

pub(crate) async fn update(
    state: &AppState,
    ctx: &OpContext,
    input: &impl Serialize,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    let arguments: TaskUpdateArguments = arguments(input, Some(request_id(ctx)))?;
    written(tasks::task_update(state, &principal, arguments).await)
}

pub(crate) async fn mark_blocked(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    expected_version: i64,
    reason: String,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::mark_blocked(
            state,
            &principal,
            MarkBlockedArguments {
                work_item_id: task_id.to_string(),
                expected_version,
                reason,
                client_request_id: request_id(ctx),
            },
        )
        .await,
    )
}

pub(crate) async fn mark_ready_for_review(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    expected_version: i64,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::mark_ready_for_review(
            state,
            &principal,
            MarkReadyForReviewArguments {
                work_item_id: task_id.to_string(),
                expected_version,
                client_request_id: request_id(ctx),
            },
        )
        .await,
    )
}

pub(crate) async fn set_archived(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    expected_version: i64,
    archived: bool,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::set_archived(
            state,
            &principal,
            writes::SetArchivedArguments {
                work_item_id: task_id.to_string(),
                expected_version,
                archived,
                client_request_id: request_id(ctx),
            },
        )
        .await,
    )
}

pub(crate) async fn comment(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    text: String,
    attachment_ids: Option<Vec<String>>,
    reply_to_comment_id: Option<String>,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::post_update(
            state,
            &principal,
            PostUpdateArguments {
                work_item_id: task_id.to_string(),
                update: text,
                client_request_id: request_id(ctx),
                attachment_ids,
                reply_to_comment_id,
            },
        )
        .await,
    )
}

pub(crate) async fn comment_edit(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    comment_id: Uuid,
    text: String,
    expected_revision: Option<i32>,
) -> Result<EditedComment, OpError> {
    let principal = principal_of(ctx);
    answer(
        comment_tools::task_comment_edit(
            state,
            &principal,
            TaskCommentEditArguments {
                task_id: task_id.to_string(),
                comment_id: comment_id.to_string(),
                text,
                expected_revision,
            },
        )
        .await,
    )
}

pub(crate) async fn comment_delete(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    comment_id: Uuid,
) -> Result<DeletedComment, OpError> {
    let principal = principal_of(ctx);
    answer(
        comment_tools::task_comment_delete(
            state,
            &principal,
            TaskCommentDeleteArguments {
                task_id: task_id.to_string(),
                comment_id: comment_id.to_string(),
            },
        )
        .await,
    )
}

pub(crate) async fn criteria(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    expected_version: i64,
    operations: Vec<CriterionOperationArgument>,
) -> Result<CriteriaValue, OpError> {
    let principal = principal_of(ctx);
    answer(
        criteria::task_criteria(
            state,
            &principal,
            TaskCriteriaArguments {
                task_id: task_id.to_string(),
                expected_version,
                operations,
                client_request_id: Some(request_id(ctx)),
            },
        )
        .await,
    )
}

// ---------------------------------------------------------------------------
// K2.14 and K2.15: the planning, link, bulk and read tools' commands, run by
// `work_items::ops`. Each operation calls the hand-written tool's command
// with the caller the dispatcher checked.

/// The caller as a tool's command reads it, acting in `space` when the call
/// didn't locate one (a credential held to its own space).
pub(crate) fn principal_for(ctx: &OpContext, space: Option<Uuid>) -> McpPrincipal {
    principal_in(ctx, space)
}

/// A tool's answer as an operation's.
pub(crate) fn tool_answer<T>(result: ToolResult<T>) -> Result<T, OpError> {
    answer(result)
}

/// The request id a command that keeps its own idempotency scopes its retry
/// to: the caller's, or a fresh one (it runs once, R52).
pub(crate) fn command_request_id(ctx: &OpContext) -> String {
    request_id(ctx)
}

/// A batch tool's answer as an operation's: a rejected batch is a conflict
/// whose `details.current.problems` lists each problem, with its index.
pub(crate) fn batch_answer<T>(result: super::task_batch::BatchResult<T>) -> Result<T, OpError> {
    use super::task_batch::BatchResult as Batch;
    match result {
        Batch::Ok { value } => Ok(value),
        Batch::Rejected { message, problems } => Err(OpError::Conflict {
            reason: ConflictReason::State { code: "rejected" },
            message: Cow::Owned(message),
            current: Some(json!({ "problems": problems })),
        }),
        Batch::NotFound { .. } => Err(OpError::not_found("That project")),
        Batch::InvalidRequest { field, message } => Err(OpError::Invalid {
            field: Cow::Owned(field),
            message: Cow::Owned(message),
        }),
        Batch::PermissionDenied {
            message,
            retry_category,
        } => answer::<T>(ToolResult::PermissionDenied {
            message,
            retry_category,
        }),
        Batch::Conflict { message } => answer::<T>(ToolResult::Conflict { message }),
        Batch::TemporarilyUnavailable { message } => {
            answer::<T>(ToolResult::TemporarilyUnavailable { message })
        }
    }
}
