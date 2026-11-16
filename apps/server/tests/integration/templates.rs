//! C1.24: templates for pages and boards. "New page from template" copies a
//! template page (or a built-in one) with `{{date}}`, `{{author}}` and
//! `{{project}}` filled in, from the web and from `page_write`; a board starts
//! from a built-in template's drawing; and a template in a project the
//! caller can't see is not there for them.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{send, tool},
    world::WorldBuilder,
};

#[tokio::test]
async fn pages_and_boards_start_from_templates() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("SECRET")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .build("c1_24_templates")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = world.project("WORK");
    let secret = world.project("SECRET");
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let rook = &world.agents["rook"];
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let name_of = |actor: Uuid| async move {
        sqlx::query_scalar::<_, String>(
            "SELECT display_name FROM project_memberships WHERE actor_id = $1 AND project_id = $2::uuid",
        )
        .bind(actor)
        .bind(work)
        .fetch_one(pool)
        .await
        .expect("name")
    };

    // Lena makes a page into a template.
    let (status, template) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some("template-page"),
            json!({
                "title": "Standup",
                "body": "Standup on {{date}} by {{author}} for {{project}}. {{unknown}} stays.",
                "isTemplate": true,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{template}");
    assert_eq!(template["isTemplate"], true, "{template}");
    let template_id = template["id"].as_str().expect("template").to_owned();

    // The gallery has the built-in ones and hers.
    let (status, gallery) = send(
        app,
        mara.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/page-templates"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gallery}");
    let ids = gallery["templates"]
        .as_array()
        .expect("templates")
        .iter()
        .map(|template| template["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"meeting-notes".to_owned()), "{gallery}");
    assert!(ids.contains(&template_id), "{gallery}");

    // "New page from template" in the web: Mara's copy, filled in for her.
    let (status, page) = send(
        app,
        mara.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some("from-template"),
            json!({ "fromTemplateId": template_id }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let mara_name = name_of(mara.actor).await;
    assert_eq!(page["title"], "Standup (2)", "{page}");
    assert_eq!(
        page["body"],
        format!("Standup on {today} by {mara_name} for WORK. {{{{unknown}}}} stays."),
    );
    assert_eq!(page["isTemplate"], false, "{page}");

    // An agent does the same with page_write, from a built-in template.
    let written = tool(
        app,
        &rook.token,
        &rook.session,
        "page_write",
        json!({
            "projectId": work,
            "fromTemplateId": "meeting-notes",
            "clientRequestId": "rook-notes",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let rook_name = name_of(rook.actor).await;
    assert_eq!(written["value"]["title"], format!("Meeting notes {today}"));
    let body = written["value"]["body"].as_str().expect("body");
    assert!(body.contains(&rook_name) && body.contains("WORK"), "{body}");
    assert!(!body.contains("{{"), "{body}");

    // A template in a project Mara isn't in isn't there for her.
    let (status, hidden) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{secret}/pages"),
            Some("secret-template"),
            json!({"title": "Secret plan", "body": "the plan", "isTemplate": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hidden}");
    sqlx::query("DELETE FROM project_memberships WHERE actor_id = $1 AND project_id = $2::uuid")
        .bind(mara.actor)
        .bind(secret)
        .execute(pool)
        .await
        .expect("leave SECRET");
    let (status, refused) = send(
        app,
        mara.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some("hidden-template"),
            json!({ "fromTemplateId": hidden["id"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert!(!refused.to_string().contains("the plan"), "{refused}");
    let (_, gallery) = send(
        app,
        mara.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/page-templates"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!gallery.to_string().contains("Secret plan"), "{gallery}");

    // A board from a built-in template has its drawing.
    let board = tool(
        app,
        &rook.token,
        &rook.session,
        "whiteboard_create",
        json!({"title": "Release flow", "projectId": work, "fromTemplateId": "flowchart"}),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    assert!(
        board["value"]["whiteboard"]["elementCount"]
            .as_i64()
            .unwrap_or(0)
            >= 5,
        "{board}"
    );

    world.finish().await;
}
