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

/// The test app: the manifest it serves, and the install calls it got.
#[derive(Default)]
struct TestApp {
    origin: String,
    manifest: Value,
    installs: Vec<(HeaderMap, String)>,
}

type Shared = Arc<Mutex<TestApp>>;

async fn manifest(State(app): State<Shared>) -> Json<Value> {
    Json(app.lock().expect("app").manifest.clone())
}

async fn install(State(app): State<Shared>, headers: HeaderMap, body: String) -> StatusCode {
    app.lock().expect("app").installs.push((headers, body));
    StatusCode::OK
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
        let ran = jobs::run_due(&world.state, &registry, &options)
            .await
            .expect("run jobs");
        if ran == 0 {
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
    let role_edit = tool(
        &world,
        "desk",
        "role_write",
        json!({"projectId": world.project("WORK"), "role": "Builds (app)",
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
