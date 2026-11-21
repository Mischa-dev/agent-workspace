//! M1.12: connected modules. The silent failures: a manifest that makes
//! this server reach its own private network, an app holding a scope nobody
//! approved (or keeping one its manifest dropped), a re-fetched manifest
//! widening what it may do before anyone approves it, an install code that
//! works twice or reaches someone other than the manifest's address, and a
//! revoked app whose next call still goes through.

use std::sync::{Arc, Mutex};

use agent_workspace_server::jobs::{self, WorkerOptions};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode, header},
    routing::{get, post},
};
use serde_json::{Value, json};

use super::support::{
    app::send,
    callers::{Call, Caller, drain_outbox},
    world::{Workspace, WorldBuilder},
};

/// The test app: the manifest it serves, and the install and operation
/// calls it got.
#[derive(Default)]
struct TestApp {
    origin: String,
    manifest: Value,
    installs: Vec<(HeaderMap, String)>,
    calls: Vec<(HeaderMap, String)>,
    /// The fake file source answers its list outside the contract.
    broken: bool,
}

type Shared = Arc<Mutex<TestApp>>;

async fn manifest(State(app): State<Shared>) -> Json<Value> {
    Json(app.lock().expect("app").manifest.clone())
}

async fn install(State(app): State<Shared>, headers: HeaderMap, body: String) -> StatusCode {
    app.lock().expect("app").installs.push((headers, body));
    StatusCode::OK
}

/// The builds panel: a form to start a build; an empty branch is the
/// form's fault, a branch starts one, and `bad` answers a form that asks
/// for a secret, which Workspace refuses to pass on.
async fn panel(
    State(app): State<Shared>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    let sent: Value = serde_json::from_str(&body).unwrap_or_default();
    app.lock().expect("app").calls.push((headers, body));
    let form = |field: &str| {
        json!({"format": "aw-panel/1", "title": "Builds", "blocks": [
            {"type": "status", "state": "ok", "label": "Nothing is running"},
            {"type": "form", "id": "start",
             "schema": {"type": "object", "properties": {field: {"type": "string"}}},
             "submit": {"label": "Start", "action": {"id": "start"}}}
        ], "state": "page=1"})
    };
    let answer = match sent["action"]["id"].as_str() {
        None => json!({"panel": form("branch")}),
        Some("start")
            if sent["action"]["values"]["branch"]
                .as_str()
                .unwrap_or_default()
                .is_empty() =>
        {
            json!({"errors": {"branch": "A branch is needed."}})
        }
        Some("start") => json!({"done": {"message": "Build started.", "result": {"build": "215"}}}),
        Some("bad") => json!({"panel": form("api_token")}),
        // M1.15: a frame on the app's own origin, or on one it never
        // declared; and the result a finished frame asks for.
        Some("keys") => {
            let origin = app.lock().expect("app").origin.clone();
            json!({"frame": {"url": format!("{origin}/keys?step=1"), "title": "Model keys",
                             "height": 640}})
        }
        Some("elsewhere") => {
            json!({"frame": {"url": "https://elsewhere.example/keys", "title": "Keys"}})
        }
        Some("aw.frame_done") if sent["action"]["values"]["ref"] == "saved-1" => {
            json!({"done": {"message": "Keys saved.", "result": {"saved": true}}})
        }
        Some("aw.frame_done") => json!({"errors": {"keys": "Nothing was saved."}}),
        // M1.16: a host's create panel says which agent it made (and tries
        // to name the proposal itself, which only Workspace may).
        Some("create_agent") => json!({"done": {"message": "Created.", "result": {
            "agentRef": sent["action"]["values"]["ref"], "proposalId": "forged",
            "displayName": "Fixer"}}}),
        Some(_) => json!({"close": true}),
    };
    (StatusCode::OK, Json(answer))
}

/// A fake file source (M1.18): one folder holding one file. Its list
/// carries a field a store might leak (a token), which must never pass.
async fn files(
    State(app): State<Shared>,
    axum::extract::Path(call): axum::extract::Path<String>,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let sent: Value = serde_json::from_str(&body).unwrap_or_default();
    let broken = {
        let mut app = app.lock().expect("app");
        app.calls.push((headers, body));
        app.broken
    };
    let file = json!({"ref": "f1", "name": "plan.md", "kind": "file", "size": 5,
                      "mime": "text/markdown", "modifiedAt": "2026-11-19T08:00:00Z",
                      "webUrl": "https://files.example.com/f1", "accessToken": "store-token-1"});
    let folder = json!({"ref": "d1", "name": "Plans", "kind": "folder", "modifiedAt": "2026-11-18T08:00:00Z"});
    let answer = match call.as_str() {
        "list" if broken => json!({"files": [file]}),
        "list" if sent["input"]["folder"].is_null() => {
            json!({"items": [folder, file], "nextCursor": null})
        }
        "list" => json!({"items": []}),
        "get" if sent["input"]["ref"] == "f1" => file,
        "get" if sent["input"]["ref"] == "d1" => folder,
        "get" => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "no such file"})),
            )
                .into_response();
        }
        "content" => return "# Plan".into_response(),
        "thumbnail" => json!({"url": null}),
        "changes" => json!({"changes": [{"ref": "f1", "change": "updated"}], "cursor": "c1"}),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    Json(answer).into_response()
}

/// A declared operation: answers with what it was asked, or refuses an
/// unknown build as the input's fault.
async fn rerun(
    State(app): State<Shared>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    let sent: Value = serde_json::from_str(&body).unwrap_or_default();
    app.lock().expect("app").calls.push((headers, body));
    let build = sent["input"]["build"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if build == "404" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": {"message": "No such build."}})),
        );
    }
    (StatusCode::OK, Json(json!({"rerun": true, "build": build})))
}

fn builds(origin: &str, scopes: Value) -> Value {
    json!({
        "format": "agent-workspace-module/1",
        "id": "acme.builds",
        "name": "Builds",
        "version": "1.0.0",
        "description": "Shows each build and links builds to tasks.",
        "publisher": {"name": "Acme"},
        "baseUrl": format!("{origin}/workspace/"),
        "install": "install",
        "account": {"name": "Builds"},
        "scopes": scopes,
        "kinds": [{"kind": "acme.build", "one": "build", "many": "builds"}],
        "events": {"emits": [{"type": "acme.build.finished", "kind": "acme.build"}]},
        "panels": [{"id": "builds", "slot": "space.tab", "title": "Builds", "entry": "panels/builds"}],
    })
}

fn first_scopes() -> Value {
    json!([
        {"scope": "members.view", "why": "To show who started a build.", "required": true},
        {"scope": "modules.manage", "why": "To see its own install."},
        {"scope": "events:task.updated", "why": "To notice tasks moved to Done."}
    ])
}

async fn test_app() -> Shared {
    let shared: Shared = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    {
        let mut app = shared.lock().expect("app");
        app.manifest = builds(&origin, first_scopes());
        app.origin = origin;
    }
    let router = Router::new()
        .route("/manifest.json", get(manifest))
        .route("/workspace/install", post(install))
        .route("/workspace/ops/rerun", post(rerun))
        .route("/workspace/ops/purge", post(rerun))
        .route("/workspace/panels/builds", post(panel))
        .route("/workspace/files/{call}", post(files))
        .with_state(shared.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    shared
}

async fn tool(world: &Workspace, who: &str, name: &'static str, arguments: Value) -> Value {
    let call = Call::Tool { name, arguments };
    match world.agents.get(who) {
        Some(agent) => agent.call(&world.app, call).await.1,
        None => world.apps[who].call(&world.app, call).await.1,
    }
}

/// A call as the connected module's own account, with its secret.
async fn as_app(
    world: &Workspace,
    secret: &str,
    name: &str,
    arguments: Value,
) -> (StatusCode, Value) {
    send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/public/v1/{name}"))
            .header(header::AUTHORIZATION, format!("Bearer {secret}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(arguments.to_string()))
            .expect("request"),
    )
    .await
}

async fn exchange(world: &Workspace, code: &str) -> (StatusCode, Value) {
    send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/connected-modules/token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "code": code }).to_string()))
            .expect("request"),
    )
    .await
}

/// Publishes the outbox and runs due jobs until both are quiet.
async fn pump(world: &Workspace) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "m112-test".to_owned(),
    };
    for _ in 0..10 {
        drain_outbox(&world.state).await;
        let handled = agent_workspace_server::events::catch_up(world.pool())
            .await
            .expect("catch up");
        let ran = jobs::run_due(&world.state, &registry, &options)
            .await
            .expect("run jobs");
        if handled == 0 && ran == 0 {
            return;
        }
    }
}

fn refused(answer: &(StatusCode, Value)) -> bool {
    answer.0 == StatusCode::FORBIDDEN
        || answer.0 == StatusCode::UNAUTHORIZED
        || matches!(
            answer.1["outcome"].as_str(),
            Some("permission_denied" | "forbidden" | "unauthenticated")
        )
}

#[tokio::test]
async fn a_connected_module_holds_what_was_approved_until_it_is_revoked() {
    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .agent("helper", "member")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m112_connected_modules")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let url = format!("{origin}/manifest.json");

    // An address on this server's own network, not one the operator
    // trusts, is never fetched.
    let port = origin.rsplit(':').next().expect("port");
    let private = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": format!("https://127.0.0.1:{port}/manifest.json"), "preview": true}),
    )
    .await;
    assert_eq!(private["outcome"], "invalid_request", "{private}");
    assert!(private.to_string().contains("private network"), "{private}");

    // A preview shows what it would do, and leaves nothing behind.
    let preview = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "preview": true}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let preview = &preview["value"];
    assert_eq!(preview["key"], "acme.builds");
    assert_eq!(preview["scopes"].as_array().expect("scopes").len(), 3);
    // A preview that chose nothing grants only what the app needs, so
    // someone who can't grant an optional scope still sees the app.
    let approved = preview["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .filter(|scope| scope["approved"] == true)
        .map(|scope| scope["scope"].as_str().expect("scope"))
        .collect::<Vec<_>>();
    assert_eq!(approved, ["members.view"], "{preview}");
    let digest = preview["digest"].as_str().expect("digest").to_owned();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM modules WHERE kind = 'connected'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);

    // Installing needs modules.manage; a required scope can't be left out.
    let member = tool(
        &world,
        "helper",
        "connected_module_install",
        json!({"url": url}),
    )
    .await;
    assert_ne!(member["outcome"], "ok", "{member}");
    let partial = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "scopes": ["modules.manage"]}),
    )
    .await;
    assert_eq!(partial["outcome"], "invalid_request", "{partial}");
    let stale = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "digest": "0".repeat(64)}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    let installed = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "scopes": ["members.view"], "digest": digest,
               "clientRequestId": "install-builds"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let module = installed["value"]["moduleId"]
        .as_str()
        .expect("module")
        .to_owned();
    assert_eq!(installed["value"]["state"], "pending");
    // A retry makes nothing twice.
    let again = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "scopes": ["members.view"], "digest": digest,
               "clientRequestId": "install-builds"}),
    )
    .await;
    assert_eq!(again["value"]["moduleId"], module.as_str(), "{again}");
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM module_apps")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(made, 1);

    // The code goes to the manifest's install address, signed.
    pump(&world).await;
    let (headers, body) = app
        .lock()
        .expect("app")
        .installs
        .first()
        .cloned()
        .expect("install call");
    let sent: Value = serde_json::from_str(&body).expect("install body");
    let code = sent["code"].as_str().expect("code").to_owned();
    assert_eq!(sent["module"]["id"], module.as_str());

    let (status, credentials) = exchange(&world, &code).await;
    assert_eq!(status, StatusCode::OK, "{credentials}");
    let secret = credentials["clientSecret"]
        .as_str()
        .expect("secret")
        .to_owned();
    let outbound = credentials["outboundSigningSecret"]
        .as_str()
        .expect("outbound");
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .expect(name)
    };
    assert!(agent_workspace_delivery_contract::webhook::verify_standard(
        outbound,
        header("webhook-id"),
        header("webhook-timestamp"),
        header("webhook-signature"),
        &body,
    ));
    // Once only.
    assert_eq!(exchange(&world, &code).await.0, StatusCode::BAD_REQUEST);
    let state: String = sqlx::query_scalar("SELECT state FROM module_apps")
        .fetch_one(&pool)
        .await
        .expect("state");
    assert_eq!(state, "active");

    // The app acts as its own account, with exactly what was approved.
    let mine = as_app(&world, &secret, "space_list", json!({})).await;
    assert_eq!(mine.0, StatusCode::OK, "{}", mine.1);
    let listing = as_app(&world, &secret, "connected_module_list", json!({})).await;
    assert!(refused(&listing), "{listing:?}");
    // The role operations read its standing like anyone's.
    let roles = as_app(&world, &secret, "role_list", json!({})).await;
    assert_eq!(roles.0, StatusCode::OK, "{}", roles.1);
    let app_role = tool(&world, "desk", "role_get", json!({"role": "Builds (app)"})).await;
    let role_edit = tool(
        &world,
        "desk",
        "role_update",
        json!({"role": "Builds (app)", "expectedVersion": app_role["value"]["version"],
               "permissions": ["members.view", "roles.manage"]}),
    )
    .await;
    assert!(
        role_edit.to_string().contains("connected module"),
        "an app's role changes only by approval: {role_edit}"
    );

    // A manifest that asks for more waits; what it dropped goes at once.
    app.lock().expect("app").manifest = builds(
        &origin,
        json!([
            {"scope": "members.view", "why": "To show who started a build.", "required": true},
            {"scope": "modules.manage", "why": "To see its own install."},
            {"scope": "tasks.create", "why": "To file a task for a failed build."}
        ]),
    );
    let refreshed = tool(
        &world,
        "desk",
        "connected_module_refresh",
        json!({"moduleId": module}),
    )
    .await;
    assert_eq!(refreshed["value"]["refreshed"], "pending", "{refreshed}");
    assert!(refreshed["value"]["module"]["pending"].is_object());
    let listing = as_app(&world, &secret, "connected_module_list", json!({})).await;
    assert!(
        refused(&listing),
        "nothing applies before approval: {listing:?}"
    );

    // A member can't approve; the owner approves the scopes it should hold.
    let member = tool(
        &world,
        "helper",
        "connected_module_approve",
        json!({"moduleId": module, "scopes": ["members.view", "modules.manage"]}),
    )
    .await;
    assert_ne!(member["outcome"], "ok", "{member}");
    let approved = tool(
        &world,
        "desk",
        "connected_module_approve",
        json!({"moduleId": module, "scopes": ["members.view", "modules.manage"]}),
    )
    .await;
    assert_eq!(approved["outcome"], "ok", "{approved}");
    assert!(approved["value"]["pending"].is_null());
    let listing = as_app(&world, &secret, "connected_module_list", json!({})).await;
    assert_eq!(listing.0, StatusCode::OK, "{listing:?}");

    // Dropping a scope from the manifest takes it away at once, when the
    // daily re-read finds it: no one has to ask.
    app.lock().expect("app").manifest = builds(
        &origin,
        json!([{"scope": "members.view", "why": "To show who started a build.", "required": true}]),
    );
    sqlx::query(
        "UPDATE module_apps SET manifest_fetched_at = now() - interval '2 days' \
         WHERE module_id = $1::uuid",
    )
    .bind(module.as_str())
    .execute(&pool)
    .await
    .expect("an old read");
    jobs::enqueue(&pool, jobs::NewJob::new("connected_module.refetch_sweep"))
        .await
        .expect("sweep");
    pump(&world).await;
    let scopes: Vec<String> =
        sqlx::query_scalar("SELECT scopes_approved FROM module_apps WHERE module_id = $1::uuid")
            .bind(module.as_str())
            .fetch_one(&pool)
            .await
            .expect("scopes");
    assert_eq!(scopes, ["members.view"]);
    let listing = as_app(&world, &secret, "connected_module_list", json!({})).await;
    assert!(refused(&listing), "{listing:?}");
    let announced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE type = 'module.connected' AND thing_id = $1::uuid",
    )
    .bind(module.as_str())
    .fetch_one(&pool)
    .await
    .expect("events");
    assert!(announced > 0, "the workspace hears of it");
    let again = tool(
        &world,
        "desk",
        "connected_module_refresh",
        json!({"moduleId": module}),
    )
    .await;
    assert_eq!(again["value"]["refreshed"], "unchanged", "{again}");

    // Revoked: the next call is refused, the module is off, and the code
    // and secrets are gone.
    let revoked = tool(
        &world,
        "desk",
        "connected_module_revoke",
        json!({"moduleId": module, "clientRequestId": "revoke-builds"}),
    )
    .await;
    assert_eq!(revoked["outcome"], "ok", "{revoked}");
    assert_eq!(revoked["value"]["state"], "revoked");
    let after = as_app(&world, &secret, "space_list", json!({})).await;
    assert!(refused(&after), "{after:?}");
    let (on, secrets): (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT install.state, app.outbound_secret_id FROM module_apps app \
         JOIN module_installs install ON install.module_key = 'acme.builds' \
          AND install.workspace_id = app.workspace_id AND install.space_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("switch");
    assert_eq!(on, "off");
    assert!(secrets.is_none());
    world.finish().await;
}

/// The builds manifest with two declared operations: rerunning a build
/// needs tasks.create, which members hold; purging one needs
/// modules.manage, which they don't.
fn builds_with_operations(origin: &str) -> Value {
    let mut manifest = builds(
        origin,
        json!([
            {"scope": "members.view", "why": "To show who started a build.", "required": true},
            {"scope": "tasks.create", "why": "To file a task for a failed build."},
            {"scope": "modules.manage", "why": "To purge old builds."}
        ]),
    );
    manifest["operations"] = json!([
        {"name": "acme_build_rerun", "verb": "change", "permission": "tasks.create",
         "description": "Run a build again.",
         "input": {"type": "object", "properties": {"build": {"type": "string"}},
                   "required": ["build"], "additionalProperties": false},
         "endpoint": "ops/rerun"},
        {"name": "acme_build_purge", "verb": "change", "permission": "modules.manage",
         "description": "Delete a build's artifacts.",
         "input": {"type": "object", "properties": {"build": {"type": "string"}},
                   "required": ["build"]},
         "endpoint": "ops/purge"}
    ]);
    manifest
}

/// What an install leaves the app: its module, its account's secret, and
/// the two signing secrets.
struct Connected {
    module: String,
    secret: String,
    outbound: String,
    inbound: String,
}

/// Installs the app's manifest as the owner's app and trades the code.
async fn connect(world: &Workspace, app: &Shared, url: &str) -> Connected {
    let installed = tool(
        world,
        "desk",
        "connected_module_install",
        json!({"url": url, "clientRequestId": "install-ops"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let module = installed["value"]["moduleId"]
        .as_str()
        .expect("module")
        .to_owned();
    pump(world).await;
    let body = app
        .lock()
        .expect("app")
        .installs
        .last()
        .cloned()
        .expect("install call")
        .1;
    let sent: Value = serde_json::from_str(&body).expect("install body");
    let (status, credentials) = exchange(world, sent["code"].as_str().expect("code")).await;
    assert_eq!(status, StatusCode::OK, "{credentials}");
    let text = |key: &str| credentials[key].as_str().expect(key).to_owned();
    Connected {
        module,
        secret: text("clientSecret"),
        outbound: text("outboundSigningSecret"),
        inbound: text("inboundSigningSecret"),
    }
}

/// An event the app sends in, signed with `secret` at `timestamp`.
async fn send_event(
    world: &Workspace,
    module: &str,
    secret: &str,
    id: &str,
    timestamp: i64,
    event: &Value,
) -> (StatusCode, Value) {
    let body = event.to_string();
    let signature =
        agent_workspace_host_protocol::sign_push(secret, id, timestamp, &body).expect("sign");
    send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/modules/{module}/events"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("webhook-id", id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", signature)
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

#[tokio::test]
async fn a_connected_module_runs_its_operations_and_sends_in_its_events() {
    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    app.lock().expect("app").manifest = builds_with_operations(&origin);
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .agent("helper", "member")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m113_remote_operations")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let connected = connect(&world, &app, &format!("{origin}/manifest.json")).await;
    let helper = &world.agents["helper"];

    // Each declared operation is a tool for whoever holds its permission.
    let listed = super::support::app::tools_list(&world.app, &helper.token, &helper.session)
        .await
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert!(
        listed.iter().any(|name| name == "acme_build_rerun"),
        "{listed:?}"
    );
    assert!(
        !listed.iter().any(|name| name == "acme_build_purge"),
        "{listed:?}"
    );

    // A call reaches the module signed with the outbound secret, with the
    // caller and input; its answer is the call's value.
    let ran = tool(
        &world,
        "helper",
        "acme_build_rerun",
        json!({"build": "214"}),
    )
    .await;
    assert_eq!(ran["outcome"], "ok", "{ran}");
    assert_eq!(
        ran["value"]["result"],
        json!({"rerun": true, "build": "214"})
    );
    let (headers, body) = app
        .lock()
        .expect("app")
        .calls
        .last()
        .cloned()
        .expect("call");
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .expect(name)
            .to_owned()
    };
    assert!(agent_workspace_delivery_contract::webhook::verify_standard(
        &connected.outbound,
        &header("webhook-id"),
        &header("webhook-timestamp"),
        &header("webhook-signature"),
        &body,
    ));
    let sent: Value = serde_json::from_str(&body).expect("call body");
    assert_eq!(sent["operation"], "acme_build_rerun");
    assert_eq!(sent["input"], json!({"build": "214"}));
    assert_eq!(sent["caller"]["account"], helper.actor.to_string());
    let calls = app.lock().expect("app").calls.len();

    // Input the schema refuses never leaves; without the permission it is
    // refused; the module's own refusal comes back as the input's fault.
    let typed = tool(&world, "helper", "acme_build_rerun", json!({"build": 214})).await;
    assert_eq!(typed["outcome"], "invalid_request", "{typed}");
    let purge = tool(
        &world,
        "helper",
        "acme_build_purge",
        json!({"build": "214"}),
    )
    .await;
    assert_eq!(purge["outcome"], "permission_denied", "{purge}");
    assert_eq!(app.lock().expect("app").calls.len(), calls);
    let missing = tool(
        &world,
        "helper",
        "acme_build_rerun",
        json!({"build": "404"}),
    )
    .await;
    assert_eq!(missing["outcome"], "invalid_request", "{missing}");
    assert!(missing.to_string().contains("No such build."), "{missing}");
    let owner = tool(&world, "desk", "acme_build_purge", json!({"build": "214"})).await;
    assert_eq!(owner["outcome"], "ok", "{owner}");
    let nothing = tool(&world, "helper", "acme_build_nothing", json!({})).await;
    assert_eq!(nothing["outcome"], "not_found", "{nothing}");

    // The app writes its own things in a space it is in; nobody else writes
    // things of its kind. (It joins the project here as a person would add
    // it: space_member_add doesn't list app accounts yet.)
    let work: uuid::Uuid = world.project("WORK").parse().expect("project id");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, role_id, display_name, handle) \
         SELECT gen_random_uuid(), $1, app.account_id, 'member', app.role_id, 'Builds', 'builds-app' \
         FROM module_apps app WHERE app.module_id = $2::uuid",
    )
    .bind(work)
    .bind(&connected.module)
    .execute(&pool)
    .await
    .expect("app joins the project");
    let made = as_app(
        &world,
        &connected.secret,
        "thing_upsert",
        json!({"kind": "acme.build", "title": "Build 214", "status": "running",
               "spaceId": work, "url": format!("{origin}/b/214"),
               "fields": {"branch": "main"}, "clientRequestId": "build-214"}),
    )
    .await;
    assert_eq!(made.0, StatusCode::OK, "{}", made.1);
    let thing = made.1["value"]["id"].as_str().expect("thing").to_owned();
    let changed = as_app(
        &world,
        &connected.secret,
        "thing_upsert",
        json!({"kind": "acme.build", "id": thing, "title": "Build 214", "status": "passed"}),
    )
    .await;
    assert_eq!(changed.1["value"]["version"], 2, "{}", changed.1);
    assert_eq!(changed.1["value"]["existed"], true);
    // Taken out of the space, the app can't reach the thing it left there.
    let left = sqlx::query(
        "DELETE FROM project_memberships membership USING module_apps app \
         WHERE membership.project_id = $1 AND membership.actor_id = app.account_id \
           AND app.module_id = $2::uuid",
    )
    .bind(work)
    .bind(&connected.module)
    .execute(&pool)
    .await
    .expect("app leaves the project");
    assert_eq!(left.rows_affected(), 1);
    let outside = as_app(
        &world,
        &connected.secret,
        "thing_upsert",
        json!({"kind": "acme.build", "id": thing, "title": "Build 214", "status": "failed"}),
    )
    .await;
    assert_ne!(outside.0, StatusCode::OK, "{}", outside.1);
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, role_id, display_name, handle) \
         SELECT gen_random_uuid(), $1, app.account_id, 'member', app.role_id, 'Builds', 'builds-app' \
         FROM module_apps app WHERE app.module_id = $2::uuid",
    )
    .bind(work)
    .bind(&connected.module)
    .execute(&pool)
    .await
    .expect("app joins the project again");
    let forged = tool(
        &world,
        "helper",
        "thing_upsert",
        json!({"kind": "acme.build", "title": "Fake build", "clientRequestId": "fake"}),
    )
    .await;
    assert_eq!(forged["outcome"], "permission_denied", "{forged}");
    let (kind, space): (String, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT kind, space_id FROM things WHERE id = $1::uuid")
            .bind(&thing)
            .fetch_one(&pool)
            .await
            .expect("thing row");
    assert_eq!(kind, "acme.build");
    assert_eq!(space, Some(work));

    // A rule on the module's event, then the event: forged, stale,
    // undeclared and replayed ones are refused; the real one fires it.
    let rule = tool(
        &world,
        "desk",
        "automation_write",
        json!({"projectId": work, "operation": "create", "name": "Builds",
               "trigger": {"kind": "module_event", "eventType": "acme.build.finished"},
               "actions": [{"kind": "notify", "recipients": "owner", "message": "A build finished"}]}),
    )
    .await;
    assert_eq!(rule["outcome"], "ok", "{rule}");
    let now = chrono::Utc::now().timestamp();
    let finished = json!({"type": "acme.build.finished", "thing": {"id": thing},
                          "data": {"build": "214", "seconds": 81}});
    let forged = send_event(
        &world,
        &connected.module,
        &connected.outbound,
        "evt_1",
        now,
        &finished,
    )
    .await;
    assert_eq!(forged.0, StatusCode::UNAUTHORIZED, "{}", forged.1);
    let stale = send_event(
        &world,
        &connected.module,
        &connected.inbound,
        "evt_1",
        now - 600,
        &finished,
    )
    .await;
    assert_eq!(stale.0, StatusCode::UNAUTHORIZED, "{}", stale.1);
    let undeclared = send_event(
        &world,
        &connected.module,
        &connected.inbound,
        "evt_2",
        now,
        &json!({"type": "acme.build.vanished", "thing": {"id": thing}}),
    )
    .await;
    assert_eq!(undeclared.0, StatusCode::BAD_REQUEST, "{}", undeclared.1);
    let accepted = send_event(
        &world,
        &connected.module,
        &connected.inbound,
        "evt_1",
        now,
        &finished,
    )
    .await;
    assert_eq!(accepted.0, StatusCode::ACCEPTED, "{}", accepted.1);
    let replayed = send_event(
        &world,
        &connected.module,
        &connected.inbound,
        "evt_1",
        now,
        &finished,
    )
    .await;
    assert_eq!(replayed.0, StatusCode::CONFLICT, "{}", replayed.1);
    pump(&world).await;
    let fired: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM notifications WHERE kind = 'automation' ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await
    .expect("notifications");
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert!(
        fired[0].to_string().contains("A build finished"),
        "{fired:?}"
    );
    world.finish().await;
}

#[tokio::test]
async fn a_connected_module_draws_its_panels_and_hears_their_actions() {
    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .agent("helper", "member")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m114_panels")
        .await
    else {
        return;
    };
    let connected = connect(&world, &app, &format!("{origin}/manifest.json")).await;
    let helper = &world.agents["helper"];
    let work = world.project("WORK");

    // A space's tab lists the panel, as a layout part of its own.
    let listed = tool(
        &world,
        "helper",
        "module_panel_list",
        json!({"slot": "space.tab", "spaceId": work}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(
        listed["value"]["panels"],
        json!([{"moduleId": connected.module, "moduleKey": "acme.builds", "panel": "builds",
                "slot": "space.tab", "title": "Builds", "part": "panel.acme.builds.builds"}])
    );
    let elsewhere = tool(
        &world,
        "helper",
        "module_panel_list",
        json!({"slot": "home.card"}),
    )
    .await;
    assert_eq!(elsewhere["value"]["panels"], json!([]), "{elsewhere}");

    // Opening asks the app, signed, naming the viewer and the space.
    let opened = tool(
        &world,
        "helper",
        "module_panel_open",
        json!({"moduleId": connected.module, "panel": "builds", "spaceId": work}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["answer"]["kind"], "panel");
    assert_eq!(
        opened["value"]["answer"]["panel"]["blocks"][1]["id"],
        "start"
    );
    let (headers, body) = app
        .lock()
        .expect("app")
        .calls
        .last()
        .cloned()
        .expect("call");
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .expect(name)
            .to_owned()
    };
    assert!(agent_workspace_delivery_contract::webhook::verify_standard(
        &connected.outbound,
        &header("webhook-id"),
        &header("webhook-timestamp"),
        &header("webhook-signature"),
        &body,
    ));
    let sent: Value = serde_json::from_str(&body).expect("panel body");
    assert_eq!(sent["viewer"]["account"], helper.actor.to_string());
    assert_eq!(sent["space"]["id"], work);
    assert_eq!(sent["context"]["slot"], "space.tab");
    assert!(sent["action"].is_null(), "{sent}");

    // A space's tab isn't drawn without a space.
    let nowhere = tool(
        &world,
        "helper",
        "module_panel_open",
        json!({"moduleId": connected.module, "panel": "builds"}),
    )
    .await;
    assert_eq!(nowhere["outcome"], "not_found", "{nowhere}");

    // The form's round trip: errors under the field, then done.
    let press = |values: Value, id: &str, request: &str| {
        json!({"moduleId": connected.module, "panel": "builds", "spaceId": work,
               "state": "page=1", "action": {"id": id, "values": values},
               "clientRequestId": request})
    };
    let empty = tool(
        &world,
        "helper",
        "module_panel_press",
        press(json!({"branch": ""}), "start", "press-1"),
    )
    .await;
    assert_eq!(empty["outcome"], "ok", "{empty}");
    assert_eq!(
        empty["value"]["answer"],
        json!({"kind": "errors", "errors": {"branch": "A branch is needed."}})
    );
    let started = tool(
        &world,
        "helper",
        "module_panel_press",
        press(json!({"branch": "main"}), "start", "press-2"),
    )
    .await;
    assert_eq!(
        started["value"]["answer"],
        json!({"kind": "done", "message": "Build started.", "result": {"build": "215"}}),
        "{started}"
    );
    let sent: Value = serde_json::from_str(
        &app.lock()
            .expect("app")
            .calls
            .last()
            .cloned()
            .expect("call")
            .1,
    )
    .expect("press body");
    assert_eq!(
        sent["action"],
        json!({"id": "start", "values": {"branch": "main"}})
    );
    assert_eq!(sent["state"], "page=1");

    // A panel asking for a secret never reaches the viewer.
    let bad = tool(
        &world,
        "helper",
        "module_panel_press",
        press(json!({}), "bad", "press-3"),
    )
    .await;
    assert_eq!(bad["outcome"], "invalid_request", "{bad}");
    assert!(bad.to_string().contains("api_token"), "{bad}");
    world.finish().await;
}

/// A raw response: status, headers and text.
async fn raw(world: &Workspace, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    use tower::ServiceExt;
    let response = world.app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// A redemption the app signs with `secret`.
async fn redeem(world: &Workspace, module: &str, secret: &str, token: &str) -> (StatusCode, Value) {
    let body = json!({ "token": token }).to_string();
    let id = uuid::Uuid::now_v7().to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let signature =
        agent_workspace_host_protocol::sign_push(secret, &id, timestamp, &body).expect("sign");
    send(
        &world.app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/modules/{module}/frames/redeem"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("webhook-id", id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", signature)
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

/// M1.15: a panel's frame. Only an origin the manifest declares is framed;
/// the host page frames that one origin, runs no script, opens once and
/// only for its viewer; the launch token redeems once, for that viewer; and
/// a frame finishes only with the result the app gives Workspace itself.
#[tokio::test]
async fn a_framed_panel_opens_once_for_its_viewer_and_finishes_with_the_apps_result() {
    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    app.lock().expect("app").manifest["frameOrigins"] = json!([origin]);
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .person("lena", "member")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m115_frames")
        .await
    else {
        return;
    };
    let connected = connect(&world, &app, &format!("{origin}/manifest.json")).await;
    let work = world.project("WORK").to_owned();
    let lena = &world.people["lena"];
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let press = |action: &str, key: &str| {
        lena.browser.request(
            "POST",
            &op("module_panel_press"),
            Some(key),
            json!({"moduleId": connected.module, "panel": "builds", "spaceId": work,
                   "action": {"id": action}}),
        )
    };

    // A frame on an origin the manifest doesn't declare is refused.
    let (_, elsewhere) = send(&world.app, press("elsewhere", "frame-0")).await;
    assert_eq!(elsewhere["outcome"], "invalid_request", "{elsewhere}");

    let (_, framed) = send(&world.app, press("keys", "frame-1")).await;
    assert_eq!(framed["outcome"], "ok", "{framed}");
    let answer = &framed["value"]["answer"];
    assert_eq!(answer["kind"], "frame", "{framed}");
    assert_eq!(answer["origin"], origin);
    assert_eq!(answer["height"], 640);
    let launch = answer["launchId"].as_str().expect("launch").to_owned();
    let address = answer["address"].as_str().expect("address").to_owned();
    assert_eq!(address, format!("/panel-frame/{launch}"));

    // Finishing before the app redeemed anything does nothing.
    let finish = |reference: &str, key: &str| {
        lena.browser.request(
            "POST",
            &op("module_frame_check"),
            Some(key),
            json!({"launchId": launch, "ref": reference}),
        )
    };
    let (_, early) = send(&world.app, finish("saved-1", "finish-0")).await;
    assert_eq!(early["outcome"], "not_found", "{early}");

    // Someone else's browser can't open it, and doesn't use it up.
    let (status, _, _) = raw(
        &world,
        world
            .owner
            .browser
            .request("GET", &address, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);

    // The viewer's opens it once: one origin framed, no script.
    let (status, headers, page) = raw(
        &world,
        lena.browser.request("GET", &address, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let policy = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .expect("policy")
        .to_owned();
    assert!(policy.contains(&format!("frame-src {origin};")), "{policy}");
    assert!(policy.contains("script-src 'none'"), "{policy}");
    assert!(policy.contains("frame-ancestors 'self'"), "{policy}");
    assert_eq!(
        headers
            .get(header::X_FRAME_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("SAMEORIGIN")
    );
    assert!(!page.contains("<script"), "{page}");
    assert!(
        page.contains(&format!("src=\"{origin}/keys?step=1&amp;aw_launch=")),
        "{page}"
    );
    let token = page
        .split("aw_launch=")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("token")
        .to_owned();
    let (status, _, _) = raw(
        &world,
        lena.browser.request("GET", &address, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "opens once");

    // The app redeems the token once, signed, and learns who it draws for.
    let (status, _) = redeem(&world, &connected.module, &connected.outbound, &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the wrong secret");
    let (status, redeemed) = redeem(&world, &connected.module, &connected.inbound, &token).await;
    assert_eq!(status, StatusCode::OK, "{redeemed}");
    assert_eq!(redeemed["viewer"]["account"], lena.actor.to_string());
    assert_eq!(redeemed["launch"], launch);
    assert_eq!(redeemed["panel"], "builds");
    assert_eq!(redeemed["space"]["id"], work);
    let (status, again) = redeem(&world, &connected.module, &connected.inbound, &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{again}");

    // Someone else can't finish it; the viewer's finish asks the app.
    let (_, theirs) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &op("module_frame_check"),
            Some("finish-owner"),
            json!({"launchId": launch, "ref": "saved-1"}),
        ),
    )
    .await;
    assert_eq!(theirs["outcome"], "not_found", "{theirs}");
    let (_, finished) = send(&world.app, finish("saved-1", "finish-1")).await;
    assert_eq!(finished["outcome"], "ok", "{finished}");
    assert_eq!(
        finished["value"]["answer"],
        json!({"kind": "done", "message": "Keys saved.", "result": {"saved": true}})
    );
    let sent: Value = serde_json::from_str(
        &app.lock()
            .expect("app")
            .calls
            .last()
            .cloned()
            .expect("call")
            .1,
    )
    .expect("finish body");
    assert_eq!(
        sent["action"],
        json!({"id": "aw.frame_done", "values": {"launch": launch, "ref": "saved-1"}})
    );
    assert_eq!(sent["viewer"]["account"], lena.actor.to_string());
    let (_, twice) = send(&world.app, finish("saved-1", "finish-2")).await;
    assert_eq!(twice["outcome"], "not_found", "{twice}");

    // A launch nobody opened in time lapses.
    let (_, framed) = send(&world.app, press("keys", "frame-2")).await;
    let lapsed = framed["value"]["answer"]["address"]
        .as_str()
        .expect("address")
        .to_owned();
    sqlx::query(
        "UPDATE module_frame_launches SET expires_at = now() - interval '1 minute' \
         WHERE id = $1::uuid",
    )
    .bind(
        framed["value"]["answer"]["launchId"]
            .as_str()
            .expect("launch"),
    )
    .execute(world.pool())
    .await
    .expect("lapse");
    let (status, _, _) = raw(
        &world,
        lena.browser.request("GET", &lapsed, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    world.finish().await;
}

#[tokio::test]
async fn a_file_source_passes_its_check_and_lists_files_by_reference() {
    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .agent("helper", "member")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m118_file_sources")
        .await
    else {
        return;
    };
    let url = format!("{origin}/manifest.json");
    let calls = json!({"list": "files/list", "get": "files/get", "content": "files/content",
                       "thumbnail": "files/thumbnail", "changes": "files/changes"});

    // A source that doesn't answer changes can't be installed, and says why.
    let mut missing = calls.clone();
    missing.as_object_mut().expect("calls").remove("changes");
    app.lock().expect("app").manifest["fileSource"] = json!({"calls": missing});
    let refused = tool(
        &world,
        "desk",
        "connected_module_install",
        json!({"url": url, "preview": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert!(
        refused.to_string().contains("fileSource.calls.changes"),
        "{refused}"
    );

    // Installed, it runs the conformance suite once it can sign, and passes.
    app.lock().expect("app").manifest["fileSource"] = json!({"calls": calls});
    let connected = connect(&world, &app, &url).await;
    pump(&world).await;
    let listed = tool(&world, "desk", "connected_module_list", json!({})).await;
    let check = &listed["value"]["modules"][0]["fileCheck"];
    assert_eq!(check["problems"], json!([]), "{listed}");
    let called: Vec<String> = app
        .lock()
        .expect("app")
        .calls
        .iter()
        .filter_map(|(_, body)| serde_json::from_str::<Value>(body).ok())
        .filter_map(|body| body["call"].as_str().map(str::to_owned))
        .collect();
    for call in ["list", "get", "content", "thumbnail", "changes"] {
        assert!(called.iter().any(|made| made == call), "{call}: {called:?}");
    }

    // A member lists and gets files; nothing but the contract comes back.
    let files = tool(
        &world,
        "helper",
        "file_source_list",
        json!({"moduleId": connected.module}),
    )
    .await;
    assert_eq!(files["outcome"], "ok", "{files}");
    assert_eq!(files["value"]["items"][1]["name"], "plan.md");
    assert!(!files.to_string().contains("store-token-1"), "{files}");
    let file = tool(
        &world,
        "helper",
        "file_source_get",
        json!({"moduleId": connected.module, "ref": "f1"}),
    )
    .await;
    assert_eq!(
        file["value"]["webUrl"], "https://files.example.com/f1",
        "{file}"
    );
    let gone = tool(
        &world,
        "helper",
        "file_source_get",
        json!({"moduleId": connected.module, "ref": "f9"}),
    )
    .await;
    assert_eq!(gone["outcome"], "not_found", "{gone}");
    let (headers, body) = app
        .lock()
        .expect("app")
        .calls
        .last()
        .cloned()
        .expect("call");
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .expect(name)
            .to_owned()
    };
    assert!(agent_workspace_delivery_contract::webhook::verify_standard(
        &connected.outbound,
        &header("webhook-id"),
        &header("webhook-timestamp"),
        &header("webhook-signature"),
        &body,
    ));

    // A source answering outside the contract fails its check, with the
    // reason, and is refused until it passes again.
    app.lock().expect("app").broken = true;
    let member = tool(
        &world,
        "helper",
        "connected_module_test",
        json!({"moduleId": connected.module, "clientRequestId": "check-1"}),
    )
    .await;
    assert_eq!(member["outcome"], "permission_denied", "{member}");
    let checked = tool(
        &world,
        "desk",
        "connected_module_test",
        json!({"moduleId": connected.module, "clientRequestId": "check-2"}),
    )
    .await;
    assert_eq!(checked["outcome"], "ok", "{checked}");
    assert_eq!(checked["value"]["problems"][0]["path"], "list", "{checked}");
    let refused = tool(
        &world,
        "helper",
        "file_source_list",
        json!({"moduleId": connected.module}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert!(
        refused.to_string().contains("failed its check"),
        "{refused}"
    );
    app.lock().expect("app").broken = false;
    let passed = tool(
        &world,
        "desk",
        "connected_module_test",
        json!({"moduleId": connected.module, "clientRequestId": "check-3"}),
    )
    .await;
    assert_eq!(passed["value"]["problems"], json!([]), "{passed}");
    let again = tool(
        &world,
        "helper",
        "file_source_list",
        json!({"moduleId": connected.module}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");

    // The source's own grant to its store never shows in the log.
    let logged: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM events WHERE payload::text LIKE '%store-token-1%') \
              + (SELECT count(*) FROM event_staging WHERE payload::text LIKE '%store-token-1%')",
    )
    .fetch_one(world.pool())
    .await
    .expect("events");
    assert_eq!(logged, 0);
    world.finish().await;
}

/// M1.16: an agent host draws its steps as its connected module's panels.
/// The silent failures: an offer or action opening a panel of a module the
/// host isn't joined to, and a panel's `done` handing on an agent the host
/// never proposed here (or naming the proposal itself).
#[tokio::test]
async fn a_hosts_offer_opens_its_module_panel_and_hands_on_only_its_own_agent() {
    use super::support::test_host::TestHost;

    let app = test_app().await;
    let origin = app.lock().expect("app").origin.clone();
    app.lock().expect("app").manifest["panels"]
        .as_array_mut()
        .expect("panels")
        .push(
            json!({"id": "create", "slot": "agents.create", "title": "Create an agent",
                     "entry": "panels/builds"}),
        );
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .config("AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS", origin.clone())
        .build("m116_host_panels")
        .await
    else {
        return;
    };
    let connected = connect(&world, &app, &format!("{origin}/manifest.json")).await;
    // A host may only describe an https address; the test app serves http,
    // so its install takes the address the host describes.
    let address = "https://agents.example.test/aw/manifest.json";
    sqlx::query("UPDATE module_apps SET manifest_url = $2 WHERE module_id = $1::uuid")
        .bind(&connected.module)
        .bind(address)
        .execute(world.pool())
        .await
        .expect("address");
    let owner = &world.owner;
    let op = |name: &'static str, input: Value| {
        let request = owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&uuid::Uuid::now_v7().to_string()),
            input,
        );
        async { send(&world.app, request).await.1 }
    };
    let describe = |manifest: &str| {
        json!({
            "software": {"name": "test-host", "version": "1.0"},
            "capabilities": {"start": true},
            "reportIntervalSeconds": 60,
            "issuer": "https://host.example",
            "module": {"manifestUrl": manifest},
            "offers": [
                {"id": "new", "title": "New agent", "start": {"panel": "create"}},
                {"id": "unknown", "title": "Other", "start": {"panel": "nowhere"}},
            ],
            "actions": [{"id": "settings", "label": "Settings", "needs": "panel"}],
        })
    };
    let mut host = TestHost::enrol(
        &world.app,
        &owner.browser,
        world.organization,
        "Agent host",
        json!({"connection": "dial_out"}),
    )
    .await;
    let (status, described) = host
        .call("POST", "/api/v1/host/describe", Some(describe(address)))
        .await;
    assert_eq!(status, StatusCode::OK, "{described}");
    let other = TestHost::enrol(
        &world.app,
        &owner.browser,
        world.organization,
        "Another host",
        json!({"connection": "dial_out"}),
    )
    .await;
    let (status, described) = other
        .call(
            "POST",
            "/api/v1/host/describe",
            Some(describe("https://other.example.test/aw/manifest.json")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{described}");

    // The offer opens the panel of the host's own module, and only one it
    // declares; another host's same offer opens nothing of this module.
    let started = op(
        "agent_offer_start",
        json!({"host": host.id, "offerId": "new"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    assert_eq!(
        started["value"]["panel"],
        json!({"moduleId": connected.module, "panel": "create"}),
        "{started}"
    );
    let undeclared = op(
        "agent_offer_start",
        json!({"host": host.id, "offerId": "unknown"}),
    )
    .await;
    assert_eq!(undeclared["outcome"], "ok", "{undeclared}");
    assert!(undeclared["value"]["panel"].is_null(), "{undeclared}");
    let elsewhere = op(
        "agent_offer_start",
        json!({"host": other.id, "offerId": "new"}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "ok", "{elsewhere}");
    assert!(elsewhere["value"]["panel"].is_null(), "{elsewhere}");

    // The host proposes the agent it made; the panel's done hands it on
    // beside that proposal, by its name or its outside reference.
    let (status, proposed) = host
        .call(
            "POST",
            "/api/v1/host/agents/register",
            Some(json!({
                "workspaceId": world.organization,
                "hostAgentRef": "fixer-1",
                "profile": {"name": "Fixer", "handle": "fixer", "summary": "Fixes builds"},
                "ref": {"issuer": "https://host.example", "id": "agt_91", "lineage": "l-01"},
                "requestedBy": owner.actor,
                "clientRequestId": "m116-fixer",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{proposed}");
    let proposal = proposed["proposalId"]
        .as_str()
        .expect("proposal")
        .to_owned();
    let finish = |agent_ref: Value| {
        tool(
            &world,
            "desk",
            "module_panel_press",
            json!({"moduleId": connected.module, "panel": "create",
                   "action": {"id": "create_agent", "values": {"ref": agent_ref}}}),
        )
    };
    for agent_ref in [
        json!("fixer-1"),
        json!({"issuer": "https://host.example", "id": "agt_91"}),
    ] {
        let done = finish(agent_ref.clone()).await;
        assert_eq!(done["outcome"], "ok", "{done}");
        let result = &done["value"]["answer"]["result"];
        assert_eq!(result["agentRef"], agent_ref, "{done}");
        assert_eq!(result["proposalId"], proposal.as_str(), "{done}");
        assert_eq!(result["requestId"], proposed["requestId"], "{done}");
        assert_eq!(result["displayName"], "Fixer", "{done}");
    }
    // An agent the host never proposed here is dropped, and so is the
    // module's own claim to a proposal.
    for agent_ref in [
        json!("someone-else"),
        json!({"issuer": "https://elsewhere.example", "id": "agt_91"}),
    ] {
        let done = finish(agent_ref).await;
        assert_eq!(done["outcome"], "ok", "{done}");
        let result = &done["value"]["answer"]["result"];
        assert!(result["agentRef"].is_null(), "{done}");
        assert!(result["proposalId"].is_null(), "{done}");
        assert_eq!(result["displayName"], "Fixer", "{done}");
    }

    // An action that needs a panel: the host's reply names it, and it
    // opens only when the host's module declares it.
    let mut opened = Vec::new();
    for named in ["create", "nowhere"] {
        let ran = op(
            "agent_action_run",
            json!({"host": host.id, "actionId": "settings"}),
        )
        .await;
        assert_eq!(ran["outcome"], "ok", "{ran}");
        let command_id = ran["value"]["commandId"].clone();
        let commands = host.poll().await;
        assert!(
            commands
                .iter()
                .any(|command| command["commandId"] == command_id),
            "{commands:?}"
        );
        let (status, fetched) = host.fetch(&command_id).await;
        assert_eq!(status, StatusCode::OK, "{fetched}");
        let (status, acked) = host
            .ack(
                &command_id,
                json!({"outcome": "done", "result": {"panel": named}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{acked}");
        let reply = op("agent_action_get", json!({"commandId": command_id})).await;
        assert_eq!(reply["outcome"], "ok", "{reply}");
        opened.push(reply["value"]["panel"].clone());
    }
    assert_eq!(
        opened,
        vec![
            json!({"moduleId": connected.module, "panel": "create"}),
            Value::Null
        ]
    );
}
