//! Web writes that became operations (K2.22), for tests still written
//! against the old addresses. [`app::send`](super::app::send) runs such a
//! request through its operation, as the web now does, and reads the thing
//! back the way the old route answered, so a test keeps its assertions.
//! New tests call the operation directly (`Browser::op`).

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Map, Value};
use tower::ServiceExt;
use uuid::Uuid;

/// One old write as its operation: the input, where the old answer is read
/// back from (when it can be), and how the old route shaped it.
struct Translated {
    name: &'static str,
    input: Value,
    read_back: fn(&Value) -> Option<String>,
    shape: fn(&Value, Value) -> (StatusCode, Value),
}

/// The thing read back, as it is.
fn as_read(value: &Value, mut thing: Value) -> (StatusCode, Value) {
    if let Some(suggestion) = value["suggestion"]["suggestionId"].as_str() {
        thing["suggestionId"] = Value::from(suggestion);
    }
    (StatusCode::OK, thing)
}

/// One message as the timeline shows it, for the old message routes.
fn message_of(value: &Value) -> Option<String> {
    Some(format!(
        "/api/v1/conversations/{}/messages/{}",
        value["conversationId"].as_str()?,
        value["messageId"].as_str()?
    ))
}

/// The old message routes answered with the conversation, the message and
/// its timeline entry.
fn message_answer(value: &Value, read: &Value) -> Value {
    serde_json::json!({
        "conversationId": value["conversationId"],
        "message": read["entry"]["message"],
        "entry": read["entry"],
    })
}

fn sent(value: &Value, read: Value) -> (StatusCode, Value) {
    let status = if value["idempotentReplay"] == true {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (status, message_answer(value, &read))
}

/// The old thread routes answered with the thread and its read state.
fn started(value: &Value, read: Value) -> (StatusCode, Value) {
    let status = if value["created"] == false {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (
        status,
        serde_json::json!({
            "conversation": read["conversation"],
            "readState": read["readState"],
        }),
    )
}

/// The old member routes answered with everyone in the conversation after.
fn members(value: &Value, _: Value) -> (StatusCode, Value) {
    let members = value["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| {
            serde_json::json!({
                "id": member["id"],
                "kind": member["kind"],
                "displayName": member["name"],
                "handle": member["handle"],
            })
        })
        .collect::<Vec<_>>();
    (
        StatusCode::OK,
        serde_json::json!({
            "conversationId": value["conversationId"],
            "version": value["version"],
            "members": members,
        }),
    )
}

fn edited(value: &Value, read: Value) -> (StatusCode, Value) {
    let mut answer = message_answer(value, &read);
    answer["changed"] = value["changed"].clone();
    (StatusCode::OK, answer)
}

fn removed(value: &Value, read: Value) -> (StatusCode, Value) {
    let mut answer = message_answer(value, &read);
    answer["alreadyRemoved"] = value["alreadyRemoved"].clone();
    (StatusCode::OK, answer)
}

/// Only the fields of `body` an operation takes, renamed where it names them
/// differently.
fn picked(body: &Value, fields: &[(&str, &str)], extra: &[(&str, Value)]) -> Value {
    let mut input = Map::new();
    for (from, to) in fields {
        if let Some(value) = body.get(*from).filter(|value| !value.is_null()) {
            input.insert((*to).to_owned(), value.clone());
        }
    }
    for (key, value) in extra {
        input.insert((*key).to_owned(), value.clone());
    }
    // Clients never chose authorship; the operation refuses one that tries.
    if let Some(author) = body.get("authorActorId") {
        input.insert("authorActorId".to_owned(), author.clone());
    }
    Value::Object(input)
}

fn with(body: &Value, extra: &[(&str, Value)]) -> Value {
    let mut input = body.as_object().cloned().unwrap_or_else(Map::new);
    for (key, value) in extra {
        input.insert((*key).to_owned(), value.clone());
    }
    Value::Object(input)
}

fn page_of(value: &Value) -> Option<String> {
    value["id"].as_str().map(|id| format!("/api/v1/pages/{id}"))
}

fn translate(method: &str, path: &str, body: &Value) -> Option<Translated> {
    let path = path.split('?').next()?;
    let parts = path
        .strip_prefix("/api/v1/")?
        .split('/')
        .collect::<Vec<_>>();
    match (method, parts.as_slice()) {
        ("POST", ["projects", project, "pages"]) => Some(Translated {
            name: "page_create",
            input: with(body, &[("projectId", Value::from(*project))]),
            read_back: page_of,
            shape: as_read,
        }),
        ("PATCH", ["pages", page]) => Some(Translated {
            name: "page_update",
            input: with(body, &[("pageId", Value::from(*page))]),
            read_back: page_of,
            shape: as_read,
        }),
        (
            "POST",
            [
                "channels" | "threads",
                conversation,
                action @ ("archive" | "restore"),
            ],
        ) => Some(Translated {
            name: "conversation_update",
            input: with(
                body,
                &[
                    ("conversationId", Value::from(*conversation)),
                    ("archived", Value::from(*action == "archive")),
                ],
            ),
            read_back: |_| None,
            shape: |value, _| (StatusCode::OK, value.clone()),
        }),
        (
            "POST",
            [
                "channels" | "threads" | "conversations",
                conversation,
                "messages",
            ],
        ) => Some(Translated {
            name: "message_send",
            input: picked(
                body,
                &[
                    ("content", "content"),
                    ("attachments", "attachmentIds"),
                    ("replyToMessageId", "replyToMessageId"),
                ],
                &[("conversationId", Value::from(*conversation))],
            ),
            read_back: message_of,
            shape: sent,
        }),
        ("POST", ["channels" | "conversations", conversation, "threads"]) => Some(Translated {
            name: "thread_start",
            input: picked(
                body,
                &[("name", "name"), ("originMessageId", "fromMessageId")],
                &[("conversationId", Value::from(*conversation))],
            ),
            read_back: |value| {
                value["id"]
                    .as_str()
                    .map(|id| format!("/api/v1/conversations/{id}/timeline"))
            },
            shape: started,
        }),
        ("POST", ["projects", project, "channels"]) => Some(Translated {
            name: "channel_create",
            input: picked(
                body,
                &[
                    ("name", "name"),
                    ("private", "private"),
                    ("autoJoin", "autoJoin"),
                ],
                &[("projectId", Value::from(*project))],
            ),
            read_back: |value| {
                value["id"]
                    .as_str()
                    .map(|id| format!("/api/v1/conversations/{id}/timeline"))
            },
            shape: started,
        }),
        ("POST", ["conversations", conversation, "members"]) => Some(Translated {
            name: "conversation_members_update",
            input: serde_json::json!({
                "conversationId": conversation,
                "add": [body["actorId"].clone()],
            }),
            read_back: |_| None,
            shape: members,
        }),
        ("POST", ["conversations", conversation, "members", member, "remove"]) => {
            Some(Translated {
                name: "conversation_members_update",
                input: serde_json::json!({
                    "conversationId": conversation,
                    "remove": [member],
                }),
                read_back: |_| None,
                shape: members,
            })
        }
        ("PATCH", ["conversations", _, "messages", message]) => Some(Translated {
            name: "message_update",
            input: picked(
                body,
                &[
                    ("content", "content"),
                    ("expectedRevision", "expectedRevision"),
                ],
                &[("messageId", Value::from(*message))],
            ),
            read_back: message_of,
            shape: edited,
        }),
        ("DELETE", ["conversations", _, "messages", message]) => Some(Translated {
            name: "message_delete",
            input: picked(body, &[], &[("messageId", Value::from(*message))]),
            read_back: message_of,
            shape: removed,
        }),
        _ => None,
    }
}

async fn exchange(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Runs `request` through its operation when it is an old web write, or
/// gives it back untouched.
pub(crate) async fn send(
    app: &axum::Router,
    request: Request<Body>,
) -> Result<(StatusCode, Value), Request<Body>> {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if method == "GET" || !path.starts_with("/api/v1/") {
        return Err(request);
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.expect("request body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(op) = translate(&method, &path, &body) else {
        return Err(Request::from_parts(parts, Body::from(bytes)));
    };
    let header = |name: &str| parts.headers.get(name).cloned();
    let key = header("idempotency-key")
        .and_then(|value| value.to_str().ok().map(str::to_owned))
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let mut call = Request::builder()
        .method("POST")
        .uri(format!("/api/v2/ops/{}", op.name))
        .header("content-type", "application/json")
        .header("idempotency-key", key);
    for name in ["origin", "cookie", "x-csrf-token", "authorization"] {
        if let Some(value) = header(name) {
            call = call.header(name, value);
        }
    }
    let call = call
        .body(Body::from(serde_json::to_vec(&op.input).expect("input")))
        .expect("operation request");
    let (status, answer) = exchange(app, call).await;
    if answer["outcome"] != "ok" {
        // The old routes named the refusal under `error`, by its own code
        // or else its outcome, and answered a bad input with 400.
        let mut answer = answer;
        let code = match (answer["outcome"].as_str(), answer["code"].as_str()) {
            (Some("conflict"), Some("state" | "version")) => Value::from("version_conflict"),
            (_, Some(code)) => Value::from(code),
            (outcome, None) => outcome.map_or(Value::Null, Value::from),
        };
        answer["error"] = serde_json::json!({
            "code": code,
            "message": answer["message"].clone(),
        });
        let status = if status == StatusCode::UNPROCESSABLE_ENTITY {
            StatusCode::BAD_REQUEST
        } else {
            status
        };
        return Ok((status, answer));
    }
    let value = answer["value"].clone();
    let Some(address) = (op.read_back)(&value) else {
        return Ok((op.shape)(&value, Value::Null));
    };
    let mut read = Request::builder().method("GET").uri(address);
    for name in ["origin", "cookie", "authorization"] {
        if let Some(header) = header(name) {
            read = read.header(name, header);
        }
    }
    let (read_status, thing) = exchange(app, read.body(Body::empty()).expect("read back")).await;
    if read_status != StatusCode::OK {
        return Ok((status, value));
    }
    Ok((op.shape)(&value, thing))
}

/// The app as tests build it, with the old web writes run through their
/// operations ([`send`]), so a test that calls the router directly still
/// reaches them.
pub(crate) fn router(state: agent_workspace_server::AppState) -> axum::Router {
    use axum::response::IntoResponse;
    let app = agent_workspace_server::router(state);
    let inner = app.clone();
    app.layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let inner = inner.clone();
            async move {
                match send(&inner, request).await {
                    Ok((status, body)) => (status, axum::Json(body)).into_response(),
                    Err(request) => next.run(request).await,
                }
            }
        },
    ))
}
