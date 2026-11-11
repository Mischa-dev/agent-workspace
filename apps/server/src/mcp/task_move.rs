//! `task_move`: moving tasks between projects (docs/plans/mcp-tools.md,
//! decision 1). The move itself, its preview and its checks are shared with
//! the web app in `crate::work_item_move`; this is the tool's arguments and
//! results.
//!
//! Called with just a task and a project (as before AWD-102), it moves at
//! once: custom field values that fit carry over and the rest stay on the
//! original. With `preview: true` it changes nothing and shows what carries
//! over and what needs a choice; sending the preview's `snapshot` with
//! `resolutions` (and a `clientRequestId`) then moves with those choices, or
//! refuses if anything changed.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::{
    McpPrincipal,
    common::{ToolResult, confined_project, parse_id},
    tasks::command_actor,
    writes::parse_request_id,
};
use crate::work_item_move::{
    self, MoveDone, MovePlan, MoveRequest, MovedTask, Mover, Refusal, Resolution, Surface,
};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskMoveArguments {
    /// The task's ID, link, or key such as AW-7. Its subtasks move with it.
    pub task_id: String,
    /// Where it goes: a project ID, link, or key such as "AWD".
    pub to_project_id: String,
    /// true: change nothing, and show what carries over and what needs a
    /// choice (custom field values that don't fit the target). Then move
    /// with `snapshot` and `resolutions`.
    #[serde(default)]
    pub preview: Option<bool>,
    /// The preview's `snapshot`. The move refuses it if anything the
    /// preview showed has changed; preview again then.
    #[serde(default)]
    pub snapshot: Option<String>,
    /// One choice per issue in the preview: `{issue, action: "leave"}`, or
    /// `{issue, action: "set", targetField, value?}`.
    #[serde(default)]
    pub resolutions: Option<Vec<Resolution>>,
    /// true: leave every issue without a resolution on the original.
    #[serde(default)]
    pub leave_unresolved: Option<bool>,
    /// A stable ID, such as move-aw-7-2026-11-07, so a retry returns the
    /// first result instead of moving again. The same ID with other choices
    /// is a conflict.
    #[serde(default)]
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskMoveValue {
    /// The task first, then its subtasks. Empty for a preview.
    pub moved: Vec<MovedTask>,
    /// Anything that didn't carry over exactly, such as an assignee who isn't
    /// in the target project. Only you read these, once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// False when the task had already been moved (this is where it went),
    /// and for a preview.
    pub created: bool,
    /// True when this is the result of the same request, sent again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
    /// How many custom field values stayed on the original only.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub left_behind: u32,
    /// With `preview: true`: what would happen. Nothing changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<MovePlan>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

const REQUEST_ID_MESSAGE: &str = "Use a clientRequestId of 1 to 128 letters, digits, dashes, dots or underscores, and send the same one when you retry.";

pub(super) async fn task_move(
    state: &crate::AppState,
    principal: &McpPrincipal,
    arguments: TaskMoveArguments,
) -> ToolResult<TaskMoveValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(task_id) = parse_id(&arguments.task_id) else {
        return ToolResult::not_found("That task");
    };
    let Some(target_project_id) = parse_id(&arguments.to_project_id) else {
        return ToolResult::not_found("That target project");
    };
    let source_project_id = principal.project_id;
    let mover = Mover {
        actor_id: principal.actor_id,
        external_agent_connection_id: principal.external_agent_connection_id,
        via_grant: principal.via_grant_id(),
        confined_to: confined_project(principal),
        source: command_actor(principal, source_project_id),
        target: command_actor(principal, target_project_id),
        surface: Surface::Mcp,
    };
    if arguments.preview == Some(true) {
        if arguments.snapshot.is_some()
            || arguments.resolutions.is_some()
            || arguments.leave_unresolved.is_some()
        {
            return ToolResult::invalid(
                "preview",
                "A preview takes only the task and the target. Send snapshot and resolutions to move.",
            );
        }
        return match work_item_move::preview(
            state,
            &mover,
            source_project_id,
            task_id,
            target_project_id,
        )
        .await
        {
            Ok(Ok(plan)) => ToolResult::ok(TaskMoveValue {
                moved: Vec::new(),
                notes: Vec::new(),
                created: false,
                replayed: false,
                left_behind: 0,
                preview: Some(plan),
            }),
            Ok(Err(refusal)) => refused(refusal),
            Err(error) => ToolResult::from_error(error, "task_move"),
        };
    }
    let idempotency_key = match arguments.client_request_id.as_deref() {
        Some(value) => match parse_request_id(value) {
            Some(key) => Some(key),
            None => return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE),
        },
        None => None,
    };
    let request = MoveRequest {
        task_id,
        target_project_id,
        snapshot: arguments.snapshot,
        resolutions: arguments.resolutions.unwrap_or_default(),
        leave_unresolved: arguments.leave_unresolved == Some(true),
        idempotency_key,
    };
    match work_item_move::execute(state, &mover, source_project_id, request).await {
        Ok(Ok(done)) => ToolResult::ok(value_of(done)),
        Ok(Err(refusal)) => refused(refusal),
        Err(error) => ToolResult::from_error(error, "task_move"),
    }
}

fn value_of(done: MoveDone) -> TaskMoveValue {
    TaskMoveValue {
        moved: done.moved,
        notes: done.notes,
        created: done.created,
        replayed: done.replayed,
        left_behind: done.left_behind,
        preview: None,
    }
}

fn refused(refusal: Refusal) -> ToolResult<TaskMoveValue> {
    match refusal {
        Refusal::NotFound(what) => ToolResult::not_found(what),
        Refusal::Denied(message) => ToolResult::denied(message),
        Refusal::Invalid { field, message } => ToolResult::invalid(&field, message),
        Refusal::Conflict(message) => ToolResult::conflict(message),
        Refusal::ModuleOff => ToolResult::module_off("task_move", "Tasks"),
    }
}
