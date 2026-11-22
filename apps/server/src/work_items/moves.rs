//! HTTP routes for moving a task, with its subtasks, to another project
//! (AWD-102): a preview that changes nothing, then one confirmation with the
//! mover's choices. The rules, checks and the move itself are
//! `crate::work_item_move`, which `task_move` uses too.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{browser_actor, planning::refusal};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    work_item_move::{self, Mover, Refusal, Surface},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MovePreviewQuery {
    target_project_id: Uuid,
}

fn mover(
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    source_project_id: Uuid,
    target_project_id: Uuid,
) -> Result<Mover, AppError> {
    Ok(Mover {
        actor_id: principal.actor_id,
        external_agent_connection_id: None,
        via_grant: None,
        confined_to: None,
        source: browser_actor(headers, principal, source_project_id)?,
        target: browser_actor(headers, principal, target_project_id)?,
        surface: Surface::Web,
        poster: Some(super::comments::browser_poster(principal, headers)?),
    })
}

fn refused(refusal_reason: Refusal) -> Response {
    match refusal_reason {
        Refusal::NotFound(_) => AppError::NotFound("work_item").into_response(),
        Refusal::Denied(message) => {
            refusal(StatusCode::FORBIDDEN, "forbidden", message, json!(null))
        }
        Refusal::Invalid { field, message } => refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            message,
            json!({ "field": field }),
        ),
        Refusal::Conflict(message) => {
            refusal(StatusCode::CONFLICT, "conflict", message, json!(null))
        }
        Refusal::ModuleOff => refusal(
            StatusCode::FORBIDDEN,
            "module_off",
            "Tasks is turned off in that project.".to_owned(),
            json!(null),
        ),
    }
}

/// What moving this task to `targetProjectId` would do, for the person
/// asking only. Nothing changes.
pub(super) async fn preview_move(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<MovePreviewQuery>,
) -> Result<Response, AppError> {
    let mover = mover(&headers, &principal, project_id, query.target_project_id)?;
    Ok(
        match work_item_move::preview(
            &state,
            &mover,
            project_id,
            work_item_id,
            query.target_project_id,
        )
        .await?
        {
            Ok(plan) => Json(plan).into_response(),
            Err(refusal_reason) => refused(refusal_reason),
        },
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DestinationsView {
    projects: Vec<work_item_move::Destination>,
}

/// The projects this task could move into, for the person asking, by their
/// actual permissions there (custom roles included). It grants nothing; the
/// preview and the move check everything again.
pub(super) async fn move_destinations(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let mover = mover(&headers, &principal, project_id, project_id)?;
    Ok(
        match work_item_move::destinations(&state, &mover, project_id, work_item_id).await? {
            Ok(projects) => Json(DestinationsView { projects }).into_response(),
            Err(refusal_reason) => refused(refusal_reason),
        },
    )
}
