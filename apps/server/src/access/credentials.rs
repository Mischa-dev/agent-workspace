//! K1.23: an account's long-lived secrets: made, rotated, revoked, and
//! exchanged for short-lived access tokens. The secrets live in
//! `account_credentials` (K1.22); the tokens they exchange for are
//! `access::tokens`'s.
//!
//! A secret reaches one space (`Space`, as every older person's credential
//! did) or every workspace its account is in (`Account`, ruling R19). The
//! value is shown once; only its prefix and digest are kept.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;

pub const SECRET_PREFIX: &str = "aw_sk_";
/// As the bearer resolver reads it.
const SECRET_PREFIX_LENGTH: usize = 16;
/// The longest a secret may live.
pub const MAX_DAYS: i32 = 365;
/// The longest an old value keeps working after a rotation.
pub const MAX_GRACE_MINUTES: i64 = 7 * 24 * 60;
/// How long an exchanged access token lives.
pub const EXCHANGED_MINUTES: i64 = 60;

/// How far a secret reaches.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialReach {
    /// The one space it was made in.
    Space,
    /// Every workspace the account is a member of; a call names which.
    Account,
}

impl CredentialReach {
    pub const fn stored(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::Account => "account",
        }
    }

    pub fn parse(value: &str) -> Self {
        if value == "account" {
            Self::Account
        } else {
            Self::Space
        }
    }
}

/// A secret to make.
#[derive(Clone, Debug)]
pub struct NewSecret<'a> {
    pub account_id: Uuid,
    pub home_space: Uuid,
    pub client_name: &'a str,
    /// `read`, and `write` when it may change things.
    pub scopes: Vec<String>,
    pub reach: CredentialReach,
    pub days: i32,
    /// An agent's connection the secret belongs to, when it has one.
    pub connection_id: Option<Uuid>,
    pub created_by: Uuid,
    pub request_id: Option<&'a str>,
}

/// A secret, as listed: never its value.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CredentialView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// The space it was made in.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub client_name: String,
    /// The start of the value, to tell secrets apart.
    pub token_prefix: String,
    pub scopes: Vec<String>,
    /// `space` or `account`.
    pub reach: String,
    /// `active` or `revoked`.
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// The secret that replaced it, when it was rotated.
    #[schemars(with = "Option<String>")]
    pub replaced_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const VIEW: &str = "SELECT id, actor_id AS account_id, project_id AS space_id, client_name, \
    token_prefix, scopes, reach, status, expires_at, last_used_at, \
    replaced_by_credential_id AS replaced_by, created_at FROM account_credentials";

/// A secret just made: the value is in no row, and absent on a replay.
#[derive(Clone, Debug)]
pub struct Made {
    pub credential: CredentialView,
    pub secret: Option<String>,
}

fn generate() -> (String, String, [u8; 32]) {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{SECRET_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = token[..SECRET_PREFIX_LENGTH].to_owned();
    let digest = Sha256::digest(token.as_bytes()).into();
    (token, prefix, digest)
}

/// What a secret may say it allows: always `read`; `write`; and for an
/// agent's, what its connection holds.
fn valid_scopes(scopes: &[String]) -> bool {
    (1..=4).contains(&scopes.len())
        && scopes.iter().any(|scope| scope == "read")
        && scopes.iter().all(|scope| {
            matches!(
                scope.as_str(),
                "read" | "write" | "events" | "worker_request"
            )
        })
}

/// One secret, by id.
pub async fn get(
    connection: &mut PgConnection,
    credential_id: Uuid,
) -> Result<Option<CredentialView>, AppError> {
    Ok(
        sqlx::query_as::<_, CredentialView>(&format!("{VIEW} WHERE id = $1"))
            .bind(credential_id)
            .fetch_optional(&mut *connection)
            .await?,
    )
}

/// An account's secrets, newest first (revoked ones too, for a while).
pub async fn list_for(
    connection: &mut PgConnection,
    account_id: Uuid,
) -> Result<Vec<CredentialView>, AppError> {
    Ok(sqlx::query_as::<_, CredentialView>(&format!(
        "{VIEW} WHERE actor_id = $1 \
         AND (status = 'active' OR revoked_at > now() - interval '30 days') \
         ORDER BY created_at DESC, id DESC LIMIT 100"
    ))
    .bind(account_id)
    .fetch_all(&mut *connection)
    .await?)
}

/// Make a secret. With a request id already used by the same maker, the
/// first secret is answered again without its value.
pub async fn create_secret(
    connection: &mut PgConnection,
    secret: NewSecret<'_>,
) -> Result<Made, AppError> {
    if let Some(request_id) = secret.request_id {
        let made = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM account_credentials WHERE created_by_actor_id = $1 AND request_id = $2",
        )
        .bind(secret.created_by)
        .bind(request_id)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(id) = made {
            let credential = get(connection, id)
                .await?
                .ok_or(AppError::NotFound("credential"))?;
            return Ok(Made {
                credential,
                secret: None,
            });
        }
    }
    let name = secret.client_name.trim();
    if name != secret.client_name
        || !(1..=80).contains(&name.chars().count())
        || !name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-'))
    {
        return Err(AppError::Refused {
            field: "name",
            message: "Name it with 1 to 80 letters, digits, spaces, dots, dashes or underscores.",
        });
    }
    if !valid_scopes(&secret.scopes) {
        return Err(AppError::Refused {
            field: "scopes",
            message: "Give it \"read\", and \"write\" if it may change things.",
        });
    }
    if !(1..=MAX_DAYS).contains(&secret.days) {
        return Err(AppError::Refused {
            field: "days",
            message: "A secret lives between 1 and 365 days.",
        });
    }
    let (token, prefix, digest) = generate();
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO account_credentials (
            id, project_id, actor_id, client_name, token_prefix, token_digest, scopes,
            expires_at, external_agent_connection_id, reach, created_by_actor_id, request_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(days => $8), $9, $10, $11, $12)
        "#,
    )
    .bind(id)
    .bind(secret.home_space)
    .bind(secret.account_id)
    .bind(name)
    .bind(&prefix)
    .bind(digest.as_slice())
    .bind(&secret.scopes)
    .bind(secret.days)
    .bind(secret.connection_id)
    .bind(secret.reach.stored())
    .bind(secret.created_by)
    .bind(secret.request_id)
    .execute(&mut *connection)
    .await?;
    let credential = get(connection, id)
        .await?
        .ok_or(AppError::NotFound("credential"))?;
    Ok(Made {
        credential,
        secret: Some(token),
    })
}

/// A rotation: the new secret, and until when the old value still works.
#[derive(Clone, Debug)]
pub struct Rotated {
    pub made: Made,
    pub old_secret_valid_until: Option<DateTime<Utc>>,
}

/// Replace a secret with a new value. The old value keeps working for
/// `grace_minutes` (0: it stops at once), so whatever holds it can move.
pub async fn rotate(
    connection: &mut PgConnection,
    credential_id: Uuid,
    grace_minutes: i64,
    days: Option<i32>,
    rotated_by: Uuid,
) -> Result<Rotated, AppError> {
    if !(0..=MAX_GRACE_MINUTES).contains(&grace_minutes) {
        return Err(AppError::Refused {
            field: "graceMinutes",
            message: "Keep the old value working for 0 minutes to 7 days.",
        });
    }
    #[derive(FromRow)]
    struct Old {
        project_id: Uuid,
        actor_id: Uuid,
        client_name: String,
        scopes: Vec<String>,
        reach: String,
        external_agent_connection_id: Option<Uuid>,
        days_left: i32,
    }
    let old = sqlx::query_as::<_, Old>(
        "SELECT project_id, actor_id, client_name, scopes, reach, external_agent_connection_id, \
                GREATEST(1, CEIL(EXTRACT(EPOCH FROM expires_at - now()) / 86400))::int AS days_left \
         FROM account_credentials \
         WHERE id = $1 AND status = 'active' AND replaced_by_credential_id IS NULL \
           AND expires_at > now() \
         FOR UPDATE",
    )
    .bind(credential_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("credential"))?;
    let made = create_secret(
        connection,
        NewSecret {
            account_id: old.actor_id,
            home_space: old.project_id,
            client_name: &old.client_name,
            scopes: old.scopes,
            reach: CredentialReach::parse(&old.reach),
            days: days.unwrap_or(old.days_left.min(MAX_DAYS)),
            connection_id: old.external_agent_connection_id,
            created_by: rotated_by,
            request_id: None,
        },
    )
    .await?;
    let old_secret_valid_until = if grace_minutes == 0 {
        sqlx::query(
            "UPDATE account_credentials SET status = 'revoked', revoked_at = now(), \
             replaced_by_credential_id = $2, version = version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(credential_id)
        .bind(made.credential.id)
        .execute(&mut *connection)
        .await?;
        revoke_tokens_of(connection, credential_id).await?;
        None
    } else {
        Some(
            sqlx::query_scalar::<_, DateTime<Utc>>(
                "UPDATE account_credentials SET replaced_by_credential_id = $2, \
                 expires_at = LEAST(expires_at, now() + make_interval(mins => $3::int)), \
                 version = version + 1, updated_at = now() \
                 WHERE id = $1 RETURNING expires_at",
            )
            .bind(credential_id)
            .bind(made.credential.id)
            .bind(grace_minutes as i32)
            .fetch_one(&mut *connection)
            .await?,
        )
    };
    Ok(Rotated {
        made,
        old_secret_valid_until,
    })
}

/// A secret stops working, and so does every token exchanged from it.
pub async fn revoke(connection: &mut PgConnection, credential_id: Uuid) -> Result<bool, AppError> {
    let revoked = sqlx::query(
        "UPDATE account_credentials SET status = 'revoked', revoked_at = now(), \
         version = version + 1, updated_at = now() WHERE id = $1 AND status = 'active'",
    )
    .bind(credential_id)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        > 0;
    revoke_tokens_of(connection, credential_id).await?;
    Ok(revoked)
}

async fn revoke_tokens_of(
    connection: &mut PgConnection,
    credential_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE access_tokens SET revoked_at = now() WHERE credential_id = $1 AND revoked_at IS NULL",
    )
    .bind(credential_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
