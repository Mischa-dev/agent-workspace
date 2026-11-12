//! The browser's routes for a page's comments and suggestions (AWD-53);
//! registered next to the other page routes in `memory::router`.
//!
//! - `GET /api/v1/pages/{id}/discussion`: the page's comment threads and
//!   suggestions, and what the caller may do with them.
//! - `POST /api/v1/pages/{id}/comments`: a new thread (maybe anchored to
//!   words in a block) or a reply.
//! - `PATCH /api/v1/pages/{id}/comments/{comment_id}`: edit your comment,
//!   or resolve or reopen its thread.
//! - `DELETE /api/v1/pages/{id}/comments/{comment_id}`: delete your comment.
//! - `POST /api/v1/pages/{id}/suggestions/{suggestion_id}/decision`: accept
//!   or reject an agent's suggested edits.

use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    comments::{self, CommentChange, NewComment, ThreadView},
    suggestions::{self, Decision, StoredEdit, SuggestionRow},
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::parse_actor_kind,
    error::{ApiJson, ApiPath, AppError},
    memory,
    page_docs::store,
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
    let project_id = memory::page_project(&state.pool, page_id).await?;
    memory::project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        action,
    )
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
        parse_actor_kind(&principal.actor_kind)?,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentRequest {
    pub body: String,
    pub block_id: Option<String>,
    pub quote: Option<String>,
    pub reply_to: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentCreated {
    pub id: Uuid,
    pub thread_id: Uuid,
}

/// The page locked for a write by someone who may edit it.
async fn locked_for_comment(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &AuthenticatedActor,
    page_id: Uuid,
) -> Result<(memory::Writer, store::LockedPage), AppError> {
    let writer = memory::browser_writer(principal)?;
    let project_id = memory::page_project(&state.pool, page_id).await?;
    memory::project_access(
        &mut **transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    let page = store::lock_page(transaction, page_id)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    Ok((writer, page))
}

pub async fn create_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CommentRequest>,
) -> Result<Json<CommentCreated>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let (writer, page) = locked_for_comment(&state, &mut transaction, &principal, page_id).await?;
    let (id, thread_id) = comments::create(
        &mut transaction,
        writer,
        page_id,
        &page,
        &NewComment {
            body: &request.body,
            block_id: request.block_id.as_deref(),
            quote: request.quote.as_deref(),
            reply_to: request.reply_to,
        },
    )
    .await?
    .map_err(comments::Refusal::into_error)?;
    memory::audit(
        &mut transaction,
        &principal,
        page.project_id,
        "page.comment_created",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(CommentCreated { id, thread_id }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentPatch {
    pub body: Option<String>,
    pub resolved: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentChanged {
    pub thread_id: Uuid,
}

pub async fn update_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((page_id, comment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CommentPatch>,
) -> Result<Json<CommentChanged>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let change = match (&request.body, request.resolved) {
        (Some(body), None) => CommentChange::Edit(body),
        (None, Some(resolved)) => CommentChange::Resolve(resolved),
        _ => return Err(AppError::Validation("body")),
    };
    let mut transaction = state.pool.begin().await?;
    let (writer, page) = locked_for_comment(&state, &mut transaction, &principal, page_id).await?;
    let thread_id = comments::change(
        &mut transaction,
        writer,
        page.project_id,
        page_id,
        comment_id,
        change,
    )
    .await?
    .map_err(comments::Refusal::into_error)?;
    memory::audit(
        &mut transaction,
        &principal,
        page.project_id,
        "page.comment_updated",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(CommentChanged { thread_id }))
}

pub async fn delete_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((page_id, comment_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<CommentChanged>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let (writer, page) = locked_for_comment(&state, &mut transaction, &principal, page_id).await?;
    let thread_id = comments::change(
        &mut transaction,
        writer,
        page.project_id,
        page_id,
        comment_id,
        CommentChange::Delete,
    )
    .await?
    .map_err(comments::Refusal::into_error)?;
    memory::audit(
        &mut transaction,
        &principal,
        page.project_id,
        "page.comment_deleted",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(CommentChanged { thread_id }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionRequest {
    pub decision: Decision,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    pub status: &'static str,
    pub version: i64,
}

pub async fn decide_suggestion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((page_id, suggestion_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<DecisionRequest>,
) -> Result<Json<DecisionView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let writer = memory::browser_writer(&principal)?;
    let project_id = memory::page_project(&state.pool, page_id).await?;
    memory::project_access(
        &mut *transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    let decided = suggestions::decide(
        &mut transaction,
        writer,
        page_id,
        suggestion_id,
        request.decision,
    )
    .await?
    .map_err(suggestions::Refusal::into_error)?;
    memory::audit(
        &mut transaction,
        &principal,
        project_id,
        match decided.status {
            "accepted" => "page.suggestion_accepted",
            "rejected" => "page.suggestion_rejected",
            _ => "page.suggestion_withdrawn",
        },
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(DecisionView {
        status: decided.status,
        version: decided.version,
    }))
}
