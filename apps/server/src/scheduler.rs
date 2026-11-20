use std::time::Duration;

use sqlx::{FromRow, Postgres, Transaction};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    AppState,
    cluster::{Cluster, Lease, LeaseName},
    domain::RunState,
    error::AppError,
    outbox,
    policy::{Action, RoleGrant, within_agent_limits},
};

const SCHEDULER_LEASE: Duration = Duration::from_secs(30);
/// How long a claim waits for a run's thread before leaving the run for the
/// next pass.
const THREAD_LOCK_TIMEOUT: &str = "1s";
const REQUESTER_REFUSED_REASON: &str =
    "Cancelled before it started: whoever started it can no longer start runs in this project.";
const SPONSORED_REQUESTER_REFUSED_REASON: &str = "Cancelled before it started: the agent that asked for it can no longer start runs in this project.";
const SPONSOR_REFUSED_REASON: &str = "Cancelled before it started: the person who approved it can no longer start runs in this project.";
const REQUESTER_APP_REFUSED_REASON: &str = "Cancelled before it started: whoever started it, or the app it was started from, can no longer start runs in this project.";
/// Run states that hold a harness turn (and count against limits).
const ACTIVE_TURN_STATES: &str = "('starting', 'working', 'blocked', 'recovering')";

/// Something may be ready to start: the built-in host's holder looks now
/// (K5.3). Here when this instance holds it; otherwise a wake on the control
/// channel reaches the holder, whose one-second tick is the safety net. A
/// state without a cluster (a test's) drains here, as before.
pub fn kick(state: AppState) {
    tokio::spawn(async move {
        let result = match state.supervisors.cluster() {
            None => drain(&state, None).await,
            Some(cluster) => match cluster.held_lease(&LeaseName::BuiltinHost) {
                Some(lease) => drain(&state, Some((cluster, &lease))).await,
                None => crate::db::scoped_one(&state.pool, async |connection| {
                    sqlx::query("SELECT pg_notify(realtime_channel('control'), 'host:kick')")
                        .execute(&mut *connection)
                        .await
                })
                .await
                .map(|_| ())
                .map_err(AppError::from),
            },
        };
        if let Err(error) = result {
            tracing::error!(
                error_category = error.log_category(),
                "Agent Run scheduler kick failed"
            );
        }
    });
}

/// One pass now, for tests and tools that drive the queue themselves.
pub async fn drain_now(state: &AppState) -> Result<(), AppError> {
    drain(state, None).await
}

/// The built-in host (K5.3): only the holder of the `builtin-host` lease
/// recovers runs, admits them and runs host jobs. Every instance stands by
/// and takes over when the holder's lease runs out or is released.
pub async fn run_host(
    state: AppState,
    cluster: Cluster,
    shutdown: watch::Receiver<bool>,
) -> Result<(), AppError> {
    state.supervisors.set_cluster(cluster.clone());
    crate::cluster::while_held(
        cluster.clone(),
        LeaseName::BuiltinHost,
        shutdown,
        move |lease| host(state.clone(), cluster.clone(), lease),
    )
    .await;
    Ok(())
}

async fn host(state: AppState, cluster: Cluster, lease: Lease) -> Result<(), AppError> {
    let result = async {
        crate::recovery::reconcile_owned(&state, &cluster, &lease).await?;
        let jobs = crate::jobs::default_registry();
        let builtin = crate::hosts::builtin::transport::client(&state);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                // A kick from another instance, or a lease let go.
                () = cluster.released() => {}
                () = lease.lost.cancelled() => return Ok(()),
            }
            if let Err(error) = drain_with(&state, Some((&cluster, &lease)), &builtin).await {
                tracing::error!(
                    error_category = error.log_category(),
                    "Agent Run scheduler pass failed"
                );
            }
            if let Err(error) = crate::jobs::run_due_host(&state, &jobs, &lease).await {
                tracing::warn!(error_category = error.log_category(), "host jobs failed");
            }
        }
    }
    .await;
    // Losing the lease stops every local turn before anyone else may
    // recover them.
    state.supervisors.stop_local_turns().await;
    result
}

/// Drain every second without a lease: for tests of the queue alone. A
/// server runs [`run_host`].
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            // A database blip must not stop the server: log and try again
            // on the next tick, as the log's consumers do.
            _ = interval.tick() => {
                if let Err(error) = drain(&state, None).await {
                    tracing::error!(
                        error_category = error.log_category(),
                        "Agent Run scheduler pass failed"
                    );
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

/// Admit what can start. With a lease, every claim is fenced by it, and a
/// lost lease admits nothing more.
async fn drain(state: &AppState, fence: Option<(&Cluster, &Lease)>) -> Result<(), AppError> {
    drain_with(
        state,
        fence,
        &crate::hosts::builtin::transport::client(state),
    )
    .await
}

/// [`drain`] with the built-in host's client already made.
async fn drain_with(
    state: &AppState,
    fence: Option<(&Cluster, &Lease)>,
    builtin: &agent_workspace_host_protocol::HostClient,
) -> Result<(), AppError> {
    // Claims runs of every workspace, each checked against its run's space.
    crate::tenancy::system(
        "runs.host",
        Box::pin(drain_with_here(state, fence, builtin)),
    )
    .await
}

async fn drain_with_here(
    state: &AppState,
    fence: Option<(&Cluster, &Lease)>,
    builtin: &agent_workspace_host_protocol::HostClient,
) -> Result<(), AppError> {
    while claim_next(state, fence).await? {
        if fence.is_some_and(|(_, lease)| lease.lost.is_cancelled()) {
            return Err(AppError::LeaseLost);
        }
    }
    // A1.16: the built-in host carries out the starts it was just sent
    // (and anything else waiting for it), through the host protocol.
    crate::hosts::builtin::driver::pump_with(state, builtin).await?;
    Ok(())
}

/// Why a queued run may no longer start, if it may not, checked when a slot
/// frees up (principle 9):
///
/// - its requester's role in the project must still let them start runs
///   (custom roles included), and for a run their app or script started,
///   within that app's agent limit as it is now;
/// - for a sponsored run (an external agent's request a person approved),
///   the requester's role and the sponsor's role must both still let them
///   start runs. Sponsorship stands in for an app limit the requester
///   doesn't have, not for either party's role.
///
/// Whether memberships, actors and workspace memberships are still active
/// stays with the admission trigger (validate_agent_run_scope_and_admission).
///
/// Read on the claim's own connection: the claim holds the run's row, so it
/// waits on nothing else while it decides.
async fn admission_refusal(
    transaction: &mut Transaction<'_, Postgres>,
    requester: Uuid,
    requester_kind: &str,
    sponsor: Option<Uuid>,
    project_id: Uuid,
    through_app: bool,
) -> Result<Option<&'static str>, AppError> {
    if !may_start_runs(
        transaction,
        requester,
        requester_kind,
        project_id,
        through_app,
    )
    .await?
    {
        return Ok(Some(match (sponsor.is_some(), through_app) {
            (true, _) => SPONSORED_REQUESTER_REFUSED_REASON,
            (false, true) => REQUESTER_APP_REFUSED_REASON,
            (false, false) => REQUESTER_REFUSED_REASON,
        }));
    }
    if let Some(sponsor) = sponsor
        && !may_start_runs(transaction, sponsor, "human", project_id, false).await?
    {
        return Ok(Some(SPONSOR_REFUSED_REASON));
    }
    Ok(None)
}

/// Whether `actor_id`'s role in the project lets them start runs now, and
/// for a run their app started (`through_app`), within the app's limit.
/// System and worker actors hold no role and aren't checked here.
async fn may_start_runs(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    kind: &str,
    project_id: Uuid,
    through_app: bool,
) -> Result<bool, AppError> {
    let account = matches!(kind, "human" | "external_agent"); // kind-check: mechanical (system and worker actors hold no role)
    if !account {
        return Ok(true);
    }
    let person = kind == "human"; // kind-check: mechanical (an app acts as the person who connected it)
    let limits = if through_app && person {
        Some(
            crate::roles::app_access(transaction, actor_id)
                .await?
                .limits,
        )
    } else {
        None
    };
    let membership = sqlx::query_as::<_, (String, Option<Vec<String>>, bool)>(
        r#"
        SELECT role, workspace_role_permissions(role_id), enabled
        FROM project_memberships
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((role, permissions, enabled)) = membership else {
        return Ok(false);
    };
    let grant = RoleGrant::stored(&role, permissions.as_deref())?;
    Ok(within_agent_limits(limits, async {
        crate::access::membership_allows(
            actor_id,
            project_id,
            grant,
            enabled,
            Action::WorkerInvoke.permission(),
        )
    })
    .await)
}

/// What one look at the queue found.
enum Claim {
    /// A run was claimed and its start command written.
    Started,
    /// The oldest eligible run was cancelled (`admission_refusal`): look again.
    Cancelled,
    Nothing,
}

/// Claim the next run that can start; whether one was.
async fn claim_next(state: &AppState, fence: Option<(&Cluster, &Lease)>) -> Result<bool, AppError> {
    loop {
        match claim_one(state, fence).await? {
            Claim::Started => return Ok(true),
            Claim::Cancelled => {}
            Claim::Nothing => return Ok(false),
        }
    }
}

/// Lock order, against removing a message (which locks its thread and then
/// redacts the prompt on the run it started): a claim never holds a run
/// while it waits on a thread. Lapsed claims are cleared in a statement of
/// their own, the candidate is chosen without row locks (the advisory lock
/// keeps claims one at a time), then the thread is taken, waiting briefly
/// (`THREAD_LOCK_TIMEOUT`), and only then the run.
async fn claim_one(state: &AppState, fence: Option<(&Cluster, &Lease)>) -> Result<Claim, AppError> {
    sqlx::query(
        r#"
        UPDATE agent_runs
        SET scheduler_claim_id = NULL,
            scheduler_claimed_at = NULL,
            scheduler_lease_expires_at = NULL,
            updated_at = now()
        WHERE state = 'queued'
          AND scheduler_lease_expires_at <= now()
        "#,
    )
    .execute(crate::db::scoped(&state.pool))
    .await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('agent_workspace_scheduler', 0))")
        .execute(&mut *transaction)
        .await?;
    // Only the built-in host's holder claims (K5.3).
    if let Some((cluster, lease)) = fence {
        cluster.fence(&mut transaction, lease).await?;
    }
    // The host-wide cap only counts runs on this server; cloud harnesses
    // never wait on it (H8).
    let cloud_kinds = state.supervisors.cloud_kinds();
    let local_active = sqlx::query_scalar::<_, i64>(&format!(
        r#"
        SELECT count(*) FROM agent_runs run
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.state IN {ACTIVE_TURN_STATES}
          AND account.adapter_kind <> ALL($1)
        "#
    ))
    .bind(&cloud_kinds)
    .fetch_one(&mut *transaction)
    .await?;
    let host_full = local_active
        >= i64::try_from(state.config.codex_host_concurrency_limit).unwrap_or(i64::MAX);

    #[derive(FromRow)]
    struct Candidate {
        run_id: Uuid,
        project_id: Uuid,
        conversation_id: Uuid,
        prompt: String,
        requesting_actor_id: Uuid,
        requester_kind: String,
        sponsoring_actor_id: Option<Uuid>,
        made_through_app: bool,
    }
    // Oldest first among runs whose account has a free slot (up to its
    // `max_concurrent_runs`), whose thread has no turn running, and, for
    // local harnesses, while the host has room.
    let candidate = sqlx::query_as::<_, Candidate>(&format!(
        r#"
        SELECT run.id AS run_id, run.project_id, run.conversation_id, run.prompt,
               run.requesting_actor_id,
               requester.kind AS requester_kind,
               run.sponsoring_actor_id, run.made_through_app
        FROM agent_runs run
        JOIN actors requester ON requester.id = run.requesting_actor_id
        JOIN repositories repository
          ON repository.id = run.repository_id
         AND repository.project_id = run.project_id
        JOIN projects project ON project.id = run.project_id
        -- H1.24: a suspended or deleting workspace's runs wait.
        JOIN organizations workspace
          ON workspace.id = project.organization_id
         AND workspace.lifecycle NOT IN ('suspended', 'deleting')
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.state = 'queued'
          AND run.scheduler_claim_id IS NULL
          AND (account.adapter_kind = ANY($2) OR NOT $1)
          AND (
              SELECT count(*) FROM agent_runs active
              JOIN execution_profiles active_profile
                ON active_profile.id = active.execution_profile_id
              WHERE active_profile.provider_account_id = account.id
                AND active.state IN {ACTIVE_TURN_STATES}
          ) < account.max_concurrent_runs
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs active
              WHERE active.conversation_id = run.conversation_id
                AND active.state IN {ACTIVE_TURN_STATES}
          )
        ORDER BY run.created_at, run.id
        LIMIT 1
        "#
    ))
    .bind(host_full)
    .bind(&cloud_kinds)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(candidate) = candidate else {
        transaction.commit().await?;
        return Ok(Claim::Nothing);
    };
    // The thread first (starting the run, the admission trigger takes it
    // too), waiting briefly for whoever holds it, such as a start request
    // or a message removal; held longer, the run waits for the next pass.
    sqlx::query(&format!("SET LOCAL lock_timeout = '{THREAD_LOCK_TIMEOUT}'"))
        .execute(&mut *transaction)
        .await?;
    match sqlx::query("SELECT id FROM conversations WHERE id = $1 FOR SHARE")
        .bind(candidate.conversation_id)
        .execute(&mut *transaction)
        .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
            transaction.rollback().await?;
            return Ok(Claim::Nothing);
        }
        Err(error) => return Err(error.into()),
    }
    sqlx::query("SET LOCAL lock_timeout = 0")
        .execute(&mut *transaction)
        .await?;
    // Then the run, if it's still waiting and nobody else holds it.
    let still_queued = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM agent_runs
        WHERE id = $1 AND state = 'queued' AND scheduler_claim_id IS NULL
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(candidate.run_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if still_queued.is_none() {
        transaction.commit().await?;
        return Ok(Claim::Nothing);
    }
    // Whoever started it (and, for a sponsored run, whoever approved it)
    // must still be able to start runs here now (`admission_refusal`).
    // Otherwise it's cancelled, saying why, and the next one is looked at.
    if let Some(reason) = admission_refusal(
        &mut transaction,
        candidate.requesting_actor_id,
        &candidate.requester_kind,
        candidate.sponsoring_actor_id,
        candidate.project_id,
        candidate.made_through_app,
    )
    .await?
    {
        crate::hosts::builtin::transition(
            &mut transaction,
            candidate.run_id,
            RunState::Canceled,
            Some(reason),
        )
        .await?;
        outbox::enqueue_agent_run_updated(
            &mut transaction,
            candidate.project_id,
            candidate.conversation_id,
            candidate.run_id,
        )
        .await?;
        transaction.commit().await?;
        return Ok(Claim::Cancelled);
    }
    let claim_id = Uuid::now_v7();
    let lease_seconds = i64::try_from(SCHEDULER_LEASE.as_secs()).unwrap_or(30);
    sqlx::query(
        r#"
        UPDATE agent_runs
        SET scheduler_claim_id = $2,
            scheduler_claimed_at = now(),
            scheduler_lease_expires_at = now() + make_interval(secs => $3),
            updated_at = now()
        WHERE id = $1 AND state = 'queued'
        "#,
    )
    .bind(candidate.run_id)
    .bind(claim_id)
    .bind(lease_seconds as f64)
    .execute(&mut *transaction)
    .await?;
    crate::hosts::builtin::transition(&mut transaction, candidate.run_id, RunState::Starting, None)
        .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        candidate.project_id,
        candidate.conversation_id,
        candidate.run_id,
    )
    .await?;
    // A1.16: the built-in host is told to start it, as any host is.
    issue_start(&mut transaction, candidate.run_id, &candidate.prompt).await?;
    transaction.commit().await?;
    Ok(Claim::Started)
}

/// The built-in host's `start` command for a claimed run: its input and
/// cause, as a host elsewhere would get them (`hosts::dispatch`).
pub async fn issue_start(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    prompt: &str,
) -> Result<(), AppError> {
    use crate::hosts::dispatch::{Host, RemoteHost, StartInput};
    let run = crate::runs::ops::locked(transaction, run_id)
        .await
        .map_err(|_| AppError::Internal("load the claimed run"))?
        .ok_or(AppError::NotFound("agent_run"))?;
    let host_id = run
        .host_id
        .ok_or(AppError::Internal("a built-in run without its host"))?;
    RemoteHost { id: host_id }
        .start(
            transaction,
            &run,
            "builtin",
            StartInput {
                prompt: Some(prompt.to_owned()),
                settings: None,
            },
        )
        .await?;
    Ok(())
}
