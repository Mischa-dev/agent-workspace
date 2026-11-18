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
    callers::{Agent, Call, Caller, drain_outbox},
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

    // Muting the sheet's comments quiets them for Lena.
    sqlx::query(
        "INSERT INTO conversation_notification_levels (project_id, actor_id, conversation_id, level) \
         SELECT project_id, $1, id, 'nothing' FROM conversations WHERE id = $2::uuid",
    )
    .bind(lena.actor)
    .bind(&comments)
    .execute(pool)
    .await
    .expect("lena mutes");
    let told_before = thing_comments_for(pool, lena.actor).await;
    let muted = tool(
        app,
        &rook.token,
        &rook.session,
        "comment_create",
        json!({"thing": {"kind": "sheet", "id": sheet}, "text": "Deposit is two months."}),
    )
    .await;
    assert_eq!(muted["outcome"], "ok", "{muted}");
    drain_outbox(&world.state).await;
    assert_eq!(thing_comments_for(pool, lena.actor).await, told_before);

    // Once the sheet is archived its comments take nothing new, even sent
    // straight into the conversation instead of through comment_create.
    sqlx::query(
        "UPDATE sheets SET archived_at = now(), archived_by_actor_id = $2 WHERE id = $1::uuid",
    )
    .bind(&sheet)
    .bind(lena.actor)
    .execute(pool)
    .await
    .expect("archive the sheet");
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "message_send",
        json!({"conversationId": comments, "text": "Too late", "clientRequestId": "rook-late"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (late,) = sqlx::query_as::<_, (i64,)>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid AND plain_text = 'Too late'",
    )
    .bind(&comments)
    .fetch_one(pool)
    .await
    .expect("late");
    assert_eq!(late, 0);
}

async fn thing_comments_for(pool: &sqlx::PgPool, actor: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notification_occurrences occurrence \
         JOIN notifications notification ON notification.id = occurrence.notification_id \
         WHERE notification.kind = 'thing_comment' AND occurrence.recipient_actor_id = $1",
    )
    .bind(actor)
    .fetch_one(pool)
    .await
    .expect("thing comments")
}

/// The `comment.mentioned` events an agent hears, as (comment kind,
/// comment, thing), read the way the agent reads them.
async fn mentioned_events(
    app: &axum::Router,
    pool: &sqlx::PgPool,
    agent: &Agent,
) -> Vec<(String, Uuid, String)> {
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("sequence events");
    let (_, page) = agent
        .call(
            app,
            Call::Tool {
                name: "event_list",
                arguments: json!({ "limit": 100 }),
            },
        )
        .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let mut found = Vec::new();
    for event in page["value"]["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["type"] == "message.mentioned")
    {
        assert!(!event.to_string().contains("thoughts"), "ids only: {event}");
        let data = &event["data"];
        let message: Uuid = data["messageId"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .expect("message");
        // The thing the comment is on.
        let (kind, thing) = sqlx::query_as::<_, (String, Uuid)>(
            r#"
            SELECT COALESCE(parent.subject_kind, place.subject_kind),
                   COALESCE(parent.subject_id, place.subject_id)
            FROM messages message
            JOIN conversations place ON place.id = message.conversation_id
            LEFT JOIN conversations parent ON parent.id = place.parent_channel_id
            WHERE message.id = $1
            "#,
        )
        .bind(message)
        .fetch_one(pool)
        .await
        .expect("the comment's thing");
        found.push((kind, message, thing.to_string()));
    }
    found
}

/// C1.1: a task or page comment that mentions an agent tells it with one
/// `message.mentioned` event (comments are messages since C1.4), carrying ids and
/// no words; an agent not in the project, and the author, get none. A task
/// comment is one path for the web and agent tools.
#[tokio::test]
async fn comments_that_mention_agents_tell_them() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .agent("kite", "member")
        .build("c1_1_comment_mentioned")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = world.project("WORK").to_owned();
    let lena = &world.people["lena"];
    let rook = &world.agents["rook"];
    let kite = &world.agents["kite"];
    // Kite is in the workspace but not in WORK.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&work)
    .bind(kite.actor)
    .execute(pool)
    .await
    .expect("leave WORK");
    assert!(mentioned_events(app, pool, rook).await.is_empty());
    assert!(mentioned_events(app, pool, kite).await.is_empty());

    let (status, task) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some(&Uuid::now_v7().to_string()),
            json!({"title": "Ship it"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{task}");
    let task_id = task["id"]
        .as_str()
        .or_else(|| task["workItem"]["id"].as_str())
        .expect("task id")
        .to_owned();
    let (status, posted) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items/{task_id}/comments"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": mention(rook.actor, "can you look, ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{posted}");
    let comment_id =
        Uuid::parse_str(posted["comment"]["id"].as_str().expect("comment")).expect("id");
    let events = mentioned_events(app, pool, rook).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "task");
    assert_eq!(events[0].1, comment_id);
    assert_eq!(events[0].2, task_id);

    // An agent's own comment naming itself and an agent outside the project
    // tells neither.
    let own = tool(
        app,
        &rook.token,
        &rook.session,
        "task_comment",
        json!({"taskId": task_id, "text": "@rook and @kite on it", "clientRequestId": "rook-1"}),
    )
    .await;
    assert_eq!(own["outcome"], "applied", "{own}");
    assert_eq!(mentioned_events(app, pool, rook).await.len(), 1);
    assert!(mentioned_events(app, pool, kite).await.is_empty());

    // A page comment names agents by handle.
    let (status, page) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"title": "Plan", "body": "The plan."}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let page_id = page["id"].as_str().expect("page").to_owned();
    let (status, created) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/pages/{page_id}/comments"),
            Some(&Uuid::now_v7().to_string()),
            json!({"body": "@Rook and @kite, thoughts?"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let page_comment = created["id"].as_str().expect("comment").to_owned();
    let events = mentioned_events(app, pool, rook).await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1].0, "page");
    assert_eq!(events[1].1.to_string(), page_comment);
    assert_eq!(events[1].2, page_id);
    assert!(mentioned_events(app, pool, kite).await.is_empty());
}

/// C1.19: a comment on a cell stays on that cell when rows go in above it
/// and a column comes out before it, and a thread whose cell is deleted
/// keeps its comments with no anchor.
#[tokio::test]
async fn sheet_anchor_follows_a_row_insert() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("c1_19_sheet_anchor")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = world.project("WORK").to_owned();
    let lena = &world.people["lena"];
    let rook = &world.agents["rook"];
    let sheet = tool(
        app,
        &rook.token,
        &rook.session,
        "sheet_write",
        json!({
            "projectId": work,
            "name": "Budget",
            "cells": [{"cell": "C3", "input": "Rent"}, {"cell": "A9", "input": "Spare"}],
            "clientRequestId": "anchor-sheet",
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
    // C3 is row 2, column 2 counted from 0; A9 is row 8.
    let mut threads = Vec::new();
    for anchor in [
        json!({"kind": "cell", "tabId": tab, "row": 2, "column": 2}),
        json!({"kind": "cell", "tabId": tab, "row": 8, "column": 0}),
    ] {
        let (_, made) = op(
            app,
            &lena.browser,
            workspace,
            "comment_create",
            json!({"thing": {"kind": "sheet", "id": sheet}, "text": "Check this", "anchor": anchor}),
        )
        .await;
        assert_eq!(made["outcome"], "ok", "{made}");
        threads.push(
            made["value"]["threadId"]
                .as_str()
                .expect("thread")
                .to_owned(),
        );
    }

    // Two rows above C3 and the first column out: the cell is now B5, and
    // A9's column is gone.
    let wrote = tool(
        app,
        &rook.token,
        &rook.session,
        "sheet_write",
        json!({
            "projectId": work,
            "sheet": sheet,
            "tabs": [
                {"tab": "Sheet1", "insertRowsAt": 1, "count": 2},
                {"tab": "Sheet1", "deleteColumnsAt": "A"},
            ],
        }),
    )
    .await;
    assert_eq!(wrote["outcome"], "ok", "{wrote}");
    let input = sqlx::query_scalar::<_, String>(
        "SELECT input FROM sheet_cells WHERE tab_id = $1 AND row_number = 5 AND column_number = 2",
    )
    .bind(tab)
    .fetch_one(pool)
    .await
    .expect("the cell moved to B5");
    assert_eq!(input, "Rent");
    let anchor_of = |thread: String| async move {
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT anchor FROM conversations WHERE id = $1::uuid",
        )
        .bind(thread)
        .fetch_one(pool)
        .await
        .expect("anchor")
    };
    assert_eq!(
        anchor_of(threads[0].clone()).await,
        Some(json!({"kind": "cell", "tabId": tab, "row": 4, "column": 1}))
    );
    // A9 was in the deleted column, so its thread is about the whole sheet.
    assert_eq!(anchor_of(threads[1].clone()).await, None);

    let (_, listed) = op(
        app,
        &lena.browser,
        workspace,
        "comment_list",
        json!({"thing": {"kind": "sheet", "id": sheet}}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let threads = listed["value"]["threads"].as_array().expect("threads");
    assert_eq!(
        threads.len(),
        2,
        "both threads keep their comments: {listed}"
    );
    assert!(
        threads.iter().all(|thread| thread["messageCount"] == 1),
        "{listed}"
    );
}

/// Follow-up from #287's review: a board's discussion moves with the board,
/// to another space and to the workspace, with its threads, replies and
/// reactions, and the new space's members can read and add to it.
#[tokio::test]
async fn a_boards_discussion_moves_with_it() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .person("lena", "member")
        .person("nina", "member")
        .build("d2_board_discussion_moves")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let other = Uuid::parse_str(world.project("OTHER")).expect("other");
    let lena = &world.people["lena"];
    let nina = &world.people["nina"];
    // Nina is only in OTHER.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work)
    .bind(nina.actor)
    .execute(pool)
    .await
    .expect("nina leaves WORK");

    let (_, board) = op(
        app,
        &lena.browser,
        workspace,
        "whiteboard_create",
        json!({
            "title": "Flow",
            "projectId": work,
            "operations": [{"op": "add", "type": "rectangle", "id": "start", "text": "Start"}],
        }),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let thing = json!({"kind": "whiteboard", "id": board});
    let (_, first) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({"thing": thing, "text": "Start smaller.", "anchor": {"kind": "elements", "elementIds": ["start"], "at": {"x": 1, "y": 2}}}),
    )
    .await;
    assert_eq!(first["outcome"], "ok", "{first}");
    let thread = first["value"]["threadId"]
        .as_str()
        .expect("thread")
        .to_owned();
    let (_, reply) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({"thing": thing, "text": "And label it.", "threadId": thread}),
    )
    .await;
    assert_eq!(reply["outcome"], "ok", "{reply}");
    let (_, stream) = op(
        app,
        &lena.browser,
        workspace,
        "comment_create",
        json!({"thing": thing, "text": "Looks good overall."}),
    )
    .await;
    assert_eq!(stream["outcome"], "ok", "{stream}");
    let comments = Uuid::parse_str(
        stream["value"]["conversationId"]
            .as_str()
            .expect("comments"),
    )
    .expect("comments id");

    // Nina can't see it while the board is in WORK.
    let (_, hidden) = op(
        app,
        &nina.browser,
        workspace,
        "comment_list",
        json!({"thing": thing}),
    )
    .await;
    assert_eq!(hidden["outcome"], "not_found", "{hidden}");

    // Lena moves the board to OTHER: the whole discussion goes with it.
    let (_, moved) = op(
        app,
        &lena.browser,
        workspace,
        "whiteboard_update",
        json!({"whiteboardId": board, "projectId": other}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let places = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT project_id FROM conversations WHERE id = $1 OR parent_channel_id = $1 \
         UNION SELECT DISTINCT project_id FROM messages WHERE conversation_id IN \
             (SELECT id FROM conversations WHERE id = $1 OR parent_channel_id = $1)",
    )
    .bind(comments)
    .fetch_all(pool)
    .await
    .expect("places");
    assert_eq!(places, vec![other]);
    let (_, listed) = op(
        app,
        &nina.browser,
        workspace,
        "comment_list",
        json!({"thing": thing}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["threads"][0]["messageCount"], 2, "{listed}");
    assert_eq!(listed["value"]["messageCount"], 1, "{listed}");
    let (_, nina_says) = op(
        app,
        &nina.browser,
        workspace,
        "comment_create",
        json!({"thing": thing, "text": "Agreed.", "threadId": thread}),
    )
    .await;
    assert_eq!(nina_says["outcome"], "ok", "{nina_says}");

    // And to the whole workspace: into the workspace's own space.
    let (_, workspace_wide) = op(
        app,
        &lena.browser,
        workspace,
        "whiteboard_update",
        json!({"whiteboardId": board, "workspaceWide": true}),
    )
    .await;
    assert_eq!(workspace_wide["outcome"], "ok", "{workspace_wide}");
    let (board_space, comments_space) = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT board.project_id, conversation.project_id FROM whiteboards board, conversations conversation \
         WHERE board.id = $1::uuid AND conversation.id = $2",
    )
    .bind(&board)
    .bind(comments)
    .fetch_one(pool)
    .await
    .expect("spaces");
    assert_eq!(board_space, comments_space);
    assert_ne!(comments_space, other);
    let (_, after) = op(
        app,
        &nina.browser,
        workspace,
        "comment_list",
        json!({"thing": thing}),
    )
    .await;
    assert_eq!(after["value"]["threads"][0]["messageCount"], 3, "{after}");
}

/// A task's discussion (its comments, their threads and their files) stays
/// with whoever sees tasks: an app whose agent limit keeps tasks from it
/// can't list it, read it or fetch its files, though its person can.
#[tokio::test]
async fn task_discussions_keep_the_task_limit() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("laptop")
        .build("c1_4_task_discussion_limit")
        .await
    else {
        return;
    };
    let app = &world.app;
    let laptop = &world.apps["laptop"];
    let call = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };

    let made = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Ship it", "clientRequestId": "limit-task"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let task = made["value"]["workItemId"]
        .as_str()
        .expect("task")
        .to_owned();
    let upload = call(
        "file_upload",
        json!({"taskId": task, "forComment": true, "name": "trace.log", "text": "trace"}),
    )
    .await;
    assert_eq!(upload["outcome"], "ok", "{upload}");
    let file = upload["value"]["fileId"].as_str().expect("file").to_owned();
    let commented = call(
        "task_comment",
        json!({"taskId": task, "text": "Trace attached", "clientRequestId": "limit-comment",
               "attachmentIds": [file]}),
    )
    .await;
    assert_eq!(commented["outcome"], "applied", "{commented}");
    let listed = call(
        "comment_list",
        json!({"thing": {"kind": "task", "id": task}}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let conversation = listed["value"]["conversationId"]
        .as_str()
        .expect("comments")
        .to_owned();
    let read = call("conversation_read", json!({"conversationId": conversation})).await;
    assert!(read.to_string().contains("Trace attached"), "{read}");
    let fetched = call("file_read", json!({"fileId": file})).await;
    assert_eq!(fetched["outcome"], "ok", "{fetched}");

    // Keep tasks from the person's apps: the same reads find nothing.
    sqlx::query(
        "UPDATE organization_memberships SET agent_limit = $1 WHERE organization_id = $2 AND actor_id = $3",
    )
    .bind(vec!["projects.view", "channels.post"])
    .bind(world.organization)
    .bind(laptop.person)
    .execute(world.pool())
    .await
    .expect("keep tasks from the apps");
    let listed = call(
        "comment_list",
        json!({"thing": {"kind": "task", "id": task}}),
    )
    .await;
    assert_ne!(listed["outcome"], "ok", "{listed}");
    let read = call("conversation_read", json!({"conversationId": conversation})).await;
    assert_ne!(read["outcome"], "ok", "{read}");
    assert!(!read.to_string().contains("Trace attached"), "{read}");
    let opened = call("conversation_open", json!({"conversationId": conversation})).await;
    assert_ne!(opened["outcome"], "ok", "{opened}");
    let fetched = call("file_read", json!({"fileId": file})).await;
    assert_ne!(fetched["outcome"], "ok", "{fetched}");
    let posted = call(
        "comment_create",
        json!({"thing": {"kind": "task", "id": task}, "text": "Still here?"}),
    )
    .await;
    assert_ne!(posted["outcome"], "ok", "{posted}");
    world.finish().await;
}
