//! K3.6: boards and sheets in search. The silent, costly failures: a board
//! or a sheet found by someone who can't open it (outside the workspace, not
//! in the sheet's project, or with Sheets switched off), and their words
//! never being found at all.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::add_person,
    modules::switch,
};

/// The hits `browser` gets from the web search for `text`.
async fn hits(setup: &Setup, browser: &Browser, text: &str) -> Vec<Value> {
    let (status, body) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/search?q={text}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["hits"].as_array().expect("hits").clone()
}

fn found(hits: &[Value], kind: &str, id: &str) -> bool {
    hits.iter()
        .any(|hit| hit["kind"] == kind && hit["id"] == id)
}

#[tokio::test]
async fn board_and_sheet_words_are_found_only_by_those_who_can_open_them() {
    let Some(setup) = setup("k3_6_search_kinds").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTH", "Other").await;
    // A workspace member in another project: sees the workspace's boards,
    // not WORK's sheets.
    let (_bea, bea) = add_person(&setup, &[&other], "bea").await;
    // Someone in another workspace altogether: signed up here, then moved.
    let (ola_id, ola) = add_person(&setup, &[&other], "ola").await;
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(&setup.pool)
        .await
        .expect("other workspace");
    sqlx::query("UPDATE organization_memberships SET organization_id = $1 WHERE actor_id = $2")
        .bind(elsewhere)
        .bind(ola_id)
        .execute(&setup.pool)
        .await
        .expect("move ola");
    sqlx::query("DELETE FROM project_memberships WHERE actor_id = $1")
        .bind(ola_id)
        .execute(&setup.pool)
        .await
        .expect("ola leaves OTH");

    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    // A workspace board with words on it, and a project sheet with words
    // in a cell.
    let board = call(
        "whiteboard_create",
        json!({
            "title": "Planning wall",
            "workspaceId": setup.organization_id,
            "operations": [{"op": "add", "type": "rectangle", "id": "a", "text": "Quokka migration"}],
        }),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let sheet = call(
        "sheet_write",
        json!({
            "projectId": "WORK",
            "name": "Budget",
            "cells": [{"cell": "A1", "input": "Wombat hosting"}, {"cell": "B1", "input": "12"}],
            "clientRequestId": "k3-6-sheet",
        }),
    )
    .await;
    assert_eq!(sheet["outcome"], "ok", "{sheet}");
    let sheet = sheet["value"]["id"].as_str().expect("sheet").to_owned();

    // The owner finds both by their words, on the web and through the tool.
    let owner = hits(&setup, &setup.browser, "quokka").await;
    assert!(found(&owner, "whiteboard", &board), "{owner:?}");
    let hit = owner.iter().find(|hit| hit["id"] == board).expect("board");
    assert_eq!(hit["projectId"], Value::Null, "{hit}");
    assert_eq!(hit["workspaceId"], setup.organization_id.to_string());
    let owner = hits(&setup, &setup.browser, "wombat").await;
    assert!(found(&owner, "sheet", &sheet), "{owner:?}");
    let hit = owner.iter().find(|hit| hit["id"] == sheet).expect("sheet");
    assert_eq!(hit["projectId"], work, "{hit}");
    let by_tool = call(
        "search",
        json!({"query": "quokka", "kinds": ["whiteboards"]}),
    )
    .await;
    assert_eq!(by_tool["outcome"], "ok", "{by_tool}");
    assert!(
        by_tool["value"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .any(|hit| hit["kind"] == "whiteboard" && hit["id"] == board),
        "{by_tool}"
    );
    let by_tool = call("search", json!({"query": "wombat", "kinds": ["sheets"]})).await;
    assert!(
        by_tool["value"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .any(|hit| hit["kind"] == "sheet" && hit["id"] == sheet),
        "{by_tool}"
    );

    // A workspace member sees the workspace's board, not WORK's sheet.
    assert!(found(
        &hits(&setup, &bea, "quokka").await,
        "whiteboard",
        &board
    ));
    assert!(hits(&setup, &bea, "wombat").await.is_empty());
    // Someone outside the workspace sees neither.
    assert!(hits(&setup, &ola, "quokka").await.is_empty());
    assert!(hits(&setup, &ola, "wombat").await.is_empty());

    // With Sheets off, the sheet leaves search; the board stays.
    let (status, body) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"sheets": false}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !found(
            &hits(&setup, &setup.browser, "wombat").await,
            "sheet",
            &sheet
        ),
        "the sheet is found with Sheets off"
    );
    let by_tool = call("search", json!({"query": "wombat"})).await;
    assert_eq!(by_tool["value"]["results"], json!([]), "{by_tool}");
    assert!(found(
        &hits(&setup, &setup.browser, "quokka").await,
        "whiteboard",
        &board
    ));
}
