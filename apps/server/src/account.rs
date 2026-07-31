use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    idempotency::IdempotencyKey,
    provider::{
        AccountSummary, LoginCeremonyError, LoginCeremonySnapshot, LoginKind, LoginReservation,
        LoginStart, RateLimitSummary,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account",
            get(read_account),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
            post(start_browser_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
            post(start_device_code_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
            post(login_with_api_key),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
            post(cancel_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
            post(logout),
        )
}

#[derive(Debug, FromRow)]
struct ProfileAccess {
    codex_home_path: String,
    status: String,
    account_state: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiKeyRequest {
    api_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountStatusResponse {
    profile_id: Uuid,
    account_state: String,
    provider_available: bool,
    account: Option<AccountView>,
    rate_limits: Option<RateLimitView>,
    ceremony: Option<LoginCeremonyView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    account_type: String,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitView {
    primary: Option<RateLimitWindowView>,
    secondary: Option<RateLimitWindowView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindowView {
    used_percent: u16,
    resets_at: Option<i64>,
    window_duration_minutes: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginCeremonyView {
    ceremony_id: Uuid,
    kind: &'static str,
    state: &'static str,
    expires_at_unix_seconds: u64,
    auth_url: Option<String>,
    verification_url: Option<String>,
    user_code: Option<String>,
}

async fn read_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let profile = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    let mut ceremony = state
        .supervisors
        .login_ceremony(profile_id, principal.actor_id, principal.session_id)
        .await
        .map(login_ceremony_view);
    if profile.status == "disabled" {
        return Ok(Json(AccountStatusResponse {
            profile_id,
            account_state: profile.account_state,
            provider_available: false,
            account: None,
            rate_limits: None,
            ceremony,
        }));
    }

    let provider = state
        .supervisors
        .acquire(profile_id, PathBuf::from(profile.codex_home_path))
        .await;
    let Ok(mut provider) = provider else {
        persist_account_state(&state, profile_id, "unknown", "unavailable").await?;
        return Ok(Json(AccountStatusResponse {
            profile_id,
            account_state: "unknown".to_owned(),
            provider_available: false,
            account: None,
            rate_limits: None,
            ceremony,
        }));
    };
    let account = match provider.read_account().await {
        Ok(account) => account,
        Err(error) => {
            tracing::warn!(
                %profile_id,
                error_type = std::any::type_name_of_val(&error),
                "Codex account status is unavailable"
            );
            persist_account_state(&state, profile_id, "unknown", "unavailable").await?;
            return Ok(Json(AccountStatusResponse {
                profile_id,
                account_state: "unknown".to_owned(),
                provider_available: false,
                account: None,
                rate_limits: None,
                ceremony,
            }));
        }
    };
    let (account_state, profile_status) = if account.authenticated {
        ("authenticated", "available")
    } else {
        ("authentication_required", "authentication_required")
    };
    persist_account_state(&state, profile_id, account_state, profile_status).await?;
    if account.authenticated {
        state.supervisors.clear_login(profile_id, None).await;
        ceremony = None;
    }
    let rate_limits = if account.authenticated {
        provider.read_rate_limits().await.ok().map(rate_limit_view)
    } else {
        None
    };
    Ok(Json(AccountStatusResponse {
        profile_id,
        account_state: account_state.to_owned(),
        provider_available: true,
        account: account_view(account),
        rate_limits,
        ceremony,
    }))
}

async fn start_browser_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    start_login(
        &state,
        &principal,
        &headers,
        project_id,
        profile_id,
        LoginKind::Browser,
        None,
    )
    .await
}

async fn start_device_code_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    start_login(
        &state,
        &principal,
        &headers,
        project_id,
        profile_id,
        LoginKind::DeviceCode,
        None,
    )
    .await
}

async fn login_with_api_key(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<ApiKeyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    let result = start_login(
        &state,
        &principal,
        &headers,
        project_id,
        profile_id,
        LoginKind::ApiKey,
        Some(request.api_key.as_str()),
    )
    .await;
    request.api_key.zeroize();
    result
}

#[allow(clippy::too_many_arguments)]
async fn start_login(
    state: &AppState,
    principal: &AuthenticatedActor,
    headers: &HeaderMap,
    project_id: Uuid,
    profile_id: Uuid,
    kind: LoginKind,
    api_key: Option<&str>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let idempotency_key = IdempotencyKey::from_headers(headers)?;
    let profile = require_profile_owner(state, principal, project_id, profile_id).await?;
    if profile.status == "disabled" {
        return Err(AppError::Conflict(
            "execution_profile",
            "profile is disabled",
        ));
    }
    let reservation = state
        .supervisors
        .reserve_login(
            profile_id,
            principal.actor_id,
            principal.session_id,
            idempotency_key.as_str(),
            kind,
        )
        .await
        .map_err(login_ceremony_error)?;
    let ceremony_id = match reservation {
        LoginReservation::Replay(snapshot) => return Ok(Json(login_ceremony_view(snapshot))),
        LoginReservation::Reserved(ceremony_id) => ceremony_id,
    };

    let provider = state
        .supervisors
        .acquire(profile_id, PathBuf::from(profile.codex_home_path))
        .await;
    let mut provider = match provider {
        Ok(provider) => provider,
        Err(error) => {
            state
                .supervisors
                .clear_login(profile_id, Some(ceremony_id))
                .await;
            return Err(provider_error(profile_id, "start login supervisor", error));
        }
    };
    let response = match kind {
        LoginKind::Browser => provider.start_browser_login().await,
        LoginKind::DeviceCode => provider.start_device_code_login().await,
        LoginKind::ApiKey => {
            provider
                .login_with_api_key(api_key.ok_or(AppError::Validation("apiKey"))?)
                .await
        }
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            state
                .supervisors
                .clear_login(profile_id, Some(ceremony_id))
                .await;
            return Err(provider_error(profile_id, "start login", error));
        }
    };
    let snapshot = state
        .supervisors
        .complete_login(profile_id, ceremony_id, response)
        .await
        .map_err(login_ceremony_error)?;
    let (account_state, profile_status) = if kind == LoginKind::ApiKey {
        ("authenticated", "available")
    } else {
        ("authentication_required", "authentication_required")
    };
    persist_account_state(state, profile_id, account_state, profile_status).await?;
    Ok(Json(login_ceremony_view(snapshot)))
}

async fn cancel_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let profile = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    let ceremony = state
        .supervisors
        .login_ceremony(profile_id, principal.actor_id, principal.session_id)
        .await;
    let Some(ceremony) = ceremony else {
        persist_account_state(
            &state,
            profile_id,
            "authentication_required",
            "authentication_required",
        )
        .await?;
        return Ok(Json(AccountStatusResponse {
            profile_id,
            account_state: "authentication_required".to_owned(),
            provider_available: true,
            account: None,
            rate_limits: None,
            ceremony: None,
        }));
    };
    let login_id = match ceremony.response.as_ref() {
        Some(LoginStart::Browser { login_id, .. })
        | Some(LoginStart::DeviceCode { login_id, .. }) => Some(login_id.clone()),
        Some(LoginStart::ApiKey) => None,
        None => return Err(AppError::Conflict("login_ceremony", "login is starting")),
    };
    if let Some(login_id) = login_id {
        let mut provider = state
            .supervisors
            .acquire(profile_id, PathBuf::from(profile.codex_home_path))
            .await
            .map_err(|error| provider_error(profile_id, "cancel login supervisor", error))?;
        provider
            .cancel_login(&login_id)
            .await
            .map_err(|error| provider_error(profile_id, "cancel login", error))?;
    }
    state
        .supervisors
        .clear_login(profile_id, Some(ceremony.ceremony_id))
        .await;
    persist_account_state(
        &state,
        profile_id,
        "authentication_required",
        "authentication_required",
    )
    .await?;
    Ok(Json(AccountStatusResponse {
        profile_id,
        account_state: "authentication_required".to_owned(),
        provider_available: true,
        account: None,
        rate_limits: None,
        ceremony: None,
    }))
}

async fn logout(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let profile = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    persist_account_state(&state, profile_id, "logged_out", "authentication_required").await?;
    state.supervisors.shutdown_profile(profile_id).await;
    state.supervisors.clear_login(profile_id, None).await;
    if profile.account_state == "logged_out" {
        return Ok(Json(AccountStatusResponse {
            profile_id,
            account_state: "logged_out".to_owned(),
            provider_available: false,
            account: None,
            rate_limits: None,
            ceremony: None,
        }));
    }
    let mut provider = state
        .supervisors
        .acquire(profile_id, PathBuf::from(profile.codex_home_path))
        .await
        .map_err(|error| provider_error(profile_id, "logout supervisor", error))?;
    provider
        .logout_account()
        .await
        .map_err(|error| provider_error(profile_id, "logout", error))?;
    drop(provider);
    state.supervisors.shutdown_profile(profile_id).await;
    Ok(Json(AccountStatusResponse {
        profile_id,
        account_state: "logged_out".to_owned(),
        provider_available: false,
        account: None,
        rate_limits: None,
        ceremony: None,
    }))
}

async fn require_profile_owner(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<ProfileAccess, AppError> {
    sqlx::query_as::<_, ProfileAccess>(
        r#"
        SELECT profile.codex_home_path, profile.status, profile.account_state
        FROM execution_profiles profile
        JOIN projects project ON project.id = profile.project_id
        JOIN project_memberships membership
          ON membership.project_id = profile.project_id
         AND membership.actor_id = profile.owner_actor_id
         AND membership.enabled
        JOIN provider_connections connection
          ON connection.id = profile.provider_connection_id
         AND connection.project_id = profile.project_id
         AND connection.enabled
        WHERE profile.id = $1
          AND profile.project_id = $2
          AND profile.owner_actor_id = $3
          AND project.archived_at IS NULL
          AND membership.role IN ('owner', 'editor')
        "#,
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("execution_profile"))
}

async fn persist_account_state(
    state: &AppState,
    profile_id: Uuid,
    account_state: &'static str,
    profile_status: &'static str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE execution_profiles
        SET account_state = $2,
            status = CASE WHEN status = 'disabled' THEN status ELSE $3 END,
            last_health_at = now(),
            last_failure_category = NULL,
            version = version + CASE
                WHEN account_state IS DISTINCT FROM $2
                  OR (status <> 'disabled' AND status IS DISTINCT FROM $3)
                THEN 1 ELSE 0 END,
            updated_at = CASE
                WHEN account_state IS DISTINCT FROM $2
                  OR (status <> 'disabled' AND status IS DISTINCT FROM $3)
                THEN now() ELSE updated_at END
        WHERE id = $1
        "#,
    )
    .bind(profile_id)
    .bind(account_state)
    .bind(profile_status)
    .execute(&state.pool)
    .await?;
    Ok(())
}

fn account_view(account: AccountSummary) -> Option<AccountView> {
    account.account_type.map(|account_type| AccountView {
        account_type,
        email: account.email,
    })
}

fn rate_limit_view(rate_limits: RateLimitSummary) -> RateLimitView {
    let window = |value: crate::provider::RateLimitWindow| RateLimitWindowView {
        used_percent: value.used_percent,
        resets_at: value.resets_at,
        window_duration_minutes: value.window_duration_minutes,
    };
    RateLimitView {
        primary: rate_limits.primary.map(window),
        secondary: rate_limits.secondary.map(window),
    }
}

fn login_ceremony_view(ceremony: LoginCeremonySnapshot) -> LoginCeremonyView {
    let (state, auth_url, verification_url, user_code) = match ceremony.response {
        Some(LoginStart::Browser { auth_url, .. }) => ("pending", Some(auth_url), None, None),
        Some(LoginStart::DeviceCode {
            verification_url,
            user_code,
            ..
        }) => ("pending", None, Some(verification_url), Some(user_code)),
        Some(LoginStart::ApiKey) => ("completed", None, None, None),
        None => ("starting", None, None, None),
    };
    LoginCeremonyView {
        ceremony_id: ceremony.ceremony_id,
        kind: match ceremony.kind {
            LoginKind::Browser => "browser",
            LoginKind::DeviceCode => "device_code",
            LoginKind::ApiKey => "api_key",
        },
        state,
        expires_at_unix_seconds: ceremony.expires_at_unix_seconds,
        auth_url,
        verification_url,
        user_code,
    }
}

fn login_ceremony_error(error: LoginCeremonyError) -> AppError {
    match error {
        LoginCeremonyError::Active => {
            AppError::Conflict("login_ceremony", "another login is active")
        }
        LoginCeremonyError::Capacity => {
            AppError::Conflict("login_ceremony", "host login capacity is reached")
        }
        LoginCeremonyError::NotFound => AppError::NotFound("login_ceremony"),
    }
}

fn provider_error(
    profile_id: Uuid,
    operation: &'static str,
    error: impl std::fmt::Debug,
) -> AppError {
    tracing::error!(
        %profile_id,
        operation,
        error_type = std::any::type_name_of_val(&error),
        "Codex account operation failed"
    );
    AppError::Conflict("execution_profile", "Codex account operation failed")
}
