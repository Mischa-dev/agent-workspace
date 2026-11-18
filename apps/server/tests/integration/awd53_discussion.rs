//! AWD-53 criteria 6 and 7: an agent's edits to someone else's page arrive
//! as suggestions, and pages carry comment threads anchored to their words.
//! The silent, costly failures: an agent changing a person's page without
//! asking, an accepted suggestion undoing text written since it was made,
//! an agent accepting its own suggestion on a person's page, someone
//! editing or deleting another person's comment, and a person outside the
//! project reading or writing any of it.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::add_person,
    support::agents_suggest,
};

const BODY: &str = "# Plan\n\nFirst we sketch.\n\n## Risks\n\n- slow CI\n\nShip it.\n";

#[tokio::test]
async fn agents_suggest_on_other_peoples_pages_and_comments_follow_the_words() {
    let Some(setup) = setup("awd53_discussion").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (status, page) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            None,
            json!({"title": "Plan", "body": BODY}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let page = page["id"].as_str().expect("page").to_owned();
    let (mara, mara_token) =
        create_agent(app, &setup.browser, &space(&setup).await, "Mara", "mara").await;
    add_member(&setup, &work, mara, "member").await;
    // Mara's role is the "suggests" copy of Member: she suggests on pages
    // others made.
    agents_suggest(&setup.pool).await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let page_body = || async {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT body, version FROM project_pages WHERE id = $1::uuid",
        )
        .bind(&page)
        .fetch_one(&setup.pool)
        .await
        .expect("page")
    };
    let discussion = || async {
        let (status, body) = send(
            app,
            setup.browser.request(
                "GET",
                &format!("/api/v1/pages/{page}/discussion"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    };

    // The page is the owner's: Mara's edit is kept as a suggestion and the
    // page doesn't change.
    let read = call("page_blocks", json!({"pageId": page})).await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let block = |position: usize| read["value"]["blocks"][position].clone();
    let edited = call(
        "page_edit",
        json!({"pageId": page, "note": "Fix the risk", "edits": [{
            "op": "replace", "blockId": block(3)["id"], "hash": block(3)["hash"],
            "markdown": "- slow CI (fixed)"
        }]}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    assert_eq!(
        edited["value"]["suggestion"]["status"], "pending",
        "{edited}"
    );
    let first = edited["value"]["suggestion"]["suggestionId"]
        .as_str()
        .expect("suggestion")
        .to_owned();
    assert_eq!(page_body().await, (BODY.to_owned(), 1));

    // A whole new text from page_write is a suggestion too, of only the
    // blocks it changes.
    let rewritten = call(
        "page_write",
        json!({"pageId": page, "body": BODY.replace("Ship it.", "Ship it on Friday.")}),
    )
    .await;
    assert_eq!(rewritten["outcome"], "ok", "{rewritten}");
    assert_eq!(rewritten["value"]["version"], 1, "{rewritten}");
    assert_eq!(rewritten["value"]["pendingSuggestions"], 2, "{rewritten}");
    let second = rewritten["value"]["suggestion"]["suggestionId"]
        .as_str()
        .expect("suggestion")
        .to_owned();
    let listed = call("page_suggestions", json!({"pageId": page})).await;
    let edits = &listed["value"]["suggestions"][1]["edits"];
    assert_eq!(edits.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(edits[0]["op"], "replace", "{listed}");
    assert_eq!(edits[0]["before"], "Ship it.", "{listed}");

    // Mara can't decide her own suggestion on someone else's page.
    let self_accept = call(
        "page_suggestion_decide",
        json!({"pageId": page, "suggestionId": first, "decision": "accept"}),
    )
    .await;
    assert_eq!(self_accept["outcome"], "permission_denied", "{self_accept}");

    // Both are proposals (C1.7): Mara lists her own, and deciding through
    // proposal_decide follows the same rule.
    let mine = call("proposal_list", json!({})).await;
    assert_eq!(mine["outcome"], "ok", "{mine}");
    let proposals = mine["value"]["proposals"].as_array().expect("proposals");
    assert_eq!(proposals.len(), 2, "{mine}");
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal["thingKind"] == "page"
                && proposal["thingId"] == page.as_str()
                && proposal["operation"] == "page_edit"
                && proposal["status"] == "pending"),
        "{mine}"
    );
    let self_accept = call(
        "proposal_decide",
        json!({"proposalId": first, "decision": "accept", "clientRequestId": "self-accept"}),
    )
    .await;
    assert_eq!(self_accept["outcome"], "permission_denied", "{self_accept}");

    // The owner accepts the first: the page changes as Mara's edit, with
    // the owner named in the note.
    let (status, accepted) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/pages/{page}/suggestions/{first}/decision"),
            None,
            json!({"decision": "accept"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["status"], "accepted");
    let (body, version) = page_body().await;
    assert_eq!(version, 2);
    assert!(body.contains("- slow CI (fixed)"), "{body}");
    assert!(body.contains("Ship it.\n"), "{body}");
    let (author, note) = sqlx::query_as::<_, (uuid::Uuid, String)>(
        "SELECT actor_id, note FROM project_page_revisions WHERE page_id = $1::uuid AND version = 2",
    )
    .bind(&page)
    .fetch_one(&setup.pool)
    .await
    .expect("revision");
    assert_eq!(author, mara, "the accepted edit is the agent's");
    assert!(
        note.starts_with("Fix the risk (Suggestion accepted by"),
        "{note}"
    );
    let applied = sqlx::query_scalar::<_, bool>(
        "SELECT proposal.applied_change_set_id = change.id \
         FROM proposals proposal JOIN document_change_sets change \
           ON change.proposal_id = proposal.id \
         WHERE proposal.id = $1::uuid",
    )
    .bind(&first)
    .fetch_one(&setup.pool)
    .await
    .expect("accepting made a change set");
    assert!(applied);

    // The owner edits the last paragraph by hand; the second suggestion,
    // made against the old text, no longer applies and stays pending.
    let (status, saved) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}"),
            None,
            json!({"body": body.replace("Ship it.", "Ship it soon."), "expectedVersion": 2}),
        ),
    )
    .await;
    assert!(status.is_success(), "{saved}");
    let (status, stale) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/pages/{page}/suggestions/{second}/decision"),
            None,
            json!({"decision": "accept"}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status} {stale}");
    let (body, _) = page_body().await;
    assert!(body.contains("Ship it soon."), "{body}");
    assert!(!body.contains("Friday"), "{body}");
    let withdrawn = call(
        "proposal_decide",
        json!({"proposalId": second, "decision": "withdraw", "clientRequestId": "withdraw-second"}),
    )
    .await;
    assert_eq!(withdrawn["value"]["status"], "withdrawn", "{withdrawn}");
    assert_eq!(withdrawn["value"]["thingId"], page.as_str(), "{withdrawn}");
    let pending = discussion().await;
    assert!(
        pending["suggestions"]
            .as_array()
            .expect("suggestions")
            .iter()
            .all(|suggestion| suggestion["status"] != "pending"),
        "{pending}"
    );

    // Mara's own page changes directly.
    let own = call(
        "page_write",
        json!({"projectId": "WORK", "title": "Mara's notes", "body": "One"}),
    )
    .await;
    let own_page = own["value"]["id"].as_str().expect("page").to_owned();
    let appended = call(
        "page_edit",
        json!({"pageId": own_page, "edits": [{"op": "append", "markdown": "Two"}]}),
    )
    .await;
    assert!(appended["value"]["suggestion"].is_null(), "{appended}");
    assert_eq!(appended["value"]["version"], 2, "{appended}");

    // Change sets (C1.9): the append is one change Mara can undo, and the
    // undo is a change she can undo in turn.
    let own_body = || async {
        sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1::uuid")
            .bind(&own_page)
            .fetch_one(&setup.pool)
            .await
            .expect("own page")
    };
    let changes = call(
        "change_list",
        json!({"thingKind": "page", "thingId": own_page}),
    )
    .await;
    assert_eq!(changes["outcome"], "ok", "{changes}");
    let latest = changes["value"]["changes"][0].clone();
    assert_eq!(latest["versionTo"], 2, "{changes}");
    assert_eq!(latest["undoable"], true, "{changes}");
    let undone = call(
        "change_undo",
        json!({"changeSetId": latest["id"], "clientRequestId": "undo-append"}),
    )
    .await;
    assert_eq!(undone["outcome"], "ok", "{undone}");
    assert_eq!(undone["value"]["version"], 3, "{undone}");
    assert!(!own_body().await.contains("Two"));
    let again = call(
        "change_undo",
        json!({"changeSetId": latest["id"], "clientRequestId": "undo-append-again"}),
    )
    .await;
    assert_eq!(again["outcome"], "conflict", "{again}");
    let redone = call(
        "change_undo",
        json!({"changeSetId": undone["value"]["change"]["id"], "clientRequestId": "undo-the-undo"}),
    )
    .await;
    assert_eq!(redone["outcome"], "ok", "{redone}");
    assert!(own_body().await.contains("Two"));
    // On the owner's page Mara only suggests, so she can't undo there.
    let theirs = call("change_list", json!({"thingKind": "page", "thingId": page})).await;
    let theirs = theirs["value"]["changes"][0]["id"].clone();
    assert!(theirs.is_string(), "the owner's edits are change sets");
    let denied = call(
        "change_undo",
        json!({"changeSetId": theirs, "clientRequestId": "undo-theirs"}),
    )
    .await;
    assert_eq!(denied["outcome"], "permission_denied", "{denied}");

    // Comments: Mara quotes words in a block; quoting words that aren't
    // there is refused.
    let reread = call("page_blocks", json!({"pageId": page})).await;
    let sketch = reread["value"]["blocks"][1]["id"].clone();
    let missing = call(
        "page_comment",
        json!({"pageId": page, "blockId": sketch, "quote": "we paint", "text": "Hm"}),
    )
    .await;
    assert_eq!(missing["outcome"], "invalid_request", "{missing}");
    let commented = call(
        "page_comment",
        json!({"pageId": page, "blockId": sketch, "quote": "we sketch", "text": "Sketch where?"}),
    )
    .await;
    assert_eq!(commented["outcome"], "ok", "{commented}");
    let thread = commented["value"]["threadId"]
        .as_str()
        .expect("thread")
        .to_owned();
    agent_workspace_server::events::catch_up(&setup.pool)
        .await
        .expect("catch up with the log");
    let notified = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE kind = 'thing_comment' AND detail->>'subjectId' = $1",
    )
    .bind(&page)
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(notified, 1, "the page's maker hears about it");

    // The owner replies and resolves it; nobody edits or deletes someone
    // else's comment.
    let (status, reply) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/pages/{page}/comments"),
            None,
            json!({"body": "On the whiteboard", "replyTo": thread}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, resolved) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}/comments/{thread}"),
            None,
            json!({"resolved": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}/comments/{thread}"),
            None,
            json!({"body": "Rewritten by someone else"}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "{status} {refused}");
    let open = call("page_comments", json!({"pageId": page})).await;
    assert_eq!(open["value"]["threads"], json!([]), "{open}");
    let all = call(
        "page_comments",
        json!({"pageId": page, "includeResolved": true}),
    )
    .await;
    let threads = &all["value"]["threads"];
    assert_eq!(threads[0]["resolved"], true, "{all}");
    assert_eq!(threads[0]["anchorFound"], true, "{all}");
    assert_eq!(
        threads[0]["comments"][1]["text"], "On the whiteboard",
        "{all}"
    );

    // Mara replies, which reopens it. When the quoted words are edited
    // away, the thread stays, marked as no longer found.
    let reopened = call(
        "page_comment",
        json!({"pageId": page, "replyTo": thread, "text": "Thanks"}),
    )
    .await;
    assert_eq!(reopened["outcome"], "ok", "{reopened}");
    let (body, version) = page_body().await;
    let (status, saved) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!("/api/v1/pages/{page}"),
            None,
            json!({"body": body.replace("First we sketch.", "First we draw."), "expectedVersion": version}),
        ),
    )
    .await;
    assert!(status.is_success(), "{saved}");
    let after = discussion().await;
    assert_eq!(after["threads"][0]["resolved"], false, "{after}");
    assert_eq!(after["threads"][0]["anchorFound"], false, "{after}");

    // Mara deletes her own comment; the thread keeps its replies.
    let first_comment = after["threads"][0]["comments"][0]["id"].clone();
    let deleted = call(
        "page_comment_update",
        json!({"pageId": page, "commentId": first_comment, "delete": true}),
    )
    .await;
    assert_eq!(deleted["outcome"], "ok", "{deleted}");
    let after = discussion().await;
    assert_eq!(
        after["threads"][0]["comments"][0]["deleted"], true,
        "{after}"
    );
    assert_eq!(after["threads"][0]["comments"][0]["body"], "", "{after}");
    // Its words leave the inbox it reached, too.
    let copies = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE detail::text LIKE '%Sketch where%'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(
        copies, 0,
        "a deleted comment's words stay in no notification"
    );
    assert_eq!(
        after["threads"][0]["comments"].as_array().map(Vec::len),
        Some(3)
    );
    // The thread still resolves through its first comment, deleted or not,
    // but the deleted comment itself can't be edited.
    let (status, resolved) = send(
        app,
        setup.browser.request(
            "PATCH",
            &format!(
                "/api/v1/pages/{page}/comments/{}",
                first_comment.as_str().expect("id")
            ),
            None,
            json!({"resolved": true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(discussion().await["threads"][0]["resolved"], true);
    let edited = call(
        "page_comment_update",
        json!({"pageId": page, "commentId": first_comment, "text": "Back again"}),
    )
    .await;
    assert_eq!(edited["outcome"], "not_found", "{edited}");

    // Someone outside the project sees and changes nothing.
    let other = create_project(&setup, "SIDE", "Side").await;
    let (_, stranger) = add_person(&setup, &[&other], "stranger").await;
    let (status, _) = send(
        app,
        stranger.request(
            "GET",
            &format!("/api/v1/pages/{page}/discussion"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        app,
        stranger.request(
            "POST",
            &format!("/api/v1/pages/{page}/comments"),
            None,
            json!({"body": "Hi"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// C1.22: a comment asks for a change, and the change answers in its
/// thread: linked there with Undo, and only to a thread on that page.
#[tokio::test]
async fn a_change_a_comment_asked_for_shows_in_its_thread_with_undo() {
    let Some(setup) = setup("c122_ask_in_comment").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let new_page = |title: &'static str| {
        let work = work.clone();
        async move {
            let (status, page) = send(
                app,
                setup.browser.request(
                    "POST",
                    &format!("/api/v1/projects/{work}/pages"),
                    None,
                    json!({"title": title, "body": BODY}),
                ),
            )
            .await;
            assert!(status.is_success(), "{page}");
            page["id"].as_str().expect("page").to_owned()
        }
    };
    let page = new_page("Plan").await;
    let other = new_page("Other").await;
    let ask = |page: String| async move {
        let (status, asked) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/pages/{page}/comments"),
                None,
                json!({"body": "Can someone make the risks clearer?"}),
            ),
        )
        .await;
        assert!(status.is_success(), "{asked}");
    };
    ask(page.clone()).await;
    ask(other.clone()).await;
    let discussion = |page: String| async move {
        let (status, body) = send(
            app,
            setup.browser.request(
                "GET",
                &format!("/api/v1/pages/{page}/discussion"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    };
    let thread = discussion(page.clone()).await["threads"][0]["id"]
        .as_str()
        .expect("thread")
        .to_owned();
    let elsewhere = discussion(other.clone()).await["threads"][0]["id"]
        .as_str()
        .expect("thread")
        .to_owned();

    let helper = super::agent_interface::connect_app(app, &setup.browser, "Helper", "full").await;
    let session = initialize(app, &helper).await.expect("Helper MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = helper.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let read = call("page_blocks", json!({"pageId": page})).await;
    let risk = read["value"]["blocks"][3].clone();
    let edit = |thread: String| {
        let page = page.clone();
        let risk = risk.clone();
        async move {
            call(
                "page_edit",
                json!({"pageId": page, "threadId": thread, "edits": [{
                    "op": "replace", "blockId": risk["id"], "hash": risk["hash"],
                    "markdown": "- slow CI: builds take 40 minutes"
                }]}),
            )
            .await
        }
    };

    // A thread on another page isn't this page's.
    let wrong = edit(elsewhere).await;
    assert_eq!(wrong["outcome"], "not_found", "{wrong}");

    let edited = edit(thread.clone()).await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let shown = discussion(page.clone()).await;
    let changes = &shown["threads"][0]["changes"];
    assert_eq!(changes.as_array().map(Vec::len), Some(1), "{shown}");
    assert_eq!(changes[0]["undone"], false, "{shown}");
    let change_set = changes[0]["changeSetId"]
        .as_str()
        .expect("change set")
        .to_owned();
    assert_eq!(
        discussion(other.clone()).await["threads"][0].get("changes"),
        None
    );

    // Undo from the thread puts the words back.
    let undone = call("change_undo", json!({"changeSetId": change_set})).await;
    assert_eq!(undone["outcome"], "ok", "{undone}");
    let shown = discussion(page.clone()).await;
    assert_eq!(shown["threads"][0]["changes"][0]["undone"], true, "{shown}");
    let body =
        sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1::uuid")
            .bind(&page)
            .fetch_one(&setup.pool)
            .await
            .expect("page");
    assert!(body.contains("- slow CI\n"), "{body}");
}
