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
/// Failed sign-ins across every name. While they're spent, only names that
/// signed in lately on this process get through to their own limits, so
/// junk names can't lock everyone out.
const GLOBAL_LOGIN_FAILURES_PER_WINDOW: u32 = 60;
/// How long a name that signed in counts as known while failures flood in.
const RECENT_LOGIN_TTL: Duration = Duration::from_secs(7 * 24 * 3_600);
// The app is reachable from the internet, so cap slow password guessing too.
const LOGIN_ATTEMPTS_PER_HOUR: u32 = 30;
/// Sign-in attempts from one sender address, checked before the shared
/// ceiling so one script can't use up everyone's.
const LOGIN_ATTEMPTS_PER_ADDRESS_PER_WINDOW: u32 = 20;
const HOUR: Duration = Duration::from_secs(3_600);
const AUTHENTICATED_WRITES_PER_WINDOW: u32 = 240;
const EXPENSIVE_READS_PER_WINDOW: u32 = 120;
const INVITATION_ATTEMPTS_PER_WINDOW: u32 = 20;
/// Link tokens (invitations, confirmation and reset links) that were
/// malformed or matched nothing, across every caller. Only misses spend it;
/// while it's spent, link lookups wait.
const GLOBAL_INVITATION_FAILURES_PER_WINDOW: u32 = 120;
const MCP_REQUESTS_PER_WINDOW: u32 = 600;
/// MCP and public API calls into one workspace, from all its agents and
/// apps together (H1.1): one workspace can't take the server's capacity.
pub const WORKSPACE_CALLS_PER_WINDOW: u32 = 3_000;
const MCP_AUTH_ATTEMPTS_PER_WINDOW: u32 = 600;
/// Run reports (A1.3) from one account: 20 a second on average, in bursts
/// of up to 100. In-process until K5's shared limits.
const RUN_REPORTS_PER_WINDOW: u32 = 100;
const RUN_REPORT_WINDOW: Duration = Duration::from_secs(5);
/// Tokens that didn't resolve, across every caller. Valid tokens never
/// spend it; while it's spent, only tokens that authenticated lately are
/// looked up.
const GLOBAL_MCP_AUTH_FAILURES_PER_WINDOW: u32 = 900;
/// Access tokens last twelve hours, so a day covers every app in use.
const RECENT_MCP_TOKEN_TTL: Duration = Duration::from_secs(24 * 3_600);
const OAUTH_CEREMONIES_PER_WINDOW: u32 = 30;
/// App sign-in ceremonies from one sender address, when the address is
/// known.
const OAUTH_CEREMONIES_PER_ADDRESS_PER_WINDOW: u32 = 60;
/// App registrations from one sender address, when the address is known.
const OAUTH_REGISTRATIONS_PER_ADDRESS_PER_WINDOW: u32 = 20;
/// Ceremonies from callers with no known address, across all of them.
const GLOBAL_OAUTH_CEREMONIES_PER_WINDOW: u32 = 240;
/// Token refreshes have their own budget: one app running many sessions on a
/// grant refreshes from all of them at once, and a refused refresh drops the
/// app's tools, so it mustn't share the sign-in ceremonies' smaller one.
const OAUTH_REFRESHES_PER_WINDOW: u32 = 300;
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
/// Starts from callers with no known address, across all of them.
const GLOBAL_WORKOS_STARTS_PER_WINDOW: u32 = 600;
const WEBHOOK_ATTEMPTS_PER_WINDOW: u32 = 300;
/// Reports sent to one bug report link, and to all of them: enough for a
/// burst of real reports, not for a loop to flood a project.
const BUG_REPORTS_PER_WINDOW: u32 = 30;
/// Bug reports and public form submissions into one workspace, across all
/// its links (H1.1).
const WORKSPACE_PUBLIC_SUBMISSIONS_PER_WINDOW: u32 = 300;
/// Password reset requests: a few per account an hour and a few dozen per
/// sender address, checked before a ceiling for the whole server so one
/// script can't use up everyone's. The mail itself has its own ceiling,
/// spent only when a reset email really goes out (to a real account).
const PASSWORD_RESETS_PER_HOUR: u32 = 5;
const PASSWORD_RESETS_PER_ADDRESS_PER_HOUR: u32 = 30;
/// Reset requests from callers with no known address, across all of them.
const GLOBAL_PASSWORD_RESETS_PER_WINDOW: u32 = 120;
const RESET_EMAILS_PER_WINDOW: u32 = 30;
const MAX_LOGIN_KEYS: usize = 4_096;
const MAX_INVITATION_KEYS: usize = 4_096;
const MAX_SESSION_KEYS: usize = 4_096;
const CONCURRENT_PASSWORD_KDFS: usize = 2;

/// Who a limiter's budget belongs to (H1.1). A budget shared by the whole
/// server is spent only by failures and junk, or by callers the server can't
/// tell apart: a caller that signed in, or sent a valid token, never shares
/// one with everyone else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitScope {
    /// One sender address (`AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER`).
    Address,
    /// One login name or email address.
    Login,
    /// One credential, session or token.
    Credential,
    /// One account.
    Account,
    /// One workspace, across all its people, agents and apps.
    Workspace,
    /// One link token (an invitation, a bug report or form link).
    Link,
    /// One connected app (an OAuth client).
    Client,
    /// The whole server, spent only by failures and junk.
    ServerFailuresOnly,
    /// The whole server, spent only by callers with no known address.
    ServerUnattributed,
    /// The whole server, spent by everyone: only the few in
    /// [`SERVER_WIDE`], each with its reason.
    Server,
}

/// One limiter, as declared.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    /// The field in [`RateLimiters`].
    pub name: &'static str,
    pub scope: LimitScope,
    pub per_window: u32,
    pub window_seconds: u64,
}

const fn limit(name: &'static str, scope: LimitScope, per_window: u32, window: Duration) -> Limit {
    Limit {
        name,
        scope,
        per_window,
        window_seconds: window.as_secs(),
    }
}

/// Every limiter and its scope: the one list (R24 rule 3). K5's shared
/// store takes these keys and scopes as they are.
pub const LIMITS: &[Limit] = &[
    limit(
        "login",
        LimitScope::Login,
        LOGIN_ATTEMPTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "login_hourly",
        LimitScope::Login,
        LOGIN_ATTEMPTS_PER_HOUR,
        HOUR,
    ),
    limit(
        "login_senders",
        LimitScope::Address,
        LOGIN_ATTEMPTS_PER_ADDRESS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_login_failures",
        LimitScope::ServerFailuresOnly,
        GLOBAL_LOGIN_FAILURES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "authenticated_writes",
        LimitScope::Credential,
        AUTHENTICATED_WRITES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "expensive_reads",
        LimitScope::Credential,
        EXPENSIVE_READS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "invitation_attempts",
        LimitScope::Link,
        INVITATION_ATTEMPTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_invitation_failures",
        LimitScope::ServerFailuresOnly,
        GLOBAL_INVITATION_FAILURES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "mcp_requests",
        LimitScope::Credential,
        MCP_REQUESTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "workspace_calls",
        LimitScope::Workspace,
        WORKSPACE_CALLS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "run_reports",
        LimitScope::Account,
        RUN_REPORTS_PER_WINDOW,
        RUN_REPORT_WINDOW,
    ),
    limit(
        "mcp_auth_attempts",
        LimitScope::Credential,
        MCP_AUTH_ATTEMPTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_mcp_auth_failures",
        LimitScope::ServerFailuresOnly,
        GLOBAL_MCP_AUTH_FAILURES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_ceremonies",
        LimitScope::Client,
        OAUTH_CEREMONIES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_ceremony_senders",
        LimitScope::Address,
        OAUTH_CEREMONIES_PER_ADDRESS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_registration_senders",
        LimitScope::Address,
        OAUTH_REGISTRATIONS_PER_ADDRESS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_oauth_ceremonies",
        LimitScope::ServerUnattributed,
        GLOBAL_OAUTH_CEREMONIES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_refreshes",
        LimitScope::Client,
        OAUTH_REFRESHES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_refresh_failures",
        LimitScope::Address,
        OAUTH_REFRESH_FAILURES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "oauth_refresh_client_failures",
        LimitScope::Client,
        OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_oauth_refresh_failures",
        LimitScope::ServerFailuresOnly,
        GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "workos_starts",
        LimitScope::Address,
        WORKOS_STARTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "global_workos_starts",
        LimitScope::ServerUnattributed,
        GLOBAL_WORKOS_STARTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "webhook_attempts",
        LimitScope::Server,
        WEBHOOK_ATTEMPTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "bug_reports",
        LimitScope::Link,
        BUG_REPORTS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "workspace_public_submissions",
        LimitScope::Workspace,
        WORKSPACE_PUBLIC_SUBMISSIONS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "password_resets",
        LimitScope::Login,
        PASSWORD_RESETS_PER_HOUR,
        HOUR,
    ),
    limit(
        "password_reset_senders",
        LimitScope::Address,
        PASSWORD_RESETS_PER_ADDRESS_PER_HOUR,
        HOUR,
    ),
    limit(
        "global_password_resets",
        LimitScope::ServerUnattributed,
        GLOBAL_PASSWORD_RESETS_PER_WINDOW,
        WINDOW,
    ),
    limit(
        "reset_emails",
        LimitScope::Server,
        RESET_EMAILS_PER_WINDOW,
        WINDOW,
    ),
];

/// The only limiters every caller shares, and why each can't be narrower.
/// A guard test holds [`LIMITS`] to this list: a new one is a plan change.
pub const SERVER_WIDE: &[(&str, &str)] = &[
    (
        "webhook_attempts",
        "GitHub deliveries come from GitHub's shared addresses before any workspace is known; \
         this bounds signature checks",
    ),
    (
        "reset_emails",
        "reset emails leave through the server's own mail sender, whose standing every \
         workspace shares",
    ),
];

#[derive(Clone)]
pub struct RateLimiters {
    login: FixedWindowLimiter<[u8; 32]>,
    login_hourly: FixedWindowLimiter<[u8; 32]>,
    login_senders: FixedWindowLimiter<[u8; 32]>,
    global_login_failures: FixedWindowLimiter<()>,
    recent_logins: RecentKeys,
    authenticated_writes: FixedWindowLimiter<Uuid>,
    expensive_reads: FixedWindowLimiter<Uuid>,
    invitation_attempts: FixedWindowLimiter<[u8; 32]>,
    global_invitation_failures: FixedWindowLimiter<()>,
    mcp_requests: FixedWindowLimiter<Uuid>,
    workspace_calls: FixedWindowLimiter<Uuid>,
    run_reports: FixedWindowLimiter<Uuid>,
    mcp_auth_attempts: FixedWindowLimiter<[u8; 32]>,
    global_mcp_auth_failures: FixedWindowLimiter<()>,
    recent_mcp_tokens: RecentKeys,
    oauth_ceremonies: FixedWindowLimiter<[u8; 32]>,
    oauth_ceremony_senders: FixedWindowLimiter<[u8; 32]>,
    oauth_registration_senders: FixedWindowLimiter<[u8; 32]>,
    global_oauth_ceremonies: FixedWindowLimiter<()>,
    oauth_refreshes: FixedWindowLimiter<[u8; 32]>,
    oauth_refresh_failures: FixedWindowLimiter<[u8; 32]>,
    oauth_refresh_client_failures: FixedWindowLimiter<[u8; 32]>,
    global_oauth_refresh_failures: FixedWindowLimiter<()>,
    recent_oauth_clients: RecentKeys,
    workos_starts: FixedWindowLimiter<[u8; 32]>,
    global_workos_starts: FixedWindowLimiter<()>,
    webhook_attempts: FixedWindowLimiter<()>,
    bug_reports: FixedWindowLimiter<[u8; 32]>,
    workspace_public_submissions: FixedWindowLimiter<Uuid>,
    password_resets: FixedWindowLimiter<[u8; 32]>,
    password_reset_senders: FixedWindowLimiter<[u8; 32]>,
    global_password_resets: FixedWindowLimiter<()>,
    reset_emails: FixedWindowLimiter<()>,
    password_kdfs: Arc<Semaphore>,
}

impl RateLimiters {
    /// A sign-in attempt for `normalized_login`, charged to the name. While
    /// failed sign-ins everywhere have spent the shared budget, only a name
    /// that signed in lately gets through; everyone else waits.
    pub fn check_login(&self, normalized_login: &str) -> Result<(), AppError> {
        let key = digest(normalized_login);
        let now = Instant::now();
        if let Some(retry_after) = self.global_login_failures.spent_at(&(), now)
            && !self.recent_logins.contains_at(&key, now)
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.login.check(key).map_err(AppError::rate_limited)?;
        self.login_hourly.check(key).map_err(AppError::rate_limited)
    }

    /// A sign-in whose password (or code) didn't check out.
    pub fn note_login_failure(&self) {
        // A spent budget refuses unknown names next time; nothing more to do.
        let _ = self.global_login_failures.check(());
    }

    /// A name that just signed in.
    pub fn note_login_success(&self, normalized_login: &str) {
        self.recent_logins
            .insert(digest(normalized_login), Instant::now());
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

    /// A link token (an invitation, a confirmation or reset link) about to
    /// be looked up, charged to the token. While misses everywhere have
    /// spent the shared budget, lookups wait; a hit never spends it.
    pub fn check_invitation(&self, token_prefix: &str) -> Result<(), AppError> {
        if let Some(retry_after) = self
            .global_invitation_failures
            .spent_at(&(), Instant::now())
        {
            return Err(AppError::rate_limited(retry_after));
        }
        let key: [u8; 32] = Sha256::digest(token_prefix.as_bytes()).into();
        self.invitation_attempts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// A link token that was malformed or matched nothing.
    pub fn note_invitation_failure(&self) {
        let _ = self.global_invitation_failures.check(());
    }

    /// Passes `result` through, charging a miss (`NotFound`) to the shared
    /// link-failure budget.
    pub fn charge_link_miss<T>(&self, result: Result<T, AppError>) -> Result<T, AppError> {
        if matches!(result, Err(AppError::NotFound(_))) {
            self.note_invitation_failure();
        }
        result
    }

    pub fn check_mcp_request(&self, credential_id: Uuid) -> Result<(), AppError> {
        self.mcp_requests
            .check(credential_id)
            .map_err(AppError::rate_limited)
    }

    /// An MCP or public API call into `workspace`, from any of its agents
    /// and apps (H1.1). Browser calls aren't charged: people have their own
    /// per-session budgets.
    pub fn check_workspace_call(&self, workspace: Uuid) -> Result<(), AppError> {
        self.workspace_calls
            .check(workspace)
            .map_err(AppError::rate_limited)
    }

    /// A bug report or public form submission into `workspace`, once its
    /// link resolved.
    pub fn check_workspace_public_submission(&self, workspace: Uuid) -> Result<(), AppError> {
        self.workspace_public_submissions
            .check(workspace)
            .map_err(AppError::rate_limited)
    }

    /// One run report (run_open, run_update, run_activity and the like)
    /// from `account_id`.
    pub fn check_run_report(&self, account_id: Uuid) -> Result<(), AppError> {
        self.run_reports
            .check(account_id)
            .map_err(AppError::rate_limited)
    }

    /// Before a bearer token is looked up: charged to the token. While
    /// tokens that didn't resolve have spent the shared budget, only a token
    /// that authenticated lately is looked up.
    pub fn check_mcp_auth(&self, untrusted_token: &str) -> Result<(), AppError> {
        let key = digest(untrusted_token);
        let now = Instant::now();
        if let Some(retry_after) = self.global_mcp_auth_failures.spent_at(&(), now)
            && !self.recent_mcp_tokens.contains_at(&key, now)
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.mcp_auth_attempts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// A bearer token that didn't resolve.
    pub fn note_mcp_auth_failure(&self) {
        let _ = self.global_mcp_auth_failures.check(());
    }

    /// A bearer token that resolved; only its digest is kept.
    pub fn note_mcp_auth_success(&self, token: &str) {
        self.recent_mcp_tokens.insert(digest(token), Instant::now());
    }

    /// A step of an app's sign-in ceremony from `address` (the caller's,
    /// when known), charged to the sender, or to the shared budget for
    /// callers with no address, and then to the app it names.
    pub fn check_oauth_ceremony(
        &self,
        address: Option<&str>,
        untrusted_key: &str,
    ) -> Result<(), AppError> {
        match address {
            Some(address) => self.oauth_ceremony_senders.check(digest(address)),
            None => self.global_oauth_ceremonies.check(()),
        }
        .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(untrusted_key.as_bytes()).into();
        self.oauth_ceremonies
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// An app registration from `address` (the caller's, when known).
    pub fn check_oauth_registration_sender(&self, address: Option<&str>) -> Result<(), AppError> {
        match address {
            Some(address) => self
                .oauth_registration_senders
                .check(digest(address))
                .map_err(AppError::rate_limited),
            None => Ok(()),
        }
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

    /// `None` means the caller has no distinguishing key; only those share
    /// the global ceiling, so a caller with an address never waits on it.
    pub fn check_workos_start(&self, untrusted_key: Option<&str>) -> Result<(), AppError> {
        let Some(untrusted_key) = untrusted_key else {
            return self
                .global_workos_starts
                .check(())
                .map_err(AppError::rate_limited);
        };
        let key: [u8; 32] = Sha256::digest(untrusted_key.as_bytes()).into();
        self.workos_starts
            .check(key)
            .map_err(AppError::rate_limited)
    }

    /// A password reset request from `address` (the caller's, when known)
    /// for `normalized_key` (a login name or an email address, lower case).
    /// The sender's budget comes first (or, with no address, the one all
    /// such callers share), then the account's.
    pub fn check_password_reset(
        &self,
        address: Option<&str>,
        normalized_key: &str,
    ) -> Result<(), AppError> {
        match address {
            Some(address) => self.password_reset_senders.check(digest(address)),
            None => self.global_password_resets.check(()),
        }
        .map_err(AppError::rate_limited)?;
        self.password_resets
            .check(digest(normalized_key))
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
    /// The workspace's own budget is charged once the link resolves
    /// ([`Self::check_workspace_public_submission`]).
    pub fn check_bug_report(&self, token_digest: &[u8]) -> Result<(), AppError> {
        let key: [u8; 32] = Sha256::digest(token_digest).into();
        self.bug_reports.check(key).map_err(AppError::rate_limited)
    }
}

impl RateLimiters {
    /// The limiters, with the ceilings the configuration sets.
    pub fn for_config(config: &crate::config::AppConfig) -> Self {
        Self::default().with_workspace_calls_per_window(config.workspace_calls_per_minute)
    }

    /// The per-workspace call ceiling set to `per_window` a minute.
    pub fn with_workspace_calls_per_window(mut self, per_window: u32) -> Self {
        self.workspace_calls = FixedWindowLimiter::new(per_window.max(1), WINDOW, MAX_SESSION_KEYS);
        self
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
            global_login_failures: FixedWindowLimiter::new(
                GLOBAL_LOGIN_FAILURES_PER_WINDOW,
                WINDOW,
                1,
            ),
            recent_logins: RecentKeys::new(RECENT_LOGIN_TTL, MAX_LOGIN_KEYS),
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
            global_invitation_failures: FixedWindowLimiter::new(
                GLOBAL_INVITATION_FAILURES_PER_WINDOW,
                WINDOW,
                1,
            ),
            mcp_requests: FixedWindowLimiter::new(
                MCP_REQUESTS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            workspace_calls: FixedWindowLimiter::new(
                WORKSPACE_CALLS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            run_reports: FixedWindowLimiter::new(
                RUN_REPORTS_PER_WINDOW,
                RUN_REPORT_WINDOW,
                MAX_SESSION_KEYS,
            ),
            mcp_auth_attempts: FixedWindowLimiter::new(
                MCP_AUTH_ATTEMPTS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            global_mcp_auth_failures: FixedWindowLimiter::new(
                GLOBAL_MCP_AUTH_FAILURES_PER_WINDOW,
                WINDOW,
                1,
            ),
            recent_mcp_tokens: RecentKeys::new(RECENT_MCP_TOKEN_TTL, MAX_SESSION_KEYS),
            oauth_ceremonies: FixedWindowLimiter::new(
                OAUTH_CEREMONIES_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            oauth_ceremony_senders: FixedWindowLimiter::new(
                OAUTH_CEREMONIES_PER_ADDRESS_PER_WINDOW,
                WINDOW,
                MAX_LOGIN_KEYS,
            ),
            oauth_registration_senders: FixedWindowLimiter::new(
                OAUTH_REGISTRATIONS_PER_ADDRESS_PER_WINDOW,
                WINDOW,
                MAX_LOGIN_KEYS,
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
            workspace_public_submissions: FixedWindowLimiter::new(
                WORKSPACE_PUBLIC_SUBMISSIONS_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
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
        GLOBAL_LOGIN_FAILURES_PER_WINDOW, GLOBAL_MCP_AUTH_FAILURES_PER_WINDOW,
        GLOBAL_OAUTH_REFRESH_FAILURES_PER_WINDOW, MCP_AUTH_ATTEMPTS_PER_WINDOW,
        MCP_REQUESTS_PER_WINDOW, OAUTH_CEREMONIES_PER_WINDOW,
        OAUTH_REFRESH_FAILURES_PER_CLIENT_PER_WINDOW, OAUTH_REFRESH_FAILURES_PER_WINDOW,
        OAUTH_REFRESHES_PER_WINDOW, RateLimiters, WEBHOOK_ATTEMPTS_PER_WINDOW,
    };
    use super::{
        GLOBAL_INVITATION_FAILURES_PER_WINDOW, GLOBAL_WORKOS_STARTS_PER_WINDOW, LIMITS, LimitScope,
        SERVER_WIDE, WORKSPACE_PUBLIC_SUBMISSIONS_PER_WINDOW,
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

    /// P0.13: failed sign-ins for junk names spend the shared budget; a
    /// name that signed in lately still gets through, a never-seen one
    /// doesn't.
    #[test]
    fn junk_sign_ins_dont_lock_out_people_who_signed_in_lately() {
        let limiters = RateLimiters::default();
        limiters.note_login_success("alex");
        for index in 0..GLOBAL_LOGIN_FAILURES_PER_WINDOW {
            assert!(
                limiters.check_login(&format!("junk-{index}")).is_ok(),
                "junk name {index} is inside the failure budget"
            );
            limiters.note_login_failure();
        }
        assert!(limiters.check_login("alex").is_ok());
        assert!(matches!(
            limiters.check_login("never-seen"),
            Err(AppError::RateLimited { .. })
        ));
        // Successful sign-ins never spend it.
        let limiters = RateLimiters::default();
        for index in 0..(GLOBAL_LOGIN_FAILURES_PER_WINDOW * 2) {
            let name = format!("person-{index}");
            assert!(limiters.check_login(&name).is_ok());
            limiters.note_login_success(&name);
        }
        assert!(limiters.check_login("someone-new").is_ok());
    }

    /// P0.13: valid tokens never share a budget; junk tokens spend the
    /// failure budget without refusing a token that authenticated earlier.
    #[test]
    fn valid_mcp_tokens_never_share_a_global_budget() {
        let limiters = RateLimiters::default();
        for request in 0..1_000 {
            let token = format!("token-{}", request % 20);
            assert!(limiters.check_mcp_auth(&token).is_ok(), "{request}");
            limiters.note_mcp_auth_success(&token);
        }
        for index in 0..GLOBAL_MCP_AUTH_FAILURES_PER_WINDOW {
            assert!(limiters.check_mcp_auth(&format!("junk-{index}")).is_ok());
            limiters.note_mcp_auth_failure();
        }
        assert!(limiters.check_mcp_auth("token-3").is_ok());
        assert!(matches!(
            limiters.check_mcp_auth("never-seen"),
            Err(AppError::RateLimited { .. })
        ));
    }

    #[test]
    fn kdf_concurrency_is_bounded() {
        let limiters = RateLimiters::default();

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
            assert!(limiters.check_oauth_ceremony(None, "client-a").is_ok());
        }
        assert!(matches!(
            limiters.check_oauth_ceremony(None, "client-a"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_oauth_ceremony(None, "client-b").is_ok());
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

    /// H1.1: every limiter in [`RateLimiters`] is declared, with its scope,
    /// in [`LIMITS`], and nothing is declared that doesn't exist.
    #[test]
    fn every_limiter_is_declared_with_a_scope() {
        let source = include_str!("rate_limit.rs");
        let start = source
            .find("pub struct RateLimiters {")
            .expect("the struct");
        let body = &source[start..start + source[start..].find("\n}").expect("its end")];
        let fields = body
            .lines()
            .filter(|line| line.contains("FixedWindowLimiter<"))
            .filter_map(|line| line.trim().split(':').next())
            .collect::<std::collections::BTreeSet<_>>();
        let declared = LIMITS
            .iter()
            .map(|limit| limit.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            fields, declared,
            "declare every limiter in LIMITS, with its scope"
        );
        assert_eq!(
            declared.len(),
            LIMITS.len(),
            "each limiter is declared once"
        );
    }

    /// H1.1 (the guard of H1 6.2): no new budget every caller shares. A
    /// server-wide one is spent only by failures, junk or callers with no
    /// address, or is one of the few listed with their reasons.
    #[test]
    fn no_new_server_wide_limit() {
        let server_wide = LIMITS
            .iter()
            .filter(|limit| limit.scope == LimitScope::Server)
            .map(|limit| limit.name)
            .collect::<Vec<_>>();
        let listed = SERVER_WIDE
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        assert_eq!(
            server_wide, listed,
            "a limiter every caller shares needs a plan change, not a new entry"
        );
        assert!(SERVER_WIDE.iter().all(|(_, why)| !why.is_empty()));
    }

    /// H1.1: one workspace's agents spending their budget don't refuse
    /// another workspace's.
    #[test]
    fn one_workspace_spending_its_calls_leaves_another_alone() {
        let limiters = RateLimiters::default().with_workspace_calls_per_window(5);
        let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
        for _ in 0..5 {
            assert!(limiters.check_workspace_call(first).is_ok());
        }
        assert!(matches!(
            limiters.check_workspace_call(first),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_workspace_call(second).is_ok());

        for _ in 0..WORKSPACE_PUBLIC_SUBMISSIONS_PER_WINDOW {
            assert!(limiters.check_workspace_public_submission(first).is_ok());
        }
        assert!(matches!(
            limiters.check_workspace_public_submission(first),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_workspace_public_submission(second).is_ok());
    }

    /// H1.1: callers the server can tell apart never share a budget: good
    /// refreshes, sign-in starts and reset requests from known addresses,
    /// and link tokens that resolve.
    #[test]
    fn good_callers_never_spend_a_server_wide_budget() {
        let limiters = RateLimiters::default();
        for client in 0..(OAUTH_REFRESHES_PER_WINDOW * 5) {
            assert!(
                limiters
                    .check_oauth_refresh(&format!("app-{client}"))
                    .is_ok()
            );
        }
        for sender in 0..(GLOBAL_WORKOS_STARTS_PER_WINDOW * 2) {
            assert!(
                limiters
                    .check_workos_start(Some(&format!("198.51.100.{sender}")))
                    .is_ok()
            );
        }
        for sender in 0..300 {
            assert!(
                limiters
                    .check_password_reset(
                        Some(&format!("192.0.2.{sender}")),
                        &format!("person-{sender}")
                    )
                    .is_ok()
            );
        }
        for link in 0..(GLOBAL_INVITATION_FAILURES_PER_WINDOW * 2) {
            assert!(limiters.check_invitation(&format!("aw_inv_{link}")).is_ok());
        }
        for sender in 0..300 {
            assert!(
                limiters
                    .check_oauth_ceremony(
                        Some(&format!("203.0.113.{sender}")),
                        &format!("app-{sender}")
                    )
                    .is_ok()
            );
        }
        // Misses spend the link budget; then lookups wait.
        for _ in 0..GLOBAL_INVITATION_FAILURES_PER_WINDOW {
            limiters.note_invitation_failure();
        }
        assert!(matches!(
            limiters.check_invitation("aw_inv_new"),
            Err(AppError::RateLimited { .. })
        ));
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
