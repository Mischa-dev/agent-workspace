//! A1.17 step 4: model accounts are the built-in host's. The server asks
//! about one (its status, a sign-in, a sign-out, a banked reset, its models,
//! whether a cloud environment is there) with an
//! `action` command to the workspace's built-in host row
//! ([`agent_workspace_host_protocol::account`]), and the host carries it out
//! with the account's harness and answers in its acknowledgement.
//!
//! - [`ask`] is the server's half. When this process carries the built-in
//!   host (embedded), it writes the command already taken, so the host's own
//!   loop doesn't also pick it up, and carries it out at once through the
//!   protocol in process. Otherwise (the host's own program, or another
//!   instance holds it) it waits for the host's answer. Either way no
//!   transaction is held while the harness works (rule 14), and an
//!   answered question is deleted once read: no row keeps what the account
//!   said, and polls don't pile up.
//! - [`answer`] is the host's half, run from its command loop.

use std::{path::PathBuf, time::Duration};

use agent_workspace_builtin_host::runtime::commands::{self as host_commands, refused};
use agent_workspace_host_protocol::{
    Ack, AckOutcome, Command, CommandKind,
    account::{
        ACCOUNT_ACTION, AccountAnswer, AccountAsk, AccountQuestion, LoginStarted, REFUSED_BUSY,
        REFUSED_FAILED, REFUSED_UNSUPPORTED, RateLimitWindow, RateLimits, ResetSpent,
    },
};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    hosts::commands::{self, NewCommand},
    provider::{AccountHandle, HarnessError, LoginStart, RateLimitSummary, adapter::ResetOutcome},
};

/// How long a question waits for the host to take it up when the host runs
/// elsewhere. One the host hasn't taken by then is closed.
const PICKUP_WAIT: Duration = Duration::from_secs(20);
/// Beyond the harness's own request limits, how long a question the host
/// has taken may still take: a control slot may need five seconds to free.
const IN_FLIGHT_MARGIN: Duration = Duration::from_secs(15);
/// The most harness requests one question makes (a status reads the
/// account, then its usage windows), each within the harness's limit.
const REQUESTS_PER_QUESTION: u32 = 2;
const LOOK_EVERY: Duration = Duration::from_millis(200);

/// What the host made of a question.
#[derive(Debug)]
pub enum Asked {
    Answered(AccountAnswer),
    /// Every control session of the harness is busy.
    Busy,
    /// The harness can't do that.
    Unsupported,
    /// The harness failed (what failed, in a word, such as `transport`), or
    /// the host didn't answer in time (`unanswered`).
    Failed(String),
}

impl Asked {
    /// What the host made of the question, in a word, for logs and the
    /// stored failure of a usage read.
    pub fn category(&self) -> &str {
        match self {
            Self::Answered(_) => "unexpected_answer",
            Self::Busy => REFUSED_BUSY,
            Self::Unsupported => REFUSED_UNSUPPORTED,
            Self::Failed(category) => category,
        }
    }
}

/// Whether this process is the built-in host now: it runs the host
/// embedded and holds the host's lease (a standby leaves the question to
/// the holder, so a sign-in and its cancel reach the same harness). A
/// state without a cluster (a test's) is its own host.
fn host_is_here(state: &AppState) -> bool {
    state.config.roles.work
        && state.config.builtin_host
        && state.config.builtin_host_remote.is_none()
        && state.supervisors.cluster().is_none_or(|cluster| {
            cluster
                .held_lease(&crate::cluster::LeaseName::BuiltinHost)
                .is_some()
        })
}

/// Ask the built-in host of `workspace_id` about one of its accounts.
pub async fn ask(state: &AppState, workspace_id: Uuid, ask: AccountAsk) -> Result<Asked, AppError> {
    ask_with_key(state, workspace_id, ask, None).await
}

/// [`ask`], handing the host an API key to sign in with (`ask` is an
/// `ApiKeyLogin`). When this process is the host the key is handed over
/// in memory; otherwise it is sealed beside the sign-in for the host to
/// take once, which needs the secrets keyring.
pub async fn ask_with_key(
    state: &AppState,
    workspace_id: Uuid,
    ask: AccountAsk,
    api_key: Option<&str>,
) -> Result<Asked, AppError> {
    let here = host_is_here(state);
    if let (false, Some(key), AccountQuestion::ApiKeyLogin { ceremony_id }) =
        (here, api_key, &ask.question)
    {
        let keys = crate::secrets::keyring(&state.config).map_err(|_| {
            AppError::Conflict(
                "provider_account",
                "signing in with an API key through a separate host needs the server's secrets key",
            )
        })?;
        crate::sign_ins::seal_api_key(
            &state.pool,
            keys.as_ref(),
            ask.account_id,
            *ceremony_id,
            key,
        )
        .await?;
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let host_id = super::assistant::builtin_host_id(&mut transaction, workspace_id)
        .await?
        .ok_or(AppError::Conflict(
            "provider_account",
            "this workspace has no built-in host",
        ))?;
    let command = commands::issue(
        &mut transaction,
        NewCommand {
            workspace_id,
            host_id,
            run_id: None,
            request_id: None,
            kind: CommandKind::Action,
            start_key: None,
            body: json!({"action": ACCOUNT_ACTION, "input": ask}),
        },
    )
    .await?;
    if here {
        commands::take_here(&mut transaction, command.id).await?;
    }
    transaction.commit().await?;
    let ack = if here {
        let client = super::transport::client(state);
        let mut handed = serde_json::Map::new();
        if let Some(key) = api_key {
            handed.insert("apiKey".into(), Value::String(key.to_owned()));
        }
        let carried = crate::tenancy::system(
            "runs.host",
            host_commands::carry_out_one(&super::driver::host(state), &client, command.id, handed),
        )
        .await;
        carried.ok()
    } else {
        wait_for_answer(state, command.id).await?
    };
    commands::forget_answered(&state.pool, command.id).await?;
    Ok(match ack {
        Some(ack) => read_ack(ack),
        None => Asked::Failed("unanswered".to_owned()),
    })
}

/// The host's acknowledgement of `command_id`, once it comes; `None` when
/// it doesn't come in time. A question the host hasn't taken up within
/// [`PICKUP_WAIT`] is closed, so a late host doesn't carry out a stale
/// one. One it has taken is never closed under it (a sign-in or a spent
/// reset may already have happened): it is waited for as long as the
/// harness may take, and its answer still lands if it comes later.
async fn wait_for_answer(state: &AppState, command_id: Uuid) -> Result<Option<Ack>, AppError> {
    let started = tokio::time::Instant::now();
    let mut deadline = started + PICKUP_WAIT;
    let mut taken = false;
    loop {
        if let Some(ack) = commands::answer_of(&state.pool, command_id).await? {
            return Ok(Some(ack));
        }
        if tokio::time::Instant::now() >= deadline {
            if taken || commands::close_if_not_taken(&state.pool, command_id).await? {
                return commands::answer_of(&state.pool, command_id).await;
            }
            taken = true;
            deadline = started
                + state.config.codex_request_timeout * REQUESTS_PER_QUESTION
                + IN_FLIGHT_MARGIN;
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

fn read_ack(ack: Ack) -> Asked {
    match ack.outcome {
        Some(AckOutcome::Done | AckOutcome::Accepted) => ack
            .result
            .and_then(|result| serde_json::from_value::<AccountAnswer>(result).ok())
            .map_or_else(
                || Asked::Failed("invalid_response".to_owned()),
                Asked::Answered,
            ),
        Some(AckOutcome::Refused) => match ack.reason.as_deref() {
            Some(REFUSED_BUSY) => Asked::Busy,
            Some(REFUSED_UNSUPPORTED) => Asked::Unsupported,
            Some(reason) if is_word(reason) => Asked::Failed(reason.to_owned()),
            _ => Asked::Failed(REFUSED_FAILED.to_owned()),
        },
        _ => Asked::Failed(REFUSED_FAILED.to_owned()),
    }
}

/// A refusal's reason is kept only when it is a short word.
fn is_word(reason: &str) -> bool {
    (1..=32).contains(&reason.len()) && reason.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

// ------------------------------------------------------------ the host

/// Whether `command` is an account question.
pub fn is_account_action(command: &Command) -> bool {
    command.detail.get("action").and_then(Value::as_str) == Some(ACCOUNT_ACTION)
}

/// Carry out an account question with the account's harness.
pub async fn answer(state: &AppState, command: &Command) -> Ack {
    let Some(ask) = command
        .detail
        .get("input")
        .cloned()
        .and_then(|input| serde_json::from_value::<AccountAsk>(input).ok())
    else {
        return refused("invalid", "That account question can't be read.");
    };
    let handle = AccountHandle {
        id: ask.account_id,
        home: PathBuf::from(&ask.home),
        max_concurrent_runs: ask.max_concurrent_runs.max(1),
    };
    let api_key = command
        .detail
        .get("apiKey")
        .and_then(Value::as_str)
        .map(|key| zeroize::Zeroizing::new(key.to_owned()));
    match carry_out(state, &ask.harness, handle, ask.question, api_key).await {
        Ok(answer) => {
            let mut ack = Ack::new(AckOutcome::Done);
            ack.result = serde_json::to_value(answer).ok();
            ack
        }
        Err(failure) => {
            let reason = failure.reason();
            tracing::info!(account_id = %ask.account_id, reason, "an account question failed");
            refused(reason, "The account's harness couldn't do that.")
        }
    }
}

/// Why the host couldn't answer.
enum Failure {
    Harness(HarnessError),
    /// The usage was read, but the server didn't take the report.
    Report,
}

impl From<HarnessError> for Failure {
    fn from(error: HarnessError) -> Self {
        Self::Harness(error)
    }
}

impl Failure {
    fn reason(&self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Harness(error) => match error {
                HarnessError::HostCapacity
                | HarnessError::AccountCapacity
                | HarnessError::ControlCapacity => REFUSED_BUSY,
                HarnessError::Unsupported(_) => REFUSED_UNSUPPORTED,
                HarnessError::ShuttingDown => "shutting_down",
                HarnessError::CircuitOpen => "circuit_open",
                HarnessError::ConfigurationChanged | HarnessError::InvalidConfiguration => {
                    "configuration"
                }
                HarnessError::Transport(_) => "transport",
                HarnessError::MissingResult(_) | HarnessError::InvalidResponse => {
                    "invalid_response"
                }
                HarnessError::UnknownAdapter => "unknown_harness",
            },
        }
    }
}

async fn carry_out(
    state: &AppState,
    harness: &str,
    handle: AccountHandle,
    question: AccountQuestion,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Result<AccountAnswer, Failure> {
    let account_id = handle.id;
    if matches!(question, AccountQuestion::Logout) {
        // Sign-out starts from a fresh session, and ends with none.
        state
            .supervisors
            .shutdown_account(harness, account_id)
            .await;
    }
    if let AccountQuestion::CheckEnvironment { environment } = &question {
        // Only a cloud harness has environments of its own; it checks
        // with its command line, not a control session.
        if harness != crate::provider::CODEX_CLOUD_KIND {
            return Err(HarnessError::Unsupported("environments").into());
        }
        let found = state
            .supervisors
            .codex_cloud()
            .check_environment(&handle, environment)
            .await
            .is_ok();
        return Ok(AccountAnswer::Environment { found });
    }
    let mut session = state.supervisors.acquire_control(harness, handle).await?;
    let answer = match question {
        AccountQuestion::Status => {
            let account = session.read_account().await?;
            let rate_limits = if account.authenticated {
                session.read_rate_limits().await.ok().map(rate_limits)
            } else {
                None
            };
            AccountAnswer::Status {
                authenticated: account.authenticated,
                account_type: account.account_type,
                email: account.email,
                rate_limits,
            }
        }
        AccountQuestion::Login { kind } => AccountAnswer::LoginStarted {
            start: login_started(session.start_login(kind).await?),
        },
        AccountQuestion::ApiKeyLogin { .. } => {
            // The server sealed the key beside the sign-in and handed it
            // over with the command, read once; without it there is
            // nothing to sign in with.
            let key = api_key.ok_or(HarnessError::MissingResult("the API key"))?;
            AccountAnswer::LoginStarted {
                start: login_started(session.login_with_api_key(key.as_str()).await?),
            }
        }
        AccountQuestion::CancelLogin { login_id } => {
            session.cancel_login(&login_id).await?;
            AccountAnswer::Done
        }
        AccountQuestion::Logout => {
            session.logout().await?;
            drop(session);
            state
                .supervisors
                .shutdown_account(harness, account_id)
                .await;
            return Ok(AccountAnswer::Done);
        }
        AccountQuestion::Usage => {
            let report = session.read_usage().await?;
            drop(session);
            // Stored the way any host's windows are, through the protocol.
            agent_workspace_builtin_host::runtime::windows::report(
                &super::transport::client(state),
                account_id,
                &report,
                false,
            )
            .await
            .map_err(|_| Failure::Report)?;
            return Ok(AccountAnswer::Done);
        }
        AccountQuestion::SpendReset {
            credit_id,
            attempt_key,
        } => AccountAnswer::ResetSpent {
            outcome: match session
                .spend_reset_credit(credit_id.as_deref(), &attempt_key)
                .await?
            {
                ResetOutcome::Reset => ResetSpent::Reset,
                ResetOutcome::NothingToReset => ResetSpent::NothingToReset,
                ResetOutcome::NoCredit => ResetSpent::NoCredit,
                ResetOutcome::AlreadyRedeemed => ResetSpent::AlreadyRedeemed,
            },
        },
        AccountQuestion::Models => AccountAnswer::Models {
            models: session.list_models().await?,
        },
        AccountQuestion::CheckEnvironment { .. } => {
            return Err(HarnessError::Unsupported("environments").into());
        }
    };
    Ok(answer)
}

fn login_started(start: LoginStart) -> LoginStarted {
    match start {
        LoginStart::Browser { login_id, auth_url } => LoginStarted::Browser { login_id, auth_url },
        LoginStart::DeviceCode {
            login_id,
            verification_url,
            user_code,
        } => LoginStarted::DeviceCode {
            login_id,
            verification_url,
            user_code,
        },
        LoginStart::ApiKey => LoginStarted::ApiKey,
    }
}

fn rate_limits(summary: RateLimitSummary) -> RateLimits {
    let window = |window: crate::provider::RateLimitWindow| RateLimitWindow {
        used_percent: window.used_percent,
        resets_at: window.resets_at,
        window_duration_minutes: window.window_duration_minutes,
    };
    RateLimits {
        primary: summary.primary.map(window),
        secondary: summary.secondary.map(window),
    }
}

/// What the host's fetch of an account question adds that no row keeps:
/// for an API-key sign-in, the key sealed beside it, taken once.
pub async fn attach_secret(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    body: &Value,
    command: &mut serde_json::Map<String, Value>,
) -> Result<(), AppError> {
    if body.get("action").and_then(Value::as_str) != Some(ACCOUNT_ACTION) {
        return Ok(());
    }
    let Some(ceremony_id) = body
        .get("input")
        .and_then(|input| serde_json::from_value::<AccountAsk>(input.clone()).ok())
        .and_then(|ask| match ask.question {
            AccountQuestion::ApiKeyLogin { ceremony_id } => Some(ceremony_id),
            _ => None,
        })
    else {
        return Ok(());
    };
    // A key handed over in memory was never sealed: the secrets key is
    // needed only to open one that was.
    if !crate::sign_ins::has_api_key(connection, ceremony_id).await? {
        return Ok(());
    }
    let keys = crate::secrets::keyring(&state.config)?;
    if let Some(key) = crate::sign_ins::take_api_key(connection, keys.as_ref(), ceremony_id).await?
    {
        command.insert("apiKey".into(), Value::String(key.as_str().to_owned()));
    }
    Ok(())
}

/// The provider account behind an execution profile, and the adapter that
/// runs it.
#[derive(Clone, Debug, FromRow)]
pub struct ProfileAccount {
    pub account_id: Uuid,
    pub adapter_kind: String,
    pub home_path: String,
    pub max_concurrent_runs: i16,
}

impl ProfileAccount {
    /// A question for the built-in host about this account.
    pub fn ask(&self, question: AccountQuestion) -> AccountAsk {
        AccountAsk {
            account_id: self.account_id,
            harness: self.adapter_kind.clone(),
            home: self.home_path.clone(),
            max_concurrent_runs: u16::try_from(self.max_concurrent_runs).unwrap_or(1),
            question,
        }
    }

    pub(crate) fn handle(&self) -> AccountHandle {
        AccountHandle {
            id: self.account_id,
            home: PathBuf::from(&self.home_path),
            max_concurrent_runs: u16::try_from(self.max_concurrent_runs).unwrap_or(1),
        }
    }

    pub(crate) async fn for_profile(
        pool: &sqlx::PgPool,
        execution_profile_id: Uuid,
    ) -> Result<Self, AppError> {
        Ok(sqlx::query_as::<_, Self>(
            r#"
            SELECT account.id AS account_id, account.adapter_kind, account.home_path,
                   account.max_concurrent_runs
            FROM execution_profiles profile
            JOIN provider_accounts account ON account.id = profile.provider_account_id
            WHERE profile.id = $1
            "#,
        )
        .bind(execution_profile_id)
        .fetch_one(crate::db::scoped(pool))
        .await?)
    }
}

/// How the harness process for an account is doing, when this process
/// carries the built-in host; `None` from anywhere else, since a host
/// elsewhere doesn't report its processes.
pub async fn health(
    state: &AppState,
    harness: &str,
    account_id: Uuid,
) -> Option<crate::harness::report::ProfileHealth> {
    if !host_is_here(state) {
        return None;
    }
    state.supervisors.health(harness, account_id).await
}
