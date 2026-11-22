//! The browser's read of a page's comments and suggestions (AWD-53);
//! registered next to the other page routes in `memory::router`.
//!
//! - `GET /api/v1/pages/{id}/discussion`: the page's comment threads and
//!   suggestions, and what the caller may do with them.
//!
//! Commenting, editing, resolving and deciding suggestions are the
//! `page_comment_create`, `page_comment_update` and `page_suggestion_decide`
//! operations (K2.22).

use axum::{Json, extract::State};
use serde::Serialize;
use uuid::Uuid;

use super::{
    comments::{self, ThreadView},
    suggestions::{self, StoredEdit, SuggestionRow},
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
    memory,
    policy::Action,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionView {
    #[serde(flatten)]
    pub row: SuggestionRow,
    pub edits: Vec<StoredEdit>,
}

impl From<SuggestionRow> for SuggestionView {
    fn from(row: SuggestionRow) -> Self {
        let edits = row.edits.0.clone();
        Self { row, edits }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionView {
    pub page_id: Uuid,
    pub threads: Vec<ThreadView>,
    pub suggestions: Vec<SuggestionView>,
    /// Whether the caller may comment, resolve and decide suggestions.
    pub can_comment: bool,
    pub viewer_id: Uuid,
}

/// The page's project, when the caller may take `action` there; a page they
/// can't see is not found.
async fn page_access(
    state: &AppState,
    principal: &AuthenticatedActor,
    page_id: Uuid,
    action: Action,
) -> Result<Uuid, AppError> {
    let project_id = memory::page_project(&state.pool, page_id, principal.actor_id).await?;
    memory::project_access(&state.pool, principal.actor_id, project_id, action)
        .await
        .map_err(|_| AppError::NotFound("page"))?;
    Ok(project_id)
}

pub async fn get_discussion(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
) -> Result<Json<DiscussionView>, AppError> {
    let project_id = page_access(&state, &principal, page_id, Action::ProjectView).await?;
    let can_comment = memory::project_access(
        &state.pool,
        principal.actor_id,
        project_id,
        Action::PageEdit,
    )
    .await
    .is_ok();
    let blocks = comments::current_blocks(&state.pool, page_id).await?;
    let threads = comments::list(&state.pool, page_id, blocks.as_deref(), true).await?;
    let suggestions = suggestions::list(&state.pool, page_id, true)
        .await?
        .into_iter()
        .map(SuggestionView::from)
        .collect();
    Ok(Json(DiscussionView {
        page_id,
        threads,
        suggestions,
        can_comment,
        viewer_id: principal.actor_id,
    }))
}
