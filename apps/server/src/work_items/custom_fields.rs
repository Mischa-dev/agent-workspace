//! A project's task fields (AWD-89) for the web: everyone in the project
//! who can see tasks reads them. Adding and changing them are the
//! `field_create` and `field_update` operations; the rules live in
//! `crate::fields`.

use axum::{Json, extract::State};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    authorize_work_item,
    planning::{IncludeArchivedQuery, may_change_workflow},
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
