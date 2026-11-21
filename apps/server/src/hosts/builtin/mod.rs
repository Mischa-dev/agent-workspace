//! The built-in host: the agents this server runs itself (A1 section 3.11).
//! Its detail row is `agent_runs`; it keeps the run's record in `runs`
//! itself (`runs::builtin`). Everything that writes that detail row from outside
//! the provider machinery goes through here:
//!
//! - A1.10, one run creation path: the browser and MCP start and retry, the
//!   sponsored run a person approves, and a task's "work with an agent" all
//!   admit and create their run with [`admit`] and [`create`], under one cap
//!   and one defaults rule ([`resolve_config`]).
//! - [`transition`] moves the detail row along the built-in state machine.
//!   When a run finishes it announces that through `runs::finished`; it no
//!   longer calls the features that care.

use serde_json::Value;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

pub mod accounts;
pub mod assistant;
pub mod capacity;
pub mod driver;
pub mod http;
pub mod install;
pub mod program;
pub mod recovery;
pub mod report;
pub mod requests;
pub mod transport;
pub mod worktrees;

use crate::{
    AppState,
    domain::RunState,
    error::AppError,
    harness::run_config::RunConfig,
    hosts::builtin::http::RunSetup,
    outbox,
    runs::{ACTIVE_RUN_STATES, run_cap},
};

/// What a start path knows about the run it wants.
pub(crate) struct NewRun<'a> {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub setup: &'a RunSetup,
    pub requested_by: Uuid,
    /// The person whose provider account an approved run request uses.
    pub sponsored_by: Option<Uuid>,
    pub external_execution_request_id: Option<Uuid>,
    pub predecessor_run_id: Option<Uuid>,
    pub prompt: &'a str,
    pub prompt_message_id: Option<Uuid>,
    pub authorization: Value,
    pub config: &'a RunConfig,
    /// A person's app or script is in the call (`scheduler::requester_refusal`).
    pub made_through_app: bool,
}

/// The run [`create`] made.
pub(crate) struct Created {
    pub run_id: Uuid,
    /// Its place among every queued built-in run, counting itself.
    pub queue_position: i64,
}

/// Whether a run may start in this thread now, before anything is written:
/// one active built-in run per thread, and, for a call through an app, the
/// per-person cap counted against `cap_holder` (the sponsor of an approved
/// run request, otherwise the requester). Every start path asks this first.
pub(crate) async fn admit(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    cap_holder: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(holder) = cap_holder
        && run_cap(transaction, holder).await?.reached()
    {
        return Err(AppError::Conflict(
            "agent_run",
            "you already have 5 runs queued or working; wait for one to finish",
        ));
    }
    if thread_busy(&mut **transaction, conversation_id).await? {
        return Err(AppError::Conflict(
            "agent_run",
            "Thread already has an active run",
        ));
    }
    Ok(())
}

/// Whether the thread has a built-in run that hasn't finished.
pub(crate) async fn thread_busy(
    executor: impl sqlx::PgExecutor<'_>,
    conversation_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
            WHERE conversation_id = $1 AND state = ANY($2)
        )
        "#,
    )
    .bind(conversation_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_one(executor)
    .await?)
}

/// The settings a new run freezes: what the launch asked for, else the
/// person's and project's run settings, else the provider account's own
/// defaults, each checked against what the harness offers. The sponsored
/// path used to skip the person's and project's settings (P0's hand-over).
pub(crate) async fn resolve_config(
    state: &AppState,
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    setup: &RunSetup,
    launch: &RunConfig,
) -> Result<RunConfig, AppError> {
    let defaults = crate::launch::run_defaults(connection, project_id, actor_id).await?;
    resolve_config_with(state, &defaults, setup, launch).await
}

/// A1.17: tells the server which harnesses this host runs, so it offers
/// them and checks launches against them without asking the host.
pub async fn describe_harnesses(
    state: &AppState,
    client: &agent_workspace_host_protocol::HostClient,
    lease_token: i64,
) -> Result<(), agent_workspace_host_protocol::ClientError> {
    client
        .builtin_harnesses(&agent_workspace_host_protocol::BuiltinHarnesses {
            harnesses: state.supervisors.builtin_manifests(),
            lease_token,
        })
        .await
        .map(|_| ())
}

/// [`resolve_config`] when the caller already read the run settings.
pub(crate) async fn resolve_config_with(
    state: &AppState,
    defaults: &crate::launch::RunDefaults,
    setup: &RunSetup,
    launch: &RunConfig,
) -> Result<RunConfig, AppError> {
    let account_defaults = match state.supervisors.manifest(&setup.adapter_kind) {
        Some(manifest) => crate::launch::merged_defaults(&setup.run_defaults, defaults, &manifest),
        None => setup.run_defaults.clone(),
    };
    crate::model_catalog::resolve_run_config(
        state,
        setup.organization_id,
        &setup.adapter_kind,
        &account_defaults,
        launch,
    )
    .await
}

/// Record a new built-in run and queue it. Call [`admit`] first, in the same
/// transaction.
pub(crate) async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    new: NewRun<'_>,
) -> Result<Created, AppError> {
    let run_id = Uuid::now_v7();
    let settings = serde_json::to_value(new.config)
        .map_err(|_| AppError::Internal("serialize run settings"))?;
    crate::runs::builtin::open(
        transaction,
        &crate::runs::builtin::NewBuiltinRun {
            id: run_id,
            project_id: new.project_id,
            execution_worker_id: new.setup.execution_worker_id,
            prompt: new.prompt,
            from_request: new.external_execution_request_id.is_some(),
            conversation_id: new.conversation_id,
            requested_by: new.requested_by,
            sponsored_by: new.sponsored_by,
            predecessor_run_id: new.predecessor_run_id,
            settings: &settings,
        },
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            sponsoring_actor_id, external_execution_request_id,
            execution_profile_id, repository_id, predecessor_run_id,
            state, prompt, authorization_snapshot, prompt_message_id, config,
            made_through_app
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'created', $10, $11, $12, $13, $14)
        "#,
    )
    .bind(run_id)
    .bind(new.conversation_id)
    .bind(new.setup.execution_worker_id)
    .bind(new.requested_by)
    .bind(new.sponsored_by)
    .bind(new.external_execution_request_id)
    .bind(new.setup.execution_profile_id)
    .bind(new.setup.repository_id)
    .bind(new.predecessor_run_id)
    .bind(new.prompt)
    .bind(&new.authorization)
    .bind(new.prompt_message_id)
    .bind(&settings)
    .bind(new.made_through_app)
    .execute(&mut **transaction)
    .await?;
    transition(transaction, run_id, RunState::Queued, None).await?;
    let queue_position = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM agent_runs queued
        JOIN agent_runs current ON current.id = $1
        WHERE queued.state = 'queued'
          AND (queued.created_at, queued.id) <= (current.created_at, current.id)
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut **transaction)
    .await?;
    outbox::enqueue_agent_run_updated(transaction, new.project_id, new.conversation_id, run_id)
        .await?;
    Ok(Created {
        run_id,
        queue_position,
    })
}

/// Lock a built-in run for a change, returning its state. Take it before
/// anything else about the run: every write to it (a claim, a step, a state)
/// locks this row first and its `runs` record second, so taking them the
/// other way round deadlocks.
///
/// It also shares its thread and space here, as the run's scope trigger does
/// on every state change. Taken later, after a write has read someone's
/// membership (telling them of a request, say), that order is membership
/// then space, the reverse of a task write's (space then membership), and
/// the two deadlock.
pub async fn lock(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<RunState>, AppError> {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT run.state
        FROM agent_runs run
        -- visibility: the run's own thread, locked for its write, never read out
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN projects space ON space.id = conversation.project_id
        WHERE run.id = $1
        FOR UPDATE OF run
        FOR SHARE OF conversation, space
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?
    .map(|state| state.parse::<RunState>())
    .transpose()
}

/// Move a built-in run to `next`, writing its state-change step. Refuses a
/// move the built-in state machine doesn't allow; a move to where it already
/// is changes nothing.
pub async fn transition(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    next: RunState,
    reason: Option<&str>,
) -> Result<RunState, AppError> {
    let current = lock(transaction, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;

    if current == next {
        return Ok(current);
    }
    if !current.can_transition_to(next) {
        return Err(AppError::Conflict("agent_run", "invalid state transition"));
    }

    let terminal = next.is_terminal();
    let (failure_reason, started_at, finished_at) = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        ),
    >(
        r#"
        UPDATE agent_runs
           SET state = $2,
               failure_reason = COALESCE($3, failure_reason),
               started_at = CASE
                   WHEN $2 IN ('starting', 'working') THEN COALESCE(started_at, now())
                   ELSE started_at
               END,
               finished_at = CASE WHEN $4 THEN now() ELSE finished_at END,
               updated_at = now()
         WHERE id = $1
        RETURNING failure_reason, started_at, finished_at
        "#,
    )
    .bind(run_id)
    .bind(next.to_string())
    .bind(reason)
    .bind(terminal)
    .fetch_one(&mut **transaction)
    .await?;
    crate::runs::builtin::moved(
        transaction,
        run_id,
        crate::runs::builtin::Moved {
            state: next,
            failure_reason: failure_reason.as_deref(),
            started_at,
            finished_at,
        },
    )
    .await?;

    crate::runs::steps::record_new(
        transaction,
        run_id,
        "run_state_changed",
        &format!("Run moved from {current} to {next}."),
        serde_json::json!({
            "from": current,
            "to": next,
            "reason": reason,
        }),
    )
    .await?;

    if terminal {
        // Its driver, wherever it runs, hears at once that it should stop.
        sqlx::query("SELECT pg_notify(realtime_channel('control'), 'host:kick')")
            .execute(&mut **transaction)
            .await?;
        // What it still asked for is withdrawn (A1.14).
        crate::requests::withdraw_for_run(transaction, run_id).await?;
        crate::runs::finished::announce(transaction, run_id).await?;
    }

    Ok(next)
}

/// A1.16: move only the built-in host's own detail row along its state
/// machine. The run's record (`runs`) follows from what the host reports
/// through the protocol (`report::RunReports`), never from here.
pub async fn move_detail(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    next: RunState,
    reason: Option<&str>,
) -> Result<RunState, AppError> {
    let current = lock(transaction, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    if current == next {
        return Ok(current);
    }
    if !current.can_transition_to(next) {
        return Err(AppError::Conflict("agent_run", "invalid state transition"));
    }
    sqlx::query(
        r#"
        UPDATE agent_runs
           SET state = $2,
               failure_reason = COALESCE($3, failure_reason),
               started_at = CASE
                   WHEN $2 IN ('starting', 'working') THEN COALESCE(started_at, now())
                   ELSE started_at
               END,
               finished_at = CASE WHEN $4 THEN now() ELSE finished_at END,
               updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(next.to_string())
    .bind(reason)
    .bind(next.is_terminal())
    .execute(&mut **transaction)
    .await?;
    if next.is_terminal() {
        // Its driver, wherever it runs, hears at once that it should stop.
        sqlx::query("SELECT pg_notify(realtime_channel('control'), 'host:kick')")
            .execute(&mut **transaction)
            .await?;
    }
    // Open screens showing the run's card in its thread refresh it.
    let place = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT conversation.project_id, conversation.id
        -- visibility: the run's own thread, to tell its open screens
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some((space_id, conversation_id)) = place {
        crate::outbox::enqueue_agent_run_updated(transaction, space_id, conversation_id, run_id)
            .await?;
    }
    Ok(next)
}
