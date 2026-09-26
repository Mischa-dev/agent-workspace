pub mod password;

use std::future::Future;

use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{COOKIE, SET_COOKIE},
        request::Parts,
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    config::{AppConfig, Environment},
    error::{ApiJson, AppError},
    policy::{Action, ActorKind, Membership, MembershipRole, Resource, Subject, authorize},
};

const SESSION_COOKIE_DEV: &str = "agent_workspace_session";
const CSRF_COOKIE_DEV: &str = "agent_workspace_csrf";
const SESSION_COOKIE_SECURE: &str = "__Host-agent_workspace_session";
const CSRF_COOKIE_SECURE: &str = "__Host-agent_workspace_csrf";
const SESSION_TOKEN_BYTES: usize = 32;
const MAX_COOKIE_VALUE_BYTES: usize = 256;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/session", get(read_session))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
}

#[derive(Clone, Debug)]
pub struct AuthenticatedActor {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub actor_id: Uuid,
    pub actor_kind: String,
    pub login_name: String,
    pub display_name: String,
    csrf_token_hash: Vec<u8>,
}

pub(crate) struct SessionIdentity {
    pub user_id: Uuid,
    pub actor_id: Uuid,
    pub actor_kind: String,
    pub login_name: String,
    pub display_name: String,
    pub authentication_epoch: i64,
}

pub(crate) struct EstablishedSession {
    pub principal: AuthenticatedActor,
    session_token: String,
    csrf_token: String,
    rotated_from_session_id: Option<Uuid>,
}

impl FromRequestParts<AppState> for AuthenticatedActor {
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let headers = parts.headers.clone();
        let state = state.clone();
        async move {
            load_authenticated_actor(&state, &headers)
                .await?
                .ok_or(AppError::AuthenticationRequired)
        }
    }
}

#[derive(Debug, FromRow)]
struct SessionRow {
    session_id: Uuid,
    user_id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    login_name: String,
    display_name: String,
    csrf_token_hash: Vec<u8>,
}

#[derive(Debug, FromRow)]
struct CredentialRow {
    user_id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    login_name: String,
    display_name: String,
    authentication_epoch: i64,
    password_phc: String,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationSummary {
    id: Uuid,
    name: String,
    slug: String,
    role: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    authenticated: bool,
    user: Option<SessionUser>,
    organizations: Vec<OrganizationSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionUser {
    id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    login_name: String,
    display_name: String,
    capabilities: SessionCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionCapabilities {
    create_project: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LoginRequest {
    login_name: String,
    password: String,
}

async fn read_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, AppError> {
    let principal = load_authenticated_actor(&state, &headers).await?;
    Ok(Json(session_response(&state, principal.as_ref()).await?))
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<LoginRequest>,
) -> Result<Response, AppError> {
    if let Err(error) = require_allowed_origin(&state.config, &headers) {
        request.password.zeroize();
        return Err(error);
    }
    let login_name = match normalize_login_name(&request.login_name) {
        Ok(login_name) => login_name,
        Err(error) => {
            request.password.zeroize();
            return Err(error);
        }
    };
    if let Err(error) = state.rate_limits.check_login(&login_name) {
        request.password.zeroize();
        return Err(error);
    }
    if !(12..=1024).contains(&request.password.len()) {
        request.password.zeroize();
        return Err(AppError::InvalidCredentials);
    }

    let credential = match sqlx::query_as::<_, CredentialRow>(
        r#"
        SELECT
            u.id AS user_id,
            u.actor_id,
            a.kind AS actor_kind,
            u.login_name,
            u.display_name,
            u.authentication_epoch,
            c.password_phc
        FROM users u
        JOIN actors a ON a.id = u.actor_id
        JOIN user_password_credentials c ON c.user_id = u.id
        WHERE lower(u.login_name) = $1
          AND u.status = 'active'
          AND a.status = 'active'
          AND a.kind = 'human'
        "#,
    )
    .bind(&login_name)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(credential) => credential,
        Err(error) => {
            request.password.zeroize();
            return Err(error.into());
        }
    };
    let _password_kdf_permit = match state.rate_limits.try_acquire_password_kdf() {
        Ok(permit) => permit,
        Err(error) => {
            request.password.zeroize();
            return Err(error);
        }
    };

    let Some(credential) = credential else {
        password::verify_unknown_login(request.password).await?;
        return Err(AppError::InvalidCredentials);
    };
    let verified =
        password::verify_password(request.password, credential.password_phc.clone()).await?;
    if !verified {
        return Err(AppError::InvalidCredentials);
    }

    let mut transaction = state.pool.begin().await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id: credential.user_id,
            actor_id: credential.actor_id,
            actor_kind: credential.actor_kind,
            login_name: credential.login_name,
            display_name: credential.display_name,
            authentication_epoch: credential.authentication_epoch,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, session_id, authentication_class, action,
            target_kind, target_id, outcome, metadata
        )
        VALUES (
            $1, $2, $3, 'password_session', 'authentication.login',
            'session', $3, 'succeeded', '{}'::jsonb
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(established.principal.actor_id)
    .bind(established.principal.session_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

pub(crate) async fn establish_session_in_transaction(
    config: &AppConfig,
    headers: &HeaderMap,
    transaction: &mut Transaction<'_, Postgres>,
    identity: SessionIdentity,
) -> Result<EstablishedSession, AppError> {
    let session_token = random_token();
    let csrf_token = random_token();
    let session_id = Uuid::now_v7();
    let mut rotated_from_session_id = None;
    if let Some(existing_token) = cookie_value(headers, session_cookie_name(config))
        && !existing_token.is_empty()
        && existing_token.len() <= MAX_COOKIE_VALUE_BYTES
    {
        rotated_from_session_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE sessions
            SET revoked_at = now(), updated_at = now()
            WHERE token_hash = $1 AND revoked_at IS NULL
            RETURNING id
            "#,
        )
        .bind(hash_secret(&existing_token).to_vec())
        .fetch_optional(&mut **transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO sessions (
            id, user_id, token_hash, csrf_token_hash, authentication_epoch,
            rotated_from_session_id, created_at, last_seen_at,
            idle_expires_at, absolute_expires_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            now(), now(), now() + interval '7 days', now() + interval '30 days'
        )
        "#,
    )
    .bind(session_id)
    .bind(identity.user_id)
    .bind(hash_secret(&session_token).to_vec())
    .bind(hash_secret(&csrf_token).to_vec())
    .bind(identity.authentication_epoch)
    .bind(rotated_from_session_id)
    .execute(&mut **transaction)
    .await?;
    Ok(EstablishedSession {
        principal: AuthenticatedActor {
            session_id,
            user_id: identity.user_id,
            actor_id: identity.actor_id,
            actor_kind: identity.actor_kind,
            login_name: identity.login_name,
            display_name: identity.display_name,
            csrf_token_hash: hash_secret(&csrf_token).to_vec(),
        },
        session_token,
        csrf_token,
        rotated_from_session_id,
    })
}

pub(crate) async fn respond_with_established_session(
    state: &AppState,
    established: EstablishedSession,
) -> Result<Response, AppError> {
    if let Some(rotated_session_id) = established.rotated_from_session_id {
        state.realtime.revoke_session(rotated_session_id);
    }
    let response = session_response(state, Some(&established.principal)).await?;
    let mut response_headers = HeaderMap::new();
    append_cookie(
        &mut response_headers,
        session_cookie(&state.config, &established.session_token, false),
    )?;
    append_cookie(
        &mut response_headers,
        csrf_cookie(&state.config, &established.csrf_token, false),
    )?;
    Ok((StatusCode::OK, response_headers, Json(response)).into_response())
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    if let Some(principal) = load_authenticated_actor(&state, &headers).await? {
        // Logout remains available even when this session has exhausted its
        // ordinary mutation budget. Origin and CSRF checks still apply.
        require_csrf(&state.config, &headers, &principal)?;
        let mut transaction = state.pool.begin().await?;
        sqlx::query(
            "UPDATE sessions SET revoked_at = COALESCE(revoked_at, now()), updated_at = now() WHERE id = $1",
        )
        .bind(principal.session_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, session_id, authentication_class, action,
                target_kind, target_id, outcome, metadata
            )
            VALUES (
                $1, $2, $3, 'password_session', 'authentication.logout',
                'session', $3, 'succeeded', '{}'::jsonb
            )
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(principal.actor_id)
        .bind(principal.session_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        state.realtime.revoke_session(principal.session_id);
    }

    let mut response_headers = HeaderMap::new();
    append_cookie(
        &mut response_headers,
        session_cookie(&state.config, "", true),
    )?;
    append_cookie(&mut response_headers, csrf_cookie(&state.config, "", true))?;
    Ok((StatusCode::NO_CONTENT, response_headers).into_response())
}

pub fn require_authenticated_write(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
) -> Result<(), AppError> {
    require_csrf(&state.config, headers, principal)?;
    state
        .rate_limits
        .check_authenticated_write(principal.session_id)
}

pub fn require_csrf(
    config: &AppConfig,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
) -> Result<(), AppError> {
    require_allowed_origin(config, headers)?;
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return Err(AppError::OriginRejected);
    }
    let header_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= MAX_COOKIE_VALUE_BYTES)
        .ok_or(AppError::CsrfInvalid)?;
    let cookie_token = cookie_value(headers, csrf_cookie_name(config))
        .filter(|value| !value.is_empty() && value.len() <= MAX_COOKIE_VALUE_BYTES)
        .ok_or(AppError::CsrfInvalid)?;
    if !bool::from(header_token.as_bytes().ct_eq(cookie_token.as_bytes())) {
        return Err(AppError::CsrfInvalid);
    }
    let digest = hash_secret(header_token);
    if !bool::from(digest.as_slice().ct_eq(&principal.csrf_token_hash)) {
        return Err(AppError::CsrfInvalid);
    }
    Ok(())
}

pub fn require_allowed_origin(config: &AppConfig, headers: &HeaderMap) -> Result<(), AppError> {
    let origin = headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::OriginRejected)?;
    if config
        .allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        Ok(())
    } else {
        Err(AppError::OriginRejected)
    }
}

pub fn origin_is_allowed(config: &AppConfig, headers: &HeaderMap) -> bool {
    require_allowed_origin(config, headers).is_ok()
}

pub async fn load_authenticated_actor(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<AuthenticatedActor>, AppError> {
    let Some(token) = cookie_value(headers, session_cookie_name(&state.config)) else {
        return Ok(None);
    };
    if token.is_empty() || token.len() > MAX_COOKIE_VALUE_BYTES {
        return Ok(None);
    }
    let token_hash = hash_secret(&token);
    let row = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT
            s.id AS session_id,
            u.id AS user_id,
            u.actor_id,
            a.kind AS actor_kind,
            u.login_name,
            u.display_name,
            s.csrf_token_hash
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        JOIN actors a ON a.id = u.actor_id
        WHERE s.token_hash = $1
          AND s.revoked_at IS NULL
          AND s.idle_expires_at > now()
          AND s.absolute_expires_at > now()
          AND s.authentication_epoch = u.authentication_epoch
          AND u.status = 'active'
          AND a.status = 'active'
          AND a.kind = 'human'
        "#,
    )
    .bind(token_hash.to_vec())
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    sqlx::query(
        r#"
        UPDATE sessions
        SET last_seen_at = now(),
            idle_expires_at = LEAST(now() + interval '7 days', absolute_expires_at)
        WHERE id = $1 AND last_seen_at < now() - interval '5 minutes'
        "#,
    )
    .bind(row.session_id)
    .execute(&state.pool)
    .await?;
    Ok(Some(AuthenticatedActor {
        session_id: row.session_id,
        user_id: row.user_id,
        actor_id: row.actor_id,
        actor_kind: row.actor_kind,
        login_name: row.login_name,
        display_name: row.display_name,
        csrf_token_hash: row.csrf_token_hash,
    }))
}

async fn session_response(
    state: &AppState,
    principal: Option<&AuthenticatedActor>,
) -> Result<SessionResponse, AppError> {
    let Some(principal) = principal else {
        return Ok(SessionResponse {
            authenticated: false,
            user: None,
            organizations: Vec::new(),
        });
    };
    let organizations = sqlx::query_as::<_, OrganizationSummary>(
        r#"
        SELECT o.id, o.name, o.slug, om.role
        FROM organization_memberships om
        JOIN organizations o ON o.id = om.organization_id
        WHERE om.actor_id = $1 AND om.enabled
        ORDER BY lower(o.name), o.id
        "#,
    )
    .bind(principal.actor_id)
    .fetch_all(&state.pool)
    .await?;
    let actor_kind = policy_actor_kind(&principal.actor_kind)?;
    let mut create_project = false;
    for organization in &organizations {
        let role = policy_membership_role(&organization.role)?;
        let subject = Subject::new(
            principal.actor_id,
            actor_kind,
            true,
            Some(Membership::organization(organization.id, role, true)),
            None,
        );
        if authorize(
            &subject,
            Action::OrganizationProjectCreate,
            Resource::Organization {
                organization_id: organization.id,
            },
        )
        .is_allowed()
        {
            create_project = true;
            break;
        }
    }
    Ok(SessionResponse {
        authenticated: true,
        user: Some(SessionUser {
            id: principal.user_id,
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            login_name: principal.login_name.clone(),
            display_name: principal.display_name.clone(),
            capabilities: SessionCapabilities { create_project },
        }),
        organizations,
    })
}

fn policy_actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

fn policy_membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

pub(crate) fn normalize_login_name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if !(3..=64).contains(&value.len())
        || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(AppError::InvalidCredentials);
    }
    Ok(value.to_ascii_lowercase())
}

fn random_token() -> String {
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_secret(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(COOKIE).iter().find_map(|header| {
        header.to_str().ok().and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (candidate, value) = cookie.trim().split_once('=')?;
                (candidate == name).then(|| value.to_owned())
            })
        })
    })
}

fn session_cookie_name(config: &AppConfig) -> &'static str {
    if config.environment == Environment::Development {
        SESSION_COOKIE_DEV
    } else {
        SESSION_COOKIE_SECURE
    }
}

pub(crate) fn csrf_cookie_name(config: &AppConfig) -> &'static str {
    if config.environment == Environment::Development {
        CSRF_COOKIE_DEV
    } else {
        CSRF_COOKIE_SECURE
    }
}

fn session_cookie(config: &AppConfig, value: &str, expire: bool) -> String {
    cookie(
        session_cookie_name(config),
        value,
        config.environment != Environment::Development,
        true,
        expire,
    )
}

fn csrf_cookie(config: &AppConfig, value: &str, expire: bool) -> String {
    cookie(
        csrf_cookie_name(config),
        value,
        config.environment != Environment::Development,
        false,
        expire,
    )
}

fn cookie(name: &str, value: &str, secure: bool, http_only: bool, expire: bool) -> String {
    let mut attributes = format!("{name}={value}; Path=/; SameSite=Strict");
    if http_only {
        attributes.push_str("; HttpOnly");
    }
    if secure {
        attributes.push_str("; Secure");
    }
    if expire {
        attributes.push_str("; Max-Age=0");
    } else {
        attributes.push_str("; Max-Age=2592000");
    }
    attributes
}

fn append_cookie(headers: &mut HeaderMap, value: String) -> Result<(), AppError> {
    let value = HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("construct authentication cookie"))?;
    headers.append(SET_COOKIE, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::http::{HeaderMap, HeaderValue};

    use super::{
        AuthenticatedActor, csrf_cookie, hash_secret, normalize_login_name, require_allowed_origin,
        require_csrf, session_cookie,
    };
    use crate::{config::AppConfig, error::AppError, rate_limit::RateLimiters};
    use uuid::Uuid;

    fn config() -> AppConfig {
        AppConfig::from_values(&BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://example.invalid/database".to_owned(),
        )]))
        .expect("development config")
    }

    #[test]
    fn login_names_are_bounded_ascii_and_normalized() {
        assert_eq!(
            normalize_login_name(" Mischa.N ").expect("login"),
            "mischa.n"
        );
        assert!(normalize_login_name("no").is_err());
        assert!(normalize_login_name("mischa@example.com").is_err());
        assert!(normalize_login_name("míscha").is_err());
        assert!(normalize_login_name("mischa.").is_err());
    }

    #[test]
    fn development_and_secure_cookie_attributes_are_explicit() {
        let development = config();
        let session = session_cookie(&development, "token", false);
        assert!(session.contains("HttpOnly"));
        assert!(session.contains("SameSite=Strict"));
        assert!(!session.contains("Secure"));

        let mut production_values = BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/database".to_owned(),
            ),
            ("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned()),
            (
                "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
                "https://workspace.example.com".to_owned(),
            ),
        ]);
        production_values.insert(
            "AGENT_WORKSPACE_SERVER_ADDR".to_owned(),
            "0.0.0.0:3100".to_owned(),
        );
        let production = AppConfig::from_values(&production_values).expect("production config");
        assert!(session_cookie(&production, "token", false).contains("Secure"));
        assert!(!csrf_cookie(&production, "token", false).contains("HttpOnly"));
    }

    #[test]
    fn csrf_requires_exact_origin_cookie_header_and_digest() {
        let config = config();
        let csrf = "csrf-token";
        let principal = AuthenticatedActor {
            session_id: Uuid::nil(),
            user_id: Uuid::nil(),
            actor_id: Uuid::nil(),
            actor_kind: "human".to_owned(),
            login_name: "mischa".to_owned(),
            display_name: "Mischa".to_owned(),
            csrf_token_hash: hash_secret(csrf).to_vec(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:3100"));
        headers.insert("x-csrf-token", HeaderValue::from_static(csrf));
        headers.insert(
            "cookie",
            HeaderValue::from_static("agent_workspace_csrf=csrf-token"),
        );
        assert!(require_csrf(&config, &headers, &principal).is_ok());
        headers.insert(
            "origin",
            HeaderValue::from_static("http://127.0.0.1:3100.attacker.example"),
        );
        assert!(require_allowed_origin(&config, &headers).is_err());
        assert!(require_csrf(&config, &headers, &principal).is_err());
    }

    #[test]
    fn logout_csrf_remains_valid_after_the_write_budget_is_exhausted() {
        let config = config();
        let csrf = "csrf-token";
        let principal = AuthenticatedActor {
            session_id: Uuid::now_v7(),
            user_id: Uuid::nil(),
            actor_id: Uuid::nil(),
            actor_kind: "human".to_owned(),
            login_name: "mischa".to_owned(),
            display_name: "Mischa".to_owned(),
            csrf_token_hash: hash_secret(csrf).to_vec(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:3100"));
        headers.insert("x-csrf-token", HeaderValue::from_static(csrf));
        headers.insert(
            "cookie",
            HeaderValue::from_static("agent_workspace_csrf=csrf-token"),
        );
        let limits = RateLimiters::default();
        for _ in 0..240 {
            assert!(
                limits
                    .check_authenticated_write(principal.session_id)
                    .is_ok()
            );
        }
        assert!(matches!(
            limits.check_authenticated_write(principal.session_id),
            Err(AppError::RateLimited { .. })
        ));

        assert!(
            require_csrf(&config, &headers, &principal).is_ok(),
            "logout uses the CSRF boundary directly and remains available"
        );
    }
}
