//! Claims and handoffs as operations (K2.14): `task_claim` and
//! `task_release`, over `work_item_commands::claims`, inside the
//! dispatcher's transaction. A person claims and releases from the web
//! through the same operations.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
    mcp::task_commands,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted,
        error::{ConflictReason, DenyReason},
        refs::IdRef,
    },
    work_item_commands::claims::{
        self, ClaimRow, Claimed, DEFAULT_CLAIM_MINUTES, HandoffRow, MAX_HANDOFF_CHARS, Released,
        Releaser,
    },
};

/// Who is working on a task right now.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimValue {
    pub task_id: String,
    pub claimed_by: String,
    pub note: Option<String>,
    pub claimed_at: String,
    pub expires_at: String,
    /// On a claim you just took: the last handoff note left on the task.
    /// Read it before you start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffNote>,
}

impl From<ClaimRow> for ClaimValue {
    fn from(row: ClaimRow) -> Self {
        Self {
            task_id: row.work_item_id.to_string(),
            claimed_by: row.claimed_by,
            note: row.note,
            claimed_at: row.claimed_at,
            expires_at: row.expires_at,
            handoff: None,
        }
    }
}

/// A note left when someone stopped working on a task.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandoffNote {
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub note: String,
    pub at: String,
}

impl From<HandoffRow> for HandoffNote {
    fn from(row: HandoffRow) -> Self {
        Self {
            by: row.by,
            via: row.via,
            note: row.note,
            at: row.at,
        }
    }
}

/// What a release did.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseValue {
    pub task_id: String,
    pub released: bool,
    /// Whether your handoff note was saved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handoff_saved: bool,
}

/// A claim command's error: a task gone in the meantime is not found.
fn task_error(op: &'static str, error: crate::error::AppError) -> OpError {
    match error {
        crate::error::AppError::NotFound(_) => OpError::not_found("That task"),
        error => OpError::logged(op, &error),
    }
}

fn space_of(call: &OpCall<'_>) -> Result<uuid::Uuid, OpError> {
    call.ctx.space_id.ok_or(OpError::not_found("That task"))
}

// ---------------------------------------------------------------------------
// task_claim

pub struct TaskClaim;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskClaimInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// What you're doing, shown to others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How long the claim lasts, in minutes (default 120, at most 1440).
    /// Claiming again renews it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u16>,
}

impl Targeted for TaskClaimInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskClaim {
    const NAME: &'static str = "task_claim";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Claim task";
    const DESCRIPTION: &'static str = "Claim a task before working on it, so other agents and \
        people see you're on it. Fails if someone else holds it. Claims last 2 hours by \
        default; claim again to renew. Returns the last handoff note left on the task, if any: \
        read it first.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskClaimInput;
    type Output = ClaimValue;

    async fn run(call: &mut OpCall<'_>, input: TaskClaimInput) -> Result<ClaimValue, OpError> {
        let minutes = input.minutes.map_or(DEFAULT_CLAIM_MINUTES, i32::from);
        if !(5..=1440).contains(&minutes) {
            return Err(OpError::invalid("minutes", "Claim for 5 to 1440 minutes."));
        }
        let note = input
            .note
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(500).collect::<String>());
        let task_id = input.task_id.id();
        let space = space_of(call)?;
        let actor = call.ctx.caller.account_id;
        let via = call.ctx.caller.via_grant_id;
        let tx = call.tx().await?;
        crate::domain::set_via_grant(tx, via)
            .await
            .map_err(|error| OpError::logged("task_claim", &error))?;
        let claimed = claims::claim(tx, space, task_id, actor, note.as_deref(), minutes)
            .await
            .map_err(|error| task_error("task_claim", error))?;
        match claimed {
            Claimed::Held { .. } => {}
            Claimed::Archived => {
                return Err(OpError::Conflict {
                    reason: ConflictReason::Archived,
                    message: "That task is archived.".into(),
                    current: None,
                });
            }
            Claimed::HeldByOther(holder) => {
                return Err(OpError::Conflict {
                    reason: ConflictReason::State { code: "claimed" },
                    message: match &holder {
                        Some(holder) => format!(
                            "{} is working on this until {}{}. Pick another task, or ask them.",
                            holder.claimed_by,
                            holder.expires_at,
                            holder
                                .note
                                .as_deref()
                                .map(|note| format!(" ({note})"))
                                .unwrap_or_default()
                        )
                        .into(),
                        None => "Someone else just claimed this task. Try again.".into(),
                    },
                    current: holder.map(|holder| {
                        serde_json::json!({
                            "claimedBy": holder.claimed_by,
                            "expiresAt": holder.expires_at,
                            "note": holder.note,
                        })
                    }),
                });
            }
        }
        let mut claim = ClaimValue::from(
            claims::current(tx, task_id)
                .await
                .map_err(|error| OpError::logged("task_claim", &error))?
                .ok_or(OpError::Unavailable)?,
        );
        claim.handoff = claims::handoffs(tx, task_id, 1)
            .await
            .map_err(|error| OpError::logged("task_claim", &error))?
            .into_iter()
            .next()
            .map(HandoffNote::from);
        call.audit_target("task", task_id, serde_json::json!({ "claim": true }));
        Ok(claim)
    }
}

// ---------------------------------------------------------------------------
// task_release

pub struct TaskRelease;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskReleaseInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// For whoever picks the task up next: where the work got to, what's
    /// left, and anything that would trip them up (a branch, a failing test,
    /// a question you're waiting on). Up to 4,000 characters. The next
    /// task_claim returns it, and task_get shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// End someone else's claim (they may still be partway through). Needs
    /// "Manage project" in the task's space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anyone: Option<bool>,
}

impl Targeted for TaskReleaseInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskRelease {
    const NAME: &'static str = "task_release";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Release task";
    const DESCRIPTION: &'static str = "Release your claim on a task when you stop working on it. \
        If it isn't done, leave a handoff note: where you got to and what's left. The next \
        task_claim returns it.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskReleaseInput;
    type Output = ReleaseValue;

    async fn run(call: &mut OpCall<'_>, input: TaskReleaseInput) -> Result<ReleaseValue, OpError> {
        let handoff = input
            .handoff
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if handoff
            .as_deref()
            .is_some_and(|note| note.chars().count() > MAX_HANDOFF_CHARS || note.contains('\0'))
        {
            return Err(OpError::invalid(
                "handoff",
                "Keep the handoff note to 4,000 characters.",
            ));
        }
        let task_id = input.task_id.id();
        let space = space_of(call)?;
        let releaser = if input.anyone == Some(true) {
            let ctx = call.ctx;
            let tx = call.tx().await?;
            let may = crate::access::check_locked(
                tx,
                &ctx.caller,
                crate::access::Need::maybe(Some(Perm::ProjectsManage)).writing(true),
                crate::access::Target::Space(space),
            )
            .await
            .map_err(|error| OpError::logged("task_release", &error))?;
            if may.is_err() {
                return Err(OpError::Denied {
                    reason: DenyReason::Role {
                        permission: Perm::ProjectsManage,
                    },
                    message: "Ending someone else's claim needs \"Manage project\" here.".into(),
                });
            }
            Releaser::Anyone
        } else {
            Releaser::Holder
        };
        let actor = call.ctx.caller.account_id;
        let via = call.ctx.caller.via_grant_id;
        let tx = call.tx().await?;
        let released =
            claims::release(tx, space, task_id, actor, via, handoff.as_deref(), releaser)
                .await
                .map_err(|error| task_error("task_release", error))?;
        let released = match released {
            Released::Ended { .. } => true,
            Released::Nothing => false,
            Released::NotHolder => {
                return Err(OpError::Conflict {
                    reason: ConflictReason::State { code: "not_holder" },
                    message: "You don't hold this task's claim, so there's nothing to hand off. \
                              Comment on the task instead."
                        .into(),
                    current: None,
                });
            }
        };
        call.audit_target("task", task_id, serde_json::json!({ "claim": false }));
        Ok(ReleaseValue {
            task_id: task_id.to_string(),
            released,
            handoff_saved: handoff.is_some() && released,
        })
    }
}

// ---------------------------------------------------------------------------
// task_claim_next (AWD-220)

/// How often a waiting `task_claim_next` looks at the queue again.
const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(3);
/// The longest one call waits; a worker that wants longer calls again.
const MAX_WAIT_SECONDS: u8 = 30;

pub struct TaskClaimNext;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskClaimNextInput {
    /// The space whose tasks to take from: its ID or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Only tasks that carry every one of these labels, by name (at most 10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// true takes only tasks that also pass the checks for unattended work
    /// (acceptance criteria, a description, somewhere to run), as
    /// task_ready_list with scope agents lists them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Seconds to wait for a task when none is ready (at most 30). When the
    /// wait ends with nothing, call again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_seconds: Option<u8>,
    /// What you're doing, shown to others on the task you get.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How long the claim lasts, in minutes (default 120, at most 1440).
    /// task_claim on the task renews it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u16>,
}

impl Targeted for TaskClaimNextInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

/// The task a `task_claim_next` took.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NextTask {
    pub task_id: String,
    /// Its key, such as AWD-12.
    pub key: String,
    pub title: String,
    pub priority: String,
    /// Why it was the next one, in words.
    pub why: Vec<String>,
    pub url: String,
}

/// What `task_claim_next` did.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimNextValue {
    /// false: nothing was ready for you; nothing changed.
    pub claimed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<NextTask>,
    /// Your claim on it, with the last handoff note if one was left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimValue>,
    /// What to do next.
    pub message: String,
}

impl Operation for TaskClaimNext {
    const NAME: &'static str = "task_claim_next";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Claim the next ready task";
    const DESCRIPTION: &'static str = "Take the best task that is ready in a space and claim it \
        in one step, so several workers can share a queue without picking the same one. It \
        takes tasks that can start now (blockers done, not marked blocked), that nobody holds \
        and that aren't assigned or handed to someone else; labels narrows it. waitSeconds \
        waits for one to become ready. The task moves to the space's in-progress status. Open \
        it with task_get, do the work, then task_finish (or task_release with a handoff if you \
        stop early).";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskClaimNextInput;
    type Output = ClaimNextValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskClaimNextInput,
    ) -> Result<ClaimNextValue, OpError> {
        let minutes = input.minutes.map_or(DEFAULT_CLAIM_MINUTES, i32::from);
        if !(5..=1440).contains(&minutes) {
            return Err(OpError::invalid("minutes", "Claim for 5 to 1440 minutes."));
        }
        let labels = input
            .labels
            .iter()
            .map(|label| label.trim().to_owned())
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>();
        if labels.len() > 10 {
            return Err(OpError::invalid("labels", "Name at most 10 labels."));
        }
        let note = input
            .note
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(500).collect::<String>());
        let strict = input.strict == Some(true);
        let space = space_of(call)?;
        let actor = call.ctx.caller.account_id;
        let via = call.ctx.caller.via_grant_id;
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(u64::from(
                input.wait_seconds.unwrap_or(0).min(MAX_WAIT_SECONDS),
            ));
        // Tasks another worker took between the look and the claim.
        let mut lost = Vec::new();
        loop {
            let ranked =
                task_commands::ready_candidates(call.state, call.ctx, space, strict).await?;
            let candidates = ranked
                .iter()
                .filter(|task| !task.claimed && !lost.contains(&task.id))
                .map(|task| task.id)
                .collect::<Vec<_>>();
            let open = claims::takeable(&call.state.pool, space, actor, &candidates, &labels)
                .await
                .map_err(|error| OpError::logged("task_claim_next", &error))?;
            if let Some(task) = ranked.iter().find(|task| open.contains(&task.id)) {
                // A short transaction of its own, committed before anything
                // else: the claim is what the other workers race on, and
                // the status change after it runs as its own command.
                let logged = |error: sqlx::Error| {
                    OpError::logged("task_claim_next", &crate::error::AppError::from(error))
                };
                let mut tx = crate::db::scoped_tx(&call.state.pool)
                    .await
                    .map_err(logged)?;
                crate::domain::set_via_grant(&mut tx, via)
                    .await
                    .map_err(|error| OpError::logged("task_claim_next", &error))?;
                let claimed =
                    claims::claim(&mut tx, space, task.id, actor, note.as_deref(), minutes)
                        .await
                        .map_err(|error| task_error("task_claim_next", error))?;
                if !matches!(claimed, Claimed::Held { .. }) {
                    lost.push(task.id);
                    continue;
                }
                let mut claim = ClaimValue::from(
                    claims::current(&mut tx, task.id)
                        .await
                        .map_err(|error| OpError::logged("task_claim_next", &error))?
                        .ok_or(OpError::Unavailable)?,
                );
                claim.handoff = claims::handoffs(&mut tx, task.id, 1)
                    .await
                    .map_err(|error| OpError::logged("task_claim_next", &error))?
                    .into_iter()
                    .next()
                    .map(HandoffNote::from);
                tx.commit().await.map_err(logged)?;
                // So the board shows it being worked on. A workflow with no
                // way there, or a role that can't move it, leaves the task
                // where it is: the claim already says who has it.
                let started = task_commands::walk_to(
                    call.state,
                    call.ctx,
                    space,
                    task.id,
                    "started",
                    Some(&["backlog", "unstarted"]),
                )
                .await
                .is_ok_and(|walked| walked.arrived);
                call.audit_target("task", task.id, serde_json::json!({ "claim": true }));
                return Ok(ClaimNextValue {
                    claimed: true,
                    message: format!(
                        "You hold {}{}. Open it with task_get, and end with task_finish.",
                        task.key,
                        if started {
                            ", and it is in progress"
                        } else {
                            ""
                        }
                    ),
                    task: Some(NextTask {
                        task_id: task.id.to_string(),
                        key: task.key.clone(),
                        title: task.title.clone(),
                        priority: task.priority.clone(),
                        why: task.reasons.clone(),
                        url: task.href.clone(),
                    }),
                    claim: Some(claim),
                });
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(ClaimNextValue {
                    claimed: false,
                    task: None,
                    claim: None,
                    message: "Nothing is ready for you here right now. Call again, with \
                              waitSeconds to wait for a task."
                        .to_owned(),
                });
            }
            tokio::time::sleep(LOOK_EVERY.min(deadline - now)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// task_finish (AWD-220)

pub struct TaskFinish;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskFinishInput {
    /// The task: its ID, link, or key such as AWD-12.
    pub task_id: IdRef,
    /// What came of it, for whoever asked and whoever reads the task later:
    /// what you did, where it is (a branch, a pull request, a page), and
    /// anything left. For outcome failed: what stopped you and what would
    /// unblock it. Markdown, up to 8,000 characters. It is posted on the
    /// task, so its watchers are told.
    pub result: String,
    /// done (the default): the task moves to its finished status. review:
    /// it moves to review for someone to check. failed: it stays open and
    /// is marked blocked with your result as the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl Targeted for TaskFinishInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

/// What `task_finish` did.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskFinished {
    pub task_id: String,
    /// done, review or failed.
    pub outcome: String,
    /// The comment that holds your result.
    pub result_comment_id: String,
    /// The task's version now.
    pub version: i64,
    /// Whether a claim of yours was released.
    pub released: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Operation for TaskFinish {
    const NAME: &'static str = "task_finish";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Finish task";
    const DESCRIPTION: &'static str = "End your work on a task with its result, in one step: \
        the result is posted on the task (its watchers and whoever asked are told), the task \
        moves on (outcome done, review or failed) through whatever statuses lie between, and \
        your claim is released. Refused while someone else holds the task. To stop without a \
        result, use task_release with a handoff.";
    const EMITS: &'static [&'static str] = &["task.commented", "task.updated"];

    type Input = TaskFinishInput;
    type Output = TaskFinished;

    async fn run(call: &mut OpCall<'_>, input: TaskFinishInput) -> Result<TaskFinished, OpError> {
        let result = input.result.trim().to_owned();
        if result.is_empty() || result.chars().count() > 8_000 || result.contains('\0') {
            return Err(OpError::invalid(
                "result",
                "Say what came of it, in 1 to 8,000 characters.",
            ));
        }
        let outcome = input.outcome.as_deref().unwrap_or("done");
        if !matches!(outcome, "done" | "review" | "failed") {
            return Err(OpError::invalid("outcome", "Pass done, review or failed."));
        }
        let task_id = input.task_id.id();
        let space = space_of(call)?;
        let actor = call.ctx.caller.account_id;
        let via = call.ctx.caller.via_grant_id;
        let pool = &call.state.pool;
        let holder = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT actor_id FROM work_item_claims WHERE work_item_id = $1 AND expires_at > now()",
        )
        .bind(task_id)
        .fetch_optional(crate::db::scoped(pool))
        .await
        .map_err(crate::error::AppError::from)?;
        if holder.is_some_and(|holder| holder != actor) {
            return Err(OpError::Conflict {
                reason: ConflictReason::State { code: "claimed" },
                message: "Someone else is working on this task, so it isn't yours to finish. \
                          Comment on it instead."
                    .into(),
                current: None,
            });
        }
        // The result first: if the move is refused, the words are kept and a
        // retry with the same request id posts them once.
        let posted = task_commands::comment(
            call.state,
            call.ctx,
            task_id,
            task_commands::CommentText {
                text: Some(result.clone()),
                content: None,
            },
            None,
            None,
            None,
        )
        .await?;
        let (version, url) = if outcome == "failed" {
            let version = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM task_rows WHERE id = $1 AND project_id = $2",
            )
            .bind(task_id)
            .bind(space)
            .fetch_optional(crate::db::scoped(pool))
            .await
            .map_err(crate::error::AppError::from)?
            .ok_or(OpError::not_found("That task"))?;
            let reason = result.lines().next().unwrap_or_default();
            let reason = reason.chars().take(280).collect::<String>();
            let blocked =
                task_commands::mark_blocked(call.state, call.ctx, task_id, version, reason).await?;
            (blocked.version, blocked.url)
        } else {
            let group = if outcome == "review" {
                "review"
            } else {
                "completed"
            };
            let walked =
                task_commands::walk_to(call.state, call.ctx, space, task_id, group, None).await?;
            if !walked.arrived {
                return Err(OpError::Conflict {
                    reason: ConflictReason::State { code: "workflow" },
                    message: match walked.status {
                        Some(status) => format!(
                            "Your result is posted, but this space's workflow has no way from \
                             the task's status to {status}. Move it with task_update, then \
                             task_release."
                        )
                        .into(),
                        None => format!(
                            "Your result is posted, but this space has no {} status. Move the \
                             task with task_update, then task_release.",
                            if outcome == "review" {
                                "review"
                            } else {
                                "finished"
                            }
                        )
                        .into(),
                    },
                    current: None,
                });
            }
            (walked.version, None)
        };
        let tx = call.tx().await?;
        let released = matches!(
            claims::release(tx, space, task_id, actor, via, None, Releaser::Holder)
                .await
                .map_err(|error| task_error("task_finish", error))?,
            Released::Ended { .. }
        );
        call.audit_target("task", task_id, serde_json::json!({ "finished": outcome }));
        Ok(TaskFinished {
            task_id: task_id.to_string(),
            outcome: outcome.to_owned(),
            result_comment_id: posted.result_id,
            version,
            released,
            url: url.or(posted.url),
        })
    }
}
