//! AWD-110: records everywhere. The silent, costly failures: search that
//! finds an archived record (or one in an archived collection), a record
//! opened as a thing with the wrong link, and a task that doesn't list the
//! records pointing at it.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn records_are_searched_and_opened_like_any_other_thing() {
    let Some(setup) = setup("awd110_records").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let task = call(
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the bridge checkpoint", "clientRequestId": "task"}),
    )
    .await;
    assert_eq!(task["outcome"], "applied", "{task}");
    let task_id = task["workItemId"].as_str().expect("task").to_owned();
    let task_key = task["key"].as_str().expect("key").to_owned();

    let created = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Playtest notes",
            "fields": [
                {"name": "Notes", "type": "text"},
                {"name": "Fix", "type": "link", "target": "task"},
            ],
        }),
    )
    .await;
    assert_eq!(created["outcome"], "ok", "{created}");
    let written = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtest notes",
            "records": [
                {"title": "Checkpoint skipped", "fields": {"Notes": "the zeppelin level resets", "Fix": task_key}},
                {"title": "Old zeppelin note", "fields": {"Notes": "stale"}},
            ],
            "clientRequestId": "notes",
        }),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let records = written["value"]["records"].as_array().expect("records");
    let checkpoint = records[0]["id"].as_str().expect("id").to_owned();
    let old = &records[1];
    let archived = call(
        "record_write",
        json!({
            "projectId": "WORK",
            "collection": "Playtest notes",
            "records": [{"record": old["id"], "expectedVersion": old["version"], "archived": true}],
            "clientRequestId": "archive-old",
        }),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");

    // Search finds a record by a word in a text field, links to it in its
    // collection, and never finds the archived one.
    let (status, found) = send(
        app,
        setup.browser.request(
            "GET",
            "/api/v1/search?q=zeppelin&kinds=records",
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let hits = found["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["kind"], "record");
    assert_eq!(hits[0]["title"], "Checkpoint skipped");
    let collection_id = created["value"]["collection"]["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            hits[0]["path"]
                .as_str()
                .expect("path")
                .split('/')
                .nth(4)
                .expect("collection")
                .to_owned()
        });
    let href = format!("/projects/{work}/collections/{collection_id}?record={checkpoint}");
    assert_eq!(hits[0]["path"], href, "{found}");
    let by_agent = call("search", json!({"query": "zeppelin", "kinds": ["records"]})).await;
    assert_eq!(by_agent["outcome"], "ok", "{by_agent}");
    assert_eq!(
        by_agent["value"]["results"].as_array().map(Vec::len),
        Some(1),
        "{by_agent}"
    );

    // Opened from its link, a record is a thing in its collection, linking
    // to the task its field names; the task lists it back.
    let opened = call("thing_open", json!({"kind": "record", "id": href})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let thing = &opened["value"]["thing"];
    assert_eq!(thing["id"], checkpoint.as_str(), "{opened}");
    assert_eq!(thing["title"], "Playtest notes › Checkpoint skipped");
    assert_eq!(thing["href"], href.as_str());
    let links = opened["value"]["links"].as_array().expect("links");
    assert!(
        links
            .iter()
            .any(|link| link["other"]["id"] == task_id.as_str() && link["direction"] == "outgoing"),
        "{opened}"
    );
    let task_thing = call("thing_open", json!({"kind": "task", "id": task_id})).await;
    assert_eq!(task_thing["outcome"], "ok", "{task_thing}");
    assert!(
        task_thing["value"]["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["other"]["kind"] == "record"
                && link["other"]["id"] == checkpoint.as_str()
                && link["direction"] == "incoming"),
        "{task_thing}"
    );

    // An archived collection's records leave search and stop opening.
    sqlx::query("UPDATE collections SET archived_at = now(), archived_by_actor_id = created_by_actor_id WHERE name = 'Playtest notes'")
        .execute(&setup.pool)
        .await
        .expect("archive the collection");
    let (_, after) = send(
        app,
        setup
            .browser
            .request("GET", "/api/v1/search?q=zeppelin", None, Value::Null),
    )
    .await;
    assert!(
        !after["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .any(|hit| hit["kind"] == "record"),
        "{after}"
    );
    let gone = call("thing_open", json!({"kind": "record", "id": checkpoint})).await;
    assert_eq!(gone["outcome"], "not_found", "{gone}");
}
