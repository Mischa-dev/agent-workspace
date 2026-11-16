//! Claims and handoffs as operations (K2.14): `task_claim` and
//! `task_release`, over `work_item_commands::claims`, inside the
//! dispatcher's transaction. A person claims and releases from the web
//! through the same operations.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
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
            return Err(OpError::invalid(
                "minutes",
                "Claim for 5 to 1440 minutes.",
            ));
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
        let released = claims::release(
            tx,
            space,
            task_id,
            actor,
            via,
            handoff.as_deref(),
            releaser,
        )
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
