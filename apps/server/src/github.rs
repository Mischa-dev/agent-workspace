use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
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
        Action, ActorKind, Membership, Permission, ProjectScope, Resource, RoleGrant, Subject,
        authorize,
    },
    work_item_commands::{self, AuditAttribution, CommandActor, record_audit_for},
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

mod connect;
pub(crate) use connect::add_picked_repository_as;

pub fn router() -> Router<AppState> {
    connect::routes(Router::new())
        .route(
            "/api/v1/projects/{project_id}/integrations/github",
            get(github_status),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/sync",
            post(sync_installation),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/automation",
            get(github_automation_settings).put(update_github_automation_settings),
        )
        .route(
            "/api/v1/projects/{project_id}/agent-runs/{run_id}/publish",
            post(publish_agent_run),
        )
        .route(
            "/api/v1/projects/{project_id}/agent-runs/{run_id}/pull-request",
            post(create_pull_request),
        )
        .route(
            "/api/v1/projects/{project_id}/pull-requests/{pull_request_id}/refresh",
            post(refresh_pull_request),
        )
        .route(
            "/api/v1/projects/{project_id}/pull-requests/{pull_request_id}/merge",
            post(merge_pull_request),
        )
}

pub fn webhook_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/integrations/github/webhook", post(receive_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            limit_webhook_attempts,
        ))
        .with_state(state)
}

async fn limit_webhook_attempts(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Err(error) = state.rate_limits.check_webhook_attempt() {
        return error.into_response();
    }
    next.run(request).await
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
            // A database blip must not stop the server: log and try again
            // on the next tick.
            _ = interval.tick() => {
                if let Err(error) = drain_jobs(&state).await {
                    tracing::error!(
                        error_category = error.log_category(),
                        "GitHub job worker pass failed"
                    );
                }
            }
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
    role_permissions: Option<Vec<String>>,
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
pub(crate) struct GitHubAutomationSettingsView {
    pub project_id: Uuid,
    pub pull_request_open_enabled: bool,
    pub pull_request_merge_enabled: bool,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateGitHubAutomationSettingsRequest {
    expected_version: i64,
    pull_request_open_enabled: bool,
    pull_request_merge_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishJobView {
    pub job_id: Uuid,
    pub agent_run_id: Uuid,
    pub repository_workspace_id: Uuid,
    pub intended_commit: String,
    pub state: String,
    pub failure_category: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreatePullRequestRequest {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    work_item_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestRequestView {
    pub request_id: Uuid,
    pub agent_run_id: Uuid,
    pub repository_workspace_id: Uuid,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub state: String,
    pub failure_category: Option<String>,
    pub github_pull_request_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestRefreshView {
    job_id: Uuid,
    pull_request_id: Uuid,
    state: String,
    failure_category: Option<String>,
}

#[derive(Debug, FromRow)]
struct PullRequestCandidate {
    conversation_id: Uuid,
    repository_id: Uuid,
    repository_workspace_id: Uuid,
    branch_name: String,
    published_commit: String,
    default_branch: String,
    github_installation_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ClaimedPullRequestJob {
    job_id: Uuid,
    request_id: Uuid,
    project_id: Uuid,
    repository_id: Uuid,
    repository_workspace_id: Uuid,
    agent_run_id: Uuid,
    requesting_actor_id: Uuid,
    claim_id: Uuid,
    attempts: i32,
    max_attempts: i32,
    conversation_id: Uuid,
    installation_id: i64,
    github_repository_id: i64,
    owner: String,
    repository_name: String,
    head_ref: String,
    head_sha: String,
    base_ref: String,
    title: String,
    body: String,
    work_item_ids: sqlx::types::Json<Vec<Uuid>>,
    mutation_attempted: bool,
    scope_available: bool,
}

#[derive(Debug, FromRow)]
struct ClaimedPullRequestSyncJob {
    job_id: Uuid,
    project_id: Uuid,
    pull_request_id: Uuid,
    claim_id: Uuid,
    attempts: i32,
    max_attempts: i32,
    webhook_delivery_id: Option<Uuid>,
    integration_actor_id: Uuid,
    installation_id: i64,
    github_repository_id: i64,
    owner: String,
    repository_name: String,
    pull_request_number: i64,
    node_id: String,
    html_url: String,
    head_ref: String,
    base_ref: String,
    external_created_at: DateTime<Utc>,
    known_merge_commit_sha: Option<String>,
    scope_available: bool,
}

/// A webhook's pull request the app isn't tracking yet, whose branch, title
/// or body names one of the project's open tasks (AWD-59).
#[derive(Debug, FromRow)]
struct ClaimedPullRequestAdoptionJob {
    job_id: Uuid,
    project_id: Uuid,
    claim_id: Uuid,
    attempts: i32,
    max_attempts: i32,
    webhook_delivery_id: Option<Uuid>,
    github_installation_row_id: Uuid,
    integration_actor_id: Uuid,
    installation_id: i64,
    repository_id: Uuid,
    github_repository_id: i64,
    owner: String,
    repository_name: String,
    pull_request_number: i64,
    node_id: String,
    known_merge_commit_sha: Option<String>,
    scope_available: bool,
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
    ContentsRead,
    ContentsWrite,
    PullRequestsWrite,
    /// Merging a pull request writes its base branch.
    PullRequestsMerge,
    ChecksRead,
    IssuesRead,
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
            Self::ContentsRead => json!({
                "metadata": "read",
                "contents": "read"
            }),
            Self::ContentsWrite => json!({
                "metadata": "read",
                "contents": "write"
            }),
            Self::PullRequestsWrite => json!({
                "metadata": "read",
                "contents": "read",
                "pull_requests": "write"
            }),
            Self::PullRequestsMerge => json!({
                "metadata": "read",
                "contents": "write",
                "pull_requests": "write"
            }),
            Self::IssuesRead => json!({
                "metadata": "read",
                "issues": "read"
            }),
            Self::ChecksRead => json!({
                "metadata": "read",
                "contents": "read",
                "pull_requests": "read",
                "checks": "read"
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

#[derive(Debug, FromRow)]
struct RepositoryGitScope {
    installation_id: i64,
    github_repository_id: i64,
}

pub(crate) async fn repository_git_token(
    state: &AppState,
    project_id: Uuid,
    repository_id: Uuid,
) -> Result<SecretString, AppError> {
    repository_git_token_with(&state.pool, &state.config, project_id, repository_id).await
}

/// [`repository_git_token`] for code that holds only the pool and config
/// (the harness host).
pub(crate) async fn repository_git_token_with(
    pool: &sqlx::PgPool,
    app_config: &crate::config::AppConfig,
    project_id: Uuid,
    repository_id: Uuid,
) -> Result<SecretString, AppError> {
    let config = app_config
        .github
        .as_ref()
        .ok_or(AppError::Conflict("github_integration", "not_configured"))?;
    let scope = sqlx::query_as::<_, RepositoryGitScope>(
        r#"
        SELECT installation.installation_id,
               repository.github_repository_id
        FROM github_installations installation
        JOIN github_installation_repositories access
          ON access.project_id = installation.project_id
         AND access.github_installation_id = installation.id
         AND access.access_state = 'selected'
        JOIN repositories repository
          ON repository.project_id = access.project_id
         AND repository.id = access.repository_id
         AND repository.github_repository_id = access.github_repository_id
        WHERE installation.project_id = $1
          AND installation.enabled
          AND installation.state = 'available'
          AND repository.id = $2
          AND repository.enabled
          AND repository.trusted
          AND repository.archived_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(repository_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::Conflict(
        "github_integration",
        "repository_scope_unavailable",
    ))?;
    let installation_id = u64::try_from(scope.installation_id)
        .map_err(|_| AppError::Internal("GitHub Installation ID"))?;
    let github_repository_id = u64::try_from(scope.github_repository_id)
        .map_err(|_| AppError::Internal("GitHub Repository ID"))?;
    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::ContentsRead,
        )
        .await
        .map_err(map_github_error)?;
    Ok(token.secret)
}

/// Why issues couldn't be read, in words for the person importing.
pub(crate) enum IssuesUnavailable {
    NotConfigured,
    NoAccess,
    Failed,
}

impl IssuesUnavailable {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::NotConfigured => "GitHub isn't set up on this server.",
            Self::NoAccess => {
                "The GitHub App can't read this repository's issues. Give it Issues: read permission and connect the repository again."
            }
            Self::Failed => "GitHub didn't answer. Try the import again in a moment.",
        }
    }
}

/// Reads up to `limit` issues (pull requests included, as GitHub lists
/// them) of a repository connected to the project. Needs the GitHub App's
/// optional Issues: read permission.
pub(crate) async fn repository_issues(
    state: &AppState,
    project_id: Uuid,
    repository_id: Uuid,
    issue_state: &str,
    limit: usize,
) -> Result<Result<Vec<crate::work_items::import_parse::GitHubIssue>, IssuesUnavailable>, AppError>
{
    let Some(config) = state.config.github.as_ref() else {
        return Ok(Err(IssuesUnavailable::NotConfigured));
    };
    let scope = sqlx::query_as::<_, (i64, i64, String, String)>(
        r#"
        SELECT installation.installation_id, repository.github_repository_id,
               repository.owner, repository.name
        FROM github_installations installation
        JOIN github_installation_repositories access
          ON access.project_id = installation.project_id
         AND access.github_installation_id = installation.id
         AND access.access_state = 'selected'
        JOIN repositories repository
          ON repository.project_id = access.project_id
         AND repository.id = access.repository_id
         AND repository.github_repository_id = access.github_repository_id
        WHERE installation.project_id = $1
          AND installation.enabled
          AND installation.state = 'available'
          AND repository.id = $2
          AND repository.enabled
          AND repository.archived_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(repository_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((installation_id, github_repository_id, owner, name)) = scope else {
        return Ok(Err(IssuesUnavailable::NoAccess));
    };
    let (Ok(installation_id), Ok(github_repository_id)) = (
        u64::try_from(installation_id),
        u64::try_from(github_repository_id),
    ) else {
        return Err(AppError::Internal("GitHub IDs"));
    };
    let Ok(client) = GitHubClient::new(config) else {
        return Ok(Err(IssuesUnavailable::Failed));
    };
    let Ok(token) = client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::IssuesRead,
        )
        .await
    else {
        return Ok(Err(IssuesUnavailable::NoAccess));
    };
    let mut issues = Vec::new();
    for page in 1..=100 {
        let Ok(batch) = client
            .issues_page(&token, &owner, &name, issue_state, page)
            .await
        else {
            return Ok(Err(IssuesUnavailable::Failed));
        };
        let last = batch.len() < 100;
        issues.extend(batch);
        if last || issues.len() >= limit {
            break;
        }
    }
    issues.truncate(limit);
    Ok(Ok(issues))
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

#[derive(Clone, Debug, Deserialize)]
struct GitHubPullRequest {
    number: u64,
    node_id: String,
    html_url: String,
    title: String,
    #[serde(default)]
    body: Option<String>,
    user: GitHubPullRequestUser,
    draft: bool,
    state: String,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
    created_at: String,
    updated_at: String,
    head: GitHubPullRequestRef,
    base: GitHubPullRequestRef,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    requested_reviewers: Vec<GitHubPullRequestUser>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubPullRequestUser {
    login: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubPullRequestRef {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitHubCheckRuns {
    total_count: u64,
    check_runs: Vec<GitHubCheckRun>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubCheckRun {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    details_url: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubReview {
    id: u64,
    user: GitHubPullRequestUser,
    state: String,
    submitted_at: Option<String>,
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

struct CreatePullRequestCall<'a> {
    owner: &'a str,
    repository: &'a str,
    title: &'a str,
    body: &'a str,
    head_ref: &'a str,
    base_ref: &'a str,
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

    async fn open_pull_requests(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        head_ref: &str,
        base_ref: &str,
    ) -> Result<Vec<GitHubPullRequest>, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend(["repos", owner, repository, "pulls"]);
        url.query_pairs_mut()
            .append_pair("state", "open")
            .append_pair("head", &format!("{owner}:{head_ref}"))
            .append_pair("base", base_ref)
            .append_pair("per_page", "10");
        let response = self
            .http
            .get(url)
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        let pull_requests: Vec<GitHubPullRequest> = decode_response(response).await?;
        if pull_requests.len() > 10 {
            return Err(GitHubError::Response);
        }
        Ok(pull_requests)
    }

    async fn create_pull_request(
        &self,
        token: &InstallationToken,
        request: CreatePullRequestCall<'_>,
    ) -> Result<GitHubPullRequest, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend(["repos", request.owner, request.repository, "pulls"]);
        let response = self
            .http
            .post(url)
            .headers(github_headers(token.expose())?)
            .json(&json!({
                "title": request.title,
                "body": request.body,
                "head": request.head_ref,
                "base": request.base_ref,
                "draft": false,
                "maintainer_can_modify": false
            }))
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        decode_response(response).await
    }

    async fn pull_request(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        number: u64,
    ) -> Result<GitHubPullRequest, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend(["repos", owner, repository, "pulls", &number.to_string()]);
        let response = self
            .http
            .get(url)
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        decode_response(response).await
    }

    /// Merges a pull request if its head is still `head_sha`.
    async fn merge_pull_request(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        number: u64,
        head_sha: &str,
        method: &str,
    ) -> Result<MergeCall, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend([
                "repos",
                owner,
                repository,
                "pulls",
                &number.to_string(),
                "merge",
            ]);
        let response = self
            .http
            .put(url)
            .headers(github_headers(token.expose())?)
            .json(&json!({ "sha": head_sha, "merge_method": method }))
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        match response.status() {
            StatusCode::METHOD_NOT_ALLOWED => Ok(MergeCall::NotMergeable),
            StatusCode::CONFLICT => Ok(MergeCall::HeadMoved),
            status if status.is_success() => {
                let merged: GitHubMergeResponse = decode_response(response).await?;
                Ok(if merged.merged && is_commit_sha(&merged.sha) {
                    MergeCall::Merged(merged.sha)
                } else {
                    MergeCall::NotMergeable
                })
            }
            _ => Err(GitHubError::Response),
        }
    }

    async fn check_runs(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        head_sha: &str,
    ) -> Result<Vec<GitHubCheckRun>, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend([
                "repos",
                owner,
                repository,
                "commits",
                head_sha,
                "check-runs",
            ]);
        url.query_pairs_mut().append_pair("per_page", "100");
        let response: GitHubCheckRuns = decode_response(
            self.http
                .get(url)
                .headers(github_headers(token.expose())?)
                .send()
                .await
                .map_err(|_| GitHubError::Http)?,
        )
        .await?;
        if response.total_count > 100 || response.check_runs.len() > 100 {
            return Err(GitHubError::Response);
        }
        Ok(response.check_runs)
    }

    /// One page of a repository's issues, oldest first. Issue bodies can be
    /// long, so a page may be larger than other responses.
    async fn issues_page(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        state: &str,
        page: u32,
    ) -> Result<Vec<crate::work_items::import_parse::GitHubIssue>, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend(["repos", owner, repository, "issues"]);
        url.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("sort", "created")
            .append_pair("direction", "asc")
            .append_pair("per_page", "100")
            .append_pair("page", &page.to_string());
        decode_response_within(
            self.http
                .get(url)
                .headers(github_headers(token.expose())?)
                .send()
                .await
                .map_err(|_| GitHubError::Http)?,
            16 * 1024 * 1024,
        )
        .await
    }

    async fn reviews(
        &self,
        token: &InstallationToken,
        owner: &str,
        repository: &str,
        number: u64,
    ) -> Result<Vec<GitHubReview>, GitHubError> {
        let mut url =
            reqwest::Url::parse(&self.config.api_base_url).map_err(|_| GitHubError::Response)?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::Response)?
            .extend([
                "repos",
                owner,
                repository,
                "pulls",
                &number.to_string(),
                "reviews",
            ]);
        url.query_pairs_mut().append_pair("per_page", "100");
        let reviews: Vec<GitHubReview> = decode_response(
            self.http
                .get(url)
                .headers(github_headers(token.expose())?)
                .send()
                .await
                .map_err(|_| GitHubError::Http)?,
        )
        .await?;
        if reviews.len() > 100 {
            return Err(GitHubError::Response);
        }
        Ok(reviews)
    }
}

async fn decode_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, GitHubError> {
    decode_response_within(response, MAX_GITHUB_RESPONSE_BYTES).await
}

async fn decode_response_within<T: DeserializeOwned>(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<T, GitHubError> {
    if !response.status().is_success() {
        return Err(GitHubError::Response);
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(GitHubError::Response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| GitHubError::Http)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
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
        Action::GithubIntegrationManage,
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

async fn github_automation_settings(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<GitHubAutomationSettingsView>, AppError> {
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let settings = fetch_github_automation_settings(&mut transaction, project_id).await?;
    transaction.commit().await?;
    Ok(Json(settings))
}

async fn update_github_automation_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateGitHubAutomationSettingsRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("projects", project_id, "github_automation")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
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
            let settings: GitHubAutomationSettingsView = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(settings)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let settings = set_automation_in(
        &mut transaction,
        project_id,
        Some(request.expected_version),
        Some(request.pull_request_open_enabled),
        Some(request.pull_request_merge_enabled),
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session',
            'github.automation_settings_update', 'project', $3,
            'succeeded', $5,
            jsonb_build_object(
                'pull_request_open_enabled', $6::boolean,
                'pull_request_merge_enabled', $7::boolean,
                'version', $8::bigint
            )
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(
        headers
            .get(crate::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128),
    )
    .bind(settings.pull_request_open_enabled)
    .bind(settings.pull_request_merge_enabled)
    .bind(settings.version)
    .execute(&mut *transaction)
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &settings).await?;
    transaction.commit().await?;
    Ok((StatusCode::OK, Json(settings)).into_response())
}

/// Turns the project's pull request automation on or off, for a caller
/// already allowed to manage its GitHub integration. A switch left out keeps
/// its setting; without `expected_version`, the current one. Not audited here.
pub(crate) async fn set_automation_in(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    expected_version: Option<i64>,
    pull_request_open_enabled: Option<bool>,
    pull_request_merge_enabled: Option<bool>,
) -> Result<GitHubAutomationSettingsView, AppError> {
    let updated = sqlx::query_as::<_, GitHubAutomationSettingsView>(
        r#"
        UPDATE project_github_automation_settings
        SET pull_request_open_enabled = coalesce($3, pull_request_open_enabled),
            pull_request_merge_enabled = coalesce($4, pull_request_merge_enabled),
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND ($2::bigint IS NULL OR version = $2)
        RETURNING project_id, pull_request_open_enabled,
                  pull_request_merge_enabled, version, updated_at::text AS updated_at
        "#,
    )
    .bind(project_id)
    .bind(expected_version)
    .bind(pull_request_open_enabled)
    .bind(pull_request_merge_enabled)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(settings) = updated else {
        let current = fetch_github_automation_settings(transaction, project_id).await?;
        return Err(AppError::Conflict(
            "github_automation_settings",
            if Some(current.version) == expected_version {
                "update_failed"
            } else {
                "version"
            },
        ));
    };
    Ok(settings)
}

async fn fetch_github_automation_settings(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<GitHubAutomationSettingsView, AppError> {
    sqlx::query_as::<_, GitHubAutomationSettingsView>(
        r#"
        SELECT project_id, pull_request_open_enabled,
               pull_request_merge_enabled, version,
               updated_at::text AS updated_at
        FROM project_github_automation_settings
        WHERE project_id = $1
        FOR SHARE
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))
}

/// A project's GitHub installation linked or re-checked, with the
/// repositories it reaches, still in its transaction for the caller to audit
/// and commit.
pub(crate) struct SyncedInstallation {
    pub transaction: Transaction<'static, Postgres>,
    pub installation_id: Uuid,
    pub repository_count: usize,
    pub role: RoleGrant,
}

/// Links (or re-checks) the project's GitHub App installation and the
/// repositories it reaches, after checking with GitHub that it does, for a
/// caller allowed to manage the project's GitHub integration. Not audited
/// here.
pub(crate) async fn sync_installation_as(
    state: &AppState,
    actor_id: Uuid,
    kind: ActorKind,
    project_id: Uuid,
    installation_id: u64,
    repository_ids: &[Uuid],
) -> Result<SyncedInstallation, AppError> {
    if installation_id == 0 || repository_ids.is_empty() || repository_ids.len() > 50 {
        return Err(AppError::Validation("githubInstallation"));
    }
    let mut requested_repository_ids = repository_ids.to_vec();
    requested_repository_ids.sort_unstable();
    requested_repository_ids.dedup();
    if requested_repository_ids.len() != repository_ids.len() {
        return Err(AppError::Validation("repositoryIds"));
    }
    let github_config = state
        .config
        .github
        .as_ref()
        .ok_or(AppError::Conflict("github_integration", "not_configured"))?;

    let mut transaction = state.pool.begin().await?;
    let role = require_project_access_as(
        &mut transaction,
        actor_id,
        kind,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let repositories =
        load_requested_repositories(&mut transaction, project_id, &requested_repository_ids)
            .await?;
    let typed_installation_id =
        i64::try_from(installation_id).map_err(|_| AppError::Validation("installationId"))?;
    // Where people can sign in to GitHub, only installations the workspace
    // proved access to can be linked; knowing the number is not enough.
    if github_config.connect.is_some()
        && !connect::workspace_claims_installation(
            &mut transaction,
            project_id,
            typed_installation_id,
        )
        .await?
    {
        return Err(AppError::NotFound("github_installation"));
    }
    transaction.commit().await?;
    if repositories.len() != requested_repository_ids.len() {
        return Err(AppError::NotFound("repository"));
    }

    let client = GitHubClient::new(github_config).map_err(map_github_error)?;
    let installation = client
        .installation(installation_id)
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
            installation_id,
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
    require_project_access_as(
        &mut transaction,
        actor_id,
        kind,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let current_repositories =
        load_requested_repositories(&mut transaction, project_id, &requested_repository_ids)
            .await?;
    if current_repositories != repositories {
        return Err(AppError::Conflict("repository", "changed_during_sync"));
    }
    if github_config.connect.is_none() {
        connect::record_operator_claim(&mut transaction, project_id, actor_id, &installation)
            .await?;
    }
    let actor_id = ensure_github_actor(&mut transaction, project_id).await?;
    let stored_installation_id = persist_installation(
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
    .bind(stored_installation_id)
    .execute(&mut *transaction)
    .await?;
    if !repositories.is_empty() {
        let repository_ids = repositories
            .iter()
            .map(|repository| repository.id)
            .collect::<Vec<_>>();
        let github_repository_ids = repositories
            .iter()
            .map(|repository| repository.github_repository_id)
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO github_installation_repositories (
                project_id, github_installation_id, repository_id,
                github_repository_id, access_state, last_verified_at
            )
            SELECT $1, $2, selected.repository_id,
                   selected.github_repository_id, 'selected', now()
            FROM unnest($3::uuid[], $4::bigint[])
                 AS selected(repository_id, github_repository_id)
            ON CONFLICT (github_installation_id, repository_id) DO UPDATE
                SET github_repository_id = EXCLUDED.github_repository_id,
                    access_state = 'selected',
                    last_verified_at = now(),
                    updated_at = now()
            "#,
        )
        .bind(project_id)
        .bind(stored_installation_id)
        .bind(&repository_ids)
        .bind(&github_repository_ids)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE repositories
            SET github_app_installation_id = $3, updated_at = now()
            WHERE project_id = $1 AND id = ANY($2::uuid[])
            "#,
        )
        .bind(project_id)
        .bind(&repository_ids)
        .bind(i64::try_from(installation_id).map_err(|_| AppError::Validation("installationId"))?)
        .execute(&mut *transaction)
        .await?;
    }
    Ok(SyncedInstallation {
        transaction,
        installation_id: stored_installation_id,
        repository_count: repositories.len(),
        role,
    })
}

async fn sync_installation(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SyncInstallationRequest>,
) -> Result<Json<GitHubStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let SyncedInstallation {
        mut transaction,
        installation_id,
        repository_count,
        role,
    } = sync_installation_as(
        &state,
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        project_id,
        request.installation_id,
        &request.repository_ids,
    )
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
    .bind(i32::try_from(repository_count).map_err(|_| AppError::Internal("repository count"))?)
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

/// What a publish, pull request or refresh request queued: a new job, or
/// the one already queued (or done) for the same thing.
pub(crate) struct Queued<T> {
    pub view: T,
    pub new: bool,
}

/// Queues pushing a finished run's branch to GitHub, for a caller allowed to
/// publish in the project who can see the run's thread. Not audited here;
/// the caller commits and then calls `kick_jobs`.
pub(crate) async fn queue_publish_in(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    run_id: Uuid,
) -> Result<Queued<PublishJobView>, AppError> {
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
          AND conversation_visible_to(run.conversation_id, $3)
          AND repository.enabled
          AND repository.trusted
          AND repository.archived_at IS NULL
          AND workspace.state IN ('active', 'retained')
        FOR SHARE OF run, repository, workspace, worktree
        "#,
    )
    .bind(project_id)
    .bind(run_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
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
    .fetch_one(&mut **transaction)
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
    .fetch_optional(&mut **transaction)
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
    .execute(&mut **transaction)
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
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(Queued {
            view: existing,
            new: false,
        });
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
    .bind(actor_id)
    .bind(&view.intended_commit)
    .execute(&mut **transaction)
    .await?;
    outbox::enqueue_agent_run_updated(
        transaction,
        project_id,
        candidate.conversation_id,
        candidate.agent_run_id,
    )
    .await?;
    Ok(Queued { view, new: true })
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
    let Queued { view, new } = queue_publish_in(
        &state,
        &mut transaction,
        principal.actor_id,
        project_id,
        run_id,
    )
    .await?;
    if !new {
        let status = if view.state == "completed" {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        complete(&mut transaction, &reservation, status, &view).await?;
        transaction.commit().await?;
        return Ok((status, Json(view)).into_response());
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
    .bind(view.job_id)
    .bind(request_id)
    .bind(view.agent_run_id)
    .bind(view.repository_workspace_id)
    .execute(&mut *transaction)
    .await?;
    complete(&mut transaction, &reservation, StatusCode::ACCEPTED, &view).await?;
    transaction.commit().await?;
    kick_jobs(state);
    Ok((StatusCode::ACCEPTED, Json(view)).into_response())
}

/// A pull request's title (trimmed), body and tasks, as GitHub and the app
/// take them.
pub(crate) fn checked_pull_request_input(
    title: &str,
    body: &str,
    work_item_ids: &[Uuid],
) -> Result<String, AppError> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > 256
        || title.chars().any(char::is_control)
        || body.chars().count() > 10_000
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        || work_item_ids.len() > 20
        || work_item_ids.iter().copied().collect::<BTreeSet<_>>().len() != work_item_ids.len()
    {
        return Err(AppError::Invalid("pull request input"));
    }
    Ok(title.to_owned())
}

/// Queues opening a pull request from a finished run's published branch,
/// for a caller allowed to open pull requests in the project who can see the
/// run's thread. `title` is already checked (`checked_pull_request_input`).
/// Not audited here; the caller commits and then calls `kick_jobs`.
pub(crate) async fn queue_pull_request_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    run_id: Uuid,
    title: &str,
    body: &str,
    work_item_ids: &[Uuid],
) -> Result<Queued<PullRequestRequestView>, AppError> {
    if !work_item_ids.is_empty() {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM work_items WHERE project_id = $1 AND id = ANY($2) AND archived_at IS NULL",
        )
        .bind(project_id)
        .bind(work_item_ids)
        .fetch_one(&mut **transaction)
        .await?;
        if usize::try_from(count).ok() != Some(work_item_ids.len()) {
            return Err(AppError::NotFound("work_item"));
        }
    }
    let candidate = sqlx::query_as::<_, PullRequestCandidate>(
        r#"
        SELECT run.conversation_id, repository.id AS repository_id,
               workspace.id AS repository_workspace_id,
               workspace.branch_name, workspace.published_commit,
               repository.default_branch,
               installation.id AS github_installation_id
        FROM agent_runs run
        JOIN repositories repository
          ON repository.project_id = run.project_id
         AND repository.id = run.repository_id
         AND repository.enabled
         AND repository.trusted
         AND repository.archived_at IS NULL
        JOIN repository_workspaces workspace
          ON workspace.project_id = run.project_id
         AND workspace.id = run.repository_workspace_id
         AND workspace.repository_id = repository.id
         AND workspace.state IN ('active', 'retained')
         AND workspace.published_commit IS NOT NULL
        JOIN github_installation_repositories access
          ON access.project_id = run.project_id
         AND access.repository_id = repository.id
         AND access.github_repository_id = repository.github_repository_id
         AND access.access_state = 'selected'
        JOIN github_installations installation
          ON installation.project_id = run.project_id
         AND installation.id = access.github_installation_id
         AND installation.enabled
         AND installation.state = 'available'
        WHERE run.project_id = $1
          AND run.id = $2
          AND conversation_visible_to(run.conversation_id, $3)
          AND run.state IN ('completed', 'failed', 'canceled', 'interrupted')
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs active
              WHERE active.repository_workspace_id = workspace.id
                AND active.state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
          )
        FOR SHARE OF run, repository, workspace, access, installation
        "#,
    )
    .bind(project_id)
    .bind(run_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Conflict("agent_run", "branch_not_published"))?;
    if let Some(existing) = sqlx::query_as::<_, PullRequestRequestView>(
        r#"
        SELECT id AS request_id, agent_run_id, repository_workspace_id,
               head_ref, head_sha, base_ref, state, failure_category,
               github_pull_request_id
        FROM github_pull_request_requests
        WHERE project_id = $1 AND repository_workspace_id = $2
        FOR SHARE
        "#,
    )
    .bind(project_id)
    .bind(candidate.repository_workspace_id)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(Queued {
            view: existing,
            new: false,
        });
    }
    let request_id = Uuid::now_v7();
    let view = PullRequestRequestView {
        request_id,
        agent_run_id: run_id,
        repository_workspace_id: candidate.repository_workspace_id,
        head_ref: candidate.branch_name,
        head_sha: candidate.published_commit,
        base_ref: candidate.default_branch,
        state: "queued".to_owned(),
        failure_category: None,
        github_pull_request_id: None,
    };
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_requests (
            id, project_id, github_installation_id, repository_id,
            repository_workspace_id, agent_run_id, requesting_actor_id,
            head_ref, head_sha, base_ref, title, body
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(request_id)
    .bind(project_id)
    .bind(candidate.github_installation_id)
    .bind(candidate.repository_id)
    .bind(candidate.repository_workspace_id)
    .bind(run_id)
    .bind(actor_id)
    .bind(&view.head_ref)
    .bind(&view.head_sha)
    .bind(&view.base_ref)
    .bind(title)
    .bind(body)
    .execute(&mut **transaction)
    .await?;
    let job_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO github_jobs (
            id, project_id, github_installation_id, repository_id,
            kind, payload, repository_workspace_id, agent_run_id,
            requesting_actor_id, intended_commit, pull_request_request_id
        ) VALUES (
            $1, $2, $3, $4, 'pull_request_create',
            jsonb_build_object('work_item_ids', $5::uuid[]),
            $6, $7, $8, $9, $10
        )
        "#,
    )
    .bind(job_id)
    .bind(project_id)
    .bind(candidate.github_installation_id)
    .bind(candidate.repository_id)
    .bind(work_item_ids)
    .bind(candidate.repository_workspace_id)
    .bind(run_id)
    .bind(actor_id)
    .bind(&view.head_sha)
    .bind(request_id)
    .execute(&mut **transaction)
    .await?;
    outbox::enqueue_agent_run_updated(transaction, project_id, candidate.conversation_id, run_id)
        .await?;
    Ok(Queued { view, new: true })
}

async fn create_pull_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, run_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<CreatePullRequestRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if state.config.github.is_none() {
        return Err(AppError::Conflict("github_integration", "not_configured"));
    }
    request.title =
        checked_pull_request_input(&request.title, &request.body, &request.work_item_ids)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("agent_runs", run_id, "pull_request")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::PullRequestCreate,
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
            let view: PullRequestRequestView = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(view)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let Queued { view, new } = queue_pull_request_in(
        &mut transaction,
        principal.actor_id,
        project_id,
        run_id,
        &request.title,
        &request.body,
        &request.work_item_ids,
    )
    .await?;
    if !new {
        let status = if view.state == "completed" {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        complete(&mut transaction, &reservation, status, &view).await?;
        transaction.commit().await?;
        return Ok((status, Json(view)).into_response());
    }
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session',
            'github.pull_request_queued', 'github_pull_request_request', $5,
            'succeeded', $6,
            jsonb_build_object('agent_run_id', $7::uuid, 'repository_workspace_id', $8::uuid)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(view.request_id)
    .bind(
        headers
            .get(crate::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128),
    )
    .bind(run_id)
    .bind(view.repository_workspace_id)
    .execute(&mut *transaction)
    .await?;
    complete(&mut transaction, &reservation, StatusCode::ACCEPTED, &view).await?;
    transaction.commit().await?;
    kick_jobs(state);
    Ok((StatusCode::ACCEPTED, Json(view)).into_response())
}

/// Queues reading a pull request's checks and review from GitHub again,
/// for a caller allowed to refresh pull requests in the project who can see
/// this one. The caller commits and then calls `kick_jobs`.
pub(crate) async fn queue_refresh_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    pull_request_id: Uuid,
) -> Result<Queued<PullRequestRefreshView>, AppError> {
    let pull_request = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT pull_request.github_installation_id, pull_request.repository_id
        FROM github_pull_requests pull_request
        JOIN repositories repository
          ON repository.project_id = pull_request.project_id
         AND repository.id = pull_request.repository_id
         AND repository.enabled
         AND repository.archived_at IS NULL
        WHERE pull_request.project_id = $1 AND pull_request.id = $2
          AND pull_request_visible_to(pull_request.id, $3)
        FOR SHARE OF pull_request, repository
        "#,
    )
    .bind(project_id)
    .bind(pull_request_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("github_pull_request"))?;
    if let Some(existing) = sqlx::query_as::<_, PullRequestRefreshView>(
        r#"
        SELECT id AS job_id, github_pull_request_id AS pull_request_id,
               state, failure_category
        FROM github_jobs
        WHERE project_id = $1 AND github_pull_request_id = $2
          AND kind IN ('pull_request_sync', 'check_review_sync')
          AND state IN ('queued', 'running', 'failed')
        ORDER BY created_at DESC
        LIMIT 1
        FOR SHARE
        "#,
    )
    .bind(project_id)
    .bind(pull_request_id)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(Queued {
            view: existing,
            new: false,
        });
    }
    let job_id = Uuid::now_v7();
    let view = PullRequestRefreshView {
        job_id,
        pull_request_id,
        state: "queued".to_owned(),
        failure_category: None,
    };
    sqlx::query(
        r#"
        INSERT INTO github_jobs (
            id, project_id, github_installation_id, repository_id,
            kind, requesting_actor_id, github_pull_request_id
        ) VALUES ($1, $2, $3, $4, 'check_review_sync', $5, $6)
        "#,
    )
    .bind(job_id)
    .bind(project_id)
    .bind(pull_request.0)
    .bind(pull_request.1)
    .bind(actor_id)
    .bind(pull_request_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE github_pull_requests SET sync_state = 'stale', last_sync_attempt_at = now(), updated_at = now() WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(pull_request_id)
    .execute(&mut **transaction)
    .await?;
    Ok(Queued { view, new: true })
}

async fn refresh_pull_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, pull_request_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if state.config.github.is_none() {
        return Err(AppError::Conflict("github_integration", "not_configured"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("github_pull_requests", pull_request_id, "refresh")?;
    let request_hash = RequestHash::for_value(&json!({}))?;
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::PullRequestRefresh,
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
            let view: PullRequestRefreshView = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(view)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let Queued { view, .. } = queue_refresh_in(
        &mut transaction,
        principal.actor_id,
        project_id,
        pull_request_id,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::ACCEPTED, &view).await?;
    transaction.commit().await?;
    kick_jobs(state);
    Ok((StatusCode::ACCEPTED, Json(view)).into_response())
}

async fn drain_jobs(state: &AppState) -> Result<(), AppError> {
    loop {
        // Each job keeps renewing its lease while it works: a publish alone
        // can take minutes (a token, two ref reads and a push), far past one
        // lease, and an expired lease lets another worker run it again.
        if let Some(job) = claim_publish_job(state).await? {
            let job_id = job.job_id;
            match while_leased(
                state,
                job.job_id,
                job.claim_id,
                process_publish_job(state, &job),
            )
            .await
            {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    tracing::error!(
                        %job_id,
                        error_category = error.log_category(),
                        "GitHub publication job processing failed"
                    );
                    fail_publish_job(state, &job, "job_processing_failed", false).await?;
                }
                None => lost_lease(job_id),
            }
            continue;
        }
        if let Some(job) = claim_pull_request_job(state).await? {
            let job_id = job.job_id;
            match while_leased(
                state,
                job.job_id,
                job.claim_id,
                process_pull_request_job(state, &job),
            )
            .await
            {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    tracing::error!(
                        %job_id,
                        error_category = error.log_category(),
                        "GitHub pull-request job processing failed"
                    );
                    fail_pull_request_job(state, &job, "job_processing_failed", false).await?;
                }
                None => lost_lease(job_id),
            }
            continue;
        }
        if let Some(job) = claim_pull_request_adoption_job(state).await? {
            let job_id = job.job_id;
            match while_leased(
                state,
                job.job_id,
                job.claim_id,
                process_pull_request_adoption_job(state, &job),
            )
            .await
            {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    tracing::error!(
                        %job_id,
                        error_category = error.log_category(),
                        "GitHub pull-request pickup job processing failed"
                    );
                    fail_pull_request_adoption_job(state, &job, "job_processing_failed", false)
                        .await?;
                }
                None => lost_lease(job_id),
            }
            continue;
        }
        if let Some(job) = claim_pull_request_sync_job(state).await? {
            let job_id = job.job_id;
            match while_leased(
                state,
                job.job_id,
                job.claim_id,
                process_pull_request_sync_job(state, &job),
            )
            .await
            {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    tracing::error!(
                        %job_id,
                        error_category = error.log_category(),
                        "GitHub pull-request synchronization job processing failed"
                    );
                    fail_pull_request_sync_job(state, &job, "job_processing_failed", false).await?;
                }
                None => lost_lease(job_id),
            }
            continue;
        }
        return Ok(());
    }
}

fn lost_lease(job_id: Uuid) {
    tracing::warn!(
        %job_id,
        "a GitHub job lost its lease and was stopped; it will be retried"
    );
}

/// Runs a claimed GitHub job's work while extending its lease. Returns None
/// when the lease is lost (another worker took the job, or renewals kept
/// failing until it may have expired): the work is dropped there, so no
/// later step runs twice. Work already handed to a blocking thread (a push)
/// finishes on its own, but nothing after it does.
async fn while_leased<T>(
    state: &AppState,
    job_id: Uuid,
    claim_id: Uuid,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    let pool = state.pool.clone();
    hold_lease(GITHUB_JOB_LEASE, work, move || {
        let pool = pool.clone();
        async move { extend_github_lease(&pool, job_id, claim_id).await }
    })
    .await
}

async fn extend_github_lease(
    pool: &sqlx::PgPool,
    job_id: Uuid,
    claim_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE github_jobs
        SET lease_expires_at = now() + make_interval(secs => $3), updated_at = now()
        WHERE id = $1 AND state = 'running' AND lease_owner = $2
        "#,
    )
    .bind(job_id)
    .bind(claim_id)
    .bind(GITHUB_JOB_LEASE.as_secs_f64())
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Extends a lease every third of its length while `work` runs, as
/// background jobs do (`jobs::renew_lease`). A renewal that answers "not
/// yours" ends the work at once; one that fails is retried while the lease
/// surely still holds (a wait of a third plus a try of a sixth).
async fn hold_lease<T, R, F, E>(
    lease: Duration,
    work: impl std::future::Future<Output = T>,
    mut renew: R,
) -> Option<T>
where
    R: FnMut() -> F,
    F: std::future::Future<Output = Result<bool, E>>,
{
    let renewals = async {
        let mut renewed_at = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(lease / 3).await;
            let sent = tokio::time::Instant::now();
            match tokio::time::timeout(lease / 6, renew()).await {
                Ok(Ok(true)) => renewed_at = sent,
                Ok(Ok(false)) => return,
                Ok(Err(_)) | Err(_) => {
                    if renewed_at.elapsed() + lease / 3 + lease / 6 >= lease {
                        return;
                    }
                }
            }
        }
    };
    tokio::select! {
        result = work => Some(result),
        () = renewals => None,
    }
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
        SELECT job.id AS job_id, job.project_id,
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

async fn claim_pull_request_job(
    state: &AppState,
) -> Result<Option<ClaimedPullRequestJob>, AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE github_pull_request_requests request
        SET state = job.state, failure_category = job.failure_category,
            updated_at = now()
        FROM github_jobs job
        WHERE job.pull_request_request_id = request.id
          AND job.kind = 'pull_request_create'
          AND job.state IN ('failed', 'exhausted')
          AND request.state = 'running'
        "#,
    )
    .execute(&mut *transaction)
    .await?;
    let claim_id = Uuid::now_v7();
    let job = sqlx::query_as::<_, ClaimedPullRequestJob>(
        r#"
        SELECT job.id AS job_id, request.id AS request_id,
               job.project_id, job.repository_id,
               job.repository_workspace_id, job.agent_run_id,
               job.requesting_actor_id, $1::uuid AS claim_id,
               job.attempts + 1 AS attempts, job.max_attempts,
               run.conversation_id, installation.installation_id,
               repository.github_repository_id, repository.owner,
               repository.name AS repository_name,
               request.head_ref, request.head_sha, request.base_ref,
               request.title, request.body,
               (request.mutation_attempted_at IS NOT NULL) AS mutation_attempted,
               COALESCE(
                   installation.enabled
                   AND installation.state = 'available'
                   AND access.access_state = 'selected'
                   AND access.github_repository_id = repository.github_repository_id
                   AND repository.enabled
                   AND repository.trusted
                   AND repository.archived_at IS NULL,
                   false
               ) AS scope_available,
               job.payload -> 'work_item_ids' AS work_item_ids
        FROM github_jobs job
        JOIN github_pull_request_requests request
          ON request.project_id = job.project_id
         AND request.id = job.pull_request_request_id
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
        JOIN agent_runs run
          ON run.project_id = job.project_id
         AND run.id = job.agent_run_id
        WHERE job.kind = 'pull_request_create'
          AND job.state IN ('queued', 'failed')
          AND job.next_attempt_at <= now()
          AND job.attempts < job.max_attempts
        ORDER BY job.next_attempt_at, job.created_at, job.id
        FOR UPDATE OF job, request SKIP LOCKED
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
    sqlx::query(
        "UPDATE github_pull_request_requests SET state = 'running', failure_category = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(job.request_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(job))
}

async fn claim_pull_request_sync_job(
    state: &AppState,
) -> Result<Option<ClaimedPullRequestSyncJob>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let unresolved = sqlx::query_as::<_, (Option<Uuid>, String)>(
        r#"
        WITH candidate AS (
            SELECT job.id
            FROM github_jobs job
            WHERE job.kind IN ('pull_request_sync', 'check_review_sync')
              AND job.github_pull_request_id IS NULL
              AND job.state IN ('queued', 'failed')
              AND job.next_attempt_at <= now()
              AND job.attempts < job.max_attempts
              -- A pull request that names one of the project's tasks is
              -- picked up first (claim_pull_request_adoption_job).
              AND NOT (job.kind = 'pull_request_sync' AND github_job_names_open_task(job.project_id, job.payload))
              AND NOT EXISTS (
                  SELECT 1
                  FROM github_pull_requests pull_request
                  WHERE pull_request.project_id = job.project_id
                    AND pull_request.repository_id = job.repository_id
                    AND (
                        (
                            (job.payload ->> 'number') ~ '^[0-9]{1,18}$'
                            AND pull_request.pull_request_number = (job.payload ->> 'number')::bigint
                        )
                        OR (
                            NOT COALESCE((job.payload ->> 'number') ~ '^[0-9]{1,18}$', false)
                            AND
                            (job.payload ->> 'head_sha') ~ '^[0-9a-f]{40}$'
                            AND pull_request.head_sha = job.payload ->> 'head_sha'
                        )
                    )
              )
            ORDER BY job.next_attempt_at, job.created_at, job.id
            FOR UPDATE OF job SKIP LOCKED
            LIMIT 1
        )
        UPDATE github_jobs job
        SET attempts = attempts + 1,
            state = CASE WHEN attempts + 1 >= max_attempts THEN 'exhausted' ELSE 'failed' END,
            failure_category = 'pull_request_unresolved',
            next_attempt_at = now() + make_interval(
                secs => LEAST(((attempts + 1) * (attempts + 1))::double precision, 60.0)
            ),
            updated_at = now()
        FROM candidate
        WHERE job.id = candidate.id
        RETURNING job.webhook_delivery_id, job.state
        "#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some((Some(delivery_id), delivery_state)) = unresolved {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = $2, failure_category = 'pull_request_unresolved', processed_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .bind(delivery_state)
        .execute(&mut *transaction)
        .await?;
    }
    let claim_id = Uuid::now_v7();
    let job = sqlx::query_as::<_, ClaimedPullRequestSyncJob>(
        r#"
        SELECT job.id AS job_id, job.project_id,
               pull_request.id AS pull_request_id, $1::uuid AS claim_id,
               job.attempts + 1 AS attempts, job.max_attempts,
               job.webhook_delivery_id, installation.integration_actor_id,
               installation.installation_id, repository.github_repository_id,
               repository.owner, repository.name AS repository_name,
               pull_request.pull_request_number, pull_request.node_id,
               pull_request.html_url, pull_request.head_ref,
               pull_request.base_ref,
               pull_request.external_created_at,
               CASE
                   WHEN COALESCE(
                       job.payload ->> 'merge_commit_sha',
                       delivery.redelivery_payload ->> 'merge_commit_sha',
                       pull_request.merge_commit_sha
                   ) ~ '^[0-9a-f]{40}$'
                   THEN COALESCE(
                       job.payload ->> 'merge_commit_sha',
                       delivery.redelivery_payload ->> 'merge_commit_sha',
                       pull_request.merge_commit_sha
                   )
               END AS known_merge_commit_sha,
               installation.enabled
                   AND installation.state = 'available'
                   AND access.access_state = 'selected'
                   AND access.github_repository_id = repository.github_repository_id
                   AND repository.enabled
                   AND repository.archived_at IS NULL
                   AS scope_available
        FROM github_jobs job
        JOIN github_installations installation
          ON installation.project_id = job.project_id
         AND installation.id = job.github_installation_id
        JOIN repositories repository
          ON repository.project_id = job.project_id
         AND repository.id = job.repository_id
        LEFT JOIN github_installation_repositories access
          ON access.project_id = job.project_id
         AND access.github_installation_id = installation.id
         AND access.repository_id = repository.id
        LEFT JOIN github_webhook_deliveries delivery
          ON delivery.project_id = job.project_id
         AND delivery.id = job.webhook_delivery_id
        JOIN github_pull_requests pull_request
          ON pull_request.project_id = job.project_id
         AND pull_request.repository_id = repository.id
         AND (
             pull_request.id = job.github_pull_request_id
             OR (
                 job.github_pull_request_id IS NULL
                 AND (
                     (
                         (job.payload ->> 'number') ~ '^[0-9]{1,18}$'
                         AND pull_request.pull_request_number = (job.payload ->> 'number')::bigint
                     )
                     OR (
                         NOT COALESCE((job.payload ->> 'number') ~ '^[0-9]{1,18}$', false)
                         AND
                         (job.payload ->> 'head_sha') ~ '^[0-9a-f]{40}$'
                         AND pull_request.head_sha = job.payload ->> 'head_sha'
                     )
                 )
             )
         )
        WHERE job.kind IN ('pull_request_sync', 'check_review_sync')
          AND job.state IN ('queued', 'failed')
          AND job.next_attempt_at <= now()
          AND job.attempts < job.max_attempts
        ORDER BY job.next_attempt_at, job.created_at, job.id
        FOR UPDATE OF job, pull_request SKIP LOCKED
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
    sqlx::query(
        "UPDATE github_pull_requests SET sync_state = 'stale', last_sync_attempt_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(job.pull_request_id)
    .execute(&mut *transaction)
    .await?;
    if let Some(delivery_id) = job.webhook_delivery_id {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = 'processing', failure_category = NULL, processed_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .execute(&mut *transaction)
        .await?;
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

async fn process_pull_request_job(
    state: &AppState,
    job: &ClaimedPullRequestJob,
) -> Result<(), AppError> {
    if !job.scope_available {
        return fail_pull_request_job(state, job, "pull_request_scope_unavailable", true).await;
    }
    let Some(config) = state.config.github.as_ref() else {
        return fail_pull_request_job(state, job, "github_not_configured", true).await;
    };
    let installation_id = match u64::try_from(job.installation_id) {
        Ok(value) => value,
        Err(_) => {
            return fail_pull_request_job(state, job, "installation_identity_invalid", true).await;
        }
    };
    let github_repository_id = match u64::try_from(job.github_repository_id) {
        Ok(value) => value,
        Err(_) => {
            return fail_pull_request_job(state, job, "repository_identity_invalid", true).await;
        }
    };
    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = match client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::PullRequestsWrite,
        )
        .await
    {
        Ok(token) => token,
        Err(GitHubError::Scope) => {
            return fail_pull_request_job(state, job, "github_scope_incompatible", true).await;
        }
        Err(_) => {
            return fail_pull_request_job(state, job, "github_token_unavailable", false).await;
        }
    };
    let existing = client
        .open_pull_requests(
            &token,
            &job.owner,
            &job.repository_name,
            &job.head_ref,
            &job.base_ref,
        )
        .await;
    match existing {
        Ok(pull_requests) if pull_requests.len() > 1 => {
            drop(token);
            return fail_pull_request_job(state, job, "duplicate_open_pull_requests", true).await;
        }
        Ok(mut pull_requests) if pull_requests.len() == 1 => {
            let pull_request = pull_requests
                .pop()
                .ok_or(AppError::Internal("one pull request"))?;
            drop(token);
            validate_pull_request(job, &pull_request)?;
            return complete_pull_request_job(state, job, &pull_request).await;
        }
        Ok(_) => {}
        Err(_) => {
            drop(token);
            return fail_pull_request_job(state, job, "pull_request_lookup_failed", false).await;
        }
    }
    if job.mutation_attempted {
        drop(token);
        return fail_pull_request_job(state, job, "create_outcome_uncertain", false).await;
    }
    let marked = sqlx::query(
        r#"
        UPDATE github_pull_request_requests request
        SET mutation_attempted_at = now(), updated_at = now()
        FROM github_jobs job
        WHERE request.id = $1
          AND job.id = $2
          AND job.pull_request_request_id = request.id
          AND job.state = 'running'
          AND job.lease_owner = $3
        "#,
    )
    .bind(job.request_id)
    .bind(job.job_id)
    .bind(job.claim_id)
    .execute(&state.pool)
    .await?;
    if marked.rows_affected() != 1 {
        drop(token);
        return Err(AppError::Conflict("github_job", "lease_lost"));
    }
    let created = client
        .create_pull_request(
            &token,
            CreatePullRequestCall {
                owner: &job.owner,
                repository: &job.repository_name,
                title: &job.title,
                body: &job.body,
                head_ref: &job.head_ref,
                base_ref: &job.base_ref,
            },
        )
        .await;
    let pull_request = match created {
        Ok(pull_request) => pull_request,
        Err(_) => {
            match client
                .open_pull_requests(
                    &token,
                    &job.owner,
                    &job.repository_name,
                    &job.head_ref,
                    &job.base_ref,
                )
                .await
            {
                Ok(mut pull_requests) if pull_requests.len() == 1 => pull_requests
                    .pop()
                    .ok_or(AppError::Internal("one reconciled pull request"))?,
                Ok(pull_requests) if pull_requests.len() > 1 => {
                    drop(token);
                    return fail_pull_request_job(state, job, "duplicate_open_pull_requests", true)
                        .await;
                }
                _ => {
                    drop(token);
                    return fail_pull_request_job(state, job, "create_outcome_uncertain", false)
                        .await;
                }
            }
        }
    };
    drop(token);
    validate_pull_request(job, &pull_request)?;
    complete_pull_request_job(state, job, &pull_request).await
}

fn validate_pull_request(
    job: &ClaimedPullRequestJob,
    pull_request: &GitHubPullRequest,
) -> Result<(), AppError> {
    let expected_url = format!(
        "https://github.com/{}/{}/pull/{}",
        job.owner, job.repository_name, pull_request.number
    );
    if pull_request.number == 0
        || pull_request.node_id.is_empty()
        || pull_request.node_id.len() > 256
        || pull_request.node_id.chars().any(char::is_control)
        || pull_request.html_url != expected_url
        || pull_request.html_url.len() > 2048
        || pull_request.html_url.chars().any(char::is_control)
        || pull_request.head.name != job.head_ref
        || pull_request.head.sha != job.head_sha
        || pull_request.base.name != job.base_ref
        || pull_request.title.is_empty()
        || pull_request.title.chars().count() > 256
        || pull_request.title.chars().any(char::is_control)
        || pull_request.user.login.is_empty()
        || pull_request.user.login.len() > 100
        || pull_request.user.login.chars().any(char::is_control)
        || !matches!(pull_request.state.as_str(), "open" | "closed")
        || pull_request.created_at.len() > 64
        || pull_request.updated_at.len() > 64
        || pull_request.created_at.chars().any(char::is_control)
        || pull_request.updated_at.chars().any(char::is_control)
        || pull_request
            .merge_commit_sha
            .as_deref()
            .is_some_and(|sha| sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AppError::Conflict(
            "github_pull_request",
            "provider identity mismatch",
        ));
    }
    Ok(())
}

async fn complete_pull_request_job(
    state: &AppState,
    job: &ClaimedPullRequestJob,
    pull_request: &GitHubPullRequest,
) -> Result<(), AppError> {
    let pull_request_number = i64::try_from(pull_request.number)
        .map_err(|_| AppError::Internal("pull request number"))?;
    let merged = pull_request.merged_at.is_some();
    let canonical_state = if merged {
        "merged"
    } else {
        pull_request.state.as_str()
    };
    let merge_commit = if merged {
        pull_request.merge_commit_sha.as_deref()
    } else {
        None
    };
    if merged && merge_commit.is_none() {
        return Err(AppError::Conflict(
            "github_pull_request",
            "merged commit unavailable",
        ));
    }
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
    let pull_request_id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id,
            github_repository_id, pull_request_number, node_id, html_url,
            head_ref, head_sha, base_ref, title, author_login, draft, state,
            merged_at, merge_commit_sha, external_created_at,
            external_updated_at
        ) VALUES (
            $1, $2, $3,
            (SELECT github_installation_id FROM github_pull_request_requests WHERE id = $4),
            $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
            $16::text::timestamptz, $17, $18::text::timestamptz,
            $19::text::timestamptz
        )
        ON CONFLICT (repository_id, pull_request_number) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(pull_request_id)
    .bind(job.project_id)
    .bind(job.repository_id)
    .bind(job.request_id)
    .bind(job.github_repository_id)
    .bind(pull_request_number)
    .bind(&pull_request.node_id)
    .bind(&pull_request.html_url)
    .bind(&pull_request.head.name)
    .bind(&pull_request.head.sha)
    .bind(&pull_request.base.name)
    .bind(&pull_request.title)
    .bind(&pull_request.user.login)
    .bind(pull_request.draft)
    .bind(canonical_state)
    .bind(pull_request.merged_at.as_deref())
    .bind(merge_commit)
    .bind(&pull_request.created_at)
    .bind(&pull_request.updated_at)
    .fetch_optional(&mut *transaction)
    .await?;
    let pull_request_id = if let Some(id) = inserted {
        id
    } else {
        let existing = sqlx::query_as::<_, (Uuid, String, String, String, String)>(
            r#"
            SELECT id, node_id, html_url, head_ref, base_ref
            FROM github_pull_requests
            WHERE repository_id = $1 AND pull_request_number = $2
            FOR SHARE
            "#,
        )
        .bind(job.repository_id)
        .bind(pull_request_number)
        .fetch_one(&mut *transaction)
        .await?;
        if existing.1 != pull_request.node_id
            || existing.2 != pull_request.html_url
            || existing.3 != pull_request.head.name
            || existing.4 != pull_request.base.name
        {
            return Err(AppError::Conflict(
                "github_pull_request",
                "durable identity mismatch",
            ));
        }
        existing.0
    };
    sqlx::query(
        r#"
        UPDATE github_pull_request_requests
        SET state = 'completed', failure_category = NULL,
            github_pull_request_id = $2, completed_at = now(), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(job.request_id)
    .bind(pull_request_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_conversation_links (
            project_id, github_pull_request_id, conversation_id,
            source, created_by_actor_id
        ) VALUES ($1, $2, $3, 'run_artifact', $4)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(job.project_id)
    .bind(pull_request_id)
    .bind(job.conversation_id)
    .bind(job.requesting_actor_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_workspace_links (
            project_id, github_pull_request_id, repository_workspace_id,
            agent_run_id, commit_sha
        ) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(job.project_id)
    .bind(pull_request_id)
    .bind(job.repository_workspace_id)
    .bind(job.agent_run_id)
    .bind(&job.head_sha)
    .execute(&mut *transaction)
    .await?;
    if !job.work_item_ids.0.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_work_item_links (
                project_id, github_pull_request_id, work_item_id,
                source, confirmed, created_by_actor_id
            )
            SELECT $1, $2, work_item_id, 'explicit', true, $4
            FROM unnest($3::uuid[]) AS selected(work_item_id)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(job.project_id)
        .bind(pull_request_id)
        .bind(&job.work_item_ids.0)
        .bind(job.requesting_actor_id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class,
            action, target_kind, target_id, outcome, metadata
        ) VALUES (
            $1, $2, $3, 'github_job', 'github.pull_request_create',
            'github_pull_request', $4, 'succeeded',
            jsonb_build_object(
                'agent_run_id', $5::uuid,
                'repository_workspace_id', $6::uuid,
                'pull_request_number', $7::bigint
            )
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.requesting_actor_id)
    .bind(job.project_id)
    .bind(pull_request_id)
    .bind(job.agent_run_id)
    .bind(job.repository_workspace_id)
    .bind(pull_request_number)
    .execute(&mut *transaction)
    .await?;
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

async fn fail_pull_request_job(
    state: &AppState,
    job: &ClaimedPullRequestJob,
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
        "UPDATE github_pull_request_requests SET state = $2, failure_category = $3, updated_at = now() WHERE id = $1",
    )
    .bind(job.request_id)
    .bind(next_state)
    .bind(category)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class,
            action, target_kind, target_id, outcome, metadata
        ) VALUES (
            $1, $2, $3, 'github_job', 'github.pull_request_create',
            'github_pull_request_request', $4, 'failed',
            jsonb_build_object('failure_category', $5::text)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.requesting_actor_id)
    .bind(job.project_id)
    .bind(job.request_id)
    .bind(category)
    .execute(&mut *transaction)
    .await?;
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

/// Claims a webhook's pull request the app doesn't track yet whose branch,
/// title or body names one of the project's open tasks (AWD-59). Others
/// stay unresolved, as before.
async fn claim_pull_request_adoption_job(
    state: &AppState,
) -> Result<Option<ClaimedPullRequestAdoptionJob>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let claim_id = Uuid::now_v7();
    let job = sqlx::query_as::<_, ClaimedPullRequestAdoptionJob>(
        r#"
        SELECT job.id AS job_id, job.project_id, $1::uuid AS claim_id,
               job.attempts + 1 AS attempts, job.max_attempts,
               job.webhook_delivery_id,
               installation.id AS github_installation_row_id,
               installation.integration_actor_id,
               installation.installation_id,
               repository.id AS repository_id,
               repository.github_repository_id,
               repository.owner, repository.name AS repository_name,
               (job.payload ->> 'number')::bigint AS pull_request_number,
               job.payload ->> 'node_id' AS node_id,
               CASE
                   WHEN COALESCE(
                       job.payload ->> 'merge_commit_sha',
                       delivery.redelivery_payload ->> 'merge_commit_sha'
                   ) ~ '^[0-9a-f]{40}$'
                   THEN COALESCE(
                       job.payload ->> 'merge_commit_sha',
                       delivery.redelivery_payload ->> 'merge_commit_sha'
                   )
               END AS known_merge_commit_sha,
               installation.enabled
                   AND installation.state = 'available'
                   AND access.access_state = 'selected'
                   AND access.github_repository_id = repository.github_repository_id
                   AND repository.enabled
                   AND repository.archived_at IS NULL
                   AS scope_available
        FROM github_jobs job
        JOIN github_installations installation
          ON installation.project_id = job.project_id
         AND installation.id = job.github_installation_id
        JOIN repositories repository
          ON repository.project_id = job.project_id
         AND repository.id = job.repository_id
        LEFT JOIN github_installation_repositories access
          ON access.project_id = job.project_id
         AND access.github_installation_id = installation.id
         AND access.repository_id = repository.id
        LEFT JOIN github_webhook_deliveries delivery
          ON delivery.project_id = job.project_id
         AND delivery.id = job.webhook_delivery_id
        WHERE job.kind = 'pull_request_sync'
          AND job.github_pull_request_id IS NULL
          AND job.state IN ('queued', 'failed')
          AND job.next_attempt_at <= now()
          AND job.attempts < job.max_attempts
          AND (job.payload ->> 'number') ~ '^[1-9][0-9]{0,17}$'
          AND char_length(COALESCE(job.payload ->> 'node_id', '')) BETWEEN 1 AND 256
          AND github_job_names_open_task(job.project_id, job.payload)
          AND NOT EXISTS (
              SELECT 1
              FROM github_pull_requests pull_request
              WHERE pull_request.repository_id = job.repository_id
                AND pull_request.pull_request_number = (job.payload ->> 'number')::bigint
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
    if let Some(delivery_id) = job.webhook_delivery_id {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = 'processing', failure_category = NULL, processed_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(Some(job))
}

/// Reads the pull request from GitHub, starts tracking it and suggests it on
/// the tasks it names, then hands the job back to the usual sync, which fills
/// in its checks and reviews.
async fn process_pull_request_adoption_job(
    state: &AppState,
    job: &ClaimedPullRequestAdoptionJob,
) -> Result<(), AppError> {
    if !job.scope_available {
        return fail_pull_request_adoption_job(state, job, "pull_request_scope_unavailable", true)
            .await;
    }
    let Some(config) = state.config.github.as_ref() else {
        return fail_pull_request_adoption_job(state, job, "github_not_configured", true).await;
    };
    let (Ok(installation_id), Ok(repository_id), Ok(pull_request_number)) = (
        u64::try_from(job.installation_id),
        u64::try_from(job.github_repository_id),
        u64::try_from(job.pull_request_number),
    ) else {
        return fail_pull_request_adoption_job(state, job, "pull_request_identity_invalid", true)
            .await;
    };
    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = match client
        .installation_token(
            installation_id,
            &[repository_id],
            InstallationTokenScope::ChecksRead,
        )
        .await
    {
        Ok(token) => token,
        Err(GitHubError::Scope) => {
            return fail_pull_request_adoption_job(state, job, "github_scope_incompatible", true)
                .await;
        }
        Err(_) => {
            return fail_pull_request_adoption_job(state, job, "github_token_unavailable", false)
                .await;
        }
    };
    let fetched = client
        .pull_request(
            &token,
            &job.owner,
            &job.repository_name,
            pull_request_number,
        )
        .await;
    drop(token);
    let Ok(mut pull_request) = fetched else {
        return fail_pull_request_adoption_job(state, job, "pull_request_fetch_failed", false)
            .await;
    };
    reconcile_merge_commit_identity(
        &mut pull_request.merge_commit_sha,
        job.known_merge_commit_sha.as_deref(),
        pull_request.merged_at.is_some(),
    )?;
    let expected_url = format!(
        "https://github.com/{}/{}/pull/{}",
        job.owner, job.repository_name, pull_request_number
    );
    let one_line = |value: &str, most: usize| {
        !value.is_empty() && value.chars().count() <= most && !value.chars().any(char::is_control)
    };
    let sha = |value: &str| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    };
    let merged = pull_request.merged_at.is_some();
    if pull_request.number != pull_request_number
        || pull_request.node_id != job.node_id
        || pull_request.html_url != expected_url
        || !sha(&pull_request.head.sha)
        || !one_line(&pull_request.head.name, 255)
        || !one_line(&pull_request.base.name, 255)
        || !one_line(&pull_request.title, 256)
        || !one_line(&pull_request.user.login, 100)
        || !matches!(pull_request.state.as_str(), "open" | "closed")
        || (merged && !pull_request.merge_commit_sha.as_deref().is_some_and(sha))
        || DateTime::parse_from_rfc3339(&pull_request.created_at).is_err()
        || DateTime::parse_from_rfc3339(&pull_request.updated_at).is_err()
    {
        return fail_pull_request_adoption_job(state, job, "pull_request_identity_invalid", true)
            .await;
    }
    let canonical_state = if merged {
        "merged"
    } else {
        pull_request.state.as_str()
    };

    let mut transaction = state.pool.begin().await?;
    // Back to the queue, where the usual sync finds the pull request now
    // tracked and reads its checks and reviews. The sync gets a fresh attempt
    // budget: pickup happens once per pull request, so this can't loop, and a
    // pickup that took its last attempt still leaves the sync one to run.
    let updated = sqlx::query(
        r#"
        UPDATE github_jobs
        SET state = 'queued', attempts = 0, lease_owner = NULL, lease_expires_at = NULL,
            failure_category = NULL, next_attempt_at = now(), updated_at = now()
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
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO github_pull_requests (
            id, project_id, repository_id, github_installation_id,
            github_repository_id, pull_request_number, node_id, html_url,
            head_ref, head_sha, base_ref, title, author_login, draft, state,
            merged_at, merge_commit_sha, external_created_at,
            external_updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
            $16::text::timestamptz, $17, $18::text::timestamptz,
            $19::text::timestamptz
        )
        ON CONFLICT (repository_id, pull_request_number) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.project_id)
    .bind(job.repository_id)
    .bind(job.github_installation_row_id)
    .bind(job.github_repository_id)
    .bind(job.pull_request_number)
    .bind(&pull_request.node_id)
    .bind(&pull_request.html_url)
    .bind(&pull_request.head.name)
    .bind(&pull_request.head.sha)
    .bind(&pull_request.base.name)
    .bind(&pull_request.title)
    .bind(&pull_request.user.login)
    .bind(pull_request.draft)
    .bind(canonical_state)
    .bind(pull_request.merged_at.as_deref())
    .bind(if merged {
        pull_request.merge_commit_sha.as_deref()
    } else {
        None
    })
    .bind(&pull_request.created_at)
    .bind(&pull_request.updated_at)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(pull_request_id) = inserted {
        let suggested = suggest_task_links(
            &mut transaction,
            job.project_id,
            pull_request_id,
            &pull_request,
            job.integration_actor_id,
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, project_id, authentication_class,
                action, target_kind, target_id, outcome, metadata
            ) VALUES (
                $1, $2, $3, 'github_job', 'github.pull_request_pick_up',
                'github_pull_request', $4, 'succeeded',
                jsonb_build_object(
                    'pull_request_number', $5::bigint,
                    'suggested_tasks', $6::integer
                )
            )
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(job.integration_actor_id)
        .bind(job.project_id)
        .bind(pull_request_id)
        .bind(job.pull_request_number)
        .bind(suggested)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn fail_pull_request_adoption_job(
    state: &AppState,
    job: &ClaimedPullRequestAdoptionJob,
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
    if let Some(delivery_id) = job.webhook_delivery_id {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = $2, failure_category = $3, processed_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .bind(next_state)
        .bind(category)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn process_pull_request_sync_job(
    state: &AppState,
    job: &ClaimedPullRequestSyncJob,
) -> Result<(), AppError> {
    if !job.scope_available {
        return fail_pull_request_sync_job(state, job, "pull_request_scope_unavailable", true)
            .await;
    }
    let Some(config) = state.config.github.as_ref() else {
        return fail_pull_request_sync_job(state, job, "github_not_configured", true).await;
    };
    let installation_id = match u64::try_from(job.installation_id) {
        Ok(value) => value,
        Err(_) => {
            return fail_pull_request_sync_job(state, job, "installation_identity_invalid", true)
                .await;
        }
    };
    let repository_id = match u64::try_from(job.github_repository_id) {
        Ok(value) => value,
        Err(_) => {
            return fail_pull_request_sync_job(state, job, "repository_identity_invalid", true)
                .await;
        }
    };
    let pull_request_number = match u64::try_from(job.pull_request_number) {
        Ok(value) => value,
        Err(_) => {
            return fail_pull_request_sync_job(state, job, "pull_request_identity_invalid", true)
                .await;
        }
    };
    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = match client
        .installation_token(
            installation_id,
            &[repository_id],
            InstallationTokenScope::ChecksRead,
        )
        .await
    {
        Ok(token) => token,
        Err(GitHubError::Scope) => {
            return fail_pull_request_sync_job(state, job, "github_scope_incompatible", true).await;
        }
        Err(_) => {
            return fail_pull_request_sync_job(state, job, "github_token_unavailable", false).await;
        }
    };
    let mut pull_request = match client
        .pull_request(
            &token,
            &job.owner,
            &job.repository_name,
            pull_request_number,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => {
            drop(token);
            return fail_pull_request_sync_job(state, job, "pull_request_fetch_failed", false)
                .await;
        }
    };
    if let Err(error) = reconcile_merge_commit_identity(
        &mut pull_request.merge_commit_sha,
        job.known_merge_commit_sha.as_deref(),
        pull_request.merged_at.is_some(),
    ) {
        drop(token);
        return Err(error);
    }
    if let Err(error) = validate_synced_pull_request(job, &pull_request) {
        drop(token);
        return Err(error);
    }
    let check_runs = match client
        .check_runs(
            &token,
            &job.owner,
            &job.repository_name,
            &pull_request.head.sha,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => {
            drop(token);
            return fail_pull_request_sync_job(state, job, "check_runs_fetch_failed", false).await;
        }
    };
    let reviews = match client
        .reviews(
            &token,
            &job.owner,
            &job.repository_name,
            pull_request_number,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => {
            drop(token);
            return fail_pull_request_sync_job(state, job, "reviews_fetch_failed", false).await;
        }
    };
    drop(token);
    let projection = validate_and_reduce_review_projection(&pull_request, &check_runs, &reviews)?;
    complete_pull_request_sync_job(
        state,
        job,
        &pull_request,
        &check_runs,
        &reviews,
        &projection,
    )
    .await
}

struct PullRequestReviewProjection {
    checks_state: &'static str,
    review_state: &'static str,
    mergeability_state: &'static str,
    requested_reviewers: Vec<String>,
}

fn validate_synced_pull_request(
    job: &ClaimedPullRequestSyncJob,
    pull_request: &GitHubPullRequest,
) -> Result<(), AppError> {
    let expected_url = format!(
        "https://github.com/{}/{}/pull/{}",
        job.owner, job.repository_name, pull_request.number
    );
    let created_at = DateTime::parse_from_rfc3339(&pull_request.created_at)
        .map_err(|_| AppError::Conflict("github_pull_request", "external timestamp invalid"))?
        .with_timezone(&Utc);
    DateTime::parse_from_rfc3339(&pull_request.updated_at)
        .map_err(|_| AppError::Conflict("github_pull_request", "external timestamp invalid"))?;
    if pull_request.number != u64::try_from(job.pull_request_number).unwrap_or_default()
        || pull_request.node_id != job.node_id
        || pull_request.html_url != job.html_url
        || pull_request.html_url != expected_url
        || pull_request.head.name != job.head_ref
        || pull_request.base.name != job.base_ref
        || created_at != job.external_created_at
        || pull_request.head.sha.len() != 40
        || !pull_request
            .head
            .sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || pull_request.title.is_empty()
        || pull_request.title.chars().count() > 256
        || pull_request.title.chars().any(char::is_control)
        || !valid_github_login(&pull_request.user.login)
        || !matches!(pull_request.state.as_str(), "open" | "closed")
        || pull_request
            .merge_commit_sha
            .as_deref()
            .is_some_and(|sha| sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        || (pull_request.merged_at.is_some() && pull_request.merge_commit_sha.is_none())
    {
        return Err(AppError::Conflict(
            "github_pull_request",
            "provider identity mismatch",
        ));
    }
    if let Some(merged_at) = pull_request.merged_at.as_deref() {
        DateTime::parse_from_rfc3339(merged_at)
            .map_err(|_| AppError::Conflict("github_pull_request", "external timestamp invalid"))?;
    }
    Ok(())
}

fn reconcile_merge_commit_identity(
    provider_sha: &mut Option<String>,
    webhook_sha: Option<&str>,
    merged: bool,
) -> Result<(), AppError> {
    if let (Some(provider_sha), Some(webhook_sha)) = (provider_sha.as_deref(), webhook_sha)
        && provider_sha != webhook_sha
    {
        return Err(AppError::Conflict(
            "github_pull_request",
            "merge commit identity mismatch",
        ));
    }
    if merged && provider_sha.is_none() {
        *provider_sha = webhook_sha.map(str::to_owned);
    }
    Ok(())
}

fn validate_and_reduce_review_projection(
    pull_request: &GitHubPullRequest,
    check_runs: &[GitHubCheckRun],
    reviews: &[GitHubReview],
) -> Result<PullRequestReviewProjection, AppError> {
    let mut has_pending_check = false;
    let mut has_failed_check = false;
    for check in check_runs {
        if check.id == 0
            || check.name.is_empty()
            || check.name.chars().count() > 256
            || check.name.chars().any(char::is_control)
            || !matches!(
                check.status.as_str(),
                "queued" | "in_progress" | "completed"
            )
            || !valid_external_url(check.details_url.as_deref())
            || !valid_optional_timestamp(check.started_at.as_deref())
            || !valid_optional_timestamp(check.completed_at.as_deref())
        {
            return Err(AppError::Conflict(
                "github_check_run",
                "provider field invalid",
            ));
        }
        match (check.status.as_str(), check.conclusion.as_deref()) {
            ("queued" | "in_progress", None) => has_pending_check = true,
            ("completed", Some("success" | "neutral" | "skipped")) => {}
            (
                "completed",
                Some(
                    "action_required" | "cancelled" | "failure" | "stale" | "startup_failure"
                    | "timed_out",
                ),
            ) => has_failed_check = true,
            _ => {
                return Err(AppError::Conflict(
                    "github_check_run",
                    "provider state invalid",
                ));
            }
        }
    }
    let checks_state = if check_runs.is_empty() {
        "unknown"
    } else if has_failed_check {
        "failure"
    } else if has_pending_check {
        "pending"
    } else {
        "success"
    };

    let mut latest_reviews = BTreeMap::<String, (String, u64, String)>::new();
    for review in reviews {
        let state = review.state.to_ascii_lowercase();
        if review.id == 0
            || !valid_github_login(&review.user.login)
            || !matches!(
                state.as_str(),
                "approved" | "changes_requested" | "commented" | "dismissed"
            )
            || !valid_optional_timestamp(review.submitted_at.as_deref())
        {
            return Err(AppError::Conflict(
                "github_review",
                "provider field invalid",
            ));
        }
        let order = review.submitted_at.clone().unwrap_or_default();
        let entry = latest_reviews
            .entry(review.user.login.clone())
            .or_insert_with(|| (order.clone(), review.id, state.clone()));
        if (order.as_str(), review.id) > (entry.0.as_str(), entry.1) {
            *entry = (order, review.id, state);
        }
    }
    let mut requested_reviewers = pull_request
        .requested_reviewers
        .iter()
        .map(|reviewer| reviewer.login.clone())
        .collect::<Vec<_>>();
    requested_reviewers.sort();
    requested_reviewers.dedup();
    if requested_reviewers.len() > 50
        || requested_reviewers
            .iter()
            .any(|login| !valid_github_login(login))
    {
        return Err(AppError::Conflict(
            "github_pull_request",
            "requested reviewer invalid",
        ));
    }
    let review_state = if latest_reviews
        .values()
        .any(|(_, _, state)| state == "changes_requested")
    {
        "changes_requested"
    } else if latest_reviews
        .values()
        .any(|(_, _, state)| state == "approved")
    {
        "approved"
    } else if !requested_reviewers.is_empty() || !reviews.is_empty() {
        "pending"
    } else {
        "unknown"
    };
    let canonical_state = if pull_request.merged_at.is_some() {
        "merged"
    } else {
        pull_request.state.as_str()
    };
    let mergeability_state = if canonical_state != "open"
        || pull_request.draft
        || checks_state == "failure"
        || review_state == "changes_requested"
    {
        "blocked"
    } else {
        match pull_request.mergeable {
            Some(true) => "ready",
            Some(false) => "blocked",
            None => "unknown",
        }
    };
    Ok(PullRequestReviewProjection {
        checks_state,
        review_state,
        mergeability_state,
        requested_reviewers,
    })
}

fn valid_github_login(value: &str) -> bool {
    !value.is_empty() && value.len() <= 100 && !value.chars().any(char::is_control)
}

fn valid_optional_timestamp(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.len() <= 64
            && !value.chars().any(char::is_control)
            && DateTime::parse_from_rfc3339(value).is_ok()
    })
}

fn valid_external_url(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.len() <= 2048
            && !value.chars().any(char::is_control)
            && reqwest::Url::parse(value).is_ok_and(|url| url.scheme() == "https")
    })
}

async fn complete_pull_request_sync_job(
    state: &AppState,
    job: &ClaimedPullRequestSyncJob,
    pull_request: &GitHubPullRequest,
    check_runs: &[GitHubCheckRun],
    reviews: &[GitHubReview],
    projection: &PullRequestReviewProjection,
) -> Result<(), AppError> {
    let canonical_state = if pull_request.merged_at.is_some() {
        "merged"
    } else {
        pull_request.state.as_str()
    };
    let requested_reviewers = serde_json::to_value(&projection.requested_reviewers)
        .map_err(|_| AppError::Internal("serialize requested reviewers"))?;
    let mut transaction = state.pool.begin().await?;
    // The automation below changes task statuses, so it takes its turn on the
    // project row before anything else, as every task write does
    // (work_item_commands::lock_membership) before it locks tasks and the
    // board column a status change lands in. Locked the other way round,
    // such a write would wait for a task this holds while this waits for the
    // project row (its audit rows' foreign key share-locks it). NO KEY
    // UPDATE waits for those writers but never for a foreign key's share
    // lock, so other GitHub jobs that only reference the project don't wait.
    sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR NO KEY UPDATE")
        .bind(job.project_id)
        .fetch_optional(&mut *transaction)
        .await?;
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
    let pull_request_version = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE github_pull_requests
        SET head_sha = $3, title = $4, author_login = $5, draft = $6,
            state = $7, merged_at = $8::text::timestamptz,
            merge_commit_sha = $9, external_updated_at = $10::text::timestamptz,
            checks_state = $11, review_state = $12,
            mergeability_state = $13, requested_reviewers = $14,
            sync_state = 'fresh', last_sync_attempt_at = now(),
            last_success_at = now(), sync_failure_category = NULL,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        RETURNING version
        "#,
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .bind(&pull_request.head.sha)
    .bind(&pull_request.title)
    .bind(&pull_request.user.login)
    .bind(pull_request.draft)
    .bind(canonical_state)
    .bind(pull_request.merged_at.as_deref())
    .bind(if canonical_state == "merged" {
        pull_request.merge_commit_sha.as_deref()
    } else {
        None
    })
    .bind(&pull_request.updated_at)
    .bind(projection.checks_state)
    .bind(projection.review_state)
    .bind(projection.mergeability_state)
    .bind(requested_reviewers)
    .fetch_one(&mut *transaction)
    .await?;
    suggest_task_links(
        &mut transaction,
        job.project_id,
        job.pull_request_id,
        pull_request,
        job.integration_actor_id,
    )
    .await?;

    sqlx::query("DELETE FROM github_pull_request_check_runs WHERE github_pull_request_id = $1")
        .bind(job.pull_request_id)
        .execute(&mut *transaction)
        .await?;
    for check in check_runs {
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_check_runs (
                id, project_id, github_pull_request_id, github_check_run_id,
                name, status, conclusion, details_url,
                external_started_at, external_completed_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9::text::timestamptz, $10::text::timestamptz
            )
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(job.project_id)
        .bind(job.pull_request_id)
        .bind(i64::try_from(check.id).map_err(|_| AppError::Internal("check run id"))?)
        .bind(&check.name)
        .bind(&check.status)
        .bind(check.conclusion.as_deref())
        .bind(check.details_url.as_deref())
        .bind(check.started_at.as_deref())
        .bind(check.completed_at.as_deref())
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("DELETE FROM github_pull_request_reviews WHERE github_pull_request_id = $1")
        .bind(job.pull_request_id)
        .execute(&mut *transaction)
        .await?;
    for review in reviews {
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_reviews (
                id, project_id, github_pull_request_id, github_review_id,
                author_login, state, external_submitted_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7::text::timestamptz)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(job.project_id)
        .bind(job.pull_request_id)
        .bind(i64::try_from(review.id).map_err(|_| AppError::Internal("review id"))?)
        .bind(&review.user.login)
        .bind(review.state.to_ascii_lowercase())
        .bind(review.submitted_at.as_deref())
        .execute(&mut *transaction)
        .await?;
    }
    apply_pull_request_work_item_automation(
        &mut transaction,
        &AutomationJob::from(job),
        pull_request_version,
        canonical_state,
        pull_request.draft,
    )
    .await?;
    if let Some(delivery_id) = job.webhook_delivery_id {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = 'processed', failure_category = NULL, processed_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class,
            action, target_kind, target_id, outcome, metadata
        ) VALUES (
            $1, $2, $3, 'github_job', 'github.pull_request_sync',
            'github_pull_request', $4, 'succeeded',
            jsonb_build_object(
                'checks_state', $5::text,
                'review_state', $6::text,
                'mergeability_state', $7::text,
                'external_updated_at', $8::text
            )
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.integration_actor_id)
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .bind(projection.checks_state)
    .bind(projection.review_state)
    .bind(projection.mergeability_state)
    .bind(&pull_request.updated_at)
    .execute(&mut *transaction)
    .await?;
    enqueue_pull_request_invalidations(&mut transaction, job).await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(Debug, FromRow)]
struct AutomationWorkItemRow {
    id: Uuid,
    version: i64,
    status_id: Uuid,
    semantic_category: String,
    archived: bool,
}

/// What the pull request automation needs from its sync job.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct AutomationJob {
    pub job_id: Uuid,
    pub project_id: Uuid,
    pub pull_request_id: Uuid,
    pub integration_actor_id: Uuid,
}

impl From<&ClaimedPullRequestSyncJob> for AutomationJob {
    fn from(job: &ClaimedPullRequestSyncJob) -> Self {
        Self {
            job_id: job.job_id,
            project_id: job.project_id,
            pull_request_id: job.pull_request_id,
            integration_actor_id: job.integration_actor_id,
        }
    }
}

/// What a pull request sync does to its linked tasks once GitHub has
/// answered, in its own transaction with the project row locked first as
/// the sync does. For tests, which can't reach GitHub.
#[doc(hidden)]
pub async fn apply_pull_request_automation(
    pool: &sqlx::PgPool,
    job: AutomationJob,
    pull_request_version: i64,
    pull_request_state: &str,
    draft: bool,
) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR NO KEY UPDATE")
        .bind(job.project_id)
        .fetch_optional(&mut *transaction)
        .await?;
    apply_pull_request_work_item_automation(
        &mut transaction,
        &job,
        pull_request_version,
        pull_request_state,
        draft,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Moves the pull request's explicitly linked tasks on. The caller holds the
/// project row lock (see `complete_pull_request_sync_job`).
async fn apply_pull_request_work_item_automation(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AutomationJob,
    pull_request_version: i64,
    pull_request_state: &str,
    draft: bool,
) -> Result<(), AppError> {
    let settings = sqlx::query_as::<_, (bool, bool)>(
        r#"
        SELECT pull_request_open_enabled, pull_request_merge_enabled
        FROM project_github_automation_settings
        WHERE project_id = $1
        FOR SHARE
        "#,
    )
    .bind(job.project_id)
    .fetch_one(&mut **transaction)
    .await?;
    let (rule, enabled, target_category) = match (pull_request_state, draft) {
        ("merged", _) => ("pull_request_merge", settings.1, "verification"),
        ("open", false) => ("pull_request_open", settings.0, "review"),
        ("open", true) => {
            set_pull_request_automation_projection(
                transaction,
                job,
                "skipped",
                Some("pull_request_draft"),
            )
            .await?;
            return Ok(());
        }
        _ => {
            set_pull_request_automation_projection(
                transaction,
                job,
                "skipped",
                Some("pull_request_closed"),
            )
            .await?;
            return Ok(());
        }
    };
    let work_items = sqlx::query_as::<_, AutomationWorkItemRow>(
        r#"
        SELECT item.id, item.version, item.status_id,
               status.semantic_category,
               (item.archived_at IS NOT NULL) AS archived
        FROM github_pull_request_work_item_links link
        JOIN work_items item
          ON item.project_id = link.project_id AND item.id = link.work_item_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE link.project_id = $1
          AND link.github_pull_request_id = $2
          AND link.source = 'explicit'
          AND link.confirmed
        ORDER BY item.id
        FOR UPDATE OF item, status
        "#,
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .fetch_all(&mut **transaction)
    .await?;
    if work_items.is_empty() {
        set_pull_request_automation_projection(
            transaction,
            job,
            "skipped",
            Some("no_explicit_work_items"),
        )
        .await?;
        return Ok(());
    }

    let target_statuses = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, name
        FROM work_item_statuses
        WHERE project_id = $1 AND semantic_category = $2 AND enabled
        ORDER BY id
        "#,
    )
    .bind(job.project_id)
    .bind(target_category)
    .fetch_all(&mut **transaction)
    .await?;
    let target_status = (target_statuses.len() == 1).then(|| target_statuses[0].clone());
    let mut applied = 0usize;
    let mut reasons = BTreeSet::new();
    for item in work_items {
        if !enabled {
            insert_automation_skip(
                transaction,
                job,
                pull_request_version,
                &item,
                rule,
                "rule_disabled",
            )
            .await?;
            reasons.insert("rule_disabled");
            continue;
        }
        if item.archived || matches!(item.semantic_category.as_str(), "completed" | "canceled") {
            insert_automation_skip(
                transaction,
                job,
                pull_request_version,
                &item,
                rule,
                "terminal_or_archived",
            )
            .await?;
            reasons.insert("terminal_or_archived");
            continue;
        }
        let Some((target_status_id, _)) = target_status.as_ref() else {
            insert_automation_skip(
                transaction,
                job,
                pull_request_version,
                &item,
                rule,
                "ambiguous_target_status",
            )
            .await?;
            reasons.insert("ambiguous_target_status");
            continue;
        };
        if item.status_id == *target_status_id {
            insert_automation_skip(
                transaction,
                job,
                pull_request_version,
                &item,
                rule,
                "already_in_target_status",
            )
            .await?;
            reasons.insert("already_in_target_status");
            continue;
        }
        let allowed = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM work_item_status_transitions
                WHERE project_id = $1 AND from_status_id = $2 AND to_status_id = $3
            )
            "#,
        )
        .bind(job.project_id)
        .bind(item.status_id)
        .bind(target_status_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !allowed {
            insert_automation_skip(
                transaction,
                job,
                pull_request_version,
                &item,
                rule,
                "transition_not_allowed",
            )
            .await?;
            reasons.insert("transition_not_allowed");
            continue;
        }
        // The move goes through the task command, as a person's would: one
        // version bump, an activity with what changed, the assigned agent's
        // event, watching agents' notes, one audit row and one outbox event.
        let actor = CommandActor {
            actor_id: job.integration_actor_id,
            actor_kind: crate::policy::ActorKind::Integration,
            project_id: job.project_id,
            audit: AuditAttribution::Job {
                job_id: job.job_id,
                source: "github_automation",
            },
        };
        let current = work_item_commands::lock_work_item(transaction, job.project_id, item.id)
            .await?
            .ok_or(AppError::Conflict("work_item", "version"))?;
        let outcome = work_item_commands::update_locked_work_item(
            transaction,
            &actor,
            current,
            work_item_commands::UpdateWorkItemCommand {
                expected_version: item.version,
                title: None,
                description: None,
                type_id: None,
                status_id: Some(*target_status_id),
                priority: None,
                assignee_actor_id: None,
                parent_work_item_id: None,
                board_placement: Some(work_item_commands::BoardPlacement::Bottom),
                labels: None,
                fields: work_item_commands::TaskFieldChanges::default(),
                field_values: None,
                summary: "GitHub pull request automation changed status",
                audit_action: "github.work_item_automation_transition",
                audit_version_metadata: work_item_commands::AuditVersionMetadata::Include,
            },
        )
        .await?;
        let work_item_commands::UpdateWorkItemOutcome::Applied { finalized, .. } = outcome else {
            return Err(AppError::Conflict("work_item", "version"));
        };
        sqlx::query(
            r#"
            INSERT INTO github_pull_request_automation_results (
                id, project_id, github_pull_request_id, pull_request_version,
                work_item_id, rule, outcome, previous_status_id, new_status_id,
                integration_actor_id, work_item_activity_id
            ) VALUES ($1, $2, $3, $4, $5, $6, 'applied', $7, $8, $9, $10)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(job.project_id)
        .bind(job.pull_request_id)
        .bind(pull_request_version)
        .bind(item.id)
        .bind(rule)
        .bind(item.status_id)
        .bind(target_status_id)
        .bind(job.integration_actor_id)
        .bind(finalized.activity_id)
        .execute(&mut **transaction)
        .await?;
        applied += 1;
    }
    if applied > 0 {
        set_pull_request_automation_projection(transaction, job, "applied", None).await?;
    } else {
        let reason = if reasons.len() == 1 {
            reasons.first().copied().unwrap_or("no_eligible_transition")
        } else {
            "no_eligible_transition"
        };
        set_pull_request_automation_projection(transaction, job, "skipped", Some(reason)).await?;
    }
    Ok(())
}

async fn insert_automation_skip(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AutomationJob,
    pull_request_version: i64,
    item: &AutomationWorkItemRow,
    rule: &str,
    reason: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO github_pull_request_automation_results (
            id, project_id, github_pull_request_id, pull_request_version,
            work_item_id, rule, outcome, reason, previous_status_id,
            integration_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, 'skipped', $7, $8, $9)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .bind(pull_request_version)
    .bind(item.id)
    .bind(rule)
    .bind(reason)
    .bind(item.status_id)
    .bind(job.integration_actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn set_pull_request_automation_projection(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AutomationJob,
    state: &str,
    reason: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE github_pull_requests
        SET automation_state = $3, automation_reason = $4,
            automation_evaluated_at = now(), updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .bind(state)
    .bind(reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Suggests the pull request on each of the project's open tasks its branch,
/// title or body names (AWD-59), unconfirmed, so it never moves a task until
/// a person confirms it. Links already there, and ones dismissed, stay as
/// they are.
async fn suggest_task_links(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    pull_request_id: Uuid,
    pull_request: &GitHubPullRequest,
    actor_id: Uuid,
) -> Result<i32, AppError> {
    let branch_keys = task_keys_in(&pull_request.head.name);
    let mut body_keys = task_keys_in(&pull_request.title);
    for key in task_keys_in(pull_request.body.as_deref().unwrap_or("")) {
        if body_keys.len() < MAX_TASK_KEYS && !body_keys.contains(&key) {
            body_keys.push(key);
        }
    }
    if branch_keys.is_empty() && body_keys.is_empty() {
        return Ok(0);
    }
    Ok(
        sqlx::query_scalar::<_, i32>("SELECT github_suggest_task_links($1, $2, $3, $4, $5)")
            .bind(project_id)
            .bind(pull_request_id)
            .bind(&branch_keys)
            .bind(&body_keys)
            .bind(actor_id)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

async fn enqueue_pull_request_invalidations(
    transaction: &mut Transaction<'_, Postgres>,
    job: &ClaimedPullRequestSyncJob,
) -> Result<(), AppError> {
    let runs = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT DISTINCT run.id, run.conversation_id
        FROM github_pull_request_workspace_links link
        JOIN agent_runs run
          ON run.project_id = link.project_id AND run.id = link.agent_run_id
        WHERE link.project_id = $1 AND link.github_pull_request_id = $2
        "#,
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (run_id, conversation_id) in runs {
        outbox::enqueue_agent_run_updated(transaction, job.project_id, conversation_id, run_id)
            .await?;
    }
    let work_items = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT work_item.id, work_item.version
        FROM github_pull_request_work_item_links link
        JOIN work_items work_item
          ON work_item.project_id = link.project_id AND work_item.id = link.work_item_id
        WHERE link.project_id = $1 AND link.github_pull_request_id = $2
        "#,
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (work_item_id, version) in work_items {
        outbox::enqueue_work_item_updated(transaction, job.project_id, work_item_id, version)
            .await?;
    }
    Ok(())
}

async fn fail_pull_request_sync_job(
    state: &AppState,
    job: &ClaimedPullRequestSyncJob,
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
        "UPDATE github_pull_requests SET sync_state = 'failed', sync_failure_category = $3, last_sync_attempt_at = now(), updated_at = now() WHERE project_id = $1 AND id = $2",
    )
    .bind(job.project_id)
    .bind(job.pull_request_id)
    .bind(category)
    .execute(&mut *transaction)
    .await?;
    if let Some(delivery_id) = job.webhook_delivery_id {
        sqlx::query(
            "UPDATE github_webhook_deliveries SET state = $2, failure_category = $3, processed_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(delivery_id)
        .bind(next_state)
        .bind(category)
        .execute(&mut *transaction)
        .await?;
    }
    enqueue_pull_request_invalidations(&mut transaction, job).await?;
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
            || (name == "actions" && level == "read")
            // Optional: lets imports read a repository's issues (W11).
            || (name == "issues" && level == "read");
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

/// The project's GitHub installation and the repositories it reaches, as
/// the project's settings show it. Null when none is linked.
pub(crate) async fn installation_value(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Value, AppError> {
    let view = match load_installation(transaction, project_id).await? {
        Some(installation) => {
            let repositories =
                load_installation_repositories(transaction, project_id, installation.id).await?;
            Some(installation_view(installation, repositories))
        }
        None => None,
    };
    serde_json::to_value(view).map_err(|_| AppError::Internal("GitHub installation view"))
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
    let current_secret = read_webhook_secret(&config.webhook_secret_path)?;
    let previous_secret = config
        .webhook_previous_secret_path
        .as_ref()
        .map(|path| read_webhook_secret(path))
        .transpose()?;
    verify_signature_with_overlap(
        &current_secret,
        previous_secret.as_ref().map(|secret| secret.as_slice()),
        body,
        &signature,
    )?;
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
            "SELECT payload_sha256 FROM github_webhook_deliveries WHERE github_installation_id = $1 AND delivery_id = $2",
        )
        .bind(registration.0)
        .bind(&parsed.delivery_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| WebhookError::Database)?;
        if existing.ct_eq(&checksum).unwrap_u8() != 1 {
            return Err(WebhookError::Conflict);
        }
        let requeued = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE github_jobs job
            SET state = 'queued', attempts = 0,
                lease_owner = NULL, lease_expires_at = NULL,
                failure_category = NULL, next_attempt_at = now(),
                completed_at = NULL, updated_at = now()
            FROM github_webhook_deliveries delivery
            WHERE delivery.github_installation_id = $1
              AND delivery.delivery_id = $2
              AND job.webhook_delivery_id = delivery.id
              AND job.state IN ('failed', 'exhausted')
            RETURNING delivery.id
            "#,
        )
        .bind(registration.0)
        .bind(&parsed.delivery_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| WebhookError::Database)?;
        if let Some(delivery_id) = requeued {
            sqlx::query(
                "UPDATE github_webhook_deliveries SET state = 'received', failure_category = NULL, processed_at = NULL, redelivery_payload = $2, updated_at = now() WHERE id = $1",
            )
            .bind(delivery_id)
            .bind(&parsed.payload)
            .execute(&mut *transaction)
            .await
            .map_err(|_| WebhookError::Database)?;
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

fn read_webhook_secret(path: &Path) -> Result<Zeroizing<Vec<u8>>, WebhookError> {
    let mut secret = read_secret_file(path, 4096).map_err(|_| WebhookError::Unavailable)?;
    while secret.last().is_some_and(|byte| byte.is_ascii_whitespace()) {
        secret.pop();
    }
    if secret.len() < 32 {
        return Err(WebhookError::Unavailable);
    }
    Ok(secret)
}

fn verify_signature_with_overlap(
    current_secret: &[u8],
    previous_secret: Option<&[u8]>,
    body: &[u8],
    signature: &str,
) -> Result<(), WebhookError> {
    let current_matches = verify_signature(current_secret, body, signature).is_ok();
    let previous_matches = previous_secret
        .map(|secret| verify_signature(secret, body, signature).is_ok())
        .unwrap_or(false);
    if current_matches || previous_matches {
        Ok(())
    } else {
        Err(WebhookError::Signature)
    }
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
        "check_suite" => [
            ("check_suite_id", "/check_suite/id"),
            ("head_sha", "/check_suite/head_sha"),
            ("number", "/check_suite/pull_requests/0/number"),
        ]
        .as_slice(),
        "check_run" => [
            ("check_run_id", "/check_run/id"),
            ("head_sha", "/check_run/head_sha"),
            ("number", "/check_run/pull_requests/0/number"),
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
    if event == "pull_request"
        && action == Some("closed")
        && value
            .pointer("/pull_request/merged")
            .and_then(Value::as_bool)
            == Some(true)
        && let Some(merge_commit_sha) = value
            .pointer("/pull_request/merge_commit_sha")
            .and_then(Value::as_str)
            .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        payload.insert(
            "merge_commit_sha".to_owned(),
            Value::String(merge_commit_sha.to_owned()),
        );
    }
    if event == "pull_request" {
        // Keys the pull request names (AWD-59), so one the app didn't open
        // can still be picked up for a task it names. Which project's keys
        // count is checked when the job runs.
        let text = |pointer: &str| value.pointer(pointer).and_then(Value::as_str).unwrap_or("");
        let mut keys = task_keys_in(text("/pull_request/head/ref"));
        for key in task_keys_in(text("/pull_request/title"))
            .into_iter()
            .chain(task_keys_in(text("/pull_request/body")))
        {
            if keys.len() < MAX_TASK_KEYS && !keys.contains(&key) {
                keys.push(key);
            }
        }
        if !keys.is_empty() {
            payload.insert(
                "task_keys".to_owned(),
                Value::Array(keys.into_iter().map(Value::String).collect()),
            );
        }
    }
    Ok(Value::Object(payload))
}

/// The most task keys one pull request's branch, or its title and body, can
/// suggest.
const MAX_TASK_KEYS: usize = 20;

/// Task keys named in some text, such as `AWD-59` in a branch
/// `claude/awd-59-pr-card` or a title "Fix the board (AWD-7)": a letter, one
/// to nine more letters or digits, a hyphen and a number, standing on its
/// own. Upper-cased, in order, each once, at most twenty.
fn task_keys_in(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut index = 0;
    while index < bytes.len() && keys.len() < MAX_TASK_KEYS {
        let starts_word = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        if !(starts_word && bytes[index].is_ascii_alphabetic()) {
            index += 1;
            continue;
        }
        let prefix_end = index
            + bytes[index..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric())
                .count();
        let prefix_length = prefix_end - index;
        if (2..=10).contains(&prefix_length) && bytes.get(prefix_end) == Some(&b'-') {
            let digits_start = prefix_end + 1;
            let digits = bytes[digits_start..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            let after = digits_start + digits;
            if (1..=9).contains(&digits)
                && bytes[digits_start] != b'0'
                && !bytes.get(after).is_some_and(u8::is_ascii_alphanumeric)
            {
                let key = text[index..after].to_ascii_uppercase();
                if !keys.contains(&key) {
                    keys.push(key);
                }
                index = after;
                continue;
            }
        }
        index = prefix_end.max(index + 1);
    }
    keys
}

/// What GitHub said to a merge.
enum MergeCall {
    Merged(String),
    /// Checks, reviews, conflicts or the repository's merge settings stop it.
    NotMergeable,
    /// Its head isn't the commit the caller saw.
    HeadMoved,
}

#[derive(Deserialize)]
struct GitHubMergeResponse {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    merged: bool,
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// What the caller asks a merge for. Both are optional: the head defaults to
/// the one Agent Workspace last read, the method to a squash.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MergePullRequestRequest {
    /// Merge only if the pull request's head is still this commit.
    #[serde(default)]
    pub expected_head_sha: Option<String>,
    /// `squash` (the default), `merge` or `rebase`.
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestMergeView {
    pub pull_request_id: Uuid,
    pub number: i64,
    pub method: String,
    pub merge_commit_sha: String,
    /// true when it was merged already, by this call's earlier try or
    /// someone on GitHub.
    pub already_merged: bool,
    /// The sync that records the merge; it moves the pull request's tasks.
    pub refresh: PullRequestRefreshView,
}

#[derive(FromRow)]
struct MergeTarget {
    pull_request_number: i64,
    head_sha: String,
    state: String,
    draft: bool,
    installation_id: i64,
    github_repository_id: i64,
    owner: String,
    repository_name: String,
    scope_available: bool,
}

/// Merges an open pull request through the project's GitHub App, for a
/// caller who manages the project's code and can see the pull request
/// (AWD-59). Nothing is held open while GitHub answers. A merge writes an
/// audit row and queues a sync, which records it and moves linked tasks.
/// Asking again after it merged answers with that merge.
pub(crate) async fn merge_pull_request_as(
    state: &AppState,
    actor: &CommandActor,
    pull_request_id: Uuid,
    request: &MergePullRequestRequest,
) -> Result<PullRequestMergeView, AppError> {
    let method = request.method.as_deref().unwrap_or("squash");
    if !matches!(method, "squash" | "merge" | "rebase") {
        return Err(AppError::Invalid("merge method"));
    }
    if request
        .expected_head_sha
        .as_deref()
        .is_some_and(|value| !is_commit_sha(value))
    {
        return Err(AppError::Invalid("expected head sha"));
    }
    let project_id = actor.project_id;
    let mut transaction = state.pool.begin().await?;
    require_project_access_as(
        &mut transaction,
        actor.actor_id,
        actor.actor_kind,
        project_id,
        Action::PullRequestMerge,
    )
    .await?;
    let target = sqlx::query_as::<_, MergeTarget>(
        r#"
        SELECT pull_request.pull_request_number, pull_request.head_sha,
               pull_request.state, pull_request.draft,
               installation.installation_id,
               repository.github_repository_id,
               repository.owner, repository.name AS repository_name,
               COALESCE(
                   installation.enabled
                   AND installation.state = 'available'
                   AND access.access_state = 'selected'
                   AND access.github_repository_id = repository.github_repository_id
                   AND repository.enabled
                   AND repository.archived_at IS NULL,
                   false
               ) AS scope_available
        FROM github_pull_requests pull_request
        JOIN github_installations installation
          ON installation.project_id = pull_request.project_id
         AND installation.id = pull_request.github_installation_id
        JOIN repositories repository
          ON repository.project_id = pull_request.project_id
         AND repository.id = pull_request.repository_id
        LEFT JOIN github_installation_repositories access
          ON access.project_id = pull_request.project_id
         AND access.github_installation_id = installation.id
         AND access.repository_id = repository.id
        WHERE pull_request.project_id = $1 AND pull_request.id = $2
          AND pull_request_visible_to(pull_request.id, $3)
        "#,
    )
    .bind(project_id)
    .bind(pull_request_id)
    .bind(actor.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("github_pull_request"))?;
    transaction.commit().await?;

    if target.state != "open" {
        return Err(AppError::Conflict("github_pull_request", "not_open"));
    }
    if target.draft {
        return Err(AppError::Conflict("github_pull_request", "draft"));
    }
    if !target.scope_available {
        return Err(AppError::Conflict(
            "github_installation",
            "repository_unavailable",
        ));
    }
    let head_sha = request
        .expected_head_sha
        .clone()
        .unwrap_or_else(|| target.head_sha.clone());
    if head_sha != target.head_sha {
        return Err(AppError::Conflict("github_pull_request", "head_moved"));
    }
    let Some(config) = state.config.github.as_ref() else {
        return Err(AppError::Conflict("github_integration", "not_configured"));
    };
    let (Ok(installation_id), Ok(github_repository_id), Ok(number)) = (
        u64::try_from(target.installation_id),
        u64::try_from(target.github_repository_id),
        u64::try_from(target.pull_request_number),
    ) else {
        return Err(AppError::Internal("GitHub identity invalid"));
    };
    let client = GitHubClient::new(config).map_err(map_github_error)?;
    let token = client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::PullRequestsMerge,
        )
        .await
        .map_err(map_github_error)?;
    let called = client
        .merge_pull_request(
            &token,
            &target.owner,
            &target.repository_name,
            number,
            &head_sha,
            method,
        )
        .await;
    let (merge_commit_sha, already_merged) = match called {
        Ok(MergeCall::Merged(sha)) => (sha, false),
        Ok(MergeCall::HeadMoved) => {
            return Err(AppError::Conflict("github_pull_request", "head_moved"));
        }
        // GitHub refuses a pull request that is merged already; one merged
        // by an earlier try of this call counts as done.
        Ok(MergeCall::NotMergeable) | Err(_) => {
            let current = client
                .pull_request(&token, &target.owner, &target.repository_name, number)
                .await;
            match current {
                Ok(current)
                    if current.merged_at.is_some()
                        && current.head.sha == head_sha
                        && current
                            .merge_commit_sha
                            .as_deref()
                            .is_some_and(is_commit_sha) =>
                {
                    (current.merge_commit_sha.unwrap_or_default(), true)
                }
                _ if matches!(called, Ok(MergeCall::NotMergeable)) => {
                    return Err(AppError::Conflict("github_pull_request", "not_mergeable"));
                }
                _ => return Err(AppError::Internal("GitHub integration unavailable")),
            }
        }
    };
    drop(token);

    let mut transaction = state.pool.begin().await?;
    record_audit_for(
        &mut transaction,
        actor,
        "github_pull_request",
        "github.pull_request_merged",
        pull_request_id,
        json!({
            "number": target.pull_request_number,
            "method": method,
            "head_sha": head_sha,
            "merge_commit_sha": merge_commit_sha,
            "already_merged": already_merged,
        }),
    )
    .await?;
    let Queued { view: refresh, new } = queue_refresh_in(
        &mut transaction,
        actor.actor_id,
        project_id,
        pull_request_id,
    )
    .await?;
    transaction.commit().await?;
    if new {
        kick_jobs(state.clone());
    }
    Ok(PullRequestMergeView {
        pull_request_id,
        number: target.pull_request_number,
        method: method.to_owned(),
        merge_commit_sha,
        already_merged,
        refresh,
    })
}

async fn merge_pull_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, pull_request_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<MergePullRequestRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let actor = CommandActor {
        actor_id: principal.actor_id,
        actor_kind: actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id: headers
                .get(crate::REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= 128)
                .map(str::to_owned),
        },
    };
    let view = merge_pull_request_as(&state, &actor, pull_request_id, &request)
        .await
        .map_err(|error| match error {
            AppError::Conflict(what, why) => match merge_refusal(what, why) {
                Some(message) => AppError::Refused {
                    field: "pullRequest",
                    message,
                },
                None => AppError::Conflict(what, why),
            },
            error => error,
        })?;
    Ok((StatusCode::OK, Json(view)).into_response())
}

/// Why a merge didn't go ahead, for the person or agent who asked.
pub(crate) fn merge_refusal(what: &str, why: &str) -> Option<&'static str> {
    Some(match (what, why) {
        ("github_integration", "not_configured") => "GitHub isn't set up on this server yet.",
        ("github_pull_request", "not_open") => "That pull request isn't open.",
        ("github_pull_request", "draft") => {
            "That pull request is a draft. Mark it ready on GitHub first."
        }
        ("github_pull_request", "head_moved") => {
            "Its head moved since you looked. Check the new commit, then merge again."
        }
        ("github_pull_request", "not_mergeable") => {
            "GitHub won't merge it yet: checks, reviews or conflicts block it, or the repository doesn't allow that merge method."
        }
        ("github_installation", "repository_unavailable") => {
            "The project's GitHub App doesn't reach that repository. Check the project's GitHub settings."
        }
        _ => return None,
    })
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
) -> Result<RoleGrant, AppError> {
    require_project_access_as(
        transaction,
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        project_id,
        action,
    )
    .await
}

/// `require_project_access` for a caller resolved elsewhere, such as a
/// person's connected app.
pub(crate) async fn require_project_access_as(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    kind: ActorKind,
    project_id: Uuid,
    action: Action,
) -> Result<RoleGrant, AppError> {
    let access = sqlx::query_as::<_, ProjectAccess>(
        r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id) AS role_permissions
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        WHERE project.id = $1 AND project.archived_at IS NULL
        FOR SHARE OF project, membership, organization_membership
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    let role = RoleGrant::stored(&access.role, access.role_permissions.as_deref())?;
    let subject = Subject::new(
        actor_id,
        kind,
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

fn role_allows_manage(role: RoleGrant) -> bool {
    role.permissions().contains(Permission::CodeManage)
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    use super::*;

    // Short, so the tests run in real time.
    const TEST_LEASE: Duration = Duration::from_millis(600);

    #[test]
    fn task_keys_are_found_where_a_pull_request_names_them() {
        assert_eq!(task_keys_in("claude/awd-59-pr-card"), vec!["AWD-59"]);
        assert_eq!(
            task_keys_in("Fix the board (AWD-7), see AW-12 and awd-7"),
            vec!["AWD-7", "AW-12"]
        );
        assert_eq!(
            task_keys_in("Closes [OPS2-3](https://x/OPS2-3)"),
            vec!["OPS2-3"]
        );
        // Not keys: a leading zero, a number that runs on into letters, a
        // one-letter or eleven-letter prefix, or one glued to a word.
        for text in [
            "AWD-059",
            "AWD-7b",
            "A-7",
            "ABCDEFGHIJK-7",
            "9AWD-7",
            "AWD-",
            "AWD-1234567890",
        ] {
            assert!(task_keys_in(text).is_empty(), "{text}");
        }
        let many = (1..=30)
            .map(|number| format!("AW-{number}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(task_keys_in(&many).len(), MAX_TASK_KEYS);
    }

    #[test]
    fn a_pull_request_webhook_keeps_the_task_keys_it_names() {
        let payload = bounded_job_payload(
            "pull_request",
            Some("opened"),
            &json!({"pull_request": {
                "number": 9, "head": {"ref": "awd-59-card", "sha": "a"}, "base": {"ref": "main"},
                "title": "PR card (AWD-59)", "body": "Also AWD-60 and AW-1"
            }}),
        )
        .expect("payload");
        assert_eq!(payload["task_keys"], json!(["AWD-59", "AWD-60", "AW-1"]));
        let none = bounded_job_payload(
            "pull_request",
            Some("opened"),
            &json!({"pull_request": {"number": 9, "title": "No key here", "body": null}}),
        )
        .expect("payload");
        assert!(none.get("task_keys").is_none());
    }

    // A publish takes far longer than one lease; the lease must keep being
    // extended for all of it, or another worker reclaims the job and pushes
    // with the same credentials a second time.
    #[tokio::test]
    async fn a_long_job_keeps_its_lease_until_it_finishes() {
        let renewals = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&renewals);
        let result = hold_lease(
            TEST_LEASE,
            async {
                tokio::time::sleep(TEST_LEASE * 6).await;
                "pushed"
            },
            move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Ok::<bool, ()>(true) }
            },
        )
        .await;
        assert_eq!(result, Some("pushed"));
        assert!(renewals.load(std::sync::atomic::Ordering::SeqCst) >= 5);
    }

    #[tokio::test]
    async fn a_job_that_loses_its_lease_stops() {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = std::sync::Arc::clone(&finished);
        let result = hold_lease(
            TEST_LEASE,
            async move {
                tokio::time::sleep(TEST_LEASE * 2).await;
                done.store(true, std::sync::atomic::Ordering::SeqCst);
            },
            || async { Ok::<bool, ()>(false) },
        )
        .await;
        assert_eq!(result, None);
        assert!(
            !finished.load(std::sync::atomic::Ordering::SeqCst),
            "the work after the lost lease never ran"
        );
    }

    #[tokio::test]
    async fn renewals_that_keep_failing_stop_the_job_before_the_lease_ends() {
        let started = tokio::time::Instant::now();
        let result = hold_lease(TEST_LEASE, tokio::time::sleep(TEST_LEASE * 4), || async {
            Err::<bool, ()>(())
        })
        .await;
        assert_eq!(result, None);
        assert!(started.elapsed() < TEST_LEASE);
    }

    #[test]
    fn installation_token_debug_is_redacted() {
        let token = InstallationToken {
            secret: SecretString::from("opaque-long-token"),
            _expires_at: "future".to_owned(),
        };
        assert!(!format!("{token:?}").contains(token.expose()));
    }

    #[test]
    fn signed_or_canonical_merge_commit_recovers_a_missing_rest_projection() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tools/github-spike/fixtures/merged-close-rest-null-sha.json"
        ))
        .expect("merged close compatibility fixture");
        let signed_webhook = &fixture["signed_webhook"];
        let payload = bounded_job_payload(
            "pull_request",
            signed_webhook["action"].as_str(),
            signed_webhook,
        )
        .expect("bounded signed merged-close payload");
        let merge_sha = payload["merge_commit_sha"]
            .as_str()
            .expect("signed merge commit identity");
        let mut rest_projection: GitHubPullRequest =
            serde_json::from_value(fixture["rest_response"].clone())
                .expect("REST compatibility projection");
        assert!(rest_projection.merged_at.is_some());
        assert!(rest_projection.merge_commit_sha.is_none());
        reconcile_merge_commit_identity(
            &mut rest_projection.merge_commit_sha,
            Some(merge_sha),
            true,
        )
        .expect("signed webhook merge identity");
        assert_eq!(rest_projection.merge_commit_sha.as_deref(), Some(merge_sha));

        let mut later_manual_refresh: GitHubPullRequest =
            serde_json::from_value(fixture["rest_response"].clone())
                .expect("later REST compatibility projection");
        reconcile_merge_commit_identity(
            &mut later_manual_refresh.merge_commit_sha,
            Some(merge_sha),
            true,
        )
        .expect("retained canonical merge identity");
        assert_eq!(
            later_manual_refresh.merge_commit_sha.as_deref(),
            Some(merge_sha)
        );

        let mut conflicting_provider_sha =
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned());
        assert!(
            reconcile_merge_commit_identity(&mut conflicting_provider_sha, Some(merge_sha), true,)
                .is_err()
        );
    }

    #[test]
    fn pull_request_job_payload_keeps_only_bounded_merge_identity() {
        let merge_sha = "16abd896bc03495fad5e4be5851bc0862d059eca";
        let payload = bounded_job_payload(
            "pull_request",
            Some("closed"),
            &json!({
                "pull_request": {
                    "number": 3,
                    "node_id": "PR_kwDOTo2lks75Xg3O",
                    "head": {
                        "sha": "215b8b2b650c77a3dddbda917263f633fb6a3067",
                        "ref": "codex/run-aw-acceptance"
                    },
                    "base": { "ref": "main" },
                    "merged": true,
                    "merge_commit_sha": merge_sha,
                    "body": "must not be retained"
                },
                "repository": { "private": true },
                "installation": { "token": "must not be retained" }
            }),
        )
        .expect("bounded pull request payload");
        assert_eq!(payload["merge_commit_sha"], merge_sha);
        assert!(payload.get("body").is_none());
        assert!(payload.get("installation").is_none());

        let stale_open_payload = bounded_job_payload(
            "pull_request",
            Some("synchronize"),
            &json!({
                "pull_request": {
                    "number": 3,
                    "merged": false,
                    "merge_commit_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }),
        )
        .expect("bounded synchronize payload");
        assert!(stale_open_payload.get("merge_commit_sha").is_none());

        let review_payload = bounded_job_payload(
            "pull_request_review",
            Some("submitted"),
            &json!({
                "pull_request": {
                    "number": 3,
                    "merged": true,
                    "merge_commit_sha": merge_sha
                }
            }),
        )
        .expect("bounded review payload");
        assert!(review_payload.get("merge_commit_sha").is_none());
    }

    #[test]
    fn pull_request_scope_and_external_identity_are_exact() {
        // Merging writes the base branch, and nothing else.
        assert_eq!(
            InstallationTokenScope::PullRequestsMerge.permissions(),
            json!({
                "metadata": "read",
                "contents": "write",
                "pull_requests": "write"
            })
        );
        assert_eq!(
            InstallationTokenScope::PullRequestsWrite.permissions(),
            json!({
                "metadata": "read",
                "contents": "read",
                "pull_requests": "write"
            })
        );
        let job = ClaimedPullRequestJob {
            job_id: Uuid::now_v7(),
            request_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            repository_id: Uuid::now_v7(),
            repository_workspace_id: Uuid::now_v7(),
            agent_run_id: Uuid::now_v7(),
            requesting_actor_id: Uuid::now_v7(),
            claim_id: Uuid::now_v7(),
            attempts: 1,
            max_attempts: 3,
            conversation_id: Uuid::now_v7(),
            installation_id: 150_232_852,
            github_repository_id: 1_317_905_810,
            owner: "Mischa-dev".to_owned(),
            repository_name: "agent-workspace-m0-spike".to_owned(),
            head_ref: "codex/run-m5-pr".to_owned(),
            head_sha: "0123456789012345678901234567890123456789".to_owned(),
            base_ref: "main".to_owned(),
            title: "M5 exact pull request".to_owned(),
            body: String::new(),
            work_item_ids: sqlx::types::Json(Vec::new()),
            mutation_attempted: false,
            scope_available: true,
        };
        let mut pull_request = GitHubPullRequest {
            number: 3,
            node_id: "PR_kwDOExample".to_owned(),
            html_url: "https://github.com/Mischa-dev/agent-workspace-m0-spike/pull/3".to_owned(),
            title: "M5 exact pull request".to_owned(),
            body: None,
            user: GitHubPullRequestUser {
                login: "Mischa-dev".to_owned(),
            },
            draft: false,
            state: "open".to_owned(),
            merged_at: None,
            merge_commit_sha: None,
            created_at: "2026-08-01T10:00:00Z".to_owned(),
            updated_at: "2026-08-01T10:00:00Z".to_owned(),
            head: GitHubPullRequestRef {
                name: job.head_ref.clone(),
                sha: job.head_sha.clone(),
            },
            base: GitHubPullRequestRef {
                name: job.base_ref.clone(),
                sha: "1111111111111111111111111111111111111111".to_owned(),
            },
            mergeable: Some(true),
            requested_reviewers: Vec::new(),
        };
        assert!(validate_pull_request(&job, &pull_request).is_ok());
        pull_request.head.sha = "2222222222222222222222222222222222222222".to_owned();
        assert!(validate_pull_request(&job, &pull_request).is_err());
    }

    #[test]
    fn check_review_projection_is_bounded_and_never_overstates_certainty() {
        assert_eq!(
            InstallationTokenScope::ChecksRead.permissions(),
            json!({
                "metadata": "read",
                "contents": "read",
                "pull_requests": "read",
                "checks": "read"
            })
        );
        let mut pull_request = GitHubPullRequest {
            number: 7,
            node_id: "PR_kwDOCanonical".to_owned(),
            html_url: "https://github.com/Mischa-dev/agent-workspace-m0-spike/pull/7".to_owned(),
            title: "Canonical review state".to_owned(),
            body: None,
            user: GitHubPullRequestUser {
                login: "Mischa-dev".to_owned(),
            },
            draft: false,
            state: "open".to_owned(),
            merged_at: None,
            merge_commit_sha: None,
            created_at: "2026-08-01T10:00:00Z".to_owned(),
            updated_at: "2026-08-01T10:00:01Z".to_owned(),
            head: GitHubPullRequestRef {
                name: "codex/m5-review".to_owned(),
                sha: "0123456789012345678901234567890123456789".to_owned(),
            },
            base: GitHubPullRequestRef {
                name: "main".to_owned(),
                sha: "1111111111111111111111111111111111111111".to_owned(),
            },
            mergeable: Some(true),
            requested_reviewers: vec![GitHubPullRequestUser {
                login: "reviewer".to_owned(),
            }],
        };
        let mut checks = vec![GitHubCheckRun {
            id: 10,
            name: "CI".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
            details_url: Some("https://example.test/check/10".to_owned()),
            started_at: Some("2026-08-01T10:00:00Z".to_owned()),
            completed_at: Some("2026-08-01T10:00:01Z".to_owned()),
        }];
        let reviews = vec![GitHubReview {
            id: 20,
            user: GitHubPullRequestUser {
                login: "reviewer".to_owned(),
            },
            state: "APPROVED".to_owned(),
            submitted_at: Some("2026-08-01T10:00:02Z".to_owned()),
        }];
        let projection = validate_and_reduce_review_projection(&pull_request, &checks, &reviews)
            .expect("valid canonical projection");
        assert_eq!(projection.checks_state, "success");
        assert_eq!(projection.review_state, "approved");
        assert_eq!(projection.mergeability_state, "ready");

        checks[0].conclusion = Some("failure".to_owned());
        let projection = validate_and_reduce_review_projection(&pull_request, &checks, &reviews)
            .expect("failure projection");
        assert_eq!(projection.checks_state, "failure");
        assert_eq!(projection.mergeability_state, "blocked");

        pull_request.mergeable = None;
        checks.clear();
        let projection = validate_and_reduce_review_projection(&pull_request, &checks, &[])
            .expect("unknown projection");
        assert_eq!(projection.checks_state, "unknown");
        assert_eq!(projection.review_state, "pending");
        assert_eq!(projection.mergeability_state, "unknown");
    }

    #[test]
    fn webhook_signature_and_action_allowlist_are_strict() {
        let secret = b"a-high-entropy-development-webhook-secret";
        let previous_secret = b"the-previous-high-entropy-webhook-secret";
        let body = br#"{"installation":{"id":1},"repository":{"id":2,"full_name":"owner/repository"},"after":"0123456789012345678901234567890123456789"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key");
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature(secret, body, &signature).is_ok());
        assert!(verify_signature(secret, b"changed", &signature).is_err());
        assert!(
            verify_signature_with_overlap(secret, Some(previous_secret), body, &signature).is_ok()
        );
        let mut previous_mac =
            Hmac::<Sha256>::new_from_slice(previous_secret).expect("previous HMAC key");
        previous_mac.update(body);
        let previous_signature = format!(
            "sha256={}",
            hex::encode(previous_mac.finalize().into_bytes())
        );
        assert!(
            verify_signature_with_overlap(
                secret,
                Some(previous_secret),
                body,
                &previous_signature,
            )
            .is_ok()
        );
        assert!(verify_signature_with_overlap(secret, None, body, &previous_signature).is_err());
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
            webhook_previous_secret_path: None,
            api_base_url: "https://api.github.com".to_owned(),
            api_version: GITHUB_API_VERSION.to_owned(),
            connect: None,
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
