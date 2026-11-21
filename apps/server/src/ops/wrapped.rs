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
            $(home: $home:literal,)?
            $(resource: $resource:expr,)?
            $(audited: $audited:literal,)?
            $(guard_kind: $guard_kind:literal,)?
            $(shown_once: [$($shown:literal),*],)?
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
            $(const HOME_WORKSPACE_BY_DEFAULT: bool = $home;)?
            $(const RESOURCE: Option<crate::ops::ResourceAddress> = Some($resource);)?
            $(const SHOWN_ONCE: &'static [&'static str] = &[$($shown),*];)?
            $(const GUARD_KIND: Option<&'static str> = Some($guard_kind);)?
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
    // As `as_tool`, and where the call reaches no space at all (a person's
    // session naming none), in the workspace's own space, whose workspace
    // the command reads.
    ($call:ident, in_workspace, $command:path, $arguments:expr) => {{
        let mut principal = crate::work_items::ops::reads::reader($call.state, $call.ctx).await?;
        if principal.project_id.is_nil() {
            principal.project_id = crate::ops::wrapped::own_space($call.state, $call.ctx).await?;
        }
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

/// Runs a write tool's command that keeps its own retry key, with the
/// call's request id, so the key is reserved in the command's transaction
/// with the write (`Idempotency::Command`). The command's own replay mark
/// becomes the envelope's; its value is the first answer's.
macro_rules! keyed {
    ($call:ident, $command:path, $arguments:expr) => {{
        let mut arguments = $arguments;
        arguments.client_request_id =
            Some(crate::mcp::task_commands::command_request_id($call.ctx));
        if $call.ctx.request_id.is_some() {
            $call.key_honoured();
        }
        let mut value = crate::ops::wrapped::run_tool!($call, $command, arguments)?;
        $call.mark_replayed(::std::mem::take(&mut value.idempotent_replay));
        Ok(value)
    }};
}

pub(crate) use {keyed, run_tool, tool_op};

/// The space a credential was made in (`$1`, an MCP secret or an app's
/// grant, held by `$2`): where a call naming none acted before operations.
pub(crate) const CREDENTIAL_ANCHOR_SQL: &str = "SELECT anchor.project_id FROM ( \
     SELECT project_id FROM account_credentials WHERE id = $1 AND actor_id = $2 \
     UNION ALL \
     SELECT project_id FROM mcp_oauth_grants WHERE id = $1 AND actor_id = $2 \
     ) anchor LIMIT 1";

/// The workspace's own space, for a command that reads its workspace from
/// a space; nil when the call spans workspaces.
pub(crate) async fn own_space(
    state: &crate::AppState,
    ctx: &super::OpContext,
) -> Result<uuid::Uuid, super::OpError> {
    if ctx.workspace_id.is_nil() {
        return Ok(uuid::Uuid::nil());
    }
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(|error| super::OpError::logged(ctx.op, &error.into()))?;
    crate::spaces::workspace::find(&mut transaction, ctx.workspace_id)
        .await
        .map(Option::unwrap_or_default)
        .map_err(|error| super::OpError::logged(ctx.op, &error))
}

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
