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
    state.rate_limits.check_mcp_auth(authentication_key)?;
    let principal = mcp::resolve_bearer_principal(state, headers)
        .await?
        .ok_or(AppError::AuthenticationRequired)?;
    state
        .rate_limits
        .check_mcp_request(principal.credential_id)?;
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

/// The HTTP status for a tool result, from its `outcome`.
pub(crate) fn status_for(result: &Value) -> StatusCode {
    match result.get("outcome").and_then(Value::as_str) {
        Some("not_found") => StatusCode::NOT_FOUND,
        Some("permission_denied") => StatusCode::FORBIDDEN,
        Some("invalid_request" | "invalid_transition") => StatusCode::UNPROCESSABLE_ENTITY,
        Some("conflict") => StatusCode::CONFLICT,
        Some("temporarily_unavailable") => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::OK,
    }
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

/// Moves a schema's `$defs` into `components.schemas` under the tool's name,
/// and points its references there.
fn hoist(tool: &str, schema: &Map<String, Value>, components: &mut Map<String, Value>) -> Value {
    let mut schema = schema.clone();
    schema.remove("$schema");
    let prefix = pascal(tool);
    if let Some(Value::Object(definitions)) = schema.remove("$defs") {
        for (name, definition) in definitions {
            let mut definition = definition;
            rewrite_refs(&mut definition, &prefix);
            components.insert(format!("{prefix}{name}"), definition);
        }
    }
    let mut value = Value::Object(schema);
    rewrite_refs(&mut value, &prefix);
    value
}

fn rewrite_refs(value: &mut Value, prefix: &str) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/$defs/")
            {
                *reference = format!("#/components/schemas/{prefix}{name}");
            }
            for child in object.values_mut() {
                rewrite_refs(child, prefix);
            }
        }
        Value::Array(items) => {
            for child in items {
                rewrite_refs(child, prefix);
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
                        let hoisted = hoist(&root, schema, &mut components);
                        let key = pascal(&root);
                        components.insert(key.clone(), hoisted);
                        results.insert(raw, key.clone());
                        key
                    }
                };
                json!({ "$ref": format!("#/components/schemas/{key}") })
            })
            .unwrap_or_else(|| json!({ "type": "object" }));
        let mut responses = Map::new();
        responses.insert(
            "200".to_owned(),
            json!({
                "description": "The result. Its `outcome` says whether it worked.",
                "content": { "application/json": { "schema": output.clone() } },
            }),
        );
        // A refusal is the operation's own result with its `outcome`: one
        // response per operation that its refusing statuses point at, so the
        // document stays small as tools are added. The introduction says
        // what each status means.
        let refused = format!("{}Refused", pascal(&name));
        refusals.insert(
            refused.clone(),
            json!({
                "description": "Refused: the status and the result's `outcome` and `message` say why.",
                "content": { "application/json": { "schema": output.clone() } },
            }),
        );
        for (status, _) in ERROR_STATUSES {
            let response = if matches!(*status, "403" | "404" | "409" | "422" | "503") {
                json!({ "$ref": format!("#/components/responses/{refused}") })
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
        assert_eq!(pascal("task_create_many"), "TaskCreateMany");
    }
}
