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
    limits::{Bucket, LimitKey, LimitSource, Limits, Recent, Tier},
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
/// Link tokens (invitations, confirmation and reset links) that were
/// malformed or matched nothing, across every caller. Only misses spend it;
/// while it's spent, link lookups wait.
pub static INVITATION_FAILURES: Bucket = exact("invitations.failures", 120, MINUTE);
pub static MCP_REQUESTS: Bucket = chunked("mcp.requests", 600, MINUTE, 10);
/// MCP and public API calls into one workspace, from all its agents and
/// apps together (H1.1): one workspace can't take the server's capacity.
/// `AGENT_WORKSPACE_WORKSPACE_CALLS_PER_MINUTE` sets another number.
pub const WORKSPACE_CALLS_PER_WINDOW: u32 = 3_000;
pub static WORKSPACE_CALLS: Bucket =
    chunked("workspace.calls", WORKSPACE_CALLS_PER_WINDOW, MINUTE, 10);
pub static MCP_AUTH: Bucket = exact("mcp.auth", 600, MINUTE);
/// Run reports (A1.3) from one account: 20 a second on average, in bursts
/// of up to 100.
pub static RUN_REPORTS: Bucket = chunked("runs.reports", 100, Duration::from_secs(5), 5);
/// A run's streamed reply (A1.17), per run credential: kept apart from
/// [`MCP_AUTH`] so live words never starve the run's own tool calls. Hosts
/// gather words for a moment, so 20 a second is room to spare.
pub static RUN_STREAMS: Bucket = exact("runs.stream", 1200, MINUTE);
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
/// App sign-in ceremonies from one sender address, when the address is
/// known.
pub static OAUTH_CEREMONY_SENDERS: Bucket = exact("oauth.ceremonies.address", 60, MINUTE);
/// Ceremonies from callers with no known address, across all of them.
pub static OAUTH_CEREMONIES_GLOBAL: Bucket = exact("oauth.ceremonies.global", 240, MINUTE);
/// App registrations from one sender address, when the address is known.
pub static OAUTH_REGISTRATION_SENDERS: Bucket = exact("oauth.registrations.address", 20, MINUTE);
/// Token refreshes have their own budget: one app running many sessions on a
/// grant refreshes from all of them at once, and a refused refresh drops the
/// app's tools, so it mustn't share the sign-in ceremonies' smaller one.
pub static OAUTH_REFRESHES: Bucket = exact("oauth.refreshes", 300, MINUTE);
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
/// Starts from callers with no known address, across all of them.
pub static WORKOS_STARTS_GLOBAL: Bucket = exact("workos.starts.global", 600, MINUTE);
pub static WEBHOOK_ATTEMPTS: Bucket = exact("webhooks.attempts", 300, MINUTE);
/// Reports sent to one bug report link: enough for a burst of real
/// reports, not for a loop to flood a project.
pub static BUG_REPORTS: Bucket = exact("bug_reports", 30, MINUTE);
/// Deliveries to one incoming hook (K3.26), before its signature is
/// checked; the workspace's public budget is charged too.
pub static INCOMING_HOOK_DELIVERIES: Bucket = exact("incoming_hooks.deliveries", 120, MINUTE);
/// Bug reports and public form submissions into one workspace, across all
/// its links (H1.1).
pub static WORKSPACE_PUBLIC_SUBMISSIONS: Bucket =
    exact("workspace.public_submissions", 300, MINUTE);
/// Custom emoji changes in one workspace, charged by the events each one
/// sends: one to every space, so an emoji added in a workspace of many
/// spaces costs that many.
pub static CUSTOM_EMOJI_EVENTS: Bucket = exact("custom_emoji.events", 2_000, HOUR);
/// Answers to one public form from one sender address (T1.29).
pub static FORM_ANSWER_SENDERS: Bucket = exact(
    "form_answers.address",
    crate::collection_forms::ANSWERS_PER_ADDRESS_PER_HOUR,
    HOUR,
);
/// Password reset requests: a few per account an hour and a few dozen per
/// sender address; callers with no known address share one ceiling. The mail itself has its own ceiling,
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

    /// A link token (an invitation, a confirmation or reset link) about to
    /// be looked up, charged to the token. While misses everywhere have
    /// spent the shared budget, lookups wait; a hit never spends it.
    pub async fn check_invitation(&self, token_prefix: &str) -> Result<(), AppError> {
        if let Some(retry_after) = self
            .limits
            .spent_or_open(&INVITATION_FAILURES, LimitKey::Global)
            .await
        {
            return Err(AppError::rate_limited(retry_after));
        }
        self.limits
            .check(&INVITATIONS, LimitKey::Secret(&digest(token_prefix)))
            .await
    }

    /// A link token that was malformed or matched nothing.
    pub async fn note_invitation_failure(&self) {
        self.limits
            .note(&INVITATION_FAILURES, LimitKey::Global)
            .await;
    }

    /// Passes `result` through, charging a miss (`NotFound`) to the shared
    /// link-failure budget.
    pub async fn charge_link_miss<T>(&self, result: Result<T, AppError>) -> Result<T, AppError> {
        if matches!(result, Err(AppError::NotFound(_))) {
            self.note_invitation_failure().await;
        }
        result
    }

    /// [`Self::charge_link_miss`] for a lookup made on `held` (the caller's
    /// transaction or connection): on a miss `held` goes back to the pool
    /// before the shared counter takes a connection of its own, so a
    /// one-connection pool never waits on itself. On a hit both come back.
    pub async fn release_on_link_miss<T, H>(
        &self,
        held: H,
        result: Result<T, AppError>,
    ) -> Result<(T, H), AppError> {
        match result {
            Ok(value) => Ok((value, held)),
            Err(error) => {
                drop(held);
                self.charge_link_miss(Err(error)).await
            }
        }
    }

    pub async fn check_mcp_request(&self, credential_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&MCP_REQUESTS, LimitKey::Credential(credential_id))
            .await
    }

    /// An MCP or public API call into `workspace`, from any of its agents
    /// and apps (H1.1). Browser calls aren't charged: people have their own
    /// per-session budgets.
    pub async fn check_workspace_call(&self, workspace: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&WORKSPACE_CALLS, LimitKey::Workspace(workspace))
            .await
    }

    /// A bug report or public form submission into `workspace`, once its
    /// link resolved.
    pub async fn check_workspace_public_submission(&self, workspace: Uuid) -> Result<(), AppError> {
        self.limits
            .check(
                &WORKSPACE_PUBLIC_SUBMISSIONS,
                LimitKey::Workspace(workspace),
            )
            .await
    }

    /// A custom emoji change in `workspace` that will send `events` events
    /// (one to each of its spaces), charged before it's made.
    pub async fn check_custom_emoji_events(
        &self,
        workspace: Uuid,
        events: u32,
    ) -> Result<(), AppError> {
        self.limits
            .check_cost(
                &CUSTOM_EMOJI_EVENTS,
                LimitKey::Workspace(workspace),
                events.max(1),
            )
            .await
    }

    /// One run report (run_open, run_update, run_activity and the like)
    /// from `account_id`.
    pub async fn check_run_report(&self, account_id: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&RUN_REPORTS, LimitKey::Account(account_id))
            .await
    }

    /// [`Self::check_mcp_auth`] for a piece of a run's streamed reply,
    /// charged to its own budget.
    pub async fn check_run_stream(&self, untrusted_token: &str) -> Result<(), AppError> {
        self.check_token(&RUN_STREAMS, untrusted_token).await
    }

    /// Before a bearer token is looked up: charged to the token. While
    /// tokens that didn't resolve have spent the shared budget, only a token
    /// that authenticated lately is looked up.
    pub async fn check_mcp_auth(&self, untrusted_token: &str) -> Result<(), AppError> {
        self.check_token(&MCP_AUTH, untrusted_token).await
    }

    async fn check_token(
        &self,
        bucket: &'static Bucket,
        untrusted_token: &str,
    ) -> Result<(), AppError> {
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
        self.limits.check(bucket, LimitKey::Secret(&key)).await
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

    /// A step of an app's sign-in ceremony from `address` (the caller's,
    /// when known), charged to the sender, or to the shared budget for
    /// callers with no address, and then to the app it names.
    pub async fn check_oauth_ceremony(
        &self,
        address: Option<&str>,
        untrusted_key: &str,
    ) -> Result<(), AppError> {
        match address {
            Some(address) => {
                self.limits
                    .check(&OAUTH_CEREMONY_SENDERS, LimitKey::Address(address))
                    .await?
            }
            None => {
                self.limits
                    .check(&OAUTH_CEREMONIES_GLOBAL, LimitKey::Global)
                    .await?
            }
        }
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

    /// `None` means the caller has no distinguishing key; only those share
    /// the global ceiling, so a caller with an address never waits on it.
    pub async fn check_workos_start(&self, untrusted_key: Option<&str>) -> Result<(), AppError> {
        let Some(untrusted_key) = untrusted_key else {
            return self
                .limits
                .check(&WORKOS_STARTS_GLOBAL, LimitKey::Global)
                .await;
        };
        self.limits
            .check(&WORKOS_STARTS, LimitKey::Secret(&digest(untrusted_key)))
            .await
    }

    /// A password reset request from `address` (the caller's, when known)
    /// for `normalized_key` (a login name or an email address, lower case).
    /// The sender's budget comes first (or, with no address, the one all
    /// such callers share), then the account's.
    pub async fn check_password_reset(
        &self,
        address: Option<&str>,
        normalized_key: &str,
    ) -> Result<(), AppError> {
        match address {
            Some(address) => {
                self.limits
                    .check(&PASSWORD_RESET_SENDERS, LimitKey::Address(address))
                    .await?
            }
            None => {
                self.limits
                    .check(&PASSWORD_RESETS_GLOBAL, LimitKey::Global)
                    .await?
            }
        }
        self.limits
            .check(&PASSWORD_RESETS, LimitKey::Secret(&digest(normalized_key)))
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

    /// A delivery to the incoming hook `hook` (K3.26).
    pub async fn check_incoming_hook(&self, hook: Uuid) -> Result<(), AppError> {
        self.limits
            .check(&INCOMING_HOOK_DELIVERIES, LimitKey::Secret(hook.as_bytes()))
            .await
    }

    pub async fn check_webhook_attempt(&self) -> Result<(), AppError> {
        self.limits.check(&WEBHOOK_ATTEMPTS, LimitKey::Global).await
    }

    /// A report sent to the bug report link whose token has this digest.
    /// The workspace's own budget is charged once the link resolves
    /// ([`Self::check_workspace_public_submission`]).
    pub async fn check_bug_report(&self, token_digest: &[u8]) -> Result<(), AppError> {
        self.limits
            .check(&BUG_REPORTS, LimitKey::Secret(token_digest))
            .await
    }

    /// An answer to one public form from one sender address. A caller
    /// whose address isn't known is bounded by the form's own limits.
    pub async fn check_form_answer_sender(
        &self,
        link_id: Uuid,
        address: Option<&str>,
    ) -> Result<(), AppError> {
        let Some(address) = address else {
            return Ok(());
        };
        let mut hasher = Sha256::new();
        hasher.update(link_id.as_bytes());
        hasher.update(address.as_bytes());
        let key: [u8; 32] = hasher.finalize().into();
        self.limits
            .check(&FORM_ANSWER_SENDERS, LimitKey::Secret(&key))
            .await
    }
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self::with_workspace_calls_per_window(WORKSPACE_CALLS_PER_WINDOW)
    }
}

impl RateLimiters {
    /// The budgets, with the ceilings the configuration sets.
    pub fn for_config(config: &crate::config::AppConfig) -> Self {
        Self::with_workspace_calls_per_window(config.workspace_calls_per_minute)
    }

    /// The budgets, with the per-workspace call ceiling at `per_window` a
    /// minute.
    pub fn with_workspace_calls_per_window(per_window: u32) -> Self {
        Self {
            limits: Limits::new(Arc::new(Configured {
                workspace_calls: per_window.max(1),
            })),
            password_kdfs: Arc::new(Semaphore::new(CONCURRENT_PASSWORD_KDFS)),
        }
    }
}

/// The numbers the configuration sets; every other bucket keeps its own.
struct Configured {
    workspace_calls: u32,
}

impl LimitSource for Configured {
    fn limit_for(&self, bucket: &Bucket, _key: &LimitKey<'_>) -> Option<u32> {
        (bucket.name == WORKSPACE_CALLS.name).then_some(self.workspace_calls)
    }
}

/// Who a budget belongs to (H1.1). A budget shared by the whole server is
/// spent only by failures and junk, or by callers the server can't tell
/// apart: a caller that signed in, or sent a valid token, never shares one
/// with everyone else.
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

/// Every bucket and who it belongs to: the one list (R24 rule 3), for the
/// name check, the scope guard and anyone listing the budgets.
pub static LIMITS: &[(&Bucket, LimitScope)] = &[
    (&LOGIN, LimitScope::Login),
    (&LOGIN_HOURLY, LimitScope::Login),
    (&LOGIN_SENDERS, LimitScope::Address),
    (&LOGIN_FAILURES, LimitScope::ServerFailuresOnly),
    (&WRITES, LimitScope::Credential),
    (&EXPENSIVE_READS, LimitScope::Credential),
    (&INVITATIONS, LimitScope::Link),
    (&INVITATION_FAILURES, LimitScope::ServerFailuresOnly),
    (&MCP_REQUESTS, LimitScope::Credential),
    (&WORKSPACE_CALLS, LimitScope::Workspace),
    (&MCP_AUTH, LimitScope::Credential),
    (&RUN_REPORTS, LimitScope::Account),
    (&RUN_STREAMS, LimitScope::Credential),
    (&MCP_AUTH_FAILURES, LimitScope::ServerFailuresOnly),
    (&OAUTH_CEREMONIES, LimitScope::Client),
    (&OAUTH_CEREMONY_SENDERS, LimitScope::Address),
    (&OAUTH_CEREMONIES_GLOBAL, LimitScope::ServerUnattributed),
    (&OAUTH_REGISTRATION_SENDERS, LimitScope::Address),
    (&OAUTH_REFRESHES, LimitScope::Client),
    (&OAUTH_REFRESH_FAILURES, LimitScope::Address),
    (
        &OAUTH_REFRESH_FAILURES_GLOBAL,
        LimitScope::ServerFailuresOnly,
    ),
    (&OAUTH_REFRESH_FAILURES_PER_CLIENT, LimitScope::Client),
    (&WORKOS_STARTS, LimitScope::Address),
    (&WORKOS_STARTS_GLOBAL, LimitScope::ServerUnattributed),
    (&WEBHOOK_ATTEMPTS, LimitScope::Server),
    (&BUG_REPORTS, LimitScope::Link),
    (&INCOMING_HOOK_DELIVERIES, LimitScope::Link),
    (&WORKSPACE_PUBLIC_SUBMISSIONS, LimitScope::Workspace),
    (&CUSTOM_EMOJI_EVENTS, LimitScope::Workspace),
    (&FORM_ANSWER_SENDERS, LimitScope::Address),
    (&PASSWORD_RESETS, LimitScope::Login),
    (&PASSWORD_RESET_SENDERS, LimitScope::Address),
    (&PASSWORD_RESETS_GLOBAL, LimitScope::ServerUnattributed),
    (&RESET_EMAILS, LimitScope::Server),
];

/// The only budgets every caller shares, and why each can't be narrower.
/// A guard test holds [`LIMITS`] to this list: a new one is a plan change.
pub const SERVER_WIDE: &[(&str, &str)] = &[
    (
        "webhooks.attempts",
        "GitHub deliveries come from GitHub's shared addresses before any workspace is known; \
         this bounds signature checks",
    ),
    (
        "password_resets.emails",
        "reset emails leave through the server's own mail sender, whose standing every \
         workspace shares",
    ),
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
    use super::{CONCURRENT_PASSWORD_KDFS, LIMITS, LimitScope, RateLimiters, SERVER_WIDE};

    /// H1.1: every bucket in this file is declared, with its scope, in
    /// [`LIMITS`], and each once.
    #[test]
    fn every_bucket_is_declared_with_a_scope() {
        let source = include_str!("rate_limit.rs");
        let statics = source
            .lines()
            .filter_map(|line| line.strip_prefix("pub static "))
            .filter_map(|rest| rest.split_once(": Bucket"))
            .map(|(name, _)| name.trim())
            .collect::<std::collections::BTreeSet<_>>();
        let list = &source[source.find("pub static LIMITS").expect("the list")..];
        let list =
            &list[list.find("= &[").expect("its start")..list.find("\n];").expect("its end")];
        let declared = list
            .split('&')
            .skip(1)
            .map(|rest| {
                rest.split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
                    .next()
                    .unwrap_or_default()
            })
            .filter(|name| !name.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            statics, declared,
            "declare every bucket in LIMITS, with its scope"
        );
        assert_eq!(declared.len(), LIMITS.len(), "each bucket is declared once");
    }

    /// H1.1 (the guard of H1 6.2): no new budget every caller shares. A
    /// server-wide one is spent only by failures, junk or callers with no
    /// address, or is one of the few listed with their reasons.
    #[test]
    fn no_new_server_wide_limit() {
        let server_wide = LIMITS
            .iter()
            .filter(|(_, scope)| *scope == LimitScope::Server)
            .map(|(bucket, _)| bucket.name)
            .collect::<Vec<_>>();
        let listed = SERVER_WIDE
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        assert_eq!(
            server_wide, listed,
            "a budget every caller shares needs a plan change, not a new entry"
        );
        assert!(SERVER_WIDE.iter().all(|(_, why)| !why.is_empty()));
    }

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
        let names = super::LIMITS.iter().map(|(bucket, _)| bucket.name);
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
