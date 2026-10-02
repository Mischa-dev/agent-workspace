//! Emoji reactions on messages (docs/plans/mcp-tools.md, Phase 3). The costly
//! failures are silent: a reaction (or the fact of one) reaching someone who
//! can't see the conversation, someone reacting who can't post there, a
//! removed message's reactions still showing or changing, and a reaction
//! changing anything else (unread counts, reply requests).

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::{
        add_person, count, create_channel, direct_space, edit, get, owner_id, remove, say,
        set_role, text,
    },
};

const NAMES: [&str; 12] = [
    "thumbsup",
    "thumbsdown",
    "heart",
    "smile",
    "tada",
    "confused",
    "rocket",
    "eyes",
    "check",
    "x",
    "pray",
    "thinking",
];

/// PUT (react) or DELETE (take away) through the web.
async fn react_http(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    message: &str,
    name: &str,
    reacted: bool,
) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request(
            if reacted { "PUT" } else { "DELETE" },
            &format!("/api/v1/conversations/{conversation}/messages/{message}/reactions/{name}"),
            None,
            Value::Null,
        ),
    )
    .await
}

async fn create_group(setup: &Setup, browser: &Browser, key: &str, members: &[Uuid]) -> String {
    let (status, group) = send(
        &setup.app,
        browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some(key),
            json!({ "memberIds": members }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned()
}

/// The timeline entry for `message`, as `browser` sees it.
async fn timeline_message(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    message: &str,
) -> Value {
    let (status, timeline) = get(
        setup,
        browser,
        &format!("/api/v1/conversations/{conversation}/timeline?includeArchived=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["id"] == message)
        .unwrap_or_else(|| panic!("{message} in {timeline}"))["message"]
        .clone()
}

/// The MCP item for `message` in a conversation_read or conversation_open.
fn item<'a>(result: &'a Value, message: &str) -> &'a Value {
    result["value"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|item| item["messageId"] == message)
        .unwrap_or_else(|| panic!("{message} in {result}"))
}

async fn reaction_events(pool: &PgPool, message: &str) -> i64 {
    count(
        pool,
        "SELECT count(*) FROM outbox_events WHERE event_type = 'conversation.message.reactions.updated' AND resource_id = $1",
        message,
    )
    .await
}

async fn insert_reaction(
    pool: &PgPool,
    message: &str,
    actor: Uuid,
    emoji: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO message_reactions (project_id, conversation_id, message_id, actor_id, emoji)
        SELECT project_id, conversation_id, id, $2, $3 FROM messages WHERE id = $1
        "#,
    )
    .bind(message.parse::<Uuid>().expect("message"))
    .bind(actor)
    .bind(emoji)
    .execute(pool)
    .await
    .map(|_| ())
}

fn check_violation(result: Result<(), sqlx::Error>) {
    match result {
        Err(sqlx::Error::Database(error)) => {
            assert_eq!(error.code().as_deref(), Some("23514"), "{error}");
        }
        other => panic!("expected a check violation, got {other:?}"),
    }
}

#[tokio::test]
async fn reactions_stay_inside_the_conversation() {
    let Some(setup) = setup("react_privacy").await else {
        return;
    };
    let app = &setup.app;
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (nyx, nyx_token) = create_agent(app, &setup.browser, &space, "Nyx", "nyx").await;
    let (_, pat) = add_person(&setup, &[&space], "pat").await;
    let owner = owner_id(&setup.pool).await;
    let group = create_group(&setup, &setup.browser, "keys-group", &[mara]).await;
    let message = say(
        &setup,
        &setup.browser,
        &group,
        "keys-1",
        text("Rotate the keys"),
    )
    .await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    let reacted = mara_call(
        "message_react",
        json!({"messageId": message, "emoji": "check"}),
    )
    .await;
    assert_eq!(reacted["outcome"], "ok", "{reacted}");
    assert_eq!(reacted["value"]["emoji"], "✅");
    assert_eq!(reacted["value"]["name"], "check");
    assert_eq!(reacted["value"]["reacted"], true);
    assert_eq!(reacted["value"]["changed"], true);
    assert_eq!(
        reacted["value"]["reactions"],
        json!([{"emoji": "✅", "name": "check", "count": 1, "byYou": true,
                "people": ["Mara (@mara, agent)"]}])
    );
    // Doing it again changes nothing and tells nobody.
    let again = mara_call(
        "message_react",
        json!({"messageId": message, "emoji": "✅"}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["changed"], false);
    assert_eq!(again["value"]["reactions"][0]["count"], 1);
    assert_eq!(reaction_events(&setup.pool, &message).await, 1);
    // The event is about the group, never the whole space: everyone in the
    // organization is in the DM/group project.
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT conversation_id FROM outbox_events WHERE event_type = 'conversation.message.reactions.updated' AND resource_id = $1",
        )
        .bind(message.parse::<Uuid>().expect("message"))
        .fetch_one(&setup.pool)
        .await
        .expect("event"),
        Some(group.parse::<Uuid>().expect("group"))
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM outbox_events WHERE event_type = 'conversation.message.reactions.updated' AND resource_id = $1",
        )
        .bind(message.parse::<Uuid>().expect("message"))
        .fetch_one(&setup.pool)
        .await
        .expect("payload"),
        json!({"conversationId": group, "messageId": message}),
        "IDs only"
    );

    // Nyx isn't in the group: the message doesn't exist for him.
    let nyx_session = initialize(app, &nyx_token).await.expect("Nyx MCP");
    let link = format!("http://127.0.0.1:3100/messages/{group}#message-{message}");
    for (name, arguments) in [
        (
            "message_react",
            json!({"messageId": message, "emoji": "eyes"}),
        ),
        (
            "message_react",
            json!({"messageId": link, "emoji": "check", "reacted": false}),
        ),
        ("conversation_read", json!({"conversationId": group})),
    ] {
        let refused = tool(app, &nyx_token, &nyx_session, name, arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{name}: {refused}");
    }
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM message_reactions WHERE actor_id = $1",
            &nyx.to_string()
        )
        .await,
        0
    );
    // The database refuses it too.
    check_violation(insert_reaction(&setup.pool, &message, nyx, "👀").await);

    // Pat, a person outside the group, gets 404 for it, even paired with a
    // conversation Pat can see.
    let pats = create_group(&setup, &pat, "pat-group", &[owner]).await;
    for conversation in [&group, &pats] {
        let (status, refused) =
            react_http(&setup, &pat, conversation, &message, "eyes", true).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
        let (status, refused) =
            react_http(&setup, &pat, conversation, &message, "check", false).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    }
    // The owner, who can see both, can't pair the message with the wrong
    // conversation either.
    let (status, _) = react_http(&setup, &setup.browser, &pats, &message, "eyes", true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let pat_message = say(&setup, &pat, &pats, "pat-1", text("Hello")).await;
    let (status, _) = react_http(&setup, &setup.browser, &group, &pat_message, "eyes", true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The owner, through an account-wide app, reacts by link.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let codex_session = initialize(app, &codex).await.expect("Codex MCP");
    let by_link = tool(
        app,
        &codex,
        &codex_session,
        "message_react",
        json!({"messageId": link, "emoji": ":EYES:"}),
    )
    .await;
    assert_eq!(by_link["outcome"], "ok", "{by_link}");
    assert_eq!(by_link["value"]["emoji"], "👀");
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT via_oauth_grant_id IS NOT NULL FROM message_reactions WHERE message_id = $1 AND emoji = '👀'",
        )
        .bind(message.parse::<Uuid>().expect("message"))
        .fetch_one(&setup.pool)
        .await
        .expect("via"),
        "a reaction through a connected app records the app"
    );

    // Mara leaves the group: her reaction stays, and she can't change it.
    let (status, removed) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/members/{mara}/remove"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    for arguments in [
        json!({"messageId": message, "emoji": "eyes"}),
        json!({"messageId": message, "emoji": "check", "reacted": false}),
    ] {
        let refused = mara_call("message_react", arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
    }
    let shown = timeline_message(&setup, &setup.browser, &group, &message).await;
    let reactions = shown["reactions"].as_array().expect("reactions");
    assert_eq!(reactions.len(), 2, "{shown}");
    assert_eq!(reactions[0]["name"], "check");
    assert_eq!(reactions[0]["count"], 1);
    assert_eq!(reactions[0]["byViewer"], false);
    assert_eq!(reactions[0]["actors"][0]["id"], mara.to_string());
    assert_eq!(reactions[1]["name"], "eyes");
    assert_eq!(reactions[1]["byViewer"], true);
}

#[tokio::test]
async fn only_people_who_can_post_can_react() {
    let Some(setup) = setup("react_roles").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = direct_space(&setup).await;
    let (vee, vee_token) = create_agent(app, &setup.browser, &space, "Vee", "vee").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let added = call(
        "project_member_set",
        json!({"projectId": work, "person": "@vee"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    set_role(&setup.pool, &work, vee, "viewer").await;
    let general = create_channel(&setup, &work, "general").await;
    let message = say(
        &setup,
        &setup.browser,
        &general,
        "general-1",
        text("Ship it"),
    )
    .await;

    // A viewer agent reads, but can't react.
    let vee_session = initialize(app, &vee_token).await.expect("Vee MCP");
    let read = tool(
        app,
        &vee_token,
        &vee_session,
        "conversation_read",
        json!({"conversationId": general}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let refused = tool(
        app,
        &vee_token,
        &vee_session,
        "message_react",
        json!({"messageId": message, "emoji": "eyes"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("can't react")),
        "{refused}"
    );
    // A viewer person gets 403 on the web: they can see the conversation.
    let (viewer, viewer_browser) = add_person(&setup, &[&work], "val").await;
    set_role(&setup.pool, &work, viewer, "viewer").await;
    let (status, _) = react_http(&setup, &viewer_browser, &general, &message, "eyes", true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A read-only app can't either.
    let reader = connect_app(app, &setup.browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    let refused = tool(
        app,
        &reader,
        &reader_session,
        "message_react",
        json!({"messageId": message, "emoji": "eyes"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("read-only")),
        "{refused}"
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM message_reactions WHERE message_id = $1",
            &message
        )
        .await,
        0
    );

    // Skin tones and colons resolve; anything off the list is refused.
    let toned = call(
        "message_react",
        json!({"messageId": message, "emoji": "👍🏽"}),
    )
    .await;
    assert_eq!(toned["outcome"], "ok", "{toned}");
    assert_eq!(toned["value"]["emoji"], "👍");
    let eyes = call(
        "message_react",
        json!({"messageId": message, "emoji": ":EYES:"}),
    )
    .await;
    assert_eq!(eyes["value"]["emoji"], "👀", "{eyes}");
    for emoji in ["🦄", "👀✅", ""] {
        let refused = call(
            "message_react",
            json!({"messageId": message, "emoji": emoji}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{emoji}: {refused}");
        assert_eq!(refused["field"], "emoji");
    }
    let refused = call(
        "message_react",
        json!({"messageId": "not a message", "emoji": "eyes"}),
    )
    .await;
    assert_eq!(refused["field"], "messageId", "{refused}");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT string_agg(emoji, '' ORDER BY created_at) FROM message_reactions WHERE message_id = $1",
        )
        .bind(message.parse::<Uuid>().expect("message"))
        .fetch_one(&setup.pool)
        .await
        .expect("stored"),
        "👍👀"
    );

    // Every name on the list goes through the web and the database CHECK.
    for name in NAMES {
        let (status, added) =
            react_http(&setup, &setup.browser, &general, &message, name, true).await;
        assert_eq!(status, StatusCode::OK, "{name}: {added}");
        let mine = added["reactions"]
            .as_array()
            .expect("reactions")
            .iter()
            .find(|reaction| reaction["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {added}"));
        assert_eq!(mine["byViewer"], true);
        // The app acts as the owner: what it added is already there.
        assert_eq!(
            added["changed"],
            !matches!(name, "thumbsup" | "eyes"),
            "{name}: {added}"
        );
        let (status, taken) =
            react_http(&setup, &setup.browser, &general, &message, name, false).await;
        assert_eq!(status, StatusCode::OK, "{name}: {taken}");
        assert_eq!(taken["changed"], true, "{name}: {taken}");
    }
    let (status, _) = react_http(&setup, &setup.browser, &general, &message, "unicorn", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = react_http(&setup, &setup.browser, &general, &message, "👀", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the web takes names only");

    let (status, _) = react_http(&setup, &setup.browser, &general, &message, "rocket", true).await;
    assert_eq!(status, StatusCode::OK);

    // Archived conversations keep their reactions, frozen.
    let archived = call(
        "conversation_update",
        json!({"conversationId": general, "archived": true}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let refused = call(
        "message_react",
        json!({"messageId": message, "emoji": "tada"}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("archived")),
        "{refused}"
    );
    let (status, _) = react_http(&setup, &setup.browser, &general, &message, "tada", true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = react_http(
        &setup,
        &setup.browser,
        &general,
        &message,
        "thumbsup",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let shown = timeline_message(&setup, &setup.browser, &general, &message).await;
    assert_eq!(shown["reactions"][0]["name"], "rocket", "{shown}");
    assert_eq!(
        shown["reactions"].as_array().map(Vec::len),
        Some(1),
        "{shown}"
    );
}

#[tokio::test]
async fn reactions_read_back_and_change_nothing_else() {
    let Some(setup) = setup("react_quiet").await else {
        return;
    };
    let app = &setup.app;
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let group = create_group(&setup, &setup.browser, "quiet-group", &[mara]).await;
    let message = say(&setup, &setup.browser, &group, "quiet-1", text("Status?")).await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let open = || {
        let codex = codex.clone();
        let session = session.clone();
        let group = group.clone();
        async move {
            tool(
                app,
                &codex,
                &session,
                "conversation_open",
                json!({"conversationId": group}),
            )
            .await
        }
    };
    let before = open().await;
    assert_eq!(before["outcome"], "ok", "{before}");
    assert!(item(&before, &message).get("reactions").is_none());
    let asked = count(
        &setup.pool,
        "SELECT count(*) FROM reply_requests WHERE message_id = $1",
        &message,
    )
    .await;
    let (status, read_before) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara_call = |arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, "message_react", arguments).await }
    };
    for emoji in ["eyes", "check"] {
        let reacted = mara_call(json!({"messageId": message, "emoji": emoji})).await;
        assert_eq!(reacted["outcome"], "ok", "{reacted}");
    }
    let opened = open().await;
    let reactions = &item(&opened, &message)["reactions"];
    assert_eq!(
        reactions,
        &json!([
            {"emoji": "👀", "name": "eyes", "count": 1, "byYou": false,
             "people": ["Mara (@mara, agent)"]},
            {"emoji": "✅", "name": "check", "count": 1, "byYou": false,
             "people": ["Mara (@mara, agent)"]}
        ]),
        "in the order first added"
    );
    let read = tool(
        app,
        &codex,
        &session,
        "conversation_read",
        json!({"conversationId": group}),
    )
    .await;
    assert_eq!(&item(&read, &message)["reactions"], reactions);

    let taken = mara_call(json!({"messageId": message, "emoji": "👀", "reacted": false})).await;
    assert_eq!(taken["value"]["changed"], true, "{taken}");
    assert_eq!(taken["value"]["reacted"], false);
    let again = mara_call(json!({"messageId": message, "emoji": "eyes", "reacted": false})).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["changed"], false);
    assert_eq!(reaction_events(&setup.pool, &message).await, 3);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM audit_events WHERE action = 'conversation.message_unreacted' AND target_id = $1",
            &message
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM audit_events WHERE action = 'conversation.message_reacted' AND target_id = $1 AND metadata = jsonb_build_object('conversation_id', (SELECT conversation_id::text FROM messages WHERE id = $1), 'reaction', 'eyes')",
            &message
        )
        .await,
        1,
        "audit metadata holds IDs and the reaction's name only"
    );

    // Nothing else moved: no new entry, no unread, no reply request.
    let after = open().await;
    assert_eq!(
        after["value"]["highestSequence"],
        before["value"]["highestSequence"]
    );
    assert_eq!(
        after["value"]["unreadCount"],
        before["value"]["unreadCount"]
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM reply_requests WHERE message_id = $1",
            &message
        )
        .await,
        asked
    );
    let (_, read_after) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert_eq!(read_after["readState"], read_before["readState"]);

    // The web: react, then read it back from the timeline and the entry.
    let (status, added) = react_http(&setup, &setup.browser, &group, &message, "tada", true).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["changed"], true);
    assert_eq!(added["conversationId"], group);
    let tada = &added["reactions"][1];
    assert_eq!(tada["emoji"], "🎉");
    assert_eq!(tada["byViewer"], true);
    let shown = timeline_message(&setup, &setup.browser, &group, &message).await;
    assert_eq!(shown["reactions"], added["reactions"]);
    let (status, entry) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/messages/{message}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(entry["entry"]["message"]["reactions"], added["reactions"]);

    // Edits keep reactions.
    let (status, edited) = edit(
        &setup,
        &setup.browser,
        &group,
        &message,
        "quiet-edit",
        json!({"content": text("Status, please?")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["message"]["reactions"], added["reactions"]);
    let shown = timeline_message(&setup, &setup.browser, &group, &message).await;
    assert_eq!(shown["reactions"], added["reactions"]);
}

#[tokio::test]
async fn removed_messages_hide_and_freeze_their_reactions() {
    let Some(setup) = setup("react_removed").await else {
        return;
    };
    let app = &setup.app;
    let owner = owner_id(&setup.pool).await;
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let group = create_group(&setup, &setup.browser, "removed-group", &[mara]).await;
    let message = say(
        &setup,
        &setup.browser,
        &group,
        "removed-1",
        text("token abc"),
    )
    .await;
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let mara_call = |arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, "message_react", arguments).await }
    };
    let reacted = mara_call(json!({"messageId": message, "emoji": "eyes"})).await;
    assert_eq!(reacted["outcome"], "ok", "{reacted}");
    let (status, _) = react_http(&setup, &setup.browser, &group, &message, "check", true).await;
    assert_eq!(status, StatusCode::OK);

    let (status, removed) = remove(&setup, &setup.browser, &group, &message).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["message"]["reactions"], json!([]), "{removed}");
    let events = reaction_events(&setup.pool, &message).await;

    // Hidden everywhere: the timeline, the entry, and MCP reads.
    let shown = timeline_message(&setup, &setup.browser, &group, &message).await;
    assert_eq!(shown["reactions"], json!([]), "{shown}");
    let (_, entry) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/messages/{message}"),
    )
    .await;
    assert_eq!(entry["entry"]["message"]["reactions"], json!([]));
    let read = tool(
        app,
        &mara_token,
        &mara_session,
        "conversation_read",
        json!({"conversationId": group}),
    )
    .await;
    let shown = item(&read, &message);
    assert_eq!(shown["removed"], true);
    assert!(shown.get("reactions").is_none(), "{shown}");

    // Frozen: no surface can change them, and the database refuses too.
    let refused = mara_call(json!({"messageId": message, "emoji": "check"})).await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("removed")),
        "{refused}"
    );
    let refused = mara_call(json!({"messageId": message, "emoji": "eyes", "reacted": false})).await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    for reacted in [true, false] {
        let (status, _) =
            react_http(&setup, &setup.browser, &group, &message, "check", reacted).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
    check_violation(insert_reaction(&setup.pool, &message, owner, "🎉").await);
    assert_eq!(reaction_events(&setup.pool, &message).await, events);
    // The rows hold no text, so they are kept.
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM message_reactions WHERE message_id = $1",
            &message
        )
        .await,
        2
    );
}
