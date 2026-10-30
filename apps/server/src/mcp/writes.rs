use std::collections::HashMap;

use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, READ_ONLY_MESSAGE, accessible_project_ids_in,
        archived_task_message, parse_id, public_url, version_conflict_message,
    },
    planning,
    workflow::Workflow,
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash, complete, reserve,
        resource_bound_scope,
    },
    message_content::MessageContent,
    policy::{Action, ActorKind, Decision},
    work_item_commands::{
        AllocatedNumberMetadata, ArchiveCommand, ArchiveOutcome, AuditAttribution,
        AuditVersionMetadata, BoardPlacement, ChangeFinalization, ChangePayload, CommandActor,
        ConflictKind, CreateWorkItemCommand, CreateWorkItemOutcome, LockedWorkItem,
        MAX_SUBTASK_LEVELS, NewTaskFields, ParentProblem, PlacementProblem, TaskFieldChanges,
        UpdateWorkItemCommand, UpdateWorkItemOutcome, advance_version, authorize_mutation,
        conflict_kind, create_work_item as execute_create_work_item, enqueue_update,
        finalize_change, finalize_comment, insert_activity as insert_work_item_activity,
        lock_membership, lock_work_item, normalize_criterion_text, place_in_column, record_audit,
        set_archived as execute_set_archived, update_work_item_by_id,
    },
    work_item_labels::{self, LabelDelta, MAX_LABELS_PER_TASK, echo},
};

const MAX_TITLE_CHARACTERS: usize = crate::work_item_commands::MAX_TITLE_CHARACTERS;
const MAX_UPDATE_CHARACTERS: usize = 20_000;
const MAX_BLOCKED_REASON_CHARACTERS: usize = 2_000;
/// `audit_events.request_id` holds at most 128 characters (migration 0004),
/// and MCP writes record the clientRequestId there.
const MAX_REQUEST_ID_BYTES: usize = 128;

const TASK_ID_MESSAGE: &str = "Pass the task's ID, link, or key such as AWD-12.";
const EXPECTED_VERSION_MESSAGE: &str =
    "Pass the version you last read (from task_open or task_list) as expectedVersion.";
pub(super) const REQUEST_ID_MESSAGE: &str = "Use a clientRequestId of 1 to 128 letters, digits, dashes, dots or underscores, and send the same one when you retry.";
pub(super) const TITLE_MESSAGE: &str = "Give it a title of 1 to 240 characters, on one line.";
pub(super) const DESCRIPTION_MESSAGE: &str = "The description can be at most 100,000 characters, with no control characters except line breaks and tabs.";
pub(super) const PRIORITY_MESSAGE: &str = "Use no_priority, low, medium, high, or urgent.";
const ASSIGNEE_ID_MESSAGE: &str = "Pass the assignee as \"me\", an @handle, or an actor ID.";
const ASSIGNEE_MESSAGE: &str =
    "That person can't be assigned tasks here: only active people and agents in the project can.";
pub(super) const STATUS_MESSAGE: &str =
    "Pass a status name, such as \"In Progress\" (see workflow_get).";
pub(super) const TYPE_MESSAGE: &str = "Pass a type name, such as \"Bug\" (see workflow_get).";
pub(super) const MILESTONE_MESSAGE: &str =
    "Pass an active milestone of this project by name or ID (see milestone_list), or \"none\".";
pub(super) const DATES_MESSAGE: &str = "The start date must be on or before the due date.";
const POSITION_MESSAGE: &str = "Use top, bottom, or a position number (1 is the top of the column). To put it next to a task, use beforeTaskId or afterTaskId instead.";
pub(super) const LABEL_COUNT_MESSAGE: &str = "Pass at most 20 labels at a time.";
const NOTHING_TO_CHANGE_MESSAGE: &str = "Pass at least one change: title, description, priority, assignee, type, status, position, parentId, clearParent, addLabels, removeLabels, milestone, startDate, dueDate, estimate, clearEstimate, or archived.";
/// The most acceptance criteria one task_create call takes.
const MAX_CRITERIA_ON_CREATE: usize = 50;
const TASK_NOT_FOUND_MESSAGE: &str = "That task wasn't found, or you can't see it.";

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetStatusArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub status: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostUpdateArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    /// The comment or update. It may span several lines and use Markdown.
    #[serde(rename = "text", alias = "update")]
    pub update: String,
    pub client_request_id: String,
    /// Files to post with it: up to 10 fileIds from file_upload with taskId
    /// and forComment: true on this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<String>>,
}

// The argument structs below are also each write's idempotency request hash
// (RequestHash::for_value), which follows field order. Fields added after
// the first release go at the end and are left out when absent, so a retry
// of an older call hashes exactly as it did.

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkItemArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub assignee_actor_id: Option<String>,
    pub clear_assignee: Option<bool>,
    pub client_request_id: String,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_parent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_task_id: Option<String>,
    /// An active milestone by name or ID, or "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// YYYY-MM-DD, or "" to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// YYYY-MM-DD, or "" to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_estimate: Option<bool>,
}

// The create arguments flatten NewTaskDetails, which serde can't combine
// with deny_unknown_fields; the tool's own arguments (tasks.rs) deny them.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSubtaskArguments {
    #[serde(rename = "parentId", alias = "parentWorkItemId")]
    pub parent_work_item_id: String,
    pub title: String,
    pub description: Option<String>,
    pub client_request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_actor_id: Option<String>,
    #[serde(flatten)]
    pub details: NewTaskDetails,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkItemArguments {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub assignee_actor_id: Option<String>,
    pub client_request_id: String,
    #[serde(flatten)]
    pub details: NewTaskDetails,
}

/// What a new task can be given beyond its title, description, priority
/// and assignee. Every field is left out of the request hash when absent.
#[derive(Clone, Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewTaskDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_criteria: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetArchivedArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub archived: bool,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkPullRequestArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub pull_request_id: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkBlockedArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub reason: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkReadyForReviewArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub work_item_id: String,
    pub expected_version: i64,
    pub client_request_id: String,
}

/// What a task write did. Fields added after the first release are optional,
/// so stored replays and existing clients keep working; human text goes in
/// `message`, never in `retryCategory`.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WriteResult {
    Applied {
        work_item_id: String,
        version: i64,
        changed_fields: Vec<String>,
        activity_id: Option<String>,
        result_id: String,
        idempotent_replay: bool,
        /// The task's key, such as AWD-12.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        /// Where people open the task.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Anything else worth knowing, in plain words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// The task's labels after the change, when the request changed them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        labels: Option<Vec<String>>,
    },
    Conflict {
        work_item_id: String,
        current_version: i64,
        current: Option<ConflictProjection>,
        changed_fields: Vec<String>,
        retry_category: String,
        /// What happened and what to do next.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    NotFound {
        retry_category: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    PermissionDenied {
        retry_category: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    InvalidRequest {
        field: String,
        retry_category: String,
        /// What's wrong with the field and what would work.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    InvalidTransition {
        current_status_id: String,
        requested_status_id: String,
        retry_category: String,
        /// The task's status now, by name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_status: Option<String>,
        /// The status asked for, by name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested_status: Option<String>,
        /// Where the task can go from its current status.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_statuses: Vec<StatusOption>,
        /// The shortest way to the requested status, by name, one status
        /// change per update; empty when it can't be reached.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        path: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

/// A status a task can move to.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusOption {
    pub id: String,
    pub name: String,
    /// backlog, unstarted, started, review, verification, completed, or canceled.
    pub category: String,
}

impl WriteResult {
    fn replayed(mut self) -> Self {
        if let Self::Applied {
            idempotent_replay, ..
        } = &mut self
        {
            *idempotent_replay = true;
        }
        self
    }

    fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            retry_category: "retry_with_backoff".to_owned(),
        }
    }

    pub(super) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            retry_category: "correct_input".to_owned(),
            message: Some(message.into()),
        }
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            retry_category: "do_not_retry".to_owned(),
            message: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConflictProjection {
    id: String,
    version: i64,
    title: String,
    status_id: String,
    priority: String,
    assignee_actor_id: Option<String>,
    archived: bool,
    blocked_reason: Option<String>,
}

impl From<&LockedWorkItem> for ConflictProjection {
    fn from(item: &LockedWorkItem) -> Self {
        Self {
            id: item.id.to_string(),
            version: item.version,
            title: item.title.clone(),
            status_id: item.status_id.to_string(),
            priority: item.priority.clone(),
            assignee_actor_id: item.assignee_actor_id.map(|id| id.to_string()),
            archived: item.archived,
            blocked_reason: item.blocked_reason.clone(),
        }
    }
}

pub async fn create_work_item(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CreateWorkItemArguments,
) -> WriteResult {
    let result = async {
        let request_hash = RequestHash::for_value(&arguments)?;
        create_task(
            state,
            principal,
            NewTask {
                parent: None,
                title: &arguments.title,
                description: arguments.description.as_deref(),
                priority: arguments.priority.as_deref(),
                assignee_actor_id: arguments.assignee_actor_id.as_deref(),
                details: &arguments.details,
                client_request_id: &arguments.client_request_id,
            },
            request_hash,
        )
        .await
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_create", error),
    }
}

/// Creates a subtask. It takes the same path as a top-level task, so a
/// subtask keeps its priority, assignee, status, type, labels, criteria and
/// place on the board; only its idempotency scope and request hash differ.
pub async fn create_subtask(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CreateSubtaskArguments,
) -> WriteResult {
    let result = async {
        let request_hash = RequestHash::for_value(&arguments)?;
        create_task(
            state,
            principal,
            NewTask {
                parent: Some(&arguments.parent_work_item_id),
                title: &arguments.title,
                description: arguments.description.as_deref(),
                priority: arguments.priority.as_deref(),
                assignee_actor_id: arguments.assignee_actor_id.as_deref(),
                details: &arguments.details,
                client_request_id: &arguments.client_request_id,
            },
            request_hash,
        )
        .await
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_create_subtask", error),
    }
}

/// One task_create call, top-level or a subtask.
struct NewTask<'a> {
    /// The parent as the call gave it; None for a top-level task.
    parent: Option<&'a str>,
    title: &'a str,
    description: Option<&'a str>,
    priority: Option<&'a str>,
    assignee_actor_id: Option<&'a str>,
    details: &'a NewTaskDetails,
    client_request_id: &'a str,
}

async fn create_task(
    state: &AppState,
    principal: &McpPrincipal,
    task: NewTask<'_>,
    request_hash: RequestHash,
) -> Result<WriteResult, AppError> {
    let parent_id = match task.parent {
        Some(parent) => match parse_id(parent) {
            Some(id) => Some(id),
            None => {
                return Ok(WriteResult::invalid(
                    "parentWorkItemId",
                    "Pass the parent task's ID, link, or key such as AWD-12.",
                ));
            }
        },
        None => None,
    };
    let Some(title) = normalize_text(task.title.to_owned(), MAX_TITLE_CHARACTERS, false) else {
        return Ok(WriteResult::invalid("title", TITLE_MESSAGE));
    };
    let description = match task.description {
        Some(value) => match normalize_description(value.to_owned()) {
            Some(value) => value,
            None => return Ok(WriteResult::invalid("description", DESCRIPTION_MESSAGE)),
        },
        None => String::new(),
    };
    let priority = task.priority.unwrap_or("no_priority").to_owned();
    if !valid_priority(&priority) {
        return Ok(WriteResult::invalid("priority", PRIORITY_MESSAGE));
    }
    let assignee_actor_id = match task.assignee_actor_id {
        Some(value) => match parse_id(value) {
            Some(actor_id) => Some(actor_id),
            None => return Ok(WriteResult::invalid("assigneeActorId", ASSIGNEE_ID_MESSAGE)),
        },
        None => None,
    };
    let details = task.details;
    if let Some(result) = check_name(details.status.as_deref(), "status", STATUS_MESSAGE)
        .or_else(|| check_name(details.item_type.as_deref(), "type", TYPE_MESSAGE))
    {
        return Ok(result);
    }
    let labels = details.labels.clone().unwrap_or_default();
    if labels.len() > MAX_LABELS_PER_TASK {
        return Ok(WriteResult::invalid("labels", LABEL_COUNT_MESSAGE));
    }
    let criteria = match normalize_criteria(details.acceptance_criteria.as_deref().unwrap_or(&[])) {
        Ok(criteria) => criteria,
        Err(message) => return Ok(WriteResult::invalid("acceptanceCriteria", message)),
    };
    let placement = match requested_placement(
        details.position.as_deref(),
        details.before_task_id.as_deref(),
        details.after_task_id.as_deref(),
    ) {
        Ok(placement) => placement,
        Err((field, message)) => return Ok(WriteResult::invalid(field, message)),
    };
    let mut dates = [None, None];
    for (slot, (field, value)) in dates.iter_mut().zip([
        ("startDate", details.start_date.as_deref()),
        ("dueDate", details.due_date.as_deref()),
    ]) {
        if let Some(value) = value {
            match planning::date_change(value) {
                Some(date) => *slot = date,
                None => return Ok(WriteResult::invalid(field, planning::DATE_MESSAGE)),
            }
        }
    }
    let [start_date, due_date] = dates;
    let estimate = match planning::estimate_change(details.estimate, None) {
        Ok(estimate) => estimate.flatten(),
        Err(message) => return Ok(WriteResult::invalid("estimate", message)),
    };
    let Some(key) = parse_request_id(task.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
    };
    let mut transaction = state.pool.begin().await?;
    if let Some(result) = authorize_write(&mut transaction, principal).await? {
        return Ok(result);
    }
    let scope = match parent_id {
        Some(parent_id) => resource_bound_scope("mcp_work_items", parent_id, "create_subtask")?,
        None => resource_bound_scope("mcp_work_items", principal.project_id, "create")?,
    };
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(replay.decode::<WriteResult>()?.replayed());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    // Every refusal below returns before the commit, so the reservation
    // rolls back and a corrected retry can reuse the clientRequestId.
    if let Some(parent_id) = parent_id
        && let Some(elsewhere) =
            parent_in_other_project(&mut transaction, principal, parent_id).await?
    {
        return Ok(WriteResult::invalid(
            "parentWorkItemId",
            format!(
                "The parent {} is in {}, not {}. Leave out projectId: a subtask goes in its parent's project.",
                elsewhere.key, elsewhere.project, elsewhere.here
            ),
        ));
    }
    let workflow = if details.status.is_some() || details.item_type.is_some() || placement.is_some()
    {
        Some(Workflow::load(&mut transaction, principal.project_id).await?)
    } else {
        None
    };
    let mut status_id = None;
    let mut type_id = None;
    if let Some(workflow) = &workflow {
        if let Some(status) = details.status.as_deref() {
            match workflow.status(status) {
                Ok(status) => status_id = Some(status.id),
                Err(message) => return Ok(WriteResult::invalid("status", message)),
            }
        }
        if let Some(item_type) = details.item_type.as_deref() {
            match workflow.work_type(item_type) {
                Ok(kind) => type_id = Some(kind.id),
                Err(message) => return Ok(WriteResult::invalid("type", message)),
            }
        }
    }
    let label_ids =
        match resolve_labels(&mut transaction, principal.project_id, &labels, "labels").await? {
            Ok(ids) => ids,
            Err(result) => return Ok(result),
        };
    let milestone_id = match details.milestone.as_deref() {
        Some(value) => {
            match planning::milestone_change(&mut transaction, principal.project_id, value).await? {
                Some(milestone) => milestone,
                None => return Ok(WriteResult::invalid("milestone", MILESTONE_MESSAGE)),
            }
        }
        None => None,
    };
    let (summary, audit_action, changed_field, metadata, number_metadata) = match parent_id {
        Some(parent_id) => (
            "Subtask created through MCP",
            "work_item.mcp_subtask_created",
            "subtask",
            serde_json::json!({"parentWorkItemId": parent_id}),
            AllocatedNumberMetadata::Omit,
        ),
        None => (
            "Work Item created through MCP",
            "work_item.mcp_created",
            "work_item",
            serde_json::json!({}),
            AllocatedNumberMetadata::Include,
        ),
    };
    let outcome = execute_create_work_item(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        CreateWorkItemCommand {
            title: &title,
            description: &description,
            type_id,
            status_id,
            priority: &priority,
            assignee_actor_id,
            parent_work_item_id: parent_id,
            label_ids: &label_ids,
            acceptance_criteria: &criteria,
            board_placement: placement,
            fields: NewTaskFields {
                milestone_id,
                start_date,
                due_date,
                estimate,
            },
            activity_summary: summary,
            activity_metadata: metadata.clone(),
            activity_number_metadata: number_metadata,
            audit_action,
            audit_metadata: metadata,
        },
    )
    .await?;
    let (work_item_id, finalized) = match outcome {
        CreateWorkItemOutcome::Created {
            work_item_id,
            finalized,
            ..
        } => (work_item_id, finalized),
        CreateWorkItemOutcome::InvalidAssignee => {
            return Ok(WriteResult::invalid("assigneeActorId", ASSIGNEE_MESSAGE));
        }
        CreateWorkItemOutcome::InvalidMilestone => {
            return Ok(WriteResult::invalid("milestone", MILESTONE_MESSAGE));
        }
        CreateWorkItemOutcome::InvalidDates => {
            return Ok(WriteResult::invalid("dueDate", DATES_MESSAGE));
        }
        CreateWorkItemOutcome::ParentNotFound => {
            return Ok(WriteResult::not_found(
                "That parent task wasn't found, or you can't see it.",
            ));
        }
        CreateWorkItemOutcome::ParentArchived(parent) => {
            let task = task_key(&mut transaction, principal.project_id, parent.number).await?;
            return Ok(WriteResult::Conflict {
                work_item_id: parent.id.to_string(),
                current_version: parent.version,
                current: Some((&parent).into()),
                changed_fields: vec!["archived".to_owned()],
                retry_category: "do_not_retry".to_owned(),
                message: Some(format!(
                    "{task} is archived. Restore it first, or pick another parent."
                )),
            });
        }
        CreateWorkItemOutcome::ParentTooDeep => {
            return Ok(WriteResult::invalid(
                "parentWorkItemId",
                parent_problem_text(&ParentProblem::TooDeep, &HashMap::new()),
            ));
        }
        CreateWorkItemOutcome::InvalidPlacement(problem) => {
            let column = match (status_id, &workflow) {
                (Some(status_id), _) => Some(status_id),
                (None, Some(workflow)) => workflow.default_status().map(|status| status.id),
                (None, None) => None,
            }
            .ok_or(AppError::Internal("a placed task has a column"))?;
            return placement_problem(
                &mut transaction,
                principal.project_id,
                column,
                &problem,
                placement_field(placement),
            )
            .await;
        }
    };
    let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
    let mut result = applied(
        work_item_id,
        1,
        vec![changed_field.to_owned()],
        Some(finalized.activity_id),
        finalized.audit_id,
        location,
    );
    if !labels.is_empty() {
        result = with_labels(&mut transaction, result, principal.project_id, work_item_id).await?;
    }
    complete(&mut transaction, &reservation, StatusCode::CREATED, &result).await?;
    transaction.commit().await?;
    Ok(result)
}

pub async fn set_status(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SetStatusArguments,
) -> WriteResult {
    match set_status_inner(state, principal, arguments).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_set_status", error),
    }
}

async fn set_status_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SetStatusArguments,
) -> Result<WriteResult, AppError> {
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
    };
    if arguments.expected_version < 1 {
        return Ok(WriteResult::invalid("status", EXPECTED_VERSION_MESSAGE));
    }
    if arguments.status.is_empty() || arguments.status.len() > 80 {
        return Ok(WriteResult::invalid("status", STATUS_MESSAGE));
    }
    let Some(key) = parse_request_id(&arguments.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
    };
    let mut transaction = state.pool.begin().await?;
    if let Some(result) = authorize_write(&mut transaction, principal).await? {
        return Ok(result);
    }
    let scope = resource_bound_scope("mcp_work_items", work_item_id, "set_status")?;
    let request_hash = RequestHash::for_value(&arguments)?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(replay.decode::<WriteResult>()?.replayed());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let workflow = Workflow::load(&mut transaction, principal.project_id).await?;
    let status_id = match workflow.status(&arguments.status) {
        Ok(status) => status.id,
        Err(message) => return Ok(WriteResult::invalid("status", message)),
    };
    let outcome = update_work_item_by_id(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: arguments.expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id: Some(status_id),
            priority: None,
            assignee_actor_id: None,
            parent_work_item_id: None,
            board_placement: None,
            labels: None,
            fields: Default::default(),
            summary: "Work Item status updated through MCP",
            audit_action: "work_item.mcp_status_updated",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    finish_update_outcome(
        transaction,
        state,
        principal,
        &reservation,
        arguments.expected_version,
        outcome,
        "work_item.mcp_status_unchanged",
        key.as_str(),
        false,
    )
    .await
}

pub async fn update_work_item(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UpdateWorkItemArguments,
) -> WriteResult {
    match update_work_item_inner(state, principal, arguments, None).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_update", error),
    }
}

/// Changes fields and the status together in one transaction, so a rejected
/// status leaves the fields untouched too.
pub async fn update_work_item_and_status(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UpdateWorkItemArguments,
    status: String,
) -> WriteResult {
    match update_work_item_inner(state, principal, arguments, Some(status)).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_update", error),
    }
}

async fn update_work_item_inner(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: UpdateWorkItemArguments,
    status: Option<String>,
) -> Result<WriteResult, AppError> {
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
    };
    if arguments.expected_version < 1 {
        return Ok(WriteResult::invalid("update", EXPECTED_VERSION_MESSAGE));
    }
    let add_labels = arguments.add_labels.clone().unwrap_or_default();
    let remove_labels = arguments.remove_labels.clone().unwrap_or_default();
    let clear_parent = arguments.clear_parent == Some(true);
    let mut dates = [None, None];
    for (slot, (field, value)) in dates.iter_mut().zip([
        ("startDate", arguments.start_date.as_deref()),
        ("dueDate", arguments.due_date.as_deref()),
    ]) {
        if let Some(value) = value {
            match planning::date_change(value) {
                Some(date) => *slot = Some(date),
                None => return Ok(WriteResult::invalid(field, planning::DATE_MESSAGE)),
            }
        }
    }
    let [start_date, due_date] = dates;
    let estimate = match planning::estimate_change(arguments.estimate, arguments.clear_estimate) {
        Ok(estimate) => estimate,
        Err(message) => return Ok(WriteResult::invalid("estimate", message)),
    };
    if status.is_none()
        && arguments.title.is_none()
        && arguments.description.is_none()
        && arguments.priority.is_none()
        && arguments.assignee_actor_id.is_none()
        && arguments.clear_assignee != Some(true)
        && arguments.item_type.is_none()
        && arguments.parent_id.is_none()
        && !clear_parent
        && add_labels.is_empty()
        && remove_labels.is_empty()
        && arguments.position.is_none()
        && arguments.before_task_id.is_none()
        && arguments.after_task_id.is_none()
        && arguments.milestone.is_none()
        && start_date.is_none()
        && due_date.is_none()
        && estimate.is_none()
    {
        return Ok(WriteResult::invalid("update", NOTHING_TO_CHANGE_MESSAGE));
    }
    if arguments.clear_assignee == Some(true) && arguments.assignee_actor_id.is_some() {
        return Ok(WriteResult::invalid(
            "update",
            "Pass assignee or clearAssignee, not both.",
        ));
    }
    if clear_parent && arguments.parent_id.is_some() {
        return Ok(WriteResult::invalid(
            "parentId",
            "Pass parentId or clearParent, not both.",
        ));
    }
    if let Some(result) = check_name(status.as_deref(), "status", STATUS_MESSAGE)
        .or_else(|| check_name(arguments.item_type.as_deref(), "type", TYPE_MESSAGE))
    {
        return Ok(result);
    }
    arguments.title = match arguments
        .title
        .take()
        .map(|value| normalize_text(value, MAX_TITLE_CHARACTERS, false))
    {
        Some(Some(value)) => Some(value),
        Some(None) => return Ok(WriteResult::invalid("title", TITLE_MESSAGE)),
        None => None,
    };
    arguments.description = match arguments.description.take().map(normalize_description) {
        Some(Some(value)) => Some(value),
        Some(None) => return Ok(WriteResult::invalid("description", DESCRIPTION_MESSAGE)),
        None => None,
    };
    if arguments
        .priority
        .as_deref()
        .is_some_and(|priority| !valid_priority(priority))
    {
        return Ok(WriteResult::invalid("priority", PRIORITY_MESSAGE));
    }
    let assignee_actor_id = match arguments.assignee_actor_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(actor_id) => Some(actor_id),
            None => return Ok(WriteResult::invalid("assigneeActorId", ASSIGNEE_ID_MESSAGE)),
        },
        None => None,
    };
    let parent_id = match arguments.parent_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => {
                return Ok(WriteResult::invalid(
                    "parentId",
                    format!(
                        "No task {} that you can reach. Pass the parent's ID, link, or key such as AWD-3.",
                        echo(value)
                    ),
                ));
            }
        },
        None => None,
    };
    for (field, list) in [("addLabels", &add_labels), ("removeLabels", &remove_labels)] {
        if list.len() > MAX_LABELS_PER_TASK {
            return Ok(WriteResult::invalid(field, LABEL_COUNT_MESSAGE));
        }
    }
    let placement = match requested_placement(
        arguments.position.as_deref(),
        arguments.before_task_id.as_deref(),
        arguments.after_task_id.as_deref(),
    ) {
        Ok(placement) => placement,
        Err((field, message)) => return Ok(WriteResult::invalid(field, message)),
    };
    let Some(key) = parse_request_id(&arguments.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
    };
    let mut transaction = state.pool.begin().await?;
    if let Some(result) = authorize_write(&mut transaction, principal).await? {
        return Ok(result);
    }
    let (scope, request_hash) = match &status {
        None => (
            resource_bound_scope("mcp_work_items", work_item_id, "update")?,
            RequestHash::for_value(&arguments)?,
        ),
        Some(status) => (
            resource_bound_scope("mcp_work_items", work_item_id, "update_with_status")?,
            RequestHash::for_value(&(&arguments, status))?,
        ),
    };
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(replay.decode::<WriteResult>()?.replayed());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    // Every refusal below returns before the commit, so the reservation
    // rolls back and a corrected retry can reuse the clientRequestId.
    let assignee_update = if arguments.clear_assignee == Some(true) {
        Some(None)
    } else {
        assignee_actor_id.map(Some)
    };
    let mut status_id = None;
    let mut type_id = None;
    if status.is_some() || arguments.item_type.is_some() {
        let workflow = Workflow::load(&mut transaction, principal.project_id).await?;
        if let Some(status) = &status {
            match workflow.status(status) {
                Ok(status) => status_id = Some(status.id),
                Err(message) => return Ok(WriteResult::invalid("status", message)),
            }
        }
        if let Some(item_type) = arguments.item_type.as_deref() {
            match workflow.work_type(item_type) {
                Ok(kind) => type_id = Some(kind.id),
                Err(message) => return Ok(WriteResult::invalid("type", message)),
            }
        }
    }
    if let Some(parent_id) = parent_id
        && let Some(elsewhere) =
            parent_in_other_project(&mut transaction, principal, parent_id).await?
    {
        return Ok(WriteResult::invalid(
            "parentId",
            format!(
                "{} is in {}, but this task is in {}. A subtask must be in its parent's project; move one of them first with task_move.",
                elsewhere.key, elsewhere.project, elsewhere.here
            ),
        ));
    }
    let add = match resolve_labels(
        &mut transaction,
        principal.project_id,
        &add_labels,
        "addLabels",
    )
    .await?
    {
        Ok(ids) => ids,
        Err(result) => return Ok(result),
    };
    let remove = match resolve_labels(
        &mut transaction,
        principal.project_id,
        &remove_labels,
        "removeLabels",
    )
    .await?
    {
        Ok(ids) => ids,
        Err(result) => return Ok(result),
    };
    if let Some(both) = add.iter().find(|id| remove.contains(id)) {
        let name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM project_labels WHERE project_id = $1 AND id = $2",
        )
        .bind(principal.project_id)
        .bind(both)
        .fetch_one(&mut *transaction)
        .await?;
        return Ok(WriteResult::invalid(
            "removeLabels",
            format!("\"{name}\" is in both addLabels and removeLabels. Pick one."),
        ));
    }
    let milestone_id = match arguments.milestone.as_deref() {
        Some(value) => {
            match planning::milestone_change(&mut transaction, principal.project_id, value).await? {
                Some(milestone) => Some(milestone),
                None => return Ok(WriteResult::invalid("milestone", MILESTONE_MESSAGE)),
            }
        }
        None => None,
    };
    let labels_touched = !add.is_empty() || !remove.is_empty();
    let outcome = update_work_item_by_id(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: arguments.expected_version,
            title: arguments.title.as_deref(),
            description: arguments.description.as_deref(),
            type_id,
            status_id,
            priority: arguments.priority.as_deref(),
            assignee_actor_id: assignee_update,
            parent_work_item_id: if clear_parent {
                Some(None)
            } else {
                parent_id.map(Some)
            },
            board_placement: placement,
            labels: labels_touched.then_some(LabelDelta {
                add: &add,
                remove: &remove,
            }),
            fields: TaskFieldChanges {
                milestone_id,
                start_date,
                due_date,
                estimate,
                blocked_reason: None,
            },
            summary: "Work Item updated through MCP",
            audit_action: "work_item.mcp_updated",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    if let UpdateWorkItemOutcome::InvalidPlacement(problem) = &outcome {
        // The task is locked and unchanged, so its status is still the one
        // it had when it was placed.
        let column = match status_id {
            Some(status_id) => status_id,
            None => {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT status_id FROM work_items WHERE project_id = $1 AND id = $2",
                )
                .bind(principal.project_id)
                .bind(work_item_id)
                .fetch_one(&mut *transaction)
                .await?
            }
        };
        return placement_problem(
            &mut transaction,
            principal.project_id,
            column,
            problem,
            placement_field(placement),
        )
        .await;
    }
    finish_update_outcome(
        transaction,
        state,
        principal,
        &reservation,
        arguments.expected_version,
        outcome,
        "work_item.mcp_update_unchanged",
        key.as_str(),
        labels_touched,
    )
    .await
}

pub async fn post_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PostUpdateArguments,
) -> WriteResult {
    match post_update_inner(state, principal, arguments).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_post_update", error),
    }
}

async fn post_update_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PostUpdateArguments,
) -> Result<WriteResult, AppError> {
    const UPDATE_MESSAGE: &str =
        "Write the comment as text, 1 to 20,000 characters. Line breaks and Markdown are kept.";
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
    };
    let Some(update) = normalize_comment(arguments.update.clone()) else {
        return Ok(WriteResult::invalid("update", UPDATE_MESSAGE));
    };
    let validated = match MessageContent::from_plain_text(update.clone()) {
        Ok(content) => content,
        Err(_) => return Ok(WriteResult::invalid("update", UPDATE_MESSAGE)),
    };
    let Some(key) = parse_request_id(&arguments.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
    };
    let Ok(attachments) = super::file_tools::parse_attachment_ids(arguments.attachment_ids.clone())
    else {
        return Ok(WriteResult::invalid(
            "attachmentIds",
            "Pass up to 10 file IDs returned by file_upload (taskId, forComment: true).",
        ));
    };
    let mut transaction = state.pool.begin().await?;
    if let Some(result) = authorize_write(&mut transaction, principal).await? {
        return Ok(result);
    }
    let scope = resource_bound_scope("mcp_work_items", work_item_id, "post_update")?;
    let request_hash = RequestHash::for_value(&arguments)?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(replay.decode::<WriteResult>()?.replayed());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let Some(current) =
        lock_work_item(&mut transaction, principal.project_id, work_item_id).await?
    else {
        return Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE));
    };
    if current.archived {
        let task = task_key(&mut transaction, principal.project_id, current.number).await?;
        return Ok(WriteResult::Conflict {
            work_item_id: work_item_id.to_string(),
            current_version: current.version,
            current: Some((&current).into()),
            changed_fields: vec!["archived".to_owned()],
            retry_category: "do_not_retry".to_owned(),
            message: Some(archived_task_message(&task)),
        });
    }
    let comment_id = Uuid::now_v7();
    let content = serde_json::to_value(validated.content())
        .map_err(|_| AppError::Internal("serialize MCP update"))?;
    sqlx::query(
        r#"
        INSERT INTO work_item_comments (
            id, project_id, work_item_id, author_actor_id, content_version,
            content, plain_text
        ) VALUES ($1, $2, $3, $4, 1, $5, $6)
        "#,
    )
    .bind(comment_id)
    .bind(principal.project_id)
    .bind(work_item_id)
    .bind(principal.actor_id)
    .bind(content)
    .bind(update)
    .execute(&mut *transaction)
    .await?;
    let (_sequence, result_id) = finalize_comment(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        work_item_id,
        comment_id,
        current.version,
        "work_item.mcp_update_posted",
    )
    .await?;
    match crate::files::attach_to_comment(
        &mut transaction,
        principal.project_id,
        work_item_id,
        comment_id,
        principal.actor_id,
        &attachments,
    )
    .await
    {
        Ok(_) => {}
        Err(AppError::Validation(_)) => {
            return Ok(WriteResult::invalid(
                "attachmentIds",
                "Those files aren't your unsent uploads for a comment on this task.",
            ));
        }
        Err(error) => return Err(error),
    }
    let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
    let result = applied(
        work_item_id,
        current.version,
        vec!["update".to_owned()],
        Some(comment_id),
        result_id,
        location,
    );
    complete(&mut transaction, &reservation, StatusCode::CREATED, &result).await?;
    transaction.commit().await?;
    Ok(result)
}

pub async fn link_pull_request(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LinkPullRequestArguments,
) -> WriteResult {
    let result = async {
        let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
            return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
        };
        let Some(pull_request_id) = parse_id(&arguments.pull_request_id) else {
            return Ok(WriteResult::invalid(
                "pullRequestId",
                "Pass the pull request's ID (see pull_request_get).",
            ));
        };
        if arguments.expected_version < 1 {
            return Ok(WriteResult::invalid(
                "expectedVersion",
                EXPECTED_VERSION_MESSAGE,
            ));
        }
        let Some(key) = parse_request_id(&arguments.client_request_id) else {
            return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? {
            return Ok(result);
        }
        let scope = resource_bound_scope("mcp_work_items", work_item_id, "link_pull_request")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await?
        {
            IdempotencyOutcome::Replay(replay) => {
                transaction.commit().await?;
                return Ok(replay.decode::<WriteResult>()?.replayed());
            }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(current) =
            lock_work_item(&mut transaction, principal.project_id, work_item_id).await?
        else {
            return Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE));
        };
        if let Some(conflict) = conflict_for(
            &mut transaction,
            principal.project_id,
            &current,
            arguments.expected_version,
        )
        .await?
        {
            return Ok(conflict);
        }
        let (changed, version, activity_id) = match change_pull_request_link(
            &mut transaction,
            principal.project_id,
            work_item_id,
            current.version,
            None,
            pull_request_id,
            principal.actor_id,
            true,
            "Pull request linked through MCP",
        )
        .await?
        {
            PullRequestLinkChange::NotInProject => {
                return Ok(WriteResult::not_found(
                    "That pull request wasn't found in this task's project.",
                ));
            }
            PullRequestLinkChange::Unchanged => (false, current.version, None),
            PullRequestLinkChange::Changed {
                version,
                activity_id,
            } => (true, version, Some(activity_id)),
            PullRequestLinkChange::VersionConflict => {
                return Err(AppError::Internal("pull request link checked its version"));
            }
        };
        let result_id = insert_mcp_audit(
            &mut transaction,
            principal,
            "work_item.mcp_pull_request_linked",
            work_item_id,
            key.as_str(),
            serde_json::json!({"pullRequestId": pull_request_id, "changed": changed}),
        )
        .await?;
        let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
        let result = applied(
            work_item_id,
            version,
            if changed {
                vec!["pullRequest".to_owned(), "version".to_owned()]
            } else {
                Vec::new()
            },
            activity_id,
            result_id,
            location,
        );
        complete(&mut transaction, &reservation, StatusCode::OK, &result).await?;
        transaction.commit().await?;
        Ok(result)
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_link_pull_request", error),
    }
}

/// What linking or unlinking a pull request did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PullRequestLinkChange {
    /// No such pull request in the task's project.
    NotInProject,
    Unchanged,
    /// The change would apply, but the task isn't at the version given.
    VersionConflict,
    Changed {
        version: i64,
        activity_id: Uuid,
    },
}

/// Links a pull request Agent Workspace knows about to a task in the same
/// project, or unlinks it, for task_link and task_link_pull_request alike. A
/// link made here is explicit and confirmed, so the project's GitHub
/// automation moves the task when the pull request opens or merges; a
/// suggested link (from a branch name) becomes explicit. A change bumps the
/// version and writes the activity and the outbox event; the caller has
/// locked the project and the task and writes the audit row.
/// `expected_version` is checked only when something would change, so a
/// retry of a link that's already there answers `Unchanged`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn change_pull_request_link(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    current_version: i64,
    expected_version: Option<i64>,
    pull_request_id: Uuid,
    actor_id: Uuid,
    link: bool,
    activity_summary: &str,
) -> Result<PullRequestLinkChange, AppError> {
    // Nobody links (or unlinks) a pull request they can't see: one opened
    // from a private thread stays private (migration 0050).
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM github_pull_requests WHERE project_id = $1 AND id = $2 AND pull_request_visible_to(id, $3))",
    )
    .bind(project_id)
    .bind(pull_request_id)
    .bind(actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !exists {
        return Ok(PullRequestLinkChange::NotInProject);
    }
    let source = sqlx::query_scalar::<_, String>(
        "SELECT source FROM github_pull_request_work_item_links WHERE project_id = $1 AND github_pull_request_id = $2 AND work_item_id = $3",
    )
    .bind(project_id)
    .bind(pull_request_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let already = if link {
        source.as_deref() == Some("explicit")
    } else {
        source.is_none()
    };
    if already {
        return Ok(PullRequestLinkChange::Unchanged);
    }
    if expected_version.is_some_and(|expected| expected != current_version) {
        return Ok(PullRequestLinkChange::VersionConflict);
    }
    let changed = if link {
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_work_item_links (
                project_id, github_pull_request_id, work_item_id, source,
                confirmed, created_by_actor_id
            ) VALUES ($1, $2, $3, 'explicit', true, $4)
            ON CONFLICT (github_pull_request_id, work_item_id) DO UPDATE
            SET source = 'explicit', confirmed = true
            WHERE github_pull_request_work_item_links.source <> 'explicit'
            "#,
        )
        .bind(project_id)
        .bind(pull_request_id)
        .bind(work_item_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?
    } else {
        sqlx::query(
            "DELETE FROM github_pull_request_work_item_links WHERE project_id = $1 AND work_item_id = $2 AND github_pull_request_id = $3",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(pull_request_id)
        .execute(&mut **transaction)
        .await?
    }
    .rows_affected()
        == 1;
    if !changed {
        return Ok(PullRequestLinkChange::Unchanged);
    }
    let version = advance_version(transaction, project_id, work_item_id, current_version).await?;
    let activity_id = insert_work_item_activity(
        transaction,
        project_id,
        work_item_id,
        actor_id,
        "work_item_updated",
        activity_summary,
        &serde_json::json!({"pullRequestId": pull_request_id, "linked": link}),
    )
    .await?;
    enqueue_update(transaction, project_id, work_item_id, version).await?;
    Ok(PullRequestLinkChange::Changed {
        version,
        activity_id,
    })
}

pub async fn mark_blocked(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MarkBlockedArguments,
) -> WriteResult {
    let result = async {
        let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
            return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
        };
        let Some(reason) = normalize_text(
            arguments.reason.clone(),
            MAX_BLOCKED_REASON_CHARACTERS,
            false,
        ) else {
            return Ok(WriteResult::invalid(
                "reason",
                "Say what's blocking it in 1 to 2,000 characters, on one line.",
            ));
        };
        if arguments.expected_version < 1 {
            return Ok(WriteResult::invalid(
                "expectedVersion",
                EXPECTED_VERSION_MESSAGE,
            ));
        }
        let Some(key) = parse_request_id(&arguments.client_request_id) else {
            return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? {
            return Ok(result);
        }
        let scope = resource_bound_scope("mcp_work_items", work_item_id, "mark_blocked")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await?
        {
            IdempotencyOutcome::Replay(replay) => {
                transaction.commit().await?;
                return Ok(replay.decode::<WriteResult>()?.replayed());
            }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(current) =
            lock_work_item(&mut transaction, principal.project_id, work_item_id).await?
        else {
            return Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE));
        };
        if let Some(conflict) = conflict_for(
            &mut transaction,
            principal.project_id,
            &current,
            arguments.expected_version,
        )
        .await?
        {
            return Ok(conflict);
        }
        if current.blocked_reason.as_deref() == Some(&reason) {
            return finalize_noop(
                transaction,
                state,
                principal,
                &reservation,
                work_item_id,
                current.version,
                "work_item.mcp_blocker_unchanged",
                key.as_str(),
                false,
            )
            .await;
        }
        sqlx::query(
            "UPDATE work_items SET blocked_reason = $3, blocked_at = now(), blocked_by_actor_id = $4, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $5",
        )
        .bind(principal.project_id)
        .bind(work_item_id)
        .bind(reason)
        .bind(principal.actor_id)
        .bind(arguments.expected_version)
        .execute(&mut *transaction)
        .await?;
        finalize_work_item_change(
            transaction,
            state,
            principal,
            &reservation,
            work_item_id,
            arguments.expected_version + 1,
            ChangePayload {
                changed_fields: vec!["blockedReason".to_owned()],
                ..ChangePayload::default()
            },
            "Work Item marked blocked through MCP",
            "work_item.mcp_marked_blocked",
            key.as_str(),
        )
        .await
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_mark_blocked", error),
    }
}

pub async fn mark_ready_for_review(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MarkReadyForReviewArguments,
) -> WriteResult {
    let result = async {
        let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
            return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
        };
        if arguments.expected_version < 1 {
            return Ok(WriteResult::invalid(
                "expectedVersion",
                EXPECTED_VERSION_MESSAGE,
            ));
        }
        let Some(key) = parse_request_id(&arguments.client_request_id) else {
            return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? {
            return Ok(result);
        }
        let scope = resource_bound_scope("mcp_work_items", work_item_id, "ready_for_review")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await?
        {
            IdempotencyOutcome::Replay(replay) => {
                transaction.commit().await?;
                return Ok(replay.decode::<WriteResult>()?.replayed());
            }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(current) =
            lock_work_item(&mut transaction, principal.project_id, work_item_id).await?
        else {
            return Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE));
        };
        if let Some(conflict) = conflict_for(
            &mut transaction,
            principal.project_id,
            &current,
            arguments.expected_version,
        )
        .await?
        {
            return Ok(conflict);
        }
        let workflow = Workflow::load(&mut transaction, principal.project_id).await?;
        let Ok(review) = workflow.status("review") else {
            return Ok(WriteResult::invalid(
                "reviewStatus",
                "This project has no single review status (see workflow_get). Move the task with task_update instead.",
            ));
        };
        let review_status_id = review.id;
        if current.status_id != review_status_id
            && !workflow.allows(current.status_id, review_status_id)
        {
            return Ok(invalid_transition(
                &workflow,
                current.status_id,
                review_status_id,
            ));
        }
        let mut changed = Vec::new();
        if current.status_id != review_status_id {
            changed.push("status".to_owned());
        }
        if current.blocked_reason.is_some() {
            changed.push("blockedReason".to_owned());
        }
        if changed.is_empty() {
            return finalize_noop(
                transaction,
                state,
                principal,
                &reservation,
                work_item_id,
                current.version,
                "work_item.mcp_review_unchanged",
                key.as_str(),
                false,
            )
            .await;
        }
        // Like any status change, it goes to the bottom of its new column.
        let board_rank = if current.status_id == review_status_id {
            current.board_rank
        } else {
            place_in_column(
                &mut transaction,
                principal.project_id,
                review_status_id,
                work_item_id,
                Some((current.status_id, current.board_rank)),
                BoardPlacement::Bottom,
            )
            .await?
            .map_err(|_| AppError::Internal("place a task at the bottom of review"))?
            .board_rank
        };
        sqlx::query(
            "UPDATE work_items SET status_id = $3, board_rank = $5, blocked_reason = NULL, blocked_at = NULL, blocked_by_actor_id = NULL, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $4",
        )
        .bind(principal.project_id)
        .bind(work_item_id)
        .bind(review_status_id)
        .bind(arguments.expected_version)
        .bind(board_rank)
        .execute(&mut *transaction)
        .await?;
        finalize_work_item_change(
            transaction,
            state,
            principal,
            &reservation,
            work_item_id,
            arguments.expected_version + 1,
            ChangePayload {
                status: (current.status_id != review_status_id)
                    .then_some((current.status_id, review_status_id)),
                changed_fields: changed,
                ..ChangePayload::default()
            },
            "Work Item marked ready for review through MCP",
            "work_item.mcp_ready_for_review",
            key.as_str(),
        )
        .await
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_mark_ready_for_review", error),
    }
}

/// Archives or restores a task through the one archive command the web app
/// uses too. Archiving releases the task's claim; restoring the archived
/// original of a moved task is refused.
pub async fn set_archived(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SetArchivedArguments,
) -> WriteResult {
    match set_archived_inner(state, principal, arguments).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_set_archived", error),
    }
}

async fn set_archived_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SetArchivedArguments,
) -> Result<WriteResult, AppError> {
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
    };
    if arguments.expected_version < 1 {
        return Ok(WriteResult::invalid(
            "expectedVersion",
            EXPECTED_VERSION_MESSAGE,
        ));
    }
    let Some(key) = parse_request_id(&arguments.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
    };
    let mut transaction = state.pool.begin().await?;
    // Archiving takes "Archive tasks", not "Edit tasks".
    if let Some(result) =
        authorize_write_for(&mut transaction, principal, Action::WorkItemArchive).await?
    {
        return Ok(result);
    }
    let scope = resource_bound_scope(
        "mcp_work_items",
        work_item_id,
        if arguments.archived {
            "archive"
        } else {
            "restore"
        },
    )?;
    let request_hash = RequestHash::for_value(&arguments)?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            transaction.commit().await?;
            return Ok(replay.decode::<WriteResult>()?.replayed());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let (summary, audit_action, unchanged_action) = if arguments.archived {
        (
            "Work Item archived through MCP",
            "work_item.mcp_archived",
            "work_item.mcp_archive_unchanged",
        )
    } else {
        (
            "Work Item restored through MCP",
            "work_item.mcp_restored",
            "work_item.mcp_restore_unchanged",
        )
    };
    let outcome = execute_set_archived(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        work_item_id,
        ArchiveCommand {
            expected_version: arguments.expected_version,
            archived: arguments.archived,
            summary,
            audit_action,
        },
    )
    .await?;
    match outcome {
        ArchiveOutcome::Applied {
            version,
            finalized,
            open_subtasks,
        } => {
            let location =
                locate(&mut transaction, state, principal.project_id, work_item_id).await?;
            let mut result = applied(
                work_item_id,
                version,
                vec!["archived".to_owned()],
                Some(finalized.activity_id),
                finalized.audit_id,
                location,
            );
            if !open_subtasks.is_empty()
                && let WriteResult::Applied { message, .. } = &mut result
            {
                let keys =
                    keys_in_project(&mut transaction, principal.project_id, &open_subtasks).await?;
                let names = open_subtasks
                    .iter()
                    .filter_map(|id| keys.get(id).cloned())
                    .collect::<Vec<_>>();
                let more = if open_subtasks.len() >= 20 {
                    " (and maybe more; task_list with parentId lists them)"
                } else {
                    ""
                };
                *message = Some(if names.len() == 1 {
                    format!("Archived. Its subtask {} is still open{more}.", names[0])
                } else {
                    format!(
                        "Archived. Its subtasks {} are still open{more}.",
                        join_and(&names)
                    )
                });
            }
            complete(&mut transaction, &reservation, StatusCode::OK, &result).await?;
            transaction.commit().await?;
            Ok(result)
        }
        ArchiveOutcome::Unchanged(current) => {
            finalize_noop(
                transaction,
                state,
                principal,
                &reservation,
                current.id,
                current.version,
                unchanged_action,
                key.as_str(),
                false,
            )
            .await
        }
        ArchiveOutcome::VersionConflict(current) => Ok(conflict_for(
            &mut transaction,
            principal.project_id,
            &current,
            arguments.expected_version,
        )
        .await?
        .ok_or(AppError::Internal("archive returned an empty conflict"))?),
        ArchiveOutcome::MovedAway { moved_to } => {
            let (number, version) = sqlx::query_as::<_, (i64, i64)>(
                "SELECT number, version FROM work_items WHERE project_id = $1 AND id = $2",
            )
            .bind(principal.project_id)
            .bind(work_item_id)
            .fetch_one(&mut *transaction)
            .await?;
            let original = task_key(&mut transaction, principal.project_id, number).await?;
            // Name the new task only to someone who can reach its project.
            let moved = sqlx::query_as::<_, (Uuid, String)>(
                r#"
                SELECT item.project_id, project.project_key || '-' || item.number::text
                FROM work_items item JOIN projects project ON project.id = item.project_id
                WHERE item.id = $1
                "#,
            )
            .bind(moved_to)
            .fetch_optional(&mut *transaction)
            .await?;
            let reachable = accessible_project_ids_in(&mut *transaction, principal).await?;
            let message = moved_away_message(
                &original,
                moved
                    .as_ref()
                    .filter(|(project_id, _)| reachable.contains(project_id))
                    .map(|(_, moved_key)| moved_key.as_str()),
            );
            Ok(WriteResult::Conflict {
                work_item_id: work_item_id.to_string(),
                current_version: version,
                current: None,
                changed_fields: vec!["archived".to_owned()],
                retry_category: "do_not_retry".to_owned(),
                message: Some(message),
            })
        }
        ArchiveOutcome::NotFound => Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE)),
    }
}

/// Why the archived original of a moved task can't be restored. `moved_to`
/// is the new task's key, given only when the caller can reach its project.
pub(super) fn moved_away_message(original: &str, moved_to: Option<&str>) -> String {
    match moved_to {
        Some(moved_key) => format!(
            "{original} was moved to {moved_key}. Work on {moved_key}; this original stays archived as its history."
        ),
        None => format!(
            "{original} was moved to another project; this original stays archived as its history."
        ),
    }
}

/// Whether a principal may write tasks in `principal.project_id` right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WriteAccess {
    Allowed,
    ReadOnly,
    NotFound,
    ProjectArchived,
    Denied,
}

/// Checks write scope, membership, project state and role
/// (`Action::McpWrite`) for `principal.project_id`, after marking which app
/// a person acts through. It takes the project lock every task write in a
/// project takes first.
pub(super) async fn check_write_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
) -> Result<WriteAccess, AppError> {
    check_access_for(transaction, principal, Action::McpWrite).await
}

/// `check_write_access` with the role checked for `action` instead, for the
/// few writes a role below member may make (watching a task it can see).
/// The credential still needs write scope.
pub(super) async fn check_access_for(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    action: Action,
) -> Result<WriteAccess, AppError> {
    if !principal.can_write() {
        return Ok(WriteAccess::ReadOnly);
    }
    // Rows written below (activity, comments) record the app a person is
    // acting through.
    super::common::mark_via(transaction, principal).await?;
    let membership = lock_membership(transaction, principal.actor_id, principal.project_id).await?;
    let Some(membership) = membership else {
        return Ok(WriteAccess::NotFound);
    };
    if !membership.enabled
        || !matches!(
            principal.actor_kind,
            ActorKind::Human | ActorKind::ExternalAgent
        )
    {
        return Ok(WriteAccess::NotFound);
    }
    if membership.project_archived_at.is_some() {
        return Ok(WriteAccess::ProjectArchived);
    }
    let decision = authorize_mutation(
        &mcp_actor(principal, String::new()),
        &membership,
        action,
        None,
    )?;
    Ok(if matches!(decision, Decision::Allow) {
        WriteAccess::Allowed
    } else {
        WriteAccess::Denied
    })
}

async fn authorize_write(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
) -> Result<Option<WriteResult>, AppError> {
    authorize_write_for(transaction, principal, Action::McpWrite).await
}

/// `authorize_write` with the role checked for `action`.
async fn authorize_write_for(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    action: Action,
) -> Result<Option<WriteResult>, AppError> {
    Ok(match check_access_for(transaction, principal, action).await? {
        WriteAccess::Allowed => None,
        WriteAccess::ReadOnly => Some(WriteResult::PermissionDenied {
            retry_category: "request_write_scope".to_owned(),
            message: Some(READ_ONLY_MESSAGE.to_owned()),
        }),
        WriteAccess::NotFound => Some(WriteResult::not_found(
            "That project wasn't found, or you aren't a member.",
        )),
        WriteAccess::ProjectArchived => Some(WriteResult::Conflict {
            work_item_id: String::new(),
            current_version: 0,
            current: None,
            changed_fields: vec!["projectArchived".to_owned()],
            retry_category: "do_not_retry".to_owned(),
            message: Some(ARCHIVED_PROJECT_MESSAGE.to_owned()),
        }),
        WriteAccess::Denied => Some(WriteResult::PermissionDenied {
            retry_category: "ask_project_owner".to_owned(),
            message: Some(
                "Your role in this project doesn't allow changing its tasks. Ask a project owner to make you a member."
                    .to_owned(),
            ),
        }),
    })
}

/// A clientRequestId that can be stored with the idempotency key and in the
/// audit log.
pub(super) fn parse_request_id(value: &str) -> Option<IdempotencyKey> {
    if value.trim().len() > MAX_REQUEST_ID_BYTES {
        return None;
    }
    IdempotencyKey::parse(value).ok()
}

/// The conflict to return when the caller's version is stale or the task is
/// archived, or None when the change can go ahead.
async fn conflict_for(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    current: &LockedWorkItem,
    expected_version: i64,
) -> Result<Option<WriteResult>, AppError> {
    let Some(kind) = conflict_kind(current, expected_version) else {
        return Ok(None);
    };
    let task = task_key(transaction, project_id, current.number).await?;
    Ok(Some(WriteResult::Conflict {
        work_item_id: current.id.to_string(),
        current_version: current.version,
        current: Some(current.into()),
        changed_fields: vec![match kind {
            ConflictKind::Version => "version".to_owned(),
            ConflictKind::Archived => "archived".to_owned(),
        }],
        retry_category: match kind {
            ConflictKind::Version => "refetch_then_decide".to_owned(),
            ConflictKind::Archived => "do_not_retry".to_owned(),
        },
        message: Some(match kind {
            ConflictKind::Version => version_conflict_message(&task, current.version),
            ConflictKind::Archived => archived_task_message(&task),
        }),
    }))
}

/// Why a status change isn't allowed, with where the task can go instead
/// and the shortest way to the status asked for. Nothing is written, and the
/// clientRequestId reservation rolls back with the transaction, so a
/// corrected retry can reuse it.
pub(super) fn invalid_transition(workflow: &Workflow, from: Uuid, to: Uuid) -> WriteResult {
    WriteResult::InvalidTransition {
        current_status_id: from.to_string(),
        requested_status_id: to.to_string(),
        retry_category: "ask_user_or_choose_allowed_transition".to_owned(),
        current_status: Some(workflow.status_name(from).to_owned()),
        requested_status: Some(workflow.status_name(to).to_owned()),
        allowed_statuses: workflow
            .allowed_from(from)
            .map(|status| StatusOption {
                id: status.id.to_string(),
                name: status.name.clone(),
                category: status.category.clone(),
            })
            .collect(),
        path: workflow
            .shortest_path(from, to)
            .unwrap_or_default()
            .into_iter()
            .map(|id| workflow.status_name(id).to_owned())
            .collect(),
        message: workflow.transition_problem(from, to),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_update_outcome(
    mut transaction: Transaction<'_, Postgres>,
    state: &AppState,
    principal: &McpPrincipal,
    reservation: &IdempotencyReservation,
    expected_version: i64,
    outcome: UpdateWorkItemOutcome,
    noop_action: &str,
    request_id: &str,
    report_labels: bool,
) -> Result<WriteResult, AppError> {
    match outcome {
        UpdateWorkItemOutcome::Applied {
            work_item_id,
            version,
            changed_fields,
            finalized,
        } => {
            let location =
                locate(&mut transaction, state, principal.project_id, work_item_id).await?;
            let mut result = applied(
                work_item_id,
                version,
                changed_fields,
                Some(finalized.activity_id),
                finalized.audit_id,
                location,
            );
            if report_labels {
                result = with_labels(&mut transaction, result, principal.project_id, work_item_id)
                    .await?;
            }
            complete(&mut transaction, reservation, StatusCode::OK, &result).await?;
            transaction.commit().await?;
            Ok(result)
        }
        UpdateWorkItemOutcome::Noop(current) => {
            finalize_noop(
                transaction,
                state,
                principal,
                reservation,
                current.id,
                current.version,
                noop_action,
                request_id,
                report_labels,
            )
            .await
        }
        UpdateWorkItemOutcome::Conflict(current) => Ok(conflict_for(
            &mut transaction,
            principal.project_id,
            &current,
            expected_version,
        )
        .await?
        .ok_or(AppError::Internal("command returned an empty conflict"))?),
        UpdateWorkItemOutcome::NotFound => Ok(WriteResult::not_found(TASK_NOT_FOUND_MESSAGE)),
        UpdateWorkItemOutcome::InvalidAssignee => {
            Ok(WriteResult::invalid("assigneeActorId", ASSIGNEE_MESSAGE))
        }
        UpdateWorkItemOutcome::InvalidMilestone => {
            Ok(WriteResult::invalid("milestone", MILESTONE_MESSAGE))
        }
        UpdateWorkItemOutcome::InvalidDates => Ok(WriteResult::invalid("dueDate", DATES_MESSAGE)),
        UpdateWorkItemOutcome::InvalidTransition {
            current_status_id,
            requested_status_id,
        } => {
            let workflow = Workflow::load(&mut transaction, principal.project_id).await?;
            Ok(invalid_transition(
                &workflow,
                current_status_id,
                requested_status_id,
            ))
        }
        UpdateWorkItemOutcome::TooManyLabels { limit, would_have } => Ok(WriteResult::invalid(
            "addLabels",
            format!(
                "A task can have at most {limit} labels; this change would give it {would_have}. Remove some with removeLabels first."
            ),
        )),
        UpdateWorkItemOutcome::InvalidParent(problem) => {
            let ids = match &problem {
                ParentProblem::Archived(id) => vec![*id],
                ParentProblem::Cycle { path } => path.clone(),
                _ => Vec::new(),
            };
            let keys = keys_in_project(&mut transaction, principal.project_id, &ids).await?;
            Ok(WriteResult::invalid(
                "parentId",
                parent_problem_text(&problem, &keys),
            ))
        }
        UpdateWorkItemOutcome::InvalidPlacement(problem) => {
            let ids = match &problem {
                PlacementProblem::AnchorNotInColumn { anchor } => vec![*anchor],
                _ => Vec::new(),
            };
            let keys = keys_in_project(&mut transaction, principal.project_id, &ids).await?;
            Ok(WriteResult::invalid(
                "position",
                placement_problem_text(&problem, &keys),
            ))
        }
    }
}

/// Plain words for a parent that won't do. `keys` names tasks in the
/// caller's project only, so nothing from another project is revealed.
pub(super) fn parent_problem_text(problem: &ParentProblem, keys: &HashMap<Uuid, String>) -> String {
    let key = |id: &Uuid| {
        keys.get(id)
            .cloned()
            .unwrap_or_else(|| "That task".to_owned())
    };
    match problem {
        ParentProblem::NotFound => {
            "No such task in this project. A subtask must be in its parent's project.".to_owned()
        }
        ParentProblem::IsSelf => "A task can't be its own parent.".to_owned(),
        ParentProblem::Archived(id) => format!(
            "{} is archived. Restore it first, or pick another parent.",
            key(id)
        ),
        ParentProblem::Cycle { path } => {
            let (Some(task), Some(parent)) = (path.first(), path.last()) else {
                return "A task can't go under its own subtask.".to_owned();
            };
            format!(
                "{} is already under {} ({}), so {} can't go under it. Move {} out first, or pick another parent.",
                key(parent),
                key(task),
                path.iter().map(key).collect::<Vec<_>>().join(" › "),
                key(task),
                key(parent),
            )
        }
        ParentProblem::TooDeep => format!(
            "That would nest subtasks more than {MAX_SUBTASK_LEVELS} levels deep. Pick a parent higher up."
        ),
    }
}

/// Plain words for a board position that won't do.
pub(super) fn placement_problem_text(
    problem: &PlacementProblem,
    keys: &HashMap<Uuid, String>,
) -> String {
    match problem {
        PlacementProblem::AnchorIsSelf => "A task can't go before or after itself.".to_owned(),
        PlacementProblem::AnchorNotInColumn { anchor } => format!(
            "{} isn't an open task in that column. Pass a task from the same column, or use top or bottom.",
            keys.get(anchor).map_or("That task", String::as_str)
        ),
        PlacementProblem::OutOfRange { open_tasks } => format!(
            "That column has {open_tasks} other open tasks, so use a position from 1 to {} (or top or bottom).",
            open_tasks + 1
        ),
    }
}

/// A board position as agents write it: top, bottom, or a number where 1
/// is the top of the column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BoardPosition {
    Top,
    Bottom,
    /// 1-based.
    Number(usize),
}

/// The highest position number accepted; a column never holds more.
const MAX_POSITION: usize = 10_000;

pub(super) fn parse_position(value: &str) -> Option<BoardPosition> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("top") {
        return Some(BoardPosition::Top);
    }
    if value.eq_ignore_ascii_case("bottom") {
        return Some(BoardPosition::Bottom);
    }
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<usize>()
        .ok()
        .filter(|number| (1..=MAX_POSITION).contains(number))
        .map(BoardPosition::Number)
}

/// An argument that won't do: the field, and what would work instead.
type Refusal = (&'static str, String);

/// Where a call asked to put a task in its status column: `position`, or
/// next to an anchor task (`beforeTaskId`, `afterTaskId`). At most one.
fn requested_placement(
    position: Option<&str>,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<Option<BoardPlacement>, Refusal> {
    let anchor = |value: &str, field: &'static str| {
        parse_id(value).ok_or_else(|| {
            (
                field,
                format!(
                    "No task {} that you can reach. Pass an open task from the same column by its ID, link, or key such as AWD-12.",
                    echo(value)
                ),
            )
        })
    };
    match (position, before, after) {
        (None, None, None) => Ok(None),
        (Some(position), None, None) => match parse_position(position) {
            Some(BoardPosition::Top) => Ok(Some(BoardPlacement::Top)),
            Some(BoardPosition::Bottom) => Ok(Some(BoardPlacement::Bottom)),
            Some(BoardPosition::Number(number)) => Ok(Some(BoardPlacement::Index(number - 1))),
            None => Err(("position", POSITION_MESSAGE.to_owned())),
        },
        (None, Some(before), None) => {
            anchor(before, "beforeTaskId").map(|id| Some(BoardPlacement::Before(id)))
        }
        (None, None, Some(after)) => {
            anchor(after, "afterTaskId").map(|id| Some(BoardPlacement::After(id)))
        }
        _ => Err((
            "position",
            "Pass one of position, beforeTaskId, or afterTaskId, not several.".to_owned(),
        )),
    }
}

/// The argument a placement came from, for the error that refuses it.
fn placement_field(placement: Option<BoardPlacement>) -> &'static str {
    match placement {
        Some(BoardPlacement::Before(_)) => "beforeTaskId",
        Some(BoardPlacement::After(_)) => "afterTaskId",
        _ => "position",
    }
}

/// Plain words for a place on the board that won't do, naming the column
/// the task goes in. Only tasks in this project are ever named.
async fn placement_problem(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    column: Uuid,
    problem: &PlacementProblem,
    field: &str,
) -> Result<WriteResult, AppError> {
    let workflow = Workflow::load(transaction, project_id).await?;
    let column = workflow.status_name(column).to_owned();
    let instead = format!(
        "Pass a task from the {column} column, or use position (top, bottom, or a number)."
    );
    let message = match problem {
        PlacementProblem::AnchorIsSelf => "A task can't go before or after itself.".to_owned(),
        PlacementProblem::OutOfRange { open_tasks } => {
            return Ok(WriteResult::invalid(
                "position",
                format!(
                    "{column} has {open_tasks} other open tasks, so use a position from 1 to {} (or top or bottom).",
                    open_tasks + 1
                ),
            ));
        }
        PlacementProblem::AnchorNotInColumn { anchor } => {
            let found = sqlx::query_as::<_, (String, Uuid, bool)>(
                r#"
                SELECT project.project_key || '-' || item.number::text, item.status_id,
                       item.archived_at IS NOT NULL
                FROM work_items item JOIN projects project ON project.id = item.project_id
                WHERE item.project_id = $1 AND item.id = $2
                "#,
            )
            .bind(project_id)
            .bind(anchor)
            .fetch_optional(&mut **transaction)
            .await?;
            match found {
                Some((key, _, true)) => format!("{key} is archived. {instead}"),
                Some((key, status, false)) => format!(
                    "{key} is in {}, but this task goes in {column}. {instead}",
                    workflow.status_name(status)
                ),
                None => format!("That task isn't in this project. {instead}"),
            }
        }
    };
    Ok(WriteResult::invalid(field, message))
}

/// A parent in another project the caller can reach, named so the agent
/// can fix the call. A task in a project the caller can't reach is never
/// named: the command then answers that no such task is in this project.
struct ParentElsewhere {
    key: String,
    project: String,
    here: String,
}

async fn parent_in_other_project(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    parent_id: Uuid,
) -> Result<Option<ParentElsewhere>, AppError> {
    let found = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT item.project_id, project.project_key || '-' || item.number::text, project.name
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.id = $1
        "#,
    )
    .bind(parent_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((project_id, key, project)) = found else {
        return Ok(None);
    };
    if project_id == principal.project_id
        || !accessible_project_ids_in(&mut **transaction, principal)
            .await?
            .contains(&project_id)
    {
        return Ok(None);
    }
    let here = sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1")
        .bind(principal.project_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(Some(ParentElsewhere { key, project, here }))
}

/// Label IDs for names or IDs among the project's active labels, or the
/// error naming what's unknown or archived and the labels it has.
async fn resolve_labels(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    references: &[String],
    field: &str,
) -> Result<Result<Vec<Uuid>, WriteResult>, AppError> {
    if references.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let resolution = work_item_labels::resolve(transaction, project_id, references).await?;
    if !resolution.is_complete() {
        let message =
            work_item_labels::unknown_message(transaction, project_id, &resolution).await?;
        return Ok(Err(WriteResult::invalid(field, message)));
    }
    Ok(Ok(resolution.ids()))
}

/// Adds the task's labels after the change to an applied result, before
/// it's stored for replays.
async fn with_labels(
    transaction: &mut Transaction<'_, Postgres>,
    mut result: WriteResult,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<WriteResult, AppError> {
    if let WriteResult::Applied { labels, .. } = &mut result {
        *labels = Some(
            work_item_labels::active_names(&mut **transaction, project_id, work_item_id).await?,
        );
    }
    Ok(result)
}

/// Acceptance criteria for a new task, normalized, or the error naming the
/// one that won't do.
pub(super) fn normalize_criteria(texts: &[String]) -> Result<Vec<String>, String> {
    if texts.len() > MAX_CRITERIA_ON_CREATE {
        return Err(format!(
            "Pass at most {MAX_CRITERIA_ON_CREATE} acceptance criteria when creating a task."
        ));
    }
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            normalize_criterion_text(text).map_err(|_| {
                format!(
                    "Acceptance criterion {} is empty, longer than 1,000 characters, or has control characters other than line breaks.",
                    index + 1
                )
            })
        })
        .collect()
}

/// A status or type name of a sensible length, or the error for it.
fn check_name(value: Option<&str>, field: &str, message: &str) -> Option<WriteResult> {
    value
        .filter(|value| value.trim().is_empty() || value.len() > 80)
        .map(|_| WriteResult::invalid(field, message))
}

/// "A", "A and B", "A, B and C".
pub(super) fn join_and(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Keys such as AWD-12 for tasks in one project; others are left out.
async fn keys_in_project(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        "#,
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect())
}

async fn task_key(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    number: i64,
) -> Result<String, AppError> {
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    Ok(format!("{project_key}-{number}"))
}

/// A task's key (such as AWD-12) and the link people open it at.
pub(super) async fn locate(
    transaction: &mut Transaction<'_, Postgres>,
    state: &AppState,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<(String, String), AppError> {
    let key = sqlx::query_scalar::<_, String>(
        r#"
        SELECT project.project_key || '-' || item.number::text
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok((
        key,
        public_url(
            state,
            &format!("/projects/{project_id}/work-items/{work_item_id}"),
        ),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn finalize_work_item_change(
    mut transaction: Transaction<'_, Postgres>,
    state: &AppState,
    principal: &McpPrincipal,
    reservation: &IdempotencyReservation,
    work_item_id: Uuid,
    version: i64,
    payload: ChangePayload,
    summary: &str,
    action: &str,
    request_id: &str,
) -> Result<WriteResult, AppError> {
    let finalized = finalize_change(
        &mut transaction,
        &mcp_actor(principal, request_id.to_owned()),
        work_item_id,
        ChangeFinalization {
            activity_kind: "work_item_updated",
            summary,
            audit_action: action,
            activity_payload: payload.activity(),
            audit_metadata: payload.audit(version),
            version,
        },
    )
    .await?;
    let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
    let result = applied(
        work_item_id,
        version,
        payload.changed_fields,
        Some(finalized.activity_id),
        finalized.audit_id,
        location,
    );
    complete(&mut transaction, reservation, StatusCode::OK, &result).await?;
    transaction.commit().await?;
    Ok(result)
}

async fn insert_mcp_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    action: &str,
    target_id: Uuid,
    request_id: &str,
    metadata: serde_json::Value,
) -> Result<Uuid, AppError> {
    record_audit(
        transaction,
        &mcp_actor(principal, request_id.to_owned()),
        action,
        target_id,
        metadata,
    )
    .await
}

/// The command actor for an MCP call in `principal.project_id`. `request_id`
/// goes in the audit log, so it must already fit there (parse_request_id).
pub(super) fn mcp_actor(principal: &McpPrincipal, request_id: String) -> CommandActor {
    CommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind,
        project_id: principal.project_id,
        audit: AuditAttribution::Mcp {
            authentication_class: principal.authentication_class,
            request_id,
            client_name: principal.client_name.clone(),
            mcp_credential_id: principal.mcp_credential_id,
            oauth_grant_id: principal.oauth_grant_id,
            external_agent_connection_id: principal.external_agent_connection_id,
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_noop(
    mut transaction: Transaction<'_, Postgres>,
    state: &AppState,
    principal: &McpPrincipal,
    reservation: &IdempotencyReservation,
    work_item_id: Uuid,
    version: i64,
    action: &str,
    request_id: &str,
    report_labels: bool,
) -> Result<WriteResult, AppError> {
    let result_id = insert_mcp_audit(
        &mut transaction,
        principal,
        action,
        work_item_id,
        request_id,
        serde_json::json!({"changed": false}),
    )
    .await?;
    let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
    let mut result = applied(work_item_id, version, Vec::new(), None, result_id, location);
    if report_labels {
        result = with_labels(&mut transaction, result, principal.project_id, work_item_id).await?;
    }
    complete(&mut transaction, reservation, StatusCode::OK, &result).await?;
    transaction.commit().await?;
    Ok(result)
}

fn applied(
    work_item_id: Uuid,
    version: i64,
    changed_fields: Vec<String>,
    activity_id: Option<Uuid>,
    result_id: Uuid,
    (key, url): (String, String),
) -> WriteResult {
    WriteResult::Applied {
        work_item_id: work_item_id.to_string(),
        version,
        changed_fields,
        activity_id: activity_id.map(|id| id.to_string()),
        result_id: result_id.to_string(),
        idempotent_replay: false,
        key: Some(key),
        url: Some(url),
        message: None,
        labels: None,
    }
}

pub(super) fn valid_priority(value: &str) -> bool {
    matches!(value, "no_priority" | "low" | "medium" | "high" | "urgent")
}

fn normalize_text(value: String, max_characters: usize, allow_empty: bool) -> Option<String> {
    crate::work_item_commands::normalize_single_line(&value, max_characters, allow_empty).ok()
}

/// Comments may span lines and carry Markdown, as they can in the web app.
pub(super) fn normalize_comment(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_UPDATE_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return None;
    }
    Some(value.replace("\r\n", "\n"))
}

/// Descriptions are Markdown and may span lines, as they can in the web app.
fn normalize_description(value: String) -> Option<String> {
    crate::work_item_commands::normalize_description(&value).ok()
}

fn command_error(command: &str, error: AppError) -> WriteResult {
    match error {
        AppError::Conflict("idempotency_key", "payload mismatch") => WriteResult::Conflict {
            work_item_id: String::new(),
            current_version: 0,
            current: None,
            changed_fields: vec!["clientRequestId".to_owned()],
            retry_category: "use_new_request_id".to_owned(),
            message: Some(
                "That clientRequestId was already used for a different request. Use a new one for this change."
                    .to_owned(),
            ),
        },
        error => {
            let (database_code, database_constraint) = error.database_log_fields();
            tracing::error!(
                command,
                category = error.log_category(),
                database_code,
                database_constraint,
                "MCP write failed"
            );
            WriteResult::unavailable()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        BoardPlacement, BoardPosition, CreateSubtaskArguments, CreateWorkItemArguments,
        NewTaskDetails, UpdateWorkItemArguments, WriteResult, normalize_comment, normalize_text,
        parse_id, parse_position, parse_request_id, requested_placement,
    };

    #[test]
    fn comments_keep_line_breaks_and_markdown() {
        assert_eq!(
            normalize_comment("  Done:\r\n- one\n- **two**\n\n\tnext  ".to_owned()),
            Some("Done:\n- one\n- **two**\n\n\tnext".to_owned())
        );
        assert_eq!(normalize_comment("a\u{0000}b".to_owned()), None);
        assert_eq!(normalize_comment(" \n ".to_owned()), None);
        assert_eq!(normalize_comment("x".repeat(20_001)), None);
    }

    #[test]
    fn immutable_ids_and_text_bounds_are_fail_closed() {
        assert!(parse_id("not-an-id").is_none());
        assert_eq!(
            normalize_text(" hello ".to_owned(), 10, false),
            Some("hello".to_owned())
        );
        assert_eq!(normalize_text("\u{0000}".to_owned(), 10, false), None);
        assert_eq!(normalize_text(String::new(), 10, false), None);
    }

    #[test]
    fn write_results_use_the_camel_case_mcp_contract() {
        let result = serde_json::to_value(WriteResult::Applied {
            work_item_id: Uuid::nil().to_string(),
            version: 2,
            changed_fields: vec!["title".to_owned()],
            activity_id: None,
            result_id: Uuid::now_v7().to_string(),
            idempotent_replay: false,
            key: Some("AWD-12".to_owned()),
            url: None,
            message: None,
            labels: None,
        })
        .expect("serialize write result");
        assert_eq!(result["idempotentReplay"], json!(false));
        assert_eq!(result["changedFields"], json!(["title"]));
        assert_eq!(result["key"], json!("AWD-12"));
        assert!(result.get("idempotent_replay").is_none());
        assert!(result.get("url").is_none(), "{result}");
        assert!(result.get("message").is_none(), "{result}");
        assert!(result.get("labels").is_none(), "{result}");
    }

    /// Replays stored before the optional fields existed must still decode,
    /// and a result without them must look exactly as it did.
    #[test]
    fn results_stored_before_the_new_fields_still_replay() {
        let applied = json!({
            "outcome": "applied",
            "workItemId": Uuid::nil().to_string(),
            "version": 3,
            "changedFields": ["status"],
            "activityId": null,
            "resultId": Uuid::nil().to_string(),
            "idempotentReplay": false
        });
        let decoded = serde_json::from_value::<WriteResult>(applied.clone()).expect("old applied");
        let WriteResult::Applied {
            key,
            url,
            message,
            labels,
            ..
        } = &decoded
        else {
            panic!("not applied: {decoded:?}");
        };
        assert!(key.is_none() && url.is_none() && message.is_none() && labels.is_none());
        assert_eq!(serde_json::to_value(&decoded).expect("re-encode"), applied);

        for old in [
            json!({
                "outcome": "invalid_transition",
                "currentStatusId": Uuid::nil().to_string(),
                "requestedStatusId": Uuid::nil().to_string(),
                "retryCategory": "ask_user_or_choose_allowed_transition"
            }),
            json!({
                "outcome": "conflict",
                "workItemId": "",
                "currentVersion": 0,
                "current": null,
                "changedFields": ["clientRequestId"],
                "retryCategory": "use_new_request_id"
            }),
            json!({"outcome": "not_found", "retryCategory": "do_not_retry"}),
            json!({"outcome": "permission_denied", "retryCategory": "ask_project_owner"}),
            json!({"outcome": "invalid_request", "field": "title", "retryCategory": "correct_input"}),
        ] {
            let decoded = serde_json::from_value::<WriteResult>(old.clone())
                .unwrap_or_else(|error| panic!("{old}: {error}"));
            assert_eq!(serde_json::to_value(&decoded).expect("re-encode"), old);
        }
    }

    /// The argument structs are the idempotency request hash. A call shaped
    /// as before Phase 2 must encode byte for byte as it did, or a retry
    /// across the deploy would be refused (or, for a create, made twice).
    #[test]
    fn older_calls_hash_exactly_as_before() {
        let create = CreateWorkItemArguments {
            title: "Offline mode".to_owned(),
            description: None,
            priority: Some("high".to_owned()),
            assignee_actor_id: None,
            client_request_id: "plan-1".to_owned(),
            details: NewTaskDetails::default(),
        };
        assert_eq!(
            serde_json::to_string(&create).expect("create"),
            r#"{"title":"Offline mode","description":null,"priority":"high","assigneeActorId":null,"clientRequestId":"plan-1"}"#
        );
        let subtask = CreateSubtaskArguments {
            parent_work_item_id: Uuid::nil().to_string(),
            title: "Sync".to_owned(),
            description: Some("Both ways".to_owned()),
            client_request_id: "plan-2".to_owned(),
            priority: None,
            assignee_actor_id: None,
            details: NewTaskDetails::default(),
        };
        assert_eq!(
            serde_json::to_string(&subtask).expect("subtask"),
            format!(
                r#"{{"parentId":"{}","title":"Sync","description":"Both ways","clientRequestId":"plan-2"}}"#,
                Uuid::nil()
            )
        );
        let update = UpdateWorkItemArguments {
            work_item_id: Uuid::nil().to_string(),
            expected_version: 3,
            title: Some("Renamed".to_owned()),
            description: None,
            priority: None,
            assignee_actor_id: None,
            clear_assignee: None,
            client_request_id: "plan-3".to_owned(),
            item_type: None,
            parent_id: None,
            clear_parent: None,
            add_labels: None,
            remove_labels: None,
            position: None,
            before_task_id: None,
            after_task_id: None,
            milestone: None,
            start_date: None,
            due_date: None,
            estimate: None,
            clear_estimate: None,
        };
        assert_eq!(
            serde_json::to_string(&update).expect("update"),
            format!(
                r#"{{"taskId":"{}","expectedVersion":3,"title":"Renamed","description":null,"priority":null,"assigneeActorId":null,"clearAssignee":null,"clientRequestId":"plan-3"}}"#,
                Uuid::nil()
            )
        );
        // New fields are hashed when they're given.
        let typed = CreateWorkItemArguments {
            details: NewTaskDetails {
                item_type: Some("Bug".to_owned()),
                ..NewTaskDetails::default()
            },
            ..create
        };
        assert!(
            serde_json::to_string(&typed)
                .expect("typed")
                .ends_with(r#""clientRequestId":"plan-1","type":"Bug"}"#)
        );
    }

    #[test]
    fn positions_read_as_agents_write_them() {
        assert_eq!(parse_position("3"), Some(BoardPosition::Number(3)));
        assert_eq!(parse_position(" 1 "), Some(BoardPosition::Number(1)));
        assert_eq!(parse_position("Top"), Some(BoardPosition::Top));
        assert_eq!(parse_position("BOTTOM"), Some(BoardPosition::Bottom));
        for refused in [
            "0",
            "-1",
            "+2",
            "1.5",
            "middle",
            "",
            "10001",
            "99999999999999999999",
        ] {
            assert_eq!(parse_position(refused), None, "{refused}");
        }
        assert_eq!(
            requested_placement(Some("2"), None, None).expect("index"),
            Some(BoardPlacement::Index(1))
        );
        let anchor = Uuid::now_v7();
        assert_eq!(
            requested_placement(
                None,
                None,
                Some(&format!(
                    "https://example.test/projects/x/work-items/{anchor}"
                ))
            )
            .expect("link"),
            Some(BoardPlacement::After(anchor))
        );
        assert_eq!(requested_placement(None, None, None).expect("none"), None);
        for (position, before, field) in [
            (Some("middle"), None, "position"),
            (Some("top"), Some(anchor.to_string()), "position"),
            (None, Some("AWD-12".to_owned()), "beforeTaskId"),
        ] {
            let Err((refused, message)) = requested_placement(position, before.as_deref(), None)
            else {
                panic!("{position:?} {before:?} was accepted");
            };
            assert_eq!(refused, field);
            assert!(!message.is_empty());
        }
    }

    #[test]
    fn client_request_ids_fit_the_audit_log() {
        assert!(parse_request_id(&"a".repeat(128)).is_some());
        assert!(parse_request_id(&"a".repeat(129)).is_none());
        assert!(parse_request_id(&"a".repeat(200)).is_none());
        assert!(parse_request_id(" plan-import-17 ").is_some());
        assert!(parse_request_id("has space").is_none());
        assert!(parse_request_id("").is_none());
    }
}
