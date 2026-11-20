//! A1.17: what the built-in host does with each command for a run. A start
//! prepares the run's worktree from the repository its start command
//! brought and starts its harness turn; a cancel stops it and keeps the
//! worktree; an answer moves it along; an inspection says what the host
//! knows. The host's own rows (its run detail, its workspaces and
//! worktrees) are behind [`StartHost`], so this works over whatever store
//! the host keeps.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_workspace_host_protocol::{Ack, AckOutcome, Command, HostClient};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    commands::refused,
    report::RunReports,
    requests::{Outcome, outcome_of},
};
use crate::{
    RuntimeError, git,
    git::{RegisteredRepository, expected_repository_mirror_path, managed_worktree_path},
};

const STORAGE_FAILURE_REASON: &str =
    "Run admission stopped because the host storage watermark blocks new work.";
const WORKTREE_START_FAILURE_REASON: &str =
    "Worktree creation failed; inspect the retained repository workspace.";
const PROVIDER_START_FAILURE_REASON: &str =
    "The agent harness could not start. The worktree was preserved.";
const UNKNOWN_HARNESS_REASON: &str =
    "This run's harness is not installed on the server. Nothing was started.";
const CLEANED_RETRY_REASON: &str = "The previous worktree was removed by retention cleanup before the retry could use it. Start a new Thread to continue.";
const UNSAFE_RETRY_REASON: &str = "The previous worktree is dirty, missing, or no longer has its owned branch. It was preserved; start a new Thread or inspect it before retrying.";
/// Why a run stops when the cancel names no reason.
pub const STOPPED_BY_REQUESTER: &str = "Interrupted by the requesting Actor.";

/// The host's own row for a run its scheduler claimed and has not started.
#[derive(Clone, Debug)]
pub struct RunDetail {
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub repository_id: Uuid,
    pub execution_profile_id: Uuid,
    pub adapter_kind: String,
    pub prompt: String,
    pub predecessor_run_id: Option<Uuid>,
    pub claim_id: Uuid,
}

/// A claimed run as the host starts it: its own row, and what the start
/// command brought (the repository, its space's key, the prompt).
#[derive(Clone, Debug)]
pub struct ClaimedRun {
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub repository_id: Uuid,
    pub clone_url: String,
    pub default_branch: String,
    pub project_key: String,
    pub execution_profile_id: Uuid,
    pub adapter_kind: String,
    pub prompt: String,
    pub predecessor_run_id: Option<Uuid>,
    pub claim_id: Uuid,
}

/// What a retried run's predecessor left.
#[derive(Clone, Debug)]
pub enum PredecessorWorkspace {
    /// No workspace: the retry gets a fresh one.
    None,
    /// A retained workspace with a worktree the retry may reuse.
    Retained {
        workspace_id: Uuid,
        branch: String,
        worktree_id: Uuid,
        path: PathBuf,
    },
    /// Not this run's predecessor, not finished, or its workspace is gone.
    Unsafe,
}

/// How claiming a retained workspace for a retry went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryClaim {
    Claimed,
    /// The scheduler's claim on the run was lost.
    Lost,
    /// Retention cleanup removed the workspace since it was inspected.
    Cleaned,
}

/// A fresh workspace for a run: its ids, branch, base and worktree path.
#[derive(Clone, Debug)]
pub struct NewWorkspace<'a> {
    pub workspace_id: Uuid,
    pub worktree_id: Uuid,
    pub branch: &'a str,
    pub base_commit: &'a str,
    pub path: &'a Path,
}

/// What the start, stop and answer work needs from the host it runs in:
/// its own rows for runs, workspaces and worktrees, and starting a turn.
pub trait StartHost: Send + Sync {
    type Error: From<RuntimeError> + Send;

    /// A short name for an error, for the log (never its text).
    fn category(error: &Self::Error) -> &'static str;

    /// Where the host keeps its mirrors and worktrees.
    fn data_root(&self) -> &Path;

    /// The host's state for a run (`None`: it has no row for it).
    fn detail_state(&self, run_id: Uuid) -> impl Future<Output = Option<String>> + Send;

    /// The host's row for a run claimed and not yet started.
    fn claimed_detail(
        &self,
        run_id: Uuid,
    ) -> impl Future<Output = Result<Option<RunDetail>, Self::Error>> + Send;

    /// End, as cancelled, a run this host has no row for (an assistant's).
    fn end_without_detail(
        &self,
        run_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The reports of a run this host started.
    fn open_reports(
        &self,
        run_id: Uuid,
    ) -> impl Future<Output = Result<RunReports, Self::Error>> + Send;

    /// A run whose answer lets it go on works again, or fails if it can no
    /// longer be admitted.
    fn go_on(
        &self,
        reports: &RunReports,
        run_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The run is interrupted (its turn, watching it, stops); its workspace
    /// is kept and its harness session idles.
    fn interrupted(
        &self,
        run_id: Uuid,
        reason: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Keep `command_id` as the start the run's credential comes from,
    /// unless it was already taken. Only one carry-out takes it.
    fn take_start(
        &self,
        run_id: Uuid,
        claim_id: Uuid,
        command_id: Uuid,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// Whether a start this carry-out didn't take is under way.
    fn start_going(&self, run_id: Uuid, command_id: Uuid) -> impl Future<Output = bool> + Send;

    /// Whether the disk has room for a run to grow.
    fn room_to_grow(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The branch prefix of a harness, if it is installed.
    fn branch_prefix(&self, adapter_kind: &str) -> Option<String>;

    /// What the retried run's predecessor left.
    fn predecessor_workspace(
        &self,
        run: &ClaimedRun,
        predecessor_run_id: Uuid,
    ) -> impl Future<Output = Result<PredecessorWorkspace, Self::Error>> + Send;

    /// Record what an inspection found of a worktree.
    fn worktree_inspected(
        &self,
        worktree_id: Uuid,
        state: &'static str,
        dirty: bool,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Claim a retained workspace for a retry, under the retention lock.
    fn claim_retry(
        &self,
        run: &ClaimedRun,
        workspace_id: Uuid,
        worktree_id: Uuid,
    ) -> impl Future<Output = Result<RetryClaim, Self::Error>> + Send;

    /// Record a fresh workspace for the run. `false`: the scheduler's claim
    /// on the run was lost and nothing was recorded.
    fn allocate(
        &self,
        run: &ClaimedRun,
        workspace: NewWorkspace<'_>,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// The worktree was created.
    fn worktree_ready(
        &self,
        worktree_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Start the run's harness turn in `worktree`.
    fn start_turn(
        &self,
        reports: Arc<RunReports>,
        run: &ClaimedRun,
        worktree: PathBuf,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The run failed to start: its row fails, its workspace is kept (and
    /// `failed_worktree`, if any, is marked failed), its claim is cleared.
    fn failed(
        &self,
        run: &ClaimedRun,
        reason: &'static str,
        failed_worktree: Option<Uuid>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The run started: the scheduler's claim on it is done.
    fn clear_claim(
        &self,
        run_id: Uuid,
        claim_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

fn finished(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "canceled" | "interrupted")
}

/// Whether the host knows the run.
pub async fn inspect<H: StartHost>(host: &H, command: &Command) -> Ack {
    let known = match command.run_id {
        Some(run_id) => host.detail_state(run_id).await,
        None => None,
    };
    let mut ack = Ack::new(AckOutcome::Done);
    ack.result = Some(match known {
        Some(state) => json!({"known": true, "state": state}),
        None => json!({"known": false}),
    });
    ack
}

/// An answer to what a run asked. The harness waiting on it reads it
/// itself; here the host moves its own state along, so the run goes on (or
/// stops) even with no turn waiting: an approval denied, or a request nobody
/// answered, stops it.
pub async fn answered<H: StartHost>(host: &H, command: &Command) -> Ack {
    let (Some(run_id), Some(answer)) = (command.run_id, command.detail.get("answer")) else {
        return Ack::new(AckOutcome::Accepted);
    };
    let approval = answer["kind"] == "approval";
    let view =
        json!({"kind": answer["kind"], "status": answer["status"], "answer": answer["answer"]});
    let goes_on = outcome_of(&view) != Outcome::Deny;
    if goes_on && host.detail_state(run_id).await.as_deref() != Some("blocked") {
        return Ack::new(AckOutcome::Accepted);
    }
    let reports = match host.open_reports(run_id).await {
        Ok(reports) => reports,
        Err(error) => {
            tracing::warn!(%run_id, error_category = H::category(&error), "a run's answer wasn't heard");
            return Ack::new(AckOutcome::Uncertain);
        }
    };
    let moved = if goes_on {
        host.go_on(&reports, run_id).await
    } else {
        stop_unanswered(host, &reports, run_id, approval).await
    };
    if let Err(error) = moved {
        tracing::warn!(%run_id, error_category = H::category(&error), "a run's answer wasn't carried out");
        return Ack::new(AckOutcome::Uncertain);
    }
    Ack::new(AckOutcome::Accepted)
}

/// A cancel: the turn stops (it watches the run's row), the worktree is
/// kept, and the run's record hears that it stopped.
pub async fn cancelled<H: StartHost>(host: &H, command: &Command) -> Ack {
    let Some(run_id) = command.run_id else {
        return Ack::new(AckOutcome::Done);
    };
    let Some(current) = host.detail_state(run_id).await else {
        // An assistant run has no row: its record ends here, which retires
        // its credential, so its next model or tool call is refused and its
        // turn stops there.
        return match host.end_without_detail(run_id).await {
            Ok(()) => Ack::new(AckOutcome::Done),
            Err(error) => {
                tracing::warn!(%run_id, error_category = H::category(&error), "a run didn't stop");
                Ack::new(AckOutcome::Uncertain)
            }
        };
    };
    if finished(&current) {
        return Ack::new(AckOutcome::Done);
    }
    let reason = command
        .detail
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or(STOPPED_BY_REQUESTER)
        .to_owned();
    let stopped = match host.open_reports(run_id).await {
        Ok(reports) => stop(host, &reports, run_id, &reason, "The run was interrupted.").await,
        Err(error) => Err(error),
    };
    match stopped {
        Ok(()) => Ack::new(AckOutcome::Done),
        Err(error) => {
            tracing::warn!(%run_id, error_category = H::category(&error), "a run didn't stop");
            Ack::new(AckOutcome::Uncertain)
        }
    }
}

/// Stop a run: its record hears it was interrupted, its row is interrupted
/// and the host keeps its worktree. Safe to call twice.
async fn stop<H: StartHost>(
    host: &H,
    reports: &RunReports,
    run_id: Uuid,
    reason: &str,
    summary: &str,
) -> Result<(), H::Error> {
    if host
        .detail_state(run_id)
        .await
        .is_none_or(|current| finished(&current))
    {
        return Ok(());
    }
    reports
        .step(
            &format!("run_interrupted:{run_id}"),
            "run_interrupted",
            summary,
            json!({}),
            true,
        )
        .await?;
    host.interrupted(run_id, reason).await
}

/// Stop a run whose approval was denied, or whose request went unanswered.
pub async fn stop_unanswered<H: StartHost>(
    host: &H,
    reports: &RunReports,
    run_id: Uuid,
    approval: bool,
) -> Result<(), H::Error> {
    let (reason, summary) = if approval {
        (
            "The requested approval was denied.",
            "The run was interrupted after the approval was denied.",
        )
    } else {
        (
            "The question went unanswered.",
            "The run was interrupted after its question went unanswered.",
        )
    };
    stop(host, reports, run_id, reason, summary).await
}

/// A start: the run the scheduler claimed gets its worktree and its harness
/// turn. A start offered again for a run already going is accepted again;
/// one for a run no longer waiting to start is refused.
pub async fn start<H: StartHost>(host: &H, client: &HostClient, command: &Command) -> Ack {
    let Some(run_id) = command.run_id else {
        return refused("invalid", "A start needs its run.");
    };
    let Ok(claimed) = claimed(host, run_id, command).await else {
        return Ack::new(AckOutcome::Uncertain);
    };
    // The run's reports carry the credential its start brought; the start
    // is kept so a later turn (or a restart) can fetch a fresh one. Two
    // passes can carry out the same start at once (a kick's and the tick's):
    // only the one that takes it goes on, so the other never fetches a
    // credential that would replace the first one's.
    let claimed = match claimed {
        Some(run) => match host
            .take_start(run_id, run.claim_id, command.command_id)
            .await
        {
            Ok(true) => Some(run),
            Ok(false) => None,
            Err(_) => return Ack::new(AckOutcome::Uncertain),
        },
        None => None,
    };
    let Some(run) = claimed else {
        return if host.start_going(run_id, command.command_id).await {
            Ack::new(AckOutcome::Accepted)
        } else {
            refused("not_waiting", "That run isn't waiting to start.")
        };
    };
    let Ok(reports) = RunReports::for_start(client.clone(), run_id, command.command_id).await
    else {
        return Ack::new(AckOutcome::Uncertain);
    };
    match admit(host, &Arc::new(reports), run).await {
        Ok(()) => Ack::new(AckOutcome::Accepted),
        Err(error) => {
            tracing::error!(
                %run_id,
                error_category = H::category(&error),
                "Agent Run admission failed"
            );
            refused("start_failed", "The run couldn't start; its log says why.")
        }
    }
}

/// The run a start is for, as the scheduler claimed it: the host's own row,
/// with the repository and prompt its start brought. A start naming another
/// repository than the run's is no start for it.
async fn claimed<H: StartHost>(
    host: &H,
    run_id: Uuid,
    command: &Command,
) -> Result<Option<ClaimedRun>, H::Error> {
    let Some(detail) = host.claimed_detail(run_id).await? else {
        return Ok(None);
    };
    Ok(from_start(detail, &command.detail))
}

/// The run as its start command describes it, over the host's own row.
fn from_start(detail: RunDetail, start: &Map<String, Value>) -> Option<ClaimedRun> {
    let repository = start.get("repository")?;
    let text = |key: &str| repository[key].as_str().map(str::to_owned);
    let id = text("id").and_then(|id| id.parse::<Uuid>().ok())?;
    if id != detail.repository_id {
        return None;
    }
    let prompt = start
        .get("input")
        .and_then(|input| input["prompt"].as_str())
        .map_or(detail.prompt, str::to_owned);
    Some(ClaimedRun {
        run_id: detail.run_id,
        project_id: detail.project_id,
        conversation_id: detail.conversation_id,
        repository_id: id,
        clone_url: text("cloneUrl")?,
        default_branch: text("defaultBranch")?,
        project_key: text("spaceKey")?,
        execution_profile_id: detail.execution_profile_id,
        adapter_kind: detail.adapter_kind,
        prompt,
        predecessor_run_id: detail.predecessor_run_id,
        claim_id: detail.claim_id,
    })
}

async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| RuntimeError::Internal(what))?
}

async fn admit<H: StartHost>(
    host: &H,
    reports: &Arc<RunReports>,
    run: ClaimedRun,
) -> Result<(), H::Error> {
    if let Err(error) = host.room_to_grow().await {
        fail_admission(host, reports, &run, STORAGE_FAILURE_REASON, None).await?;
        return Err(error);
    }
    let data_root = host.data_root().to_path_buf();
    let mirror = expected_repository_mirror_path(&data_root, run.repository_id)?;
    let repository = RegisteredRepository {
        id: run.repository_id,
        clone_url: run.clone_url.clone(),
        default_branch: run.default_branch.clone(),
        mirror_path: mirror.clone(),
    };
    let root = data_root.clone();
    let prepared = match blocking("join repository preparation", move || {
        git::inspect_prepared_repository(&root, &repository)
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            fail_admission(host, reports, &run, PROVIDER_START_FAILURE_REASON, None).await?;
            return Err(error.into());
        }
    };

    if let Some(predecessor_run_id) = run.predecessor_run_id {
        match reusable_retry_workspace(host, &run, predecessor_run_id, &mirror).await? {
            Retry::Reuse(workspace_id, worktree_id, worktree) => {
                match host.claim_retry(&run, workspace_id, worktree_id).await? {
                    RetryClaim::Claimed => {}
                    RetryClaim::Lost => {
                        return Err(RuntimeError::Conflict(
                            "agent_run",
                            "scheduler claim was lost",
                        )
                        .into());
                    }
                    RetryClaim::Cleaned => {
                        fail_admission(host, reports, &run, CLEANED_RETRY_REASON, None).await?;
                        return Err(RuntimeError::Conflict(
                            "git_worktree",
                            "previous worktree was cleaned up before the retry claimed it",
                        )
                        .into());
                    }
                }
                start_admitted_run(host, reports, &run, worktree).await?;
                return host.clear_claim(run.run_id, run.claim_id).await;
            }
            Retry::Fresh => {}
            Retry::Unsafe => {
                fail_admission(host, reports, &run, UNSAFE_RETRY_REASON, None).await?;
                return Err(RuntimeError::Conflict(
                    "git_worktree",
                    "previous worktree is not safe to reuse",
                )
                .into());
            }
        }
    }

    let workspace_id = Uuid::now_v7();
    let worktree_id = Uuid::now_v7();
    let Some(branch_prefix) = host.branch_prefix(&run.adapter_kind) else {
        fail_admission(host, reports, &run, UNKNOWN_HARNESS_REASON, None).await?;
        return Err(RuntimeError::Conflict("agent_run", "harness is not installed").into());
    };
    let branch = git::unique_run_branch(&branch_prefix, &run.project_key)?;
    let worktree = managed_worktree_path(&data_root, run.repository_id, workspace_id)?;
    let recorded = host
        .allocate(
            &run,
            NewWorkspace {
                workspace_id,
                worktree_id,
                branch: &branch,
                base_commit: &prepared.base_commit,
                path: &worktree,
            },
        )
        .await?;
    if !recorded {
        return Err(RuntimeError::Conflict("agent_run", "scheduler claim was lost").into());
    }

    let (mirror_for_add, worktree_for_add) = (mirror.clone(), worktree.clone());
    let base_commit = prepared.base_commit.clone();
    let created = blocking("join worktree creation", move || {
        git::create_worktree(&mirror_for_add, &worktree_for_add, &branch, &base_commit)
    })
    .await;
    if created.is_err() {
        fail_admission(
            host,
            reports,
            &run,
            WORKTREE_START_FAILURE_REASON,
            Some(worktree_id),
        )
        .await?;
        return Err(RuntimeError::Conflict("git_worktree", "worktree creation failed").into());
    }
    host.worktree_ready(worktree_id).await?;
    start_admitted_run(host, reports, &run, worktree).await?;
    host.clear_claim(run.run_id, run.claim_id).await
}

async fn start_admitted_run<H: StartHost>(
    host: &H,
    reports: &Arc<RunReports>,
    run: &ClaimedRun,
    worktree: PathBuf,
) -> Result<(), H::Error> {
    if let Err(error) = host.start_turn(Arc::clone(reports), run, worktree).await {
        fail_admission(host, reports, run, PROVIDER_START_FAILURE_REASON, None).await?;
        return Err(error);
    }
    Ok(())
}

enum Retry {
    Fresh,
    Reuse(Uuid, Uuid, PathBuf),
    Unsafe,
}

/// Whether a retry can reuse its predecessor's worktree: retained, clean,
/// and still on the branch it owns. One that isn't is marked so.
async fn reusable_retry_workspace<H: StartHost>(
    host: &H,
    run: &ClaimedRun,
    predecessor_run_id: Uuid,
    mirror: &Path,
) -> Result<Retry, H::Error> {
    let (workspace_id, branch, worktree_id, worktree) =
        match host.predecessor_workspace(run, predecessor_run_id).await? {
            PredecessorWorkspace::None => return Ok(Retry::Fresh),
            PredecessorWorkspace::Unsafe => return Ok(Retry::Unsafe),
            PredecessorWorkspace::Retained {
                workspace_id,
                branch,
                worktree_id,
                path,
            } => (workspace_id, branch, worktree_id, path),
        };
    let (mirror, inspection_path) = (mirror.to_path_buf(), worktree.clone());
    let inspection =
        tokio::task::spawn_blocking(move || git::inspect_worktree(&mirror, &inspection_path))
            .await
            .map_err(|_| RuntimeError::Internal("join retry worktree inspection"))?;
    let Ok(inspection) = inspection else {
        host.worktree_inspected(worktree_id, "missing", false)
            .await?;
        return Ok(Retry::Unsafe);
    };
    if inspection.dirty || inspection.branch != branch {
        let state = if inspection.dirty { "dirty" } else { "failed" };
        host.worktree_inspected(worktree_id, state, inspection.dirty)
            .await?;
        return Ok(Retry::Unsafe);
    }
    Ok(Retry::Reuse(workspace_id, worktree_id, worktree))
}

/// A run that couldn't start: its record hears why (through the protocol,
/// like any host's), and the host's rows fail. Nothing if it already ended.
async fn fail_admission<H: StartHost>(
    host: &H,
    reports: &RunReports,
    run: &ClaimedRun,
    reason: &'static str,
    failed_worktree: Option<Uuid>,
) -> Result<(), H::Error> {
    if host
        .detail_state(run.run_id)
        .await
        .is_some_and(|current| finished(&current))
    {
        return Ok(());
    }
    reports
        .note(
            &format!("run_failed:{}", run.run_id),
            "run_failed",
            reason,
            true,
        )
        .await?;
    reports.moved("failed", Some(reason)).await?;
    host.failed(run, reason, failed_worktree).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(repository_id: Uuid) -> RunDetail {
        RunDetail {
            run_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            conversation_id: Uuid::now_v7(),
            repository_id,
            execution_profile_id: Uuid::now_v7(),
            adapter_kind: "demo".to_owned(),
            prompt: "stored".to_owned(),
            predecessor_run_id: None,
            claim_id: Uuid::now_v7(),
        }
    }

    #[test]
    fn a_start_brings_the_repository_and_prompt() {
        let repository_id = Uuid::now_v7();
        let start = json!({
            "repository": {"id": repository_id, "cloneUrl": "https://example.test/r.git",
                           "defaultBranch": "main", "spaceKey": "KEY"},
            "input": {"prompt": "brought"},
        });
        let run = from_start(detail(repository_id), start.as_object().expect("an object"))
            .expect("a run");
        assert_eq!(run.clone_url, "https://example.test/r.git");
        assert_eq!(run.project_key, "KEY");
        assert_eq!(run.prompt, "brought");
    }

    #[test]
    fn a_start_for_another_repository_or_without_one_is_no_start() {
        let start = json!({"repository": {"id": Uuid::now_v7(), "cloneUrl": "u",
                                          "defaultBranch": "main", "spaceKey": "K"}});
        let start = start.as_object().expect("an object");
        assert!(from_start(detail(Uuid::now_v7()), start).is_none());
        assert!(from_start(detail(Uuid::now_v7()), &Map::new()).is_none());
    }
}
