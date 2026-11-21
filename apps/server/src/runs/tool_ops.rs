//! The run, launch, usage and agent profile tools as operations (K2.19).
//!
//! Each runs the command its hand-written tool ran (`ops::wrapped`), as the
//! caller the dispatcher checked, so the old names keep their arguments and
//! answers while the edge goes. A1's run operations (`run_create`,
//! `run_detail_get`, `run_cancel`, `request_*`) sit beside them; K2.23
//! retires these names onto those.

use uuid::Uuid;

use crate::{
    mcp::{
        agent_profile_tools, batch_tools, launch_tools, model_catalog_tools, publish_tools,
        run_pages, runs, task_commands::command_request_id, usage_tools,
    },
    ops::{
        Perm, Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{run_tool, tool_op},
    },
};

/// The Runs module's.
pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<UsageWindowList>()
        .add::<UsageBatchPreview>()
        .add::<UsageBatchLaunch>()
        .add::<RunStart>()
        .add::<TaskLaunch>()
        .add::<RunGet>()
        .add::<RunStop>()
        .add::<RunRetry>()
        .add::<RunFailureHide>()
        .add::<RunPublish>()
        .add::<RunList>()
        .add::<RunStepList>()
        .add::<RunUsageList>()
        .add::<LaunchList>()
        .add::<LaunchCancel>()
        .add::<ModelCatalogSet>();
}

/// The Agents module's.
pub fn register_agent_ops(registry: &mut Registry) {
    registry.add::<AgentProfileList>().add::<AgentProfileSet>();
}

/// The thing an id names. One that doesn't parse goes on to the command,
/// which says what's wrong with it, as the tool did.
fn named(kind: &'static str, id: &str) -> Target {
    parse_id(id).map_or(Target::Account, |id| Target::thing(kind, id))
}

/// A space when one is named; otherwise the connection's own, as the tool
/// read it.
fn space_or_workspace(project_id: Option<&str>) -> Target {
    match project_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(project) => Target::Space(parse_id(project).unwrap_or_else(Uuid::nil)),
        None => Target::Workspace(None),
    }
}

impl Targeted for usage_tools::UsageWindowsArguments {
    fn target(&self) -> Target {
        Target::Account
    }
}

tool_op!(UsageWindowList {
    name: "usage_window_list",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "Usage windows",
    description: "Your provider accounts' usage: every window (a five-hour session, a week, a month, or a reset schedule you set) with percent used, percent unused and when it resets, banked resets you could spend, and how many ready tasks each account could run. Read only; schedules, alert rules and spending a banked reset happen in the app.",
    emits: [],
    input: usage_tools::UsageWindowsArguments,
    output: usage_tools::UsageWindowsValue,
    run: |call, input| { run_tool!(call, in_workspace, usage_tools::usage_windows, input) }
});

impl Targeted for batch_tools::UsageBatchArguments {
    fn target(&self) -> Target {
        Target::Account
    }
}

tool_op!(UsageBatchPreview {
    name: "usage_batch_preview",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "Suggested batch",
    description: "Use it before it resets: the batch of ready tasks one of your provider accounts could run before its usage window resets. Tasks come from every project the account serves whose default environment runs on its harness, ranked like task_ready_list and sized to the capacity left; projects that can't take part say what to change. Launch it with usage_batch_launch.",
    emits: [],
    input: batch_tools::UsageBatchArguments,
    output: batch_tools::UsageBatchValue,
    run: |call, input| { run_tool!(call, in_workspace, batch_tools::usage_batch, input) }
});

impl Targeted for batch_tools::UsageBatchLaunchArguments {
    fn target(&self) -> Target {
        Target::Account
    }
}

tool_op!(UsageBatchLaunch {
    name: "usage_batch_launch",
    module: "runs",
    kind: Create,
    permission: Some(Perm::RunsLaunch),
    idempotency: Command,
    destructive: true,
    home: true,
    audited: true,
    title: "Launch batch",
    description: "Launch a batch on one of your provider accounts: the suggested batch from usage_batch_preview, or the tasks you pick from it. Each task goes through its project's launch queue in that project's default environment with its defaults, on this account, and is checked again as you when it starts. Follow them with launch_list.",
    emits: ["run.updated"],
    input: batch_tools::UsageBatchLaunchArguments,
    output: batch_tools::UsageBatchLaunchValue,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        run_tool!(call, in_workspace, batch_tools::usage_batch_launch, input)
    }
});

impl Targeted for runs::RunStartArguments {
    fn target(&self) -> Target {
        named("conversation", &self.thread_id)
    }
}

tool_op!(RunStart {
    name: "run_start",
    module: "runs",
    kind: Create,
    permission: Some(Perm::RunsLaunch),
    idempotency: Command,
    destructive: true,
    audited: true,
    title: "Start a run",
    description: "Start a run on your own provider account, in a thread. To work on a task: thread_start in a channel, task_link the thread to the task, then run_start there with the full instructions (the run sees only your prompt and the repository). Returns the run; follow it with run_get. People with full access only; at most 5 of your runs at once through connected apps.",
    emits: ["run.updated"],
    input: runs::RunStartArguments,
    output: runs::RunValue,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        run_tool!(call, runs::run_start, input)
    }
});

impl Targeted for launch_tools::TaskLaunchArguments {
    fn target(&self) -> Target {
        self.task_ids
            .first()
            .map_or(Target::Account, |id| named("task", id))
    }
}

tool_op!(TaskLaunch {
    name: "task_launch",
    module: "runs",
    kind: Create,
    permission: Some(Perm::RunsLaunch),
    idempotency: Command,
    destructive: true,
    audited: true,
    title: "Launch tasks",
    description: "Launch tasks: start a run for each with the project's defaults (environment, harness, model) and a prompt built from the task by the project's launch template, in the task's thread. One task starts at once; several, or startWhen at, window_reset or blockers_done, go through the launch queue, which starts them as your account's slots free up, blocked ones last. People with full access only. Follow a run with run_get and the queue with launch_list.",
    emits: ["run.updated"],
    input: launch_tools::TaskLaunchArguments,
    output: launch_tools::TaskLaunchValue,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        run_tool!(call, launch_tools::task_launch, input)
    }
});

impl Targeted for runs::RunGetArguments {
    fn target(&self) -> Target {
        named("run", &self.run_id)
    }
}

tool_op!(RunGet {
    name: "run_get",
    module: "runs",
    kind: Describe,
    permission: None,
    idempotency: None,
    title: "Get run",
    description: "A run: its stage (queued, working, waiting_for_approval, finished, failed, published, checks_pending, checks_passed, checks_failed), its pull request and checks, and what to do next. Poll it every pollAfterSeconds to follow a run.",
    emits: [],
    input: runs::RunGetArguments,
    output: runs::RunValue,
    run: |call, input| { run_tool!(call, runs::run_get, input) }
});

impl Targeted for runs::RunControlArguments {
    fn target(&self) -> Target {
        named("run", &self.run_id)
    }
}

tool_op!(RunStop {
    name: "run_stop",
    module: "runs",
    kind: Change,
    permission: Some(Perm::RunsInterrupt),
    idempotency: Command,
    destructive: true,
    audited: true,
    title: "Stop run",
    description: "Stop a run that's queued, working or waiting for an answer, as the run's Stop button does. Its workspace is kept and its open questions are withdrawn; stopping one already stopped changes nothing. Start it again with run_retry (people) or run_request (agents).",
    emits: ["run.updated"],
    input: runs::RunControlArguments,
    output: runs::RunValue,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        run_tool!(call, runs::run_stop, input)
    }
});

tool_op!(RunRetry {
    name: "run_retry",
    module: "runs",
    kind: Change,
    permission: Some(Perm::RunsLaunch),
    idempotency: Command,
    audited: true,
    title: "Retry run",
    description: "Start a finished, failed or stopped run again: a new run in the same thread with the same prompt, repository and settings, on your own account. Returns the new run. People with full access only; agents ask with run_request.",
    emits: ["run.updated"],
    input: runs::RunControlArguments,
    output: runs::RunValue,
    run: |call, input| {
        let mut input = input;
        input.client_request_id = Some(command_request_id(call.ctx));
        if call.ctx.request_id.is_some() {
            call.key_honoured();
        }
        run_tool!(call, runs::run_retry, input)
    }
});

impl Targeted for runs::RunDismissFailureArguments {
    fn target(&self) -> Target {
        named("run", &self.run_id)
    }
}

tool_op!(RunFailureHide {
    name: "run_failure_hide",
    module: "runs",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Dismiss run failure",
    description: "Dismiss a failed run's notice for yourself, as the notice's Dismiss button does: the run stops showing as needing your attention. The run and its failure stay; dismissing twice changes nothing.",
    emits: ["run.updated"],
    input: runs::RunDismissFailureArguments,
    output: runs::RunValue,
    run: |call, input| { run_tool!(call, runs::run_dismiss_failure, input) }
});

impl Targeted for publish_tools::RunPublishArguments {
    fn target(&self) -> Target {
        named("run", &self.run_id)
    }
}

tool_op!(RunPublish {
    name: "run_publish",
    module: "runs",
    kind: Change,
    permission: Some(Perm::CodePublish),
    idempotency: Dispatcher,
    audited: true,
    title: "Publish run branch",
    description: "Push a finished run's branch to GitHub, as the run page's Publish button does. It's queued for the GitHub worker; run_get shows when it's published. Publishing the same commit again returns the job already queued. Then pull_request_create opens a pull request from it.",
    emits: ["run.updated"],
    input: publish_tools::RunPublishArguments,
    output: publish_tools::QueuedValue,
    run: |call, input| { run_tool!(call, publish_tools::run_publish, input) }
});

impl Targeted for runs::RunListArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

tool_op!(RunList {
    name: "run_list",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "List runs",
    description: "List runs you can see, newest first: active ones by default, or finished, failed, or all. mine: true for only runs you started.",
    emits: [],
    input: runs::RunListArguments,
    output: runs::RunListValue,
    run: |call, input| { run_tool!(call, in_workspace, runs::run_list, input) }
});

impl Targeted for run_pages::RunStepsArguments {
    fn target(&self) -> Target {
        named("run", &self.run_id)
    }
}

tool_op!(RunStepList {
    name: "run_step_list",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "Read run steps",
    description: "What a run did: its steps in order (commands with exit code and the end of their output, file changes, the agent's notes, other tools, approvals and test results), test totals, the files it changed against its base, and tokens and estimated cost per model. includeDiff: true adds the unified diff. Secrets were removed before steps were stored.",
    emits: [],
    input: run_pages::RunStepsArguments,
    output: run_pages::RunStepsValue,
    run: |call, input| { run_tool!(call, run_pages::run_steps, input) }
});

impl Targeted for run_pages::RunUsageArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

tool_op!(RunUsageList {
    name: "run_usage_list",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "Run usage and cost",
    description: "Tokens in, out and cached, and estimated cost, of the runs you can see, totalled per day, project, account or model (groupBy) over the last days (30 by default). Cost comes from the model catalog's prices, or what the harness reported. Your own accounts are named; other people's show as one row.",
    emits: [],
    input: run_pages::RunUsageArguments,
    output: run_pages::RunUsageValue,
    run: |call, input| { run_tool!(call, in_workspace, run_pages::run_usage, input) }
});

impl Targeted for run_pages::AgentProfileListArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

tool_op!(AgentProfileList {
    name: "agent_profile_list",
    module: "agents",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "List agent profiles",
    description: "A project's agents and what each is doing now (idle, working on a task, or waiting for an answer), the agent profiles people made there (Reviewer, Planner, Fixer or their own: @handle, harness, model settings, instructions, allowed tools, environment), the harnesses a new agent could run on and the templates. Tag an agent's @handle in a thread to give it work. Profiles are made in project settings.",
    emits: [],
    input: run_pages::AgentProfileListArguments,
    output: run_pages::AgentProfileListValue,
    run: |call, input| { run_tool!(call, in_workspace, run_pages::agent_profile_list, input) }
});

impl Targeted for agent_profile_tools::AgentProfileWriteArguments {
    fn target(&self) -> Target {
        Target::Space(parse_id(&self.project_id).unwrap_or_else(Uuid::nil))
    }
}

tool_op!(AgentProfileSet {
    name: "agent_profile_set",
    module: "agents",
    kind: Change,
    permission: Some(Perm::SettingsManage),
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    title: "Change an agent",
    description: "People: make an agent in a project (often from a template such as reviewer), change it (name, @handle, harness, model settings, instructions, allowed tools, environment), turn it off or on, or remove it. The project's owners and admins, through an app connected as themselves; agents read them with agent_profile_list. Audited.",
    emits: ["setting.changed"],
    input: agent_profile_tools::AgentProfileWriteArguments,
    output: agent_profile_tools::AgentProfileWriteValue,
    run: |call, input| { run_tool!(call, agent_profile_tools::agent_profile_write, input) }
});

impl Targeted for launch_tools::ProjectArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

tool_op!(LaunchList {
    name: "launch_list",
    module: "runs",
    kind: Query,
    permission: None,
    idempotency: None,
    spans: true,
    title: "List launches",
    description: "A project's launch queue: what waits to start and why (blocked by a task, waiting for an account slot, a set time), and the latest launches that started, failed or were cancelled.",
    emits: [],
    input: launch_tools::ProjectArguments,
    output: launch_tools::LaunchListValue,
    run: |call, input| { run_tool!(call, in_workspace, launch_tools::launch_list, input) }
});

impl Targeted for launch_tools::LaunchCancelArguments {
    fn target(&self) -> Target {
        named("launch", &self.launch_id)
    }
}

tool_op!(LaunchCancel {
    name: "launch_cancel",
    module: "runs",
    kind: Change,
    // Whoever asked for the launch, or manages the space: the command
    // decides, as the queue offers it; starting runs isn't needed.
    permission: None,
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    title: "Cancel launch",
    description: "Take a launch out of the queue before it starts, as the queue's Cancel button does: your own, or anyone's if you own or administer the project. Pass its launchId from launch_list. One that already started can't be cancelled; stop its run with run_stop.",
    emits: ["run.updated"],
    input: launch_tools::LaunchCancelArguments,
    output: launch_tools::LaunchValue,
    run: |call, input| { run_tool!(call, in_workspace, launch_tools::launch_cancel, input) }
});

impl Targeted for model_catalog_tools::ModelCatalogWriteArguments {
    fn target(&self) -> Target {
        Target::Space(parse_id(&self.project_id).unwrap_or_else(Uuid::nil))
    }
}

tool_op!(ModelCatalogSet {
    name: "model_catalog_set",
    module: "runs",
    kind: Change,
    permission: Some(Perm::SettingsManage),
    idempotency: Dispatcher,
    audited: true,
    title: "Change model list",
    description: "People: change a workspace's model list for one harness, as its Runs and models settings do: edit a model (name, context size, prices, reasoning levels, speeds, hiding it, making it the default) or add one by hand with model and overrides, remove one with remove, or refresh the list from the harness through your own signed-in account with refresh. The workspace's owners and admins, through an app connected as themselves. Audited.",
    emits: ["setting.changed"],
    input: model_catalog_tools::ModelCatalogWriteArguments,
    output: model_catalog_tools::ModelCatalogWriteValue,
    run: |call, input| { run_tool!(call, model_catalog_tools::model_catalog_write, input) }
});
