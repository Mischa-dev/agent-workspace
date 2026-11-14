//! Two workspaces, A and B, with look-alike people, agents, projects and
//! everything in them (see `workspace_isolation`), for tests that check
//! nothing crosses between workspaces. Moved here unchanged from
//! `workspace_isolation.rs`, which re-exports it.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    os::unix::fs::PermissionsExt,
    sync::Arc,
};

use agent_workspace_server::{
    AppState,
    auth::password::hash_password,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
    router,
    secrets::FileKeyring,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    agent_interface::{
        Browser, PASSWORD, connect_app, create_agent, initialize, login, schema_pool, send,
    },
    mcp_admin::{Setup, create_project},
    mcp_task_board::{insert_repository, seed_pull_request},
    mcp_tasks::{add_member, space},
    message_edits::{create_channel, edit, say, text},
};

use crate::workspace_isolation::{MARKER, ORIGIN, TINY_PNG, call_tool, structured, succeeded};

/// Everything one workspace has, as IDs and keys a client would use.
pub(crate) struct World {
    pub(crate) marker: &'static str,
    pub(crate) browser: Browser,
    pub(crate) organization: Uuid,
    /// The workspace's current slug (K4.5).
    pub(crate) slug: String,
    pub(crate) rook: Uuid,
    pub(crate) agent_token: String,
    pub(crate) agent_connection: Uuid,
    pub(crate) app_token: String,
    pub(crate) space: String,
    pub(crate) project: String,
    pub(crate) channel: String,
    pub(crate) message: String,
    pub(crate) thread: String,
    pub(crate) dm: String,
    pub(crate) dm_message: String,
    pub(crate) group: String,
    pub(crate) task: Uuid,
    pub(crate) subtask: Uuid,
    /// A comment on `task`, edited once.
    pub(crate) comment: Uuid,
    pub(crate) page: String,
    /// A comment thread on `page` (AWD-53), and two edits the agent
    /// suggested to it, pending: the page is a person's.
    pub(crate) page_thread: Uuid,
    pub(crate) page_suggestions: [Uuid; 2],
    pub(crate) repository: Uuid,
    pub(crate) pull_request: Uuid,
    pub(crate) pull_request_url: String,
    pub(crate) profile: Uuid,
    pub(crate) provider_account: Uuid,
    pub(crate) run_thread: Uuid,
    pub(crate) run: Uuid,
    pub(crate) approval: Uuid,
    pub(crate) plan: Uuid,
    pub(crate) import: String,
    pub(crate) webhook: Uuid,
    pub(crate) rule: Uuid,
    pub(crate) whiteboard: Uuid,
    pub(crate) project_whiteboard: Uuid,
    /// A collection (AWD-90) with a field, a saved view and a record whose
    /// values name `rook`, task WORK-1 and `page`.
    pub(crate) collection: Uuid,
    pub(crate) collection_field: Uuid,
    pub(crate) collection_view: Uuid,
    pub(crate) record: Uuid,
    /// A sheet (AWD-54) with a header, a value and a formula.
    pub(crate) sheet: Uuid,
    /// A file sent with `file_message` in the channel, and a task's image.
    pub(crate) file: Uuid,
    pub(crate) file_message: String,
    pub(crate) task_file: Uuid,
    /// The link preview `file_message` shows, made ready by the test.
    pub(crate) link_preview: Uuid,
}

/// The app on a fresh schema with workspaces A and B built, signed in as
/// each owner. Shared with `write_isolation`, which runs beside this test.
pub(crate) struct TwoWorkspaces {
    pub(crate) app: axum::Router,
    pub(crate) pool: PgPool,
    pub(crate) realtime: RealtimeHub,
    pub(crate) a: World,
    pub(crate) b: World,
    admin: PgPool,
    schema: String,
    supervisors: SupervisorRegistry,
    _temporary: tempfile::TempDir,
    _keys: tempfile::TempDir,
}

impl TwoWorkspaces {
    /// Stops the providers and drops the schema.
    pub(crate) async fn finish(self) {
        self.supervisors.shutdown_all().await;
        drop(self.app);
        self.pool.close().await;
        agent_workspace_server::test_support::drop_schema(&self.admin, &self.schema)
            .await
            .expect("drop schema");
    }
}

/// None (and a note) when there is no database to test against.
pub(crate) async fn two_workspaces(name: &str) -> Option<TwoWorkspaces> {
    two_workspaces_with(name, false).await
}

/// [`two_workspaces`], with a GitHub App configured when `github` is set, so
/// the GitHub routes get as far as their access checks. Its key is a
/// placeholder: nothing can sign in to GitHub with it.
pub(crate) async fn two_workspaces_with(name: &str, github: bool) -> Option<TwoWorkspaces> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{name}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");

    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    std::fs::create_dir_all(&data_root).expect("data root");
    // The secrets keyring lives outside the data and web roots, owner-only.
    let keys = tempfile::tempdir().expect("keyring root");
    let keyring_path = keys.path().join("secrets.keyring");
    std::fs::write(
        &keyring_path,
        FileKeyring::generate().serialize().as_bytes(),
    )
    .expect("write keyring");
    std::fs::set_permissions(&keyring_path, std::fs::Permissions::from_mode(0o600))
        .expect("keyring permissions");
    let mut values = BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root.join("profiles").to_string_lossy().into_owned(),
        ),
        (
            // No Codex here: account reads report the provider unavailable.
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            temporary
                .path()
                .join("no-codex")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
            "2".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS".to_owned(),
            "1".to_owned(),
        ),
    ]);
    if github {
        for (key, file, value) in [
            (
                "GITHUB_PRIVATE_KEY_PATH",
                "github-app.pem",
                "placeholder-key",
            ),
            (
                "GITHUB_WEBHOOK_SECRET_PATH",
                "github-webhook",
                "webhook-secret-for-tests",
            ),
            (
                "GITHUB_CLIENT_SECRET_PATH",
                "github-client",
                "client-secret-for-tests",
            ),
        ] {
            let path = keys.path().join(file);
            std::fs::write(&path, value).expect("GitHub secret");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("GitHub secret permissions");
            values.insert(
                format!("AGENT_WORKSPACE_{key}"),
                path.to_string_lossy().into_owned(),
            );
        }
        for (key, value) in [
            ("GITHUB_APP_ID", "12345"),
            ("GITHUB_APP_SLUG", "aw-test"),
            ("GITHUB_CLIENT_ID", "Iv1.testclient"),
        ] {
            values.insert(format!("AGENT_WORKSPACE_{key}"), value.to_owned());
        }
    }
    let mut config = AppConfig::from_values(&values).expect("test configuration");
    // Uploads here don't depend on how much disk the host has left, which
    // changes as CI runs.
    config.data_quota_bytes = u64::MAX;
    config.minimum_free_bytes = 0;
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let realtime = RealtimeHub::new(8192);
    let app = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: realtime.clone(),
        supervisors: supervisors.clone(),
    });

    // Workspace A: the first owner, through the normal bootstrap.
    let a_owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Acme".to_owned()),
            organization_slug: Some(format!("acme-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap A");
    let a_browser = login(&app).await;
    // Workspace B: bootstrap only makes the first workspace, so B's owner is
    // made the way a SaaS sign-up would: a new organization and its owner.
    let b_organization = second_workspace_owner(&pool).await;
    let b_browser = login_as(&app, &format!("owner-{MARKER}")).await;

    let a = build_workspace(&app, &pool, "A", "", a_owner.organization_id, a_browser).await;
    let b = build_workspace(&app, &pool, "B", MARKER, b_organization, b_browser).await;
    // The link previews' images, as the fetch job would have stored them.
    for world in [&a, &b] {
        let folder = data_root
            .join("files")
            .join(world.organization.simple().to_string());
        std::fs::create_dir_all(&folder).expect("preview folder");
        std::fs::write(
            folder.join(format!("{}.preview", world.link_preview.simple())),
            tiny_png(),
        )
        .expect("preview image");
    }
    Some(TwoWorkspaces {
        app,
        pool,
        realtime,
        a,
        b,
        admin,
        schema,
        supervisors,
        _temporary: temporary,
        _keys: keys,
    })
}

async fn second_workspace_owner(pool: &PgPool) -> Uuid {
    let organization = Uuid::now_v7();
    let actor = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("B owner actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
        .bind(organization)
        .bind(format!("Acme {MARKER}"))
        .bind(format!("acme-{MARKER}-{}", Uuid::now_v7().simple()))
        .execute(pool)
        .await
        .expect("B organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(organization)
    .bind(actor)
    .execute(pool)
    .await
    .expect("B owner membership");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user)
    .bind(actor)
    .bind(format!("owner-{MARKER}"))
    .bind(format!("Owner {MARKER}"))
    .execute(pool)
    .await
    .expect("B owner user");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user)
        .bind(
            hash_password(PASSWORD.to_owned())
                .await
                .expect("hash password"),
        )
        .execute(pool)
        .await
        .expect("B owner password");
    organization
}

pub(crate) async fn login_as(app: &axum::Router, login_name: &str) -> Browser {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": login_name, "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK, "login {login_name}");
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let _ = to_bytes(response.into_body(), 100_000).await;
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    Browser {
        cookie: cookies.join("; "),
        csrf,
    }
}

/// A person who can sign in, with the same handle in both workspaces.
pub(crate) async fn add_person(
    setup: &Setup,
    projects: &[&str],
    login_name: &str,
    handle: &str,
    display_name: &str,
) -> Uuid {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(&setup.pool)
        .await
        .expect("person actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(setup.organization_id)
    .bind(actor_id)
    .execute(&setup.pool)
    .await
    .expect("organization membership");
    for project in projects {
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(actor_id)
        .bind(display_name)
        .bind(handle)
        .execute(&setup.pool)
        .await
        .expect("project membership");
    }
    bootstrap_development_member(
        &setup.pool,
        BootstrapCommand {
            login_name: login_name.to_owned(),
            display_name: display_name.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor_id),
            organization_id: Some(setup.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap person");
    actor_id
}

pub(crate) fn tiny_png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG.trim_start_matches("data:image/png;base64,"))
        .expect("png")
}

/// Uploads raw bytes to a file route; returns the file's ID.
pub(crate) async fn upload(
    app: &axum::Router,
    browser: &Browser,
    uri: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Uuid {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("origin", ORIGIN)
        .header("cookie", &browser.cookie)
        .header("x-csrf-token", &browser.csrf)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("upload request");
    let (status, uploaded) = send(app, request).await;
    assert_eq!(status, StatusCode::CREATED, "upload {uri}: {uploaded}");
    Uuid::parse_str(&string_at(&uploaded, "/id")).expect("file id")
}

/// `value` as a query string value.
pub(crate) fn url_encode(value: &str) -> String {
    serde_urlencoded::to_string([("v", value)])
        .expect("encode")
        .trim_start_matches("v=")
        .to_owned()
}

pub(crate) fn string_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no {pointer} in {value}"))
        .to_owned()
}

/// Build one workspace's look-alike data. `marker` goes into every name and
/// text B writes; A's `marker` is empty.
async fn build_workspace(
    app: &axum::Router,
    pool: &PgPool,
    label: &'static str,
    marker: &'static str,
    organization: Uuid,
    browser: Browser,
) -> World {
    let m = if marker.is_empty() {
        String::new()
    } else {
        format!(" {marker}")
    };
    let tag = if marker.is_empty() { "a" } else { marker };
    let setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: browser.cookie.clone(),
            csrf: browser.csrf.clone(),
        },
        organization_id: organization,
        pool: pool.clone(),
        cleanup: None,
    };
    let space = space(&setup).await;
    let project = create_project(&setup, "WORK", &format!("Work{m}")).await;
    let rook = add_person(
        &setup,
        &[&project, &space],
        &format!("rook-{tag}"),
        "rook",
        &format!("Rook{m}"),
    )
    .await;
    add_person(
        &setup,
        &[&space],
        &format!("wren-{tag}"),
        "wren",
        &format!("Wren{m}"),
    )
    .await;
    let (agent, agent_token) =
        create_agent(app, &browser, &space, &format!("Mara{m}"), "mara").await;
    add_member(&setup, &project, agent, "member").await;
    let agent_connection = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(pool)
    .await
    .expect("agent connection");

    let app_token = connect_app(app, &browser, &format!("Codex{m}"), "full").await;
    let session = initialize(app, &app_token).await.expect("app MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = app_token.clone();
        let session = session.clone();
        async move {
            let (status, body) = call_tool(app, &token, &session, name, arguments.clone()).await;
            let value = structured(&body);
            assert!(
                succeeded(&value["outcome"]),
                "{label} fixture {name} {arguments} -> {status} {body}"
            );
            value
        }
    };

    call(
        "label_write",
        json!({"projectId": "WORK", "name": format!("Launch{m}")}),
    )
    .await;
    call(
        "field_write",
        json!({
            "projectId": "WORK",
            "name": format!("Risk{m}"),
            "type": "single_choice",
            "options": [{"name": format!("High{m}")}],
        }),
    )
    .await;
    call(
        "field_write",
        json!({"projectId": "WORK", "name": format!("Owner{m}"), "type": "person"}),
    )
    .await;
    let mut task_fields = serde_json::Map::new();
    task_fields.insert(format!("Risk{m}"), json!(format!("High{m}")));
    task_fields.insert(format!("Owner{m}"), json!("@rook"));
    call(
        "task_create",
        json!({
            "projectId": "WORK",
            "title": format!("Launch plan{m}"),
            "description": format!("Ship the launch{m}"),
            "assignee": "@rook",
            "labels": [format!("Launch{m}")],
            "acceptanceCriteria": [format!("Launch checklist{m}")],
            "fields": task_fields,
            "clientRequestId": "task-1"
        }),
    )
    .await;
    call(
        "task_create",
        json!({"parentId": "WORK-1", "title": format!("Launch subtask{m}"), "clientRequestId": "task-2"}),
    )
    .await;
    let task_number = |number: i64| {
        let project = project.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE project_id = $1::uuid AND number = $2",
            )
            .bind(project)
            .bind(number)
            .fetch_one(pool)
            .await
            .expect("task")
        }
    };
    let task = task_number(1).await;
    let subtask = task_number(2).await;
    let comment = call(
        "task_comment",
        json!({"workItemId": "WORK-1", "update": format!("Launch comment{m}"), "clientRequestId": "comment-1"}),
    )
    .await;
    let comment = Uuid::parse_str(&string_at(&comment, "/activityId")).expect("comment id");
    // An edit, so the comment has versions to read.
    call(
        "task_comment_edit",
        json!({"taskId": "WORK-1", "commentId": comment, "text": format!("Launch comment, corrected{m}")}),
    )
    .await;
    call(
        "milestone_write",
        json!({"projectId": "WORK", "name": format!("Launch milestone{m}"), "addTasks": ["WORK-1"]}),
    )
    .await;
    let page = call(
        "page_write",
        json!({"projectId": "WORK", "title": format!("Launch page{m}"), "body": format!("Launch notes{m}")}),
    )
    .await;
    let page = string_at(&page, "/value/id");
    call(
        "page_write",
        json!({"pageId": page, "body": format!("Launch notes, second draft{m}"), "skillDescription": format!("Use when launching{m}"), "expectedVersion": 1}),
    )
    .await;
    // Someone has had the page open, so it has a live document. Its
    // generation is the page's ID, for write cases to name.
    call("page_blocks", json!({"pageId": page})).await;
    sqlx::query("UPDATE project_page_documents SET generation = page_id WHERE page_id = $1")
        .bind(Uuid::parse_str(&page).expect("page id"))
        .execute(pool)
        .await
        .expect("page document generation");
    let page_thread = call(
        "page_comment",
        json!({"pageId": page, "text": format!("Launch page comment{m}")}),
    )
    .await;
    let page_thread =
        Uuid::parse_str(&string_at(&page_thread, "/value/threadId")).expect("page thread");

    // Collections (AWD-90): fields that point at a person, a task and a
    // page, a saved view, and a record that uses them, changed once so it
    // has history.
    let collection = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": format!("Playtests{m}"),
            "description": format!("What players found{m}"),
            "titleName": format!("Finding{m}"),
            "fields": [
                {"name": format!("Severity{m}"), "type": "single_choice", "options": [{"name": format!("Severe{m}")}]},
                {"name": format!("Tester{m}"), "type": "person"},
                {"name": format!("Build{m}"), "type": "link", "target": "task"},
                {"name": format!("Spec{m}"), "type": "link", "target": "page"},
                {"name": format!("Notes{m}"), "type": "text"},
            ],
            "views": [{
                "name": format!("Severe ones{m}"),
                "filters": [{"field": format!("Severity{m}"), "op": "is", "values": [format!("Severe{m}")]}],
                "sort": [{"field": "number", "direction": "desc"}],
                "groupBy": format!("Tester{m}"),
            }],
            "clientRequestId": "collection-1",
        }),
    )
    .await;
    let collection =
        Uuid::parse_str(&string_at(&collection, "/value/collection/id")).expect("collection id");
    let mut record_values = serde_json::Map::new();
    record_values.insert(format!("Severity{m}"), json!(format!("Severe{m}")));
    record_values.insert(format!("Tester{m}"), json!("@rook"));
    record_values.insert(format!("Build{m}"), json!("WORK-1"));
    record_values.insert(format!("Spec{m}"), json!(page));
    record_values.insert(
        format!("Notes{m}"),
        json!(format!("Falls through the floor{m}")),
    );
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": collection,
            "records": [{"title": format!("Crash on level 2{m}"), "fields": record_values}],
            "clientRequestId": "record-1",
        }),
    )
    .await;
    let record = Uuid::parse_str(&string_at(&written, "/value/records/0/id")).expect("record id");
    call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": collection,
            "records": [{"record": "#1", "title": format!("Crash on level 3{m}")}],
            "clientRequestId": "record-2",
        }),
    )
    .await;
    let collection_field = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM field_definitions WHERE collection_id = $1 ORDER BY position, id LIMIT 1",
    )
    .bind(collection)
    .fetch_one(pool)
    .await
    .expect("collection field");
    let collection_view = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_views WHERE collection_id = $1 ORDER BY position, id LIMIT 1",
    )
    .bind(collection)
    .fetch_one(pool)
    .await
    .expect("collection view");

    // Sheets (AWD-54): a header row, a value and a formula over it.
    let sheet = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": format!("Budget{m}"),
            "description": format!("What it costs{m}"),
            "cells": [
                {"cell": "A1", "input": "Item"},
                {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": format!("Rent{m}")},
                {"cell": "B2", "input": "1200"},
                {"cell": "B3", "input": "=SUM(B2:B2)"},
            ],
            "clientRequestId": "sheet-1",
        }),
    )
    .await;
    let sheet = Uuid::parse_str(&string_at(&sheet, "/value/id")).expect("sheet id");

    let channel = create_channel(&setup, &project, "launch").await;
    let message = say(
        &setup,
        &browser,
        &channel,
        "message-1",
        text(&format!("Launch message{m}")),
    )
    .await;
    let (status, edited) = edit(
        &setup,
        &browser,
        &channel,
        &message,
        "edit-1",
        json!({"content": text(&format!("Launch message, edited{m}"))}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{label} edit: {edited}");
    let thread = call(
        "thread_start",
        json!({"conversationId": channel, "name": format!("Launch thread{m}"), "text": format!("Launch thread reply{m}")}),
    )
    .await;
    let thread = string_at(&thread, "/value/id");
    call(
        "decision_record",
        json!({"projectId": "WORK", "title": format!("Launch decision{m}"), "conversationId": channel}),
    )
    .await;
    call(
        "pin_set",
        json!({"conversationId": channel, "messageId": message}),
    )
    .await;

    let (status, dm) = send(
        app,
        browser.request("POST", "/api/v1/direct/dms", None, json!({"actorId": rook})),
    )
    .await;
    assert!(status.is_success(), "{label} DM: {status} {dm}");
    let dm = string_at(&dm, "/conversation/id");
    let dm_message = say(
        &setup,
        &browser,
        &dm,
        "dm-1",
        text(&format!("Launch DM{m}")),
    )
    .await;
    let group = call(
        "conversation_start",
        json!({"people": ["@rook", "@wren"], "name": format!("Launch group{m}"), "text": format!("Launch group hello{m}")}),
    )
    .await;
    let group = string_at(&group, "/value/conversation/id");

    // The agent claims the task, so the claim route has something to show.
    let agent_session = initialize(app, &agent_token).await.expect("agent MCP");
    let (_, claimed) = call_tool(
        app,
        &agent_token,
        &agent_session,
        "task_claim",
        json!({"taskId": "WORK-1", "note": format!("Launch claim{m}")}),
    )
    .await;
    assert!(
        succeeded(&structured(&claimed)["outcome"]),
        "{label} claim: {claimed}"
    );
    let mut page_suggestions = [Uuid::nil(); 2];
    for (index, slot) in page_suggestions.iter_mut().enumerate() {
        let (_, suggested) = call_tool(
            app,
            &agent_token,
            &agent_session,
            "page_edit",
            json!({"pageId": page, "edits": [{"op": "append", "markdown": format!("Launch suggestion {index}{m}")}]}),
        )
        .await;
        let suggested = structured(&suggested);
        assert!(
            succeeded(&suggested["outcome"]),
            "{label} suggestion: {suggested}"
        );
        *slot = Uuid::parse_str(&string_at(&suggested, "/value/suggestion/suggestionId"))
            .expect("suggestion id");
    }

    let (status, invitation) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/invitations"),
            Some("invite-1"),
            json!({"role": "viewer", "expiresInHours": 24}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} invitation: {status} {invitation}"
    );

    // Views and data (W11): a personal view, a task template on a weekly
    // schedule, a project template saved from the project, a draft import.
    let post = |path: String, key: &'static str, body: Value| {
        let request = browser.request("POST", &path, Some(key), body);
        async move {
            let (status, value) = send(app, request).await;
            assert!(status.is_success(), "{label} {path}: {status} {value}");
            value
        }
    };
    post(
        format!("/api/v1/projects/{project}/views"),
        "view-1",
        json!({"name": format!("Launch view{m}"), "query": {"q": format!("Launch{m}")}}),
    )
    .await;
    let task_template = post(
        format!("/api/v1/projects/{project}/task-templates"),
        "template-1",
        json!({"name": format!("Launch template{m}"), "title": format!("Launch review{m}"), "subtasks": [{"title": format!("Launch step{m}")}]}),
    )
    .await;
    let task_template = string_at(&task_template, "/id");
    post(
        format!("/api/v1/projects/{project}/recurring-tasks"),
        "recurring-1",
        json!({"templateId": task_template, "frequency": "weekly", "weekdays": [1]}),
    )
    .await;
    post(
        format!("/api/v1/projects/{project}/save-as-template"),
        "project-template-1",
        json!({"name": format!("Launch setup{m}"), "description": format!("Launch defaults{m}")}),
    )
    .await;
    let import = post(
        format!("/api/v1/projects/{project}/imports"),
        "import-1",
        json!({"source": "markdown", "text": format!("# Launch{m}\n- [ ] Launch import{m}\n"), "fileName": format!("launch{tag}.md")}),
    )
    .await;
    let import = string_at(&import, "/id");

    // Code: the same repository name in both, and a pull request on it.
    let repository = insert_repository(&setup, &project, "acme/widgets", true, true, None).await;
    let (pull_request, pull_request_url) = seed_pull_request(&setup, &project, repository, 7).await;

    // Runs: a personal Codex profile and a run blocked on an approval in a
    // repository-bound thread.
    let (status, registered) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/execution-profiles"),
            Some("profile-1"),
            json!({}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} profile: {status} {registered}"
    );
    let profile = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM execution_profiles WHERE project_id = $1::uuid",
    )
    .bind(&project)
    .fetch_one(pool)
    .await
    .expect("profile");
    let provider_account = sqlx::query_scalar::<_, Uuid>(
        "SELECT provider_account_id FROM execution_profiles WHERE id = $1",
    )
    .bind(profile)
    .fetch_one(pool)
    .await
    .expect("provider account");
    sqlx::query("UPDATE execution_profiles SET status = 'available' WHERE id = $1")
        .bind(profile)
        .execute(pool)
        .await
        .expect("profile available");
    sqlx::query("UPDATE execution_workers SET enabled = true, pending_setup = false WHERE project_id = $1::uuid")
        .bind(&project)
        .execute(pool)
        .await
        .expect("worker enabled");
    let worker = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM execution_workers WHERE project_id = $1::uuid",
    )
    .bind(&project)
    .fetch_one(pool)
    .await
    .expect("worker");
    let run_thread = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
        VALUES ($1, $2::uuid, 'thread', $3::uuid, $4, $5)
        "#,
    )
    .bind(run_thread)
    .bind(&project)
    .bind(&channel)
    .bind(repository)
    .bind(format!("Launch run thread{m}"))
    .execute(pool)
    .await
    .expect("run thread");
    let run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, started_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'failed', $7, '{}'::jsonb, now())
        "#,
    )
    .bind(run)
    .bind(run_thread)
    .bind(worker)
    .bind(rook)
    .bind(profile)
    .bind(repository)
    .bind(format!("Launch run{m}"))
    .execute(pool)
    .await
    .expect("run");
    // Admission checks a live Codex account on insert; a run that is already
    // waiting on an approval only needs the row.
    sqlx::query("UPDATE agent_runs SET state = 'blocked' WHERE id = $1")
        .bind(run)
        .execute(pool)
        .await
        .expect("run blocked");
    let approval = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        )
        VALUES ($1, $2, $3, $4, 'approval', $5,
                '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
                'any_authorized', false)
        "#,
    )
    .bind(approval)
    .bind(run)
    .bind(rook)
    .bind(format!("isolation-{approval}"))
    .bind(format!("Launch approval{m}"))
    .execute(pool)
    .await
    .expect("approval");

    // W12: what the run did and cost, an agent profile made from a
    // template, and a harness the workspace registered (a connected runner).
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (id, agent_run_id, event_key, kind, summary, payload)
        VALUES ($1, $2, 'note:isolation', 'agent_note', $3, $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(run)
    .bind(format!("Looking around{m}"))
    .bind(json!({ "text": format!("Looking around{m}") }))
    .execute(pool)
    .await
    .expect("run step");
    sqlx::query(
        r#"
        INSERT INTO agent_run_usage (
            agent_run_id, model, organization_id, project_id, provider_account_id,
            adapter_kind, usage_date, input_tokens, output_tokens, cached_input_tokens,
            estimated_cost, currency
        )
        VALUES ($1, $2, $3, $4::uuid, $5, 'codex', current_date, 100, 50, 10, 0.25, 'USD')
        "#,
    )
    .bind(run)
    .bind(format!("model{m}"))
    .bind(organization)
    .bind(&project)
    .bind(provider_account)
    .execute(pool)
    .await
    .expect("run usage");
    let uri = format!("/api/v1/projects/{project}/agent-profiles");
    let (status, written) = send(
        app,
        browser.request(
            "POST",
            &uri,
            None,
            json!({
                "template": "reviewer",
                "name": format!("Reviewer{m}"),
                "description": format!("Reviews{m}"),
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} POST {uri}: {status} {written}"
    );
    let uri = format!("/api/v1/organizations/{organization}/harnesses");
    let (status, written) = send(
        app,
        browser.request(
            "POST",
            &uri,
            None,
            json!({
                "name": format!("Runner{m}"),
                "transport": "runner",
                "runnerActorId": agent,
                "manifest": {
                    "description": format!("Runs elsewhere{m}"),
                    "models": [{"id": format!("runner-model{marker}"), "displayName": "Runner model"}],
                },
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} POST {uri}: {status} {written}"
    );

    // Settings and secrets at workspace and project scope.
    let model = if marker.is_empty() {
        "launch-model".to_owned()
    } else {
        format!("{marker}-model")
    };
    let secret_name = if marker.is_empty() {
        "launch_key".to_owned()
    } else {
        format!("{marker}_key")
    };
    for (uri, body) in [
        (
            "/api/v1/settings/values/run.model".to_owned(),
            json!({"scope": "workspace", "organizationId": organization, "value": model}),
        ),
        (
            "/api/v1/settings/values/run.model".to_owned(),
            json!({"scope": "project", "projectId": project, "value": format!("{model}-project")}),
        ),
        (
            format!("/api/v1/organizations/{organization}/secrets/{secret_name}"),
            json!({"value": format!("value-{secret_name}"), "description": format!("Launch key{m}")}),
        ),
        (
            format!("/api/v1/organizations/{organization}/personal-secrets/{secret_name}"),
            json!({"value": format!("personal-{secret_name}")}),
        ),
        (
            format!("/api/v1/projects/{project}/secrets/{secret_name}"),
            json!({"value": format!("project-{secret_name}")}),
        ),
    ] {
        let (status, written) = send(app, browser.request("PUT", &uri, None, body)).await;
        assert!(status.is_success(), "{label} PUT {uri}: {status} {written}");
    }
    // An environment and a queued launch (W7), started far ahead so the
    // queue leaves it waiting.
    let uri = format!("/api/v1/projects/{project}/environments");
    let (status, written) = send(
        app,
        browser.request(
            "POST",
            &uri,
            None,
            json!({
                "name": format!("Cloud{m}"),
                "harness": "codex_cloud",
                "providerEnvironmentId": "env-launch",
                "setupNotes": format!("Notes{m}"),
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} POST {uri}: {status} {written}"
    );
    let uri = format!("/api/v1/projects/{project}/launches");
    let start_at = (chrono::Utc::now() + chrono::Duration::days(20)).to_rfc3339();
    let (status, written) = send(
        app,
        browser.request(
            "POST",
            &uri,
            Some(&format!("launch-{label}")),
            json!({
                "workItemIds": [task],
                "startWhen": "at",
                "startAt": start_at,
                "prompt": format!("Ship it{m}"),
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} POST {uri}: {status} {written}"
    );

    // AI planning: a plan draft and a pending "blocks" suggestion between the
    // task and its subtask, citing the task.
    let (status, imported) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/ai/plans"),
            None,
            json!({
                "title": format!("Launch plan{m}"),
                "source": {"kind": "structured", "plan": {"tasks": [
                    {"ref": "t1", "title": format!("Launch step{m}")}
                ]}}
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{label} plan: {status} {imported}");
    let plan = Uuid::parse_str(&string_at(&imported, "/plan/id")).expect("plan id");
    let (first, second) = if task < subtask {
        (task, subtask)
    } else {
        (subtask, task)
    };
    sqlx::query(
        r#"
        INSERT INTO ai_suggestions (
            id, organization_id, project_id, work_item_id, kind,
            other_project_id, other_work_item_id, confidence, reason, sources,
            pair_key, origin, provider, model, prompt_id, prompt_version
        )
        VALUES ($1, $2, $3::uuid, $4, 'blocks', $3::uuid, $5, 0.8, $6,
                jsonb_build_array(jsonb_build_object('kind', 'task', 'id', $4::text)),
                $7, 'analyze', 'fake', 'fake-quick', 'dependency_judge', 1)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization)
    .bind(&project)
    .bind(task)
    .bind(subtask)
    .bind(format!("Launch blocks{m}"))
    .bind(format!("link:{first}:{second}"))
    .execute(pool)
    .await
    .expect("suggestion");

    // Usage and push (W9): a reported window, a reset schedule, a device
    // and a muted group, each carrying the marker where it has text.
    sqlx::query(
        r#"
        INSERT INTO usage_windows (
            account_id, organization_id, window_key, limit_name, duration_minutes,
            used_percent, resets_at
        ) VALUES ($1, $2, 'codex.secondary', $3, 10080, 40, now() + interval '2 days')
        "#,
    )
    .bind(provider_account)
    .bind(organization)
    .bind(format!("Codex{m}"))
    .execute(pool)
    .await
    .expect("usage window");
    let device = aws_lc_rs::agreement::PrivateKey::generate(&aws_lc_rs::agreement::ECDH_P256)
        .expect("device key");
    let device_public = device.compute_public_key().expect("device public key");
    let b64 = |bytes: &[u8]| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    };
    for (method, uri, body) in [
        (
            "POST",
            format!("/api/v1/usage/accounts/{provider_account}/schedules"),
            json!({"label": format!("Weekly{m}"), "cadence": "weekly", "weekday": 1,
                   "minuteOfDay": 540, "timeZone": "UTC"}),
        ),
        (
            "POST",
            "/api/v1/push/subscriptions".to_owned(),
            json!({
                "endpoint": format!("https://fcm.googleapis.com/fcm/send/{tag}-device"),
                "keys": {"p256dh": b64(device_public.as_ref()), "auth": b64(&[7_u8; 16])},
                "label": format!("Phone{m}"),
                "organizationId": organization,
            }),
        ),
        (
            "PUT",
            "/api/v1/notification-settings/mutes".to_owned(),
            json!({"projectId": project, "group": "runs", "muted": true}),
        ),
    ] {
        let (status, written) = send(app, browser.request(method, &uri, None, body)).await;
        assert!(
            status.is_success(),
            "{label} {method} {uri}: {status} {written}"
        );
    }

    // Later, reminders and scheduling (W21): a saved message with a
    // reminder, a saved task, a bare reminder, a scheduled message and a
    // channel set to every message.
    let soon = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/later".to_owned(),
            json!({"messageId": message, "note": format!("Read{m}"), "remindAt": soon}),
        ),
        (
            "POST",
            "/api/v1/later".to_owned(),
            json!({"workItemId": task}),
        ),
        (
            "POST",
            "/api/v1/later".to_owned(),
            json!({"note": format!("Call back{m}"), "remindAt": soon, "organizationId": organization}),
        ),
        (
            "POST",
            format!("/api/v1/conversations/{channel}/scheduled-messages"),
            json!({
                "content": {"version": 1, "nodes": [{"type": "text", "text": format!("Later{m}")}]},
                "sendAt": soon,
            }),
        ),
        (
            "PUT",
            format!("/api/v1/conversations/{channel}/notification-level"),
            json!({"level": "all"}),
        ),
    ] {
        let (status, written) = send(app, browser.request(method, &uri, None, body)).await;
        assert!(
            status.is_success(),
            "{label} {method} {uri}: {status} {written}"
        );
    }

    // Webhooks and automations (W14): a subscription on the whole
    // workspace and a rule that notifies its maker.
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            "/api/v1/webhooks",
            None,
            json!({
                "name": format!("Hook{m}"),
                "url": format!("https://hooks.example.com/{tag}"),
                "eventTypes": ["run.finished", "message.created"],
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{label} webhook: {status} {created}");
    let webhook = Uuid::parse_str(&string_at(&created, "/webhook/id")).expect("webhook id");
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/automations"),
            None,
            json!({
                "name": format!("Rule{m}"),
                "trigger": {"kind": "task_created"},
                "actions": [{"kind": "notify", "recipients": "owner",
                             "message": format!("New task{m}")}],
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} automation: {status} {created}"
    );
    let rule = Uuid::parse_str(&string_at(&created, "/id")).expect("rule id");
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/bug-report-links"),
            None,
            json!({"name": format!("Game{m}")}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} bug report link: {status} {created}"
    );

    // AWD-76: a saved workspace template, carrying the project's channels.
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/organizations/{organization}/workspace-templates"),
            None,
            json!({
                "name": format!("Our setup{m}"),
                "description": format!("How we work{m}"),
                "projectId": project,
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{label} workspace template: {created}"
    );

    // Layouts: a saved preset, the workspace's default and the project's
    // layout, and rules for viewers and for Rook.
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/organizations/{organization}/layout/presets"),
            None,
            json!({
                "name": format!("Chores{m}"),
                "description": format!("For the house{m}"),
                "layout": {
                    "preset": "household",
                    "words": {"task": {"one": format!("chore{m}"), "many": format!("chores{m}")}},
                },
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{label} layout preset: {created}"
    );
    let layout_preset = string_at(&created, "/id");
    for (path, body) in [
        (
            format!("/api/v1/organizations/{organization}/layout"),
            json!({"layout": {"preset": layout_preset}}),
        ),
        (
            format!("/api/v1/projects/{project}/layout"),
            json!({"layout": {
                "preset": "personal_life",
                "words": {"page": {"one": format!("note{m}"), "many": format!("notes{m}")}},
            }}),
        ),
        (
            format!("/api/v1/projects/{project}/layout/rules"),
            json!({"rules": [
                {"role": "viewer", "preset": layout_preset},
                {"actorId": rook, "preset": "bug_reporting"},
            ]}),
        ),
    ] {
        let (status, changed) = send(app, browser.request("PUT", &path, None, body)).await;
        assert!(status.is_success(), "{label} {path}: {status} {changed}");
    }

    // Whiteboards: one for the whole workspace, drawn on and with an image,
    // and one in the project, linked to the task, the project and the page.
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            "/api/v1/whiteboards",
            None,
            json!({"title": format!("Sketch{m}"), "organizationId": organization}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} whiteboard: {status} {created}"
    );
    let whiteboard =
        Uuid::parse_str(&string_at(&created, "/whiteboard/id")).expect("whiteboard id");
    let (status, drawn) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/whiteboards/{whiteboard}/operations"),
            None,
            json!({"operations": [
                {"op": "add", "type": "rectangle", "id": "box", "text": format!("Box{m}")},
                {"op": "add", "type": "ellipse", "id": "end", "x": 300, "text": "End"},
                {"op": "connect", "from": "box", "to": "end", "text": format!("Flow{m}")},
            ]}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} whiteboard drawing: {status} {drawn}"
    );
    let (status, stored) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/whiteboards/{whiteboard}/files"),
            None,
            json!({"id": "photo1", "mimeType": "image/png", "dataURL": TINY_PNG}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} whiteboard file: {status} {stored}"
    );
    let (status, created) = send(
        app,
        browser.request(
            "POST",
            "/api/v1/whiteboards",
            None,
            json!({
                "title": format!("Plan{m}"),
                "projectId": project,
                "link": {"targetKind": "task", "targetId": task},
            }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} project whiteboard: {status} {created}"
    );
    let project_whiteboard =
        Uuid::parse_str(&string_at(&created, "/whiteboard/id")).expect("whiteboard id");
    for (board, kind, target) in [
        (project_whiteboard, "page", page.clone()),
        (whiteboard, "project", project.clone()),
    ] {
        let (status, linked) = send(
            app,
            browser.request(
                "POST",
                &format!("/api/v1/whiteboards/{board}/links"),
                None,
                json!({"targetKind": kind, "targetId": target}),
            ),
        )
        .await;
        assert!(
            status.is_success(),
            "{label} whiteboard link: {status} {linked}"
        );
    }

    // Files (W20): a file sent with a message that links to a page, and an
    // image kept on the task.
    let file = upload(
        app,
        &browser,
        &format!("/api/v1/conversations/{channel}/files?name=report-{tag}.txt"),
        "text/plain",
        format!("Notes{m}").into_bytes(),
    )
    .await;
    let (status, sent) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/messages"),
            Some(&format!("files-{tag}")),
            json!({
                "content": text(&format!("Report{m} https://example.com/{tag}-page")),
                "attachments": [file],
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{label} file message: {status} {sent}");
    let file_message = string_at(&sent, "/message/id");
    let task_file = upload(
        app,
        &browser,
        &format!("/api/v1/projects/{project}/work-items/{task}/files?name=spec-{tag}.png"),
        "image/png",
        tiny_png(),
    )
    .await;
    let link_preview = sqlx::query_scalar::<_, Uuid>(
        "SELECT link_preview_id FROM message_link_previews WHERE message_id = $1",
    )
    .bind(Uuid::parse_str(&file_message).expect("message id"))
    .fetch_one(pool)
    .await
    .expect("link preview");
    sqlx::query(
        r#"
        UPDATE link_previews
        SET status = 'ready', title = $2, image_key = $3, image_bytes = $4,
            image_type = 'image/png', image_width = 1, image_height = 1, fetched_at = now()
        WHERE id = $1
        "#,
    )
    .bind(link_preview)
    .bind(format!("Page{m}"))
    .bind(format!(
        "{}/{}.preview",
        organization.simple(),
        link_preview.simple()
    ))
    .bind(tiny_png().len() as i64)
    .execute(pool)
    .await
    .expect("ready preview");
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
        .bind(organization)
        .fetch_one(pool)
        .await
        .expect("workspace slug");

    World {
        marker,
        browser,
        organization,
        slug,
        rook,
        agent_token,
        agent_connection,
        app_token,
        space,
        project,
        channel,
        message,
        thread,
        dm,
        dm_message,
        group,
        task,
        subtask,
        comment,
        page,
        page_thread,
        page_suggestions,
        repository,
        pull_request,
        pull_request_url,
        profile,
        provider_account,
        run_thread,
        run,
        approval,
        plan,
        import,
        webhook,
        rule,
        whiteboard,
        project_whiteboard,
        collection,
        collection_field,
        collection_view,
        record,
        sheet,
        file,
        file_message,
        task_file,
        link_preview,
    }
}

/// Every ID a workspace owns: its organization, its people and agents, and
/// every row (in any table) of its projects, their conversations, tasks and
/// runs. Found from the catalog, so new tables are swept automatically.
pub(crate) async fn sweep_ids(pool: &PgPool, organization: Uuid) -> HashSet<String> {
    let mut ids = BTreeSet::<Uuid>::new();
    ids.insert(organization);
    let projects =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects WHERE organization_id = $1")
            .bind(organization)
            .fetch_all(pool)
            .await
            .expect("projects");
    ids.extend(projects.iter().copied());
    let actors = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor_id FROM organization_memberships WHERE organization_id = $1
        UNION
        SELECT membership.actor_id FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE project.organization_id = $1
        "#,
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("actors");
    ids.extend(actors.iter().copied());
    let users = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE actor_id = ANY($1)")
        .bind(&actors)
        .fetch_all(pool)
        .await
        .expect("users");
    ids.extend(users);

    // Tables with a UUID `id` and one of these parent columns.
    let tables = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT parent.table_name::text, parent.column_name::text
        FROM information_schema.columns parent
        JOIN information_schema.columns own
          ON own.table_schema = parent.table_schema
         AND own.table_name = parent.table_name
         AND own.column_name = 'id'
         AND own.data_type = 'uuid'
        JOIN information_schema.tables listed
          ON listed.table_schema = parent.table_schema
         AND listed.table_name = parent.table_name
         AND listed.table_type = 'BASE TABLE'
        WHERE parent.table_schema = current_schema()
          AND parent.data_type = 'uuid'
          AND parent.column_name IN (
              'organization_id', 'project_id', 'conversation_id', 'work_item_id',
              'agent_run_id', 'actor_id', 'repository_id', 'page_id', 'message_id'
          )
        ORDER BY 1, 2
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("catalog");
    // Children can hang off children, so sweep until nothing new turns up.
    loop {
        let before = ids.len();
        let known = ids.iter().copied().collect::<Vec<_>>();
        for (table, column) in &tables {
            let found = sqlx::query_scalar::<_, Uuid>(&format!(
                "SELECT id FROM {table} WHERE {column} = ANY($1)"
            ))
            .bind(&known)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|error| panic!("sweep {table}.{column}: {error}"));
            ids.extend(found);
        }
        if ids.len() == before {
            break;
        }
    }
    ids.into_iter()
        .flat_map(|id| [id.to_string(), id.simple().to_string()])
        .collect()
}
