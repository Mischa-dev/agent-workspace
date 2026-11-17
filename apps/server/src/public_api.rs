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

async fn openapi_document() -> Response {
    let mut response = Json(document()).into_response();
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("public, max-age=300"),
    );
    response
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

/// An operation's result is the envelope: its `ok` branch and one branch
/// per refusal, the same for every operation. Each refusal branch is
/// written once in `components.schemas` and referenced, which keeps the
/// document small as operations are added.
fn share_refusals(result: &mut Value, components: &mut Map<String, Value>) {
    let Some(branches) = result.get_mut("oneOf").and_then(Value::as_array_mut) else {
        return;
    };
    for branch in branches {
        let Some(outcome) = branch["properties"]["outcome"]["const"]
            .as_str()
            .filter(|outcome| *outcome != "ok")
            .map(str::to_owned)
        else {
            continue;
        };
        let key = format!("Refusal{}", pascal(&outcome));
        let shared = components
            .entry(key.clone())
            .or_insert_with(|| branch.clone());
        if shared == branch {
            *branch = json!({ "$ref": format!("#/components/schemas/{key}") });
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
        let input = hoist(&name, &tool.input_schema, &mut components);
        let output = tool
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
                        let mut hoisted = hoist(&root, schema, &mut components);
                        share_refusals(&mut hoisted, &mut components);
                        let key = pascal(&root);
                        components.insert(key.clone(), hoisted);
                        results.insert(raw, key.clone());
                        key
                    }
                };
                json!({ "$ref": format!("#/components/schemas/{key}") })
            })
            .unwrap_or_else(|| json!({ "type": "object" }));
        // The result, for success and refusal alike: one response per
        // operation that its 200 and its refusing statuses all point at, so
        // the document stays small as tools are added. The introduction says
        // what each status means.
        let mut responses = Map::new();
        let result = format!("{}Result", pascal(&name));
        refusals.insert(
            result.clone(),
            json!({
                "description": "The result. Its `outcome` says whether it worked; on a refusal, \
                    the status, `outcome` and `message` say why.",
                "content": { "application/json": { "schema": output } },
            }),
        );
        let own = json!({ "$ref": format!("#/components/responses/{result}") });
        responses.insert("200".to_owned(), own.clone());
        // A refusal (403, 404, 409, 422) is the operation's own result, as
        // is 503; the statuses every operation shares (401, 413, 429) are
        // named, and take precedence over the 4XX range.
        responses.insert("4XX".to_owned(), own.clone());
        for (status, _) in ERROR_STATUSES {
            let response = if *status == "503" {
                own.clone()
            } else if matches!(*status, "403" | "404" | "409" | "422") {
                continue;
            } else {
                // The same for every operation, so referenced rather than
                // repeated, which keeps the document small as tools are added.
                json!({ "$ref": format!("#/components/responses/Error{status}") })
            };
            responses.insert((*status).to_owned(), response);
        }
        paths.insert(
            format!("/{name}"),
            json!({
                "post": {
                    "operationId": name,
                    "summary": title,
                    "description": tool.description.as_deref().unwrap_or_default(),
                    "tags": [tag],
                    "x-read-only": read_only,
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": input } },
                    },
                    "responses": responses,
                }
            }),
        );
    }
    merge_identical(&mut components, &mut [&mut paths, &mut refusals]);
    json!({
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
            "responses": ERROR_STATUSES
                .iter()
                .filter(|(status, _)| !matches!(*status, "403" | "404" | "409" | "422" | "503"))
                .map(|(status, description)| {
                    (
                        format!("Error{status}"),
                        json!({
                            "description": description,
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" },
                                },
                            },
                        }),
                    )
                })
                .chain(refusals)
                .collect::<Map<_, _>>(),
            "securitySchemes": {
                "bearerToken": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "A personal API token, a connected app's token or an agent's token.",
                },
            },
        },
    })
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
        // The reference page fetches it whole, so keep it under a megabyte.
        // Raised from 768 KiB with AWD-62's question tools; past this, serve
        // it compressed or slim the result schemas rather than raise it again.
        let size = serde_json::to_vec(&document).expect("serialize").len();
        assert!(size < 896 * 1024, "the document is {size} bytes");
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
