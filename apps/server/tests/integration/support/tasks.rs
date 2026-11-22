//! Task writes as the web app makes them now: through the task operations
//! (`task_create`, `task_update`, `task_archive`, `task_restore`,
//! `task_link`, `task_update_many`, `task_move`), which replaced the
//! `/api/v1/projects/{id}/work-items…` write routes (K2.22).
//!
//! Tests written against those routes keep their bodies: [`send`] takes the
//! old method, address and body, runs the operation they meant, and answers
//! as the route did when it works (the task as it reads now, `201` for a
//! new one), or with the operation's whole answer when it doesn't.

use axum::http::StatusCode;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::app::{Browser, send as send_request};

/// One old task write, as an operation: its name, its input, the space it
/// is in, and the task it answers with (None for a new task, whose id comes
/// back from the operation).
#[derive(Debug)]
pub(crate) struct TaskOp {
    pub(crate) name: &'static str,
    pub(crate) input: Value,
    pub(crate) space: String,
    pub(crate) task: Option<String>,
}

/// The operation an old task write meant, or None when `path` isn't one.
pub(crate) fn op_for(method: &str, path: &str, body: &Value) -> Option<TaskOp> {
    let rest = path.split('?').next()?.strip_prefix("/api/v1/projects/")?;
    let parts = rest.split('/').collect::<Vec<_>>();
    let space = (*parts.first()?).to_owned();
    if parts.get(1) != Some(&"work-items") {
        return None;
    }
    let task = parts.get(2).map(|task| (*task).to_owned());
    let at = |name: &'static str, input: Value| TaskOp {
        name,
        input,
        space: space.clone(),
        task: task.clone(),
    };
    match (method, &parts[2..]) {
        ("POST", []) => Some(TaskOp {
            task: None,
            ..at("task_create", create_input(&space, body))
        }),
        ("POST", ["bulk"]) => Some(TaskOp {
            task: None,
            ..at("task_update_many", bulk_input(&space, body))
        }),
        ("PATCH", [task]) => Some(at("task_update", update_input(task, body))),
        ("POST", [task, "move"]) => Some(at("task_update", move_input(task, body))),
        ("POST", [task, verb @ ("archive" | "restore")]) => Some(at(
            if *verb == "archive" {
                "task_archive"
            } else {
                "task_restore"
            },
            json!({"taskId": task, "expectedVersion": body["expectedVersion"]}),
        )),
        ("POST", [task, "threads", conversation]) => Some(at(
            "task_link",
            json!({"taskId": task, "conversationId": conversation}),
        )),
        ("POST", [task, "repositories", repository]) => {
            let mut input = json!({"taskId": task, "repositoryId": repository});
            if let Some(role) = body.get("role").filter(|role| !role.is_null()) {
                input["role"] = role.clone();
            }
            Some(at("task_link", input))
        }
        ("POST", [task, "project-move"]) => Some(TaskOp {
            task: None,
            ..at("task_move", project_move_input(task, body))
        }),
        _ => None,
    }
}

/// Runs an old task write through its operation as `browser`, with `key`
/// as its request id (a fresh one when None).
pub(crate) async fn send(
    app: &axum::Router,
    browser: &Browser,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let op = op_for(method, path, &body)
        .unwrap_or_else(|| panic!("{method} {path} is not a task write"));
    let key = key
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let (status, answer) = send_request(
        app,
        browser.request(
            "POST",
            &format!("/api/v2/ops/{}", op.name),
            Some(&key),
            op.input,
        ),
    )
    .await;
    if answer["outcome"] != "ok" {
        return (status, answer);
    }
    let value = answer["value"].clone();
    let task = match (op.name, op.task) {
        // The bulk change and the move answer with what they did.
        ("task_update_many" | "task_move", _) => return (status, value),
        ("task_create", _) => value["workItemId"].as_str().map(str::to_owned),
        (_, task) => task,
    };
    let Some(task) = task else {
        return (status, value);
    };
    let (read, item) = send_request(
        app,
        browser.request(
            "GET",
            &format!("/api/v1/projects/{}/work-items/{task}", op.space),
            None,
            Value::Null,
        ),
    )
    .await;
    let status = if op.name == "task_create" {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    // Someone who may change a task without seeing it gets the
    // operation's own answer.
    if read != StatusCode::OK {
        return (status, value);
    }
    (status, item)
}

/// [`send`] for a request a test already built (with
/// [`Browser::request`] or by hand): its method, address, request id,
/// session and body, through the operation.
pub(crate) async fn send_built(
    app: &axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> (StatusCode, Value) {
    let (parts, body) = request.into_parts();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let cookie = header("cookie").unwrap_or_default();
    let csrf = header("x-csrf-token").unwrap_or_default();
    let key = header("idempotency-key");
    let bytes = axum::body::to_bytes(body, 4_000_000)
        .await
        .expect("request body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    send(
        app,
        &browser(&cookie, &csrf),
        parts.method.as_str(),
        &parts.uri.to_string(),
        key.as_deref(),
        body,
    )
    .await
}

/// Builds a `Browser` from a session's cookie and CSRF token, for clients
/// that keep their own.
pub(crate) fn browser(cookie: &str, csrf: &str) -> Browser {
    Browser {
        cookie: cookie.to_owned(),
        csrf: csrf.to_owned(),
    }
}

fn renamed(body: &Value, names: &[(&str, &str)]) -> Map<String, Value> {
    let mut input = Map::new();
    let Some(object) = body.as_object() else {
        return input;
    };
    for (key, value) in object {
        let name = names
            .iter()
            .find(|(old, _)| old == key)
            .map_or(key.as_str(), |(_, new)| new);
        input.insert(name.to_owned(), value.clone());
    }
    input
}

fn create_input(space: &str, body: &Value) -> Value {
    let mut input = renamed(
        body,
        &[
            ("typeId", "type"),
            ("statusId", "status"),
            ("assigneeActorId", "assignee"),
            ("parentWorkItemId", "parentId"),
            ("milestoneId", "milestone"),
            ("labelIds", "labels"),
            ("fieldValues", "fields"),
            ("sourceMessageId", "fromMessageId"),
            ("targetPosition", "position"),
            ("beforeId", "beforeTaskId"),
            ("afterId", "afterTaskId"),
        ],
    );
    // Empty or null values were "not given" there.
    input.retain(|_, value| {
        !(value.is_null()
            || value.as_str() == Some("")
            || value.as_array().is_some_and(Vec::is_empty))
    });
    if let Some(position) = input.get("position").and_then(Value::as_i64) {
        input.insert("position".to_owned(), json!(position.to_string()));
    }
    // A subtask goes in its parent's space, named or not.
    if !input.contains_key("parentId") {
        input.insert("projectId".to_owned(), json!(space));
    }
    Value::Object(input)
}

fn update_input(task: &str, body: &Value) -> Value {
    let mut input = Map::new();
    input.insert("taskId".to_owned(), json!(task));
    let Some(object) = body.as_object() else {
        return Value::Object(input);
    };
    for (key, value) in object {
        let null = value.is_null();
        let (name, value) = match key.as_str() {
            "typeId" => ("type", value.clone()),
            "statusId" => ("status", value.clone()),
            "assigneeActorId" if null => ("clearAssignee", json!(true)),
            "assigneeActorId" => ("assignee", value.clone()),
            "delegateActorId" if null => ("delegate", json!("none")),
            "delegateActorId" => ("delegate", value.clone()),
            "parentWorkItemId" if null => ("clearParent", json!(true)),
            "parentWorkItemId" => ("parentId", value.clone()),
            "milestoneId" if null => ("milestone", json!("none")),
            "milestoneId" => ("milestone", value.clone()),
            "startDate" | "dueDate" if null => (key.as_str(), json!("")),
            "estimate" if null => ("clearEstimate", json!(true)),
            "blockedReason" if null => ("unblock", json!(true)),
            "addLabelIds" => ("addLabels", value.clone()),
            "removeLabelIds" => ("removeLabels", value.clone()),
            "fieldValues" => ("fields", value.clone()),
            _ => (key.as_str(), value.clone()),
        };
        input.insert(name.to_owned(), value);
    }
    Value::Object(input)
}

fn move_input(task: &str, body: &Value) -> Value {
    let mut input = json!({
        "taskId": task,
        "expectedVersion": body["expectedVersion"],
        "status": body["targetStatusId"],
    });
    if let Some(position) = body.get("targetPosition").filter(|value| !value.is_null()) {
        input["position"] = json!(position.as_i64().map_or_else(
            || position.as_str().unwrap_or_default().to_owned(),
            |position| position.to_string()
        ));
    }
    for (old, new) in [("beforeId", "beforeTaskId"), ("afterId", "afterTaskId")] {
        if let Some(anchor) = body.get(old).filter(|value| !value.is_null()) {
            input[new] = anchor.clone();
        }
    }
    input
}

fn bulk_input(space: &str, body: &Value) -> Value {
    let items = body["items"].as_array().cloned().unwrap_or_default();
    let mut input = json!({
        "projectId": space,
        "taskIds": items.iter().map(|item| item["id"].clone()).collect::<Vec<_>>(),
    });
    let versions = items
        .iter()
        .filter(|item| !item["expectedVersion"].is_null())
        .filter_map(|item| {
            Some((
                item["id"].as_str()?.to_owned(),
                item["expectedVersion"].clone(),
            ))
        })
        .collect::<Map<_, _>>();
    if !versions.is_empty() {
        input["expectedVersions"] = Value::Object(versions);
    }
    let changes = &body["changes"];
    for (old, new) in [
        ("statusId", "status"),
        ("priority", "priority"),
        ("addLabelIds", "addLabels"),
        ("removeLabelIds", "removeLabels"),
        ("archived", "archived"),
        ("fieldValues", "fields"),
    ] {
        if let Some(value) = changes.get(old).filter(|value| !value.is_null()) {
            input[new] = value.clone();
        }
    }
    match changes.get("assigneeActorId") {
        Some(Value::Null) => input["clearAssignee"] = json!(true),
        Some(actor) => input["assignee"] = actor.clone(),
        None => {}
    }
    match changes.get("milestoneId") {
        Some(Value::Null) => input["milestone"] = json!("none"),
        Some(milestone) => input["milestone"] = milestone.clone(),
        None => {}
    }
    match changes.get("dueDate") {
        Some(Value::Null) => input["dueDate"] = json!(""),
        Some(date) => input["dueDate"] = date.clone(),
        None => {}
    }
    input
}

fn project_move_input(task: &str, body: &Value) -> Value {
    let mut input = renamed(body, &[("targetProjectId", "toProjectId")]);
    input.insert("taskId".to_owned(), json!(task));
    Value::Object(input)
}
