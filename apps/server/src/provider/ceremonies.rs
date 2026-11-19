//! Provider sign-ins in progress (K5.14), in the database so any instance
//! can answer the page's next poll, a replayed start or a cancel: one row
//! per provider account, for ten minutes at most. A row keeps what the
//! person is shown (an address, a device code), never a key or token.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::adapter::{LoginKind, LoginStart};
use crate::error::AppError;

/// How many sign-ins may be open at once, server-wide.
const MAX_LOGIN_CEREMONIES: i64 = 32;
pub const LOGIN_CEREMONY_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub struct LoginCeremonySnapshot {
    pub ceremony_id: Uuid,
    pub kind: LoginKind,
    pub response: Option<LoginStart>,
    pub expires_at_unix_seconds: u64,
}

pub enum LoginReservation {
    Reserved(Uuid),
    Replay(LoginCeremonySnapshot),
}

#[derive(Debug, thiserror::Error)]
pub enum LoginCeremonyError {
    #[error("another login ceremony is active for this account")]
    Active,
    #[error("the host login ceremony limit is reached")]
    Capacity,
    #[error("the login ceremony is not available to this session")]
    NotFound,
    #[error(transparent)]
    Store(#[from] AppError),
}

impl From<sqlx::Error> for LoginCeremonyError {
    fn from(error: sqlx::Error) -> Self {
        Self::Store(error.into())
    }
}

/// What a row keeps of the provider's answer.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Stored {
    Browser {
        login_id: String,
        auth_url: String,
    },
    DeviceCode {
        login_id: String,
        verification_url: String,
        user_code: String,
    },
    ApiKey,
}

impl From<&LoginStart> for Stored {
    fn from(start: &LoginStart) -> Self {
        match start.clone() {
            LoginStart::Browser { login_id, auth_url } => Self::Browser { login_id, auth_url },
            LoginStart::DeviceCode {
                login_id,
                verification_url,
                user_code,
            } => Self::DeviceCode {
                login_id,
                verification_url,
                user_code,
            },
            LoginStart::ApiKey => Self::ApiKey,
        }
    }
}

impl From<Stored> for LoginStart {
    fn from(stored: Stored) -> Self {
        match stored {
            Stored::Browser { login_id, auth_url } => Self::Browser { login_id, auth_url },
            Stored::DeviceCode {
                login_id,
                verification_url,
                user_code,
            } => Self::DeviceCode {
                login_id,
                verification_url,
                user_code,
            },
            Stored::ApiKey => Self::ApiKey,
        }
    }
}

fn kind_name(kind: LoginKind) -> &'static str {
    match kind {
        LoginKind::Browser => "browser",
        LoginKind::DeviceCode => "device_code",
        LoginKind::ApiKey => "api_key",
    }
}

fn kind_of(name: &str) -> LoginKind {
    match name {
        "browser" => LoginKind::Browser,
        "api_key" => LoginKind::ApiKey,
        _ => LoginKind::DeviceCode,
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    ceremony_id: Uuid,
    actor_id: Uuid,
    session_id: Uuid,
    idempotency_key: String,
    kind: String,
    response: Option<serde_json::Value>,
    expires_at_unix_seconds: i64,
}

impl Row {
    fn snapshot(&self) -> LoginCeremonySnapshot {
        LoginCeremonySnapshot {
            ceremony_id: self.ceremony_id,
            kind: kind_of(&self.kind),
            response: self
                .response
                .clone()
                .and_then(|value| serde_json::from_value::<Stored>(value).ok())
                .map(LoginStart::from),
            expires_at_unix_seconds: u64::try_from(self.expires_at_unix_seconds).unwrap_or(0),
        }
    }
}

const COLUMNS: &str = "ceremony_id, actor_id, session_id, idempotency_key, kind, response, \
    extract(epoch FROM expires_at)::bigint AS expires_at_unix_seconds";

/// Starts a sign-in for `account_id`, or answers the one this session
/// already started with the same key.
pub async fn reserve_login(
    pool: &PgPool,
    account_id: Uuid,
    actor_id: Uuid,
    session_id: Uuid,
    idempotency_key: &str,
    kind: LoginKind,
) -> Result<LoginReservation, LoginCeremonyError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    // One reservation at a time, server-wide, so the cap holds.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('provider-login-ceremonies', 0))")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM provider_login_ceremonies WHERE expires_at <= now()")
        .execute(&mut *transaction)
        .await?;
    let existing = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM provider_login_ceremonies WHERE account_id = $1"
    ))
    .bind(account_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(existing) = existing {
        if existing.actor_id == actor_id
            && existing.session_id == session_id
            && existing.idempotency_key == idempotency_key
            && kind_of(&existing.kind) == kind
            && existing.response.is_some()
        {
            return Ok(LoginReservation::Replay(existing.snapshot()));
        }
        return Err(LoginCeremonyError::Active);
    }
    let open = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_login_ceremonies")
        .fetch_one(&mut *transaction)
        .await?;
    if open >= MAX_LOGIN_CEREMONIES {
        return Err(LoginCeremonyError::Capacity);
    }
    let ceremony_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO provider_login_ceremonies (
            account_id, workspace_id, ceremony_id, actor_id, session_id,
            idempotency_key, kind, expires_at
        )
        SELECT id, organization_id, $2, $3, $4, $5, $6, now() + make_interval(secs => $7)
        FROM provider_accounts WHERE id = $1
        "#,
    )
    .bind(account_id)
    .bind(ceremony_id)
    .bind(actor_id)
    .bind(session_id)
    .bind(idempotency_key)
    .bind(kind_name(kind))
    .bind(LOGIN_CEREMONY_LIFETIME.as_secs_f64())
    .execute(&mut *transaction)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(LoginCeremonyError::NotFound);
    }
    transaction.commit().await?;
    Ok(LoginReservation::Reserved(ceremony_id))
}

/// Records what the provider answered the start with.
pub async fn complete_login(
    pool: &PgPool,
    account_id: Uuid,
    ceremony_id: Uuid,
    response: LoginStart,
) -> Result<LoginCeremonySnapshot, LoginCeremonyError> {
    let stored = serde_json::to_value(Stored::from(&response))
        .map_err(|_| AppError::Internal("record a sign-in"))?;
    sqlx::query_as::<_, Row>(&format!(
        r#"
        UPDATE provider_login_ceremonies SET response = $3
        WHERE account_id = $1 AND ceremony_id = $2 AND expires_at > now()
        RETURNING {COLUMNS}
        "#
    ))
    .bind(account_id)
    .bind(ceremony_id)
    .bind(stored)
    .fetch_optional(crate::db::scoped(pool))
    .await?
    .map(|row| row.snapshot())
    .ok_or(LoginCeremonyError::NotFound)
}

/// The sign-in this session started for the account, if it is still open.
pub async fn login_ceremony(
    pool: &PgPool,
    account_id: Uuid,
    actor_id: Uuid,
    session_id: Uuid,
) -> Result<Option<LoginCeremonySnapshot>, AppError> {
    Ok(sqlx::query_as::<_, Row>(&format!(
        r#"
        SELECT {COLUMNS} FROM provider_login_ceremonies
        WHERE account_id = $1 AND actor_id = $2 AND session_id = $3 AND expires_at > now()
        "#
    ))
    .bind(account_id)
    .bind(actor_id)
    .bind(session_id)
    .fetch_optional(crate::db::scoped(pool))
    .await?
    .map(|row| row.snapshot()))
}

/// Ends the account's sign-in: that one, or any.
pub async fn clear_login(
    pool: &PgPool,
    account_id: Uuid,
    ceremony_id: Option<Uuid>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM provider_login_ceremonies
        WHERE account_id = $1 AND ($2::uuid IS NULL OR ceremony_id = $2)
        "#,
    )
    .bind(account_id)
    .bind(ceremony_id)
    .execute(crate::db::scoped(pool))
    .await?;
    Ok(())
}
