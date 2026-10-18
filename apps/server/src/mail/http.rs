//! Settings › Workspace › Email: how the workspace sends mail, for its
//! owners. Reading never returns the password, only whether one is set.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use zeroize::Zeroize;

use super::{MailError, Message, Provider, SMTP_PASSWORD_SECRET, normalize_address, smtp};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    secrets::{self, NewSecret, SecretOwner, SecretValue},
    workspace::{WorkspaceRole, caller_role},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/mail",
            get(read_http).put(save_http).delete(remove_http),
        )
        .route(
            "/api/v1/organizations/{organization_id}/mail/test",
            post(test_http),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastTest {
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSettingsView {
    pub configured: bool,
    pub enabled: bool,
    pub provider: Option<&'static str>,
    pub smtp: Option<smtp::Settings>,
    pub from_address: Option<String>,
    pub from_name: Option<String>,
    pub password_set: bool,
    pub last_test: Option<LastTest>,
    pub updated_at: Option<DateTime<Utc>>,
    /// The caller's confirmed address, where a test goes by default.
    pub your_address: Option<String>,
}

/// Owners only: mail settings send as the workspace, and their password
/// reaches outside the server.
async fn require_owner(
    state: &AppState,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<(), AppError> {
    let mut connection = state.pool.acquire().await?;
    match caller_role(&mut connection, principal, organization_id, false).await? {
        WorkspaceRole::Owner => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

pub(crate) async fn view(
    state: &AppState,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<MailSettingsView, AppError> {
    let row = super::settings_row(&state.pool, organization_id).await?;
    let password_set = secrets::find(
        &state.pool,
        SecretOwner::Workspace { organization_id },
        SMTP_PASSWORD_SECRET,
    )
    .await?
    .is_some();
    let your_address = super::verified_address_for_actor(&state.pool, actor_id).await?;
    let Some(row) = row else {
        return Ok(MailSettingsView {
            configured: false,
            enabled: false,
            provider: None,
            smtp: None,
            from_address: None,
            from_name: None,
            password_set,
            last_test: None,
            updated_at: None,
            your_address,
        });
    };
    let provider = Provider::parse(&row.provider, &row.provider_config);
    Ok(MailSettingsView {
        configured: true,
        enabled: row.enabled,
        provider: provider.as_ref().map(Provider::name),
        smtp: provider.map(|Provider::Smtp(settings)| settings),
        from_address: Some(row.from_address),
        from_name: row.from_name,
        password_set,
        last_test: row.last_test_at.map(|at| LastTest {
            at,
            ok: row.last_test_ok.unwrap_or(false),
            error: row.last_test_error,
        }),
        updated_at: Some(row.updated_at),
        your_address,
    })
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<MailSettingsView>, AppError> {
    require_owner(&state, &principal, organization_id).await?;
    Ok(Json(
        view(&state, organization_id, principal.actor_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRequest {
    provider: String,
    smtp: smtp::Settings,
    from_address: String,
    from_name: Option<String>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    /// A new password; left out keeps the stored one.
    password: Option<String>,
    /// Forget the stored password (a relay that needs none).
    #[serde(default)]
    clear_password: bool,
}

impl Drop for SaveRequest {
    fn drop(&mut self) {
        if let Some(password) = &mut self.password {
            password.zeroize();
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

async fn save_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(mut request): ApiJson<SaveRequest>,
) -> Result<Json<MailSettingsView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    require_owner(&state, &principal, organization_id).await?;
    if request.provider != "smtp" {
        return Err(AppError::Validation("provider"));
    }
    request.smtp.host = request.smtp.host.trim().to_owned();
    request.smtp.username = request
        .smtp
        .username
        .take()
        .map(|username| username.trim().to_owned())
        .filter(|username| !username.is_empty());
    request.smtp.validate().map_err(AppError::Validation)?;
    let from_address =
        normalize_address(&request.from_address).ok_or(AppError::Validation("fromAddress"))?;
    let from_name = request
        .from_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    if from_name
        .as_deref()
        .is_some_and(|name| name.chars().count() > 120 || name.chars().any(char::is_control))
    {
        return Err(AppError::Validation("fromName"));
    }
    let password = request
        .password
        .take()
        .filter(|password| !password.is_empty());
    if password
        .as_ref()
        .is_some_and(|password| password.len() > 1024)
    {
        return Err(AppError::Validation("password"));
    }
    let config =
        serde_json::to_value(&request.smtp).map_err(|_| AppError::Internal("mail settings"))?;
    let mut transaction = state.pool.begin().await?;
    let previous = sqlx::query_scalar::<_, Value>(
        "SELECT provider_config FROM workspace_mail_settings WHERE organization_id = $1 FOR UPDATE",
    )
    .bind(organization_id)
    .fetch_optional(&mut *transaction)
    .await?;
    // The stored password only ever goes to the server it was typed for:
    // pointing the settings at another host, port or account without
    // typing it again forgets it.
    let moved = previous.is_some_and(|previous| {
        ["host", "port", "username"]
            .iter()
            .any(|field| previous.get(field) != config.get(field))
    });
    sqlx::query(
        r#"
        INSERT INTO workspace_mail_settings (
            organization_id, provider, provider_config, from_address, from_name, enabled,
            updated_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (organization_id) DO UPDATE SET
            provider = EXCLUDED.provider, provider_config = EXCLUDED.provider_config,
            from_address = EXCLUDED.from_address, from_name = EXCLUDED.from_name,
            enabled = EXCLUDED.enabled, updated_by_actor_id = EXCLUDED.updated_by_actor_id,
            last_test_at = NULL, last_test_ok = NULL, last_test_error = NULL,
            updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(&request.provider)
    .bind(&config)
    .bind(&from_address)
    .bind(&from_name)
    .bind(request.enabled)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    let owner = SecretOwner::Workspace { organization_id };
    if let Some(password) = password {
        let keys = secrets::keyring(&state.config).map_err(|_| AppError::SecretsNotConfigured)?;
        secrets::put(
            &mut transaction,
            keys.as_ref(),
            NewSecret {
                owner,
                name: SMTP_PASSWORD_SECRET,
                description: Some("Signs this workspace in to its mail server."),
                value: &SecretValue::new(password),
                actor_id: Some(principal.actor_id),
                system_managed: true,
            },
        )
        .await?;
    } else if (request.clear_password || moved)
        && let Some(id) = secrets::find(&mut *transaction, owner, SMTP_PASSWORD_SECRET).await?
    {
        secrets::delete(&mut *transaction, id).await?;
    }
    crate::workspace::audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace.mail.update",
        "organization",
        Some(organization_id),
        serde_json::json!({
            "provider": request.provider,
            "enabled": request.enabled,
            "security": request.smtp.security,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(
        view(&state, organization_id, principal.actor_id).await?,
    ))
}

async fn remove_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    require_owner(&state, &principal, organization_id).await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("DELETE FROM workspace_mail_settings WHERE organization_id = $1")
        .bind(organization_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(id) = secrets::find(
        &mut *transaction,
        SecretOwner::Workspace { organization_id },
        SMTP_PASSWORD_SECRET,
    )
    .await?
    {
        secrets::delete(&mut *transaction, id).await?;
    }
    crate::workspace::audit(
        &mut transaction,
        &headers,
        &principal,
        organization_id,
        "workspace.mail.remove",
        "organization",
        Some(organization_id),
        serde_json::json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestRequest {
    /// Where to send it; left out, the caller's confirmed address.
    to: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResponse {
    ok: bool,
    sent_to: String,
    error: Option<String>,
}

/// Send a test message now and say what happened.
async fn test_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<TestRequest>,
) -> Result<Json<TestResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    require_owner(&state, &principal, organization_id).await?;
    let Some(row) = super::settings_row(&state.pool, organization_id).await? else {
        return Err(AppError::Conflict("mail", "mail isn't set up yet"));
    };
    if row
        .last_test_at
        .is_some_and(|at| Utc::now() - at < chrono::Duration::seconds(5))
    {
        return Err(AppError::rate_limited(5));
    }
    let to = match request
        .to
        .as_deref()
        .map(str::trim)
        .filter(|to| !to.is_empty())
    {
        Some(to) => normalize_address(to).ok_or(AppError::Validation("to"))?,
        None => super::verified_address_for_actor(&state.pool, principal.actor_id)
            .await?
            .ok_or(AppError::Validation("to"))?,
    };
    let workspace_name =
        sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
            .bind(organization_id)
            .fetch_one(&state.pool)
            .await?;
    let transport = super::load(&state, row).await?;
    let result = super::send(
        &state,
        &transport,
        &Message {
            to: to.clone(),
            subject: format!("Test email from {workspace_name}"),
            text: format!(
                "This is a test from {workspace_name} on {}.\n\nIf you can read this, the workspace can send email: password resets, address confirmations and the notifications people choose to get by email.\n",
                crate::events::public_origin(&state)
            ),
        },
    )
    .await;
    let error = result
        .as_ref()
        .err()
        .map(MailError::message)
        .map(str::to_owned);
    sqlx::query(
        r#"
        UPDATE workspace_mail_settings
        SET last_test_at = now(), last_test_ok = $2, last_test_error = $3
        WHERE organization_id = $1
        "#,
    )
    .bind(organization_id)
    .bind(error.is_none())
    .bind(
        error
            .as_deref()
            .map(|error| error.chars().take(400).collect::<String>()),
    )
    .execute(&state.pool)
    .await?;
    Ok(Json(TestResponse {
        ok: error.is_none(),
        sent_to: to,
        error,
    }))
}
