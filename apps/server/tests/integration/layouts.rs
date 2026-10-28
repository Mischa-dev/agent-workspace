//! Layouts (AWD-66): which parts of the app each person sees. They're visual
//! only, but who may set one for whom is easy to get silently wrong: members
//! can't change a project's layout or its rules, a rule for one person beats
//! the one for their role, a member's "Show hidden parts" does nothing, a
//! removed preset keeps working where it's used, and one workspace's saved
//! presets never reach another's projects.

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::Executor;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
    workspace_isolation::{add_person, login_as},
};

async fn get(app: &axum::Router, browser: &Browser, path: &str) -> Value {
    let (status, body) = send(app, browser.request("GET", path, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    body
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn owners_choose_layouts_for_roles_and_people_and_members_only_hide_more() {
    let Some(setup) = setup("layouts").await else {
        return;
    };
    let app = &setup.app;
    let owner = &setup.browser;
    let organization = setup.organization_id;
    let work = create_project(&setup, "WORK", "Work").await;
    let rook = add_person(&setup, &[&work], "rook", "rook", "Rook").await;
    let wren = add_person(&setup, &[&work], "wren", "wren", "Wren").await;
    add_person(&setup, &[&work], "finch", "finch", "Finch").await;
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE actor_id = $1")
        .bind(wren)
        .execute(&setup.pool)
        .await
        .expect("make Wren a viewer");
    let rook_browser = login_as(app, "rook").await;
    let wren_browser = login_as(app, "wren").await;
    let finch_browser = login_as(app, "finch").await;
    let project_layout = format!("/api/v1/projects/{work}/layout");

    // Nothing chosen: everything shows, and members can't change it.
    let fresh = get(app, &rook_browser, &project_layout).await;
    assert_eq!(fresh["effective"]["source"], "default", "{fresh}");
    assert!(strings(&fresh["effective"]["hidden"]).is_empty(), "{fresh}");
    assert_eq!(fresh["capabilities"]["manage"], false);
    for (method, path, body) in [
        (
            "PUT",
            project_layout.clone(),
            json!({"layout": {"preset": "minimal"}}),
        ),
        (
            "PUT",
            format!("{project_layout}/rules"),
            json!({"rules": [{"actorId": rook, "preset": "software_team"}]}),
        ),
        (
            "PUT",
            format!("/api/v1/organizations/{organization}/layout"),
            json!({"layout": {"preset": "minimal"}}),
        ),
        (
            "POST",
            format!("/api/v1/organizations/{organization}/layout/presets"),
            json!({"name": "Mine", "layout": {"preset": "minimal"}}),
        ),
    ] {
        let (status, body) = send(app, rook_browser.request(method, &path, None, body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }

    // The owner makes the project Personal life, gives Rook Bug reporting
    // and viewers Stakeholder, and members a preset the workspace saved.
    let (status, chores) = send(
        app,
        owner.request(
            "POST",
            &format!("/api/v1/organizations/{organization}/layout/presets"),
            None,
            json!({
                "name": "Chores",
                "layout": {
                    "preset": "household",
                    "changes": {"area.runs": true},
                    "words": {"task": {"one": "chore", "many": "chores"}},
                },
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{chores}");
    let chores = chores["id"].as_str().expect("preset id").to_owned();
    let (status, set) = send(
        app,
        owner.request(
            "PUT",
            &project_layout,
            None,
            json!({"layout": {"preset": "personal_life"}, "expectedVersion": 0}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["version"], 1);
    let (status, ruled) = send(
        app,
        owner.request(
            "PUT",
            &format!("{project_layout}/rules"),
            None,
            json!({"rules": [
                {"actorId": rook, "preset": "bug_reporting"},
                {"role": "viewer", "preset": "stakeholder"},
                {"role": "member", "preset": chores},
            ]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ruled}");
    assert_eq!(ruled["rules"].as_array().map(Vec::len), Some(3), "{ruled}");

    // A rule for one person beats the one for their role.
    let rook_view = get(app, &rook_browser, &project_layout).await;
    assert_eq!(rook_view["effective"]["source"], "person", "{rook_view}");
    assert_eq!(rook_view["effective"]["words"]["task"]["one"], "bug");
    let rook_hidden = strings(&rook_view["effective"]["hidden"]);
    assert!(rook_hidden.contains(&"area.chat") && !rook_hidden.contains(&"area.tasks"));
    // Only owners and admins see who gets what.
    assert!(
        rook_view["rules"].as_array().is_some_and(Vec::is_empty),
        "{rook_view}"
    );

    let wren_view = get(app, &wren_browser, &project_layout).await;
    assert_eq!(wren_view["effective"]["source"], "role", "{wren_view}");
    assert_eq!(wren_view["effective"]["preset"]["id"], "stakeholder");
    // Stakeholder renames nothing, so the project's words stay.
    assert_eq!(wren_view["effective"]["words"]["task"]["many"], "to-dos");

    let finch_view = get(app, &finch_browser, &project_layout).await;
    assert_eq!(finch_view["effective"]["preset"]["name"], "Chores");
    assert_eq!(finch_view["effective"]["words"]["task"]["many"], "chores");
    assert!(!strings(&finch_view["effective"]["hidden"]).contains(&"area.runs"));

    let owner_view = get(app, owner, &project_layout).await;
    assert_eq!(owner_view["effective"]["source"], "project", "{owner_view}");
    assert_eq!(owner_view["rules"].as_array().map(Vec::len), Some(3));
    assert_eq!(owner_view["capabilities"]["manage"], true);

    // A removed preset leaves pickers but keeps working where it's used.
    let (status, removed) = send(
        app,
        owner.request(
            "DELETE",
            &format!("/api/v1/organizations/{organization}/layout/presets/{chores}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{removed}");
    let finch_view = get(app, &finch_browser, &project_layout).await;
    assert_eq!(
        finch_view["effective"]["preset"]["removed"], true,
        "{finch_view}"
    );
    assert_eq!(finch_view["effective"]["words"]["task"]["many"], "chores");

    // "Show hidden parts" works for owners and admins only; anyone can hide
    // more for themselves.
    let (status, mine) = send(
        app,
        rook_browser.request(
            "PUT",
            &format!("/api/v1/organizations/{organization}/layout/mine"),
            None,
            json!({"hidden": ["task.labels"], "showHidden": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let rook_view = get(app, &rook_browser, &project_layout).await;
    let rook_hidden = strings(&rook_view["effective"]["hidden"]);
    assert!(rook_hidden.contains(&"area.chat") && rook_hidden.contains(&"task.labels"));
    assert_eq!(rook_view["effective"]["showingHidden"], false);
    assert_eq!(rook_view["myView"]["canShowHidden"], false);
    let (status, mine) = send(
        app,
        owner.request(
            "PUT",
            &format!("/api/v1/organizations/{organization}/layout/mine"),
            None,
            json!({"hidden": [], "showHidden": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let owner_view = get(app, owner, &project_layout).await;
    assert!(
        strings(&owner_view["effective"]["hidden"]).is_empty(),
        "{owner_view}"
    );
    assert!(!strings(&owner_view["effective"]["layoutHidden"]).is_empty());

    // Unknown parts and presets, and stale versions, are refused.
    for body in [
        json!({"layout": {"preset": "minimal", "changes": {"area.nothing": true}}}),
        json!({"layout": {"preset": "no_such_preset"}}),
        json!({"layout": {"preset": "minimal", "words": {"robot": {"one": "a", "many": "b"}}}}),
    ] {
        let (status, refused) = send(app, owner.request("PUT", &project_layout, None, body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }
    let (status, stale) = send(
        app,
        owner.request(
            "PUT",
            &project_layout,
            None,
            json!({"layout": {"preset": "kanban"}, "expectedVersion": 0}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");

    // Another workspace's preset can't be used or changed from here.
    let elsewhere = Uuid::now_v7();
    let stranger = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    setup
        .pool
        .execute(
            format!(
                "INSERT INTO organizations (id, name, slug) VALUES ('{elsewhere}', 'Elsewhere', 'elsewhere-{}');
                 INSERT INTO actors (id, kind) VALUES ('{stranger}', 'human');
                 INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ('{elsewhere}', '{stranger}', 'owner');
                 INSERT INTO layout_presets (id, organization_id, name, base, created_by_actor_id, updated_by_actor_id)
                 VALUES ('{foreign}', '{elsewhere}', 'Theirs', 'minimal', '{stranger}', '{stranger}');",
                elsewhere.simple()
            )
            .as_str(),
        )
        .await
        .expect("another workspace and its preset");
    for (path, body) in [
        (
            project_layout.clone(),
            json!({"layout": {"preset": foreign}}),
        ),
        (
            format!("{project_layout}/rules"),
            json!({"rules": [{"role": "member", "preset": foreign}]}),
        ),
        (
            format!("/api/v1/organizations/{organization}/layout"),
            json!({"layout": {"preset": foreign}}),
        ),
    ] {
        let (status, refused) = send(app, owner.request("PUT", &path, None, body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {refused}");
    }
    for path in [
        format!("/api/v1/organizations/{organization}/layout/presets/{foreign}"),
        format!("/api/v1/organizations/{elsewhere}/layout/presets/{foreign}"),
    ] {
        let (status, refused) = send(
            app,
            owner.request("PATCH", &path, None, json!({"name": "Mine now"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {refused}");
    }
    let (status, hidden) = send(
        app,
        owner.request(
            "GET",
            &format!("/api/v1/organizations/{elsewhere}/layout"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");

    // Through MCP: the owner's connected app sets the preset by name and
    // project_open names it; an agent may not.
    let app_token = connect_app(app, owner, "Desk", "full").await;
    let session = initialize(app, &app_token).await.expect("app session");
    let chosen = tool(
        app,
        &app_token,
        &session,
        "project_layout_set",
        json!({"projectId": work, "preset": "Simple to-do list"}),
    )
    .await;
    assert_eq!(chosen["outcome"], "ok", "{chosen}");
    assert_eq!(chosen["value"]["layout"]["preset"], "Simple to-do list");
    let unknown = tool(
        app,
        &app_token,
        &session,
        "project_layout_set",
        json!({"projectId": work, "preset": "Spaceship"}),
    )
    .await;
    assert_eq!(unknown["outcome"], "invalid_request", "{unknown}");
    let opened = tool(
        app,
        &app_token,
        &session,
        "project_open",
        json!({"projectId": work}),
    )
    .await;
    assert_eq!(
        opened["value"]["layout"]["preset"], "Simple to-do list",
        "{opened}"
    );
    assert!(
        opened["value"]["layout"]["tone"]
            .as_str()
            .is_some_and(|tone| !tone.is_empty())
    );
    let messages = space(&setup).await;
    let (agent, agent_token) = create_agent(app, owner, &messages, "Mara", "mara").await;
    add_member(&setup, &work, agent, "member").await;
    let agent_session = initialize(app, &agent_token).await.expect("agent session");
    let refused = tool(
        app,
        &agent_token,
        &agent_session,
        "project_layout_set",
        json!({"projectId": work, "preset": "Minimal"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A person's app goes by their role in the project, as the settings page
    // does: a member can't, a project admin who is only a member of the
    // workspace can.
    let rook_token = connect_app(app, &rook_browser, "Rook's desk", "full").await;
    let rook_session = initialize(app, &rook_token).await.expect("Rook's session");
    let as_member = tool(
        app,
        &rook_token,
        &rook_session,
        "project_layout_set",
        json!({"projectId": work, "preset": "Minimal"}),
    )
    .await;
    assert_eq!(as_member["outcome"], "permission_denied", "{as_member}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'admin' WHERE actor_id = $1 AND project_id = $2",
    )
    .bind(rook)
    .bind(Uuid::parse_str(&work).expect("project id"))
    .execute(&setup.pool)
    .await
    .expect("make Rook a project admin");
    let as_admin = tool(
        app,
        &rook_token,
        &rook_session,
        "project_layout_set",
        json!({"projectId": work, "preset": "Minimal"}),
    )
    .await;
    assert_eq!(as_admin["outcome"], "ok", "{as_admin}");
    assert_eq!(as_admin["value"]["layout"]["preset"], "Minimal");
}
