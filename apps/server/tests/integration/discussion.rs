//! C1.3: the discussion foundation. People and agents comment on sheets and
//! boards through one operation; a mentioned agent is asked and answers in
//! the thing's comments; anchors fit the thing; and comments are exactly as
//! visible as their thing: a member of another project gets nothing, and a
//! board in the workspace space is every workspace member's and nobody
//! else's.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{Browser, send, tool},
    callers::drain_outbox,
    world::WorldBuilder,
};

fn mention(actor: Uuid, text: &str) -> Value {
    json!({
        "version": 1,
        "nodes": [{"type": "text", "text": text}, {"type": "mention", "actorId": actor}]
    })
}

async fn op(
    app: &axum::Router,
    browser: &Browser,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    send(
        app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&key),
            input,
        ),
    )
    .await
}

async fn visible(pool: &sqlx::PgPool, conversation: &str, viewer: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT conversation_visible_to($1::uuid, $2) AND conversation_readable_by($1::uuid, $2)",
    )
    .bind(Uuid::parse_str(conversation).expect("id"))
    .bind(viewer)
    .fetch_one(pool)
    .await
    .expect("visibility")
}

#[tokio::test]
async fn things_have_discussions_as_visible_as_they_are() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .person("lena", "member")
        .person("nina", "member")
        .person("omar", "member")
        .agent("rook", "member")
        .build("c1_3_discussion")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = world.project("WORK").to_owned();
    let lena = &world.people["lena"];
    let nina = &world.people["nina"];
    let omar = &world.people["omar"];
    let rook = &world.agents["rook"];
    // Nina is in the workspace but not in WORK: a member of another project.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(Uuid::parse_str(&work).expect("work"))
    .bind(nina.actor)
    .execute(pool)
    .await
    .expect("nina leaves WORK");

    // A sheet in WORK, and boards in WORK and outside any project.
    let sheet = tool(
        app,
        &rook.token,
        &rook.session,
        "sheet_write",
        json!({
            "projectId": work,
            "name": "Budget",
            "cells": [{"cell": "A1", "input": "Rent"}],
            "clientRequestId": "discussion-sheet",
        }),
    )
    .await;
    assert_eq!(sheet["outcome"], "ok", "{sheet}");
    let sheet = sheet["value"]["id"].as_str().expect("sheet").to_owned();
    let tab = sqlx::query_scalar::<_, Uuid>("SELECT id FROM sheet_tabs WHERE sheet_id = $1::uuid")
        .bind(&sheet)
        .fetch_one(pool)
        .await
        .expect("tab");
    let mut boards = Vec::new();
    for project in [Some(work.clone()), None] {
        let mut input = json!({
            "title": "Flow",
            "operations": [{"op": "add", "type": "rectangle", "id": "start", "text": "Start"}],
        });
        if let Some(project) = &project {
            input["projectId"] = json!(project);
        }
        let board = tool(app, &rook.token, &rook.session, "whiteboard_create", input).await;
        assert_eq!(board["outcome"], "ok", "{board}");
        boards.push(
            board["value"]["whiteboard"]["id"]
                .as_str()
                .expect("board")
                .to_owned(),
        );
    }

    // Lena comments on the sheet, mentioning Rook: Rook is asked to reply in
    // the sheet's comments, and answers there.
    let (status, posted) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({
            "thing": {"kind": "sheet", "id": sheet},
            "content": mention(rook.actor, "Can you check rent? "),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(posted["outcome"], "ok", "{posted}");
    let comments = posted["value"]["conversationId"]
        .as_str()
        .expect("comments")
        .to_owned();
    assert_eq!(
        posted["value"]["href"],
        format!("/projects/{work}/sheets/{sheet}").as_str()
    );
    let message = Uuid::parse_str(posted["value"]["messageId"].as_str().expect("message"))
        .expect("message id");
    let (request, generation) = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT id, generation FROM reply_requests WHERE message_id = $1 AND agent_actor_id = $2",
    )
    .bind(message)
    .bind(rook.actor)
    .fetch_one(pool)
    .await
    .expect("Rook is asked to reply");
    let claimed = tool(
        app,
        &rook.token,
        &rook.session,
        "reply_claim",
        json!({"requestId": request}),
    )
    .await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    let answered = tool(
        app,
        &rook.token,
        &rook.session,
        "reply_send",
        json!({
            "requestId": request,
            "generation": claimed["value"]["request"]["generation"].clone(),
            "text": "Rent is right.",
        }),
    )
    .await;
    assert_eq!(
        answered["outcome"], "ok",
        "{answered} (generation {generation})"
    );
    let (in_comments,) = sqlx::query_as::<_, (i64,)>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid",
    )
    .bind(&comments)
    .fetch_one(pool)
    .await
    .expect("comments");
    assert_eq!(in_comments, 2, "the question and Rook's answer");

    // A second first comment finds the same conversation.
    let again = tool(
        app,
        &rook.token,
        &rook.session,
        "comment_create",
        json!({
            "thing": {"kind": "sheet", "id": sheet},
            "text": "And the deposit?",
            "clientRequestId": "rook-deposit",
        }),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["conversationId"], comments.as_str());

    // A thread about one cell; a tab of another sheet is refused, and so is
    // an anchor that doesn't fit a sheet.
    let (_, cell) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({
            "thing": {"kind": "sheet", "id": sheet},
            "text": "Is this monthly?",
            "anchor": {"kind": "cell", "tabId": tab, "row": 0, "column": 0, "extra": "dropped"},
        }),
    )
    .await;
    assert_eq!(cell["outcome"], "ok", "{cell}");
    let thread = cell["value"]["threadId"]
        .as_str()
        .expect("thread")
        .to_owned();
    for anchor in [
        json!({"kind": "cell", "tabId": Uuid::now_v7(), "row": 0, "column": 0}),
        json!({"kind": "elements", "elementIds": ["a"], "at": {"x": 0, "y": 0}}),
    ] {
        let (_, refused) = op(
            app,
            &lena.browser,
            workspace,
            "comment_create",
            json!({"thing": {"kind": "sheet", "id": sheet}, "text": "x", "anchor": anchor}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
    let (_, listed) = op(
        app,
        &lena.browser,
        workspace,
        "comment_list",
        json!({"thing": {"kind": "sheet", "id": sheet}}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["messageCount"], 3, "{listed}");
    let threads = listed["value"]["threads"].as_array().expect("threads");
    assert_eq!(threads.len(), 1);
    assert_eq!(
        threads[0]["anchor"],
        json!({"kind": "cell", "tabId": tab, "row": 0, "column": 0})
    );
    let (_, resolved) = op(
        app,
        &lena.browser,
        workspace,
        "comment_thread_update",
        json!({"threadId": thread, "status": "Resolved"}),
    )
    .await;
    assert_eq!(resolved["value"]["status"], "Resolved", "{resolved}");

    // Nina, in the workspace but not WORK, gets nothing: not the list, not
    // the timeline, not a comment of her own, not the thread.
    for (name, input) in [
        (
            "comment_list",
            json!({"thing": {"kind": "sheet", "id": sheet}}),
        ),
        (
            "comment_create",
            json!({"thing": {"kind": "sheet", "id": sheet}, "text": "peek"}),
        ),
        (
            "comment_thread_update",
            json!({"threadId": thread, "status": "Open"}),
        ),
    ] {
        let (_, refused) = op(app, &nina.browser, workspace, name, input).await;
        assert_eq!(refused["outcome"], "not_found", "{name}: {refused}");
    }
    let (status, _) = send(
        app,
        nina.browser.request(
            "GET",
            &format!("/api/v1/conversations/{comments}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!visible(pool, &comments, nina.actor).await);
    assert!(!visible(pool, &thread, nina.actor).await);
    assert!(visible(pool, &thread, lena.actor).await);

    // Tasks and pages keep their own comments for now.
    let (_, page_refused) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({"thing": {"kind": "task", "id": Uuid::now_v7()}, "text": "x"}),
    )
    .await;
    assert_eq!(page_refused["outcome"], "not_found", "{page_refused}");

    // The board outside any project: its comments are in the workspace
    // space, so every workspace member sees them (Nina too) and someone who
    // leaves the workspace doesn't.
    let (_, on_board) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({
            "thing": {"kind": "whiteboard", "id": boards[1]},
            "text": "Start smaller.",
            "anchor": {"kind": "elements", "elementIds": ["start"], "at": {"x": 10, "y": 20}},
        }),
    )
    .await;
    assert_eq!(on_board["outcome"], "ok", "{on_board}");
    let board_comments = on_board["value"]["conversationId"]
        .as_str()
        .expect("board comments")
        .to_owned();
    let (_, nina_reads) = op(
        app,
        &nina.browser,
        workspace,
        "comment_list",
        json!({"thing": {"kind": "whiteboard", "id": boards[1]}}),
    )
    .await;
    assert_eq!(
        nina_reads["value"]["threads"].as_array().map(Vec::len),
        Some(1),
        "{nina_reads}"
    );
    assert!(visible(pool, &board_comments, nina.actor).await);
    sqlx::query("UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2")
        .bind(workspace)
        .bind(omar.actor)
        .execute(pool)
        .await
        .expect("omar leaves");
    assert!(!visible(pool, &board_comments, omar.actor).await);

    // The board in WORK follows WORK: Nina can't comment there.
    let (_, refused) = op(
        app,
        &nina.browser,
        workspace,
        "comment_create",
        json!({"thing": {"kind": "whiteboard", "id": boards[0]}, "text": "peek"}),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    // The sheet's maker hears about comments; links open the sheet.
    drain_outbox(&world.state).await;
    let (status, inbox) = send(
        app,
        lena.browser
            .request("GET", "/api/v1/inbox", None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    let told = inbox["items"]
        .as_array()
        .or_else(|| inbox["notifications"].as_array())
        .expect("items")
        .iter()
        .find(|item| item["kind"] == "thing_comment")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        told["href"]
            .as_str()
            .is_some_and(|href| href.starts_with(&format!("/projects/{work}/sheets/{sheet}"))),
        "Lena commented before, so Rook's answer tells her: {inbox}"
    );
}
