//! HTTP routes for planning a project's tasks: dependencies, related and
//! duplicate tasks, watching, claims, labels, milestones, the workflow and
//! bulk changes. The rules live in `work_item_planning`,
//! `work_item_workflow`, `work_item_labels` and `work_item_commands`, which
//! MCP calls too.

use std::collections::BTreeMap;

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
    ProjectMembershipRow, authorize_work_item, browser_actor, capabilities, current_work_item_view,
    list, lock_project_membership, parse_task_date, project_membership, validate_priority,
    work_item_version_conflict_response,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::ActorView,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash, complete, reserve,
        resource_bound_scope,
    },
    policy::{Action, Permission},
    work_item_commands::{
        ArchiveCommand, ArchiveOutcome, AuditVersionMetadata, CommandActor, TaskFieldChanges,
        UpdateWorkItemCommand, UpdateWorkItemOutcome, enqueue_update, insert_activity,
        record_audit_for, set_archived, update_work_item_by_id,
    },
    work_item_labels::{
        self, ChangeLabelOutcome, CreateLabelOutcome, LabelChange, LabelDelta, LabelLimit,
        LabelRow, NewLabel,
    },
    work_item_planning::{
        self as planning, Direction, LinkRefusal, MAX_LINKS_PER_CALL, MilestoneChange,
        MilestoneRefusal, MilestoneView, Reader, RelationKind,
    },
    work_item_workflow::{self as workflow, WorkflowDocument, WorkflowRefusal},
};

/// Most tasks one bulk change touches.
const MAX_BULK_ITEMS: usize = 200;

// ---------------------------------------------------------------------------
// Shared write plumbing

/// A write that holds the project lock and an idempotency reservation.
pub(super) struct Write {
    pub(super) transaction: Transaction<'static, Postgres>,
    pub(super) membership: ProjectMembershipRow,
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
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let request_hash = RequestHash::for_value(request)?;
    let mut transaction = state.pool.begin().await?;
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
            membership,
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

    async fn task(&mut self, viewer: Uuid, task_id: Uuid) -> Result<super::WorkItemView, AppError> {
        current_work_item_view(
            &mut self.transaction,
            self.actor.project_id,
            task_id,
            viewer,
            capabilities(&self.membership)?,
        )
        .await
    }

    /// The task's current version, or a conflict response when it isn't
    /// `expected`.
    async fn check_version(
        &mut self,
        viewer: Uuid,
        task_id: Uuid,
        expected: i64,
    ) -> Result<Option<Response>, AppError> {
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM work_items WHERE project_id = $1 AND id = $2 AND archived_at IS NULL",
        )
        .bind(self.actor.project_id)
        .bind(task_id)
        .fetch_optional(&mut *self.transaction)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
        if version == expected {
            return Ok(None);
        }
        let current = self.task(viewer, task_id).await?;
        Ok(Some(work_item_version_conflict_response(current)?))
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

/// Owners and admins (or a role that kept "Change and archive projects"):
/// who may release someone else's claim.
pub(super) fn is_manager(membership: &ProjectMembershipRow) -> Result<bool, AppError> {
    membership.may(Permission::ProjectsManage)
}

/// Who may change the project's task workflow.
pub(super) fn may_change_workflow(membership: &ProjectMembershipRow) -> Result<bool, AppError> {
    membership.may(Permission::TasksWorkflow)
}

// ---------------------------------------------------------------------------
// Dependencies and relations

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DependenciesRequest {
    expected_version: i64,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(default)]
    remove: bool,
}

pub(super) async fn change_dependencies(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<DependenciesRequest>,
) -> Result<Response, AppError> {
    let count = request.blocked_by.len() + request.blocks.len();
    if count == 0 || count > MAX_LINKS_PER_CALL {
        return Err(AppError::Validation("blockedBy"));
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            Some(task_id),
            resource_bound_scope("work_items", task_id, "dependencies")?,
            &request,
        )
        .await
    );
    if let Some(conflict) = write
        .check_version(principal.actor_id, task_id, request.expected_version)
        .await?
    {
        return Ok(conflict);
    }
    let reader = Reader {
        actor_id: principal.actor_id,
        connection_id: None,
    };
    let references = request
        .blocked_by
        .iter()
        .map(|reference| (reference, Direction::BlockedBy))
        .chain(
            request
                .blocks
                .iter()
                .map(|reference| (reference, Direction::Blocks)),
        )
        .collect::<Vec<_>>();
    let mut others = Vec::with_capacity(references.len());
    for (reference, direction) in &references {
        match planning::resolve_reference(&mut write.transaction, reader, project_id, reference)
            .await?
        {
            Some((other, _)) => others.push((other, *direction)),
            None => {
                let field = match direction {
                    Direction::BlockedBy => "blockedBy",
                    Direction::Blocks => "blocks",
                };
                return Ok(invalid(field, LinkRefusal::NotFound.message(reference)));
            }
        }
    }
    let actor = write.actor.clone();
    match planning::change_dependencies(
        &mut write.transaction,
        &actor,
        task_id,
        &others,
        request.remove,
    )
    .await?
    {
        Ok(_) => {}
        Err((index, refusal)) => return Ok(link_refusal(&refusal, references[index].0)),
    }
    let item = write.task(principal.actor_id, task_id).await?;
    write.finish(StatusCode::OK, &item).await
}

fn link_refusal(refused: &LinkRefusal, reference: &str) -> Response {
    let message = refused.message(reference);
    match refused {
        LinkRefusal::Cycle { path } => refusal(
            StatusCode::CONFLICT,
            "dependency_cycle",
            message,
            json!({ "path": path }),
        ),
        LinkRefusal::AlreadyDuplicate { .. }
        | LinkRefusal::CantClose { .. }
        | LinkRefusal::TooMany { .. }
        | LinkRefusal::Archived { .. } => {
            refusal(StatusCode::CONFLICT, "conflict", message, Value::Null)
        }
        LinkRefusal::NotFound | LinkRefusal::SelfLink | LinkRefusal::OtherWorkspace => {
            invalid("target", message)
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RelationRequest {
    expected_version: i64,
    kind: String,
    target: String,
    #[serde(default)]
    remove: bool,
    #[serde(default)]
    close: bool,
}

pub(super) async fn change_relation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<RelationRequest>,
) -> Result<Response, AppError> {
    let kind = RelationKind::parse(&request.kind).ok_or(AppError::Validation("kind"))?;
    if request.close && (request.remove || kind != RelationKind::Duplicates) {
        return Err(AppError::Validation("close"));
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            Some(task_id),
            resource_bound_scope("work_items", task_id, "relations")?,
            &request,
        )
        .await
    );
    if let Some(conflict) = write
        .check_version(principal.actor_id, task_id, request.expected_version)
        .await?
    {
        return Ok(conflict);
    }
    let reader = Reader {
        actor_id: principal.actor_id,
        connection_id: None,
    };
    let Some((other, _)) =
        planning::resolve_reference(&mut write.transaction, reader, project_id, &request.target)
            .await?
    else {
        return Ok(invalid(
            "target",
            LinkRefusal::NotFound.message(&request.target),
        ));
    };
    let actor = write.actor.clone();
    if let Err(refused) = planning::change_relation(
        &mut write.transaction,
        &actor,
        task_id,
        other,
        kind,
        request.remove,
        request.close,
    )
    .await?
    {
        return Ok(link_refusal(&refused, &request.target));
    }
    let item = write.task(principal.actor_id, task_id).await?;
    write.finish(StatusCode::OK, &item).await
}

// ---------------------------------------------------------------------------
// Watching and claims

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WatchRequest {
    watching: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchResponse {
    watching: bool,
    watchers: Vec<ActorView>,
}

pub(super) async fn watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WatchRequest>,
) -> Result<Response, AppError> {
    // Anyone who can see a task can follow it.
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemView,
            Some(task_id),
            resource_bound_scope("work_items", task_id, "watch")?,
            &request,
        )
        .await
    );
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_items WHERE project_id = $1 AND id = $2)",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_one(&mut *write.transaction)
    .await?;
    if !exists {
        return Err(AppError::NotFound("work_item"));
    }
    planning::set_watching(
        &mut write.transaction,
        project_id,
        task_id,
        principal.actor_id,
        request.watching,
    )
    .await?;
    let watchers = planning::watchers(&mut write.transaction, project_id, task_id).await?;
    // The watcher list is capped for display, so it can't say whether the
    // caller is on it; the change just made does.
    let body = WatchResponse {
        watching: request.watching,
        watchers,
    };
    write.finish(StatusCode::OK, &body).await
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyRequest {}

/// Lets go of an agent's claim on a task: the claimer, or an owner or
/// admin taking back a task someone left claimed.
pub(super) async fn release_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            Some(task_id),
            resource_bound_scope("work_items", task_id, "release_claim")?,
            &request,
        )
        .await
    );
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_items WHERE project_id = $1 AND id = $2)",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_one(&mut *write.transaction)
    .await?;
    if !exists {
        return Err(AppError::NotFound("work_item"));
    }
    let claimer = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM work_item_claims WHERE project_id = $1 AND work_item_id = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(&mut *write.transaction)
    .await?;
    if let Some(claimer) = claimer {
        if claimer != principal.actor_id && !is_manager(&write.membership)? {
            return Err(AppError::Forbidden);
        }
        sqlx::query("DELETE FROM work_item_claims WHERE project_id = $1 AND work_item_id = $2")
            .bind(project_id)
            .bind(task_id)
            .execute(&mut *write.transaction)
            .await?;
        insert_activity(
            &mut write.transaction,
            project_id,
            task_id,
            principal.actor_id,
            "work_item_updated",
            "Released the claim",
            &json!({ "claim": false, "claimedBy": claimer }),
        )
        .await?;
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM work_items WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(task_id)
        .fetch_one(&mut *write.transaction)
        .await?;
        enqueue_update(&mut write.transaction, project_id, task_id, version).await?;
    }
    let item = write.task(principal.actor_id, task_id).await?;
    write.finish(StatusCode::OK, &item).await
}

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

fn label_limit_message(limit: LabelLimit) -> String {
    match limit {
        LabelLimit::Active => format!(
            "This project already has {} labels. Archive one first.",
            work_item_labels::MAX_ACTIVE_LABELS_PER_PROJECT
        ),
        LabelLimit::Total => format!(
            "This project has kept {} labels, archived ones included, which is the most it can.",
            work_item_labels::MAX_LABEL_ROWS_PER_PROJECT
        ),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateLabelRequest {
    name: String,
    color: Option<String>,
    #[serde(default)]
    description: String,
}

pub(super) async fn create_label(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateLabelRequest>,
) -> Result<Response, AppError> {
    let name =
        work_item_labels::normalize_name(&request.name).ok_or(AppError::Validation("name"))?;
    let color = request
        .color
        .as_deref()
        .map(|color| work_item_labels::normalize_color(color).ok_or(AppError::Validation("color")))
        .transpose()?;
    let description = work_item_labels::normalize_label_description(&request.description)
        .ok_or(AppError::Validation("description"))?;
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("projects", project_id, "labels_create")?,
            &request,
        )
        .await
    );
    let outcome = work_item_labels::create_label(
        &mut write.transaction,
        project_id,
        principal.actor_id,
        NewLabel {
            name: &name,
            color: color.as_deref(),
            description: &description,
        },
    )
    .await?;
    let row = match outcome {
        CreateLabelOutcome::Created(row) => {
            record_audit_for(
                &mut write.transaction,
                &write.actor,
                "work_item_label",
                "work_item_label.created",
                row.id,
                json!({ "name": row.name, "color": row.color }),
            )
            .await?;
            row
        }
        CreateLabelOutcome::Exists(row) => row,
        CreateLabelOutcome::ArchivedNameTaken(row) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "label_archived",
                format!(
                    "An archived label is called \"{}\". Restore it, or pick another name.",
                    row.name
                ),
                json!({ "labelId": row.id }),
            ));
        }
        CreateLabelOutcome::ProjectFull(limit) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "label_limit",
                label_limit_message(limit),
                Value::Null,
            ));
        }
    };
    write
        .finish(StatusCode::CREATED, &LabelView::from(row))
        .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateLabelRequest {
    expected_version: i64,
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
    archived: Option<bool>,
}

pub(super) async fn update_label(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, label_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateLabelRequest>,
) -> Result<Response, AppError> {
    let name = request
        .name
        .as_deref()
        .map(|name| work_item_labels::normalize_name(name).ok_or(AppError::Validation("name")))
        .transpose()?;
    let color = request
        .color
        .as_deref()
        .map(|color| work_item_labels::normalize_color(color).ok_or(AppError::Validation("color")))
        .transpose()?;
    let description = request
        .description
        .as_deref()
        .map(|text| {
            work_item_labels::normalize_label_description(text)
                .ok_or(AppError::Validation("description"))
        })
        .transpose()?;
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("work_item_labels", label_id, "update")?,
            &request,
        )
        .await
    );
    let outcome = work_item_labels::change_label(
        &mut write.transaction,
        project_id,
        label_id,
        principal.actor_id,
        LabelChange {
            name: name.as_deref(),
            color: color.as_deref(),
            description: description.as_deref(),
            archived: request.archived,
            expected_version: Some(request.expected_version),
        },
    )
    .await?;
    let row = match outcome {
        ChangeLabelOutcome::Changed {
            label,
            previous,
            changed_fields,
        } => {
            let action = match (previous.archived, label.archived) {
                (false, true) => "work_item_label.archived",
                (true, false) => "work_item_label.restored",
                _ => "work_item_label.updated",
            };
            record_audit_for(
                &mut write.transaction,
                &write.actor,
                "work_item_label",
                action,
                label.id,
                json!({
                    "changedFields": changed_fields,
                    "previous": { "name": previous.name, "color": previous.color },
                    "previousVersion": previous.version,
                }),
            )
            .await?;
            label
        }
        ChangeLabelOutcome::Unchanged(row) => row,
        ChangeLabelOutcome::NotFound => return Err(AppError::NotFound("label")),
        ChangeLabelOutcome::VersionConflict(row) => {
            return Err(AppError::VersionConflict {
                resource: "label",
                current_version: row.version,
            });
        }
        ChangeLabelOutcome::NameTaken { other } => {
            return Ok(invalid(
                "name",
                format!("Another label is already called \"{other}\"."),
            ));
        }
        ChangeLabelOutcome::ProjectFull(limit) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "label_limit",
                label_limit_message(limit),
                Value::Null,
            ));
        }
    };
    write.finish(StatusCode::OK, &LabelView::from(row)).await
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
    let mut connection = state.pool.acquire().await?;
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateMilestoneRequest {
    name: String,
    #[serde(default)]
    description: String,
    target_date: Option<String>,
}

pub(super) async fn create_milestone(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateMilestoneRequest>,
) -> Result<Response, AppError> {
    let name =
        planning::normalize_milestone_name(&request.name).ok_or(AppError::Validation("name"))?;
    let description = planning::normalize_milestone_description(&request.description)
        .ok_or(AppError::Validation("description"))?;
    let target_date = parse_task_date(request.target_date.as_deref(), "targetDate")?;
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("projects", project_id, "milestones_create")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    let (id, _) = planning::create_milestone(
        &mut write.transaction,
        &actor,
        &name,
        &description,
        target_date,
    )
    .await?;
    let view = milestone_view(&mut write, project_id, id).await?;
    write.finish(StatusCode::CREATED, &view).await
}

async fn milestone_view(
    write: &mut Write,
    project_id: Uuid,
    milestone_id: Uuid,
) -> Result<MilestoneView, AppError> {
    planning::milestone(&mut write.transaction, project_id, milestone_id)
        .await?
        .ok_or(AppError::Internal("milestone just written is missing"))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateMilestoneRequest {
    expected_version: i64,
    name: Option<String>,
    description: Option<String>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    target_date: Option<Option<String>>,
    archived: Option<bool>,
}

pub(super) async fn update_milestone(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, milestone_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateMilestoneRequest>,
) -> Result<Response, AppError> {
    let change = MilestoneChange {
        name: request
            .name
            .as_deref()
            .map(|name| {
                planning::normalize_milestone_name(name).ok_or(AppError::Validation("name"))
            })
            .transpose()?,
        description: request
            .description
            .as_deref()
            .map(|text| {
                planning::normalize_milestone_description(text)
                    .ok_or(AppError::Validation("description"))
            })
            .transpose()?,
        target_date: request
            .target_date
            .as_ref()
            .map(|date| parse_task_date(date.as_deref(), "targetDate"))
            .transpose()?,
        archived: request.archived,
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("project_milestones", milestone_id, "update")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    match planning::update_milestone(
        &mut write.transaction,
        &actor,
        milestone_id,
        Some(request.expected_version),
        &change,
    )
    .await?
    {
        Ok(()) => {}
        Err(MilestoneRefusal::NotFound) => return Err(AppError::NotFound("milestone")),
        Err(MilestoneRefusal::VersionConflict { current_version }) => {
            return Err(AppError::VersionConflict {
                resource: "milestone",
                current_version,
            });
        }
        Err(MilestoneRefusal::NameTaken) => {
            return Ok(invalid("name", "Another milestone already has that name."));
        }
    }
    let view = milestone_view(&mut write, project_id, milestone_id).await?;
    write.finish(StatusCode::OK, &view).await
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
               (SELECT count(*) FROM work_items item
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
               (SELECT count(*) FROM work_items item
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
    let mut connection = state.pool.acquire().await?;
    Ok(Json(
        workflow_view(&mut connection, project_id, can_edit).await?,
    ))
}

/// The document with its replacements in a fixed order, so a retried
/// request hashes the same.
#[derive(Serialize)]
struct HashableDocument<'a> {
    document: &'a WorkflowDocument,
    replacements: BTreeMap<&'a str, &'a str>,
}

pub(super) async fn replace_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(mut document): ApiJson<WorkflowDocument>,
) -> Result<Response, AppError> {
    let replacements = std::mem::take(&mut document.replacements);
    let hashable = HashableDocument {
        document: &document,
        replacements: replacements
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect(),
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemView,
            None,
            resource_bound_scope("projects", project_id, "workflow_replace")?,
            &hashable,
        )
        .await
    );
    // Changing the workflow changes every member's board: owners and
    // admins, or a role that kept "Change the workflow".
    if !may_change_workflow(&write.membership)? {
        return Err(AppError::Forbidden);
    }
    document.replacements = replacements;
    let actor = write.actor.clone();
    match workflow::replace_workflow(&mut write.transaction, &actor, &document).await? {
        Ok(_) => {}
        Err(WorkflowRefusal::VersionConflict { current_version }) => {
            return Err(AppError::VersionConflict {
                resource: "work_item_workflow",
                current_version,
            });
        }
        Err(WorkflowRefusal::Invalid { field, message }) => return Ok(invalid(field, message)),
    }
    let view = workflow_view(&mut write.transaction, project_id, true).await?;
    write.finish(StatusCode::OK, &view).await
}

// ---------------------------------------------------------------------------
// Bulk changes

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BulkItem {
    id: Uuid,
    expected_version: i64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BulkChanges {
    status_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    assignee_actor_id: Option<Option<Uuid>>,
    priority: Option<String>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    milestone_id: Option<Option<Uuid>>,
    #[serde(default)]
    add_label_ids: Vec<Uuid>,
    #[serde(default)]
    remove_label_ids: Vec<Uuid>,
    archived: Option<bool>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    due_date: Option<Option<String>>,
    /// Custom fields ({field ID or name: value}; null clears one), checked
    /// for each task as a single edit's fieldValues are (AWD-101).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field_values: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BulkRequest {
    items: Vec<BulkItem>,
    changes: BulkChanges,
}

#[derive(Debug, Serialize)]
struct BulkProblem {
    id: Uuid,
    key: Option<String>,
    code: &'static str,
    message: String,
}

impl BulkChanges {
    fn edits_fields(&self) -> bool {
        self.status_id.is_some()
            || self.assignee_actor_id.is_some()
            || self.priority.is_some()
            || self.milestone_id.is_some()
            || !self.add_label_ids.is_empty()
            || !self.remove_label_ids.is_empty()
            || self.due_date.is_some()
            || self
                .field_values
                .as_ref()
                .is_some_and(|fields| !fields.is_empty())
    }

    /// The custom fields as written, by field ID or name.
    fn written_fields(&self) -> Vec<(String, Value)> {
        self.field_values
            .iter()
            .flatten()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect()
    }
}

/// Applies one change to many tasks, all or nothing: if any task can't take
/// it, nothing changes and every problem comes back.
pub(super) async fn bulk_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<BulkRequest>,
) -> Result<Response, AppError> {
    let changes = &request.changes;
    if request.items.is_empty() || request.items.len() > MAX_BULK_ITEMS {
        return Err(AppError::Validation("items"));
    }
    let mut seen = std::collections::HashSet::new();
    if !request.items.iter().all(|item| seen.insert(item.id)) {
        return Err(AppError::Validation("items"));
    }
    // Archiving is its own change, so a restore can't also edit fields the
    // archived task can't take.
    match (changes.archived.is_some(), changes.edits_fields()) {
        (true, true) | (false, false) => return Err(AppError::Validation("changes")),
        _ => {}
    }
    if let Some(priority) = changes.priority.as_deref() {
        validate_priority(priority)?;
    }
    if changes.add_label_ids.len() + changes.remove_label_ids.len() > super::MAX_LABEL_CHANGES
        || changes
            .add_label_ids
            .iter()
            .any(|id| changes.remove_label_ids.contains(id))
    {
        return Err(AppError::Validation("labelIds"));
    }
    let due_date = changes
        .due_date
        .as_ref()
        .map(|date| parse_task_date(date.as_deref(), "dueDate"))
        .transpose()?;
    let written_fields = changes.written_fields();
    if written_fields.len() > crate::work_item_fields::MAX_VALUES_PER_CHANGE {
        return Err(AppError::Validation("fieldValues"));
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            // Archiving is its own permission.
            if changes.archived.is_some() {
                Action::WorkItemArchive
            } else {
                Action::WorkItemEdit
            },
            None,
            resource_bound_scope("projects", project_id, "work_items_bulk")?,
            &request,
        )
        .await
    );
    let labels: Vec<Uuid> = [
        changes.add_label_ids.as_slice(),
        changes.remove_label_ids.as_slice(),
    ]
    .concat();
    if !super::labels_are_active(&mut write.transaction, project_id, &labels).await? {
        return Err(AppError::Validation("labelIds"));
    }
    let keys = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        "#,
    )
    .bind(project_id)
    .bind(request.items.iter().map(|item| item.id).collect::<Vec<_>>())
    .fetch_all(&mut *write.transaction)
    .await?
    .into_iter()
    .collect::<std::collections::HashMap<_, _>>();
    // Custom fields are checked for each task against its own values, so an
    // archived option a task already holds may stay.
    let (definitions, project_key) = if written_fields.is_empty() {
        (Vec::new(), String::new())
    } else {
        (
            crate::work_item_fields::definitions(&mut *write.transaction, project_id, true).await?,
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&mut *write.transaction)
                .await?,
        )
    };

    // One task at a time in ID order, so two bulk changes over the same
    // tasks lock them in the same order.
    let mut order = request.items.iter().collect::<Vec<_>>();
    order.sort_by_key(|item| item.id);
    let actor = write.actor.clone();
    let mut problems = Vec::new();
    for item in order {
        let key = keys.get(&item.id).cloned();
        let problem = |code: &'static str, message: &str| BulkProblem {
            id: item.id,
            key: key.clone(),
            code,
            message: message.to_owned(),
        };
        if key.is_none() {
            problems.push(problem("not_found", "This task isn't in the project."));
            continue;
        }
        if let Some(archived) = changes.archived {
            let outcome = set_archived(
                &mut write.transaction,
                &actor,
                item.id,
                ArchiveCommand {
                    expected_version: item.expected_version,
                    archived,
                    summary: if archived {
                        "Work Item archived"
                    } else {
                        "Work Item restored"
                    },
                    audit_action: if archived {
                        "work_item.archived"
                    } else {
                        "work_item.restored"
                    },
                },
            )
            .await?;
            match outcome {
                ArchiveOutcome::Applied { .. } | ArchiveOutcome::Unchanged(_) => {}
                ArchiveOutcome::NotFound => {
                    problems.push(problem("not_found", "This task isn't in the project."));
                }
                ArchiveOutcome::VersionConflict(_) => problems.push(problem(
                    "version_conflict",
                    "Someone changed this task after you opened it.",
                )),
                ArchiveOutcome::MovedAway { .. } => problems.push(problem(
                    "moved",
                    "This task moved to another project, so it can't be restored here.",
                )),
            }
            continue;
        }
        let field_values = if written_fields.is_empty() {
            None
        } else {
            let current = crate::work_item_fields::current_values(
                &mut write.transaction,
                project_id,
                item.id,
            )
            .await?;
            // A task of a collection that acts like a task has its fields.
            let own = crate::work_item_fields::definitions_for_task_or(
                &mut write.transaction,
                project_id,
                item.id,
                &definitions,
            )
            .await?;
            let resolved = crate::work_item_fields::resolve_for(
                &mut write.transaction,
                project_id,
                crate::work_item_fields::Writer {
                    actor_id: principal.actor_id,
                    project_key: &project_key,
                },
                &own,
                "fieldValues",
                &written_fields,
                current.as_ref(),
            )
            .await?;
            match resolved {
                Ok(values) => Some(values),
                Err(value_problem) => {
                    problems.push(BulkProblem {
                        id: item.id,
                        key: key.clone(),
                        code: "invalid_field",
                        message: value_problem.message,
                    });
                    continue;
                }
            }
        };
        let outcome = update_work_item_by_id(
            &mut write.transaction,
            &actor,
            item.id,
            UpdateWorkItemCommand {
                field_values: field_values.as_ref(),
                expected_version: item.expected_version,
                title: None,
                description: None,
                type_id: None,
                status_id: changes.status_id,
                priority: changes.priority.as_deref(),
                assignee_actor_id: changes.assignee_actor_id,
                parent_work_item_id: None,
                board_placement: None,
                labels: (!labels.is_empty()).then_some(LabelDelta {
                    add: &changes.add_label_ids,
                    remove: &changes.remove_label_ids,
                }),
                fields: TaskFieldChanges {
                    milestone_id: changes.milestone_id,
                    due_date,
                    ..TaskFieldChanges::default()
                },
                summary: "Work Item updated",
                audit_action: "work_item.bulk_updated",
                audit_version_metadata: AuditVersionMetadata::Include,
            },
        )
        .await?;
        match outcome {
            UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_) => {}
            UpdateWorkItemOutcome::NotFound => {
                problems.push(problem("not_found", "This task isn't in the project."));
            }
            UpdateWorkItemOutcome::Conflict(current) if current.archived => {
                problems.push(problem("archived", "This task is archived."));
            }
            UpdateWorkItemOutcome::Conflict(_) => problems.push(problem(
                "version_conflict",
                "Someone changed this task after you opened it.",
            )),
            UpdateWorkItemOutcome::InvalidAssignee => problems.push(problem(
                "invalid_assignee",
                "That person isn't an active member of the project.",
            )),
            UpdateWorkItemOutcome::InvalidTransition { .. } => problems.push(problem(
                "invalid_transition",
                "The workflow doesn't allow moving this task to that status.",
            )),
            UpdateWorkItemOutcome::TooManyLabels { limit, .. } => problems.push(problem(
                "too_many_labels",
                &format!("A task can carry at most {limit} labels."),
            )),
            UpdateWorkItemOutcome::InvalidMilestone => problems.push(problem(
                "invalid_milestone",
                "That milestone is archived or not in this project.",
            )),
            UpdateWorkItemOutcome::InvalidDates => problems.push(problem(
                "invalid_dates",
                "The due date would be before the task's start date.",
            )),
            UpdateWorkItemOutcome::InvalidParent(_)
            | UpdateWorkItemOutcome::InvalidPlacement(_) => {
                return Err(AppError::Internal("bulk change moved a task"));
            }
        }
    }
    if !problems.is_empty() {
        // Dropping the transaction undoes the tasks that did change.
        let conflict = problems
            .iter()
            .all(|problem| problem.code == "version_conflict");
        return Ok(refusal(
            if conflict {
                StatusCode::CONFLICT
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            },
            "bulk_rejected",
            format!(
                "Nothing changed: {} of {} tasks can't take this change.",
                problems.len(),
                request.items.len()
            ),
            json!({ "problems": problems }),
        ));
    }
    let rows = request
        .items
        .iter()
        .map(|item| (item.id, None))
        .collect::<Vec<_>>();
    let caps = capabilities(&write.membership)?;
    // Read inside the transaction, so the cards show the change.
    let cards = list::cards(&mut *write.transaction, project_id, &rows, caps).await?;
    write
        .finish(StatusCode::OK, &json!({ "items": cards }))
        .await
}
