//! Remote operations (M1.13): an operation a connected module's manifest
//! declares is a tool of its own name for every account allowed to use it.
//! A call is checked as any operation is (the declared permission where it
//! acts, the module switched on), then posted to the module's endpoint with
//! the caller and input, signed with the install's outbound secret
//! (Standard Webhooks), and the module's answer is the call's value.
//!
//! The edges run a declared operation as `connected_module_run`
//! (`wrap`), so the dispatcher's checks, audit row and request ids apply.
//! The call holds no transaction while the module answers (rule 14).

use std::time::Duration;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use rmcp::schemars::{self, JsonSchema};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use super::manifest::{OperationSpec, Scope};
use super::{APP_COLUMNS, AppRow};
use crate::{
    AppState, access,
    error::AppError,
    mcp::McpPrincipal,
    ops::{OpCall, OpError},
    policy::PermissionSet,
};

/// The operation the edges run a declared one as.
pub const CALL_OP: &str = "connected_module_run";
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest input sent and answer taken.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_ANSWER_BYTES: usize = 256 * 1024;
const MAX_MESSAGE_CHARS: usize = 300;

/// The active installs in a workspace that are switched on, with their
/// declared operations; `name` narrows to the one declaring it.
async fn live_installs(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    name: Option<&str>,
) -> Result<Vec<AppRow>, AppError> {
    Ok(sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM module_apps app \
         JOIN modules module ON module.id = app.module_id \
         WHERE app.workspace_id = $1 AND app.state = 'active' \
           AND module.archived_at IS NULL \
           AND EXISTS (SELECT 1 FROM module_installs switch \
                       WHERE switch.workspace_id = app.workspace_id \
                         AND switch.space_id IS NULL \
                         AND switch.module_key = module.module_key AND switch.state = 'on') \
           AND ($2::text IS NULL OR EXISTS ( \
                SELECT 1 FROM jsonb_array_elements(app.manifest -> 'operations') declared \
                WHERE declared ->> 'name' = $2)) \
         ORDER BY app.created_at, app.module_id"
    ))
    .bind(workspace_id)
    .bind(name)
    .fetch_all(&mut *connection)
    .await?)
}

fn declared<'a>(operations: &'a [OperationSpec], name: &str) -> Option<&'a OperationSpec> {
    operations.iter().find(|operation| operation.name == name)
}

/// Whether the declared verb changes anything.
fn writes(spec: &OperationSpec) -> bool {
    matches!(spec.verb.as_str(), "create" | "change")
}

fn permission_of(spec: &OperationSpec) -> Option<crate::policy::Permission> {
    match spec.permission.as_deref().map(Scope::read) {
        Some(Scope::Permission(permission)) => Some(permission),
        _ => None,
    }
}

// ------------------------------------------------------------------ tools

/// The tool for one declared operation: its input plus the arguments the
/// edge reads (`workspace`, `clientRequestId`).
fn tool_of(module_name: &str, spec: &OperationSpec) -> Tool {
    let mut input = match &spec.input {
        Value::Object(object) => object.clone(),
        _ => JsonObject::new(),
    };
    input
        .entry("type")
        .or_insert_with(|| Value::String("object".into()));
    if let Some(properties) = input
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        properties.insert(
            "workspace".into(),
            json!({
                "type": "string",
                "description": "The workspace to act in, by id or slug. Needed only when you reach more than one.",
            }),
        );
        properties.insert(
            "clientRequestId".into(),
            json!({
                "type": "string",
                "description": "Your id for this call, kept the same when you retry it; the module receives it as requestId.",
            }),
        );
    }
    let description = format!(
        "{} From the connected module {module_name}, which runs it.",
        spec.description
            .as_deref()
            .unwrap_or("An operation of a connected module.")
            .trim()
    );
    let reads = !writes(spec);
    let annotations = ToolAnnotations::with_title(spec.name.clone())
        .read_only(reads)
        .destructive(false)
        .idempotent(reads)
        .open_world(true);
    let mut tool = Tool::new(spec.name.clone(), description, input).with_annotations(annotations);
    // It answers as the operation it runs as does.
    tool.output_schema = crate::ops::registry()
        .get(CALL_OP)
        .and_then(|entry| crate::ops::mcp::tool_of(&entry.meta).output_schema);
    tool
}

/// The declared operations a caller could call somewhere: in each
/// workspace they reach, from installs switched on there, needing a
/// permission their roles hold. Names an older tool or operation already
/// has are left out (those win a call). A credential that can't write
/// lists none, since the call is a change.
pub async fn tools_for(
    state: &AppState,
    principal: &McpPrincipal,
    reach: PermissionSet,
    taken: impl Fn(&str) -> bool,
) -> Result<Vec<Tool>, AppError> {
    if !principal.can_write() {
        return Ok(Vec::new());
    }
    let caller = crate::ops::mcp::caller_of(state, principal)
        .await
        .map_err(|_| AppError::Internal("connected module tools: caller"))?;
    let workspaces = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        crate::ops::access_shim::reach(&mut connection, &caller)
            .await
            .map_err(|_| AppError::Internal("connected module tools: reach"))?
    };
    let mut tools: Vec<Tool> = Vec::new();
    for workspace in workspaces {
        let installs = crate::tenancy::scoped(
            crate::tenancy::Tenancy::Workspace {
                account: Some(caller.account_id),
                workspace,
            },
            crate::db::scoped_one(&state.pool, async |connection| {
                live_installs(connection, workspace, None).await
            }),
        )
        .await?;
        for install in installs {
            let manifest = install.manifest()?;
            for spec in &manifest.operations {
                let allowed = permission_of(spec).is_some_and(|needed| reach.contains(needed));
                if allowed && !taken(&spec.name) && !tools.iter().any(|tool| tool.name == spec.name)
                {
                    tools.push(tool_of(&manifest.name, spec));
                }
            }
        }
    }
    Ok(tools)
}

/// A declared operation's arguments as `connected_module_run`'s: the
/// edge's own arguments stay on top, the rest is the operation's input.
pub fn wrap(name: &str, arguments: Option<JsonObject>) -> JsonObject {
    let mut input = arguments.unwrap_or_default();
    let mut wrapped = JsonObject::new();
    for key in ["workspace", "clientRequestId"] {
        if let Some(value) = input.remove(key) {
            wrapped.insert(key.to_owned(), value);
        }
    }
    wrapped.insert("operation".to_owned(), Value::String(name.to_owned()));
    wrapped.insert("input".to_owned(), Value::Object(input));
    wrapped
}

// ------------------------------------------------------------------ input

/// Checks the input against the declared schema's top level: an object,
/// its required properties there, no others when it says so, and each
/// property's declared type and choices.
pub(crate) fn check_input(schema: &Value, input: &Value) -> Result<(), OpError> {
    let invalid = |message: String| OpError::Invalid {
        field: "input".into(),
        message: message.into(),
    };
    let Value::Object(given) = input else {
        return Err(invalid(
            "The input is an object of the operation's arguments.".into(),
        ));
    };
    if serde_json::to_vec(input).map_or(true, |bytes| bytes.len() > MAX_INPUT_BYTES) {
        return Err(invalid("The input is larger than 64 KB.".into()));
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for required in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !given.contains_key(required) {
            return Err(invalid(format!("Give {required}.")));
        }
    }
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    for (key, value) in given {
        let Some(property) = properties.and_then(|properties| properties.get(key)) else {
            if closed {
                return Err(invalid(format!("The operation takes no {key}.")));
            }
            continue;
        };
        if let Some(choices) = property.get("enum").and_then(Value::as_array)
            && !choices.contains(value)
        {
            return Err(invalid(format!("{key} is not one of its choices.")));
        }
        let fits = match property.get("type").and_then(Value::as_str) {
            Some("string") => value.is_string(),
            Some("number") => value.is_number(),
            Some("integer") => value.is_i64() || value.is_u64(),
            Some("boolean") => value.is_boolean(),
            Some("array") => value.is_array(),
            Some("object") => value.is_object(),
            Some("null") => value.is_null(),
            _ => true,
        };
        if !fits {
            return Err(invalid(format!(
                "{key} is not the type the operation takes."
            )));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ call

/// `path` under the install's base address.
pub(super) fn endpoint_of(base_url: &str, path: &str) -> Result<Url, OpError> {
    Url::parse(base_url)
        .ok()
        .and_then(|base| base.join(path).ok())
        .ok_or(OpError::Unavailable)
}

/// What a module answered a signed request with.
pub(super) struct Posted {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Posts `body` to a module, signed with its outbound secret (Standard
/// Webhooks), and reads at most `max` bytes of its answer. A module that
/// doesn't answer in time, or can't be reached, is `Unavailable`.
pub(super) async fn post_signed(
    state: &AppState,
    endpoint: Url,
    secret: &str,
    body: String,
    timeout: Duration,
    max: usize,
) -> Result<Posted, OpError> {
    let id = format!("msg_{}", Uuid::now_v7().simple());
    let timestamp = chrono::Utc::now().timestamp();
    let signature = agent_workspace_host_protocol::sign_push(secret, &id, timestamp, &body)
        .ok_or(OpError::Unavailable)?;
    let unavailable = |_| OpError::Unavailable;
    let client = super::client(state, &endpoint, timeout)
        .await
        .map_err(unavailable)?;
    let mut response = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("webhook-id", &id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signature)
        .body(body)
        .send()
        .await
        .map_err(|_| OpError::Unavailable)?;
    let status = response.status().as_u16();
    let mut answer = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| OpError::Unavailable)? {
        if answer.len() + chunk.len() > max {
            return Err(OpError::Invalid {
                field: "answer".into(),
                message: format!(
                    "The connected module's answer is larger than {} KB.",
                    max / 1024
                )
                .into(),
            });
        }
        answer.extend_from_slice(&chunk);
    }
    Ok(Posted {
        status,
        body: answer,
    })
}

/// What a declared operation answered.
#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAnswer {
    pub operation: String,
    #[schemars(with = "String")]
    pub module_id: Uuid,
    /// The module's answer, as it gave it.
    #[schemars(with = "Value")]
    pub result: Value,
}

/// A short plain message from the module's refusal, if it gave one.
pub(super) fn message_of(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let text = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)?;
    let text: String = text
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect();
    (!text.trim().is_empty()).then_some(text)
}

/// Runs a declared operation for the call's caller.
pub(crate) async fn call(
    call: &mut OpCall<'_>,
    name: &str,
    input: Value,
) -> Result<RemoteAnswer, OpError> {
    let state = call.state;
    let workspace = call.ctx.workspace_id;
    let caller = call.ctx.caller.clone();
    let not_found = || OpError::not_found("That connected module operation");
    // Read what's needed, check access and reveal the secret, then let the
    // connection go before the module is called.
    let (install, spec, secret) = crate::db::scoped_one(&state.pool, async |connection| {
        let Some(install) = live_installs(connection, workspace, Some(name))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let manifest = install.manifest()?;
        let Some(spec) = declared(&manifest.operations, name).cloned() else {
            return Ok(None);
        };
        let Some(permission) = permission_of(&spec) else {
            return Ok(None);
        };
        let need = access::Need::permission(permission).writing(writes(&spec));
        if access::check(
            connection,
            &caller,
            need,
            access::Target::Workspace(workspace),
        )
        .await?
        .is_err()
        {
            return Ok(Some(Err(permission)));
        }
        let Some(outbound) = install.outbound_secret_id else {
            return Ok(None);
        };
        let keys = crate::secrets::keyring(&state.config)?;
        let secret = crate::secrets::reveal(
            &mut *connection,
            keys.as_ref(),
            outbound,
            "connected module operation",
        )
        .await?;
        Ok::<_, AppError>(Some(Ok((install, spec, secret))))
    })
    .await
    .map_err(|error| OpError::logged(CALL_OP, &error))?
    .ok_or_else(not_found)?
    .map_err(|permission| OpError::Denied {
        reason: crate::ops::DenyReason::Role { permission },
        message: format!(
            "Your role here doesn't include \"{}\", which this operation needs.",
            permission.info().label
        )
        .into(),
    })?;
    check_input(&spec.input, &input)?;

    let endpoint = endpoint_of(&install.base_url, &spec.endpoint)?;
    let body = json!({
        "format": "agent-workspace-call/1",
        "operation": name,
        "input": input,
        "caller": {
            "account": caller.account_id,
            "kind": crate::ops::context::actor_kind_key(caller.kind),
            "requestedBy": caller.requested_by,
        },
        "workspace": { "id": workspace },
        "module": { "id": install.module_id, "key": install.module_key },
        "requestId": call.ctx.request_id,
    })
    .to_string();
    let Posted {
        status,
        body: answer,
    } = post_signed(
        state,
        endpoint,
        secret.expose(),
        body,
        CALL_TIMEOUT,
        MAX_ANSWER_BYTES,
    )
    .await?;
    if !(200..300).contains(&status) {
        let message = message_of(&answer);
        return Err(match status {
            400 | 422 => OpError::Invalid {
                field: "input".into(),
                message: message
                    .unwrap_or_else(|| "The connected module refused the input.".into())
                    .into(),
            },
            404 => OpError::not_found("What the operation names"),
            409 => OpError::Conflict {
                reason: crate::ops::ConflictReason::State {
                    code: "module_conflict",
                },
                message: message
                    .unwrap_or_else(|| "The connected module refused it as a conflict.".into())
                    .into(),
                current: None,
            },
            _ => OpError::Unavailable,
        });
    }
    let result = if answer.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answer).map_err(|_| OpError::Invalid {
            field: "operation".into(),
            message: "The connected module's answer isn't JSON.".into(),
        })?
    };
    let account = caller.account_id;
    let tx = call.tx().await?;
    let mut ran = crate::events::NewEvent::thing(
        &crate::events::catalog::MODULE_OPERATION_RAN,
        workspace,
        "",
        Uuid::nil(),
    )
    .audience(crate::events::Audience::Account(account))
    .workspace(workspace)
    .actor(account)
    .payload(json!({ "moduleId": install.module_id, "operation": name }));
    ran.thing = None;
    ran.space_id = None;
    crate::events::emit(tx, ran).await?;
    call.audit_target(
        "module",
        install.module_id,
        json!({ "operation": name, "status": status }),
    );
    Ok(RemoteAnswer {
        operation: name.to_owned(),
        module_id: install.module_id,
        result,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_checked_against_the_declared_top_level() {
        let schema = json!({
            "type": "object",
            "properties": {
                "build": { "type": "string" },
                "mode": { "enum": ["fast", "full"] },
                "count": { "type": "integer" }
            },
            "required": ["build"],
            "additionalProperties": false
        });
        assert!(check_input(&schema, &json!({ "build": "214" })).is_ok());
        assert!(
            check_input(
                &schema,
                &json!({ "build": "214", "mode": "fast", "count": 2 })
            )
            .is_ok()
        );
        assert!(check_input(&schema, &json!({})).is_err());
        assert!(check_input(&schema, &json!({ "build": 214 })).is_err());
        assert!(check_input(&schema, &json!({ "build": "1", "mode": "slow" })).is_err());
        assert!(check_input(&schema, &json!({ "build": "1", "count": 1.5 })).is_err());
        assert!(check_input(&schema, &json!({ "build": "1", "other": true })).is_err());
        assert!(check_input(&schema, &json!(["build"])).is_err());
    }

    #[test]
    fn the_edge_arguments_stay_on_top() {
        let mut arguments = JsonObject::new();
        arguments.insert("build".into(), json!("214"));
        arguments.insert("workspace".into(), json!("acme"));
        arguments.insert("clientRequestId".into(), json!("r1"));
        let wrapped = wrap("acme_build_rerun", Some(arguments));
        assert_eq!(wrapped["operation"], json!("acme_build_rerun"));
        assert_eq!(wrapped["input"], json!({ "build": "214" }));
        assert_eq!(wrapped["workspace"], json!("acme"));
        assert_eq!(wrapped["clientRequestId"], json!("r1"));
    }

    #[test]
    fn a_refusal_message_is_short_and_plain() {
        let long = "x".repeat(1000);
        let body = json!({ "error": { "message": long } }).to_string();
        assert_eq!(
            message_of(body.as_bytes()).expect("a message").len(),
            MAX_MESSAGE_CHARS
        );
        assert_eq!(
            message_of(br#"{"message":"No such build.\n"}"#).as_deref(),
            Some("No such build.")
        );
        assert_eq!(message_of(b"<html>"), None);
    }
}
