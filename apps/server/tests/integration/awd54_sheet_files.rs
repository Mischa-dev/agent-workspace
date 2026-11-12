//! AWD-54 slice 3: sheets in and out as files. The silent, costly failures:
//! an export that loses formulas or tabs, an import that drops cells or
//! lands half a file, and an agent importing a file it can't see.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

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

async fn import(setup: &Setup, uri: &str, key: &str, bytes: &[u8]) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("origin", "http://127.0.0.1:3100")
        .header("cookie", &setup.browser.cookie)
        .header("x-csrf-token", &setup.browser.csrf)
        .header("idempotency-key", key)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes.to_vec()))
        .expect("import");
    send(&setup.app, request).await
}

/// A cell's value in a sheet's body, by row and column.
fn value_in(body: &Value, row: i64, column: i64) -> Value {
    body["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|cell| cell["row"] == row && cell["column"] == column)
        .map(|cell| cell["value"].clone())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn sheets_go_out_to_excel_and_csv_and_come_back() {
    let Some(setup) = setup("awd54_sheet_files").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");

    // An agent imports CSV text: each field as typed, formulas worked out.
    let made = tool(
        app,
        &codex,
        &session,
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": "Costs",
            "import": {"csv": "Item,Cost\nRent,1200\nFood,300\nTotal,=SUM(B2:B3)\n"},
            "clientRequestId": "import-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert_eq!(made["value"]["created"], true);
    let sheet = made["value"]["id"].as_str().expect("sheet").to_owned();
    // Import goes into a new sheet only, on its own.
    let mixed = tool(
        app,
        &codex,
        &session,
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Costs",
            "import": {"csv": "a"},
            "clientRequestId": "import-2",
        }),
    )
    .await;
    assert_eq!(mixed["outcome"], "invalid_request", "{mixed}");
    // A file it can't see is refused as one.
    let unseen = tool(
        app,
        &codex,
        &session,
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": "Unseen",
            "import": {"fileId": Uuid::now_v7().to_string()},
            "clientRequestId": "import-3",
        }),
    )
    .await;
    assert_eq!(unseen["outcome"], "invalid_request", "{unseen}");

    // Add a second tab whose formula reads the first.
    let tabbed = tool(
        app,
        &codex,
        &session,
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Costs",
            "tabs": [{"add": "Summary"}],
            "cells": [{"tab": "Summary", "cell": "A1", "input": "=Sheet1!B4*2"}],
            "clientRequestId": "import-4",
        }),
    )
    .await;
    assert_eq!(tabbed["outcome"], "ok", "{tabbed}");

    // CSV: one tab's values as they show.
    let (status, disposition, csv) = download(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/sheets/{sheet}/export?format=csv"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(disposition.contains("Costs - Sheet1.csv"), "{disposition}");
    assert_eq!(
        String::from_utf8(csv).expect("text"),
        "Item,Cost\nRent,1200\nFood,300\nTotal,1500\n"
    );

    // Excel: every tab, with formulas, that imports back as the same sheet.
    let (status, disposition, xlsx) = download(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/sheets/{sheet}/export?format=xlsx"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(disposition.contains("Costs.xlsx"), "{disposition}");
    let uri = format!("/api/v1/projects/{work}/sheets/import?name=Costs%20copy&format=xlsx");
    let (status, copy) = import(&setup, &uri, "import-http-1", &xlsx).await;
    assert_eq!(status, StatusCode::CREATED, "{copy}");
    assert_eq!(copy["sheet"]["tabs"], json!(["Sheet1", "Summary"]));
    assert_eq!(value_in(&copy, 4, 2), json!(1500.0));
    let summary = copy["tabs"][1]["id"].as_str().expect("tab").to_owned();
    let copy_id = copy["sheet"]["id"].as_str().expect("copy").to_owned();
    let (status, read) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/projects/{work}/sheets/{copy_id}?tab={summary}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["cells"][0]["input"], "=Sheet1!B4*2");
    assert_eq!(value_in(&read, 1, 1), json!(3000.0));

    // A retry of the same import is the same answer, not a second sheet.
    let (status, again) = import(&setup, &uri, "import-http-1", &xlsx).await;
    assert_eq!(status, StatusCode::CREATED, "{again}");
    assert_eq!(again["sheet"]["id"], copy["sheet"]["id"]);

    // What isn't a spreadsheet is refused, and nothing is made of it.
    let broken = format!("/api/v1/projects/{work}/sheets/import?name=Broken&format=xlsx");
    let (status, refused) = import(&setup, &broken, "import-http-2", b"not a zip").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sheets WHERE name = 'Broken'")
        .fetch_one(&setup.pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}
