//! Connect GitHub from Settings: install the App, prove which installations
//! you can reach by signing in to GitHub, then pick repositories.
//!
//! An installation number alone proves nothing, so a workspace can only use
//! installations it has claimed: someone in it signed in to GitHub through the
//! App's OAuth client and GitHub listed the installation as theirs. The OAuth
//! state is random, used once, expires in ten minutes and is bound to the
//! browser that started it by a cookie only that browser holds. The user's
//! GitHub token is used for that one listing and then dropped.

use axum::{
    extract::Query,
    response::Redirect,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};

use super::*;
use crate::{
    auth::cookie_value,
    config::{Environment, GitHubConnectConfig},
    invitations::public_origin,
};

const GITHUB_WEB_ORIGIN: &str = "https://github.com";
const CALLBACK_PATH: &str = "/api/v1/integrations/github/callback";
const ATTEMPT_MINUTES: i32 = 10;
const MAX_CLAIMED_INSTALLATIONS: i64 = 20;

pub(super) fn routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/api/v1/projects/{project_id}/integrations/github/connection",
            get(connection),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/connect",
            post(start_connect),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/available-repositories",
            get(available_repositories),
        )
        .route(
            "/api/v1/projects/{project_id}/integrations/github/repositories",
            post(add_repository),
        )
        .route("/api/v1/integrations/github/callback", get(callback))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionResponse {
    configured: bool,
    connect_available: bool,
    app_slug: Option<String>,
    installations: Vec<ClaimView>,
    capabilities: GitHubCapabilities,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ClaimView {
    installation_id: i64,
    account_login: String,
    target_type: String,
    claimed_by_display_name: Option<String>,
    claimed_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectRequest {
    mode: ConnectMode,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConnectMode {
    Install,
    Authorize,
}

#[derive(Debug, Serialize)]
struct ConnectResponse {
    url: String,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    installation_id: Option<String>,
    error: Option<String>,
}

#[derive(Debug, FromRow)]
struct Attempt {
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    expired: bool,
    used: bool,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserInstallations {
    installations: Vec<GitHubInstallation>,
}

#[derive(Debug, Deserialize)]
struct PickerRepositories {
    repositories: Vec<PickerRepository>,
}

#[derive(Debug, Deserialize)]
struct PickerRepository {
    id: u64,
    name: String,
    full_name: String,
    owner: GitHubOwner,
    default_branch: Option<String>,
    private: bool,
    archived: Option<bool>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubOwner {
    login: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailableRepositoriesResponse {
    repositories: Vec<AvailableRepository>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailableRepository {
    installation_id: i64,
    account_login: String,
    github_repository_id: i64,
    owner: String,
    name: String,
    full_name: String,
    default_branch: String,
    private: bool,
    description: Option<String>,
    added: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddRepositoryRequest {
    installation_id: i64,
    github_repository_id: i64,
}

#[derive(Debug, FromRow)]
struct ClaimRow {
    installation_id: i64,
    account_login: String,
}

fn connect_config(state: &AppState) -> Option<(&GitHubConfig, &GitHubConnectConfig)> {
    let github = state.config.github.as_ref()?;
    Some((github, github.connect.as_ref()?))
}

async fn project_organization(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar("SELECT organization_id FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("project"))
}

/// Whether the project's workspace has proved access to an installation.
pub(super) async fn workspace_claims_installation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    installation_id: i64,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM github_installation_claims claim
            JOIN projects project ON project.organization_id = claim.organization_id
            WHERE project.id = $1 AND claim.installation_id = $2
        )
        "#,
    )
    .bind(project_id)
    .bind(installation_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Record that the project's workspace may use an installation. Used where
/// the host has not set up GitHub sign-in, so typing the number is the only
/// way in and the host operator vouches for the App's installations.
pub(super) async fn record_operator_claim(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_id: Uuid,
    installation: &GitHubInstallation,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO github_installation_claims (
            organization_id, installation_id, account_login, target_type, claimed_by_actor_id
        )
        SELECT organization_id, $2, $3, $4, $5 FROM projects WHERE id = $1
        ON CONFLICT (organization_id, installation_id) DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(i64::try_from(installation.id).map_err(|_| AppError::Validation("installationId"))?)
    .bind(&installation.account.login)
    .bind(&installation.target_type)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn load_claims(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<Vec<ClaimView>, AppError> {
    Ok(sqlx::query_as::<_, ClaimView>(
        r#"
        SELECT claim.installation_id, claim.account_login, claim.target_type,
               profile.display_name AS claimed_by_display_name, claim.claimed_at
        FROM github_installation_claims claim
        LEFT JOIN organization_actor_profiles profile
          ON profile.organization_id = claim.organization_id
         AND profile.actor_id = claim.claimed_by_actor_id
        WHERE claim.organization_id = $1
        ORDER BY lower(claim.account_login), claim.installation_id
        LIMIT $2
        "#,
    )
    .bind(organization_id)
    .bind(MAX_CLAIMED_INSTALLATIONS)
    .fetch_all(&mut **transaction)
    .await?)
}

async fn connection(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ConnectionResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let organization_id = project_organization(&mut transaction, project_id).await?;
    let installations = load_claims(&mut transaction, organization_id).await?;
    transaction.commit().await?;
    let connect = connect_config(&state);
    Ok(Json(ConnectionResponse {
        configured: state.config.github.is_some(),
        connect_available: connect.is_some(),
        app_slug: connect.map(|(_, connect)| connect.app_slug.clone()),
        installations,
        capabilities: GitHubCapabilities {
            manage: role_allows_manage(role),
        },
    }))
}

fn cookie_name(state: &AppState) -> &'static str {
    if state.config.environment == Environment::Development {
        "aw_github_connect"
    } else {
        "__Host-aw_github_connect"
    }
}

fn browser_cookie(state: &AppState, value: &str, max_age: i32) -> String {
    let secure = if state.config.environment == Environment::Development {
        ""
    } else {
        "; Secure"
    };
    // Lax, unlike the session cookie: GitHub's redirect back is a top-level
    // navigation from another site, and this cookie has to ride along.
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        cookie_name(state)
    )
}

fn random_value() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    OsRng.fill_bytes(bytes.as_mut());
    Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref()))
}

fn digest(domain: &str, value: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(value.as_bytes());
    hasher.finalize().to_vec()
}

fn callback_url(state: &AppState) -> String {
    format!("{}{CALLBACK_PATH}", public_origin(state))
}

fn connect_url(
    state: &AppState,
    connect: &GitHubConnectConfig,
    mode: ConnectMode,
    oauth_state: &str,
) -> Result<String, AppError> {
    let mut url = match mode {
        ConnectMode::Install => url::Url::parse(&format!(
            "{GITHUB_WEB_ORIGIN}/apps/{}/installations/new",
            connect.app_slug
        )),
        ConnectMode::Authorize => {
            url::Url::parse(&format!("{GITHUB_WEB_ORIGIN}/login/oauth/authorize"))
        }
    }
    .map_err(|_| AppError::Internal("GitHub connect URL"))?;
    if matches!(mode, ConnectMode::Authorize) {
        url.query_pairs_mut()
            .append_pair("client_id", &connect.client_id)
            .append_pair("redirect_uri", &callback_url(state));
    }
    url.query_pairs_mut().append_pair("state", oauth_state);
    Ok(url.to_string())
}

async fn start_connect(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ConnectRequest>,
) -> Result<(HeaderMap, Json<ConnectResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let (_, connect) = connect_config(&state).ok_or(AppError::Conflict(
        "github_integration",
        "connect_not_configured",
    ))?;
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let organization_id = project_organization(&mut transaction, project_id).await?;
    sqlx::query("DELETE FROM github_connect_attempts WHERE expires_at < now() - interval '1 day'")
        .execute(&mut *transaction)
        .await?;
    let oauth_state = random_value();
    let browser = random_value();
    sqlx::query(
        r#"
        INSERT INTO github_connect_attempts (
            state_digest, browser_digest, organization_id, project_id, actor_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, now() + make_interval(mins => $6))
        "#,
    )
    .bind(digest("aw-github-connect-state:v1:", &oauth_state))
    .bind(digest("aw-github-connect-browser:v1:", &browser))
    .bind(organization_id)
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(ATTEMPT_MINUTES)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let url = connect_url(&state, connect, request.mode, &oauth_state)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&browser_cookie(&state, &browser, ATTEMPT_MINUTES * 60))
            .map_err(|_| AppError::Internal("GitHub connect cookie"))?,
    );
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((response_headers, Json(ConnectResponse { url })))
}

/// Where the callback sends the browser. Only fixed paths and codes, so the
/// query string can't steer it anywhere else.
fn settings_redirect(project_id: Option<Uuid>, outcome: Result<(), &'static str>) -> String {
    let base = project_id.map_or_else(
        || "/settings".to_owned(),
        |project_id| format!("/settings/projects/{project_id}/repositories"),
    );
    match outcome {
        Ok(()) => format!("{base}?github=connected"),
        Err(reason) => format!("{base}?github=error&reason={reason}"),
    }
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let (project_id, outcome) = finish_callback(&state, &headers, query).await;
    let mut response =
        Redirect::to(&settings_redirect(project_id, outcome.map(|_| ()))).into_response();
    let response_headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&browser_cookie(&state, "", 0)) {
        response_headers.insert(header::SET_COOKIE, value);
    }
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response_headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn finish_callback(
    state: &AppState,
    headers: &HeaderMap,
    query: CallbackQuery,
) -> (Option<Uuid>, Result<usize, &'static str>) {
    let Some(oauth_state) = query.state.filter(|value| value.len() <= 256) else {
        return (None, Err("expired"));
    };
    let browser = cookie_value(headers, cookie_name(state)).filter(|value| value.len() <= 256);
    let attempt = match take_attempt(state, &oauth_state, browser.as_deref()).await {
        Ok(attempt) => attempt,
        Err(reason) => return (None, Err(reason)),
    };
    let project_id = Some(attempt.project_id);
    if query.error.is_some() {
        return (project_id, Err("denied"));
    }
    let Some(code) = query
        .code
        .filter(|value| !value.is_empty() && value.len() <= 512)
    else {
        return (project_id, Err("denied"));
    };
    let expected_installation = match query.installation_id.as_deref() {
        None => None,
        Some(value) => match value.parse::<u64>() {
            Ok(id) if id > 0 => Some(id),
            _ => return (project_id, Err("failed")),
        },
    };
    let Some((github, connect)) = connect_config(state) else {
        return (project_id, Err("failed"));
    };
    let installations =
        match reachable_installations(state, github, connect, Zeroizing::new(code)).await {
            Ok(installations) => installations,
            Err(_) => return (project_id, Err("failed")),
        };
    if expected_installation.is_some_and(|expected| {
        !installations
            .iter()
            .any(|installation| installation.id == expected)
    }) {
        return (project_id, Err("not_yours"));
    }
    match record_claims(state, &attempt, &installations).await {
        Ok(()) => (project_id, Ok(installations.len())),
        Err(AppError::Forbidden | AppError::NotFound(_)) => (project_id, Err("not_yours")),
        Err(error) => {
            tracing::warn!(
                error_category = error.log_category(),
                "GitHub connect callback failed"
            );
            (project_id, Err("failed"))
        }
    }
}

/// Use up a connect attempt. It must exist, belong to this browser, be
/// unused and unexpired; any mismatch reads as expired.
async fn take_attempt(
    state: &AppState,
    oauth_state: &str,
    browser: Option<&str>,
) -> Result<Attempt, &'static str> {
    // Without this browser's cookie the attempt isn't even touched, so a
    // leaked state can't be used or burned from elsewhere.
    let Some(browser) = browser else {
        return Err("expired");
    };
    let attempt = sqlx::query_as::<_, Attempt>(
        r#"
        UPDATE github_connect_attempts
        SET used_at = COALESCE(used_at, now())
        WHERE state_digest = $1 AND browser_digest = $2
        RETURNING organization_id, project_id, actor_id,
                  expires_at <= now() AS expired,
                  used_at <> now() AS used
        "#,
    )
    .bind(digest("aw-github-connect-state:v1:", oauth_state))
    .bind(digest("aw-github-connect-browser:v1:", browser))
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| "failed")?
    .ok_or("expired")?;
    if attempt.expired || attempt.used {
        return Err("expired");
    }
    Ok(attempt)
}

/// Trade the OAuth code for the person's GitHub token and list the App's
/// installations GitHub says they can reach. The token is not kept.
async fn reachable_installations(
    state: &AppState,
    github: &GitHubConfig,
    connect: &GitHubConnectConfig,
    code: Zeroizing<String>,
) -> Result<Vec<GitHubInstallation>, GitHubError> {
    let client = GitHubClient::new(github)?;
    let client_secret = read_secret_file(&connect.client_secret_path, 4 * 1024)?;
    let client_secret = std::str::from_utf8(&client_secret).map_err(|_| GitHubError::Credential)?;
    let response = client
        .http
        .post(format!("{GITHUB_WEB_ORIGIN}/login/oauth/access_token"))
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", connect.client_id.as_str()),
            ("client_secret", client_secret.trim()),
            ("code", code.as_str()),
            ("redirect_uri", callback_url(state).as_str()),
        ])
        .send()
        .await
        .map_err(|_| GitHubError::Http)?;
    let token: OAuthTokenResponse = decode_response(response).await?;
    let token = Zeroizing::new(token.access_token.ok_or(GitHubError::Response)?);
    if token.is_empty() || token.len() > 4 * 1024 {
        return Err(GitHubError::Response);
    }
    let response = client
        .http
        .get(format!(
            "{}/user/installations?per_page=100",
            github.api_base_url
        ))
        .headers(github_headers(&token)?)
        .send()
        .await
        .map_err(|_| GitHubError::Http)?;
    let listed: UserInstallations = decode_response(response).await?;
    Ok(listed
        .installations
        .into_iter()
        .filter(|installation| installation.app_id == github.app_id)
        .filter(valid_claim_shape)
        .take(MAX_CLAIMED_INSTALLATIONS as usize)
        .collect())
}

fn valid_claim_shape(installation: &GitHubInstallation) -> bool {
    installation.id > 0
        && i64::try_from(installation.id).is_ok()
        && !installation.account.login.is_empty()
        && installation.account.login.len() <= 100
        && !installation.account.login.chars().any(char::is_control)
        && matches!(installation.target_type.as_str(), "User" | "Organization")
}

async fn record_claims(
    state: &AppState,
    attempt: &Attempt,
    installations: &[GitHubInstallation],
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    // The person who started this must still be allowed to manage the
    // project's GitHub connection when they come back.
    let (role, role_permissions, kind) =
        sqlx::query_as::<_, (String, Option<Vec<String>>, String)>(
            r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id), actor.kind
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
        WHERE project.id = $1 AND project.organization_id = $3
          AND project.archived_at IS NULL
        FOR SHARE OF project, membership, organization_membership
        "#,
        )
        .bind(attempt.project_id)
        .bind(attempt.actor_id)
        .bind(attempt.organization_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::Forbidden)?;
    let subject = Subject::new(
        attempt.actor_id,
        actor_kind(&kind)?,
        true,
        Some(Membership::project(
            attempt.project_id,
            crate::policy::RoleGrant::stored(&role, role_permissions.as_deref())?,
            true,
        )),
        None,
    );
    if !authorize(
        &subject,
        Action::GithubIntegrationManage,
        Resource::Project(ProjectScope::project(attempt.project_id)),
    )
    .is_allowed()
    {
        return Err(AppError::Forbidden);
    }
    for installation in installations {
        sqlx::query(
            r#"
            INSERT INTO github_installation_claims (
                organization_id, installation_id, account_login, target_type, claimed_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (organization_id, installation_id) DO UPDATE
                SET account_login = EXCLUDED.account_login,
                    target_type = EXCLUDED.target_type,
                    claimed_by_actor_id = EXCLUDED.claimed_by_actor_id,
                    claimed_at = now()
            "#,
        )
        .bind(attempt.organization_id)
        .bind(i64::try_from(installation.id).map_err(|_| AppError::Validation("installationId"))?)
        .bind(&installation.account.login)
        .bind(&installation.target_type)
        .bind(attempt.actor_id)
        .execute(&mut *transaction)
        .await?;
    }
    let installation_ids = installations
        .iter()
        .map(|installation| installation.id)
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, organization_id, authentication_class,
            action, target_kind, target_id, outcome, channel, metadata
        ) VALUES ($1, $2, $3, $4, 'github_oauth', 'github.installation_claim',
                  'project', $3, 'succeeded', 'browser', $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(attempt.actor_id)
    .bind(attempt.project_id)
    .bind(attempt.organization_id)
    .bind(json!({ "installation_ids": installation_ids }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn available_repositories(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<AvailableRepositoriesResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    let Some(github) = state.config.github.as_ref() else {
        return Ok(Json(AvailableRepositoriesResponse {
            repositories: Vec::new(),
        }));
    };
    let organization_id = project_organization(&mut transaction, project_id).await?;
    let claims = sqlx::query_as::<_, ClaimRow>(
        "SELECT installation_id, account_login FROM github_installation_claims WHERE organization_id = $1 ORDER BY lower(account_login), installation_id LIMIT $2",
    )
    .bind(organization_id)
    .bind(MAX_CLAIMED_INSTALLATIONS)
    .fetch_all(&mut *transaction)
    .await?;
    let added = sqlx::query_scalar::<_, i64>(
        "SELECT github_repository_id FROM repositories WHERE project_id = $1 AND github_repository_id IS NOT NULL AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    transaction.commit().await?;

    let client = GitHubClient::new(github).map_err(map_github_error)?;
    let mut repositories = Vec::new();
    for claim in claims {
        let installation_id = u64::try_from(claim.installation_id)
            .map_err(|_| AppError::Internal("stored installation ID"))?;
        // An installation that was removed, suspended or set up with the
        // wrong permissions just lists nothing; the others still show.
        let Ok(listed) = client
            .installation_repositories(installation_id, github)
            .await
        else {
            continue;
        };
        for repository in listed {
            let Ok(github_repository_id) = i64::try_from(repository.id) else {
                continue;
            };
            if repository.archived == Some(true) {
                continue;
            }
            repositories.push(AvailableRepository {
                installation_id: claim.installation_id,
                account_login: claim.account_login.clone(),
                github_repository_id,
                owner: repository.owner.login,
                name: repository.name,
                full_name: repository.full_name,
                default_branch: repository
                    .default_branch
                    .unwrap_or_else(|| "main".to_owned()),
                private: repository.private,
                description: repository
                    .description
                    .map(|text| text.chars().take(300).collect()),
                added: added.contains(&github_repository_id),
            });
        }
    }
    Ok(Json(AvailableRepositoriesResponse { repositories }))
}

impl GitHubClient {
    async fn installation_repositories(
        &self,
        installation_id: u64,
        config: &GitHubConfig,
    ) -> Result<Vec<PickerRepository>, GitHubError> {
        let installation = self.installation(installation_id).await?;
        validate_installation(config, &installation)?;
        let token = self
            .installation_token_all(installation_id, InstallationTokenScope::VerifyInstallation)
            .await?;
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
        let listed: PickerRepositories = decode_response(response).await?;
        Ok(listed.repositories)
    }

    async fn installation_token_all(
        &self,
        installation_id: u64,
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
            .json(&json!({ "permissions": scope.permissions() }))
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

    async fn repository_by_id(
        &self,
        token: &InstallationToken,
        github_repository_id: u64,
    ) -> Result<PickerRepository, GitHubError> {
        let response = self
            .http
            .get(format!(
                "{}/repositories/{github_repository_id}",
                self.config.api_base_url
            ))
            .headers(github_headers(token.expose())?)
            .send()
            .await
            .map_err(|_| GitHubError::Http)?;
        decode_response(response).await
    }
}

async fn add_repository(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<AddRepositoryRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.installation_id <= 0 {
        return Err(AppError::Validation("installationId"));
    }
    if request.github_repository_id <= 0 {
        return Err(AppError::Validation("githubRepositoryId"));
    }
    let github = state
        .config
        .github
        .as_ref()
        .ok_or(AppError::Conflict("github_integration", "not_configured"))?;

    // Check the person and the claim before asking GitHub anything.
    let mut transaction = state.pool.begin().await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryManage,
    )
    .await?;
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    // It answers with the repository it registers: only for someone who
    // also sees code here.
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryView,
    )
    .await?;
    if !workspace_claims_installation(&mut transaction, project_id, request.installation_id).await?
    {
        return Err(AppError::NotFound("github_installation"));
    }
    if let Some(linked) = load_installation(&mut transaction, project_id).await?
        && linked.installation_id != request.installation_id
    {
        return Err(AppError::Conflict(
            "github_installation",
            "project_uses_another_installation",
        ));
    }
    transaction.commit().await?;

    let installation_id = request.installation_id as u64;
    let github_repository_id = request.github_repository_id as u64;
    let client = GitHubClient::new(github).map_err(map_github_error)?;
    let installation = client
        .installation(installation_id)
        .await
        .map_err(map_github_error)?;
    validate_installation(github, &installation).map_err(map_github_error)?;
    let token = client
        .installation_token(
            installation_id,
            &[github_repository_id],
            InstallationTokenScope::VerifyInstallation,
        )
        .await
        // GitHub refuses a token for a repository the installation can't see.
        .map_err(|_| AppError::NotFound("repository"))?;
    let repository = client
        .repository_by_id(&token, github_repository_id)
        .await
        .map_err(|_| AppError::NotFound("repository"))?;
    drop(token);
    if repository.id != github_repository_id {
        return Err(AppError::NotFound("repository"));
    }
    let default_branch = repository
        .default_branch
        .clone()
        .ok_or(AppError::Validation("defaultBranch"))?;

    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::GithubIntegrationManage,
    )
    .await?;
    if !workspace_claims_installation(&mut transaction, project_id, request.installation_id).await?
    {
        return Err(AppError::NotFound("github_installation"));
    }
    let repository_id = crate::repositories::register_picked_repository(
        &state,
        &mut transaction,
        project_id,
        request.github_repository_id,
        &repository.owner.login,
        &repository.name,
        &default_branch,
    )
    .await?;
    let actor_id = ensure_github_actor(&mut transaction, project_id).await?;
    let installation_row_id = persist_installation(
        &mut transaction,
        project_id,
        actor_id,
        github,
        &installation,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO github_installation_repositories (
            project_id, github_installation_id, repository_id,
            github_repository_id, access_state, last_verified_at
        ) VALUES ($1, $2, $3, $4, 'selected', now())
        ON CONFLICT (github_installation_id, repository_id) DO UPDATE
            SET github_repository_id = EXCLUDED.github_repository_id,
                access_state = 'selected', last_verified_at = now(), updated_at = now()
        "#,
    )
    .bind(project_id)
    .bind(installation_row_id)
    .bind(repository_id)
    .bind(request.github_repository_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE repositories SET github_app_installation_id = $3, updated_at = now() WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(repository_id)
    .bind(request.installation_id)
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
            action, target_kind, target_id, outcome, request_id, channel, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session', 'github.repository_connect',
            'repository', $5, 'succeeded', $6, 'browser',
            jsonb_build_object('installation_id', $7::bigint, 'github_repository_id', $8::bigint)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(repository_id)
    .bind(request_id)
    .bind(request.installation_id)
    .bind(request.github_repository_id)
    .execute(&mut *transaction)
    .await?;
    let view = crate::repositories::repository_json(
        &mut transaction,
        project_id,
        repository_id,
        role_allows_manage(role),
    )
    .await?;
    transaction.commit().await?;
    crate::repositories::prepare_in_background(state.clone(), project_id, repository_id);
    Ok((StatusCode::CREATED, Json(view)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_stay_on_fixed_settings_paths() {
        let project_id = Uuid::nil();
        assert_eq!(
            settings_redirect(Some(project_id), Ok(())),
            format!("/settings/projects/{project_id}/repositories?github=connected")
        );
        assert_eq!(
            settings_redirect(None, Err("expired")),
            "/settings?github=error&reason=expired"
        );
    }

    #[test]
    fn digests_are_domain_separated() {
        assert_ne!(
            digest("aw-github-connect-state:v1:", "x"),
            digest("aw-github-connect-browser:v1:", "x")
        );
        assert_eq!(digest("a", "b").len(), 32);
    }
}
