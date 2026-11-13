//! AWD-110: records everywhere. The silent, costly failures: search that
//! finds an archived record (or one in an archived collection), a record
//! opened as a thing with the wrong link, and a task that doesn't list the
//! records pointing at it; a rule or webhook that hears another
//! collection's records, or a rule on records that changes tasks.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    jobs::{self, WorkerOptions},
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::{RealtimeHub, publish_ready_outbox_batch},
    router,
};
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

type Received = Arc<Mutex<Vec<Value>>>;

/// A loopback receiver that keeps every body POSTed to it.
async fn receiver() -> (String, Received) {
    let received: Received = Arc::default();
    let store = received.clone();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |body: String| {
            let store = store.clone();
            async move {
                store
                    .lock()
                    .expect("receiver lock")
                    .push(serde_json::from_str(&body).unwrap_or(Value::Null));
                StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind receiver");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve receiver");
    });
    (origin, received)
}

/// Publishes the outbox and runs due jobs until both are quiet.
async fn pump(state: &AppState) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "awd110-test".to_owned(),
    };
    for _ in 0..10 {
        let published = publish_ready_outbox_batch(&state.pool, &state.realtime, "awd110-test")
            .await
            .expect("publish outbox");
        let ran = jobs::run_due(state, &registry, &options)
            .await
            .expect("run jobs");
        if published == 0 && ran == 0 {
            return;
        }
    }
}

#[tokio::test]
async fn record_events_reach_rules_and_webhooks_for_their_collection() {
    let Some(setup) = setup("awd110_record_events").await else {
        return;
    };
    let (origin, received) = receiver().await;
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database URL");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), database_url),
        (
            "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS".to_owned(),
            origin.clone(),
        ),
    ]))
    .expect("configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let state = AppState {
        config: Arc::new(config),
        pool: setup.pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(64),
        supervisors,
    };
    // The same database through an app that trusts the receiver.
    let app = &router(state.clone());
    create_project(&setup, "WORK", "Work").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let mut collections = Vec::new();
    for name in ["Requests", "Other"] {
        let created = call(
            "collection_write",
            json!({"projectId": "WORK", "name": name, "fields": [{"name": "Notes", "type": "text"}]}),
        )
        .await;
        assert_eq!(created["outcome"], "ok", "{created}");
        collections.push(
            created["value"]["collection"]["id"]
                .as_str()
                .expect("id")
                .to_owned(),
        );
    }
    let requests = collections[0].clone();

    // A record rule takes no task conditions and doesn't touch tasks.
    for (actions, extra) in [
        (
            json!([{"kind": "notify", "recipients": "owner", "message": "x"}]),
            json!({"conditions": {"filters": {"priority": "high"}}}),
        ),
        (
            json!([{"kind": "set_field", "field": "priority", "value": "high"}]),
            json!({}),
        ),
        (
            json!([{"kind": "notify", "recipients": "assignee", "message": "x"}]),
            json!({}),
        ),
    ] {
        let mut arguments = json!({
            "projectId": "WORK",
            "operation": "create",
            "name": "Refused",
            "trigger": {"kind": "record_created", "collectionId": requests},
            "actions": actions,
        });
        for (key, value) in extra.as_object().expect("extra") {
            arguments[key] = value.clone();
        }
        let refused = call("automation_write", arguments).await;
        assert_ne!(refused["outcome"], "ok", "{refused}");
    }
    let rule = call(
        "automation_write",
        json!({
            "projectId": "WORK",
            "operation": "create",
            "name": "New requests",
            "trigger": {"kind": "record_created", "collectionId": requests},
            "actions": [{"kind": "notify", "recipients": "owner", "message": "New in {{collection.name}}: {{record.title}}"}],
        }),
    )
    .await;
    assert_eq!(rule["outcome"], "ok", "{rule}");
    let hook = call(
        "webhook_write",
        json!({
            "operation": "create",
            "name": "Records",
            "url": format!("{origin}/hook"),
            "eventTypes": ["record.created", "record.updated"],
        }),
    )
    .await;
    assert_eq!(hook["outcome"], "ok", "{hook}");

    let written = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Requests", "records": [{"title": "Dark mode", "fields": {"Notes": "please"}}], "clientRequestId": "r1"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let record = written["value"]["records"][0].clone();
    let elsewhere = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Other", "records": [{"title": "Not a request"}], "clientRequestId": "o1"}),
    )
    .await;
    assert_eq!(elsewhere["outcome"], "ok", "{elsewhere}");
    pump(&state).await;
    let changed = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Requests", "records": [{"record": record["id"], "expectedVersion": record["version"], "fields": {"Notes": "please, soon"}}], "clientRequestId": "r2"}),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    pump(&state).await;

    // The rule fired once, for its own collection's new record.
    let fired = sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM notifications WHERE kind = 'automation' ORDER BY created_at",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert!(
        fired[0].to_string().contains("New in Requests: Dark mode"),
        "{fired:?}"
    );

    // The webhook heard both collections, once each, and the change.
    let bodies = received.lock().expect("receiver lock").clone();
    let mut seen = bodies
        .iter()
        .map(|body| {
            (
                body["type"].as_str().unwrap_or_default().to_owned(),
                body["data"]["record"]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("record.created".to_owned(), "Dark mode".to_owned()),
            ("record.created".to_owned(), "Not a request".to_owned()),
            ("record.updated".to_owned(), "Dark mode".to_owned()),
        ],
        "{bodies:#?}"
    );
    let update = bodies
        .iter()
        .find(|body| body["type"] == "record.updated")
        .expect("update");
    assert_eq!(
        update["data"]["record"]["fields"]["Notes"], "please, soon",
        "{update}"
    );
    assert_eq!(
        update["data"]["record"]["changedFields"],
        json!(["Notes"]),
        "{update}"
    );
    assert_eq!(
        update["data"]["record"]["collection"]["id"],
        requests.as_str()
    );

    // A firing queued before its collection is archived does nothing after.
    let late = call(
        "record_write",
        json!({"projectId": "WORK", "collection": "Requests", "records": [{"title": "Too late"}], "clientRequestId": "r3"}),
    )
    .await;
    assert_eq!(late["outcome"], "ok", "{late}");
    publish_ready_outbox_batch(&state.pool, &state.realtime, "awd110-test")
        .await
        .expect("publish outbox");
    sqlx::query(
        "UPDATE collections SET archived_at = now(), archived_by_actor_id = created_by_actor_id \
         WHERE id = $1::uuid",
    )
    .bind(requests.as_str())
    .execute(&setup.pool)
    .await
    .expect("archive Requests");
    pump(&state).await;
    let fired = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE kind = 'automation'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(fired, 1);
}

/// A link field can hold another collection's records: only that
/// collection's, found by title or number, and both records list the link.
#[tokio::test]
async fn link_fields_point_at_another_collections_records() {
    let Some(setup) = setup("awd110_record_links").await else {
        return;
    };
    let app = &setup.app;
    create_project(&setup, "LINKS", "Links").await;
    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };

    let customers = call(
        "collection_write",
        json!({"projectId": "LINKS", "name": "Customers", "fields": [{"name": "Plan", "type": "text"}]}),
    )
    .await;
    assert_eq!(customers["outcome"], "ok", "{customers}");
    let other = call(
        "collection_write",
        json!({"projectId": "LINKS", "name": "Vendors", "fields": [{"name": "Plan", "type": "text"}]}),
    )
    .await;
    assert_eq!(other["outcome"], "ok", "{other}");
    let missing = call(
        "collection_write",
        json!({"projectId": "LINKS", "name": "Broken", "fields": [{"name": "Who", "type": "link", "target": "record:Nobody"}]}),
    )
    .await;
    assert_ne!(missing["outcome"], "ok", "{missing}");
    let requests = call(
        "collection_write",
        json!({
            "projectId": "LINKS",
            "name": "Requests",
            "fields": [{"name": "Customer", "type": "link", "target": "record:Customers"}],
        }),
    )
    .await;
    assert_eq!(requests["outcome"], "ok", "{requests}");

    let acme = call(
        "record_write",
        json!({"projectId": "LINKS", "collection": "Customers", "records": [{"title": "Acme"}], "clientRequestId": "acme"}),
    )
    .await;
    assert_eq!(acme["outcome"], "ok", "{acme}");
    let acme_id = acme["value"]["records"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let vendor = call(
        "record_write",
        json!({"projectId": "LINKS", "collection": "Vendors", "records": [{"title": "Globex"}], "clientRequestId": "globex"}),
    )
    .await;
    assert_eq!(vendor["outcome"], "ok", "{vendor}");

    // Another collection's record isn't one this field can hold.
    let wrong = call(
        "record_write",
        json!({"projectId": "LINKS", "collection": "Requests", "records": [{"title": "Bad", "fields": {"Customer": "Globex"}}], "clientRequestId": "wrong"}),
    )
    .await;
    assert_ne!(wrong["outcome"], "ok", "{wrong}");

    let request = call(
        "record_write",
        json!({"projectId": "LINKS", "collection": "Requests", "records": [{"title": "Dark mode", "fields": {"Customer": "acme"}}], "clientRequestId": "dark"}),
    )
    .await;
    assert_eq!(request["outcome"], "ok", "{request}");
    let request_id = request["value"]["records"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert!(request.to_string().contains("Acme"), "{request}");

    let opened = call("thing_open", json!({"kind": "record", "id": request_id})).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(
        opened["value"]["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["other"]["id"] == acme_id.as_str() && link["direction"] == "outgoing"),
        "{opened}"
    );
    let customer = call("thing_open", json!({"kind": "record", "id": acme_id})).await;
    assert!(
        customer["value"]["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["other"]["id"] == request_id.as_str()
                && link["direction"] == "incoming"),
        "{customer}"
    );

    // A view filters the link field by a record, and the definition
    // collection_describe gives replays as no change.
    let viewed = call(
        "collection_write",
        json!({
            "projectId": "LINKS",
            "collection": "Requests",
            "views": [{"name": "Acme's", "filters": [{"field": "Customer", "op": "is", "values": ["Acme"]}]}],
        }),
    )
    .await;
    assert_eq!(viewed["outcome"], "ok", "{viewed}");
    let described = call(
        "collection_describe",
        json!({"projectId": "LINKS", "collection": "Requests"}),
    )
    .await;
    assert_eq!(described["outcome"], "ok", "{described}");
    let mut replay = described["value"]["definition"].clone();
    assert_eq!(
        replay["fields"][0]["target"], "record:Customers",
        "{replay}"
    );
    replay["projectId"] = json!("LINKS");
    replay["collection"] = json!("Requests");
    replay.as_object_mut().expect("definition").remove("name");
    let again = call("collection_write", replay).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["value"]["changes"], json!([]), "{again}");
    let retarget = call(
        "collection_write",
        json!({
            "projectId": "LINKS",
            "collection": "Requests",
            "fields": [{"name": "Customer", "type": "link", "target": "record:Vendors"}],
        }),
    )
    .await;
    assert_ne!(retarget["outcome"], "ok", "{retarget}");
}
