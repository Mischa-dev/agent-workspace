//! K3.13: links written in text reach the link store without a route per
//! kind. The silent, costly failures: a link to another workspace's thing
//! that one workspace can then follow, an address form (`/w/<slug>/...`)
//! that stops counting, and new places to write (page comments, boards,
//! sheets) whose links never show.

use super::{
    agent_interface::{Browser, send},
    support::two_workspaces::two_workspaces,
};
use axum::http::StatusCode;
use serde_json::Value;

/// The links the thing's page shows, as (label, other kind, other id).
async fn links_of(
    app: &axum::Router,
    browser: &Browser,
    kind: &str,
    id: &str,
) -> Vec<(String, String, String)> {
    let (status, opened) = send(
        app,
        browser.request(
            "GET",
            &format!("/api/v1/things/{kind}/{id}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    opened["links"]
        .as_array()
        .expect("links")
        .iter()
        .map(|link| {
            (
                link["label"].as_str().expect("label").to_owned(),
                link["other"]["kind"].as_str().expect("kind").to_owned(),
                link["other"]["id"].as_str().expect("id").to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn text_links_count_by_the_registry_and_never_cross_workspaces() {
    let Some(world) = two_workspaces("k3_13_text_links").await else {
        return;
    };
    let (a, b) = (&world.a, &world.b);
    let pool = &world.pool;

    // A page names a sheet under the workspace's own prefix, embeds a
    // board, and points at B's task.
    let body = format!(
        "Numbers: https://work.example/w/{slug}/projects/{project}/sheets/{sheet}?tab=x\n\n\
         [Sketch](/projects/{project}/whiteboards/{board} \"embed\")\n\n\
         Theirs: /projects/{b_project}/work-items/{b_task}",
        slug = a.slug,
        project = a.project,
        sheet = a.sheet,
        board = a.project_whiteboard,
        b_project = b.project,
        b_task = b.task,
    );
    sqlx::query("UPDATE project_pages SET body = $2 WHERE id = $1::uuid")
        .bind(&a.page)
        .bind(&body)
        .execute(pool)
        .await
        .expect("page");

    let sheet = links_of(&world.app, &a.browser, "sheet", &a.sheet.to_string()).await;
    assert!(
        sheet.contains(&(
            "Referenced in".to_owned(),
            "page".to_owned(),
            a.page.clone()
        )),
        "{sheet:?}"
    );
    let board = links_of(
        &world.app,
        &a.browser,
        "whiteboard",
        &a.project_whiteboard.to_string(),
    )
    .await;
    assert!(
        board.contains(&("Embedded in".to_owned(), "page".to_owned(), a.page.clone())),
        "{board:?}"
    );
    let page = links_of(&world.app, &a.browser, "page", &a.page).await;
    assert!(
        page.contains(&(
            "Embeds".to_owned(),
            "whiteboard".to_owned(),
            a.project_whiteboard.to_string()
        )),
        "{page:?}"
    );
    // No link joins the two workspaces.
    let crossing = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM links link \
         JOIN things source ON source.id = link.from_thing_id \
         JOIN things target ON target.id = link.to_thing_id \
         WHERE source.workspace_id <> target.workspace_id \
            OR link.workspace_id <> source.workspace_id",
    )
    .fetch_one(pool)
    .await
    .expect("crossing");
    assert_eq!(crossing, 0);
    let b_task = links_of(&world.app, &b.browser, "task", &b.task.to_string()).await;
    assert!(!b_task.iter().any(|(_, _, id)| *id == a.page), "{b_task:?}");

    // A page comment (a message in a thread under the page's comments)
    // and a board's text count too, held by the page and the board.
    let (status, comment) = send(
        &world.app,
        a.browser.request(
            "POST",
            &format!("/api/v1/pages/{}/comments", a.page),
            None,
            serde_json::json!({"body": format!(
                "See /projects/{}/collections/{}?record={}",
                a.project, a.collection, a.record
            )}),
        ),
    )
    .await;
    assert!(status.is_success(), "{comment}");
    let record = links_of(&world.app, &a.browser, "record", &a.record.to_string()).await;
    assert!(
        record.contains(&(
            "Referenced in".to_owned(),
            "page".to_owned(),
            a.page.clone()
        )),
        "{record:?}"
    );
    sqlx::query(
        "INSERT INTO whiteboard_elements \
            (whiteboard_id, element_id, element_type, version, version_nonce, data, seq) \
         VALUES ($1, 'k313-note', 'text', 1, 1, $2, 1)",
    )
    .bind(a.project_whiteboard)
    .bind(serde_json::json!({"text": format!("for /projects/{}/work-items/{}", a.project, a.task)}))
    .execute(pool)
    .await
    .expect("element");
    let task = links_of(&world.app, &a.browser, "task", &a.task.to_string()).await;
    let board_id = a.project_whiteboard.to_string();
    assert!(
        task.contains(&(
            "Referenced in".to_owned(),
            "whiteboard".to_owned(),
            board_id.clone()
        )),
        "{task:?}"
    );
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM links \
         WHERE source_kind = 'whiteboard' AND source_id = $1 AND to_thing_id = $2",
    )
    .bind(a.project_whiteboard)
    .bind(a.task)
    .fetch_one(pool)
    .await
    .expect("held");
    assert_eq!(held, 1, "the board's text links the task");
    // Taking the text away takes the link away.
    sqlx::query(
        "UPDATE whiteboard_elements SET is_deleted = true, seq = 2 WHERE element_id = 'k313-note'",
    )
    .execute(pool)
    .await
    .expect("delete element");
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM links \
         WHERE source_kind = 'whiteboard' AND source_id = $1 AND to_thing_id = $2",
    )
    .bind(a.project_whiteboard)
    .bind(a.task)
    .fetch_one(pool)
    .await
    .expect("held");
    assert_eq!(held, 0, "the board's text no longer links the task");

    world.finish().await;
}
