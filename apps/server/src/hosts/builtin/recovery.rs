//! A1.17: recovery of the built-in host's runs as its lease is taken. The
//! work is the host crate's (`runtime::recovery`); here are the host's rows,
//! kept in the server's database until it has a store of its own, each
//! write fenced by the lease.

use std::path::PathBuf;

use agent_workspace_builtin_host::runtime::recovery::{
    self, Found, OwnedWorktree, RecoverableRun, RecoveryHost,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    cluster::{Cluster, Lease},
    domain::RunState,
    error::AppError,
    outbox,
    provider::{self, ProviderRecovery},
};

/// Reconciles runs that were non-terminal when the host process stopped.
///
/// Provider and Git inspection happen outside database transactions. A stored
/// provider Thread is resumed and read before the application decides whether
/// completion is provable. Any ambiguous state terminates as interrupted while
/// retaining the exact worktree.
pub async fn reconcile_non_terminal_runs(state: &AppState) -> Result<u64, AppError> {
    // Recovers runs of every workspace.
    crate::tenancy::system(
        "runs.host",
        Box::pin(recovery::recover_runs(&Recovery { state, fence: None })),
    )
    .await
}

/// The built-in host's lease that recovery's writes are fenced by, so a
/// server that lost the host can't overwrite what the new holder recovers.
/// None only in tests of recovery alone.
pub(crate) type HostFence<'a> = Option<(&'a Cluster, &'a Lease)>;

/// A transaction that fails with `LeaseLost` unless the fence's lease is
/// still held, and keeps it from being taken until the transaction ends.
pub(crate) async fn fenced_tx(
    pool: &PgPool,
    fence: HostFence<'_>,
) -> Result<Transaction<'static, Postgres>, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    if let Some((cluster, lease)) = fence {
        cluster.fence(&mut transaction, lease).await?;
    }
    Ok(transaction)
}

/// What the built-in host's holder does as it takes the lease (K5.3), at
/// startup or when it takes over from a holder that died: recover the runs
/// that were running, each transition fenced by the lease, then the
/// worktrees and what finished runs still asked for.
pub async fn reconcile_owned(
    state: &AppState,
    cluster: &Cluster,
    lease: &Lease,
) -> Result<(), AppError> {
    // Recovers runs of every workspace.
    crate::tenancy::system(
        "runs.host",
        Box::pin(recovery::take_over(&Recovery {
            state,
            fence: Some((cluster, lease)),
        })),
    )
    .await
}

pub async fn reconcile_registered_worktrees(state: &AppState) -> Result<u64, AppError> {
    // Recovers runs of every workspace.
    crate::tenancy::system(
        "runs.host",
        Box::pin(recovery::inspect_worktrees(&Recovery {
            state,
            fence: None,
        })),
    )
    .await
}

/// What finished runs still asked for is withdrawn (their finish withdraws
/// it; this catches any a crash left open).
pub async fn withdraw_finished_run_requests(state: &AppState) -> Result<u64, AppError> {
    // Recovers runs of every workspace.
    crate::tenancy::system("runs.host", async {
        Recovery { state, fence: None }.withdraw_requests().await
    })
    .await
}

/// The built-in host's rows, as recovery reads and writes them.
struct Recovery<'a> {
    state: &'a AppState,
    fence: HostFence<'a>,
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: Uuid,
    previous_state: String,
    conversation_id: Uuid,
    project_id: Uuid,
}

impl RecoveryHost for Recovery<'_> {
    type Error = AppError;

    // A queued run's scheduler claim is left alone: a lapsed one is cleared
    // by the next claim, and a live one belongs to whoever holds it (K5.3).
    async fn recoverable_runs(&self) -> Result<Vec<RecoverableRun>, AppError> {
        let runs = sqlx::query_as::<_, RunRow>(
            r#"
            SELECT run.id, run.state AS previous_state, run.conversation_id,
                   conversation.project_id
            -- visibility: recovery reads only the project of each run's own conversation
            FROM agent_runs run
            JOIN conversations conversation ON conversation.id = run.conversation_id
            WHERE run.state IN ('starting', 'working', 'blocked', 'recovering')
            ORDER BY run.created_at
            "#,
        )
        .fetch_all(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(runs
            .into_iter()
            .map(|run| RecoverableRun {
                id: run.id,
                previous_state: run.previous_state,
                conversation_id: run.conversation_id,
                project_id: run.project_id,
            })
            .collect())
    }

    async fn recovering(&self, run: &RecoverableRun) -> Result<(), AppError> {
        let mut transaction = fenced_tx(&self.state.pool, self.fence).await?;
        crate::hosts::builtin::transition(&mut transaction, run.id, RunState::Recovering, None)
            .await?;
        outbox::enqueue_agent_run_updated(
            &mut transaction,
            run.project_id,
            run.conversation_id,
            run.id,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn owned_worktree(&self, run_id: Uuid) -> Result<Option<OwnedWorktree>, AppError> {
        let found = sqlx::query_as::<_, (Uuid, String, Uuid, String)>(
            r#"
            SELECT gw.id, gw.path, ar.repository_id, rw.branch_name
            FROM agent_runs ar
            JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
            JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
            WHERE ar.id = $1
              AND ar.repository_id IS NOT NULL
              AND gw.state IN ('creating', 'ready', 'dirty', 'missing', 'failed')
            "#,
        )
        .bind(run_id)
        .fetch_optional(crate::db::scoped(&self.state.pool))
        .await?;
        let Some((id, path, repository_id, branch)) = found else {
            return Ok(None);
        };
        // The mirror is wherever this host keeps it, not where the server
        // that registered the repository would.
        let mirror = crate::hosts::builtin::git::expected_repository_mirror_path(
            &self.state.config.data_root,
            repository_id,
        )?;
        Ok(Some(OwnedWorktree {
            id,
            path: PathBuf::from(path),
            mirror,
            branch,
        }))
    }

    async fn record_inspection(
        &self,
        worktree_id: Uuid,
        state: &'static str,
        dirty: bool,
    ) -> Result<(), AppError> {
        // Inspection took a while: write only while the host is ours. A
        // worktree retention removed meanwhile (a sweep the last holder
        // finished as this one took over) stays removed.
        let mut transaction = fenced_tx(&self.state.pool, self.fence).await?;
        sqlx::query(
            r#"
            UPDATE git_worktrees
            SET state = $2, dirty = $3,
                last_inspected_at = now(), updated_at = now()
            WHERE id = $1 AND state <> 'removed'
            "#,
        )
        .bind(worktree_id)
        .bind(state)
        .bind(dirty)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn inspect_harness(&self, run_id: Uuid, worktree: PathBuf) -> ProviderRecovery {
        provider::inspect_provider_for_recovery(self.state, run_id, worktree).await
    }

    async fn complete(&self, run: &RecoverableRun, message: &str, found: Found) -> bool {
        provider::complete_recovered_run(
            self.state,
            self.fence,
            run.id,
            message,
            &run.previous_state,
            found.worktree_state,
            found.dirty,
        )
        .await
        .is_ok()
    }

    async fn follow(&self, run_id: Uuid, worktree: PathBuf) -> bool {
        provider::follow_recovered_run(self.state, self.fence, run_id, worktree)
            .await
            .unwrap_or(false)
    }

    async fn interrupt(
        &self,
        run: &RecoverableRun,
        reason: &str,
        found: Found,
    ) -> Result<(), AppError> {
        let mut transaction = fenced_tx(&self.state.pool, self.fence).await?;
        crate::hosts::builtin::transition(
            &mut transaction,
            run.id,
            RunState::Interrupted,
            Some(reason),
        )
        .await?;
        sqlx::query(
            r#"
            UPDATE repository_workspaces rw
            SET state = 'retained', updated_at = now()
            FROM agent_runs ar
            WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
            "#,
        )
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        let step = crate::runs::steps::record(
            &mut transaction,
            run.id,
            "startup-recovery:interrupted",
            "recovery",
            reason,
            serde_json::json!({
                "previous_state": run.previous_state,
                "result": "interrupted",
                "provider_resumed": found.harness_resumed,
                "worktree_preserved": true,
                "worktree_state": found.worktree_state,
                "dirty": found.dirty
            }),
        )
        .await?;
        if let Some(step) = step {
            crate::runs::steps::show_in_thread(&mut transaction, run.conversation_id, step).await?;
        }
        outbox::enqueue_agent_run_updated(
            &mut transaction,
            run.project_id,
            run.conversation_id,
            run.id,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn idle_sessions(&self) -> Result<u64, AppError> {
        let mut transaction = fenced_tx(&self.state.pool, self.fence).await?;
        let result = sqlx::query(
            r#"
            UPDATE provider_sessions ps
            SET state = 'idle', updated_at = now()
            WHERE ps.state = 'active'
              AND EXISTS (
                  SELECT 1
                  FROM agent_runs linked
                  WHERE linked.provider_session_id = ps.id
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM agent_runs active
                  WHERE active.provider_session_id = ps.id
                    AND active.state IN (
                        'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                    )
              )
            "#,
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    async fn registered_worktrees(&self) -> Result<Vec<(Uuid, PathBuf)>, AppError> {
        let worktrees = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT id, path
            FROM git_worktrees
            WHERE state IN ('creating', 'ready', 'dirty', 'missing', 'failed')
            ORDER BY created_at
            "#,
        )
        .fetch_all(crate::db::scoped(&self.state.pool))
        .await?;
        Ok(worktrees
            .into_iter()
            .map(|(id, path)| (id, PathBuf::from(path)))
            .collect())
    }

    async fn withdraw_requests(&self) -> Result<u64, AppError> {
        let mut transaction = fenced_tx(&self.state.pool, self.fence).await?;
        let runs = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT request.run_id
            FROM requests request
            JOIN agent_runs run ON run.id = request.run_id
            WHERE request.status = 'open' AND run.finished_at IS NOT NULL
            ORDER BY request.run_id
            "#,
        )
        .fetch_all(&mut *transaction)
        .await?;
        for run_id in &runs {
            crate::requests::withdraw_for_run(&mut transaction, *run_id).await?;
        }
        transaction.commit().await?;
        Ok(runs.len() as u64)
    }
}
