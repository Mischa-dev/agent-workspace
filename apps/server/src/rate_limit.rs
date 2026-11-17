//! The server's request budgets, by name. Each is a bucket in the shared
//! limiter ([`crate::limits`], K5.6): counted in PostgreSQL, so every
//! instance spends from one budget and a restart forgets nothing. The
//! password-hashing permits stay per instance (they bound this process's
//! CPU, not anyone's budget).

use std::{sync::Arc, time::Duration};

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::{
    error::AppError,
    limits::{Bucket, LimitKey, Limits, Recent, Tier},
};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3_600);
const DAY: Duration = Duration::from_secs(24 * 3_600);

const fn exact(name: &'static str, limit: u32, window: Duration) -> Bucket {
    Bucket {
        name,
        limit,
        window,
        tier: Tier::Exact,
    }
}

const fn chunked(name: &'static str, limit: u32, window: Duration, chunk: u32) -> Bucket {
    Bucket {
        name,
        limit,
        window,
        tier: Tier::Chunked { chunk },
    }
}

pub static LOGIN: Bucket = exact("login", 10, MINUTE);
// The app is reachable from the internet, so cap slow password guessing too.
pub static LOGIN_HOURLY: Bucket = exact("login.hourly", 30, HOUR);
/// Sign-in attempts from one sender address, checked before the shared
/// ceiling so one script can't use up everyone's.
pub static LOGIN_SENDERS: Bucket = exact("login.address", 20, MINUTE);
/// Failed sign-ins across every name. While they're spent, only names that
/// signed in lately get through to their own limits, so junk names can't
/// lock everyone out.
pub static LOGIN_FAILURES: Bucket = exact("login.failures", 60, MINUTE);
/// How long a name that signed in counts as known while failures flood in.
pub static RECENT_LOGINS: Recent = Recent {
    name: "login.recent",
    ttl: Duration::from_secs(7 * 24 * 3_600),
};
pub static WRITES: Bucket = chunked("writes", 240, MINUTE, 10);
pub static EXPENSIVE_READS: Bucket = chunked("reads.expensive", 120, MINUTE, 10);
pub static INVITATIONS: Bucket = exact("invitations", 20, MINUTE);
pub static INVITATIONS_GLOBAL: Bucket = exact("invitations.global", 120, MINUTE);
pub static MCP_REQUESTS: Bucket = chunked("mcp.requests", 600, MINUTE, 10);
pub static MCP_AUTH: Bucket = exact("mcp.auth", 600, MINUTE);
/// Run reports (A1.3) from one account: 20 a second on average, in bursts
/// of up to 100.
pub static RUN_REPORTS: Bucket = chunked("runs.reports", 100, Duration::from_secs(5), 5);
/// Tokens that didn't resolve, across every caller. Valid tokens never
/// spend it; while it's spent, only tokens that authenticated lately are
/// looked up.
pub static MCP_AUTH_FAILURES: Bucket = exact("mcp.auth.failures", 900, MINUTE);
/// Access tokens last twelve hours, so a day covers every app in use.
pub static RECENT_MCP_TOKENS: Recent = Recent {
    name: "mcp.recent",
    ttl: DAY,
};
pub static OAUTH_CEREMONIES: Bucket = exact("oauth.ceremonies", 30, MINUTE);
pub static OAUTH_CEREMONIES_GLOBAL: Bucket = exact("oauth.ceremonies.global", 240, MINUTE);
/// App registrations from one sender address, when the address is known.
pub static OAUTH_REGISTRATION_SENDERS: Bucket = exact("oauth.registrations.address", 20, MINUTE);
/// Token refreshes have their own budget: one app running many sessions on a
/// grant refreshes from all of them at once, and a refused refresh drops the
/// app's tools, so it mustn't share the sign-in ceremonies' smaller one.
pub static OAUTH_REFRESHES: Bucket = exact("oauth.refreshes", 300, MINUTE);
pub static OAUTH_REFRESHES_GLOBAL: Bucket = exact("oauth.refreshes.global", 1_200, MINUTE);
/// Refreshes whose token didn't check out, per sender address and in all.
/// These are spent only by bad refreshes, so junk can't use up the budget
/// real apps need; the global one keeps junk from reaching the database
/// without end.
pub static OAUTH_REFRESH_FAILURES: Bucket = exact("oauth.refresh_failures.address", 30, MINUTE);
pub static OAUTH_REFRESH_FAILURES_GLOBAL: Bucket =
    exact("oauth.refresh_failures.global", 600, MINUTE);
/// Bad refreshes naming one app, which still reach the database while the
/// global budget is spent if that app got tokens lately. The client ID isn't
/// proof the request came from the app, so this keeps that way in bounded:
/// a real app's own refresh failures are rare, so it stays well clear.
pub static OAUTH_REFRESH_FAILURES_PER_CLIENT: Bucket =
    exact("oauth.refresh_failures.client", 60, MINUTE);
/// While bad refreshes flood in from everywhere, an app that got tokens this
/// recently still gets through. Access tokens last twelve hours, so a day
/// covers every app that's in use.
pub static RECENT_OAUTH_CLIENTS: Recent = Recent {
    name: "oauth.recent",
    ttl: DAY,
};
// WorkOS sign-in starts have their own budget so they can neither starve nor
// be starved by password logins and MCP OAuth ceremonies.
pub static WORKOS_STARTS: Bucket = exact("workos.starts", 20, MINUTE);
pub static WORKOS_STARTS_GLOBAL: Bucket = exact("workos.starts.global", 600, MINUTE);
pub static WEBHOOK_ATTEMPTS: Bucket = exact("webhooks.attempts", 300, MINUTE);
/// Reports sent to one bug report link, and to all of them: enough for a
/// burst of real reports, not for a loop to flood a project.
pub static BUG_REPORTS: Bucket = exact("bug_reports", 30, MINUTE);
pub static BUG_REPORTS_GLOBAL: Bucket = exact("bug_reports.global", 300, MINUTE);
/// Password reset requests: a few per account an hour and a few dozen per
/// sender address, checked before a ceiling for the whole server so one
/// script can't use up everyone's. The mail itself has its own ceiling,
/// spent only when a reset email really goes out (to a real account).
pub static PASSWORD_RESETS: Bucket = exact("password_resets", 5, HOUR);
pub static PASSWORD_RESET_SENDERS: Bucket = exact("password_resets.address", 30, HOUR);
pub static PASSWORD_RESETS_GLOBAL: Bucket = exact("password_resets.global", 120, MINUTE);
pub static RESET_EMAILS: Bucket = exact("password_resets.emails", 30, MINUTE);
const CONCURRENT_PASSWORD_KDFS: usize = 2;

#[derive(Clone)]
pub struct RateLimiters {
    limits: Limits,
    password_kdfs: Arc<Semaphore>,
}

impl RateLimiters {
    /// The shared limiter these budgets are counted in.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Counts in `pool` from now on (the app's router does this once).
    pub fn attach(&self, pool: &PgPool) {
        self.limits.attach(pool);
    }

    /// A sign-in attempt for `normalized_login`, charged to the name. While
    /// failed sign-ins everywhere have spent the shared budget, only a name
    /// that signed in lately gets through; everyone else waits.
    pub async fn check_login(&self, normalized_login: &str) -> Result<(), AppError> {
        let key = digest(normalized_login);
        if let Some(retry_after) = self
            .limits
            .spent_or_open(&LOGIN_FAILURES, LimitKey::Global)
            .await
            && !self
                .limits
                .seen(&RECENT_LOGINS, LimitKey::Secret(&key))
                .await
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.limits.check(&LOGIN, LimitKey::Secret(&key)).await?;
        self.limits
            .check(&LOGIN_HOURLY, LimitKey::Secret(&key))
            .await
    }

    /// [`Self::check_login`] inside the caller's transaction (a second
    /// factor, checked while its challenge row is locked).
    pub async fn check_login_on(
        &self,
        connection: &mut PgConnection,
        normalized_login: &str,
    ) -> Result<(), AppError> {
        let key = digest(normalized_login);
        if let Some(retry_after) = self
            .limits
            .spent_or_open_on(connection, &LOGIN_FAILURES, LimitKey::Global)
            .await
            && !self
                .limits
                .seen_on(connection, &RECENT_LOGINS, LimitKey::Secret(&key))
                .await
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.limits
            .check_on(connection, &LOGIN, LimitKey::Secret(&key))
            .await?;
        self.limits
            .check_on(connection, &LOGIN_HOURLY, LimitKey::Secret(&key))
            .await
    }

    /// A sign-in whose password (or code) didn't check out.
    pub async fn note_login_failure(&self) {
        // A spent budget refuses unknown names next time; nothing more to do.
        self.limits.note(&LOGIN_FAILURES, LimitKey::Global).await;
    }

    /// A name that just signed in.
    pub async fn note_login_success(&self, normalized_login: &str) {
        self.limits
            .remember(&RECENT_LOGINS, LimitKey::Secret(&digest(normalized_login)))
            .await;
    }

    /// [`Self::note_login_success`] inside the caller's transaction.
    pub async fn note_login_success_on(
        &self,
        connection: &mut PgConnection,
        normalized_login: &str,
    ) {
        self.limits
            .remember_on(
                connection,
                &RECENT_LOGINS,
                LimitKey::Secret(&digest(normalized_login)),
            )
            .await;
    }

    /// A sign-in from `address` (the caller's, when known), before
    /// [`Self::check_login`].
    pub async fn check_login_sender(&self, address: Option<&str>) -> Result<(), AppError> {
        match address {
            Some(address) => {
                self.limits
                    .check(&LOGIN_SENDERS, LimitKey::Address(address))
                    .await
            }
            None => Ok(()),
        }
    }

    pub fn try_acquire_password_kdf(&self) -> Result<OwnedSemaphorePermit, AppError> {
        Arc::clone(&self.password_kdfs)
            .try_acquire_owned()
            .map_err(|_| AppError::rate_limited(1))
    }

    pub async fn check_authenticated_write(&self, session_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&WRITES, LimitKey::Session(session_id))
            .await
    }

    pub async fn check_expensive_read(&self, session_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&EXPENSIVE_READS, LimitKey::Session(session_id))
            .await
    }

    pub async fn check_invitation(&self, token_prefix: &str) -> Result<(), AppError> {
        self.limits
            .check(&INVITATIONS_GLOBAL, LimitKey::Global)
            .await?;
        self.limits
            .check(&INVITATIONS, LimitKey::Secret(&digest(token_prefix)))
            .await
    }

    pub async fn check_mcp_request(&self, credential_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&MCP_REQUESTS, LimitKey::Credential(credential_id))
            .await
    }

    /// One run report (run_open, run_update, run_activity and the like)
    /// from `account_id`.
    pub async fn check_run_report(&self, account_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&RUN_REPORTS, LimitKey::Account(account_id))
            .await
    }

    /// Before a bearer token is looked up: charged to the token. While
    /// tokens that didn't resolve have spent the shared budget, only a token
    /// that authenticated lately is looked up.
    pub async fn check_mcp_auth(&self, untrusted_token: &str) -> Result<(), AppError> {
        let key = digest(untrusted_token);
        if let Some(retry_after) = self
            .limits
            .spent_or_open(&MCP_AUTH_FAILURES, LimitKey::Global)
            .await
            && !self
                .limits
                .seen(&RECENT_MCP_TOKENS, LimitKey::Secret(&key))
                .await
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.limits.check(&MCP_AUTH, LimitKey::Secret(&key)).await
    }

    /// A bearer token that didn't resolve.
    pub async fn note_mcp_auth_failure(&self) {
        self.limits.note(&MCP_AUTH_FAILURES, LimitKey::Global).await;
    }

    /// A bearer token that resolved; only its digest is kept.
    pub async fn note_mcp_auth_success(&self, token: &str) {
        self.limits
            .remember(&RECENT_MCP_TOKENS, LimitKey::Secret(&digest(token)))
            .await;
    }

    pub async fn check_oauth_ceremony(&self, untrusted_key: &str) -> Result<(), AppError> {
        self.limits
            .check(&OAUTH_CEREMONIES_GLOBAL, LimitKey::Global)
            .await?;
        self.limits
            .check(&OAUTH_CEREMONIES, LimitKey::Secret(&digest(untrusted_key)))
            .await
    }

    /// An app registration from `address` (the caller's, when known).
    pub async fn check_oauth_registration_sender(
        &self,
        address: Option<&str>,
    ) -> Result<(), AppError> {
        match address {
            Some(address) => {
                self.limits
                    .check(&OAUTH_REGISTRATION_SENDERS, LimitKey::Address(address))
                    .await
            }
            None => Ok(()),
        }
    }

    /// Before a refresh is looked at: refuses a sender whose refreshes keep
    /// failing and, while bad refreshes flood in from everywhere, any app
    /// that hasn't got tokens lately or whose own bad refreshes are spent
    /// (anyone can name an app's client ID). Spends nothing: a good refresh is
    /// charged to its app by `check_oauth_refresh` once its token checks out,
    /// and a bad one to its sender by `note_oauth_refresh_failure`.
    pub async fn check_oauth_refresh_sender(
        &self,
        address: Option<&str>,
        untrusted_client_id: &str,
    ) -> Result<(), AppError> {
        if let Some(address) = address
            && let Some(retry_after) = self
                .limits
                .spent_or_open(&OAUTH_REFRESH_FAILURES, LimitKey::Address(address))
                .await
        {
            return Err(AppError::rate_limited(retry_after));
        }
        if let Some(retry_after) = self
            .limits
            .spent_or_open(&OAUTH_REFRESH_FAILURES_GLOBAL, LimitKey::Global)
            .await
        {
            let client = digest(untrusted_client_id);
            if !self
                .limits
                .seen(&RECENT_OAUTH_CLIENTS, LimitKey::Secret(&client))
                .await
            {
                return Err(AppError::rate_limited(retry_after));
            }
            if let Some(retry_after) = self
                .limits
                .spent_or_open(
                    &OAUTH_REFRESH_FAILURES_PER_CLIENT,
                    LimitKey::Secret(&client),
                )
                .await
            {
                return Err(AppError::rate_limited(retry_after));
            }
        }
        Ok(())
    }

    /// A refresh whose token checked out, charged to its app.
    pub async fn check_oauth_refresh(&self, client_id: &str) -> Result<(), AppError> {
        self.limits
            .check(&OAUTH_REFRESHES_GLOBAL, LimitKey::Global)
            .await?;
        self.limits
            .check(&OAUTH_REFRESHES, LimitKey::Secret(&digest(client_id)))
            .await
    }

    /// [`Self::check_oauth_refresh`] inside the caller's transaction, which
    /// holds the refresh token's row: parallel refreshes of one grant queue
    /// on that lock, each holding a connection, so the check must not wait
    /// for another.
    pub async fn check_oauth_refresh_on(
        &self,
        connection: &mut PgConnection,
        client_id: &str,
    ) -> Result<(), AppError> {
        self.limits
            .check_on(connection, &OAUTH_REFRESHES_GLOBAL, LimitKey::Global)
            .await?;
        self.limits
            .check_on(
                connection,
                &OAUTH_REFRESHES,
                LimitKey::Secret(&digest(client_id)),
            )
            .await
    }

    /// A refresh whose token didn't check out, charged to its sender and to
    /// the app it named.
    pub async fn note_oauth_refresh_failure(
        &self,
        address: Option<&str>,
        untrusted_client_id: &str,
    ) {
        // A spent budget refuses the next attempt; nothing more to charge.
        self.limits
            .note(&OAUTH_REFRESH_FAILURES_GLOBAL, LimitKey::Global)
            .await;
        self.limits
            .note(
                &OAUTH_REFRESH_FAILURES_PER_CLIENT,
                LimitKey::Secret(&digest(untrusted_client_id)),
            )
            .await;
        if let Some(address) = address {
            self.limits
                .note(&OAUTH_REFRESH_FAILURES, LimitKey::Address(address))
                .await;
        }
    }

    /// An app that just got tokens (connected or refreshed).
    pub async fn note_oauth_client_success(&self, client_id: &str) {
        self.limits
            .remember(&RECENT_OAUTH_CLIENTS, LimitKey::Secret(&digest(client_id)))
            .await;
    }

    /// `None` means the caller has no distinguishing key; it is bounded by the
    /// global ceiling only, so one caller cannot exhaust a shared bucket.
    pub async fn check_workos_start(&self, untrusted_key: Option<&str>) -> Result<(), AppError> {
        self.limits
            .check(&WORKOS_STARTS_GLOBAL, LimitKey::Global)
            .await?;
        let Some(untrusted_key) = untrusted_key else {
            return Ok(());
        };
        self.limits
            .check(&WORKOS_STARTS, LimitKey::Secret(&digest(untrusted_key)))
            .await
    }

    /// A password reset request from `address` (the caller's, when known)
    /// for `normalized_key` (a login name or an email address, lower case).
    /// The sender's budget comes first, then the account's, so a script runs
    /// out of its own before it reaches the server's.
    pub async fn check_password_reset(
        &self,
        address: Option<&str>,
        normalized_key: &str,
    ) -> Result<(), AppError> {
        if let Some(address) = address {
            self.limits
                .check(&PASSWORD_RESET_SENDERS, LimitKey::Address(address))
                .await?;
        }
        self.limits
            .check(&PASSWORD_RESETS, LimitKey::Secret(&digest(normalized_key)))
            .await?;
        self.limits
            .check(&PASSWORD_RESETS_GLOBAL, LimitKey::Global)
            .await
    }

    /// A reset email about to go out to a real account. Err holds the
    /// seconds until the server may send another.
    /// Checked inside the sending job's transaction.
    pub async fn check_reset_email(&self, connection: &mut PgConnection) -> Result<(), u64> {
        match self
            .limits
            .check_on(connection, &RESET_EMAILS, LimitKey::Global)
            .await
        {
            Ok(()) => Ok(()),
            Err(AppError::RateLimited {
                retry_after_seconds,
            }) => Err(retry_after_seconds),
            Err(_) => Err(60),
        }
    }

    pub async fn check_webhook_attempt(&self) -> Result<(), AppError> {
        self.limits.check(&WEBHOOK_ATTEMPTS, LimitKey::Global).await
    }

    /// A report sent to the bug report link whose token has this digest.
    pub async fn check_bug_report(&self, token_digest: &[u8]) -> Result<(), AppError> {
        self.limits
            .check(&BUG_REPORTS_GLOBAL, LimitKey::Global)
            .await?;
        self.limits
            .check(&BUG_REPORTS, LimitKey::Secret(token_digest))
            .await
    }
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            password_kdfs: Arc::new(Semaphore::new(CONCURRENT_PASSWORD_KDFS)),
        }
    }
}

/// Every bucket, for the name check (and for anyone listing the budgets).
pub static ALL: &[&Bucket] = &[
    &LOGIN,
    &LOGIN_HOURLY,
    &LOGIN_SENDERS,
    &LOGIN_FAILURES,
    &WRITES,
    &EXPENSIVE_READS,
    &INVITATIONS,
    &INVITATIONS_GLOBAL,
    &MCP_REQUESTS,
    &MCP_AUTH,
    &RUN_REPORTS,
    &MCP_AUTH_FAILURES,
    &OAUTH_CEREMONIES,
    &OAUTH_CEREMONIES_GLOBAL,
    &OAUTH_REGISTRATION_SENDERS,
    &OAUTH_REFRESHES,
    &OAUTH_REFRESHES_GLOBAL,
    &OAUTH_REFRESH_FAILURES,
    &OAUTH_REFRESH_FAILURES_GLOBAL,
    &OAUTH_REFRESH_FAILURES_PER_CLIENT,
    &WORKOS_STARTS,
    &WORKOS_STARTS_GLOBAL,
    &WEBHOOK_ATTEMPTS,
    &BUG_REPORTS,
    &BUG_REPORTS_GLOBAL,
    &PASSWORD_RESETS,
    &PASSWORD_RESET_SENDERS,
    &PASSWORD_RESETS_GLOBAL,
    &RESET_EMAILS,
];

/// The caller's address as the configured header reports it
/// (`AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER`), for per-address limits.
/// `None` when no header is configured or the request has none; those
/// callers are bounded by the shared ceilings only.
pub fn client_address<'a>(
    config: &crate::config::AppConfig,
    headers: &'a HeaderMap,
) -> Option<&'a str> {
    headers
        .get(config.client_address_header.as_deref()?)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 64)
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::{CONCURRENT_PASSWORD_KDFS, RateLimiters};

    #[test]
    fn kdf_concurrency_is_bounded() {
        let limiters = RateLimiters::default();
        let permits = (0..CONCURRENT_PASSWORD_KDFS)
            .map(|_| limiters.try_acquire_password_kdf().expect("permit"))
            .collect::<Vec<_>>();
        assert!(limiters.try_acquire_password_kdf().is_err());
        drop(permits);
        assert!(limiters.try_acquire_password_kdf().is_ok());
    }

    #[test]
    fn bucket_names_fit_the_counter_table() {
        let names = super::ALL.iter().map(|bucket| bucket.name);
        let recents = [
            super::RECENT_LOGINS.name,
            super::RECENT_MCP_TOKENS.name,
            super::RECENT_OAUTH_CLIENTS.name,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for name in names.chain(recents) {
            assert!(
                name.len() <= 63
                    && name.starts_with(|c: char| c.is_ascii_lowercase())
                    && name.chars().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '_'
                        || c == '.'),
                "{name}"
            );
            assert!(seen.insert(name), "{name} named twice");
        }
    }
}
