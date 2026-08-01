use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::ActorView,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    message_content::{MessageContent, MessageContentError, MessageNode},
    outbox::{
        ConversationCreatedPayload, OutboxConversationKind, OutboxEventPayload,
        WorkItemUpdatedPayload, enqueue_typed,
    },
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

const MAX_TITLE_CHARACTERS: usize = 240;
const MAX_DESCRIPTION_CHARACTERS: usize = 100_000;
const MAX_SEARCH_CHARACTERS: usize = 120;
const DEFAULT_PAGE_SIZE: u16 = 50;
const MAX_PAGE_SIZE: u16 = 100;
const BOARD_RANK_STEP: i64 = 1024;

const DEFAULT_TYPES: [(&str, i32, bool); 6] = [
    ("Feature", 1, false),
    ("Bug", 2, false),
    ("Task", 3, true),
    ("Request", 4, false),
    ("Research", 5, false),
    ("Improvement", 6, false),
];

const DEFAULT_STATUSES: [(&str, &str, i32, bool); 7] = [
    ("Backlog", "backlog", 1, true),
    ("Ready", "unstarted", 2, false),
    ("In Progress", "started", 3, false),
    ("Review", "review", 4, false),
    ("Verification", "verification", 5, false),
    ("Done", "completed", 6, false),
    ("Canceled", "canceled", 7, false),
];

const DEFAULT_TRANSITIONS: [(&str, &str); 17] = [
    ("Backlog", "Ready"),
    ("Backlog", "Canceled"),
    ("Ready", "Backlog"),
    ("Ready", "In Progress"),
    ("Ready", "Canceled"),
    ("In Progress", "Ready"),
    ("In Progress", "Review"),
    ("In Progress", "Canceled"),
    ("Review", "In Progress"),
    ("Review", "Verification"),
    ("Review", "Done"),
    ("Review", "Canceled"),
    ("Verification", "In Progress"),
    ("Verification", "Done"),
    ("Verification", "Canceled"),
    ("Done", "In Progress"),
    ("Canceled", "Backlog"),
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/work-items",
            get(list_work_items).post(create_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/board",
            get(get_work_item_board),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}",
            get(get_work_item).patch(update_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
            axum::routing::post(archive_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/move",
            axum::routing::post(move_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
            axum::routing::post(work_with_worker),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
            axum::routing::post(restore_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/by-key/{key}",
            get(get_work_item_by_key),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-assignees",
            get(search_assignable_actors),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-references",
            get(search_work_item_references),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-workflow",
            get(get_workflow),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
            get(list_acceptance_criteria).post(create_acceptance_criterion),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            axum::routing::patch(update_acceptance_criterion)
                .delete(remove_acceptance_criterion),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
            get(get_work_item_timeline),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
            axum::routing::post(create_work_item_comment),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            axum::routing::post(link_work_item_thread).delete(unlink_work_item_thread),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            axum::routing::post(link_work_item_repository)
                .delete(unlink_work_item_repository),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
            axum::routing::post(set_primary_work_item_repository),
        )
}

/// Reports whether the M4 workflow schema is present in the transaction's
/// current search path. Historical migration tests deliberately seed an M0-M3
/// snapshot before applying later migrations, so the development seed keeps
/// that upgrade path usable while provisioning the workflow on every current
/// schema.
pub async fn schema_is_available(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('work_item_types') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?,
    )
}

#[derive(Debug, FromRow)]
struct ProjectMembershipRow {
    project_id: Uuid,
    project_archived_at: Option<String>,
    role: String,
    membership_enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct WorkflowTypeRow {
    id: Uuid,
    name: String,
    position: i32,
    is_default: bool,
    enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct WorkflowStatusRow {
    id: Uuid,
    name: String,
    semantic_category: String,
    position: i32,
    is_default: bool,
    enabled: bool,
}

#[derive(Debug, FromRow)]
struct WorkItemRow {
    id: Uuid,
    project_id: Uuid,
    project_key: String,
    number: i64,
    title: String,
    description: String,
    type_id: Uuid,
    type_name: String,
    status_id: Uuid,
    status_name: String,
    semantic_category: String,
    priority: String,
    assignee_actor_id: Option<Uuid>,
    assignee_display_name: Option<String>,
    assignee_handle: Option<String>,
    assignee_kind: Option<String>,
    assignee_active: Option<bool>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    acceptance_criteria: serde_json::Value,
    linked_threads: serde_json::Value,
    linked_repositories: serde_json::Value,
    linked_pull_requests: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemListQuery {
    q: Option<String>,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    semantic_category: Option<String>,
    priority: Option<String>,
    assignee_actor_id: Option<Uuid>,
    unassigned: Option<bool>,
    archived: Option<bool>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateWorkItemRequest {
    title: String,
    #[serde(default)]
    description: String,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    #[serde(default = "default_priority")]
    priority: String,
    assignee_actor_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateWorkItemRequest {
    expected_version: i64,
    title: Option<String>,
    description: Option<String>,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    priority: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    assignee_actor_id: Option<Option<Uuid>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemVersionRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssigneeSearchQuery {
    q: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemReferenceSearchQuery {
    q: Option<String>,
    limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemView {
    id: Uuid,
    project_id: Uuid,
    key: String,
    number: i64,
    title: String,
    description: String,
    item_type: WorkItemTypeView,
    status: WorkItemStatusView,
    priority: String,
    assignee: Option<AssigneeView>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    acceptance_criteria: Vec<AcceptanceCriterionView>,
    linked_threads: Vec<LinkedThreadView>,
    linked_repositories: Vec<LinkedRepositoryView>,
    linked_pull_requests: Vec<LinkedPullRequestView>,
    capabilities: WorkItemCapabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedThreadView {
    id: Uuid,
    name: String,
    parent_channel_id: Uuid,
    parent_channel_name: String,
    archived_at: Option<String>,
    newest_run_state: Option<String>,
    href: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedRepositoryView {
    id: Uuid,
    owner: String,
    name: String,
    role: String,
    preparation_state: String,
    available: bool,
    archived_at: Option<String>,
    href: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedPullRequestView {
    id: Uuid,
    number: i64,
    title: String,
    url: String,
    state: String,
    draft: bool,
    source: String,
    confirmed: bool,
    checks_state: String,
    review_state: String,
    mergeability_state: String,
    sync_state: String,
    sync_failure_category: Option<String>,
    automation_state: String,
    automation_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkRepositoryRequest {
    expected_version: i64,
    role: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyLinkRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptanceCriterionView {
    id: Uuid,
    text: String,
    position: i32,
    completed: bool,
    completed_by: Option<ActorView>,
    completed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCriterionRequest {
    expected_version: i64,
    text: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCriterionRequest {
    expected_version: i64,
    text: Option<String>,
    position: Option<i32>,
    completed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveCriterionRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCommentRequest {
    content: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemReferenceView {
    id: Uuid,
    key: String,
    title: String,
    status_category: String,
    archived_at: Option<String>,
    href: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemReferenceSearchResponse {
    items: Vec<WorkItemReferenceView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCommentView {
    id: Uuid,
    author: ActorView,
    content: serde_json::Value,
    plain_text: String,
    mentioned_actors: Vec<ActorView>,
    referenced_work_items: Vec<WorkItemReferenceView>,
    created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentAppendResponse {
    work_item_id: Uuid,
    sequence: i64,
    comment: WorkItemCommentView,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemTimelineQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, FromRow)]
struct WorkItemTimelineRow {
    sequence: i64,
    kind: String,
    created_at: String,
    activity_id: Option<Uuid>,
    activity_kind: Option<String>,
    activity_summary: Option<String>,
    activity_payload: Option<serde_json::Value>,
    comment_id: Option<Uuid>,
    comment_content: Option<serde_json::Value>,
    comment_plain_text: Option<String>,
    actor_id: Uuid,
    actor_kind: String,
    actor_display_name: String,
    actor_handle: String,
}

#[derive(Debug, FromRow)]
struct WorkItemReferenceRow {
    id: Uuid,
    key: String,
    title: String,
    status_category: String,
    archived_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTimelineResponse {
    entries: Vec<WorkItemTimelineEntryView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTimelineEntryView {
    sequence: i64,
    kind: String,
    created_at: String,
    activity: Option<WorkItemActivityView>,
    comment: Option<WorkItemCommentView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemActivityView {
    id: Uuid,
    actor: ActorView,
    kind: String,
    summary: String,
    payload: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTypeView {
    id: Uuid,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemStatusView {
    id: Uuid,
    name: String,
    semantic_category: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssigneeView {
    actor_id: Uuid,
    display_name: String,
    handle: String,
    kind: String,
    active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssigneeSearchResponse {
    actors: Vec<ActorView>,
}

#[derive(Debug, FromRow)]
struct AssigneeSearchRow {
    actor_id: Uuid,
    display_name: String,
    handle: String,
    kind: String,
}

#[derive(Debug, FromRow)]
struct CriterionRow {
    text: String,
    position: i32,
    completed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCapabilities {
    view: bool,
    edit: bool,
    archive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkWithWorkerRequest {
    expected_version: i64,
    channel_id: Uuid,
    prompt: String,
    repository_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkWithWorkerResponse {
    thread_id: Uuid,
    run: crate::api::StartRunResponse,
}

#[derive(Debug, FromRow)]
struct WorkerInvocationRow {
    conversation_id: Uuid,
    repository_id: Uuid,
    request_hash: Vec<u8>,
    response_body: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemListResponse {
    items: Vec<WorkItemCardView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemBoardResponse {
    columns: Vec<WorkItemBoardColumn>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemBoardColumn {
    status: WorkItemStatusView,
    items: Vec<WorkItemCardView>,
    has_more: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveWorkItemRequest {
    expected_version: i64,
    target_status_id: Uuid,
    target_position: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCardView {
    id: Uuid,
    project_id: Uuid,
    key: String,
    number: i64,
    title: String,
    item_type: WorkItemTypeView,
    status: WorkItemStatusView,
    priority: String,
    assignee: Option<AssigneeView>,
    version: i64,
    archived_at: Option<String>,
    updated_at: String,
    capabilities: WorkItemCapabilities,
}

impl From<WorkItemView> for WorkItemCardView {
    fn from(item: WorkItemView) -> Self {
        Self {
            id: item.id,
            project_id: item.project_id,
            key: item.key,
            number: item.number,
            title: item.title,
            item_type: item.item_type,
            status: item.status,
            priority: item.priority,
            assignee: item.assignee,
            version: item.version,
            archived_at: item.archived_at,
            updated_at: item.updated_at,
            capabilities: item.capabilities,
        }
    }
}

#[derive(Debug, Serialize)]
struct WorkItemConflictEnvelope {
    error: WorkItemConflictBody,
}

#[derive(Debug, Serialize)]
struct WorkItemConflictBody {
    code: &'static str,
    message: &'static str,
    details: WorkItemConflictDetails,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemConflictDetails {
    resource: &'static str,
    current_version: i64,
    current: WorkItemView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowResponse {
    types: Vec<WorkflowTypeView>,
    statuses: Vec<WorkflowStatusDefinitionView>,
    transitions: Vec<WorkflowTransitionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowTypeView {
    id: Uuid,
    name: String,
    position: i32,
    is_default: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowStatusDefinitionView {
    id: Uuid,
    name: String,
    semantic_category: String,
    position: i32,
    is_default: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowTransitionView {
    from_status_id: Uuid,
    to_status_id: Uuid,
}

#[derive(Debug, FromRow)]
struct WorkflowTransitionRow {
    from_status_id: Uuid,
    to_status_id: Uuid,
}

pub async fn provision_missing_default_workflows(pool: &PgPool) -> Result<u64, AppError> {
    let project_ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects ORDER BY id")
        .fetch_all(pool)
        .await?;
    let mut provisioned = 0;
    for project_id in project_ids {
        let mut transaction = pool.begin().await?;
        if ensure_default_workflow(&mut transaction, project_id).await? {
            provisioned += 1;
        }
        transaction.commit().await?;
    }
    Ok(provisioned)
}

pub async fn ensure_default_workflow(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<bool, AppError> {
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("project"))?;

    let types = sqlx::query_as::<_, WorkflowTypeRow>(
        "SELECT id, name, position, is_default, enabled FROM work_item_types WHERE project_id = $1 ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let statuses = sqlx::query_as::<_, WorkflowStatusRow>(
        "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let transition_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_status_transitions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;

    if types.is_empty() && statuses.is_empty() && transition_count == 0 {
        let mut status_ids = std::collections::BTreeMap::new();
        for (name, position, is_default) in DEFAULT_TYPES {
            sqlx::query(
                "INSERT INTO work_item_types (id, project_id, name, position, is_default) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(project_id)
            .bind(name)
            .bind(position)
            .bind(is_default)
            .execute(&mut **transaction)
            .await?;
        }
        for (name, category, position, is_default) in DEFAULT_STATUSES {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO work_item_statuses (id, project_id, name, semantic_category, position, is_default) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(project_id)
            .bind(name)
            .bind(category)
            .bind(position)
            .bind(is_default)
            .execute(&mut **transaction)
            .await?;
            status_ids.insert(name, id);
        }
        for (from, to) in DEFAULT_TRANSITIONS {
            sqlx::query(
                "INSERT INTO work_item_status_transitions (project_id, from_status_id, to_status_id) VALUES ($1, $2, $3)",
            )
            .bind(project_id)
            .bind(status_ids[from])
            .bind(status_ids[to])
            .execute(&mut **transaction)
            .await?;
        }
        return Ok(true);
    }

    let types_match = types.len() == DEFAULT_TYPES.len()
        && types.iter().zip(DEFAULT_TYPES).all(|(row, expected)| {
            row.name == expected.0
                && row.position == expected.1
                && row.is_default == expected.2
                && row.enabled
        });
    let statuses_match = statuses.len() == DEFAULT_STATUSES.len()
        && statuses
            .iter()
            .zip(DEFAULT_STATUSES)
            .all(|(row, expected)| {
                row.name == expected.0
                    && row.semantic_category == expected.1
                    && row.position == expected.2
                    && row.is_default == expected.3
                    && row.enabled
            });
    if !types_match || !statuses_match || transition_count != DEFAULT_TRANSITIONS.len() as i64 {
        return Err(AppError::Conflict(
            "work_item_workflow",
            "existing workflow does not match the M4 default",
        ));
    }
    let transition_names = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT source.name, target.name
        FROM work_item_status_transitions transition
        JOIN work_item_statuses source
          ON source.project_id = transition.project_id
         AND source.id = transition.from_status_id
        JOIN work_item_statuses target
          ON target.project_id = transition.project_id
         AND target.id = transition.to_status_id
        WHERE transition.project_id = $1
        ORDER BY source.name, target.name
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut expected = DEFAULT_TRANSITIONS
        .into_iter()
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
        .collect::<Vec<_>>();
    expected.sort();
    if transition_names != expected {
        return Err(AppError::Conflict(
            "work_item_workflow",
            "existing transitions do not match the M4 default",
        ));
    }
    Ok(false)
}

async fn get_workflow(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<WorkflowResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let types = sqlx::query_as::<_, WorkflowTypeRow>(
        "SELECT id, name, position, is_default, enabled FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    let statuses = sqlx::query_as::<_, WorkflowStatusRow>(
        "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    let transitions = sqlx::query_as::<_, WorkflowTransitionRow>(
        "SELECT from_status_id, to_status_id FROM work_item_status_transitions WHERE project_id = $1 ORDER BY from_status_id, to_status_id",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(WorkflowResponse {
        types: types
            .into_iter()
            .map(|row| WorkflowTypeView {
                id: row.id,
                name: row.name,
                position: row.position,
                is_default: row.is_default,
            })
            .collect(),
        statuses: statuses
            .into_iter()
            .map(|row| WorkflowStatusDefinitionView {
                id: row.id,
                name: row.name,
                semantic_category: row.semantic_category,
                position: row.position,
                is_default: row.is_default,
            })
            .collect(),
        transitions: transitions
            .into_iter()
            .map(|row| WorkflowTransitionView {
                from_status_id: row.from_status_id,
                to_status_id: row.to_status_id,
            })
            .collect(),
    }))
}

async fn list_work_items(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<WorkItemListQuery>,
) -> Result<Json<WorkItemListResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let search = normalize_search(query.q.as_deref())?;
    validate_priority_filter(query.priority.as_deref())?;
    validate_semantic_category(query.semantic_category.as_deref())?;
    if query.unassigned == Some(true) && query.assignee_actor_id.is_some() {
        return Err(AppError::Validation("assignee filter"));
    }
    validate_list_filter_ids(
        &state.pool,
        project_id,
        query.type_id,
        query.status_id,
        query.assignee_actor_id,
    )
    .await?;
    let before_number = query
        .cursor
        .as_deref()
        .map(|value| decode_cursor(value, project_id))
        .transpose()?;
    let limit = i64::from(
        query
            .limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    );
    let rows = sqlx::query_as::<_, WorkItemRow>(
        r#"
        SELECT
            item.id, item.project_id, project.project_key,
            item.number, item.title, ''::text AS description,
            item.type_id, item_type.name AS type_name,
            item.status_id, status.name AS status_name,
            status.semantic_category, item.priority,
            item.assignee_actor_id,
            assignee.display_name AS assignee_display_name,
            assignee.handle AS assignee_handle,
            assignee_actor.kind AS assignee_kind,
            CASE WHEN assignee.actor_id IS NULL THEN NULL
                 ELSE assignee.enabled AND assignee_actor.status = 'active'
            END AS assignee_active,
            item.version, item.archived_at::text AS archived_at,
            item.created_at::text AS created_at,
            item.updated_at::text AS updated_at,
            '[]'::jsonb AS acceptance_criteria,
            '[]'::jsonb AS linked_threads,
            '[]'::jsonb AS linked_repositories,
            '[]'::jsonb AS linked_pull_requests
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_types item_type
          ON item_type.project_id = item.project_id AND item_type.id = item.type_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id
         AND assignee.actor_id = item.assignee_actor_id
        LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
        WHERE item.project_id = $1
          AND ($2::bigint IS NULL OR item.number < $2)
          AND ($3::text IS NULL OR item.title ILIKE $3 OR (project.project_key || '-' || item.number::text) ILIKE $3)
          AND ($4::uuid IS NULL OR item.type_id = $4)
          AND ($5::uuid IS NULL OR item.status_id = $5)
          AND ($6::text IS NULL OR status.semantic_category = $6)
          AND ($7::text IS NULL OR item.priority = $7)
          AND ($8::uuid IS NULL OR item.assignee_actor_id = $8)
          AND (NOT $9::boolean OR item.assignee_actor_id IS NULL)
          AND (($10::boolean AND item.archived_at IS NOT NULL) OR (NOT $10::boolean AND item.archived_at IS NULL))
        ORDER BY item.number DESC
        LIMIT $11
        "#,
    )
    .bind(project_id)
    .bind(before_number)
    .bind(search.map(|value| format!("%{value}%")))
    .bind(query.type_id)
    .bind(query.status_id)
    .bind(query.semantic_category)
    .bind(query.priority)
    .bind(query.assignee_actor_id)
    .bind(query.unassigned.unwrap_or(false))
    .bind(query.archived.unwrap_or(false))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() > limit as usize;
    let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let next_cursor = if has_more {
        rows.last()
            .map(|row| encode_cursor(project_id, row.number))
            .transpose()?
    } else {
        None
    };
    let capabilities = capabilities(&membership)?;
    let items = rows
        .into_iter()
        .map(|row| work_item_view(row, capabilities).map(WorkItemCardView::from))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(WorkItemListResponse { items, next_cursor }))
}

async fn get_work_item_board(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<WorkItemListQuery>,
) -> Result<Json<WorkItemBoardResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let search = normalize_search(query.q.as_deref())?;
    validate_priority_filter(query.priority.as_deref())?;
    validate_semantic_category(query.semantic_category.as_deref())?;
    if query.cursor.is_some() {
        return Err(AppError::Invalid("board cursor"));
    }
    if query.unassigned == Some(true) && query.assignee_actor_id.is_some() {
        return Err(AppError::Validation("assignee filter"));
    }
    validate_list_filter_ids(
        &state.pool,
        project_id,
        query.type_id,
        query.status_id,
        query.assignee_actor_id,
    )
    .await?;
    let statuses = sqlx::query_as::<_, WorkflowStatusRow>(
        "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    let limit = i64::from(
        query
            .limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    );
    let caps = capabilities(&membership)?;
    let mut columns = Vec::with_capacity(statuses.len());
    for status in statuses {
        if query
            .status_id
            .is_some_and(|status_id| status_id != status.id)
        {
            continue;
        }
        let rows = sqlx::query_as::<_, WorkItemRow>(
            r#"
            SELECT
                item.id, item.project_id, project.project_key,
                item.number, item.title, ''::text AS description,
                item.type_id, item_type.name AS type_name,
                item.status_id, status.name AS status_name,
                status.semantic_category, item.priority,
                item.assignee_actor_id,
                assignee.display_name AS assignee_display_name,
                assignee.handle AS assignee_handle,
                assignee_actor.kind AS assignee_kind,
                CASE WHEN assignee.actor_id IS NULL THEN NULL
                     ELSE assignee.enabled AND assignee_actor.status = 'active'
                END AS assignee_active,
                item.version, item.archived_at::text AS archived_at,
                item.created_at::text AS created_at,
                item.updated_at::text AS updated_at,
                '[]'::jsonb AS acceptance_criteria,
                '[]'::jsonb AS linked_threads,
                '[]'::jsonb AS linked_repositories,
                '[]'::jsonb AS linked_pull_requests
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_types item_type
              ON item_type.project_id = item.project_id AND item_type.id = item.type_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
            LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
            WHERE item.project_id = $1 AND item.status_id = $2
              AND ($3::text IS NULL OR item.title ILIKE $3 OR (project.project_key || '-' || item.number::text) ILIKE $3)
              AND ($4::uuid IS NULL OR item.type_id = $4)
              AND ($5::text IS NULL OR status.semantic_category = $5)
              AND ($6::text IS NULL OR item.priority = $6)
              AND ($7::uuid IS NULL OR item.assignee_actor_id = $7)
              AND (NOT $8::boolean OR item.assignee_actor_id IS NULL)
              AND (($9::boolean AND item.archived_at IS NOT NULL) OR (NOT $9::boolean AND item.archived_at IS NULL))
            ORDER BY item.board_rank, item.id
            LIMIT $10
            "#,
        )
        .bind(project_id)
        .bind(status.id)
        .bind(search.as_ref().map(|value| format!("%{value}%")))
        .bind(query.type_id)
        .bind(query.semantic_category.as_deref())
        .bind(query.priority.as_deref())
        .bind(query.assignee_actor_id)
        .bind(query.unassigned.unwrap_or(false))
        .bind(query.archived.unwrap_or(false))
        .bind(limit + 1)
        .fetch_all(&state.pool)
        .await?;
        let has_more = rows.len() > limit as usize;
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| work_item_view(row, caps).map(WorkItemCardView::from))
            .collect::<Result<Vec<_>, _>>()?;
        columns.push(WorkItemBoardColumn {
            status: WorkItemStatusView {
                id: status.id,
                name: status.name,
                semantic_category: status.semantic_category,
            },
            items,
            has_more,
        });
    }
    Ok(Json(WorkItemBoardResponse { columns }))
}

async fn validate_list_filter_ids(
    pool: &PgPool,
    project_id: Uuid,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    assignee_actor_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(type_id) = type_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_types WHERE project_id = $1 AND id = $2)",
        )
        .bind(project_id)
        .bind(type_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("typeId"));
        }
    }
    if let Some(status_id) = status_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_statuses WHERE project_id = $1 AND id = $2)",
        )
        .bind(project_id)
        .bind(status_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("statusId"));
        }
    }
    if let Some(assignee_actor_id) = assignee_actor_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2)",
        )
        .bind(project_id)
        .bind(assignee_actor_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("assigneeActorId"));
        }
    }
    Ok(())
}

async fn get_work_item(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<WorkItemView>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(work_item_view(row, capabilities(&membership)?)?))
}

async fn get_work_item_by_key(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, key)): ApiPath<(Uuid, String)>,
) -> Result<Json<WorkItemView>, AppError> {
    validate_readable_key(&key)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let work_item_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT item.id
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1
          AND project.project_key || '-' || item.number::text = $2
        "#,
    )
    .bind(project_id)
    .bind(key)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(work_item_view(row, capabilities(&membership)?)?))
}

async fn update_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<UpdateWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1
        || (request.title.is_none()
            && request.description.is_none()
            && request.type_id.is_none()
            && request.status_id.is_none()
            && request.priority.is_none()
            && request.assignee_actor_id.is_none())
    {
        return Err(AppError::Validation("Work Item update"));
    }
    request.title = request.title.as_deref().map(normalize_title).transpose()?;
    request.description = request
        .description
        .as_deref()
        .map(normalize_description)
        .transpose()?;
    if let Some(priority) = request.priority.as_deref() {
        validate_priority(priority)?;
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "update")?;
    let request_hash = RequestHash::for_value(&serde_json::json!({
        "expectedVersion": request.expected_version,
        "title": request.title,
        "description": request.description,
        "typeId": request.type_id,
        "statusId": request.status_id,
        "priority": request.priority,
        "assigneeSpecified": request.assignee_actor_id.is_some(),
        "assigneeActorId": request.assignee_actor_id.flatten(),
    }))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current_row = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let capabilities = capabilities(&membership)?;
    if current_row.version != request.expected_version {
        let current = work_item_view(current_row, capabilities)?;
        return work_item_version_conflict_response(current);
    }
    if current_row.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }

    let type_id = match request.type_id {
        Some(type_id) => {
            select_definition(
                &mut transaction,
                project_id,
                "work_item_types",
                Some(type_id),
            )
            .await?
        }
        None => current_row.type_id,
    };
    let status_id = match request.status_id {
        Some(status_id) => {
            select_definition(
                &mut transaction,
                project_id,
                "work_item_statuses",
                Some(status_id),
            )
            .await?
        }
        None => current_row.status_id,
    };
    if status_id != current_row.status_id {
        require_status_transition(
            &mut transaction,
            project_id,
            current_row.status_id,
            status_id,
        )
        .await?;
    }
    if let Some(assignee_actor_id) = request.assignee_actor_id {
        validate_assignee(&mut transaction, project_id, assignee_actor_id).await?;
    }
    let title = request.title.unwrap_or_else(|| current_row.title.clone());
    let description = request
        .description
        .unwrap_or_else(|| current_row.description.clone());
    let priority = request
        .priority
        .unwrap_or_else(|| current_row.priority.clone());
    let assignee_actor_id = request
        .assignee_actor_id
        .unwrap_or(current_row.assignee_actor_id);
    let mut changed_fields = Vec::new();
    if title != current_row.title {
        changed_fields.push("title");
    }
    if description != current_row.description {
        changed_fields.push("description");
    }
    if type_id != current_row.type_id {
        changed_fields.push("type");
    }
    if status_id != current_row.status_id {
        changed_fields.push("status");
    }
    if priority != current_row.priority {
        changed_fields.push("priority");
    }
    if assignee_actor_id != current_row.assignee_actor_id {
        changed_fields.push("assignee");
    }
    if changed_fields.is_empty() {
        let item = work_item_view(current_row, capabilities)?;
        complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
        transaction.commit().await?;
        return work_item_response(StatusCode::OK, item);
    }

    let updated = sqlx::query(
        r#"
        UPDATE work_items
        SET title = $3, description = $4, type_id = $5, status_id = $6,
            priority = $7, assignee_actor_id = $8,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $9
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(title)
    .bind(description)
    .bind(type_id)
    .bind(status_id)
    .bind(priority)
    .bind(assignee_actor_id)
    .bind(request.expected_version)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal(
            "locked Work Item update lost its version",
        ));
    }
    let version = request
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        "work_item_updated",
        "Work Item updated",
        "work_item.updated",
        serde_json::json!({"changedFields": changed_fields}),
        version,
    )
    .await?;
    let row = fetch_work_item_in_transaction(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::Internal("updated Work Item is unavailable"))?;
    let item = work_item_view(row, capabilities)?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn archive_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    set_work_item_archived(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        request,
        true,
    )
    .await
}

async fn restore_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    set_work_item_archived(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        request,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn set_work_item_archived(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    request: WorkItemVersionRequest,
    archived: bool,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let idempotency_key = IdempotencyKey::from_headers(headers)?;
    let action = if archived { "archive" } else { "restore" };
    let scope = resource_bound_scope("work_items", work_item_id, action)?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current_row = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let capabilities = capabilities(&membership)?;
    let already_in_target_state = current_row.archived_at.is_some() == archived;
    if already_in_target_state {
        let item = work_item_view(current_row, capabilities)?;
        complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
        transaction.commit().await?;
        return work_item_response(StatusCode::OK, item);
    }
    if current_row.version != request.expected_version {
        let current = work_item_view(current_row, capabilities)?;
        return work_item_version_conflict_response(current);
    }
    let updated = if archived {
        sqlx::query(
            r#"
            UPDATE work_items
            SET archived_at = now(), archived_by_actor_id = $3,
                version = version + 1, updated_at = now()
            WHERE project_id = $1 AND id = $2 AND version = $4
            "#,
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(principal.actor_id)
        .bind(request.expected_version)
        .execute(&mut *transaction)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE work_items
            SET archived_at = NULL, archived_by_actor_id = NULL,
                version = version + 1, updated_at = now()
            WHERE project_id = $1 AND id = $2 AND version = $3
            "#,
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(request.expected_version)
        .execute(&mut *transaction)
        .await?
    };
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal(
            "locked Work Item archive lost its version",
        ));
    }
    let version = request
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    let (kind, summary, audit_action) = if archived {
        (
            "work_item_archived",
            "Work Item archived",
            "work_item.archived",
        )
    } else {
        (
            "work_item_restored",
            "Work Item restored",
            "work_item.restored",
        )
    };
    record_work_item_change(
        &mut transaction,
        headers,
        principal,
        project_id,
        work_item_id,
        kind,
        summary,
        audit_action,
        serde_json::json!({"previousVersion": request.expected_version}),
        version,
    )
    .await?;
    let row = fetch_work_item_in_transaction(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::Internal("archived Work Item is unavailable"))?;
    let item = work_item_view(row, capabilities)?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn link_work_item_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, conversation_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyLinkRequest>,
) -> Result<Response, AppError> {
    change_work_item_thread_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        conversation_id,
        request,
        true,
    )
    .await
}

async fn work_with_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<WorkWithWorkerRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.prompt = normalize_worker_prompt(&request.prompt)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkerInvoke,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "{}:work_item_worker_invocation:{}:{}",
            principal.actor_id,
            work_item_id,
            idempotency_key.as_str()
        ))
        .execute(&mut *transaction)
        .await?;
    let existing = sqlx::query_as::<_, WorkerInvocationRow>(
        r#"
        SELECT conversation_id, repository_id, request_hash, response_body
        FROM work_item_worker_invocations
        WHERE requesting_actor_id = $1 AND work_item_id = $2 AND idempotency_key = $3
        FOR UPDATE
        "#,
    )
    .bind(principal.actor_id)
    .bind(work_item_id)
    .bind(idempotency_key.as_str())
    .fetch_optional(&mut *transaction)
    .await?;

    let (thread_id, repository_id, work_item_key) = if let Some(existing) = existing {
        if existing.request_hash.as_slice() != request_hash.as_bytes() {
            return Err(AppError::Conflict("idempotency_key", "payload mismatch"));
        }
        if let Some(response_body) = existing.response_body {
            let response: WorkWithWorkerResponse = serde_json::from_value(response_body)
                .map_err(|_| AppError::Internal("decode Work with Worker response"))?;
            transaction.commit().await?;
            return Ok((StatusCode::ACCEPTED, Json(response)).into_response());
        }
        let key = sqlx::query_scalar::<_, String>(
            "SELECT project.project_key || '-' || item.number::text FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.project_id = $1 AND item.id = $2",
        )
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
        transaction.commit().await?;
        (existing.conversation_id, existing.repository_id, key)
    } else {
        let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
            .await?
            .ok_or(AppError::NotFound("work_item"))?;
        if current.version != request.expected_version {
            let current = work_item_view(current, capabilities(&membership)?)?;
            return work_item_version_conflict_response(current);
        }
        if current.archived_at.is_some() {
            return Err(AppError::Conflict("work_item", "archived"));
        }
        let channel_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations
                WHERE project_id = $1 AND id = $2 AND kind = 'channel' AND archived_at IS NULL
            )
            "#,
        )
        .bind(project_id)
        .bind(request.channel_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !channel_exists {
            return Err(AppError::NotFound("channel"));
        }

        let thread_id = Uuid::now_v7();
        let work_item_key = format!("{}-{}", current.project_key, current.number);
        let thread_name = worker_thread_name(&work_item_key, &current.title, thread_id);
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
            VALUES ($1, $2, 'thread', $3, $4)
            "#,
        )
        .bind(thread_id)
        .bind(project_id)
        .bind(request.channel_id)
        .bind(&thread_name)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversation_read_states (
                project_id, conversation_id, actor_id, highest_seen_sequence
            ) VALUES ($1, $2, $3, 0)
            "#,
        )
        .bind(project_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO work_item_conversation_links (
                project_id, work_item_id, conversation_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;

        let setup = crate::api::select_run_setup(
            &mut transaction,
            project_id,
            thread_id,
            principal.actor_id,
            request.repository_id,
        )
        .await?;
        let version =
            advance_work_item_version(&mut transaction, project_id, work_item_id, current.version)
                .await?;
        enqueue_typed(
            &mut transaction,
            project_id,
            OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
                conversation_id: thread_id,
                kind: OutboxConversationKind::Thread,
                version: 1,
            }),
        )
        .await?;
        record_work_item_change(
            &mut transaction,
            &headers,
            &principal,
            project_id,
            work_item_id,
            "worker_invoked",
            "Work with Worker started",
            "work_item.worker_invoked",
            serde_json::json!({
                "conversationId": thread_id,
                "repositoryId": setup.repository_id,
            }),
            version,
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO work_item_worker_invocations (
                id, project_id, work_item_id, conversation_id, requesting_actor_id,
                idempotency_key, request_hash, repository_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(work_item_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .bind(idempotency_key.as_str())
        .bind(request_hash.as_bytes().as_slice())
        .bind(setup.repository_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        (thread_id, setup.repository_id, work_item_key)
    };

    let provider_prompt = format!(
        "Work Item {work_item_key} ({work_item_id}).\n\nUser instructions:\n{}",
        request.prompt
    );
    let (_, Json(run)) = crate::api::start_agent_run(
        State(state.clone()),
        headers,
        principal.clone(),
        ApiPath(thread_id),
        ApiJson(crate::api::StartRunRequest {
            prompt: provider_prompt,
            repository_id: Some(repository_id),
        }),
    )
    .await?;
    let response = WorkWithWorkerResponse { thread_id, run };
    let response_body = serde_json::to_value(&response)
        .map_err(|_| AppError::Internal("serialize Work with Worker response"))?;
    let mut transaction = state.pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE work_item_worker_invocations
        SET agent_run_id = $4, response_body = $5, updated_at = now()
        WHERE requesting_actor_id = $1 AND work_item_id = $2 AND idempotency_key = $3
          AND request_hash = $6
          AND (agent_run_id IS NULL OR agent_run_id = $4)
        "#,
    )
    .bind(principal.actor_id)
    .bind(work_item_id)
    .bind(idempotency_key.as_str())
    .bind(response.run.run_id)
    .bind(response_body)
    .bind(request_hash.as_bytes().as_slice())
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "work_item_worker_invocation",
            "completion raced with a different run",
        ));
    }
    transaction.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(response)).into_response())
}

fn normalize_worker_prompt(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 18_000
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("worker prompt"));
    }
    Ok(value.to_owned())
}

fn worker_thread_name(key: &str, title: &str, thread_id: Uuid) -> String {
    let suffix = &thread_id.simple().to_string()[24..];
    let prefix = format!("{key}: ");
    let suffix = format!(" [{suffix}]");
    let available = 120usize.saturating_sub(prefix.chars().count() + suffix.chars().count());
    let title = title.chars().take(available).collect::<String>();
    format!("{prefix}{title}{suffix}")
}

async fn unlink_work_item_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, conversation_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyLinkRequest>,
) -> Result<Response, AppError> {
    change_work_item_thread_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        conversation_id,
        request,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn change_work_item_thread_link(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    conversation_id: Uuid,
    request: EmptyLinkRequest,
    linked: bool,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let scope = resource_bound_scope(
        "work_items",
        work_item_id,
        if linked {
            "thread_link"
        } else {
            "thread_unlink"
        },
    )?;
    let request_hash = RequestHash::for_value(&(conversation_id, request))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let thread_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM conversations WHERE project_id = $1 AND id = $2 AND kind = 'thread')",
    ).bind(project_id).bind(conversation_id).fetch_one(&mut *transaction).await?;
    if !thread_exists {
        return Err(AppError::NotFound("conversation"));
    }
    let changed = if linked {
        sqlx::query(r#"
            INSERT INTO work_item_conversation_links (project_id, work_item_id, conversation_id, created_by_actor_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (work_item_id, conversation_id) DO NOTHING
        "#).bind(project_id).bind(work_item_id).bind(conversation_id).bind(principal.actor_id)
            .execute(&mut *transaction).await?.rows_affected() == 1
    } else {
        sqlx::query("DELETE FROM work_item_conversation_links WHERE project_id = $1 AND work_item_id = $2 AND conversation_id = $3")
            .bind(project_id).bind(work_item_id).bind(conversation_id)
            .execute(&mut *transaction).await?.rows_affected() == 1
    };
    if changed {
        let version =
            advance_work_item_version(&mut transaction, project_id, work_item_id, current.version)
                .await?;
        let (kind, summary, audit_action) = if linked {
            ("thread_linked", "Thread linked", "work_item.thread_linked")
        } else {
            (
                "thread_unlinked",
                "Thread unlinked",
                "work_item.thread_unlinked",
            )
        };
        record_work_item_change(
            &mut transaction,
            headers,
            principal,
            project_id,
            work_item_id,
            kind,
            summary,
            audit_action,
            serde_json::json!({"conversationId": conversation_id}),
            version,
        )
        .await?;
    }
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        capabilities(&membership)?,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn link_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<LinkRepositoryRequest>,
) -> Result<Response, AppError> {
    if !matches!(request.role.as_str(), "primary" | "related") {
        return Err(AppError::Validation("repository role"));
    }
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        Some(request.role),
        "repository_link",
    )
    .await
}

async fn unlink_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        None,
        "repository_unlink",
    )
    .await
}

async fn set_primary_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        Some("primary".to_owned()),
        "repository_primary",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn change_work_item_repository_link(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    repository_id: Uuid,
    expected_version: i64,
    desired_role: Option<String>,
    action: &'static str,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, action)?;
    let request_hash = RequestHash::for_value(&(repository_id, expected_version, &desired_role))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let caps = capabilities(&membership)?;
    if current.version != expected_version {
        return work_item_version_conflict_response(work_item_view(current, caps)?);
    }
    let repository = sqlx::query_as::<_, (bool, bool, String, Option<String>)>(
        r#"
        SELECT enabled, trusted, preparation_state, archived_at::text
        FROM repositories WHERE project_id = $1 AND id = $2
    "#,
    )
    .bind(project_id)
    .bind(repository_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("repository"))?;
    if desired_role.as_deref() == Some("primary")
        && (!repository.0 || !repository.1 || repository.2 != "ready" || repository.3.is_some())
    {
        return Err(AppError::Conflict(
            "repository",
            "the primary repository must be enabled, trusted, ready, and active",
        ));
    }
    let existing_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM work_item_repository_links WHERE project_id = $1 AND work_item_id = $2 AND repository_id = $3",
    ).bind(project_id).bind(work_item_id).bind(repository_id).fetch_optional(&mut *transaction).await?;
    let changed = match desired_role.as_deref() {
        None => sqlx::query("DELETE FROM work_item_repository_links WHERE project_id = $1 AND work_item_id = $2 AND repository_id = $3")
            .bind(project_id).bind(work_item_id).bind(repository_id)
            .execute(&mut *transaction).await?.rows_affected() == 1,
        Some(role) if existing_role.as_deref() == Some(role) => false,
        Some(role) => {
            if role == "primary" {
                sqlx::query("UPDATE work_item_repository_links SET role = 'related', updated_at = now() WHERE project_id = $1 AND work_item_id = $2 AND role = 'primary'")
                    .bind(project_id).bind(work_item_id).execute(&mut *transaction).await?;
            }
            sqlx::query(r#"
                INSERT INTO work_item_repository_links (project_id, work_item_id, repository_id, role, created_by_actor_id)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (work_item_id, repository_id) DO UPDATE SET role = EXCLUDED.role, updated_at = now()
            "#).bind(project_id).bind(work_item_id).bind(repository_id).bind(role).bind(principal.actor_id)
                .execute(&mut *transaction).await?;
            true
        }
    };
    if changed {
        let version =
            advance_work_item_version(&mut transaction, project_id, work_item_id, expected_version)
                .await?;
        let (kind, summary, audit_action) = match desired_role.as_deref() {
            None => (
                "repository_unlinked",
                "Repository unlinked",
                "work_item.repository_unlinked",
            ),
            Some("primary") if existing_role.is_some() => (
                "repository_primary_changed",
                "Primary repository changed",
                "work_item.repository_primary_changed",
            ),
            _ => (
                "repository_linked",
                "Repository linked",
                "work_item.repository_linked",
            ),
        };
        record_work_item_change(
            &mut transaction,
            headers,
            principal,
            project_id,
            work_item_id,
            kind,
            summary,
            audit_action,
            serde_json::json!({"repositoryId": repository_id, "role": desired_role}),
            version,
        )
        .await?;
    }
    let item = current_work_item_view(&mut transaction, project_id, work_item_id, caps).await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn search_assignable_actors(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<AssigneeSearchQuery>,
) -> Result<Json<AssigneeSearchResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::NotFound("project"));
    }
    let search = normalize_search(query.q.as_deref())?
        .unwrap_or_default()
        .to_lowercase();
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 50));
    let rows = sqlx::query_as::<_, AssigneeSearchRow>(
        r#"
        SELECT membership.actor_id, membership.display_name, membership.handle, actor.kind
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
          AND (
              $2 = ''
              OR strpos(lower(membership.display_name), $2) > 0
              OR strpos(lower(membership.handle), $2) > 0
          )
        ORDER BY lower(membership.display_name), lower(membership.handle), actor.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(search)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(AssigneeSearchResponse {
        actors: rows
            .into_iter()
            .map(|row| ActorView {
                id: row.actor_id,
                display_name: row.display_name,
                handle: row.handle,
                kind: row.kind,
                avatar_url: None,
            })
            .collect(),
    }))
}

async fn search_work_item_references(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<WorkItemReferenceSearchQuery>,
) -> Result<Json<WorkItemReferenceSearchResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let search = normalize_search(query.q.as_deref())?.unwrap_or_default();
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 50));
    let rows = sqlx::query_as::<_, WorkItemReferenceRow>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key,
               item.title, status.semantic_category AS status_category,
               item.archived_at::text AS archived_at
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1
          AND (
              $2 = '' OR item.title ILIKE '%' || $2 || '%'
              OR project.project_key || '-' || item.number::text ILIKE '%' || $2 || '%'
          )
        ORDER BY item.archived_at NULLS FIRST, item.number DESC, item.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(search)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(WorkItemReferenceSearchResponse {
        items: rows
            .into_iter()
            .map(|row| work_item_reference_view(project_id, row))
            .collect(),
    }))
}

async fn move_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<MoveWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.target_position < 1 || request.target_position > 10_000 {
        return Err(AppError::Validation("targetPosition"));
    }
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "move")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let caps = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, caps)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::NotFound("work_item"));
    }
    let target_status = sqlx::query_as::<_, WorkflowStatusRow>(
        "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 AND id = $2 AND enabled",
    )
    .bind(project_id)
    .bind(request.target_status_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Validation("targetStatusId"))?;
    if current.status_id != request.target_status_id {
        let allowed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_status_transitions WHERE project_id = $1 AND from_status_id = $2 AND to_status_id = $3)",
        )
        .bind(project_id)
        .bind(current.status_id)
        .bind(request.target_status_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !allowed {
            return Err(AppError::Conflict("work_item_status", "transition"));
        }
    }
    let mut target_rows = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT id, board_rank FROM work_items
        WHERE project_id = $1 AND status_id = $2 AND id <> $3 AND archived_at IS NULL
        ORDER BY board_rank, id
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(request.target_status_id)
    .bind(work_item_id)
    .fetch_all(&mut *transaction)
    .await?;
    let target_index = usize::try_from(request.target_position - 1)
        .map_err(|_| AppError::Validation("targetPosition"))?;
    if target_index > target_rows.len() {
        return Err(AppError::Validation("targetPosition"));
    }
    let mut rank = allocate_board_rank(&target_rows, target_index)?;
    if rank.is_none() {
        for (index, (id, existing_rank)) in target_rows.iter_mut().enumerate() {
            let balanced_rank = i64::try_from(index + 1)
                .ok()
                .and_then(|value| value.checked_mul(BOARD_RANK_STEP))
                .ok_or(AppError::Internal("rebalance Work Item board rank"))?;
            if *existing_rank != balanced_rank {
                sqlx::query(
                    "UPDATE work_items SET board_rank = $3 WHERE project_id = $1 AND id = $2",
                )
                .bind(project_id)
                .bind(*id)
                .bind(balanced_rank)
                .execute(&mut *transaction)
                .await?;
                *existing_rank = balanced_rank;
            }
        }
        rank = allocate_board_rank(&target_rows, target_index)?;
    }
    let board_rank = rank.ok_or(AppError::Internal("allocate Work Item board rank"))?;
    let updated = sqlx::query(
        r#"
        UPDATE work_items
        SET status_id = $3, board_rank = $4, version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $5
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(request.target_status_id)
    .bind(board_rank)
    .bind(request.expected_version)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("locked Work Item move lost its version"));
    }
    let version = request
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        "work_item_updated",
        "Work Item moved",
        "work_item.moved",
        serde_json::json!({
            "previousStatusId": current.status_id,
            "statusId": target_status.id,
            "targetPosition": request.target_position,
        }),
        version,
    )
    .await?;
    let item = current_work_item_view(&mut transaction, project_id, work_item_id, caps).await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

fn allocate_board_rank(rows: &[(Uuid, i64)], index: usize) -> Result<Option<i64>, AppError> {
    let previous = index
        .checked_sub(1)
        .and_then(|previous| rows.get(previous))
        .map(|row| row.1);
    let next = rows.get(index).map(|row| row.1);
    match (previous, next) {
        (None, None) => Ok(Some(BOARD_RANK_STEP)),
        (Some(previous), None) => previous
            .checked_add(BOARD_RANK_STEP)
            .map(Some)
            .ok_or(AppError::Internal("append Work Item board rank")),
        (None, Some(next)) if next > 1 => Ok(Some(next / 2)),
        (None, Some(_)) => Ok(None),
        (Some(previous), Some(next)) if next - previous > 1 => {
            Ok(Some(previous + (next - previous) / 2))
        }
        (Some(_), Some(_)) => Ok(None),
    }
}

async fn create_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(mut request): ApiJson<CreateWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.title = normalize_title(&request.title)?;
    request.description = normalize_description(&request.description)?;
    validate_priority(&request.priority)?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("projects", project_id, "work_items_create")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemCreate, None)?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            work_item_response(replay.status(), item)
        }
        IdempotencyOutcome::Reserved(reservation) => {
            ensure_default_workflow(&mut transaction, project_id).await?;
            let type_id = select_definition(
                &mut transaction,
                project_id,
                "work_item_types",
                request.type_id,
            )
            .await?;
            let status_id = select_definition(
                &mut transaction,
                project_id,
                "work_item_statuses",
                request.status_id,
            )
            .await?;
            validate_assignee(&mut transaction, project_id, request.assignee_actor_id).await?;
            let number = sqlx::query_scalar::<_, i64>(
                r#"
                UPDATE project_counters
                SET next_work_item_number = next_work_item_number + 1,
                    updated_at = now()
                WHERE project_id = $1
                RETURNING next_work_item_number - 1
                "#,
            )
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
            let board_rank = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(max(board_rank), 0) + $3 FROM work_items WHERE project_id = $1 AND status_id = $2",
            )
            .bind(project_id)
            .bind(status_id)
            .bind(BOARD_RANK_STEP)
            .fetch_one(&mut *transaction)
            .await?;
            let work_item_id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO work_items (
                    id, project_id, number, title, description, type_id,
                    status_id, priority, assignee_actor_id, created_by_actor_id,
                    board_rank
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                "#,
            )
            .bind(work_item_id)
            .bind(project_id)
            .bind(number)
            .bind(&request.title)
            .bind(&request.description)
            .bind(type_id)
            .bind(status_id)
            .bind(&request.priority)
            .bind(request.assignee_actor_id)
            .bind(principal.actor_id)
            .bind(board_rank)
            .execute(&mut *transaction)
            .await?;
            insert_work_item_activity(
                &mut transaction,
                project_id,
                work_item_id,
                principal.actor_id,
                "work_item_created",
                "Work Item created",
                &serde_json::json!({}),
            )
            .await?;
            enqueue_typed(
                &mut transaction,
                project_id,
                OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
                    work_item_id,
                    version: 1,
                }),
            )
            .await?;
            insert_audit_event(
                &mut transaction,
                &headers,
                &principal,
                project_id,
                "work_item.created",
                work_item_id,
                serde_json::json!({"number": number}),
            )
            .await?;
            let row = fetch_work_item_in_transaction(&mut transaction, project_id, work_item_id)
                .await?
                .ok_or(AppError::Internal("created Work Item is unavailable"))?;
            let item = work_item_view(row, capabilities(&membership)?)?;
            complete(&mut transaction, &reservation, StatusCode::CREATED, &item).await?;
            transaction.commit().await?;
            work_item_response(StatusCode::CREATED, item)
        }
    }
}

async fn list_acceptance_criteria(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<Vec<AcceptanceCriterionView>>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(
        work_item_view(row, capabilities(&membership)?)?.acceptance_criteria,
    ))
}

async fn create_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<CreateCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.text = normalize_criterion_text(&request.text)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "criterion_create")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let position = sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(max(position), 0) + 1 FROM work_item_acceptance_criteria WHERE project_id = $1 AND work_item_id = $2",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_one(&mut *transaction)
    .await?;
    if position > 1000 {
        return Err(AppError::Validation("acceptanceCriteria"));
    }
    let criterion_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_item_acceptance_criteria (
            id, project_id, work_item_id, text, position, created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(criterion_id)
    .bind(project_id)
    .bind(work_item_id)
    .bind(&request.text)
    .bind(position)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    let version = advance_work_item_version(
        &mut transaction,
        project_id,
        work_item_id,
        request.expected_version,
    )
    .await?;
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        "criterion_created",
        "Acceptance criterion added",
        "work_item.criterion_created",
        serde_json::json!({"criterionId": criterion_id, "position": position}),
        version,
    )
    .await?;
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::CREATED, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::CREATED, item)
}

async fn update_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, criterion_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<UpdateCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1
        || (request.text.is_none() && request.position.is_none() && request.completed.is_none())
    {
        return Err(AppError::Validation("acceptance criterion update"));
    }
    request.text = request
        .text
        .as_deref()
        .map(normalize_criterion_text)
        .transpose()?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("criteria", criterion_id, "update")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let criterion = sqlx::query_as::<_, CriterionRow>(
        r#"
        SELECT text, position, completed
        FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2 AND id = $3
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(criterion_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("acceptance_criterion"))?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_acceptance_criteria WHERE project_id = $1 AND work_item_id = $2",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_one(&mut *transaction)
    .await?;
    let target_position = request.position.unwrap_or(criterion.position);
    if target_position < 1 || i64::from(target_position) > count {
        return Err(AppError::Validation("position"));
    }
    let text = request.text.unwrap_or_else(|| criterion.text.clone());
    let completed_state = request.completed.unwrap_or(criterion.completed);
    if text == criterion.text
        && target_position == criterion.position
        && completed_state == criterion.completed
    {
        let item = work_item_view(current, item_capabilities)?;
        complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
        transaction.commit().await?;
        return work_item_response(StatusCode::OK, item);
    }
    if target_position != criterion.position {
        sqlx::query("SET CONSTRAINTS work_item_acceptance_criteria_position_unique DEFERRED")
            .execute(&mut *transaction)
            .await?;
        if target_position > criterion.position {
            sqlx::query(
                "UPDATE work_item_acceptance_criteria SET position = position - 1, updated_at = now() WHERE project_id = $1 AND work_item_id = $2 AND position > $3 AND position <= $4",
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(criterion.position)
            .bind(target_position)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE work_item_acceptance_criteria SET position = position + 1, updated_at = now() WHERE project_id = $1 AND work_item_id = $2 AND position >= $3 AND position < $4",
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(target_position)
            .bind(criterion.position)
            .execute(&mut *transaction)
            .await?;
        }
    }
    sqlx::query(
        r#"
        UPDATE work_item_acceptance_criteria
        SET text = $4, position = $5, completed = $6,
            completed_by_actor_id = CASE WHEN $6 THEN
                CASE WHEN completed THEN completed_by_actor_id ELSE $7 END
                ELSE NULL END,
            completed_at = CASE WHEN $6 THEN
                CASE WHEN completed THEN completed_at ELSE now() END
                ELSE NULL END,
            updated_at = now()
        WHERE project_id = $1 AND work_item_id = $2 AND id = $3
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(criterion_id)
    .bind(&text)
    .bind(target_position)
    .bind(completed_state)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    let version = advance_work_item_version(
        &mut transaction,
        project_id,
        work_item_id,
        request.expected_version,
    )
    .await?;
    let (kind, summary) = match (criterion.completed, completed_state) {
        (false, true) => ("criterion_completed", "Acceptance criterion completed"),
        (true, false) => ("criterion_reopened", "Acceptance criterion reopened"),
        _ => ("criterion_updated", "Acceptance criterion updated"),
    };
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        kind,
        summary,
        "work_item.criterion_updated",
        serde_json::json!({"criterionId": criterion_id}),
        version,
    )
    .await?;
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn remove_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, criterion_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<RemoveCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("criteria", criterion_id, "remove")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
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
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() || membership.project_archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let position = sqlx::query_scalar::<_, i32>(
        "DELETE FROM work_item_acceptance_criteria WHERE project_id = $1 AND work_item_id = $2 AND id = $3 RETURNING position",
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(criterion_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("acceptance_criterion"))?;
    sqlx::query(
        "UPDATE work_item_acceptance_criteria SET position = position - 1, updated_at = now() WHERE project_id = $1 AND work_item_id = $2 AND position > $3",
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(position)
    .execute(&mut *transaction)
    .await?;
    let version = advance_work_item_version(
        &mut transaction,
        project_id,
        work_item_id,
        request.expected_version,
    )
    .await?;
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        "criterion_removed",
        "Acceptance criterion removed",
        "work_item.criterion_removed",
        serde_json::json!({"criterionId": criterion_id, "position": position}),
        version,
    )
    .await?;
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn create_work_item_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CreateCommentRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "comment_create")?;
    let request_hash = RequestHash::for_value(&content)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
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
            let body: CommentAppendResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(body)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let mention_ids = content.mentioned_actor_ids();
    let mention_rows = fetch_comment_actor_rows(&mut transaction, project_id, &mention_ids).await?;
    let handles = mention_rows
        .iter()
        .map(|actor| (actor.id, actor.handle.clone()))
        .collect::<HashMap<_, _>>();
    let references = content.work_item_references();
    let reference_ids = references
        .iter()
        .map(|(_, work_item_id)| *work_item_id)
        .collect::<Vec<_>>();
    let reference_rows =
        fetch_work_item_reference_rows(&mut transaction, project_id, &reference_ids).await?;
    let labels = reference_rows
        .iter()
        .map(|item| (item.id, item.key.clone()))
        .collect::<HashMap<_, _>>();
    let mention_nodes = content
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match node {
            MessageNode::Mention { actor_id } => Some((index, *actor_id)),
            MessageNode::Text { .. } | MessageNode::WorkItemRef { .. } => None,
        })
        .collect::<Vec<_>>();
    let (content, plain_text, _) = content
        .validate_with_resources(
            |actor_id| handles.get(&actor_id).cloned(),
            |referenced_id| labels.get(&referenced_id).cloned(),
        )
        .map_err(message_content_error)?
        .into_parts();
    let content_json = serde_json::to_value(content)
        .map_err(|_| AppError::Internal("serialize Work Item comment"))?;
    let comment_id = Uuid::now_v7();
    let created_at = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO work_item_comments (
            id, project_id, work_item_id, author_actor_id,
            content_version, content, plain_text
        ) VALUES ($1, $2, $3, $4, 1, $5, $6)
        RETURNING created_at::text
        "#,
    )
    .bind(comment_id)
    .bind(project_id)
    .bind(work_item_id)
    .bind(principal.actor_id)
    .bind(&content_json)
    .bind(&plain_text)
    .fetch_one(&mut *transaction)
    .await?;
    for (node_index, actor_id) in mention_nodes {
        sqlx::query(
            "INSERT INTO work_item_comment_mentions (project_id, work_item_id, comment_id, actor_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(comment_id)
        .bind(actor_id)
        .bind(i32::try_from(node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut *transaction)
        .await?;
    }
    for (node_index, referenced_id) in &references {
        sqlx::query(
            "INSERT INTO work_item_comment_references (project_id, source_work_item_id, comment_id, referenced_work_item_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(comment_id)
        .bind(referenced_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut *transaction)
        .await?;
    }
    let sequence = insert_work_item_comment_timeline_entry(
        &mut transaction,
        project_id,
        work_item_id,
        comment_id,
    )
    .await?;
    enqueue_typed(
        &mut transaction,
        project_id,
        OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id,
            version: current.version,
        }),
    )
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "work_item.comment_created",
        work_item_id,
        serde_json::json!({"commentId": comment_id, "sequence": sequence}),
    )
    .await?;
    let author = fetch_comment_actor_rows(&mut transaction, project_id, &[principal.actor_id])
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::Internal("comment author unavailable"))?;
    let body = CommentAppendResponse {
        work_item_id,
        sequence,
        comment: WorkItemCommentView {
            id: comment_id,
            author,
            content: content_json,
            plain_text,
            mentioned_actors: mention_rows,
            referenced_work_items: reference_rows
                .into_iter()
                .map(|row| work_item_reference_view(project_id, row))
                .collect(),
            created_at,
        },
    };
    complete(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn get_work_item_timeline(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<WorkItemTimelineQuery>,
) -> Result<Json<WorkItemTimelineResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    fetch_work_item(&state.pool, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let before_sequence = query
        .cursor
        .as_deref()
        .map(|cursor| decode_work_item_timeline_cursor(cursor, project_id, work_item_id))
        .transpose()?;
    let limit = i64::from(query.limit.unwrap_or(50).clamp(1, 100));
    let rows = sqlx::query_as::<_, WorkItemTimelineRow>(
        r#"
        SELECT entry.sequence, entry.kind, entry.created_at::text AS created_at,
               activity.id AS activity_id, activity.kind AS activity_kind,
               activity.summary AS activity_summary,
               activity.payload AS activity_payload,
               comment.id AS comment_id, comment.content AS comment_content,
               comment.plain_text AS comment_plain_text,
               actor.id AS actor_id, actor.kind AS actor_kind,
               membership.display_name AS actor_display_name,
               membership.handle AS actor_handle
        FROM work_item_timeline_entries entry
        LEFT JOIN work_item_activity activity ON activity.id = entry.activity_id
        LEFT JOIN work_item_comments comment ON comment.id = entry.comment_id
        JOIN actors actor ON actor.id = COALESCE(activity.actor_id, comment.author_actor_id)
        JOIN project_memberships membership
          ON membership.project_id = entry.project_id AND membership.actor_id = actor.id
        WHERE entry.project_id = $1 AND entry.work_item_id = $2
          AND ($3::bigint IS NULL OR entry.sequence < $3)
        ORDER BY entry.sequence DESC
        LIMIT $4
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(before_sequence)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() > limit as usize;
    let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let next_cursor = if has_more {
        rows.last()
            .map(|row| encode_work_item_timeline_cursor(project_id, work_item_id, row.sequence))
            .transpose()?
    } else {
        None
    };
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(timeline_entry_view(&state.pool, project_id, work_item_id, row).await?);
    }
    Ok(Json(WorkItemTimelineResponse {
        entries,
        next_cursor,
    }))
}

async fn select_definition(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    table: &'static str,
    selected_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let query = match (table, selected_id) {
        ("work_item_types", Some(_)) => {
            "SELECT id FROM work_item_types WHERE project_id = $1 AND id = $2 AND enabled"
        }
        ("work_item_statuses", Some(_)) => {
            "SELECT id FROM work_item_statuses WHERE project_id = $1 AND id = $2 AND enabled"
        }
        ("work_item_types", None) => {
            "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled AND is_default"
        }
        ("work_item_statuses", None) => {
            "SELECT id FROM work_item_statuses WHERE project_id = $1 AND enabled AND is_default"
        }
        _ => return Err(AppError::Internal("unknown Work Item definition")),
    };
    let mut query = sqlx::query_scalar::<_, Uuid>(query).bind(project_id);
    if let Some(selected_id) = selected_id {
        query = query.bind(selected_id);
    }
    query
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::Validation("Work Item definition"))
}

async fn fetch_comment_actor_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<ActorView>, AppError> {
    if actor_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.actor_id = ANY($2)
          AND membership.enabled AND actor.status = 'active'
        ORDER BY lower(membership.display_name), actor.id
        FOR SHARE OF membership, actor
        "#,
    )
    .bind(project_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, kind, display_name, handle)| ActorView {
        id,
        kind,
        display_name,
        handle,
        avatar_url: None,
    })
    .collect())
}

async fn fetch_work_item_reference_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_ids: &[Uuid],
) -> Result<Vec<WorkItemReferenceRow>, AppError> {
    if work_item_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, WorkItemReferenceRow>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key,
               item.title, status.semantic_category AS status_category,
               item.archived_at::text AS archived_at
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        ORDER BY item.number, item.id
        FOR SHARE OF item
        "#,
    )
    .bind(project_id)
    .bind(work_item_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

fn work_item_reference_view(project_id: Uuid, row: WorkItemReferenceRow) -> WorkItemReferenceView {
    WorkItemReferenceView {
        id: row.id,
        key: row.key,
        title: row.title,
        status_category: row.status_category,
        archived_at: row.archived_at,
        href: format!("/projects/{project_id}/work-items/{}", row.id),
    }
}

async fn timeline_entry_view(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
    row: WorkItemTimelineRow,
) -> Result<WorkItemTimelineEntryView, AppError> {
    let actor = ActorView {
        id: row.actor_id,
        kind: row.actor_kind,
        display_name: row.actor_display_name,
        handle: row.actor_handle,
        avatar_url: None,
    };
    match row.kind.as_str() {
        "activity" => Ok(WorkItemTimelineEntryView {
            sequence: row.sequence,
            kind: row.kind,
            created_at: row.created_at,
            activity: Some(WorkItemActivityView {
                id: row
                    .activity_id
                    .ok_or(AppError::Internal("activity timeline entry"))?,
                actor,
                kind: row
                    .activity_kind
                    .ok_or(AppError::Internal("activity timeline kind"))?,
                summary: row
                    .activity_summary
                    .ok_or(AppError::Internal("activity timeline summary"))?,
                payload: row
                    .activity_payload
                    .ok_or(AppError::Internal("activity timeline payload"))?,
            }),
            comment: None,
        }),
        "comment" => {
            let comment_id = row
                .comment_id
                .ok_or(AppError::Internal("comment timeline entry"))?;
            let mentioned_actors = sqlx::query_as::<_, (Uuid, String, String, String)>(
                r#"
                SELECT actor.id, actor.kind, membership.display_name, membership.handle
                FROM work_item_comment_mentions mention
                JOIN actors actor ON actor.id = mention.actor_id
                JOIN project_memberships membership
                  ON membership.project_id = mention.project_id
                 AND membership.actor_id = mention.actor_id
                WHERE mention.project_id = $1 AND mention.work_item_id = $2
                  AND mention.comment_id = $3
                GROUP BY actor.id, actor.kind, membership.display_name, membership.handle
                ORDER BY min(mention.node_index), actor.id
                "#,
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(comment_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, kind, display_name, handle)| ActorView {
                id,
                kind,
                display_name,
                handle,
                avatar_url: None,
            })
            .collect();
            let referenced_work_items = sqlx::query_as::<_, WorkItemReferenceRow>(
                r#"
                SELECT item.id,
                       project.project_key || '-' || item.number::text AS key,
                       item.title, status.semantic_category AS status_category,
                       item.archived_at::text AS archived_at
                FROM work_item_comment_references reference
                JOIN work_items item
                  ON item.project_id = reference.project_id
                 AND item.id = reference.referenced_work_item_id
                JOIN projects project ON project.id = item.project_id
                JOIN work_item_statuses status
                  ON status.project_id = item.project_id AND status.id = item.status_id
                WHERE reference.project_id = $1
                  AND reference.source_work_item_id = $2
                  AND reference.comment_id = $3
                GROUP BY item.id, project.project_key, item.number, item.title,
                         status.semantic_category, item.archived_at
                ORDER BY min(reference.node_index), item.id
                "#,
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(comment_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|item| work_item_reference_view(project_id, item))
            .collect();
            Ok(WorkItemTimelineEntryView {
                sequence: row.sequence,
                kind: row.kind,
                created_at: row.created_at.clone(),
                activity: None,
                comment: Some(WorkItemCommentView {
                    id: comment_id,
                    author: actor,
                    content: row
                        .comment_content
                        .ok_or(AppError::Internal("comment timeline content"))?,
                    plain_text: row
                        .comment_plain_text
                        .ok_or(AppError::Internal("comment timeline text"))?,
                    mentioned_actors,
                    referenced_work_items,
                    created_at: row.created_at,
                }),
            })
        }
        _ => Err(AppError::Internal("Work Item timeline kind")),
    }
}

async fn validate_assignee(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    assignee_actor_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(assignee_actor_id) = assignee_actor_id else {
        return Ok(());
    };
    let valid = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1
              AND membership.actor_id = $2
              AND membership.enabled
              AND actor.status = 'active'
              AND actor.kind IN ('human', 'external_agent')
        )
        "#,
    )
    .bind(project_id)
    .bind(assignee_actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(AppError::Validation("assigneeActorId"))
    }
}

async fn require_status_transition(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    from_status_id: Uuid,
    to_status_id: Uuid,
) -> Result<(), AppError> {
    let allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM work_item_status_transitions
            WHERE project_id = $1
              AND from_status_id = $2
              AND to_status_id = $3
        )
        "#,
    )
    .bind(project_id)
    .bind(from_status_id)
    .bind(to_status_id)
    .fetch_one(&mut **transaction)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(AppError::Conflict("work_item_status", "transition"))
    }
}

async fn project_membership(
    pool: &PgPool,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembershipRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT project.id AS project_id,
               project.archived_at::text AS project_archived_at,
               membership.role, membership.enabled AS membership_enabled
        FROM projects project
        JOIN project_memberships membership ON membership.project_id = project.id
        WHERE project.id = $1 AND membership.actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?)
}

async fn lock_project_membership(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembershipRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT project.id AS project_id,
               project.archived_at::text AS project_archived_at,
               membership.role, membership.enabled AS membership_enabled
        FROM projects project
        JOIN project_memberships membership ON membership.project_id = project.id
        WHERE project.id = $1 AND membership.actor_id = $2
        FOR UPDATE OF project, membership
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn authorize_work_item(
    principal: &AuthenticatedActor,
    membership: &ProjectMembershipRow,
    action: Action,
    work_item_id: Option<Uuid>,
) -> Result<(), AppError> {
    let scope = work_item_id.map_or_else(
        || ProjectScope::project(membership.project_id),
        |id| ProjectScope::work_item(membership.project_id, id),
    );
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            membership.project_id,
            membership_role(&membership.role)?,
            membership.membership_enabled,
        )),
        None,
    );
    match authorize(&subject, action, Resource::Project(scope)) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) if action == Action::WorkItemView => Err(AppError::NotFound("work_item")),
        Decision::Deny(_) => Err(AppError::Forbidden),
    }
}

fn capabilities(membership: &ProjectMembershipRow) -> Result<WorkItemCapabilities, AppError> {
    let role = membership_role(&membership.role)?;
    Ok(WorkItemCapabilities {
        view: membership.membership_enabled,
        edit: membership.membership_enabled
            && matches!(
                role,
                MembershipRole::Owner | MembershipRole::Admin | MembershipRole::Member
            ),
        archive: membership.membership_enabled
            && matches!(
                role,
                MembershipRole::Owner | MembershipRole::Admin | MembershipRole::Member
            ),
    })
}

async fn fetch_work_item(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    Ok(sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_SELECT)
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?)
}

async fn fetch_work_item_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    Ok(sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_SELECT)
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(&mut **transaction)
        .await?)
}

async fn fetch_work_item_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    let query = format!("{WORK_ITEM_SELECT} FOR UPDATE OF item");
    Ok(sqlx::query_as::<_, WorkItemRow>(&query)
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(&mut **transaction)
        .await?)
}

async fn current_work_item_view(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    capabilities: WorkItemCapabilities,
) -> Result<WorkItemView, AppError> {
    let row = fetch_work_item_in_transaction(transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::Internal("updated Work Item is unavailable"))?;
    work_item_view(row, capabilities)
}

async fn advance_work_item_version(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    expected_version: i64,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        r#"
        UPDATE work_items
        SET version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $3
        RETURNING version
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(expected_version)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Internal("locked Work Item lost its version"))
}

const WORK_ITEM_SELECT: &str = r#"
    SELECT
        item.id, item.project_id, project.project_key,
        item.number, item.title, item.description,
        item.type_id, item_type.name AS type_name,
        item.status_id, status.name AS status_name,
        status.semantic_category, item.priority,
        item.assignee_actor_id,
        assignee.display_name AS assignee_display_name,
        assignee.handle AS assignee_handle,
        assignee_actor.kind AS assignee_kind,
        CASE WHEN assignee.actor_id IS NULL THEN NULL
             ELSE assignee.enabled AND assignee_actor.status = 'active'
        END AS assignee_active,
        item.version, item.archived_at::text AS archived_at,
        item.created_at::text AS created_at,
        item.updated_at::text AS updated_at,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', criterion.id,
                    'text', criterion.text,
                    'position', criterion.position,
                    'completed', criterion.completed,
                    'completedBy', CASE
                        WHEN criterion.completed_by_actor_id IS NULL THEN NULL
                        ELSE jsonb_build_object(
                            'id', completed_actor.id,
                            'kind', completed_actor.kind,
                            'displayName', completed_membership.display_name,
                            'handle', completed_membership.handle,
                            'avatarUrl', NULL
                        )
                    END,
                    'completedAt', criterion.completed_at::text,
                    'createdAt', criterion.created_at::text,
                    'updatedAt', criterion.updated_at::text
                ) ORDER BY criterion.position, criterion.id
            )
            FROM work_item_acceptance_criteria criterion
            LEFT JOIN project_memberships completed_membership
              ON completed_membership.project_id = criterion.project_id
             AND completed_membership.actor_id = criterion.completed_by_actor_id
            LEFT JOIN actors completed_actor
              ON completed_actor.id = criterion.completed_by_actor_id
            WHERE criterion.project_id = item.project_id
              AND criterion.work_item_id = item.id
        ), '[]'::jsonb) AS acceptance_criteria,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', thread.id,
                    'name', thread.name,
                    'parentChannelId', channel.id,
                    'parentChannelName', channel.name,
                    'archivedAt', thread.archived_at::text,
                    'newestRunState', newest_run.state,
                    'href', '/projects/' || item.project_id || '/threads/' || thread.id
                ) ORDER BY link.created_at, thread.id
            )
            FROM work_item_conversation_links link
            JOIN conversations thread
              ON thread.project_id = link.project_id AND thread.id = link.conversation_id
            JOIN conversations channel
              ON channel.project_id = thread.project_id AND channel.id = thread.parent_channel_id
            LEFT JOIN LATERAL (
                SELECT run.state
                FROM agent_runs run
                WHERE run.project_id = thread.project_id
                  AND run.conversation_id = thread.id
                ORDER BY run.created_at DESC, run.id DESC
                LIMIT 1
            ) newest_run ON true
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
        ), '[]'::jsonb) AS linked_threads,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', repository.id,
                    'owner', repository.owner,
                    'name', repository.name,
                    'role', link.role,
                    'preparationState', repository.preparation_state,
                    'available', repository.enabled AND repository.trusted
                        AND repository.archived_at IS NULL
                        AND repository.preparation_state = 'ready',
                    'archivedAt', repository.archived_at::text,
                    'href', '/projects/' || item.project_id || '/settings'
                ) ORDER BY CASE link.role WHEN 'primary' THEN 0 ELSE 1 END,
                         repository.owner, repository.name, repository.id
            )
            FROM work_item_repository_links link
            JOIN repositories repository
              ON repository.project_id = link.project_id AND repository.id = link.repository_id
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
        ), '[]'::jsonb) AS linked_repositories,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', pull_request.id,
                    'number', pull_request.pull_request_number,
                    'title', pull_request.title,
                    'url', pull_request.html_url,
                    'state', pull_request.state,
                    'draft', pull_request.draft,
                    'checksState', pull_request.checks_state,
                    'reviewState', pull_request.review_state,
                    'mergeabilityState', pull_request.mergeability_state,
                    'syncState', pull_request.sync_state,
                    'syncFailureCategory', pull_request.sync_failure_category,
                    'automationState', pull_request.automation_state,
                    'automationReason', pull_request.automation_reason,
                    'source', link.source,
                    'confirmed', link.confirmed
                ) ORDER BY pull_request.pull_request_number, pull_request.id
            )
            FROM github_pull_request_work_item_links link
            JOIN github_pull_requests pull_request
              ON pull_request.project_id = link.project_id
             AND pull_request.id = link.github_pull_request_id
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
        ), '[]'::jsonb) AS linked_pull_requests
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = item.project_id AND item_type.id = item.type_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id
     AND assignee.actor_id = item.assignee_actor_id
    LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
    WHERE item.project_id = $1 AND item.id = $2
"#;

fn work_item_view(
    row: WorkItemRow,
    capabilities: WorkItemCapabilities,
) -> Result<WorkItemView, AppError> {
    validate_priority(&row.priority)?;
    validate_semantic_category(Some(&row.semantic_category))?;
    let assignee = match (
        row.assignee_actor_id,
        row.assignee_display_name,
        row.assignee_handle,
        row.assignee_kind,
        row.assignee_active,
    ) {
        (Some(actor_id), Some(display_name), Some(handle), Some(kind), Some(active)) => {
            Some(AssigneeView {
                actor_id,
                display_name,
                handle,
                kind,
                active,
            })
        }
        (None, None, None, None, None) => None,
        _ => return Err(AppError::Internal("invalid Work Item assignee projection")),
    };
    let acceptance_criteria = serde_json::from_value(row.acceptance_criteria)
        .map_err(|_| AppError::Internal("invalid Work Item criteria projection"))?;
    let linked_threads = serde_json::from_value(row.linked_threads)
        .map_err(|_| AppError::Internal("invalid linked Thread projection"))?;
    let linked_repositories = serde_json::from_value(row.linked_repositories)
        .map_err(|_| AppError::Internal("invalid linked Repository projection"))?;
    let linked_pull_requests = serde_json::from_value(row.linked_pull_requests)
        .map_err(|_| AppError::Internal("invalid linked pull-request projection"))?;
    Ok(WorkItemView {
        id: row.id,
        project_id: row.project_id,
        key: format!("{}-{}", row.project_key, row.number),
        number: row.number,
        title: row.title,
        description: row.description,
        item_type: WorkItemTypeView {
            id: row.type_id,
            name: row.type_name,
        },
        status: WorkItemStatusView {
            id: row.status_id,
            name: row.status_name,
            semantic_category: row.semantic_category,
        },
        priority: row.priority,
        assignee,
        version: row.version,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        acceptance_criteria,
        linked_threads,
        linked_repositories,
        linked_pull_requests,
        capabilities,
    })
}

fn normalize_title(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_TITLE_CHARACTERS
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation("title"));
    }
    Ok(value.to_owned())
}

fn normalize_description(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.chars().count() > MAX_DESCRIPTION_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("description"));
    }
    Ok(value.to_owned())
}

fn normalize_criterion_text(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 1000
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("criterion text"));
    }
    Ok(value.to_owned())
}

fn normalize_search(value: Option<&str>) -> Result<Option<String>, AppError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| {
        value.chars().count() > MAX_SEARCH_CHARACTERS || value.chars().any(char::is_control)
    }) {
        return Err(AppError::Validation("search"));
    }
    Ok(value.map(str::to_owned))
}

fn validate_readable_key(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        || !value.rsplit_once('-').is_some_and(|(project_key, number)| {
            !project_key.is_empty()
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && number.parse::<i64>().is_ok_and(|number| number > 0)
        })
    {
        return Err(AppError::Invalid("Work Item key"));
    }
    Ok(())
}

fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn default_priority() -> String {
    "no_priority".to_owned()
}

fn validate_priority(value: &str) -> Result<(), AppError> {
    if matches!(value, "no_priority" | "low" | "medium" | "high" | "urgent") {
        Ok(())
    } else {
        Err(AppError::Validation("priority"))
    }
}

fn validate_priority_filter(value: Option<&str>) -> Result<(), AppError> {
    value.map_or(Ok(()), validate_priority)
}

fn validate_semantic_category(value: Option<&str>) -> Result<(), AppError> {
    if value.is_none_or(|value| {
        matches!(
            value,
            "backlog"
                | "unstarted"
                | "started"
                | "review"
                | "verification"
                | "completed"
                | "canceled"
        )
    }) {
        Ok(())
    } else {
        Err(AppError::Validation("semanticCategory"))
    }
}

fn encode_cursor(project_id: Uuid, before_number: i64) -> Result<String, AppError> {
    if before_number < 1 {
        return Err(AppError::Internal("encode Work Item cursor"));
    }
    let mut bytes = Vec::with_capacity(25);
    bytes.push(1);
    bytes.extend_from_slice(project_id.as_bytes());
    bytes.extend_from_slice(&before_number.to_be_bytes());
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(value: &str, project_id: Uuid) -> Result<i64, AppError> {
    if value.len() > 64 {
        return Err(AppError::Invalid("Work Item cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::Invalid("Work Item cursor"))?;
    if bytes.len() != 25 || bytes[0] != 1 || bytes[1..17] != *project_id.as_bytes() {
        return Err(AppError::Invalid("Work Item cursor"));
    }
    let number = i64::from_be_bytes(
        bytes[17..25]
            .try_into()
            .map_err(|_| AppError::Invalid("Work Item cursor"))?,
    );
    if number < 1 {
        return Err(AppError::Invalid("Work Item cursor"));
    }
    Ok(number)
}

fn encode_work_item_timeline_cursor(
    project_id: Uuid,
    work_item_id: Uuid,
    before_sequence: i64,
) -> Result<String, AppError> {
    if before_sequence < 1 {
        return Err(AppError::Internal("encode Work Item timeline cursor"));
    }
    let mut bytes = Vec::with_capacity(41);
    bytes.push(1);
    bytes.extend_from_slice(project_id.as_bytes());
    bytes.extend_from_slice(work_item_id.as_bytes());
    bytes.extend_from_slice(&before_sequence.to_be_bytes());
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_work_item_timeline_cursor(
    value: &str,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<i64, AppError> {
    if value.len() > 80 {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::Invalid("Work Item timeline cursor"))?;
    if bytes.len() != 41
        || bytes[0] != 1
        || bytes[1..17] != *project_id.as_bytes()
        || bytes[17..33] != *work_item_id.as_bytes()
    {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    let sequence = i64::from_be_bytes(
        bytes[33..41]
            .try_into()
            .map_err(|_| AppError::Invalid("Work Item timeline cursor"))?,
    );
    if sequence < 1 {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    Ok(sequence)
}

fn message_content_error(_error: MessageContentError) -> AppError {
    AppError::Validation("comment content")
}

fn work_item_response(status: StatusCode, item: WorkItemView) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(&format!(
        "/projects/{}/work-items/{}",
        item.project_id, item.id
    ))
    .map_err(|_| AppError::Internal("construct Work Item Location header"))?;
    Ok((status, [(LOCATION, location)], Json(item)).into_response())
}

fn work_item_version_conflict_response(item: WorkItemView) -> Result<Response, AppError> {
    Ok((
        StatusCode::CONFLICT,
        Json(WorkItemConflictEnvelope {
            error: WorkItemConflictBody {
                code: "version_conflict",
                message: "The resource changed after it was loaded.",
                details: WorkItemConflictDetails {
                    resource: "work_item",
                    current_version: item.version,
                    current: item,
                },
            },
        }),
    )
        .into_response())
}

#[allow(clippy::too_many_arguments)]
async fn record_work_item_change(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    kind: &'static str,
    summary: &'static str,
    audit_action: &'static str,
    payload: serde_json::Value,
    version: i64,
) -> Result<(), AppError> {
    insert_work_item_activity(
        transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        kind,
        summary,
        &payload,
    )
    .await?;
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id,
            version,
        }),
    )
    .await?;
    insert_audit_event(
        transaction,
        headers,
        principal,
        project_id,
        audit_action,
        work_item_id,
        payload,
    )
    .await
}

pub(crate) async fn insert_work_item_activity(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    kind: &str,
    summary: &str,
    payload: &serde_json::Value,
) -> Result<Uuid, AppError> {
    let activity_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_item_activity (
            id, project_id, work_item_id, actor_id, kind, summary, payload
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(activity_id)
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    let sequence =
        allocate_work_item_timeline_sequence(transaction, project_id, work_item_id).await?;
    sqlx::query(
        r#"
        INSERT INTO work_item_timeline_entries (
            project_id, work_item_id, sequence, kind, activity_id
        )
        VALUES ($1, $2, $3, 'activity', $4)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(sequence)
    .bind(activity_id)
    .execute(&mut **transaction)
    .await?;
    Ok(activity_id)
}

async fn allocate_work_item_timeline_sequence(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        r#"
        UPDATE work_items
        SET next_timeline_sequence = next_timeline_sequence + 1
        WHERE project_id = $1 AND id = $2
        RETURNING next_timeline_sequence - 1
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("work_item"))
}

async fn insert_work_item_comment_timeline_entry(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<i64, AppError> {
    let sequence =
        allocate_work_item_timeline_sequence(transaction, project_id, work_item_id).await?;
    sqlx::query(
        r#"
        INSERT INTO work_item_timeline_entries (
            project_id, work_item_id, sequence, kind, comment_id
        )
        VALUES ($1, $2, $3, 'comment', $4)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(sequence)
    .bind(comment_id)
    .execute(&mut **transaction)
    .await?;
    Ok(sequence)
}

async fn insert_audit_event(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &'static str,
    work_item_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        )
        VALUES (
            $1, $2, $3, $4, 'password_session',
            $5, 'work_item', $6, 'succeeded', $7, $8
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(work_item_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("invalid membership role")),
    }
}

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("invalid actor kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DESCRIPTION_CHARACTERS, UpdateWorkItemRequest, decode_cursor, encode_cursor,
        normalize_description, normalize_title, normalize_worker_prompt, validate_priority,
        validate_readable_key, worker_thread_name,
    };
    use uuid::Uuid;

    #[test]
    fn work_item_text_and_priority_are_bounded() {
        assert_eq!(
            normalize_title("  Build it  ").expect("normalize valid Work Item title"),
            "Build it"
        );
        assert!(normalize_title("bad\u{1b}").is_err());
        assert!(normalize_description("line one\nline two").is_ok());
        assert!(normalize_description("bad\u{7}").is_err());
        assert!(normalize_description(&"a".repeat(MAX_DESCRIPTION_CHARACTERS + 1)).is_err());
        assert!(validate_priority("urgent").is_ok());
        assert!(validate_priority("critical").is_err());
    }

    #[test]
    fn work_item_cursor_is_project_bound() {
        let project_id = Uuid::now_v7();
        let cursor = encode_cursor(project_id, 42).expect("encode valid Work Item cursor");
        assert_eq!(
            decode_cursor(&cursor, project_id).expect("decode valid Work Item cursor"),
            42
        );
        assert!(decode_cursor(&cursor, Uuid::now_v7()).is_err());
    }

    #[test]
    fn readable_keys_and_nullable_assignee_patch_are_unambiguous() {
        assert!(validate_readable_key("AW-42").is_ok());
        assert!(validate_readable_key("aw-42").is_err());
        assert!(validate_readable_key("AW-nope").is_err());

        let omitted: UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "expectedVersion": 1,
            "title": "Keep assignment"
        }))
        .expect("decode omitted assignee patch");
        assert_eq!(omitted.assignee_actor_id, None);
        let cleared: UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "expectedVersion": 1,
            "assigneeActorId": null
        }))
        .expect("decode cleared assignee patch");
        assert_eq!(cleared.assignee_actor_id, Some(None));
    }

    #[test]
    fn worker_prompt_and_thread_name_are_bounded() {
        assert_eq!(
            normalize_worker_prompt("  make the change  ").expect("valid Worker prompt"),
            "make the change"
        );
        assert!(normalize_worker_prompt("\u{1b}").is_err());
        assert!(normalize_worker_prompt(&"a".repeat(18_001)).is_err());

        let name = worker_thread_name("AW-42", &"界".repeat(200), Uuid::now_v7());
        assert!(name.chars().count() <= 120);
        assert!(name.starts_with("AW-42: "));
    }
}
