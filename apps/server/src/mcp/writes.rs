use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    error::AppError,
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    message_content::MessageContent,
    outbox::{OutboxEventPayload, WorkItemUpdatedPayload, enqueue_typed},
    work_items::insert_work_item_activity,
};

const MAX_TITLE_CHARACTERS: usize = 240;
const MAX_DESCRIPTION_CHARACTERS: usize = 100_000;
const MAX_UPDATE_CHARACTERS: usize = 20_000;
const MAX_BLOCKED_REASON_CHARACTERS: usize = 2_000;
const BOARD_RANK_STEP: i64 = 1024;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetStatusArguments {
    pub work_item_id: String,
    pub expected_version: i64,
    pub status: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostUpdateArguments {
    pub work_item_id: String,
    pub update: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkItemArguments {
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
    pub parent_work_item_id: String,
    pub title: String,
    pub description: Option<String>,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkPullRequestArguments {
    pub work_item_id: String,
    pub expected_version: i64,
    pub pull_request_id: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkBlockedArguments {
    pub work_item_id: String,
    pub expected_version: i64,
    pub reason: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkReadyForReviewArguments {
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

#[derive(Debug, FromRow)]
struct LockedWorkItem {
    id: Uuid,
    version: i64,
    title: String,
    description: String,
    status_id: Uuid,
    priority: String,
    assignee_actor_id: Option<Uuid>,
    archived: bool,
    blocked_reason: Option<String>,
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

#[derive(Debug, FromRow)]
struct MembershipRow {
    role: String,
    project_archived: bool,
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
    let Some(current) =
        lock_work_item(&mut transaction, principal.project_id, work_item_id).await?
    else {
        return Ok(not_found());
    };
    if let Some(conflict) = version_or_archive_conflict(&current, arguments.expected_version) {
        return Ok(conflict);
    }
    let Some(status_id) =
        resolve_status(&mut transaction, principal.project_id, &arguments.status).await?
    else {
        return Ok(WriteResult::invalid("status"));
    };
    if status_id != current.status_id
        && !transition_allowed(
            &mut transaction,
            principal.project_id,
            current.status_id,
            status_id,
        )
        .await?
    {
        return Ok(WriteResult::InvalidTransition {
            current_status_id: current.status_id.to_string(),
            requested_status_id: status_id.to_string(),
            retry_category: "ask_user_or_choose_allowed_transition".to_owned(),
        });
    }
    if status_id == current.status_id {
        return finalize_noop(
            transaction,
            principal,
            &reservation,
            current.id,
            current.version,
            "work_item.mcp_status_unchanged",
            &arguments.client_request_id,
        )
        .await;
    }
    sqlx::query(
        "UPDATE work_items SET status_id = $3, version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $4",
    )
    .bind(principal.project_id)
    .bind(work_item_id)
    .bind(status_id)
    .bind(arguments.expected_version)
    .execute(&mut *transaction)
    .await?;
    finalize_work_item_change(
        transaction,
        principal,
        &reservation,
        work_item_id,
        arguments.expected_version + 1,
        vec!["status".to_owned()],
        "Work Item status updated through MCP",
        "work_item.mcp_status_updated",
        &arguments.client_request_id,
    )
    .await
}

pub async fn update_work_item(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UpdateWorkItemArguments,
) -> WriteResult {
    match update_work_item_inner(state, principal, arguments).await {
        Ok(result) => result,
        Err(error) => command_error("work_item_update", error),
    }
}

async fn update_work_item_inner(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: UpdateWorkItemArguments,
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
    if let Some(actor_id) = assignee_actor_id
        && !membership_exists(&mut transaction, principal.project_id, actor_id).await?
    {
        return Ok(WriteResult::invalid("assigneeActorId"));
    }
    let scope = resource_bound_scope("mcp_work_items", work_item_id, "update")?;
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
    if let Some(conflict) = version_or_archive_conflict(&current, arguments.expected_version) {
        return Ok(conflict);
    }
    let title = arguments
        .title
        .clone()
        .unwrap_or_else(|| current.title.clone());
    let description = arguments
        .description
        .clone()
        .unwrap_or_else(|| current.description.clone());
    let priority = arguments
        .priority
        .clone()
        .unwrap_or_else(|| current.priority.clone());
    let assignee = if arguments.clear_assignee == Some(true) {
        None
    } else {
        assignee_actor_id.or(current.assignee_actor_id)
    };
    let mut changed = Vec::new();
    if title != current.title {
        changed.push("title".to_owned());
    }
    if description != current.description {
        changed.push("description".to_owned());
    }
    if priority != current.priority {
        changed.push("priority".to_owned());
    }
    if assignee != current.assignee_actor_id {
        changed.push("assignee".to_owned());
    }
    if changed.is_empty() {
        return finalize_noop(
            transaction,
            principal,
            &reservation,
            current.id,
            current.version,
            "work_item.mcp_update_unchanged",
            &arguments.client_request_id,
        )
        .await;
    }
    sqlx::query(
        r#"
        UPDATE work_items
        SET title = $3, description = $4, priority = $5, assignee_actor_id = $6,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $7
        "#,
    )
    .bind(principal.project_id)
    .bind(work_item_id)
    .bind(title)
    .bind(description)
    .bind(priority)
    .bind(assignee)
    .bind(arguments.expected_version)
    .execute(&mut *transaction)
    .await?;
    finalize_work_item_change(
        transaction,
        principal,
        &reservation,
        work_item_id,
        arguments.expected_version + 1,
        changed,
        "Work Item updated through MCP",
        "work_item.mcp_updated",
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
    let sequence = sqlx::query_scalar::<_, i64>(
        r#"
        WITH allocated AS (
            UPDATE work_items
            SET next_timeline_sequence = next_timeline_sequence + 1
            WHERE project_id = $1 AND id = $2
            RETURNING next_timeline_sequence - 1 AS sequence
        ), inserted AS (
            INSERT INTO work_item_timeline_entries (
                project_id, work_item_id, sequence, kind, comment_id
            )
            SELECT $1, $2, sequence, 'comment', $3 FROM allocated
            RETURNING sequence
        )
        SELECT sequence FROM inserted
        "#,
    )
    .bind(principal.project_id)
    .bind(work_item_id)
    .bind(comment_id)
    .fetch_one(&mut *transaction)
    .await?;
    enqueue_update(
        &mut transaction,
        principal.project_id,
        work_item_id,
        current.version,
    )
    .await?;
    let result_id = insert_mcp_audit(
        &mut transaction,
        principal,
        "work_item.mcp_update_posted",
        work_item_id,
        &arguments.client_request_id,
        serde_json::json!({"commentId": comment_id, "sequence": sequence}),
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
    mut arguments: CreateSubtaskArguments,
) -> WriteResult {
    let result = async {
        let Some(parent_id) = parse_id(&arguments.parent_work_item_id) else {
            return Ok(WriteResult::invalid("parentWorkItemId"));
        };
        let Some(title) = normalize_text(arguments.title.clone(), MAX_TITLE_CHARACTERS, false) else {
            return Ok(WriteResult::invalid("title"));
        };
        let description = match arguments.description.take() {
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
        let reservation = match reserve(&mut transaction, principal.actor_id, &scope, &key, request_hash).await? {
            IdempotencyOutcome::Replay(replay) => {
                transaction.commit().await?;
                return Ok(replay.decode::<WriteResult>()?.replayed());
            }
            IdempotencyOutcome::Reserved(reservation) => reservation,
        };
        let Some(parent) = lock_work_item(&mut transaction, principal.project_id, parent_id).await?
        else { return Ok(not_found()); };
        if parent.archived {
            return Ok(WriteResult::Conflict {
                work_item_id: parent_id.to_string(), current_version: parent.version,
                current: Some((&parent).into()),
                changed_fields: vec!["archived".to_owned()], retry_category: "do_not_retry".to_owned()
            });
        }
        let number = sqlx::query_scalar::<_, i64>(
            "UPDATE project_counters SET next_work_item_number = next_work_item_number + 1, updated_at = now() WHERE project_id = $1 RETURNING next_work_item_number - 1",
        )
        .bind(principal.project_id)
        .fetch_one(&mut *transaction)
        .await?;
        let (type_id, status_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT type.id, status.id
            FROM work_item_types type
            CROSS JOIN work_item_statuses status
            WHERE type.project_id = $1 AND type.enabled AND type.is_default
              AND status.project_id = $1 AND status.enabled AND status.is_default
            "#,
        )
        .bind(principal.project_id)
        .fetch_one(&mut *transaction)
        .await?;
        let board_rank = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(max(board_rank), 0) + $3 FROM work_items WHERE project_id = $1 AND status_id = $2",
        )
        .bind(principal.project_id)
        .bind(status_id)
        .bind(BOARD_RANK_STEP)
        .fetch_one(&mut *transaction)
        .await?;
        let subtask_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO work_items (
                id, project_id, number, title, description, type_id, status_id,
                priority, created_by_actor_id, board_rank, parent_work_item_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'no_priority', $8, $9, $10)
            "#,
        )
        .bind(subtask_id).bind(principal.project_id).bind(number).bind(title).bind(description)
        .bind(type_id).bind(status_id).bind(principal.actor_id).bind(board_rank).bind(parent_id)
        .execute(&mut *transaction).await?;
        let activity_id = insert_work_item_activity(
            &mut transaction, principal.project_id, subtask_id, principal.actor_id,
            "work_item_created", "Subtask created through MCP", &serde_json::json!({"parentWorkItemId": parent_id}),
        ).await?;
        enqueue_update(&mut transaction, principal.project_id, subtask_id, 1).await?;
        let result_id = insert_mcp_audit(
            &mut transaction, principal, "work_item.mcp_subtask_created", subtask_id,
            &arguments.client_request_id, serde_json::json!({"parentWorkItemId": parent_id, "number": number}),
        ).await?;
        let result = applied(subtask_id, 1, vec!["subtask".to_owned()], Some(activity_id), result_id);
        complete(&mut transaction, &reservation, StatusCode::CREATED, &result).await?;
        transaction.commit().await?;
        Ok(result)
    }.await;
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
    let membership = sqlx::query_as::<_, MembershipRow>(
        r#"
        SELECT pm.role, p.archived_at IS NOT NULL AS project_archived
        FROM project_memberships pm
        JOIN projects p ON p.id = pm.project_id
        JOIN actors a ON a.id = pm.actor_id
        WHERE pm.project_id = $1 AND pm.actor_id = $2
          AND pm.enabled AND a.kind = 'human' AND a.status = 'active'
        FOR SHARE OF pm, p, a
        "#,
    )
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(membership) = membership else {
        return Ok(Some(not_found()));
    };
    if membership.project_archived {
        return Ok(Some(WriteResult::Conflict {
            work_item_id: String::new(),
            current_version: 0,
            current: None,
            changed_fields: vec!["projectArchived".to_owned()],
            retry_category: "do_not_retry".to_owned(),
        }));
    }
    if !matches!(membership.role.as_str(), "owner" | "admin" | "member") {
        return Ok(Some(WriteResult::PermissionDenied {
            retry_category: "ask_project_owner".to_owned(),
        }));
    }
    Ok(None)
}

async fn lock_work_item(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<LockedWorkItem>, AppError> {
    Ok(sqlx::query_as::<_, LockedWorkItem>(
        r#"
        SELECT id, version, title, description, status_id, priority,
               assignee_actor_id, archived_at IS NOT NULL AS archived,
               blocked_reason
        FROM work_items
        WHERE project_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn version_or_archive_conflict(
    current: &LockedWorkItem,
    expected_version: i64,
) -> Option<WriteResult> {
    if current.version != expected_version {
        return Some(WriteResult::Conflict {
            work_item_id: current.id.to_string(),
            current_version: current.version,
            current: Some(current.into()),
            changed_fields: vec!["version".to_owned()],
            retry_category: "refetch_then_decide".to_owned(),
        });
    }
    current.archived.then(|| WriteResult::Conflict {
        work_item_id: current.id.to_string(),
        current_version: current.version,
        current: Some(current.into()),
        changed_fields: vec!["archived".to_owned()],
        retry_category: "do_not_retry".to_owned(),
    })
}

async fn resolve_status(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    let id = value.parse::<Uuid>().ok();
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM work_item_statuses
        WHERE project_id = $1 AND enabled
          AND (($2::uuid IS NOT NULL AND id = $2) OR lower(name) = lower($3) OR semantic_category = lower($3))
        ORDER BY position, id LIMIT 2
        "#,
    ).bind(project_id).bind(id).bind(value).fetch_all(&mut **transaction).await?;
    Ok((rows.len() == 1).then(|| rows[0]))
}

async fn transition_allowed(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    from: Uuid,
    to: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_item_status_transitions WHERE project_id = $1 AND from_status_id = $2 AND to_status_id = $3)",
    ).bind(project_id).bind(from).bind(to).fetch_one(&mut **transaction).await?)
}

async fn membership_exists(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2 AND enabled)",
    ).bind(project_id).bind(actor_id).fetch_one(&mut **transaction).await?)
}

async fn advance_version(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    expected: i64,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        "UPDATE work_items SET version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $3 RETURNING version",
    ).bind(project_id).bind(work_item_id).bind(expected).fetch_optional(&mut **transaction).await?
    .ok_or(AppError::Internal("locked MCP Work Item version changed"))
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
    let activity_id = insert_work_item_activity(
        &mut transaction,
        principal.project_id,
        work_item_id,
        principal.actor_id,
        "work_item_updated",
        summary,
        &serde_json::json!({"changedFields": changed_fields}),
    )
    .await?;
    enqueue_update(
        &mut transaction,
        principal.project_id,
        work_item_id,
        version,
    )
    .await?;
    let result_id = insert_mcp_audit(
        &mut transaction,
        principal,
        action,
        work_item_id,
        request_id,
        serde_json::json!({"changedFields": changed_fields, "version": version}),
    )
    .await?;
    let result = applied(
        work_item_id,
        version,
        changed_fields,
        Some(activity_id),
        result_id,
    );
    complete(&mut transaction, reservation, StatusCode::OK, &result).await?;
    transaction.commit().await?;
    Ok(result)
}

async fn enqueue_update(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    version: i64,
) -> Result<(), AppError> {
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id,
            version,
        }),
    )
    .await?;
    Ok(())
}

async fn insert_mcp_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    action: &str,
    target_id: Uuid,
    request_id: &str,
    metadata: serde_json::Value,
) -> Result<Uuid, AppError> {
    let audit_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action, target_kind,
            target_id, outcome, request_id, metadata, channel, client_name,
            mcp_credential_id
        ) VALUES ($1, $2, $3, $4, $5, 'work_item', $6, 'succeeded', $7, $8, 'mcp', $9, $10)
        "#,
    )
    .bind(audit_id)
    .bind(principal.actor_id)
    .bind(principal.project_id)
    .bind(principal.authentication_class)
    .bind(action)
    .bind(target_id)
    .bind(request_id)
    .bind(metadata)
    .bind(&principal.client_name)
    .bind(principal.credential_id)
    .execute(&mut **transaction)
    .await?;
    Ok(audit_id)
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
    let value = value.trim().to_owned();
    ((!value.is_empty() || allow_empty)
        && value.chars().count() <= max_characters
        && !value.chars().any(char::is_control))
    .then_some(value)
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
            tracing::error!(command, category = error.log_category(), "MCP write failed");
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
