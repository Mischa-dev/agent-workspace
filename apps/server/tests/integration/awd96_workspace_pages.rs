//! AWD-96: pages outside projects. The silent, costly failures: a
//! workspace page reaching someone outside the workspace (or who left it),
//! the workspace's own space showing up as a project, Projects off taking
//! workspace pages with it, and the space taking anything but pages.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::add_person,
    modules::switch,
};

async fn pages_space(setup: &Setup, browser: &Browser) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request(
            "GET",
            &format!(
                "/api/v1/organizations/{}/pages-space",
                setup.organization_id
            ),
            None,
            Value::Null,
        ),
    )
    .await
}

async fn get(setup: &Setup, browser: &Browser, path: &str) -> (StatusCode, Value) {
    send(&setup.app, browser.request("GET", path, None, Value::Null)).await
}

#[tokio::test]
async fn workspace_pages_reach_the_whole_workspace_and_nobody_else() {
    let Some(setup) = setup("awd96_pages").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (status, space) = pages_space(&setup, &setup.browser).await;
    assert_eq!(status, StatusCode::OK, "{space}");
    let space = space["projectId"].as_str().expect("space").to_owned();
    // Asking again finds the same space.
    let (_, again) = pages_space(&setup, &setup.browser).await;
    assert_eq!(again["projectId"], space.as_str());

    let (status, page) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{space}/pages"),
            None,
            json!({"title": "How we work", "body": "Everyone reads this."}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let page = page["id"].as_str().expect("page").to_owned();

    // Someone in the workspace (in another project) reads and lists it;
    // the space is never listed as a project.
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    let (status, bea_space) = pages_space(&setup, &bea_browser).await;
    assert_eq!(status, StatusCode::OK, "{bea_space}");
    assert_eq!(bea_space["projectId"], space.as_str());
    let (status, listed) = get(
        &setup,
        &bea_browser,
        &format!("/api/v1/projects/{space}/pages"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.to_string().contains(&page), "{listed}");
    let (status, projects) = get(&setup, &bea_browser, "/api/v1/projects").await;
    assert_eq!(status, StatusCode::OK, "{projects}");
    assert!(!projects.to_string().contains(&space), "{projects}");

    // The space holds pages and nothing else.
    let (status, body) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{space}/work-items"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "module_off", "{body}");

    // Projects off: the workspace's pages stay, a project's pages go.
    let (status, body) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"projects": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get(&setup, &bea_browser, &format!("/api/v1/pages/{page}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{space}/pages"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/pages"),
    )
    .await;
    assert_eq!(body["error"]["code"], "module_off", "{body}");
    assert_eq!(body["error"]["details"]["module"], "projects", "{body}");

    // An agent in the workspace reads and writes them too, as "workspace".
    let (status, body) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"projects": true}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_agent, token) = create_agent(app, &setup.browser, &work, "Scribe", "scribe").await;
    let (status, body) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"projects": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session = initialize(app, &token).await.expect("agent MCP");
    let listed = tool(
        app,
        &token,
        &session,
        "page_list",
        json!({"projectId": "workspace"}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed.to_string().contains(&page), "{listed}");
    let written = tool(
        app,
        &token,
        &session,
        "page_write",
        json!({"projectId": "workspace", "title": "Agent notes", "body": "Written with Projects off."}),
    )
    .await;
    assert!(
        written["outcome"] == "ok" || written["outcome"] == "applied",
        "{written}"
    );

    // Someone who leaves the workspace loses its pages at once.
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(bea)
    .execute(&setup.pool)
    .await
    .expect("bea leaves");
    let (status, _) = get(&setup, &bea_browser, &format!("/api/v1/pages/{page}")).await;
    assert_ne!(status, StatusCode::OK);
    let enabled: bool = sqlx::query_scalar(
        "SELECT enabled FROM project_memberships WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&space)
    .bind(bea)
    .fetch_one(&setup.pool)
    .await
    .expect("bea's space membership");
    assert!(!enabled);

    // A project that doesn't exist, or a workspace the caller isn't in, has
    // no space for them.
    let (status, _) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/organizations/{}/pages-space", Uuid::now_v7()),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}
