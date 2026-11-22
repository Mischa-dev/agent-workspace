//! A1.16, A1.17: the built-in host, driven by its commands. The scheduler
//! admits a queued built-in run and writes it a `start` command, as any host
//! gets one; the host reads its commands through the public host protocol
//! (in process, `transport`) and carries them out in its crate
//! (`runtime::start`). What is left here is the host's own rows, kept in the
//! server's database until the host has a store of its own (A1.17 phase B).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_workspace_builtin_host::runtime::{
    commands,
    start::{
        self, ClaimedRun, NewWorkspace, PredecessorWorkspace, RetryClaim, RunDetail, StartHost,
    },
};
use agent_workspace_host_protocol::{Ack, Command, CommandKind, HostClient};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{AppState, domain::RunState, error::AppError, provider};

use super::report::RunReports;

/// The built-in host's own rows for its runs, over the server's database.
pub(super) struct BuiltinCommands<'a> {
    state: &'a AppState,
}

/// The built-in host's command handlers, for a command carried out at once.
pub(super) fn host(state: &AppState) -> BuiltinCommands<'_> {
    BuiltinCommands { state }
}

impl commands::CommandHost for BuiltinCommands<'_> {
    async fn start(&self, client: &HostClient, command: &Command) -> Ack {
        if super::assistant::is_assistant_start(command) {
            super::assistant::accept_start(self.state, command).await
        } else {
            start::start(self, client, command).await
        }
    }

    async fn inspect(&self, command: &Command) -> Ack {
        start::inspect(self, command).await
    }

    async fn answered(&self, command: &Command) -> Ack {
        start::answered(self, command).await
    }

    async fn cancelled(&self, command: &Command) -> Ack {
        start::cancelled(self, command).await
    }

    async fn action(&self, command: &Command) -> Ack {
        if super::accounts::is_account_action(command) {
            super::accounts::answer(self.state, command).await
        } else if super::repositories::is_repository_action(command) {
            super::repositories::answer(self.state, command).await
        } else {
            commands::refused("unknown_action", "The built-in host has no such action.")
        }
    }
}

/// Carry out every command the built-in hosts have waiting. The lease holder
/// calls it on each tick; tests call it through `scheduler::drain_now`.
pub async fn pump(state: &AppState) -> Result<usize, AppError> {
    let between = || async { Ok::<(), AppError>(()) };
    pump_with(state, &super::transport::client(state), between).await
}

/// [`pump`] with a client already made (the lease holder keeps one), and
/// what to do between commands while the pass is busy (the scheduler
/// admits runs there): see [`commands::pump`].
pub async fn pump_with<B, F>(
    state: &AppState,
    client: &HostClient,
    between: B,
) -> Result<usize, AppError>
where
    B: FnMut() -> F,
    F: std::future::Future<Output = Result<(), AppError>>,
{
    commands::pump(&BuiltinCommands { state }, client, between).await
}

/// Carry out at once what the built-in host has waiting for one run, of
/// these kinds: a person's Stop is heard before the answer to it returns,
/// still through the protocol (listed, fetched, acknowledged).
pub async fn hear_now(
    state: &AppState,
    run_id: Uuid,
    kinds: &[CommandKind],
) -> Result<(), AppError> {
    let client = super::transport::client(state);
    Ok(commands::hear_now(&BuiltinCommands { state }, &client, run_id, kinds).await?)
}

/// A run whose answer lets it go on is working again, unless it can no
/// longer be admitted: then it fails, saying why.
pub(crate) async fn go_on(
    state: &AppState,
    reports: &RunReports,
    run_id: Uuid,
) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let moved = super::move_detail(&mut transaction, run_id, RunState::Working, None)
        .await
        .map_err(crate::domain::refuse_unadmitted_run);
    match moved {
        Ok(_) => {
            transaction.commit().await?;
            Ok(())
        }
        Err(AppError::RunNotAdmitted { reason }) => {
            drop(transaction);
            provider::fail_run(state, reports, run_id, reason).await
        }
        Err(error) => Err(error),
    }
}

/// Stop a run whose approval was denied, or whose request went unanswered.
pub(crate) async fn stop_unanswered(
    state: &AppState,
    reports: &RunReports,
    run_id: Uuid,
    approval: bool,
) -> Result<(), AppError> {
    start::stop_unanswered(&BuiltinCommands { state }, reports, run_id, approval).await
}

/// The host's own row for a claimed run.
#[derive(FromRow)]
struct DetailRow {
    run_id: Uuid,
    project_id: Uuid,
    conversation_id: Uuid,
    repository_id: Uuid,
    execution_profile_id: Uuid,
    adapter_kind: String,
    prompt: String,
    predecessor_run_id: Option<Uuid>,
    claim_id: Uuid,
}

/// Whether the scheduler's claim on a run still holds and it has no
/// workspace yet.
async fn still_claimed(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    claim_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
             WHERE id = $1 AND state = 'starting'
               AND scheduler_claim_id = $2
               AND repository_workspace_id IS NULL
        )
        "#,
    )
    .bind(run_id)
    .bind(claim_id)
    .fetch_one(&mut **transaction)
    .await?)
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

impl StartHost for BuiltinCommands<'_> {
    type Error = AppError;

    fn category(error: &AppError) -> &'static str {
        error.log_category()
    }

    fn data_root(&self) -> &Path {
        &self.state.config.data_root
    }

    async fn detail_state(&self, run_id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(crate::db::scoped(&self.state.pool))
            .await
            .ok()
            .flatten()
    }

    async fn claimed_detail(&self, run_id: Uuid) -> Result<Option<RunDetail>, AppError> {
        let row = sqlx::query_as::<_, DetailRow>(
            r#"
            SELECT run.id AS run_id, run.project_id, run.conversation_id,
                   run.repository_id, run.execution_profile_id, account.adapter_kind,
                   run.prompt, run.predecessor_run_id, run.scheduler_claim_id AS claim_id
            FROM agent_runs run
            JOIN execution_profiles profile ON profile.id = run.execution_profile_id
            JOIN provider_accounts account ON account.id = profile.provider_account_id
            WHERE run.id = $1 AND run.state = 'starting'
              AND run.scheduler_claim_id IS NOT NULL
              AND run.repository_workspace_id IS NULL
            "#,
        )
        .bind(run_id)
        .fetch_optional(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(row.map(|row| RunDetail {
            run_id: row.run_id,
            project_id: row.project_id,
            conversation_id: row.conversation_id,
            repository_id: row.repository_id,
            execution_profile_id: row.execution_profile_id,
            adapter_kind: row.adapter_kind,
            prompt: row.prompt,
            predecessor_run_id: row.predecessor_run_id,
            claim_id: row.claim_id,
        }))
    }

    async fn end_without_detail(&self, run_id: Uuid) -> Result<(), AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        crate::runs::ops::end_by_service(
            &mut transaction,
            run_id,
            crate::runs::RunState::Cancelled,
            "Stopped.",
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn open_reports(&self, run_id: Uuid) -> Result<RunReports, AppError> {
        super::report::open(self.state, run_id).await
    }

    async fn go_on(&self, reports: &RunReports, run_id: Uuid) -> Result<(), AppError> {
        go_on(self.state, reports, run_id).await
    }

    async fn report_changes(&self, reports: &RunReports, run_id: Uuid) {
        provider::report_changes_of(self.state, reports, run_id).await;
    }

    async fn interrupted(&self, run_id: Uuid, reason: &str) -> Result<(), AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        super::move_detail(
            &mut transaction,
            run_id,
            RunState::Interrupted,
            Some(reason),
        )
        .await?;
        super::http::retain_run_workspace(&mut transaction, run_id).await?;
        provider::idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn take_start(
        &self,
        run_id: Uuid,
        claim_id: Uuid,
        command_id: Uuid,
    ) -> Result<bool, AppError> {
        // A concurrent taker waits on the row and then finds it taken.
        let taken = sqlx::query(
            r#"
            UPDATE agent_runs SET start_command_id = $3
             WHERE id = $1 AND state = 'starting'
               AND scheduler_claim_id = $2
               AND repository_workspace_id IS NULL
               AND start_command_id IS DISTINCT FROM $3
            "#,
        )
        .bind(run_id)
        .bind(claim_id)
        .bind(command_id)
        .execute(crate::db::scoped(&self.state.pool))
        .await?
        .rows_affected();
        Ok(taken == 1)
    }

    async fn start_going(&self, run_id: Uuid, command_id: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT state IN ('working', 'blocked', 'recovering')
                OR (state = 'starting' AND start_command_id = $2)
              FROM agent_runs WHERE id = $1
            "#,
        )
        .bind(run_id)
        .bind(command_id)
        .fetch_optional(crate::db::scoped(&self.state.pool))
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    }

    async fn room_to_grow(&self) -> Result<(), AppError> {
        let root = self.state.config.data_root.clone();
        let quota = self.state.config.data_quota_bytes;
        let free = self.state.config.minimum_free_bytes;
        tokio::task::spawn_blocking(move || {
            crate::storage_admission::require_growth(&root, quota, free)
        })
        .await
        .map_err(|_| AppError::Internal("join storage admission"))?
    }

    fn branch_prefix(&self, adapter_kind: &str) -> Option<String> {
        self.state
            .supervisors
            .manifest(adapter_kind)
            .map(|manifest| manifest.branch_prefix.clone())
    }

    async fn predecessor_workspace(
        &self,
        run: &ClaimedRun,
        predecessor_run_id: Uuid,
    ) -> Result<PredecessorWorkspace, AppError> {
        let found = sqlx::query_as::<
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
        .fetch_optional(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(match found {
            None => PredecessorWorkspace::Unsafe,
            Some((None, None, None, None, None)) => PredecessorWorkspace::None,
            Some((Some(held), Some(workspace_id), Some(branch), Some(worktree_id), Some(path)))
                if held == workspace_id =>
            {
                PredecessorWorkspace::Retained {
                    workspace_id,
                    branch,
                    worktree_id,
                    path: PathBuf::from(path),
                }
            }
            Some(_) => PredecessorWorkspace::Unsafe,
        })
    }

    async fn worktree_inspected(
        &self,
        worktree_id: Uuid,
        state: &'static str,
        dirty: bool,
    ) -> Result<(), AppError> {
        // A worktree the retention cleanup removed stays removed: an
        // inspection that raced the removal found it gone on purpose.
        sqlx::query(
            "UPDATE git_worktrees SET state = $2, dirty = $3, last_inspected_at = now(), updated_at = now() WHERE id = $1 AND state <> 'removed'",
        )
        .bind(worktree_id)
        .bind(state)
        .bind(dirty)
        .execute(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(())
    }

    async fn claim_retry(
        &self,
        run: &ClaimedRun,
        workspace_id: Uuid,
        worktree_id: Uuid,
    ) -> Result<RetryClaim, AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        if !still_claimed(&mut transaction, run.run_id, run.claim_id).await? {
            return Ok(RetryClaim::Lost);
        }
        // Retention cleanup removes retained worktrees under this lock. Take
        // it, then claim only if the workspace is still retained: one
        // cleaned up since the inspection is gone.
        super::worktrees::lock_repository_retention(&mut transaction, run.repository_id).await?;
        // The retry publishes its own commit: the predecessor's publication
        // must not stand in for it, or a pull request could open from a
        // commit this run never made.
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
            return Ok(RetryClaim::Cleaned);
        }
        sqlx::query(
            "UPDATE agent_runs SET repository_workspace_id = $2, updated_at = now() WHERE id = $1",
        )
        .bind(run.run_id)
        .bind(workspace_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(RetryClaim::Claimed)
    }

    async fn allocate(
        &self,
        run: &ClaimedRun,
        workspace: NewWorkspace<'_>,
    ) -> Result<bool, AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        if !still_claimed(&mut transaction, run.run_id, run.claim_id).await? {
            return Ok(false);
        }
        sqlx::query(
            r#"
            INSERT INTO repository_workspaces (
                id, repository_id, conversation_id, branch_name, base_commit
            )
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(workspace.workspace_id)
        .bind(run.repository_id)
        .bind(run.conversation_id)
        .bind(workspace.branch)
        .bind(workspace.base_commit)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO git_worktrees (id, repository_workspace_id, path, state)
            VALUES ($1, $2, $3, 'creating')
            "#,
        )
        .bind(workspace.worktree_id)
        .bind(workspace.workspace_id)
        .bind(workspace.path.to_string_lossy().as_ref())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE agent_runs SET repository_workspace_id = $2, updated_at = now() WHERE id = $1",
        )
        .bind(run.run_id)
        .bind(workspace.workspace_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    async fn worktree_ready(&self, worktree_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE git_worktrees
            SET state = 'ready', last_inspected_at = now(), updated_at = now()
            WHERE id = $1 AND state = 'creating'
            "#,
        )
        .bind(worktree_id)
        .execute(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(())
    }

    async fn start_turn(
        &self,
        reports: Arc<RunReports>,
        run: &ClaimedRun,
        worktree: PathBuf,
    ) -> Result<(), AppError> {
        provider::start_run(
            self.state,
            reports,
            run.run_id,
            run.conversation_id,
            run.execution_profile_id,
            worktree,
            run.prompt.clone(),
        )
        .await
        .map(|_| ())
    }

    async fn failed(
        &self,
        run: &ClaimedRun,
        reason: &'static str,
        failed_worktree: Option<Uuid>,
    ) -> Result<(), AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        super::move_detail(&mut transaction, run.run_id, RunState::Failed, Some(reason)).await?;
        provider::idle_provider_session_for_terminal_run(&mut transaction, run.run_id).await?;
        if let Some(worktree_id) = failed_worktree {
            sqlx::query(
                "UPDATE git_worktrees SET state = 'failed', updated_at = now() WHERE id = $1",
            )
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
        transaction.commit().await?;
        Ok(())
    }

    async fn clear_claim(&self, run_id: Uuid, claim_id: Uuid) -> Result<(), AppError> {
        let mut transaction = crate::db::scoped_tx(&self.state.pool).await?;
        clear_claim_in_transaction(&mut transaction, run_id, claim_id).await?;
        transaction.commit().await?;
        Ok(())
    }
}
