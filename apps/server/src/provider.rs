use std::{path::PathBuf, time::Duration};

use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use tokio::time::sleep;
use uuid::Uuid;

use std::sync::Arc;

use crate::{
    AppState,
    domain::RunState,
    error::AppError,
    git_runtime::worktree_is_dirty,
    hosts::builtin::report::{self, RunReports},
    message_content::{MessageContentError, PlainTextMessageBuffer, ValidatedMessageContent},
    realtime::RealtimeEvent,
};

pub mod catalog;
pub mod ceremonies;
pub(crate) mod registry;

pub use agent_workspace_builtin_host::harness::{
    acp, adapter, claude_cloud, claude_code, codex_app_server, codex_cloud, command_line, demo,
    host, manifest, run_config, secret_fd,
};

pub use adapter::{
    AccountHandle, AccountSummary, HarnessError, HarnessSession, LoginKind, LoginStart,
    ProfileHealth, RateLimitSummary, RateLimitWindow,
};
use adapter::{HarnessAnswer, HarnessEvent, HarnessRequest, ItemKind, RecoveredTurn, RequestKind};
pub use ceremonies::{LoginCeremonyError, LoginCeremonySnapshot, LoginReservation};
pub use codex_app_server::{CODEX_KIND, codex_manifest};
pub use codex_cloud::{CODEX_CLOUD_KIND, codex_cloud_manifest};
pub use registry::SupervisorRegistry;
use run_config::RunConfig;

#[derive(Debug)]
pub struct StartedProviderRun {
    pub provider_session_id: Uuid,
    pub provider_thread_id: String,
}

#[derive(Debug)]
pub(crate) enum ProviderRecovery {
    Completed { message: String },
    Resumed { latest_turn_status: String },
    UnboundTurn,
    MismatchedTurn,
    RejectedMessage,
    Unavailable,
}

/// The provider account behind an execution profile, and the adapter that
/// runs it.
#[derive(Clone, Debug, FromRow)]
pub(crate) struct ProfileAccount {
    pub(crate) account_id: Uuid,
    pub(crate) adapter_kind: String,
    pub(crate) home_path: String,
    pub(crate) max_concurrent_runs: i16,
}

impl ProfileAccount {
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

async fn run_config(state: &AppState, run_id: Uuid) -> Result<RunConfig, AppError> {
    let config = sqlx::query_scalar::<_, Value>("SELECT config FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(crate::db::scoped(&state.pool))
        .await?;
    Ok(RunConfig::from_value(&config))
}

/// Resolves a run's settings for the account it runs on: the launch
/// overrides over the account's defaults over the workspace catalog and the
/// adapter's manifest. Refuses values the harness doesn't offer.
pub(crate) async fn resolve_run_config(
    state: &AppState,
    organization_id: Uuid,
    adapter_kind: &str,
    account_defaults: &Value,
    launch: &RunConfig,
) -> Result<RunConfig, AppError> {
    let manifest = state
        .supervisors
        .manifest(adapter_kind)
        .ok_or(AppError::Conflict(
            "agent_run",
            "this harness is not installed",
        ))?;
    let catalog = catalog::run_catalog(&state.pool, organization_id, &manifest).await?;
    launch
        .resolve(
            &RunConfig::from_value(account_defaults),
            &manifest,
            &catalog,
        )
        .map_err(|error| AppError::Validation(error.0))
}

/// `resolve_run_config` for settings that were valid once: whatever the
/// harness or catalog stopped offering falls back on its own, and the rest
/// (isolation above all) is kept.
pub(crate) async fn resolve_stored_run_config(
    state: &AppState,
    organization_id: Uuid,
    adapter_kind: &str,
    account_defaults: &Value,
    launch: &RunConfig,
) -> Result<RunConfig, AppError> {
    let manifest = state
        .supervisors
        .manifest(adapter_kind)
        .ok_or(AppError::Conflict(
            "agent_run",
            "this harness is not installed",
        ))?;
    let catalog = catalog::run_catalog(&state.pool, organization_id, &manifest).await?;
    launch
        .resolve_keeping_what_still_fits(
            &RunConfig::from_value(account_defaults),
            &manifest,
            &catalog,
        )
        .map_err(|error| AppError::Validation(error.0))
}

/// The run's frozen settings. Runs created without them (by paths that
/// predate H4) get their account's defaults once, here, and keep them.
async fn ensure_run_config(state: &AppState, run_id: Uuid) -> Result<RunConfig, AppError> {
    let config = run_config(state, run_id).await?;
    if config != RunConfig::default() {
        return Ok(config);
    }
    let (organization_id, adapter_kind, run_defaults) = sqlx::query_as::<_, (Uuid, String, Value)>(
        r#"
            SELECT account.organization_id, account.adapter_kind, account.run_defaults
            FROM agent_runs run
            JOIN execution_profiles profile ON profile.id = run.execution_profile_id
            JOIN provider_accounts account ON account.id = profile.provider_account_id
            WHERE run.id = $1
            "#,
    )
    .bind(run_id)
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    // Defaults that stopped matching the catalog fall back one by one.
    let resolved = resolve_stored_run_config(
        state,
        organization_id,
        &adapter_kind,
        &run_defaults,
        &RunConfig::default(),
    )
    .await?;
    let value = serde_json::to_value(&resolved)
        .map_err(|_| AppError::Internal("serialize run settings"))?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query("UPDATE agent_runs SET config = $2 WHERE id = $1 AND config = '{}'::jsonb")
        .bind(run_id)
        .bind(&value)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    run_config(state, run_id).await
}

const SLOT_RELEASE_WAIT: Duration = Duration::from_secs(10);
const SLOT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

#[allow(clippy::too_many_arguments)]
pub async fn start_run(
    state: &AppState,
    reports: Arc<RunReports>,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    execution_profile_id: Uuid,
    worktree: PathBuf,
    prompt: String,
) -> Result<StartedProviderRun, AppError> {
    let account = ProfileAccount::for_profile(&state.pool, execution_profile_id).await?;
    let config = ensure_run_config(state, run_id).await?;
    // The scheduler admits from database counts; a run that just ended may
    // still hold its harness slot until its session winds down, so a full
    // harness is waited on briefly rather than failing the run.
    let mut waited = Duration::ZERO;
    let provider = loop {
        match state
            .supervisors
            .acquire_run(&account.adapter_kind, account.handle())
            .await
        {
            Err(HarnessError::HostCapacity | HarnessError::AccountCapacity)
                if waited < SLOT_RELEASE_WAIT =>
            {
                sleep(SLOT_RETRY_INTERVAL).await;
                waited += SLOT_RETRY_INTERVAL;
            }
            result => break result,
        }
    };
    let mut provider = match provider {
        Ok(provider) => {
            mark_account_healthy(state, account.account_id).await?;
            provider
        }
        Err(error) => {
            mark_account_failure(state, account.account_id, "supervisor_unavailable").await;
            return Err(provider_error(run_id, "acquire harness session", error));
        }
    };

    let existing = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, provider_thread_id
        FROM provider_sessions
        WHERE conversation_id = $1 AND execution_profile_id = $2
        "#,
    )
    .bind(conversation_id)
    .bind(execution_profile_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?;

    let (provider_session_id, provider_thread_id) = if let Some((session_id, thread_id)) = existing
    {
        provider
            .resume_thread(&thread_id, &worktree, &config)
            .await
            .map_err(|error| provider_error(run_id, "resume thread", error))?;
        settle_open_turn(provider.as_mut(), &thread_id)
            .await
            .map_err(|error| match error {
                OpenTurn::StillRunning => AppError::Conflict(
                    "agent_run",
                    "the agent's previous turn on this conversation is still running",
                ),
                OpenTurn::Harness(error) => provider_error(run_id, "settle previous turn", error),
            })?;
        (session_id, thread_id)
    } else {
        let thread_id = provider
            .start_thread(&worktree, &config)
            .await
            .map_err(|error| provider_error(run_id, "start thread", error))?;
        let session_id = Uuid::now_v7();
        sqlx::query(
            r#"
                INSERT INTO provider_sessions (
                    id, conversation_id, execution_profile_id,
                    provider_thread_id, state
                )
                VALUES ($1, $2, $3, $4, 'active')
                "#,
        )
        .bind(session_id)
        .bind(conversation_id)
        .bind(execution_profile_id)
        .bind(&thread_id)
        .execute(crate::db::scoped(&state.pool))
        .await?;
        (session_id, thread_id)
    };

    // Persist the provider Thread ownership before starting a turn. If the
    // process dies around the external turn/start side effect, recovery can
    // distinguish an unbound turn from a durably identified one and must not
    // mistake an older completed turn for this run.
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let bound = sqlx::query(
        r#"
        UPDATE agent_runs
        SET provider_session_id = $2, updated_at = now()
        WHERE id = $1
          AND state = 'starting'
          AND (provider_session_id IS NULL OR provider_session_id = $2)
        "#,
    )
    .bind(run_id)
    .bind(provider_session_id)
    .execute(&mut *transaction)
    .await?;
    if bound.rows_affected() != 1 {
        transaction.rollback().await?;
        idle_unowned_provider_session(state, provider_session_id).await?;
        return Err(AppError::Conflict(
            "agent_run",
            "provider Session binding is no longer available",
        ));
    }
    sqlx::query("UPDATE provider_sessions SET state = 'active', updated_at = now() WHERE id = $1")
        .bind(provider_session_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let turn_id = provider
        .start_turn(&provider_thread_id, &worktree, &prompt, &config)
        .await
        .map_err(|error| provider_error(run_id, "start turn", error))?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let bound = sqlx::query(
        r#"
        UPDATE agent_runs
        SET provider_turn_id = $2, updated_at = now()
        WHERE id = $1
          AND state = 'starting'
          AND provider_session_id = $3
          AND provider_turn_id IS NULL
        "#,
    )
    .bind(run_id)
    .bind(&turn_id)
    .bind(provider_session_id)
    .execute(&mut *transaction)
    .await?;
    if bound.rows_affected() != 1 {
        transaction.rollback().await?;
        let _ = provider.interrupt_turn(&provider_thread_id, &turn_id).await;
        idle_unowned_provider_session(state, provider_session_id).await?;
        return Err(AppError::Conflict(
            "agent_run",
            "provider turn binding is no longer available",
        ));
    }
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Working, None).await?;
    transaction.commit().await?;
    reports.moved("working", None).await?;
    // A turn people can watch elsewhere (a cloud task) is the run's address
    // there, and the thread's agent session (A3).
    if let Some(link) = provider.turn_link(&turn_id)
        && let Err(error) = reports
            .update(
                json!({"externalUrl": link}),
                &format!("link-{}", run_id.simple()),
            )
            .await
    {
        tracing::warn!(error_category = AppError::from(error).log_category(), %run_id, "run address not reported");
    }

    spawn_drive(
        state,
        reports,
        provider,
        project_id,
        run_id,
        conversation_id,
        provider_thread_id.clone(),
        turn_id,
    );

    Ok(StartedProviderRun {
        provider_session_id,
        provider_thread_id,
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_drive(
    state: &AppState,
    reports: Arc<RunReports>,
    provider: Box<dyn HarnessSession>,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    thread_for_task: String,
    turn_id: String,
) {
    let background_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = drive_turn(
            background_state.clone(),
            Arc::clone(&reports),
            provider,
            project_id,
            run_id,
            conversation_id,
            thread_for_task,
            turn_id,
        )
        .await
        {
            tracing::error!(
                error_category = error.log_category(),
                %run_id,
                "agent turn failed"
            );
            fail_run_reliably(
                &background_state,
                &reports,
                run_id,
                "The agent's turn failed. The worktree was preserved.",
            )
            .await;
        }
    });
}

/// After a restart, a turn still running where it runs (a cloud task) is
/// followed again instead of being interrupted. Returns whether it was.
pub(crate) async fn follow_recovered_run(
    state: &AppState,
    fence: crate::recovery::HostFence<'_>,
    run_id: Uuid,
    worktree: PathBuf,
) -> Result<bool, AppError> {
    let Some((project_id, conversation_id, provider_thread_id, execution_profile_id, turn_id)) =
        sqlx::query_as::<_, (Uuid, Uuid, String, Uuid, String)>(
            r#"
            SELECT c.project_id, ar.conversation_id, ps.provider_thread_id,
                   ar.execution_profile_id, ar.provider_turn_id
            -- visibility: the run worker reads its own run's conversation
            FROM agent_runs ar
            JOIN conversations c ON c.id = ar.conversation_id
            JOIN provider_sessions ps ON ps.id = ar.provider_session_id
            WHERE ar.id = $1 AND ar.provider_turn_id IS NOT NULL
            "#,
        )
        .bind(run_id)
        .fetch_optional(crate::db::scoped(&state.pool))
        .await?
    else {
        return Ok(false);
    };
    let account = ProfileAccount::for_profile(&state.pool, execution_profile_id).await?;
    if state
        .supervisors
        .manifest(&account.adapter_kind)
        .is_none_or(|found| found.runs_on != manifest::RunLocation::Cloud)
    {
        return Ok(false);
    }
    let config = run_config(state, run_id).await?;
    let Ok(mut provider) = state
        .supervisors
        .acquire_run(&account.adapter_kind, account.handle())
        .await
    else {
        return Ok(false);
    };
    if provider
        .resume_thread(&provider_thread_id, &worktree, &config)
        .await
        .is_err()
        || provider
            .follow_turn(&provider_thread_id, &turn_id, &worktree, &config)
            .await
            .is_err()
    {
        return Ok(false);
    }
    let reports = Arc::new(report::open(state, run_id).await?);
    // Following took a while: take the run back only while the host is ours.
    let mut transaction = crate::recovery::fenced_tx(&state.pool, fence).await?;
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Working, None).await?;
    transaction.commit().await?;
    reports.moved("working", None).await?;
    spawn_drive(
        state,
        reports,
        provider,
        project_id,
        run_id,
        conversation_id,
        provider_thread_id,
        turn_id,
    );
    Ok(true)
}

#[derive(Debug)]
enum OpenTurn {
    StillRunning,
    Harness(HarnessError),
}

/// Before a new turn on a resumed thread, make sure the thread's last turn
/// is over. A run interrupted by a restart can leave its turn running at the
/// harness (recovery couldn't follow it), and no run here owns it any more:
/// stop it, and refuse to start if it won't stop, rather than run two turns
/// on one thread. Harnesses that can't read a thread's turns (cloud tasks,
/// which run each turn on its own) have nothing to settle.
async fn settle_open_turn(
    provider: &mut dyn HarnessSession,
    thread_id: &str,
) -> Result<(), OpenTurn> {
    let latest = match provider.read_latest_turn(thread_id).await {
        Ok(latest) => latest,
        Err(HarnessError::Unsupported(_)) => return Ok(()),
        Err(error) => return Err(OpenTurn::Harness(error)),
    };
    if latest.status != "inProgress" {
        return Ok(());
    }
    let Some(turn_id) = latest.id else {
        return Err(OpenTurn::StillRunning);
    };
    tracing::warn!("stopping a provider turn no run owns before starting a new one");
    provider
        .interrupt_turn(thread_id, &turn_id)
        .await
        .map_err(OpenTurn::Harness)?;
    match provider.read_latest_turn(thread_id).await {
        Ok(after) if after.status == "inProgress" && after.id.as_deref() == Some(&turn_id) => {
            Err(OpenTurn::StillRunning)
        }
        Ok(_) => Ok(()),
        Err(error) => Err(OpenTurn::Harness(error)),
    }
}

pub(crate) async fn inspect_provider_for_recovery(
    state: &AppState,
    run_id: Uuid,
    worktree: PathBuf,
) -> ProviderRecovery {
    let provider = sqlx::query_as::<_, (String, Uuid, Option<String>)>(
        r#"
        SELECT ps.provider_thread_id, ar.execution_profile_id, ar.provider_turn_id
        FROM agent_runs ar
        JOIN provider_sessions ps ON ps.id = ar.provider_session_id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await;
    let Ok(Some((provider_thread_id, execution_profile_id, provider_turn_id))) = provider else {
        return ProviderRecovery::Unavailable;
    };
    let Some(provider_turn_id) = provider_turn_id else {
        return ProviderRecovery::UnboundTurn;
    };
    let Ok(account) = ProfileAccount::for_profile(&state.pool, execution_profile_id).await else {
        return ProviderRecovery::Unavailable;
    };
    let Ok(config) = run_config(state, run_id).await else {
        return ProviderRecovery::Unavailable;
    };

    // Reading history never takes a run slot.
    let provider = state
        .supervisors
        .acquire_control(&account.adapter_kind, account.handle())
        .await;
    let Ok(mut provider) = provider else {
        return ProviderRecovery::Unavailable;
    };
    if provider
        .resume_thread(&provider_thread_id, &worktree, &config)
        .await
        .is_err()
    {
        return ProviderRecovery::Unavailable;
    }
    let history = provider
        .read_turn(&provider_thread_id, &provider_turn_id)
        .await;
    let Ok(history) = history else {
        return ProviderRecovery::Unavailable;
    };
    if !recovered_turn_has_identity(&history, &provider_turn_id) {
        return ProviderRecovery::MismatchedTurn;
    }
    let latest_turn_status = history.status;
    if latest_turn_status == "completed" {
        let message = history.final_message.unwrap_or_default();
        if validated_worker_message(&message).is_err() {
            tracing::warn!(
                %run_id,
                "rejected invalid provider Message while reconciling completed turn"
            );
            return ProviderRecovery::RejectedMessage;
        }
        return ProviderRecovery::Completed { message };
    }
    ProviderRecovery::Resumed { latest_turn_status }
}

fn recovered_turn_has_identity(history: &RecoveredTurn, expected_turn_id: &str) -> bool {
    history.id.as_deref() == Some(expected_turn_id)
}

async fn mark_account_healthy(state: &AppState, account_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_accounts
        SET last_health_at = now(), last_failure_category = NULL,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(account_id)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    Ok(())
}

async fn idle_unowned_provider_session(
    state: &AppState,
    provider_session_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_sessions session
           SET state = 'idle', updated_at = now()
         WHERE session.id = $1
           AND NOT EXISTS (
               SELECT 1
                 FROM agent_runs run
                WHERE run.provider_session_id = session.id
                  AND run.state IN (
                      'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                  )
           )
        "#,
    )
    .bind(provider_session_id)
    .execute(crate::db::scoped(&state.pool))
    .await?;
    Ok(())
}

async fn mark_account_failure(state: &AppState, account_id: Uuid, category: &'static str) {
    if let Err(error) = sqlx::query(
        r#"
        UPDATE provider_accounts
        SET last_health_at = now(), last_failure_category = $2,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(account_id)
    .bind(category)
    .execute(crate::db::scoped(&state.pool))
    .await
    {
        tracing::error!(
            error_type = std::any::type_name_of_val(&error),
            %account_id,
            "failed to persist redacted provider account health"
        );
    }
}

pub(crate) async fn complete_recovered_run(
    state: &AppState,
    fence: crate::recovery::HostFence<'_>,
    run_id: Uuid,
    message: &str,
    previous_state: &str,
    worktree_state: &str,
    dirty: bool,
) -> Result<(), AppError> {
    let (project_id, conversation_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
            SELECT c.project_id, ar.conversation_id
            -- visibility: the run worker reads its own run's conversation
            FROM agent_runs ar
            JOIN conversations c ON c.id = ar.conversation_id
            WHERE ar.id = $1
            "#,
    )
    .bind(run_id)
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    let reports = report::open(state, run_id).await?;
    let mut transaction = crate::recovery::fenced_tx(&state.pool, fence).await?;
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Working, None).await?;
    transaction.commit().await?;
    if fence.is_some_and(|(_, lease)| lease.lost.is_cancelled()) {
        return Err(AppError::LeaseLost);
    }
    let _ = (project_id, conversation_id);
    complete_run(
        state,
        &reports,
        run_id,
        message,
        Some(RecoveryCompletion {
            previous_state,
            worktree_state,
            dirty,
        }),
    )
    .await
}

struct RecoveryCompletion<'a> {
    previous_state: &'a str,
    worktree_state: &'a str,
    dirty: bool,
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    state: AppState,
    reports: Arc<RunReports>,
    mut provider: Box<dyn HarnessSession>,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    provider_thread_id: String,
    turn_id: String,
) -> Result<(), AppError> {
    // Losing the built-in host's lease stops this turn like a shutdown does:
    // the run stays as it is for the next holder to recover (K5.3).
    let _active = state.supervisors.turn_started();
    let lease_lost = state.supervisors.host_lease_lost();
    let stopping = || state.supervisors.is_shutting_down() || lease_lost.is_cancelled();
    let mut message = PlainTextMessageBuffer::default();
    let message_stream_id = Uuid::now_v7();
    loop {
        tokio::select! {
            event = provider.next_event() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if !stopping() {
                            sleep(Duration::from_millis(100)).await;
                        }
                        if stopping() {
                            provider.shutdown().await;
                            return Ok(());
                        }
                        return Err(provider_error(run_id, "read event", error));
                    }
                };
                match event {
                    HarnessEvent::Request(request) => handle_provider_request(
                        &state,
                        &reports,
                        &mut provider,
                        run_id,
                        request,
                        &lease_lost,
                    ).await?,
                    HarnessEvent::MessageDelta(delta) => {
                        if let Err(error) = message.push(&delta) {
                            provider.shutdown().await;
                            return Err(provider_message_error(run_id, error));
                        }
                        // To the conversation's sockets on every instance (K5.5).
                        crate::realtime::ephemeral::stream_delta(&state, RealtimeEvent {
                            event_id: Uuid::now_v7(),
                            kind: "message.delta",
                            project_id,
                            conversation_id: Some(conversation_id),
                            payload: json!({
                                "message_id": message_stream_id,
                                "agent_run_id": run_id,
                                "delta": delta
                            }),
                        }).await;
                    }
                    HarnessEvent::ItemStarted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        record_step(&reports, &event, "command_started", true).await?;
                    }
                    HarnessEvent::ItemCompleted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        record_step(&reports, &event, "command_completed", true).await?;
                    }
                    HarnessEvent::ItemCompleted(event) if event.kind == ItemKind::FileChange => {
                        record_step(&reports, &event, "file_change", true).await?;
                    }
                    HarnessEvent::ItemCompleted(event)
                        if event.kind == ItemKind::Other && event.detail.tool.is_some() => {
                        record_step(&reports, &event, "tool_used", false).await?;
                    }
                    HarnessEvent::TurnCompleted { completed } => {
                        if !completed {
                            return Err(AppError::Conflict(
                                "provider",
                                "the agent's turn did not complete successfully",
                            ));
                        }
                        complete_run(
                            &state,
                            &reports,
                            run_id,
                            message.as_str(),
                            None,
                        ).await?;
                        return Ok(());
                    }
                    HarnessEvent::Error => {
                        return Err(AppError::Conflict("provider", "the harness reported an error"));
                    }
                    HarnessEvent::UsageUpdated(report) => {
                        crate::usage::record_run_report(&state, provider.account_id(), report).await;
                    }
                    HarnessEvent::TokensUsed(usage) => {
                        crate::run_log::record_tokens(&state, run_id, usage).await;
                    }
                    HarnessEvent::Note(text) => {
                        if !text.trim().is_empty() {
                            let key = format!("note:{}", Uuid::now_v7());
                            reports.step(
                                &key,
                                "agent_note",
                                &crate::run_log::cap(&crate::run_log::redact(text.trim()), 160),
                                crate::run_log::note_payload(&text),
                                false,
                            ).await?;
                        }
                    }
                    HarnessEvent::Plan(items) => {
                        let items = items
                            .iter()
                            .map(|item| json!({
                                "title": crate::run_log::cap(&crate::run_log::redact(&item.title), 500),
                                "status": item.status,
                            }))
                            .collect::<Vec<_>>();
                        let key = format!("plan:{}", Uuid::now_v7());
                        reports.step(&key, "plan", "The plan.", json!({ "items": items }), false).await?;
                    }
                    HarnessEvent::ItemStarted(_)
                    | HarnessEvent::ItemCompleted(_)
                    | HarnessEvent::Ignored => {}
                }
            }
            // Stop arrives as a wake on the control channel from whichever
            // server took it (`hosts::builtin::transition`); the check every
            // second is the safety net for a lost wake or no listener.
            () = stop_wake(&state) => {
                if stopping() {
                    provider.shutdown().await;
                    return Ok(());
                }
                if provider.is_stopping() {
                    provider.shutdown().await;
                    fail_run(
                        &state,
                        &reports,
                        run_id,
                        "The account this run used was signed out. Its worktree was kept.",
                    ).await?;
                    return Ok(());
                }
                let run_state = sqlx::query_scalar::<_, String>(
                    "SELECT state FROM agent_runs WHERE id = $1",
                )
                .bind(run_id)
                .fetch_one(crate::db::scoped(&state.pool))
                .await?;
                if run_state == "interrupted" {
                    let _ = provider.interrupt_turn(&provider_thread_id, &turn_id).await;
                    return Ok(());
                }
            }
            () = state.supervisors.shutdown_requested() => {
                provider.shutdown().await;
                return Ok(());
            }
            () = lease_lost.cancelled() => {
                provider.shutdown().await;
                return Ok(());
            }
        }
    }
}

/// Wait until a stopped run should be looked at: a wake on the control
/// channel, or a second at most.
async fn stop_wake(state: &AppState) {
    tokio::select! {
        () = sleep(Duration::from_secs(1)) => {}
        () = state.supervisors.lease_released() => {}
    }
}

async fn handle_provider_request(
    state: &AppState,
    reports: &RunReports,
    provider: &mut Box<dyn HarnessSession>,
    run_id: Uuid,
    request: HarnessRequest,
    lease_lost: &tokio_util::sync::CancellationToken,
) -> Result<(), AppError> {
    if request.kind == RequestKind::Unsupported {
        provider
            .answer(request, HarnessAnswer::Deny)
            .await
            .map_err(|error| {
                provider_error(run_id, "reject unsupported provider request", error)
            })?;
        return Ok(());
    }
    let approval = request.kind == RequestKind::Approval;
    let options = request
        .options
        .iter()
        .map(|option| (option.id.clone(), option.label.clone()))
        .collect::<Vec<_>>();
    // A1.14, A1.16: the ask is an `interrupt` the run reports, which opens
    // a request; whoever may answer it is told.
    let asked = crate::hosts::builtin::requests::interrupt(
        &request.durable_id,
        if approval {
            crate::hosts::builtin::requests::Ask::Approval
        } else {
            crate::hosts::builtin::requests::Ask::Question
        },
        &request.summary,
        &options,
    );
    // Raised first: a run shown blocked always has what it waits on.
    let request_id = reports.raise(asked).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Blocked, None).await?;
    transaction.commit().await?;

    // The answer comes to the host as an `answer` command; the driver reads
    // it with the run's credential. The wake on the control channel is the
    // prompt; the one-second check is the safety net.
    loop {
        tokio::select! {
            () = sleep(Duration::from_secs(1)) => {}
            () = state.supervisors.lease_released() => {}
            () = lease_lost.cancelled() => {}
        }
        if state.supervisors.is_shutting_down() || lease_lost.is_cancelled() {
            let _ = provider.answer(request, HarnessAnswer::Deny).await;
            provider.shutdown().await;
            return Ok(());
        }
        if provider.is_stopping() {
            let _ = provider.answer(request, HarnessAnswer::Deny).await;
            provider.shutdown().await;
            fail_run(
                state,
                reports,
                run_id,
                "The account this run used was signed out. Its worktree was kept.",
            )
            .await?;
            return Ok(());
        }
        use crate::hosts::builtin::requests::Outcome;
        let stopped = crate::runs::builtin::state_of(crate::db::scoped(&state.pool), run_id)
            .await?
            .is_none_or(|state| state == RunState::Interrupted);
        let outcome = if stopped {
            Outcome::Interrupted
        } else {
            reports.outcome(request_id).await?
        };
        let answer = match outcome {
            Outcome::Waiting => continue,
            Outcome::Interrupted => {
                provider
                    .answer(request, HarnessAnswer::Deny)
                    .await
                    .map_err(|error| {
                        provider_error(run_id, "decline interrupted request", error)
                    })?;
                return Ok(());
            }
            Outcome::Approve => HarnessAnswer::Approve,
            Outcome::Deny => HarnessAnswer::Deny,
            Outcome::Text(text) => HarnessAnswer::Text(text),
        };
        // An approval denied, or a request nobody answered, stops the run.
        if matches!(answer, HarnessAnswer::Deny) {
            crate::hosts::builtin::driver::stop_unanswered(state, reports, run_id, approval)
                .await?;
        } else {
            crate::hosts::builtin::driver::go_on(state, reports, run_id).await?;
            if crate::runs::builtin::state_of(crate::db::scoped(&state.pool), run_id)
                .await?
                .is_none_or(|state| state.is_terminal())
            {
                let _ = provider.answer(request, HarnessAnswer::Deny).await;
                return Ok(());
            }
        }
        provider
            .answer(request, answer)
            .await
            .map_err(|error| provider_error(run_id, "answer provider request", error))?;
        return Ok(());
    }
}

async fn complete_run(
    state: &AppState,
    reports: &RunReports,
    run_id: Uuid,
    message: &str,
    recovery: Option<RecoveryCompletion<'_>>,
) -> Result<(), AppError> {
    let validated_message =
        validated_worker_message(message).map_err(|error| provider_message_error(run_id, error))?;
    let worktree_path = sqlx::query_scalar::<_, String>(
        r#"
        SELECT gw.path
        FROM agent_runs ar
        JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
        JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    let dirty = tokio::task::spawn_blocking(move || {
        worktree_is_dirty(PathBuf::from(worktree_path).as_path())
    })
    .await
    .map_err(|_| AppError::Internal("join worktree inspection task"))??;

    // What the agent said goes to the thread like any host's answer (a
    // `text_message` for the thread, posted by `runs::say`), and the run
    // finishes, all in one report.
    let mut activities = Vec::new();
    if let Some(validated_message) = validated_message {
        let (_, plain_text, _) = validated_message.into_parts();
        activities.push(report::activity(
            "provider:reply",
            "agent_reply",
            "The agent's answer.",
            json!({"text": plain_text, "inThread": true}),
            false,
        ));
    }
    if let Some(recovery) = &recovery {
        activities.push(report::activity(
            "startup-recovery:completed",
            "recovery",
            "Service restarted during a run. Provider history proved that the turn completed, so the durable run was reconciled as completed.",
            json!({
                "previous_state": recovery.previous_state,
                "result": "completed",
                "provider_resumed": true,
                "worktree_preserved": true,
                "worktree_state": recovery.worktree_state,
                "dirty": recovery.dirty
            }),
            true,
        ));
    }
    activities.push(report::activity(
        "provider:run:completed",
        "run_completed",
        "The agent's turn completed.",
        json!({}),
        true,
    ));
    reports.activities(activities).await?;

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query(
        r#"
        UPDATE git_worktrees gw
        SET state = CASE WHEN $2 THEN 'dirty' ELSE 'ready' END,
            dirty = $2,
            last_inspected_at = now(),
            updated_at = now()
        FROM repository_workspaces rw, agent_runs ar
        WHERE ar.id = $1
          AND rw.id = ar.repository_workspace_id
          AND gw.repository_workspace_id = rw.id
        "#,
    )
    .bind(run_id)
    .bind(dirty)
    .execute(&mut *transaction)
    .await?;
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Completed, None).await?;
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
    transaction.commit().await?;
    Ok(())
}

/// Reports one step of a run with its redacted detail, and the tests its
/// output reported as a step of their own.
async fn record_step(
    reports: &RunReports,
    item: &crate::provider::adapter::HarnessItem,
    kind: &str,
    in_thread: bool,
) -> Result<(), AppError> {
    let payload = crate::run_log::item_payload(item);
    let summary = crate::run_log::step_summary(kind, &item.detail);
    let tests = payload.get("tests").cloned();
    let mut steps = vec![report::activity(
        &item.key, kind, &summary, payload, in_thread,
    )];
    if let (Some(tests), "command_completed") = (tests, kind) {
        let passed = tests["passed"].as_u64().unwrap_or(0);
        let failed = tests["failed"].as_u64().unwrap_or(0);
        let summary = if failed == 0 {
            format!("Tests passed: {passed}")
        } else {
            format!("Tests failed: {failed} of {}", passed + failed)
        };
        steps.push(report::activity(
            &format!("{}:tests", item.key),
            "test_result",
            &summary,
            json!({ "tests": tests, "command": item.detail.command.as_deref().map(|command| crate::run_log::cap(&crate::run_log::redact(command), 2_000)) }),
            false,
        ));
    }
    Ok(reports.activities(steps).await?)
}

/// Attempts at failing a run whose turn failed before leaving it to
/// recovery, and the wait before the first retry (doubled each time).
const FAIL_RUN_ATTEMPTS: u32 = 4;
const FAIL_RUN_FIRST_RETRY: Duration = Duration::from_millis(500);

/// A turn that failed must not leave its run "working" because the database
/// was briefly unreachable when it said so. Each failure is logged and
/// retried; if every attempt fails, the run is left to the built-in host's
/// reconciliation (K5.3), which interrupts it when the lease next changes
/// hands. Nothing is dropped silently.
async fn fail_run_reliably(
    state: &AppState,
    reports: &RunReports,
    run_id: Uuid,
    reason: &'static str,
) {
    let mut wait = FAIL_RUN_FIRST_RETRY;
    for attempt in 1..=FAIL_RUN_ATTEMPTS {
        match fail_run(state, reports, run_id, reason).await {
            Ok(()) => return,
            Err(error) => {
                tracing::error!(
                    error_category = error.log_category(),
                    %run_id,
                    attempt,
                    "could not record the failed turn"
                );
                if attempt < FAIL_RUN_ATTEMPTS {
                    sleep(wait).await;
                    wait *= 2;
                }
            }
        }
    }
    tracing::error!(
        %run_id,
        "left the failed turn's run to the built-in host's reconciliation"
    );
}

/// The run failed: its record hears why (shown in its thread), then the
/// host's own rows are put away.
pub(crate) async fn fail_run(
    state: &AppState,
    reports: &RunReports,
    run_id: Uuid,
    reason: &str,
) -> Result<(), AppError> {
    let current = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(crate::db::scoped(&state.pool))
        .await?;
    if matches!(
        current.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        return Ok(());
    }
    reports
        .note("provider:run:failed", "run_failed", reason, true)
        .await?;
    reports.moved("failed", Some(reason)).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    crate::hosts::builtin::move_detail(&mut transaction, run_id, RunState::Failed, Some(reason))
        .await?;
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn idle_provider_session_for_terminal_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_sessions session
           SET state = 'idle', updated_at = now()
          FROM agent_runs run
         WHERE run.id = $1
           AND run.provider_session_id = session.id
           AND run.state IN ('completed', 'failed', 'canceled', 'interrupted')
           AND NOT EXISTS (
               SELECT 1
                 FROM agent_runs other
                WHERE other.provider_session_id = session.id
                  AND other.id <> run.id
                  AND other.state IN (
                      'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                  )
           )
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn provider_error(run_id: Uuid, operation: &'static str, error: impl std::fmt::Debug) -> AppError {
    tracing::error!(
        %run_id,
        operation,
        error_type = std::any::type_name_of_val(&error),
        "harness operation failed"
    );
    AppError::Conflict("provider", "the agent harness operation failed")
}

fn validated_worker_message(
    message: &str,
) -> Result<Option<ValidatedMessageContent>, MessageContentError> {
    let mut buffer = PlainTextMessageBuffer::default();
    buffer.push(message)?;
    if buffer.as_str().trim().is_empty() {
        return Ok(None);
    }
    buffer.into_validated().map(Some)
}

fn provider_message_error(run_id: Uuid, error: MessageContentError) -> AppError {
    tracing::warn!(
        %run_id,
        validation_error = ?error,
        "agent Message output violated the shared Message boundary"
    );
    AppError::Conflict("provider", "the agent's Message output is invalid")
}

#[cfg(test)]
mod tests {
    use super::{
        OpenTurn, recovered_turn_has_identity, settle_open_turn, validated_worker_message,
    };
    use crate::message_content::{MAX_MESSAGE_TEXT_CHARACTERS, MessageContentError};
    use crate::provider::adapter::RecoveredTurn;

    #[test]
    fn worker_messages_share_unicode_and_control_boundaries() {
        let exact = "🙂".repeat(MAX_MESSAGE_TEXT_CHARACTERS);
        let validated = validated_worker_message(&exact)
            .expect("exact scalar-value boundary")
            .expect("non-empty Worker Message");
        assert_eq!(validated.plain_text(), exact);
        assert_eq!(
            validated_worker_message(&format!("{exact}🙂")),
            Err(MessageContentError::TextTooLong)
        );
        assert_eq!(
            validated_worker_message("safe\u{0007}unsafe"),
            Err(MessageContentError::UnsupportedControlCharacter)
        );
        assert!(
            validated_worker_message(" \n\t ")
                .expect("safe whitespace")
                .is_none()
        );
    }

    #[test]
    fn recovery_requires_the_exact_durable_provider_turn_identity() {
        let history = RecoveredTurn {
            id: Some("turn-current".to_owned()),
            status: "completed".to_owned(),
            final_message: Some("proof".to_owned()),
        };
        assert!(recovered_turn_has_identity(&history, "turn-current"));
        assert!(!recovered_turn_has_identity(&history, "turn-older"));
        assert!(!recovered_turn_has_identity(
            &RecoveredTurn {
                id: None,
                status: "missing".to_owned(),
                final_message: None,
            },
            "turn-current"
        ));
    }

    #[tokio::test]
    async fn a_new_turn_waits_for_no_orphaned_turn() {
        use std::{path::PathBuf, sync::Arc};

        use crate::provider::{
            adapter::{AccountHandle, HarnessAdapter},
            demo::{DemoAdapter, DemoHarness},
            registry::ShutdownSignal,
            run_config::RunConfig,
        };

        let worktree = tempfile::tempdir().expect("worktree");
        let demo = Arc::new(DemoAdapter::new(Arc::new(ShutdownSignal::default()), 8));
        let adapter = DemoHarness(Arc::clone(&demo));
        let account = AccountHandle {
            id: uuid::Uuid::now_v7(),
            home: PathBuf::from("/nonexistent/demo-home"),
            max_concurrent_runs: 2,
        };
        let config = RunConfig::default();
        let mut first = adapter
            .acquire_run(account.clone())
            .await
            .expect("first session");
        let thread = first
            .start_thread(worktree.path(), &config)
            .await
            .expect("thread");
        let orphan = first
            .start_turn(&thread, worktree.path(), "before the restart", &config)
            .await
            .expect("turn");
        // The server restarted: the run that owned the turn was interrupted,
        // but the harness still has the turn running.
        let mut retry = adapter.acquire_run(account.clone()).await.expect("retry");
        retry
            .resume_thread(&thread, worktree.path(), &config)
            .await
            .expect("resume");
        settle_open_turn(retry.as_mut(), &thread)
            .await
            .expect("the orphaned turn is stopped");
        let turns = demo.turns(account.id);
        let stopped = turns
            .iter()
            .find(|turn| turn.turn_id == orphan)
            .expect("orphaned turn");
        assert_eq!(stopped.status, "interrupted");
        // With nothing open, settling is a no-op.
        settle_open_turn(retry.as_mut(), &thread)
            .await
            .expect("nothing to settle");
        assert!(!matches!(
            settle_open_turn(retry.as_mut(), &thread).await,
            Err(OpenTurn::StillRunning)
        ));
    }
}
