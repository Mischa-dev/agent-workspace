use std::{path::Path, time::Duration};

use serde_json::json;
use sqlx::{FromRow, Postgres, Transaction};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    AppState,
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::AppError,
    git_runtime::{
        self, RegisteredRepository, expected_repository_mirror_path, managed_worktree_path,
    },
    outbox, provider,
};

const SCHEDULER_LEASE: Duration = Duration::from_secs(30);
const WORKTREE_START_FAILURE_REASON: &str =
    "Worktree creation failed; inspect the retained repository workspace.";
const PROVIDER_START_FAILURE_REASON: &str =
    "Codex App Server could not start. The worktree was preserved.";
const CLEANED_RETRY_REASON: &str = "The previous worktree was removed by retention cleanup before the retry could use it. Start a new Thread to continue.";
const UNSAFE_RETRY_REASON: &str = "The previous worktree is dirty, missing, or no longer has its owned branch. It was preserved; start a new Thread or inspect it before retrying.";

enum RetryWorkspace {
    FreshAllocation,
    Reuse(Uuid, Uuid, std::path::PathBuf),
    Unsafe,
}

#[derive(Debug, FromRow)]
struct ClaimedRun {
    run_id: Uuid,
    project_id: Uuid,
    conversation_id: Uuid,
    repository_id: Uuid,
    mirror_path: String,
    clone_url: String,
    default_branch: String,
    project_key: String,
    execution_profile_id: Uuid,
    requesting_actor_id: Uuid,
    prompt: String,
    predecessor_run_id: Option<Uuid>,
    claim_id: Uuid,
}

pub fn kick(state: AppState) {
    tokio::spawn(async move {
        if let Err(error) = drain(&state).await {
            tracing::error!(
                error_category = error.log_category(),
                "Agent Run scheduler kick failed"
            );
        }
    });
}

pub async fn drain_now(state: &AppState) -> Result<(), AppError> {
    drain(state).await
}

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => drain(&state).await?,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn drain(state: &AppState) -> Result<(), AppError> {
    while let Some(run) = claim_next(state).await? {
        let run_id = run.run_id;
        if let Err(error) = admit(state, run).await {
            tracing::error!(
                %run_id,
                error_category = error.log_category(),
                "Agent Run admission failed"
            );
        }
    }
    Ok(())
}

async fn claim_next(state: &AppState) -> Result<Option<ClaimedRun>, AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('agent_workspace_scheduler', 0))")
        .execute(&mut *transaction)
        .await?;
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
    .execute(&mut *transaction)
    .await?;
    let active = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM agent_runs
        WHERE state IN ('starting', 'working', 'blocked', 'recovering')
        "#,
    )
    .fetch_one(&mut *transaction)
    .await?;
    if active >= i64::try_from(state.config.codex_host_concurrency_limit).unwrap_or(i64::MAX) {
        transaction.commit().await?;
        return Ok(None);
    }

    #[derive(FromRow)]
    struct Candidate {
        run_id: Uuid,
        project_id: Uuid,
        conversation_id: Uuid,
        repository_id: Uuid,
        mirror_path: String,
        clone_url: String,
        default_branch: String,
        project_key: String,
        execution_profile_id: Uuid,
        requesting_actor_id: Uuid,
        prompt: String,
        predecessor_run_id: Option<Uuid>,
    }
    let candidate = sqlx::query_as::<_, Candidate>(
        r#"
        SELECT run.id AS run_id, run.project_id, run.conversation_id,
               run.repository_id, repository.mirror_path, repository.clone_url,
               repository.default_branch, project.project_key,
               run.execution_profile_id, run.requesting_actor_id, run.prompt,
               run.predecessor_run_id
        FROM agent_runs run
        JOIN repositories repository
          ON repository.id = run.repository_id
         AND repository.project_id = run.project_id
        JOIN projects project ON project.id = run.project_id
        WHERE run.state = 'queued'
          AND run.scheduler_claim_id IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_runs active
              WHERE active.execution_profile_id = run.execution_profile_id
                AND active.state IN ('starting', 'working', 'blocked', 'recovering')
          )
        ORDER BY run.created_at, run.id
        FOR UPDATE OF run SKIP LOCKED
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(candidate) = candidate else {
        transaction.commit().await?;
        return Ok(None);
    };
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
    transition_run(&mut transaction, candidate.run_id, RunState::Starting, None).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        candidate.project_id,
        candidate.conversation_id,
        candidate.run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Some(ClaimedRun {
        run_id: candidate.run_id,
        project_id: candidate.project_id,
        conversation_id: candidate.conversation_id,
        repository_id: candidate.repository_id,
        mirror_path: candidate.mirror_path,
        clone_url: candidate.clone_url,
        default_branch: candidate.default_branch,
        project_key: candidate.project_key,
        execution_profile_id: candidate.execution_profile_id,
        requesting_actor_id: candidate.requesting_actor_id,
        prompt: candidate.prompt,
        predecessor_run_id: candidate.predecessor_run_id,
        claim_id,
    }))
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
            return Err(error);
        }
    };

    if let Some(predecessor_run_id) = run.predecessor_run_id {
        match reusable_retry_workspace(state, &run, predecessor_run_id, &mirror).await? {
            RetryWorkspace::Reuse(workspace_id, worktree_id, worktree) => {
                let mut transaction = state.pool.begin().await?;
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
                let claimed = sqlx::query(
                    "UPDATE repository_workspaces SET state = 'active', updated_at = now() WHERE id = $1 AND state = 'retained'",
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
    let branch = git_runtime::unique_run_branch(&run.project_key)?;
    let worktree = managed_worktree_path(&state.config.data_root, run.repository_id, workspace_id)?;
    let mut transaction = state.pool.begin().await?;
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
    .execute(&state.pool)
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
    if state.config.demo_executor {
        prepare_demo_blocker(state, run).await?;
    } else if let Err(error) = provider::start_run(
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
    .fetch_optional(&state.pool)
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
        .execute(&state.pool)
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
        .execute(&state.pool)
        .await?;
        return Ok(RetryWorkspace::Unsafe);
    }
    Ok(RetryWorkspace::Reuse(workspace_id, worktree_id, worktree))
}

async fn prepare_demo_blocker(state: &AppState, run: &ClaimedRun) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    transition_run(&mut transaction, run.run_id, RunState::Working, None).await?;
    let approval_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        )
        VALUES ($1, $2, $3, $4, 'approval', $5, $6, 'owner', true)
        "#,
    )
    .bind(approval_id)
    .bind(run.run_id)
    .bind(run.requesting_actor_id)
    .bind(format!("development-approval-{approval_id}"))
    .bind("Allow the development executor to append its proof line to KNOWN_FILE.txt?")
    .bind(json!([
        {"id": "approve", "label": "Allow once"},
        {"id": "deny", "label": "Interrupt run"}
    ]))
    .execute(&mut *transaction)
    .await?;
    crate::external_events::enqueue_external_approval_requested(&mut transaction, approval_id)
        .await?;
    transition_run(&mut transaction, run.run_id, RunState::Blocked, None).await?;
    let event_id = insert_event(
        &mut transaction,
        run.run_id,
        "approval_requested",
        "The run is waiting for an approval.",
        json!({"approval_id": approval_id}),
    )
    .await?;
    insert_entry(&mut transaction, run.conversation_id, event_id).await?;
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

async fn fail_admission(
    state: &AppState,
    run: &ClaimedRun,
    reason: &'static str,
    failed_worktree_id: Option<Uuid>,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
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
    transition_run(&mut transaction, run.run_id, RunState::Failed, Some(reason)).await?;
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
    let event_id = insert_event(
        &mut transaction,
        run.run_id,
        "run_failed",
        reason,
        json!({}),
    )
    .await?;
    insert_entry(&mut transaction, run.conversation_id, event_id).await?;
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
    let mut transaction = state.pool.begin().await?;
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

async fn insert_event(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    kind: &str,
    summary: &str,
    payload: serde_json::Value,
) -> Result<Uuid, AppError> {
    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (id, agent_run_id, event_key, kind, summary, payload)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(event_id)
    .bind(run_id)
    .bind(format!("{kind}:{event_id}"))
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    Ok(event_id)
}

async fn insert_entry(
    transaction: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
    event_id: Uuid,
) -> Result<(), AppError> {
    let sequence = allocate_conversation_sequence(transaction, conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, agent_run_event_id
        )
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(sequence)
    .bind(event_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
