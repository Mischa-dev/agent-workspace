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
    comments::{self, CommentChange, Commenter, NewComment, ThreadView},
    suggestions::{self, Decision, StoredEdit, SuggestionRow},
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    idempotency::IdempotencyKey,
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

pub async fn create_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CommentRequest>,
) -> Result<Json<CommentCreated>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let project_id = page_access(&state, &principal, page_id, Action::PageEdit).await?;
    let (actor, creator) = crate::work_items::comments::browser_poster(&principal, &headers)?;
    let (id, thread_id) = comments::create(
        &state,
        Commenter { actor, creator },
        page_id,
        NewComment {
            body: &request.body,
            block_id: request.block_id.as_deref(),
            quote: request.quote.as_deref(),
            reply_to: request.reply_to,
            key: IdempotencyKey::from_headers(&headers).ok(),
        },
    )
    .await?
    .map_err(comments::Refusal::into_error)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    memory::audit(
        &mut transaction,
        &principal,
        project_id,
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

async fn change_comment(
    state: &AppState,
    principal: &AuthenticatedActor,
    page_id: Uuid,
    comment_id: Uuid,
    change: CommentChange<'_>,
    action: &str,
) -> Result<Json<CommentChanged>, AppError> {
    let project_id = page_access(state, principal, page_id, Action::PageEdit).await?;
    let actor = crate::work_items::comments::browser_message_actor(principal);
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let thread_id = comments::change(state, &mut transaction, &actor, page_id, comment_id, change)
        .await?
        .map_err(comments::Refusal::into_error)?;
    memory::audit(
        &mut transaction,
        principal,
        project_id,
        action,
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(CommentChanged { thread_id }))
}

pub async fn update_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((page_id, comment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CommentPatch>,
) -> Result<Json<CommentChanged>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let change = match (&request.body, request.resolved) {
        (Some(body), None) => CommentChange::Edit(body),
        (None, Some(resolved)) => CommentChange::Resolve(resolved),
        _ => return Err(AppError::Validation("body")),
    };
    change_comment(
        &state,
        &principal,
        page_id,
        comment_id,
        change,
        "page.comment_updated",
    )
    .await
}

pub async fn delete_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((page_id, comment_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<CommentChanged>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    change_comment(
        &state,
        &principal,
        page_id,
        comment_id,
        CommentChange::Delete,
        "page.comment_deleted",
    )
    .await
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let writer = memory::browser_writer(&principal)?;
    let project_id = memory::page_project(&state.pool, page_id, principal.actor_id).await?;
    // Anyone who may at least suggest here may withdraw their own;
    // accepting and rejecting need edit access to the page (K1.12).
    let caller = crate::access::Caller::person(writer.actor_id, writer.actor_kind);
    let _may_write = crate::access::write_mode(
        &mut transaction,
        &caller,
        "page",
        project_id,
        true,
        Some("pages"),
    )
    .await?
    .map_err(|_| AppError::NotFound("page"))?;
    let creator = suggestions::page_creator(&mut transaction, page_id).await?;
    let may_edit = crate::access::write_mode(
        &mut transaction,
        &caller,
        "page",
        project_id,
        creator == Some(writer.actor_id),
        Some("pages"),
    )
    .await?
    .is_ok_and(|grant| grant.mode == crate::access::WriteMode::Direct);
    let decided = suggestions::decide(
        &mut transaction,
        writer,
        may_edit,
        project_id,
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
