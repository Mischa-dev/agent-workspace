//! K3.12: the link store, copied and mirrored (task dependencies and
//! relations are written to it directly since K3.16). The silent, costly failures:
//! a link made through one of today's tools that never reaches `links` (or
//! arrives with the wrong kind, direction or author), one taken away that
//! stays, and a copy that loses or doubles what the old stores hold.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
};

type Link = (Uuid, Uuid, String, String);

/// Every link as (from, to, kind, origin).
async fn links(pool: &PgPool) -> BTreeSet<Link> {
    sqlx::query_as::<_, Link>("SELECT from_thing_id, to_thing_id, kind, origin FROM links")
        .fetch_all(pool)
        .await
        .expect("links")
        .into_iter()
        .collect()
}

async fn author(pool: &PgPool, from: &str, to: &str, kind: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT created_by_account_id FROM links \
         WHERE from_thing_id = $1::uuid AND to_thing_id = $2::uuid AND kind = $3",
    )
    .bind(from)
    .bind(to)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("{kind} link {from} -> {to}: {error}"))
}

fn link(from: &str, to: &str, kind: &str, origin: &str) -> Link {
    (
        from.parse().expect("from"),
        to.parse().expect("to"),
        kind.to_owned(),
        origin.to_owned(),
    )
}

/// The id a tool's result names.
fn id_of(result: &Value) -> String {
    if let Some(id) = result["workItemId"].as_str() {
        return id.to_owned();
    }
    let value = &result["value"];
    for path in [
        "/id",
        "/messageId",
        "/workItemId",
        "/whiteboard/id",
        "/page/id",
        "/decision/id",
        "/conversation/id",
    ] {
        if let Some(id) = value.pointer(path).and_then(Value::as_str) {
            return id.to_owned();
        }
    }
    panic!("no id in {result}")
}

#[tokio::test]
async fn every_old_link_reaches_the_store_and_leaves_it() {
    let Some(setup) = setup("k3_12_links").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move {
            let result = tool(app, &codex, &session, name, arguments).await;
            assert!(
                matches!(result["outcome"].as_str(), Some("ok" | "applied")),
                "{name}: {result}"
            );
            result
        }
    };

    let task =
        |title: &'static str| call("task_create", json!({"projectId": "WORK", "title": title}));
    let gate = id_of(&task("Fix the gate").await);
    let hinge = id_of(&task("Order a hinge").await);
    let paint = id_of(&task("Paint the gate").await);
    let me = sqlx::query_scalar::<_, Uuid>(
        "SELECT created_by_actor_id FROM work_items WHERE id = $1::uuid",
    )
    .bind(&gate)
    .fetch_one(pool)
    .await
    .expect("author");
    let page = id_of(
        &call(
            "page_create",
            json!({"projectId": "WORK", "title": "Gate notes", "body": "Hinges."}),
        )
        .await,
    );
    let channel = id_of(
        &call(
            "channel_create",
            json!({"projectId": "WORK", "name": "gate"}),
        )
        .await,
    );
    let say = |text: String| {
        call(
            "message_send",
            json!({"conversationId": channel, "text": text}),
        )
    };
    let asked = id_of(&say("Who has the key?".to_owned()).await);
    let decided = id_of(&say("Green it is.".to_owned()).await);
    let thread = id_of(
        &call(
            "thread_start",
            json!({"conversationId": channel, "fromMessageId": asked}),
        )
        .await,
    );
    let decision = id_of(
        &call(
            "decision_record",
            json!({"title": "Paint it green", "fromMessageId": decided, "clientRequestId": "k3-12-decision"}),
        )
        .await,
    );
    let board = id_of(
        &call(
            "whiteboard_create",
            json!({"projectId": "WORK", "title": "Gate sketch"}),
        )
        .await,
    );
    let copy = id_of(&call("whiteboard_duplicate", json!({"whiteboardId": board})).await);

    // Through today's tools.
    call("task_link", json!({"taskId": gate, "blockedBy": [hinge]})).await;
    call("task_link", json!({"taskId": gate, "otherTaskId": paint})).await;
    call(
        "task_link",
        json!({"taskId": paint, "otherTaskId": hinge, "kind": "duplicates"}),
    )
    .await;
    for (field, target) in [
        ("pageId", &page),
        ("decisionId", &decision),
        ("conversationId", &thread),
        ("messageId", &asked),
        ("whiteboardId", &board),
    ] {
        call("task_link", json!({"taskId": gate, field: target})).await;
    }
    call(
        "whiteboard_link",
        json!({"whiteboardId": board, "targetKind": "page", "targetId": page}),
    )
    .await;
    let url = format!("/projects/{work}/work-items/{hinge}");
    let mentioning = id_of(&say(format!("See {url} first.")).await);
    call(
        "task_comment_create",
        json!({"taskId": paint, "text": format!("Waits on {url}"), "clientRequestId": "k3-12-comment"}),
    )
    .await;
    let comment = sqlx::query_scalar::<_, Uuid>(
        "SELECT message.id FROM messages message \
         JOIN conversations comments ON comments.id = message.conversation_id \
         WHERE comments.kind = 'comments' AND comments.subject_id = $1::uuid",
    )
    .bind(&paint)
    .fetch_one(pool)
    .await
    .expect("comment")
    .to_string();
    call(
        "collection_create",
        json!({"projectId": "WORK", "name": "Suppliers", "fields": [{"name": "Fix", "type": "link", "target": "task"}]}),
    )
    .await;
    let written = call(
        "record_create_many",
        json!({"projectId": "WORK", "collection": "Suppliers", "records": [{"title": "Hinge Co", "fields": {"Fix": gate}}], "clientRequestId": "k3-12-record"}),
    )
    .await;
    let record = &written["value"]["records"][0];
    let record_id = record["id"].as_str().expect("record").to_owned();

    let expected = [
        link(&hinge, &gate, "blocks", "explicit"),
        link(&gate, &paint, "relates_to", "explicit"),
        link(&paint, &hinge, "duplicates", "explicit"),
        link(&gate, &page, "about", "explicit"),
        link(&gate, &decision, "about", "explicit"),
        link(&gate, &thread, "about", "explicit"),
        link(&gate, &asked, "made_from", "explicit"),
        link(&board, &gate, "about", "explicit"),
        link(&board, &page, "about", "explicit"),
        link(&mentioning, &hinge, "mentions", "text"),
        link(&paint, &hinge, "mentions", "text"),
        link(&record_id, &gate, "relates_to", "field"),
        link(&thread, &asked, "made_from", "derived"),
        link(&decision, &decided, "made_from", "derived"),
        link(&copy, &board, "made_from", "derived"),
    ];
    let stored = links(pool).await;
    for want in &expected {
        assert!(stored.contains(want), "missing {want:?} in {stored:#?}");
    }
    for (from, to, kind) in [
        (&hinge, &gate, "blocks"),
        (&gate, &paint, "relates_to"),
        (&gate, &page, "about"),
        (&board, &page, "about"),
        (&mentioning, &hinge, "mentions"),
        (&record_id, &gate, "relates_to"),
    ] {
        assert_eq!(author(pool, from, to, kind).await, Some(me), "{kind}");
    }
    let text = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT source_kind, source_id FROM links \
         WHERE origin = 'text' AND from_thing_id = $1::uuid",
    )
    .bind(&paint)
    .fetch_one(pool)
    .await
    .expect("comment link");
    assert_eq!(text, ("message".to_owned(), comment.parse().expect("id")));

    // A fresh copy from the old stores gives the same set. Explicit links
    // are kept in the store alone (K3.16, K3.23), so they stay.
    let cleared = sqlx::query("DELETE FROM links WHERE origin <> 'explicit'")
        .execute(pool)
        .await
        .expect("clear")
        .rows_affected();
    let copy_all = || async {
        sqlx::query_scalar::<_, i64>("SELECT links_copy_all()")
            .fetch_one(pool)
            .await
            .expect("copy")
    };
    let added = copy_all().await;
    assert_eq!(links(pool).await, stored);
    assert_eq!(u64::try_from(added).expect("count"), cleared);
    assert_eq!(
        stored.len() - usize::try_from(cleared).expect("cleared"),
        stored.iter().filter(|link| link.3 == "explicit").count()
    );
    assert_eq!(copy_all().await, 0, "a second copy adds nothing");

    // Taking each away takes its link.
    call(
        "task_link",
        json!({"taskId": gate, "blockedBy": [hinge], "remove": true}),
    )
    .await;
    call(
        "task_link",
        json!({"taskId": gate, "otherTaskId": paint, "remove": true}),
    )
    .await;
    call(
        "task_link",
        json!({"taskId": paint, "otherTaskId": hinge, "kind": "duplicates", "remove": true}),
    )
    .await;
    for (field, target) in [
        ("pageId", &page),
        ("decisionId", &decision),
        ("conversationId", &thread),
        ("messageId", &asked),
        ("whiteboardId", &board),
    ] {
        call(
            "task_link",
            json!({"taskId": gate, field: target, "remove": true}),
        )
        .await;
    }
    call(
        "whiteboard_link",
        json!({"whiteboardId": board, "targetKind": "page", "targetId": page, "remove": true}),
    )
    .await;
    call("message_delete", json!({"messageId": mentioning})).await;
    call(
        "task_comment_delete",
        json!({"taskId": paint, "commentId": comment}),
    )
    .await;
    call(
        "record_update_many",
        json!({"projectId": "WORK", "collection": "Suppliers", "records": [{"record": record_id, "expectedVersion": record["version"], "fields": {"Fix": null}}], "clientRequestId": "k3-12-record-clear"}),
    )
    .await;
    let left = links(pool).await;
    assert_eq!(
        left,
        BTreeSet::from([
            link(&thread, &asked, "made_from", "derived"),
            link(&decision, &decided, "made_from", "derived"),
            link(&copy, &board, "made_from", "derived"),
        ]),
        "only the made-from links stay"
    );

    // A made-from link goes with what was made.
    sqlx::query("DELETE FROM project_decisions WHERE id = $1::uuid")
        .bind(&decision)
        .execute(pool)
        .await
        .expect("delete decision");
    assert!(
        !links(pool)
            .await
            .iter()
            .any(|(from, ..)| from.to_string() == decision)
    );
}
