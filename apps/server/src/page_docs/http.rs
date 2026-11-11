//! The browser's routes for a page's live document (AWD-53 slice 3); they're
//! registered next to the other page routes in `memory::router`.
//!
//! - `GET /api/v1/pages/{id}/document`: the whole document (converting the
//!   page if it hasn't one yet), the page's version, and whether the caller
//!   may edit it.
//! - `POST /api/v1/pages/{id}/document/sync`: what the caller's copy lacks,
//!   given its state vector. A read: anyone who can see the page may sync.
//! - `POST /api/v1/pages/{id}/document/updates`: the caller's changes, with
//!   what its copy lacks in the answer.
//!
//! Open editors hear that a page changed from `page.updated` on the realtime
//! channel, and sync. Documents and state vectors travel as base64 Yjs
//! updates; a copy from another generation of the document is told to load
//! the page again rather than merge.

use axum::{Json, extract::State, http::HeaderMap};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{store, ydoc};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write, require_csrf},
    conversations::parse_actor_kind,
    error::{ApiJson, ApiPath, AppError},
    memory,
    policy::Action,
};

/// The largest request the updates route takes: a change at its largest, in
/// base64, with room for the state vector.
pub const MAX_UPDATE_REQUEST_BYTES: usize = 3 * 1024 * 1024;
/// The largest state vector a browser may send, in bytes.
const MAX_STATE_VECTOR_BYTES: usize = 256 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    pub page_id: Uuid,
    pub generation: Uuid,
    pub version: i64,
    /// The whole document, as a base64 Yjs update.
    pub state: String,
    /// Whether the caller may change the page (an archived page needs
    /// bringing back first).
    pub can_edit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRequest {
    pub generation: Uuid,
    /// The caller's state vector, base64.
    pub state_vector: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRequest {
    pub generation: Uuid,
    /// The caller's changes the server may lack, as a base64 Yjs update.
    pub update: String,
    /// The caller's state vector after those changes, base64.
    pub state_vector: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResponse {
    /// `synced`: `update` holds what the caller lacked. `resync`: the
    /// caller's update builds on changes the server lacks; send what
    /// `stateVector` (the server's) is missing. `replaced`: the page's
    /// document was rebuilt; load it again.
    pub status: &'static str,
    pub generation: Uuid,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_vector: Option<String>,
}

fn decode(value: &str, max_bytes: usize, field: &'static str) -> Result<Vec<u8>, AppError> {
    if value.len() > max_bytes.div_ceil(3) * 4 {
        return Err(AppError::PayloadTooLarge);
    }
    STANDARD
        .decode(value)
        .map_err(|_| AppError::Validation(field))
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

pub async fn get_document(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
) -> Result<Json<DocumentView>, AppError> {
    let project_id = page_access(&state, &principal, page_id, Action::ProjectView).await?;
    let can_edit = memory::project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        Action::PageEdit,
    )
    .await
    .is_ok();
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM project_pages WHERE id = $1",
    )
    .bind(page_id)
    .fetch_one(&state.pool)
    .await?;
    let snapshot = store::open(&state.pool, page_id).await?;
    Ok(Json(DocumentView {
        page_id,
        generation: snapshot.generation,
        version: snapshot.version,
        state: STANDARD.encode(&snapshot.state),
        can_edit: can_edit && !archived,
    }))
}

pub async fn sync_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SyncRequest>,
) -> Result<Json<SyncResponse>, AppError> {
    // A read, so it doesn't count against the caller's writes; it's a POST
    // only because a state vector can outgrow a URL.
    require_csrf(&state.config, &headers, &principal)?;
    let since = decode(&request.state_vector, MAX_STATE_VECTOR_BYTES, "stateVector")?;
    page_access(&state, &principal, page_id, Action::ProjectView).await?;
    let snapshot = store::open(&state.pool, page_id).await?;
    if snapshot.generation != request.generation {
        return Ok(Json(replaced(&snapshot.generation, snapshot.version)));
    }
    let (update, state_vector) = store::missing(&snapshot.state, &since)?;
    Ok(Json(SyncResponse {
        status: "synced",
        generation: snapshot.generation,
        version: snapshot.version,
        update: Some(STANDARD.encode(update)),
        state_vector: Some(STANDARD.encode(state_vector)),
    }))
}

fn replaced(generation: &Uuid, version: i64) -> SyncResponse {
    SyncResponse {
        status: "replaced",
        generation: *generation,
        version,
        update: None,
        state_vector: None,
    }
}

pub async fn update_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<SyncResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = memory::browser_writer(&principal)?;
    let update = decode(&request.update, store::MAX_UPDATE_BYTES, "update")?;
    let since = decode(&request.state_vector, MAX_STATE_VECTOR_BYTES, "stateVector")?;
    let project_id = memory::page_project(&state.pool, page_id).await?;
    let mut transaction = state.pool.begin().await?;
    memory::project_access(
        &mut *transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    let page = store::lock_page(&mut transaction, page_id)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    if page.archived {
        return Err(AppError::Refused {
            field: "page",
            message: "This page is archived. Bring it back to edit it.",
        });
    }
    let stored = store::document(&mut transaction, page_id, &page).await?;
    if stored.generation != request.generation {
        transaction.commit().await?;
        return Ok(Json(replaced(&stored.generation, page.version)));
    }
    let applied = match store::apply_browser_update(&stored.state, &page.body, &update, &since)? {
        Ok(applied) => applied,
        Err(ydoc::Refused::Resync) => {
            let state_vector = store::state_vector_of(&stored.state)?;
            transaction.commit().await?;
            return Ok(Json(SyncResponse {
                status: "resync",
                generation: stored.generation,
                version: page.version,
                update: None,
                state_vector: Some(STANDARD.encode(state_vector)),
            }));
        }
        Err(ydoc::Refused::Unreadable) => return Err(AppError::Validation("update")),
        Err(ydoc::Refused::Invalid(_)) => {
            return Err(AppError::Refused {
                field: "update",
                message: "That change can't be saved: it isn't one this editor makes. Reload the page and try again.",
            });
        }
    };
    let (version, new_version) = store::save(
        &mut transaction,
        writer,
        page_id,
        &page,
        &applied.change,
        None,
        store::History::Live,
    )
    .await?;
    if new_version {
        memory::audit(
            &mut transaction,
            &principal,
            project_id,
            "page.updated",
            "page",
            page_id,
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(Json(SyncResponse {
        status: "synced",
        generation: stored.generation,
        version,
        update: Some(STANDARD.encode(applied.reply)),
        state_vector: Some(STANDARD.encode(applied.state_vector)),
    }))
}
