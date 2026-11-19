//! Operations over a hand-written tool's command (K2.17's first step):
//! the operation names the tool's arguments as its input and runs the
//! command as the caller the dispatcher checked, so the checks and answers
//! inside stay the same while the tool's edge goes. K2.22 moves the bodies
//! into their modules.

/// One operation over a tool's command: its constants and its `run`.
/// `audited: true` says the command writes its own audit row (or that the
/// write is someone's own state, which keeps none); `spans: true` lets a
/// read naming no workspace list across all the caller reaches.
macro_rules! tool_op {
    (
        $op:ident {
            name: $name:literal,
            module: $module:literal,
            kind: $kind:ident,
            permission: $perm:expr,
            idempotency: $idem:ident,
            $(destructive: $destructive:literal,)?
            $(spans: $spans:literal,)?
            $(audited: $audited:literal,)?
            title: $title:literal,
            description: $description:expr,
            emits: [$($emit:literal),*],
            input: $input:ty,
            output: $output:ty,
            run: |$call:ident, $arg:ident| $body:block
        }
    ) => {
        pub(crate) struct $op;

        impl crate::ops::Operation for $op {
            const NAME: &'static str = $name;
            const KIND: crate::ops::OpKind = crate::ops::OpKind::$kind;
            const MODULE: &'static str = $module;
            const PERMISSION: Option<crate::ops::Perm> = $perm;
            const IDEMPOTENCY: crate::ops::Idempotency = crate::ops::Idempotency::$idem;
            $(const DESTRUCTIVE: bool = $destructive;)?
            $(const SPANS_WORKSPACES: bool = $spans;)?
            const TITLE: &'static str = $title;
            const DESCRIPTION: &'static str = $description;
            const EMITS: &'static [&'static str] = &[$($emit),*];

            type Input = $input;
            type Output = $output;

            async fn run(
                $call: &mut crate::ops::OpCall<'_>,
                $arg: $input,
            ) -> Result<$output, crate::ops::OpError> {
                let answer: Result<$output, crate::ops::OpError> = $body;
                $(if $audited && answer.is_ok() {
                    $call.audited_by_command();
                })?
                answer
            }
        }
    };
}

/// Runs a tool's command as the caller, in the space the call located (or
/// in `space` when it located none).
macro_rules! run_tool {
    // A call that names no space acts in its connection's own (as the
    // hand-written tools did): a space-held credential's, or the space the
    // credential was made in.
    ($call:ident, anchored, $command:path, $arguments:expr) => {{
        let principal = crate::work_items::ops::reads::reader($call.state, $call.ctx).await?;
        if principal.project_id.is_nil() {
            return Err(crate::ops::OpError::invalid(
                "projectId",
                "Say which space (see project_list).",
            ));
        }
        crate::mcp::task_commands::tool_answer($command($call.state, &principal, $arguments).await)
    }};
    // As the hand-written tool was called: in the connection's own space,
    // and across every space the caller reaches unless the call named one.
    ($call:ident, as_tool, $command:path, $arguments:expr) => {{
        let principal = crate::work_items::ops::reads::reader($call.state, $call.ctx).await?;
        crate::mcp::task_commands::tool_answer($command($call.state, &principal, $arguments).await)
    }};
    ($call:ident, $command:path, $arguments:expr) => {
        crate::ops::wrapped::run_tool!($call, None, $command, $arguments)
    };
    ($call:ident, $space:expr, $command:path, $arguments:expr) => {{
        let principal = crate::mcp::task_commands::principal_for($call.ctx, $space);
        crate::mcp::task_commands::tool_answer($command($call.state, &principal, $arguments).await)
    }};
}

pub(crate) use {run_tool, tool_op};

/// A `_write` tool split in two: a create names no existing thing, an
/// update does.
pub(crate) fn names_existing(
    found: bool,
    create: bool,
    field: &'static str,
) -> Result<(), super::OpError> {
    match (create, found) {
        (true, true) => Err(super::OpError::invalid(
            field,
            "This makes a new one: leave this out, or use the update operation.",
        )),
        (false, false) => Err(super::OpError::invalid(
            field,
            "Say which one to change (its name or ID).",
        )),
        _ => Ok(()),
    }
}
