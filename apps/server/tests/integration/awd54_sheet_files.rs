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

/// A cell's line in a sheet's body, by row and column.
fn cell_in(body: &Value, row: i64, column: i64) -> Value {
    body["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|cell| cell["row"] == row && cell["column"] == column)
        .cloned()
        .unwrap_or(Value::Null)
}

/// C1.14: `10%` keeps showing as 10% and computes as 0.1; formatting a
/// block doesn't make an edit read before it stale; a styled `.xlsx` goes
/// out and comes back with its formats; a revision's formats come back
/// with it.
#[tokio::test]
async fn formats_and_styles_stay_through_writes_files_and_history() {
    let Some(setup) = setup("awd54_sheet_formats").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "LOOK", "Looks").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let made = call(
        "sheet_write",
        json!({
            "projectId": "LOOK",
            "name": "Rates",
            "cells": [
                {"cell": "A1", "input": "Rate"},
                {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": "10%"},
                {"cell": "B2", "input": "=A2*200"},
            ],
            "clientRequestId": "look-0",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    let read_at = made["value"]["revision"].as_i64().expect("revision");
    let read = call(
        "sheet_read",
        json!({"projectId": "LOOK", "sheet": "Rates", "range": "A2:B2"}),
    )
    .await;
    assert_eq!(read["value"]["cells"][0]["value"], json!(0.1), "{read}");
    assert_eq!(read["value"]["cells"][0]["display"], "10%", "{read}");
    assert_eq!(read["value"]["cells"][1]["value"], json!(20.0), "{read}");

    // Formats on a block, empty cells included; nothing typed changes.
    let formatted = call(
        "sheet_write",
        json!({
            "projectId": "LOOK",
            "sheet": "Rates",
            "formats": [
                {"range": "A1:C1", "bold": true, "fill": "#ffeeaa", "align": "center"},
                {"range": "B2:B5", "numberFormat": "$#,##0.00"},
            ],
            "clientRequestId": "look-1",
        }),
    )
    .await;
    assert_eq!(formatted["outcome"], "ok", "{formatted}");
    assert_eq!(formatted["value"]["cellsChanged"], 0, "{formatted}");
    assert_eq!(formatted["value"]["formatsChanged"], 7, "{formatted}");
    let styled_at = formatted["value"]["revision"].as_i64().expect("revision");
    assert_eq!(styled_at, read_at + 1);
    let bad = call(
        "sheet_write",
        json!({
            "projectId": "LOOK",
            "sheet": "Rates",
            "formats": [{"range": "A1", "fill": "yellow"}],
            "clientRequestId": "look-bad",
        }),
    )
    .await;
    assert_eq!(bad["outcome"], "invalid_request", "{bad}");
    assert_eq!(bad["field"], "formats[0].fill", "{bad}");
    // An edit read before the formats still lands.
    let typed = call(
        "sheet_write",
        json!({
            "projectId": "LOOK",
            "sheet": "Rates",
            "cells": [{"cell": "A2", "input": "0.25", "ifRevision": read_at}, {"cell": "B4", "input": "7"}],
            "clientRequestId": "look-2",
        }),
    )
    .await;
    assert_eq!(typed["outcome"], "ok", "{typed}");
    let now = typed["value"]["revision"].as_i64().expect("revision");
    let read = call("sheet_read", json!({"projectId": "LOOK", "sheet": "Rates"})).await;
    let cells = read["value"]["cells"].as_array().expect("cells");
    let line = |address: &str| {
        cells
            .iter()
            .find(|cell| cell["cell"] == address)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(line("A2")["display"], "25%", "{read}");
    assert_eq!(line("B2")["display"], "$50.00", "{read}");
    assert_eq!(line("B2")["numberFormat"], "$#,##0.00", "{read}");
    assert_eq!(line("B4")["display"], "$7.00", "{read}");
    assert_eq!(line("A1")["style"]["bold"], true, "{read}");
    assert_eq!(line("A1")["style"]["fill"], "#FFEEAA", "{read}");
    // Cells with only a look aren't filled cells.
    assert!(line("C1").is_null(), "{read}");
    assert_eq!(read["value"]["sheet"]["cells"], 5, "{read}");

    // Out to Excel and back: formats and styles come too.
    let (status, _, xlsx) = download(
        &setup,
        &setup.browser,
        &format!("/api/v1/projects/{work}/sheets/{sheet}/export?format=xlsx"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let uri = format!("/api/v1/projects/{work}/sheets/import?name=Rates%20copy&format=xlsx");
    let (status, copy) = import(&setup, &uri, "look-import", &xlsx).await;
    assert_eq!(status, StatusCode::CREATED, "{copy}");
    assert_eq!(cell_in(&copy, 2, 1)["display"], "25%", "{copy}");
    assert_eq!(cell_in(&copy, 2, 2)["display"], "$50.00", "{copy}");
    assert_eq!(cell_in(&copy, 2, 2)["value"], json!(50.0), "{copy}");
    assert_eq!(cell_in(&copy, 1, 1)["style"]["bold"], true, "{copy}");
    assert_eq!(cell_in(&copy, 1, 3)["input"], "", "{copy}");
    assert_eq!(cell_in(&copy, 1, 3)["style"]["fill"], "#FFEEAA", "{copy}");

    // History: the formats revision says so and shows them, and the
    // revision before it comes back plain.
    let listed = call("history_list", json!({"thing": sheet})).await;
    assert!(listed.to_string().contains("Formatted 7 cells"), "{listed}");
    let shown = call(
        "history_show",
        json!({"thing": sheet, "version": styled_at}),
    )
    .await;
    assert_eq!(shown["outcome"], "ok", "{shown}");
    assert_eq!(
        shown["value"]["content"]["tabs"][0]["formats"]["A1"]["style"]["bold"], true,
        "{shown}"
    );
    let restored = call(
        "history_restore",
        json!({"thing": sheet, "version": read_at, "expectedVersion": now, "clientRequestId": "look-restore"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let read = call("sheet_read", json!({"projectId": "LOOK", "sheet": "Rates"})).await;
    let cells = read["value"]["cells"].as_array().expect("cells");
    let a1 = cells.iter().find(|cell| cell["cell"] == "A1").expect("A1");
    assert!(a1.get("style").is_none(), "{read}");
    let a2 = cells.iter().find(|cell| cell["cell"] == "A2").expect("A2");
    assert_eq!(a2["display"], "10%", "{read}");
    let b2 = cells.iter().find(|cell| cell["cell"] == "B2").expect("B2");
    assert!(b2.get("display").is_none(), "{read}");
    assert_eq!(b2["value"], json!(20.0), "{read}");
    assert!(!cells.iter().any(|cell| cell["cell"] == "B4"), "{read}");
}

/// C1.16: sorting a block moves its rows and a formula that pointed at a
/// moved row follows it; sheet_search finds what cells show; a saved view
/// keeps its filters and comes back in the list.
#[tokio::test]
async fn sorting_finding_and_saved_views() {
    let Some(setup) = setup("awd54_sheet_sort").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "SORT", "Sorting").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let made = call(
        "sheet_write",
        json!({
            "projectId": "SORT",
            "name": "Team",
            "cells": [
                {"cell": "A1", "input": "Name"}, {"cell": "B1", "input": "Score"},
                {"cell": "A2", "input": "Cy"}, {"cell": "B2", "input": "3"},
                {"cell": "A3", "input": "Ana"}, {"cell": "B3", "input": "7"},
                {"cell": "A4", "input": "Di"}, {"cell": "B4", "input": "1"},
                {"cell": "A5", "input": "Bo"}, {"cell": "B5", "input": "11"},
                {"cell": "D1", "input": "=B3"},
                {"cell": "D2", "input": "=SUM(B2:B5)"},
            ],
            "clientRequestId": "sort-0",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();

    let sorted = call(
        "sheet_write",
        json!({"projectId": "SORT", "sheet": sheet, "sorts": [{"range": "A2:B5", "by": [{"column": "A"}]}]}),
    )
    .await;
    assert_eq!(sorted["outcome"], "ok", "{sorted}");
    let read = call(
        "sheet_read",
        json!({"projectId": "SORT", "sheet": sheet, "range": "A1:D5"}),
    )
    .await;
    let at = |name: &str| {
        read["value"]["cells"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|cell| cell["cell"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    let names: Vec<Value> = ["A2", "A3", "A4", "A5"]
        .map(|name| at(name)["value"].clone())
        .into();
    assert_eq!(
        names,
        vec![json!("Ana"), json!("Bo"), json!("Cy"), json!("Di")],
        "{read}"
    );
    assert_eq!(at("D1")["formula"], "=B2", "{read}");
    assert_eq!(at("D1")["value"], json!(7.0), "{read}");
    assert_eq!(at("D2")["value"], json!(22.0), "{read}");

    let found = call("sheet_search", json!({"sheetId": sheet, "query": "an"})).await;
    assert_eq!(found["outcome"], "ok", "{found}");
    assert_eq!(found["value"]["matches"][0]["cell"], "A2", "{found}");
    assert_eq!(
        found["value"]["matches"].as_array().map(Vec::len),
        Some(1),
        "{found}"
    );
    let formulas = call(
        "sheet_search",
        json!({"sheetId": sheet, "query": "sum(", "inFormulas": true}),
    )
    .await;
    assert_eq!(formulas["value"]["matches"][0]["cell"], "D2", "{formulas}");

    let view = call(
        "sheet_view_create",
        json!({"sheetId": sheet, "name": "High scores", "shared": true,
               "filters": [{"column": "B", "op": "atLeast", "value": 5}],
               "sort": [{"column": "B", "descending": true}],
               "hiddenColumns": ["D"], "clientRequestId": "view-0"}),
    )
    .await;
    assert_eq!(view["outcome"], "ok", "{view}");
    let bad = call(
        "sheet_view_create",
        json!({"sheetId": sheet, "name": "Bad", "filters": [{"column": "B2", "op": "equals"}],
               "clientRequestId": "view-bad"}),
    )
    .await;
    assert_ne!(bad["outcome"], "ok", "{bad}");
    let listed = call("sheet_view_list", json!({"sheetId": sheet})).await;
    assert_eq!(
        listed["value"]["views"][0]["name"], "High scores",
        "{listed}"
    );
    assert_eq!(listed["value"]["views"][0]["shared"], true, "{listed}");
    assert_eq!(
        listed["value"]["views"][0]["filters"][0]["op"], "atLeast",
        "{listed}"
    );
    let id = view["value"]["id"].clone();
    let version = view["value"]["version"].clone();
    let renamed = call(
        "sheet_view_update",
        json!({"viewId": id, "expectedVersion": version, "name": "Top"}),
    )
    .await;
    assert_eq!(renamed["value"]["name"], "Top", "{renamed}");
    let stale = call(
        "sheet_view_update",
        json!({"viewId": id, "expectedVersion": version, "name": "Late"}),
    )
    .await;
    assert_ne!(stale["outcome"], "ok", "{stale}");
    let gone = call("sheet_view_delete", json!({"viewId": id})).await;
    assert_eq!(gone["outcome"], "ok", "{gone}");
}

/// C1.17: a "less than 0 is red" rule colours the right cells after an
/// edit, readers see it, a bad rule is refused by field, and a restore
/// brings the rules back.
#[tokio::test]
async fn a_less_than_zero_rule_colours_the_right_cells() {
    let Some(setup) = setup("awd54_sheet_rules").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "RULE", "Rules").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_write",
        json!({
            "projectId": "RULE",
            "name": "Money",
            "cells": [
                {"cell": "A1", "input": "Net"},
                {"cell": "A2", "input": "5"},
                {"cell": "A3", "input": "-2"},
                {"cell": "A4", "input": "=A2-10"},
            ],
            "clientRequestId": "rule-0",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    let before_rules = made["value"]["revision"].as_i64().expect("revision");

    let bad = call(
        "sheet_write",
        json!({"projectId": "RULE", "sheet": sheet, "conditionalFormats": [{"rules": [
            {"range": "A2:A9", "when": "lessThan", "value": 0, "style": {"color": "red"}}
        ]}]}),
    )
    .await;
    assert_eq!(bad["outcome"], "invalid_request", "{bad}");
    assert!(
        bad.to_string().contains("conditionalFormats[0].rules[0]"),
        "{bad}"
    );

    let ruled = call(
        "sheet_write",
        json!({"projectId": "RULE", "sheet": sheet, "conditionalFormats": [{"rules": [
            {"range": "a2:a9", "when": "lessThan", "value": 0, "style": {"color": "#C62828"}}
        ]}]}),
    )
    .await;
    assert_eq!(ruled["outcome"], "ok", "{ruled}");
    let red = |read: &Value, name: &str| {
        read["value"]["cells"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|cell| cell["cell"] == name)
            .and_then(|cell| {
                cell["conditionalStyle"]["color"]
                    .as_str()
                    .map(str::to_owned)
            })
    };
    let read = call("sheet_read", json!({"projectId": "RULE", "sheet": sheet})).await;
    assert_eq!(red(&read, "A2"), None, "{read}");
    assert_eq!(red(&read, "A3").as_deref(), Some("#C62828"), "{read}");
    assert_eq!(red(&read, "A4").as_deref(), Some("#C62828"), "{read}");
    assert_eq!(
        read["value"]["tabs"][0]["conditionalFormats"][0]["range"], "A2:A9",
        "{read}"
    );

    // An edit: A2 goes to 20, so A4 is 10 and no longer red; A2 turns red at -1.
    let edited = call(
        "sheet_write",
        json!({"projectId": "RULE", "sheet": sheet, "cells": [{"cell": "A2", "input": "20"}, {"cell": "A3", "input": "-1"}]}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let read = call("sheet_read", json!({"projectId": "RULE", "sheet": sheet})).await;
    assert_eq!(red(&read, "A2"), None, "{read}");
    assert_eq!(red(&read, "A3").as_deref(), Some("#C62828"), "{read}");
    assert_eq!(red(&read, "A4"), None, "{read}");

    // A row inserted above moves the rule with its cells.
    let moved = call(
        "sheet_write",
        json!({"projectId": "RULE", "sheet": sheet, "tabs": [{"tab": "Sheet1", "insertRowsAt": 1, "count": 1}]}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let read = call("sheet_read", json!({"projectId": "RULE", "sheet": sheet})).await;
    assert_eq!(
        read["value"]["tabs"][0]["conditionalFormats"][0]["range"], "A3:A10",
        "{read}"
    );
    assert_eq!(red(&read, "A4").as_deref(), Some("#C62828"), "{read}");

    // Back to before the rule: no rules, nothing red.
    let current = call("sheet_read", json!({"projectId": "RULE", "sheet": sheet})).await;
    let revision = current["value"]["sheet"]["revision"]
        .as_i64()
        .expect("revision");
    let restored = call(
        "history_restore",
        json!({"thing": sheet, "version": before_rules, "expectedVersion": revision, "clientRequestId": "rule-restore"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let read = call("sheet_read", json!({"projectId": "RULE", "sheet": sheet})).await;
    assert!(
        read["value"]["tabs"][0].get("conditionalFormats").is_none(),
        "{read}"
    );
    assert_eq!(red(&read, "A3"), None, "{read}");
}
