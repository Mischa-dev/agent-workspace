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
    let with_archived = tool(
        app,
        &codex,
        &session,
        "projects_list",
        json!({"includeArchived": true}),
    )
    .await;
    assert!(
        with_archived["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == old.as_str() && project["archived"] == true),
        "{with_archived}"
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

#[tokio::test]
async fn channels_threads_pins_and_decisions_can_be_shaped() {
    let Some(setup) = setup("mcp_channels").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");

    // Creating a channel twice returns the same one.
    let design = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": "WORK", "name": "design"}),
    )
    .await;
    assert_eq!(design["outcome"], "ok", "{design}");
    assert_eq!(design["value"]["created"], true);
    let design_id = design["value"]["id"].as_str().expect("channel").to_owned();
    let again = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": work, "name": "Design"}),
    )
    .await;
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(again["value"]["id"], design_id.as_str());
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "channel_create",
        json!({"projectId": work, "name": "lurking"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A message, a thread from it, and the thread's status and outcome.
    let sent = tool(
        app,
        &codex,
        &session,
        "message_send",
        json!({"conversationId": design_id, "text": "Let's use design tokens."}),
    )
    .await;
    let message_id = sent["value"]["messageId"]
        .as_str()
        .expect("message")
        .to_owned();
    let thread = tool(
        app,
        &codex,
        &session,
        "thread_start",
        json!({"conversationId": design_id, "fromMessageId": message_id}),
    )
    .await;
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
    let decided = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": thread_id, "status": "decided", "outcome": "Tokens it is"}),
    )
    .await;
    assert_eq!(decided["outcome"], "ok", "{decided}");
    assert_eq!(decided["value"]["status"], "Decided");
    assert_eq!(decided["value"]["outcome"], "Tokens it is");
    let wrong_kind = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "status": "Done"}),
    )
    .await;
    assert_eq!(wrong_kind["outcome"], "invalid_request", "{wrong_kind}");

    // Renaming, and a name that's taken.
    let renamed = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "name": "design-system"}),
    )
    .await;
    assert_eq!(renamed["value"]["name"], "design-system", "{renamed}");
    let other = tool(
        app,
        &codex,
        &session,
        "channel_create",
        json!({"projectId": work, "name": "other"}),
    )
    .await;
    let other_id = other["value"]["id"].as_str().expect("other").to_owned();
    let taken = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": other_id, "name": "design-system"}),
    )
    .await;
    assert_eq!(taken["outcome"], "conflict", "{taken}");

    // Archive a channel, see it leave the list, and restore it.
    let archived = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "archived": true}),
    )
    .await;
    assert_eq!(archived["value"]["archived"], true, "{archived}");
    let channels = tool(
        app,
        &codex,
        &session,
        "conversation_list",
        json!({"projectId": work, "kind": "channel"}),
    )
    .await;
    assert!(
        !channels["value"]["conversations"]
            .as_array()
            .expect("conversations")
            .iter()
            .any(|conversation| conversation["id"] == design_id.as_str()),
        "{channels}"
    );
    let restored = tool(
        app,
        &codex,
        &session,
        "conversation_update",
        json!({"conversationId": design_id, "archived": false, "name": "design"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["archived"], false);
    assert_eq!(restored["value"]["name"], "design");

    // Pins: pin the message, then unpin it.
    let pinned = tool(
        app,
        &codex,
        &session,
        "pin_set",
        json!({"conversationId": design_id, "messageId": message_id}),
    )
    .await;
    assert_eq!(pinned["outcome"], "ok", "{pinned}");
    assert_eq!(pinned["value"]["pins"].as_array().expect("pins").len(), 1);
    let unpinned = tool(
        app,
        &codex,
        &session,
        "pin_set",
        json!({"conversationId": design_id, "messageId": message_id, "pinned": false}),
    )
    .await;
    assert_eq!(
        unpinned["value"]["pins"].as_array().expect("pins").len(),
        0,
        "{unpinned}"
    );

    // Decisions: a superseded one drops out unless asked for.
    let first = tool(
        app,
        &codex,
        &session,
        "decision_record",
        json!({"projectId": work, "title": "Use CSS variables", "conversationId": design_id}),
    )
    .await;
    let first_id = first["value"]["id"].as_str().expect("decision").to_owned();
    let second = tool(
        app,
        &codex,
        &session,
        "decision_record",
        json!({"projectId": work, "title": "Use design tokens", "supersedes": first_id}),
    )
    .await;
    assert_eq!(second["outcome"], "ok", "{second}");
    let current = tool(
        app,
        &codex,
        &session,
        "decision_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    let titles = current["value"]["decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .map(|decision| decision["title"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Use design tokens"], "{current}");
    let everything = tool(
        app,
        &codex,
        &session,
        "decision_list",
        json!({"projectId": work, "includeSuperseded": true}),
    )
    .await;
    let superseded = everything["value"]["decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .find(|decision| decision["id"] == first_id.as_str())
        .cloned()
        .expect("superseded decision");
    assert_eq!(
        superseded["supersededBy"], second["value"]["id"],
        "{everything}"
    );
}

#[tokio::test]
async fn owners_add_change_and_remove_members() {
    let Some(setup) = setup("mcp_members").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
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
    let (_, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");

    let set = |arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, "project_member_set", arguments).await }
    };

    let added = set(json!({"projectId": work, "person": "@mara"})).await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(added["value"]["role"], "member");
    assert_eq!(added["value"]["kind"], "agent");
    let again = set(json!({"projectId": work, "person": "mara"})).await;
    assert_eq!(again["outcome"], "ok", "adding twice is harmless: {again}");
    let refused = set(json!({"projectId": work, "person": "@mara", "role": "admin"})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let viewer = set(json!({"projectId": work, "person": "@mara", "role": "viewer"})).await;
    assert_eq!(viewer["value"]["role"], "viewer", "{viewer}");
    let own = set(json!({"projectId": work, "person": "@owner", "remove": true})).await;
    assert_eq!(own["outcome"], "invalid_request", "{own}");

    // Mara can't manage members, and loses access the moment she's removed.
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let hers = tool(
        app,
        &mara_token,
        &mara_session,
        "project_member_set",
        json!({"projectId": work, "person": "@owner", "role": "viewer"}),
    )
    .await;
    assert_eq!(hers["outcome"], "permission_denied", "{hers}");
    let before = tool(
        app,
        &mara_token,
        &mara_session,
        "workflow_get",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(before["outcome"], "ok", "{before}");
    let removed = set(json!({"projectId": work, "person": "@mara", "remove": true})).await;
    assert_eq!(removed["value"]["active"], false, "{removed}");
    let after = tool(
        app,
        &mara_token,
        &mara_session,
        "workflow_get",
        json!({"projectId": work}),
    )
    .await;
    assert_ne!(after["outcome"], "ok", "{after}");

    // Adding her back restores her.
    let back = set(json!({"projectId": work, "person": "@mara", "role": "member"})).await;
    assert_eq!(back["value"]["active"], true, "{back}");
    assert_eq!(back["value"]["role"], "member");
}
