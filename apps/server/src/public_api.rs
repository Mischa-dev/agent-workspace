//! X3: the public API. A versioned REST API for scripts and services, with an
//! OpenAPI document generated from the Rust types.
//!
//! Every operation is one of the MCP tools, so the API can do whatever the
//! web app and agents can, with the same checks: `POST /api/public/v1/<tool>`
//! with the tool's arguments as the JSON body returns the tool's result.
//! The tools are where web, MCP and API parity already lives (principle 5),
//! so a feature that adds a tool appears here and in the document on its own.
//!
//! Callers authenticate with a bearer token: a personal API token (S7, one
//! project, read or read and write, acting as its owner), a connected app's
//! OAuth token, or an agent's token. There is no second kind of token. Each
//! call is checked against the caller's current memberships, exactly as an
//! MCP call is.
//!
//! The result's `outcome` sets the HTTP status (`ok` 200, `not_found` 404,
//! `permission_denied` 403, `invalid_request` 422, `conflict` 409,
//! `temporarily_unavailable` 503); the body is the result as the tool wrote
//! it, so a script reads the same JSON an agent does.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};

use crate::{
    AppState,
    error::AppError,
    mcp::{self, DirectCallError},
};

/// The API's version, in its paths and its document.
pub const API_VERSION: &str = "1.0.0";
pub const BASE_PATH: &str = "/api/public/v1";
/// The largest body a call may send, the same as an MCP call.
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/public/v1/openapi.json", get(openapi_document))
        .route("/api/public/v1/{tool}", post(call_operation))
}

/// The document, gzipped for a client that takes it: it is close to a
/// megabyte as JSON and a tenth of that compressed.
async fn openapi_document(headers: HeaderMap) -> Response {
    let gzip = headers
        .get_all(http::header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';');
            let name = parts.next().unwrap_or("").trim();
            let refused = parts.any(|parameter| {
                parameter
                    .trim()
                    .strip_prefix("q=")
                    .and_then(|q| q.trim().parse::<f32>().ok())
                    == Some(0.0)
            });
            name.eq_ignore_ascii_case("gzip") && !refused
        });
    let mut response = if gzip {
        match gzipped(&document()) {
            Ok(body) => (
                [
                    (http::header::CONTENT_TYPE, "application/json"),
                    (http::header::CONTENT_ENCODING, "gzip"),
                ],
                body,
            )
                .into_response(),
            Err(_) => Json(document()).into_response(),
        }
    } else {
        Json(document()).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("public, max-age=300"),
    );
    headers.insert(
        http::header::VARY,
        http::HeaderValue::from_static("accept-encoding"),
    );
    response
}

fn gzipped(document: &Value) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&serde_json::to_vec(document).map_err(std::io::Error::other)?)?;
    encoder.finish()
}

async fn call_operation(
    State(state): State<AppState>,
    Path(tool): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match call(&state, &tool, &headers, &body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn call(
    state: &AppState,
    tool: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, AppError> {
    let authentication_key = mcp::exact_bearer_token(headers).unwrap_or("invalid");
    state.rate_limits.check_mcp_auth(authentication_key).await?;
    let Some(principal) = mcp::resolve_bearer_principal(state, headers).await? else {
        state.rate_limits.note_mcp_auth_failure().await;
        return Err(AppError::AuthenticationRequired);
    };
    state
        .rate_limits
        .note_mcp_auth_success(authentication_key)
        .await;
    state
        .rate_limits
        .check_mcp_request(principal.credential_id)
        .await?;
    if body.len() > MAX_BODY_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let arguments = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice::<Value>(body).map_err(|_| AppError::Invalid("JSON body"))?
    };
    match mcp::call_tool_direct(state, principal, tool, arguments).await {
        Ok(result) => {
            let status = status_for(&result);
            let mut response = (status, Json(result)).into_response();
            // An old name for an operation (K2 3.6) says it's on its way out.
            if let Some(old) = crate::ops::aliases::active(tool)
                && !crate::ops::registry().contains(tool)
            {
                let headers = response.headers_mut();
                headers.insert("deprecation", axum::http::HeaderValue::from_static("true"));
                if let Some(sunset) = old.sunset_header()
                    && let Ok(value) = axum::http::HeaderValue::from_str(&sunset)
                {
                    headers.insert("sunset", value);
                }
            }
            Ok(response)
        }
        Err(DirectCallError::UnknownTool) => Err(AppError::NotFound("api_route")),
        Err(DirectCallError::InvalidArguments(message)) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "outcome": "invalid_request",
                "field": "body",
                "message": bounded(&message),
            })),
        )
            .into_response()),
        Err(DirectCallError::Refused(error)) => Err(error),
    }
}

fn bounded(message: &str) -> String {
    message.chars().take(1_000).collect()
}

/// The HTTP status for a tool result, from its `outcome`. A result with no
/// outcome is a plain read; an outcome nobody mapped is a server fault, not
/// a success.
pub(crate) fn status_for(result: &Value) -> StatusCode {
    match result.get("outcome").and_then(Value::as_str) {
        None => StatusCode::OK,
        Some(outcome) => outcome_status(outcome).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// The status for each outcome a tool can answer with; a test keeps this
/// in step with the tools' output schemas.
fn outcome_status(outcome: &str) -> Option<StatusCode> {
    Some(match outcome {
        "ok" | "applied" | "found" | "events" | "requested" => StatusCode::OK,
        // K2.21: a long call kept and run by a job.
        "started" => StatusCode::ACCEPTED,
        "not_found" => StatusCode::NOT_FOUND,
        "permission_denied" => StatusCode::FORBIDDEN,
        "invalid_request" | "invalid_transition" | "rejected" => StatusCode::UNPROCESSABLE_ENTITY,
        "conflict" => StatusCode::CONFLICT,
        "cursor_expired" => StatusCode::GONE,
        "busy" | "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "temporarily_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        // The operation layer's refusals, with the statuses `OpError::status`
        // gives them.
        "unauthenticated" => StatusCode::UNAUTHORIZED,
        "limit_reached" => StatusCode::PAYMENT_REQUIRED,
        "workspace_suspended" => StatusCode::LOCKED,
        "too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// The OpenAPI document

/// Groups operations by the noun their tool starts with.
pub(crate) fn tag_for(tool: &str) -> &'static str {
    if tool == "my_permissions" {
        return "Roles and access";
    }
    let noun = tool.split('_').next().unwrap_or(tool);
    match noun {
        "briefing" | "inbox" | "notification" | "mark" | "ask" | "question" => {
            "Inbox and notifications"
        }
        "project" | "projects" | "workflow" | "settings" => "Projects",
        "role" => "Roles and access",
        "workspace" => "Workspace",
        "conversation" | "message" | "file" | "thread" | "channel" | "pin" | "people" => {
            "Conversations"
        }
        "reply" | "session" | "agent" => "Agents and replies",
        "task" | "label" | "field" | "milestone" | "view" | "my" | "template" => "Tasks",
        "page" | "skill" | "decision" | "search" => "Pages and search",
        "collection" | "record" => "Collections",
        "sheet" => "Sheets",
        "run" | "launch" | "approval" | "environment" | "external" | "usage" => {
            "Runs and launching"
        }
        "repository" | "pull" => "Code",
        "webhook" => "Webhooks",
        "automation" => "Automations",
        _ => "Other",
    }
}

const ERROR_STATUSES: &[(&str, &str)] = &[
    (
        "401",
        "No token, or the token is revoked, expired or unknown.",
    ),
    (
        "403",
        "The token's owner may not do this here, or the token is read only.",
    ),
    (
        "404",
        "No such operation, or the thing named wasn't found or you can't see it.",
    ),
    (
        "409",
        "Someone changed it first, or it's in a state that refuses this.",
    ),
    ("413", "The body is larger than 1 MiB."),
    ("422", "The arguments don't match the operation's schema."),
    ("429", "Too many calls; wait for Retry-After seconds."),
    (
        "503",
        "Agent Workspace couldn't finish that. Try again shortly.",
    ),
];

/// An output schema's descriptions cut to their first sentence: the
/// result's shape is the reference, and the operation's own description and
/// its arguments say the rest. Kept whole, results take most of the
/// document.
fn brief(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match child {
                    Value::String(text) if key == "description" => {
                        if let Some(end) = first_sentence_end(text) {
                            text.truncate(end);
                        }
                    }
                    _ => brief(child),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(brief),
        _ => {}
    }
}

/// Where a description's first sentence ends, when more follows it.
fn first_sentence_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(1)).find_map(|index| {
        (matches!(bytes[index], b'.' | b'?' | b'!' | b';' | b':')
            && bytes[index + 1].is_ascii_whitespace()
            && !text[index + 1..].trim().is_empty())
        .then_some(index + 1)
    })
}

/// Moves a schema's `$defs` into `components.schemas` and points its
/// references there. A definition keeps its own name when every tool that
/// has one defines it the same way (shared message content, a thread's
/// summary), so it is written once; a tool whose copy differs gets its own,
/// under the tool's name.
fn hoist(tool: &str, schema: &Map<String, Value>, components: &mut Map<String, Value>) -> Value {
    let mut schema = schema.clone();
    schema.remove("$schema");
    let prefix = pascal(tool);
    let definitions = match schema.remove("$defs") {
        Some(Value::Object(definitions)) => definitions,
        _ => Map::new(),
    };
    let mut names = definitions
        .keys()
        .map(|name| (name.clone(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    // A definition that must take the tool's name changes the ones that
    // refer to it, so decide until nothing moves.
    loop {
        let mut moved = false;
        for (name, definition) in &definitions {
            if names.get(name) != Some(name) {
                continue;
            }
            let mut written = definition.clone();
            rewrite_refs(&mut written, &names);
            if components
                .get(name)
                .is_some_and(|existing| *existing != written)
            {
                names.insert(name.clone(), format!("{prefix}{name}"));
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for (name, mut definition) in definitions {
        rewrite_refs(&mut definition, &names);
        let key = names.get(&name).cloned().unwrap_or(name);
        components.insert(key, definition);
    }
    let mut value = Value::Object(schema);
    rewrite_refs(&mut value, &names);
    value
}

/// The arguments every operation takes (the workspace, a write's
/// `clientRequestId`, `preview`) are written once and referenced, so each
/// new operation adds only what is its own.
fn share_arguments(input: &mut Value, components: &mut Map<String, Value>) {
    let Some(properties) = input.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (argument, key) in [
        ("workspace", "WorkspaceArgument"),
        ("clientRequestId", "ClientRequestId"),
        ("preview", "PreviewArgument"),
    ] {
        let Some(schema) = properties.get(argument) else {
            continue;
        };
        let shared = components
            .entry(key.to_owned())
            .or_insert_with(|| schema.clone());
        if shared == schema {
            properties.insert(
                argument.to_owned(),
                json!({ "$ref": format!("#/components/schemas/{key}") }),
            );
        }
    }
}

/// Each tool hoists its own copy of the types it shares with others (a
/// field type, an inbox item, a run record): keep one of each identical
/// schema, under the first of its names, and point every reference there.
/// Repeated until nothing merges, since merging the leaves can make the
/// schemas that refer to them identical too.
fn merge_identical(components: &mut Map<String, Value>, others: &mut [&mut Map<String, Value>]) {
    loop {
        let mut names = components.keys().cloned().collect::<Vec<_>>();
        names.sort();
        let mut first: BTreeMap<String, String> = BTreeMap::new();
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        for name in names {
            let raw = serde_json::to_string(&components[&name]).unwrap_or_default();
            match first.get(&raw) {
                Some(kept) => {
                    renamed.insert(name, kept.clone());
                }
                None => {
                    first.insert(raw, name);
                }
            }
        }
        if renamed.is_empty() {
            return;
        }
        for name in renamed.keys() {
            components.remove(name);
        }
        for schema in components.values_mut() {
            point_refs(schema, &renamed);
        }
        for map in others.iter_mut() {
            for value in map.values_mut() {
                point_refs(value, &renamed);
            }
        }
    }
}

/// An operation's result is the envelope: its `ok` branch, the refusal
/// branch every operation shares (its `outcome` names which refusal), and
/// for a long operation the `started` branch. Each branch but `ok` is
/// written once in `components.schemas` and referenced, which keeps the
/// document small as operations are added.
fn share_refusals(result: &mut Value, components: &mut Map<String, Value>) {
    let Some(branches) = result.get_mut("oneOf").and_then(Value::as_array_mut) else {
        return;
    };
    for branch in branches.iter_mut() {
        let outcome = &branch["properties"]["outcome"];
        let key = match outcome["const"].as_str() {
            // A call set aside, by a paused account (A2.10) or a team's
            // guard (A2.27), is answered the same for every operation:
            // shared too.
            Some("ok") if branch.get("anyOf").is_some() => "SetAside".to_owned(),
            Some("ok") => {
                share_ok_envelope(branch, components);
                continue;
            }
            Some("started") => "Started".to_owned(),
            Some(outcome) => format!("Refusal{}", pascal(outcome)),
            // The operation layer's own refusals, one branch listing them
            // all: shared too, under the first name free or already
            // holding the same branch.
            None if outcome["enum"].is_array() => (1..)
                .map(|index| match index {
                    1 => "Refusal".to_owned(),
                    index => format!("Refusal{index}"),
                })
                .find(|key| components.get(key).is_none_or(|shared| shared == branch))
                .unwrap_or_default(),
            None => continue,
        };
        let shared = components
            .entry(key.clone())
            .or_insert_with(|| branch.clone());
        if shared == branch {
            *branch = json!({ "$ref": format!("#/components/schemas/{key}") });
        }
    }
    // The shared branches together (paused, started, each refusal) are the
    // same list for most operations: one `oneOf` of them, named once.
    let (kept, shared): (Vec<Value>, Vec<Value>) = branches
        .drain(..)
        .partition(|branch| !is_shared_branch(branch));
    if shared.len() < 2 {
        branches.extend(kept);
        branches.extend(shared);
        return;
    }
    let others = json!({ "oneOf": shared });
    let key = (1..)
        .map(|index| match index {
            1 => "NotOk".to_owned(),
            index => format!("NotOk{index}"),
        })
        .find(|key| components.get(key).is_none_or(|known| *known == others))
        .unwrap_or_default();
    components.entry(key.clone()).or_insert(others);
    branches.extend(kept);
    branches.push(json!({ "$ref": format!("#/components/schemas/{key}") }));
}

/// A branch that is only a reference to a shared one (not the `ok` branch,
/// which carries the operation's own value beside its reference).
fn is_shared_branch(branch: &Value) -> bool {
    branch
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.contains_key("$ref"))
}

/// The `ok` branch's envelope (everything but its `value`) is the same for
/// every operation: written once as `Ok` and referenced beside the
/// operation's own `value`, which keeps the document small as operations
/// are added.
fn share_ok_envelope(branch: &mut Value, components: &mut Map<String, Value>) {
    let Some(object) = branch.as_object_mut() else {
        return;
    };
    let Some(Value::Object(properties)) = object.get("properties") else {
        return;
    };
    let Some(value) = properties.get("value").cloned() else {
        return;
    };
    let mut envelope = object.clone();
    if let Some(Value::Object(properties)) = envelope.get_mut("properties") {
        properties.remove("value");
    }
    let envelope = Value::Object(envelope);
    let shared = components
        .entry("Ok".to_owned())
        .or_insert_with(|| envelope.clone());
    if *shared == envelope {
        *branch = json!({
            "$ref": "#/components/schemas/Ok",
            "properties": { "value": value },
        });
    }
}

/// An argument left out and one sent as `null` mean the same, so an
/// optional argument's type is written without `null`: what a client should
/// send, and a shorter document.
fn drop_optional_nulls(input: &mut Value) {
    let required = input
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(properties) = input.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        if required.iter().any(|known| known.as_str() == Some(name)) {
            continue;
        }
        let Some(types) = property.get("type").and_then(Value::as_array) else {
            continue;
        };
        let kept: Vec<Value> = types
            .iter()
            .filter(|kind| *kind != "null")
            .cloned()
            .collect();
        if kept.len() == 1 && types.len() == 2 {
            property["type"] = kept[0].clone();
        }
    }
}

fn point_refs(value: &mut Value, renamed: &BTreeMap<String, String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(kept) = reference
                    .strip_prefix("#/components/schemas/")
                    .and_then(|name| renamed.get(name))
            {
                *reference = format!("#/components/schemas/{kept}");
            }
            for child in object.values_mut() {
                point_refs(child, renamed);
            }
        }
        Value::Array(items) => {
            for child in items {
                point_refs(child, renamed);
            }
        }
        _ => {}
    }
}

fn rewrite_refs(value: &mut Value, names: &BTreeMap<String, String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/$defs/")
            {
                let key = names.get(name).cloned().unwrap_or_else(|| name.to_owned());
                *reference = format!("#/components/schemas/{key}");
            }
            for child in object.values_mut() {
                rewrite_refs(child, names);
            }
        }
        Value::Array(items) => {
            for child in items {
                rewrite_refs(child, names);
            }
        }
        _ => {}
    }
}

fn pascal(tool: &str) -> String {
    tool.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + characters.as_str()
            })
        })
        .collect()
}

const DESCRIPTION: &str = "Everything you can do in Agent Workspace, for scripts and services. \
Each operation is a POST with the operation's arguments as a JSON object and returns a JSON \
result with an `outcome` (`ok`, `not_found`, `permission_denied`, `invalid_request`, \
`conflict`, `temporarily_unavailable`) that also sets the HTTP status.\n\n\
Authenticate with `Authorization: Bearer <token>`: a personal API token from Settings › \
Account › API tokens, which acts as you in one project (read only, or read and write), or a \
connected app's token. Every call is checked against your current access, so a token stops \
working wherever you lose access.\n\n\
Projects and tasks can be named by key (`AWD`, `AWD-12`) wherever an ID is asked for. \
Operations that change things take a `clientRequestId` so a retry never applies twice, and \
changes to a task take the `expectedVersion` you last read.";

/// Drops what a schema says twice, to keep the document small: a schema's
/// `title` (the operation's id or the component's name says it), what JSON
/// Schema assumes without being told (`default: null`,
/// `additionalProperties: true`), and the
/// unsigned integer formats the schema generator adds, which aren't
/// OpenAPI's (`minimum: 0` already says it, and the type's own maximum
/// adds nothing).
fn slim(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.get("title").is_some_and(Value::is_string) {
                map.remove("title");
            }
            // What JSON Schema assumes anyway: a null default on an optional
            // value, and properties beyond the listed ones allowed.
            if map.get("default").is_some_and(Value::is_null) {
                map.remove("default");
            }
            if map.get("additionalProperties") == Some(&Value::Bool(true)) {
                map.remove("additionalProperties");
            }
            // A plain integer is read as 64 bits wherever the format is left
            // out; only narrower ones say so.
            if map.get("format").and_then(Value::as_str) == Some("int64") {
                map.remove("format");
            }
            if map
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|format| format.starts_with("uint"))
            {
                map.remove("format");
                if map
                    .get("maximum")
                    .and_then(Value::as_u64)
                    .is_some_and(|maximum| {
                        [
                            u64::from(u8::MAX),
                            u64::from(u16::MAX),
                            u64::from(u32::MAX),
                            u64::MAX,
                        ]
                        .contains(&maximum)
                    })
                {
                    map.remove("maximum");
                }
            }
            for (key, child) in map.iter_mut() {
                // Property names are names, not keywords.
                if key == "properties"
                    && let Value::Object(properties) = child
                {
                    properties.values_mut().for_each(slim);
                } else {
                    slim(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(slim),
        _ => {}
    }
}

/// The OpenAPI 3.1 document for every operation.
pub(crate) fn document() -> Value {
    let mut tools = mcp::tool_catalog();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let mut components = Map::new();
    let mut paths = Map::new();
    let mut tags = BTreeMap::new();
    let mut results: BTreeMap<String, String> = BTreeMap::new();
    let mut refusals = Map::new();
    // First, so no tool's own definition takes its name.
    components.insert(
        "Error".to_owned(),
        json!({
            "type": "object",
            "required": ["error"],
            "properties": {
                "error": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" },
                        "retryAfterSeconds": { "type": "integer" },
                    },
                },
            },
        }),
    );
    for tool in tools {
        let name = tool.name.to_string();
        let tag = tag_for(&name);
        tags.entry(tag).or_insert(0_usize);
        *tags.get_mut(tag).expect("tag") += 1;
        let read_only = tool
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint)
            .unwrap_or(false);
        let title = tool
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.title.clone())
            .or(tool.title.clone())
            .unwrap_or_else(|| name.replace('_', " "));
        let mut input = hoist(&name, &tool.input_schema, &mut components);
        share_arguments(&mut input, &mut components);
        drop_optional_nulls(&mut input);
        let (result, output) = tool
            .output_schema
            .as_ref()
            .map(|schema| {
                // Six responses share it, so it's referenced rather than
                // repeated: inline, the document passes a megabyte. Tools
                // with the same result (run_get, run_stop, ...) share one.
                let raw = serde_json::to_string(schema.as_ref()).unwrap_or_default();
                let key = match results.get(&raw) {
                    Some(key) => String::clone(key),
                    None => {
                        let root = format!("{name}_result");
                        let mut briefed = Value::Object(Map::clone(schema.as_ref()));
                        brief(&mut briefed);
                        let briefed = briefed.as_object().cloned().unwrap_or_default();
                        let mut hoisted = hoist(&root, &briefed, &mut components);
                        share_refusals(&mut hoisted, &mut components);
                        let key = pascal(&root);
                        components.insert(key.clone(), hoisted);
                        results.insert(raw, key.clone());
                        key
                    }
                };
                let reference = json!({ "$ref": format!("#/components/schemas/{key}") });
                (key, reference)
            })
            .unwrap_or_else(|| ("Result".to_owned(), json!({ "type": "object" })));
        // The result, a refusal (the same result with its `outcome`) and
        // the shared `Error` body (401, 413 and 429, the same for every
        // operation) are one response, shared by every operation with that
        // result, so an operation adds no responses of its own and the
        // document stays small as operations are added. It is the `default`
        // (every status, 200 included), written once rather than again
        // under 200. The introduction says what each status means.
        refusals.entry(result.clone()).or_insert_with(|| {
            json!({
                "description": "The result, a refusal (see `outcome`), or an `Error`.",
                "content": { "application/json": { "schema": {
                    "oneOf": [output, { "$ref": "#/components/schemas/Error" }],
                } } },
            })
        });
        let responses =
            json!({ "default": { "$ref": format!("#/components/responses/{result}") } });
        let mut post = json!({
            "operationId": name,
            "summary": title,
            "description": tool.description.as_deref().unwrap_or_default(),
            "tags": [tag],
            "requestBody": {
                "required": true,
                "content": { "application/json": { "schema": input } },
            },
            "responses": responses,
        });
        // Only reads say so: a missing mark is a write, which keeps the
        // document small as operations are added.
        if read_only {
            post["x-read-only"] = true.into();
        }
        paths.insert(format!("/{name}"), json!({ "post": post }));
    }
    for path in paths.values_mut() {
        slim(path);
    }
    for schema in components.values_mut() {
        slim(schema);
    }
    merge_identical(&mut components, &mut [&mut paths, &mut refusals]);
    let mut document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Agent Workspace API",
            "version": API_VERSION,
            "description": format!(
                "{DESCRIPTION}\n\nStatuses: {}.",
                ERROR_STATUSES
                    .iter()
                    .map(|(status, description)| format!("{status}, {}", description.trim_end_matches('.')))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        },
        "servers": [{ "url": BASE_PATH }],
        "security": [{ "bearerToken": [] }],
        "tags": tags.keys().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
        "paths": paths,
        "components": {
            "schemas": components,
            "responses": refusals,
            "securitySchemes": {
                "bearerToken": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "A personal API token, a connected app's token or an agent's token.",
                },
            },
        },
    });
    share_common_properties(&mut document);
    share_repeated_properties(&mut document);
    document
}

/// Properties most operations take with the same schema (which workspace,
/// the retry id) are written once in `components` and referenced, which
/// keeps the document small as operations are added.
fn share_common_properties(document: &mut Value) {
    const SHARED: &[(&str, &str)] = &[
        ("workspace", "WorkspaceSelector"),
        ("clientRequestId", "ClientRequestId"),
        ("projectId", "ProjectSelector"),
    ];
    for (property, component) in SHARED {
        let mut seen: BTreeMap<String, (usize, Value)> = BTreeMap::new();
        visit_properties(document, property, &mut |schema| {
            seen.entry(schema.to_string())
                .or_insert((0, schema.clone()))
                .0 += 1;
        });
        let Some((_, (count, common))) = seen.into_iter().max_by_key(|(_, (count, _))| *count)
        else {
            continue;
        };
        if count < 2 {
            continue;
        }
        let reference = json!({ "$ref": format!("#/components/schemas/{component}") });
        visit_properties(document, property, &mut |schema| {
            if *schema == common {
                *schema = reference.clone();
            }
        });
        document["components"]["schemas"][*component] = common;
    }
}

/// Calls `visit` on every schema of a property named `name`.
fn visit_properties(value: &mut Value, name: &str, visit: &mut dyn FnMut(&mut Value)) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get_mut("properties")
                && let Some(schema) = properties.get_mut(name)
            {
                visit(schema);
            }
            for (_, child) in map.iter_mut() {
                visit_properties(child, name, visit);
            }
        }
        Value::Array(items) => {
            for item in items {
                visit_properties(item, name, visit);
            }
        }
        _ => {}
    }
}

/// Any other property written the same way by several operations (a task
/// id with its description, a cursor, a limit) is written once in
/// `components` too, when referencing it is shorter than repeating it.
fn share_repeated_properties(document: &mut Value) {
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    each_property(document, &mut |name, schema| {
        if schema.get("$ref").is_none() {
            *seen
                .entry((name.to_owned(), schema.to_string()))
                .or_default() += 1;
        }
    });
    let mut shared: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut ranked: Vec<_> = seen.into_iter().collect();
    // The most repeated schema of a name takes the plain name.
    ranked.sort_by(
        |((left_name, left), left_count), ((right_name, right), right_count)| {
            left_name
                .cmp(right_name)
                .then(right_count.cmp(left_count))
                .then(left.cmp(right))
        },
    );
    let mut schemas = document["components"]["schemas"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for ((name, schema), count) in ranked {
        // Component names allow letters, digits, `.`, `-` and `_` only.
        let Some(first) = name.chars().next() else {
            continue;
        };
        if !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            continue;
        }
        let base = format!("{}{}", first.to_ascii_uppercase(), &name[1..]);
        let mut component = base.clone();
        let mut suffix = 1;
        while schemas.contains_key(&component) {
            suffix += 1;
            component = format!("{base}{suffix}");
        }
        let reference = format!(r##"{{"$ref":"#/components/schemas/{component}"}}"##).len();
        let written_once = schema.len() + component.len() + 4;
        if count < 2 || count * schema.len() <= count * reference + written_once {
            continue;
        }
        let value: Value = serde_json::from_str(&schema).expect("a schema it just wrote");
        schemas.insert(component.clone(), value);
        shared.insert((name, schema), component);
    }
    if shared.is_empty() {
        return;
    }
    replace_properties(document, &shared);
    let added: BTreeMap<&String, (String, String)> = shared
        .iter()
        .map(|(key, component)| (component, key.clone()))
        .collect();
    for component in added.keys() {
        if let Some(schema) = schemas.get_mut(*component) {
            replace_properties(schema, &shared);
        }
    }
    document["components"]["schemas"] = Value::Object(schemas);
    // A schema counted inside a larger one that is now shared may be left
    // with too few uses to be worth its own component: put it back.
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    count_refs(document, &mut uses);
    let schemas = document["components"]["schemas"]
        .as_object_mut()
        .expect("schemas");
    let mut inline = BTreeMap::new();
    for (component, (_, schema)) in &added {
        let count = uses.get(component.as_str()).copied().unwrap_or(0);
        let reference = format!(r##"{{"$ref":"#/components/schemas/{component}"}}"##).len();
        if count * schema.len() <= count * reference + schema.len() + component.len() + 4
            && let Some(value) = schemas.remove(component.as_str())
        {
            inline.insert(format!("#/components/schemas/{component}"), value);
        }
    }
    if !inline.is_empty() {
        inline_refs(document, &inline);
    }
}

fn count_refs(value: &Value, uses: &mut BTreeMap<String, usize>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                *uses.entry(name.to_owned()).or_default() += 1;
            }
            map.values().for_each(|child| count_refs(child, uses));
        }
        Value::Array(items) => items.iter().for_each(|item| count_refs(item, uses)),
        _ => {}
    }
}

/// Replaces each reference in `inline` with the schema it named.
fn inline_refs(value: &mut Value, inline: &BTreeMap<String, Value>) {
    if let Some(schema) = value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| inline.get(reference))
    {
        *value = schema.clone();
        inline_refs(value, inline);
        return;
    }
    match value {
        Value::Object(map) => map
            .values_mut()
            .for_each(|child| inline_refs(child, inline)),
        Value::Array(items) => items.iter_mut().for_each(|item| inline_refs(item, inline)),
        _ => {}
    }
}

/// Calls `visit` with the name and schema of every property.
fn each_property(value: &Value, visit: &mut dyn FnMut(&str, &Value)) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "properties"
                    && let Value::Object(properties) = child
                {
                    for (name, schema) in properties {
                        visit(name, schema);
                        each_property(schema, visit);
                    }
                } else {
                    each_property(child, visit);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| each_property(item, visit)),
        _ => {}
    }
}

/// Points every shared property schema at its component.
fn replace_properties(value: &mut Value, shared: &BTreeMap<(String, String), String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "properties"
                    && let Value::Object(properties) = child
                {
                    for (name, schema) in properties.iter_mut() {
                        if let Some(component) = shared.get(&(name.clone(), schema.to_string())) {
                            *schema =
                                json!({ "$ref": format!("#/components/schemas/{component}") });
                        } else {
                            replace_properties(schema, shared);
                        }
                    }
                } else {
                    replace_properties(child, shared);
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| replace_properties(item, shared)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{document, pascal, status_for};

    #[test]
    fn every_tool_is_an_operation_with_resolvable_schemas() {
        let document = document();
        let paths = document["paths"].as_object().expect("paths");
        assert!(paths.len() > 50, "only {} operations", paths.len());
        for (path, item) in paths {
            let operation = &item["post"];
            assert_eq!(
                format!("/{}", operation["operationId"].as_str().expect("id")),
                *path
            );
            assert!(operation["requestBody"]["content"]["application/json"]["schema"].is_object());
            assert!(operation["tags"][0].is_string());
        }
        // Every reference points at a schema the document holds.
        let schemas = document["components"]["schemas"]
            .as_object()
            .expect("schemas");
        // The reference page fetches it whole, gzipped (a tenth of its size
        // as JSON), so keep that small; the JSON itself has room to grow as
        // operations are added. Past these, slim the result schemas.
        let size = serde_json::to_vec(&document).expect("serialize").len();
        if std::env::var_os("SHOW_PUBLIC_API_SIZE").is_some() {
            eprintln!("the public API document is {size} bytes");
        }
        assert!(size < 1536 * 1024, "the document is {size} bytes");
        let compressed = super::gzipped(&document).expect("gzip").len();
        assert!(
            compressed < 192 * 1024,
            "the document is {compressed} bytes gzipped"
        );
        let mut references = Vec::new();
        collect_refs(&document, &mut references);
        assert!(!references.is_empty());
        let responses = document["components"]["responses"]
            .as_object()
            .expect("responses");
        for reference in references {
            if let Some(name) = reference.strip_prefix("#/components/responses/") {
                assert!(
                    responses.contains_key(name),
                    "dangling reference {reference}"
                );
                continue;
            }
            let name = reference
                .strip_prefix("#/components/schemas/")
                .unwrap_or_else(|| panic!("reference outside components: {reference}"));
            assert!(schemas.contains_key(name), "dangling reference {reference}");
        }
        assert!(document["paths"]["/task_create"].is_object());
        assert_eq!(document["paths"]["/task_list"]["post"]["x-read-only"], true);
    }

    fn collect_refs(value: &Value, references: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(reference)) = object.get("$ref") {
                    references.push(reference.clone());
                }
                object
                    .values()
                    .for_each(|child| collect_refs(child, references));
            }
            Value::Array(items) => items
                .iter()
                .for_each(|child| collect_refs(child, references)),
            _ => {}
        }
    }

    /// Every `outcome` value any tool's output schema names.
    fn schema_outcomes() -> std::collections::BTreeSet<String> {
        fn walk(root: &Value, node: &Value, found: &mut std::collections::BTreeSet<String>) {
            match node {
                Value::Object(map) => {
                    if let Some(outcome) = map
                        .get("properties")
                        .and_then(|properties| properties.get("outcome"))
                    {
                        let outcome = match outcome.get("$ref").and_then(Value::as_str) {
                            Some(reference) => root
                                .pointer(reference.trim_start_matches('#'))
                                .unwrap_or(outcome),
                            None => outcome,
                        };
                        let values = outcome
                            .get("const")
                            .into_iter()
                            .chain(
                                outcome
                                    .get("enum")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten(),
                            )
                            .chain(
                                ["oneOf", "anyOf"]
                                    .iter()
                                    .filter_map(|key| outcome.get(*key).and_then(Value::as_array))
                                    .flatten()
                                    .filter_map(|variant| variant.get("const")),
                            );
                        found.extend(values.filter_map(Value::as_str).map(str::to_owned));
                    }
                    map.values().for_each(|child| walk(root, child, found));
                }
                Value::Array(items) => items.iter().for_each(|child| walk(root, child, found)),
                _ => {}
            }
        }
        let mut found = std::collections::BTreeSet::new();
        for tool in crate::mcp::tool_catalog() {
            if let Some(schema) = tool.output_schema.as_deref() {
                let schema = Value::Object(schema.clone());
                walk(&schema, &schema, &mut found);
            }
        }
        found
    }

    /// P0.9: a new outcome in any tool's schema needs a status here.
    #[test]
    fn every_outcome_in_a_schema_has_a_status() {
        let outcomes = schema_outcomes();
        assert!(outcomes.contains("permission_denied"), "{outcomes:?}");
        let unmapped = outcomes
            .iter()
            .filter(|outcome| super::outcome_status(outcome).is_none())
            .collect::<Vec<_>>();
        assert!(
            unmapped.is_empty(),
            "outcomes without an HTTP status in public_api::outcome_status: {unmapped:?}"
        );
    }

    #[test]
    fn outcomes_set_the_status() {
        assert_eq!(status_for(&json!({"outcome": "ok"})).as_u16(), 200);
        assert_eq!(status_for(&json!({"outcome": "applied"})).as_u16(), 200);
        assert_eq!(status_for(&json!({"outcome": "not_found"})).as_u16(), 404);
        assert_eq!(
            status_for(&json!({"outcome": "permission_denied"})).as_u16(),
            403
        );
        assert_eq!(status_for(&json!({"outcome": "conflict"})).as_u16(), 409);
        assert_eq!(
            status_for(&json!({"outcome": "invalid_transition"})).as_u16(),
            422
        );
        assert_eq!(status_for(&json!({"outcome": "rejected"})).as_u16(), 422);
        assert_eq!(status_for(&json!({"outcome": "busy"})).as_u16(), 429);
        assert_eq!(
            status_for(&json!({"outcome": "cursor_expired"})).as_u16(),
            410
        );
        assert_eq!(status_for(&json!({"outcome": "found"})).as_u16(), 200);
        assert_eq!(status_for(&json!({"items": []})).as_u16(), 200);
        assert_eq!(status_for(&json!({"outcome": "unheard_of"})).as_u16(), 500);
        assert_eq!(pascal("task_create_many"), "TaskCreateMany");
    }
}
