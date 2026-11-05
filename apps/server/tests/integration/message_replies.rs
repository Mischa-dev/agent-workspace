//! AWD-70: replying to one message. A reply quotes the message it answers,
//! read live, and only ever from its own conversation. The costly failure is
//! silent: a reply that carries the text of a message from a private group
//! into a channel where people who can't see the group read it.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, initialize, tool},
    mcp_admin::create_project,
    message_edits::{add_person, create_channel, direct_space, get, say, text},
    w9_capacity_and_push::{call, fixture},
};

fn reply(content: &str, to: &str) -> Value {
    json!({ "content": text(content), "replyToMessageId": to })
}

/// How many messages in `conversation` say exactly `words`.
async fn sent_in(pool: &sqlx::PgPool, conversation: Uuid, words: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1 AND plain_text = $2",
    )
    .bind(conversation)
    .bind(words)
    .fetch_one(pool)
    .await
    .expect("count")
}

/// The `replyTo` of the message saying `words` in a timeline.
fn quote_on(timeline: &Value, words: &str) -> Value {
    timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["plainText"] == words)
        .map(|entry| entry["message"]["replyTo"].clone())
        .unwrap_or_else(|| panic!("no message saying {words}: {timeline}"))
}

#[tokio::test]
async fn a_reply_quotes_only_its_own_conversation_and_never_leaks_a_private_one() {
    let Some(fixture) = fixture("message_replies").await else {
        return;
    };
    let setup = &fixture.setup;
    let pool = &setup.pool;
    let project = create_project(setup, "RPL", "Replies").await;
    let space = direct_space(setup).await;
    let channel = create_channel(setup, &project, "general").await;
    let (mara, mara_browser) = add_person(setup, &[&project, &space], "mara").await;
    let (_, pat_browser) = add_person(setup, &[&project, &space], "pat").await;
    let (status, group) = call(
        setup,
        &setup.browser,
        "POST",
        "/api/v1/direct/groups",
        Some("reply-group"),
        json!({ "memberIds": [mara] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let secret = say(setup, &setup.browser, &group, "s1", text("Secret plan")).await;
    let open = say(setup, &setup.browser, &channel, "o1", text("Lunch at noon")).await;
    let send = |browser: &Browser, into: &str, key: &str, body: Value| {
        let browser = Browser {
            cookie: browser.cookie.clone(),
            csrf: browser.csrf.clone(),
        };
        let uri = format!("/api/v1/channels/{into}/messages");
        let key = key.to_owned();
        async move { call(setup, &browser, "POST", &uri, Some(&key), body).await }
    };
    let channel_id = channel.parse::<Uuid>().expect("channel");

    // Into the channel, a reply to the group's message is refused, for
    // someone outside the group and for its owner alike, and says nothing
    // about the original.
    for (browser, key) in [(&pat_browser, "r1"), (&setup.browser, "r2")] {
        let (status, body) = send(browser, &channel, key, reply("Agreed", &secret)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(!body.to_string().contains("Secret plan"), "{body}");
    }
    // A message that doesn't exist gets the same answer.
    let (status, _) = send(
        &pat_browser,
        &channel,
        "r3",
        reply("Agreed", &Uuid::now_v7().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(sent_in(pool, channel_id, "Agreed").await, 0);

    // A reply in the same conversation quotes the original, for the sender
    // and for everyone reading.
    let (status, sent) = send(&pat_browser, &channel, "r4", reply("I'm in", &open)).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(
        sent["message"]["replyTo"]["messageId"],
        open.as_str(),
        "{sent}"
    );
    assert_eq!(
        sent["message"]["replyTo"]["text"], "Lunch at noon",
        "{sent}"
    );
    let (again, _) = send(&pat_browser, &channel, "r4", reply("I'm in", &open)).await;
    assert!(again.is_success());
    assert_eq!(
        sent_in(pool, channel_id, "I'm in").await,
        1,
        "a retry never sends twice"
    );
    let timeline_uri = format!("/api/v1/conversations/{channel}/timeline");
    let (_, mara_view) = get(setup, &mara_browser, &timeline_uri).await;
    let quote = quote_on(&mara_view, "I'm in");
    assert_eq!(quote["available"], true, "{quote}");
    assert_eq!(quote["text"], "Lunch at noon", "{quote}");
    assert!(
        quote["href"]
            .as_str()
            .is_some_and(|href| href.ends_with(&format!("#message-{open}"))),
        "{quote}"
    );

    // The database holds the line too: a reply can't point into another
    // conversation, whatever the code above it does.
    let refused = sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id, content_version,
            content, plain_text, reply_to_message_id
        )
        SELECT $1, conversation.project_id, conversation.id, $3, 1, $4, 'Leak', $5
        FROM conversations conversation WHERE conversation.id = $2
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(channel.parse::<Uuid>().expect("channel"))
    .bind(mara)
    .bind(text("Leak"))
    .bind(secret.parse::<Uuid>().expect("secret"))
    .execute(pool)
    .await;
    let code = refused
        .expect_err("a cross-conversation reply was stored")
        .as_database_error()
        .and_then(|error| error.code().map(|code| code.into_owned()));
    assert_eq!(
        code.as_deref(),
        Some("23503"),
        "refused by the same-conversation key"
    );

    // In the group, Mara replies to the secret; Pat can't read the group, so
    // there's nothing of it for him anywhere.
    let (status, sent) = call(
        setup,
        &mara_browser,
        "POST",
        &format!("/api/v1/conversations/{group}/messages"),
        Some("g1"),
        reply("Count me in", &secret),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let (status, _) = get(
        setup,
        &pat_browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Through MCP, as Pat's connected app: the same rules, and the quote
    // shows on reading the channel.
    let token = connect_app(&setup.app, &pat_browser, "Pat's desk", "full").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let refused = tool(
        &setup.app,
        &token,
        &session,
        "message_send",
        json!({
            "conversationId": channel,
            "text": "Agreed",
            "replyToMessageId": secret,
            "clientRequestId": "mcp-r1",
        }),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    assert_eq!(refused["field"], "replyToMessageId", "{refused}");
    assert!(!refused.to_string().contains("Secret plan"), "{refused}");
    let link = format!("https://example.test/projects/{project}/channels/{channel}#message-{open}");
    let sent = tool(
        &setup.app,
        &token,
        &session,
        "message_send",
        json!({
            "conversationId": channel,
            "text": "Booked a table",
            "replyToMessageId": link,
            "clientRequestId": "mcp-r2",
        }),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let read = tool(
        &setup.app,
        &token,
        &session,
        "conversation_read",
        json!({ "conversationId": channel }),
    )
    .await;
    let reply_item = read["value"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["text"] == "Booked a table")
        .cloned()
        .unwrap_or_else(|| panic!("the MCP reply: {read}"));
    assert_eq!(
        reply_item["replyTo"]["messageId"],
        open.as_str(),
        "{reply_item}"
    );
    assert_eq!(
        reply_item["replyTo"]["text"], "Lunch at noon",
        "{reply_item}"
    );
}
