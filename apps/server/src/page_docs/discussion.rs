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
    access::sharing::{self, ShareLevel},
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

pub async fn get_discussion(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
) -> Result<Json<DiscussionView>, AppError> {
    let project_id = memory::page_project(&state.pool, page_id, principal.actor_id).await?;
    // A member of its space reads the discussion; someone it was shared
    // with on its own (K1.32) only when the share lets them comment.
    let can_comment = if memory::project_access(
        &state.pool,
        principal.actor_id,
        project_id,
        Action::ProjectView,
    )
    .await
    .is_ok()
    {
        memory::project_access(
            &state.pool,
            principal.actor_id,
            project_id,
            Action::PageEdit,
        )
        .await
        .is_ok()
    } else if sharing::page_allows(
        &state.pool,
        principal.actor_id,
        page_id,
        ShareLevel::Comment,
    )
    .await?
    {
        true
    } else {
        return Err(AppError::NotFound("page"));
    };
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
