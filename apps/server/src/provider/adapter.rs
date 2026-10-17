//! The harness adapter interface (H2): what the run bookkeeping in
//! `provider.rs`, the scheduler, recovery and the account pages need from any
//! coding-agent harness. Codex App Server is one adapter and the in-process
//! demo harness another; neither's wire format leaves its own module.
//!
//! An adapter hands out sessions. A *run* session holds one of the account's
//! run slots for as long as a turn is driven; a *control* session (account
//! status, sign-in, model listing) never takes a run slot, so opening account
//! pages can't block runs (F8).

use std::{future::Future, path::Path, path::PathBuf, pin::Pin};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use super::{
    manifest::{AdapterManifest, ModelEntry},
    run_config::RunConfig,
};

pub type HarnessFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HarnessError>> + Send + 'a>>;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("the host run limit is reached")]
    HostCapacity,
    #[error("the account's run limit is reached")]
    AccountCapacity,
    #[error("the account's idle process limit is reached")]
    ControlCapacity,
    #[error("the harness is shutting down")]
    ShuttingDown,
    #[error("the harness circuit is temporarily open")]
    CircuitOpen,
    #[error("the account configuration changed while its harness was active")]
    ConfigurationChanged,
    #[error("the harness transport failed")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the harness response omitted {0}")]
    MissingResult(&'static str),
    #[error("the harness returned an invalid response")]
    InvalidResponse,
    #[error("the harness configuration is invalid")]
    InvalidConfiguration,
    #[error("the harness does not support {0}")]
    Unsupported(&'static str),
    #[error("no harness adapter is registered for this kind")]
    UnknownAdapter,
}

/// The account a session runs as: its private home directory and how many
/// turns it may run at once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountHandle {
    pub id: Uuid,
    pub home: PathBuf,
    pub max_concurrent_runs: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    CommandExecution,
    FileChange,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    Approval,
    Question,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RequestOption {
    pub id: String,
    pub label: String,
}

/// A harness asking the people in the thread for something. `token` is the
/// adapter's own handle for answering it and is never read outside the
/// adapter.
#[derive(Debug)]
pub struct HarnessRequest {
    pub kind: RequestKind,
    pub durable_id: String,
    pub summary: String,
    pub options: Vec<RequestOption>,
    pub token: Value,
}

impl HarnessRequest {
    pub fn public_options(&self) -> Value {
        serde_json::to_value(&self.options).unwrap_or_else(|_| Value::Array(Vec::new()))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct HarnessItem {
    pub key: String,
    pub kind: ItemKind,
}

/// The neutral events a turn produces.
#[derive(Debug)]
pub enum HarnessEvent {
    MessageDelta(String),
    ItemStarted(HarnessItem),
    ItemCompleted(HarnessItem),
    TurnCompleted {
        completed: bool,
    },
    Request(HarnessRequest),
    /// The harness reported fresh usage while running (U1). Sparse: windows
    /// it leaves out keep their last reading.
    UsageUpdated(UsageReport),
    Error,
    Ignored,
}

#[derive(Debug, Eq, PartialEq)]
pub enum HarnessAnswer {
    Approve,
    Deny,
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredTurn {
    pub id: Option<String>,
    pub status: String,
    pub final_message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSummary {
    pub authenticated: bool,
    pub account_type: Option<String>,
    pub email: Option<String>,
}

#[derive(Clone, Eq, PartialEq)]
pub enum LoginStart {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginKind {
    Browser,
    DeviceCode,
    ApiKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitWindow {
    pub used_percent: u16,
    pub resets_at: Option<i64>,
    pub window_duration_minutes: Option<i64>,
}

/// The account's usage windows, as far as the harness reports them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitSummary {
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
}

/// One usage window of an account (U1): a five-hour session, a week, a
/// month of credits. Plans differ, so an account has whichever windows its
/// harness reports, possibly only a weekly one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageWindow {
    /// Stable for the account across reads, such as `codex.secondary`:
    /// letters, digits and `._-`, at most 64 characters.
    pub key: String,
    /// The harness's own name for the limit, when it gives one.
    pub limit_name: Option<String>,
    pub used_percent: u16,
    /// Unix seconds.
    pub resets_at: Option<i64>,
    pub duration_minutes: Option<i64>,
    /// The harness says this limit is reached.
    pub limit_reached: bool,
}

/// A banked reset (U6): spending one refills the account's windows now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetCredit {
    /// The harness's opaque id for it.
    pub id: String,
    pub title: Option<String>,
    /// Unix seconds; `None` when it doesn't expire.
    pub expires_at: Option<i64>,
    pub granted_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResetCredits {
    pub available: u32,
    /// Detail for some or all of the available ones.
    pub credits: Vec<ResetCredit>,
}

/// Everything a harness says about an account's usage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageReport {
    pub windows: Vec<UsageWindow>,
    /// `None` when the harness has no banked resets.
    pub reset_credits: Option<ResetCredits>,
    /// The plan's name, such as `pro`, when the harness says.
    pub plan: Option<String>,
}

impl UsageReport {
    /// The two-window summary older harnesses report, as windows `primary`
    /// and `secondary`.
    pub fn from_rate_limits(summary: RateLimitSummary) -> Self {
        let window = |key: &str, window: RateLimitWindow| UsageWindow {
            key: key.to_owned(),
            limit_name: None,
            used_percent: window.used_percent,
            resets_at: window.resets_at,
            duration_minutes: window.window_duration_minutes,
            limit_reached: window.used_percent >= 100,
        };
        Self {
            windows: [
                summary.primary.map(|value| window("primary", value)),
                summary.secondary.map(|value| window("secondary", value)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            reset_credits: None,
            plan: None,
        }
    }
}

/// What spending a banked reset did.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    /// The same attempt already went through.
    AlreadyRedeemed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProfileHealth {
    pub process_running: bool,
    pub consecutive_failures: u32,
    pub circuit_open: bool,
    pub process_starts: u64,
}

/// One leased connection to a harness for one account.
pub trait HarnessSession: Send {
    fn account_id(&self) -> Uuid;

    /// The account was signed out or is being stopped; drop the turn.
    fn is_stopping(&self) -> bool;

    fn start_thread<'a>(
        &'a mut self,
        cwd: &'a Path,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String>;

    fn resume_thread<'a>(
        &'a mut self,
        thread_id: &'a str,
        cwd: &'a Path,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()>;

    fn start_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        cwd: &'a Path,
        prompt: &'a str,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String>;

    fn interrupt_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, ()>;

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent>;

    fn answer(&mut self, request: HarnessRequest, answer: HarnessAnswer) -> HarnessFuture<'_, ()>;

    fn read_latest_turn<'a>(&'a mut self, thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn>;

    /// One known turn, for recovery. Harnesses whose threads hold one turn
    /// at a time read the latest; ones that track turns on their own (a
    /// cloud task) read that turn.
    fn read_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        _turn_id: &'a str,
    ) -> HarnessFuture<'a, RecoveredTurn> {
        self.read_latest_turn(thread_id)
    }

    /// Follow a turn that was started before a restart and is still running
    /// elsewhere, so `next_event` reports it again. Only harnesses whose
    /// turns outlive this server (cloud tasks) can.
    fn follow_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        _turn_id: &'a str,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async {
            Err(HarnessError::Unsupported(
                "following a turn after a restart",
            ))
        })
    }

    /// A link people can open to watch the turn where it runs, such as a
    /// cloud task's page.
    fn turn_link(&self, _turn_id: &str) -> Option<String> {
        None
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary>;

    fn start_login(&mut self, kind: LoginKind) -> HarnessFuture<'_, LoginStart>;

    fn login_with_api_key<'a>(&'a mut self, api_key: &'a str) -> HarnessFuture<'a, LoginStart>;

    fn cancel_login<'a>(&'a mut self, login_id: &'a str) -> HarnessFuture<'a, ()>;

    fn logout(&mut self) -> HarnessFuture<'_, ()>;

    fn read_rate_limits(&mut self) -> HarnessFuture<'_, RateLimitSummary>;

    /// Every usage window and banked reset the harness reports (U1). The
    /// default turns [`read_rate_limits`](Self::read_rate_limits) into
    /// windows.
    fn read_usage(&mut self) -> HarnessFuture<'_, UsageReport> {
        Box::pin(async move {
            Ok(UsageReport::from_rate_limits(
                self.read_rate_limits().await?,
            ))
        })
    }

    /// Spend one banked reset (U6); `credit_id` picks which, or the harness
    /// chooses. `idempotency_key` is the same on every retry of one attempt.
    fn spend_reset_credit<'a>(
        &'a mut self,
        _credit_id: Option<&'a str>,
        _idempotency_key: &'a str,
    ) -> HarnessFuture<'a, ResetOutcome> {
        Box::pin(async { Err(HarnessError::Unsupported("banked resets")) })
    }

    /// The models the harness offers right now, for adapters whose manifest
    /// says they can list them.
    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>>;

    /// Stop the underlying process (if any) without waiting for the turn.
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// A harness the server can run agents on.
pub trait HarnessAdapter: Send + Sync {
    fn manifest(&self) -> &AdapterManifest;

    /// A session that holds one of the account's run slots until dropped.
    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>>;

    /// A session for account status, sign-in and model listing. It never
    /// takes a run slot and never waits for one.
    fn acquire_control(&self, account: AccountHandle)
    -> HarnessFuture<'_, Box<dyn HarnessSession>>;

    fn health(
        &self,
        account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>>;

    /// Stop everything running for the account (sign-out).
    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}
