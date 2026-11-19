//! Task reads as operations (K2.15): the task list, a task, what's ready,
//! the dependency graph and my work. Each runs the read its hand-written
//! tool ran, as the caller the dispatcher checked, held to the workspace
//! the call acts in. `task_list` filters through the web list engine's
//! `work_items::list::Filters`, so the web list and agents see the same
//! tasks for the same filter.

use uuid::Uuid;

use crate::{
    AppState,
    mcp::{
        McpPrincipal, ai_tools,
        task_commands::{principal_for, tool_answer},
        tasks, views_data,
    },
    ops::{
        Idempotency, OpCall, OpContext, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::parse_id,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<TaskList>()
        .add::<TaskGet>()
        .add::<TaskReadyList>()
        .add::<DependencyGraphGet>()
        .add::<MyWorkGet>();
}

/// The caller as a read reads it: in the space the call named, otherwise
/// across the projects it reaches in the call's workspace (every workspace
/// for a spanning read). A credential held to one space reads from there,
/// as the hand-written tools did: its space isn't one the call named. A
/// connection that names no space keeps its credential's own project as
/// its anchor (what an agent's reply inbox is read through), unnamed.
pub(crate) async fn reader(state: &AppState, ctx: &OpContext) -> Result<McpPrincipal, OpError> {
    let mut principal = principal_for(ctx, None);
    principal.project_explicit =
        principal.project_id != Uuid::nil() && ctx.space_id != ctx.caller.confined_space;
    if principal.project_id.is_nil()
        && let Some(credential) = principal.mcp_credential_id
    {
        let anchor = sqlx::query_scalar::<_, Uuid>(
            "SELECT project_id FROM account_credentials WHERE id = $1 AND actor_id = $2",
        )
        .bind(credential)
        .bind(principal.actor_id)
        .fetch_optional(crate::db::scoped(&state.pool))
        .await
        .map_err(|error| OpError::logged(ctx.op, &error.into()))?;
        if let Some(anchor) = anchor {
            principal.project_id = anchor;
        }
    }
    Ok(principal)
}

/// A space named by id, key or link (the dispatcher turns a key into the
/// id first); none means the workspace.
fn space_or_workspace(project_id: Option<&str>) -> Target {
    match project_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(project) => Target::Space(parse_id(project).unwrap_or_else(Uuid::nil)),
        None => Target::Workspace(None),
    }
}

macro_rules! read_op {
    (
        $op:ident {
            name: $name:literal,
            kind: $kind:ident,
            permission: $perm:expr,
            title: $title:literal,
            description: $description:expr,
            input: $input:ty,
            output: $output:ty,
            run: $command:path
        }
    ) => {
        pub(crate) struct $op;

        impl Operation for $op {
            const NAME: &'static str = $name;
            const KIND: OpKind = OpKind::$kind;
            const MODULE: &'static str = "tasks";
            const PERMISSION: Option<Perm> = $perm;
            const IDEMPOTENCY: Idempotency = Idempotency::None;
            const HELD_SPACE_BY_DEFAULT: bool = true;
            const SPANS_WORKSPACES: bool = true;
            const TITLE: &'static str = $title;
            const DESCRIPTION: &'static str = $description;

            type Input = $input;
            type Output = $output;

            async fn run(call: &mut OpCall<'_>, input: $input) -> Result<$output, OpError> {
                let principal = reader(call.state, call.ctx).await?;
                tool_answer($command(call.state, &principal, input).await)
            }
        }
    };
}

impl Targeted for tasks::TaskListArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

read_op!(TaskList {
    name: "task_list",
    kind: Query,
    permission: Some(Perm::TasksView),
    title: "List tasks",
    description: "List tasks, in board order by default (the order people set; boardPosition \
        1 is the top of its column). Filter by space (projectId, or several with projectIds), \
        status, assignee (me, none, @handle), labels (tasks with all of them; anyLabels for any \
        of them), type, priority, milestone, createdBy, updatedSince (a date, a timestamp, or \
        24h/7d), blocked, claimed (yes, no, me, @handle), due dates, words, or parent. Status, \
        assignee, type, priority and milestone take several values (any of them), and \
        notStatus, notAssignee, notLabels, notType, notPriority and notMilestone leave tasks \
        out. Done and canceled tasks are left out unless includeDone or you name their status, \
        and archived ones unless includeArchived. view lists a saved view's tasks (view_list). \
        total counts every match; for the next page, pass the nextCursor you got as cursor.",
    input: tasks::TaskListArguments,
    output: tasks::TaskListValue,
    run: tasks::task_list
});

impl Targeted for tasks::TaskOpenArguments {
    fn target(&self) -> Target {
        Target::thing("task", parse_id(&self.task_id).unwrap_or_else(Uuid::nil))
    }
}

read_op!(TaskGet {
    name: "task_get",
    kind: Describe,
    permission: Some(Perm::TasksView),
    title: "Open task",
    description: "Open a task by ID, link or key (such as AW-12): its fields and labels, \
        acceptance criteria, who's working on it, subtasks, linked threads, messages, pages, \
        decisions and pull requests, and the latest comments. Pass include: [\"history\"] for \
        the task's full timeline: who changed what, when, and through which app (latest 30 by \
        default; olderCursor for more).",
    input: tasks::TaskOpenArguments,
    output: tasks::TaskDetail,
    run: tasks::task_open
});

impl Targeted for ai_tools::TaskReadyListArguments {
    fn target(&self) -> Target {
        space_or_workspace(Some(&self.project_id))
    }
}

read_op!(TaskReadyList {
    name: "task_ready_list",
    kind: Query,
    permission: Some(Perm::TasksView),
    title: "List tasks ready to start",
    description: "What to work on next in a space: open tasks ranked by priority, how many \
        tasks each unblocks, and how close its milestone or due date is, each with readiness \
        checks (blockers done, not marked blocked, acceptance criteria, a description, a \
        repository). scope agents lists only tasks an agent can pick up now. taskId returns one \
        task's checks.",
    input: ai_tools::TaskReadyListArguments,
    output: serde_json::Value,
    run: ai_tools::task_ready_list
});

impl Targeted for ai_tools::DependencyGraphArguments {
    fn target(&self) -> Target {
        space_or_workspace(Some(&self.project_id))
    }
}

read_op!(DependencyGraphGet {
    name: "dependency_graph_get",
    kind: Describe,
    permission: Some(Perm::TasksView),
    title: "Dependency graph",
    description: "A space's dependency graph: open tasks and what blocks them (blockers in \
        other spaces you can see included), the tasks that can start now (frontier, best \
        first), the longest chain of open work by estimate (criticalPath), and suggested \
        blocks waiting for review as edges with a suggestionId. Narrow by milestone, label or \
        parent task.",
    input: ai_tools::DependencyGraphArguments,
    output: serde_json::Value,
    run: ai_tools::dependency_graph
});

impl Targeted for views_data::MyWorkArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

read_op!(MyWorkGet {
    name: "my_work_get",
    kind: Describe,
    permission: None,
    title: "My work",
    description: "Your tasks across the workspace (or one space): assigned to you and \
        unfinished (due first), being worked on by you or agents you set up, and in review \
        where you made or watch them.",
    input: views_data::MyWorkArguments,
    output: views_data::MyWorkValue,
    run: views_data::my_work
});
