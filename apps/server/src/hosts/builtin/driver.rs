//! A1.16: the built-in host, driven by its commands. The scheduler admits a
//! queued built-in run and writes it a `start` command, as any host gets
//! one; the host reads its commands through the public host protocol (in
//! process, `transport`) and carries them out here: a start prepares the
//! run's worktree and starts its harness turn.

use std::path::Path;

use agent_workspace_host_protocol::{Ack, AckOutcome, Command, CommandKind, HostClient};
use serde_json::json;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    domain::RunState,
    error::AppError,
    git_runtime::{
        self, RegisteredRepository, expected_repository_mirror_path, managed_worktree_path,
    },
    outbox, provider,
};

const WORKTREE_START_FAILURE_REASON: &str =
    "Worktree creation failed; inspect the retained repository workspace.";
const PROVIDER_START_FAILURE_REASON: &str =
    "The agent harness could not start. The worktree was preserved.";
const UNKNOWN_HARNESS_REASON: &str =
    "This run's harness is not installed on the server. Nothing was started.";
const CLEANED_RETRY_REASON: &str = "The previous worktree was removed by retention cleanup before the retry could use it. Start a new Thread to continue.";
const UNSAFE_RETRY_REASON: &str = "The previous worktree is dirty, missing, or no longer has its owned branch. It was preserved; start a new Thread or inspect it before retrying.";

enum RetryWorkspace {
    FreshAllocation,
    Reuse(Uuid, Uuid, std::path::PathBuf),
    Unsafe,
}

#[derive(Debug, FromRow)]
pub(crate) struct ClaimedRun {
    run_id: Uuid,
    project_id: Uuid,
    conversation_id: Uuid,
    repository_id: Uuid,
    mirror_path: String,
    clone_url: String,
    default_branch: String,
    project_key: String,
    execution_profile_id: Uuid,
    adapter_kind: String,
    prompt: String,
    predecessor_run_id: Option<Uuid>,
    claim_id: Uuid,
}

/// Carry out every command the built-in hosts have waiting. The lease holder
/// calls it on each tick; tests call it through `scheduler::drain_now`.
pub async fn pump(state: &AppState) -> Result<usize, AppError> {
    pump_with(state, &super::transport::client(state)).await
}

/// [`pump`] with a client already made (the lease holder keeps one).
pub async fn pump_with(state: &AppState, client: &HostClient) -> Result<usize, AppError> {
    let mut handled = 0;
    loop {
        let listed = client
            .commands(0, 0)
            .await
            .map_err(|_| AppError::Internal("read the built-in host's commands"))?;
        if listed.commands.is_empty() {
            return Ok(handled);
        }
        for summary in listed.commands {
            let Ok(command) = client.command(summary.command_id).await else {
                // Closed since it was listed: nothing to do.
                continue;
            };
            if let Some(ack) = carry_out(state, client, &command).await {
                let _ = client.ack(command.command_id, &ack).await;
            }
            handled += 1;
        }
    }
}

async fn carry_out(state: &AppState, _client: &HostClient, command: &Command) -> Option<Ack> {
    match command.kind {
        CommandKind::Start => Some(start(state, command).await),
        CommandKind::Inspect => Some(inspect(state, command).await),
        CommandKind::Cancel | CommandKind::Answer => Some(Ack::new(AckOutcome::Done)),
        CommandKind::Steer => Some(refused(
            "unsupported",
            "The built-in host takes a new message as a new run.",
        )),
        CommandKind::Action => Some(refused(
            "unknown_action",
            "The built-in host has no actions.",
        )),
    }
}

fn refused(reason: &str, message: &str) -> Ack {
    let mut ack = Ack::new(AckOutcome::Refused);
    ack.reason = Some(reason.to_owned());
    ack.message = Some(message.to_owned());
    ack
}

/// The built-in host's own state for a run, from its detail row.
async fn detail_state(state: &AppState, run_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(crate::db::scoped(&state.pool))
        .await
        .ok()
        .flatten()
}

/// Whether the built-in host knows the run.
async fn inspect(state: &AppState, command: &Command) -> Ack {
    let known = match command.run_id {
        Some(run_id) => detail_state(state, run_id).await,
        None => None,
    };
    let mut ack = Ack::new(AckOutcome::Done);
    ack.result = Some(match known {
        Some(state) => json!({"known": true, "state": state}),
        None => json!({"known": false}),
    });
    ack
}

/// A start: the run the scheduler claimed for it gets its worktree and its
/// harness turn. A start offered again for a run already going is
/// accepted again; one for a run no longer waiting to start is refused.
async fn start(state: &AppState, command: &Command) -> Ack {
    let Some(run_id) = command.run_id else {
        return refused("invalid", "A start needs its run.");
    };
    let Ok(claimed) = claimed(state, run_id).await else {
        return Ack::new(AckOutcome::Uncertain);
    };
    let Some(run) = claimed else {
        let going = detail_state(state, run_id)
            .await
            .is_some_and(|state| matches!(state.as_str(), "working" | "blocked" | "recovering"));
        return if going {
            Ack::new(AckOutcome::Accepted)
        } else {
            refused("not_waiting", "That run isn't waiting to start.")
        };
    };
    match admit(state, run).await {
        Ok(()) => Ack::new(AckOutcome::Accepted),
        Err(error) => {
            tracing::error!(
                %run_id,
                error_category = error.log_category(),
                "Agent Run admission failed"
            );
            refused("start_failed", "The run couldn't start; its log says why.")
        }
    }
}

/// The run a start is for, as the scheduler claimed it.
async fn claimed(state: &AppState, run_id: Uuid) -> Result<Option<ClaimedRun>, AppError> {
    Ok(sqlx::query_as::<_, ClaimedRun>(
        r#"
        SELECT run.id AS run_id, run.project_id, run.conversation_id,
               run.repository_id, repository.mirror_path, repository.clone_url,
               repository.default_branch, project.project_key,
               run.execution_profile_id, account.adapter_kind, run.prompt,
               run.predecessor_run_id, run.scheduler_claim_id AS claim_id
        FROM agent_runs run
        JOIN repositories repository ON repository.id = run.repository_id
        JOIN projects project ON project.id = run.project_id
        JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE run.id = $1 AND run.state = 'starting'
          AND run.scheduler_claim_id IS NOT NULL
          AND run.repository_workspace_id IS NULL
        "#,
    )
    .bind(run_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?)
}

async fn admit(state: &AppState, run: ClaimedRun) -> Result<(), AppError> {
    let admission_root = state.config.data_root.clone();
    let admission_quota = state.config.data_quota_bytes;
    let admission_free = state.config.minimum_free_bytes;
    if let Err(error) = tokio::task::spawn_blocking(move || {
        crate::storage_admission::require_growth(&admission_root, admission_quota, admission_free)
    })
    .await
    .map_err(|_| AppError::Internal("join storage admission"))?
    {
        fail_admission(
            state,
            &run,
            "Run admission stopped because the host storage watermark blocks new work.",
            None,
        )
        .await?;
        return Err(error);
    }
    let mirror = expected_repository_mirror_path(&state.config.data_root, run.repository_id)?;
    if Path::new(&run.mirror_path) != mirror {
        fail_admission(state, &run, PROVIDER_START_FAILURE_REASON, None).await?;
        return Err(AppError::Conflict(
            "repository",
            "stored mirror path does not match the runtime root",
        ));
    }
    let repository = RegisteredRepository {
        id: run.repository_id,
        clone_url: run.clone_url.clone(),
        default_branch: run.default_branch.clone(),
        mirror_path: mirror.clone(),
    };
    let data_root = state.config.data_root.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        git_runtime::inspect_prepared_repository(&data_root, &repository)
    })
    .await
    .map_err(|_| AppError::Internal("join repository preparation"))?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            fail_admission(state, &run, PROVIDER_START_FAILURE_REASON, None).await?;
            return Err(error.into());
        }
    };

    if let Some(predecessor_run_id) = run.predecessor_run_id {
        match reusable_retry_workspace(state, &run, predecessor_run_id, &mirror).await? {
            RetryWorkspace::Reuse(workspace_id, worktree_id, worktree) => {
                let mut transaction = crate::db::scoped_tx(&state.pool).await?;
                let owned = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS (
                        SELECT 1 FROM agent_runs
                         WHERE id = $1 AND state = 'starting'
                           AND scheduler_claim_id = $2
                           AND repository_workspace_id IS NULL
                    )
                    "#,
                )
                .bind(run.run_id)
                .bind(run.claim_id)
                .fetch_one(&mut *transaction)
                .await?;
                if !owned {
                    return Err(AppError::Conflict("agent_run", "scheduler claim was lost"));
                }
                // Retention cleanup removes retained worktrees under this
                // lock. Take it, then claim only if the workspace is still
                // retained: one cleaned up since the inspection is gone.
                crate::retention::lock_repository_retention(&mut transaction, run.repository_id)
                    .await?;
                // The retry publishes its own commit: the predecessor's
                // publication must not stand in for it, or a pull request
                // could open from a commit this run never made.
                let claimed = sqlx::query(
                    "UPDATE repository_workspaces SET state = 'active', published_commit = NULL, published_at = NULL, published_by_actor_id = NULL, publish_failure_category = NULL, publish_version = publish_version + 1, updated_at = now() WHERE id = $1 AND state = 'retained'",
                )
                .bind(workspace_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected()
                    == 1
                    && sqlx::query(
                        "UPDATE git_worktrees SET state = 'ready', dirty = false, last_inspected_at = now(), updated_at = now() WHERE id = $1 AND state IN ('ready', 'dirty', 'missing', 'failed')",
                    )
                    .bind(worktree_id)
                    .execute(&mut *transaction)
                    .await?
                    .rows_affected()
                        == 1;
                if !claimed {
                    transaction.rollback().await?;
                    fail_admission(state, &run, CLEANED_RETRY_REASON, None).await?;
                    return Err(AppError::Conflict(
                        "git_worktree",
                        "previous worktree was cleaned up before the retry claimed it",
                    ));
                }
                sqlx::query(
                    "UPDATE agent_runs SET repository_workspace_id = $2, updated_at = now() WHERE id = $1",
                )
                .bind(run.run_id)
                .bind(workspace_id)
                .execute(&mut *transaction)
                .await?;
                transaction.commit().await?;

                start_admitted_run(state, &run, worktree).await?;
                clear_claim(state, run.run_id, run.claim_id).await?;
                return Ok(());
            }
            RetryWorkspace::FreshAllocation => {}
            RetryWorkspace::Unsafe => {
                fail_admission(state, &run, UNSAFE_RETRY_REASON, None).await?;
                return Err(AppError::Conflict(
                    "git_worktree",
                    "previous worktree is not safe to reuse",
                ));
            }
        }
    }

    let workspace_id = Uuid::now_v7();
    let worktree_id = Uuid::now_v7();
    let Some(branch_prefix) = state
        .supervisors
        .manifest(&run.adapter_kind)
        .map(|manifest| manifest.branch_prefix.clone())
    else {
        fail_admission(state, &run, UNKNOWN_HARNESS_REASON, None).await?;
        return Err(AppError::Conflict("agent_run", "harness is not installed"));
    };
    let branch = git_runtime::unique_run_branch(&branch_prefix, &run.project_key)?;
    let worktree = managed_worktree_path(&state.config.data_root, run.repository_id, workspace_id)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let owned = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
            WHERE id = $1 AND state = 'starting'
              AND scheduler_claim_id = $2
              AND repository_workspace_id IS NULL
        )
        "#,
    )
    .bind(run.run_id)
    .bind(run.claim_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !owned {
        return Err(AppError::Conflict("agent_run", "scheduler claim was lost"));
    }
    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (
            id, repository_id, conversation_id, branch_name, base_commit
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(workspace_id)
    .bind(run.repository_id)
    .bind(run.conversation_id)
    .bind(&branch)
    .bind(&prepared.base_commit)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO git_worktrees (id, repository_workspace_id, path, state)
        VALUES ($1, $2, $3, 'creating')
        "#,
    )
    .bind(worktree_id)
    .bind(workspace_id)
    .bind(worktree.to_string_lossy().as_ref())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE agent_runs SET repository_workspace_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(run.run_id)
    .bind(workspace_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let mirror_for_add = mirror.clone();
    let worktree_for_add = worktree.clone();
    let branch_for_add = branch.clone();
    let commit_for_add = prepared.base_commit.clone();
    let creation = tokio::task::spawn_blocking(move || {
        git_runtime::create_worktree(
            &mirror_for_add,
            &worktree_for_add,
            &branch_for_add,
            &commit_for_add,
        )
    })
    .await;
    if !matches!(creation, Ok(Ok(()))) {
        fail_admission(
            state,
            &run,
            WORKTREE_START_FAILURE_REASON,
            Some(worktree_id),
        )
        .await?;
        return Err(AppError::Conflict(
            "git_worktree",
            "worktree creation failed",
        ));
    }
    sqlx::query(
        r#"
        UPDATE git_worktrees
        SET state = 'ready', last_inspected_at = now(), updated_at = now()
        WHERE id = $1 AND state = 'creating'
        "#,
    )
    .bind(worktree_id)
    .execute(crate::db::scoped(&state.pool))
    .await?;

    start_admitted_run(state, &run, worktree).await?;
    clear_claim(state, run.run_id, run.claim_id).await?;
    Ok(())
}

async fn start_admitted_run(
    state: &AppState,
    run: &ClaimedRun,
    worktree: std::path::PathBuf,
) -> Result<(), AppError> {
    if let Err(error) = provider::start_run(
        state,
        run.project_id,
        run.run_id,
        run.conversation_id,
        run.execution_profile_id,
        worktree,
        run.prompt.clone(),
    )
    .await
    {
        fail_admission(state, run, PROVIDER_START_FAILURE_REASON, None).await?;
        return Err(error);
    }
    Ok(())
}

async fn reusable_retry_workspace(
    state: &AppState,
    run: &ClaimedRun,
    predecessor_run_id: Uuid,
    mirror: &Path,
) -> Result<RetryWorkspace, AppError> {
    let workspace = sqlx::query_as::<
        _,
        (
            Option<Uuid>,
            Option<Uuid>,
            Option<String>,
            Option<Uuid>,
            Option<String>,
        ),
    >(
        r#"
        SELECT predecessor.repository_workspace_id,
               workspace.id, workspace.branch_name, worktree.id, worktree.path
          FROM agent_runs predecessor
          LEFT JOIN repository_workspaces workspace
            ON workspace.id = predecessor.repository_workspace_id
           AND workspace.state = 'retained'
          LEFT JOIN git_worktrees worktree
            ON worktree.repository_workspace_id = workspace.id
           AND worktree.state IN ('ready', 'dirty', 'missing', 'failed')
         WHERE predecessor.id = $1
           AND predecessor.project_id = $2
           AND predecessor.conversation_id = $3
           AND predecessor.repository_id = $4
           AND predecessor.state IN ('completed', 'failed', 'canceled', 'interrupted')
        "#,
    )
    .bind(predecessor_run_id)
    .bind(run.project_id)
    .bind(run.conversation_id)
    .bind(run.repository_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?;
    let Some(workspace) = workspace else {
        return Ok(RetryWorkspace::Unsafe);
    };
    let (workspace_id, branch, worktree_id, path) = match workspace {
        (None, None, None, None, None) => return Ok(RetryWorkspace::FreshAllocation),
        (
            Some(predecessor_workspace_id),
            Some(workspace_id),
            Some(branch),
            Some(worktree_id),
            Some(path),
        ) if predecessor_workspace_id == workspace_id => (workspace_id, branch, worktree_id, path),
        _ => return Ok(RetryWorkspace::Unsafe),
    };
    let mirror = mirror.to_path_buf();
    let worktree = std::path::PathBuf::from(path);
    let inspection_path = worktree.clone();
    let inspection = tokio::task::spawn_blocking(move || {
        git_runtime::inspect_worktree(&mirror, &inspection_path)
    })
    .await
    .map_err(|_| AppError::Internal("join retry worktree inspection"))?;
    let Ok(inspection) = inspection else {
        sqlx::query(
            "UPDATE git_worktrees SET state = 'missing', dirty = false, last_inspected_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(worktree_id)
        .execute(crate::db::scoped(&state.pool))
        .await?;
        return Ok(RetryWorkspace::Unsafe);
    };
    if inspection.dirty || inspection.branch != branch {
        sqlx::query(
            "UPDATE git_worktrees SET state = $2, dirty = $3, last_inspected_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(worktree_id)
        .bind(if inspection.dirty { "dirty" } else { "failed" })
        .bind(inspection.dirty)
        .execute(crate::db::scoped(&state.pool))
        .await?;
        return Ok(RetryWorkspace::Unsafe);
    }
    Ok(RetryWorkspace::Reuse(workspace_id, worktree_id, worktree))
}

async fn fail_admission(
    state: &AppState,
    run: &ClaimedRun,
    reason: &'static str,
    failed_worktree_id: Option<Uuid>,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1 FOR UPDATE")
            .bind(run.run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if matches!(
        current.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        transaction.commit().await?;
        return Ok(());
    }
    crate::hosts::builtin::transition(&mut transaction, run.run_id, RunState::Failed, Some(reason))
        .await?;
    provider::idle_provider_session_for_terminal_run(&mut transaction, run.run_id).await?;
    if let Some(worktree_id) = failed_worktree_id {
        sqlx::query("UPDATE git_worktrees SET state = 'failed', updated_at = now() WHERE id = $1")
            .bind(worktree_id)
            .execute(&mut *transaction)
            .await?;
    }
    sqlx::query(
        r#"
        UPDATE repository_workspaces workspace
        SET state = 'retained', updated_at = now()
        FROM agent_runs run
        WHERE run.id = $1 AND workspace.id = run.repository_workspace_id
        "#,
    )
    .bind(run.run_id)
    .execute(&mut *transaction)
    .await?;
    clear_claim_in_transaction(&mut transaction, run.run_id, run.claim_id).await?;
    let event_id = crate::runs::steps::record_new(
        &mut transaction,
        run.run_id,
        "run_failed",
        reason,
        json!({}),
    )
    .await?;
    crate::runs::steps::show_in_thread(&mut transaction, run.conversation_id, event_id).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        run.project_id,
        run.conversation_id,
        run.run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn clear_claim(state: &AppState, run_id: Uuid, claim_id: Uuid) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    clear_claim_in_transaction(&mut transaction, run_id, claim_id).await?;
    transaction.commit().await?;
    Ok(())
}

async fn clear_claim_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    claim_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE agent_runs
        SET scheduler_claim_id = NULL,
            scheduler_claimed_at = NULL,
            scheduler_lease_expires_at = NULL,
            updated_at = now()
        WHERE id = $1 AND scheduler_claim_id = $2
        "#,
    )
    .bind(run_id)
    .bind(claim_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
