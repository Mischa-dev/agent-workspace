//! W20: files on messages and tasks, and link previews. The costly failures
//! are silent: a file in a DM or private project opening for someone outside
//! it (by browser or over MCP), a removed message's files staying readable,
//! someone sending another person's upload, and a hidden preview coming back.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{Browser, create_agent, creates_once, initialize, send, test_app_with, tool},
    mcp_admin::{Setup, create_project, setup},
    mcp_tasks::{add_member, space},
    message_edits::{add_person, create_channel, text},
};

pub(super) async fn with_data_root(prefix: &str) -> Option<(Setup, tempfile::TempDir)> {
    let mut setup = setup(prefix).await?;
    let root = tempfile::tempdir().expect("data root");
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    setup.app = test_app_with(
        setup.pool.clone(),
        &database_url,
        &[(
            "AGENT_WORKSPACE_DATA_ROOT",
            root.path().to_string_lossy().into_owned(),
        )],
    );
    Some((setup, root))
}

pub(super) async fn upload(
    setup: &Setup,
    browser: &Browser,
    uri: &str,
    content_type: &str,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("origin", "http://127.0.0.1:3100")
        .header("cookie", &browser.cookie)
        .header("x-csrf-token", &browser.csrf)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes.to_vec()))
        .expect("upload");
    send(&setup.app, request).await
}

pub(super) async fn upload_to(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    name: &str,
) -> String {
    let (status, file) = upload(
        setup,
        browser,
        &format!("/api/v1/conversations/{conversation}/files?name={name}"),
        "text/plain",
        format!("contents of {name}").as_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    file["id"].as_str().expect("file").to_owned()
}

/// The file's bytes as `browser` gets them, or the status.
pub(super) async fn read(
    setup: &Setup,
    browser: &Browser,
    file: &str,
) -> Result<Vec<u8>, StatusCode> {
    let response = setup
        .app
        .clone()
        .oneshot(browser.request(
            "GET",
            &format!("/api/v1/files/{file}/content"),
            None,
            Value::Null,
        ))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000)
        .await
        .expect("body");
    if status == StatusCode::OK {
        Ok(bytes.to_vec())
    } else {
        Err(status)
    }
}

pub(super) async fn send_with(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    key: &str,
    words: &str,
    files: &[&str],
) -> (StatusCode, Value) {
    send(
        &setup.app,
        browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(key),
            json!({"content": text(words), "attachments": files}),
        ),
    )
    .await
}

async fn timeline_entry(
    setup: &Setup,
    browser: &Browser,
    conversation: &str,
    message: &str,
) -> Value {
    let (status, timeline) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/conversations/{conversation}/timeline"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{timeline}");
    timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["id"] == message)
        .cloned()
        .unwrap_or_else(|| panic!("message {message} not in {timeline}"))
}

#[tokio::test]
async fn uploading_again_stores_one_file() {
    let Some((setup, _root)) = with_data_root("w20_retry").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (agent, token) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, agent, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let channel = create_channel(&setup, &work, "general").await;
    creates_once(
        &setup.app,
        &setup.pool,
        &token,
        &session,
        "file_upload",
        json!({"conversationId": channel, "name": "run.log", "text": "all green", "clientRequestId": "upload-retry-1"}),
        json!({"conversationId": channel, "name": "run.log", "text": "all red", "clientRequestId": "upload-retry-1"}),
        "SELECT count(*) FROM files",
    )
    .await;
}

/// Keyed creates hold one connection each, never one while taking another:
/// on a pool of two, six uploads and six scheduled messages side by side,
/// each sent twice with its key, all finish, and each makes one thing.
#[tokio::test]
async fn keyed_creates_side_by_side_fit_a_small_pool() {
    let Some((setup, root)) = with_data_root("w20_small_pool").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let space = space(&setup).await;
    let (agent, token) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    add_member(&setup, &work, agent, "member").await;
    let channel = create_channel(&setup, &work, "general").await;
    let small = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect_with((*setup.pool.connect_options()).clone())
        .await
        .expect("small pool");
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let app = test_app_with(
        small.clone(),
        &database_url,
        &[(
            "AGENT_WORKSPACE_DATA_ROOT",
            root.path().to_string_lossy().into_owned(),
        )],
    );
    let send_at = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    // Six uploads and six schedules, each sent twice at once with the same
    // key, every call on its own MCP session (the helper reuses one JSON-RPC
    // id, which one session can't tell apart).
    let mut calls = vec![];
    for index in 0..6 {
        let upload = json!({
            "conversationId": channel, "name": format!("run-{index}.log"),
            "text": "all green", "clientRequestId": format!("small-upload-{index}"),
        });
        let schedule = json!({
            "conversationId": channel, "text": format!("Standup {index}"),
            "sendAt": send_at, "clientRequestId": format!("small-schedule-{index}"),
        });
        for (name, arguments) in [
            ("file_upload", upload.clone()),
            ("file_upload", upload),
            ("message_schedule", schedule.clone()),
            ("message_schedule", schedule),
        ] {
            let session = initialize(&app, &token).await.expect("MCP session");
            calls.push((name, arguments, session));
        }
    }
    let answers =
        futures_util::future::join_all(calls.iter().map(|(name, arguments, session)| {
            tool(&app, &token, session, name, arguments.clone())
        }))
        .await;
    for answer in &answers {
        assert_eq!(answer["outcome"], "ok", "{answer}");
    }
    let (files, scheduled) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM files), (SELECT count(*) FROM scheduled_messages)",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("counts");
    assert_eq!((files, scheduled), (6, 6));
    small.close().await;
}

#[tokio::test]
async fn files_open_only_where_their_message_or_task_does() {
    let Some((setup, _root)) = with_data_root("w20_files").await else {
        return;
    };
    let space = space(&setup).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTH", "Other").await;
    let (rook_id, rook) = add_person(&setup, &[&space, &work], "rook").await;
    let (_wren_id, wren) = add_person(&setup, &[&space, &work], "wren").await;
    let (_mara_id, mara) = add_person(&setup, &[&other], "mara").await;
    let owner = &setup.browser;

    // A DM between the owner and Rook: Wren is in the same places but not in
    // the DM.
    let (status, dm) = send(
        &setup.app,
        owner.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({"actorId": rook_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();

    // Wren can't upload into a DM they aren't in.
    let (status, _) = upload(
        &setup,
        &wren,
        &format!("/api/v1/conversations/{dm}/files?name=x.txt"),
        "text/plain",
        b"x",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Before it's sent, an upload is its uploader's alone.
    let file = upload_to(&setup, owner, &dm, "salaries.txt").await;
    assert!(read(&setup, owner, &file).await.is_ok());
    assert_eq!(read(&setup, &rook, &file).await, Err(StatusCode::NOT_FOUND));
    // Nobody else can send it, even into the same DM.
    let (status, refused) = send_with(&setup, &rook, &dm, "steal", "mine", &[&file]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // Sent (with no text), it opens for the DM and nobody else.
    let (status, sent) = send_with(&setup, owner, &dm, "dm-file", "", &[&file]).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let message = sent["message"]["id"].as_str().expect("message").to_owned();
    assert_eq!(sent["message"]["attachments"][0]["name"], "salaries.txt");
    assert_eq!(
        read(&setup, &rook, &file).await.expect("Rook reads"),
        b"contents of salaries.txt"
    );
    assert_eq!(read(&setup, &wren, &file).await, Err(StatusCode::NOT_FOUND));
    assert_eq!(read(&setup, &mara, &file).await, Err(StatusCode::NOT_FOUND));
    // Sent once, it can't be attached again.
    let (status, _) = send_with(&setup, owner, &dm, "again", "again", &[&file]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let entry = timeline_entry(&setup, &rook, &dm, &message).await;
    assert_eq!(entry["message"]["attachments"][0]["id"], file.as_str());

    // Search finds it by name for Rook, not for Wren.
    for (browser, expected) in [(&rook, true), (&wren, false)] {
        let (status, found) = send(
            &setup.app,
            browser.request(
                "GET",
                "/api/v1/search?q=salaries&kinds=files",
                None,
                Value::Null,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{found}");
        let hit = found["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .any(|hit| hit["id"] == file.as_str());
        assert_eq!(hit, expected, "{found}");
    }

    // Removing the message removes its file from view.
    let (status, removed) = send(
        &setup.app,
        owner.request(
            "DELETE",
            &format!("/api/v1/conversations/{dm}/messages/{message}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(read(&setup, &rook, &file).await, Err(StatusCode::NOT_FOUND));
    assert_eq!(read(&setup, owner, &file).await, Err(StatusCode::NOT_FOUND));
    let entry = timeline_entry(&setup, &rook, &dm, &message).await;
    assert!(entry["message"].get("attachments").is_none(), "{entry}");

    // A task's file opens for its project's members only.
    let (status, task) = send(
        &setup.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{work}/work-items"),
            Some("task-1"),
            json!({"title": "Ship it"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let (status, task_file) = upload(
        &setup,
        owner,
        &format!("/api/v1/projects/{work}/work-items/{task}/files?name=plan.md"),
        "text/markdown",
        b"# Plan",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_file}");
    let task_file = task_file["id"].as_str().expect("file").to_owned();
    assert!(read(&setup, &wren, &task_file).await.is_ok());
    assert_eq!(
        read(&setup, &mara, &task_file).await,
        Err(StatusCode::NOT_FOUND)
    );
    let (status, listed) = send(
        &setup.app,
        mara.request(
            "GET",
            &format!("/api/v1/projects/{work}/work-items/{task}/files"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{listed}");

    // Over MCP: an agent in the project reads the task's file and a
    // channel's, never the DM's, and posts a file of its own.
    let (agent, token) = create_agent(&setup.app, owner, &space, "Mara bot", "marabot").await;
    add_member(&setup, &work, agent, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let channel = create_channel(&setup, &work, "general").await;
    let shared = upload_to(&setup, owner, &channel, "shared.txt").await;
    let (status, _) = send_with(&setup, owner, &channel, "shared", "here", &[&shared]).await;
    assert_eq!(status, StatusCode::CREATED);
    let dm_file = upload_to(&setup, owner, &dm, "private.txt").await;
    let (status, _) = send_with(&setup, owner, &dm, "private", "secret", &[&dm_file]).await;
    assert_eq!(status, StatusCode::CREATED);

    let read_tool = tool(
        &setup.app,
        &token,
        &session,
        "file_read",
        json!({"fileId": shared}),
    )
    .await;
    assert_eq!(read_tool["outcome"], "ok", "{read_tool}");
    assert_eq!(read_tool["value"]["text"], "contents of shared.txt");
    let read_tool = tool(
        &setup.app,
        &token,
        &session,
        "file_read",
        json!({"fileId": task_file}),
    )
    .await;
    assert_eq!(read_tool["value"]["text"], "# Plan", "{read_tool}");
    for hidden in [&dm_file, &file] {
        let refused = tool(
            &setup.app,
            &token,
            &session,
            "file_read",
            json!({"fileId": hidden}),
        )
        .await;
        assert_ne!(refused["outcome"], "ok", "{refused}");
        assert!(!refused.to_string().contains("private.txt"), "{refused}");
    }
    let refused = tool(
        &setup.app,
        &token,
        &session,
        "file_upload",
        json!({"conversationId": dm, "name": "x.txt", "text": "x"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    let uploaded = tool(
        &setup.app,
        &token,
        &session,
        "file_upload",
        json!({"conversationId": channel, "name": "run-output.log", "text": "all green"}),
    )
    .await;
    assert_eq!(uploaded["outcome"], "ok", "{uploaded}");
    let output = uploaded["value"]["fileId"]
        .as_str()
        .expect("file")
        .to_owned();
    // Unsent, the owner can't see the agent's upload yet.
    assert_eq!(
        read(&setup, owner, &output).await,
        Err(StatusCode::NOT_FOUND)
    );
    let posted = tool(
        &setup.app,
        &token,
        &session,
        "message_send",
        json!({"conversationId": channel, "attachmentIds": [output]}),
    )
    .await;
    assert_eq!(posted["outcome"], "ok", "{posted}");
    assert_eq!(
        read(&setup, owner, &output).await.expect("owner reads"),
        b"all green"
    );
    let listed = tool(
        &setup.app,
        &token,
        &session,
        "file_list",
        json!({"messageId": posted["value"]["messageId"]}),
    )
    .await;
    assert_eq!(
        listed["value"]["files"][0]["name"], "run-output.log",
        "{listed}"
    );
    let opened = tool(
        &setup.app,
        &token,
        &session,
        "conversation_read",
        json!({"conversationId": channel}),
    )
    .await;
    assert!(
        opened.to_string().contains("run-output.log"),
        "attachments show in conversation_read: {opened}"
    );

    // On a task: the agent adds a file to the task and one to a comment, and
    // task_open lists both where they belong. Someone else's upload can't be
    // posted on its comment.
    let own = tool(
        &setup.app,
        &token,
        &session,
        "file_upload",
        json!({"taskId": task, "name": "notes.txt", "text": "task notes"}),
    )
    .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    let for_comment = tool(
        &setup.app,
        &token,
        &session,
        "file_upload",
        json!({"taskId": task, "forComment": true, "name": "trace.log", "text": "trace"}),
    )
    .await;
    assert_eq!(for_comment["outcome"], "ok", "{for_comment}");
    let for_comment = for_comment["value"]["fileId"]
        .as_str()
        .expect("file")
        .to_owned();
    // Held for a comment, nobody else sees it yet.
    assert_eq!(
        read(&setup, owner, &for_comment).await,
        Err(StatusCode::NOT_FOUND)
    );
    let (status, owners_upload) = upload(
        &setup,
        owner,
        &format!("/api/v1/projects/{work}/work-items/{task}/files?name=mine.txt&for=comment"),
        "text/plain",
        b"mine",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{owners_upload}");
    let refused = tool(
        &setup.app,
        &token,
        &session,
        "task_comment",
        json!({
            "taskId": task,
            "text": "taking someone's file",
            "clientRequestId": "w20-comment-refused",
            "attachmentIds": [owners_upload["id"]],
        }),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let commented = tool(
        &setup.app,
        &token,
        &session,
        "task_comment",
        json!({
            "taskId": task,
            "text": "Trace attached",
            "clientRequestId": "w20-comment",
            "attachmentIds": [for_comment],
        }),
    )
    .await;
    assert_eq!(commented["outcome"], "applied", "{commented}");
    assert_eq!(
        read(&setup, owner, &for_comment)
            .await
            .expect("owner reads"),
        b"trace"
    );
    let opened = tool(
        &setup.app,
        &token,
        &session,
        "task_open",
        json!({"taskId": task}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let names = |files: &Value| {
        files
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|file| file["name"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        names(&opened["value"]["files"]),
        ["plan.md", "notes.txt"],
        "{opened}"
    );
    let comment = opened["value"]["comments"]
        .as_array()
        .and_then(|comments| comments.iter().find(|c| c["text"] == "Trace attached"))
        .expect("the comment");
    assert_eq!(names(&comment["files"]), ["trace.log"], "{opened}");
    assert!(
        !opened.to_string().contains("mine.txt"),
        "an unsent upload stays out of task_open: {opened}"
    );
}

#[tokio::test]
async fn link_previews_show_once_ready_and_stay_hidden() {
    let Some((setup, _root)) = with_data_root("w20_previews").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let (_rook_id, rook) = add_person(&setup, &[&work], "rook").await;
    let owner = &setup.browser;
    let channel = create_channel(&setup, &work, "general").await;
    let (status, sent) = send_with(
        &setup,
        owner,
        &channel,
        "link",
        "Look at https://example.com/post, and https://example.com/post again.",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let message = sent["message"]["id"].as_str().expect("message").to_owned();
    let previews = sqlx::query_scalar::<_, Uuid>(
        "SELECT link_preview_id FROM message_link_previews WHERE message_id = $1",
    )
    .bind(Uuid::parse_str(&message).expect("message"))
    .fetch_all(&setup.pool)
    .await
    .expect("previews");
    assert_eq!(previews.len(), 1, "one preview per distinct link");
    let preview = previews[0];
    let entry = timeline_entry(&setup, &rook, &channel, &message).await;
    assert!(
        entry["message"].get("linkPreviews").is_none(),
        "pending stays hidden"
    );

    sqlx::query(
        "UPDATE link_previews SET status = 'ready', title = 'A post', fetched_at = now() WHERE id = $1",
    )
    .bind(preview)
    .execute(&setup.pool)
    .await
    .expect("ready");
    let entry = timeline_entry(&setup, &rook, &channel, &message).await;
    assert_eq!(
        entry["message"]["linkPreviews"][0]["title"], "A post",
        "{entry}"
    );

    // Only the author (or a moderator) hides it.
    let hide =
        format!("/api/v1/conversations/{channel}/messages/{message}/link-previews/{preview}");
    let (status, _) = send(&setup.app, rook.request("DELETE", &hide, None, Value::Null)).await;
    assert!(status.is_client_error(), "{status}");
    let (status, _) = send(
        &setup.app,
        owner.request("DELETE", &hide, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let entry = timeline_entry(&setup, &rook, &channel, &message).await;
    assert!(entry["message"].get("linkPreviews").is_none(), "{entry}");
}

/// K5.11: an upload that isn't a picture streams into the store as it
/// arrives, and whatever the server refuses (too big, over the workspace's
/// room) leaves no bytes behind; bytes nothing ever recorded are swept.
#[tokio::test]
async fn streamed_uploads_keep_nothing_they_refuse() {
    use std::{collections::BTreeMap, sync::Arc};

    use agent_workspace_server::{
        AppState, config::AppConfig, files, provider::SupervisorRegistry, rate_limit::RateLimiters,
        realtime::RealtimeHub,
    };
    use sha2::{Digest, Sha256};

    let Some((setup, root)) = with_data_root("w20_streamed").await else {
        return;
    };
    let work = create_project(&setup, "WORK", "Work").await;
    let channel = create_channel(&setup, &work, "general").await;
    let owner = &setup.browser;
    let stored = |root: &std::path::Path| {
        let folder = root
            .join("files")
            .join(setup.organization_id.simple().to_string());
        std::fs::read_dir(folder)
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0)
    };
    let streamed = |uri: String, chunks: usize| {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("origin", "http://127.0.0.1:3100")
            .header("cookie", &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header(header::CONTENT_TYPE, "text/csv")
            .body(Body::from_stream(futures_util::stream::iter(
                (0..chunks).map(|index| {
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(format!(
                        "{index:08},{}\n",
                        "x".repeat(16 * 1024 - 10)
                    )))
                }),
            )))
            .expect("upload");
        send(&setup.app, request)
    };

    // Twenty chunks of 16 KiB arrive and are kept, with their digest.
    let (status, file) = streamed(
        format!("/api/v1/conversations/{channel}/files?name=big.csv"),
        20,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    assert_eq!(file["size"], 20 * 16 * 1024);
    assert_eq!(file["contentType"], "text/csv");
    let id = file["id"].as_str().expect("file");
    let bytes = read(&setup, owner, id).await.expect("read back");
    assert_eq!(bytes.len(), 20 * 16 * 1024);
    let (sha256, store_id): (String, String) =
        sqlx::query_as("SELECT sha256, store_id FROM files WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&setup.pool)
            .await
            .expect("row");
    assert_eq!(sha256, hex::encode(Sha256::digest(&bytes)));
    assert_eq!(store_id, "local");
    assert_eq!(stored(root.path()), 1);

    // A 1 MB cap and room: too big is refused mid-stream, over the room is
    // refused once counted, and neither keeps a byte.
    for key in ["files.max_upload_mb", "files.workspace_quota_mb"] {
        sqlx::query(
            r#"
            INSERT INTO settings (id, organization_id, scope, key, value)
            VALUES ($1, $2, 'workspace', $3, $4)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(setup.organization_id)
        .bind(key)
        .bind(json!(1))
        .execute(&setup.pool)
        .await
        .expect("setting");
    }
    let (status, _) = streamed(
        format!("/api/v1/conversations/{channel}/files?name=huge.csv"),
        80,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, refused) = streamed(
        format!("/api/v1/conversations/{channel}/files?name=over.csv"),
        50,
    )
    .await;
    // The workspace's storage limit is metering's (H1.26): refused with its
    // numbers.
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");
    assert_eq!(refused["error"]["code"], "limit_reached", "{refused}");
    assert_eq!(stored(root.path()), 1, "refused uploads keep nothing");
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM file_objects_pending")
        .fetch_one(&setup.pool)
        .await
        .expect("pending");
    assert_eq!(pending, 0);

    // Bytes written an hour ago that no row claimed are swept; fresh ones
    // (an upload still arriving) are left alone.
    let folder = root
        .path()
        .join("files")
        .join(setup.organization_id.simple().to_string());
    for (name, age) in [("0a", 2), ("0b", 0)] {
        let key = format!("{}/{name}", setup.organization_id.simple());
        std::fs::write(folder.join(name), b"left behind").expect("orphan");
        sqlx::query(
            r#"
            INSERT INTO file_objects_pending (storage_key, workspace_id, store_id, started_at)
            VALUES ($1, $2, 'local', now() - make_interval(hours => $3))
            "#,
        )
        .bind(&key)
        .bind(setup.organization_id)
        .bind(age)
        .execute(&setup.pool)
        .await
        .expect("pending");
    }
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
    ]))
    .expect("configuration");
    let state = AppState {
        supervisors: SupervisorRegistry::new(&config).expect("supervisors"),
        config: Arc::new(config),
        pool: setup.pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
    };
    files::cleanup(&state).await.expect("cleanup");
    assert!(!folder.join("0a").exists(), "an old orphan is swept");
    assert!(folder.join("0b").exists(), "a fresh one is left");
    assert!(read(&setup, owner, id).await.is_ok(), "a kept file stays");
    let left: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM file_objects_pending")
        .fetch_all(&setup.pool)
        .await
        .expect("pending");
    assert_eq!(left, [format!("{}/0b", setup.organization_id.simple())]);
}

/// C1.33: a page keeps files of its own, such as a picture pasted into it.
/// They open for whoever can see the page and for nobody else, by browser
/// or over MCP; message and task files read as before (above).
#[tokio::test]
async fn a_page_keeps_its_own_files() {
    let Some((setup, _root)) = with_data_root("c133_page_files").await else {
        return;
    };
    let space = space(&setup).await;
    let work = create_project(&setup, "WORK", "Work").await;
    let other = create_project(&setup, "OTH", "Other").await;
    let (_rook_id, rook) = add_person(&setup, &[&space, &work], "rook").await;
    let (_mara_id, mara) = add_person(&setup, &[&space, &other], "mara").await;
    let owner = &setup.browser;
    let (status, page) = send(
        &setup.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            None,
            json!({"title": "Plan", "body": "Hello"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let page = page["id"].as_str().expect("page").to_owned();

    // A picture of the page, and a text file with a preview.
    let png = super::support::two_workspaces::tiny_png();
    let (status, picture) = upload(
        &setup,
        owner,
        &format!("/api/v1/things/page/{page}/files?name=plan.png"),
        "image/png",
        &png,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{picture}");
    assert!(picture["width"].as_i64().is_some(), "{picture}");
    let picture = picture["id"].as_str().expect("file").to_owned();
    let (status, notes) = upload(
        &setup,
        owner,
        &format!("/api/v1/things/page/{page}/files?name=notes.md"),
        "text/markdown",
        b"# Notes\n\nline two",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{notes}");
    assert_eq!(notes["preview"], "# Notes\n\nline two", "{notes}");
    let notes = notes["id"].as_str().expect("file").to_owned();

    // Someone outside the page's project can't add to it, and a kind that
    // keeps no files is refused.
    let (status, _) = upload(
        &setup,
        &mara,
        &format!("/api/v1/things/page/{page}/files?name=x.txt"),
        "text/plain",
        b"x",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = upload(
        &setup,
        owner,
        &format!("/api/v1/things/sheet/{page}/files?name=x.txt"),
        "text/plain",
        b"x",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Readable by the page's readers only.
    assert_eq!(
        read(&setup, &rook, &picture).await.expect("Rook reads"),
        png
    );
    assert_eq!(
        read(&setup, &mara, &picture).await,
        Err(StatusCode::NOT_FOUND)
    );
    for (browser, expected) in [(&rook, StatusCode::OK), (&mara, StatusCode::NOT_FOUND)] {
        let (status, described) = send(
            &setup.app,
            browser.request("GET", &format!("/api/v1/files/{notes}"), None, Value::Null),
        )
        .await;
        assert_eq!(status, expected, "{described}");
    }

    // An agent in the project adds one over MCP and reads it back.
    let (agent, token) = create_agent(&setup.app, owner, &space, "Wren", "wren").await;
    add_member(&setup, &work, agent, "member").await;
    let session = initialize(&setup.app, &token).await.expect("MCP session");
    let added = tool(
        &setup.app,
        &token,
        &session,
        "file_upload",
        json!({"thing": {"kind": "page", "id": page}, "name": "run.log", "text": "all green"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let read_back = tool(
        &setup.app,
        &token,
        &session,
        "file_read",
        json!({"fileId": added["value"]["fileId"]}),
    )
    .await;
    assert_eq!(read_back["outcome"], "ok", "{read_back}");

    // Its uploader removes a page's file; nobody else does.
    let (status, _) = send(
        &setup.app,
        rook.request(
            "DELETE",
            &format!("/api/v1/files/{notes}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &setup.app,
        owner.request(
            "DELETE",
            &format!("/api/v1/files/{notes}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        read(&setup, owner, &notes).await,
        Err(StatusCode::NOT_FOUND)
    );
}
