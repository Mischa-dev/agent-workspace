//! A project's task fields (AWD-89) for the web: everyone in the project
//! who can see tasks reads them. Adding and changing them are the
//! `field_create` and `field_update` operations; the rules live in
//! `crate::fields`.

use axum::{Json, extract::State, response::Response};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    authorize_work_item,
    planning::{IncludeArchivedQuery, invalid, may_change_workflow},
    project_membership,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    fields::FieldView,
    policy::Action,
    work_item_fields,
};

pub(super) async fn list_fields(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<IncludeArchivedQuery>,
) -> Result<Json<Value>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let definitions = work_item_fields::definitions(
        &state.pool,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?;
    // Defining fields is the workflow's permission (AWD-74 roles), as the
    // writes below and field_write check it; editing tasks isn't needed.
    let can_manage = may_change_workflow(&membership)? && membership.project_archived_at.is_none();
    Ok(Json(json!({
        "fields": definitions.iter().map(FieldView::from).collect::<Vec<_>>(),
        "canManage": can_manage,
    })))
}

/// The web app's `fieldValues` ({field ID or name: value}), checked and
/// resolved in this transaction, or the refusal to send. `task` is the
/// task being changed (None for a new one), so archived options it already
/// holds may stay chosen.
pub(super) async fn web_field_values(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    written: &serde_json::Map<String, Value>,
    task: Option<Uuid>,
) -> Result<Result<work_item_fields::FieldValueChanges, Response>, AppError> {
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    // A task of a collection that acts like a task follows its fields.
    let definitions = match task {
        Some(task) => {
            work_item_fields::definitions_for_task(&mut *connection, project_id, task, true).await?
        }
        None => work_item_fields::definitions(&mut *connection, project_id, true).await?,
    };
    let written = written
        .iter()
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<Vec<_>>();
    let current = match task {
        Some(task) => work_item_fields::current_values(&mut *connection, project_id, task).await?,
        None => None,
    };
    Ok(work_item_fields::resolve_for(
        connection,
        project_id,
        work_item_fields::Writer {
            actor_id,
            project_key: &project_key,
        },
        &definitions,
        "fieldValues",
        &written,
        current.as_ref(),
    )
    .await?
    .map_err(|problem| invalid(&problem.field, problem.message)))
}
