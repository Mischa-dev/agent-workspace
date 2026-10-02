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

pub(super) fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

pub(super) async fn owner_id(pool: &PgPool) -> Uuid {
    sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
        .fetch_one(pool)
        .await
        .expect("owner actor")
}

pub(super) async fn direct_space(setup: &Setup) -> String {
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

pub(super) async fn create_channel(setup: &Setup, project: &str, name: &str) -> String {
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
pub(super) async fn say(
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

pub(super) async fn edit(
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

pub(super) async fn remove(
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

pub(super) async fn get(setup: &Setup, browser: &Browser, uri: &str) -> (StatusCode, Value) {
    send(&setup.app, browser.request("GET", uri, None, Value::Null)).await
}

pub(super) async fn count(pool: &PgPool, sql: &str, id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(id.parse::<Uuid>().expect("UUID"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

/// A person who can sign in, a member of each of `projects`.
pub(super) async fn add_person(setup: &Setup, projects: &[&str], login: &str) -> (Uuid, Browser) {
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

pub(super) async fn set_role(pool: &PgPool, project: &str, actor: Uuid, role: &str) {
    sqlx::query("UPDATE project_memberships SET role = $3 WHERE project_id = $1 AND actor_id = $2")
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(actor)
        .bind(role)
        .execute(pool)
        .await
        .expect("set role");
}

/// Rows of `table` whose text mentions `needle` anywhere.
pub(super) async fn copies(pool: &PgPool, table: &str, needle: &str) -> i64 {
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
    let nova = |name: &'static str, arguments: Value| {
        let token = nova_token.clone();
        let session = nova_session.clone();
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
    // Nova gives hers back; a released request can be retried, so removal
    // withdraws it too.
    let nova_inbox = nova("inbox_list", json!({})).await;
    let nova_request = nova_inbox["value"]["requests"][0]["requestId"].clone();
    let nova_claim = nova("reply_claim", json!({"requestId": nova_request})).await;
    assert_eq!(nova_claim["outcome"], "ok", "{nova_claim}");
    let released = nova(
        "reply_release",
        json!({
            "requestId": nova_request,
            "generation": nova_claim["value"]["request"]["generation"],
            "reason": "Not mine."
        }),
    )
    .await;
    assert_eq!(released["outcome"], "ok", "{released}");
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
    // Status changes on the board are audited without the thread's name.
    for thread in &threads {
        let (status, updated) = send(
            app,
            setup.browser.request(
                "PATCH",
                &format!("/api/v1/threads/{thread}"),
                None,
                json!({ "status": "Done" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        let audit = sqlx::query_scalar::<_, Value>(
            "SELECT metadata FROM audit_events WHERE action = 'conversation.thread_updated' AND target_id = $1",
        )
        .bind(thread.parse::<Uuid>().expect("thread"))
        .fetch_one(&setup.pool)
        .await
        .expect("thread status audit");
        assert_eq!(audit, json!({ "status": "Done" }));
    }
    // Rows written before that named the thread; the hand-named one was
    // once named after the message too.
    for thread in &threads {
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, project_id, authentication_class, action,
                target_kind, target_id, outcome, metadata
            )
            VALUES ($1, $2, $3, 'password_session', 'conversation.thread_updated',
                    'conversation', $4, 'succeeded', $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .bind(work.parse::<Uuid>().expect("project"))
        .bind(thread.parse::<Uuid>().expect("thread"))
        .bind(json!({ "status": "Open", "name": plain_text }))
        .execute(&setup.pool)
        .await
        .expect("older thread audit");
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

    // Agents never remove or edit someone else's message.
    let refused = mara("message_delete", json!({"messageId": message})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(
        refused["message"],
        "Agents can only remove their own messages. Ask a project owner or admin to remove this one."
    );
    let refused = mara(
        "message_edit",
        json!({"messageId": message, "text": "Nothing to see"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(refused["message"], "You can only edit your own messages.");

    // Codex, connected as the owner, removes it by its link.
    let link =
        format!("http://127.0.0.1:3100/projects/{work}/channels/{general}#message-{message}");
    let removed = call("message_delete", json!({"messageId": link})).await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    let removed = removed["value"].clone();
    assert_eq!(removed["messageId"], message.as_str());
    assert_eq!(removed["conversationId"], general.as_str());
    assert_eq!(removed["alreadyRemoved"], false);
    assert_eq!(removed["cancelledReplyRequests"], 2, "{removed}");
    assert!(
        removed["note"]
            .as_str()
            .is_some_and(|note| note.contains("rotate the secret")),
        "{removed}"
    );
    let removed_by = removed["removedBy"].as_str().expect("remover").to_owned();
    assert!(removed_by.ends_with("(@owner)"), "{removed}");
    let listed = removed["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .map(|thread| (thread["id"].clone(), thread["name"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        [
            (json!(threads[0]), json!("Thread")),
            (json!(threads[1]), json!("Rotation plan")),
        ],
        "the remover sees the threads, after renaming"
    );

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
        "SELECT metadata FROM audit_events WHERE action = 'conversation.mcp_message_removed' AND target_kind = 'message' AND target_id = $1",
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
    // Older audit rows naming those threads keep who changed what, not the
    // name.
    let mut thread_audits = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT metadata FROM audit_events
        WHERE action = 'conversation.thread_updated' AND target_id = ANY($1)
        "#,
    )
    .bind(
        threads
            .iter()
            .map(|thread| thread.parse::<Uuid>().expect("thread"))
            .collect::<Vec<_>>(),
    )
    .fetch_all(&setup.pool)
    .await
    .expect("thread audits");
    thread_audits.sort_by_key(ToString::to_string);
    assert_eq!(
        thread_audits,
        [
            json!({ "status": "Done" }),
            json!({ "status": "Done" }),
            json!({ "status": "Open" }),
            json!({ "status": "Open" }),
        ]
    );

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
    assert_eq!(mara_reply["outcome"], "conflict", "{mara_reply}");
    assert_eq!(
        mara_reply["message"],
        "The message you were answering was removed, so no reply is needed. Your reply wasn't sent."
    );
    let nova_again = nova("reply_claim", json!({"requestId": nova_request})).await;
    assert_eq!(nova_again["outcome"], "conflict", "{nova_again}");
    assert_eq!(
        nova_again["message"],
        "The message you were asked about was removed, so no reply is needed."
    );
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
        let item = read["value"]["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|item| item["messageId"] == message.as_str())
            .expect("tombstone item")
            .clone();
        assert_eq!(item["removed"], true, "{item}");
        assert_eq!(item["text"], "", "{item}");
        assert_eq!(item["removedBy"], removed_by.as_str(), "{item}");
        assert_eq!(item["revision"], 2, "{item}");
        let around = tool(
            app,
            token,
            session,
            "conversation_read",
            json!({"conversationId": general, "aroundMessageId": link}),
        )
        .await;
        assert_eq!(around["outcome"], "ok", "{around}");
        assert!(!around.to_string().contains(&secret), "{around}");
        let opened = tool(
            app,
            token,
            session,
            "conversation_open",
            json!({"conversationId": general}),
        )
        .await;
        assert_eq!(opened["outcome"], "ok", "{opened}");
        assert!(!opened.to_string().contains(&secret), "{opened}");
        assert_eq!(opened["value"]["pinned"], json!([]), "{opened}");
        let listed = tool(
            app,
            token,
            session,
            "conversation_list",
            json!({"projectId": work}),
        )
        .await;
        assert_eq!(listed["outcome"], "ok", "{listed}");
        assert!(!listed.to_string().contains(&secret), "{listed}");
        let channel = listed["value"]["conversations"]
            .as_array()
            .expect("conversations")
            .iter()
            .find(|conversation| conversation["id"] == general.as_str())
            .expect("#general")
            .clone();
        assert_eq!(
            channel["lastMessage"]["text"], "Message removed",
            "{channel}"
        );
        assert_eq!(channel["lastMessage"]["removed"], true, "{channel}");
        let history = tool(
            app,
            token,
            session,
            "message_history",
            json!({"messageId": message}),
        )
        .await;
        assert_eq!(history["outcome"], "ok", "{history}");
        assert_eq!(history["value"]["removed"], true, "{history}");
        let versions = history["value"]["revisions"].as_array().expect("revisions");
        assert_eq!(versions.len(), 2, "{history}");
        for (index, version) in versions.iter().enumerate() {
            assert_eq!(version["revision"], index + 1);
            assert_eq!(version["redacted"], true);
            assert_eq!(version["text"], "");
        }
        assert_eq!(versions[0]["via"], "Codex", "{history}");
        let briefing = tool(app, token, session, "briefing", json!({})).await;
        assert!(!briefing.to_string().contains(&secret), "{briefing}");
        assert!(
            !briefing.to_string().contains(&message),
            "no mention of it is left: {briefing}"
        );
        let task = tool(
            app,
            token,
            session,
            "task_open",
            json!({"taskId": "WORK-1"}),
        )
        .await;
        assert_eq!(task["outcome"], "ok", "{task}");
        assert!(!task.to_string().contains(&secret), "{task}");
        let task_link = task["value"]["links"]
            .as_array()
            .expect("links")
            .iter()
            .find(|link| link["id"] == message.as_str())
            .expect("message link")
            .clone();
        assert_eq!(task_link["text"], "Message removed", "{task_link}");
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
    let (status, pins) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{general}/pins"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pins}");
    assert!(!pins.to_string().contains(&message), "{pins}");
    let (status, hits) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/search?q={secret}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hits}");
    assert_eq!(hits["hits"], json!([]), "{hits}");

    // Nothing new can be made from it.
    let pin_again = call(
        "pin_set",
        json!({"conversationId": general, "messageId": message}),
    )
    .await;
    assert_eq!(pin_again["outcome"], "not_found", "{pin_again}");
    let second_task = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Audit the logs"}),
    )
    .await;
    assert_eq!(second_task["outcome"], "applied", "{second_task}");
    let link_again = call(
        "task_link",
        json!({"taskId": "WORK-2", "messageId": message}),
    )
    .await;
    assert_eq!(link_again["outcome"], "not_found", "{link_again}");
    let decision = call(
        "decision_record",
        json!({"title": "Rotate keys monthly", "fromMessageId": message}),
    )
    .await;
    assert_eq!(decision["outcome"], "not_found", "{decision}");
    let (status, web_pin) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{general}/pins"),
            None,
            json!({"kind": "message", "targetId": message}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{web_pin}");
    // Its existing thread is still found; no new one starts from it.
    let thread_again = call(
        "thread_start",
        json!({"conversationId": general, "fromMessageId": message}),
    )
    .await;
    assert_eq!(thread_again["outcome"], "ok", "{thread_again}");
    assert_eq!(thread_again["value"]["created"], false, "{thread_again}");
    assert_eq!(thread_again["value"]["id"], threads[0].as_str());
    assert_eq!(thread_again["value"]["name"], "Thread");
    // A withdrawn request can't be retried into asking about it again.
    let (status, retried) = send(
        app,
        setup.browser.request(
            "POST",
            &format!(
                "/api/v1/reply-requests/{}/retry",
                nova_request.as_str().expect("Nova's request")
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{retried}");

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
    let again = call("message_delete", json!({"messageId": message})).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["alreadyRemoved"], true, "{again}");
    assert_eq!(again["value"]["cancelledReplyRequests"], 0, "{again}");
    assert_eq!(again["value"]["removedBy"], removed_by.as_str());
    let edit_again = call(
        "message_edit",
        json!({"messageId": message, "text": format!("key {secret}"), "clientRequestId": "secret-edit"}),
    )
    .await;
    assert_eq!(edit_again["outcome"], "conflict", "{edit_again}");
    assert_eq!(
        edit_again["message"],
        "That message was removed, so it can't be edited."
    );
    assert!(!edit_again.to_string().contains(&secret));
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM audit_events WHERE target_id = $1",
            &message
        )
        .await,
        audits_before
    );

    // An agent's own message, edited through MCP, with a thread named after
    // it by thread_start: the agent removes it, and every copy goes.
    let own_secret = format!("SECRET-{}", Uuid::now_v7().simple());
    let own = mara(
        "message_send",
        json!({"conversationId": general, "text": format!("token {own_secret}")}),
    )
    .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    let own_id = own["value"]["messageId"].as_str().expect("own").to_owned();
    let own_edit = json!({
        "messageId": own_id,
        "text": format!("token {own_secret} (staging)"),
        "expectedRevision": 1,
        "clientRequestId": "own-edit"
    });
    let edited_own = mara("message_edit", own_edit.clone()).await;
    assert_eq!(edited_own["outcome"], "ok", "{edited_own}");
    assert_eq!(edited_own["value"]["revision"], 2, "{edited_own}");
    assert_eq!(edited_own["value"]["changed"], true);
    assert_eq!(edited_own["value"]["idempotentReplay"], false);
    assert!(
        edited_own["value"]["url"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("#message-{own_id}"))),
        "{edited_own}"
    );
    let replayed = mara("message_edit", own_edit).await;
    assert_eq!(replayed["outcome"], "ok", "{replayed}");
    assert_eq!(replayed["value"]["idempotentReplay"], true, "{replayed}");
    assert_eq!(replayed["value"]["revision"], 2);
    let own_thread = mara(
        "thread_start",
        json!({"conversationId": general, "fromMessageId": own_id}),
    )
    .await;
    assert_eq!(own_thread["outcome"], "ok", "{own_thread}");
    assert_eq!(own_thread["value"]["created"], true);
    assert!(
        own_thread["value"]["name"]
            .as_str()
            .is_some_and(|name| name.contains(&own_secret)),
        "named after the message: {own_thread}"
    );
    let own_removed = mara("message_delete", json!({"messageId": own_id})).await;
    assert_eq!(own_removed["outcome"], "ok", "{own_removed}");
    assert_eq!(
        own_removed["value"]["threads"][0]["id"],
        own_thread["value"]["id"]
    );
    assert_eq!(
        own_removed["value"]["threads"][0]["name"], "Thread (2)",
        "{own_removed}"
    );
    assert!(!own_removed.to_string().contains(&own_secret));
    // thread_start's opening message is keyed by a hash of its text; removing
    // it must not leave that hash behind as a guess-checking oracle.
    let opened = mara(
        "thread_start",
        json!({"conversationId": general, "fromMessageId": own_id, "name": "Opening", "text": format!("opening {own_secret}")}),
    )
    .await;
    let opening_id = opened["value"]["message"]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("opening message: {opened}"))
        .to_owned();
    let removed_opening = mara("message_delete", json!({"messageId": opening_id})).await;
    assert_eq!(removed_opening["outcome"], "ok", "{removed_opening}");
    let text_keys = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM idempotency_keys WHERE response_body->'message'->>'id' = $1 AND key LIKE 'thread-start.%'",
    )
    .bind(&opening_id)
    .fetch_one(&setup.pool)
    .await
    .expect("idempotency keys");
    assert_eq!(text_keys, 0, "a key derived from the removed text survived");
    let own_audit = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_events WHERE action = 'conversation.mcp_message_removed' AND target_id = $1",
    )
    .bind(own_id.parse::<Uuid>().expect("own"))
    .fetch_one(&setup.pool)
    .await
    .expect("own removal audit");
    assert_eq!(own_audit["as"], "author", "{own_audit}");
    assert_eq!(own_audit["renamed_threads"], 1, "{own_audit}");
    let own_history = mara("message_history", json!({"messageId": own_id})).await;
    assert_eq!(own_history["outcome"], "ok", "{own_history}");
    assert!(!own_history.to_string().contains(&own_secret));
    assert_eq!(
        own_history["value"]["revisions"]
            .as_array()
            .map(|revisions| revisions
                .iter()
                .all(|revision| revision["redacted"] == true && revision["text"] == "")),
        Some(true),
        "{own_history}"
    );
    let replayed = mara(
        "message_edit",
        json!({
            "messageId": own_id,
            "text": format!("token {own_secret} (staging)"),
            "expectedRevision": 1,
            "clientRequestId": "own-edit"
        }),
    )
    .await;
    assert_eq!(replayed["outcome"], "conflict", "{replayed}");
    assert!(!replayed.to_string().contains(&own_secret));
    for (token, session) in [
        (codex.as_str(), session.as_str()),
        (nova_token.as_str(), nova_session.as_str()),
    ] {
        let found = tool(app, token, session, "search", json!({"query": own_secret})).await;
        assert_eq!(found["value"]["results"], json!([]), "{found}");
        let listed = tool(
            app,
            token,
            session,
            "conversation_list",
            json!({"projectId": work, "query": own_secret}),
        )
        .await;
        assert_eq!(listed["value"]["conversations"], json!([]), "{listed}");
    }

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

    // No copy of the text is left in any table that held one, or that
    // could: decisions, Codex runs and sponsored runs keep copies only when
    // someone wrote one there (a run's prompt is the documented exception).
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
        "project_decisions",
        "agent_runs",
        "external_execution_requests",
    ] {
        for needle in [&secret, &own_secret] {
            assert_eq!(
                copies(&setup.pool, table, needle).await,
                0,
                "{table} keeps a copy"
            );
        }
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
    let (mara, mara_token) = create_agent(app, owner_browser, &space, "Mara", "mara").await;
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

    // The same rules through MCP, in plain words.
    let mara_call = |name: &'static str, arguments: Value| {
        let token = mara_token.clone();
        let session = mara_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let codex_call = |name: &'static str, arguments: Value| {
        let token = codex.clone();
        let session = codex_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let refused = codex_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Owner's words"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(refused["message"], "You can only edit your own messages.");
    let edited = mara_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Mara here, fixed", "expectedRevision": 1}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    assert_eq!(edited["value"]["revision"], 2);
    let stale = mara_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Mara here, again", "expectedRevision": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert_eq!(
        stale["message"],
        "This message was edited since you read it (now revision 2). Read it again with conversation_read, then retry."
    );
    let same = mara_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Mara here, fixed"}),
    )
    .await;
    assert_eq!(same["outcome"], "ok", "{same}");
    assert_eq!(same["value"]["changed"], false);
    assert_eq!(same["value"]["revision"], 2);
    let reused = mara_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Mara here, once", "clientRequestId": "mara-1"}),
    )
    .await;
    assert_eq!(reused["outcome"], "ok", "{reused}");
    let reused = mara_call(
        "message_edit",
        json!({"messageId": from_mara, "text": "Mara here, twice", "clientRequestId": "mara-1"}),
    )
    .await;
    assert_eq!(reused["outcome"], "conflict", "{reused}");
    assert_eq!(
        reused["message"],
        "That clientRequestId was already used for a different edit. Use a new one."
    );
    let owners_second = say(
        &setup,
        owner_browser,
        &general,
        "owner-2",
        text("Owner's second"),
    )
    .await;
    set_role(&setup.pool, &work, mara, "admin").await;
    let refused = mara_call("message_delete", json!({"messageId": owners_second})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(
        refused["message"],
        "Agents can only remove their own messages. Ask a project owner or admin to remove this one."
    );
    set_role(&setup.pool, &work, mara, "member").await;
    let pat_app = connect_app(app, &pat_browser, "Codex", "full").await;
    let pat_session = initialize(app, &pat_app).await.expect("Pat's MCP");
    let refused = tool(
        app,
        &pat_app,
        &pat_session,
        "message_delete",
        json!({"messageId": owners_second}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(
        refused["message"],
        "You can only remove your own messages. Project owners and admins can remove anyone's."
    );
    let reader = connect_app(app, owner_browser, "Reader", "read").await;
    let reader_session = initialize(app, &reader).await.expect("Reader MCP");
    for (name, arguments) in [
        (
            "message_edit",
            json!({"messageId": owners_second, "text": "Read only"}),
        ),
        ("message_delete", json!({"messageId": owners_second})),
    ] {
        let refused = tool(app, &reader, &reader_session, name, arguments).await;
        assert_eq!(refused["outcome"], "permission_denied", "{name}: {refused}");
    }
    let history = tool(
        app,
        &reader,
        &reader_session,
        "message_history",
        json!({"messageId": from_mara}),
    )
    .await;
    assert_eq!(history["outcome"], "ok", "{history}");
    let texts = history["value"]["revisions"]
        .as_array()
        .expect("revisions")
        .iter()
        .map(|revision| revision["text"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            json!("Mara here"),
            json!("Mara here, fixed"),
            json!("Mara here, once")
        ]
    );
    assert!(
        history["value"]["revisions"][0]["writtenBy"]
            .as_str()
            .is_some_and(|by| by.ends_with("(@mara, agent)")),
        "{history}"
    );
    let by_owner = codex_call("message_delete", json!({"messageId": owners_second})).await;
    assert_eq!(by_owner["outcome"], "ok", "{by_owner}");
    assert_eq!(by_owner["value"]["alreadyRemoved"], false);
    // An edit retried on the other surface replays: both store the same body.
    let owners_third = say(
        &setup,
        owner_browser,
        &general,
        "owner-3",
        text("Owner's third"),
    )
    .await;
    let (status, on_web) = edit(
        &setup,
        owner_browser,
        &general,
        &owners_third,
        "shared-edit",
        json!({"content": text("Owner's third, fixed")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{on_web}");
    let on_mcp = codex_call(
        "message_edit",
        json!({"messageId": owners_third, "text": "Owner's third, fixed", "clientRequestId": "shared-edit"}),
    )
    .await;
    assert_eq!(on_mcp["outcome"], "ok", "{on_mcp}");
    assert_eq!(on_mcp["value"]["idempotentReplay"], true, "{on_mcp}");
    assert_eq!(on_mcp["value"]["revision"], 2);
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
    let history = codex_call("message_history", json!({"messageId": owners_third})).await;
    assert_eq!(
        history["outcome"], "ok",
        "archived history stays readable through MCP too: {history}"
    );
    assert_eq!(
        history["value"]["revisions"][1]["text"],
        "Owner's third, fixed"
    );
    let refused = codex_call(
        "message_edit",
        json!({"messageId": owners_third, "text": "Too late"}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(
        refused["message"],
        "That conversation is archived. Restore it before editing messages."
    );
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
    let (bob, bob_token) = create_agent(app, &setup.browser, &space, "Bob", "bob").await;
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
    // Through MCP the message doesn't exist for Bob: not found, never denied.
    let bob_session = initialize(app, &bob_token).await.expect("Bob MCP");
    let link = format!("http://127.0.0.1:3100/messages/{group}#message-{message}");
    for (name, arguments) in [
        ("conversation_read", json!({"conversationId": group})),
        ("message_history", json!({"messageId": message})),
        ("message_history", json!({"messageId": link})),
        (
            "message_edit",
            json!({"messageId": message, "text": "Mine now"}),
        ),
        ("message_delete", json!({"messageId": link})),
    ] {
        let refused = tool(app, &bob_token, &bob_session, name, arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{name}: {refused}");
    }
    let found = tool(
        app,
        &bob_token,
        &bob_session,
        "search",
        json!({"query": "Plans"}),
    )
    .await;
    assert_eq!(found["value"]["results"], json!([]), "{found}");
    let briefing = tool(app, &bob_token, &bob_session, "briefing", json!({})).await;
    assert!(!briefing.to_string().contains(&message), "{briefing}");

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
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let codex_session = initialize(app, &codex).await.expect("Codex MCP");
    for (name, arguments) in [
        ("message_history", json!({"messageId": hidden})),
        (
            "message_edit",
            json!({"messageId": hidden, "text": "Mine now"}),
        ),
        ("message_delete", json!({"messageId": hidden})),
    ] {
        let refused = tool(app, &codex, &codex_session, name, arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{name}: {refused}");
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
