//! C1.11: proposals on sheets and whiteboards. The silent, costly
//! failures: a role that only suggests changing cells or drawing anyway,
//! accepting a proposal over cells or elements someone changed since
//! (undoing their work), and accepting part of one applying more than was
//! chosen.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::add_person,
};

/// Gives `actor` a role in `project` that suggests on sheets and doesn't
/// edit them.
async fn sheet_suggester(setup: &Setup, project: &str, actor: Uuid) {
    suggester(setup, project, actor, "Sheet suggester", "sheets.suggest").await;
}

/// Gives `actor` a role in `project` holding only `permission` (a suggest
/// level) besides a member's basics.
async fn suggester(setup: &Setup, project: &str, actor: Uuid, name: &str, permission: &str) {
    let (status, role) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/roles", setup.organization_id),
            None,
            json!({"name": name, "baseRole": "member", "permissions": [permission]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(project)
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("membership");
    let (status, changed) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/projects/{project}/members/{actor}"),
            Some("make-suggester"),
            json!({"roleId": role["id"], "expectedVersion": version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
}

#[tokio::test]
async fn a_suggest_only_role_proposes_cells_and_accepting_keeps_newer_work() {
    let Some(setup) = setup("c111_sheet_proposals").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let owner = connect_app(app, &setup.browser, "Owner app", "full").await;
    let owner_session = initialize(app, &owner).await.expect("owner MCP");
    let as_owner = |name: &'static str, arguments: Value| {
        let token = owner.clone();
        let session = owner_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    sheet_suggester(&setup, &work, bea).await;
    let bea_app = connect_app(app, &bea_browser, "Bea app", "full").await;
    let bea_session = initialize(app, &bea_app).await.expect("Bea MCP");
    let as_bea = |name: &'static str, arguments: Value| {
        let token = bea_app.clone();
        let session = bea_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let made = as_owner(
        "sheet_write",
        json!({"projectId": "WORK", "name": "Budget", "cells": [
            {"cell": "A1", "input": "1"}, {"cell": "A2", "input": "2"}, {"cell": "A3", "input": "3"},
            {"cell": "A4", "input": "4"}, {"cell": "A5", "input": "5"},
        ], "clientRequestId": "make"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let sheet = made["value"]["id"].as_str().expect("sheet").to_owned();
    let inputs = || async {
        let read = as_owner(
            "sheet_read",
            json!({"projectId": "WORK", "sheet": sheet, "range": "A1:A5"}),
        )
        .await;
        ["A1", "A2", "A3", "A4", "A5"].map(|name| {
            read["value"]["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|cell| cell["cell"] == name)
                .map(|cell| cell["value"].clone())
                .unwrap_or(Value::Null)
        })
    };

    // Bea's write of five cells becomes a proposal; nothing changes yet.
    let proposed = as_bea(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "cells": [
            {"cell": "A1", "input": "10"}, {"cell": "A2", "input": "20"}, {"cell": "A3", "input": "30"},
            {"cell": "A4", "input": "40"}, {"cell": "A5", "input": "50"},
        ], "clientRequestId": "propose-1"}),
    )
    .await;
    assert_eq!(proposed["outcome"], "ok", "{proposed}");
    assert_eq!(proposed["value"]["cellsChanged"], 0, "{proposed}");
    let first = proposed["value"]["proposalId"]
        .as_str()
        .expect("a proposal")
        .to_owned();
    assert_eq!(
        proposed["value"]["proposed"].as_array().map(Vec::len),
        Some(5),
        "{proposed}"
    );
    assert_eq!(
        inputs().await,
        [1, 2, 3, 4, 5].map(|n: i32| json!(f64::from(n)))
    );

    // Tabs and rows aren't hers to propose.
    let tab = as_bea(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "tabs": [{"add": "More"}], "clientRequestId": "tab"}),
    )
    .await;
    assert_ne!(tab["outcome"], "ok", "{tab}");

    // She can't accept her own proposal; the owner changes A5 meanwhile.
    let own = as_bea(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "clientRequestId": "bea-accepts"}),
    )
    .await;
    assert_ne!(own["outcome"], "ok", "{own}");
    let edited = as_owner(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "cells": [{"cell": "A5", "input": "99"}], "clientRequestId": "a5"}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");

    // Accepting all of it would undo A5: refused, and nothing applied.
    let whole = as_owner(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "clientRequestId": "accept-whole"}),
    )
    .await;
    assert_eq!(whole["outcome"], "conflict", "{whole}");
    assert_eq!(
        inputs().await,
        [1, 2, 3, 4, 99].map(|n: i32| json!(f64::from(n)))
    );

    // Three of five applies exactly those three.
    let part = as_owner(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "parts": [0, 1, 2], "clientRequestId": "accept-part"}),
    )
    .await;
    assert_eq!(part["outcome"], "ok", "{part}");
    assert_eq!(part["value"]["status"], "partly_accepted", "{part}");
    assert_eq!(
        inputs().await,
        [10, 20, 30, 4, 99].map(|n: i32| json!(f64::from(n)))
    );
    let (changed_by,) = sqlx::query_as::<_, (Option<Uuid>,)>(
        "SELECT updated_by_actor_id FROM sheet_cells cell JOIN sheet_tabs tab ON tab.id = cell.tab_id \
         WHERE tab.sheet_id = $1::uuid AND cell.row_number = 1 AND cell.column_number = 1",
    )
    .bind(&sheet)
    .fetch_one(&setup.pool)
    .await
    .expect("A1");
    assert_eq!(changed_by, Some(bea), "the accepted cells are the author's");

    // A second one she withdraws; the owner sees none waiting.
    let second = as_bea(
        "sheet_write",
        json!({"projectId": "WORK", "sheet": sheet, "cells": [{"cell": "A4", "input": "44"}], "clientRequestId": "propose-2"}),
    )
    .await;
    let second = second["value"]["proposalId"]
        .as_str()
        .unwrap_or_else(|| panic!("{second}"))
        .to_owned();
    let withdrawn = as_bea(
        "proposal_decide",
        json!({"proposalId": second, "decision": "withdraw", "clientRequestId": "withdraw"}),
    )
    .await;
    assert_eq!(withdrawn["value"]["status"], "withdrawn", "{withdrawn}");
    let listed = as_owner(
        "proposal_list",
        json!({"thingKind": "sheet", "thingId": sheet}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["proposals"], json!([]), "{listed}");
    assert_eq!(
        inputs().await,
        [10, 20, 30, 4, 99].map(|n: i32| json!(f64::from(n)))
    );
}

#[tokio::test]
async fn a_suggest_only_role_proposes_drawing_and_accepting_keeps_newer_work() {
    let Some(setup) = setup("c111_board_proposals").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let owner = connect_app(app, &setup.browser, "Owner app", "full").await;
    let owner_session = initialize(app, &owner).await.expect("owner MCP");
    let as_owner = |name: &'static str, arguments: Value| {
        let token = owner.clone();
        let session = owner_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let (bea, bea_browser) = add_person(&setup, &[&work], "bea").await;
    suggester(&setup, &work, bea, "Board suggester", "whiteboards.suggest").await;
    let bea_app = connect_app(app, &bea_browser, "Bea app", "full").await;
    let bea_session = initialize(app, &bea_app).await.expect("Bea MCP");
    let as_bea = |name: &'static str, arguments: Value| {
        let token = bea_app.clone();
        let session = bea_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let made = as_owner(
        "whiteboard_create",
        json!({"title": "Plan", "projectId": work, "operations": [
            {"op": "add", "type": "rectangle", "id": "box-a", "text": "A", "x": 0, "y": 0},
            {"op": "add", "type": "rectangle", "id": "box-b", "text": "B", "x": 300, "y": 0},
        ], "clientRequestId": "board"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let board = made["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let element = |id: &'static str| {
        let board = board.clone();
        let pool = setup.pool.clone();
        async move {
            sqlx::query_as::<_, (Value, Option<Uuid>)>(
                "SELECT data, updated_by_actor_id FROM whiteboard_elements \
                 WHERE whiteboard_id = $1::uuid AND element_id = $2",
            )
            .bind(&board)
            .bind(id)
            .fetch_optional(&pool)
            .await
            .expect("element")
        }
    };
    let x_of = |row: &Option<(Value, Option<Uuid>)>| {
        row.as_ref()
            .and_then(|(data, _)| data["x"].as_f64())
            .unwrap_or(f64::NAN)
    };

    // Bea's drawing becomes a proposal; nothing on the board changes.
    let proposed = as_bea(
        "whiteboard_update_elements",
        json!({"whiteboardId": board, "operations": [
            {"op": "move", "ids": ["box-a"], "dx": 50, "dy": 0},
            {"op": "move", "ids": ["box-b"], "dx": 50, "dy": 0},
            {"op": "add", "type": "ellipse", "id": "new-c", "text": "C", "x": 0, "y": 300},
        ], "clientRequestId": "propose-1"}),
    )
    .await;
    assert_eq!(proposed["outcome"], "ok", "{proposed}");
    let first = proposed["value"]["proposalId"]
        .as_str()
        .unwrap_or_else(|| panic!("a proposal: {proposed}"))
        .to_owned();
    assert!(element("new-c").await.is_none(), "nothing drawn yet");
    assert_eq!(x_of(&element("box-a").await), 0.0);

    // An image isn't hers to put on the board.
    let image = as_bea(
        "whiteboard_update_elements",
        json!({"whiteboardId": board, "operations": [{"op": "add", "type": "image",
            "dataBase64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            "mimeType": "image/png"}], "clientRequestId": "image"}),
    )
    .await;
    assert_ne!(image["outcome"], "ok", "{image}");

    // She can't accept her own; the owner moves box-b meanwhile.
    let own = as_bea(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "clientRequestId": "bea-accepts"}),
    )
    .await;
    assert_ne!(own["outcome"], "ok", "{own}");
    let moved = as_owner(
        "whiteboard_update_elements",
        json!({"whiteboardId": board, "operations": [{"op": "move", "ids": ["box-b"], "dx": 0, "dy": 100}],
               "clientRequestId": "owner-moves"}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");

    // The waiting proposal shows what it does, with a picture of it.
    let listed = as_owner(
        "proposal_list",
        json!({"thingKind": "whiteboard", "thingId": board}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let waiting = &listed["value"]["proposals"][0];
    assert_eq!(waiting["thingTitle"], "Plan", "{listed}");
    let parts = waiting["change"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .map(|element| element["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        parts.contains(&"new-c".to_owned()) && parts.contains(&"box-b".to_owned()),
        "{parts:?}"
    );
    let preview =
        sqlx::query_scalar::<_, Value>("SELECT preview FROM proposals WHERE id = $1::uuid")
            .bind(&first)
            .fetch_one(&setup.pool)
            .await
            .expect("preview");
    assert!(
        preview["after"]
            .as_str()
            .is_some_and(|svg| svg.starts_with("<svg")),
        "{preview}"
    );

    // Accepting all of it would undo the owner's move: refused, nothing applied.
    let whole = as_owner(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "clientRequestId": "accept-whole"}),
    )
    .await;
    assert_eq!(whole["outcome"], "conflict", "{whole}");
    assert!(element("new-c").await.is_none());
    assert_eq!(x_of(&element("box-a").await), 0.0);

    // Everything but box-b applies exactly that, as Bea's.
    // (box-b's label moved with it, so it stays out too.)
    let chosen = waiting["change"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .enumerate()
        .filter(|(_, element)| {
            element["id"] != "box-b" && element["after"]["containerId"] != "box-b"
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let part = as_owner(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "parts": chosen, "clientRequestId": "accept-part"}),
    )
    .await;
    assert_eq!(part["outcome"], "ok", "{part}");
    assert_eq!(part["value"]["status"], "partly_accepted", "{part}");
    assert_eq!(x_of(&element("box-a").await), 50.0);
    assert_eq!(
        x_of(&element("box-b").await),
        300.0,
        "the owner's move stands"
    );
    let drawn = element("new-c").await.expect("new-c drawn");
    assert_eq!(drawn.1, Some(bea), "accepted elements are the author's");

    // From the web, a suggest-mode drawing goes up as elements; she withdraws it.
    let (status, sent) = send(
        app,
        bea_browser.request(
            "POST",
            "/api/v2/ops/whiteboard_propose",
            Some("bea-web-propose"),
            json!({"whiteboardId": board, "note": "One more", "elements": [{
                "id": "web-d", "type": "rectangle", "x": 600, "y": 0, "width": 80, "height": 40,
                "version": 1, "versionNonce": 7, "isDeleted": false
            }]}),
        ),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{status} {sent}");
    let second = sent["value"]["proposalId"]
        .as_str()
        .unwrap_or_else(|| panic!("{sent}"))
        .to_owned();
    assert!(element("web-d").await.is_none());
    let withdrawn = as_bea(
        "proposal_decide",
        json!({"proposalId": second, "decision": "withdraw", "clientRequestId": "withdraw"}),
    )
    .await;
    assert_eq!(withdrawn["value"]["status"], "withdrawn", "{withdrawn}");
    let listed = as_owner(
        "proposal_list",
        json!({"thingKind": "whiteboard", "thingId": board}),
    )
    .await;
    assert_eq!(listed["value"]["proposals"], json!([]), "{listed}");
}
