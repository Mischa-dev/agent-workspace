//! K3.19: history as a seam. The silent, costly failures: a restore that
//! overwrites instead of making a new version, one credited to the wrong
//! account or heard by nobody, a kind that can't restore pretending it did,
//! and a history read about a thing the caller can't see.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
    support::{
        app::{send, tool as support_tool},
        world::WorldBuilder,
    },
};

#[tokio::test]
async fn pages_and_boards_restore_through_one_seam() {
    let Some(setup) = setup("k3_19_history").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let ok = |result: Value, name: &str| {
        assert!(
            matches!(result["outcome"].as_str(), Some("ok" | "applied")),
            "{name}: {result}"
        );
        result
    };

    // A page with two versions.
    let page = ok(
        call(
            "page_write",
            json!({"projectId": "WORK", "title": "Gate", "body": "Paint it red."}),
        )
        .await,
        "page_write",
    );
    let page = page["value"]["id"].as_str().expect("page").to_owned();
    ok(
        call(
            "page_write",
            json!({"pageId": page, "body": "Paint it blue.", "expectedVersion": 1}),
        )
        .await,
        "page_write",
    );
    let listed = ok(
        call("history_list", json!({"thing": page})).await,
        "history_list",
    );
    let value = &listed["value"];
    assert_eq!(value["kind"], "page", "{listed}");
    assert_eq!(value["caps"]["restore"], true);
    let entries = value["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2, "{listed}");
    assert_eq!(entries[0]["version"], 2);
    assert_eq!(entries[0]["changed"], json!(["body"]));
    let shown = ok(
        call("history_show", json!({"thing": page, "version": 1})).await,
        "history_show",
    );
    assert_eq!(shown["value"]["content"]["body"], "Paint it red.");

    // A stale restore is refused; a current one makes version 3.
    let stale = call(
        "history_restore",
        json!({"thing": page, "version": 1, "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let restored = ok(
        call(
            "history_restore",
            json!({"thing": page, "version": 1, "expectedVersion": 2}),
        )
        .await,
        "history_restore",
    );
    assert_eq!(restored["value"]["version"], 3, "{restored}");
    let (body, version, editor) = sqlx::query_as::<_, (String, i64, Option<Uuid>)>(
        "SELECT body, version, updated_by_actor_id FROM project_pages WHERE id = $1::uuid",
    )
    .bind(&page)
    .fetch_one(pool)
    .await
    .expect("page");
    assert_eq!((body.as_str(), version), ("Paint it red.", 3));
    let me = editor.expect("restorer");
    let heard = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM event_staging \
         WHERE type = 'page.version_restored' AND thing_id = $1::uuid \
           AND actor_account_id = $2 AND thing_version = 3",
    )
    .bind(&page)
    .bind(me)
    .fetch_one(pool)
    .await
    .expect("event");
    assert_eq!(heard, 1, "the restore is in the log, by the restorer");
    let after = ok(
        call("history_list", json!({"thing": page, "limit": 1})).await,
        "history_list",
    );
    assert_eq!(after["value"]["entries"][0]["version"], 3);

    // A board: the second edit saves the first edit's scene as revision 1.
    let board = ok(
        call(
            "whiteboard_create",
            json!({"projectId": "WORK", "title": "Gate sketch"}),
        )
        .await,
        "whiteboard_create",
    );
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .or_else(|| board["value"]["id"].as_str())
        .expect("board")
        .to_owned();
    let edit = |operations: Value| {
        let board = board.clone();
        async move {
            ok(
                call(
                    "whiteboard_edit",
                    json!({"whiteboardId": board, "operations": operations}),
                )
                .await,
                "whiteboard_edit",
            )
        }
    };
    edit(json!([{"op": "add", "type": "rectangle", "id": "gate", "x": 0, "y": 0}])).await;
    let second =
        edit(json!([{"op": "add", "type": "rectangle", "id": "post", "x": 300, "y": 0}])).await;
    let seq = second["value"]["sceneSeq"].as_i64().expect("seq");
    let listed = ok(
        call("history_list", json!({"thing": board})).await,
        "history_list",
    );
    let revision = listed["value"]["entries"][0]["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("a revision in {listed}"));
    let restored = ok(
        call(
            "history_restore",
            json!({"thing": board, "version": revision, "expectedVersion": seq}),
        )
        .await,
        "history_restore",
    );
    assert!(restored["value"]["version"].as_i64().expect("seq") > seq);
    let live = sqlx::query_scalar::<_, String>(
        "SELECT element_id FROM whiteboard_elements \
         WHERE whiteboard_id = $1::uuid AND NOT is_deleted ORDER BY element_id",
    )
    .bind(&board)
    .fetch_all(pool)
    .await
    .expect("elements");
    assert_eq!(
        live,
        ["gate"],
        "the post added after revision {revision} is gone"
    );
    let heard = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM event_staging \
         WHERE type = 'whiteboard.version_restored' AND thing_id = $1::uuid AND actor_account_id = $2",
    )
    .bind(&board)
    .bind(me)
    .fetch_one(pool)
    .await
    .expect("event");
    assert_eq!(heard, 1);

    // A sheet restores through the same seam (C1.13; its own test is in
    // awd54_sheets.rs).
    let sheet = ok(
        call(
            "sheet_write",
            json!({"projectId": "WORK", "name": "Costs", "cells": [{"cell": "A1", "input": "Paint"}], "clientRequestId": "k3-19-sheet"}),
        )
        .await,
        "sheet_write",
    );
    let sheet = sheet["value"]["id"]
        .as_str()
        .or_else(|| sheet["value"]["sheet"]["id"].as_str())
        .expect("sheet")
        .to_owned();
    let listed = ok(
        call("history_list", json!({"thing": sheet})).await,
        "history_list",
    );
    assert_eq!(listed["value"]["caps"]["restore"], true);

    // A task keeps changes, not versions: restore says so, with why.
    let task = ok(
        call(
            "task_create",
            json!({"projectId": "WORK", "title": "Paint the hall", "clientRequestId": "k3-19-task"}),
        )
        .await,
        "task_create",
    );
    let task = task["value"]["workItemId"]
        .as_str()
        .expect("task")
        .to_owned();
    let refused = call(
        "history_restore",
        json!({"thing": task, "version": 1, "expectedVersion": 1}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    let said = refused.to_string();
    assert!(
        said.contains("not_supported") && said.contains("comes later"),
        "{refused}"
    );

    // A thing that doesn't exist is not found (workspace_isolation.rs holds
    // one in another workspace to the same answer).
    let missing = call("history_list", json!({"thing": Uuid::now_v7()})).await;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
}

/// By id alone, a private conversation's message stays its members' even
/// while the registry doesn't yet know its conversation (a backfill still
/// running leaves `access_parent_id` empty), and a workspace board, which
/// has no space, is found.
#[tokio::test]
async fn history_by_id_keeps_private_messages_and_finds_workspace_boards() {
    let Some(world) = WorldBuilder::default()
        .project("HIST")
        .person("ana", "member")
        .agent("bob", "member")
        .connected_app("helper")
        .private_group("secret", &["ana"])
        .build("k3_19_private_history")
        .await
    else {
        return;
    };
    let app = &world.app;
    let helper = &world.apps["helper"];
    let bob = &world.agents["bob"];
    let as_helper = |name: &'static str, arguments: Value| {
        support_tool(app, &helper.token, &helper.session, name, arguments)
    };
    let sent = as_helper(
        "message_send",
        json!({"conversationId": world.conversations["secret"], "text": "between us", "clientRequestId": "k3-19-secret"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let message = sent["value"]["messageId"]
        .as_str()
        .expect("message id")
        .to_owned();

    // The registry loses the message's conversation, as before a backfill
    // reaches it.
    sqlx::query("UPDATE things SET access_parent_id = NULL, parent_id = NULL WHERE id = $1::uuid")
        .bind(&message)
        .execute(world.pool())
        .await
        .expect("forget the parent");
    let own = as_helper("history_list", json!({"thing": message})).await;
    assert_eq!(own["outcome"], "ok", "a member reads it: {own}");
    for (name, arguments) in [
        ("history_list", json!({"thing": message})),
        ("history_show", json!({"thing": message, "version": 1})),
    ] {
        let theirs = support_tool(app, &bob.token, &bob.session, name, arguments).await;
        assert_eq!(
            theirs["outcome"], "not_found",
            "{name} as a non-member: {theirs}"
        );
        assert!(!theirs.to_string().contains("between us"), "{theirs}");
    }

    // A workspace board (no space) is located and listed.
    let (status, created) = send(
        app,
        world.owner.browser.request(
            "POST",
            "/api/v1/whiteboards",
            None,
            json!({"title": "Whole workspace", "organizationId": world.organization}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {created}");
    let board = created["whiteboard"]["id"].as_str().expect("board id");
    let listed = as_helper("history_list", json!({"thing": board})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["kind"], "whiteboard", "{listed}");
    world.finish().await;
}
