use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::WWW_AUTHENTICATE},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use rmcp::{
    ServerHandler,
    handler::server::{
        tool::Extension,
        wrapper::{Json, Parameters},
    },
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

mod oauth;
mod reads;
mod writes;

pub const MCP_PATH: &str = "/api/v1/mcp";
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const TOKEN_BYTES: usize = 32;
const TOKEN_PREFIX_LENGTH: usize = 16;
const MAX_TOKEN_BYTES: usize = 128;
const MAX_MCP_BODY_BYTES: usize = 256 * 1024;
const LAST_USED_THROTTLE_MINUTES: i32 = 5;
#[cfg(test)]
const SERVER_INSTRUCTIONS: &str = "Identify the Project and Work Item by immutable ID or canonical Agent Workspace URL before acting. Read current state before writes; avoid noisy status-only updates. Every non-commutative write requires the current expectedVersion and a stable clientRequestId. On conflict, refetch and retry only when the result says retry is safe. Link pull requests explicitly to the intended Work Item. Permissions are Project-scoped and never authorize cross-Project discovery. Connecting or calling tools never creates an Agent Workspace Thread or hosted run.";

#[derive(Clone, Debug)]
pub struct McpPrincipal {
    pub credential_id: Uuid,
    pub mcp_credential_id: Option<Uuid>,
    pub oauth_grant_id: Option<Uuid>,
    pub actor_id: Uuid,
    pub project_id: Uuid,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub authentication_class: &'static str,
}

impl McpPrincipal {
    pub fn can_read(&self) -> bool {
        self.scopes.iter().any(|scope| scope == "read")
    }

    pub fn can_write(&self) -> bool {
        self.scopes.iter().any(|scope| scope == "write")
    }
}

#[derive(Debug, FromRow)]
struct CredentialAuthRow {
    credential_id: Uuid,
    actor_id: Uuid,
    project_id: Uuid,
    client_name: String,
    token_digest: Vec<u8>,
    scopes: Vec<String>,
}

#[derive(Clone)]
struct McpServer {
    state: AppState,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectGetArguments {
    project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProjectProjection {
    id: String,
    organization_id: String,
    key: String,
    name: String,
    version: i64,
    role: String,
    canonical_url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ProjectGetResult {
    Found { project: ProjectProjection },
    PermissionDenied { retry_category: String },
    NotFound { retry_category: String },
    TemporarilyUnavailable { retry_category: String },
}

#[derive(Debug, FromRow)]
struct ProjectProjectionRow {
    id: Uuid,
    organization_id: Uuid,
    project_key: String,
    name: String,
    version: i64,
    role: String,
}

#[tool_router]
impl McpServer {
    #[tool(
        description = "Read the authenticated Project identity, membership role, and canonical URL",
        annotations(
            title = "Get Project",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn project_get(
        &self,
        Parameters(arguments): Parameters<ProjectGetArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<ProjectGetResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(ProjectGetResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        if !principal.can_read()
            || arguments
                .project_id
                .is_some_and(|project_id| project_id.parse::<Uuid>() != Ok(principal.project_id))
        {
            return Json(ProjectGetResult::NotFound {
                retry_category: "do_not_retry".to_owned(),
            });
        }

        let row = sqlx::query_as::<_, ProjectProjectionRow>(
            r#"
            SELECT
                p.id,
                p.organization_id,
                p.project_key,
                p.name,
                p.version,
                pm.role
            FROM projects p
            JOIN project_memberships pm
              ON pm.project_id = p.id AND pm.actor_id = $2
            JOIN organization_memberships om
              ON om.organization_id = p.organization_id
             AND om.actor_id = $2
             AND om.enabled
            JOIN actors a ON a.id = pm.actor_id
            JOIN users u ON u.actor_id = pm.actor_id AND u.status = 'active'
            WHERE p.id = $1
              AND p.archived_at IS NULL
              AND pm.enabled
              AND a.status = 'active'
              AND a.kind = 'human'
            "#,
        )
        .bind(principal.project_id)
        .bind(principal.actor_id)
        .fetch_optional(&self.state.pool)
        .await;

        match row {
            Ok(Some(row))
                if mcp_policy_allows(
                    principal.actor_id,
                    principal.project_id,
                    &row.role,
                    Action::McpRead,
                ) =>
            {
                Json(ProjectGetResult::Found {
                    project: ProjectProjection {
                        id: row.id.to_string(),
                        organization_id: row.organization_id.to_string(),
                        key: row.project_key,
                        name: row.name,
                        version: row.version,
                        role: row.role,
                        canonical_url: format!(
                            "http://{}/projects/{}",
                            self.state.config.server_addr, row.id
                        ),
                    },
                })
            }
            Ok(Some(_)) => Json(ProjectGetResult::NotFound {
                retry_category: "do_not_retry".to_owned(),
            }),
            Ok(None) => Json(ProjectGetResult::NotFound {
                retry_category: "do_not_retry".to_owned(),
            }),
            Err(error) => {
                tracing::error!(error = %error, "MCP project_get failed");
                Json(ProjectGetResult::TemporarilyUnavailable {
                    retry_category: "retry_with_backoff".to_owned(),
                })
            }
        }
    }

    #[tool(
        description = "Read bounded Project guidance for local development work",
        annotations(
            title = "Get Project instructions",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn project_instructions_get(
        &self,
        Parameters(_arguments): Parameters<reads::ProjectInstructionsArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::ProjectInstructionsProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::project_instructions_get(&self.state, principal).await)
    }

    #[tool(
        description = "Read one bounded Work Item by immutable ID, canonical URL, or Project-scoped key",
        annotations(
            title = "Get Work Item",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_get(
        &self,
        Parameters(arguments): Parameters<reads::WorkItemGetArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::WorkItemProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::work_item_get(&self.state, principal, &arguments.identifier).await)
    }

    #[tool(
        description = "Search bounded Work Item summaries deterministically inside the authenticated Project",
        annotations(
            title = "Search Work Items",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_search(
        &self,
        Parameters(arguments): Parameters<reads::WorkItemSearchArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::WorkItemSearchProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::work_item_search(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Compose bounded Project guidance and current Work Item context with source versions",
        annotations(
            title = "Get Work Item context",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_context_get(
        &self,
        Parameters(arguments): Parameters<reads::WorkItemGetArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::WorkItemContextProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::work_item_context_get(&self.state, principal, &arguments.identifier).await)
    }

    #[tool(
        description = "Read bounded trusted Repository metadata without host paths or repository content",
        annotations(
            title = "Get Repository",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn repository_get(
        &self,
        Parameters(arguments): Parameters<reads::RepositoryGetArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::RepositoryProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::repository_get(&self.state, principal, &arguments.repository_id).await)
    }

    #[tool(
        description = "Read the bounded canonical pull-request projection by immutable ID or GitHub URL",
        annotations(
            title = "Get pull request",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn pull_request_get(
        &self,
        Parameters(arguments): Parameters<reads::PullRequestGetArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<reads::ReadResult<reads::PullRequestProjection>> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(reads::ReadResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(reads::pull_request_get(&self.state, principal, &arguments.identifier).await)
    }

    #[tool(
        description = "Move a Work Item through its Project-owned workflow using its current version",
        annotations(
            title = "Set Work Item status",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_set_status(
        &self,
        Parameters(arguments): Parameters<writes::SetStatusArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::set_status(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Append one useful bounded update to a Work Item without replacing canonical fields",
        annotations(
            title = "Post Work Item update",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_post_update(
        &self,
        Parameters(arguments): Parameters<writes::PostUpdateArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::post_update(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Replace bounded canonical Work Item fields with optimistic concurrency",
        annotations(
            title = "Update Work Item",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_update(
        &self,
        Parameters(arguments): Parameters<writes::UpdateWorkItemArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::update_work_item(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Create one additive subtask under an immutable parent Work Item",
        annotations(
            title = "Create Work Item subtask",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_create_subtask(
        &self,
        Parameters(arguments): Parameters<writes::CreateSubtaskArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::create_subtask(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Explicitly link one existing canonical GitHub pull request to a Work Item",
        annotations(
            title = "Link pull request to Work Item",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn work_item_link_pull_request(
        &self,
        Parameters(arguments): Parameters<writes::LinkPullRequestArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::link_pull_request(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Mark a Work Item blocked with a bounded actionable reason",
        annotations(
            title = "Mark Work Item blocked",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_mark_blocked(
        &self,
        Parameters(arguments): Parameters<writes::MarkBlockedArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::mark_blocked(&self.state, principal, arguments).await)
    }

    #[tool(
        description = "Clear a blocker and move a Work Item through an allowed Review transition",
        annotations(
            title = "Mark Work Item ready for review",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn work_item_mark_ready_for_review(
        &self,
        Parameters(arguments): Parameters<writes::MarkReadyForReviewArguments>,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Json<writes::WriteResult> {
        let Some(principal) = parts.extensions.get::<McpPrincipal>() else {
            return Json(writes::WriteResult::PermissionDenied {
                retry_category: "reauthenticate".to_owned(),
            });
        };
        Json(writes::mark_ready_for_review(&self.state, principal, arguments).await)
    }
}

#[tool_handler(
    name = "agent-workspace",
    version = "0.6.0",
    instructions = "Identify the Project and Work Item by immutable ID or canonical Agent Workspace URL before acting. Read current state before writes; avoid noisy status-only updates. Every non-commutative write requires the current expectedVersion and a stable clientRequestId. On conflict, refetch and retry only when the result says retry is safe. Link pull requests explicitly to the intended Work Item. Permissions are Project-scoped and never authorize cross-Project discovery. Connecting or calling tools never creates an Agent Workspace Thread or hosted run."
)]
impl ServerHandler for McpServer {}

pub fn router(state: AppState) -> Router<AppState> {
    let handler = McpServer {
        state: state.clone(),
    };
    let service: StreamableHttpService<McpServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(["localhost", "127.0.0.1", "[::1]"])
            .with_allowed_origins(state.config.allowed_origins.clone())
            .with_legacy_session_mode(true)
            .with_json_response(true)
            .with_max_request_body_bytes(MAX_MCP_BODY_BYTES)
            .with_cancellation_token(CancellationToken::new()),
    );

    Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(state, authenticate_mcp))
}

pub fn oauth_router() -> Router<AppState> {
    oauth::router()
}

async fn authenticate_mcp(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let authentication_key = exact_bearer_token(request.headers()).unwrap_or("invalid");
    if let Err(error) = state.rate_limits.check_mcp_auth(authentication_key) {
        return error.into_response();
    }
    match resolve_bearer_principal(&state, request.headers()).await {
        Ok(Some(principal)) => {
            if let Err(error) = state.rate_limits.check_mcp_request(principal.credential_id) {
                return error.into_response();
            }
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Ok(None) => unauthorized_response(&state),
        Err(error) => {
            tracing::error!(error = %error, "MCP credential lookup failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

fn unauthorized_response(state: &AppState) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    let challenge = format!(
        "Bearer resource_metadata=\"http://{}/.well-known/oauth-protected-resource/api/v1/mcp\"",
        state.config.server_addr
    );
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, value);
    }
    response
}

async fn resolve_bearer_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<McpPrincipal>, AppError> {
    let Some(token) = exact_bearer_token(headers) else {
        return Ok(None);
    };
    if token.starts_with("aw_oauth_") {
        return oauth::resolve_access_token(state, token).await;
    }
    let Some(prefix) = token.get(..TOKEN_PREFIX_LENGTH) else {
        return Ok(None);
    };
    let candidate_digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let rows = sqlx::query_as::<_, CredentialAuthRow>(
        r#"
        SELECT
            c.id AS credential_id,
            c.actor_id,
            c.project_id,
            c.client_name,
            c.token_digest,
            c.scopes
        FROM mcp_client_credentials c
        JOIN actors a ON a.id = c.actor_id
        JOIN users u ON u.actor_id = c.actor_id AND u.status = 'active'
        JOIN projects p ON p.id = c.project_id
        JOIN project_memberships pm
          ON pm.project_id = c.project_id AND pm.actor_id = c.actor_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = c.actor_id
         AND om.enabled
        WHERE c.token_prefix = $1
          AND c.status = 'active'
          AND c.revoked_at IS NULL
          AND c.expires_at > now()
          AND a.kind = 'human'
          AND a.status = 'active'
          AND p.archived_at IS NULL
          AND pm.enabled
        "#,
    )
    .bind(prefix)
    .fetch_all(&state.pool)
    .await?;

    let matched = rows.into_iter().find(|row| {
        row.token_digest.len() == candidate_digest.len()
            && bool::from(row.token_digest.as_slice().ct_eq(&candidate_digest))
    });
    let Some(row) = matched else {
        return Ok(None);
    };

    sqlx::query(
        r#"
        UPDATE mcp_client_credentials
        SET last_used_at = now(), updated_at = now()
        WHERE id = $1
          AND (last_used_at IS NULL OR last_used_at < now() - make_interval(mins => $2::integer))
        "#,
    )
    .bind(row.credential_id)
    .bind(LAST_USED_THROTTLE_MINUTES)
    .execute(&state.pool)
    .await?;

    Ok(Some(McpPrincipal {
        credential_id: row.credential_id,
        mcp_credential_id: Some(row.credential_id),
        oauth_grant_id: None,
        actor_id: row.actor_id,
        project_id: row.project_id,
        client_name: row.client_name,
        scopes: row.scopes,
        authentication_class: "mcp_bearer",
    }))
}

fn exact_bearer_token(headers: &HeaderMap) -> Option<&str> {
    if headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .count()
        != 1
    {
        return None;
    }
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(token)
}

#[derive(Clone, Debug)]
pub struct IssueCredentialCommand {
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedCredential {
    pub credential_id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub token: String,
    pub token_prefix: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetadata {
    pub id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub client_name: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub status: String,
    pub version: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub replaced_by_credential_id: Option<Uuid>,
}

pub async fn issue_credential(
    pool: &sqlx::PgPool,
    command: IssueCredentialCommand,
) -> Result<IssuedCredential, AppError> {
    validate_issue_command(pool, &command).await?;
    let (token, token_prefix, token_digest) = generate_token();
    let credential_id = Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    let expires_at = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO mcp_client_credentials (
            id, project_id, actor_id, client_name, token_prefix, token_digest,
            scopes, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(days => $8))
        RETURNING expires_at::text
        "#,
    )
    .bind(credential_id)
    .bind(command.project_id)
    .bind(command.actor_id)
    .bind(&command.client_name)
    .bind(&token_prefix)
    .bind(token_digest.to_vec())
    .bind(&command.scopes)
    .bind(i32::from(command.expires_in_days))
    .fetch_one(&mut *transaction)
    .await?;
    insert_credential_audit(
        &mut transaction,
        command.actor_id,
        command.project_id,
        credential_id,
        &command.client_name,
        "mcp.credential.create",
    )
    .await?;
    transaction.commit().await?;

    Ok(IssuedCredential {
        credential_id,
        project_id: command.project_id,
        actor_id: command.actor_id,
        client_name: command.client_name,
        scopes: command.scopes,
        token,
        token_prefix,
        expires_at,
    })
}

pub async fn list_credentials(
    pool: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Vec<CredentialMetadata>, AppError> {
    Ok(sqlx::query_as::<_, CredentialMetadata>(
        r#"
        SELECT
            id,
            project_id,
            actor_id,
            client_name,
            token_prefix,
            scopes,
            status,
            version,
            created_at::text AS created_at,
            last_used_at::text AS last_used_at,
            expires_at::text AS expires_at,
            revoked_at::text AS revoked_at,
            replaced_by_credential_id
        FROM mcp_client_credentials
        WHERE project_id = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 100
        "#,
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?)
}

pub async fn revoke_credential(
    pool: &sqlx::PgPool,
    credential_id: Uuid,
    expected_version: i64,
) -> Result<CredentialMetadata, AppError> {
    let mut transaction = pool.begin().await?;
    let row = credential_metadata_for_update(&mut transaction, credential_id).await?;
    if row.version != expected_version || row.status != "active" {
        return Err(AppError::Conflict(
            "mcp_credential",
            "stale version or revoked",
        ));
    }
    sqlx::query(
        r#"
        UPDATE mcp_client_credentials
        SET status = 'revoked', revoked_at = now(), version = version + 1, updated_at = now()
        WHERE id = $1 AND version = $2 AND status = 'active'
        "#,
    )
    .bind(credential_id)
    .bind(expected_version)
    .execute(&mut *transaction)
    .await?;
    insert_credential_audit(
        &mut transaction,
        row.actor_id,
        row.project_id,
        credential_id,
        &row.client_name,
        "mcp.credential.revoke",
    )
    .await?;
    transaction.commit().await?;
    list_credentials(pool, row.project_id)
        .await?
        .into_iter()
        .find(|credential| credential.id == credential_id)
        .ok_or(AppError::NotFound("mcp_credential"))
}

pub async fn replace_credential(
    pool: &sqlx::PgPool,
    credential_id: Uuid,
    expected_version: i64,
    expires_in_days: u16,
) -> Result<IssuedCredential, AppError> {
    if !(1..=365).contains(&expires_in_days) {
        return Err(AppError::Invalid("expires_in_days"));
    }
    let (token, token_prefix, token_digest) = generate_token();
    let replacement_id = Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    let row = credential_metadata_for_update(&mut transaction, credential_id).await?;
    if row.version != expected_version || row.status != "active" {
        return Err(AppError::Conflict(
            "mcp_credential",
            "stale version or revoked",
        ));
    }
    let expires_at = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO mcp_client_credentials (
            id, project_id, actor_id, client_name, token_prefix, token_digest,
            scopes, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(days => $8))
        RETURNING expires_at::text
        "#,
    )
    .bind(replacement_id)
    .bind(row.project_id)
    .bind(row.actor_id)
    .bind(&row.client_name)
    .bind(&token_prefix)
    .bind(token_digest.to_vec())
    .bind(&row.scopes)
    .bind(i32::from(expires_in_days))
    .fetch_one(&mut *transaction)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE mcp_client_credentials
        SET status = 'revoked', revoked_at = now(), replaced_by_credential_id = $3,
            version = version + 1, updated_at = now()
        WHERE id = $1 AND version = $2 AND status = 'active'
        "#,
    )
    .bind(credential_id)
    .bind(expected_version)
    .bind(replacement_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "mcp_credential",
            "stale version or revoked",
        ));
    }
    insert_credential_audit(
        &mut transaction,
        row.actor_id,
        row.project_id,
        credential_id,
        &row.client_name,
        "mcp.credential.replace",
    )
    .await?;
    transaction.commit().await?;

    Ok(IssuedCredential {
        credential_id: replacement_id,
        project_id: row.project_id,
        actor_id: row.actor_id,
        client_name: row.client_name,
        scopes: row.scopes,
        token,
        token_prefix,
        expires_at,
    })
}

async fn validate_issue_command(
    pool: &sqlx::PgPool,
    command: &IssueCredentialCommand,
) -> Result<(), AppError> {
    if command.client_name.trim() != command.client_name
        || command.client_name.is_empty()
        || command.client_name.chars().count() > 80
        || !(1..=365).contains(&command.expires_in_days)
    {
        return Err(AppError::Invalid("mcp_credential"));
    }
    let mut scopes = command.scopes.clone();
    scopes.sort();
    scopes.dedup();
    if scopes != command.scopes
        || !scopes
            .iter()
            .all(|scope| scope == "read" || scope == "write")
        || !scopes.iter().any(|scope| scope == "read")
    {
        return Err(AppError::Invalid("mcp_scopes"));
    }
    let role = sqlx::query_scalar::<_, String>(
        r#"
        SELECT pm.role
        FROM project_memberships pm
        JOIN projects p ON p.id = pm.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = pm.actor_id
         AND om.enabled
        JOIN actors a ON a.id = pm.actor_id
        JOIN users u ON u.actor_id = pm.actor_id AND u.status = 'active'
        WHERE pm.project_id = $1
          AND pm.actor_id = $2
          AND pm.enabled
          AND p.archived_at IS NULL
          AND a.kind = 'human'
          AND a.status = 'active'
        "#,
    )
    .bind(command.project_id)
    .bind(command.actor_id)
    .fetch_optional(pool)
    .await?;
    let action = if command.scopes.iter().any(|scope| scope == "write") {
        Action::McpWrite
    } else {
        Action::McpRead
    };
    if !role
        .is_some_and(|role| mcp_policy_allows(command.actor_id, command.project_id, &role, action))
    {
        return Err(AppError::NotFound("project_membership"));
    }
    Ok(())
}

fn mcp_policy_allows(actor_id: Uuid, project_id: Uuid, role: &str, action: Action) -> bool {
    let role = match role {
        "owner" => MembershipRole::Owner,
        "admin" => MembershipRole::Admin,
        "member" => MembershipRole::Member,
        "viewer" => MembershipRole::Viewer,
        _ => return false,
    };
    matches!(
        authorize(
            &Subject::new(
                actor_id,
                ActorKind::Human,
                true,
                Some(Membership::project(project_id, role, true)),
                None,
            ),
            action,
            Resource::Project(ProjectScope::project(project_id)),
        ),
        Decision::Allow
    )
}

fn generate_token() -> (String, String, [u8; 32]) {
    let mut bytes = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("aw_mcp_{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = token[..TOKEN_PREFIX_LENGTH].to_owned();
    let digest = Sha256::digest(token.as_bytes()).into();
    (token, prefix, digest)
}

async fn credential_metadata_for_update(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credential_id: Uuid,
) -> Result<CredentialMetadata, AppError> {
    sqlx::query_as::<_, CredentialMetadata>(
        r#"
        SELECT
            id,
            project_id,
            actor_id,
            client_name,
            token_prefix,
            scopes,
            status,
            version,
            created_at::text AS created_at,
            last_used_at::text AS last_used_at,
            expires_at::text AS expires_at,
            revoked_at::text AS revoked_at,
            replaced_by_credential_id
        FROM mcp_client_credentials
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(credential_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("mcp_credential"))
}

async fn insert_credential_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    credential_id: Uuid,
    client_name: &str,
    action: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action,
            target_kind, target_id, outcome, channel, client_name,
            mcp_credential_id, metadata
        )
        VALUES (
            $1, $2, $3, 'host_cli', $4,
            'mcp_credential', $5, 'succeeded', 'host_cli', $6,
            $5, '{}'::jsonb
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(project_id)
    .bind(action)
    .bind(credential_id)
    .bind(client_name)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::{SERVER_INSTRUCTIONS, exact_bearer_token, generate_token};

    #[test]
    fn generated_token_has_256_bits_and_only_safe_prefix_metadata() {
        let (token, prefix, digest) = generate_token();
        assert!(token.starts_with("aw_mcp_"));
        assert_eq!(prefix.len(), 16);
        assert_eq!(digest.len(), 32);
        assert!(token.len() >= 50);
        assert!(!prefix.contains(' '));
    }

    #[test]
    fn bearer_syntax_is_exact_and_bounded() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer token"));
        assert_eq!(exact_bearer_token(&headers), None);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer token extra"),
        );
        assert_eq!(exact_bearer_token(&headers), None);
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer token"));
        assert_eq!(exact_bearer_token(&headers), Some("token"));
        headers.append(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer second-token"),
        );
        assert_eq!(exact_bearer_token(&headers), None);
    }

    #[test]
    fn instructions_lead_with_the_safety_contract() {
        let first = &SERVER_INSTRUCTIONS[..SERVER_INSTRUCTIONS.len().min(512)];
        for required in [
            "Project and Work Item",
            "avoid noisy status-only updates",
            "expectedVersion",
            "Link pull requests explicitly",
            "Permissions are Project-scoped",
        ] {
            assert!(first.contains(required), "missing {required}");
        }
    }
}
