//! AWD-76: workspace templates. Using one sets the modules, layout, roles,
//! default role and a starter project with channels and collections; using
//! it again changes nothing. Only people who manage the workspace use or
//! save templates; agents read them and suggest one.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::space,
    message_edits::add_person,
};

fn templates_path(setup: &Setup) -> String {
    format!(
        "/api/v1/organizations/{}/workspace-templates",
        setup.organization_id
    )
}

async fn apply(setup: &Setup, template: &str) -> (StatusCode, Value) {
    send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("{}/apply", templates_path(setup)),
            None,
            json!({"template": template}),
        ),
    )
    .await
}

async fn onboarding(setup: &Setup) -> Value {
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/organizations/{}/onboarding", setup.organization_id),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn step_ids(onboarding: &Value) -> Vec<String> {
    onboarding["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["id"].as_str().expect("id").to_owned())
        .collect()
}

async fn count(setup: &Setup, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(setup.organization_id)
        .fetch_one(&setup.pool)
        .await
        .expect(sql)
}

#[tokio::test]
async fn a_template_sets_up_the_workspace_and_using_it_again_changes_nothing() {
    let Some(setup) = setup("awd76_templates").await else {
        return;
    };
    let (status, listed) = send(
        &setup.app,
        setup
            .browser
            .request("GET", &templates_path(&setup), None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canUse"], true);
    let ids = listed["templates"]
        .as_array()
        .expect("templates")
        .iter()
        .map(|template| template["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "everything",
            "chat_with_agents",
            "personal",
            "small_team",
            "software_team",
            "company"
        ]
    );
    assert!(
        listed["templates"][0]["whatItDoes"][0]
            .as_str()
            .is_some_and(|line| line.contains("every module")),
        "{listed}"
    );

    // Setup starts with picking a template, then who gets what.
    let before = onboarding(&setup).await;
    assert_eq!(&step_ids(&before)[..3], ["template", "workspace", "roles"]);
    assert_eq!(before["nextStep"], "template");

    let (status, applied) = apply(&setup, "Software team").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let changes = applied["changes"].to_string();
    assert!(changes.contains("Added the Contractor role."), "{changes}");
    assert!(
        changes.contains("Made the project Engineering (ENG)."),
        "{changes}"
    );
    assert!(changes.contains("#releases"), "{changes}");
    assert!(changes.contains("Releases collection"), "{changes}");
    let project = applied["projectId"].as_str().expect("project").to_owned();
    let project_id = project.parse::<Uuid>().expect("project ID");

    let channels = sqlx::query_scalar::<_, String>(
        "SELECT name FROM conversations WHERE project_id = $1 AND kind = 'channel' ORDER BY name",
    )
    .bind(project_id)
    .fetch_all(&setup.pool)
    .await
    .expect("channels");
    assert_eq!(channels, ["general", "incidents", "releases"]);
    let fields = sqlx::query_scalar::<_, String>(
        r#"
        SELECT field.name FROM field_definitions field
        JOIN collections collection ON field.collection_id = collection.id
        WHERE collection.project_id = $1 AND collection.name = 'Releases'
        ORDER BY field.position
        "#,
    )
    .bind(project_id)
    .fetch_all(&setup.pool)
    .await
    .expect("collection fields");
    assert_eq!(fields, ["Date", "Status", "Notes"]);
    let layout = sqlx::query_scalar::<_, String>(
        "SELECT value->>'preset' FROM settings WHERE organization_id = $1 AND scope = 'workspace' AND key = 'layout.default'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("layout");
    assert_eq!(layout, "software_team");

    // The path carries on in the project it made.
    let after = onboarding(&setup).await;
    assert_eq!(after["steps"][0]["done"], true, "{after}");
    assert_eq!(after["projectId"], project.as_str());

    // Using it again finds everything there.
    let roles_before = count(
        &setup,
        "SELECT count(*) FROM workspace_roles WHERE organization_id = $1",
    )
    .await;
    let projects_before = count(
        &setup,
        "SELECT count(*) FROM projects WHERE organization_id = $1",
    )
    .await;
    let (status, again) = apply(&setup, "software_team").await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["changes"], json!([]), "{again}");
    assert_eq!(again["projectId"], project.as_str());
    assert_eq!(
        count(
            &setup,
            "SELECT count(*) FROM workspace_roles WHERE organization_id = $1"
        )
        .await,
        roles_before
    );
    assert_eq!(
        count(
            &setup,
            "SELECT count(*) FROM projects WHERE organization_id = $1"
        )
        .await,
        projects_before
    );

    // Company: stricter roles, Employee by default, and Contractor's agents
    // limited, with no automations for it.
    let (status, company) = apply(&setup, "company").await;
    assert_eq!(status, StatusCode::OK, "{company}");
    let (default_role, contractor_limit, contractor_off) =
        sqlx::query_as::<_, (String, Option<Vec<String>>, Vec<String>)>(
            r#"
            SELECT (SELECT name FROM workspace_roles WHERE organization_id = $1 AND is_default),
                   contractor.agent_limit, contractor.modules_off
            FROM workspace_roles contractor
            WHERE contractor.organization_id = $1 AND contractor.name = 'Contractor'
            "#,
        )
        .bind(setup.organization_id)
        .fetch_one(&setup.pool)
        .await
        .expect("roles");
    assert_eq!(default_role, "Employee");
    assert!(contractor_limit.is_some_and(|limit| !limit.contains(&"tasks.create".to_owned())));
    assert_eq!(contractor_off, ["automations"]);

    // Chat with my agents: projects go off, and with them every setup step
    // that belongs to a project. Nothing is deleted.
    let projects_before = count(
        &setup,
        "SELECT count(*) FROM projects WHERE organization_id = $1 AND archived_at IS NULL",
    )
    .await;
    let (status, chat) = apply(&setup, "chat_with_agents").await;
    assert_eq!(status, StatusCode::OK, "{chat}");
    assert!(chat["changes"].to_string().contains("Turned off"), "{chat}");
    let steps = step_ids(&onboarding(&setup).await);
    assert_eq!(steps, ["template", "workspace", "roles", "agent"]);
    assert_eq!(
        count(
            &setup,
            "SELECT count(*) FROM projects WHERE organization_id = $1 AND archived_at IS NULL"
        )
        .await,
        projects_before
    );
    let (status, everything) = apply(&setup, "everything").await;
    assert_eq!(status, StatusCode::OK, "{everything}");
    assert!(step_ids(&onboarding(&setup).await).contains(&"github".to_owned()));

    // A template naming a project key someone else holds gets another key.
    let (status, personal) = apply(&setup, "personal").await;
    assert_eq!(status, StatusCode::OK, "{personal}");
    assert!(
        personal["changes"]
            .to_string()
            .contains("Reading list collection"),
        "{personal}"
    );
}

#[tokio::test]
async fn only_people_who_manage_the_workspace_use_or_save_templates() {
    let Some(setup) = setup("awd76_template_access").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;

    // A member reads them but can't use or save one.
    let (_, bea) = add_person(&setup, &[&work], "bea").await;
    let (status, listed) = send(
        app,
        bea.request("GET", &templates_path(&setup), None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canUse"], false);
    let (status, refused) = send(
        app,
        bea.request(
            "POST",
            &format!("{}/apply", templates_path(&setup)),
            None,
            json!({"template": "chat_with_agents"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = send(
        app,
        bea.request(
            "POST",
            &templates_path(&setup),
            None,
            json!({"name": "Bea's"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(
        count(
            &setup,
            "SELECT count(*) FROM workspace_template_uses WHERE organization_id = $1"
        )
        .await
            + count(
                &setup,
                "SELECT count(*) FROM workspace_templates WHERE organization_id = $1"
            )
            .await,
        0
    );

    // An agent reads them and can suggest one, but never uses one.
    let (agent, agent_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    super::mcp_tasks::add_member(&setup, &work, agent, "member").await;
    let agent_session = initialize(app, &agent_token).await.expect("agent");
    let read = tool(
        app,
        &agent_token,
        &agent_session,
        "workspace_template_list",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["canUse"], false);
    let refused = tool(
        app,
        &agent_token,
        &agent_session,
        "workspace_template_apply",
        json!({"projectId": work, "template": "chat_with_agents"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = tool(
        app,
        &agent_token,
        &agent_session,
        "workspace_template_save",
        json!({"projectId": work, "name": "Mara's"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // The owner's app saves the workspace with the project's channels and
    // collections, and uses templates.
    let collection = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections"),
            Some("make-playtests"),
            json!({"name": "Playtests"}),
        ),
    )
    .await;
    assert!(collection.0.is_success(), "{}", collection.1);
    let owner_app = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("owner app");
    let saved = tool(
        app,
        &owner_app,
        &owner_session,
        "workspace_template_save",
        json!({"projectId": work, "name": "Our way", "includeProject": true}),
    )
    .await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    let definition = &saved["value"]["template"]["definition"];
    assert_eq!(definition["project"]["key"], "WORK", "{definition}");
    assert_eq!(
        definition["project"]["collections"][0]["name"], "Playtests",
        "{definition}"
    );
    assert_eq!(definition["defaultRole"], "Member", "{definition}");
    let used = tool(
        app,
        &owner_app,
        &owner_session,
        "workspace_template_apply",
        json!({"projectId": work, "template": "Our way"}),
    )
    .await;
    assert_eq!(used["outcome"], "ok", "{used}");
    // Nothing changed: it's this workspace as it is.
    assert_eq!(used["value"]["changes"], json!([]), "{used}");
    let removed = tool(
        app,
        &owner_app,
        &owner_session,
        "workspace_template_save",
        json!({"projectId": work, "remove": "Our way"}),
    )
    .await;
    assert_eq!(removed["value"]["done"], "removed", "{removed}");
    let gone = tool(
        app,
        &owner_app,
        &owner_session,
        "workspace_template_apply",
        json!({"projectId": work, "template": "Our way"}),
    )
    .await;
    assert_eq!(gone["outcome"], "not_found", "{gone}");
}
