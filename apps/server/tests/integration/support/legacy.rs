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

/// One old write as its operation: the input, and where the old answer is
/// read back from (when it can be).
struct Translated {
    name: &'static str,
    input: Value,
    read_back: fn(&Value) -> Option<String>,
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
        }),
        ("PATCH", ["pages", page]) => Some(Translated {
            name: "page_update",
            input: with(body, &[("pageId", Value::from(*page))]),
            read_back: page_of,
        }),
        ("POST", ["channels" | "threads", conversation, action @ ("archive" | "restore")]) => {
            Some(Translated {
                name: "conversation_update",
                input: with(
                    body,
                    &[
                        ("conversationId", Value::from(*conversation)),
                        ("archived", Value::from(*action == "archive")),
                    ],
                ),
                read_back: |_| None,
            })
        }
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
    let bytes = to_bytes(body, 4_000_000).await.expect("request body");
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
        return Ok((status, answer));
    }
    let value = answer["value"].clone();
    let Some(address) = (op.read_back)(&value) else {
        return Ok((status, value));
    };
    let mut read = Request::builder().method("GET").uri(address);
    for name in ["origin", "cookie", "authorization"] {
        if let Some(header) = header(name) {
            read = read.header(name, header);
        }
    }
    let (read_status, mut thing) =
        exchange(app, read.body(Body::empty()).expect("read back")).await;
    if read_status != StatusCode::OK {
        return Ok((status, value));
    }
    if let Some(suggestion) = value["suggestion"]["suggestionId"].as_str() {
        thing["suggestionId"] = Value::from(suggestion);
    }
    Ok((StatusCode::OK, thing))
}
