use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    error::AppError,
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    message_content::MessageContent,
    policy::{Action, ActorKind, Decision},
    work_item_commands::{
        AllocatedNumberMetadata, AuditAttribution, AuditVersionMetadata, ChangeFinalization,
        CommandActor, ConflictKind, CreateWorkItemCommand, CreateWorkItemOutcome, LockedWorkItem,
        UpdateWorkItemCommand, UpdateWorkItemOutcome, advance_version, authorize_mutation,
        conflict_kind, create_work_item as execute_create_work_item, enqueue_update,
        finalize_change, finalize_comment, insert_activity as insert_work_item_activity,
        lock_membership, lock_work_item, record_audit, resolve_status, transition_allowed,
        update_work_item_by_id,
    },
};

const MAX_TITLE_CHARACTERS: usize = crate::work_item_commands::MAX_TITLE_CHARACTERS;
const MAX_DESCRIPTION_CHARACTERS: usize = crate::work_item_commands::MAX_DESCRIPTION_CHARACTERS;
const MAX_UPDATE_CHARACTERS: usize = 20_000;
const MAX_BLOCKED_REASON_CHARACTERS: usize = 2_000;

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
    },
    Conflict {
        work_item_id: String,
        current_version: i64,
        current: Option<ConflictProjection>,
        changed_fields: Vec<String>,
        retry_category: String,
    },
    NotFound {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    InvalidRequest {
        field: String,
        retry_category: String,
    },
    InvalidTransition {
        current_status_id: String,
        requested_status_id: String,
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
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

    fn invalid(field: &str) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            retry_category: "correct_input".to_owned(),
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
            return Ok(WriteResult::invalid("title"));
        };
        let description = match arguments.description.clone() {
            Some(value) => match normalize_text(value, MAX_DESCRIPTION_CHARACTERS, true) {
                Some(value) => value,
                None => return Ok(WriteResult::invalid("description")),
            },
            None => String::new(),
        };
        let priority = arguments
            .priority
            .clone()
            .unwrap_or_else(|| "no_priority".to_owned());
        if !matches!(
            priority.as_str(),
            "no_priority" | "low" | "medium" | "high" | "urgent"
        ) {
            return Ok(WriteResult::invalid("priority"));
        }
        let assignee_actor_id = match arguments.assignee_actor_id.as_deref() {
            Some(value) => match parse_id(value) {
                Some(actor_id) => Some(actor_id),
                None => return Ok(WriteResult::invalid("assigneeActorId")),
            },
            None => None,
        };
        let key = match IdempotencyKey::parse(&arguments.client_request_id) {
            Ok(key) => key,
            Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
            &mcp_actor(principal, arguments.client_request_id.clone()),
            CreateWorkItemCommand {
                title: &title,
                description: &description,
                type_id: None,
                status_id: None,
                priority: &priority,
                assignee_actor_id,
                parent_work_item_id: None,
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
                return Ok(WriteResult::invalid("assigneeActorId"));
            }
            CreateWorkItemOutcome::ParentNotFound | CreateWorkItemOutcome::ParentArchived(_) => {
                return Err(AppError::Internal("top-level MCP Work Item had a parent"));
            }
        };
        let result = applied(
            work_item_id,
            1,
            vec!["work_item".to_owned()],
            Some(finalized.activity_id),
            finalized.audit_id,
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
        return Ok(WriteResult::invalid("workItemId"));
    };
    if arguments.expected_version < 1 || arguments.status.is_empty() || arguments.status.len() > 80
    {
        return Ok(WriteResult::invalid("status"));
    }
    let key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(key) => key,
        Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
    let Some(status_id) =
        resolve_status(&mut transaction, principal.project_id, &arguments.status).await?
    else {
        return Ok(WriteResult::invalid("status"));
    };
    let outcome = update_work_item_by_id(
        &mut transaction,
        &mcp_actor(principal, arguments.client_request_id.clone()),
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: arguments.expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id: Some(status_id),
            priority: None,
            assignee_actor_id: None,
            summary: "Work Item status updated through MCP",
            audit_action: "work_item.mcp_status_updated",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    finish_update_outcome(
        transaction,
        principal,
        &reservation,
        arguments.expected_version,
        outcome,
        "work_item.mcp_status_unchanged",
        &arguments.client_request_id,
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
        return Ok(WriteResult::invalid("workItemId"));
    };
    if arguments.expected_version < 1
        || (arguments.title.is_none()
            && arguments.description.is_none()
            && arguments.priority.is_none()
            && arguments.assignee_actor_id.is_none()
            && arguments.clear_assignee != Some(true))
        || arguments.clear_assignee == Some(true) && arguments.assignee_actor_id.is_some()
    {
        return Ok(WriteResult::invalid("update"));
    }
    if status
        .as_deref()
        .is_some_and(|status| status.is_empty() || status.len() > 80)
    {
        return Ok(WriteResult::invalid("status"));
    }
    arguments.title = match arguments
        .title
        .take()
        .map(|value| normalize_text(value, MAX_TITLE_CHARACTERS, false))
    {
        Some(Some(value)) => Some(value),
        Some(None) => return Ok(WriteResult::invalid("title")),
        None => None,
    };
    arguments.description = match arguments
        .description
        .take()
        .map(|value| normalize_text(value, MAX_DESCRIPTION_CHARACTERS, true))
    {
        Some(Some(value)) => Some(value),
        Some(None) => return Ok(WriteResult::invalid("description")),
        None => None,
    };
    if arguments.priority.as_deref().is_some_and(|priority| {
        !matches!(
            priority,
            "no_priority" | "low" | "medium" | "high" | "urgent"
        )
    }) {
        return Ok(WriteResult::invalid("priority"));
    }
    let assignee_actor_id = match arguments.assignee_actor_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(actor_id) => Some(actor_id),
            None => return Ok(WriteResult::invalid("assigneeActorId")),
        },
        None => None,
    };
    let key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(key) => key,
        Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
            match resolve_status(&mut transaction, principal.project_id, status).await? {
                Some(status_id) => Some(status_id),
                None => return Ok(WriteResult::invalid("status")),
            }
        }
        None => None,
    };
    let outcome = update_work_item_by_id(
        &mut transaction,
        &mcp_actor(principal, arguments.client_request_id.clone()),
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: arguments.expected_version,
            title: arguments.title.as_deref(),
            description: arguments.description.as_deref(),
            type_id: None,
            status_id,
            priority: arguments.priority.as_deref(),
            assignee_actor_id: assignee_update,
            summary: "Work Item updated through MCP",
            audit_action: "work_item.mcp_updated",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    finish_update_outcome(
        transaction,
        principal,
        &reservation,
        arguments.expected_version,
        outcome,
        "work_item.mcp_update_unchanged",
        &arguments.client_request_id,
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
    let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
        return Ok(WriteResult::invalid("workItemId"));
    };
    let Some(update) = normalize_text(arguments.update.clone(), MAX_UPDATE_CHARACTERS, false)
    else {
        return Ok(WriteResult::invalid("update"));
    };
    let validated = match MessageContent::from_plain_text(update.clone()) {
        Ok(content) => content,
        Err(_) => return Ok(WriteResult::invalid("update")),
    };
    let key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(key) => key,
        Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
        return Ok(not_found());
    };
    if current.archived {
        return Ok(WriteResult::Conflict {
            work_item_id: work_item_id.to_string(),
            current_version: current.version,
            current: Some((&current).into()),
            changed_fields: vec!["archived".to_owned()],
            retry_category: "do_not_retry".to_owned(),
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
        &mcp_actor(principal, arguments.client_request_id.clone()),
        work_item_id,
        comment_id,
        current.version,
        "work_item.mcp_update_posted",
    )
    .await?;
    let result = applied(
        work_item_id,
        current.version,
        vec!["update".to_owned()],
        Some(comment_id),
        result_id,
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
            return Ok(WriteResult::invalid("parentWorkItemId"));
        };
        let Some(title) = normalize_text(arguments.title.clone(), MAX_TITLE_CHARACTERS, false)
        else {
            return Ok(WriteResult::invalid("title"));
        };
        let description = match arguments.description.clone() {
            Some(value) => match normalize_text(value, MAX_DESCRIPTION_CHARACTERS, true) {
                Some(value) => value,
                None => return Ok(WriteResult::invalid("description")),
            },
            None => String::new(),
        };
        let key = match IdempotencyKey::parse(&arguments.client_request_id) {
            Ok(key) => key,
            Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
            &mcp_actor(principal, arguments.client_request_id.clone()),
            CreateWorkItemCommand {
                title: &title,
                description: &description,
                type_id: None,
                status_id: None,
                priority: "no_priority",
                assignee_actor_id: None,
                parent_work_item_id: Some(parent_id),
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
            CreateWorkItemOutcome::ParentNotFound => return Ok(not_found()),
            CreateWorkItemOutcome::ParentArchived(parent) => {
                return Ok(WriteResult::Conflict {
                    work_item_id: parent_id.to_string(),
                    current_version: parent.version,
                    current: Some((&parent).into()),
                    changed_fields: vec!["archived".to_owned()],
                    retry_category: "do_not_retry".to_owned(),
                });
            }
            CreateWorkItemOutcome::InvalidAssignee => {
                return Err(AppError::Internal("unassigned subtask had an assignee"));
            }
        };
        let result = applied(
            subtask_id,
            1,
            vec!["subtask".to_owned()],
            Some(finalized.activity_id),
            finalized.audit_id,
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
    let result =
        async {
            let Some(work_item_id) = parse_id(&arguments.work_item_id) else {
                return Ok(WriteResult::invalid("workItemId"));
            };
            let Some(pull_request_id) = parse_id(&arguments.pull_request_id) else {
                return Ok(WriteResult::invalid("pullRequestId"));
            };
            if arguments.expected_version < 1 {
                return Ok(WriteResult::invalid("expectedVersion"));
            }
            let key = match IdempotencyKey::parse(&arguments.client_request_id) {
                Ok(key) => key,
                Err(_) => return Ok(WriteResult::invalid("clientRequestId")),
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
                return Ok(not_found());
            };
            if let Some(conflict) =
                version_or_archive_conflict(&current, arguments.expected_version)
            {
                return Ok(conflict);
            }
            let pr_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM github_pull_requests WHERE project_id = $1 AND id = $2)",
        ).bind(principal.project_id).bind(pull_request_id).fetch_one(&mut *transaction).await?;
            if !pr_exists {
                return Ok(not_found());
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
                &arguments.client_request_id,
                serde_json::json!({"pullRequestId": pull_request_id, "changed": changed}),
            )
            .await?;
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
        let Some(work_item_id) = parse_id(&arguments.work_item_id) else { return Ok(WriteResult::invalid("workItemId")); };
        let Some(reason) = normalize_text(arguments.reason.clone(), MAX_BLOCKED_REASON_CHARACTERS, false) else { return Ok(WriteResult::invalid("reason")); };
        if arguments.expected_version < 1 { return Ok(WriteResult::invalid("expectedVersion")); }
        let key = match IdempotencyKey::parse(&arguments.client_request_id) { Ok(key) => key, Err(_) => return Ok(WriteResult::invalid("clientRequestId")) };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? { return Ok(result); }
        let scope = resource_bound_scope("mcp_work_items", work_item_id, "mark_blocked")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(&mut transaction, principal.actor_id, &scope, &key, request_hash).await? {
            IdempotencyOutcome::Replay(replay) => { transaction.commit().await?; return Ok(replay.decode::<WriteResult>()?.replayed()); }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(current) = lock_work_item(&mut transaction, principal.project_id, work_item_id).await? else { return Ok(not_found()); };
        if let Some(conflict) = version_or_archive_conflict(&current, arguments.expected_version) { return Ok(conflict); }
        if current.blocked_reason.as_deref() == Some(&reason) {
            return finalize_noop(
                transaction, principal, &reservation, work_item_id, current.version,
                "work_item.mcp_blocker_unchanged", &arguments.client_request_id,
            ).await;
        }
        sqlx::query(
            "UPDATE work_items SET blocked_reason = $3, blocked_at = now(), blocked_by_actor_id = $4, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $5",
        ).bind(principal.project_id).bind(work_item_id).bind(reason).bind(principal.actor_id).bind(arguments.expected_version)
        .execute(&mut *transaction).await?;
        finalize_work_item_change(
            transaction, principal, &reservation, work_item_id, arguments.expected_version + 1,
            vec!["blockedReason".to_owned()], "Work Item marked blocked through MCP", "work_item.mcp_marked_blocked",
            &arguments.client_request_id,
        ).await
    }.await;
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
        let Some(work_item_id) = parse_id(&arguments.work_item_id) else { return Ok(WriteResult::invalid("workItemId")); };
        if arguments.expected_version < 1 { return Ok(WriteResult::invalid("expectedVersion")); }
        let key = match IdempotencyKey::parse(&arguments.client_request_id) { Ok(key) => key, Err(_) => return Ok(WriteResult::invalid("clientRequestId")) };
        let mut transaction = state.pool.begin().await?;
        if let Some(result) = authorize_write(&mut transaction, principal).await? { return Ok(result); }
        let scope = resource_bound_scope("mcp_work_items", work_item_id, "ready_for_review")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(&mut transaction, principal.actor_id, &scope, &key, request_hash).await? {
            IdempotencyOutcome::Replay(replay) => { transaction.commit().await?; return Ok(replay.decode::<WriteResult>()?.replayed()); }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(current) = lock_work_item(&mut transaction, principal.project_id, work_item_id).await? else { return Ok(not_found()); };
        if let Some(conflict) = version_or_archive_conflict(&current, arguments.expected_version) { return Ok(conflict); }
        let Some(review_status_id) = resolve_status(&mut transaction, principal.project_id, "review").await? else { return Ok(WriteResult::invalid("reviewStatus")); };
        if current.status_id != review_status_id && !transition_allowed(&mut transaction, principal.project_id, current.status_id, review_status_id).await? {
            return Ok(WriteResult::InvalidTransition {
                current_status_id: current.status_id.to_string(), requested_status_id: review_status_id.to_string(),
                retry_category: "ask_user_or_choose_allowed_transition".to_owned(),
            });
        }
        let mut changed = Vec::new();
        if current.status_id != review_status_id { changed.push("status".to_owned()); }
        if current.blocked_reason.is_some() { changed.push("blockedReason".to_owned()); }
        if changed.is_empty() {
            return finalize_noop(
                transaction, principal, &reservation, work_item_id, current.version,
                "work_item.mcp_review_unchanged", &arguments.client_request_id,
            ).await;
        }
        sqlx::query(
            "UPDATE work_items SET status_id = $3, blocked_reason = NULL, blocked_at = NULL, blocked_by_actor_id = NULL, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $4",
        ).bind(principal.project_id).bind(work_item_id).bind(review_status_id).bind(arguments.expected_version)
        .execute(&mut *transaction).await?;
        finalize_work_item_change(
            transaction, principal, &reservation, work_item_id, arguments.expected_version + 1, changed,
            "Work Item marked ready for review through MCP", "work_item.mcp_ready_for_review", &arguments.client_request_id,
        ).await
    }.await;
    match result {
        Ok(result) => result,
        Err(error) => command_error("work_item_mark_ready_for_review", error),
    }
}

async fn authorize_write(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
) -> Result<Option<WriteResult>, AppError> {
    if !principal.can_write() {
        return Ok(Some(WriteResult::PermissionDenied {
            retry_category: "request_write_scope".to_owned(),
        }));
    }
    // Rows written below (activity, comments) record the app a person is
    // acting through.
    super::common::mark_via(transaction, principal).await?;
    let membership = lock_membership(transaction, principal.actor_id, principal.project_id).await?;
    let Some(membership) = membership else {
        return Ok(Some(not_found()));
    };
    if !membership.enabled
        || !matches!(
            principal.actor_kind,
            ActorKind::Human | ActorKind::ExternalAgent
        )
    {
        return Ok(Some(not_found()));
    }
    if membership.project_archived_at.is_some() {
        return Ok(Some(WriteResult::Conflict {
            work_item_id: String::new(),
            current_version: 0,
            current: None,
            changed_fields: vec!["projectArchived".to_owned()],
            retry_category: "do_not_retry".to_owned(),
        }));
    }
    let decision = authorize_mutation(
        &mcp_actor(principal, String::new()),
        &membership,
        Action::McpWrite,
        None,
    )?;
    if !matches!(decision, Decision::Allow) {
        return Ok(Some(WriteResult::PermissionDenied {
            retry_category: "ask_project_owner".to_owned(),
        }));
    }
    Ok(None)
}

fn version_or_archive_conflict(
    current: &LockedWorkItem,
    expected_version: i64,
) -> Option<WriteResult> {
    conflict_kind(current, expected_version).map(|kind| WriteResult::Conflict {
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
    })
}

async fn finish_update_outcome(
    mut transaction: Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    reservation: &crate::idempotency::IdempotencyReservation,
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
            let result = applied(
                work_item_id,
                version,
                changed_fields,
                Some(finalized.activity_id),
                finalized.audit_id,
            );
            complete(&mut transaction, reservation, StatusCode::OK, &result).await?;
            transaction.commit().await?;
            Ok(result)
        }
        UpdateWorkItemOutcome::Noop(current) => {
            finalize_noop(
                transaction,
                principal,
                reservation,
                current.id,
                current.version,
                noop_action,
                request_id,
            )
            .await
        }
        UpdateWorkItemOutcome::Conflict(current) => {
            Ok(version_or_archive_conflict(&current, expected_version)
                .ok_or(AppError::Internal("command returned an empty conflict"))?)
        }
        UpdateWorkItemOutcome::NotFound => Ok(not_found()),
        UpdateWorkItemOutcome::InvalidAssignee => Ok(WriteResult::invalid("assigneeActorId")),
        UpdateWorkItemOutcome::InvalidTransition {
            current_status_id,
            requested_status_id,
        } => Ok(WriteResult::InvalidTransition {
            current_status_id: current_status_id.to_string(),
            requested_status_id: requested_status_id.to_string(),
            retry_category: "ask_user_or_choose_allowed_transition".to_owned(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_work_item_change(
    mut transaction: Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    reservation: &crate::idempotency::IdempotencyReservation,
    work_item_id: Uuid,
    version: i64,
    changed_fields: Vec<String>,
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
            activity_payload: serde_json::json!({"changedFields": changed_fields}),
            audit_metadata: serde_json::json!({"changedFields": changed_fields, "version": version}),
            version,
        },
    )
    .await?;
    let result = applied(
        work_item_id,
        version,
        changed_fields,
        Some(finalized.activity_id),
        finalized.audit_id,
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

fn mcp_actor(principal: &McpPrincipal, request_id: String) -> CommandActor {
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

async fn finalize_noop(
    mut transaction: Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    reservation: &crate::idempotency::IdempotencyReservation,
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
    let result = applied(work_item_id, version, Vec::new(), None, result_id);
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
) -> WriteResult {
    WriteResult::Applied {
        work_item_id: work_item_id.to_string(),
        version,
        changed_fields,
        activity_id: activity_id.map(|id| id.to_string()),
        result_id: result_id.to_string(),
        idempotent_replay: false,
    }
}

fn not_found() -> WriteResult {
    WriteResult::NotFound {
        retry_category: "do_not_retry".to_owned(),
    }
}

fn parse_id(value: &str) -> Option<Uuid> {
    (value.len() <= 64)
        .then(|| value.parse::<Uuid>().ok())
        .flatten()
}

fn normalize_text(value: String, max_characters: usize, allow_empty: bool) -> Option<String> {
    crate::work_item_commands::normalize_single_line(&value, max_characters, allow_empty).ok()
}

fn command_error(command: &str, error: AppError) -> WriteResult {
    match error {
        AppError::Conflict("idempotency_key", "payload mismatch") => WriteResult::Conflict {
            work_item_id: String::new(),
            current_version: 0,
            current: None,
            changed_fields: vec!["clientRequestId".to_owned()],
            retry_category: "use_new_request_id".to_owned(),
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

    use super::{WriteResult, normalize_text, parse_id};

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
        })
        .expect("serialize write result");
        assert_eq!(result["idempotentReplay"], json!(false));
        assert_eq!(result["changedFields"], json!(["title"]));
        assert!(result.get("idempotent_replay").is_none());
    }
}
