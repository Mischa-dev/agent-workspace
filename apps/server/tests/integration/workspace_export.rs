//! H1.9: a workspace leaves as one archive. The silent, costly failures:
//! another workspace's things in it, someone's DMs or groups in it, a
//! credential or secret value in it, and someone other than the requester
//! downloading it.

use std::io::Read;

use agent_workspace_server::export::transfer;
use axum::{body::to_bytes, http::StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    support::{
        app::{Browser, send},
        two_workspaces::{sweep_ids, two_workspaces},
    },
    workspace_isolation::leaked,
};

#[tokio::test]
async fn an_export_holds_the_workspace_and_nothing_it_must_not() {
    let Some(fixture) = two_workspaces("h1_9_export").await else {
        return;
    };
    let (app, a, b) = (&fixture.app, &fixture.a, &fixture.b);
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", a.organization);

    let (status, started) = send(
        app,
        a.browser.request(
            "POST",
            &route("workspace_export_start"),
            Some("h1-9-export"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {started}");
    assert_eq!(started["value"]["state"], "queued", "{started}");
    let id = started["value"]["id"]
        .as_str()
        .expect("transfer id")
        .parse::<Uuid>()
        .expect("uuid");
    transfer::run_export(&fixture.state, id)
        .await
        .expect("export runs");

    let (_, shown) = send(
        app,
        a.browser.request(
            "POST",
            &route("workspace_transfer_get"),
            None,
            json!({ "transferId": id }),
        ),
    )
    .await;
    assert_eq!(shown["value"]["state"], "succeeded", "{shown}");
    let url = shown["value"]["downloadUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("download address in {shown}"))
        .to_owned();
    let (_, listed) = send(
        app,
        a.browser
            .request("POST", &route("workspace_transfer_list"), None, json!({})),
    )
    .await;
    assert_eq!(listed["value"]["transfers"][0]["id"], json!(id), "{listed}");

    // Only the requester downloads it.
    let download = |browser: &Browser| {
        let request = browser.request("GET", &url, None, Value::Null);
        async move {
            let response = app.clone().oneshot(request).await.expect("response");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 64_000_000)
                .await
                .expect("body");
            (status, bytes)
        }
    };
    let (status, _) = download(&b.browser).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "B's owner never downloads A's archive"
    );
    let (status, bytes) = download(&a.browser).await;
    assert_eq!(status, StatusCode::OK);

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("zip");
    let mut entries = Vec::new();
    let mut text = String::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("entry");
        let name = entry.name().expect("entry name").into_owned();
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).expect("read entry");
        if !name.starts_with("files/blobs/") {
            text.push_str(&format!("\n== {name}\n"));
            text.push_str(&String::from_utf8(contents).expect("text entry"));
        }
        entries.push(name);
    }
    assert_eq!(
        entries.last().map(String::as_str),
        Some("manifest.json"),
        "the manifest goes last, once everything else is in: {entries:?}"
    );
    for expected in [
        "workspace/workspace.json",
        "accounts/people.jsonl",
        "files/index.jsonl",
        "needs/secrets.jsonl",
        "needs/connections.jsonl",
    ] {
        assert!(
            entries.iter().any(|name| name == expected),
            "{expected} in {entries:?}"
        );
    }
    assert!(
        entries.iter().any(|name| name.starts_with("files/blobs/")),
        "file bytes travel: {entries:?}"
    );
    let manifest: Value = {
        let mut entry = archive.by_name("manifest.json").expect("manifest");
        let mut contents = String::new();
        entry.read_to_string(&mut contents).expect("manifest text");
        serde_json::from_str(&contents).expect("manifest json")
    };
    assert_eq!(manifest["format"], transfer::FORMAT);
    assert_eq!(manifest["formatVersion"], transfer::FORMAT_VERSION);
    assert_eq!(manifest["workspace"]["slug"], a.slug.as_str());

    // A's own things are in it.
    for (what, id) in [
        ("the channel", a.channel.clone()),
        ("a channel message", a.message.clone()),
        ("the task", a.task.to_string()),
        ("the page", a.page.clone()),
        ("the record", a.record.to_string()),
        ("the file", a.file.to_string()),
    ] {
        assert!(text.contains(&id), "{what} ({id}) is in the archive");
    }
    // Nobody's DMs or groups, nor what was said in them.
    for (what, id) in [
        ("the DM", &a.dm),
        ("the DM's message", &a.dm_message),
        ("the group", &a.group),
    ] {
        assert!(!text.contains(id.as_str()), "{what} ({id}) stays out");
    }
    // Nothing of B's.
    let b_ids = sweep_ids(&fixture.pool, b.organization).await;
    let found = leaked(&text, &b_ids);
    assert!(found.is_empty(), "B's things in A's archive: {found:?}");
    // No credential or secret value.
    for secret in [&a.agent_token, &a.app_token] {
        assert!(!text.contains(secret.as_str()), "a token stays out");
    }
    for column in [
        "\"ciphertext\"",
        "\"password_hash\"",
        "\"token_digest\"",
        "\"secret_digest\"",
    ] {
        assert!(!text.contains(column), "{column} stays out");
    }

    fixture.finish().await;
}
