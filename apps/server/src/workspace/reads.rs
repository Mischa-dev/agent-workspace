//! Workspace reads as operations (K2.15): the briefing, search, the spaces
//! list and one space. Each runs the read its hand-written tool ran, as the
//! caller the dispatcher checked, held to the workspace the call acts in
//! (a space-held credential to its space).

use uuid::Uuid;

use crate::{
    mcp::{briefing, memory_tools, search_answer, task_commands::tool_answer},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
    },
    work_items::ops::reads::reader,
};

/// The workspace module's reads.
pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<BriefingGet>()
        .add::<WorkspaceSearch>()
        .add::<SearchAnswer>();
}

/// The Projects module's reads.
pub fn register_space_ops(registry: &mut Registry) {
    registry.add::<ProjectList>().add::<ProjectGet>();
}

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
            module: $module:literal,
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
            const MODULE: &'static str = $module;
            const PERMISSION: Option<Perm> = None;
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

impl Targeted for briefing::BriefingArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

read_op!(BriefingGet {
    name: "briefing_get",
    kind: Describe,
    module: "workspace",
    title: "Briefing",
    description: "Start here. What needs you right now: reply requests (agents), unread \
        notifications (mentions, assignments, approvals, runs, pull requests), unread \
        @mentions, conversations with unread messages, your open tasks and claims, and threads \
        that recently settled. Each item is a pointer you can open.",
    input: briefing::BriefingArguments,
    output: briefing::Briefing,
    run: briefing::briefing
});

impl Targeted for memory_tools::SearchArguments {
    fn target(&self) -> Target {
        match self.conversation_id.as_deref().and_then(parse_id) {
            Some(conversation) => Target::thing("conversation", conversation),
            None => space_or_workspace(self.project_id.as_deref()),
        }
    }
}

read_op!(WorkspaceSearch {
    name: "workspace_search",
    kind: Query,
    module: "workspace",
    title: "Search",
    description: "Search messages, threads, tasks, pages, decisions and collection records \
        you can see. Settled things (decisions, pages, thread outcomes) rank first. Narrow by \
        kind, space (projectId), conversation, author (from), or dates.",
    input: memory_tools::SearchArguments,
    output: memory_tools::SearchValue,
    run: memory_tools::search_tool
});

impl Targeted for search_answer::AnswerArguments {
    fn target(&self) -> Target {
        space_or_workspace(self.project_id.as_deref())
    }
}

/// Answers with sources (C2.27). Kept to the call's workspace, whose model
/// answers: what it reads goes to that workspace's provider.
pub(crate) struct SearchAnswer;

impl Operation for SearchAnswer {
    const NAME: &'static str = "search_answer";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const HELD_SPACE_BY_DEFAULT: bool = true;
    const TITLE: &'static str = "Answer from search";
    const DESCRIPTION: &'static str = "Answer a question from what search finds for you in \
        this workspace, citing each statement's sources by number ([1] is the first source). \
        It reads only what you can see, and uses the workspace's model, so it fails when the \
        workspace has none set up. Narrow it with search filters in the question or a space \
        (projectId).";

    type Input = search_answer::AnswerArguments;
    type Output = search_answer::AnswerValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: search_answer::AnswerArguments,
    ) -> Result<search_answer::AnswerValue, OpError> {
        let principal = reader(call.state, call.ctx).await?;
        tool_answer(
            search_answer::answer_tool(call.state, &principal, call.ctx.workspace_id, input).await,
        )
    }
}

impl Targeted for briefing::ProjectsListArguments {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

read_op!(ProjectList {
    name: "project_list",
    kind: Query,
    module: "projects",
    title: "List spaces",
    description: "List the spaces you can work in, with their keys, your role, and open task \
        and unread counts. The Messages space holds DMs and groups. includeArchived also lists \
        archived ones, which project_archive can restore.",
    input: briefing::ProjectsListArguments,
    output: briefing::ProjectsValue,
    run: briefing::projects_list
});

impl Targeted for briefing::ProjectOpenArguments {
    fn target(&self) -> Target {
        space_or_workspace(Some(&self.project_id))
    }
}

read_op!(ProjectGet {
    name: "project_get",
    kind: Describe,
    module: "projects",
    title: "Open space",
    description: "Get oriented in one space: its guide page (read this first), other pages, \
        recent decisions, what's next on the board, blocked and claimed tasks, open threads, \
        channels, repositories, labels, and its layout (match its tone and the words it uses).",
    input: briefing::ProjectOpenArguments,
    output: briefing::ProjectOverview,
    run: briefing::project_open
});
