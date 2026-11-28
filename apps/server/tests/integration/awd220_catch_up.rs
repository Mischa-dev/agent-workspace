//! `message_list` (AWD-220): one read of everything said in a space after
//! a point. The silent failures: a reader shown a private channel or a DM
//! they aren't in, a page that skips or repeats a message, and a message
//! that names a thing without the thing knowing.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{send, tool},
    callers::Agent,
    world::WorldBuilder,
};

async fn call(app: &axum::Router, agent: &Agent, name: &str, arguments: Value) -> Value {
    let answer = tool(app, &agent.token, &agent.session, name, arguments).await;
    assert_eq!(answer["outcome"], "ok", "{name}: {answer}");
    answer["value"].clone()
}

fn texts(page: &Value) -> Vec<String> {
    page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().expect("text").to_owned())
        .collect()
}

#[tokio::test]
async fn one_read_catches_up_on_a_space_and_shows_only_what_the_reader_can_see() {
    let Some(world) = WorldBuilder::default()
        .project("CU")
        .person("ben", "member")
        .person("mara", "member")
        .agent("codex", "member")
        .agent("rook", "member")
        .dm("ben", "mara")
        .build("awd220_catch_up")
        .await
    else {
        return;
    };
    let app = &world.app;
    let project = world.project("CU").to_owned();
    let ben = &world.people["ben"];
    let codex = &world.agents["codex"];
    let rook = &world.agents["rook"];

    let build = call(
        app,
        codex,
        "channel_create",
        json!({"projectId": project, "name": "build"}),
    )
    .await["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let secret = call(
        app,
        rook,
        "channel_create",
        json!({"projectId": project, "name": "vault", "private": true}),
    )
    .await["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let (status, task) = crate::support::tasks::send(
        app,
        &world.owner.browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        Some(&Uuid::now_v7().to_string()),
        json!({"title": "Split the importer"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();

    call(
        app,
        codex,
        "message_send",
        json!({"conversationId": build, "text": "first, in the channel"}),
    )
    .await;
    call(
        app,
        codex,
        "message_send",
        json!({"conversationId": build, "text": "second, a note", "note": true}),
    )
    .await;
    call(
        app,
        rook,
        "message_send",
        json!({"conversationId": secret, "text": "the vault code is zqvault"}),
    )
    .await;
    call(
        app,
        codex,
        "task_comment_create",
        json!({"taskId": task, "text": "third, on the task"}),
    )
    .await;
    let dm = &world.conversations["ben+mara"];
    let (status, sent) = send(
        app,
        ben.browser.request(
            "POST",
            &format!("/api/v1/conversations/{dm}/messages"),
            Some("catch-up-dm"),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "zqdirect words"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    // A message shows once every write that began before it has finished
    // (other tests' writes too: the server's transactions share one count).
    let mut all = json!({});
    for _ in 0..200 {
        all = call(app, codex, "message_list", json!({"spaceId": project})).await;
        if texts(&all).len() == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        texts(&all),
        [
            "first, in the channel",
            "second, a note",
            "third, on the task"
        ],
        "{all}"
    );
    assert_eq!(all["more"], false);
    assert_eq!(all["messages"][1]["note"], true);
    let about = all["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .find(|conversation| conversation["aboutKind"] == "task")
        .unwrap_or_else(|| panic!("the task's discussion is named: {all}"));
    assert_eq!(about["aboutId"], task.as_str());

    // The private channel reads only for its member; a DM for nobody here.
    let rooks = texts(&call(app, rook, "message_list", json!({"spaceId": project})).await);
    assert!(rooks.contains(&"the vault code is zqvault".to_owned()));
    assert!(!rooks.iter().any(|text| text.contains("zqdirect")));
    let refused = call(
        app,
        codex,
        "message_list",
        json!({"spaceId": project, "conversationId": secret}),
    )
    .await;
    assert_eq!(texts(&refused), Vec::<String>::new());

    // Pages are cut by size, never mid-message, and neither skip nor repeat.
    let mut cursor = None::<String>;
    let mut read = Vec::new();
    for _ in 0..3 {
        let mut input = json!({"spaceId": project, "maxCharacters": 5});
        if let Some(cursor) = &cursor {
            input["cursor"] = json!(cursor);
        }
        let page = call(app, codex, "message_list", input).await;
        assert_eq!(page["messages"].as_array().expect("messages").len(), 1);
        assert_eq!(page["more"], read.len() < 2, "{page}");
        read.extend(texts(&page));
        cursor = page["nextCursor"].as_str().map(str::to_owned);
    }
    assert_eq!(read, texts(&all));
    let end = call(
        app,
        codex,
        "message_list",
        json!({"spaceId": project, "cursor": cursor}),
    )
    .await;
    assert_eq!(texts(&end), Vec::<String>::new());
    assert_eq!(end["nextCursor"], json!(cursor));

    // A send that began before a write still running can't be passed: the
    // read holds back what was written after that write began, and shows
    // it once the write has finished.
    let mut slow = world.pool().begin().await.expect("a transaction");
    sqlx::query("SELECT txid_current()")
        .execute(&mut *slow)
        .await
        .expect("a transaction id");
    call(
        app,
        codex,
        "message_send",
        json!({"conversationId": build, "text": "fourth, behind a slow write"}),
    )
    .await;
    let held = call(
        app,
        codex,
        "message_list",
        json!({"spaceId": project, "cursor": cursor}),
    )
    .await;
    assert_eq!(texts(&held), Vec::<String>::new(), "{held}");
    slow.commit().await.expect("commit");
    let mut shown = Vec::new();
    for _ in 0..200 {
        shown = texts(
            &call(
                app,
                codex,
                "message_list",
                json!({"spaceId": project, "cursor": held["nextCursor"]}),
            )
            .await,
        );
        if !shown.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(shown, ["fourth, behind a slow write"]);

    // Narrowed to one thing's discussion, and without notes.
    let on_task = call(
        app,
        codex,
        "message_list",
        json!({"spaceId": project, "thingId": task}),
    )
    .await;
    assert_eq!(texts(&on_task), ["third, on the task"]);
    let loud = call(
        app,
        codex,
        "message_list",
        json!({"spaceId": project, "conversationId": build, "notes": false}),
    )
    .await;
    assert_eq!(
        texts(&loud),
        ["first, in the channel", "fourth, behind a slow write"]
    );
}

#[tokio::test]
async fn a_message_that_links_to_a_thing_is_listed_on_that_thing() {
    let Some(world) = WorldBuilder::default()
        .project("RF")
        .agent("codex", "member")
        .build("awd220_references")
        .await
    else {
        return;
    };
    let app = &world.app;
    let project = world.project("RF").to_owned();
    let codex = &world.agents["codex"];
    let page = call(
        app,
        codex,
        "page_create",
        json!({"projectId": project, "title": "Importer findings", "body": "Nothing yet."}),
    )
    .await;
    let page_id = page["id"].as_str().expect("page").to_owned();
    let url = page["url"].as_str().expect("url").to_owned();
    let channel = call(
        app,
        codex,
        "channel_create",
        json!({"projectId": project, "name": "build"}),
    )
    .await["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    call(
        app,
        codex,
        "message_send",
        json!({"conversationId": channel, "text": format!("Wrote up the parser bug in {url}"), "note": true}),
    )
    .await;
    let linked = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM links WHERE to_thing_id = $1::uuid AND origin = 'text'",
    )
    .bind(&page_id)
    .fetch_one(world.pool())
    .await
    .expect("links");
    assert_eq!(linked, 1, "the page knows the note is about it");
}
