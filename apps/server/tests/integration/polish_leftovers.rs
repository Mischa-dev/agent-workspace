//! The polish leftovers (brief 2026-10-28-keyboard-drag-polish) where a bug
//! would be silent: who may edit and delete a task comment, a deleted
//! comment's text surviving anywhere, reading a conversation leaving its
//! notifications unread, an unnamed thread keeping a removed message's
//! text as its name, and a remembered place pointing at a page someone can't
//! open.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::send,
    mcp_admin::{create_project, setup},
    message_edits::{add_person, create_channel, get, remove},
    notifications::{drain, inbox},
};

fn text(value: &str) -> Value {
    json!({"version": 1, "nodes": [{"type": "text", "text": value}]})
}

fn mention(actor: Uuid, value: &str) -> Value {
    json!({
        "version": 1,
        "nodes": [{"type": "text", "text": value}, {"type": "mention", "actorId": actor}]
    })
}

#[tokio::test]
async fn task_comments_are_edited_by_their_author_and_deleted_by_author_or_admin() {
    let Some(setup) = setup("comment_edits").await else {
        return;
    };
    let project = create_project(&setup, "CMT", "Comments").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let (ben, ben_browser) = add_person(&setup, &[&project], "ben").await;
    let base = format!("/api/v1/projects/{project}/work-items");
    let (status, task) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &base,
            Some("task"),
            json!({"title": "Pick colours"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let (status, created) = send(
        &setup.app,
        mara_browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment"),
            json!({"content": text("Teal, with the secret sk-test-123")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let comment = created["comment"]["id"]
        .as_str()
        .expect("comment")
        .to_owned();
    let path = format!("{base}/{task}/comments/{comment}");

    // Ben is a member too, but it isn't his comment.
    let (status, _) = send(
        &setup.app,
        ben_browser.request("PATCH", &path, None, json!({"content": text("Mine now")})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &setup.app,
        ben_browser.request("DELETE", &path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Mara edits hers; a stale revision is a conflict, and the newly
    // mentioned Ben hears about it.
    let (status, edited) = send(
        &setup.app,
        mara_browser.request(
            "PATCH",
            &path,
            None,
            json!({"content": mention(ben, "Teal after all, "), "expectedRevision": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["changed"], true);
    assert_eq!(edited["comment"]["revision"], 2);
    assert!(edited["comment"]["editedAt"].is_string(), "{edited}");
    let (status, _) = send(
        &setup.app,
        mara_browser.request(
            "PATCH",
            &path,
            None,
            json!({"content": text("Late edit"), "expectedRevision": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=mentions").await;
    assert_eq!(
        page["notifications"]
            .as_array()
            .expect("notifications")
            .len(),
        1,
        "{page}"
    );

    // Everyone who can see the task sees both versions.
    let (status, history) = get(&setup, &ben_browser, &format!("{path}/revisions")).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["revisions"].as_array().expect("revisions").len(), 2);
    assert!(history.to_string().contains("sk-test-123"));

    // The owner deletes it: the text and every version's text go, the
    // place in the history stays, and so does who deleted it.
    let (status, removed) = send(
        &setup.app,
        setup.browser.request("DELETE", &path, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["comment"]["plainText"], "");
    assert_eq!(
        removed["comment"]["removed"]["removedBy"]["handle"],
        "owner"
    );
    let (status, history) = get(&setup, &ben_browser, &format!("{path}/revisions")).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["removed"], true);
    let (status, timeline) = get(&setup, &ben_browser, &format!("{base}/{task}/timeline")).await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    for leaked in [history.to_string(), timeline.to_string()] {
        assert!(!leaked.contains("sk-test-123"), "{leaked}");
        assert!(!leaked.contains("Teal"), "{leaked}");
    }
    let left = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT (SELECT count(*) FROM work_item_comment_revisions
                WHERE comment_id = $1 AND plain_text IS NOT NULL)
             + (SELECT count(*) FROM idempotency_keys
                WHERE response_body::text LIKE '%sk-test-123%')
             + (SELECT count(*) FROM work_item_comment_mentions WHERE comment_id = $1)
        "#,
    )
    .bind(comment.parse::<Uuid>().expect("comment id"))
    .fetch_one(&setup.pool)
    .await
    .expect("leftovers");
    assert_eq!(left, 0, "a deleted comment's text or mentions survived");
    // Ben's mention went with it.
    drain(&setup).await;
    let page = inbox(&setup, &ben_browser, "?category=mentions").await;
    assert!(
        page["notifications"]
            .as_array()
            .expect("notifications")
            .is_empty(),
        "{page}"
    );

    // Nothing changes a deleted comment, and a retry of its creation
    // doesn't post the text again.
    let (status, _) = send(
        &setup.app,
        mara_browser.request("PATCH", &path, None, json!({"content": text("Back")})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &setup.app,
        mara_browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment"),
            json!({"content": text("Teal, with the secret sk-test-123")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Mara deletes her own; a member can't delete the owner's.
    let (status, own) = send(
        &setup.app,
        mara_browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment-two"),
            json!({"content": text("Second thoughts")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{own}");
    let own = own["comment"]["id"].as_str().expect("comment").to_owned();
    let (status, _) = send(
        &setup.app,
        mara_browser.request(
            "DELETE",
            &format!("{base}/{task}/comments/{own}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, owners) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("{base}/{task}/comments"),
            Some("comment-three"),
            json!({"content": text("Owner's note")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owners}");
    let owners = owners["comment"]["id"]
        .as_str()
        .expect("comment")
        .to_owned();
    let (status, _) = send(
        &setup.app,
        mara_browser.request(
            "DELETE",
            &format!("{base}/{task}/comments/{owners}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Someone outside the project sees nothing of it.
    let outsider_project = create_project(&setup, "OUT", "Elsewhere").await;
    let (_, outsider) = add_person(&setup, &[&outsider_project], "otto").await;
    let (status, _) = get(&setup, &outsider, &format!("{path}/revisions")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &setup.app,
        outsider.request(
            "DELETE",
            &format!("{base}/{task}/comments/{owners}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = mara;
}

#[tokio::test]
async fn reading_a_conversation_reads_its_notifications() {
    let Some(setup) = setup("seen_reads").await else {
        return;
    };
    let project = create_project(&setup, "SEE", "Seen").await;
    let general = create_channel(&setup, &project, "general").await;
    let (mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let (status, sent) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{general}/messages"),
            Some("m1"),
            json!({"content": mention(mara, "Have a look, ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let sequence = sent["entry"]["sequence"].as_i64().expect("sequence");
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(page["unreadCount"], 1, "{page}");

    // Mara opens the channel and sees the message: the mention is read.
    let (status, seen) = send(
        &setup.app,
        mara_browser.request(
            "POST",
            &format!("/api/v1/conversations/{general}/mark-seen"),
            None,
            json!({"highestSeenSequence": sequence}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(page["unreadCount"], 0, "{page}");
    assert_eq!(page["notifications"][0]["read"], true, "{page}");

    // A mention that arrives while she's reading it comes in read.
    let (status, sent) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{general}/messages"),
            Some("m2"),
            json!({"content": mention(mara, "And this, ")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let sequence = sent["entry"]["sequence"].as_i64().expect("sequence");
    let (status, _) = send(
        &setup.app,
        mara_browser.request(
            "POST",
            &format!("/api/v1/conversations/{general}/mark-seen"),
            None,
            json!({"highestSeenSequence": sequence}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "").await;
    assert_eq!(page["unreadCount"], 0, "{page}");
}

#[tokio::test]
async fn unnamed_threads_go_by_their_first_message_until_named() {
    let Some(setup) = setup("unnamed_threads").await else {
        return;
    };
    let project = create_project(&setup, "THR", "Threads").await;
    let general = create_channel(&setup, &project, "general").await;
    let start = |key: &'static str, body: Value| {
        let request = setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/threads"),
            Some(key),
            body,
        );
        let app = setup.app.clone();
        async move {
            let (status, thread) = send(&app, request).await;
            assert_eq!(status, StatusCode::CREATED, "{thread}");
            (
                thread["conversation"]["id"]
                    .as_str()
                    .expect("thread")
                    .to_owned(),
                thread["conversation"]["name"]
                    .as_str()
                    .expect("name")
                    .to_owned(),
            )
        }
    };
    let name_of = |thread: String| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT name FROM conversations WHERE id = $1")
                .bind(thread.parse::<Uuid>().expect("thread id"))
                .fetch_one(&pool)
                .await
                .expect("thread name")
        }
    };

    // Two unnamed threads at once don't collide.
    let (first, name) = start("t1", json!({})).await;
    assert_eq!(name, "New thread");
    let (second, _) = start("t2", json!({"name": "  "})).await;
    assert_ne!(first, second);

    // The first message names it.
    let (status, sent) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{first}/messages"),
            Some("first-message"),
            json!({"content": text("Where should the launch party be?\nIdeas welcome")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let message = sent["message"]["id"].as_str().expect("message").to_owned();
    assert_eq!(
        name_of(first.clone()).await,
        "Where should the launch party be?"
    );
    let (status, _) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{first}/messages"),
            Some("second-message"),
            json!({"content": text("Rooftop?")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        name_of(first.clone()).await,
        "Where should the launch party be?"
    );

    // Removing that message takes its text out of the name too.
    let (status, removed) = remove(&setup, &setup.browser, &first, &message).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(name_of(first.clone()).await, "Rooftop?");

    // A thread started from a message goes by that message.
    let origin = {
        let (status, sent) = send(
            &setup.app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/conversations/{general}/messages"),
                Some("origin"),
                json!({"content": text("Budget for the offsite")}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sent}");
        sent["message"]["id"].as_str().expect("message").to_owned()
    };
    let (from_origin, name) = start("t3", json!({"originMessageId": origin})).await;
    assert_eq!(name, "Budget for the offsite");
    let (status, removed) = remove(&setup, &setup.browser, &general, &origin).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(name_of(from_origin.clone()).await, "New thread");

    // Naming it ends that, and names are unique again.
    let (status, renamed) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/threads/{second}"),
            None,
            json!({"name": "Offsite"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    let (status, clash) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/threads/{from_origin}"),
            None,
            json!({"name": "offsite"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{clash}");
}

#[tokio::test]
async fn a_project_remembers_each_tab_and_page_but_not_an_archived_or_foreign_page() {
    let Some(setup) = setup("remembered_places").await else {
        return;
    };
    let project = create_project(&setup, "NAV", "Navigation").await;
    let other = create_project(&setup, "OTH", "Other").await;
    let page_in = |project: &str| {
        let request = setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/pages"),
            None,
            json!({"title": format!("Runbook {project}"), "body": "Steps"}),
        );
        async {
            let (status, page) = send(&setup.app, request).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            page["id"].as_str().expect("page").to_owned()
        }
    };
    let page = page_in(&project).await;
    let foreign_page = page_in(&other).await;
    let navigation = format!("/api/v1/projects/{project}/navigation");
    let remember = |location: Value, version: &Value| {
        setup.browser.request(
            "PUT",
            &navigation,
            None,
            json!({"location": location, "expectedVersion": version}),
        )
    };
    let (status, current) = get(&setup, &setup.browser, &navigation).await;
    assert_eq!(status, StatusCode::OK, "{current}");

    for (kind, path) in [
        ("milestones", "/milestones"),
        ("timeline", "/timeline"),
        ("planning", "/next"),
        ("pages", "/pages"),
        ("decisions", "/decisions"),
        ("runs", "/launches"),
    ] {
        let (_, current) = get(&setup, &setup.browser, &navigation).await;
        let (status, saved) = send(
            &setup.app,
            remember(json!({"kind": kind}), &current["version"]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{kind}: {saved}");
        assert_eq!(saved["href"], format!("/projects/{project}{path}"));
    }

    // A page in another project, or no page at all, isn't a place here.
    let (_, current) = get(&setup, &setup.browser, &navigation).await;
    let (status, _) = send(
        &setup.app,
        remember(
            json!({"kind": "page", "pageId": foreign_page}),
            &current["version"],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &setup.app,
        remember(json!({"kind": "page"}), &current["version"]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, saved) = send(
        &setup.app,
        remember(json!({"kind": "page", "pageId": page}), &current["version"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["location"]["pageId"], page.as_str());
    assert_eq!(saved["href"], format!("/projects/{project}/pages/{page}"));
    let (_, listed) = get(&setup, &setup.browser, "/api/v1/projects").await;
    let listed = listed["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|listed| listed["id"] == project.as_str())
        .expect("the project")
        .clone();
    assert_eq!(
        listed["navigation"]["href"],
        format!("/projects/{project}/pages/{page}")
    );

    // Once the page is archived, the project opens on its pages instead.
    let (status, archived) = send(
        &setup.app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}"),
            None,
            json!({"archived": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let (_, current) = get(&setup, &setup.browser, &navigation).await;
    assert_eq!(current["location"], json!({"kind": "pages"}));
    assert_eq!(current["href"], format!("/projects/{project}/pages"));
    let (status, _) = send(
        &setup.app,
        remember(json!({"kind": "page", "pageId": page}), &current["version"]),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
