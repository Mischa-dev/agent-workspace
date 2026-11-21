//! A1.17: the built-in host as its own program (`agent-workspace-host`).
//!
//! Embedded, the built-in host calls the server's router in process and
//! marks its calls with [`BuiltinCaller`] (see `transport`). As its own
//! program it calls the server over HTTP instead, with the install's
//! built-in host token: one credential for every workspace's built-in
//! `hosts` row, since the program serves them all (ruling R24).
//!
//! The token comes from a one-time code: the server writes the code to a
//! file both programs can read (`enrol-builtin-host --code-file`), and the
//! host exchanges it on its first start (`POST /api/v1/host/builtin/enrol`)
//! and keeps the token in its own data folder. Only digests are stored.
//! Writing a new code revokes the token before it, so a lost host is cut
//! off by enrolling its replacement.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::transport::BuiltinCaller;
use crate::{AppState, error::AppError, hosts::protocol::HostError};

/// The install token's prefix: told apart from a host's own (`awh_`).
pub const TOKEN_PREFIX: &str = "awbi_";
const TOKEN_PREFIX_LENGTH: usize = 12;
const CODE_PREFIX: &str = "awbc_";
/// How long a written code may wait for the host's first start.
const CODE_HOURS: i32 = 24;

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn random(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// A new one-time code for the host program, shown once. Every earlier
/// code and token stops working.
pub async fn new_code(pool: &sqlx::PgPool) -> Result<String, AppError> {
    let code = random(CODE_PREFIX);
    let mut transaction = crate::db::scoped_tx(pool).await?;
    sqlx::query(
        "UPDATE builtin_host_installs SET revoked_at = now() \
         WHERE revoked_at IS NULL",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO builtin_host_installs (id, code_hash, code_expires_at) \
         VALUES ($1, $2, now() + make_interval(hours => $3))",
    )
    .bind(Uuid::now_v7())
    .bind(digest(&code).as_slice())
    .bind(CODE_HOURS)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(code)
}

/// Where the host program reads its code: `AGENT_WORKSPACE_HOST_CODE_PATH`,
/// or `<data root>/builtin-host/code`, a folder the server and the host
/// share on a self-hosted install.
pub fn code_path(config: &crate::config::AppConfig) -> std::path::PathBuf {
    std::env::var_os("AGENT_WORKSPACE_HOST_CODE_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.data_root.join("builtin-host").join("code"))
}

/// What [`prepare`] found.
#[derive(Debug, Eq, PartialEq)]
pub enum Prepared {
    /// A host program has enrolled; nothing to do.
    Enrolled,
    /// A code written earlier still waits at this path.
    Waiting(std::path::PathBuf),
    /// A new code was written to this path.
    Written(std::path::PathBuf),
}

/// Automatic enrolment on a self-hosted install (A1.17): a server that runs
/// with the built-in host off writes a code where the host program reads
/// it, unless a host has enrolled or a code that still works waits there.
/// Servers starting together write one code between them.
pub async fn prepare(pool: &sqlx::PgPool, path: &std::path::Path) -> Result<Prepared, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    // One server at a time decides, so a second can't revoke the code the
    // first is writing.
    sqlx::query("LOCK TABLE builtin_host_installs IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await?;
    let (enrolled, waiting) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT \
           EXISTS (SELECT 1 FROM builtin_host_installs \
                   WHERE enrolled_at IS NOT NULL AND revoked_at IS NULL), \
           EXISTS (SELECT 1 FROM builtin_host_installs \
                   WHERE enrolled_at IS NULL AND revoked_at IS NULL \
                     AND code_expires_at > now() + interval '1 hour')",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if enrolled {
        return Ok(Prepared::Enrolled);
    }
    if waiting && path.exists() {
        return Ok(Prepared::Waiting(path.to_owned()));
    }
    let code = random(CODE_PREFIX);
    sqlx::query("UPDATE builtin_host_installs SET revoked_at = now() WHERE revoked_at IS NULL")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO builtin_host_installs (id, code_hash, code_expires_at) \
         VALUES ($1, $2, now() + make_interval(hours => $3))",
    )
    .bind(Uuid::now_v7())
    .bind(digest(&code).as_slice())
    .bind(CODE_HOURS)
    .execute(&mut *transaction)
    .await?;
    super::program::write_private(path, &code)
        .map_err(|_| AppError::Internal("write the built-in host's code"))?;
    transaction.commit().await?;
    Ok(Prepared::Written(path.to_owned()))
}

/// The host's first start: its code for the install's token, once.
pub async fn enrol(pool: &sqlx::PgPool, code: &str) -> Result<String, HostError> {
    let code = code.trim();
    if !code.starts_with(CODE_PREFIX) || code.len() > 128 {
        return Err(HostError::CodeRefused);
    }
    let token = random(TOKEN_PREFIX);
    let prefix = token[..TOKEN_PREFIX_LENGTH].to_owned();
    let enrolled = sqlx::query(
        r#"
        UPDATE builtin_host_installs
        SET token_prefix = $2, token_digest = $3, enrolled_at = now(), last_seen_at = now()
        WHERE code_hash = $1 AND enrolled_at IS NULL AND revoked_at IS NULL
          AND code_expires_at > now()
        "#,
    )
    .bind(digest(code).as_slice())
    .bind(&prefix)
    .bind(digest(&token).as_slice())
    .execute(crate::db::scoped(pool))
    .await
    .map_err(AppError::from)?
    .rows_affected();
    if enrolled == 1 {
        Ok(token)
    } else {
        Err(HostError::CodeRefused)
    }
}

/// Whether `token` is the install's live built-in host token.
pub async fn is_install_token(pool: &sqlx::PgPool, token: &str) -> Result<bool, AppError> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        return Ok(false);
    }
    let Some(prefix) = token.get(..TOKEN_PREFIX_LENGTH) else {
        return Ok(false);
    };
    let stored = sqlx::query_scalar::<_, Vec<u8>>(
        "UPDATE builtin_host_installs SET last_seen_at = now() \
         WHERE token_prefix = $1 AND revoked_at IS NULL \
         RETURNING token_digest",
    )
    .bind(prefix)
    .fetch_optional(crate::db::scoped(pool))
    .await?;
    let candidate = digest(token);
    Ok(stored.is_some_and(|stored| {
        stored.len() == candidate.len() && bool::from(stored.as_slice().ct_eq(&candidate))
    }))
}

/// On the host routes: whether a call is the built-in host's. In process
/// it carries [`BuiltinCaller`]; as its own program it carries the
/// install's token, which is checked here. A call with any other token goes
/// on to be checked as a host's own.
#[derive(Clone, Copy, Debug)]
pub struct BuiltinCall(bool);

impl BuiltinCall {
    pub fn is_some(self) -> bool {
        self.0
    }

    pub fn is_none(self) -> bool {
        !self.0
    }
}

impl axum::extract::FromRequestParts<AppState> for BuiltinCall {
    type Rejection = HostError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if parts.extensions.get::<BuiltinCaller>().is_some() {
            return Ok(Self(true));
        }
        let Some(token) = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| token.starts_with(TOKEN_PREFIX))
        else {
            return Ok(Self(false));
        };
        state.rate_limits.check_mcp_auth(token).await?;
        if is_install_token(&state.pool, token).await? {
            Ok(Self(true))
        } else {
            Err(HostError::Unauthorized)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_tokens_are_told_apart() {
        let code = random(CODE_PREFIX);
        let token = random(TOKEN_PREFIX);
        assert!(code.starts_with(CODE_PREFIX) && !code.starts_with(TOKEN_PREFIX));
        assert!(token.starts_with(TOKEN_PREFIX));
        assert!(!token.starts_with(crate::hosts::TOKEN_PREFIX));
        assert_eq!(token[..TOKEN_PREFIX_LENGTH].len(), TOKEN_PREFIX_LENGTH);
        assert_ne!(random(TOKEN_PREFIX), token);
    }
}
