//! AWD-95: reply to a task comment. The silent failures: a reply quoting a
//! comment from another task (whose readers may differ), a deleted comment's
//! text living on in a quote, and the person answered never hearing of it.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
    message_edits::{add_person, get},
    notifications::{drain, inbox},
};

fn text(value: &str) -> Value {
    json!({"version": 1, "nodes": [{"type": "text", "text": value}]})
}

#[tokio::test]
async fn replies_quote_a_comment_on_the_same_task_and_tell_its_author() {
    let Some(setup) = setup("awd95_replies").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "RPL", "Replies").await;
    let (_mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let base = format!("/api/v1/projects/{project}/work-items");
    let mut tasks = Vec::new();
    for (key, title) in [("task-a", "Pick colours"), ("task-b", "Pick fonts")] {
        let (status, task) = send(
            app,
            setup
                .browser
                .request("POST", &base, Some(key), json!({"title": title})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        tasks.push(task["id"].as_str().expect("task").to_owned());
    }
    let (task_a, task_b) = (&tasks[0], &tasks[1]);
    let comment =
        |browser: &super::agent_interface::Browser, task: &str, key: &str, body: Value| {
            let request =
                browser.request("POST", &format!("{base}/{task}/comments"), Some(key), body);
            send(app, request)
        };
    let (status, asked) = comment(
        &mara_browser,
        task_a,
        "asked",
        json!({"content": text("Teal or navy?")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let asked = asked["comment"]["id"].as_str().expect("asked").to_owned();
    let (status, elsewhere) = comment(
        &setup.browser,
        task_b,
        "elsewhere",
        json!({"content": text("Serif, please")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{elsewhere}");
    let elsewhere = elsewhere["comment"]["id"]
        .as_str()
        .expect("elsewhere")
        .to_owned();

    // A reply shows what it answers.
    let (status, answered) = comment(
        &setup.browser,
        task_a,
        "answered",
        json!({"content": text("Teal"), "replyToCommentId": asked}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answered}");
    assert_eq!(answered["comment"]["replyTo"]["commentId"], asked.as_str());
    assert_eq!(answered["comment"]["replyTo"]["text"], "Teal or navy?");

    // A comment on another task can't be quoted.
    let (status, refused) = comment(
        &setup.browser,
        task_a,
        "cross",
        json!({"content": text("Same here"), "replyToCommentId": elsewhere}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // Mara hears she was answered.
    drain(&setup).await;
    let page = inbox(&setup, &mara_browser, "?category=mentions").await;
    let titles = page["notifications"]
        .as_array()
        .expect("notifications")
        .iter()
        .filter_map(|notification| notification["title"].as_str())
        .collect::<Vec<_>>();
    assert!(
        titles.iter().any(|title| title.contains("replied to you")),
        "{page}"
    );

    // Once she deletes her question, the quote keeps its place but not its
    // text, and it can't be answered again.
    let (status, removed) = send(
        app,
        mara_browser.request(
            "DELETE",
            &format!("{base}/{task_a}/comments/{asked}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    // Retrying the reply returns its stored response, which no longer
    // quotes the deleted text either.
    let (status, replayed) = comment(
        &setup.browser,
        task_a,
        "answered",
        json!({"content": text("Teal"), "replyToCommentId": asked}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{replayed}");
    assert!(!replayed.to_string().contains("navy"), "{replayed}");
    assert_eq!(replayed["comment"]["replyTo"]["removed"], true);
    let (status, timeline) = get(&setup, &mara_browser, &format!("{base}/{task_a}/timeline")).await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    assert!(!timeline.to_string().contains("navy"), "{timeline}");
    let quote = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find_map(|entry| entry["comment"]["replyTo"].as_object())
        .expect("a reply in the timeline");
    assert_eq!(quote["removed"], true);
    assert!(quote["text"].is_null());
    let (status, _) = comment(
        &setup.browser,
        task_a,
        "late",
        json!({"content": text("Still teal"), "replyToCommentId": asked}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The MCP tool keeps to the same task too.
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let crossed = tool(
        app,
        &codex,
        &session,
        "task_comment",
        json!({"taskId": task_a, "text": "Agreed", "clientRequestId": "c1", "replyToCommentId": elsewhere}),
    )
    .await;
    assert_eq!(crossed["outcome"], "invalid_request", "{crossed}");
    let replied = tool(
        app,
        &codex,
        &session,
        "task_comment",
        json!({"taskId": task_b, "text": "Agreed", "clientRequestId": "c2", "replyToCommentId": elsewhere}),
    )
    .await;
    assert_eq!(replied["outcome"], "applied", "{replied}");
    let stored: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT reply_to_comment_id FROM work_item_comments WHERE id = $1::uuid",
    )
    .bind(replied["activityId"].as_str().expect("comment id"))
    .fetch_one(&setup.pool)
    .await
    .expect("reply stored");
    assert_eq!(stored.map(|id| id.to_string()), Some(elsewhere));
}
