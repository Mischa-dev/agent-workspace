//! The MCP edge: every operation in the registry is a tool of the same
//! name, listed and called next to the older hand-written tools until those
//! become operations (K2.7 onwards).
//!
//! A tool's input is the operation's input plus three arguments the edge
//! reads itself: `workspace` (an id or slug, needed by a credential that
//! reaches more than one workspace when the target doesn't say which),
//! `clientRequestId` for a write and `preview` where the operation has one.
//! Its output is the envelope (`ops::envelope`). The alias path
//! `/api/v1/w/{workspace}/mcp` names the workspace too; a call whose
//! argument disagrees with it is refused.

use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    Caller, DispatchRequest, Edge, OpError, OpKind, OpMeta, dispatch::dispatch, registry, target,
};
use crate::{
    AppState,
    mcp::McpPrincipal,
    modules::ModuleSet,
    policy::{AgentLimits, PermissionSet},
};

/// The workspace an MCP request's alias path named, left on the request by
/// `authenticate_mcp`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AliasWorkspace(pub Uuid);

/// The caller behind an MCP or public API credential: a person's app is held
/// to their agent limits, as the older tools are.
pub async fn caller_of(state: &AppState, principal: &McpPrincipal) -> Result<Caller, OpError> {
    let limits = limits_of(state, principal).await?;
    Ok(Caller::of_principal(principal, limits))
}

async fn limits_of(
    state: &AppState,
    principal: &McpPrincipal,
) -> Result<Option<AgentLimits>, OpError> {
    crate::mcp::caller_reach(state, principal)
        .await
        .map(|(_, limits)| limits)
        .map_err(|error| OpError::logged("authenticate", &error))
}

/// What a tool's description ends with: the permission it needs.
fn needs(meta: &OpMeta) -> String {
    match meta.permission {
        Some(permission) => format!(
            "{} Needs \"{}\" where it acts.",
            meta.description,
            permission.info().label
        ),
        None => meta.description.to_owned(),
    }
}

/// The tool for an operation.
pub fn tool_of(meta: &OpMeta) -> Tool {
    let mut input = (meta.input_schema)();
    if let Some(properties) = input.as_object_mut().and_then(|object| {
        object
            .entry("properties")
            .or_insert_with(|| json!({}))
            .as_object_mut()
    }) {
        properties.insert(
            "workspace".into(),
            json!({
                "type": "string",
                "description": "The workspace to act in, by id or slug. Needed only when you reach more than one and the target doesn't say which.",
            }),
        );
        if meta.kind.writes() {
            properties.insert(
                "clientRequestId".into(),
                json!({
                    "type": "string",
                    "description": "Your id for this request (1 to 128 letters, digits, '-', '_' or '.'): a retry with the same id is answered once.",
                }),
            );
        }
        if meta.preview {
            properties.insert(
                "preview".into(),
                json!({
                    "type": "boolean",
                    "description": "Show what would happen without changing anything.",
                }),
            );
        }
    }
    let input = match input {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    };
    let output = match super::envelope::schema((meta.output_schema)()) {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    };
    let reads = matches!(meta.kind, OpKind::Describe | OpKind::Query);
    let annotations = ToolAnnotations::with_title(meta.title)
        .read_only(reads)
        .destructive(meta.destructive)
        .idempotent(reads || meta.idempotency != super::Idempotency::None)
        .open_world(false);
    Tool::new(meta.name, needs(meta), input)
        .with_title(meta.title)
        .with_raw_output_schema(output.into())
        .with_annotations(annotations)
}

/// Every operation's tool.
pub fn tools() -> Vec<Tool> {
    registry()
        .entries()
        .map(|entry| tool_of(&entry.meta))
        .collect()
}

/// Whether an operation shows in a caller's tool list: they could call it
/// somewhere, by their credential, their roles and the modules on for them.
pub fn listed(
    meta: &OpMeta,
    caller: &McpPrincipal,
    reach: PermissionSet,
    modules: ModuleSet,
) -> bool {
    if meta.kind.writes() && !caller.can_write() {
        return false;
    }
    if meta
        .permission
        .is_some_and(|permission| !reach.contains(permission))
    {
        return false;
    }
    let switchable =
        crate::modules::manifest::get(meta.module).is_some_and(|module| module.switchable);
    !switchable || modules.contains(meta.module)
}

/// Whether `name` is an operation's tool, or an old name for one
/// (`aliases`) whose operation exists.
pub fn is_operation(name: &str) -> bool {
    registry().contains(name) || super::aliases::active(name).is_some()
}

/// Run an operation's tool for an MCP caller (`Edge::Mcp`), or for a caller
/// of the older direct tool path (the public API's v1 route, automations)
/// with that edge.
pub async fn call(
    state: &AppState,
    principal: &McpPrincipal,
    alias: Option<AliasWorkspace>,
    edge: Edge,
    trace_id: Option<String>,
    name: &str,
    arguments: Option<JsonObject>,
) -> CallToolResult {
    // An old name runs its operation with its old arguments and answers in
    // its old shape, saying what to use instead.
    let renamed = (!registry().contains(name))
        .then(|| super::aliases::active(name))
        .flatten();
    let (name, arguments) = match renamed {
        Some(old) => {
            let arguments = arguments.unwrap_or_default();
            ((old.canonical)(&arguments), Some((old.adapt_in)(arguments)))
        }
        None => (name, arguments),
    };
    let mut envelope = match request(state, principal, alias, edge, trace_id, arguments).await {
        Ok((mut request, input)) => {
            request.renamed_from = renamed.map(|old| old.name);
            dispatch(state, request, name, input).await.body
        }
        Err(error) => error.to_json(),
    };
    let Some(old) = renamed else {
        return CallToolResult::structured(envelope);
    };
    // The old tool answered with its value alone, so an alias does too; a
    // refusal keeps the envelope. Either way `_meta` says what to use.
    let answer = if envelope["outcome"] == "ok" {
        (old.adapt_out)(name, envelope["value"].take())
    } else {
        envelope
    };
    let mut result = CallToolResult::structured(answer);
    let mut meta = JsonObject::new();
    meta.insert(
        "deprecated".to_owned(),
        Value::String(super::aliases::deprecation(&old, name)),
    );
    result.meta = Some(rmcp::model::MetaObject(meta));
    result
}

async fn request(
    state: &AppState,
    principal: &McpPrincipal,
    alias: Option<AliasWorkspace>,
    edge: Edge,
    trace_id: Option<String>,
    arguments: Option<JsonObject>,
) -> Result<(DispatchRequest, Value), OpError> {
    let mut arguments = arguments.unwrap_or_default();
    let named = match arguments.remove("workspace") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => {
            let mut connection = state
                .pool
                .acquire()
                .await
                .map_err(|error| OpError::logged("workspace", &error.into()))?;
            Some(target::workspace_named(&mut connection, &text).await?)
        }
        Some(_) => {
            return Err(OpError::invalid(
                "workspace",
                "workspace is a workspace's id or slug.",
            ));
        }
    };
    let workspace = match (alias, named) {
        (Some(AliasWorkspace(path)), Some(named)) if path != named => {
            return Err(OpError::invalid(
                "workspace",
                "The workspace in the address and the workspace argument disagree.",
            ));
        }
        (Some(AliasWorkspace(path)), _) => Some(path),
        (None, named) => named,
    };
    let request_id = match arguments.remove("clientRequestId") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text),
        Some(_) => {
            return Err(OpError::invalid(
                "clientRequestId",
                "clientRequestId is a string.",
            ));
        }
    };
    let preview = matches!(arguments.remove("preview"), Some(Value::Bool(true)));
    let caller = caller_of(state, principal).await?;
    Ok((
        DispatchRequest {
            caller,
            edge,
            trace_id,
            workspace,
            request_id,
            preview,
            renamed_from: None,
        },
        Value::Object(arguments),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_never_shadows_a_hand_written_tool() {
        for entry in registry().entries() {
            assert!(
                !crate::mcp::has_legacy_tool(entry.meta.name),
                "{} is both an operation and a hand-written tool: remove the tool from the router",
                entry.meta.name
            );
            let tool = tool_of(&entry.meta);
            assert!(tool.output_schema.is_some(), "{}", entry.meta.name);
        }
    }
}
