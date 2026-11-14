//! M1.8: packages. Exporting a space with collections, a form, an
//! automation, an outbound webhook, a role and a skill page gives a package
//! with no token, secret, webhook address, member or form link in it; the
//! same setup exported twice has the same digest; the built-in templates
//! read as packages that validate.

use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::{packages::format::Package, workspace_templates};

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn an_exported_space_carries_its_setup_and_nothing_private() {
    let Some(setup) = setup("m1_packages").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let space = create_project(&setup, "PKG", "Playtests").await;
    let space_id = space.parse::<Uuid>().expect("space");
    let token = connect_app(app, &setup.browser, "Packager", "full").await;
    let session = initialize(app, &token).await.expect("MCP session");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let collection = call(
        "collection_write",
        json!({
            "projectId": "PKG",
            "name": "Feedback",
            "fields": [
                {"name": "Severity", "type": "single_choice", "options": [{"name": "Low"}, {"name": "High"}]},
                {"name": "Notes", "type": "text"},
            ],
        }),
    )
    .await;
    assert_eq!(collection["outcome"], "ok", "{collection}");
    let collection_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM collections WHERE project_id = $1 AND name = 'Feedback'",
    )
    .bind(space_id)
    .fetch_one(pool)
    .await
    .expect("collection");
    let form = call(
        "bug_report_link_write",
        json!({
            "projectId": "PKG",
            "name": "Feedback form",
            "collection": "Feedback",
            "form": {"fields": [{"field": "Severity", "required": true}, {"field": "Notes"}]},
        }),
    )
    .await;
    assert_eq!(form["outcome"], "ok", "{form}");
    let form_url = form["value"]["formUrl"].as_str().expect("form link");
    let form_token = form_url.rsplit('/').next().expect("token").to_owned();
    let rule = call(
        "automation_write",
        json!({
            "projectId": "PKG",
            "operation": "create",
            "name": "Triage",
            "trigger": {"kind": "record_created", "collectionId": collection_id},
            "actions": [{"kind": "notify", "recipients": "owner", "message": "New: {{record.title}}"}],
        }),
    )
    .await;
    assert_eq!(rule["outcome"], "ok", "{rule}");
    let owner: Uuid = sqlx::query_scalar(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(pool)
    .await
    .expect("owner");
    // An outbound webhook whose address carries its secret.
    let hook_secret = "T0KENinTheP4thAndQueryXyZ0123456789abcdef";
    sqlx::query(
        r#"
        INSERT INTO webhook_subscriptions (
            id, organization_id, project_id, owner_actor_id, name, url, event_types
        ) VALUES ($1, $2, $3, $4, 'Builds out', $5, '{record.created}')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .bind(space_id)
    .bind(owner)
    .bind(format!(
        "https://hooks.example.org/services/{hook_secret}?key={hook_secret}"
    ))
    .execute(pool)
    .await
    .expect("webhook");
    let role = call(
        "role_write",
        json!({"projectId": "PKG", "name": "Tester", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
    )
    .await;
    assert_eq!(role["outcome"], "ok", "{role}");
    let skill = call(
        "page_write",
        json!({
            "projectId": "PKG",
            "title": "Running a playtest",
            "body": "Book the room, then post the build.",
            "skillDescription": "Use when running a playtest",
        }),
    )
    .await;
    assert_eq!(skill["outcome"], "ok", "{skill}");

    // An archived private channel named like a carried open one: a rule
    // that posts there must not export as posting to the open one.
    let channel = call(
        "channel_create",
        json!({"projectId": "PKG", "name": "releases"}),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let open_channel = "releases".to_owned();
    let hidden = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility, archived_at, \
         archived_by_actor_id) VALUES ($1, $2, 'channel', $3, 'members', now(), $4)",
    )
    .bind(hidden)
    .bind(space_id)
    .bind(&open_channel)
    .bind(owner)
    .execute(pool)
    .await
    .expect("archived private channel");
    sqlx::query(
        r#"
        INSERT INTO automation_rules (
            id, organization_id, project_id, owner_actor_id, name, trigger_kind, actions
        ) VALUES ($1, $2, $3, $4, 'Hidden post', 'task_created', $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(setup.organization_id)
    .bind(space_id)
    .bind(owner)
    .bind(json!([{"kind": "post_message", "conversationId": hidden, "text": "New task"}]))
    .execute(pool)
    .await
    .expect("rule posting to the hidden channel");

    let exported = call("package_export", json!({"spaceId": "PKG"})).await;
    assert_eq!(exported["outcome"], "ok", "{exported}");
    let document = &exported["value"]["package"];
    let text = serde_json::to_string(document).expect("JSON");
    let package = serde_json::from_value::<Package>(document.clone()).expect("a package");
    package
        .validate()
        .unwrap_or_else(|problems| panic!("{problems:?}: {text}"));
    let kinds = package
        .contents
        .iter()
        .map(|part| format!("{}:{}", part.kind.key(), part.key))
        .collect::<Vec<_>>();
    for expected in [
        "collection:feedback",
        "form:feedback_form",
        "automation:triage",
        "automation:builds_out",
        "role:tester",
        "skill:running_a_playtest",
    ] {
        assert!(
            kinds.iter().any(|kind| kind == expected),
            "{expected}: {kinds:?}"
        );
    }
    assert!(
        package
            .needs_admin
            .contains(&"form:feedback_form".to_owned())
    );
    assert!(
        package
            .needs_admin
            .contains(&"automation:builds_out".to_owned())
    );
    let part = |kind: &str, key: &str| {
        package
            .contents
            .iter()
            .find(|part| part.kind.key() == kind && part.key == key)
            .and_then(|part| part.body.clone())
            .expect("part")
    };
    assert_eq!(part("form", "feedback_form")["collection"], "@feedback");
    assert_eq!(
        part("automation", "triage")["trigger"]["collectionId"],
        "@feedback"
    );
    let hidden_post = part("automation", "hidden_post");
    assert_eq!(
        hidden_post["actions"][0].get("conversationId"),
        None,
        "{hidden_post}"
    );
    assert_eq!(
        hidden_post["needsSetup"],
        json!(["actions.conversationId"]),
        "{hidden_post}"
    );
    assert_eq!(
        part("automation", "builds_out")["webhook"]["host"],
        "hooks.example.org"
    );

    // Nothing private, by scanning the whole output.
    let (login, display): (String, String) =
        sqlx::query_as("SELECT login_name, display_name FROM users WHERE actor_id = $1")
            .bind(owner)
            .fetch_one(pool)
            .await
            .expect("owner names");
    let hint: String =
        sqlx::query_scalar("SELECT token_hint FROM bug_report_links WHERE project_id = $1 LIMIT 1")
            .bind(space_id)
            .fetch_one(pool)
            .await
            .expect("hint");
    let whole = serde_json::to_string(&exported).expect("JSON");
    for private in [
        form_token.as_str(),
        hook_secret,
        &display,
        &owner.to_string(),
        &space,
        &collection_id.to_string(),
        &setup.organization_id.to_string(),
        &hidden.to_string(),
    ] {
        assert!(!whole.contains(private), "{private} leaked: {whole}");
    }
    if !login.is_empty() {
        assert!(!whole.contains(&format!("@{login}")), "{login}: {whole}");
    }
    assert!(!whole.contains(&format!("\"{hint}\"")), "{hint}: {whole}");
    for field in ["token", "secret", "signingSecret", "members", "email"] {
        assert!(!whole.contains(&format!("\"{field}\"")), "{field}: {whole}");
    }

    // Exported again: the same digest.
    let again = call("package_export", json!({"spaceId": space})).await;
    assert_eq!(
        again["value"]["digest"], exported["value"]["digest"],
        "{again}"
    );
    assert_eq!(
        again["value"]["digest"]
            .as_str()
            .map(|digest| digest.starts_with("sha256:")),
        Some(true)
    );

    // The workspace's own setup: switches, layout and roles.
    let workspace = call("package_export", json!({"wholeWorkspace": true})).await;
    assert_eq!(workspace["outcome"], "ok", "{workspace}");
    assert_eq!(workspace["value"]["package"]["target"], "workspace");
    // One source at a time.
    let both = call(
        "package_export",
        json!({"spaceId": "PKG", "wholeWorkspace": true}),
    )
    .await;
    assert_eq!(both["outcome"], "invalid_request", "{both}");
}

#[tokio::test]
async fn the_built_in_templates_are_packages() {
    let mut count = 0;
    for (id, name, description, definition) in workspace_templates::builtin_definitions() {
        let package = agent_workspace_server::packages::format::from_template(
            id,
            name,
            description,
            definition,
        );
        package
            .validate()
            .unwrap_or_else(|problems| panic!("{id}: {problems:?}"));
        let document = serde_json::to_value(&package).expect("JSON");
        agent_workspace_server::packages::secrets_check::check(&document)
            .unwrap_or_else(|found| panic!("{id}: {found:?}"));
        count += 1;
    }
    assert_eq!(count, 6);
}
