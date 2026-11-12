//! AWD-54: sheets, a project's spreadsheets worked out on the server. The
//! silent, costly failures: a formula that computes wrong or stops following
//! the cells it uses when rows move or a tab is renamed; one writer's cell
//! overwriting another's they never saw; a row update by key adding a
//! duplicate instead of changing the row; a write half applied; a retry
//! applied twice; and someone without `sheets.edit` changing a sheet.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
};

/// A cell's value in a sheet_read result.
fn value_at(read: &Value, cell: &str) -> Value {
    assert_eq!(read["outcome"], "ok", "{read}");
    read["value"]["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|line| line["cell"] == cell)
        .map(|line| line["value"].clone())
        .unwrap_or(Value::Null)
}

fn formula_at(read: &Value, cell: &str) -> Value {
    assert_eq!(read["outcome"], "ok", "{read}");
    read["value"]["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|line| line["cell"] == cell)
        .map(|line| line["formula"].clone())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn people_and_agents_build_and_edit_a_sheet_without_losing_each_others_cells() {
    let Some(setup) = setup("awd54_sheets").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // A new sheet: its first entry names the starting tab, and the
    // formulas are worked out as it's written.
    let created = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": "Budget",
            "tabs": [{"add": "Costs", "frozenRows": 1, "columnWidths": {"A": 180}}],
            "cells": [
                {"cell": "A1", "input": "Item"},
                {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": "Rent"},
                {"cell": "B2", "input": "1200"},
                {"cell": "A3", "input": "Power"},
                {"cell": "B3", "input": "80.5"},
                {"cell": "A4", "input": "Total"},
                {"cell": "B4", "input": "=SUM(B2:B3)"},
                {"cell": "C2", "input": "'00123"},
                {"cell": "C3", "input": "=B2/0"},
            ],
            "clientRequestId": "budget-1",
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    assert_eq!(created["value"]["created"], true);
    let sheet = created["value"]["id"].as_str().expect("id").to_owned();
    let replay = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": "Budget",
            "tabs": [{"add": "Costs", "frozenRows": 1, "columnWidths": {"A": 180}}],
            "cells": [
                {"cell": "A1", "input": "Item"},
                {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": "Rent"},
                {"cell": "B2", "input": "1200"},
                {"cell": "A3", "input": "Power"},
                {"cell": "B3", "input": "80.5"},
                {"cell": "A4", "input": "Total"},
                {"cell": "B4", "input": "=SUM(B2:B3)"},
                {"cell": "C2", "input": "'00123"},
                {"cell": "C3", "input": "=B2/0"},
            ],
            "clientRequestId": "budget-1",
        }),
    )
    .await;
    assert_eq!(replay["value"]["idempotentReplay"], true, "{replay}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "budget"}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["tab"], "Costs");
    assert_eq!(read["value"]["tabs"][0]["used"], "C4");
    assert_eq!(value_at(&read, "B4"), json!(1280.5));
    assert_eq!(formula_at(&read, "B4"), json!("=SUM(B2:B3)"));
    assert_eq!(value_at(&read, "C2"), json!("00123"));
    assert_eq!(value_at(&read, "C3"), json!("#DIV/0!"));
    let revision = read["value"]["sheet"]["revision"]
        .as_i64()
        .expect("revision");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sheets WHERE project_id = $1::uuid")
            .bind(&work)
            .fetch_one(&setup.pool)
            .await
            .expect("sheets"),
        1,
        "the retry made no second sheet"
    );

    // Two writers: Mara (an agent) changes A2; a change to another cell
    // read at the same revision goes through, but one to A2 is refused.
    let mara_wrote = mara_call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "A2", "input": "Rent (office)"}]}),
    )
    .await;
    assert_eq!(mara_wrote["outcome"], "ok", "{mara_wrote}");
    let other_cell = call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "cells": [{"cell": "B3", "input": "90", "ifRevision": revision}]}),
    )
    .await;
    assert_eq!(other_cell["outcome"], "ok", "{other_cell}");
    let changed = other_cell["value"]["changed"].as_array().expect("changed");
    assert!(
        changed
            .iter()
            .any(|cell| cell["cell"] == "B4" && cell["value"] == json!(1290.0)),
        "the total follows: {other_cell}"
    );
    let same_cell = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": sheet,
            "cells": [
                {"cell": "D1", "input": "Note"},
                {"cell": "A2", "input": "Rent (home)", "ifRevision": revision},
            ],
        }),
    )
    .await;
    assert_eq!(same_cell["outcome"], "conflict", "{same_cell}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": sheet, "range": "A1:D4"}),
    )
    .await;
    assert_eq!(value_at(&read, "A2"), json!("Rent (office)"));
    assert_eq!(
        value_at(&read, "D1"),
        Value::Null,
        "nothing of the refused write landed"
    );
    let by_mara = sqlx::query_scalar::<_, Uuid>(
        "SELECT updated_by_actor_id FROM sheet_cells WHERE sheet_id = $1::uuid AND row_number = 2 AND column_number = 1",
    )
    .bind(&sheet)
    .fetch_one(&setup.pool)
    .await
    .expect("A2");
    assert_eq!(by_mara, mara, "the cell remembers who wrote it");

    // Rows by key: an existing row changes, a new key adds a row, and a new
    // header joins row 1.
    let revision = read["value"]["sheet"]["revision"]
        .as_i64()
        .expect("revision");
    let rows = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "rows": [
                {"keyColumn": "Item", "values": {"Item": "Power", "Cost": 95, "Paid": true}},
                {"keyColumn": "Item", "values": {"Item": "Water", "Cost": 30}},
            ],
        }),
    )
    .await;
    assert_eq!(rows["outcome"], "ok", "{rows}");
    let table = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "asRows": true}),
    )
    .await;
    let headers = table["value"]["headers"].as_array().expect("headers");
    assert!(headers.contains(&json!("Paid")), "{table}");
    let lines = table["value"]["rows"].as_array().expect("rows");
    assert_eq!(lines.len(), 4, "Rent, Power, Total, Water: {table}");
    let power = lines
        .iter()
        .find(|line| line["values"]["Item"] == "Power")
        .expect("Power");
    assert_eq!(power["row"], 3);
    assert_eq!(power["values"]["Cost"], json!(95.0));
    assert_eq!(power["values"]["Paid"], json!(true));
    let water = lines
        .iter()
        .find(|line| line["values"]["Item"] == "Water")
        .expect("Water");
    assert_eq!(water["row"], 5);
    let found = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "asRows": true, "where": {"item": "power"}}),
    )
    .await;
    assert_eq!(found["value"]["matchingRows"], 1, "{found}");

    // Inserting a row moves the total and its formula with it; an edit
    // read before the insert is refused, since its cell may have moved.
    let inserted = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "tabs": [{"tab": "Costs", "insertRowsAt": 3}],
        }),
    )
    .await;
    assert_eq!(inserted["outcome"], "ok", "{inserted}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "range": "A1:B6"}),
    )
    .await;
    assert_eq!(value_at(&read, "A5"), json!("Total"));
    assert_eq!(formula_at(&read, "B5"), json!("=SUM(B2:B4)"));
    let stale = call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "B2", "input": "1", "ifRevision": revision}]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // A second tab that points at the first follows its rename.
    let summary = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "tabs": [{"add": "Summary"}, {"tab": "Costs", "rename": "Spend"}],
            "cells": [{"tab": "Summary", "cell": "A1", "input": "=Spend!B5*2"}],
        }),
    )
    .await;
    assert_eq!(summary["outcome"], "ok", "{summary}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "tab": "Summary"}),
    )
    .await;
    assert_eq!(value_at(&read, "A1"), json!(2590.0), "{read}");
    let renamed = call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": "Budget", "tabs": [{"tab": "Spend", "rename": "Outgoings"}]}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "tab": "Summary"}),
    )
    .await;
    assert_eq!(formula_at(&read, "A1"), json!("=Outgoings!B5*2"), "{read}");

    // All or nothing: a bad cell after a good one writes neither.
    let half = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "cells": [{"tab": "Summary", "cell": "B1", "input": "kept?"}, {"tab": "Nowhere", "cell": "A1", "input": "x"}],
        }),
    )
    .await;
    assert_eq!(half["outcome"], "not_found", "{half}");
    let read = call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget", "tab": "Summary", "range": "B1"}),
    )
    .await;
    assert_eq!(value_at(&read, "B1"), Value::Null);

    // Every change has its revision, by who made it.
    let revisions = sqlx::query_as::<_, (i64, Uuid)>(
        "SELECT revision, actor_id FROM sheet_revisions WHERE sheet_id = $1::uuid ORDER BY revision",
    )
    .bind(&sheet)
    .fetch_all(&setup.pool)
    .await
    .expect("revisions");
    let numbers = revisions
        .iter()
        .map(|(revision, _)| *revision)
        .collect::<Vec<_>>();
    assert_eq!(numbers, (1..=numbers.len() as i64).collect::<Vec<_>>());
    assert!(revisions.iter().any(|(_, actor)| *actor == mara));

    // Open screens hear about every revision.
    let events = sqlx::query_scalar::<_, i64>(
        "SELECT (payload->>'revision')::bigint FROM outbox_events WHERE event_type = 'sheet.updated' AND payload->>'sheetId' = $1 ORDER BY event_cursor",
    )
    .bind(&sheet)
    .fetch_all(&setup.pool)
    .await
    .expect("sheet events");
    assert_eq!(events.last(), numbers.last(), "{events:?}");
    assert!(events.len() >= 2, "{events:?}");

    // An agent that can only view reads the sheet and changes nothing.
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer' WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&work)
    .bind(mara)
    .execute(&setup.pool)
    .await
    .expect("viewer");
    let read = mara_call(
        "sheet_read",
        json!({"projectId": "WORK", "sheet": "Budget"}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["canEdit"], false);
    let refused = mara_call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "A1", "input": "Mine"}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // Archiving hides it from the list; its cells stay.
    let archived = call(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": "Budget", "archived": true}),
    )
    .await;
    assert_eq!(archived["value"]["archived"], true, "{archived}");
    let listed = call("sheet_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["value"]["sheets"], json!([]), "{listed}");
    let listed = call(
        "sheet_list",
        json!({"projectId": "WORK", "includeArchived": true}),
    )
    .await;
    assert_eq!(
        listed["value"]["sheets"][0]["tabs"],
        json!(["Outgoings", "Summary"])
    );
    assert!(listed["value"]["sheets"][0]["cells"].as_i64().unwrap_or(0) > 10);
}
