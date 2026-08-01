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
    outbox::{OutboxEventPayload, WorkItemUpdatedPayload, enqueue_typed},
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
            "/api/v1/projects/{project_id}/work-items/{work_item_id}",
            get(get_work_item).patch(update_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
            axum::routing::post(archive_work_item),
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
            "/api/v1/projects/{project_id}/work-item-workflow",
            get(get_workflow),
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
    capabilities: WorkItemCapabilities,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCapabilities {
    view: bool,
    edit: bool,
    archive: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemListResponse {
    items: Vec<WorkItemCardView>,
    next_cursor: Option<String>,
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
            item.updated_at::text AS updated_at
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
            sqlx::query(
                r#"
                INSERT INTO work_item_activity (
                    id, project_id, work_item_id, actor_id, kind, summary, payload
                )
                VALUES ($1, $2, $3, $4, 'work_item_created', 'Work Item created', '{}'::jsonb)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(project_id)
            .bind(work_item_id)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
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
        item.updated_at::text AS updated_at
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
    sqlx::query(
        r#"
        INSERT INTO work_item_activity (
            id, project_id, work_item_id, actor_id, kind, summary, payload
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(work_item_id)
    .bind(principal.actor_id)
    .bind(kind)
    .bind(summary)
    .bind(&payload)
    .execute(&mut **transaction)
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
        normalize_description, normalize_title, validate_priority, validate_readable_key,
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
}
