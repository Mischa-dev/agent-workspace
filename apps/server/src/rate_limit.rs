use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::error::AppError;

const WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPTS_PER_WINDOW: u32 = 10;
const GLOBAL_LOGIN_ATTEMPTS_PER_WINDOW: u32 = 60;
// The app is reachable from the internet, so cap slow password guessing too.
const LOGIN_ATTEMPTS_PER_HOUR: u32 = 30;
/// Sign-in attempts from one sender address, checked before the shared
/// ceiling so one script can't use up everyone's.
const LOGIN_ATTEMPTS_PER_ADDRESS_PER_WINDOW: u32 = 20;
const HOUR: Duration = Duration::from_secs(3_600);
const AUTHENTICATED_WRITES_PER_WINDOW: u32 = 240;
const EXPENSIVE_READS_PER_WINDOW: u32 = 120;
const INVITATION_ATTEMPTS_PER_WINDOW: u32 = 20;
const GLOBAL_INVITATION_ATTEMPTS_PER_WINDOW: u32 = 120;
const MCP_REQUESTS_PER_WINDOW: u32 = 600;
const MCP_AUTH_ATTEMPTS_PER_WINDOW: u32 = 600;
const GLOBAL_MCP_AUTH_ATTEMPTS_PER_WINDOW: u32 = 900;
const OAUTH_CEREMONIES_PER_WINDOW: u32 = 30;
const GLOBAL_OAUTH_CEREMONIES_PER_WINDOW: u32 = 240;
/// Token refreshes have their own budget: one app running many sessions on a
/// grant refreshes from all of them at once, and a refused refresh drops the
/// app's tools, so it mustn't share the sign-in ceremonies' smaller one.
const OAUTH_REFRESHES_PER_WINDOW: u32 = 300;
const GLOBAL_OAUTH_REFRESHES_PER_WINDOW: u32 = 1_200;
/// Refreshes whose token didn't check out, per sender address and in all.
/// These are spent only by bad refreshes, so junk can't use up the budget
/// real apps need; the global one keeps junk from reaching the database
/// without end.
const OAUTH_REFRESH_FAILURES_PER_WINDOW: u32 = 30;
const GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW: u32 = 600;
/// Bad refreshes naming one app, which still reach the database while the
/// global budget is spent if that app got tokens lately. The client ID isn't
/// proof the request came from the app, so this keeps that way in bounded:
/// a real app's own refresh failures are rare, so it stays well clear.
const OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW: u32 = 60;
/// While bad refreshes flood in from everywhere, an app that got tokens this
/// recently still gets through. Access tokens last twelve hours, so a day
/// covers every app that's in use.
const RECENT_OAUTH_CLIENT_TTL: Duration = Duration::from_secs(24 * 3_600);
// WorkOS sign-in starts have their own budget so they can neither starve nor
// be starved by password logins and MCP OAuth ceremonies.
const WORKOS_STARTS_PER_WINDOW: u32 = 20;
const GLOBAL_WORKOS_STARTS_PER_WINDOW: u32 = 600;
const WEBHOOK_ATTEMPTS_PER_WINDOW: u32 = 300;
/// Reports sent to one bug report link, and to all of them: enough for a
/// burst of real reports, not for a loop to flood a project.
const BUG_REPORTS_PER_WINDOW: u32 = 30;
const GLOBAL_BUG_REPORTS_PER_WINDOW: u32 = 300;
/// Password reset requests: a few per account an hour and a few dozen per
/// sender address, checked before a ceiling for the whole server so one
/// script can't use up everyone's. The mail itself has its own ceiling,
/// spent only when a reset email really goes out (to a real account).
const PASSWORD_RESETS_PER_HOUR: u32 = 5;
const PASSWORD_RESETS_PER_ADDRESS_PER_HOUR: u32 = 30;
const GLOBAL_PASSWORD_RESETS_PER_WINDOW: u32 = 120;
const RESET_EMAILS_PER_WINDOW: u32 = 30;
const MAX_LOGIN_KEYS: usize = 4_096;
const MAX_INVITATION_KEYS: usize = 4_096;
const MAX_SESSION_KEYS: usize = 4_096;
const CONCURRENT_PASSWORD_KDFS: usize = 2;

#[derive(Clone)]
pub struct RateLimiters {
    login: FixedWindowLimiter<[u8; 32]>,
    login_hourly: FixedWindowLimiter<[u8; 32]>,
    login_senders: FixedWindowLimiter<[u8; 32]>,
    global_login: FixedWindowLimiter<()>,
    authenticated_writes: FixedWindowLimiter<Uuid>,
    expensive_reads: FixedWindowLimiter<Uuid>,
    invitation_attempts: FixedWindowLimiter<[u8; 32]>,
    global_invitation_attempts: FixedWindowLimiter<()>,
    mcp_requests: FixedWindowLimiter<Uuid>,
    mcp_auth_attempts: FixedWindowLimiter<[u8; 32]>,
    global_mcp_auth_attempts: FixedWindowLimiter<()>,
    oauth_ceremonies: FixedWindowLimiter<[u8; 32]>,
    global_oauth_ceremonies: FixedWindowLimiter<()>,
    oauth_refreshes: FixedWindowLimiter<[u8; 32]>,
    global_oauth_refreshes: FixedWindowLimiter<()>,
    oauth_refresh_failures: FixedWindowLimiter<[u8; 32]>,
    oauth_refresh_client_failures: FixedWindowLimiter<[u8; 32]>,
    global_oauth_refresh_failures: FixedWindowLimiter<()>,
    recent_oauth_clients: RecentKeys,
    workos_starts: FixedWindowLimiter<[u8; 32]>,
    global_workos_starts: FixedWindowLimiter<()>,
    webhook_attempts: FixedWindowLimiter<()>,
    bug_reports: FixedWindowLimiter<[u8; 32]>,
    global_bug_reports: FixedWindowLimiter<()>,
    password_resets: FixedWindowLimiter<[u8; 32]>,
    password_reset_senders: FixedWindowLimiter<[u8; 32]>,
    global_password_resets: FixedWindowLimiter<()>,
    reset_emails: FixedWindowLimiter<()>,
    password_kdfs: Arc<Semaphore>,
}

impl RateLimiters {
    pub fn check_login(&self, normalized_login: &str) -> Result<(), AppError> {
        self.global_login
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(normalized_login.as_bytes()).into();
        self.login.check(key).map_err(AppError::rate_limited)?;
        self.login_hourly.check(key).map_err(AppError::rate_limited)
    }

    /// A sign-in from `address` (the caller's, when known), before
    /// [`Self::check_login`].
    pub fn check_login_sender(&self, address: Option<&str>) -> Result<(), AppError> {
        match address {
            Some(address) => self
                .login_senders
                .check(digest(address))
                .map_err(AppError::rate_limited),
            None => Ok(()),
        }
    }

    pub fn try_acquire_password_kdf(&self) -> Result<OwnedSemaphorePermit, AppError> {
        Arc::clone(&self.password_kdfs)
            .try_acquire_owned()
            .map_err(|_| AppError::rate_limited(1))
    }

    pub fn check_authenticated_write(&self, session_id: Uuid) -> Result<(), AppError> {
        self.authenticated_writes
            .check(session_id)
            .map_err(AppError::rate_limited)
    }

    pub fn check_expensive_read(&self, session_id: Uuid) -> Result<(), AppError> {
        self.expensive_reads
            .check(session_id)
            .map_err(AppError::rate_limited)
    }

    pub fn check_invitation(&self, token_prefix: &str) -> Result<(), AppError> {
        self.global_invitation_attempts
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(token_prefix.as_bytes()).into();
        self.invitation_attempts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    pub fn check_mcp_request(&self, credential_id: Uuid) -> Result<(), AppError> {
        self.mcp_requests
            .check(credential_id)
            .map_err(AppError::rate_limited)
    }

    pub fn check_mcp_auth(&self, untrusted_token: &str) -> Result<(), AppError> {
        self.global_mcp_auth_attempts
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(untrusted_token.as_bytes()).into();
        self.mcp_auth_attempts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    pub fn check_oauth_ceremony(&self, untrusted_key: &str) -> Result<(), AppError> {
        self.global_oauth_ceremonies
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(untrusted_key.as_bytes()).into();
        self.oauth_ceremonies
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// Before a refresh is looked at: refuses a sender whose refreshes keep
    /// failing and, while bad refreshes flood in from everywhere, any app
    /// that hasn't got tokens lately or whose own bad refreshes are spent
    /// (anyone can name an app's client ID). Spends nothing: a good refresh is
    /// charged to its app by `check_oauth_refresh` once its token checks out,
    /// and a bad one to its sender by `note_oauth_refresh_failure`.
    pub fn check_oauth_refresh_sender(
        &self,
        address: Option<&str>,
        untrusted_client_id: &str,
    ) -> Result<(), AppError> {
        let now = Instant::now();
        if let Some(address) = address
            && let Some(retry_after) = self.oauth_refresh_failures.spent_at(&digest(address), now)
        {
            return Err(AppError::rate_limited(retry_after));
        }
        if let Some(retry_after) = self.global_oauth_refresh_failures.spent_at(&(), now) {
            let client = digest(untrusted_client_id);
            if !self.recent_oauth_clients.contains_at(&client, now) {
                return Err(AppError::rate_limited(retry_after));
            }
            if let Some(retry_after) = self.oauth_refresh_client_failures.spent_at(&client, now) {
                return Err(AppError::rate_limited(retry_after));
            }
        }
        Ok(())
    }

    /// A refresh whose token checked out, charged to its app.
    pub fn check_oauth_refresh(&self, client_id: &str) -> Result<(), AppError> {
        self.global_oauth_refreshes
            .check(())
            .map_err(AppError::rate_limited)?;
        self.oauth_refreshes
            .check(digest(client_id))
            .map_err(AppError::rate_limited)
    }

    /// A refresh whose token didn't check out, charged to its sender and to
    /// the app it named.
    pub fn note_oauth_refresh_failure(&self, address: Option<&str>, untrusted_client_id: &str) {
        // A spent budget refuses the next attempt; nothing more to charge.
        let _ = self.global_oauth_refresh_failures.check(());
        let _ = self
            .oauth_refresh_client_failures
            .check(digest(untrusted_client_id));
        if let Some(address) = address {
            let _ = self.oauth_refresh_failures.check(digest(address));
        }
    }

    /// An app that just got tokens (connected or refreshed).
    pub fn note_oauth_client_success(&self, client_id: &str) {
        self.recent_oauth_clients
            .insert(digest(client_id), Instant::now());
    }

    /// `None` means the caller has no distinguishing key; it is bounded by the
    /// global ceiling only, so one caller cannot exhaust a shared bucket.
    pub fn check_workos_start(&self, untrusted_key: Option<&str>) -> Result<(), AppError> {
        self.global_workos_starts
            .check(())
            .map_err(AppError::rate_limited)?;
        let Some(untrusted_key) = untrusted_key else {
            return Ok(());
        };
        let key: [u8; 32] = Sha256::digest(untrusted_key.as_bytes()).into();
        self.workos_starts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// A password reset request from `address` (the caller's, when known)
    /// for `normalized_key` (a login name or an email address, lower case).
    /// The sender's budget comes first, then the account's, so a script runs
    /// out of its own before it reaches the server's.
    pub fn check_password_reset(
        &self,
        address: Option<&str>,
        normalized_key: &str,
    ) -> Result<(), AppError> {
        if let Some(address) = address {
            self.password_reset_senders
                .check(digest(address))
                .map_err(AppError::rate_limited)?;
        }
        self.password_resets
            .check(digest(normalized_key))
            .map_err(AppError::rate_limited)?;
        self.global_password_resets
            .check(())
            .map_err(AppError::rate_limited)
    }

    /// A reset email about to go out to a real account. Err holds the
    /// seconds until the server may send another.
    pub fn check_reset_email(&self) -> Result<(), u64> {
        self.reset_emails.check(())
    }

    pub fn check_webhook_attempt(&self) -> Result<(), AppError> {
        self.webhook_attempts
            .check(())
            .map_err(AppError::rate_limited)
    }

    /// A report sent to the bug report link whose token has this digest.
    pub fn check_bug_report(&self, token_digest: &[u8]) -> Result<(), AppError> {
        self.global_bug_reports
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(token_digest).into();
        self.bug_reports.check(key).map_err(AppError::rate_limited)
    }
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self {
            login: FixedWindowLimiter::new(LOGIN_ATTEMPTS_PER_WINDOW, WINDOW, MAX_LOGIN_KEYS),
            login_hourly: FixedWindowLimiter::new(LOGIN_ATTEMPTS_PER_HOUR, HOUR, MAX_LOGIN_KEYS),
            login_senders: FixedWindowLimiter::new(
                LOGIN_ATTEMPTS_PER_ADDRESS_PER_WINDOW,
                WINDOW,
                MAX_LOGIN_KEYS,
            ),
            global_login: FixedWindowLimiter::new(GLOBAL_LOGIN_ATTEMPTS_PER_WINDOW, WINDOW, 1),
            authenticated_writes: FixedWindowLimiter::new(
                AUTHENTICATED_WRITES_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            expensive_reads: FixedWindowLimiter::new(
                EXPENSIVE_READS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            invitation_attempts: FixedWindowLimiter::new(
                INVITATION_ATTEMPTS_PER_WINDOW,
                WINDOW,
                MAX_INVITATION_KEYS,
            ),
            global_invitation_attempts: FixedWindowLimiter::new(
                GLOBAL_INVITATION_ATTEMPTS_PER_WINDOW,
                WINDOW,
                1,
            ),
            mcp_requests: FixedWindowLimiter::new(
                MCP_REQUESTS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            mcp_auth_attempts: FixedWindowLimiter::new(
                MCP_AUTH_ATTEMPTS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            global_mcp_auth_attempts: FixedWindowLimiter::new(
                GLOBAL_MCP_AUTH_ATTEMPTS_PER_WINDOW,
                WINDOW,
                1,
            ),
            oauth_ceremonies: FixedWindowLimiter::new(
                OAUTH_CEREMONIES_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            global_oauth_ceremonies: FixedWindowLimiter::new(
                GLOBAL_OAUTH_CEREMONIES_PER_WINDOW,
                WINDOW,
                1,
            ),
            oauth_refreshes: FixedWindowLimiter::new(
                OAUTH_REFRESHES_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            global_oauth_refreshes: FixedWindowLimiter::new(
                GLOBAL_OAUTH_REFRESHES_PER_WINDOW,
                WINDOW,
                1,
            ),
            oauth_refresh_failures: FixedWindowLimiter::new(
                OAUTH_REFRESH_FAILURES_PER_WINDOW,
                WINDOW,
                MAX_LOGIN_KEYS,
            ),
            oauth_refresh_client_failures: FixedWindowLimiter::new(
                OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            global_oauth_refresh_failures: FixedWindowLimiter::new(
                GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW,
                WINDOW,
                1,
            ),
            recent_oauth_clients: RecentKeys::new(RECENT_OAUTH_CLIENT_TTL, MAX_SESSION_KEYS),
            workos_starts: FixedWindowLimiter::new(
                WORKOS_STARTS_PER_WINDOW,
                WINDOW,
                MAX_LOGIN_KEYS,
            ),
            global_workos_starts: FixedWindowLimiter::new(
                GLOBAL_WORKOS_STARTS_PER_WINDOW,
                WINDOW,
                1,
            ),
            webhook_attempts: FixedWindowLimiter::new(WEBHOOK_ATTEMPTS_PER_WINDOW, WINDOW, 1),
            bug_reports: FixedWindowLimiter::new(BUG_REPORTS_PER_WINDOW, WINDOW, MAX_SESSION_KEYS),
            global_bug_reports: FixedWindowLimiter::new(GLOBAL_BUG_REPORTS_PER_WINDOW, WINDOW, 1),
            password_resets: FixedWindowLimiter::new(
                PASSWORD_RESETS_PER_HOUR,
                HOUR,
                MAX_LOGIN_KEYS,
            ),
            password_reset_senders: FixedWindowLimiter::new(
                PASSWORD_RESETS_PER_ADDRESS_PER_HOUR,
                HOUR,
                MAX_LOGIN_KEYS,
            ),
            global_password_resets: FixedWindowLimiter::new(
                GLOBAL_PASSWORD_RESETS_PER_WINDOW,
                WINDOW,
                1,
            ),
            reset_emails: FixedWindowLimiter::new(RESET_EMAILS_PER_WINDOW, WINDOW, 1),
            password_kdfs: Arc::new(Semaphore::new(CONCURRENT_PASSWORD_KDFS)),
        }
    }
}

/// The caller's address as Cloudflare reports it, for per-address limits.
/// `None` where the server isn't behind Cloudflare; those callers are
/// bounded by the global ceilings only.
pub fn client_address(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 64)
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[derive(Clone)]
struct FixedWindowLimiter<K> {
    limit: u32,
    window: Duration,
    capacity: usize,
    entries: Arc<Mutex<HashMap<K, WindowEntry>>>,
}

#[derive(Clone, Copy)]
struct WindowEntry {
    started_at: Instant,
    attempts: u32,
}

impl<K> FixedWindowLimiter<K>
where
    K: Eq + Hash,
{
    fn new(limit: u32, window: Duration, capacity: usize) -> Self {
        debug_assert!(limit > 0);
        debug_assert!(!window.is_zero());
        debug_assert!(capacity > 0);
        Self {
            limit,
            window,
            capacity,
            entries: Arc::new(Mutex::new(HashMap::with_capacity(capacity))),
        }
    }

    fn check(&self, key: K) -> Result<(), u64> {
        self.check_at(key, Instant::now())
    }

    /// Seconds to wait when `key` has used up this window's budget, without
    /// spending any of it.
    fn spent_at(&self, key: &K, now: Instant) -> Option<u64> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get(key)?;
        let elapsed = now.saturating_duration_since(entry.started_at);
        (elapsed < self.window && entry.attempts >= self.limit)
            .then(|| retry_after_seconds(self.window - elapsed))
    }

    fn check_at(&self, key: K, now: Instant) -> Result<(), u64> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(entry) = entries.get_mut(&key) {
            let elapsed = now.saturating_duration_since(entry.started_at);
            if elapsed >= self.window {
                *entry = WindowEntry {
                    started_at: now,
                    attempts: 1,
                };
                return Ok(());
            }
            if entry.attempts >= self.limit {
                return Err(retry_after_seconds(self.window - elapsed));
            }
            entry.attempts += 1;
            return Ok(());
        }

        if entries.len() >= self.capacity {
            entries
                .retain(|_, entry| now.saturating_duration_since(entry.started_at) < self.window);
        }
        if entries.len() >= self.capacity {
            let retry_after = entries
                .values()
                .map(|entry| {
                    self.window
                        .saturating_sub(now.saturating_duration_since(entry.started_at))
                })
                .min()
                .unwrap_or(self.window);
            return Err(retry_after_seconds(retry_after));
        }

        entries.insert(
            key,
            WindowEntry {
                started_at: now,
                attempts: 1,
            },
        );
        Ok(())
    }
}

/// Keys seen lately, each remembered for `ttl`, at most `capacity` of them.
#[derive(Clone)]
struct RecentKeys {
    ttl: Duration,
    capacity: usize,
    entries: Arc<Mutex<HashMap<[u8; 32], Instant>>>,
}

impl RecentKeys {
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn insert(&self, key: [u8; 32], now: Instant) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            entries.retain(|_, seen| now.saturating_duration_since(*seen) < self.ttl);
            if entries.len() >= self.capacity {
                return;
            }
        }
        entries.insert(key, now);
    }

    fn contains_at(&self, key: &[u8; 32], now: Instant) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .is_some_and(|seen| now.saturating_duration_since(*seen) < self.ttl)
    }
}

fn retry_after_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{
        CONCURRENT_PASSWORD_KDFS, EXPENSIVE_READS_PER_WINDOW, FixedWindowLimiter,
        GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW, MCP_AUTH_ATTEMPTS_PER_WINDOW,
        MCP_REQUESTS_PER_WINDOW, OAUTH_CEREMONIES_PER_WINDOW,
        OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW, OAUTH_REFRESH_FAILURES_PER_WINDOW,
        OAUTH_REFRESHES_PER_WINDOW, RateLimiters, WEBHOOK_ATTEMPTS_PER_WINDOW,
    };
    use crate::error::AppError;

    #[test]
    fn fixed_window_allows_the_limit_then_reports_retry_after() {
        let limiter = FixedWindowLimiter::new(2, Duration::from_secs(60), 8);
        let start = Instant::now();

        assert!(limiter.check_at("login", start).is_ok());
        assert!(limiter.check_at("login", start).is_ok());
        assert_eq!(
            limiter.check_at("login", start + Duration::from_millis(1)),
            Err(60)
        );
        assert!(
            limiter
                .check_at("login", start + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn fixed_window_storage_is_bounded_and_expired_keys_are_reclaimed() {
        let limiter = FixedWindowLimiter::new(2, Duration::from_secs(60), 2);
        let start = Instant::now();

        assert!(limiter.check_at("first", start).is_ok());
        assert!(limiter.check_at("second", start).is_ok());
        assert_eq!(limiter.check_at("third", start), Err(60));
        assert!(
            limiter
                .check_at("third", start + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn application_limits_are_ten_logins_and_240_writes_per_minute() {
        let limiters = RateLimiters::default();
        for _ in 0..10 {
            assert!(limiters.check_login("mischa").is_ok());
        }
        assert!(matches!(
            limiters.check_login("mischa"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(
            limiters.check_login("another-login").is_ok(),
            "login names have independent digest keys"
        );

        let session_id = Uuid::now_v7();
        for _ in 0..240 {
            assert!(limiters.check_authenticated_write(session_id).is_ok());
        }
        assert!(matches!(
            limiters.check_authenticated_write(session_id),
            Err(AppError::RateLimited { .. })
        ));
        assert!(
            limiters.check_authenticated_write(Uuid::now_v7()).is_ok(),
            "sessions have independent write keys"
        );
    }

    #[test]
    fn global_login_budget_and_kdf_concurrency_are_bounded() {
        let limiters = RateLimiters::default();
        for index in 0..60 {
            assert!(
                limiters.check_login(&format!("login-{index}")).is_ok(),
                "unique login {index} remains inside the global budget"
            );
        }
        assert!(matches!(
            limiters.check_login("login-over-global-budget"),
            Err(AppError::RateLimited { .. })
        ));

        let mut permits = Vec::new();
        for _ in 0..CONCURRENT_PASSWORD_KDFS {
            permits.push(
                limiters
                    .try_acquire_password_kdf()
                    .expect("bounded KDF permit"),
            );
        }
        assert!(matches!(
            limiters.try_acquire_password_kdf(),
            Err(AppError::RateLimited { .. })
        ));
        drop(permits);
        assert!(limiters.try_acquire_password_kdf().is_ok());
    }

    #[test]
    fn invitation_attempts_are_token_scoped_and_globally_bounded() {
        let limiters = RateLimiters::default();
        for _ in 0..20 {
            assert!(limiters.check_invitation("aw_inv_first").is_ok());
        }
        assert!(matches!(
            limiters.check_invitation("aw_inv_first"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_invitation("aw_inv_second").is_ok());
    }

    #[test]
    fn mcp_oauth_webhook_and_expensive_read_budgets_are_bounded() {
        let limiters = RateLimiters::default();
        let credential_id = Uuid::now_v7();
        for _ in 0..MCP_REQUESTS_PER_WINDOW {
            assert!(limiters.check_mcp_request(credential_id).is_ok());
        }
        assert!(matches!(
            limiters.check_mcp_request(credential_id),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_mcp_request(Uuid::now_v7()).is_ok());
        for _ in 0..MCP_AUTH_ATTEMPTS_PER_WINDOW {
            assert!(limiters.check_mcp_auth("bearer-token-a").is_ok());
        }
        assert!(matches!(
            limiters.check_mcp_auth("bearer-token-a"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_mcp_auth("bearer-token-b").is_ok());

        for _ in 0..OAUTH_CEREMONIES_PER_WINDOW {
            assert!(limiters.check_oauth_ceremony("client-a").is_ok());
        }
        assert!(matches!(
            limiters.check_oauth_ceremony("client-a"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_oauth_ceremony("client-b").is_ok());
        // Refreshes don't spend the exhausted ceremony budget.
        for _ in 0..OAUTH_REFRESHES_PER_WINDOW {
            assert!(limiters.check_oauth_refresh("client-a").is_ok());
        }
        assert!(matches!(
            limiters.check_oauth_refresh("client-a"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_oauth_refresh("client-b").is_ok());
        // ...and a spent app budget doesn't refuse the sender.
        assert!(
            limiters
                .check_oauth_refresh_sender(Some("192.0.2.1"), "client-a")
                .is_ok()
        );

        for _ in 0..WEBHOOK_ATTEMPTS_PER_WINDOW {
            assert!(limiters.check_webhook_attempt().is_ok());
        }
        assert!(matches!(
            limiters.check_webhook_attempt(),
            Err(AppError::RateLimited { .. })
        ));

        let session_id = Uuid::now_v7();
        for _ in 0..EXPENSIVE_READS_PER_WINDOW {
            assert!(limiters.check_expensive_read(session_id).is_ok());
        }
        assert!(matches!(
            limiters.check_expensive_read(session_id),
            Err(AppError::RateLimited { .. })
        ));
    }

    #[test]
    fn junk_refreshes_are_charged_to_their_sender_not_to_real_apps() {
        let limiters = RateLimiters::default();
        for _ in 0..OAUTH_REFRESH_FAILURES_PER_WINDOW {
            assert!(
                limiters
                    .check_oauth_refresh_sender(Some("203.0.113.9"), "junk")
                    .is_ok()
            );
            limiters.note_oauth_refresh_failure(Some("203.0.113.9"), "junk");
        }
        assert!(matches!(
            limiters.check_oauth_refresh_sender(Some("203.0.113.9"), "junk"),
            Err(AppError::RateLimited { .. })
        ));
        // Other senders and every app's own budget are untouched.
        assert!(
            limiters
                .check_oauth_refresh_sender(Some("198.51.100.7"), "app")
                .is_ok()
        );
        assert!(limiters.check_oauth_refresh_sender(None, "app").is_ok());
        assert!(limiters.check_oauth_refresh("app").is_ok());

        // A flood from everywhere spends the global failure budget: then only
        // apps that got tokens lately get through.
        limiters.note_oauth_client_success("app");
        for _ in OAUTH_REFRESH_FAILURES_PER_WINDOW..GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW {
            limiters.note_oauth_refresh_failure(None, "junk");
        }
        assert!(matches!(
            limiters.check_oauth_refresh_sender(None, "junk"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_oauth_refresh_sender(None, "app").is_ok());
        assert!(
            limiters
                .check_oauth_refresh_sender(Some("198.51.100.7"), "app")
                .is_ok()
        );

        // Naming that app doesn't open an unbounded way past the flood: bad
        // refreshes under its client ID are capped too.
        for _ in 0..OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW {
            assert!(limiters.check_oauth_refresh_sender(None, "app").is_ok());
            limiters.note_oauth_refresh_failure(None, "app");
        }
        assert!(matches!(
            limiters.check_oauth_refresh_sender(None, "app"),
            Err(AppError::RateLimited { .. })
        ));
        limiters.note_oauth_client_success("other-app");
        assert!(
            limiters
                .check_oauth_refresh_sender(None, "other-app")
                .is_ok()
        );
    }

    #[test]
    fn spent_at_reads_a_budget_without_spending_it() {
        let limiter = FixedWindowLimiter::new(1, Duration::from_secs(60), 8);
        let start = Instant::now();
        assert_eq!(limiter.spent_at(&"sender", start), None);
        assert_eq!(limiter.spent_at(&"sender", start), None);
        assert!(limiter.check_at("sender", start).is_ok());
        assert_eq!(limiter.spent_at(&"sender", start), Some(60));
        assert_eq!(
            limiter.spent_at(&"sender", start + Duration::from_secs(60)),
            None
        );
    }
}
