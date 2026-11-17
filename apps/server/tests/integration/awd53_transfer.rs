//! AWD-53 criterion 9: a page goes out as Markdown or a Word document and
//! comes back in as a page, keeping what a page can hold.

use agent_workspace_server::page_docs::{docx, pdf, tree::from_markdown};
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
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

/// C1.23: a page goes out as a PDF with its words searchable, and a sheet
/// range it embeds shows its values only to a reader who may open the sheet.
#[tokio::test]
async fn pages_go_out_as_pdf_with_what_the_reader_may_see() {
    let Some(setup) = setup("c1_23_pdf").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTHER", "Other").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let mut sheets = Vec::new();
    for (project, name, cells) in [
        (
            "WORK",
            "Budget",
            [
                ("A1", "Item"),
                ("B1", "Cost"),
                ("A2", "Rent"),
                ("B2", "1200"),
            ],
        ),
        (
            "OTHER",
            "Pay",
            [
                ("A1", "Who"),
                ("B1", "Pay"),
                ("A2", "Mara"),
                ("B2", "SECRETPAY"),
            ],
        ),
    ] {
        let written = tool(
            app,
            &codex,
            &session,
            "sheet_write",
            json!({
                "projectId": project,
                "name": name,
                "cells": cells
                    .iter()
                    .map(|(cell, input)| json!({"cell": cell, "input": input}))
                    .collect::<Vec<_>>(),
                "clientRequestId": format!("pdf-{name}"),
            }),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "{written}");
        sheets.push(written["value"]["id"].as_str().expect("sheet").to_owned());
    }
    let board = tool(
        app,
        &codex,
        &session,
        "whiteboard_create",
        json!({
            "title": "Release flow",
            "projectId": work,
            "operations": [
                {"op": "add", "type": "rectangle", "id": "write", "text": "Write"},
                {"op": "add", "type": "ellipse", "id": "ship", "x": 400, "text": "Ship"},
                {"op": "connect", "from": "write", "to": "ship"},
            ],
        }),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let body = format!(
        "## Goals\n\nShip **on Friday**.\n\n| Step | Owner |\n| --- | --- |\n| Draft | Lena |\n\n```\nfn main() {{}}\n```\n\n[Budget](/projects/{work}/sheets/{} \"embed\")\n\n[Pay sheet](/projects/{other}/sheets/{} \"embed\")\n\n[Release flow](/whiteboards/{board} \"embed\")\n\n[WORK-1 Fix login](/projects/{work}/work-items/WORK-1 \"embed\")",
        sheets[0], sheets[1]
    );
    let (status, page) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some("pdf-page"),
            json!({"title": "Launch plan", "body": body}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page = page["id"].as_str().expect("page").to_owned();
    let uri = format!("/api/v1/pages/{page}/export?format=pdf");

    let (status, disposition, pdf) = download(&setup, &setup.browser, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(disposition.contains("Launch plan.pdf"), "{disposition}");
    assert!(pdf.starts_with(b"%PDF-"));
    let text = pdf::text_of(&pdf);
    for words in [
        "Launch",
        "Goals",
        "Friday",
        "Lena",
        "main()",
        "Rent",
        "1200",
        "SECRETPAY",
        "WORK-1",
    ] {
        assert!(text.contains(words), "{words} missing from {text}");
    }
    // The board is drawn in as a picture.
    let source = String::from_utf8_lossy(&pdf);
    assert!(source.contains("/Subtype /Image"), "no board picture");

    // Someone in the page's project but not the other one gets the page and
    // its own sheet, and only the other sheet's label and link.
    let (_reader, reader_browser) = add_person(&setup, &[&work], "rhea").await;
    let (status, _, pdf) = download(&setup, &reader_browser, &uri).await;
    assert_eq!(status, StatusCode::OK);
    let text = pdf::text_of(&pdf)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("Rent") && text.contains("Pay sheet"),
        "{text}"
    );
    assert!(!text.contains("SECRETPAY"), "{text}");
    assert!(!text.contains("Mara"), "{text}");
}
