use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    external_events::enqueue_external_event,
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, Permission, ProjectScope,
        Resource, RoleGrant, Subject, authorize,
    },
};

const TOKEN_BYTES: usize = 32;
const TOKEN_PREFIX_LENGTH: usize = 16;
// Long-lived because scheduled agents run unattended; rotate from Agents.
const CREDENTIAL_LIFETIME_DAYS: i32 = 365;
// Informational label; any client that speaks the MCP contract can connect.
const RUNTIME_KIND: &str = "mcp";
const RUNTIME_VERSION: &str = "1";
const ALLOWED_SCOPES: [&str; 5] = [
    "approval_respond",
    "events",
    "read",
    "worker_request",
    "write",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/external-agents",
            get(list_connections).post(create_connection),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}",
            axum::routing::patch(update_connection),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/credentials/rotate",
            post(rotate_credential),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/remove",
            post(remove_connection),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/webhook",
            post(set_webhook),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/webhook/test",
            post(test_webhook),
        )
}

#[derive(Debug, FromRow)]
struct AccessRow {
    organization_id: Uuid,
    project_archived: bool,
    organization_role: String,
    organization_enabled: bool,
    project_role: String,
    project_role_permissions: Option<Vec<String>>,
    project_enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct ConnectionRow {
    id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    connection_name: String,
    display_name: String,
    handle: String,
    role: String,
    runtime_kind: String,
    runtime_version: String,
    scopes: Vec<String>,
    status: String,
    version: i64,
    last_used_at: Option<String>,
    last_error_category: Option<String>,
    disabled_at: Option<String>,
    removed_at: Option<String>,
    created_at: String,
    updated_at: String,
    webhook_url: Option<String>,
    last_wakeup_at: Option<String>,
    last_wakeup_error: Option<String>,
    wakeup_format: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateConnectionRequest {
    connection_name: String,
    display_name: String,
    handle: String,
    role: String,
    scopes: Vec<String>,
    /// Optional: where to send wakeups, so a runner is set up in one form.
    #[serde(default)]
    webhook_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateConnectionRequest {
    expected_version: i64,
    role: Option<String>,
    enabled: Option<bool>,
    scopes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedVersionRequest {
    expected_version: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionView {
    id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    connection_name: String,
    display_name: String,
    handle: String,
    role: String,
    runtime_kind: String,
    runtime_version: String,
    scopes: Vec<String>,
    status: String,
    version: i64,
    last_used_at: Option<String>,
    last_error_category: Option<String>,
    disabled_at: Option<String>,
    removed_at: Option<String>,
    created_at: String,
    updated_at: String,
    webhook_url: Option<String>,
    last_wakeup_at: Option<String>,
    last_wakeup_error: Option<String>,
    /// `typed` wakeups say what happened; `inbox` is the old
    /// `inbox.available` body, kept for one release for webhooks set up
    /// before typed wakeups. Saving the webhook again switches to typed.
    wakeup_format: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsResponse {
    connections: Vec<ConnectionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OneTimeCredential {
    id: Uuid,
    token: String,
    token_prefix: String,
    expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionMutationResponse {
    connection: ConnectionView,
    one_time_credential: Option<OneTimeCredential>,
    /// Shown once, when the agent was created with a webhook.
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_secret: Option<String>,
}

async fn list_connections(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ConnectionsResponse>, AppError> {
    let access = load_access(&state, principal.actor_id, project_id, false)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(&principal, &access, project_id)?;
    let rows = sqlx::query_as::<_, ConnectionRow>(connection_query(false))
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(ConnectionsResponse {
        connections: rows.into_iter().map(ConnectionView::from).collect(),
    }))
}

async fn create_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateConnectionRequest>,
) -> Result<Json<ConnectionMutationResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let webhook_url = validate_webhook_url(
        request.webhook_url.clone(),
        &state.config.webhook_trusted_origins,
    )?;
    let normalized = validate_create(request)?;
    let mut transaction = state.pool.begin().await?;
    lock_project(&mut transaction, project_id).await?;
    let access = load_access_tx(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(&principal, &access, project_id)?;
    if access.project_archived {
        return Err(AppError::ProjectArchived);
    }
    require_grantable(
        &mut transaction,
        access.organization_id,
        project_id,
        principal.actor_id,
        &normalized.role,
        None,
    )
    .await?;
    let created = insert_agent(
        &mut transaction,
        access.organization_id,
        project_id,
        principal.actor_id,
        &normalized,
    )
    .await?;
    insert_audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        created.connection_id,
        "external_agent.created",
        serde_json::json!({
            "actor_id": created.actor_id,
            "role": normalized.role,
            "runtime_kind": RUNTIME_KIND,
            "runtime_version": RUNTIME_VERSION,
            "scopes": normalized.scopes,
        }),
    )
    .await?;
    let webhook_secret = match &webhook_url {
        Some(url) => {
            let secret = crate::replies::new_webhook_secret();
            // The signing secret lives encrypted in the secrets store (S1).
            let secret_id = crate::secrets::put_webhook_secret(
                &mut transaction,
                &state.config,
                created.connection_id,
                &crate::secrets::SecretValue::new(secret.clone()),
            )
            .await?;
            sqlx::query(
                r#"
                UPDATE external_agent_connections
                SET webhook_url = $2, webhook_secret = NULL, webhook_secret_id = $3,
                    wakeup_format = 'typed'
                WHERE id = $1
                "#,
            )
            .bind(created.connection_id)
            .bind(url)
            .bind(secret_id)
            .execute(&mut *transaction)
            .await?;
            insert_audit(
                &mut transaction,
                &headers,
                &principal,
                project_id,
                created.connection_id,
                "external_agent.webhook_updated",
                serde_json::json!({ "enabled": true }),
            )
            .await?;
            Some(secret)
        }
        None => None,
    };
    let row = load_connection_tx(&mut transaction, project_id, created.connection_id).await?;
    transaction.commit().await?;
    Ok(Json(ConnectionMutationResponse {
        connection: row.into(),
        one_time_credential: Some(created.credential),
        webhook_secret,
    }))
}

/// A trimmed webhook URL that passes the delivery checks, or None when empty.
fn validate_webhook_url(
    url: Option<String>,
    trusted_origins: &[String],
) -> Result<Option<String>, AppError> {
    let url = url
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty());
    if let Some(url) = &url {
        crate::replies::validate_webhook_url(url, trusted_origins)?;
    }
    Ok(url)
}

struct CreatedAgent {
    actor_id: Uuid,
    connection_id: Uuid,
    credential: OneTimeCredential,
}

/// Create the agent's Actor, memberships, connection, and first credential.
async fn insert_agent(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    created_by: Uuid,
    normalized: &NormalizedCreate,
) -> Result<CreatedAgent, AppError> {
    let (token, token_prefix, token_digest) = generate_token();
    let actor_id = Uuid::now_v7();
    let connection_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'external_agent')")
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (organization_id, actor_id, role)
        VALUES ($1, $2, 'member')
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle,
            invited_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .bind(&normalized.role)
    .bind(&normalized.display_name)
    .bind(&normalized.handle)
    .bind(created_by)
    .execute(&mut **transaction)
    .await
    .map_err(map_database_error)?;
    sqlx::query(
        r#"
        INSERT INTO external_agent_connections (
            id, project_id, actor_id, connection_name, runtime_kind,
            runtime_version, scopes, created_by_actor_id, updated_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
        "#,
    )
    .bind(connection_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(&normalized.connection_name)
    .bind(RUNTIME_KIND)
    .bind(RUNTIME_VERSION)
    .bind(&normalized.scopes)
    .bind(created_by)
    .execute(&mut **transaction)
    .await
    .map_err(map_database_error)?;
    let expires_at = insert_credential(
        transaction,
        credential_id,
        project_id,
        actor_id,
        connection_id,
        &normalized.connection_name,
        &normalized.scopes,
        &token_prefix,
        &token_digest,
    )
    .await?;
    enqueue_external_event(
        transaction,
        connection_id,
        project_id,
        "connection.created",
        "External agent connection is ready",
        "external_agent_connection",
        connection_id,
        &format!("/projects/{project_id}/settings"),
    )
    .await?;
    Ok(CreatedAgent {
        actor_id,
        connection_id,
        credential: OneTimeCredential {
            id: credential_id,
            token,
            token_prefix,
            expires_at,
        },
    })
}

/// Host command support: point a connection's wakeups at `url` and return the
/// new signing secret.
pub async fn set_webhook_for_host(
    pool: &sqlx::PgPool,
    config: &crate::config::AppConfig,
    connection_id: Uuid,
    url: &str,
    trusted_origins: &[String],
    typed_wakeups: bool,
) -> Result<String, AppError> {
    crate::replies::validate_webhook_url(url, trusted_origins)?;
    let secret = crate::replies::new_webhook_secret();
    let mut transaction = pool.begin().await?;
    let secret_id = crate::secrets::put_webhook_secret(
        &mut transaction,
        config,
        connection_id,
        &crate::secrets::SecretValue::new(secret.clone()),
    )
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE external_agent_connections
        SET webhook_url = $2, webhook_secret = NULL, webhook_secret_id = $3,
            version = version + 1, updated_at = now(),
            wakeup_format = CASE WHEN $4 THEN 'typed' ELSE 'inbox' END
        WHERE id = $1 AND status = 'active'
        "#,
    )
    .bind(connection_id)
    .bind(url)
    .bind(secret_id)
    .bind(typed_wakeups)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound("external agent connection"));
    }
    transaction.commit().await?;
    Ok(secret)
}

/// An agent created from the host command line.
#[derive(Debug)]
pub struct ProvisionedAgent {
    pub actor_id: Uuid,
    pub connection_id: Uuid,
    pub token: String,
    pub expires_at: String,
}

/// Create an agent without a browser session, on behalf of `owner_actor_id`
/// (who must be able to manage agents in the Project). Used by the
/// `direct-agent-create` host command so tokens go straight to a file.
pub async fn provision(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    owner_actor_id: Uuid,
    display_name: &str,
    handle: &str,
) -> Result<ProvisionedAgent, AppError> {
    let normalized = validate_create(CreateConnectionRequest {
        connection_name: display_name.to_owned(),
        display_name: display_name.to_owned(),
        handle: handle.to_owned(),
        role: "member".to_owned(),
        scopes: vec!["read".to_owned(), "write".to_owned(), "events".to_owned()],
        webhook_url: None,
    })?;
    let mut transaction = pool.begin().await?;
    lock_project(&mut transaction, project_id).await?;
    let (organization_id, owner_role, owner_role_permissions) =
        sqlx::query_as::<_, (Uuid, String, Option<Vec<String>>)>(
            r#"
        SELECT project.organization_id, membership.role,
               workspace_role_permissions(membership.role_id)
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
        WHERE project.id = $1 AND project.archived_at IS NULL AND membership.enabled
        "#,
        )
        .bind(project_id)
        .bind(owner_actor_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    if !RoleGrant::stored(&owner_role, owner_role_permissions.as_deref())
        .is_ok_and(|grant| grant.permissions().contains(Permission::AgentsManage))
    {
        return Err(AppError::Forbidden);
    }
    require_grantable(
        &mut transaction,
        organization_id,
        project_id,
        owner_actor_id,
        &normalized.role,
        None,
    )
    .await?;
    let created = insert_agent(
        &mut transaction,
        organization_id,
        project_id,
        owner_actor_id,
        &normalized,
    )
    .await?;
    transaction.commit().await?;
    Ok(ProvisionedAgent {
        actor_id: created.actor_id,
        connection_id: created.connection_id,
        token: created.credential.token,
        expires_at: created.credential.expires_at,
    })
}

async fn update_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, connection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateConnectionRequest>,
) -> Result<Json<ConnectionMutationResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1
        || request.role.is_none() && request.enabled.is_none() && request.scopes.is_none()
    {
        return Err(AppError::Invalid("external_agent_update"));
    }
    if request
        .role
        .as_deref()
        .is_some_and(|role| !matches!(role, "member" | "viewer"))
    {
        return Err(AppError::Validation("external_agent_role"));
    }
    let scopes = request.scopes.map(normalize_scopes).transpose()?;
    mutate_connection(
        &state,
        &headers,
        &principal,
        project_id,
        connection_id,
        request.expected_version,
        request.role,
        request.enabled,
        scopes,
        false,
        false,
    )
    .await
    .map(Json)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebhookRequest {
    /// Where to send wakeups, or null to stop them.
    url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookResponse {
    connection: ConnectionView,
    /// Shown once so the agent can verify signatures.
    webhook_secret: Option<String>,
}

/// Set or clear the connection's wakeup webhook. Each change issues a new
/// signing secret.
async fn set_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, connection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WebhookRequest>,
) -> Result<Json<WebhookResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let url = validate_webhook_url(request.url, &state.config.webhook_trusted_origins)?;
    let secret = url.as_ref().map(|_| crate::replies::new_webhook_secret());
    let mut transaction = state.pool.begin().await?;
    lock_project(&mut transaction, project_id).await?;
    let access = load_access_tx(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(&principal, &access, project_id)?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM external_agent_connections WHERE project_id = $1 AND id = $2 AND status <> 'removed')",
    )
    .bind(project_id)
    .bind(connection_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !exists {
        return Err(AppError::NotFound("external agent connection"));
    }
    // The signing secret lives encrypted in the secrets store (S1).
    let secret_id = match &secret {
        Some(secret) => Some(
            crate::secrets::put_webhook_secret(
                &mut transaction,
                &state.config,
                connection_id,
                &crate::secrets::SecretValue::new(secret.clone()),
            )
            .await?,
        ),
        None => None,
    };
    let updated = sqlx::query(
        r#"
        UPDATE external_agent_connections
        SET webhook_url = $3, webhook_secret = NULL, webhook_secret_id = $4,
            last_wakeup_error = NULL,
            -- A new secret means the runner is set up again, so it gets
            -- typed wakeups from here on.
            wakeup_format = 'typed',
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND status <> 'removed'
        "#,
    )
    .bind(project_id)
    .bind(connection_id)
    .bind(&url)
    .bind(secret_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound("external agent connection"));
    }
    if secret_id.is_none() {
        crate::secrets::delete_webhook_secret(&mut transaction, connection_id).await?;
    }
    insert_audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        connection_id,
        "external_agent.webhook_updated",
        serde_json::json!({ "enabled": url.is_some() }),
    )
    .await?;
    let row = load_connection_tx(&mut transaction, project_id, connection_id).await?;
    transaction.commit().await?;
    Ok(Json(WebhookResponse {
        connection: row.into(),
        webhook_secret: secret,
    }))
}

/// Send the runner a signed test wakeup now and say what it answered, so a
/// person setting one up sees at once whether the URL, the network path and
/// the signature check work. Only a project's managers can, like changing
/// the webhook.
async fn test_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, connection_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<crate::replies::TestWakeup>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let access = load_access_tx(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(&principal, &access, project_id)?;
    let (agent_id, url, plaintext, secret_id) =
        sqlx::query_as::<_, (Uuid, Option<String>, Option<String>, Option<Uuid>)>(
            r#"
            SELECT actor_id, webhook_url, webhook_secret, webhook_secret_id
            FROM external_agent_connections
            WHERE project_id = $1 AND id = $2 AND status <> 'removed'
            "#,
        )
        .bind(project_id)
        .bind(connection_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("external agent connection"))?;
    let Some(url) = url else {
        return Err(AppError::Conflict(
            "webhook_missing",
            "This agent has no webhook to test. Add its runner's URL first.",
        ));
    };
    let secret = match (secret_id, plaintext) {
        (Some(id), _) => {
            let keys = crate::secrets::keyring(&state.config)?;
            crate::secrets::reveal(&mut *transaction, keys.as_ref(), id, "agent_webhook.test")
                .await?
        }
        (None, Some(plaintext)) => crate::secrets::SecretValue::new(plaintext),
        (None, None) => {
            return Err(AppError::Conflict(
                "webhook_secret_missing",
                "This webhook has no signing secret. Save the webhook again to get one.",
            ));
        }
    };
    insert_audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        connection_id,
        "external_agent.webhook_tested",
        serde_json::json!({}),
    )
    .await?;
    // Commit before the network call: the runner may take seconds to answer.
    transaction.commit().await?;
    Ok(Json(
        crate::replies::send_test_wakeup(
            &url,
            secret.expose(),
            agent_id,
            project_id,
            &state.config.webhook_trusted_origins,
        )
        .await,
    ))
}

async fn rotate_credential(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, connection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ExpectedVersionRequest>,
) -> Result<Json<ConnectionMutationResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    mutate_connection(
        &state,
        &headers,
        &principal,
        project_id,
        connection_id,
        request.expected_version,
        None,
        None,
        None,
        true,
        false,
    )
    .await
    .map(Json)
}

async fn remove_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, connection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ExpectedVersionRequest>,
) -> Result<Json<ConnectionMutationResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    mutate_connection(
        &state,
        &headers,
        &principal,
        project_id,
        connection_id,
        request.expected_version,
        None,
        None,
        None,
        false,
        true,
    )
    .await
    .map(Json)
}

#[allow(clippy::too_many_arguments)]
async fn mutate_connection(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    connection_id: Uuid,
    expected_version: i64,
    requested_role: Option<String>,
    requested_enabled: Option<bool>,
    requested_scopes: Option<Vec<String>>,
    force_rotate: bool,
    remove: bool,
) -> Result<ConnectionMutationResponse, AppError> {
    if expected_version < 1 {
        return Err(AppError::Invalid("expected_version"));
    }
    let mut transaction = state.pool.begin().await?;
    lock_project(&mut transaction, project_id).await?;
    let access = load_access_tx(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(principal, &access, project_id)?;
    if access.project_archived {
        return Err(AppError::ProjectArchived);
    }
    let current = load_connection_tx(&mut transaction, project_id, connection_id).await?;
    if current.status == "removed" {
        return Err(AppError::Conflict("external_agent_connection", "removed"));
    }
    if current.version != expected_version {
        return Err(AppError::VersionConflict {
            resource: "external_agent_connection",
            current_version: current.version,
        });
    }
    let next_role = requested_role.as_deref().unwrap_or(&current.role);
    let next_enabled = if remove {
        false
    } else {
        requested_enabled.unwrap_or(current.status == "active")
    };
    let has_explicit_scopes = requested_scopes.is_some();
    let requested_scopes = requested_scopes.unwrap_or_else(|| current.scopes.clone());
    if next_role == "viewer"
        && has_explicit_scopes
        && requested_scopes
            .iter()
            .any(|scope| !matches!(scope.as_str(), "read" | "events"))
    {
        return Err(AppError::Validation("external_agent_scopes"));
    }
    let next_scopes = if next_role == "viewer" {
        requested_scopes
            .iter()
            .filter(|scope| matches!(scope.as_str(), "read" | "events"))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        requested_scopes
    };
    if !next_scopes.iter().any(|scope| scope == "read") {
        return Err(AppError::Validation("external_agent_scopes"));
    }
    let restores = next_enabled && current.status != "active";
    let widens_scopes = next_scopes
        .iter()
        .any(|scope| !current.scopes.contains(scope));
    let role_changed = next_role != current.role;
    let scopes_changed = next_scopes != current.scopes;
    let contract_changed = role_changed || scopes_changed;
    let enabled_changed = next_enabled != (current.status == "active");
    let should_issue =
        !remove && next_enabled && (force_rotate || contract_changed || enabled_changed);
    // Changing the role, bringing a disabled connection back, giving it a
    // scope it didn't have, or issuing a new credential (a rotation hands
    // the caller a token for the agent) all hand the agent's role out again,
    // so the role must be within what the caller holds.
    if role_changed || restores || widens_scopes || should_issue {
        // A new built-in role is checked as asked; otherwise the agent keeps
        // the role it holds, custom ones included.
        let role_id = if role_changed {
            None
        } else {
            agent_role_id(&mut transaction, current.project_id, current.actor_id).await?
        };
        require_grantable(
            &mut transaction,
            access.organization_id,
            current.project_id,
            principal.actor_id,
            next_role,
            role_id,
        )
        .await?;
    }
    if force_rotate && !next_enabled {
        return Err(AppError::Conflict("external_agent_connection", "disabled"));
    }
    if !contract_changed && !enabled_changed && !force_rotate && !remove {
        transaction.commit().await?;
        return Ok(ConnectionMutationResponse {
            connection: current.into(),
            one_time_credential: None,
            webhook_secret: None,
        });
    }

    if !next_enabled || contract_changed || force_rotate {
        revoke_credentials(&mut transaction, connection_id).await?;
    }
    if !next_enabled
        || remove
        || (current.scopes.iter().any(|scope| scope == "worker_request")
            && !next_scopes.iter().any(|scope| scope == "worker_request"))
    {
        cancel_pending_execution_requests(&mut transaction, project_id, connection_id).await?;
    }
    if enabled_changed || remove {
        sqlx::query(
            r#"
            UPDATE actors
            SET status = CASE WHEN $2 THEN 'active' ELSE 'disabled' END, updated_at = now()
            WHERE id = $1 AND kind = 'external_agent'
            "#,
        )
        .bind(current.actor_id)
        .bind(next_enabled)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE organization_memberships
            SET enabled = $3, version = version + 1, updated_at = now()
            WHERE organization_id = $1 AND actor_id = $2
            "#,
        )
        .bind(access.organization_id)
        .bind(current.actor_id)
        .bind(next_enabled)
        .execute(&mut *transaction)
        .await?;
    }
    if enabled_changed || role_changed || remove {
        sqlx::query(
            r#"
            UPDATE project_memberships
            SET role = $3, enabled = $4,
                disabled_by_actor_id = CASE WHEN $4 THEN NULL ELSE $5 END,
                disabled_at = CASE WHEN $4 THEN NULL ELSE now() END,
                version = version + 1, updated_at = now()
            WHERE project_id = $1 AND actor_id = $2
            "#,
        )
        .bind(project_id)
        .bind(current.actor_id)
        .bind(next_role)
        .bind(next_enabled)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_database_error)?;
    }
    let updated = sqlx::query(
        r#"
        UPDATE external_agent_connections
        SET scopes = $3,
            status = CASE
                WHEN $7 THEN 'removed'
                WHEN $4 THEN 'active'
                ELSE 'disabled'
            END,
            disabled_at = CASE WHEN $4 THEN NULL ELSE now() END,
            removed_at = CASE WHEN $7 THEN now() ELSE removed_at END,
            version = version + 1, updated_by_actor_id = $5, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $6
        "#,
    )
    .bind(project_id)
    .bind(connection_id)
    .bind(&next_scopes)
    .bind(next_enabled)
    .bind(principal.actor_id)
    .bind(expected_version)
    .bind(remove)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "external_agent_connection",
            "stale version",
        ));
    }

    let one_time_credential = if should_issue {
        let (token, token_prefix, token_digest) = generate_token();
        let credential_id = Uuid::now_v7();
        let expires_at = insert_credential(
            &mut transaction,
            credential_id,
            project_id,
            current.actor_id,
            connection_id,
            &current.connection_name,
            &next_scopes,
            &token_prefix,
            &token_digest,
        )
        .await?;
        Some(OneTimeCredential {
            id: credential_id,
            token,
            token_prefix,
            expires_at,
        })
    } else {
        None
    };
    let action = if remove {
        "external_agent.removed"
    } else if force_rotate {
        "external_agent.credential_rotated"
    } else if next_enabled {
        "external_agent.updated"
    } else {
        "external_agent.disabled"
    };
    let event_summary = match action {
        "external_agent.removed" => "External agent connection was removed",
        "external_agent.credential_rotated" => "External agent bearer was rotated",
        "external_agent.updated" if enabled_changed => "External agent connection was enabled",
        "external_agent.updated" => "External agent permissions were updated",
        _ => "External agent connection was disabled",
    };
    enqueue_external_event(
        &mut transaction,
        connection_id,
        project_id,
        action,
        event_summary,
        "external_agent_connection",
        connection_id,
        &format!("/projects/{project_id}/settings"),
    )
    .await?;
    insert_audit(
        &mut transaction,
        headers,
        principal,
        project_id,
        connection_id,
        action,
        serde_json::json!({
            "previous_status": current.status,
            "status": if remove { "removed" } else if next_enabled { "active" } else { "disabled" },
            "previous_role": current.role,
            "role": next_role,
            "previous_version": current.version,
            "version": current.version + 1,
        }),
    )
    .await?;
    let row = load_connection_tx(&mut transaction, project_id, connection_id).await?;
    transaction.commit().await?;
    Ok(ConnectionMutationResponse {
        connection: row.into(),
        one_time_credential,
        webhook_secret: None,
    })
}

struct NormalizedCreate {
    connection_name: String,
    display_name: String,
    handle: String,
    role: String,
    scopes: Vec<String>,
}

fn validate_create(request: CreateConnectionRequest) -> Result<NormalizedCreate, AppError> {
    let connection_name = request.connection_name.trim().to_owned();
    let display_name = request.display_name.trim().to_owned();
    let handle = request.handle.trim().to_ascii_lowercase().replace(' ', "_");
    if connection_name.is_empty()
        || connection_name.chars().count() > 80
        || !connection_name
            .chars()
            .enumerate()
            .all(|(index, character)| {
                character.is_ascii_alphanumeric()
                    || (index > 0 && matches!(character, '.' | '_' | '-' | ' '))
            })
        || display_name.is_empty()
        || display_name.chars().count() > 80
        || display_name.chars().any(char::is_control)
        || !(2..=32).contains(&handle.len())
        || !handle.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || (index > 0 && matches!(character, '_' | '-'))
        })
        || !matches!(request.role.as_str(), "member" | "viewer")
    {
        return Err(AppError::Validation("external_agent_identity"));
    }
    let scopes = normalize_scopes(request.scopes)?;
    if scopes.is_empty()
        || (request.role == "viewer"
            && scopes
                .iter()
                .any(|scope| !matches!(scope.as_str(), "read" | "events")))
    {
        return Err(AppError::Validation("external_agent_scopes"));
    }
    Ok(NormalizedCreate {
        connection_name,
        display_name,
        handle,
        role: request.role,
        scopes,
    })
}

fn normalize_scopes(mut scopes: Vec<String>) -> Result<Vec<String>, AppError> {
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty()
        || !scopes.iter().any(|scope| scope == "read")
        || !scopes
            .iter()
            .all(|scope| ALLOWED_SCOPES.contains(&scope.as_str()))
    {
        return Err(AppError::Validation("external_agent_scopes"));
    }
    Ok(scopes)
}

/// Refuses giving an agent a role that holds a permission the person
/// giving it doesn't, so a narrowed admin can't win back what their role
/// leaves out through an agent of their own. `role_id` is the custom role
/// the agent holds (or is given), whose permissions are what's handed out,
/// not its base role's; None for a built-in role. The agent works in
/// `project_id`, so what it's given there must also be within the giver's
/// role in that project, not only their workspace role: a project role that
/// leaves out "see code" or "edit tasks" can't be won back through an agent.
async fn require_grantable(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    caller_actor_id: Uuid,
    role: &str,
    role_id: Option<Uuid>,
) -> Result<(), AppError> {
    let permissions = match role_id {
        Some(role_id) => {
            sqlx::query_scalar::<_, Option<Vec<String>>>("SELECT workspace_role_permissions($1)")
                .bind(role_id)
                .fetch_one(&mut **transaction)
                .await?
        }
        None => None,
    };
    let grant = RoleGrant::stored(role, permissions.as_deref())
        .map_err(|_| AppError::Validation("agent role"))?;
    let caller = crate::roles::standing(transaction, organization_id, caller_actor_id).await?;
    if !grant.permissions().is_subset_of(caller.permissions) {
        return Err(AppError::Forbidden);
    }
    let held = crate::roles::project_holding(transaction, caller_actor_id, project_id)
        .await?
        .ok_or(AppError::Forbidden)?;
    if !crate::roles::project_checked(grant.permissions()).is_subset_of(held) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn authorize_management(
    principal: &AuthenticatedActor,
    access: &AccessRow,
    project_id: Uuid,
) -> Result<(), AppError> {
    if principal.actor_kind != "human"
        || !access.organization_enabled
        || !matches!(access.organization_role.as_str(), "owner" | "admin")
    {
        return Err(AppError::NotFound("project"));
    }
    let role = match RoleGrant::stored(
        &access.project_role,
        access.project_role_permissions.as_deref(),
    ) {
        Ok(role) if role.role() != MembershipRole::Worker => role,
        _ => return Err(AppError::NotFound("project")),
    };
    let subject = Subject::new(
        principal.actor_id,
        ActorKind::Human,
        true,
        Some(Membership::project(
            project_id,
            role,
            access.project_enabled,
        )),
        None,
    );
    match authorize(
        &subject,
        Action::ExternalAgentManage,
        Resource::Project(ProjectScope::project(project_id)),
    ) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(AppError::NotFound("project")),
    }
}

async fn lock_project(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("m9-external-agent:{project_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn load_access(
    state: &AppState,
    actor_id: Uuid,
    project_id: Uuid,
    lock: bool,
) -> Result<Option<AccessRow>, AppError> {
    debug_assert!(!lock);
    Ok(sqlx::query_as::<_, AccessRow>(access_sql(false))
        .bind(actor_id)
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?)
}

async fn load_access_tx(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<AccessRow>, AppError> {
    Ok(sqlx::query_as::<_, AccessRow>(access_sql(true))
        .bind(actor_id)
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?)
}

fn access_sql(lock: bool) -> &'static str {
    if lock {
        r#"
        SELECT p.organization_id, p.archived_at IS NOT NULL AS project_archived,
               om.role AS organization_role, om.enabled AS organization_enabled,
               pm.role AS project_role,
               workspace_role_permissions(pm.role_id) AS project_role_permissions,
               pm.enabled AS project_enabled
        FROM projects p
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id AND om.actor_id = $1
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $1
        WHERE p.id = $2
        FOR UPDATE OF p, om, pm
        "#
    } else {
        r#"
        SELECT p.organization_id, p.archived_at IS NOT NULL AS project_archived,
               om.role AS organization_role, om.enabled AS organization_enabled,
               pm.role AS project_role,
               workspace_role_permissions(pm.role_id) AS project_role_permissions,
               pm.enabled AS project_enabled
        FROM projects p
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id AND om.actor_id = $1
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $1
        WHERE p.id = $2
        "#
    }
}

fn connection_query(lock: bool) -> &'static str {
    if lock {
        r#"
        SELECT c.id, c.project_id, c.actor_id, c.connection_name,
               pm.display_name, pm.handle, pm.role, c.runtime_kind,
               c.runtime_version, c.scopes, c.status, c.version,
               c.last_used_at::text AS last_used_at,
               c.last_error_category, c.disabled_at::text AS disabled_at,
               c.removed_at::text AS removed_at, c.created_at::text AS created_at,
               c.updated_at::text AS updated_at, c.webhook_url,
               c.last_wakeup_at::text AS last_wakeup_at, c.last_wakeup_error,
               c.wakeup_format
        FROM external_agent_connections c
        JOIN project_memberships pm
          ON pm.project_id = c.project_id AND pm.actor_id = c.actor_id
        WHERE c.project_id = $1 AND c.id = $2
        FOR UPDATE OF c, pm
        "#
    } else {
        r#"
        SELECT c.id, c.project_id, c.actor_id, c.connection_name,
               pm.display_name, pm.handle, pm.role, c.runtime_kind,
               c.runtime_version, c.scopes, c.status, c.version,
               c.last_used_at::text AS last_used_at,
               c.last_error_category, c.disabled_at::text AS disabled_at,
               c.removed_at::text AS removed_at, c.created_at::text AS created_at,
               c.updated_at::text AS updated_at, c.webhook_url,
               c.last_wakeup_at::text AS last_wakeup_at, c.last_wakeup_error,
               c.wakeup_format
        FROM external_agent_connections c
        JOIN project_memberships pm
          ON pm.project_id = c.project_id AND pm.actor_id = c.actor_id
        WHERE c.project_id = $1
        ORDER BY c.created_at DESC, c.id DESC
        LIMIT 100
        "#
    }
}

async fn load_connection_tx(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    connection_id: Uuid,
) -> Result<ConnectionRow, AppError> {
    sqlx::query_as::<_, ConnectionRow>(connection_query(true))
        .bind(project_id)
        .bind(connection_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("external_agent_connection"))
}

#[allow(clippy::too_many_arguments)]
async fn insert_credential(
    transaction: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    connection_id: Uuid,
    client_name: &str,
    scopes: &[String],
    token_prefix: &str,
    token_digest: &[u8; 32],
) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO mcp_client_credentials (
            id, project_id, actor_id, client_name, token_prefix, token_digest,
            scopes, expires_at, external_agent_connection_id
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            now() + make_interval(days => $8), $9
        )
        RETURNING expires_at::text
        "#,
    )
    .bind(credential_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(client_name)
    .bind(token_prefix)
    .bind(token_digest.as_slice())
    .bind(scopes)
    .bind(CREDENTIAL_LIFETIME_DAYS)
    .bind(connection_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn revoke_credentials(
    transaction: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE mcp_client_credentials
        SET status = 'revoked', revoked_at = now(), version = version + 1,
            updated_at = now()
        WHERE external_agent_connection_id = $1 AND status = 'active'
        "#,
    )
    .bind(connection_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_pending_execution_requests(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    connection_id: Uuid,
) -> Result<(), AppError> {
    let request_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM external_execution_requests
        WHERE project_id = $1 AND external_agent_connection_id = $2
          AND status = 'pending'
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(connection_id)
    .fetch_all(&mut **transaction)
    .await?;
    if request_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE agent_approvals
        SET status = 'canceled'
        WHERE external_execution_request_id = ANY($1) AND status = 'pending'
        "#,
    )
    .bind(&request_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE external_execution_requests
        SET status = 'canceled', version = version + 1,
            resolved_at = now(), updated_at = now()
        WHERE id = ANY($1) AND status = 'pending'
        "#,
    )
    .bind(&request_ids)
    .execute(&mut **transaction)
    .await?;
    for request_id in request_ids {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            "external_execution.canceled",
            "Sponsored Worker execution was canceled because the connection was disabled",
            "external_execution_request",
            request_id,
            &format!("/projects/{project_id}/external-execution-requests/{request_id}"),
        )
        .await?;
    }
    Ok(())
}

fn generate_token() -> (String, String, [u8; 32]) {
    let mut bytes = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("aw_ext_{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = token[..TOKEN_PREFIX_LENGTH].to_owned();
    let digest = Sha256::digest(token.as_bytes()).into();
    (token, prefix, digest)
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    connection_id: Uuid,
    action: &str,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, channel,
            external_agent_connection_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session', $5,
            'external_agent_connection', $6, 'succeeded', $7, 'browser', $6, $8
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(connection_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn map_database_error(error: sqlx::Error) -> AppError {
    if matches!(
        &error,
        sqlx::Error::Database(database)
            if matches!(
                database.constraint(),
                Some("project_memberships_active_handle_unique") | Some("organization_actor_profiles_organization_id_handle_key")
                    | Some("external_agent_connections_project_id_connection_name_key")
            )
    ) {
        AppError::Conflict("external_agent", "name or handle already exists")
    } else {
        AppError::Database(error)
    }
}

impl From<ConnectionRow> for ConnectionView {
    fn from(row: ConnectionRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            actor_id: row.actor_id,
            connection_name: row.connection_name,
            display_name: row.display_name,
            handle: row.handle,
            role: row.role,
            runtime_kind: row.runtime_kind,
            runtime_version: row.runtime_version,
            scopes: row.scopes,
            status: row.status,
            version: row.version,
            last_used_at: row.last_used_at,
            last_error_category: row.last_error_category,
            disabled_at: row.disabled_at,
            removed_at: row.removed_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            webhook_url: row.webhook_url,
            last_wakeup_at: row.last_wakeup_at,
            last_wakeup_error: row.last_wakeup_error,
            wakeup_format: row.wakeup_format,
        }
    }
}

/// WorkOS identity attachment grants access as this named agent, so use the
/// same management policy and transaction locks as credential rotation.
pub(crate) async fn authorize_identity_binding(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    connection_id: Uuid,
) -> Result<Uuid, AppError> {
    lock_project(transaction, project_id).await?;
    let access = load_access_tx(transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_management(principal, &access, project_id)?;
    if access.project_archived {
        return Err(AppError::ProjectArchived);
    }
    let connection = load_connection_tx(transaction, project_id, connection_id).await?;
    if connection.status != "active" {
        return Err(AppError::Forbidden);
    }
    Ok(access.organization_id)
}

/// Refuses handing an agent's identity to someone else (binding an outside
/// identity to the connection) when the agent's role is more than `caller`
/// holds, as creating or re-enabling the agent would be refused.
pub(crate) async fn require_connection_grantable(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    caller_actor_id: Uuid,
    project_id: Uuid,
    connection_id: Uuid,
) -> Result<(), AppError> {
    let connection = load_connection_tx(transaction, project_id, connection_id).await?;
    let role_id = agent_role_id(transaction, project_id, connection.actor_id).await?;
    require_grantable(
        transaction,
        organization_id,
        project_id,
        caller_actor_id,
        &connection.role,
        role_id,
    )
    .await
}

/// The custom role an agent holds in the project, if any.
async fn agent_role_id(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT role_id FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .flatten())
}

#[cfg(test)]
mod tests {
    use super::{CreateConnectionRequest, generate_token, validate_create};

    #[test]
    fn external_tokens_have_distinct_prefix_and_full_entropy_payload() {
        let (first, prefix, digest) = generate_token();
        let (second, _, _) = generate_token();
        assert!(first.starts_with("aw_ext_"));
        assert_eq!(prefix.len(), 16);
        assert_eq!(digest.len(), 32);
        assert_ne!(first, second);
    }

    #[test]
    fn onboarding_normalizes_handle_and_rejects_viewer_writes() {
        let normalized = validate_create(CreateConnectionRequest {
            connection_name: " Hermes planning ".to_owned(),
            display_name: " Hermes ".to_owned(),
            handle: " Hermes Agent ".to_owned(),
            role: "member".to_owned(),
            scopes: vec!["write".to_owned(), "read".to_owned()],
            webhook_url: None,
        })
        .expect("valid member");
        assert_eq!(normalized.handle, "hermes_agent");
        assert_eq!(normalized.scopes, ["read", "write"]);
        assert!(
            validate_create(CreateConnectionRequest {
                connection_name: "Hermes".to_owned(),
                display_name: "Hermes".to_owned(),
                handle: "hermes".to_owned(),
                role: "viewer".to_owned(),
                scopes: vec!["read".to_owned(), "write".to_owned()],
                webhook_url: None,
            })
            .is_err()
        );
    }
}
