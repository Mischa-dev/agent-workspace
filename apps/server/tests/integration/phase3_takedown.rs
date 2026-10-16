//! Phase 3 "done when", end to end: someone pastes a token in #general, an
//! agent asks for it to be taken down, and the owner, through a connected
//! app, removes it and acknowledges with ✅. The agent itself can't remove
//! it, even as a project owner, and afterwards the token is gone from every
//! surface for everyone.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    message_edits::{add_person, copies, count, create_channel, direct_space, get, set_role},
    message_reactions::{item, timeline_message},
};

#[tokio::test]
async fn an_owner_takes_down_a_pasted_token_through_a_connected_app() {
    let Some(setup) = setup("phase3_takedown").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = direct_space(&setup).await;
    let (mara_id, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let added = call(
        "project_member_set",
        json!({"projectId": work, "person": "@mara"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    // Even an agent with the owner role never moderates.
    set_role(&setup.pool, &work, mara_id, "owner").await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let (_, bob) = add_person(&setup, &[&work], "bob").await;
    let general = create_channel(&setup, &work, "general").await;

    // Bob pastes a token and asks Mara to deploy with it.
    let token = format!("ghp_{}", Uuid::now_v7().simple());
    let paste = json!({
        "content": {"version": 1, "nodes": [
            {"type": "text", "text": format!("deploy with {token} please ")},
            {"type": "mention", "actorId": mara_id}
        ]}
    });
    let (status, pasted) = send(
        app,
        bob.request(
            "POST",
            &format!("/api/v1/conversations/{general}/messages"),
            Some("bob-paste"),
            paste.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pasted}");
    let pasted_id = pasted["message"]["id"].as_str().expect("paste").to_owned();

    // Mara answers by asking for it to be taken down, and a thread is
    // started from the paste, named after its first line.
    let inbox = mara("inbox_list", json!({})).await;
    let request = inbox["value"]["requests"][0]["requestId"].clone();
    assert!(request.is_string(), "{inbox}");
    let claim = mara("reply_claim", json!({"requestId": request})).await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let asked = mara(
        "reply_send",
        json!({
            "requestId": request,
            "generation": claim["value"]["request"]["generation"],
            "text": "That message has a live token in it. Could an owner take it down? Bob, please rotate it."
        }),
    )
    .await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    let ask_id = asked["value"]["messageId"]
        .as_str()
        .expect("Mara's request")
        .to_owned();
    let thread = mara(
        "thread_start",
        json!({"conversationId": general, "fromMessageId": pasted_id}),
    )
    .await;
    assert_eq!(thread["outcome"], "ok", "{thread}");
    assert!(
        thread["value"]["name"]
            .as_str()
            .is_some_and(|name| name.contains(&token)),
        "the suggested name carries the token: {thread}"
    );
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();
    assert_eq!(
        claim["value"]["request"]["replyConversationId"],
        thread_id.as_str(),
        "Mara answered in the thread her claim started from the paste"
    );

    // Mara can't take it down herself, or edit it away.
    let refused = mara("message_delete", json!({"messageId": pasted_id})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(
        refused["message"],
        "Agents can only remove their own messages. Ask a project owner or admin to remove this one."
    );
    let refused = mara(
        "message_edit",
        json!({"messageId": pasted_id, "text": "deploy with the usual token"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM messages WHERE id = $1 AND removed_at IS NULL",
            &pasted_id
        )
        .await,
        1,
        "still there after the refusals"
    );

    // Codex, connected as the owner, removes Bob's paste and reacts ✅ to
    // Mara's request instead of posting an acknowledgement.
    let messages_before = count(
        &setup.pool,
        "SELECT count(*) FROM messages WHERE conversation_id = $1",
        &general,
    )
    .await;
    let removed = call("message_delete", json!({"messageId": pasted_id})).await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    assert_eq!(removed["value"]["messageId"], pasted_id.as_str());
    assert_eq!(removed["value"]["alreadyRemoved"], false, "{removed}");
    assert!(
        removed["value"]["note"]
            .as_str()
            .is_some_and(|note| note.contains("rotate the secret")),
        "{removed}"
    );
    assert!(!removed.to_string().contains(&token), "{removed}");
    let reacted = call("message_react", json!({"messageId": ask_id, "emoji": "✅"})).await;
    assert_eq!(reacted["outcome"], "ok", "{reacted}");
    assert_eq!(reacted["value"]["name"], "check", "{reacted}");
    assert_eq!(reacted["value"]["changed"], true, "{reacted}");
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM messages WHERE conversation_id = $1",
            &general
        )
        .await,
        messages_before,
        "the removal and the reaction post nothing"
    );

    // Everyone sees the ✅ on Mara's request and a tombstone where the paste
    // was, never the token.
    for (token_, session_) in [
        (codex.as_str(), session.as_str()),
        (mara_token.as_str(), mara_session.as_str()),
    ] {
        let read = tool(
            app,
            token_,
            session_,
            "conversation_read",
            json!({"conversationId": general}),
        )
        .await;
        assert_eq!(read["outcome"], "ok", "{read}");
        assert!(!read.to_string().contains(&token), "{read}");
        let tombstone = item(&read, &pasted_id);
        assert_eq!(tombstone["removed"], true, "{tombstone}");
        assert_eq!(tombstone["text"], "", "{tombstone}");
        // A tag in a channel is answered in the thread started from it.
        let thread_read = tool(
            app,
            token_,
            session_,
            "conversation_read",
            json!({"conversationId": thread_id}),
        )
        .await;
        let ack = &item(&thread_read, &ask_id)["reactions"];
        assert_eq!(ack[0]["name"], "check", "{ack}");
        assert_eq!(ack[0]["count"], 1, "{ack}");
        assert!(
            ack[0]["people"][0]
                .as_str()
                .is_some_and(|who| who.contains("@owner")),
            "{ack}"
        );
        for (name, arguments) in [
            ("conversation_open", json!({"conversationId": general})),
            ("conversation_read", json!({"conversationId": thread_id})),
            ("conversation_list", json!({"projectId": work})),
            ("search", json!({"query": token})),
            ("briefing", json!({})),
            ("message_history", json!({"messageId": pasted_id})),
        ] {
            let seen = tool(app, token_, session_, name, arguments).await;
            assert_eq!(seen["outcome"], "ok", "{name}: {seen}");
            assert!(!seen.to_string().contains(&token), "{name}: {seen}");
        }
        let found = tool(app, token_, session_, "search", json!({"query": token})).await;
        assert_eq!(found["value"]["results"], json!([]), "{found}");
    }
    let inbox = mara("inbox_list", json!({})).await;
    assert_eq!(inbox["outcome"], "ok", "{inbox}");
    assert!(!inbox.to_string().contains(&token), "{inbox}");
    // The tombstone takes no new reactions.
    let frozen = mara(
        "message_react",
        json!({"messageId": pasted_id, "emoji": "eyes"}),
    )
    .await;
    assert_ne!(frozen["outcome"], "ok", "{frozen}");

    // Bob, on the web: the timeline, the thread, search and a replay of his
    // own send.
    let shown = timeline_message(&setup, &bob, &general, &pasted_id).await;
    assert!(!shown.to_string().contains(&token), "{shown}");
    let ack = timeline_message(&setup, &bob, &thread_id, &ask_id).await;
    assert_eq!(ack["reactions"][0]["name"], "check", "{ack}");
    for uri in [
        format!("/api/v1/conversations/{general}/timeline"),
        format!("/api/v1/conversations/{thread_id}/timeline"),
        format!("/api/v1/conversations/{general}/pins"),
        format!("/api/v1/search?q={token}"),
        format!("/api/v1/projects/{work}/channels"),
        "/api/v1/direct".to_owned(),
    ] {
        let (status, body) = get(&setup, &bob, &uri).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::NOT_FOUND,
            "{uri}: {status} {body}"
        );
        assert!(!body.to_string().contains(&token), "{uri}: {body}");
    }
    let (_, replay) = send(
        app,
        bob.request(
            "POST",
            &format!("/api/v1/conversations/{general}/messages"),
            Some("bob-paste"),
            paste,
        ),
    )
    .await;
    assert!(!replay.to_string().contains(&token), "{replay}");
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM messages WHERE conversation_id = $1",
            &general
        )
        .await,
        messages_before,
        "a replay never brings it back"
    );

    // No copy is left in any table.
    for table in [
        "messages",
        "message_revisions",
        "message_mentions",
        "idempotency_keys",
        "outbox_events",
        "audit_events",
        "external_agent_events",
        "reply_requests",
        "conversation_pins",
        "work_item_links",
        "conversations",
        "project_decisions",
        "message_reactions",
    ] {
        assert_eq!(
            copies(&setup.pool, table, &token).await,
            0,
            "{table} keeps a copy"
        );
    }
}
