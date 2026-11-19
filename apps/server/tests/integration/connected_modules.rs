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

    // Dropping a scope from the manifest takes it away at once.
    app.lock().expect("app").manifest = builds(
        &origin,
        json!([{"scope": "members.view", "why": "To show who started a build.", "required": true}]),
    );
    let narrowed = tool(
        &world,
        "desk",
        "connected_module_refresh",
        json!({"moduleId": module}),
    )
    .await;
    assert_eq!(narrowed["value"]["refreshed"], "applied", "{narrowed}");
    let listing = as_app(&world, &secret, "connected_module_list", json!({})).await;
    assert!(refused(&listing), "{listing:?}");

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
