use std::collections::HashMap;

use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, READ_ONLY_MESSAGE, archived_task_message, public_url,
        version_conflict_message,
    },
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
        AllocatedNumberMetadata, AuditAttribution, AuditVersionMetadata, BoardPlacement,
        ChangeFinalization, ChangePayload, CommandActor, ConflictKind, CreateWorkItemCommand,
        CreateWorkItemOutcome, LockedWorkItem, MAX_SUBTASK_LEVELS, ParentProblem, PlacementProblem,
        UpdateWorkItemCommand, UpdateWorkItemOutcome, advance_version, authorize_mutation,
        conflict_kind, create_work_item as execute_create_work_item, enqueue_update,
        finalize_change, finalize_comment, insert_activity as insert_work_item_activity,
        lock_membership, lock_work_item, place_in_column, record_audit, update_work_item_by_id,
    },
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
const REQUEST_ID_MESSAGE: &str = "Use a clientRequestId of 1 to 128 letters, digits, dashes, dots or underscores, and send the same one when you retry.";
const TITLE_MESSAGE: &str = "Give it a title of 1 to 240 characters, on one line.";
const DESCRIPTION_MESSAGE: &str = "The description can be at most 100,000 characters, with no control characters except line breaks and tabs.";
const PRIORITY_MESSAGE: &str = "Use no_priority, low, medium, high, or urgent.";
const ASSIGNEE_ID_MESSAGE: &str = "Pass the assignee as \"me\", an @handle, or an actor ID.";
const ASSIGNEE_MESSAGE: &str =
    "That person can't be assigned tasks here: only active people and agents in the project can.";
const STATUS_MESSAGE: &str = "Pass a status name, such as \"In Progress\" (see workflow_get).";
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
    /// The comment or update, in plain text.
    #[serde(rename = "text", alias = "update")]
    pub update: String,
    pub client_request_id: String,
}

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
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSubtaskArguments {
    #[serde(rename = "parentId", alias = "parentWorkItemId")]
    pub parent_work_item_id: String,
    pub title: String,
    pub description: Option<String>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWorkItemArguments {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub assignee_actor_id: Option<String>,
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
        let Some(title) = normalize_text(arguments.title.clone(), MAX_TITLE_CHARACTERS, false)
        else {
            return Ok(WriteResult::invalid("title", TITLE_MESSAGE));
        };
        let description = match arguments.description.clone() {
            Some(value) => match normalize_description(value) {
                Some(value) => value,
                None => return Ok(WriteResult::invalid("description", DESCRIPTION_MESSAGE)),
            },
            None => String::new(),
        };
        let priority = arguments
            .priority
            .clone()
            .unwrap_or_else(|| "no_priority".to_owned());
        if !valid_priority(&priority) {
            return Ok(WriteResult::invalid("priority", PRIORITY_MESSAGE));
        }
        let assignee_actor_id = match arguments.assignee_actor_id.as_deref() {
            Some(value) => match parse_id(value) {
                Some(actor_id) => Some(actor_id),
                None => return Ok(WriteResult::invalid("assigneeActorId", ASSIGNEE_ID_MESSAGE)),
            },
            None => None,
        };
        let Some(key) = parse_request_id(&arguments.client_request_id) else {
            return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? {
            return Ok(result);
        }
        let scope = resource_bound_scope("mcp_work_items", principal.project_id, "create")?;
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
        let outcome = execute_create_work_item(
            &mut transaction,
            &mcp_actor(principal, key.as_str().to_owned()),
            CreateWorkItemCommand {
                title: &title,
                description: &description,
                type_id: None,
                status_id: None,
                priority: &priority,
                assignee_actor_id,
                parent_work_item_id: None,
                label_ids: &[],
                acceptance_criteria: &[],
                board_placement: None,
                activity_summary: "Work Item created through MCP",
                activity_metadata: serde_json::json!({}),
                activity_number_metadata: AllocatedNumberMetadata::Include,
                audit_action: "work_item.mcp_created",
                audit_metadata: serde_json::json!({}),
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
            CreateWorkItemOutcome::ParentNotFound
            | CreateWorkItemOutcome::ParentArchived(_)
            | CreateWorkItemOutcome::ParentTooDeep
            | CreateWorkItemOutcome::InvalidPlacement(_) => {
                return Err(AppError::Internal("top-level MCP Work Item had a parent"));
            }
        };
        let location = locate(&mut transaction, state, principal.project_id, work_item_id).await?;
        let result = applied(
            work_item_id,
            1,
            vec!["work_item".to_owned()],
            Some(finalized.activity_id),
            finalized.audit_id,
            location,
        );
        complete(&mut transaction, &reservation, StatusCode::CREATED, &result).await?;
        transaction.commit().await?;
        Ok(result)
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_create", error),
    }
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
    if arguments.title.is_none()
        && arguments.description.is_none()
        && arguments.priority.is_none()
        && arguments.assignee_actor_id.is_none()
        && arguments.clear_assignee != Some(true)
    {
        return Ok(WriteResult::invalid(
            "update",
            "Pass at least one change: title, description, priority, assignee, clearAssignee, or status.",
        ));
    }
    if arguments.clear_assignee == Some(true) && arguments.assignee_actor_id.is_some() {
        return Ok(WriteResult::invalid(
            "update",
            "Pass assignee or clearAssignee, not both.",
        ));
    }
    if status
        .as_deref()
        .is_some_and(|status| status.is_empty() || status.len() > 80)
    {
        return Ok(WriteResult::invalid("status", STATUS_MESSAGE));
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
    let assignee_update = if arguments.clear_assignee == Some(true) {
        Some(None)
    } else {
        assignee_actor_id.map(Some)
    };
    let status_id = match &status {
        Some(status) => {
            let workflow = Workflow::load(&mut transaction, principal.project_id).await?;
            match workflow.status(status) {
                Ok(status) => Some(status.id),
                Err(message) => return Ok(WriteResult::invalid("status", message)),
            }
        }
        None => None,
    };
    let outcome = update_work_item_by_id(
        &mut transaction,
        &mcp_actor(principal, key.as_str().to_owned()),
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: arguments.expected_version,
            title: arguments.title.as_deref(),
            description: arguments.description.as_deref(),
            type_id: None,
            status_id,
            priority: arguments.priority.as_deref(),
            assignee_actor_id: assignee_update,
            parent_work_item_id: None,
            board_placement: None,
            labels: None,
            summary: "Work Item updated through MCP",
            audit_action: "work_item.mcp_updated",
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
        "work_item.mcp_update_unchanged",
        key.as_str(),
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
    const UPDATE_MESSAGE: &str = "Write the comment as plain text, 1 to 20,000 characters.";
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId", TASK_ID_MESSAGE));
    };
    let Some(update) = normalize_text(arguments.update.clone(), MAX_UPDATE_CHARACTERS, false)
    else {
        return Ok(WriteResult::invalid("update", UPDATE_MESSAGE));
    };
    let validated = match MessageContent::from_plain_text(update.clone()) {
        Ok(content) => content,
        Err(_) => return Ok(WriteResult::invalid("update", UPDATE_MESSAGE)),
    };
    let Some(key) = parse_request_id(&arguments.client_request_id) else {
        return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
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

pub async fn create_subtask(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CreateSubtaskArguments,
) -> WriteResult {
    let result = async {
        let Some(parent_id) = parse_id(&arguments.parent_work_item_id) else {
            return Ok(WriteResult::invalid(
                "parentWorkItemId",
                "Pass the parent task's ID, link, or key such as AWD-12.",
            ));
        };
        let Some(title) = normalize_text(arguments.title.clone(), MAX_TITLE_CHARACTERS, false)
        else {
            return Ok(WriteResult::invalid("title", TITLE_MESSAGE));
        };
        let description = match arguments.description.clone() {
            Some(value) => match normalize_description(value) {
                Some(value) => value,
                None => return Ok(WriteResult::invalid("description", DESCRIPTION_MESSAGE)),
            },
            None => String::new(),
        };
        let Some(key) = parse_request_id(&arguments.client_request_id) else {
            return Ok(WriteResult::invalid("clientRequestId", REQUEST_ID_MESSAGE));
        };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? {
            return Ok(result);
        }
        let scope = resource_bound_scope("mcp_work_items", parent_id, "create_subtask")?;
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
        let outcome = execute_create_work_item(
            &mut transaction,
            &mcp_actor(principal, key.as_str().to_owned()),
            CreateWorkItemCommand {
                title: &title,
                description: &description,
                type_id: None,
                status_id: None,
                priority: "no_priority",
                assignee_actor_id: None,
                parent_work_item_id: Some(parent_id),
                label_ids: &[],
                acceptance_criteria: &[],
                board_placement: None,
                activity_summary: "Subtask created through MCP",
                activity_metadata: serde_json::json!({"parentWorkItemId": parent_id}),
                activity_number_metadata: AllocatedNumberMetadata::Omit,
                audit_action: "work_item.mcp_subtask_created",
                audit_metadata: serde_json::json!({"parentWorkItemId": parent_id}),
            },
        )
        .await?;
        let (subtask_id, finalized) = match outcome {
            CreateWorkItemOutcome::Created {
                work_item_id,
                finalized,
                ..
            } => (work_item_id, finalized),
            CreateWorkItemOutcome::ParentNotFound => {
                return Ok(WriteResult::not_found(
                    "That parent task wasn't found, or you can't see it.",
                ));
            }
            CreateWorkItemOutcome::ParentArchived(parent) => {
                let task = task_key(&mut transaction, principal.project_id, parent.number).await?;
                return Ok(WriteResult::Conflict {
                    work_item_id: parent_id.to_string(),
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
            CreateWorkItemOutcome::InvalidAssignee | CreateWorkItemOutcome::InvalidPlacement(_) => {
                return Err(AppError::Internal("unassigned subtask was refused"));
            }
        };
        let location = locate(&mut transaction, state, principal.project_id, subtask_id).await?;
        let result = applied(
            subtask_id,
            1,
            vec!["subtask".to_owned()],
            Some(finalized.activity_id),
            finalized.audit_id,
            location,
        );
        complete(&mut transaction, &reservation, StatusCode::CREATED, &result).await?;
        transaction.commit().await?;
        Ok(result)
    }
    .await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_create_subtask", error),
    }
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
        let pr_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM github_pull_requests WHERE project_id = $1 AND id = $2)",
        )
        .bind(principal.project_id)
        .bind(pull_request_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !pr_exists {
            return Ok(WriteResult::not_found(
                "That pull request wasn't found in this task's project.",
            ));
        }
        let inserted = sqlx::query(
            r#"
            INSERT INTO github_pull_request_work_item_links (
                project_id, github_pull_request_id, work_item_id, source,
                confirmed, created_by_actor_id
            ) VALUES ($1, $2, $3, 'explicit', true, $4)
            ON CONFLICT (github_pull_request_id, work_item_id) DO NOTHING
            "#,
        )
        .bind(principal.project_id)
        .bind(pull_request_id)
        .bind(work_item_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;
        let changed = inserted.rows_affected() == 1;
        let version = if changed {
            advance_version(
                &mut transaction,
                principal.project_id,
                work_item_id,
                arguments.expected_version,
            )
            .await?
        } else {
            current.version
        };
        let activity_id = if changed {
            Some(
                insert_work_item_activity(
                    &mut transaction,
                    principal.project_id,
                    work_item_id,
                    principal.actor_id,
                    "work_item_updated",
                    "Pull request linked through MCP",
                    &serde_json::json!({"pullRequestId": pull_request_id}),
                )
                .await?,
            )
        } else {
            None
        };
        if changed {
            enqueue_update(
                &mut transaction,
                principal.project_id,
                work_item_id,
                version,
            )
            .await?;
        }
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
        Action::McpWrite,
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
    Ok(match check_write_access(transaction, principal).await? {
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
            let result = applied(
                work_item_id,
                version,
                changed_fields,
                Some(finalized.activity_id),
                finalized.audit_id,
                location,
            );
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
    let result = applied(work_item_id, version, Vec::new(), None, result_id, location);
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

fn valid_priority(value: &str) -> bool {
    matches!(value, "no_priority" | "low" | "medium" | "high" | "urgent")
}

fn parse_id(value: &str) -> Option<Uuid> {
    (value.len() <= 64)
        .then(|| value.parse::<Uuid>().ok())
        .flatten()
}

fn normalize_text(value: String, max_characters: usize, allow_empty: bool) -> Option<String> {
    crate::work_item_commands::normalize_single_line(&value, max_characters, allow_empty).ok()
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

    use super::{WriteResult, normalize_text, parse_id, parse_request_id};

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
