//! T1.20: a file field on records. The silent, costly failures: a cell
//! naming a file that isn't the record's own (another record's, or a file
//! the writer can't see), someone who only reads records uploading to one,
//! and an archived record's files going dark for its readers.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::WorldBuilder,
};

fn ok(answer: &Value) -> &Value {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    &answer["value"]
}

#[tokio::test]
async fn a_record_keeps_its_files_in_a_field_and_readers_keep_them_when_archived() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("vic", "viewer")
        .connected_app("Desk")
        .build("t1_record_files")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let desk = &world.apps["Desk"];
    let vic = &world.people["vic"];
    let tool = |name: &'static str, arguments: Value| async move {
        desk.call(app, Call::Tool { name, arguments }).await.1
    };

    ok(&tool(
        "collection_write",
        json!({"projectId": "WORK", "name": "Bugs", "fields": [
            {"name": "Screenshots", "type": "file"},
        ]}),
    )
    .await);

    // A new record can't name files yet: they're uploaded to it once made.
    let early = tool(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "clientRequestId": "early",
               "records": [{"title": "Early", "fields": {"Screenshots": [Uuid::now_v7()]}}]}),
    )
    .await;
    assert_eq!(early["outcome"], "invalid_request", "{early}");

    let made = tool(
        "record_write",
        json!({"projectId": "WORK", "collection": "Bugs", "clientRequestId": "two",
               "records": [{"title": "Crash on start"}, {"title": "Slow search"}]}),
    )
    .await;
    let records = ok(&made)["records"].as_array().expect("records");
    let crash = records[0]["id"].as_str().expect("id").to_owned();
    let slow = records[1]["id"].as_str().expect("id").to_owned();
    let upload = |record: String, name: &'static str| async move {
        tool(
            "file_upload",
            json!({"thing": {"kind": "record", "id": record}, "name": name, "text": "pixels"}),
        )
        .await
    };
    let shot = ok(&upload(crash.clone(), "shot.txt").await)["fileId"]
        .as_str()
        .expect("file")
        .to_owned();
    let other = ok(&upload(slow.clone(), "other.txt").await)["fileId"]
        .as_str()
        .expect("file")
        .to_owned();

    // The cell takes the record's own file, and reads it by name.
    let set = tool(
        "record_update",
        json!({"record": crash, "fields": {"Screenshots": [shot]}}),
    )
    .await;
    assert!(ok(&set)["changed"].as_bool().unwrap_or(true), "{set}");
    let read = tool(
        "record_query",
        json!({"projectId": "WORK", "collection": "Bugs"}),
    )
    .await;
    assert!(ok(&read).to_string().contains("shot.txt"), "{read}");
    let stored: Value =
        sqlx::query_scalar("SELECT field_values FROM collection_records WHERE id = $1::uuid")
            .bind(&crash)
            .fetch_one(pool)
            .await
            .expect("record");
    assert!(stored.to_string().contains(&shot), "{stored}");

    // Another record's file is refused, and nothing changes.
    let wrong = tool(
        "record_update",
        json!({"record": crash, "fields": {"Screenshots": [shot, other]}}),
    )
    .await;
    assert_eq!(wrong["outcome"], "invalid_request", "{wrong}");
    let after: Value =
        sqlx::query_scalar("SELECT field_values FROM collection_records WHERE id = $1::uuid")
            .bind(&crash)
            .fetch_one(pool)
            .await
            .expect("record");
    assert_eq!(after, stored);

    // A viewer reads the file but can't upload to the record.
    let upload_as_vic = |record: &str| {
        vic.browser.request(
            "POST",
            &format!("/api/v1/things/record/{record}/files?name=sneaky.txt"),
            None,
            json!("sneaky"),
        )
    };
    let (status, body) = send(app, upload_as_vic(&crash)).await;
    assert_eq!(status.as_u16(), 404, "{body}");
    let content = |id: &str| {
        vic.browser
            .request("GET", &format!("/api/v1/files/{id}"), None, Value::Null)
    };
    let (status, body) = send(app, content(&shot)).await;
    assert!(status.is_success(), "{status} {body}");

    // Archived, the record takes no new files, and its readers still read
    // the ones it has.
    let archived = tool("record_update", json!({"record": crash, "archived": true})).await;
    ok(&archived);
    let late = upload(crash.clone(), "late.txt").await;
    assert_eq!(late["outcome"], "conflict", "{late}");
    let (status, body) = send(app, content(&shot)).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(body["name"], "shot.txt", "{body}");

    world.finish().await;
}
