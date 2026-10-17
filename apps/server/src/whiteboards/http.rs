//! The browser's whiteboard routes.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    BoardChange, BoardSummary, Caller, LinkView, ListFilter, Placement, TargetKind, access,
    edit::{self, EditError, EditOutcome},
    elements::{self, MAX_BATCH},
    render, scene, store,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
};

/// A scene write of up to MAX_BATCH elements; Excalidraw's elements are
/// small but a long freehand stroke isn't.
const SCENE_BODY_BYTES: usize = 4 * 1024 * 1024;
/// One image as a data URL (4 MiB of bytes grows by a third in base64).
const FILE_BODY_BYTES: usize = 6 * 1024 * 1024;
const OPERATIONS_BODY_BYTES: usize = 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/whiteboards", get(list_http).post(create_http))
        .route("/api/v1/whiteboards/{id}", get(get_http).patch(change_http))
        .route("/api/v1/whiteboards/{id}/duplicate", post(duplicate_http))
        .route(
            "/api/v1/whiteboards/{id}/scene",
            get(scene_http)
                .post(write_scene_http)
                .layer(DefaultBodyLimit::max(SCENE_BODY_BYTES)),
        )
        .route(
            "/api/v1/whiteboards/{id}/operations",
            post(operations_http).layer(DefaultBodyLimit::max(OPERATIONS_BODY_BYTES)),
        )
        .route(
            "/api/v1/whiteboards/{id}/files",
            post(put_file_http).layer(DefaultBodyLimit::max(FILE_BODY_BYTES)),
        )
        .route(
            "/api/v1/whiteboards/{id}/files/{file_id}",
            get(get_file_http),
        )
        .route("/api/v1/whiteboards/{id}/preview.svg", get(preview_http))
        .route(
            "/api/v1/whiteboards/{id}/revisions/{revision}/restore",
            post(restore_http),
        )
        .route("/api/v1/whiteboards/{id}/links", post(link_http))
        .route(
            "/api/v1/whiteboards/{id}/links/{kind}/{target_id}",
            delete(unlink_http),
        )
        .route("/api/v1/whiteboard-links", get(linked_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    q: Option<String>,
    organization_id: Option<Uuid>,
    project_id: Option<Uuid>,
    deleted: Option<bool>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    id: Uuid,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    whiteboards: Vec<BoardSummary>,
    /// Workspaces the viewer can make boards in.
    workspaces: Vec<Workspace>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<ListResponse>, AppError> {
    let caller = Caller::browser(&principal)?;
    let whiteboards = super::list(
        &state.pool,
        &caller,
        &ListFilter {
            organization_id: query.organization_id,
            project_id: query.project_id,
            query: query.q,
            deleted: query.deleted.unwrap_or(false),
            limit: 200,
        },
    )
    .await?;
    let workspaces = sqlx::query_as::<_, Workspace>(
        r#"
        SELECT organization.id, organization.name
        FROM organization_memberships membership
        JOIN organizations organization ON organization.id = membership.organization_id
        WHERE membership.actor_id = $1 AND membership.enabled
        ORDER BY lower(organization.name), organization.id
        "#,
    )
    .bind(caller.actor_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ListResponse {
        whiteboards,
        workspaces,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkTarget {
    target_kind: String,
    target_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    title: String,
    /// The workspace for a board everyone there sees.
    organization_id: Option<Uuid>,
    /// The project for a board only its members see.
    project_id: Option<Uuid>,
    /// Link the new board to this task, project or page straight away.
    link: Option<LinkTarget>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardDetail {
    whiteboard: BoardSummary,
    links: Vec<LinkView>,
    can_edit: bool,
    revisions: Vec<store::RevisionSummary>,
}

async fn detail(
    state: &AppState,
    caller: &Caller,
    whiteboard_id: Uuid,
) -> Result<BoardDetail, AppError> {
    let mut connection = state.pool.acquire().await?;
    let board = access(&mut connection, caller, whiteboard_id, false).await?;
    drop(connection);
    Ok(BoardDetail {
        whiteboard: super::summary(&state.pool, whiteboard_id).await?,
        links: super::links(&state.pool, caller, whiteboard_id).await?,
        can_edit: board.can_edit,
        revisions: store::revisions(&state.pool, whiteboard_id, 30).await?,
    })
}

fn target_kind(value: &str) -> Result<TargetKind, AppError> {
    TargetKind::parse(value).ok_or(AppError::Validation("targetKind"))
}

async fn create_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CreateRequest>,
) -> Result<Json<BoardDetail>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let placement = match (request.project_id, request.organization_id) {
        (Some(project_id), _) => Placement::Project(project_id),
        (None, Some(organization_id)) => Placement::Workspace(organization_id),
        (None, None) => return Err(AppError::Validation("organizationId")),
    };
    let mut transaction = state.pool.begin().await?;
    let id = super::create(&mut transaction, &caller, placement, &request.title).await?;
    if let Some(link) = request.link {
        super::set_link(
            &mut transaction,
            &caller,
            id,
            target_kind(&link.target_kind)?,
            link.target_id,
            true,
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(Json(detail(&state, &caller, id).await?))
}

async fn get_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<BoardDetail>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(detail(&state, &caller, id).await?))
}

async fn change_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(change): ApiJson<BoardChange>,
) -> Result<Json<BoardDetail>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    super::change(&mut transaction, &caller, id, &change).await?;
    transaction.commit().await?;
    Ok(Json(detail(&state, &caller, id).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DuplicateRequest {
    title: Option<String>,
}

async fn duplicate_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<DuplicateRequest>,
) -> Result<Json<BoardDetail>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let copy = super::duplicate(&mut transaction, &caller, id, request.title.as_deref()).await?;
    transaction.commit().await?;
    Ok(Json(detail(&state, &caller, copy).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SceneQuery {
    since: Option<i64>,
}

async fn scene_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SceneQuery>,
) -> Result<Json<store::SceneView>, AppError> {
    let caller = Caller::browser(&principal)?;
    let mut connection = state.pool.acquire().await?;
    access(&mut connection, &caller, id, false).await?;
    drop(connection);
    Ok(Json(store::scene(&state.pool, id, query.since).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SceneWrite {
    elements: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SceneWriteResponse {
    #[serde(flatten)]
    outcome: store::WriteOutcome,
    /// Why an element was refused, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    refused: Option<String>,
}

async fn write_scene_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(write): ApiJson<SceneWrite>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    if write.elements.len() > MAX_BATCH {
        return Err(AppError::PayloadTooLarge);
    }
    let parsed = match write
        .elements
        .into_iter()
        .map(elements::parse)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(parsed) => parsed,
        Err(refusal) => {
            let body = serde_json::json!({
                "error": { "code": "validation_failed", "message": refusal, "field": "elements" }
            });
            return Ok((axum::http::StatusCode::BAD_REQUEST, Json(body)).into_response());
        }
    };
    let mut transaction = state.pool.begin().await?;
    let board = access(&mut transaction, &caller, id, true).await?;
    if board.deleted {
        return Err(AppError::Conflict(
            "whiteboard_deleted",
            "This board is in the trash. Restore it to draw on it.",
        ));
    }
    let scene_seq = store::lock(&mut transaction, id).await?;
    // A big removal (clearing the canvas) is saved first, so it can be undone.
    let removing = parsed.iter().filter(|element| element.is_deleted).count();
    store::save_revision(
        &mut transaction,
        &caller,
        id,
        removing >= 10,
        (removing >= 10).then_some("Before elements were removed"),
    )
    .await?;
    let outcome = store::write_elements(&mut transaction, &caller, id, scene_seq, parsed).await?;
    transaction.commit().await?;
    Ok(Json(SceneWriteResponse {
        outcome,
        refused: None,
    })
    .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationsRequest {
    operations: Vec<scene::Operation>,
    expected_scene_seq: Option<i64>,
    confirm_delete: Option<bool>,
}

/// Agents' operations over plain HTTP, for scripts and services that act as
/// a signed-in person (the MCP tool whiteboard_edit runs the same code).
async fn operations_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<OperationsRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let board = access(&mut transaction, &caller, id, true).await?;
    if board.deleted {
        return Err(AppError::Conflict(
            "whiteboard_deleted",
            "This board is in the trash. Restore it to draw on it.",
        ));
    }
    let result: Result<EditOutcome, EditError> = edit::apply(
        &mut transaction,
        &caller,
        id,
        &request.operations,
        request.expected_scene_seq,
        request.confirm_delete.unwrap_or(false),
    )
    .await;
    match result {
        Ok(outcome) => {
            transaction.commit().await?;
            Ok(Json(outcome).into_response())
        }
        Err(EditError::App(error)) => Err(error),
        Err(EditError::Invalid(message)) => Ok((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "code": "validation_failed", "message": message, "field": "operations" } })),
        )
            .into_response()),
        Err(EditError::Conflict(message)) => Ok((
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": { "code": "conflict", "message": message } })),
        )
            .into_response()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileUpload {
    id: String,
    mime_type: String,
    #[serde(rename = "dataURL")]
    data_url: String,
    /// Excalidraw's own timestamps; the server keeps its own.
    created: Option<i64>,
    last_retrieved: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileStored {
    id: String,
    stored: bool,
}

async fn put_file_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(upload): ApiJson<FileUpload>,
) -> Result<Json<FileStored>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let _ = (upload.created, upload.last_retrieved);
    let caller = Caller::browser(&principal)?;
    if !store::allowed_mime(&upload.mime_type) {
        return Err(AppError::Validation("mimeType"));
    }
    let data = store::decode_data_url(&upload.data_url, &upload.mime_type)?;
    let mut transaction = state.pool.begin().await?;
    access(&mut transaction, &caller, id, true).await?;
    let stored = store::put_file(
        &mut transaction,
        &caller,
        id,
        &upload.id,
        &upload.mime_type,
        &data,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(FileStored {
        id: upload.id,
        stored,
    }))
}

async fn get_file_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((id, file_id)): ApiPath<(Uuid, String)>,
) -> Result<Json<store::FileView>, AppError> {
    let caller = Caller::browser(&principal)?;
    if !elements::valid_id(&file_id) {
        return Err(AppError::NotFound("whiteboard file"));
    }
    let mut connection = state.pool.acquire().await?;
    access(&mut connection, &caller, id, false).await?;
    drop(connection);
    Ok(Json(store::get_file(&state.pool, id, &file_id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewQuery {
    dark: Option<bool>,
    /// The board's sceneSeq, so a changed board gets a new address.
    #[serde(rename = "v")]
    _version: Option<i64>,
}

/// A small SVG of the board for lists. Served as an image with a policy that
/// runs nothing, even if opened directly.
async fn preview_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<PreviewQuery>,
) -> Result<Response, AppError> {
    let caller = Caller::browser(&principal)?;
    let mut connection = state.pool.acquire().await?;
    access(&mut connection, &caller, id, false).await?;
    drop(connection);
    let elements = store::live_elements(&state.pool, id).await?;
    let files = store::files_for(&state.pool, id, &elements).await?;
    let rendered = render::svg(
        &elements,
        &files,
        &render::Options {
            only: None,
            background: false,
            dark: query.dark.unwrap_or(false),
        },
    );
    let mut response = rendered.svg.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox",
        ),
    );
    Ok(response)
}

async fn restore_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((id, revision)): ApiPath<(Uuid, i32)>,
) -> Result<Json<store::WriteOutcome>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let board = access(&mut transaction, &caller, id, true).await?;
    let outcome = store::restore_revision(&mut transaction, &caller, id, revision).await?;
    super::audit(
        &mut transaction,
        &caller,
        board.organization_id,
        board.project_id,
        "whiteboard.revision_restored",
        id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(outcome))
}

async fn link_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(target): ApiJson<LinkTarget>,
) -> Result<Json<BoardDetail>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    super::set_link(
        &mut transaction,
        &caller,
        id,
        target_kind(&target.target_kind)?,
        target.target_id,
        true,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(detail(&state, &caller, id).await?))
}

async fn unlink_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((id, kind, target_id)): ApiPath<(Uuid, String, Uuid)>,
) -> Result<Json<BoardDetail>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    let mut transaction = state.pool.begin().await?;
    super::set_link(
        &mut transaction,
        &caller,
        id,
        target_kind(&kind)?,
        target_id,
        false,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(detail(&state, &caller, id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkedQuery {
    target_kind: String,
    target_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedResponse {
    whiteboards: Vec<BoardSummary>,
}

async fn linked_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<LinkedQuery>,
) -> Result<Json<LinkedResponse>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(LinkedResponse {
        whiteboards: super::linked_to(
            &state.pool,
            &caller,
            target_kind(&query.target_kind)?,
            query.target_id,
        )
        .await?,
    }))
}
