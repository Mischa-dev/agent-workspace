//! Fixes from the 2026-11 review sweep (AWD-107) where a mistake would be
//! silent: a project or task key two workspaces share must never quietly
//! pick one of them, and a workspace someone was removed from must stop
//! listing its archived projects to them.

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

/// A second workspace the owner also runs, with a project under `key`.
async fn second_workspace(setup: &Setup, key: &str) -> (Uuid, String) {
    let owner =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&setup.pool)
            .await
            .expect("owner");
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(&setup.pool)
        .await
        .expect("second organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(elsewhere)
    .bind(owner)
    .execute(&setup.pool)
    .await
    .expect("second membership");
    let (status, project) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some(&format!("create-elsewhere-{key}")),
            json!({"organizationId": elsewhere, "key": key, "name": "Theirs"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    (
        elsewhere,
        project["id"].as_str().expect("project").to_owned(),
    )
}

#[tokio::test]
async fn a_key_two_workspaces_share_is_refused_rather_than_guessed() {
    let Some(setup) = setup("sweep_keys").await else {
        return;
    };
    let app = &setup.app;
    let ours = create_project(&setup, "WORK", "Ours").await;
    let (_, theirs) = second_workspace(&setup, "WORK").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    for (project, title) in [(&ours, "Our first task"), (&theirs, "Their first task")] {
        let created = tool(
            app,
            &codex,
            &session,
            "task_create",
            json!({"projectId": project, "title": title}),
        )
        .await;
        assert_eq!(created["outcome"], "applied", "{created}");
        assert_eq!(created["key"], "WORK-1", "{created}");
    }

    // Archiving by a key both workspaces use would hit one at random.
    let archived = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": "WORK"}),
    )
    .await;
    assert_ne!(archived["outcome"], "ok", "{archived}");
    let archived_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE project_key = 'WORK' AND archived_at IS NOT NULL",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("archived projects");
    assert_eq!(archived_count, 0, "neither project was archived");
    // By ID it's unambiguous, and works.
    let archived = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": theirs}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let restored = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": theirs, "archived": false}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");

    // WORK-1 names two different tasks: opening or changing it by key is
    // refused, never pointed at one of them.
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_ne!(opened["outcome"], "ok", "{opened}");
    let commented = tool(
        app,
        &codex,
        &session,
        "task_comment",
        json!({"taskId": "WORK-1", "text": "Which one?", "clientRequestId": "sweep-ambiguous"}),
    )
    .await;
    assert_ne!(commented["outcome"], "applied", "{commented}");
    let comments = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_comments")
        .fetch_one(&setup.pool)
        .await
        .expect("comments");
    assert_eq!(comments, 0, "nothing was written to either task");
    // Each task is still reachable by its ID.
    let task_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_items WHERE project_id = $1 AND number = 1",
    )
    .bind(ours.parse::<Uuid>().expect("ours"))
    .fetch_one(&setup.pool)
    .await
    .expect("our task");
    let opened = tool(
        app,
        &codex,
        &session,
        "task_open",
        json!({"taskId": task_id.to_string()}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["task"]["title"], "Our first task");
}

#[tokio::test]
async fn historical_aliases_cannot_hide_a_conflicting_task_key() {
    let Some(setup) = setup("sweep_alias_keys").await else {
        return;
    };
    let ours = create_project(&setup, "WORK", "Ours").await;
    let source = create_project(&setup, "FROMSRC", "Renamed source").await;
    let (elsewhere, theirs) = second_workspace(&setup, "OTHER").await;
    let (status, foreign_source) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some("foreign-source"),
            json!({"organizationId": elsewhere, "key": "FROMSRC", "name": "Foreign source"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{foreign_source}");
    let foreign_source = foreign_source["id"].as_str().expect("source");
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    let session = initialize(&setup.app, &token).await.expect("MCP");
    let mut tasks = Vec::new();
    for project in [
        ours.as_str(),
        source.as_str(),
        theirs.as_str(),
        foreign_source,
    ] {
        let made = tool(
            &setup.app,
            &token,
            &session,
            "task_create",
            json!({"projectId": project, "title": "Task"}),
        )
        .await;
        assert_eq!(made["outcome"], "applied", "{made}");
        tasks.push(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE project_id = $1::uuid AND number = 1",
            )
            .bind(project)
            .fetch_one(&setup.pool)
            .await
            .expect("task"),
        );
    }
    // A historical WORK-1 can point back to today's WORK-1 after project
    // keys change. Another workspace also has a historical WORK-1. Both
    // aliases must participate, even when the first duplicates a current ID.
    for (source, target) in [(tasks[1], tasks[0]), (tasks[3], tasks[2])] {
        sqlx::query("INSERT INTO work_item_key_aliases (id, organization_id, project_key, number, source_work_item_id, work_item_id, moved_by_actor_id) SELECT $1, project.organization_id, 'WORK', 1, $2, $3, (SELECT actor_id FROM users WHERE login_name = 'owner') FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.id = $2")
            .bind(Uuid::now_v7()).bind(source).bind(target)
            .execute(&setup.pool).await.expect("historical alias");
    }
    let opened = tool(
        &setup.app,
        &token,
        &session,
        "task_open",
        json!({"taskId": "WORK-1"}),
    )
    .await;
    assert_ne!(
        opened["outcome"], "ok",
        "a current-key fallback must not pick ours: {opened}"
    );
    let commented = tool(
        &setup.app,
        &token,
        &session,
        "task_comment",
        json!({"taskId": "WORK-1", "text": "Wrong task?", "clientRequestId": "alias-ambiguity"}),
    )
    .await;
    assert_ne!(commented["outcome"], "applied", "{commented}");
    let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_comments")
        .fetch_one(&setup.pool)
        .await
        .expect("comments");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn a_workspace_you_left_stops_listing_its_archived_projects() {
    let Some(setup) = setup("sweep_archived").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "WORK", "Ours").await;
    let (elsewhere, theirs) = second_workspace(&setup, "OLD").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let archived = tool(
        app,
        &codex,
        &session,
        "project_archive",
        json!({"project": theirs}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let lists_theirs = || async {
        let listed = tool(
            app,
            &codex,
            &session,
            "projects_list",
            json!({"includeArchived": true}),
        )
        .await;
        assert_eq!(listed["outcome"], "ok", "{listed}");
        listed["value"]["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == theirs.as_str())
    };
    assert!(lists_theirs().await, "a member sees its archived projects");

    // Removed from that workspace (its project membership left behind),
    // the owner no longer sees its archived projects, names and counts.
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = (SELECT actor_id FROM users WHERE login_name = 'owner')",
    )
    .bind(elsewhere)
    .execute(&setup.pool)
    .await
    .expect("leave the workspace");
    assert!(!lists_theirs().await, "its archived projects stay hidden");
}
