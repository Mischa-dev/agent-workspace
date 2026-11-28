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
        Result::LimitReached { limit, .. } => Err(OpError::LimitReached { refusal: limit }),
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
            } else if message.contains("changed since you read it") {
                // A comment edited elsewhere: the editor asks to reopen it.
                ConflictReason::State {
                    code: "version_conflict",
                }
            } else {
                ConflictReason::State { code: "state" }
            },
            message: Cow::Owned(message),
            current: None,
        }),
        ToolResult::TemporarilyUnavailable { .. } => Err(OpError::Unavailable),
        ToolResult::LimitReached { limit, .. } => Err(OpError::LimitReached { refusal: limit }),
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

pub(crate) async fn clear_blocker(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    expected_version: i64,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::clear_blocker(
            state,
            &principal,
            writes::ClearBlockerArguments {
                work_item_id: task_id.to_string(),
                expected_version,
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

/// A comment's words: text (where @handles and task keys become mentions
/// and references) or structured content as the web composer writes it.
/// Exactly one is given; the command refuses anything else.
pub(crate) struct CommentText {
    pub text: Option<String>,
    pub content: Option<crate::message_content::MessageContent>,
}

pub(crate) async fn comment(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    body: CommentText,
    attachment_ids: Option<Vec<String>>,
    reply_to_comment_id: Option<String>,
    note: Option<bool>,
) -> Result<TaskWritten, OpError> {
    let principal = principal_of(ctx);
    written(
        writes::post_update(
            state,
            &principal,
            PostUpdateArguments {
                work_item_id: task_id.to_string(),
                update: body.text,
                client_request_id: request_id(ctx),
                attachment_ids,
                reply_to_comment_id,
                content: body.content,
                note,
            },
        )
        .await,
    )
}

/// Where a walk through the workflow ended.
pub(crate) struct Walked {
    /// The task's version now.
    pub version: i64,
    /// Whether the task is in a status of the group asked for.
    pub arrived: bool,
    /// The status it was heading for, when the space has one in that group.
    pub status: Option<String>,
}

/// Move a task to the space's first status in a group (`started`,
/// `review`, `completed`), one allowed change at a time, the way a caller
/// would with several `task_update` calls (AWD-220: `task_claim_next` and
/// `task_finish` shouldn't make a worker learn each space's workflow).
/// `from_groups`, when given, limits the walk to tasks sitting in one of
/// those groups. Each step is its own command with its own retry key, so a
/// retry of the whole call picks up where the first one stopped.
pub(crate) async fn walk_to(
    state: &AppState,
    ctx: &OpContext,
    space: Uuid,
    task_id: Uuid,
    group: &str,
    from_groups: Option<&[&str]>,
) -> Result<Walked, OpError> {
    use sha2::{Digest, Sha256};
    let (workflow, current, mut version) = {
        let mut tx = crate::db::scoped_tx(&state.pool)
            .await
            .map_err(crate::error::AppError::from)?;
        let workflow = super::workflow::Workflow::load(&mut tx, space).await?;
        let row = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT status_id, version FROM task_rows WHERE id = $1 AND project_id = $2",
        )
        .bind(task_id)
        .bind(space)
        .fetch_optional(&mut *tx)
        .await
        .map_err(crate::error::AppError::from)?
        .ok_or(OpError::not_found("That task"))?;
        (workflow, row.0, row.1)
    };
    let group_of = |id: Uuid| {
        workflow
            .statuses
            .iter()
            .find(|status| status.id == id)
            .map(|status| status.category.as_str())
    };
    if group_of(current) == Some(group) {
        return Ok(Walked {
            version,
            arrived: true,
            status: Some(workflow.status_name(current).to_owned()),
        });
    }
    let Some(target) = workflow
        .statuses
        .iter()
        .find(|status| status.enabled && status.category == group)
    else {
        return Ok(Walked {
            version,
            arrived: false,
            status: None,
        });
    };
    let stays = from_groups
        .is_some_and(|groups| !group_of(current).is_some_and(|group| groups.contains(&group)));
    let path = if stays {
        None
    } else {
        workflow.shortest_path(current, target.id)
    };
    let Some(path) = path else {
        return Ok(Walked {
            version,
            arrived: false,
            status: Some(target.name.clone()),
        });
    };
    let base = request_id(ctx);
    for step in path {
        let mut step_ctx = ctx.clone();
        let digest = Sha256::digest(format!("{base}:{step}").as_bytes());
        step_ctx.request_id = Some(
            digest
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        let moved = update(
            state,
            &step_ctx,
            &json!({
                "taskId": task_id.to_string(),
                "expectedVersion": version,
                "status": step.to_string(),
            }),
        )
        .await?;
        version = moved.version;
    }
    Ok(Walked {
        version,
        arrived: true,
        status: Some(target.name.clone()),
    })
}

/// The space's tasks a worker can take now, best first (AWD-220,
/// `task_claim_next`): first those someone started and gave back (a
/// handoff, or a claim that ran out) and nobody holds, then those that can
/// start, as `task_ready_list` ranks them for this caller. `strict` keeps
/// only those that also pass the checks for unattended work.
pub(crate) async fn ready_candidates(
    state: &AppState,
    ctx: &OpContext,
    space: Uuid,
    strict: bool,
) -> Result<Vec<crate::ai::ready::RankedTask>, OpError> {
    use crate::ai::ready::{RankFilter, RankScope, rank};
    let principal = principal_in(ctx, Some(space));
    // Every page: the first tasks may all be held or someone else's, and
    // one further down is still the next to take.
    let (mut open, mut offset) = (Vec::new(), Some(0));
    while let Some(from) = offset.filter(|from| *from < MOST_CANDIDATES) {
        let ranking = rank(
            &state.pool,
            super::planning::reader(state, &principal),
            space,
            &RankFilter {
                scope: RankScope::All,
                milestone_id: None,
                limit: 200,
                offset: from,
            },
        )
        .await?;
        offset = ranking.next_offset;
        open.extend(ranking.tasks.into_iter().filter(|task| {
            if strict {
                task.readiness.ready_for_agents
            } else {
                task.readiness.ready
            }
        }));
    }
    let started = open
        .iter()
        .filter(|task| task.status_category == "started" && !task.claimed)
        .map(|task| task.id)
        .collect::<Vec<_>>();
    let given_back = crate::work_item_commands::claims::given_back(&state.pool, space, &started)
        .await
        .map_err(|error| OpError::logged("task_claim_next", &error))?;
    let (mut returned, mut fresh) = (Vec::new(), Vec::new());
    for task in open {
        if given_back.contains(&task.id) {
            returned.push(task);
        } else if matches!(task.status_category.as_str(), "backlog" | "unstarted") {
            fresh.push(task);
        }
    }
    returned.extend(fresh);
    Ok(returned)
}

/// How far down a space's open tasks `task_claim_next` looks.
const MOST_CANDIDATES: usize = 5_000;

pub(crate) async fn comment_edit(
    state: &AppState,
    ctx: &OpContext,
    task_id: Uuid,
    comment_id: Uuid,
    body: CommentText,
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
                text: body.text,
                expected_revision,
                content: body.content,
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
