//! Project admin through MCP (docs/plans/mcp-tools.md, Phase 1): the costly
//! parts are who may change a project, and that an archived project can be
//! brought back.

use agent_workspace_server::bootstrap::{BootstrapCommand, bootstrap_admin};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Executor, postgres::PgPoolOptions};
use uuid::Uuid;

use super::agent_interface::{
    Browser, PASSWORD, connect_app, create_agent, initialize, login, schema_pool, send, test_app,
    tool,
};

struct Setup {
    app: axum::Router,
    browser: Browser,
    organization_id: Uuid,
}

async fn setup(prefix: &str) -> Option<Setup> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    let owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Admin Organization".to_owned()),
            organization_slug: Some(format!("admin-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool, &database_url);
    let browser = login(&app).await;
    Some(Setup {
        app,
        browser,
        organization_id: owner.organization_id,
    })
}

async fn create_project(setup: &Setup, key: &str, name: &str) -> String {
    let (status, project) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some(&format!("create-{key}")),
            json!({"organizationId": setup.organization_id, "key": key, "name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    project["id"].as_str().expect("project").to_owned()
}

#[tokio::test]
async fn only_owners_change_projects_and_archived_ones_come_back() {
    let Some(setup) = setup("mcp_admin").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let old = create_project(&setup, "OLD", "Old work").await;
    let space = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned();
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (status, added) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/members"),
            None,
            json!({"actorId": mara, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");

    // A project key works wherever a project ID does.
    let workflow = tool(
        app,
        &codex,
        &session,
        "workflow_get",
        json!({"projectId": "work"}),
    )
    .await;
    assert_eq!(workflow["outcome"], "ok", "{workflow}");
    let statuses = workflow["value"]["statuses"].as_array().expect("statuses");
    assert!(
        statuses
            .iter()
            .any(|status| status["category"] == "completed"),
        "{workflow}"
    );
    assert!(
        !workflow["value"]["transitions"]
            .as_array()
            .expect("transitions")
            .is_empty()
    );

    // Owners see members and roles; agents are pointed at people_search.
    let members = tool(
        app,
        &codex,
        &session,
        "project_members",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(members["outcome"], "ok", "{members}");
    assert!(
        members["value"]["members"]
            .as_array()
            .expect("members")
            .iter()
            .any(|member| member["handle"] == "mara" && member["kind"] == "agent"),
        "{members}"
    );
    let hidden = tool(
        app,
        &mara_token,
        &mara_session,
        "project_members",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(hidden["outcome"], "permission_denied", "{hidden}");

    // Renaming: the owner can, an agent and a read-only app can't.
    for (token, session) in [(&mara_token, &mara_session), (&reader, &reader_session)] {
        let refused = tool(
            app,
            token,
            session,
            "project_update",
            json!({"projectId": work, "name": "Taken over"}),
        )
        .await;
        assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    }
    let renamed = tool(
        app,
        &codex,
        &session,
        "project_update",
        json!({"projectId": "WORK", "name": "Agent Workspace"}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    assert_eq!(renamed["value"]["name"], "Agent Workspace");
    assert_eq!(renamed["value"]["key"], "WORK");
    let stale = tool(
        app,
        &codex,
        &session,
        "project_update",
        json!({"projectId": work, "name": "Again", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Archiving: agents and read-only apps can't; the owner can, and can
    // bring it back by key even though archived projects are hidden.
    let refused = tool(
        app,
        &mara_token,
        &mara_session,
        "project_archive",
        json!({"project": work}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "project_archive",
        json!({"project": old}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let archived = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": "OLD"}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["archived"], true);
    let listed = tool(app, &codex, &session, "projects_list", json!({})).await;
    assert!(
        !listed["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str()),
        "{listed}"
    );
    let again = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": old}),
    )
    .await;
    assert_eq!(
        again["outcome"], "ok",
        "archiving twice is harmless: {again}"
    );
    let restored = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": "OLD", "archived": false}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["archived"], false);
    let listed = tool(app, &codex, &session, "projects_list", json!({})).await;
    assert!(
        listed["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str()),
        "{listed}"
    );
}
