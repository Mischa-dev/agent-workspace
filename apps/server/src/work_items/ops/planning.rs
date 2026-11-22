//! Tasks' planning, links and bulk writes as operations (K2.14): ordering,
//! links of every kind (dependencies, relations, pull requests and the
//! records a task came from), watching, moving, labels, fields, views,
//! milestones, templates and schedules, imports, and the bulk tools.
//!
//! Each runs the command its hand-written tool ran (`mcp::*`), with the
//! caller the dispatcher checked: the same checks inside, the same answers
//! inside the envelope. The `_write` tools became a create and an update
//! each (their old names stay as aliases, `ops::aliases`). Until K3.15's
//! `link_create` lands, `task_link` writes today's link tables.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    mcp::{
        ai_tools, bug_report_tools, field_tools, labels, planning,
        task_batch::{self, CreateManyValue, UpdateManyValue},
        task_commands::{batch_answer, command_request_id, principal_for, tool_answer},
        task_move, tasks, views_data,
    },
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<TaskReorder>()
        .add::<TaskLink>()
        .add::<TaskLinkMany>()
        .add::<TaskWatchSet>()
        .add::<TaskMove>()
        .add::<TaskCreateMany>()
        .add::<TaskUpdateMany>()
        .add::<LabelList>()
        .add::<LabelCreate>()
        .add::<LabelUpdate>()
        .add::<FieldList>()
        .add::<FieldCreate>()
        .add::<FieldUpdate>()
        .add::<MilestoneList>()
        .add::<MilestoneCreate>()
        .add::<MilestoneUpdate>()
        .add::<TemplateList>()
        .add::<TemplateCreate>()
        .add::<TemplateUpdate>()
        .add::<RecurringCreate>()
        .add::<RecurringUpdate>()
        .add::<TaskImport>()
        .add::<PlanImport>()
        .add::<BugReportLinkList>()
        .add::<BugReportLinkCreate>()
        .add::<BugReportLinkUpdate>();
}

/// One operation over a tool's command: its constants and its `run`.
macro_rules! tool_op {
    (
        $op:ident {
            name: $name:literal,
            kind: $kind:ident,
            permission: $perm:expr,
            idempotency: $idem:ident,
            $(preview: $preview:literal,)?
            $(destructive: $destructive:literal,)?
            $(long: |$long_input:ident| $long_body:expr,)?
            $(shown_once: [$($shown:literal),*],)?
            $(max_body_bytes: $max_body:expr,)?
            title: $title:literal,
            description: $description:expr,
            emits: [$($emit:literal),*],
            input: $input:ty,
            output: $output:ty,
            run: |$call:ident, $arg:ident| $body:block
        }
    ) => {
        pub(crate) struct $op;

        impl Operation for $op {
            const NAME: &'static str = $name;
            const KIND: OpKind = OpKind::$kind;
            const MODULE: &'static str = "tasks";
            const PERMISSION: Option<Perm> = $perm;
            const IDEMPOTENCY: Idempotency = Idempotency::$idem;
            $(const PREVIEW: bool = $preview;)?
            $(const DESTRUCTIVE: bool = $destructive;)?
            $(
                const LONG: bool = true;
                fn runs_long($long_input: &$input) -> bool {
                    $long_body
                }
            )?
            $(const SHOWN_ONCE: &'static [&'static str] = &[$($shown),*];)?
            $(const MAX_BODY_BYTES: usize = $max_body;)?
            const TITLE: &'static str = $title;
            const DESCRIPTION: &'static str = $description;
            const EMITS: &'static [&'static str] = &[$($emit),*];

            type Input = $input;
            type Output = $output;

            async fn run($call: &mut OpCall<'_>, $arg: $input) -> Result<$output, OpError> $body
        }
    };
}

/// K2.21: a batch over this many tasks (twice it for a change) runs as a
/// job when an MCP client calls it; a smaller one answers at once.
const LONG_BATCH: usize = 25;

/// The space a project-scoped input names (`projectId`: an id, key or
/// link; the dispatcher turns a key into the id first).
fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(Uuid::nil))
}

/// A task an input names by id or link (the dispatcher turns a key into the
/// id first).
fn task(task_id: &str) -> Target {
    Target::thing("task", parse_id(task_id).unwrap_or_else(Uuid::nil))
}

/// The space a call that may name none acts in: the one it located, or the
/// one its credential is held to.
fn acting_space(call: &OpCall<'_>) -> Result<Option<Uuid>, OpError> {
    match (call.ctx.space_id, call.ctx.caller.confined_space) {
        (Some(_), _) => Ok(None),
        (None, Some(confined)) => Ok(Some(confined)),
        (None, None) => Err(OpError::invalid(
            "projectId",
            "Say which space with projectId (see project_list).",
        )),
    }
}

/// Runs a tool's command as the caller, in `space` when the call located
/// none.
macro_rules! tool {
    ($call:ident, $space:expr, $command:path, $arguments:expr) => {{
        let principal = principal_for($call.ctx, $space);
        tool_answer($command($call.state, &principal, $arguments).await)
    }};
}

/// A `_write` tool split in two: a create names no existing thing, an
/// update does.
fn names_existing(found: bool, create: bool, field: &'static str) -> Result<(), OpError> {
    match (create, found) {
        (true, true) => Err(OpError::invalid(
            field,
            "This makes a new one: leave this out, or use the update operation.",
        )),
        (false, false) => Err(OpError::invalid(
            field,
            "Say which one to change (its name or ID).",
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Targets of the tools' arguments

impl Targeted for tasks::TaskReorderArguments {
    fn target(&self) -> Target {
        self.task_ids
            .first()
            .map_or(Target::Workspace(None), |first| task(first))
    }
}

impl Targeted for planning::TaskWatchArguments {
    fn target(&self) -> Target {
        task(&self.task_id)
    }
}

impl Targeted for task_move::TaskMoveArguments {
    fn target(&self) -> Target {
        task(&self.task_id)
    }
}

impl Targeted for task_batch::TaskCreateManyArguments {
    fn target(&self) -> Target {
        self.project_id
            .as_deref()
            .map_or(Target::Workspace(None), space)
    }
}

impl Targeted for task_batch::TaskUpdateManyArguments {
    fn target(&self) -> Target {
        match (&self.project_id, self.task_ids.first()) {
            (Some(project), _) => space(project),
            (None, Some(first)) => task(first),
            (None, None) => Target::Workspace(None),
        }
    }
}

macro_rules! in_space {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    space(&self.project_id)
                }
            }
        )*
    };
}

in_space!(
    labels::LabelListArguments,
    labels::LabelWriteArguments,
    field_tools::FieldListArguments,
    field_tools::FieldWriteArguments,
    views_data::TemplateListArguments,
    views_data::TemplateWriteArguments,
    views_data::RecurringWriteArguments,
    views_data::TaskImportArguments,
    planning::MilestoneListArguments,
    planning::MilestoneWriteArguments,
    ai_tools::PlanImportArguments,
    bug_report_tools::BugReportLinkListArguments,
    bug_report_tools::BugReportLinkWriteArguments,
);

// ---------------------------------------------------------------------------
// task_reorder

tool_op!(TaskReorder {
    name: "task_reorder",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    title: "Reorder tasks",
    description: "Put tasks in order on the board, all from one status column. Pass them in the \
        order you want, then say where: position top (the default) or bottom, or beforeTaskId / \
        afterTaskId to place them next to another task in that column. Calling it again with the \
        same arguments changes nothing.",
    emits: ["task.updated"],
    input: tasks::TaskReorderArguments,
    output: tasks::ReorderValue,
    run: |call, input| { tool!(call, None, tasks::task_reorder, input) }
});

// ---------------------------------------------------------------------------
// task_link (aliases task_depend, task_relate, task_link_pull_request)

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskLinkInput {
    /// The task: its ID, link, or key such as AWD-12.
    #[serde(alias = "workItemId")]
    pub task_id: String,
    /// Tasks that must be done before this one (keys or IDs). They may be in
    /// another space of the same workspace that you can reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    /// Tasks that can't start until this one is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<String>>,
    /// Another task this one relates to or duplicates (ID, link, or key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_task_id: Option<String>,
    /// With otherTaskId: relates_to (the default) or duplicates (this task
    /// duplicates the other one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// With kind duplicates: also close this task in the workflow's canceled
    /// status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<bool>,
    /// A thread or conversation about this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// A message this task came from: its ID, or a link to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// A page in the task's space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    /// A decision in the task's space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// A repository in the task's space: its ID, link, or owner/name (or
    /// just its name when only one repository there has it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    /// A pull request Agent Workspace knows about: its ID, link, or URL.
    /// With the space's code automation on, a linked pull request moves the
    /// task when it's opened or merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_id: Option<String>,
    /// A whiteboard: it shows on the task. One board can be on many tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whiteboard_id: Option<String>,
    /// For repositoryId only: related or primary, the repository runs on
    /// this task's thread use. Left out, a new link is related and an
    /// existing link keeps its role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The version you last read. Optional; when given, a change of a
    /// record link is refused if the task has changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// Remove the link instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
}

impl Targeted for TaskLinkInput {
    fn target(&self) -> Target {
        task(&self.task_id)
    }
}

/// What a link changed: the task's dependencies, its relations, or one
/// record link.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum TaskLinked {
    Dependencies(planning::DependencyValue),
    Relations(planning::RelationsValue),
    Link(tasks::LinkValue),
}

tool_op!(TaskLink {
    name: "task_link",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    destructive: true,
    title: "Link task",
    description: "Link a task to other tasks or to where it came from. Dependencies: blockedBy \
        (tasks that must finish first) or blocks; a link that would make a cycle is refused with \
        the path. Relations: otherTaskId with kind relates_to (the default) or duplicates (close: \
        true also closes this one as canceled). Records: a thread or conversation, a message, a \
        page, a decision, a whiteboard, a repository (role primary or related), or a pull request \
        Agent Workspace already knows about. remove: true unlinks. Linking twice changes nothing. \
        Tasks may be in another space of the same workspace that you can reach.",
    emits: ["task.updated"],
    input: TaskLinkInput,
    output: TaskLinked,
    run: |call, input| {
        let dependencies = input.blocked_by.is_some() || input.blocks.is_some();
        let records = [
            &input.conversation_id,
            &input.message_id,
            &input.page_id,
            &input.decision_id,
            &input.repository_id,
            &input.pull_request_id,
            &input.whiteboard_id,
        ]
        .iter()
        .filter(|value| value.is_some())
        .count();
        let kinds = usize::from(dependencies)
            + usize::from(input.other_task_id.is_some())
            + usize::from(records > 0);
        if kinds > 1 {
            return Err(OpError::invalid(
                "taskId",
                "Link one kind of thing per call: dependencies (blockedBy, blocks), a relation \
                 (otherTaskId), or a record.",
            ));
        }
        if dependencies {
            let arguments = planning::TaskDependArguments {
                task_id: input.task_id,
                blocked_by: input.blocked_by,
                blocks: input.blocks,
                remove: input.remove,
            };
            return tool!(call, None, planning::task_depend, arguments)
                .map(TaskLinked::Dependencies);
        }
        if let Some(other_task_id) = input.other_task_id {
            let arguments = planning::TaskRelateArguments {
                task_id: input.task_id,
                other_task_id,
                kind: input.kind,
                remove: input.remove,
                close: input.close,
            };
            return tool!(call, None, planning::task_relate, arguments).map(TaskLinked::Relations);
        }
        if input.kind.is_some() || input.close.is_some() {
            return Err(OpError::invalid(
                "kind",
                "kind and close go with otherTaskId.",
            ));
        }
        let arguments = tasks::TaskLinkArguments {
            task_id: input.task_id,
            conversation_id: input.conversation_id,
            message_id: input.message_id,
            page_id: input.page_id,
            decision_id: input.decision_id,
            repository_id: input.repository_id,
            pull_request_id: input.pull_request_id,
            whiteboard_id: input.whiteboard_id,
            role: input.role,
            expected_version: input.expected_version,
            remove: input.remove,
        };
        tool!(call, None, tasks::task_link, arguments).map(TaskLinked::Link)
    }
});

// ---------------------------------------------------------------------------
// task_link_many (AWD-168: many dependencies in one call)

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskLinkManyInput {
    /// The space the tasks are in: its id, key or link. Left out, the space
    /// of the first task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Up to 100 tasks, each with what blocks it (blockedBy) or what it
    /// blocks (blocks), by key or ID.
    pub links: Vec<planning::DependencyLinks>,
    /// True to remove these dependencies instead of adding them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
}

impl Targeted for TaskLinkManyInput {
    fn target(&self) -> Target {
        match (&self.project_id, self.links.first()) {
            (Some(project), _) => space(project),
            (None, Some(first)) => task(&first.task_id),
            (None, None) => Target::Workspace(None),
        }
    }
}

tool_op!(TaskLinkMany {
    name: "task_link_many",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    title: "Link many tasks",
    description: "Write many dependencies in one call, all or nothing: up to 100 tasks of one \
        space, each with blockedBy (tasks that must finish first) and blocks (tasks waiting for \
        it), by key or ID. remove: true takes them away. A link that would make a cycle refuses \
        the whole call, naming the task (details.index) and the path. Returns each task's key, \
        version and how many links changed.",
    emits: ["task.updated"],
    input: TaskLinkManyInput,
    output: planning::DependencyManyValue,
    run: |call, input| {
        let principal = principal_for(call.ctx, acting_space(call)?);
        tool_answer(
            planning::task_depend_many(
                call.state,
                &principal,
                input.links,
                input.remove == Some(true),
            )
            .await,
        )
    }
});

// ---------------------------------------------------------------------------
// task_watch_set (alias task_watch)

tool_op!(TaskWatchSet {
    name: "task_watch_set",
    kind: Change,
    permission: Some(Perm::TasksView),
    idempotency: Dispatcher,
    title: "Watch task",
    description: "Watch a task to hear about its changes, or stop (watching: false). Returns who \
        watches it.",
    emits: ["task.updated"],
    input: planning::TaskWatchArguments,
    output: planning::WatchValue,
    run: |call, input| { tool!(call, None, planning::task_watch, input) }
});

// ---------------------------------------------------------------------------
// task_move

tool_op!(TaskMove {
    name: "task_move",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Command,
    preview: true,
    destructive: true,
    title: "Move task",
    description: "Move a task, with its subtasks, to another space. The new task gets the title, \
        description, priority, status and type (matched by name), the assignee if they're in \
        that space, acceptance criteria, labels (by name; missing ones are created there), and \
        every comment quoted with its author and date. The original is archived with its full \
        history and links, and its old key (such as AW-7) opens the new task. Custom field \
        values carry over into a field of the same name and type when they fit there; the rest \
        stay on the original. Call first with preview: true to see what carries over and what \
        needs a choice, then move with its snapshot and one resolution per issue (set it in a \
        target field, or leave it). Moving it again returns where it went.",
    emits: ["task.created", "task.updated"],
    input: task_move::TaskMoveArguments,
    output: task_move::TaskMoveValue,
    run: |call, input| {
        let mut input = input;
        input.preview = Some(call.ctx.preview);
        input.client_request_id = (!call.ctx.preview).then(|| command_request_id(call.ctx));
        if call.ctx.request_id.is_some() && !call.ctx.preview {
            call.key_honoured();
        }
        call.audited_by_command();
        tool!(call, None, task_move::task_move, input)
    }
});

// ---------------------------------------------------------------------------
// task_create_many, task_update_many

tool_op!(TaskCreateMany {
    name: "task_create_many",
    kind: Create,
    permission: Some(Perm::TasksCreate),
    idempotency: Command,
    preview: true,
    long: |input| input.tasks.len() > LONG_BATCH,
    title: "Create many tasks",
    description: "Create up to 100 tasks in one space at once, all or nothing, such as a plan \
        turned into tasks. Each takes a title and optionally a description, priority, status and \
        type (by name), assignee, labels, acceptanceCriteria, milestone, estimate, startDate and \
        dueDate, and blockedByRefs (refs of earlier tasks in the list, or keys of existing \
        tasks). Make subtasks with parentRef (the ref of an earlier task in the list) or parentId \
        (an existing task such as AWD-12). Send preview: true first to check everything and see \
        the keys they'd get, without creating anything. createMissingLabels: true creates labels \
        the space doesn't have yet. If anything is wrong, nothing is created and each problem is \
        listed (details.current.problems, each with its index). Keep one call under about 1 MB.",
    emits: ["task.created"],
    input: task_batch::TaskCreateManyArguments,
    output: CreateManyValue,
    run: |call, input| {
        let mut input = input;
        // A retry without one would make every task twice.
        if call.ctx.request_id.is_none() && !call.ctx.preview {
            return Err(OpError::invalid(
                "clientRequestId",
                "Pass a clientRequestId, such as plan-import-2026-10-01, so a retry can't create \
                 these tasks twice.",
            ));
        }
        input.preview = Some(call.ctx.preview);
        input.client_request_id = Some(command_request_id(call.ctx));
        let space = if input.project_id.is_none() {
            acting_space(call)?
        } else {
            None
        };
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.audited_by_command();
        let principal = principal_for(call.ctx, space);
        batch_answer(task_batch::task_create_many(call.state, &principal, input).await)
    }
});

tool_op!(TaskUpdateMany {
    name: "task_update_many",
    kind: Change,
    // Archiving alone needs only "archive tasks"; the command checks each
    // change against what it does.
    permission: None,
    idempotency: Command,
    preview: true,
    destructive: true,
    long: |input| input.task_ids.len() > 2 * LONG_BATCH,
    title: "Update many tasks",
    description: "Apply one change to up to 200 tasks in one space, all or nothing: a status \
        (each task must be allowed to move there directly), priority, assignee, type, labels to \
        add or remove, milestone, due date, field values, or archived (true archives, false \
        restores; pass it on its own). Pass the versions you read as expectedVersions to stop if \
        someone changed a task since. If any task can't take the change, nothing changes and each \
        problem is listed (details.current.problems) with what to do. preview: true shows the \
        result without changing anything.",
    emits: ["task.updated"],
    input: task_batch::TaskUpdateManyArguments,
    output: UpdateManyValue,
    run: |call, input| {
        let mut input = input;
        input.preview = Some(call.ctx.preview);
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.audited_by_command();
        batch_answer(
            task_batch::task_update_many(call.state, &principal_for(call.ctx, None), input).await,
        )
    }
});

// ---------------------------------------------------------------------------
// Labels

tool_op!(LabelList {
    name: "label_list",
    kind: Query,
    permission: Some(Perm::TasksView),
    idempotency: None,
    title: "List labels",
    description: "List a space's labels by name, with each one's color, description, how many \
        tasks carry it, and the version label_update needs. Archived labels are left out unless \
        includeArchived.",
    emits: [],
    input: labels::LabelListArguments,
    output: labels::LabelListValue,
    run: |call, input| { tool!(call, None, labels::label_list, input) }
});

tool_op!(LabelCreate {
    name: "label_create",
    kind: Create,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    title: "Create label",
    description: "Create a label in a space: its name, and optionally a color and description. \
        Creating a name that already exists returns that label unchanged.",
    emits: ["label.created"],
    input: labels::LabelWriteArguments,
    output: labels::LabelWriteValue,
    run: |call, input| {
        names_existing(input.label.is_some(), true, "label")?;
        tool!(call, None, labels::label_write, input)
    }
});

tool_op!(LabelUpdate {
    name: "label_update",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    destructive: true,
    title: "Update label",
    description: "Change a label (label: its name or ID): rename, recolor, describe, archive it \
        (archived: true) or restore it (archived: false). Archiving hides it from every task \
        until it's restored; nothing is deleted.",
    emits: ["label.updated"],
    input: labels::LabelWriteArguments,
    output: labels::LabelWriteValue,
    run: |call, input| {
        names_existing(input.label.is_some(), false, "label")?;
        tool!(call, None, labels::label_write, input)
    }
});

// ---------------------------------------------------------------------------
// Fields

tool_op!(FieldList {
    name: "field_list",
    kind: Query,
    permission: Some(Perm::TasksView),
    idempotency: None,
    title: "List fields",
    description: "List a space's task fields (custom fields such as Risk or Customer): each \
        one's name, type (text, long_text, number, date, single_choice, multiple_choice, person, \
        url, email, phone, checkbox, link), options, and the version field_update needs. Set \
        values with task_update's fields, by name; filter with task_list's fieldFilters. \
        Archived fields are left out unless includeArchived.",
    emits: [],
    input: field_tools::FieldListArguments,
    output: field_tools::FieldListValue,
    run: |call, input| { tool!(call, None, field_tools::field_list, input) }
});

tool_op!(FieldCreate {
    name: "field_create",
    kind: Create,
    permission: Some(Perm::TasksWorkflow),
    idempotency: Dispatcher,
    title: "Create field",
    description: "Add a task field to a space: its name, type (text, long_text, number, date, \
        single_choice, multiple_choice, person, url, email, phone, checkbox, link), description, \
        position and a choice field's options. A field's type never changes later.",
    emits: ["field.created"],
    input: field_tools::FieldWriteArguments,
    output: field_tools::FieldWriteValue,
    run: |call, input| {
        names_existing(input.field.is_some(), true, "field")?;
        tool!(call, None, field_tools::field_write, input)
    }
});

tool_op!(FieldUpdate {
    name: "field_update",
    kind: Change,
    permission: Some(Perm::TasksWorkflow),
    idempotency: Dispatcher,
    destructive: true,
    title: "Update field",
    description: "Change a task field (field: its name or ID): rename, describe, reorder \
        (position), change a choice field's options, archive it (archived: true) or restore it \
        (archived: false). Archiving hides it everywhere and keeps every task's value.",
    emits: ["field.updated"],
    input: field_tools::FieldWriteArguments,
    output: field_tools::FieldWriteValue,
    run: |call, input| {
        names_existing(input.field.is_some(), false, "field")?;
        tool!(call, None, field_tools::field_write, input)
    }
});

// ---------------------------------------------------------------------------
// Milestones

tool_op!(MilestoneList {
    name: "milestone_list",
    kind: Query,
    permission: Some(Perm::TasksView),
    idempotency: None,
    title: "List milestones",
    description: "A space's milestones: target dates, how many tasks are finished, and which are \
        overdue.",
    emits: [],
    input: planning::MilestoneListArguments,
    output: planning::MilestoneListValue,
    run: |call, input| { tool!(call, None, planning::milestone_list, input) }
});

tool_op!(MilestoneCreate {
    name: "milestone_create",
    kind: Create,
    permission: Some(Perm::TasksEdit),
    idempotency: Command,
    title: "Create milestone",
    description: "Create a milestone (name, description, targetDate) and put tasks in it \
        (addTasks). Creating one whose name exists returns that one.",
    emits: ["milestone.created"],
    input: planning::MilestoneWriteArguments,
    output: planning::MilestoneValue,
    run: |call, input| {
        names_existing(input.milestone_id.is_some(), true, "milestoneId")?;
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.audited_by_command();
        tool!(call, None, planning::milestone_write, input)
    }
});

tool_op!(MilestoneUpdate {
    name: "milestone_update",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Command,
    title: "Update milestone",
    description: "Change a milestone (milestoneId): name, description, targetDate, archived; put \
        tasks in it (addTasks) or take them out (removeTasks).",
    emits: ["milestone.updated"],
    input: planning::MilestoneWriteArguments,
    output: planning::MilestoneValue,
    run: |call, input| {
        names_existing(input.milestone_id.is_some(), false, "milestoneId")?;
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        call.audited_by_command();
        tool!(call, None, planning::milestone_write, input)
    }
});

// ---------------------------------------------------------------------------
// Templates and schedules

tool_op!(TemplateList {
    name: "template_list",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List task templates",
    description: "A space's task templates (title, description, labels, acceptance criteria and \
        subtasks a new task starts with) and the schedules that make tasks from them.",
    emits: [],
    input: views_data::TemplateListArguments,
    output: views_data::TemplateListValue,
    run: |call, input| { tool!(call, None, views_data::template_list, input) }
});

tool_op!(TemplateCreate {
    name: "template_create",
    kind: Create,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    title: "Create task template",
    description: "Make a task template in a space: name, title, description, type, priority, \
        labels, acceptance criteria, subtasks, estimate. Make tasks from it with \
        task_from_template, or on a schedule with recurring_create.",
    emits: ["template.created"],
    input: views_data::TemplateWriteArguments,
    output: views_data::TemplateWriteValue,
    run: |call, input| {
        names_existing(input.template.is_some(), true, "template")?;
        tool!(call, None, views_data::template_write, input)
    }
});

tool_op!(TemplateUpdate {
    name: "template_update",
    kind: Change,
    permission: Some(Perm::TasksEdit),
    idempotency: Dispatcher,
    destructive: true,
    title: "Update task template",
    description: "Change a task template (template: its name or ID). Fields you leave out keep \
        what the template has. archived: true retires it and pauses schedules that use it.",
    emits: ["template.updated"],
    input: views_data::TemplateWriteArguments,
    output: views_data::TemplateWriteValue,
    run: |call, input| {
        names_existing(input.template.is_some(), false, "template")?;
        tool!(call, None, views_data::template_write, input)
    }
});

tool_op!(RecurringCreate {
    name: "recurring_create",
    kind: Create,
    permission: Some(Perm::TasksCreate),
    idempotency: Dispatcher,
    title: "Schedule a template",
    description: "Schedule a task template to make a task daily, weekly or monthly (template, \
        frequency, and when). A schedule acts as whoever last set it up, so the tasks it makes \
        are created as you.",
    emits: ["recurring.created"],
    input: views_data::RecurringWriteArguments,
    output: views_data::RecurringWriteValue,
    run: |call, input| {
        names_existing(input.recurring.is_some(), true, "recurring")?;
        // In the call's transaction: the schedule commits with its request id.
        let principal = principal_for(call.ctx, None);
        let state = call.state;
        tool_answer(
            views_data::recurring_write_in(state, Some(call.tx().await?), &principal, input).await,
        )
    }
});

tool_op!(RecurringUpdate {
    name: "recurring_update",
    kind: Change,
    permission: Some(Perm::TasksCreate),
    idempotency: Dispatcher,
    destructive: true,
    title: "Update schedule",
    description: "Change, pause (enabled: false), restart or delete a template's schedule \
        (recurring: its ID from template_list). Fields you leave out keep what the schedule has. \
        The schedule then acts as you.",
    emits: ["recurring.updated"],
    input: views_data::RecurringWriteArguments,
    output: views_data::RecurringWriteValue,
    run: |call, input| {
        names_existing(input.recurring.is_some(), false, "recurring")?;
        tool!(call, None, views_data::recurring_write, input)
    }
});

// ---------------------------------------------------------------------------
// Imports

tool_op!(TaskImport {
    name: "task_import",
    kind: Create,
    permission: Some(Perm::TasksCreate),
    idempotency: Dispatcher,
    max_body_bytes: crate::work_items::imports::MAX_IMPORT_BODY_BYTES,
    title: "Import tasks",
    description: "Import tasks from Markdown, CSV (Linear and Jira exports work as they are) or a \
        connected repository's issues. It previews first: each row's status, labels, assignee \
        and milestone, and anything that doesn't match. Call again with the importId to map its \
        columns again, change its options or cancel it, and with commit: true to create them. \
        Rows imported before from the same source are skipped.",
    emits: ["task.created"],
    input: views_data::TaskImportArguments,
    output: views_data::TaskImportValue,
    run: |call, input| { tool!(call, None, views_data::task_import, input) }
});

tool_op!(PlanImport {
    name: "plan_import",
    kind: Create,
    permission: Some(Perm::TasksCreate),
    idempotency: Dispatcher,
    title: "Import a plan",
    description: "Turn a plan into milestones, tasks and dependencies. Start with title and plan \
        (Markdown or text, or JSON in the draft shape or a roadmap with workstreams and items, \
        read as is) or pageId; you get a draft to review, where each item already in the space \
        (by key, or by the same title) is matched, listed under updates, and will be updated \
        instead of made twice. \
        Then call again with importId, commit: true and expectedVersion to create and update it \
        all, in batches that resume where they stopped. importId alone reads an import's \
        progress.",
    emits: ["task.created", "task.updated"],
    input: ai_tools::PlanImportArguments,
    output: serde_json::Value,
    run: |call, input| { tool!(call, None, ai_tools::plan_import, input) }
});

// ---------------------------------------------------------------------------
// Bug report links

tool_op!(BugReportLinkList {
    name: "bug_report_link_list",
    kind: Query,
    // Reading tasks or editing collections (forms add records) opens it;
    // the command checks which.
    permission: None,
    idempotency: None,
    title: "List bug report links",
    description: "A space's bug report links: each is a URL a game, site or form posts reports \
        to, and every report becomes a task. Lists each link's name, state, maker and report \
        count, never its URL.",
    emits: [],
    input: bug_report_tools::BugReportLinkListArguments,
    output: serde_json::Value,
    run: |call, input| { tool!(call, None, bug_report_tools::bug_report_link_list, input) }
});

tool_op!(BugReportLinkCreate {
    name: "bug_report_link_create",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    // A link's URLs carry its token: never kept for a replay.
    shown_once: ["url", "formUrl"],
    title: "Create bug report link",
    description: "Make a bug report link (its URL is shown once), optionally into a collection \
        with a form, as the space's settings do. Anyone who can add tasks makes one.",
    emits: ["bug_report_link.created"],
    input: bug_report_tools::BugReportLinkWriteArguments,
    output: serde_json::Value,
    run: |call, input| {
        names_existing(input.link_id.is_some(), true, "linkId")?;
        tool!(call, None, bug_report_tools::bug_report_link_write, input)
    }
});

tool_op!(BugReportLinkUpdate {
    name: "bug_report_link_update",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    destructive: true,
    // A link's URLs carry its token: never kept for a replay.
    shown_once: ["url", "formUrl"],
    title: "Update bug report link",
    description: "Rename or pause a bug report link, give it a new URL (replace: true), or \
        delete it (linkId). Its maker or whoever manages the space changes it.",
    emits: ["bug_report_link.updated"],
    input: bug_report_tools::BugReportLinkWriteArguments,
    output: serde_json::Value,
    run: |call, input| {
        names_existing(input.link_id.is_some(), false, "linkId")?;
        tool!(call, None, bug_report_tools::bug_report_link_write, input)
    }
});
