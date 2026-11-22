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
    // An agent's run in A's DM, and what it did: left out with the DM, to
    // any depth (the run by its conversation, its activity by its run).
    let (dm_run, dm_activity) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO runs (id, workspace_id, account_id, title, cause_kind, conversation_id, state) \
         VALUES ($1, $2, $3, 'Run in a DM', 'thread', $4::uuid, 'working')",
    )
    .bind(dm_run)
    .bind(a.organization)
    .bind(a.agent)
    .bind(&a.dm)
    .execute(&fixture.pool)
    .await
    .expect("a run in the DM");
    sqlx::query(
        "INSERT INTO run_activities (id, run_id, workspace_id, seq, kind, summary, occurred_at, \
         reported_by_account_id) VALUES ($1, $2, $3, 1, 'reasoning', 'thinking aloud', now(), $4)",
    )
    .bind(dm_activity)
    .bind(dm_run)
    .bind(a.organization)
    .bind(a.agent)
    .execute(&fixture.pool)
    .await
    .expect("its activity");

    // C1.34: a sheet snapshot and a board version the compaction jobs
    // moved to the file store leave whole, with no key into this server's
    // store.
    let store = agent_workspace_server::files::store::FileStore::from_config(&fixture.state.config);
    sqlx::query(
        "INSERT INTO whiteboard_revisions (whiteboard_id, revision, scene_seq, elements, element_count) \
         SELECT id, 1 + COALESCE((SELECT max(revision) FROM whiteboard_revisions WHERE whiteboard_id = $1), 0), \
                scene_seq, '[{\"id\": \"kept\", \"type\": \"rectangle\"}]'::jsonb, 1 \
         FROM whiteboards WHERE id = $1",
    )
    .bind(a.whiteboard)
    .execute(&fixture.pool)
    .await
    .expect("a saved board version");
    let mut moved = Vec::new();
    for (table, owner, field, owner_id) in [
        ("sheet_snapshots", "sheet_id", "state", a.sheet),
        (
            "whiteboard_revisions",
            "whiteboard_id",
            "elements",
            a.whiteboard,
        ),
    ] {
        let (revision, payload) = sqlx::query_as::<_, (i64, Value)>(&format!(
            "SELECT revision::bigint, {field} FROM {table} WHERE {owner} = $1 \
             ORDER BY revision DESC LIMIT 1"
        ))
        .bind(owner_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap_or_else(|error| panic!("a {table} row: {error}"));
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, payload.to_string().as_bytes()).expect("gzip");
        let key = format!(
            "{}/history/{}-{}-{revision}.json.gz",
            a.organization.simple(),
            table.split('_').next().unwrap_or("thing"),
            owner_id.simple()
        );
        store
            .put(&key, &encoder.finish().expect("gzip"))
            .await
            .expect("payload stored");
        let emptied = if table == "sheet_snapshots" {
            "'{}'::jsonb"
        } else {
            "'[]'::jsonb"
        };
        sqlx::query(&format!(
            "UPDATE {table} SET {field} = {emptied}, payload_key = $3, payload_store_id = $4 \
             WHERE {owner} = $1 AND revision = $2"
        ))
        .bind(owner_id)
        .bind(revision)
        .bind(&key)
        .bind(store.id())
        .execute(&fixture.pool)
        .await
        .expect("moved");
        moved.push((table, owner, field, owner_id, revision, payload));
    }

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
    crate::support::run_export(&fixture.state, id)
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

    for (table, owner, field, owner_id, revision, payload) in &moved {
        let line = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| {
                line["kind"] == *table
                    && line["data"][owner] == json!(owner_id)
                    && line["data"]["revision"].as_i64() == Some(*revision)
            })
            .unwrap_or_else(|| panic!("the moved {table} row leaves"));
        assert_eq!(&line["data"][field], payload, "{table} leaves whole");
        assert!(line["data"]["payload_key"].is_null(), "{line}");
        assert!(line["data"]["payload_store_id"].is_null(), "{line}");
    }

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
        ("a run in the DM", &dm_run.to_string()),
        ("the run's activity", &dm_activity.to_string()),
    ] {
        assert!(!text.contains(id.as_str()), "{what} ({id}) stays out");
    }
    assert!(
        !text.contains("thinking aloud"),
        "the run's activity stays out"
    );
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
