use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::redirect::Policy;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    config::{GITHUB_API_VERSION, GitHubConfig},
    error::{ApiJson, ApiPath, AppError},
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
};

const MAX_WEBHOOK_BODY_BYTES: usize = 1024 * 1024;
const MAX_GITHUB_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUIRED_EVENTS: [&str; 5] = [
    "push",
    "pull_request",
    "pull_request_review",
    "check_suite",
    "check_run",
];
const PERMITTED_EVENTS: [&str; 6] = [
    "push",
    "pull_request",
    "pull_request_review",
    "check_suite",
    "check_run",
    "repository",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/integrations/github",
            get(github_status),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/sync",
            post(sync_installation),
        )
}

pub fn webhook_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/integrations/github/webhook", post(receive_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
        .with_state(state)
}

#[derive(Debug, FromRow)]
struct ProjectAccess {
    role: String,
}

#[derive(Clone, Debug, FromRow)]
struct InstallationRow {
    id: Uuid,
    app_id: i64,
    installation_id: i64,
    account_id: i64,
    account_login: String,
    target_type: String,
    repository_selection: String,
    metadata_permission: String,
    contents_permission: String,
    pull_requests_permission: String,
    checks_permission: String,
    actions_permission: Option<String>,
    subscribed_events: Vec<String>,
    state: String,
    enabled: bool,
    version: i64,
    last_attempt_at: String,
    last_success_at: Option<String>,
    failure_category: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
struct InstallationRepositoryRow {
    id: Uuid,
    github_repository_id: i64,
    owner: String,
    name: String,
    access_state: String,
    last_verified_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitHubStatusResponse {
    configured: bool,
    api_version: &'static str,
    installation: Option<InstallationView>,
    capabilities: GitHubCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationView {
    id: Uuid,
    app_id: i64,
    installation_id: i64,
    account_id: i64,
    account_login: String,
    target_type: String,
    repository_selection: String,
    permissions: PermissionView,
    subscribed_events: Vec<String>,
    state: String,
    enabled: bool,
    version: i64,
    last_attempt_at: String,
    last_success_at: Option<String>,
    failure_category: Option<String>,
    repositories: Vec<InstallationRepositoryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionView {
    metadata: String,
    contents: String,
    pull_requests: String,
    checks: String,
    actions: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationRepositoryView {
    id: Uuid,
    github_repository_id: i64,
    owner: String,
    name: String,
    access_state: String,
    last_verified_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitHubCapabilities {
    manage: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncInstallationRequest {
    installation_id: u64,
    repository_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct GitHubInstallation {
    id: u64,
    app_id: u64,
    account: GitHubAccount,
    target_type: String,
    repository_selection: String,
    permissions: BTreeMap<String, String>,
    events: Vec<String>,
    suspended_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubAccount {
    id: u64,
    login: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: String,
    expires_at: String,
}

struct InstallationToken {
    secret: SecretString,
    _expires_at: String,
}

impl InstallationToken {
    fn expose(&self) -> &str {
        self.secret.expose_secret()
    }
}

impl std::fmt::Debug for InstallationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstallationToken([REDACTED])")
    }
}

#[derive(Debug, Deserialize)]
struct InstallationRepositories {
    repositories: Vec<GitHubRepository>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepository {
    id: u64,
    full_name: String,
}

#[derive(Debug, Serialize)]
struct AppClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[derive(Clone)]
struct GitHubClient {
    http: reqwest::Client,
    config: Arc<GitHubConfig>,
}

#[derive(Debug, thiserror::Error)]
enum GitHubError {
    #[error("GitHub credential material is unavailable")]
    Credential,
    #[error("GitHub App JWT could not be generated")]
    Jwt,
    #[error("GitHub request failed")]
    Http,
    #[error("GitHub response was rejected")]
    Response,
    #[error("GitHub installation scope is incompatible")]
    Scope,
}

impl GitHubClient {
    fn new(config: &GitHubConfig) -> Result<Self, GitHubError> {
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent("agent-workspace-m5")
            .build()
            .map_err(|_| GitHubError::Http)?;
        Ok(Self {
            http,
            config: Arc::new(config.clone()),
        })
    }

    fn app_jwt(&self) -> Result<SecretString, GitHubError> {
        let private_key = read_secret_file(&self.config.private_key_path, 64 * 1024)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GitHubError::Jwt)?
            .as_secs();
        let claims = AppClaims {
            iat: now.saturating_sub(60),
            exp: now + 9 * 60,
            iss: self.config.app_id.to_string(),
        };
        let encoding_key = EncodingKey::from_rsa_pem(&private_key).map_err(|_| GitHubError::Jwt)?;
        let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &encoding_key)
            .map_err(|_| GitHubError::Jwt)?;
        Ok(SecretString::from(jwt))
    }

    async fn installation(&self, installation_id: u64) -> Result<GitHubInstallation, GitHubError> {
        let jwt = self.app_jwt()?;
        let response = self
            .http
            .get(format!(
                "{}/app/installations/{installation_id}",
                self.config.api_base_url
            ))
            .headers(github_headers(jwt.expose_secret())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        decode_response(response).await
    }

    async fn installation_token(
        &self,
        installation_id: u64,
        repository_ids: &[u64],
    ) -> Result<InstallationToken, GitHubError> {
        let jwt = self.app_jwt()?;
        let response = self
            .http
            .post(format!(
                "{}/app/installations/{installation_id}/access_tokens",
                self.config.api_base_url
            ))
            .headers(github_headers(jwt.expose_secret())?)
            .json(&json!({
                "repository_ids": repository_ids,
                "permissions": {
                    "metadata": "read",
                    "contents": "write",
                    "pull_requests": "write",
                    "checks": "read"
                }
            }))
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        let token: TokenResponse = decode_response(response).await?;
        if token.token.is_empty() || token.token.len() > 16 * 1024 {
            return Err(GitHubError::Response);
        }
        Ok(InstallationToken {
            secret: SecretString::from(token.token),
            _expires_at: token.expires_at,
        })
    }

    async fn repositories(
        &self,
        token: &InstallationToken,
    ) -> Result<Vec<GitHubRepository>, GitHubError> {
        let response = self
            .http
            .get(format!(
                "{}/installation/repositories?per_page=100",
                self.config.api_base_url
            ))
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        let response: InstallationRepositories = decode_response(response).await?;
        if response.repositories.len() > 100 {
            return Err(GitHubError::Response);
        }
        Ok(response.repositories)
    }
}

async fn decode_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, GitHubError> {
    if !response.status().is_success() {
        return Err(GitHubError::Response);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_GITHUB_RESPONSE_BYTES as u64)
    {
        return Err(GitHubError::Response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| GitHubError::Http)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_GITHUB_RESPONSE_BYTES {
            return Err(GitHubError::Response);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| GitHubError::Response)
}

fn github_headers(bearer: &str) -> Result<HeaderMap, GitHubError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static(GITHUB_API_VERSION),
    );
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {bearer}")).map_err(|_| GitHubError::Credential)?,
    );
    Ok(headers)
}

fn read_secret_file(
    path: &std::path::Path,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, GitHubError> {
    let bytes = fs::read(path).map_err(|_| GitHubError::Credential)?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(GitHubError::Credential);
    }
    Ok(Zeroizing::new(bytes))
}

async fn github_status(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<GitHubStatusResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectManage,
    )
    .await?;
    let installation = load_installation(&mut transaction, project_id).await?;
    let repositories = if let Some(installation) = &installation {
        load_installation_repositories(&mut transaction, project_id, installation.id).await?
    } else {
        Vec::new()
    };
    transaction.commit().await?;

    Ok(Json(GitHubStatusResponse {
        configured: state.config.github.is_some(),
        api_version: GITHUB_API_VERSION,
        installation: installation
            .map(|installation| installation_view(installation, repositories)),
        capabilities: GitHubCapabilities {
            manage: role_allows_manage(role),
        },
    }))
}

async fn sync_installation(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SyncInstallationRequest>,
) -> Result<Json<GitHubStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.installation_id == 0
        || request.repository_ids.is_empty()
        || request.repository_ids.len() > 50
    {
        return Err(AppError::Validation("githubInstallation"));
    }
    let mut requested_repository_ids = request.repository_ids.clone();
    requested_repository_ids.sort_unstable();
    requested_repository_ids.dedup();
    if requested_repository_ids.len() != request.repository_ids.len() {
        return Err(AppError::Validation("repositoryIds"));
    }
    let github_config = state
        .config
        .github
        .as_ref()
        .ok_or(AppError::Conflict("github_integration", "not_configured"))?;

    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectManage,
    )
    .await?;
    let repositories =
        load_requested_repositories(&mut transaction, project_id, &requested_repository_ids)
            .await?;
    transaction.commit().await?;
    if repositories.len() != requested_repository_ids.len() {
        return Err(AppError::NotFound("repository"));
    }

    let client = GitHubClient::new(github_config).map_err(map_github_error)?;
    let installation = client
        .installation(request.installation_id)
        .await
        .map_err(map_github_error)?;
    validate_installation(github_config, &installation).map_err(map_github_error)?;
    let github_repository_ids = repositories
        .iter()
        .map(|repository| {
            u64::try_from(repository.github_repository_id)
                .map_err(|_| AppError::Internal("GitHub Repository ID"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let token = client
        .installation_token(request.installation_id, &github_repository_ids)
        .await
        .map_err(map_github_error)?;
    let accessible = client
        .repositories(&token)
        .await
        .map_err(map_github_error)?;
    validate_repository_access(&repositories, &accessible).map_err(map_github_error)?;
    drop(token);

    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectManage,
    )
    .await?;
    let current_repositories =
        load_requested_repositories(&mut transaction, project_id, &requested_repository_ids)
            .await?;
    if current_repositories != repositories {
        return Err(AppError::Conflict("repository", "changed_during_sync"));
    }
    let actor_id = ensure_github_actor(&mut transaction, project_id).await?;
    let installation_id = persist_installation(
        &mut transaction,
        project_id,
        actor_id,
        github_config,
        &installation,
    )
    .await?;
    sqlx::query(
        "UPDATE github_installation_repositories SET access_state = 'missing', last_verified_at = NULL, updated_at = now() WHERE project_id = $1 AND github_installation_id = $2",
    )
    .bind(project_id)
    .bind(installation_id)
    .execute(&mut *transaction)
    .await?;
    for repository in &repositories {
        sqlx::query(
            r#"
            INSERT INTO github_installation_repositories (
                project_id, github_installation_id, repository_id,
                github_repository_id, access_state, last_verified_at
            ) VALUES ($1, $2, $3, $4, 'selected', now())
            ON CONFLICT (github_installation_id, repository_id) DO UPDATE
                SET github_repository_id = EXCLUDED.github_repository_id,
                    access_state = 'selected',
                    last_verified_at = now(),
                    updated_at = now()
            "#,
        )
        .bind(project_id)
        .bind(installation_id)
        .bind(repository.id)
        .bind(repository.github_repository_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE repositories SET github_app_installation_id = $3, updated_at = now() WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(repository.id)
        .bind(i64::try_from(request.installation_id).map_err(|_| AppError::Validation("installationId"))?)
        .execute(&mut *transaction)
        .await?;
    }
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session',
            'github.installation_sync', 'github_installation', $5,
            'succeeded', $6, jsonb_build_object('repository_count', $7::integer)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(installation_id)
    .bind(request_id)
    .bind(i32::try_from(repositories.len()).map_err(|_| AppError::Internal("repository count"))?)
    .execute(&mut *transaction)
    .await?;
    let installation_row = load_installation(&mut transaction, project_id)
        .await?
        .ok_or(AppError::Internal("persisted GitHub installation"))?;
    let repository_rows =
        load_installation_repositories(&mut transaction, project_id, installation_row.id).await?;
    transaction.commit().await?;

    Ok(Json(GitHubStatusResponse {
        configured: true,
        api_version: GITHUB_API_VERSION,
        installation: Some(installation_view(installation_row, repository_rows)),
        capabilities: GitHubCapabilities {
            manage: role_allows_manage(role),
        },
    }))
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
struct RegisteredRepository {
    id: Uuid,
    github_repository_id: i64,
    owner: String,
    name: String,
}

async fn load_requested_repositories(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    repository_ids: &[Uuid],
) -> Result<Vec<RegisteredRepository>, AppError> {
    Ok(sqlx::query_as::<_, RegisteredRepository>(
        r#"
        SELECT id, github_repository_id, owner, name
        FROM repositories
        WHERE project_id = $1
          AND id = ANY($2)
          AND enabled
          AND trusted
          AND archived_at IS NULL
          AND github_repository_id IS NOT NULL
        ORDER BY id
        FOR SHARE
        "#,
    )
    .bind(project_id)
    .bind(repository_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

fn validate_installation(
    config: &GitHubConfig,
    installation: &GitHubInstallation,
) -> Result<(), GitHubError> {
    if installation.id == 0
        || installation.app_id != config.app_id
        || installation.account.id == 0
        || installation.account.login.is_empty()
        || installation.account.login.len() > 100
        || installation.account.login.chars().any(char::is_control)
        || !matches!(installation.target_type.as_str(), "User" | "Organization")
        || installation.repository_selection != "selected"
        || installation.suspended_at.is_some()
    {
        return Err(GitHubError::Scope);
    }
    let required_permissions = [
        ("metadata", "read"),
        ("contents", "write"),
        ("pull_requests", "write"),
        ("checks", "read"),
    ];
    for (name, level) in required_permissions {
        if installation.permissions.get(name).map(String::as_str) != Some(level) {
            return Err(GitHubError::Scope);
        }
    }
    for (name, level) in &installation.permissions {
        let permitted = required_permissions.contains(&(name.as_str(), level.as_str()))
            || (name == "actions" && level == "read");
        if !permitted && level != "none" {
            return Err(GitHubError::Scope);
        }
    }
    let events = installation
        .events
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if installation.events.len() != events.len()
        || events.len() > PERMITTED_EVENTS.len()
        || !REQUIRED_EVENTS.iter().all(|event| events.contains(event))
        || !events.iter().all(|event| PERMITTED_EVENTS.contains(event))
    {
        return Err(GitHubError::Scope);
    }
    Ok(())
}

fn validate_repository_access(
    expected: &[RegisteredRepository],
    accessible: &[GitHubRepository],
) -> Result<(), GitHubError> {
    let expected = expected
        .iter()
        .map(|repository| {
            (
                repository.github_repository_id as u64,
                format!("{}/{}", repository.owner, repository.name),
            )
        })
        .collect::<BTreeSet<_>>();
    let actual = accessible
        .iter()
        .map(|repository| (repository.id, repository.full_name.clone()))
        .collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(GitHubError::Scope);
    }
    Ok(())
}

async fn ensure_github_actor(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    if let Some((actor_id, kind)) = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT membership.actor_id, actor.kind
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND lower(membership.handle) = 'github_integration'
        FOR UPDATE OF membership, actor
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    {
        if kind != "integration" {
            return Err(AppError::Conflict("github_integration", "handle_in_use"));
        }
        sqlx::query("UPDATE actors SET status = 'active', updated_at = now() WHERE id = $1")
            .bind(actor_id)
            .execute(&mut **transaction)
            .await?;
        sqlx::query(
            "UPDATE project_memberships SET enabled = true, updated_at = now() WHERE project_id = $1 AND actor_id = $2",
        )
        .bind(project_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
        return Ok(actor_id);
    }
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle, enabled) VALUES ($1, $2, $3, 'member', 'GitHub', 'github_integration', true)",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(actor_id)
}

async fn persist_installation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    config: &GitHubConfig,
    installation: &GitHubInstallation,
) -> Result<Uuid, AppError> {
    let app_id = i64::try_from(config.app_id).map_err(|_| AppError::Validation("appId"))?;
    let installation_id =
        i64::try_from(installation.id).map_err(|_| AppError::Validation("installationId"))?;
    let account_id =
        i64::try_from(installation.account.id).map_err(|_| AppError::Validation("accountId"))?;
    let actions = installation.permissions.get("actions").cloned();
    let row_id = Uuid::now_v7();
    let id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO github_installations (
            id, project_id, integration_actor_id, app_id, installation_id,
            account_id, account_login, target_type, repository_selection,
            metadata_permission, contents_permission, pull_requests_permission,
            checks_permission, actions_permission, subscribed_events, state,
            enabled, last_attempt_at, last_success_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'selected',
            'read', 'write', 'write', 'read', $9, $10, 'available',
            true, now(), now()
        )
        ON CONFLICT (project_id, installation_id) DO UPDATE
            SET account_login = EXCLUDED.account_login,
                target_type = EXCLUDED.target_type,
                repository_selection = EXCLUDED.repository_selection,
                metadata_permission = EXCLUDED.metadata_permission,
                contents_permission = EXCLUDED.contents_permission,
                pull_requests_permission = EXCLUDED.pull_requests_permission,
                checks_permission = EXCLUDED.checks_permission,
                actions_permission = EXCLUDED.actions_permission,
                subscribed_events = EXCLUDED.subscribed_events,
                state = 'available', enabled = true,
                version = github_installations.version + 1,
                last_attempt_at = now(), last_success_at = now(),
                failure_category = NULL, updated_at = now()
        RETURNING id
        "#,
    )
    .bind(row_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(app_id)
    .bind(installation_id)
    .bind(account_id)
    .bind(&installation.account.login)
    .bind(&installation.target_type)
    .bind(actions)
    .bind(&installation.events)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(id)
}

async fn load_installation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Option<InstallationRow>, AppError> {
    Ok(sqlx::query_as::<_, InstallationRow>(
        r#"
        SELECT id, app_id, installation_id, account_id, account_login,
               target_type, repository_selection, metadata_permission,
               contents_permission, pull_requests_permission,
               checks_permission, actions_permission, subscribed_events,
               state, enabled, version, last_attempt_at::text AS last_attempt_at,
               last_success_at::text AS last_success_at, failure_category
        FROM github_installations
        WHERE project_id = $1 AND enabled
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn load_installation_repositories(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    installation_id: Uuid,
) -> Result<Vec<InstallationRepositoryRow>, AppError> {
    Ok(sqlx::query_as::<_, InstallationRepositoryRow>(
        r#"
        SELECT repository.id, link.github_repository_id, repository.owner,
               repository.name, link.access_state,
               link.last_verified_at::text AS last_verified_at
        FROM github_installation_repositories link
        JOIN repositories repository
          ON repository.project_id = link.project_id
         AND repository.id = link.repository_id
        WHERE link.project_id = $1 AND link.github_installation_id = $2
        ORDER BY lower(repository.owner), lower(repository.name), repository.id
        "#,
    )
    .bind(project_id)
    .bind(installation_id)
    .fetch_all(&mut **transaction)
    .await?)
}

fn installation_view(
    row: InstallationRow,
    repositories: Vec<InstallationRepositoryRow>,
) -> InstallationView {
    InstallationView {
        id: row.id,
        app_id: row.app_id,
        installation_id: row.installation_id,
        account_id: row.account_id,
        account_login: row.account_login,
        target_type: row.target_type,
        repository_selection: row.repository_selection,
        permissions: PermissionView {
            metadata: row.metadata_permission,
            contents: row.contents_permission,
            pull_requests: row.pull_requests_permission,
            checks: row.checks_permission,
            actions: row.actions_permission,
        },
        subscribed_events: row.subscribed_events,
        state: row.state,
        enabled: row.enabled,
        version: row.version,
        last_attempt_at: row.last_attempt_at,
        last_success_at: row.last_success_at,
        failure_category: row.failure_category,
        repositories: repositories
            .into_iter()
            .map(|row| InstallationRepositoryView {
                id: row.id,
                github_repository_id: row.github_repository_id,
                owner: row.owner,
                name: row.name,
                access_state: row.access_state,
                last_verified_at: row.last_verified_at,
            })
            .collect(),
    }
}

async fn receive_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match receive_webhook_inner(&state, &headers, &body).await {
        Ok(duplicate) => {
            let status = if duplicate {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            (
                status,
                Json(json!({ "accepted": true, "duplicate": duplicate })),
            )
                .into_response()
        }
        Err(WebhookError::Signature) => {
            (StatusCode::UNAUTHORIZED, Json(json!({ "accepted": false }))).into_response()
        }
        Err(WebhookError::Invalid) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "accepted": false }))).into_response()
        }
        Err(WebhookError::Unknown) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "accepted": false })),
        )
            .into_response(),
        Err(WebhookError::Conflict) => {
            (StatusCode::CONFLICT, Json(json!({ "accepted": false }))).into_response()
        }
        Err(WebhookError::Unavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "accepted": false })),
        )
            .into_response(),
        Err(WebhookError::Database) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "accepted": false })),
        )
            .into_response(),
    }
}

#[derive(Debug)]
enum WebhookError {
    Signature,
    Invalid,
    Unknown,
    Conflict,
    Unavailable,
    Database,
}

#[derive(Debug)]
struct ParsedWebhook {
    delivery_id: String,
    event: String,
    action: Option<String>,
    installation_id: i64,
    github_repository_id: i64,
    repository_full_name: String,
    payload: Value,
}

async fn receive_webhook_inner(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<bool, WebhookError> {
    let config = state
        .config
        .github
        .as_ref()
        .ok_or(WebhookError::Unavailable)?;
    if body.is_empty() || body.len() > MAX_WEBHOOK_BODY_BYTES {
        return Err(WebhookError::Invalid);
    }
    let signature = required_unique_header(headers, "x-hub-signature-256", 80)?;
    let mut secret = read_secret_file(&config.webhook_secret_path, 4096)
        .map_err(|_| WebhookError::Unavailable)?;
    while secret.last().is_some_and(|byte| byte.is_ascii_whitespace()) {
        secret.pop();
    }
    if secret.len() < 32 {
        return Err(WebhookError::Unavailable);
    }
    verify_signature(&secret, body, &signature)?;
    let parsed = parse_webhook(headers, body)?;
    let checksum = Sha256::digest(body).to_vec();

    let mut transaction = state
        .pool
        .begin()
        .await
        .map_err(|_| WebhookError::Database)?;
    let registration = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String)>(
        r#"
        SELECT installation.id, installation.project_id, repository.id,
               repository.owner, repository.name
        FROM github_installations installation
        JOIN github_installation_repositories link
          ON link.github_installation_id = installation.id
         AND link.project_id = installation.project_id
         AND link.access_state = 'selected'
        JOIN repositories repository
          ON repository.id = link.repository_id
         AND repository.project_id = link.project_id
        WHERE installation.installation_id = $1
          AND installation.app_id = $2
          AND installation.enabled
          AND installation.state = 'available'
          AND repository.github_repository_id = $3
          AND repository.enabled
          AND repository.archived_at IS NULL
        FOR SHARE OF installation, link, repository
        "#,
    )
    .bind(parsed.installation_id)
    .bind(i64::try_from(config.app_id).map_err(|_| WebhookError::Unavailable)?)
    .bind(parsed.github_repository_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| WebhookError::Database)?
    .ok_or(WebhookError::Unknown)?;
    let expected_full_name = format!("{}/{}", registration.3, registration.4);
    if parsed.repository_full_name != expected_full_name {
        return Err(WebhookError::Unknown);
    }
    let delivery_row_id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO github_webhook_deliveries (
            id, project_id, github_installation_id, repository_id,
            delivery_id, event, action, payload_sha256
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (github_installation_id, delivery_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(delivery_row_id)
    .bind(registration.1)
    .bind(registration.0)
    .bind(registration.2)
    .bind(&parsed.delivery_id)
    .bind(&parsed.event)
    .bind(&parsed.action)
    .bind(&checksum)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| WebhookError::Database)?;
    if inserted.is_none() {
        let existing = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT payload_sha256 FROM github_webhook_deliveries WHERE github_installation_id = $1 AND delivery_id = $2 FOR SHARE",
        )
        .bind(registration.0)
        .bind(&parsed.delivery_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| WebhookError::Database)?;
        if existing.ct_eq(&checksum).unwrap_u8() != 1 {
            return Err(WebhookError::Conflict);
        }
        transaction
            .commit()
            .await
            .map_err(|_| WebhookError::Database)?;
        return Ok(true);
    }
    let job_kind = match parsed.event.as_str() {
        "installation" => "installation_sync",
        "installation_repositories" => "repository_access_sync",
        "push" => "push_reconcile",
        "pull_request" => "pull_request_sync",
        "pull_request_review" | "check_suite" | "check_run" => "check_review_sync",
        _ => return Err(WebhookError::Unknown),
    };
    sqlx::query(
        r#"
        INSERT INTO github_jobs (
            id, project_id, github_installation_id, repository_id,
            webhook_delivery_id, kind, payload
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(registration.1)
    .bind(registration.0)
    .bind(registration.2)
    .bind(delivery_row_id)
    .bind(job_kind)
    .bind(parsed.payload)
    .execute(&mut *transaction)
    .await
    .map_err(|_| WebhookError::Database)?;
    transaction
        .commit()
        .await
        .map_err(|_| WebhookError::Database)?;
    Ok(false)
}

fn required_unique_header(
    headers: &HeaderMap,
    name: &'static str,
    max_length: usize,
) -> Result<String, WebhookError> {
    let values = headers.get_all(name).iter().collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(WebhookError::Invalid);
    }
    let value = values[0].to_str().map_err(|_| WebhookError::Invalid)?;
    if value.is_empty() || value.len() > max_length || value.chars().any(char::is_control) {
        return Err(WebhookError::Invalid);
    }
    Ok(value.to_owned())
}

fn verify_signature(secret: &[u8], body: &[u8], signature: &str) -> Result<(), WebhookError> {
    let supplied = signature
        .strip_prefix("sha256=")
        .ok_or(WebhookError::Signature)?;
    if supplied.len() != 64 {
        return Err(WebhookError::Signature);
    }
    let supplied = hex::decode(supplied).map_err(|_| WebhookError::Signature)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| WebhookError::Signature)?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if supplied.ct_eq(expected.as_slice()).unwrap_u8() != 1 {
        return Err(WebhookError::Signature);
    }
    Ok(())
}

fn parse_webhook(headers: &HeaderMap, body: &[u8]) -> Result<ParsedWebhook, WebhookError> {
    let delivery_id = required_unique_header(headers, "x-github-delivery", 128)?;
    let event = required_unique_header(headers, "x-github-event", 64)?;
    let value: Value = serde_json::from_slice(body).map_err(|_| WebhookError::Invalid)?;
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if !allowed_webhook_action(&event, action.as_deref()) {
        return Err(WebhookError::Unknown);
    }
    let installation_id = value
        .pointer("/installation/id")
        .and_then(Value::as_u64)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(WebhookError::Invalid)?;
    let github_repository_id = value
        .pointer("/repository/id")
        .and_then(Value::as_u64)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(WebhookError::Invalid)?;
    let repository_full_name = value
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 201)
        .ok_or(WebhookError::Invalid)?
        .to_owned();
    let payload = bounded_job_payload(&event, action.as_deref(), &value)?;
    Ok(ParsedWebhook {
        delivery_id,
        event,
        action,
        installation_id,
        github_repository_id,
        repository_full_name,
        payload,
    })
}

fn allowed_webhook_action(event: &str, action: Option<&str>) -> bool {
    match event {
        "push" => action.is_none(),
        "installation" => matches!(
            action,
            Some("created" | "deleted" | "suspend" | "unsuspend" | "new_permissions_accepted")
        ),
        "installation_repositories" => matches!(action, Some("added" | "removed")),
        "pull_request" => matches!(
            action,
            Some(
                "opened"
                    | "edited"
                    | "closed"
                    | "reopened"
                    | "synchronize"
                    | "converted_to_draft"
                    | "ready_for_review"
            )
        ),
        "pull_request_review" => matches!(action, Some("submitted" | "edited" | "dismissed")),
        "check_suite" => matches!(action, Some("requested" | "rerequested" | "completed")),
        "check_run" => matches!(
            action,
            Some("created" | "rerequested" | "completed" | "requested_action")
        ),
        _ => false,
    }
}

fn bounded_job_payload(
    event: &str,
    action: Option<&str>,
    value: &Value,
) -> Result<Value, WebhookError> {
    let mut payload = serde_json::Map::new();
    if let Some(action) = action {
        payload.insert("action".to_owned(), Value::String(action.to_owned()));
    }
    let pointers = match event {
        "push" => [("after", "/after"), ("before", "/before")].as_slice(),
        "pull_request" | "pull_request_review" => [
            ("number", "/pull_request/number"),
            ("node_id", "/pull_request/node_id"),
            ("head_sha", "/pull_request/head/sha"),
            ("head_ref", "/pull_request/head/ref"),
            ("base_ref", "/pull_request/base/ref"),
        ]
        .as_slice(),
        "check_suite" => [("check_suite_id", "/check_suite/id")].as_slice(),
        "check_run" => [
            ("check_run_id", "/check_run/id"),
            ("head_sha", "/check_run/head_sha"),
        ]
        .as_slice(),
        "installation" | "installation_repositories" => [].as_slice(),
        _ => return Err(WebhookError::Unknown),
    };
    for (name, pointer) in pointers {
        if let Some(item) = value.pointer(pointer) {
            let bounded = match item {
                Value::String(value)
                    if value.len() <= 512 && !value.chars().any(char::is_control) =>
                {
                    Value::String(value.clone())
                }
                Value::Number(value) => Value::Number(value.clone()),
                _ => continue,
            };
            payload.insert((*name).to_owned(), bounded);
        }
    }
    Ok(Value::Object(payload))
}

fn map_github_error(error: GitHubError) -> AppError {
    match error {
        GitHubError::Scope => AppError::Conflict("github_installation", "incompatible_scope"),
        GitHubError::Credential | GitHubError::Jwt | GitHubError::Http | GitHubError::Response => {
            AppError::Internal("GitHub integration unavailable")
        }
    }
}

async fn require_project_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<MembershipRole, AppError> {
    let access = sqlx::query_as::<_, ProjectAccess>(
        r#"
        SELECT membership.role
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        WHERE project.id = $1 AND project.archived_at IS NULL
        FOR SHARE OF project, membership
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    let role = membership_role(&access.role)?;
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(project_id, role, true)),
        None,
    );
    if authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(project_id)),
    )
    .is_allowed()
    {
        Ok(role)
    } else {
        Err(AppError::Forbidden)
    }
}

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

fn role_allows_manage(role: MembershipRole) -> bool {
    matches!(role, MembershipRole::Owner | MembershipRole::Admin)
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    use super::*;

    #[test]
    fn installation_token_debug_is_redacted() {
        let token = InstallationToken {
            secret: SecretString::from("opaque-long-token"),
            _expires_at: "future".to_owned(),
        };
        assert!(!format!("{token:?}").contains(token.expose()));
    }

    #[test]
    fn webhook_signature_and_action_allowlist_are_strict() {
        let secret = b"a-high-entropy-development-webhook-secret";
        let body = br#"{"installation":{"id":1},"repository":{"id":2,"full_name":"owner/repository"},"after":"0123456789012345678901234567890123456789"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key");
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature(secret, body, &signature).is_ok());
        assert!(verify_signature(secret, b"changed", &signature).is_err());
        assert!(allowed_webhook_action("push", None));
        assert!(!allowed_webhook_action("push", Some("created")));
        assert!(allowed_webhook_action("pull_request", Some("opened")));
        assert!(!allowed_webhook_action("issues", Some("opened")));
    }

    #[test]
    fn secret_reader_is_bounded() {
        let directory = tempfile::tempdir().expect("secret directory");
        let path = directory.path().join("secret");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .expect("secret file");
        file.write_all(b"bounded-secret").expect("write secret");
        assert_eq!(read_secret_file(&path, 64).expect("read secret").len(), 14);
        assert!(read_secret_file(&path, 4).is_err());
    }

    #[test]
    fn installation_scope_rejects_missing_or_elevated_access() {
        let config = GitHubConfig {
            app_id: 4_442_593,
            private_key_path: "/outside/private-key".into(),
            webhook_secret_path: "/outside/webhook-secret".into(),
            api_base_url: "https://api.github.com".to_owned(),
            api_version: GITHUB_API_VERSION.to_owned(),
        };
        let mut installation = GitHubInstallation {
            id: 150_232_852,
            app_id: config.app_id,
            account: GitHubAccount {
                id: 777,
                login: "Mischa-dev".to_owned(),
            },
            target_type: "Organization".to_owned(),
            repository_selection: "selected".to_owned(),
            permissions: BTreeMap::from([
                ("metadata".to_owned(), "read".to_owned()),
                ("contents".to_owned(), "write".to_owned()),
                ("pull_requests".to_owned(), "write".to_owned()),
                ("checks".to_owned(), "read".to_owned()),
            ]),
            events: REQUIRED_EVENTS
                .iter()
                .map(|event| (*event).to_owned())
                .collect(),
            suspended_at: None,
        };
        assert!(validate_installation(&config, &installation).is_ok());
        installation
            .permissions
            .insert("issues".to_owned(), "write".to_owned());
        assert!(validate_installation(&config, &installation).is_err());
        installation.permissions.remove("issues");
        installation.events.push("push".to_owned());
        assert!(validate_installation(&config, &installation).is_err());
        installation.events.pop();
        installation.events.push("repository".to_owned());
        assert!(validate_installation(&config, &installation).is_ok());
        installation.events.pop();
        installation.repository_selection = "all".to_owned();
        assert!(validate_installation(&config, &installation).is_err());
    }

    #[test]
    fn duplicate_or_ambiguous_webhook_headers_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.append("x-github-delivery", HeaderValue::from_static("one"));
        headers.append("x-github-delivery", HeaderValue::from_static("two"));
        assert!(required_unique_header(&headers, "x-github-delivery", 128).is_err());
        headers.remove("x-github-delivery");
        headers.insert("x-github-delivery", HeaderValue::from_static("one"));
        assert_eq!(
            required_unique_header(&headers, "x-github-delivery", 128)
                .expect("one bounded delivery header"),
            "one"
        );
    }
}
