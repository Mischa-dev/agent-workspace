//! HTTP routes for planning a project's tasks: dependencies, related and
//! duplicate tasks, watching, claims, labels, milestones, the workflow and
//! bulk changes. The rules live in `work_item_planning`,
//! `work_item_workflow`, `work_item_labels` and `work_item_commands`, which
//! MCP calls too.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::{
    ProjectMembershipRow, authorize_work_item, browser_actor, capabilities,
    lock_project_membership, project_membership,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiPath, ApiQuery, AppError},
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash, complete, reserve,
    },
    policy::{Action, Permission},
    work_item_commands::CommandActor,
    work_item_labels::{self, LabelRow},
    work_item_planning::{self as planning},
    work_item_workflow::{self as workflow},
};

// ---------------------------------------------------------------------------
// Shared write plumbing

/// A write that holds the project lock and an idempotency reservation.
pub(super) struct Write {
    pub(super) transaction: Transaction<'static, Postgres>,
    pub(super) reservation: IdempotencyReservation,
    pub(super) actor: CommandActor,
}

pub(super) enum Started {
    Replay(Response),
    Write(Box<Write>),
}

/// Checks the session, locks the project, authorizes `action` and reserves
/// the idempotency key. A retried request gets the first response back.
#[allow(clippy::too_many_arguments)]
pub(super) async fn start_write<T: Serialize>(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
    work_item_id: Option<Uuid>,
    scope: String,
    request: &T,
) -> Result<Started, AppError> {
    require_authenticated_write(state, headers, principal).await?;
    let key = IdempotencyKey::from_headers(headers)?;
    let request_hash = RequestHash::for_value(request)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound(if work_item_id.is_some() {
            "work_item"
        } else {
            "project"
        }))?;
    authorize_work_item(principal, &membership, action, work_item_id)?;
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
            let body: Value = replay.decode()?;
            transaction.commit().await?;
            Ok(Started::Replay(
                (replay.status(), Json(body)).into_response(),
            ))
        }
        IdempotencyOutcome::Reserved(reservation) => Ok(Started::Write(Box::new(Write {
            transaction,
            reservation,
            actor: browser_actor(headers, principal, project_id)?,
        }))),
    }
}

macro_rules! begin {
    ($started:expr) => {
        match $started? {
            Started::Replay(response) => return Ok(response),
            Started::Write(write) => *write,
        }
    };
}
pub(super) use begin;

impl Write {
    pub(super) async fn finish<T: Serialize>(
        mut self,
        status: StatusCode,
        body: &T,
    ) -> Result<Response, AppError> {
        complete(&mut self.transaction, &self.reservation, status, body).await?;
        self.transaction.commit().await?;
        Ok((status, Json(body)).into_response())
    }
}

/// An error with a message written for the person, and optional details.
pub(super) fn refusal(status: StatusCode, code: &str, message: String, details: Value) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message, "details": details } })),
    )
        .into_response()
}

pub(super) fn invalid(field: &str, message: impl Into<String>) -> Response {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_failed",
        message.into(),
        json!({ "field": field }),
    )
}

/// Who may change the project's task workflow.
pub(super) fn may_change_workflow(membership: &ProjectMembershipRow) -> Result<bool, AppError> {
    membership.may(Permission::TasksWorkflow)
}

// ---------------------------------------------------------------------------
// Dependencies and relations

// ---------------------------------------------------------------------------
// Watching and claims

// ---------------------------------------------------------------------------
// Labels

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LabelView {
    id: Uuid,
    name: String,
    color: String,
    description: String,
    archived: bool,
    task_count: i64,
    version: i64,
}

impl From<LabelRow> for LabelView {
    fn from(row: LabelRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            color: row.color,
            description: row.description,
            archived: row.archived,
            task_count: row.task_count,
            version: row.version,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct IncludeArchivedQuery {
    pub(super) include_archived: Option<bool>,
}

pub(super) async fn list_labels(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<IncludeArchivedQuery>,
) -> Result<Json<Value>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let labels = work_item_labels::list(
        &state.pool,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?
    .into_iter()
    .map(LabelView::from)
    .collect::<Vec<_>>();
    let can_edit = capabilities(&membership)?.edit && membership.project_archived_at.is_none();
    Ok(Json(json!({ "labels": labels, "canEdit": can_edit })))
}

// ---------------------------------------------------------------------------
// Milestones

pub(super) async fn list_milestones(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<IncludeArchivedQuery>,
) -> Result<Json<Value>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let milestones = planning::list_milestones(
        &mut connection,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?;
    let can_edit = capabilities(&membership)?.edit && membership.project_archived_at.is_none();
    Ok(Json(
        json!({ "milestones": milestones, "canEdit": can_edit }),
    ))
}

// ---------------------------------------------------------------------------
// Workflow

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowView {
    can_edit: bool,
    version: i64,
    estimate_unit: String,
    types: Vec<WorkflowTypeView>,
    statuses: Vec<WorkflowStatusView>,
    transitions: Vec<WorkflowTransitionView>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowTypeView {
    id: Uuid,
    name: String,
    position: i32,
    is_default: bool,
    task_count: i64,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowStatusView {
    id: Uuid,
    name: String,
    semantic_category: String,
    position: i32,
    is_default: bool,
    task_count: i64,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkflowTransitionView {
    from_status_id: Uuid,
    to_status_id: Uuid,
}

async fn workflow_view(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    can_edit: bool,
) -> Result<WorkflowView, AppError> {
    let (version, estimate_unit) = workflow::task_settings(connection, project_id).await?;
    let types = sqlx::query_as::<_, WorkflowTypeView>(
        r#"
        SELECT definition.id, definition.name, definition.position, definition.is_default,
               (SELECT count(*) FROM task_rows item
                 WHERE item.project_id = definition.project_id AND item.type_id = definition.id
                   AND item.archived_at IS NULL) AS task_count
        FROM work_item_types definition
        WHERE definition.project_id = $1 AND definition.enabled
        ORDER BY definition.position, definition.id
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let statuses = sqlx::query_as::<_, WorkflowStatusView>(
        r#"
        SELECT definition.id, definition.name, definition.semantic_category,
               definition.position, definition.is_default,
               (SELECT count(*) FROM task_rows item
                 WHERE item.project_id = definition.project_id AND item.status_id = definition.id
                   AND item.archived_at IS NULL) AS task_count
        FROM work_item_statuses definition
        WHERE definition.project_id = $1 AND definition.enabled
        ORDER BY definition.position, definition.id
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let transitions = sqlx::query_as::<_, WorkflowTransitionView>(
        "SELECT from_status_id, to_status_id FROM work_item_status_transitions WHERE project_id = $1 ORDER BY from_status_id, to_status_id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(WorkflowView {
        can_edit,
        version,
        estimate_unit,
        types,
        statuses,
        transitions,
    })
}

pub(super) async fn get_workflow(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<WorkflowView>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let can_edit = may_change_workflow(&membership)? && membership.project_archived_at.is_none();
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    Ok(Json(
        workflow_view(&mut connection, project_id, can_edit).await?,
    ))
}
