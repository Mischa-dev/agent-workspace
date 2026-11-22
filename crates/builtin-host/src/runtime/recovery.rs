//! A1.17: what the built-in host does as it takes over its runs (at startup,
//! or from a holder that died). Each run that was going is looked at again:
//! its worktree is inspected, its harness session resumed and read, and the
//! run finishes when its completion is provable, is followed again when its
//! harness is still working on it, and is otherwise interrupted with its
//! worktree kept. The host's rows are behind [`RecoveryHost`]; inspection of
//! harnesses and worktrees happens outside any of its transactions.

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::{RuntimeError, git};

/// What resuming a run's harness session found.
#[derive(Clone, Debug)]
pub enum HarnessRecovery {
    /// The turn completed with this message.
    Completed { message: String },
    /// The session resumed; its latest turn has this status.
    Resumed { latest_turn_status: String },
    /// The session was recorded before the turn it ran was.
    UnboundTurn,
    /// The session's latest turn isn't the one recorded.
    MismatchedTurn,
    /// The completion's message was invalid or too large.
    RejectedMessage,
    /// The session couldn't be resumed.
    Unavailable,
}

/// A run that was going when the host stopped.
#[derive(Clone, Debug)]
pub struct RecoverableRun {
    pub id: Uuid,
    pub previous_state: String,
    pub conversation_id: Uuid,
    pub project_id: Uuid,
}

/// The worktree a run owns, with the mirror and branch it should be on.
#[derive(Clone, Debug)]
pub struct OwnedWorktree {
    pub id: Uuid,
    pub path: PathBuf,
    pub mirror: PathBuf,
    pub branch: String,
}

/// What a worktree inspection found, for the run's log.
#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub worktree_state: &'static str,
    pub dirty: bool,
    pub harness_resumed: bool,
}

/// What recovery needs from the host it runs in. Every write is fenced by
/// the host's lease where the host has one.
pub trait RecoveryHost: Send + Sync {
    type Error: From<RuntimeError> + Send;

    /// The runs that were going, oldest first.
    fn recoverable_runs(
        &self,
    ) -> impl Future<Output = Result<Vec<RecoverableRun>, Self::Error>> + Send;

    /// The run is being recovered.
    fn recovering(
        &self,
        run: &RecoverableRun,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The worktree the run owns, if it has one still registered.
    fn owned_worktree(
        &self,
        run_id: Uuid,
    ) -> impl Future<Output = Result<Option<OwnedWorktree>, Self::Error>> + Send;

    /// Record what an inspection found of a worktree.
    fn record_inspection(
        &self,
        worktree_id: Uuid,
        state: &'static str,
        dirty: bool,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Resume the run's harness session in `worktree` and read where it is.
    fn inspect_harness(
        &self,
        run_id: Uuid,
        worktree: PathBuf,
    ) -> impl Future<Output = HarnessRecovery> + Send;

    /// Finish a run whose completion was observed. `false` if it couldn't
    /// be recorded (it is interrupted instead).
    fn complete(
        &self,
        run: &RecoverableRun,
        message: &str,
        found: Found,
    ) -> impl Future<Output = bool> + Send;

    /// Follow again a run its harness is still working on. `false` if it
    /// can't be (it is interrupted instead).
    fn follow(&self, run_id: Uuid, worktree: PathBuf) -> impl Future<Output = bool> + Send;

    /// Interrupt the run for `reason`, keeping its workspace, and log what
    /// was found.
    fn interrupt(
        &self,
        run: &RecoverableRun,
        reason: &str,
        found: Found,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Idle harness sessions no going run holds. Returns how many.
    fn idle_sessions(&self) -> impl Future<Output = Result<u64, Self::Error>> + Send;

    /// Every registered worktree that may still be on disk.
    fn registered_worktrees(
        &self,
    ) -> impl Future<Output = Result<Vec<(Uuid, PathBuf)>, Self::Error>> + Send;

    /// Withdraw what finished runs still asked for. Returns how many runs.
    fn withdraw_requests(&self) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}

/// Everything recovery does as the host is taken over: its runs, then its
/// worktrees and what finished runs still asked for.
pub async fn take_over<H: RecoveryHost>(host: &H) -> Result<(), H::Error> {
    let recovered = recover_runs(host).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "reconciled non-terminal Agent Runs");
    }
    let inspected_worktrees = inspect_worktrees(host).await?;
    let withdrawn_requests = host.withdraw_requests().await?;
    tracing::info!(
        inspected_worktrees,
        withdrawn_requests,
        "built-in host reconciliation completed"
    );
    Ok(())
}

/// Recover each run that was going. Returns how many there were.
pub async fn recover_runs<H: RecoveryHost>(host: &H) -> Result<u64, H::Error> {
    let runs = host.recoverable_runs().await?;
    for run in &runs {
        recover(host, run).await?;
    }
    let idled = host.idle_sessions().await?;
    if idled > 0 {
        tracing::warn!(
            idled_provider_sessions = idled,
            "reconciled active provider Sessions without a non-terminal Agent Run"
        );
    }
    Ok(runs.len() as u64)
}

async fn recover<H: RecoveryHost>(host: &H, run: &RecoverableRun) -> Result<(), H::Error> {
    host.recovering(run).await?;
    let (worktree, worktree_state, dirty) = match host.owned_worktree(run.id).await? {
        Some(owned) => {
            let (path, mirror, branch) = (owned.path.clone(), owned.mirror, owned.branch);
            let (worktree_state, dirty) = tokio::task::spawn_blocking(move || {
                inspect_owned_worktree(&mirror, &path, &branch)
            })
            .await
            .map_err(|_| RuntimeError::Internal("join recovery Git inspection"))?;
            host.record_inspection(owned.id, worktree_state, dirty)
                .await?;
            (Some(owned.path), worktree_state, dirty)
        }
        None => (None, "missing", false),
    };
    let outcome = match &worktree {
        Some(path) if matches!(worktree_state, "ready" | "dirty") => {
            host.inspect_harness(run.id, path.clone()).await
        }
        _ => HarnessRecovery::Unavailable,
    };
    let found = Found {
        worktree_state,
        dirty,
        harness_resumed: matches!(
            outcome,
            HarnessRecovery::Completed { .. }
                | HarnessRecovery::Resumed { .. }
                | HarnessRecovery::MismatchedTurn
                | HarnessRecovery::RejectedMessage
        ),
    };
    if let HarnessRecovery::Completed { message } = &outcome
        && host.complete(run, message, found).await
    {
        return Ok(());
    }
    // A cloud task keeps running while this host restarts: follow it again
    // rather than interrupting the run.
    if let (HarnessRecovery::Resumed { latest_turn_status }, Some(path)) = (&outcome, &worktree)
        && latest_turn_status == "inProgress"
        && host.follow(run.id, path.clone()).await
    {
        return Ok(());
    }
    host.interrupt(run, &interrupted_reason(&outcome, dirty), found)
        .await
}

/// Why a run recovery couldn't finish or follow was interrupted.
fn interrupted_reason(outcome: &HarnessRecovery, dirty: bool) -> String {
    match outcome {
        HarnessRecovery::Resumed { latest_turn_status } => {
            let status = known_turn_status(latest_turn_status);
            format!(
                "Service restarted during a run. The provider Thread resumed, but its latest turn was {status} and could not be safely continued; the worktree was preserved for retry."
            )
        }
        HarnessRecovery::Completed { .. } => {
            "Service restarted during a run. Provider completion was observed but could not be reconciled transactionally; the worktree was preserved for inspection.".to_owned()
        }
        HarnessRecovery::UnboundTurn => {
            "Service restarted after the provider Thread was recorded but before an exact turn identity was durable. No older provider result was inferred; the worktree was preserved for explicit retry.".to_owned()
        }
        HarnessRecovery::MismatchedTurn => {
            "Service restarted during a run, but the provider's latest turn did not match the durable turn identity. The ambiguous provider history and worktree were preserved for inspection.".to_owned()
        }
        HarnessRecovery::RejectedMessage => {
            "Service restarted during a run. Provider completion contained an invalid or oversized Message, so the run was interrupted and the worktree was preserved for inspection.".to_owned()
        }
        HarnessRecovery::Unavailable if dirty => {
            "Service restarted during a run. Provider resume was unavailable and the dirty worktree was preserved; inspect it before retrying.".to_owned()
        }
        HarnessRecovery::Unavailable => {
            "Service restarted during a run. Provider resume was unavailable, so the worktree was preserved for an explicit retry.".to_owned()
        }
    }
}

/// Inspect every registered worktree and record what is on disk. Returns
/// how many.
pub async fn inspect_worktrees<H: RecoveryHost>(host: &H) -> Result<u64, H::Error> {
    let worktrees = host.registered_worktrees().await?;
    for (worktree_id, path) in &worktrees {
        let path = path.clone();
        let (state, dirty) = tokio::task::spawn_blocking(move || inspect_worktree(&path))
            .await
            .map_err(|_| RuntimeError::Internal("join registered worktree inspection"))?;
        host.record_inspection(*worktree_id, state, dirty).await?;
    }
    Ok(worktrees.len() as u64)
}

fn inspect_worktree(path: &Path) -> (&'static str, bool) {
    if !path.is_dir() {
        return ("missing", false);
    }
    match git::worktree_is_dirty(path) {
        Ok(false) => ("ready", false),
        Ok(true) => ("dirty", true),
        Err(_) => ("missing", false),
    }
}

fn inspect_owned_worktree(
    mirror: &Path,
    path: &Path,
    expected_branch: &str,
) -> (&'static str, bool) {
    match git::inspect_worktree(mirror, path) {
        Ok(inspection) if inspection.branch == expected_branch && inspection.dirty => {
            ("dirty", true)
        }
        Ok(inspection) if inspection.branch == expected_branch => ("ready", false),
        Ok(_) => ("failed", false),
        Err(_) => ("missing", false),
    }
}

fn known_turn_status(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "interrupted" => "interrupted",
        "failed" => "failed",
        "inProgress" => "in progress",
        "missing" => "missing",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect_owned_worktree, inspect_worktree, known_turn_status};
    use std::path::Path;

    #[test]
    fn missing_worktree_is_not_deleted_or_called_clean() {
        assert_eq!(
            inspect_worktree(Path::new("/definitely/not/an/agent-workspace")),
            ("missing", false)
        );
    }

    #[test]
    fn provider_status_is_bounded_to_known_values() {
        assert_eq!(known_turn_status("inProgress"), "in progress");
        assert_eq!(known_turn_status("secret provider text"), "unknown");
    }

    #[test]
    fn owned_inspection_rejects_missing_or_foreign_git_state() {
        assert_eq!(
            inspect_owned_worktree(
                Path::new("/definitely/not/a/mirror"),
                Path::new("/definitely/not/a/worktree"),
                "codex/run-aw-owned",
            ),
            ("missing", false)
        );
    }
}
