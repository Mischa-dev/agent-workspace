//! AWD-75: task and board tools check the module in the project or workspace they act in, destinations included.

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
    modules::switch,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

fn assert_module_off(result: &Value, module: &str) {
    assert_eq!(result["outcome"], "permission_denied", "{result}");
    assert_eq!(result["retryCategory"], "module", "{result}");
    assert!(
        result["message"]
            .as_str()
            .is_some_and(|message| message.contains(module)),
        "{result}"
    );
}

/// The owner is in this test's workspace and a second one where every
/// module stays on, so the tool gate (which, naming no project, follows
/// every workspace) lets each call through; the tool itself has to check
/// the module where it acts: the tasks it was given by ID, a move's
/// target, a linked board, and the workspace a board or project goes in.
#[tokio::test]
async fn task_and_board_tools_check_the_module_where_they_act() {
    let Some(setup) = setup("awd75_targets").await else {
        return;
    };
    let app = &setup.app;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let token = connect_app(app, &setup.browser, "Owner's app", "full").await;
    let session = initialize(app, &token).await.expect("owner app");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let second = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Second', $2)")
        .bind(second)
        .bind(format!("second-{}", second.simple()))
        .execute(&setup.pool)
        .await
        .expect("second workspace");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(second)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    let here = create_project(&setup, "AAA", "Here").await;
    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some("awd75-targets-bbb"),
            json!({"organizationId": second, "key": "BBB", "name": "There"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let there = made["id"].as_str().expect("project").to_owned();
    let mut tasks = Vec::new();
    for (project, title) in [
        (&here, "Here one"),
        (&here, "Here two"),
        (&there, "There one"),
        (&there, "There two"),
    ] {
        let created = call("task_create", json!({"projectId": project, "title": title})).await;
        assert_eq!(created["outcome"], "applied", "{created}");
        tasks.push(created["workItemId"].as_str().expect("task").to_owned());
    }
    let board = call(
        "whiteboard_create",
        json!({"title": "Team board", "workspaceId": setup.organization_id}),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    // Before anything is off: with no project or workspace named, a board
    // and a project go into this workspace (the app's own).
    let default_board = call("whiteboard_create", json!({"title": "Default board"})).await;
    assert_eq!(default_board["outcome"], "ok", "{default_board}");
    let made_project = call("project_create", json!({"key": "PRE", "name": "Before"})).await;
    assert_eq!(made_project["outcome"], "ok", "{made_project}");
    for (table, id) in [
        ("whiteboards", &default_board["value"]["whiteboard"]["id"]),
        ("projects", &made_project["value"]["id"]),
    ] {
        let organization = sqlx::query_scalar::<_, Uuid>(&format!(
            "SELECT organization_id FROM {table} WHERE id = $1"
        ))
        .bind(id.as_str().expect("id").parse::<Uuid>().expect("uuid"))
        .fetch_one(&setup.pool)
        .await
        .expect("organization");
        assert_eq!(organization, setup.organization_id, "{table}");
    }

    // Whiteboards off here, Tasks still on: linking this workspace's board
    // to a task here, and making a board here, say Whiteboards is off.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"whiteboards": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let linked = call(
        "task_link",
        json!({"taskId": tasks[0], "whiteboardId": board}),
    )
    .await;
    assert_module_off(&linked, "Whiteboards");
    let links = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM whiteboard_links WHERE whiteboard_id = $1",
    )
    .bind(board.parse::<Uuid>().expect("board"))
    .fetch_one(&setup.pool)
    .await
    .expect("links");
    assert_eq!(links, 0, "a board was linked where Whiteboards is off");
    for arguments in [
        json!({"title": "Named here", "workspaceId": setup.organization_id}),
        json!({"title": "Defaulted here"}),
    ] {
        let refused = call("whiteboard_create", arguments.clone()).await;
        assert_module_off(&refused, "Whiteboards");
    }
    let boards = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM whiteboards WHERE title IN ('Named here', 'Defaulted here')",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("boards");
    assert_eq!(boards, 0, "a board was made where Whiteboards is off");
    let elsewhere = call(
        "whiteboard_create",
        json!({"title": "There board", "workspaceId": second}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "ok", "{elsewhere}");

    // Tasks off here: reordering tasks here by ID, and moving a task from
    // the second workspace into a project here, say Tasks is off; the same
    // in the second workspace still works.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"tasks": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let reordered = call(
        "task_reorder",
        json!({"taskIds": [tasks[1], tasks[0]], "position": "top"}),
    )
    .await;
    assert_module_off(&reordered, "Tasks");
    let reordered = call(
        "task_reorder",
        json!({"taskIds": [tasks[3], tasks[2]], "position": "top"}),
    )
    .await;
    assert_eq!(reordered["outcome"], "ok", "{reordered}");
    let moved = call(
        "task_move",
        json!({"taskId": tasks[2], "toProjectId": here}),
    )
    .await;
    assert_module_off(&moved, "Tasks");
    let (archived, in_here) = sqlx::query_as::<_, (bool, i64)>(
        r#"
        SELECT (SELECT archived_at IS NOT NULL FROM task_rows WHERE id = $1),
               (SELECT count(*) FROM task_rows WHERE project_id = $2 AND title = 'There one')
        "#,
    )
    .bind(tasks[2].parse::<Uuid>().expect("task"))
    .bind(here.parse::<Uuid>().expect("project"))
    .fetch_one(&setup.pool)
    .await
    .expect("tasks");
    assert!(
        !archived,
        "the task left a project for one where Tasks is off"
    );
    assert_eq!(in_here, 0, "a task was moved in where Tasks is off");

    // Projects off here: a project the app would make here says so, and
    // none is made.
    let (status, changed) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"projects": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let refused = call("project_create", json!({"key": "OFF", "name": "Off"})).await;
    assert_module_off(&refused, "Projects");
    let made =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE project_key = 'OFF'")
            .fetch_one(&setup.pool)
            .await
            .expect("projects");
    assert_eq!(made, 0, "a project was made where Projects is off");
}
