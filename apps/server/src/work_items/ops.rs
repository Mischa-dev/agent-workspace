//! Tasks' core writes as operations (K2.13): create, update (with blocked
//! and ready for review, which `task_mark_blocked` and
//! `task_mark_ready_for_review` now alias), archive, restore, comments and
//! acceptance criteria. Agents call them as tools of the same names
//! (`task_comment`, `task_comment_edit`, `task_criteria` and the two mark
//! tools stay as aliases), scripts through the v2 route, and rules as
//! actions. Each runs the command the hand-written tool ran
//! (`mcp::task_commands`, over `work_item_commands`), which keeps its own
//! idempotency scope, its own checks and its own audit row.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    mcp::{
        string_or_number,
        task_commands::{
            self, CriteriaValue, CriterionOperationArgument, DeletedComment, EditedComment,
            TaskWritten,
        },
    },
    message_content::MessageContent,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub mod claims;
pub mod planning;
pub mod reads;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<TaskCreate>()
        .add::<TaskUpdate>()
        .add::<TaskArchive>()
        .add::<TaskRestore>()
        .add::<TaskCommentCreate>()
        .add::<TaskCommentUpdate>()
        .add::<TaskCommentDelete>()
        .add::<TaskCriteriaUpdate>()
        .add::<TaskTriage>()
        .add::<TriageEnable>()
        .add::<TriageRuleList>()
        .add::<TriageRuleWrite>()
        .add::<CycleList>()
        .add::<CycleSet>()
        .add::<InitiativeList>()
        .add::<InitiativeCreate>()
        .add::<MilestoneReorder>()
        .add::<ProjectStatusGet>()
        .add::<ProjectStatusSet>()
        .add::<ProjectUpdatePost>()
        .add::<claims::TaskClaim>()
        .add::<claims::TaskRelease>();
    planning::register_ops(registry);
    reads::register_ops(registry);
}

/// The bookkeeping every task write shares: a keyed call is retry-safe
/// (the command scopes it), a replay says so, and the command wrote the
/// audit row.
fn settled(call: &mut OpCall<'_>, written: &TaskWritten) {
    if call.ctx.request_id.is_some() {
        call.key_honoured();
    }
    call.mark_replayed(written.idempotent_replay);
    if let Some(id) = written.task_id() {
        call.audit_target("task", id, json!({}));
    }
    call.audited_by_command();
}

// ---------------------------------------------------------------------------
// task_create

pub struct TaskCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskCreateInput {
    /// The space (project) to create it in: its id, key or link. Not needed
    /// for a subtask, or for a connection held to one space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<IdRef>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// "me", an actor ID, or an @handle.
    #[serde(
        default,
        alias = "assigneeActorId",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee: Option<String>,
    /// Make it a subtask of this task (ID, link, or key such as AWD-3). It
    /// goes in its parent's space.
    #[serde(
        default,
        alias = "parentWorkItemId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<IdRef>,
    /// Its status by name, such as "Ready" (see workflow_get). Left out, it
    /// starts in the space's first status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Its type by name, such as "Bug" (see workflow_get).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// Labels by name or ID, at most 20. They must exist already; create one
    /// with label_write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// What done looks like, one criterion per entry, in order (at most 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_criteria: Option<Vec<String>>,
    /// Where it goes in its status column: top, bottom (the default), or a
    /// number, where 1 is the top.
    #[serde(
        default,
        deserialize_with = "string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub position: Option<String>,
    /// Put it just above this task (ID, link, or key) in its column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_task_id: Option<String>,
    /// Put it just below this task (ID, link, or key) in its column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_task_id: Option<String>,
    /// An active milestone of the space, by name or ID (see
    /// milestone_list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// When work starts: YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// When it's due: YYYY-MM-DD, on or after startDate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// How big it is, in the space's unit (points or hours; see
    /// workflow_get), from 0 to 100000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<f64>,
    /// Custom field values by field name (see field_list), such as
    /// {"Risk": "High", "Owner": "@ana", "Shipped": true}. In a collection
    /// that acts like a task, its own fields (see collection_get).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
    /// Make it in a collection that acts like a task, such as "Bugs", by
    /// name or ID (project_open lists them). Left out, it's a plain task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Made from this message, one in the same space you can read: the task
    /// links back to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_message_id: Option<IdRef>,
    /// With fromMessageId, also say so in its conversation ("… made a
    /// message here into a task"). Off unless asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub say_so: Option<bool>,
}

impl Targeted for TaskCreateInput {
    fn target(&self) -> Target {
        match (self.parent_id, self.project_id) {
            (Some(parent), _) => Target::thing("task", parent.id()),
            (None, Some(space)) => Target::Space(space.id()),
            // A connection held to one space makes it there (the
            // dispatcher locates it in that space); anyone else is asked
            // which space.
            (None, None) => Target::Workspace(None),
        }
    }
}

impl Operation for TaskCreate {
    const NAME: &'static str = "task_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const HELD_SPACE_BY_DEFAULT: bool = true;
    const TITLE: &'static str = "Create task";
    const DESCRIPTION: &'static str = "Create a task in a space, or a subtask (parentId). \
        Optional: status and type by name, labels, acceptanceCriteria (a list), assignee (me or \
        @handle), and its place in its column (position: top, bottom, or a number; or \
        beforeTaskId/afterTaskId). Returns its key and link.";
    const EMITS: &'static [&'static str] = &["task.created"];

    type Input = TaskCreateInput;
    type Output = TaskWritten;

    async fn run(call: &mut OpCall<'_>, input: TaskCreateInput) -> Result<TaskWritten, OpError> {
        // A subtask goes in its parent's space; naming another is refused
        // (without naming either, as the stated one wasn't checked).
        if let (Some(_), Some(stated)) = (input.parent_id, input.project_id)
            && call.ctx.space_id != Some(stated.id())
        {
            return Err(OpError::invalid(
                "parentWorkItemId",
                "The parent is in another space than projectId. Leave out projectId: a subtask \
                 goes in its parent's space.",
            ));
        }
        let space = match (call.ctx.space_id, input.parent_id) {
            (Some(_), _) | (None, Some(_)) => None,
            (None, None) => Some(call.ctx.caller.confined_space.ok_or(OpError::invalid(
                "projectId",
                "Say which space with projectId (see projects_list).",
            ))?),
        };
        let written = task_commands::create(call.state, call.ctx, space, &input).await?;
        settled(call, &written);
        Ok(written)
    }
}

// ---------------------------------------------------------------------------
// task_update (aliases task_mark_blocked, task_mark_ready_for_review)

pub struct TaskUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskUpdateInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// The version you last read (from task_open or task_list).
    pub expected_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// "me", "none", an actor ID, or an @handle.
    #[serde(
        default,
        alias = "assigneeActorId",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_assignee: Option<bool>,
    /// Hand it to someone to do, keeping the assignee as its owner: "me",
    /// "none", an actor ID, or an @handle. People and agents can hold
    /// either role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate: Option<String>,
    /// Move to this status, by name (such as "In Progress").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Change its type, by name (such as "Bug").
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// Put it under this task (ID, link, or key) in the same space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// true makes it a top-level task again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_parent: Option<bool>,
    /// Labels to put on it, by name or ID (at most 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_labels: Option<Vec<String>>,
    /// Labels to take off it, by name or ID (at most 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_labels: Option<Vec<String>>,
    /// Its place in its column (the new one, when status changes too): top,
    /// bottom, or a number, where 1 is the top. A status change without one
    /// puts it at the bottom.
    #[serde(
        default,
        deserialize_with = "string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub position: Option<String>,
    /// Put it just above this task (ID, link, or key) in its column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_task_id: Option<String>,
    /// Put it just below this task (ID, link, or key) in its column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_task_id: Option<String>,
    /// true archives it (its claim is released), false restores it. Pass it
    /// on its own, then make other changes with the new version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// Put it in this active milestone, by name or ID (see milestone_list);
    /// "none" takes it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// Put it in one of the space's cycles that hasn't ended: "current",
    /// "next", its number or name (see cycle_list); "none" takes it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
    /// When work starts: YYYY-MM-DD, or "" to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// When it's due: YYYY-MM-DD, or "" to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// How big it is, in the space's unit (points or hours), from 0 to
    /// 100000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<f64>,
    /// true removes the estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_estimate: Option<bool>,
    /// Custom field values to set, by field name (see field_list); null
    /// clears one. Such as {"Risk": "High", "Due to customer": "2026-12-01"}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
    /// Mark it blocked, with the reason someone needs to act on. Pass it on
    /// its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// true clears its blocker and moves it to review. Pass it on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_for_review: Option<bool>,
    /// When your role only suggests on tasks and the change becomes a
    /// proposal: why, for whoever decides it (up to 500 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl TaskUpdateInput {
    /// Whether anything besides the two blocker fields is set.
    fn changes_fields(&self) -> bool {
        let mut fields = serde_json::to_value(self).unwrap_or_default();
        if let Some(object) = fields.as_object_mut() {
            for own in [
                "taskId",
                "expectedVersion",
                "blockedReason",
                "readyForReview",
                "note",
            ] {
                object.remove(own);
            }
            return !object.is_empty();
        }
        false
    }
}

impl Targeted for TaskUpdateInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskUpdate {
    const NAME: &'static str = "task_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    // Suggest is the floor: `run` asks `access::write_mode` whether the
    // change lands (edit) or waits as a proposal (suggest only, T1.41).
    const PERMISSION: Option<Perm> = Some(Perm::TasksSuggest);
    const PROPOSABLE: bool = true;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Update task";
    const DESCRIPTION: &'static str = "Change a task: title, description, priority, assignee, delegate \
        (who it is handed to), type, status (by name), its place in its column (position, beforeTaskId, afterTaskId), \
        its parent (parentId, or clearParent for top-level), labels (addLabels, removeLabels), \
        or archive it (archived: true; false restores; pass it on its own). Mark it blocked \
        with blockedReason, or clear the blocker and move it to review with readyForReview: \
        true (each on its own). Pass the version you last read as expectedVersion; on a \
        conflict, open it again and retry. If your role only suggests on tasks, a change to \
        its own fields becomes a proposal (proposalId) that someone who edits the task accepts \
        or rejects, from its request card or with proposal_decide; it is then written as yours.";
    const EMITS: &'static [&'static str] =
        &["task.updated", "task.proposals_changed", "request.raised"];

    type Input = TaskUpdateInput;
    type Output = TaskWritten;

    async fn run(call: &mut OpCall<'_>, input: TaskUpdateInput) -> Result<TaskWritten, OpError> {
        let task_id = input.task_id.id();
        let blocker = input.blocked_reason.is_some() || input.ready_for_review.is_some();
        if blocker
            && (input.changes_fields()
                || input.blocked_reason.is_some() == input.ready_for_review.is_some())
        {
            return Err(OpError::invalid(
                "blockedReason",
                "Pass blockedReason or readyForReview on its own, then make other changes with \
                 the new version.",
            ));
        }
        if let Some(written) = propose_instead(call, &input).await? {
            // The dispatcher writes the audit row for a proposal.
            if call.ctx.request_id.is_some() {
                call.key_honoured();
            }
            call.mark_replayed(written.idempotent_replay);
            return Ok(written);
        }
        let written = match (input.blocked_reason.clone(), input.ready_for_review) {
            (Some(reason), _) => {
                task_commands::mark_blocked(
                    call.state,
                    call.ctx,
                    task_id,
                    input.expected_version,
                    reason,
                )
                .await?
            }
            (None, Some(true)) => {
                task_commands::mark_ready_for_review(
                    call.state,
                    call.ctx,
                    task_id,
                    input.expected_version,
                )
                .await?
            }
            (None, Some(false)) => {
                return Err(OpError::invalid(
                    "readyForReview",
                    "readyForReview takes true; to block it again, pass blockedReason.",
                ));
            }
            (None, None) => {
                // A note is for proposals; a direct change has none.
                let mut direct = serde_json::to_value(&input).map_err(|_| OpError::Unavailable)?;
                if let Some(object) = direct.as_object_mut() {
                    object.remove("note");
                }
                task_commands::update(call.state, call.ctx, &direct).await?
            }
        };
        settled(call, &written);
        Ok(written)
    }
}

/// When the caller only suggests on tasks (T1.41), keeps the change as a
/// proposal and answers for it; None when the change is theirs to make.
async fn propose_instead(
    call: &mut OpCall<'_>,
    input: &TaskUpdateInput,
) -> Result<Option<TaskWritten>, OpError> {
    use crate::mcp::task_proposals::{self, Asked, Proposed};
    let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
    let task_id = input.task_id.id();
    let caller = call.ctx.caller.clone();
    let author = crate::work_item_commands::CommandActor::from(call.ctx);
    let principal = task_commands::principal_for(call.ctx, Some(space_id));
    let request_id = call.ctx.request_id.clone();
    let tx = call.tx().await?;
    let (creator, number, title, version) = sqlx::query_as::<_, (uuid::Uuid, i64, String, i64)>(
        "SELECT created_by_actor_id, number, title, version FROM task_rows \
         WHERE id = $1 AND project_id = $2",
    )
    .bind(task_id)
    .bind(space_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?
    .ok_or_else(|| OpError::not_found("task"))?;
    // Unlocked: the direct path takes its own locks in its own transaction.
    let mode = crate::access::write_mode(
        tx,
        &caller,
        "task",
        space_id,
        creator == author.actor_id,
        Some("tasks"),
    )
    .await?
    .map_err(|_| OpError::from(crate::error::AppError::Forbidden))?
    .mode;
    if mode != crate::access::WriteMode::Propose {
        return Ok(None);
    }
    let denied = |message: String| OpError::Denied {
        reason: crate::ops::DenyReason::Rule,
        message: message.into(),
    };
    let edit_only = input.parent_id.is_some()
        || input.clear_parent.is_some()
        || input.position.is_some()
        || input.before_task_id.is_some()
        || input.after_task_id.is_some()
        || input.archived.is_some()
        || input.milestone.is_some()
        || input.cycle.is_some()
        || input.delegate.is_some()
        || input.blocked_reason.is_some()
        || input.ready_for_review.is_some();
    if edit_only {
        return Err(denied(task_proposals::EDIT_ONLY.to_owned()));
    }
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty());
    if note.is_some_and(|note| note.chars().count() > 500) {
        return Err(OpError::invalid("note", "Keep a note to 500 characters."));
    }
    if input.clear_assignee == Some(true) && input.assignee.is_some() {
        return Err(OpError::invalid(
            "assignee",
            "Pass assignee or clearAssignee, not both.",
        ));
    }
    // A retry with the same request id answers as the first call did.
    let reservation = match request_id.as_deref() {
        Some(request_id) => {
            let key = crate::idempotency::IdempotencyKey::parse(request_id)?;
            let scope =
                crate::idempotency::resource_bound_scope("task_proposals", task_id, "propose")?;
            let hash = crate::idempotency::RequestHash::for_value(input)?;
            match crate::idempotency::reserve(tx, author.actor_id, &scope, &key, hash).await? {
                crate::idempotency::IdempotencyOutcome::Replay(replay) => {
                    let mut written = replay.decode::<TaskWritten>()?;
                    written.idempotent_replay = true;
                    return Ok(Some(written));
                }
                crate::idempotency::IdempotencyOutcome::Reserved(reservation) => Some(reservation),
            }
        }
        None => None,
    };
    let estimate = if input.clear_estimate == Some(true) {
        Some(None)
    } else {
        input.estimate.map(Some)
    };
    let asked = Asked {
        title: input.title.as_deref(),
        description: input.description.as_deref(),
        priority: input.priority.as_deref(),
        status: input.status.as_deref(),
        item_type: input.item_type.as_deref(),
        assignee: if input.clear_assignee == Some(true) {
            Some("none")
        } else {
            input.assignee.as_deref()
        },
        add_labels: input.add_labels.as_deref().unwrap_or_default(),
        remove_labels: input.remove_labels.as_deref().unwrap_or_default(),
        start_date: input.start_date.as_deref(),
        due_date: input.due_date.as_deref(),
        estimate,
        fields: input.fields.as_ref(),
    };
    let proposed = task_proposals::propose(
        tx,
        &author,
        &principal,
        caller.via_grant_id,
        task_id,
        input.expected_version,
        &asked,
        note,
    )
    .await?;
    let (proposal, changes) = match proposed {
        Proposed::Made { id, changes } => (id, changes),
        Proposed::Unchanged => {
            return Err(OpError::invalid(
                "update",
                "That wouldn't change anything on the task.",
            ));
        }
        Proposed::Problem(field, message) if field == "expectedVersion" => {
            return Err(OpError::Conflict {
                reason: crate::ops::ConflictReason::State {
                    code: "task_changed",
                },
                message: message.into(),
                current: None,
            });
        }
        Proposed::Problem(field, message) => {
            return Err(OpError::Invalid {
                field: field.into(),
                message: message.into(),
            });
        }
        Proposed::Refused(message) => return Err(denied(message)),
    };
    let key = sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
        .bind(space_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
    let key = format!("{key}-{number}");
    crate::proposals::raise_request(
        tx,
        proposal,
        &format!("Accept a change to {key} \"{title}\"?"),
        Some(creator),
    )
    .await?;
    let written = TaskWritten {
        work_item_id: task_id.to_string(),
        version,
        changed_fields: Vec::new(),
        activity_id: None,
        result_id: proposal.to_string(),
        idempotent_replay: false,
        key: Some(key),
        url: None,
        message: Some(
            "Your role here suggests changes to tasks, so this is a proposal. The task changes \
             once someone who edits it accepts."
                .to_owned(),
        ),
        labels: None,
        proposal_id: Some(proposal.to_string()),
        proposed: changes,
    };
    if let Some(reservation) = reservation {
        crate::idempotency::complete(tx, &reservation, axum::http::StatusCode::OK, &written)
            .await?;
    }
    call.audit_target("proposal", proposal, json!({ "task": task_id }));
    Ok(Some(written))
}

// ---------------------------------------------------------------------------
// task_archive, task_restore

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskVersionInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// The version you last read (from task_open or task_list).
    pub expected_version: i64,
}

impl Targeted for TaskVersionInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

pub struct TaskArchive;

impl Operation for TaskArchive {
    const NAME: &'static str = "task_archive";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksDelete);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Archive task";
    const DESCRIPTION: &'static str = "Archive a task: it leaves the lists and boards, and its \
        claim is released. task_restore brings it back with its history.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskVersionInput;
    type Output = TaskWritten;

    async fn run(call: &mut OpCall<'_>, input: TaskVersionInput) -> Result<TaskWritten, OpError> {
        let written = task_commands::set_archived(
            call.state,
            call.ctx,
            input.task_id.id(),
            input.expected_version,
            true,
        )
        .await?;
        settled(call, &written);
        Ok(written)
    }
}

pub struct TaskRestore;

impl Operation for TaskRestore {
    const NAME: &'static str = "task_restore";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksDelete);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Restore task";
    const DESCRIPTION: &'static str = "Restore an archived task to its lists and boards, as it \
        was. The archived original of a task moved to another space stays archived.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskVersionInput;
    type Output = TaskWritten;

    async fn run(call: &mut OpCall<'_>, input: TaskVersionInput) -> Result<TaskWritten, OpError> {
        let written = task_commands::set_archived(
            call.state,
            call.ctx,
            input.task_id.id(),
            input.expected_version,
            false,
        )
        .await?;
        settled(call, &written);
        Ok(written)
    }
}

// ---------------------------------------------------------------------------
// task_comment_create (alias task_comment)

pub struct TaskCommentCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskCommentCreateInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// The comment or update. It may span several lines and use Markdown.
    /// @handles of people in the space become mentions and notify them.
    #[serde(alias = "update", default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Structured content instead of text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// Files to post with it: up to 10 fileIds from file_upload with taskId
    /// and forComment: true on this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<String>>,
    /// To answer one comment on this task, its id (task_open lists them): it
    /// shows quoted above yours, and its author is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_comment_id: Option<String>,
}

impl Targeted for TaskCommentCreateInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskCommentCreate {
    const NAME: &'static str = "task_comment_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksComment);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Comment on task";
    const DESCRIPTION: &'static str = "Comment on a task: progress, a handoff, or what's left. \
        Keep it useful; skip status-only noise. @handles of people in the space are notified. \
        To answer one comment, pass replyToCommentId: it shows quoted above yours and its \
        author is told.";
    const EMITS: &'static [&'static str] = &["task.commented"];

    type Input = TaskCommentCreateInput;
    type Output = TaskWritten;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskCommentCreateInput,
    ) -> Result<TaskWritten, OpError> {
        let written = task_commands::comment(
            call.state,
            call.ctx,
            input.task_id.id(),
            task_commands::CommentText {
                text: input.text,
                content: input.content,
            },
            input.attachment_ids,
            input.reply_to_comment_id,
        )
        .await?;
        settled(call, &written);
        Ok(written)
    }
}

// ---------------------------------------------------------------------------
// task_comment_update (alias task_comment_edit), task_comment_delete

pub struct TaskCommentUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskCommentUpdateInput {
    /// The task: its ID, link, or key such as AWD-12.
    pub task_id: IdRef,
    /// The comment's ID. task_open lists the task's recent comments with
    /// their IDs.
    pub comment_id: IdRef,
    /// The new text, 1 to 20,000 characters. Line breaks and Markdown are
    /// kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// New structured content instead of text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// The revision you read (this returns it; a comment never edited is 1).
    /// Leave it out to edit whatever is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i32>,
}

impl Targeted for TaskCommentUpdateInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskCommentUpdate {
    const NAME: &'static str = "task_comment_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksComment);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Edit task comment";
    const DESCRIPTION: &'static str = "Edit your own comment on a task. Earlier versions stay in \
        the comment's history for everyone who can see the task, and it shows (edited). To take \
        down a secret, use task_comment_delete instead: editing keeps the old text in the \
        history.";
    const EMITS: &'static [&'static str] = &["task.comment_updated"];

    type Input = TaskCommentUpdateInput;
    type Output = EditedComment;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskCommentUpdateInput,
    ) -> Result<EditedComment, OpError> {
        let edited = task_commands::comment_edit(
            call.state,
            call.ctx,
            input.task_id.id(),
            input.comment_id.id(),
            task_commands::CommentText {
                text: input.text,
                content: input.content,
            },
            input.expected_revision,
        )
        .await?;
        // The command keeps no request ids, so a retry after a later edit
        // would write again: replaySafe stays false.
        call.audited_by_command();
        Ok(edited)
    }
}

pub struct TaskCommentDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskCommentDeleteInput {
    /// The task: its ID, link, or key such as AWD-12.
    pub task_id: IdRef,
    /// The comment's ID. task_open lists the task's recent comments with
    /// their IDs.
    pub comment_id: IdRef,
}

impl Targeted for TaskCommentDeleteInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskCommentDelete {
    const NAME: &'static str = "task_comment_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksComment);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Delete task comment";
    const DESCRIPTION: &'static str = "Delete a comment on a task for everyone, leaving \
        \"comment deleted\" in the task's history. Its text and every earlier version are \
        deleted, with its files. You can delete your own comments; space owners and admins can \
        delete anyone's. This can't be undone. Rotate any secret it held.";
    const EMITS: &'static [&'static str] = &["task.comment_removed"];

    type Input = TaskCommentDeleteInput;
    type Output = DeletedComment;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskCommentDeleteInput,
    ) -> Result<DeletedComment, OpError> {
        let deleted = task_commands::comment_delete(
            call.state,
            call.ctx,
            input.task_id.id(),
            input.comment_id.id(),
        )
        .await?;
        // Deleting what is deleted changes nothing.
        call.key_honoured();
        call.audited_by_command();
        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// task_criteria_update (alias task_criteria)

pub struct TaskCriteriaUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskCriteriaUpdateInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: IdRef,
    /// The task's version you last read (from task_open or task_list).
    pub expected_version: i64,
    /// 1 to 50 operations, applied in order. Either all apply or none do.
    pub operations: Vec<CriterionOperationArgument>,
}

impl Targeted for TaskCriteriaUpdateInput {
    fn target(&self) -> Target {
        Target::thing("task", self.task_id.id())
    }
}

impl Operation for TaskCriteriaUpdate {
    const NAME: &'static str = "task_criteria_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Task acceptance criteria";
    const DESCRIPTION: &'static str = "Change a task's acceptance criteria in one call: add \
        (optionally at a position), edit, check, uncheck, remove, or move. Name a criterion by \
        its ID or by its position in the version you read. Operations apply in order, and \
        either all apply or none do. Pass the task's version as expectedVersion.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TaskCriteriaUpdateInput;
    type Output = CriteriaValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskCriteriaUpdateInput,
    ) -> Result<CriteriaValue, OpError> {
        let task_id = input.task_id.id();
        let value = task_commands::criteria(
            call.state,
            call.ctx,
            task_id,
            input.expected_version,
            input.operations,
        )
        .await?;
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.mark_replayed(value.idempotent_replay);
        call.audit_target("task", task_id, json!({}));
        call.audited_by_command();
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Triage (T1.23): triage_decide, triage_enable, triage_rule_list,
// triage_rule_set

fn refusal((field, message): super::triage::Refusal) -> OpError {
    OpError::Invalid {
        field: std::borrow::Cow::Borrowed(field),
        message: std::borrow::Cow::Owned(message),
    }
}

/// Locks the call's space for a write, as task commands do.
async fn locked_space(
    call: &mut OpCall<'_>,
) -> Result<crate::work_item_commands::CommandActor, OpError> {
    let actor = crate::work_item_commands::CommandActor::from(call.ctx);
    let tx = call.tx().await?;
    crate::work_item_commands::lock_membership(tx, actor.actor_id, actor.project_id)
        .await?
        .ok_or(OpError::not_found("That space"))?;
    Ok(actor)
}

pub struct TaskTriage;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskTriageInput {
    /// The tasks, all in one space: their IDs, links or keys such as
    /// AWD-12 (at most 50).
    pub task_ids: Vec<IdRef>,
    pub action: super::triage::TriageAction,
    /// accept: the status it moves to, by name or ID; the space's first
    /// unstarted status if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// duplicate: the original task (ID, link or key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<IdRef>,
    /// snooze: when it comes back to the queue, such as
    /// 2026-12-01T09:00:00Z (within a year).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    /// decline: why, posted on the task as your comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Targeted for TaskTriageInput {
    fn target(&self) -> Target {
        Target::thing(
            "task",
            self.task_ids
                .first()
                .map_or_else(uuid::Uuid::nil, |task| task.id()),
        )
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskTriageOutput {
    /// Each task as it is now.
    pub tasks: Vec<super::triage::TriagedTask>,
}

impl Operation for TaskTriage {
    const NAME: &'static str = "triage_decide";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Triage tasks";
    const DESCRIPTION: &'static str = "Decide on tasks waiting in a space's triage queue: \
        accept moves them out of triage (to status, or the first unstarted status), decline \
        cancels them with reason as a comment, duplicate marks them duplicates of duplicateOf \
        and cancels them, and snooze hides them from the queue until a time. All the tasks \
        change, or none do.";
    const EMITS: &'static [&'static str] = &["task.updated", "task.triaged"];

    type Input = TaskTriageInput;
    type Output = TaskTriageOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: TaskTriageInput,
    ) -> Result<TaskTriageOutput, OpError> {
        let actor = locked_space(call).await?;
        let task_ids = input
            .task_ids
            .iter()
            .map(|task| task.id())
            .collect::<Vec<_>>();
        let tx = call.tx().await?;
        let tasks = super::triage::triage(
            tx,
            &actor,
            &task_ids,
            &super::triage::TriageRequest {
                action: input.action,
                status: input.status.as_deref(),
                of: input.duplicate_of.map(IdRef::id),
                until: input.until,
                reason: input.reason.as_deref(),
            },
        )
        .await?
        .map_err(refusal)?;
        for task in &tasks {
            call.audit_target("task", task.task_id, json!({ "action": input.action }));
        }
        // A decline's reason is a comment on each task, posted through the
        // send pipeline once the decision commits.
        if let (super::triage::TriageAction::Decline, Some(reason)) = (
            input.action,
            input
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty()),
        ) {
            for task in &tasks {
                let actor: crate::conversations::MessageCommandActor = call.ctx.into();
                let creator = crate::discussion::ops::creator(call.ctx);
                let task_id = task.task_id;
                let text = reason.to_owned();
                let key = format!(
                    "triage-decline-{}-{}",
                    task_id.simple(),
                    call.ctx.request_id.as_deref().map_or_else(
                        || uuid::Uuid::now_v7().simple().to_string(),
                        ToOwned::to_owned,
                    )
                );
                call.after_commit(Box::new(move |state| {
                    Box::pin(async move {
                        let Ok(key) = crate::idempotency::IdempotencyKey::parse(&key) else {
                            return;
                        };
                        if let Err(error) = crate::discussion::post(
                            &state,
                            actor,
                            creator,
                            crate::discussion::SubjectKind::Task,
                            task_id,
                            crate::discussion::CommentPlace::Stream,
                            key,
                            crate::discussion::CommentBody::Text(text),
                            &[],
                            None,
                        )
                        .await
                        {
                            tracing::warn!(?error, %task_id, "a declined task's reason wasn't posted");
                        }
                    })
                }));
            }
        }
        Ok(TaskTriageOutput { tasks })
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriageSpaceInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
}

impl Targeted for TriageSpaceInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

pub struct TriageEnable;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TriageEnableOutput {
    /// The space's triage status, where new tasks from forms and bug
    /// reports land.
    #[schemars(with = "String")]
    pub status_id: uuid::Uuid,
}

impl Operation for TriageEnable {
    const NAME: &'static str = "triage_enable";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksWorkflow);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Turn on triage";
    const DESCRIPTION: &'static str = "Give a space a triage queue: a Triage status first in \
        its workflow, which tasks from forms and bug report links land in, with a move from \
        it to every other status. A space that has one already keeps it. To stop using \
        triage, retire the status with workflow_update.";
    const EMITS: &'static [&'static str] = &["task.updated"];

    type Input = TriageSpaceInput;
    type Output = TriageEnableOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: TriageSpaceInput,
    ) -> Result<TriageEnableOutput, OpError> {
        let actor = locked_space(call).await?;
        let tx = call.tx().await?;
        let status_id = super::triage::enable(tx, &actor).await?.map_err(refusal)?;
        call.audit_target(
            "space",
            input.space_id.id(),
            json!({ "triageStatusId": status_id }),
        );
        Ok(TriageEnableOutput { status_id })
    }
}

pub struct TriageRuleList;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TriageRuleListOutput {
    /// The space's triage status; none while it doesn't use triage.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub triage_status_id: Option<uuid::Uuid>,
    /// In the order they run.
    pub rules: Vec<super::triage::Rule>,
}

impl Operation for TriageRuleList {
    const NAME: &'static str = "triage_rule_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List triage rules";
    const DESCRIPTION: &'static str = "A space's triage rules in the order they run over each \
        task that lands in triage (the first that matches acts), and its triage status.";

    type Input = TriageSpaceInput;
    type Output = TriageRuleListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: TriageSpaceInput,
    ) -> Result<TriageRuleListOutput, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        Ok(TriageRuleListOutput {
            triage_status_id: super::triage::triage_status(tx, space_id).await?,
            rules: super::triage::rules(tx, space_id).await?,
        })
    }
}

pub struct TriageRuleWrite;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriageRuleWriteInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// The rule to change or remove; leave it out to make one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<IdRef>,
    /// The rule's version you last read; a change since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Which arriving tasks it matches: a view filter (a list of clauses
    /// such as [{"field": "<field id>", "op": "is", "values": ["…"]}], or
    /// "and" and "or" groups); an empty list matches every task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<super::triage::RuleAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Its place, from 1 (runs first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// true removes the rule.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

impl Targeted for TriageRuleWriteInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TriageRuleWriteOutput {
    /// The rule as it is now; none once removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<super::triage::Rule>,
    pub removed: bool,
}

/// Tells the space's open settings that its triage rules changed, naming
/// the rule only.
async fn rules_changed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &crate::work_item_commands::CommandActor,
    rule: uuid::Uuid,
) -> Result<(), OpError> {
    let mut event = crate::events::NewEvent::thing(
        &crate::events::catalog::TRIAGE_RULES_CHANGED,
        actor.project_id,
        "space",
        actor.project_id,
    )
    .actor(actor.actor_id)
    .audience(crate::events::Audience::Space)
    .payload(json!({ "ruleId": rule }));
    // A rule isn't a thing of its own: the event is about the space.
    event.thing = None;
    crate::events::emit(tx, event).await?;
    Ok(())
}

impl Operation for TriageRuleWrite {
    const NAME: &'static str = "triage_rule_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksWorkflow);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const EMITS: &'static [&'static str] = &["triage.rules_changed"];
    const TITLE: &'static str = "Write a triage rule";
    const DESCRIPTION: &'static str = "Make, change, move or remove (remove: true) one of a \
        space's triage rules. Rules run in order over each task that lands in triage, and the \
        first whose filter matches acts: action {kind: accept, status?} moves it out of \
        triage, {kind: assign, to} gives it to someone, {kind: keep} leaves it for a person.";

    type Input = TriageRuleWriteInput;
    type Output = TriageRuleWriteOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: TriageRuleWriteInput,
    ) -> Result<TriageRuleWriteOutput, OpError> {
        let actor = locked_space(call).await?;
        let tx = call.tx().await?;
        if input.remove {
            let rule = input
                .rule
                .ok_or(OpError::invalid("rule", "Name the rule to remove."))?;
            let removed = super::triage::remove_rule(tx, actor.project_id, rule.id()).await?;
            if !removed {
                return Err(OpError::not_found("That triage rule"));
            }
            rules_changed(tx, &actor, rule.id()).await?;
            call.audit_target("triage_rule", rule.id(), json!({ "removed": true }));
            return Ok(TriageRuleWriteOutput {
                rule: None,
                removed: true,
            });
        }
        let rule = super::triage::write_rule(
            tx,
            &actor,
            input.rule.map(|rule| (rule.id(), input.expected_version)),
            super::triage::RuleChange {
                name: input.name,
                filter: input.filter,
                action: input.action,
                enabled: input.enabled,
                position: input.position,
            },
        )
        .await?
        .map_err(refusal)?;
        rules_changed(tx, &actor, rule.id).await?;
        call.audit_target("triage_rule", rule.id, json!({}));
        Ok(TriageRuleWriteOutput {
            rule: Some(rule),
            removed: false,
        })
    }
}

// ---------------------------------------------------------------------------
// cycle_list, cycle_set

pub struct CycleList;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CyclesOutput {
    pub settings: super::cycles::Settings,
    /// The cycles that haven't ended, and the last few that have, in order.
    pub cycles: Vec<super::cycles::Cycle>,
}

impl Operation for CycleList {
    const NAME: &'static str = "cycle_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List cycles";
    const DESCRIPTION: &'static str = "A space's cycle settings and its cycles: the running \
        one, the upcoming ones and the last few that ended, each with its task counts and a \
        count a day while it ran (its burn-up). Put a task in one with task_update's cycle.";

    type Input = TriageSpaceInput;
    type Output = CyclesOutput;

    async fn run(call: &mut OpCall<'_>, _input: TriageSpaceInput) -> Result<CyclesOutput, OpError> {
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        Ok(CyclesOutput {
            settings: super::cycles::settings(tx, space_id).await?,
            cycles: super::cycles::list(tx, space_id, chrono::Utc::now().date_naive()).await?,
        })
    }
}

pub struct CycleSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleSetInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// The settings' version you last read (cycle_list); a change since is
    /// refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// true turns cycles on (the running one and the upcoming ones are made
    /// at once); false stops making them, keeping those made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Days in a cycle, from 7 to 56. Cycles already made keep theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_days: Option<i32>,
    /// The weekday the first cycle starts on: 1 is Monday, 7 is Sunday.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on_weekday: Option<i16>,
    /// How many cycles after the running one are kept made, from 1 to 6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upcoming: Option<i32>,
    /// Whether a cycle's unfinished tasks move into the next when it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll_over: Option<bool>,
    /// A cycle to rename, by number, ID or name, with name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
    /// The cycle's new name (at most 80 characters); "" goes back to
    /// "Cycle N".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Targeted for CycleSetInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for CycleSet {
    const NAME: &'static str = "cycle_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksWorkflow);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["cycles.changed"];
    const TITLE: &'static str = "Set up cycles";
    const DESCRIPTION: &'static str = "Change a space's cycles: turn them on or off, their \
        length in days, the weekday they start on, how many are kept made ahead, and whether \
        unfinished tasks roll into the next cycle when one ends. Or rename one cycle (cycle \
        and name). Every day the running cycle's tasks are counted for its burn-up.";

    type Input = CycleSetInput;
    type Output = CyclesOutput;

    async fn run(call: &mut OpCall<'_>, input: CycleSetInput) -> Result<CyclesOutput, OpError> {
        let actor = locked_space(call).await?;
        let today = chrono::Utc::now().date_naive();
        let change = super::cycles::SettingsChange {
            enabled: input.enabled,
            length_days: input.length_days,
            starts_on_weekday: input.starts_on_weekday,
            upcoming: input.upcoming,
            roll_over: input.roll_over,
        };
        if change.is_empty() && input.cycle.is_none() {
            return Err(OpError::invalid(
                "enabled",
                "Pass a setting to change (enabled, lengthDays, startsOnWeekday, upcoming, \
                 rollOver), or a cycle and its new name.",
            ));
        }
        if input.cycle.is_some() != input.name.is_some() {
            return Err(OpError::invalid(
                "name",
                "Pass a cycle and its new name together.",
            ));
        }
        let tx = call.tx().await?;
        if !change.is_empty() {
            super::cycles::write_settings(tx, &actor, input.expected_version, &change, today)
                .await?
                .map_err(refusal)?;
        }
        if let (Some(cycle), Some(name)) = (input.cycle.as_deref(), input.name.as_deref()) {
            let Some(Some(id)) = super::cycles::reference_any(tx, actor.project_id, cycle).await?
            else {
                return Err(OpError::invalid(
                    "cycle",
                    "This space has no such cycle. Name it by number, ID or name (cycle_list \
                     lists them).",
                ));
            };
            super::cycles::rename(tx, actor.project_id, id, name)
                .await?
                .map_err(refusal)?;
        }
        let mut event = crate::events::NewEvent::thing(
            &crate::events::catalog::CYCLES_CHANGED,
            actor.project_id,
            "space",
            actor.project_id,
        )
        .actor(actor.actor_id)
        .audience(crate::events::Audience::Space);
        // Cycles aren't things of their own: the event is about the space.
        event.thing = None;
        crate::events::emit(tx, event).await?;
        call.audit_target("space", actor.project_id, json!({}));
        let tx = call.tx().await?;
        Ok(CyclesOutput {
            settings: super::cycles::settings(tx, actor.project_id).await?,
            cycles: super::cycles::list(tx, actor.project_id, today).await?,
        })
    }
}

// ---------------------------------------------------------------------------
// initiative_list, initiative_create

pub struct InitiativeList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitiativeListInput {
    /// true lists archived initiatives too, after the others.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_archived: bool,
}

impl Targeted for InitiativeListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Whether the caller may do `need` in the workspace's own space; Err when
/// they can't see it at all.
async fn in_workspace_space(
    call: &mut OpCall<'_>,
    space: uuid::Uuid,
    need: crate::access::Need,
) -> Result<bool, OpError> {
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    match crate::access::check(tx, &caller, need, crate::access::Target::Space(space)).await? {
        Ok(_) => Ok(true),
        Err(crate::access::Denied::NotFound) => Err(OpError::not_found("That workspace")),
        Err(_) => Ok(false),
    }
}

impl Operation for InitiativeList {
    const NAME: &'static str = "initiative_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List initiatives";
    const DESCRIPTION: &'static str = "The workspace's initiatives: work that spans spaces, \
        each a record of the workspace's Initiatives collection (change one with record_update_many \
        there) with its milestones in the spaces you can see and their task counts. Put a \
        milestone in one with milestone_write's initiative; make one with initiative_create.";

    type Input = InitiativeListInput;
    type Output = super::initiatives::Initiatives;

    async fn run(
        call: &mut OpCall<'_>,
        input: InitiativeListInput,
    ) -> Result<super::initiatives::Initiatives, OpError> {
        let workspace = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let space = {
            let tx = call.tx().await?;
            crate::spaces::workspace::find(tx, workspace).await?
        };
        if let Some(space) = space {
            in_workspace_space(call, space, crate::access::Need::SEE).await?;
        }
        let tx = call.tx().await?;
        Ok(super::initiatives::list(tx, workspace, &caller, input.include_archived).await?)
    }
}

pub struct InitiativeCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitiativeCreateInput {
    /// Its name.
    pub title: String,
    /// Values by field name: Owner (a person), Status (Planned, Active, Done
    /// or Canceled), Target date (YYYY-MM-DD), Health (On track, At risk or
    /// Off track), or any field added to the Initiatives collection.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Targeted for InitiativeCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitiativeCreateOutput {
    pub id: String,
    pub number: i64,
    pub title: String,
    /// The Initiatives collection it is a record of.
    pub collection_id: String,
}

impl Operation for InitiativeCreate {
    const NAME: &'static str = "initiative_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["record.created"];
    const TITLE: &'static str = "Create an initiative";
    const DESCRIPTION: &'static str = "Start an initiative: work that spans spaces, followed \
        through its milestones. It is a record of the workspace's Initiatives collection \
        (made with Owner, Status, Target date and Health fields the first time), so it is \
        changed with record_update_many there. Add milestones with milestone_write's initiative.";

    type Input = InitiativeCreateInput;
    type Output = InitiativeCreateOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: InitiativeCreateInput,
    ) -> Result<InitiativeCreateOutput, OpError> {
        use crate::{access::Need, collections::records};
        let title = input.title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 300 {
            return Err(OpError::invalid(
                "title",
                "Name the initiative in 1 to 300 characters.",
            ));
        }
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let space = {
            let tx = call.tx().await?;
            crate::spaces::workspace::ensure(tx, workspace).await?
        };
        let made = {
            let tx = call.tx().await?;
            super::initiatives::find(tx, workspace).await?.is_some()
        };
        let denied = |message: &'static str, permission: Perm| OpError::Denied {
            reason: crate::ops::DenyReason::Role { permission },
            message: message.into(),
        };
        if !in_workspace_space(call, space, Need::permission(Perm::CollectionsEdit)).await? {
            return Err(denied(
                "Your role in the workspace can't add initiatives.",
                Perm::CollectionsEdit,
            ));
        }
        let may_build =
            in_workspace_space(call, space, Need::permission(Perm::CollectionsBuild)).await?;
        if !made && !may_build {
            return Err(denied(
                "The workspace has no Initiatives collection yet, and your role can't make one.",
                Perm::CollectionsBuild,
            ));
        }
        let tx = call.tx().await?;
        let (space, collection) = super::initiatives::ensure(tx, workspace, actor).await?;
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space)
                .fetch_one(&mut **tx)
                .await
                .map_err(crate::error::AppError::from)?;
        let definitions =
            crate::collections::field_definitions(tx, space, collection.id, true).await?;
        let written = records::write(
            tx,
            space,
            &collection,
            crate::work_item_fields::Writer {
                actor_id: actor,
                project_key: &project_key,
            },
            may_build,
            &definitions,
            "fields",
            &[records::RecordInput {
                record: None,
                title: Some(title),
                values: input.fields.into_iter().collect(),
                archived: None,
                expected_version: None,
            }],
        )
        .await?
        .map_err(|problem| OpError::Invalid {
            field: problem.field.into(),
            message: problem.message.into(),
        })?;
        let record = written
            .into_iter()
            .next()
            .ok_or(OpError::Unavailable)?
            .record()
            .clone();
        crate::outbox::enqueue_collection_updated(tx, space, collection.id, &[record.id]).await?;
        call.audit_target("record", record.id, json!({"initiative": true}));
        Ok(InitiativeCreateOutput {
            id: record.id.to_string(),
            number: record.number,
            title: record.title,
            collection_id: collection.id.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// milestone_reorder

pub struct MilestoneReorder;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MilestoneReorderInput {
    /// The milestone to move: its ID (milestone_list).
    pub milestone_id: IdRef,
    /// Its new place among its space's milestones, 1 being first. A place
    /// past the end puts it last.
    pub position: i32,
    /// The milestone's version you last read; a change since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for MilestoneReorderInput {
    fn target(&self) -> Target {
        Target::thing("milestone", self.milestone_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneOrder {
    /// The space's milestones in their new order, archived ones last.
    pub milestones: Vec<MilestonePlace>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MilestonePlace {
    pub id: String,
    pub name: String,
    pub position: i32,
    pub version: i64,
    pub archived: bool,
}

impl Operation for MilestoneReorder {
    const NAME: &'static str = "milestone_reorder";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["milestone.updated"];
    const TITLE: &'static str = "Reorder milestones";
    const DESCRIPTION: &'static str = "Move a milestone to a new place in its space's order \
        (1 is first); the milestones page, the timeline and milestone_list show that order.";

    type Input = MilestoneReorderInput;
    type Output = MilestoneOrder;

    async fn run(
        call: &mut OpCall<'_>,
        input: MilestoneReorderInput,
    ) -> Result<MilestoneOrder, OpError> {
        let actor = locked_space(call).await?;
        let tx = call.tx().await?;
        match crate::work_item_planning::reorder_milestone(
            tx,
            &actor,
            input.milestone_id.id(),
            input.expected_version,
            input.position,
        )
        .await?
        {
            Ok(()) => {}
            Err(crate::work_item_planning::MilestoneRefusal::VersionConflict {
                current_version,
            }) => {
                return Err(OpError::Conflict {
                    reason: crate::ops::ConflictReason::Version { current_version },
                    message: format!(
                        "Someone changed this milestone first (now version {current_version}). Read it again, then retry."
                    )
                    .into(),
                    current: None,
                });
            }
            Err(_) => return Err(OpError::not_found("That milestone")),
        }
        let milestones = sqlx::query_as::<_, (uuid::Uuid, String, i32, i64, bool)>(
            "SELECT id, name, position, version, archived_at IS NOT NULL FROM project_milestones \
             WHERE project_id = $1 ORDER BY archived_at IS NOT NULL, position, id",
        )
        .bind(actor.project_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?
        .into_iter()
        .map(|(id, name, position, version, archived)| MilestonePlace {
            id: id.to_string(),
            name,
            position,
            version,
            archived,
        })
        .collect();
        call.audited_by_command();
        Ok(MilestoneOrder { milestones })
    }
}

// ---------------------------------------------------------------------------
// project_status_get, project_status_set, project_update_create

/// A space named by ID, link or key.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceRefInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
}

impl Targeted for SpaceRefInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

pub struct ProjectStatusGet;

impl Operation for ProjectStatusGet {
    const NAME: &'static str = "project_status_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksView);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get a project's status";
    const DESCRIPTION: &'static str = "A space's status: its health (on_track, at_risk or \
        off_track), target date and lead, and its latest update (a thread; read it with \
        conversation_read). Post an update with project_update_create.";

    type Input = SpaceRefInput;
    type Output = super::project_status::SpaceStatus;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SpaceRefInput,
    ) -> Result<super::project_status::SpaceStatus, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let reader = call.account_id();
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let can_update = crate::access::check(
            tx,
            &caller,
            crate::access::Need::permission(Perm::TasksEdit),
            crate::access::Target::Space(space),
        )
        .await?
        .is_ok();
        let mut status = super::project_status::status_of(tx, reader, space).await?;
        status.can_update = Some(can_update && caller.can_write);
        Ok(status)
    }
}

pub struct ProjectStatusSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectStatusSetInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// on_track, at_risk or off_track; "" clears it. Posting an update
    /// (project_update_create) sets it too, and says why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// YYYY-MM-DD; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<String>,
    /// "me", a member's ID or handle; "none" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// The status's version you last read (project_status_get); a change
    /// since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for ProjectStatusSetInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for ProjectStatusSet {
    const NAME: &'static str = "project_status_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.status_changed"];
    const TITLE: &'static str = "Set a project's status";
    const DESCRIPTION: &'static str = "Change a space's health, target date or lead. To say \
        how it's going (and set its health with it), post an update with project_update_create.";

    type Input = ProjectStatusSetInput;
    type Output = super::project_status::SpaceStatus;

    async fn run(
        call: &mut OpCall<'_>,
        input: ProjectStatusSetInput,
    ) -> Result<super::project_status::SpaceStatus, OpError> {
        use super::project_status::{self as status, StatusChange, StatusRefusal};
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let mut change = StatusChange::default();
        if let Some(health) = input.health.as_deref() {
            change.health = Some(if health.trim().is_empty() {
                None
            } else {
                Some(
                    status::parse_health(health)
                        .ok_or(OpError::invalid("health", status::HEALTH_MESSAGE))?,
                )
            });
        }
        if let Some(date) = input.target_date.as_deref() {
            change.target_date = Some(if date.trim().is_empty() {
                None
            } else {
                Some(
                    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
                        .map_err(|_| OpError::invalid("targetDate", "Use YYYY-MM-DD, or \"\"."))?,
                )
            });
        }
        let tx = call.tx().await?;
        if let Some(lead) = input.lead.as_deref() {
            change.lead = Some(
                status::resolve_lead(tx, space, actor, lead)
                    .await?
                    .map_err(|()| {
                        OpError::invalid(
                            "lead",
                            "Name a member of this space: me, an ID or a handle.",
                        )
                    })?,
            );
        }
        if change.is_empty() {
            return Err(OpError::invalid(
                "health",
                "Change something: health, targetDate or lead.",
            ));
        }
        match status::set(tx, workspace, space, actor, &change, input.expected_version).await? {
            Ok(_) => {}
            Err(StatusRefusal::VersionConflict { current_version }) => {
                return Err(OpError::Conflict {
                    reason: crate::ops::ConflictReason::Version { current_version },
                    message: format!(
                        "Someone changed this status first (now version {current_version}). Read it again, then retry."
                    )
                    .into(),
                    current: None,
                });
            }
        }
        emit_status_changed(tx, space, actor).await?;
        call.audit_target("space", space, json!({"status": true}));
        let tx = call.tx().await?;
        Ok(status::status_of(tx, actor, space).await?)
    }
}

async fn emit_status_changed(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    space: uuid::Uuid,
    actor: uuid::Uuid,
) -> Result<(), crate::error::AppError> {
    let mut event = crate::events::NewEvent::thing(
        &crate::events::catalog::SPACE_STATUS_CHANGED,
        space,
        "space",
        space,
    )
    .actor(actor)
    .audience(crate::events::Audience::Space);
    // The status isn't a thing of its own: the event is about the space.
    event.thing = None;
    crate::events::emit(tx, event).await?;
    Ok(())
}

pub struct ProjectUpdatePost;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectUpdatePostInput {
    /// The space (a project): its ID, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// How it's going: on_track, at_risk or off_track. It becomes the
    /// space's health and the update's outcome.
    pub health: String,
    /// The update: what moved, what's next, what's in the way. `@handle`
    /// mentions someone.
    pub text: String,
    /// Its name; "Update, <date>" when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The channel it goes in; the space's oldest open channel when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<IdRef>,
}

impl Targeted for ProjectUpdatePostInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdatePosted {
    /// The update's thread.
    pub thread_id: String,
    pub channel_id: String,
    pub message_id: String,
    /// The space's status after it.
    pub status: super::project_status::SpaceStatus,
}

impl Operation for ProjectUpdatePost {
    const NAME: &'static str = "project_update_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["space.status_changed", "message.created"];
    const TITLE: &'static str = "Post a project update";
    const DESCRIPTION: &'static str = "Say how a space is going, such as its weekly update: a \
        thread in one of its channels whose outcome is the health you give, which becomes the \
        space's health. It shows on the space's home, in its members' My work and in the \
        initiatives its milestones are part of. Pass a clientRequestId so a retry posts once.";

    type Input = ProjectUpdatePostInput;
    type Output = ProjectUpdatePosted;

    async fn run(
        call: &mut OpCall<'_>,
        input: ProjectUpdatePostInput,
    ) -> Result<ProjectUpdatePosted, OpError> {
        use super::project_status as status;
        use crate::{
            conversations::{
                AppendExtras, ThreadPatch, append_message_command_with,
                threads::{self, ThreadName, ThreadSpec},
            },
            idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
        };
        let health = status::parse_health(&input.health)
            .ok_or(OpError::invalid("health", status::HEALTH_MESSAGE))?;
        let text = input.text.trim().to_owned();
        if text.is_empty() {
            return Err(OpError::invalid("text", "Say how it's going."));
        }
        let title = match input.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => title.to_owned(),
            _ => format!("Update, {}", chrono::Utc::now().format("%b %-d, %Y")),
        };
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let actor = call.account_id();
        let key = match call.ctx.request_id.as_deref() {
            Some(key) => IdempotencyKey::parse(key),
            None => IdempotencyKey::parse(&uuid::Uuid::now_v7().to_string()),
        }
        .map_err(|_| OpError::invalid("clientRequestId", "Pass a short request id."))?;
        let mut creator = crate::discussion::ops::creator(call.ctx);
        // The channel must be one of this space's.
        creator.project_id = Some(space);
        let state = call.state;
        let mut tx = crate::db::scoped_tx(&state.pool)
            .await
            .map_err(crate::error::AppError::from)?;
        let channel = match input.channel_id {
            Some(channel) => channel.id(),
            None => status::default_channel(&mut tx, space, actor)
                .await?
                .ok_or(OpError::invalid(
                    "channelId",
                    "This space has no channel you can post in.",
                ))?,
        };
        // The text is checked before anything is saved, so an update the
        // message can't carry leaves the space's health as it was. The
        // thread's people are its channel's.
        let content = crate::mcp::content_from_text(state, space, Some(channel), &text).await?;
        if let Err(error) = crate::message_content::MessageContent::from_json(content.clone()) {
            use crate::message_content::MessageContentError as Problem;
            return Err(OpError::invalid(
                "text",
                match error {
                    Problem::TextTooLong => {
                        "That's too long for an update: keep it under 20,000 characters."
                    }
                    Problem::TooManyMentions => "That mentions too many people for one update.",
                    Problem::UnsupportedControlCharacter => {
                        "That has a character an update can't hold."
                    }
                    _ => "That update can't be posted as written.",
                },
            ));
        }
        let scope = idempotency::resource_bound_scope("space", space, "status_update")?;
        let thread_id = match idempotency::reserve(
            &mut tx,
            actor,
            &scope,
            &key,
            RequestHash::for_value(&json!({
                "channel": channel, "health": health, "title": title, "text": text,
            }))?,
        )
        .await?
        {
            IdempotencyOutcome::Replay(replay) => replay.decode::<uuid::Uuid>()?,
            IdempotencyOutcome::Reserved(reservation) => {
                let thread = match threads::create(
                    &mut tx,
                    &creator,
                    channel,
                    ThreadSpec::new(ThreadName::Composed(title.clone()))
                        .audited_with(json!({"projectUpdate": true})),
                )
                .await
                {
                    Ok(thread) => thread,
                    Err(crate::error::AppError::NotFound(_)) => {
                        return Err(OpError::not_found("That channel in this space"));
                    }
                    Err(error) => return Err(error.into()),
                };
                if thread.parent.kind != "channel" {
                    return Err(OpError::invalid(
                        "channelId",
                        "An update goes in one of the space's channels.",
                    ));
                }
                crate::conversations::update_thread(
                    &mut tx,
                    actor,
                    thread.id,
                    &ThreadPatch {
                        name: None,
                        status: None,
                        outcome: Some(status::health_label(health).to_owned()),
                        clear_outcome: false,
                    },
                )
                .await?;
                status::record_update(&mut tx, workspace, space, actor, thread.id, health).await?;
                emit_status_changed(&mut tx, space, actor).await?;
                idempotency::complete(
                    &mut tx,
                    &reservation,
                    axum::http::StatusCode::CREATED,
                    &thread.id,
                )
                .await?;
                thread.id
            }
        };
        tx.commit().await.map_err(crate::error::AppError::from)?;

        let posted = append_message_command_with(
            state,
            call.ctx.into(),
            thread_id,
            Some("thread"),
            key,
            content,
            &[],
            None,
            AppendExtras::default(),
        )
        .await?;
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.mark_replayed(posted.idempotent_replay);
        call.audit_target("space", space, json!({"update": thread_id}));
        let tx = call.tx().await?;
        Ok(ProjectUpdatePosted {
            thread_id: thread_id.to_string(),
            channel_id: channel.to_string(),
            message_id: posted.body.message.id.to_string(),
            status: status::status_of(tx, actor, space).await?,
        })
    }
}
