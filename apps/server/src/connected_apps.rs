//! Connected apps: the MCP clients (Claude Code, Codex, ChatGPT, ...) a
//! person has connected to their account. People can see what's connected,
//! turn the "via <app>" label on or off, and revoke access.

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    mcp::MCP_PATH,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/account/connected-apps", get(list))
        .route("/api/v1/account/connected-apps/{id}", patch(update))
        .route("/api/v1/account/connected-apps/{id}/revoke", post(revoke))
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectedApp {
    id: Uuid,
    client_name: String,
    /// "full" or "read"
    access: String,
    /// Who the app acts as: "you", or the agent's name.
    acting_as: String,
    account_wide: bool,
    show_via: bool,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectedAppsResponse {
    /// The address to give an MCP client.
    mcp_url: String,
    apps: Vec<ConnectedApp>,
}

const APP_SELECT: &str = r#"
    SELECT grant_row.id, client.client_name,
           CASE WHEN grant_row.scopes @> ARRAY['write']::text[] THEN 'full' ELSE 'read' END AS access,
           CASE WHEN grant_row.external_agent_connection_id IS NULL THEN 'you'
                ELSE COALESCE(agent.display_name, 'an agent') END AS acting_as,
           grant_row.account_wide, grant_row.show_via,
           grant_row.created_at::text AS created_at,
           (SELECT max(access.last_used_at) FROM mcp_oauth_access_tokens access
            WHERE access.grant_id = grant_row.id)::text AS last_used_at
    FROM mcp_oauth_grants grant_row
    JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
    LEFT JOIN external_agent_connections connection
      ON connection.id = grant_row.external_agent_connection_id
    LEFT JOIN project_memberships agent
      ON agent.project_id = connection.project_id AND agent.actor_id = connection.actor_id
"#;

async fn list(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<ConnectedAppsResponse>, AppError> {
    // A person sees apps connected as themselves, plus apps they connected as
    // one of the agents they manage.
    let apps = sqlx::query_as::<_, ConnectedApp>(&format!(
        r#"{APP_SELECT}
        WHERE grant_row.revoked_at IS NULL
          AND (grant_row.actor_id = $1
               OR (grant_row.external_agent_connection_id IS NOT NULL
                   AND EXISTS (
                       SELECT 1 FROM project_memberships manager
                       WHERE manager.project_id = connection.project_id
                         AND manager.actor_id = $1 AND manager.enabled
                         AND manager.role IN ('owner', 'admin'))))
        ORDER BY grant_row.created_at DESC
        LIMIT 100
        "#
    ))
    .bind(principal.actor_id)
    .fetch_all(&state.pool)
    .await?;
    let origin = state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr));
    Ok(Json(ConnectedAppsResponse {
        mcp_url: format!("{origin}{MCP_PATH}"),
        apps,
    }))
}

/// The grant, if this person may manage it.
async fn owned_grant(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    grant_id: Uuid,
) -> Result<(), AppError> {
    let allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM mcp_oauth_grants grant_row
            LEFT JOIN external_agent_connections connection
              ON connection.id = grant_row.external_agent_connection_id
            WHERE grant_row.id = $1 AND grant_row.revoked_at IS NULL
              AND (grant_row.actor_id = $2
                   OR EXISTS (
                       SELECT 1 FROM project_memberships manager
                       WHERE manager.project_id = connection.project_id
                         AND manager.actor_id = $2 AND manager.enabled
                         AND manager.role IN ('owner', 'admin')))
            FOR UPDATE OF grant_row
        )
        "#,
    )
    .bind(grant_id)
    .bind(actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(AppError::NotFound("connected app"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    show_via: bool,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(grant_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<ConnectedApp>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    owned_grant(&mut transaction, principal.actor_id, grant_id).await?;
    sqlx::query("UPDATE mcp_oauth_grants SET show_via = $2 WHERE id = $1")
        .bind(grant_id)
        .bind(request.show_via)
        .execute(&mut *transaction)
        .await?;
    let app = sqlx::query_as::<_, ConnectedApp>(&format!("{APP_SELECT} WHERE grant_row.id = $1"))
        .bind(grant_id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Json(app))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RevokeResponse {
    id: Uuid,
    revoked: bool,
}

async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(grant_id): ApiPath<Uuid>,
) -> Result<Json<RevokeResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    owned_grant(&mut transaction, principal.actor_id, grant_id).await?;
    // Same effect as the client revoking its own token: the grant and every
    // access and refresh token issued from it stop working at once.
    sqlx::query(
        "UPDATE mcp_oauth_grants SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
    )
    .bind(grant_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE mcp_oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE grant_id = $1",
    )
    .bind(grant_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE mcp_oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE grant_id = $1",
    )
    .bind(grant_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, session_id, authentication_class, action,
            target_kind, target_id, outcome, metadata, mcp_oauth_grant_id
        ) VALUES ($1, $2, $3, 'password_session', 'connected_app.revoked',
                  'mcp_oauth_grant', $4, 'succeeded', '{}'::jsonb, $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(principal.session_id)
    .bind(grant_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(RevokeResponse {
        id: grant_id,
        revoked: true,
    }))
}
