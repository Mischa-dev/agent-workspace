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
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<TaskCreate>()
        .add::<TaskUpdate>()
        .add::<TaskArchive>()
        .add::<TaskRestore>()
        .add::<TaskCommentCreate>()
        .add::<TaskCommentUpdate>()
        .add::<TaskCommentDelete>()
        .add::<TaskCriteriaUpdate>();
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
    /// that acts like a task, its own fields (see collection_describe).
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
            // A connection held to one space makes it there; anyone else
            // is asked which space.
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
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
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
        conflict, open it again and retry.";
    const EMITS: &'static [&'static str] = &["task.updated"];

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
            (None, None) => task_commands::update(call.state, call.ctx, &input).await?,
        };
        settled(call, &written);
        Ok(written)
    }
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
    #[serde(alias = "update")]
    pub text: String,
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
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
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
            input.text,
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
    pub text: String,
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
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
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
            input.text,
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
    const PERMISSION: Option<Perm> = Some(Perm::TasksEdit);
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
