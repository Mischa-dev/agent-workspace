//! Working notes (AWD-220): a message that is a record without the tap on
//! the shoulder. The silent failures: a note that makes a conversation
//! unread or notifies its watchers, a note that wakes agents it doesn't
//! address, an addressed account that isn't told, and a note that can't be
//! read by the people who can read everything else there.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{Browser, send, tool},
    callers::Agent,
    world::WorldBuilder,
};

async fn drain(pool: &sqlx::PgPool) {
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("catch up with the log");
}

async fn call(app: &axum::Router, agent: &Agent, name: &str, arguments: Value) -> Value {
    let answer = tool(app, &agent.token, &agent.session, name, arguments).await;
    assert_eq!(answer["outcome"], "ok", "{name}: {answer}");
    answer["value"].clone()
}

async fn asked(pool: &sqlx::PgPool, agent: &Agent) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reply_requests WHERE agent_actor_id = $1")
        .bind(agent.actor)
        .fetch_one(pool)
        .await
        .expect("requests")
}

async fn notified(pool: &sqlx::PgPool, person: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT kind FROM notifications WHERE recipient_actor_id = $1 ORDER BY created_at, kind",
    )
    .bind(person)
    .fetch_all(pool)
    .await
    .expect("notifications")
}

async fn newest_message(pool: &sqlx::PgPool, conversation: &str) -> i64 {
    sqlx::query_scalar("SELECT last_message_sequence FROM conversations WHERE id = $1::uuid")
        .bind(conversation)
        .fetch_one(pool)
        .await
        .expect("conversation")
}

async fn register(pool: &sqlx::PgPool, message: &str) -> String {
    sqlx::query_scalar("SELECT register FROM messages WHERE id = $1::uuid")
        .bind(message)
        .fetch_one(pool)
        .await
        .expect("message")
}

async fn timeline(app: &axum::Router, browser: &Browser, conversation: &str) -> Vec<Value> {
    let (status, page) = send(
        app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{conversation}/timeline?limit=50"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["entries"].as_array().expect("entries").clone()
}

#[tokio::test]
async fn a_note_in_a_channel_is_kept_and_taps_only_who_it_addresses() {
    let Some(world) = WorldBuilder::default()
        .project("WN")
        .person("ben", "member")
        .agent("codex", "member")
        .agent("rook", "member")
        .build("awd220_channel")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("WN").to_owned();
    let ben = &world.people["ben"];
    let codex = &world.agents["codex"];
    let rook = &world.agents["rook"];
    let (status, channel) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("channel-build"),
            json!({ "name": "build" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();

    // A note nobody is named in: recorded, and that is all.
    let sent = call(
        app,
        codex,
        "message_send",
        json!({
            "conversationId": channel, "note": true, "clientRequestId": "note-1",
            "text": "I'll take apps/server, leaving apps/web alone."
        }),
    )
    .await;
    let first = sent["messageId"].as_str().expect("message").to_owned();
    assert_eq!(register(pool, &first).await, "note");
    drain(pool).await;
    assert_eq!(
        newest_message(pool, &channel).await,
        0,
        "a note never makes its conversation unread"
    );
    assert_eq!(asked(pool, rook).await, 0, "nobody was addressed");
    assert!(notified(pool, ben.actor).await.is_empty());
    assert!(notified(pool, world.owner.actor).await.is_empty());

    // A note that names an agent asks that agent, and only that agent.
    call(
        app,
        codex,
        "message_send",
        json!({
            "conversationId": channel, "note": true, "clientRequestId": "note-2",
            "text": "@rook are you touching apps/web/src/conversations today?"
        }),
    )
    .await;
    drain(pool).await;
    assert_eq!(asked(pool, rook).await, 1, "the agent it names is asked");
    assert_eq!(asked(pool, codex).await, 0);
    assert_eq!(newest_message(pool, &channel).await, 0);
    assert!(notified(pool, ben.actor).await.is_empty());

    // Answering a note with a note reaches the author of what it answers.
    let before = asked(pool, codex).await;
    call(
        app,
        rook,
        "message_send",
        json!({
            "conversationId": channel, "note": true, "clientRequestId": "note-3",
            "replyToMessageId": first, "text": "Yes, Timeline.tsx only."
        }),
    )
    .await;
    drain(pool).await;
    assert_eq!(
        asked(pool, codex).await,
        before + 1,
        "the author of the answered note is asked"
    );

    // A note that names a person reaches that person as a mention.
    call(
        app,
        codex,
        "message_send",
        json!({
            "conversationId": channel, "note": true, "clientRequestId": "note-4",
            "text": "@ben we need a decision on the folder name."
        }),
    )
    .await;
    drain(pool).await;
    assert_eq!(notified(pool, ben.actor).await, vec!["mention".to_owned()]);
    assert_eq!(newest_message(pool, &channel).await, 0);

    // Anyone who reads the channel reads its notes, marked as notes: the
    // person in the browser and the other agent through its tools.
    let entries = timeline(app, &ben.browser, &channel).await;
    let notes = entries
        .iter()
        .filter(|entry| entry["message"]["note"] == true)
        .count();
    assert_eq!(notes, 4, "{entries:?}");
    let read = call(
        app,
        rook,
        "conversation_read",
        json!({ "conversationId": channel }),
    )
    .await;
    let messages = read["messages"].as_array().expect("messages");
    assert!(
        messages.iter().any(|message| message["note"] == true
            && message["text"]
                .as_str()
                .is_some_and(|text| text.contains("apps/server"))),
        "{read}"
    );

    // A message that isn't a note still does what messages do.
    let loud = call(
        app,
        codex,
        "message_send",
        json!({
            "conversationId": channel, "clientRequestId": "message-1",
            "text": "Both halves are merged."
        }),
    )
    .await;
    assert_eq!(
        register(pool, loud["messageId"].as_str().expect("message")).await,
        "message"
    );
    assert!(newest_message(pool, &channel).await > 0);
    let entries = timeline(app, &ben.browser, &channel).await;
    assert!(
        entries
            .last()
            .is_some_and(|entry| entry["message"]["note"].is_null()),
        "{entries:?}"
    );

    // Removing the only message leaves the notes, which still don't count.
    let (status, removed) = send(
        app,
        world.owner.browser.request(
            "DELETE",
            &format!(
                "/api/v1/conversations/{channel}/messages/{}",
                loud["messageId"].as_str().expect("message")
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{removed}");
    assert_eq!(newest_message(pool, &channel).await, 0);
}

#[tokio::test]
async fn a_working_channel_makes_notes_the_default_there_and_in_its_threads() {
    let Some(world) = WorldBuilder::default()
        .project("WC")
        .agent("codex", "member")
        .build("awd220_working_channel")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("WC").to_owned();
    let codex = &world.agents["codex"];
    let (status, channel) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("channel-agents"),
            json!({ "name": "agents" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let (status, view) = world
        .owner
        .browser
        .op(
            app,
            "channel_update",
            json!({ "channelId": channel, "workingNotes": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["workingNotes"], true, "{view}");

    let quiet = call(
        app,
        codex,
        "message_send",
        json!({ "conversationId": channel, "clientRequestId": "default-1", "text": "Starting on the parser." }),
    )
    .await;
    let quiet = quiet["messageId"].as_str().expect("message").to_owned();
    assert_eq!(register(pool, &quiet).await, "note");
    assert_eq!(newest_message(pool, &channel).await, 0);

    // A thread there follows the channel.
    let thread = call(
        app,
        codex,
        "thread_start",
        json!({ "conversationId": channel, "fromMessageId": quiet, "name": "Parser" }),
    )
    .await;
    let thread = thread["id"].as_str().expect("thread").to_owned();
    let in_thread = call(
        app,
        codex,
        "message_send",
        json!({ "conversationId": thread, "clientRequestId": "default-2", "text": "Tokens first." }),
    )
    .await;
    assert_eq!(
        register(pool, in_thread["messageId"].as_str().expect("message")).await,
        "note"
    );

    // The sender can still say a message is for everyone.
    let loud = call(
        app,
        codex,
        "message_send",
        json!({
            "conversationId": channel, "note": false, "clientRequestId": "loud-1",
            "text": "The parser is done; please review."
        }),
    )
    .await;
    assert_eq!(
        register(pool, loud["messageId"].as_str().expect("message")).await,
        "message"
    );
    assert!(newest_message(pool, &channel).await > 0);
}

#[tokio::test]
async fn a_note_on_a_task_is_kept_without_notifying_its_watchers() {
    let Some(world) = WorldBuilder::default()
        .project("WT")
        .person("ben", "member")
        .agent("codex", "member")
        .agent("rook", "member")
        .build("awd220_task")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("WT").to_owned();
    let ben = &world.people["ben"];
    let codex = &world.agents["codex"];
    let rook = &world.agents["rook"];
    // Ben is assigned, so he watches the task.
    let (status, task) = crate::support::tasks::send(
        app,
        &world.owner.browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        Some("task-1"),
        json!({ "title": "Split the importer", "assigneeActorId": ben.actor }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    drain(pool).await;
    sqlx::query("DELETE FROM notifications WHERE recipient_actor_id = $1")
        .bind(ben.actor)
        .execute(pool)
        .await
        .expect("clear the assignment");

    call(
        app,
        codex,
        "task_comment_create",
        json!({
            "taskId": task, "note": true, "clientRequestId": "task-note-1",
            "text": "@rook I have importer/parse.rs and its tests; you take importer/write.rs."
        }),
    )
    .await;
    drain(pool).await;
    assert!(
        notified(pool, ben.actor).await.is_empty(),
        "a watcher isn't told about a note"
    );

    // The task still shows it, to the person and to the other agent.
    let opened = call(app, rook, "task_get", json!({ "taskId": task })).await;
    let comments = opened["comments"].as_array().expect("comments");
    assert_eq!(comments.len(), 1, "{opened}");
    assert_eq!(comments[0]["note"], true, "{opened}");
    let comments_conversation = sqlx::query_scalar::<_, String>(
        "SELECT id::text FROM conversations WHERE kind = 'comments' AND subject_kind = 'task' AND subject_id = $1::uuid",
    )
    .bind(&task)
    .fetch_one(pool)
    .await
    .expect("the task's discussion");
    let entries = timeline(app, &ben.browser, &comments_conversation).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["message"]["note"], true);

    // A comment that isn't a note reaches the watcher as before.
    call(
        app,
        codex,
        "task_comment_create",
        json!({ "taskId": task, "clientRequestId": "task-comment-1", "text": "Both halves are up for review." }),
    )
    .await;
    drain(pool).await;
    assert_eq!(
        notified(pool, ben.actor).await,
        vec!["thing_comment".to_owned()]
    );
}
