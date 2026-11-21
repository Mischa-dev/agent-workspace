//! The app's v1 router. The built-in host's run routes live with the
//! built-in host (`hosts::builtin::http`).

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::AppError,
    hosts::builtin::http::{self, authorize_project_action},
    policy::Action,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::auth::router())
        .merge(crate::setup::router())
        .merge(crate::projects::router())
        .merge(crate::execution::router())
        .merge(crate::external_execution::router())
        .route("/api/v1/dev/bootstrap", get(development_bootstrap))
        .route(
            "/api/v1/threads/{id}/agent-runs",
            post(http::start_agent_run),
        )
        .route("/api/v1/agent-runs/{id}/retry", post(http::retry_agent_run))
        .route("/api/v1/realtime", get(crate::realtime::websocket))
        // The live contract (K5.19): tickets and resume on `api`, the
        // connection and publishes on `live`.
        .route(
            "/api/v2/live/ticket",
            get(crate::realtime::contract::http::ticket),
        )
        .route(
            "/api/v2/live/resume",
            get(crate::realtime::contract::http::resume),
        )
        .route(
            "/api/v2/live/key",
            get(crate::realtime::contract::http::key),
        )
        .merge(crate::realtime::contract::live::router())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevelopmentBootstrap {
    project_id: Uuid,
    channel_id: Uuid,
    thread_id: Uuid,
}

async fn development_bootstrap(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<DevelopmentBootstrap>, AppError> {
    if !state.config.environment.is_development() || !state.config.server_addr.ip().is_loopback() {
        return Err(AppError::DevelopmentAuthUnavailable);
    }

    #[derive(FromRow)]
    struct BootstrapRow {
        project_id: Uuid,
        channel_id: Uuid,
        thread_id: Uuid,
        membership_role: String,
        membership_role_permissions: Option<Vec<String>>,
        membership_group_permissions: Option<Vec<String>>,
        membership_enabled: bool,
    }

    let bootstrap = sqlx::query_as::<_, BootstrapRow>(
        r#"
        SELECT
            thread.project_id,
            parent.id AS channel_id,
            thread.id AS thread_id,
            pm.role AS membership_role,
            workspace_role_permissions(pm.role_id) AS membership_role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS membership_group_permissions,
            pm.enabled AS membership_enabled
        FROM conversations thread
        JOIN conversations parent ON parent.id = thread.parent_channel_id
        JOIN projects p ON p.id = thread.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $1
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = thread.project_id
         AND pm.actor_id = $1
        WHERE p.archived_at IS NULL
          AND thread.kind = 'thread'
          AND thread.archived_at IS NULL
          AND parent.archived_at IS NULL
        ORDER BY (p.project_key = 'AW') DESC, thread.created_at
        LIMIT 1
        "#,
    )
    .bind(principal.actor_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::NotFound("seeded_thread"))?;
    authorize_project_action(
        &principal,
        bootstrap.project_id,
        &bootstrap.membership_role,
        bootstrap.membership_role_permissions.as_deref(),
        bootstrap.membership_group_permissions.as_deref(),
        bootstrap.membership_enabled,
        Action::ProjectView,
    )?;

    Ok(Json(DevelopmentBootstrap {
        project_id: bootstrap.project_id,
        channel_id: bootstrap.channel_id,
        thread_id: bootstrap.thread_id,
    }))
}
