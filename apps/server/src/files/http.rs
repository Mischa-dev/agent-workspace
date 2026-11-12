//! The browser's file routes: upload into a conversation or a task, list a
//! task's files, read a file or its preview, and remove one.
//!
//! Uploads are the raw bytes as the request body, with the file's name in
//! `?name=` and its type in `Content-Type`, so a large file is never
//! base64-encoded or held twice.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    Destination, FileRow, FileStore, FileView, MAX_UPLOAD_BYTES, Reader, Upload, readable_file,
    remove_file, served_content_type, store_upload, task_files, upload_limit,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{MessageCommandActor, lock_message_conversation, require_active_conversation},
    error::{ApiPath, ApiQuery, AppError},
    policy::Action,
    work_items::{authorize_task_change, authorize_task_view},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/conversations/{id}/files",
            post(upload_to_conversation).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/files",
            get(list_task_files)
                .post(upload_to_task)
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/v1/files/{id}", delete(remove))
        .route("/api/v1/files/{id}/content", get(content))
        .route("/api/v1/files/{id}/thumbnail", get(thumbnail))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UploadQuery {
    name: String,
    /// `comment`: for a comment on the task, attached when it is posted.
    #[serde(default, rename = "for")]
    purpose: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentQuery {
    download: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskFilesResponse {
    files: Vec<FileView>,
}

async fn upload_to_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if query.purpose.is_some() {
        return Err(AppError::Validation("for"));
    }
    let actor = MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
    };
    let mut transaction = state.pool.begin().await?;
    let conversation = lock_message_conversation(
        &mut transaction,
        &actor,
        conversation_id,
        Action::MessageSend,
    )
    .await?;
    require_active_conversation(&conversation)?;
    let limit = upload_limit(&mut transaction, conversation.project_id).await?;
    // Nothing stays locked while the bytes arrive.
    transaction.commit().await?;
    let bytes = read_body(body, limit).await?;
    let row = store_upload(
        &state,
        principal.actor_id,
        Destination::Conversation {
            project_id: conversation.project_id,
            conversation_id: conversation.id,
        },
        Upload {
            name: query.name,
            declared_type: declared_type(&headers),
            bytes,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(row.view())).into_response())
}

async fn upload_to_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let for_comment = match query.purpose.as_deref() {
        None => false,
        Some("comment") => true,
        Some(_) => return Err(AppError::Validation("for")),
    };
    let mut transaction = state.pool.begin().await?;
    authorize_task_change(&mut transaction, &principal, project_id, work_item_id).await?;
    let limit = upload_limit(&mut transaction, project_id).await?;
    transaction.commit().await?;
    let bytes = read_body(body, limit).await?;
    let row = store_upload(
        &state,
        principal.actor_id,
        Destination::Task {
            project_id,
            work_item_id,
            for_comment,
        },
        Upload {
            name: query.name,
            declared_type: declared_type(&headers),
            bytes,
        },
    )
    .await?;
    if !for_comment {
        crate::work_items::notify_files_changed(&state.pool, project_id, work_item_id).await?;
    }
    Ok((StatusCode::CREATED, Json(row.view())).into_response())
}

async fn list_task_files(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<TaskFilesResponse>, AppError> {
    authorize_task_view(&state.pool, &principal, project_id, work_item_id).await?;
    let rows = task_files(
        &state.pool,
        Reader::Browser {
            actor_id: principal.actor_id,
        },
        project_id,
        work_item_id,
    )
    .await?;
    Ok(Json(TaskFilesResponse {
        files: rows.iter().map(FileRow::view).collect(),
    }))
}

async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(file_id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let row = readable_file(
        &state.pool,
        Reader::Browser {
            actor_id: principal.actor_id,
        },
        file_id,
    )
    .await?;
    let mut transaction = state.pool.begin().await?;
    let may_edit_task = match (row.attached_at.as_ref(), row.work_item_id) {
        (Some(_), Some(work_item_id)) => {
            authorize_task_change(&mut transaction, &principal, row.project_id, work_item_id)
                .await
                .is_ok()
        }
        _ => false,
    };
    remove_file(&mut transaction, &row, principal.actor_id, may_edit_task).await?;
    transaction.commit().await?;
    if let (Some(_), Some(work_item_id)) = (row.attached_at.as_ref(), row.work_item_id) {
        crate::work_items::notify_files_changed(&state.pool, row.project_id, work_item_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn content(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(file_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ContentQuery>,
) -> Result<Response, AppError> {
    let row = readable_file(
        &state.pool,
        Reader::Browser {
            actor_id: principal.actor_id,
        },
        file_id,
    )
    .await?;
    let download = query.download.is_some() || !row.is_inline_image();
    serve(
        &state,
        &row.storage_key,
        served_content_type(&row),
        &row.name,
        download,
    )
    .await
}

async fn thumbnail(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(file_id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let row = readable_file(
        &state.pool,
        Reader::Browser {
            actor_id: principal.actor_id,
        },
        file_id,
    )
    .await?;
    if !row.is_inline_image() {
        return Err(AppError::NotFound("file"));
    }
    match &row.thumbnail_key {
        Some(key) => {
            let content_type = if row.content_type == "image/jpeg" {
                "image/jpeg"
            } else {
                "image/png"
            };
            serve(&state, key, content_type, &row.name, false).await
        }
        None => {
            serve(
                &state,
                &row.storage_key,
                &row.content_type,
                &row.name,
                false,
            )
            .await
        }
    }
}

/// Streams stored bytes with headers that keep the browser from treating
/// them as part of the app: inline only for images, otherwise a download.
pub(crate) async fn serve(
    state: &AppState,
    key: &str,
    content_type: &str,
    name: &str,
    download: bool,
) -> Result<Response, AppError> {
    let stored = FileStore::from_config(&state.config).get(key).await?;
    let mut response = Response::new(stored.body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(stored.length));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(name, download))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    // Private: a shared cache must never keep it. Short: a removed file
    // stops opening soon even in a tab that already had it.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    Ok(response)
}

/// `inline` or `attachment`, with the name as an RFC 6266 `filename*` and
/// an ASCII fallback.
pub(crate) fn content_disposition(name: &str, download: bool) -> String {
    let fallback = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded = url::form_urlencoded::byte_serialize(name.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    format!(
        "{}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}",
        if download { "attachment" } else { "inline" }
    )
}

fn declared_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

pub(crate) async fn read_body(body: Body, limit: usize) -> Result<Vec<u8>, AppError> {
    let bytes = axum::body::to_bytes(body, limit)
        .await
        .map_err(|_| AppError::PayloadTooLarge)?;
    if bytes.is_empty() {
        return Err(AppError::Validation("file"));
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::content_disposition;

    #[test]
    fn names_are_quoted_safely_in_the_disposition() {
        assert_eq!(
            content_disposition("a \"b\";c.pdf", true),
            "attachment; filename=\"a _b__c.pdf\"; filename*=UTF-8''a%20%22b%22%3Bc.pdf"
        );
        assert!(
            content_disposition("Grüße.png", false).starts_with("inline; filename=\"Gr__e.png\"")
        );
    }
}
