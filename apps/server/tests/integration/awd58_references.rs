//! AWD-58: "Referenced in". The silent, costly failures: a link written in a
//! private conversation showing (with its name) to someone outside it, and a
//! removed link still listed.

use super::{
    agent_interface::{Browser, connect_app, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::{add_person, create_channel, direct_space, edit, say, text},
};
use axum::http::StatusCode;
use serde_json::{Value, json};

/// The things listed under "Referenced in" on a thing, as (kind, id).
async fn referenced_in(
    setup: &Setup,
    browser: &Browser,
    kind: &str,
    id: &str,
) -> Vec<(String, String)> {
    let (status, opened) = send(
        &setup.app,
        browser.request(
            "GET",
            &format!("/api/v1/things/{kind}/{id}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let mut found = opened["links"]
        .as_array()
        .expect("links")
        .iter()
        .filter(|link| link["label"] == "Referenced in")
        .map(|link| {
            (
                link["other"]["kind"].as_str().expect("kind").to_owned(),
                link["other"]["id"].as_str().expect("id").to_owned(),
            )
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

async fn create(setup: &Setup, path: &str, key: &str, body: Value) -> String {
    let (status, created) = send(
        &setup.app,
        setup.browser.request("POST", path, Some(key), body),
    )
    .await;
    assert!(status.is_success(), "{created}");
    created["id"]
        .as_str()
        .or_else(|| created["comment"]["id"].as_str())
        .or_else(|| created["page"]["id"].as_str())
        .unwrap_or_else(|| panic!("{created}"))
        .to_owned()
}

#[tokio::test]
async fn links_written_anywhere_show_where_the_reader_can_see_them() {
    let Some(setup) = setup("awd58_refs").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let direct = direct_space(&setup).await;
    let (_bea, bea_browser) = add_person(&setup, &[&work, &direct], "bea").await;
    let (carl, _) = add_person(&setup, &[&direct], "carl").await;
    let task = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items"),
        "awd58-task",
        json!({"title": "Ship the release"}),
    )
    .await;
    let task_link = format!("https://example.test/projects/{work}/work-items/{task}");

    // A channel both can read, a DM only the owner and Carl are in, a page,
    // another task's description and a comment on it all link to the task.
    let general = create_channel(&setup, &work, "general").await;
    let in_general = say(
        &setup,
        &setup.browser,
        &general,
        "awd58-general",
        text(&format!("Status is in {task_link} today")),
    )
    .await;
    let (status, dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": carl})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    say(
        &setup,
        &setup.browser,
        &dm,
        "awd58-dm",
        text(&format!("Between us: /projects/{work}/work-items/{task}")),
    )
    .await;
    let page = create(
        &setup,
        &format!("/api/v1/projects/{work}/pages"),
        "awd58-page",
        json!({"title": "Release notes", "body": format!("Tracked in [the task]({task_link}).")}),
    )
    .await;
    let other = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items"),
        "awd58-other",
        json!({
            "title": "Write the changelog",
            "description": format!("Follows /projects/{work}/work-items/{task}"),
        }),
    )
    .await;
    let commented = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items"),
        "awd58-commented",
        json!({"title": "Plan the launch"}),
    )
    .await;
    let comment = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items/{commented}/comments"),
        "awd58-comment",
        json!({"content": {"version": 1, "nodes": [
            {"type": "text", "text": "Waiting on "},
            {"type": "work_item_ref", "workItemId": task}
        ]}}),
    )
    .await;
    assert!(!comment.is_empty());

    let mut everywhere = vec![
        ("channel".to_owned(), general.clone()),
        ("conversation".to_owned(), dm.clone()),
        ("page".to_owned(), page.clone()),
        ("task".to_owned(), other.clone()),
        ("task".to_owned(), commented.clone()),
    ];
    everywhere.sort();
    assert_eq!(
        referenced_in(&setup, &setup.browser, "task", &task).await,
        everywhere
    );
    // Bea isn't in the DM: it isn't listed for her, by id or by name.
    let for_bea = referenced_in(&setup, &bea_browser, "task", &task).await;
    assert!(!for_bea.iter().any(|(_, id)| *id == dm), "{for_bea:?}");
    assert_eq!(for_bea.len(), everywhere.len() - 1, "{for_bea:?}");

    // The page reads the other way too, and a page links back to itself
    // never.
    let (status, opened) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/things/page/{page}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert!(
        opened["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["direction"] == "outgoing" && link["other"]["id"] == task.as_str()),
        "{opened}"
    );

    // A task whose comment links another reads "About" it.
    let (status, opened) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/things/task/{commented}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert!(
        opened["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["direction"] == "outgoing" && link["other"]["id"] == task.as_str()),
        "{opened}"
    );

    // A thread in a DM opens at its parent's address; the link counts as the
    // thread.
    let parent = uuid::Uuid::now_v7();
    let thread = uuid::Uuid::now_v7();
    let found: Vec<(String, uuid::Uuid)> = sqlx::query_as(
        "SELECT target_kind, target_id FROM thing_reference_targets($1) ORDER BY target_kind",
    )
    .bind(format!(
        "See https://work.example/messages/{parent}?view=all&thread={thread} and /messages/{parent}."
    ))
    .fetch_all(&setup.pool)
    .await
    .expect("targets");
    assert_eq!(
        found,
        vec![
            ("conversation".to_owned(), parent),
            ("thread".to_owned(), thread)
        ]
    );

    // Editing the link out of a message takes it off the list.
    let (status, edited) = edit(
        &setup,
        &setup.browser,
        &general,
        &in_general,
        "awd58-general-edit",
        json!({"content": text("Status moved elsewhere"), "expectedRevision": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let after = referenced_in(&setup, &setup.browser, "task", &task).await;
    assert!(!after.iter().any(|(_, id)| *id == general), "{after:?}");

    // An archived page keeps no links.
    sqlx::query("UPDATE project_pages SET archived_at = now() WHERE id = $1::uuid")
        .bind(&page)
        .execute(&setup.pool)
        .await
        .expect("archive page");
    let after = referenced_in(&setup, &setup.browser, "task", &task).await;
    assert!(!after.iter().any(|(_, id)| *id == page), "{after:?}");

    // What stays stored: the DM and the other task's description.
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thing_references WHERE target_id = $1::uuid AND target_kind = 'task'",
    )
    .bind(&task)
    .fetch_one(&setup.pool)
    .await
    .expect("stored references");
    assert_eq!(stored, 2, "the DM and the other task's description");
}

/// AWD-112: a collection record lists where it's linked from, and a link
/// written in a DM stays out of sight for someone outside it.
#[tokio::test]
async fn a_record_lists_where_it_is_linked_from() {
    let Some(setup) = setup("awd112_refs").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let direct = direct_space(&setup).await;
    let (_bea, bea_browser) = add_person(&setup, &[&work, &direct], "bea").await;
    let (carl, _) = add_person(&setup, &[&direct], "carl").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let created = tool(
        app,
        &codex,
        &session,
        "collection_write",
        json!({"projectId": "WORK", "name": "Playtest notes", "fields": [{"name": "Notes", "type": "text"}]}),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let written = tool(
        app,
        &codex,
        &session,
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtest notes",
            "records": [{"title": "Checkpoint skipped"}],
            "clientRequestId": "awd112-record",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let record = written["value"]["records"][0]["id"]
        .as_str()
        .expect("record")
        .to_owned();
    // The record's own address, as the app copies it.
    let (status, opened) = send(
        app,
        setup.browser.request(
            "GET",
            &format!("/api/v1/things/record/{record}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let href = opened["thing"]["href"]
        .as_str()
        .or_else(|| opened["href"].as_str())
        .unwrap_or_else(|| panic!("{opened}"))
        .to_owned();
    assert!(href.contains(&format!("record={record}")), "{href}");
    let link = format!("https://example.test{href}");

    // A channel, a DM Bea isn't in, a page, a task's description and a
    // comment on another task all link to it.
    let general = create_channel(&setup, &work, "general").await;
    say(
        &setup,
        &setup.browser,
        &general,
        "awd112-general",
        text(&format!("Seen again: {link}")),
    )
    .await;
    let (status, dm) = send(
        app,
        setup
            .browser
            .request("POST", "/api/v1/direct/dms", None, json!({"actorId": carl})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("dm").to_owned();
    say(
        &setup,
        &setup.browser,
        &dm,
        "awd112-dm",
        text(&format!("Between us: {href}")),
    )
    .await;
    let page = create(
        &setup,
        &format!("/api/v1/projects/{work}/pages"),
        "awd112-page",
        json!({"title": "Playtest summary", "body": format!("See [the note]({link}).")}),
    )
    .await;
    let described = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items"),
        "awd112-described",
        json!({"title": "Fix the checkpoint", "description": format!("Reported in {link}")}),
    )
    .await;
    let commented = create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items"),
        "awd112-commented",
        json!({"title": "Plan the next playtest"}),
    )
    .await;
    create(
        &setup,
        &format!("/api/v1/projects/{work}/work-items/{commented}/comments"),
        "awd112-comment",
        json!({"content": {"version": 1, "nodes": [{"type": "text", "text": format!("Like {link}")}]}}),
    )
    .await;

    let mut everywhere = vec![
        ("channel".to_owned(), general.clone()),
        ("conversation".to_owned(), dm.clone()),
        ("page".to_owned(), page.clone()),
        ("task".to_owned(), described.clone()),
        ("task".to_owned(), commented.clone()),
    ];
    everywhere.sort();
    assert_eq!(
        referenced_in(&setup, &setup.browser, "record", &record).await,
        everywhere
    );
    let for_bea = referenced_in(&setup, &bea_browser, "record", &record).await;
    assert!(!for_bea.iter().any(|(_, id)| *id == dm), "{for_bea:?}");
    assert_eq!(for_bea.len(), everywhere.len() - 1, "{for_bea:?}");
}
