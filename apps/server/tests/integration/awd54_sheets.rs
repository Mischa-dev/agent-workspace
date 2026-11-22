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
        "sheet_create",
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
        "sheet_create",
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
    assert_eq!(replay["replayed"], true, "{replay}");
    let read = call("sheet_get", json!({"projectId": "WORK", "sheet": "budget"})).await;
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
        "sheet_update",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "A2", "input": "Rent (office)"}]}),
    )
    .await;
    assert_eq!(mara_wrote["outcome"], "ok", "{mara_wrote}");
    let other_cell = call(
        "sheet_update",
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
        "sheet_update",
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
        "sheet_get",
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
        "sheet_update",
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
        "sheet_get",
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
        "sheet_get",
        json!({"projectId": "WORK", "sheet": "Budget", "asRows": true, "where": {"item": "power"}}),
    )
    .await;
    assert_eq!(found["value"]["matchingRows"], 1, "{found}");

    // Inserting a row moves the total and its formula with it; an edit
    // read before the insert is refused, since its cell may have moved.
    let inserted = call(
        "sheet_update",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "tabs": [{"tab": "Costs", "insertRowsAt": 3}],
        }),
    )
    .await;
    assert_eq!(inserted["outcome"], "ok", "{inserted}");
    let read = call(
        "sheet_get",
        json!({"projectId": "WORK", "sheet": "Budget", "range": "A1:B6"}),
    )
    .await;
    assert_eq!(value_at(&read, "A5"), json!("Total"));
    assert_eq!(formula_at(&read, "B5"), json!("=SUM(B2:B4)"));
    let stale = call(
        "sheet_update",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "B2", "input": "1", "ifRevision": revision}]}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // A second tab that points at the first follows its rename.
    let summary = call(
        "sheet_update",
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
        "sheet_get",
        json!({"projectId": "WORK", "sheet": "Budget", "tab": "Summary"}),
    )
    .await;
    assert_eq!(value_at(&read, "A1"), json!(2590.0), "{read}");
    let renamed = call(
        "sheet_update",
        json!({"projectId": "WORK", "sheet": "Budget", "tabs": [{"tab": "Spend", "rename": "Outgoings"}]}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let read = call(
        "sheet_get",
        json!({"projectId": "WORK", "sheet": "Budget", "tab": "Summary"}),
    )
    .await;
    assert_eq!(formula_at(&read, "A1"), json!("=Outgoings!B5*2"), "{read}");

    // All or nothing: a bad cell after a good one writes neither.
    let half = call(
        "sheet_update",
        json!({
            "projectId": "WORK",
            "sheet": "Budget",
            "cells": [{"tab": "Summary", "cell": "B1", "input": "kept?"}, {"tab": "Nowhere", "cell": "A1", "input": "x"}],
        }),
    )
    .await;
    assert_eq!(half["outcome"], "not_found", "{half}");
    let read = call(
        "sheet_get",
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
        &format!(
"SELECT (payload->>'revision')::bigint FROM {n} outbox WHERE event_type = 'sheet.updated' AND payload->>'sheetId' = $1 ORDER BY event_cursor",
 n = crate::support::NOTICES
),
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
    let read = mara_call("sheet_get", json!({"projectId": "WORK", "sheet": "Budget"})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["canEdit"], false);
    let refused = mara_call(
        "sheet_update",
        json!({"projectId": "WORK", "sheet": "Budget", "cells": [{"cell": "A1", "input": "Mine"}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // Archiving hides it from the list; its cells stay.
    let archived = call(
        "sheet_update",
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

#[tokio::test]
async fn clears_inserts_row_keys_renames_and_retries_hold_up() {
    let Some(setup) = setup("awd54_sheet_edges").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "EDGE", "Edges").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let revision_of = |result: &Value| result["value"]["revision"].as_i64().expect("revision");
    let made = call(
        "sheet_create",
        json!({
            "projectId": "EDGE",
            "name": "Log",
            "cells": [
                {"cell": "A1", "input": "ID"},
                {"cell": "B1", "input": "Note"},
                {"cell": "A2", "input": "1"},
                {"cell": "B2", "input": "first"},
                {"cell": "C2", "input": "keep"},
            ],
            "clientRequestId": "edge-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let read_at = revision_of(&made);

    // A clear is a change: an edit read before it is refused.
    let cleared = call(
        "sheet_update",
        json!({"projectId": "EDGE", "sheet": "Log", "cells": [{"cell": "C2", "input": "", "ifRevision": read_at}], "clientRequestId": "edge-2"}),
    )
    .await;
    assert_eq!(cleared["outcome"], "ok", "{cleared}");
    let stale = call(
        "sheet_update",
        json!({"projectId": "EDGE", "sheet": "Log", "cells": [{"cell": "C2", "input": "back", "ifRevision": read_at}], "clientRequestId": "edge-3"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // A row inserted far below in the same write doesn't hide a newer B2.
    let newer = call(
        "sheet_update",
        json!({"projectId": "EDGE", "sheet": "Log", "cells": [{"cell": "B2", "input": "second"}], "clientRequestId": "edge-4"}),
    )
    .await;
    assert_eq!(newer["outcome"], "ok", "{newer}");
    let hidden = call(
        "sheet_update",
        json!({
            "projectId": "EDGE",
            "sheet": "Log",
            "tabs": [{"tab": "Sheet1", "insertRowsAt": 100, "count": 1}],
            "cells": [{"cell": "B2", "input": "stale", "ifRevision": read_at}],
            "clientRequestId": "edge-5",
        }),
    )
    .await;
    assert_eq!(hidden["outcome"], "conflict", "{hidden}");

    // Rows by key see what this write set first, and 1.0 is the key 1.
    let keyed = call(
        "sheet_update",
        json!({
            "projectId": "EDGE",
            "sheet": "Log",
            "cells": [{"cell": "A3", "input": "2"}, {"cell": "B3", "input": "by cell"}],
            "rows": [
                {"keyColumn": "ID", "values": {"ID": 1.0, "Note": "by key"}},
                {"keyColumn": "ID", "values": {"ID": "3", "Note": "new"}},
                {"keyColumn": "ID", "values": {"ID": "3", "Note": "newer"}},
            ],
            "clientRequestId": "edge-6",
        }),
    )
    .await;
    assert_eq!(keyed["outcome"], "ok", "{keyed}");
    let read = call(
        "sheet_get",
        json!({"projectId": "EDGE", "sheet": "Log", "range": "A1:B6"}),
    )
    .await;
    assert_eq!(value_at(&read, "B2"), json!("by key"));
    assert_eq!(value_at(&read, "B3"), json!("by cell"));
    assert_eq!(value_at(&read, "A4"), json!(3.0));
    assert_eq!(value_at(&read, "B4"), json!("newer"));
    assert_eq!(value_at(&read, "A5"), Value::Null);

    // A new tab keeps its widths, and chained renames land.
    let tabs = call(
        "sheet_update",
        json!({
            "projectId": "EDGE",
            "sheet": "Log",
            "tabs": [{"add": "B", "columnWidths": {"A": 180}}, {"add": "C"}],
            "clientRequestId": "edge-7",
        }),
    )
    .await;
    assert_eq!(tabs["outcome"], "ok", "{tabs}");
    let renamed = call(
        "sheet_update",
        json!({
            "projectId": "EDGE",
            "sheet": "Log",
            "tabs": [{"tab": "C", "rename": "D"}, {"tab": "B", "rename": "C"}, {"tab": "Sheet1", "rename": "B"}],
            "clientRequestId": "edge-8",
        }),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let names = sqlx::query_as::<_, (String, Value)>(
        "SELECT tab.name, tab.column_widths FROM sheet_tabs tab JOIN sheets sheet ON sheet.id = tab.sheet_id \
         WHERE sheet.name = 'Log' ORDER BY tab.position",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("tabs");
    assert_eq!(
        names
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["B", "C", "D"]
    );
    assert_eq!(names[1].1, json!({"1": 180}));

    // Tab entries apply in order: a move, then a removal.
    let reordered = call(
        "sheet_update",
        json!({
            "projectId": "EDGE",
            "sheet": "Log",
            "tabs": [{"tab": "D", "position": 2}, {"tab": "B", "remove": true}],
            "clientRequestId": "edge-8b",
        }),
    )
    .await;
    assert_eq!(reordered["outcome"], "ok", "{reordered}");
    let order = sqlx::query_scalar::<_, String>(
        "SELECT tab.name FROM sheet_tabs tab JOIN sheets sheet ON sheet.id = tab.sheet_id \
         WHERE sheet.name = 'Log' ORDER BY tab.position",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("order");
    assert_eq!(order, ["D", "C"]);

    // A retry of a rename by the old name gets its first answer back.
    let rename = json!({"projectId": "EDGE", "sheet": "Log", "name": "Journal", "clientRequestId": "edge-9"});
    let first = call("sheet_update", rename.clone()).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    let retry = call("sheet_update", rename).await;
    assert_eq!(retry["replayed"], true, "{retry}");
    assert_eq!(retry["value"]["name"], "Journal");
}

/// C1.12: a change to a sheet's cells takes no lock on the project row, so a
/// task update (which holds that row) and a long sheet write never wait on
/// each other. Making a sheet still takes it, for its per-project checks.
#[tokio::test]
async fn sheet_cell_writes_do_not_wait_for_the_project_lock() {
    let Some(setup) = setup("c1_12_sheet_lock").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let created = tool(
        app,
        &codex,
        &session,
        "sheet_create",
        json!({"projectId": "WORK", "name": "Plan", "cells": [{"cell": "A1", "input": "1"}], "clientRequestId": "plan-1"}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");

    // Another writer holds the project row, as a task update does
    // (`lock_membership`'s lock on it).
    let mut holder = setup.pool.begin().await.expect("holder");
    sqlx::query("SELECT id FROM projects WHERE id = $1::uuid FOR NO KEY UPDATE")
        .bind(&work)
        .execute(&mut *holder)
        .await
        .expect("hold the project row");
    let cells = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tool(
            app,
            &codex,
            &session,
            "sheet_update",
            json!({"projectId": "WORK", "sheet": "Plan", "cells": [{"cell": "A2", "input": "=A1*2"}], "clientRequestId": "plan-2"}),
        ),
    )
    .await
    .expect("a cell write never waits for the project lock");
    assert_eq!(cells["outcome"], "ok", "{cells}");
    holder.rollback().await.expect("release");
}

/// C1.13: any revision since snapshots began can be shown and put back,
/// across row moves and tab changes, as a new revision whose values are
/// worked out again.
#[tokio::test]
async fn any_revision_comes_back_as_a_new_one() {
    let Some(setup) = setup("awd54_sheet_history").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "HIST", "History").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_create",
        json!({
            "projectId": "HIST",
            "name": "Counter",
            "cells": [{"cell": "A1", "input": "0"}, {"cell": "B1", "input": "=A1*2"}],
            "clientRequestId": "hist-0",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    // Forty revisions, more than one snapshot apart, with a row inserted
    // above the counter and a tab added along the way.
    let mut revisions = Vec::new();
    for step in 1..=40 {
        let mut arguments = json!({
            "projectId": "HIST",
            "sheet": "Counter",
            "cells": [{"cell": if step > 15 { "A2" } else { "A1" }, "input": step.to_string()}],
            "clientRequestId": format!("hist-{step}"),
        });
        if step == 15 {
            arguments["tabs"] = json!([{"tab": "Sheet1", "insertRowsAt": 1, "count": 1}]);
            arguments["cells"] = json!([{"cell": "A2", "input": "15"}]);
        }
        if step == 30 {
            arguments["tabs"] = json!([{"add": "Notes"}]);
        }
        let written = call("sheet_update", arguments).await;
        assert_eq!(written["outcome"], "ok", "{step}: {written}");
        revisions.push(written["value"]["revision"].as_i64().expect("revision"));
    }
    let now = *revisions.last().expect("revisions");
    // C1.34: snapshots past 30 days move to the file store, all but the
    // sheet's latest, and the revisions built on them still come back.
    sqlx::query(
        "UPDATE sheet_snapshots SET created_at = now() - interval '40 days' WHERE sheet_id::text = $1",
    )
    .bind(&sheet)
    .execute(&setup.pool)
    .await
    .expect("age the snapshots");
    let state = crate::support::app::state_on(&setup.pool);
    let moved = crate::support::as_server_job(
        agent_workspace_server::sheets::history::compact_snapshots(&state),
    )
    .await
    .expect("compact");
    assert!(moved > 0, "old snapshots move out");
    let inline = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sheet_snapshots WHERE sheet_id::text = $1 AND payload_key IS NULL",
    )
    .bind(&sheet)
    .fetch_one(&setup.pool)
    .await
    .expect("left inline");
    assert_eq!(inline, 1, "the latest snapshot stays");
    // Step 12: A1 held 12, before the row moved it and before the tab.
    let twelve = revisions[11];
    let shown = call("history_show", json!({"thing": sheet, "version": twelve})).await;
    assert_eq!(shown["outcome"], "ok", "{shown}");
    let tabs = &shown["value"]["content"]["tabs"];
    assert_eq!(tabs.as_array().map(Vec::len), Some(1), "{shown}");
    assert_eq!(tabs[0]["cells"]["A1"], "12", "{shown}");
    assert_eq!(tabs[0]["cells"]["B1"], "=A1*2", "{shown}");

    let stale = call(
        "history_restore",
        json!({"thing": sheet, "version": twelve, "expectedVersion": now - 1, "clientRequestId": "hist-stale"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let restored = call(
        "history_restore",
        json!({"thing": sheet, "version": twelve, "expectedVersion": now, "clientRequestId": "hist-restore"}),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["version"], now + 1, "{restored}");
    let read = call(
        "sheet_get",
        json!({"projectId": "HIST", "sheet": "Counter"}),
    )
    .await;
    assert_eq!(value_at(&read, "A1"), json!(12.0), "{read}");
    assert_eq!(
        value_at(&read, "B1"),
        json!(24.0),
        "values are worked out again"
    );
    assert_eq!(value_at(&read, "A2"), Value::Null, "{read}");
    let tabs = call(
        "sheet_get",
        json!({"projectId": "HIST", "sheet": "Counter", "tab": "Notes"}),
    )
    .await;
    assert_ne!(tabs["outcome"], "ok", "the tab added later is gone: {tabs}");

    // The restore is a revision of its own, so it can be undone the same way.
    let back = call(
        "history_restore",
        json!({"thing": sheet, "version": now, "expectedVersion": now + 1, "clientRequestId": "hist-back"}),
    )
    .await;
    assert_eq!(back["outcome"], "ok", "{back}");
    let read = call(
        "sheet_get",
        json!({"projectId": "HIST", "sheet": "Counter"}),
    )
    .await;
    assert_eq!(value_at(&read, "A2"), json!(40.0), "{read}");
    let listed = call("history_list", json!({"thing": sheet, "limit": 3})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let _ = Uuid::nil();
}

/// C1.20: a sheet as a source. A named range follows the rows inserted
/// above it, agents read it by name, and a dashboard tile over it charts its
/// rows (same labels added up) and changes when a cell does. A range whose
/// cells are all deleted goes, and the tile says so instead of failing.
#[tokio::test]
async fn a_named_range_moves_with_its_rows_and_feeds_a_dashboard_tile() {
    let Some(setup) = setup("c1_sheet_source").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "SRC", "Source").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_create",
        json!({
            "projectId": "SRC",
            "name": "Spend",
            "tabs": [{"add": "Costs"}],
            "cells": [
                {"cell": "A1", "input": "Item"},
                {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": "Rent"},
                {"cell": "B2", "input": "1200"},
                {"cell": "A3", "input": "Power"},
                {"cell": "B3", "input": "80"},
                {"cell": "A4", "input": "Rent"},
                {"cell": "B4", "input": "100"},
            ],
            "clientRequestId": "spend-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();

    for bad in ["Q1", "two words"] {
        let refused = call(
            "sheet_range_set",
            json!({"sheetId": sheet, "name": bad, "range": "A1:B4", "clientRequestId": format!("bad-{bad}")}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{bad}: {refused}");
    }
    let named = call(
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Totals", "range": "Costs!A1:B4", "clientRequestId": "name-1"}),
    )
    .await;
    assert_eq!(named["outcome"], "ok", "{named}");
    assert_eq!(
        named["value"]["ranges"],
        json!([{"name": "Totals", "tabId": named["value"]["ranges"][0]["tabId"], "tab": "Costs", "range": "A1:B4"}]),
        "{named}"
    );

    let made = call(
        "dashboard_create",
        json!({
            "spaceId": project,
            "name": "Money",
            "shared": false,
            "tiles": [{"sheet": sheet, "range": "totals", "style": "donut"}],
            "clientRequestId": "money-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let dashboard = made["value"]["id"].as_str().expect("dashboard").to_owned();
    let figures = || async {
        let read = call("dashboard_get", json!({"dashboard": dashboard})).await;
        assert_eq!(read["outcome"], "ok", "{read}");
        read["value"]["tiles"][0].clone()
    };
    let tile = figures().await;
    assert_eq!(tile["kind"], "sheet", "{tile}");
    let shown = &tile["figures"];
    assert_eq!(shown["collectionName"], "Spend", "{tile}");
    assert_eq!(shown["viewName"], "Totals", "{tile}");
    assert_eq!(shown["style"], "donut", "{tile}");
    assert_eq!(shown["measure"], "sum of Cost", "{tile}");
    assert_eq!(shown["value"], json!(1380.0), "{tile}");
    let groups = |shown: &Value| {
        shown["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|group| (group["name"].clone(), group["value"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        groups(shown),
        [
            (json!("Rent"), json!(1300.0)),
            (json!("Power"), json!(80.0))
        ],
        "{tile}"
    );

    // A row above moves the range down; agents read it by name.
    let moved = call(
        "sheet_update",
        json!({
            "projectId": "SRC",
            "sheet": sheet,
            "tabs": [{"tab": "Costs", "insertRowsAt": 1, "count": 1}],
            "cells": [{"tab": "Costs", "cell": "A1", "input": "Spend this month"}],
            "clientRequestId": "spend-2",
        }),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let read = call(
        "sheet_get",
        json!({"projectId": "SRC", "sheet": sheet, "range": "TOTALS"}),
    )
    .await;
    assert_eq!(read["value"]["tab"], "Costs", "{read}");
    assert_eq!(read["value"]["range"], "A2:B5", "{read}");
    assert_eq!(value_at(&read, "B3"), json!(1200.0), "{read}");
    assert_eq!(read["value"]["namedRanges"][0]["range"], "A2:B5", "{read}");

    // A cell changing changes the tile.
    let changed = call(
        "sheet_update",
        json!({
            "projectId": "SRC",
            "sheet": sheet,
            "cells": [{"tab": "Costs", "cell": "B4", "input": "90"}],
            "clientRequestId": "spend-3",
        }),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let tile = figures().await;
    assert_eq!(tile["figures"]["value"], json!(1390.0), "{tile}");
    assert_eq!(
        groups(&tile["figures"]),
        [
            (json!("Rent"), json!(1300.0)),
            (json!("Power"), json!(90.0))
        ],
        "{tile}"
    );

    // Deleting every row of it removes the name; the tile says so.
    let gone = call(
        "sheet_update",
        json!({
            "projectId": "SRC",
            "sheet": sheet,
            "tabs": [{"tab": "Costs", "deleteRowsAt": 2, "count": 4}],
            "clientRequestId": "spend-4",
        }),
    )
    .await;
    assert_eq!(gone["outcome"], "ok", "{gone}");
    let tile = figures().await;
    assert!(tile["figures"].is_null(), "{tile}");
    assert!(
        tile["unseen"]
            .as_str()
            .is_some_and(|text| text.contains("range is gone")),
        "{tile}"
    );
    let removed = call(
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Totals", "remove": true, "clientRequestId": "name-2"}),
    )
    .await;
    assert_eq!(removed["outcome"], "not_found", "{removed}");
}

/// C1.15: a strict rule refuses what breaks it, naming the rule, whether a
/// cell or a row by header is written; a lenient one keeps the input; the
/// rules move with inserted rows.
#[tokio::test]
async fn a_strict_rule_refuses_what_breaks_it_and_names_the_rule() {
    let Some(setup) = setup("c1_sheet_validation").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "VAL", "Validation").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_create",
        json!({
            "projectId": "VAL",
            "name": "Tracker",
            "tabs": [{"add": "Work"}],
            "cells": [
                {"cell": "A1", "input": "Task"},
                {"cell": "B1", "input": "Done"},
                {"cell": "C1", "input": "Effort"},
                {"cell": "A2", "input": "Paint"},
            ],
            "validations": [{"tab": "Work", "rules": [
                {"range": "b2:b50", "allow": "list", "choices": ["Yes", "No"], "hint": "Pick one"},
                {"range": "C2:C50", "allow": "number", "min": 1, "max": 5, "whole": true, "strict": false},
                {"range": "D2:D50", "allow": "number", "max": 5},
            ]}],
            "clientRequestId": "tracker-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();

    let maybe = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "cells": [{"tab": "Work", "cell": "B2", "input": "Maybe"}], "clientRequestId": "maybe"}),
    )
    .await;
    assert_eq!(maybe["outcome"], "invalid_request", "{maybe}");
    let said = maybe.to_string();
    assert!(
        said.contains("B2:B50") && said.contains("Yes, No") && said.contains("Pick one"),
        "the rule is named: {said}"
    );
    let by_row = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "rows": [{"tab": "Work", "keyColumn": "Task", "values": {"Task": "Paint", "Done": "Later"}}],
               "clientRequestId": "later"}),
    )
    .await;
    assert_eq!(by_row["outcome"], "invalid_request", "{by_row}");

    let fine = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "cells": [
            {"tab": "Work", "cell": "B2", "input": "yes"},
            {"tab": "Work", "cell": "C2", "input": "9"},
        ], "clientRequestId": "fine"}),
    )
    .await;
    assert_eq!(
        fine["outcome"], "ok",
        "lenient rules keep the input: {fine}"
    );

    let inserted = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "tabs": [{"tab": "Work", "insertRowsAt": 1, "count": 2}], "clientRequestId": "insert"}),
    )
    .await;
    assert_eq!(inserted["outcome"], "ok", "{inserted}");
    let read = call(
        "sheet_get",
        json!({"projectId": "VAL", "sheet": sheet, "tab": "Work"}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let tab = read["value"]["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["name"] == "Work")
        .cloned()
        .unwrap_or_default();
    assert_eq!(tab["validations"][0]["range"], "B4:B52", "{tab}");
    assert_eq!(tab["validations"][1]["range"], "C4:C52", "{tab}");
    let moved = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "cells": [{"tab": "Work", "cell": "B4", "input": "Maybe"}], "clientRequestId": "maybe-2"}),
    )
    .await;
    assert_eq!(
        moved["outcome"], "invalid_request",
        "the rule moved: {moved}"
    );
    // A number the engine keeps in its own form ("6.0" as "6") is still
    // checked.
    let written_otherwise = call(
        "sheet_update",
        json!({"projectId": "VAL", "sheet": sheet, "cells": [{"tab": "Work", "cell": "D4", "input": "6.0"}], "clientRequestId": "six"}),
    )
    .await;
    assert_eq!(
        written_otherwise["outcome"], "invalid_request",
        "{written_otherwise}"
    );

    // Rules set in the same write as an insert name the cells as they are
    // after it: they don't move with it.
    let both = call(
        "sheet_write",
        json!({"projectId": "VAL", "sheet": sheet,
               "tabs": [{"tab": "Work", "insertRowsAt": 1, "count": 1}],
               "validations": [{"tab": "Work", "rules": [{"range": "A2:A10", "allow": "number"}]}],
               "clientRequestId": "insert-and-rules"}),
    )
    .await;
    assert_eq!(both["outcome"], "ok", "{both}");
    let read = call(
        "sheet_read",
        json!({"projectId": "VAL", "sheet": sheet, "tab": "Work"}),
    )
    .await;
    let tab = read["value"]["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["name"] == "Work")
        .cloned()
        .unwrap_or_default();
    assert_eq!(tab["validations"][0]["range"], "A2:A10", "{tab}");
}

/// C1.18: a chart over a named range follows its cells: it reads them as
/// they are, keeps to the range as rows go in above it, and draws an SVG
/// for agents; one whose range is removed says so instead of guessing.
#[tokio::test]
async fn a_chart_over_a_named_range_follows_its_cells() {
    let Some(setup) = setup("c1_sheet_charts").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "CHART", "Charts").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_create",
        json!({
            "projectId": "CHART",
            "name": "Costs",
            "cells": [
                {"cell": "A1", "input": "Item"}, {"cell": "B1", "input": "Cost"},
                {"cell": "A2", "input": "Rent"}, {"cell": "B2", "input": "1200"},
                {"cell": "A3", "input": "Food"}, {"cell": "B3", "input": "300"},
            ],
            "clientRequestId": "costs-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    let named = call(
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Spend", "range": "A1:B3", "clientRequestId": "spend"}),
    )
    .await;
    assert_eq!(named["outcome"], "ok", "{named}");
    let refused = call(
        "sheet_chart_set",
        json!({"sheetId": sheet, "title": "Nothing", "source": "Nowhere", "clientRequestId": "bad-chart"}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let charted = call(
        "sheet_chart_set",
        json!({"sheetId": sheet, "title": "Spend", "style": "bar", "source": "Spend", "clientRequestId": "chart-1"}),
    )
    .await;
    assert_eq!(charted["outcome"], "ok", "{charted}");
    let chart = charted["value"]["chartId"]
        .as_str()
        .expect("chart")
        .to_owned();
    let read = |call_name: &'static str| {
        let chart = chart.clone();
        let sheet = sheet.clone();
        let call = &call;
        async move { call(call_name, json!({"sheetId": sheet, "chartId": chart})).await }
    };
    let first = read("sheet_chart_get").await;
    assert_eq!(first["outcome"], "ok", "{first}");
    assert_eq!(
        first["value"]["points"],
        json!([{"label": "Rent", "value": 1200.0}, {"label": "Food", "value": 300.0}]),
        "{first}"
    );
    assert!(
        first["value"]["svg"]
            .as_str()
            .is_some_and(|svg| svg.starts_with("<svg")),
        "{first}"
    );

    // A cell changes, and rows go in above: the chart reads the range where it is.
    let changed = call(
        "sheet_update",
        json!({"projectId": "CHART", "sheet": sheet, "cells": [{"cell": "B3", "input": "450"}],
               "clientRequestId": "costs-2"}),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    let moved = call(
        "sheet_update",
        json!({"projectId": "CHART", "sheet": sheet,
               "tabs": [{"tab": "Sheet1", "insertRowsAt": 1, "count": 2}], "clientRequestId": "costs-3"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let after = read("sheet_chart_get").await;
    assert_eq!(
        after["value"]["points"],
        json!([{"label": "Rent", "value": 1200.0}, {"label": "Food", "value": 450.0}]),
        "{after}"
    );

    // The range goes: the chart says what it draws isn't there.
    let removed = call(
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Spend", "remove": true, "clientRequestId": "unspend"}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    let gone = read("sheet_chart_get").await;
    assert_eq!(gone["value"]["found"], false, "{gone}");
    let dropped = call(
        "sheet_chart_set",
        json!({"sheetId": sheet, "chartId": chart, "remove": true, "clientRequestId": "drop-chart"}),
    )
    .await;
    assert_eq!(dropped["outcome"], "ok", "{dropped}");
    assert_eq!(dropped["value"]["charts"], json!([]), "{dropped}");
}

/// C1.10: every sheet write is a change, listed with the cells it touched
/// and undone in a step: cells changed since are left and named, the undo
/// can itself be undone, and a write that moved rows is outlined but put
/// back by a restore instead.
#[tokio::test]
async fn a_sheet_write_is_undone_in_a_step() {
    let Some(setup) = setup("c1_10_sheet_undo").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "UNDO", "Undo").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let made = call(
        "sheet_create",
        json!({"projectId": "UNDO", "name": "Budget",
               "cells": [{"cell": "A1", "input": "1"}, {"cell": "B1", "input": "=A1*2"}],
               "clientRequestId": "undo-0"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    let write = |cells: Value, key: &'static str| {
        call(
            "sheet_update",
            json!({"projectId": "UNDO", "sheet": "Budget", "cells": cells, "clientRequestId": key}),
        )
    };
    let three = write(
        json!([{"cell": "A1", "input": "5"}, {"cell": "A2", "input": "7"}, {"cell": "C1", "input": "3"}]),
        "undo-1",
    )
    .await;
    assert_eq!(three["outcome"], "ok", "{three}");
    let later = write(json!([{"cell": "C1", "input": "4"}]), "undo-2").await;
    assert_eq!(later["outcome"], "ok", "{later}");

    let list = |key: &'static str| {
        let _ = key;
        call(
            "change_list",
            json!({"thingKind": "sheet", "thingId": sheet}),
        )
    };
    let changes = list("list-1").await;
    assert_eq!(changes["outcome"], "ok", "{changes}");
    let listed = changes["value"]["changes"].as_array().expect("changes");
    assert_eq!(listed.len(), 3, "{changes}");
    let middle = &listed[1];
    assert_eq!(
        middle["touched"]["cells"].as_array().map(Vec::len),
        Some(3),
        "{changes}"
    );
    assert_eq!(middle["undoable"], true, "{changes}");

    let undo = call(
        "change_undo",
        json!({"changeSetId": middle["id"], "clientRequestId": "undo-undo"}),
    )
    .await;
    assert_eq!(undo["outcome"], "ok", "{undo}");
    assert_eq!(undo["value"]["skipped"], json!(["Sheet1!C1"]), "{undo}");
    let read = call("sheet_get", json!({"projectId": "UNDO", "sheet": "Budget"})).await;
    assert_eq!(value_at(&read, "A1"), json!(1.0), "{read}");
    assert_eq!(value_at(&read, "B1"), json!(2.0), "{read}");
    assert_eq!(value_at(&read, "A2"), Value::Null, "{read}");
    assert_eq!(value_at(&read, "C1"), json!(4.0), "{read}");

    let again = call(
        "change_undo",
        json!({"changeSetId": middle["id"], "clientRequestId": "undo-again"}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");
    let redo = call(
        "change_undo",
        json!({"changeSetId": undo["value"]["change"]["id"], "clientRequestId": "undo-redo"}),
    )
    .await;
    assert_eq!(redo["outcome"], "ok", "{redo}");
    let read = call("sheet_get", json!({"projectId": "UNDO", "sheet": "Budget"})).await;
    assert_eq!(value_at(&read, "A1"), json!(5.0), "{read}");
    assert_eq!(value_at(&read, "A2"), json!(7.0), "{read}");

    let moved = call(
        "sheet_update",
        json!({"projectId": "UNDO", "sheet": "Budget",
               "tabs": [{"tab": "Sheet1", "insertRowsAt": 1, "count": 1}],
               "cells": [{"cell": "A1", "input": "top"}], "clientRequestId": "undo-3"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let changes = list("list-2").await;
    assert_eq!(
        changes["value"]["changes"][0]["undoable"], false,
        "{changes}"
    );
}

#[tokio::test]
async fn an_added_row_reports_its_own_row_not_a_formula_below_it() {
    let Some(setup) = setup("c2_sheet_row_add_row").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "ROW", "Rows").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let made = tool(
        app,
        &codex,
        &session,
        "sheet_write",
        json!({
            "projectId": "ROW",
            "name": "Log",
            "tabs": [{"add": "Work"}, {"add": "Totals"}],
            "cells": [
                {"tab": "Work", "cell": "A1", "input": "Task"},
                {"tab": "Work", "cell": "A2", "input": "Paint"},
                {"tab": "Totals", "cell": "A40", "input": "=COUNTA(Work!A1:A100)"},
            ],
            "clientRequestId": "log-1",
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("id").to_owned();
    // The count on the other tab, forty rows down, changes with the
    // append; the row reported is still the one the values went in.
    let added = tool(
        app,
        &codex,
        &session,
        "sheet_row_add",
        json!({"sheetId": sheet, "tab": "Work", "values": {"Task": "Sand"}, "clientRequestId": "row-1"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(added["value"]["row"], 3, "{added}");
}
