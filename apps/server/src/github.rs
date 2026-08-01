use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
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
use tokio::sync::watch;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    config::{GITHUB_API_VERSION, GitHubConfig},
    error::{ApiJson, ApiPath, AppError},
    git_runtime::{
        PublishPush, expected_repository_mirror_path, inspect_publishable_worktree,
        managed_worktree_path, push_publishable_commit,
    },
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    outbox,
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
};

const MAX_WEBHOOK_BODY_BYTES: usize = 1024 * 1024;
const MAX_GITHUB_RESPONSE_BYTES: usize = 1024 * 1024;
const GITHUB_JOB_LEASE: Duration = Duration::from_secs(30);
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
        .route(
            "/api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
            post(publish_agent_run),
        )
}

pub fn webhook_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/integrations/github/webhook", post(receive_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
        .with_state(state)
}

pub fn kick_jobs(state: AppState) {
    tokio::spawn(async move {
        if let Err(error) = drain_jobs(&state).await {
            tracing::error!(
                error_category = error.log_category(),
                "GitHub job worker kick failed"
            );
        }
    });
}

pub async fn run_jobs(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AppError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => drain_jobs(&state).await?,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
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

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct PublishJobView {
    job_id: Uuid,
    agent_run_id: Uuid,
    repository_workspace_id: Uuid,
    intended_commit: String,
    state: String,
    failure_category: Option<String>,
}

#[derive(Debug, FromRow)]
struct PublishCandidate {
    agent_run_id: Uuid,
    conversation_id: Uuid,
    repository_id: Uuid,
    repository_workspace_id: Uuid,
    run_state: String,
    mirror_path: String,
    worktree_path: String,
    branch_name: String,
    base_commit: String,
}

#[derive(Debug, FromRow)]
struct ClaimedPublishJob {
    job_id: Uuid,
    project_id: Uuid,
    repository_id: Uuid,
    repository_workspace_id: Uuid,
    agent_run_id: Uuid,
    requesting_actor_id: Uuid,
    intended_commit: String,
    attempts: i32,
    max_attempts: i32,
    claim_id: Uuid,
    conversation_id: Uuid,
    installation_id: i64,
    github_repository_id: i64,
    owner: String,
    repository_name: String,
    clone_url: String,
    mirror_path: String,
    worktree_path: String,
    branch_name: String,
    base_commit: String,
    installation_available: bool,
    repository_available: bool,
    workspace_available: bool,
    run_terminal: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstallationTokenScope {
    VerifyInstallation,
    ContentsWrite,
}

impl InstallationTokenScope {
    fn permissions(self) -> Value {
        match self {
            Self::VerifyInstallation => json!({
                "metadata": "read",
                "contents": "write",
                "pull_requests": "write",
                "checks": "read"
            }),
            Self::ContentsWrite => json!({
                "metadata": "read",
                "contents": "write"
            }),
        }
    }
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

#[derive(Debug, Deserialize)]
struct GitReference {
    object: GitReferenceObject,
}

#[derive(Debug, Deserialize)]
struct GitReferenceObject {
    sha: String,
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
        scope: InstallationTokenScope,
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
                "permissions": scope.permissions()
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

    async fn branch_commit(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        branch: &str,
    ) -> Result<Option<String>, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend(["repos", owner, repository, "git", "ref"])
            .push(&format!("heads/{branch}"));
        let response = self
            .http
            .get(url)
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let reference: GitReference = decode_response(response).await?;
        if reference.object.sha.len() != 40
            || !reference
                .object
                .sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GitHubError::Response);
        }
        Ok(Some(reference.object.sha))
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
        .installation_token(
            request.installation_id,
            &github_repository_ids,
            InstallationTokenScope::VerifyInstallation,
        )
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

async fn publish_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, run_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if state.config.github.is_none() {
        return Err(AppError::Conflict("github_integration", "not_configured"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("agent_runs", run_id, "publish")?;
    let request_hash = RequestHash::for_value(&json!({ "agentRunId": run_id }))?;
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryPublish,
    )
    .await?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let view: PublishJobView = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(view)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let candidate = sqlx::query_as::<_, PublishCandidate>(
        r#"
        SELECT run.id AS agent_run_id, run.conversation_id,
               run.repository_id, workspace.id AS repository_workspace_id,
               run.state AS run_state, repository.mirror_path,
               worktree.path AS worktree_path, workspace.branch_name,
               workspace.base_commit
        FROM agent_runs run
        JOIN repositories repository
          ON repository.project_id = run.project_id
         AND repository.id = run.repository_id
        JOIN repository_workspaces workspace
          ON workspace.project_id = run.project_id
         AND workspace.id = run.repository_workspace_id
         AND workspace.repository_id = run.repository_id
         AND workspace.conversation_id = run.conversation_id
        JOIN git_worktrees worktree
          ON worktree.repository_workspace_id = workspace.id
         AND worktree.state IN ('ready', 'dirty')
        WHERE run.project_id = $1
          AND run.id = $2
          AND repository.enabled
          AND repository.trusted
          AND repository.archived_at IS NULL
          AND workspace.state IN ('active', 'retained')
        FOR SHARE OF run, repository, workspace, worktree
        "#,
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    if !matches!(
        candidate.run_state.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        return Err(AppError::Conflict("agent_run", "not_terminal"));
    }
    let workspace_active = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs active
            WHERE active.repository_workspace_id = $1
              AND active.state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        )
        "#,
    )
    .bind(candidate.repository_workspace_id)
    .fetch_one(&mut *transaction)
    .await?;
    if workspace_active {
        return Err(AppError::Conflict("repository_workspace", "active_run"));
    }
    let github_installation_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT installation.id
        FROM github_installations installation
        JOIN github_installation_repositories access
          ON access.project_id = installation.project_id
         AND access.github_installation_id = installation.id
        JOIN repositories repository
          ON repository.project_id = access.project_id
         AND repository.id = access.repository_id
         AND repository.github_repository_id = access.github_repository_id
        WHERE installation.project_id = $1
          AND access.repository_id = $2
          AND installation.enabled
          AND installation.state = 'available'
          AND access.access_state = 'selected'
        FOR SHARE OF installation, access, repository
        "#,
    )
    .bind(project_id)
    .bind(candidate.repository_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Conflict(
        "github_installation",
        "repository_unavailable",
    ))?;
    let expected_mirror =
        expected_repository_mirror_path(&state.config.data_root, candidate.repository_id)?;
    let expected_worktree = managed_worktree_path(
        &state.config.data_root,
        candidate.repository_id,
        candidate.repository_workspace_id,
    )?;
    if Path::new(&candidate.mirror_path) != expected_mirror
        || Path::new(&candidate.worktree_path) != expected_worktree
    {
        return Err(AppError::Conflict(
            "repository_workspace",
            "stored runtime path mismatch",
        ));
    }
    let branch_name = candidate.branch_name.clone();
    let base_commit = candidate.base_commit.clone();
    let inspection = tokio::task::spawn_blocking(move || {
        inspect_publishable_worktree(
            &expected_mirror,
            &expected_worktree,
            &branch_name,
            &base_commit,
        )
    })
    .await
    .map_err(|_| AppError::Internal("join Git publication inspection"))??;
    sqlx::query(
        "UPDATE git_worktrees SET state = 'ready', dirty = false, last_inspected_at = now(), updated_at = now() WHERE repository_workspace_id = $1",
    )
    .bind(candidate.repository_workspace_id)
    .execute(&mut *transaction)
    .await?;

    if let Some(existing) = sqlx::query_as::<_, PublishJobView>(
        r#"
        SELECT id AS job_id, agent_run_id, repository_workspace_id,
               intended_commit, state, failure_category
        FROM github_jobs
        WHERE project_id = $1
          AND repository_workspace_id = $2
          AND intended_commit = $3
          AND kind = 'workspace_publish'
          AND state IN ('queued', 'running', 'failed', 'completed')
        ORDER BY created_at DESC
        LIMIT 1
        FOR SHARE
        "#,
    )
    .bind(project_id)
    .bind(candidate.repository_workspace_id)
    .bind(&inspection.head_commit)
    .fetch_optional(&mut *transaction)
    .await?
    {
        let status = if existing.state == "completed" {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        complete(&mut transaction, &reservation, status, &existing).await?;
        transaction.commit().await?;
        return Ok((status, Json(existing)).into_response());
    }

    let job_id = Uuid::now_v7();
    let view = PublishJobView {
        job_id,
        agent_run_id: candidate.agent_run_id,
        repository_workspace_id: candidate.repository_workspace_id,
        intended_commit: inspection.head_commit,
        state: "queued".to_owned(),
        failure_category: None,
    };
    sqlx::query(
        r#"
        INSERT INTO github_jobs (
            id, project_id, github_installation_id, repository_id,
            kind, payload, repository_workspace_id, agent_run_id,
            requesting_actor_id, intended_commit
        ) VALUES (
            $1, $2, $3, $4, 'workspace_publish', '{}'::jsonb,
            $5, $6, $7, $8
        )
        "#,
    )
    .bind(job_id)
    .bind(project_id)
    .bind(github_installation_id)
    .bind(candidate.repository_id)
    .bind(candidate.repository_workspace_id)
    .bind(candidate.agent_run_id)
    .bind(principal.actor_id)
    .bind(&view.intended_commit)
    .execute(&mut *transaction)
    .await?;
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
            'github.workspace_publish_queued', 'github_job', $5,
            'succeeded', $6,
            jsonb_build_object('agent_run_id', $7::uuid, 'repository_workspace_id', $8::uuid)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(job_id)
    .bind(request_id)
    .bind(candidate.agent_run_id)
    .bind(candidate.repository_workspace_id)
    .execute(&mut *transaction)
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        project_id,
        candidate.conversation_id,
        candidate.agent_run_id,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::ACCEPTED, &view).await?;
    transaction.commit().await?;
    kick_jobs(state);
    Ok((StatusCode::ACCEPTED, Json(view)).into_response())
}

async fn drain_jobs(state: &AppState) -> Result<(), AppError> {
    while let Some(job) = claim_publish_job(state).await? {
        let job_id = job.job_id;
        if let Err(error) = process_publish_job(state, &job).await {
            tracing::error!(
                %job_id,
                error_category = error.log_category(),
                "GitHub publication job processing failed"
            );
            fail_publish_job(state, &job, "job_processing_failed", false).await?;
        }
    }
    Ok(())
}

async fn claim_publish_job(state: &AppState) -> Result<Option<ClaimedPublishJob>, AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = CASE
                WHEN attempts >= max_attempts THEN 'exhausted'
                ELSE 'failed'
            END,
            lease_owner = NULL, lease_expires_at = NULL,
            failure_category = 'stale_lease', next_attempt_at = now(),
            updated_at = now()
        WHERE state = 'running' AND lease_expires_at <= now()
        "#,
    )
    .execute(&mut *transaction)
    .await?;
    let claim_id = Uuid::now_v7();
    let job = sqlx::query_as::<_, ClaimedPublishJob>(
        r#"
        SELECT job.id AS job_id, job.project_id, job.repository_id,
               job.repository_workspace_id, job.agent_run_id,
               job.requesting_actor_id, job.intended_commit,
               job.attempts + 1 AS attempts, job.max_attempts,
               $1::uuid AS claim_id, run.conversation_id,
               installation.installation_id, repository.github_repository_id,
               repository.owner, repository.name AS repository_name,
               repository.clone_url, repository.mirror_path,
               worktree.path AS worktree_path, workspace.branch_name,
               workspace.base_commit,
               COALESCE(installation.enabled
                   AND installation.state = 'available'
                   AND access.access_state = 'selected'
                   AND access.github_repository_id = repository.github_repository_id,
                   false) AS installation_available,
               repository.enabled
                   AND repository.trusted
                   AND repository.archived_at IS NULL
                   AS repository_available,
               workspace.state IN ('active', 'retained')
                   AND worktree.state IN ('ready', 'dirty')
                   AS workspace_available,
               run.state IN ('completed', 'failed', 'canceled', 'interrupted')
                   AS run_terminal
        FROM github_jobs job
        JOIN github_installations installation
          ON installation.project_id = job.project_id
         AND installation.id = job.github_installation_id
        LEFT JOIN github_installation_repositories access
          ON access.project_id = job.project_id
         AND access.github_installation_id = installation.id
         AND access.repository_id = job.repository_id
        JOIN repositories repository
          ON repository.project_id = job.project_id
         AND repository.id = job.repository_id
        JOIN repository_workspaces workspace
          ON workspace.project_id = job.project_id
         AND workspace.id = job.repository_workspace_id
         AND workspace.repository_id = job.repository_id
        JOIN git_worktrees worktree
          ON worktree.repository_workspace_id = workspace.id
        JOIN agent_runs run
          ON run.project_id = job.project_id
         AND run.id = job.agent_run_id
         AND run.repository_workspace_id = workspace.id
        WHERE job.kind = 'workspace_publish'
          AND job.state IN ('queued', 'failed')
          AND job.next_attempt_at <= now()
          AND job.attempts < job.max_attempts
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs active
              WHERE active.repository_workspace_id = workspace.id
                AND active.state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
          )
        ORDER BY job.next_attempt_at, job.created_at, job.id
        FOR UPDATE OF job SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(claim_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(job) = job else {
        transaction.commit().await?;
        return Ok(None);
    };
    let lease_seconds = i64::try_from(GITHUB_JOB_LEASE.as_secs()).unwrap_or(30);
    let updated = sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = 'running', attempts = attempts + 1,
            lease_owner = $2,
            lease_expires_at = now() + make_interval(secs => $3),
            failure_category = NULL, updated_at = now()
        WHERE id = $1 AND state IN ('queued', 'failed')
        "#,
    )
    .bind(job.job_id)
    .bind(claim_id)
    .bind(lease_seconds as f64)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("github_job", "claim_lost"));
    }
    transaction.commit().await?;
    Ok(Some(job))
}

async fn process_publish_job(state: &AppState, job: &ClaimedPublishJob) -> Result<(), AppError> {
    if !job.installation_available || !job.repository_available {
        return fail_publish_job(state, job, "publication_scope_unavailable", true).await;
    }
    if !job.workspace_available || !job.run_terminal {
        return fail_publish_job(state, job, "workspace_unavailable", true).await;
    }
    let Some(config) = state.config.github.as_ref() else {
        return fail_publish_job(state, job, "github_not_configured", true).await;
    };
    let installation_id = match u64::try_from(job.installation_id) {
        Ok(value) => value,
        Err(_) => return fail_publish_job(state, job, "installation_identity_invalid", true).await,
    };
    let github_repository_id = match u64::try_from(job.github_repository_id) {
        Ok(value) => value,
        Err(_) => return fail_publish_job(state, job, "repository_identity_invalid", true).await,
    };
    let expected_clone_url = format!(
        "https://github.com/{}/{}.git",
        job.owner, job.repository_name
    );
    if job.clone_url != expected_clone_url {
        return fail_publish_job(state, job, "repository_identity_invalid", true).await;
    }
    let expected_mirror =
        expected_repository_mirror_path(&state.config.data_root, job.repository_id)?;
    let expected_worktree = managed_worktree_path(
        &state.config.data_root,
        job.repository_id,
        job.repository_workspace_id,
    )?;
    if Path::new(&job.mirror_path) != expected_mirror
        || Path::new(&job.worktree_path) != expected_worktree
    {
        return fail_publish_job(state, job, "runtime_path_mismatch", true).await;
    }
    let mirror = expected_mirror.clone();
    let worktree = expected_worktree.clone();
    let branch = job.branch_name.clone();
    let base = job.base_commit.clone();
    let inspection = tokio::task::spawn_blocking(move || {
        inspect_publishable_worktree(&mirror, &worktree, &branch, &base)
    })
    .await
    .map_err(|_| AppError::Internal("join Git publication reinspection"))?;
    match inspection {
        Ok(inspection) if inspection.head_commit == job.intended_commit => {}
        Ok(_) | Err(_) => {
            return fail_publish_job(state, job, "workspace_changed", true).await;
        }
    }

    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = match client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::ContentsWrite,
        )
        .await
    {
        Ok(token) => token,
        Err(GitHubError::Scope) => {
            return fail_publish_job(state, job, "github_scope_incompatible", true).await;
        }
        Err(_) => {
            return fail_publish_job(state, job, "github_token_unavailable", false).await;
        }
    };
    match client
        .branch_commit(&token, &job.owner, &job.repository_name, &job.branch_name)
        .await
    {
        Ok(Some(commit)) if commit == job.intended_commit => {
            drop(token);
            return complete_publish_job(state, job).await;
        }
        Ok(Some(_)) => {
            drop(token);
            return fail_publish_job(state, job, "remote_branch_diverged", true).await;
        }
        Ok(None) => {}
        Err(_) => {
            drop(token);
            return fail_publish_job(state, job, "remote_ref_unavailable", false).await;
        }
    }

    let data_root = state.config.data_root.clone();
    let mirror = expected_mirror;
    let worktree = expected_worktree;
    let branch = job.branch_name.clone();
    let base = job.base_commit.clone();
    let intended = job.intended_commit.clone();
    let remote_url = job.clone_url.clone();
    let push_token = token.secret.clone();
    let push = tokio::task::spawn_blocking(move || {
        push_publishable_commit(PublishPush {
            data_root: &data_root,
            mirror: &mirror,
            worktree: &worktree,
            expected_branch: &branch,
            base_commit: &base,
            intended_commit: &intended,
            remote_url: &remote_url,
            token: &push_token,
        })
    })
    .await
    .map_err(|_| AppError::Internal("join credentialed Git push"))?;
    let remote = client
        .branch_commit(&token, &job.owner, &job.repository_name, &job.branch_name)
        .await;
    drop(token);
    match remote {
        Ok(Some(commit)) if commit == job.intended_commit => complete_publish_job(state, job).await,
        Ok(Some(_)) => fail_publish_job(state, job, "remote_branch_diverged", true).await,
        Ok(None) if push.is_err() => fail_publish_job(state, job, "git_push_failed", false).await,
        Ok(None) => fail_publish_job(state, job, "remote_ref_unconfirmed", false).await,
        Err(_) => fail_publish_job(state, job, "remote_ref_unavailable", false).await,
    }
}

async fn complete_publish_job(state: &AppState, job: &ClaimedPublishJob) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = 'completed', lease_owner = NULL, lease_expires_at = NULL,
            failure_category = NULL, completed_at = now(), updated_at = now()
        WHERE id = $1 AND state = 'running' AND lease_owner = $2
        "#,
    )
    .bind(job.job_id)
    .bind(job.claim_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("github_job", "lease_lost"));
    }
    sqlx::query(
        r#"
        UPDATE repository_workspaces
        SET published_commit = $3, published_at = now(),
            published_by_actor_id = $4, publish_last_attempt_at = now(),
            publish_failure_category = NULL,
            publish_version = publish_version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(job.project_id)
    .bind(job.repository_workspace_id)
    .bind(&job.intended_commit)
    .bind(job.requesting_actor_id)
    .execute(&mut *transaction)
    .await?;
    insert_publish_job_audit(&mut transaction, job, "succeeded", None).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        job.project_id,
        job.conversation_id,
        job.agent_run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn fail_publish_job(
    state: &AppState,
    job: &ClaimedPublishJob,
    category: &'static str,
    terminal: bool,
) -> Result<(), AppError> {
    let exhausted = terminal || job.attempts >= job.max_attempts;
    let next_state = if exhausted { "exhausted" } else { "failed" };
    let backoff_seconds = i64::from(job.attempts.clamp(1, 6)).pow(2).min(60);
    let mut transaction = state.pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = $3, lease_owner = NULL, lease_expires_at = NULL,
            failure_category = $4,
            next_attempt_at = now() + make_interval(secs => $5),
            updated_at = now()
        WHERE id = $1 AND state = 'running' AND lease_owner = $2
        "#,
    )
    .bind(job.job_id)
    .bind(job.claim_id)
    .bind(next_state)
    .bind(category)
    .bind(backoff_seconds as f64)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("github_job", "lease_lost"));
    }
    sqlx::query(
        r#"
        UPDATE repository_workspaces
        SET publish_last_attempt_at = now(), publish_failure_category = $3,
            publish_version = publish_version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(job.project_id)
    .bind(job.repository_workspace_id)
    .bind(category)
    .execute(&mut *transaction)
    .await?;
    insert_publish_job_audit(&mut transaction, job, "failed", Some(category)).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        job.project_id,
        job.conversation_id,
        job.agent_run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn insert_publish_job_audit(
    transaction: &mut Transaction<'_, Postgres>,
    job: &ClaimedPublishJob,
    outcome: &'static str,
    failure_category: Option<&'static str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class,
            action, target_kind, target_id, outcome, metadata
        ) VALUES (
            $1, $2, $3, 'github_job', 'github.workspace_publish',
            'github_job', $4, $5,
            jsonb_build_object(
                'agent_run_id', $6::uuid,
                'repository_workspace_id', $7::uuid,
                'failure_category', $8::text
            )
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.requesting_actor_id)
    .bind(job.project_id)
    .bind(job.job_id)
    .bind(outcome)
    .bind(job.agent_run_id)
    .bind(job.repository_workspace_id)
    .bind(failure_category)
    .execute(&mut **transaction)
    .await?;
    Ok(())
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
