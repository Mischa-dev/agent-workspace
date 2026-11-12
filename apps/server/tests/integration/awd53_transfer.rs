//! AWD-53 criterion 9: a page goes out as Markdown or a Word document and
//! comes back in as a page, keeping what a page can hold.

use agent_workspace_server::page_docs::{docx, tree::from_markdown};
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    agent_interface::{Browser, send},
    mcp_admin::{Setup, create_project, setup},
    message_edits::add_person,
    w20_files::upload,
};

const BODY: &str = "## Goals\n\nShip **the editor** with [a link](https://example.com).\n\n1. Draft\n2. Review\n   - [ ] read it\n\n| Who | When |\n| --- | --- |\n| Mara | Today |";

async fn download(setup: &Setup, browser: &Browser, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let response = setup
        .app
        .clone()
        .oneshot(browser.request("GET", uri, None, Value::Null))
        .await
        .expect("response");
    let status = response.status();
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), 10_000_000)
        .await
        .expect("body")
        .to_vec();
    (status, disposition, bytes)
}

#[tokio::test]
async fn pages_go_out_to_word_and_markdown_and_come_back() {
    let Some(setup) = setup("awd53_transfer").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (status, page) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some("transfer-page"),
            json!({"title": "Launch plan", "body": BODY}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page = page["id"].as_str().expect("page").to_owned();

    // Markdown: the title as the top heading, then the page's text.
    let (status, disposition, markdown) = download(
        &setup,
        &setup.browser,
        &format!("/api/v1/pages/{page}/export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(disposition.contains("Launch plan.md"), "{disposition}");
    let markdown = String::from_utf8(markdown).expect("text");
    assert_eq!(markdown, format!("# Launch plan\n\n{BODY}\n"));

    // Word: a document that reads back as the same page.
    let (status, disposition, word) = download(
        &setup,
        &setup.browser,
        &format!("/api/v1/pages/{page}/export?format=docx"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(disposition.contains("Launch plan.docx"), "{disposition}");
    let read = docx::import(&word, None).expect("a Word document");
    assert_eq!(read.title.as_deref(), Some("Launch plan"));
    assert_eq!(read.blocks, from_markdown(BODY));

    // Back in: each file makes a new page with the same text.
    // A title another page has gets a number.
    for (name, bytes, title) in [
        ("plan.docx", word.clone(), "Launch plan (2)"),
        ("plan.md", markdown.into_bytes(), "Launch plan (3)"),
    ] {
        let (status, imported) = upload(
            &setup,
            &setup.browser,
            &format!("/api/v1/projects/{work}/pages/import?name={name}"),
            "application/octet-stream",
            &bytes,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}: {imported}");
        assert_eq!(imported["page"]["title"], title, "{name}");
        assert_eq!(imported["page"]["body"], BODY, "{name}");
        assert_eq!(imported["setAside"], json!([]), "{name}");
        assert_ne!(imported["page"]["id"], page.as_str());
    }

    // What isn't a document is refused, and says what to send instead.
    let (status, refused) = upload(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/pages/import?name=plan.docx"),
        "application/octet-stream",
        b"not a zip",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, refused) = upload(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/pages/import?name=plan.pdf"),
        "application/pdf",
        b"%PDF",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // Someone who may only read the project can export, not import.
    let (viewer, viewer_browser) = add_person(&setup, &[&work], "vic").await;
    sqlx::query("UPDATE project_memberships SET role = 'viewer' WHERE actor_id = $1")
        .bind(viewer)
        .execute(&setup.pool)
        .await
        .expect("viewer");
    let (status, _, _) = download(
        &setup,
        &viewer_browser,
        &format!("/api/v1/pages/{page}/export?format=docx"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = upload(
        &setup,
        &viewer_browser,
        &format!("/api/v1/projects/{work}/pages/import?name=plan.docx"),
        "application/octet-stream",
        &word,
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );

    // Someone outside the project gets nothing either way.
    let other = create_project(&setup, "OTHER", "Other").await;
    let (_outsider, outsider_browser) = add_person(&setup, &[&other], "otto").await;
    let (status, _, _) = download(
        &setup,
        &outsider_browser,
        &format!("/api/v1/pages/{page}/export"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = upload(
        &setup,
        &outsider_browser,
        &format!("/api/v1/projects/{work}/pages/import?name=plan.md"),
        "text/markdown",
        b"# Sneaky\n\nhi",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
