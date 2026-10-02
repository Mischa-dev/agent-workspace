//! Editing and removing messages (docs/plans/mcp-tools.md, decision 3). The
//! costly failures are silent: a removed secret surviving somewhere, someone
//! changing a message that isn't theirs, an edit losing a version, or an edit
//! reaching someone who can't see the conversation.

use agent_workspace_server::bootstrap::{BootstrapCommand, bootstrap_development_member};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, PASSWORD, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
};

fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

async fn owner_id(pool: &PgPool) -> Uuid {
    sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
        .fetch_one(pool)
        .await
        .expect("owner actor")
}

async fn direct_space(setup: &Setup) -> String {
    send(
        &setup.app,
        setup
            .browser
            .request("GET", "/api/v1/direct", None, Value::Null),
    )
    .await
    .1["projectId"]
        .as_str()
        .expect("space")
        .to_owned()
}

async fn create_channel(setup: &Setup, project: &str, name: &str) -> String {
    let (status, channel) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some(&format!("channel-{name}")),
            json!({ "name": name }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    channel["conversation"]["id"]
        .as_str()
        .or_else(|| channel["id"].as_str())
        .expect("channel")
        .to_owned()
}

/// Send through the web as `browser` and return the message ID.
async fn say(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    key: &str,
    content: Value,
) -> String {
    let (status, sent) = send(
        &setup.app,
        browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(key),
            json!({ "content": content }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["message"]["id"].as_str().expect("message").to_owned()
}

async fn edit(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    message: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request(
            "PATCH",
            &format!("/api/v1/conversations/{conversation}/messages/{message}"),
            Some(key),
            body,
        ),
    )
    .await
}

async fn remove(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    message: &str,
) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request(
            "DELETE",
            &format!("/api/v1/conversations/{conversation}/messages/{message}"),
            None,
            Value::Null,
        ),
    )
    .await
}

async fn get(setup: &Setup, browser: &Browser, uri: &str) -> (StatusCode, Value) {
    send(&setup.app, browser.request("GET", uri, None, Value::Null)).await
}

async fn count(pool: &PgPool, sql: &str, id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(id.parse::<Uuid>().expect("UUID"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

/// A person who can sign in, a member of each of `projects`.
async fn add_person(setup: &Setup, projects: &[&str], login: &str) -> (Uuid, Browser) {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(&setup.pool)
        .await
        .expect("person actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(setup.organization_id)
    .bind(actor_id)
    .execute(&setup.pool)
    .await
    .expect("organization membership");
    for project in projects {
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $4)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(actor_id)
        .bind(login)
        .execute(&setup.pool)
        .await
        .expect("project membership");
    }
    bootstrap_development_member(
        &setup.pool,
        BootstrapCommand {
            login_name: login.to_owned(),
            display_name: login.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor_id),
            organization_id: Some(setup.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap person");
    let response = setup
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", "http://127.0.0.1:3100")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": login, "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let _ = to_bytes(response.into_body(), 100_000).await;
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    (
        actor_id,
        Browser {
            cookie: cookies.join("; "),
            csrf,
        },
    )
}

async fn set_role(pool: &PgPool, project: &str, actor: Uuid, role: &str) {
    sqlx::query("UPDATE project_memberships SET role = $3 WHERE project_id = $1 AND actor_id = $2")
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(actor)
        .bind(role)
        .execute(pool)
        .await
        .expect("set role");
}

/// Rows of `table` whose text mentions `needle` anywhere.
async fn copies(pool: &PgPool, table: &str, needle: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {table} WHERE {table}::text LIKE $1"
    ))
    .bind(format!("%{needle}%"))
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("sweep {table}: {error}"))
}

#[tokio::test]
async fn removed_message_text_vanishes_everywhere() {
    let Some(setup) = setup("msg_privacy").await else {
        return;
    };
    let app = &setup.app;
    let owner = owner_id(&setup.pool).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = direct_space(&setup).await;
    let (mara_id, mara_token) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (_, nova_token) = create_agent(app, &setup.browser, &space, "Nova", "nova").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    for handle in ["@mara", "@nova"] {
        let added = call(
            "project_member_set",
            json!({"projectId": work, "person": handle}),
        )
        .await;
        assert_eq!(added["outcome"], "ok", "{added}");
    }
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let nova_session = initialize(app, &nova_token).await.expect("Nova MCP");
    let mara = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let general = create_channel(&setup, &work, "general").await;

    // Mara answers a question; removing her answer leaves its request
    // answered.
    call(
        "message_send",
        json!({"conversationId": general, "text": "@mara status?"}),
    )
    .await;
    let inbox = mara("inbox_list", json!({})).await;
    let question = inbox["value"]["requests"][0]["requestId"].clone();
    let claim = mara("reply_claim", json!({"requestId": question})).await;
    let answer = mara(
        "reply_send",
        json!({
            "requestId": question,
            "generation": claim["value"]["request"]["generation"],
            "text": "All good."
        }),
    )
    .await;
    assert_eq!(answer["outcome"], "ok", "{answer}");
    let answer_id = answer["value"]["messageId"]
        .as_str()
        .expect("answer")
        .to_owned();
    let (status, removed_answer) = remove(&setup, &setup.browser, &general, &answer_id).await;
    assert_eq!(status, StatusCode::OK, "{removed_answer}");
    assert_eq!(
        removed_answer["message"]["removed"]["removedBy"]["id"],
        owner.to_string(),
        "an owner removes an agent's message: {removed_answer}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM reply_requests WHERE reply_message_id = $1"
        )
        .bind(answer_id.parse::<Uuid>().expect("answer"))
        .fetch_one(&setup.pool)
        .await
        .expect("answered request"),
        "replied"
    );

    // The secret: sent through Codex, asking Mara and Nova, pinned, linked to
    // a task, with a thread named after it and another named by hand, then
    // edited once.
    let secret = format!("SECRET-{}", Uuid::now_v7().simple());
    let sent = call(
        "message_send",
        json!({
            "conversationId": general,
            "text": format!("key {secret} @mara @nova"),
            "clientRequestId": "secret-send"
        }),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let message = sent["value"]["messageId"]
        .as_str()
        .expect("message")
        .to_owned();
    let inbox = mara("inbox_list", json!({})).await;
    let request = inbox["value"]["requests"][0]["requestId"].clone();
    let claim = mara("reply_claim", json!({"requestId": request})).await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let pinned = call(
        "pin_set",
        json!({"conversationId": general, "messageId": message}),
    )
    .await;
    assert_eq!(pinned["outcome"], "ok", "{pinned}");
    let task = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Rotate the key"}),
    )
    .await;
    assert_eq!(task["outcome"], "applied", "{task}");
    let linked = call(
        "task_link",
        json!({"taskId": "WORK-1", "messageId": message}),
    )
    .await;
    assert_eq!(linked["outcome"], "ok", "{linked}");
    let plain_text =
        sqlx::query_scalar::<_, String>("SELECT plain_text FROM messages WHERE id = $1")
            .bind(message.parse::<Uuid>().expect("message"))
            .fetch_one(&setup.pool)
            .await
            .expect("plain text");
    let mut threads = Vec::new();
    for (key, name) in [
        ("thread-suggested", plain_text.as_str()),
        ("thread-named", "Rotation plan"),
    ] {
        let (status, thread) = send(
            app,
            setup.browser.request(
                "POST",
                &format!("/api/v1/channels/{general}/threads"),
                Some(key),
                json!({ "name": name, "originMessageId": message }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{thread}");
        threads.push(
            thread["conversation"]["id"]
                .as_str()
                .expect("thread")
                .to_owned(),
        );
    }
    let (status, edited) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "secret-edit",
        json!({
            "content": {"version": 1, "nodes": [
                {"type": "text", "text": format!("key {secret} expires Friday, ask ")},
                {"type": "mention", "actorId": mara_id}
            ]},
            "expectedRevision": 1
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["message"]["revision"], 2, "{edited}");
    assert_eq!(edited["changed"], true);

    let (status, removed) = remove(&setup, &setup.browser, &general, &message).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["alreadyRemoved"], false);
    assert_eq!(removed["message"]["plainText"], "");
    assert_eq!(removed["message"]["content"]["nodes"], json!([]));
    assert_eq!(
        removed["message"]["removed"]["removedBy"]["id"],
        owner.to_string()
    );
    assert_eq!(removed["message"]["revision"], 2, "removing doesn't edit");

    // Mara's claim and Nova's pending request are both withdrawn.
    let withdrawn = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, reason FROM reply_requests WHERE message_id = $1",
    )
    .bind(message.parse::<Uuid>().expect("message"))
    .fetch_all(&setup.pool)
    .await
    .expect("requests");
    assert_eq!(withdrawn.len(), 2, "{withdrawn:?}");
    assert!(
        withdrawn
            .iter()
            .all(|(status, reason)| status == "superseded"
                && reason.as_deref() == Some("The message was removed.")),
        "{withdrawn:?}"
    );
    let audit = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'conversation.message_removed' AND target_id = $1",
    )
    .bind(message.parse::<Uuid>().expect("message"))
    .fetch_one(&setup.pool)
    .await
    .expect("removal audit");
    assert_eq!(audit["as"], "author", "{audit}");
    assert_eq!(audit["cancelled_reply_requests"], 2, "{audit}");
    assert_eq!(audit["unpinned"], 1, "{audit}");
    assert_eq!(audit["renamed_threads"], 1, "{audit}");

    // The thread named after the message gets a neutral name; the one named
    // by hand keeps its name.
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM conversations WHERE origin_message_id = $1 ORDER BY created_at, id",
    )
    .bind(message.parse::<Uuid>().expect("message"))
    .fetch_all(&setup.pool)
    .await
    .expect("thread names");
    assert_eq!(names, ["Thread", "Rotation plan"]);

    // What people and agents see.
    let mara_reply = mara(
        "reply_send",
        json!({
            "requestId": request,
            "generation": claim["value"]["request"]["generation"],
            "text": "Rotated."
        }),
    )
    .await;
    assert_ne!(mara_reply["outcome"], "ok", "{mara_reply}");
    for (token, session) in [
        (codex.as_str(), session.as_str()),
        (mara_token.as_str(), mara_session.as_str()),
        (nova_token.as_str(), nova_session.as_str()),
    ] {
        let found = tool(app, token, session, "search", json!({"query": secret})).await;
        assert_eq!(found["value"]["results"], json!([]), "{found}");
        let read = tool(
            app,
            token,
            session,
            "conversation_read",
            json!({"conversationId": general}),
        )
        .await;
        assert_eq!(read["outcome"], "ok", "{read}");
        assert!(!read.to_string().contains(&secret), "{read}");
        let briefing = tool(app, token, session, "briefing", json!({})).await;
        assert!(!briefing.to_string().contains(&secret), "{briefing}");
    }
    for (token, session) in [
        (mara_token.as_str(), mara_session.as_str()),
        (nova_token.as_str(), nova_session.as_str()),
    ] {
        let inbox = tool(app, token, session, "inbox_list", json!({})).await;
        assert_eq!(inbox["value"]["requests"], json!([]), "{inbox}");
    }
    let (status, timeline) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{general}/timeline"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!timeline.to_string().contains(&secret), "{timeline}");
    let entry = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["id"] == message.as_str())
        .expect("tombstone entry")
        .clone();
    assert_eq!(entry["message"]["plainText"], "");
    assert_eq!(
        entry["message"]["removed"]["removedBy"]["id"],
        owner.to_string()
    );
    let (status, single) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{general}/messages/{message}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(single["entry"], entry);
    let (status, history) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{general}/messages/{message}/revisions"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["removed"], true);
    let revisions = history["revisions"].as_array().expect("revisions");
    assert_eq!(revisions.len(), 2, "{history}");
    for (index, revision) in revisions.iter().enumerate() {
        assert_eq!(revision["revision"], index + 1);
        assert_eq!(revision["redacted"], true);
        assert_eq!(revision["content"], Value::Null);
        assert_eq!(revision["plainText"], Value::Null);
        assert_eq!(revision["writtenBy"]["id"], owner.to_string());
    }
    assert_eq!(
        revisions[0]["via"], "Codex",
        "who wrote it through what stays"
    );
    assert_eq!(revisions[1]["via"], Value::Null);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM conversation_pins WHERE target_id = $1",
            &message
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM work_item_links WHERE target_id = $1",
            &message
        )
        .await,
        1,
        "task links stay"
    );

    // Retries never bring it back.
    let messages_before = count(
        &setup.pool,
        "SELECT count(*) FROM messages WHERE conversation_id = $1",
        &general,
    )
    .await;
    let replay = call(
        "message_send",
        json!({
            "conversationId": general,
            "text": format!("key {secret} @mara @nova"),
            "clientRequestId": "secret-send"
        }),
    )
    .await;
    assert_ne!(replay["outcome"], "ok", "{replay}");
    assert!(!replay.to_string().contains(&secret), "{replay}");
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM messages WHERE conversation_id = $1",
            &general
        )
        .await,
        messages_before
    );
    let tombstone_hash = Sha256::digest(br#"{"version":1,"nodes":[]}"#).to_vec();
    let replay_hashes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT request_hash FROM idempotency_keys WHERE response_body->'message'->>'id' = $1",
    )
    .bind(&message)
    .fetch_all(&setup.pool)
    .await
    .expect("send replays");
    assert_eq!(
        replay_hashes,
        std::slice::from_ref(&tombstone_hash),
        "no hash of the text is kept"
    );
    let (status, retried_edit) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "secret-edit",
        json!({
            "content": {"version": 1, "nodes": [
                {"type": "text", "text": format!("key {secret} expires Friday, ask ")},
                {"type": "mention", "actorId": mara_id}
            ]},
            "expectedRevision": 1
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{retried_edit}");
    let (status, retried_thread) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/threads"),
            Some("thread-suggested"),
            json!({ "name": plain_text, "originMessageId": message }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{retried_thread}");
    assert!(!retried_thread.to_string().contains(&secret));
    let (status, new_thread) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/threads"),
            Some("thread-after"),
            json!({ "name": "Later", "originMessageId": message }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "no new threads from it: {new_thread}"
    );
    let audits_before = count(
        &setup.pool,
        "SELECT count(*) FROM audit_events WHERE target_id = $1",
        &message,
    )
    .await;
    let (status, again) = remove(&setup, &setup.browser, &general, &message).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["alreadyRemoved"], true);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM audit_events WHERE target_id = $1",
            &message
        )
        .await,
        audits_before
    );

    // Events say what changed, never the text.
    let events = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT payload FROM outbox_events
        WHERE event_type = 'conversation.message.updated' AND resource_id = $1
        ORDER BY event_cursor
        "#,
    )
    .bind(message.parse::<Uuid>().expect("message"))
    .fetch_all(&setup.pool)
    .await
    .expect("message events");
    assert_eq!(
        events,
        [
            json!({"conversationId": general, "messageId": message, "revision": 2, "removed": false}),
            json!({"conversationId": general, "messageId": message, "revision": 2, "removed": true}),
        ]
    );

    // No copy of the text is left in any table that held one.
    for table in [
        "messages",
        "message_revisions",
        "idempotency_keys",
        "outbox_events",
        "audit_events",
        "external_agent_events",
        "reply_requests",
        "conversation_pins",
        "work_item_links",
        "conversations",
        "message_mentions",
    ] {
        assert_eq!(
            copies(&setup.pool, table, &secret).await,
            0,
            "{table} keeps a copy"
        );
    }
}

#[tokio::test]
async fn only_authors_edit_and_owners_or_admins_remove() {
    let Some(setup) = setup("msg_permissions").await else {
        return;
    };
    let app = &setup.app;
    let owner = owner_id(&setup.pool).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let general = create_channel(&setup, &work, "general").await;
    let (pat, pat_browser) = add_person(&setup, &[&work], "pat").await;
    let owner_browser = &setup.browser;

    let patted = say(&setup, &pat_browser, &general, "pat-1", text("Pat's first")).await;
    let (status, _) = edit(
        &setup,
        owner_browser,
        &general,
        &patted,
        "owner-edits-pat",
        json!({"content": text("Owner's words")}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "owners don't edit others' messages"
    );
    let (status, edited) = edit(
        &setup,
        &pat_browser,
        &general,
        &patted,
        "pat-edit-1",
        json!({"content": text("Pat's first, fixed"), "expectedRevision": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["message"]["revision"], 2);
    assert!(edited["message"]["editedAt"].is_string(), "{edited}");
    let (status, stale) = edit(
        &setup,
        &pat_browser,
        &general,
        &patted,
        "pat-edit-2",
        json!({"content": text("Pat's first, again"), "expectedRevision": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_eq!(stale["error"]["code"], "version_conflict", "{stale}");
    let events_before = count(
        &setup.pool,
        "SELECT count(*) FROM outbox_events WHERE resource_id = $1",
        &patted,
    )
    .await;
    let (status, same) = edit(
        &setup,
        &pat_browser,
        &general,
        &patted,
        "pat-edit-3",
        json!({"content": text("Pat's first, fixed")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(same["changed"], false);
    assert_eq!(same["message"]["revision"], 2);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM outbox_events WHERE resource_id = $1",
            &patted
        )
        .await,
        events_before,
        "an identical edit changes nothing"
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM message_revisions WHERE message_id = $1",
            &patted
        )
        .await,
        2
    );

    // Members remove only their own; admins and owners remove anyone's.
    let owners = say(
        &setup,
        owner_browser,
        &general,
        "owner-1",
        text("Owner's first"),
    )
    .await;
    let (status, _) = remove(&setup, &pat_browser, &general, &owners).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, timeline) = get(
        &setup,
        &pat_browser,
        &format!("/api/v1/conversations/{general}/timeline"),
    )
    .await;
    assert_eq!(timeline["capabilities"]["editMessages"], true);
    assert_eq!(timeline["capabilities"]["removeOwnMessages"], true);
    assert_eq!(timeline["capabilities"]["removeAnyMessage"], false);
    set_role(&setup.pool, &work, pat, "admin").await;
    let (_, timeline) = get(
        &setup,
        &pat_browser,
        &format!("/api/v1/conversations/{general}/timeline"),
    )
    .await;
    assert_eq!(timeline["capabilities"]["removeAnyMessage"], true);
    let (status, moderated) = remove(&setup, &pat_browser, &general, &owners).await;
    assert_eq!(status, StatusCode::OK, "{moderated}");
    assert_eq!(
        moderated["message"]["removed"]["removedBy"]["id"],
        pat.to_string()
    );
    let audit = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'conversation.message_removed' AND target_id = $1",
    )
    .bind(owners.parse::<Uuid>().expect("message"))
    .fetch_one(&setup.pool)
    .await
    .expect("moderation audit");
    assert_eq!(audit["as"], "moderator", "{audit}");
    set_role(&setup.pool, &work, pat, "member").await;
    let (status, by_owner) = remove(&setup, owner_browser, &general, &patted).await;
    assert_eq!(status, StatusCode::OK, "{by_owner}");
    assert_eq!(
        by_owner["message"]["removed"]["removedBy"]["id"],
        owner.to_string()
    );
    let (status, _) = edit(
        &setup,
        &pat_browser,
        &general,
        &patted,
        "pat-edit-removed",
        json!({"content": text("Back again")}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a removed message can't be edited"
    );

    // A viewer can't change even their own messages; an owner removes them.
    let second = say(
        &setup,
        &pat_browser,
        &general,
        "pat-2",
        text("Pat's second"),
    )
    .await;
    set_role(&setup.pool, &work, pat, "viewer").await;
    let (status, _) = edit(
        &setup,
        &pat_browser,
        &general,
        &second,
        "viewer-edit",
        json!({"content": text("Viewer's words")}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = remove(&setup, &pat_browser, &general, &second).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    set_role(&setup.pool, &work, pat, "member").await;

    // An agent's message: nobody else edits it; an owner removes it.
    let space = direct_space(&setup).await;
    let (_, mara_token) = create_agent(app, owner_browser, &space, "Mara", "mara").await;
    let codex = connect_app(app, owner_browser, "Codex", "full").await;
    let codex_session = initialize(app, &codex).await.expect("Codex MCP");
    let added = tool(
        app,
        &codex,
        &codex_session,
        "project_member_set",
        json!({"projectId": work, "person": "@mara"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let mara_session = initialize(app, &mara_token).await.expect("Mara MCP");
    let from_mara = tool(
        app,
        &mara_token,
        &mara_session,
        "message_send",
        json!({"conversationId": general, "text": "Mara here"}),
    )
    .await;
    let from_mara = from_mara["value"]["messageId"]
        .as_str()
        .expect("Mara's message")
        .to_owned();
    let (status, _) = edit(
        &setup,
        owner_browser,
        &general,
        &from_mara,
        "owner-edits-mara",
        json!({"content": text("Owner's words")}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = remove(&setup, owner_browser, &general, &from_mara).await;
    assert_eq!(status, StatusCode::OK);

    // Archived: no edits, but removal still works for the author.
    let kept = say(&setup, &pat_browser, &general, "pat-3", text("Pat's third")).await;
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM conversations WHERE id = $1")
        .bind(general.parse::<Uuid>().expect("channel"))
        .fetch_one(&setup.pool)
        .await
        .expect("channel version");
    let (status, archived) = send(
        app,
        owner_browser.request(
            "POST",
            &format!("/api/v1/channels/{general}/archive"),
            None,
            json!({"expectedVersion": version}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let (status, _) = edit(
        &setup,
        &pat_browser,
        &general,
        &kept,
        "archived-edit",
        json!({"content": text("Too late")}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, removed) = remove(&setup, &pat_browser, &general, &kept).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    let (status, history) = get(
        &setup,
        &pat_browser,
        &format!("/api/v1/conversations/{general}/messages/{kept}/revisions"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "archived history stays readable: {history}"
    );
    assert_eq!(history["revisions"][0]["redacted"], true, "{history}");
}

#[tokio::test]
async fn edits_keep_every_revision_and_projections_follow() {
    let Some(setup) = setup("msg_revisions").await else {
        return;
    };
    let app = &setup.app;
    let owner = owner_id(&setup.pool).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = direct_space(&setup).await;
    let (mara, _) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (nova, _) = create_agent(app, &setup.browser, &space, "Nova", "nova").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    for handle in ["@mara", "@nova"] {
        let added = tool(
            app,
            &codex,
            &session,
            "project_member_set",
            json!({"projectId": work, "person": handle}),
        )
        .await;
        assert_eq!(added["outcome"], "ok", "{added}");
    }
    let task = tool(
        app,
        &codex,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Zebracorn task"}),
    )
    .await;
    let task_id = task["workItemId"].as_str().expect("task").to_owned();
    let general = create_channel(&setup, &work, "general").await;
    let message = say(
        &setup,
        &setup.browser,
        &general,
        "zebracorn",
        json!({"version": 1, "nodes": [
            {"type": "text", "text": "ask "},
            {"type": "mention", "actorId": mara},
            {"type": "text", "text": " about the zebracorn in "},
            {"type": "work_item_ref", "workItemId": task_id}
        ]}),
    )
    .await;
    let requests = |agent: Uuid| {
        let pool = setup.pool.clone();
        let message = message.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM reply_requests WHERE message_id = $1::uuid AND agent_actor_id = $2",
            )
            .bind(message)
            .bind(agent)
            .fetch_all(&pool)
            .await
            .expect("requests")
        }
    };
    assert_eq!(requests(mara).await, ["pending"]);

    let instead = json!({"content": {"version": 1, "nodes": [
        {"type": "text", "text": "ask "},
        {"type": "mention", "actorId": nova},
        {"type": "text", "text": " instead"}
    ]}});
    let (status, edited) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "edit-1",
        instead.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["message"]["revision"], 2);
    let rows = sqlx::query_as::<_, (i32, String, Uuid)>(
        "SELECT revision, plain_text, written_by_actor_id FROM message_revisions WHERE message_id = $1::uuid ORDER BY revision",
    )
    .bind(&message)
    .fetch_all(&setup.pool)
    .await
    .expect("revisions");
    assert_eq!(rows.len(), 2);
    assert!(rows[0].1.contains("zebracorn"), "{rows:?}");
    assert!(rows[1].1.ends_with("instead"), "{rows:?}");
    assert!(rows.iter().all(|row| row.2 == owner));
    let mentions = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM message_mentions WHERE message_id = $1::uuid",
    )
    .bind(&message)
    .fetch_all(&setup.pool)
    .await
    .expect("mentions");
    assert_eq!(mentions, [nova]);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM message_work_item_references WHERE message_id = $1",
            &message
        )
        .await,
        0
    );
    assert_eq!(
        requests(nova).await,
        ["pending"],
        "a newly mentioned agent is asked"
    );
    assert_eq!(requests(mara).await, ["pending"], "nobody is un-asked");
    let nova_mentions = "SELECT count(*) FROM external_agent_events event JOIN external_agent_connections connection ON connection.id = event.connection_id WHERE connection.actor_id = $1 AND event.kind = 'message.mentioned'";
    assert_eq!(
        count(&setup.pool, nova_mentions, &nova.to_string()).await,
        1
    );
    let old_words = tool(
        app,
        &codex,
        &session,
        "search",
        json!({"query": "zebracorn"}),
    )
    .await;
    assert!(
        !old_words["value"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .any(|hit| hit.to_string().contains(&message)),
        "{old_words}"
    );
    let new_words = tool(app, &codex, &session, "search", json!({"query": "instead"})).await;
    assert!(
        new_words["value"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .any(|hit| hit.to_string().contains(&message)),
        "{new_words}"
    );

    // Taking Nova out and putting Mara back tells nobody again.
    let (status, back) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "edit-2",
        json!({"content": {"version": 1, "nodes": [
            {"type": "text", "text": "back to "},
            {"type": "mention", "actorId": mara}
        ]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(back["message"]["revision"], 3);
    let mara_mentions = count(&setup.pool, nova_mentions, &mara.to_string()).await;
    assert_eq!(mara_mentions, 1, "Mara was mentioned in revision 1");
    let (status, replayed) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "edit-2",
        json!({"content": {"version": 1, "nodes": [
            {"type": "text", "text": "back to "},
            {"type": "mention", "actorId": mara}
        ]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, back, "a retried edit replays");
    let (status, _) = edit(
        &setup,
        &setup.browser,
        &general,
        &message,
        "edit-2",
        instead,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "one key, one edit");
    let (_, history) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{general}/messages/{message}/revisions"),
    )
    .await;
    let texts = history["revisions"]
        .as_array()
        .expect("revisions")
        .iter()
        .map(|revision| {
            revision["plainText"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(texts.len(), 3, "{history}");
    assert!(texts[1].ends_with("instead") && texts[2].starts_with("back to"));
    let payload = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM outbox_events WHERE event_type = 'conversation.message.updated' AND resource_id = $1::uuid ORDER BY event_cursor DESC LIMIT 1",
    )
    .bind(&message)
    .fetch_one(&setup.pool)
    .await
    .expect("update event");
    assert_eq!(
        payload,
        json!({"conversationId": general, "messageId": message, "revision": 3, "removed": false})
    );

    // The database refuses every other way of changing a message.
    let id = message.parse::<Uuid>().expect("message");
    let other = say(&setup, &setup.browser, &general, "other", text("Other")).await;
    let refused = |sql: &'static str, flag: Option<String>| {
        let pool = setup.pool.clone();
        async move {
            let mut transaction = pool.begin().await.expect("transaction");
            if let Some(flag) = flag {
                sqlx::query("SELECT set_config('aw.message_command', $1, true)")
                    .bind(flag)
                    .execute(&mut *transaction)
                    .await
                    .expect("flag");
            }
            let result = sqlx::query(sql).bind(id).execute(&mut *transaction).await;
            assert!(result.is_err(), "{sql} was allowed");
        }
    };
    refused("UPDATE messages SET plain_text = 'x' WHERE id = $1", None).await;
    refused(
        "UPDATE messages SET plain_text = 'x', content = '{\"version\":1,\"nodes\":[{\"type\":\"text\",\"text\":\"x\"}]}', revision = revision + 1, edited_at = now() WHERE id = $1",
        Some(message.clone()),
    )
    .await;
    refused(
        "UPDATE messages SET removed_at = now(), removed_by_actor_id = author_actor_id, plain_text = '', content = '{\"version\": 1, \"nodes\": []}' WHERE id = $1",
        Some(message.clone()),
    )
    .await;
    refused("DELETE FROM messages WHERE id = $1", Some(message.clone())).await;
    refused(
        "DELETE FROM message_revisions WHERE message_id = $1",
        Some(message.clone()),
    )
    .await;
    refused(
        "UPDATE message_revisions SET plain_text = 'x' WHERE message_id = $1",
        Some(message.clone()),
    )
    .await;
    refused(
        "UPDATE message_revisions SET plain_text = 'x' WHERE message_id = $1",
        None,
    )
    .await;
    refused(
        "DELETE FROM message_mentions WHERE message_id = $1",
        Some(other.clone()),
    )
    .await;
    refused(
        "INSERT INTO message_revisions (project_id, message_id, revision, content, plain_text, written_by_actor_id, written_at) SELECT project_id, id, revision + 1, content, plain_text, author_actor_id, now() FROM messages WHERE id = $1",
        None,
    )
    .await;
}

#[tokio::test]
async fn edits_and_removals_never_widen_visibility() {
    let Some(setup) = setup("msg_visibility").await else {
        return;
    };
    let app = &setup.app;
    let space = direct_space(&setup).await;
    let (mara, _) = create_agent(app, &setup.browser, &space, "Mara", "mara").await;
    let (bob, _) = create_agent(app, &setup.browser, &space, "Bob", "bob").await;
    let (_, pat_browser) = add_person(&setup, &[&space], "pat").await;
    let (status, group) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("owner-group"),
            json!({ "memberIds": [mara] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let message = say(&setup, &setup.browser, &group, "group-1", text("Plans")).await;
    let asked_before = count(
        &setup.pool,
        "SELECT count(*) FROM reply_requests WHERE message_id = $1",
        &message,
    )
    .await;

    // Mentioning Bob, who isn't in the group, reaches him nowhere.
    let (status, edited) = edit(
        &setup,
        &setup.browser,
        &group,
        &message,
        "mention-bob",
        json!({"content": {"version": 1, "nodes": [
            {"type": "text", "text": "Plans for "},
            {"type": "mention", "actorId": bob}
        ]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM reply_requests WHERE message_id = $1",
            &message
        )
        .await,
        asked_before,
        "edits in groups ask nobody new"
    );
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM external_agent_events event JOIN external_agent_connections connection ON connection.id = event.connection_id WHERE connection.actor_id = $1 AND event.kind IN ('message.mentioned', 'reply.requested')",
            &bob.to_string()
        )
        .await,
        0,
        "Bob is never told about a group he can't see"
    );

    // A group the owner isn't in: its messages don't exist for the owner, by
    // any route, even paired with a conversation the owner can see.
    let (status, private) = send(
        app,
        pat_browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some("pat-group"),
            json!({ "memberIds": [mara] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{private}");
    let private = private["conversation"]["id"]
        .as_str()
        .expect("private group")
        .to_owned();
    let hidden = say(
        &setup,
        &pat_browser,
        &private,
        "pat-private",
        text("Private"),
    )
    .await;
    for conversation in [&private, &group] {
        let (status, _) = remove(&setup, &setup.browser, conversation, &hidden).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = edit(
            &setup,
            &setup.browser,
            conversation,
            &hidden,
            "hidden-edit",
            json!({"content": text("Mine now")}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        for suffix in ["", "/revisions"] {
            let (status, _) = get(
                &setup,
                &setup.browser,
                &format!("/api/v1/conversations/{conversation}/messages/{hidden}{suffix}"),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }
    // Pat can't reach the owner's group message through Pat's own group either.
    let (status, _) = get(
        &setup,
        &pat_browser,
        &format!("/api/v1/conversations/{private}/messages/{message}/revisions"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = remove(&setup, &pat_browser, &private, &message).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM messages WHERE id = $1 AND removed_at IS NOT NULL",
            &hidden
        )
        .await,
        0
    );
}
