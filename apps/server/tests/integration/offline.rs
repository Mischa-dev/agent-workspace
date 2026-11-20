//! H1.19: the self-hosted path needs no outside service. The silent,
//! costly failure: something that quietly needs a hosted service (a sign-in
//! provider, a code host, mail, an object store, a model, a metrics
//! collector) to set up, sign in, talk, plan, write, upload or take the
//! workspace away, so an install on a closed network half works.
//!
//! The server here has none of them configured: only its database and a
//! data folder. It walks first run, a message, a task, a page, an upload and
//! an export, all of which must finish.

use agent_workspace_server::{export::transfer, setup};
use axum::{body::to_bytes, http::StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    first_run::post_setup,
    support::{
        app::{app, send},
        db::db,
        two_workspaces::upload,
    },
};

#[tokio::test]
async fn a_server_with_no_outside_service_does_everything_a_person_starts_with() {
    let Some(test) = db("h1_19_offline").await else {
        return;
    };
    let data = tempfile::tempdir().expect("data root");
    let (router, state) = app(&test)
        .with(
            "AGENT_WORKSPACE_DATA_ROOT",
            data.path().to_string_lossy().into_owned(),
        )
        // The upload below doesn't depend on how much disk the host has
        // left; the watermark itself is tested on its own.
        .with(
            "AGENT_WORKSPACE_MINIMUM_FREE_BYTES",
            "1073741824".to_owned(),
        )
        .build();
    let config = &state.config;
    assert!(
        config.workos.is_none()
            && config.oidc.is_none()
            && config.github.is_none()
            && config.public_origin.is_none(),
        "the bare configuration"
    );
    let pool = &test.pool;

    // First run.
    setup::prepare(pool, config).await.expect("prepare");
    let code = std::fs::read_to_string(setup::code_path(config)).expect("code");
    let (status, session, browser) = post_setup(
        &router,
        json!({
            "code": code,
            "person": {
                "displayName": "Offline Person",
                "loginName": "offline",
                "password": "a long offline password",
            },
            "workspace": { "name": "Closed Network", "shape": "small_team" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");
    let browser = browser.expect("signed in");
    let workspace =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM organizations WHERE kind <> 'direct'")
            .fetch_one(pool)
            .await
            .expect("the workspace");
    let (channel, space) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, project_id FROM conversations \
         WHERE workspace_id = $1 AND kind = 'channel' AND archived_at IS NULL \
         ORDER BY created_at LIMIT 1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("a channel the shape made");

    // A message, a task and a page.
    let operation = |name: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            body,
        )
    };
    let (status, sent) = send(
        &router,
        operation(
            "message_send",
            json!({ "conversationId": channel, "text": "Hello from a closed network" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{sent}");
    let (status, task) = send(
        &router,
        operation(
            "task_create",
            json!({ "projectId": space, "title": "Works without the internet" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{task}");
    let (status, page) = send(
        &router,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/pages"),
            None,
            json!({ "title": "Notes", "body": "Written offline." }),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");

    // An upload, kept on this server's own disk.
    upload(
        &router,
        &browser,
        &format!("/api/v1/conversations/{channel}/files?name=offline.txt"),
        "text/plain",
        b"kept here".to_vec(),
    )
    .await;

    // And the whole workspace, to take elsewhere.
    let (status, started) = send(&router, operation("workspace_export_start", json!({}))).await;
    assert!(status.is_success(), "{started}");
    let export = started["value"]["id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("export");
    crate::support::run_export(&state, export)
        .await
        .expect("export");
    let response = router
        .clone()
        .oneshot(browser.request(
            "GET",
            &transfer::download_path(workspace, export),
            None,
            Value::Null,
        ))
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    let archive = to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .expect("archive");
    assert!(archive.starts_with(b"PK"), "a zip archive");

    test.finish().await;
}
