//! Where operators read the recorded disagreements between old and new
//! access checks (K1 §4.3): Settings › Workspace › Server, for the
//! workspace in the path. Only the server's operators who are members of
//! that workspace see them; anyone else gets what they get for the server
//! page (404 outside the workspace, 403 inside it).

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::{Need, Target};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/organizations/{organization_id}/server/access-disagreements",
        get(list_http),
    )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct DisagreementView {
    id: Uuid,
    area: String,
    operation: String,
    space_id: Option<Uuid>,
    account_id: Uuid,
    account_kind: Option<String>,
    permission: String,
    target_kind: Option<String>,
    target_id: Option<Uuid>,
    old_allowed: bool,
    new_allowed: bool,
    new_reason: Option<String>,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisagreementsView {
    disagreements: Vec<DisagreementView>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<DisagreementsView>, AppError> {
    let caller = principal.caller()?;
    {
        let mut connection = state.pool.acquire().await?;
        if super::check(
            &mut connection,
            &caller,
            Need::SEE,
            Target::Workspace(organization_id),
        )
        .await?
        .is_err()
        {
            return Err(AppError::NotFound("workspace"));
        }
    }
    if !crate::server_status::is_operator(&state, &principal).await? {
        return Err(AppError::Forbidden);
    }
    let disagreements = sqlx::query_as::<_, DisagreementView>(
        r#"
        SELECT id, area, operation, space_id, account_id, account_kind, permission, target_kind,
               target_id, old_allowed, new_allowed, new_reason, first_seen_at, last_seen_at, count
        FROM access_disagreements
        WHERE workspace_id = $1
        ORDER BY last_seen_at DESC
        LIMIT 200
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(DisagreementsView { disagreements }))
}
