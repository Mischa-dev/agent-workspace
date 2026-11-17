//! K3.5: pages, sheets and whiteboards in events, rules and webhooks. The
//! silent, costly failures: a rule that fires for every keystroke or for
//! another page, a webhook that carries a page's text or reaches someone
//! who can't see the project, and a switched-off module that still fires.

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
    realtime::RealtimeHub,
    router,
};
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{
    agent_interface::{connect_app, initialize, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::space,
    message_edits::add_person,
    modules::switch,
};

type Received = Arc<Mutex<Vec<Value>>>;

/// A loopback receiver that keeps every body POSTed to it, by path.
async fn receiver() -> (String, Received) {
    let received: Received = Arc::default();
    let store = received.clone();
    let app = axum::Router::new().route(
        "/{who}",
        axum::routing::post(
            move |axum::extract::Path(who): axum::extract::Path<String>, body: String| {
                let store = store.clone();
                async move {
                    let mut body = serde_json::from_str(&body).unwrap_or(Value::Null);
                    body["receivedBy"] = json!(who);
                    store.lock().expect("receiver lock").push(body);
                    StatusCode::NO_CONTENT
                }
            },
        ),
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

/// Sequences the log and runs due jobs until both are quiet.
async fn pump(state: &AppState) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "k3-5-test".to_owned(),
    };
    for _ in 0..10 {
        let handled = agent_workspace_server::events::catch_up(&state.pool)
            .await
            .expect("catch up");
        let ran = jobs::run_due(state, &registry, &options)
            .await
            .expect("run jobs");
        if handled == 0 && ran == 0 {
            return;
        }
    }
}

fn ok(value: Value, what: &str) -> Value {
    assert_eq!(value["outcome"], "ok", "{what}: {value}");
    value
}

#[tokio::test]
async fn pages_sheets_and_boards_reach_rules_and_webhooks_that_may_see_them() {
    let Some(setup) = setup("k3_5_content_events").await else {
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
    let app = &router(state.clone());
    let workspace_space = space(&setup).await;
    create_project(&setup, "WORK", "Work").await;
    // Someone in the workspace but not in WORK.
    let (_, outsider) = add_person(&setup, &[&workspace_space], "olaf").await;

    let codex = connect_app(app, &setup.browser, "Codex", "full").await;
    let session = initialize(app, &codex).await.expect("Codex MCP");
    let call = |name: &'static str, arguments: Value| {
        let codex = codex.clone();
        let session = session.clone();
        async move { tool(app, &codex, &session, name, arguments).await }
    };
    let outsider_app = connect_app(app, &outsider, "Olaf's script", "full").await;
    let outsider_session = initialize(app, &outsider_app).await.expect("outsider MCP");

    // A page, a sheet and a board, and one more page the rule doesn't name.
    let page = ok(
        call(
            "page_write",
            json!({"projectId": "WORK", "title": "Roadmap", "body": "Secret plans."}),
        )
        .await,
        "page",
    );
    let page_id = page["value"]["page"]["id"]
        .as_str()
        .or_else(|| page["value"]["id"].as_str())
        .expect("page id")
        .to_owned();
    let other = ok(
        call(
            "page_write",
            json!({"projectId": "WORK", "title": "Notes", "body": "Other text."}),
        )
        .await,
        "other page",
    );
    let other_id = other["value"]["page"]["id"]
        .as_str()
        .or_else(|| other["value"]["id"].as_str())
        .expect("other page id")
        .to_owned();
    let sheet = ok(
        call(
            "sheet_write",
            json!({"projectId": "WORK", "name": "Budget",
                   "cells": [{"cell": "A1", "input": "Rent"}], "clientRequestId": "k35-sheet"}),
        )
        .await,
        "sheet",
    );
    let sheet_id = sheet["value"]["id"].as_str().expect("sheet id").to_owned();
    let board = ok(
        call(
            "whiteboard_create",
            json!({"projectId": "WORK", "title": "Flow"}),
        )
        .await,
        "board",
    );
    let board_id = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board id")
        .to_owned();
    pump(&state).await;

    // A rule on each, a rule with task conditions refused, and webhooks for
    // the owner and for the outsider.
    let refused = call(
        "automation_write",
        json!({"projectId": "WORK", "operation": "create", "name": "Refused",
               "trigger": {"kind": "page_changed", "pageId": page_id},
               "conditions": {"filters": {"priority": "high"}},
               "actions": [{"kind": "notify", "recipients": "owner", "message": "x"}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    for (trigger, message) in [
        (
            json!({"kind": "page_changed", "pageId": page_id}),
            "Page {{page.title}} at {{page.url}}",
        ),
        (
            json!({"kind": "sheet_changed", "sheetId": sheet_id}),
            "Sheet {{sheet.title}}",
        ),
        (
            json!({"kind": "whiteboard_changed"}),
            "Board {{whiteboard.title}}",
        ),
    ] {
        ok(
            call(
                "automation_write",
                json!({"projectId": "WORK", "operation": "create", "name": "Watch",
                       "trigger": trigger,
                       "actions": [{"kind": "notify", "recipients": "owner", "message": message}]}),
            )
            .await,
            "rule",
        );
    }
    let event_types = json!(["page.updated", "sheet.updated", "whiteboard.updated"]);
    ok(
        call(
            "webhook_write",
            json!({"operation": "create", "name": "Docs",
                   "url": format!("{origin}/owner"), "eventTypes": event_types}),
        )
        .await,
        "owner webhook",
    );
    ok(
        tool(
            app,
            &outsider_app,
            &outsider_session,
            "webhook_write",
            json!({"operation": "create", "name": "Docs",
                   "url": format!("{origin}/outsider"), "eventTypes": event_types}),
        )
        .await,
        "outsider webhook",
    );

    // An agent edits each one.
    ok(
        call(
            "page_write",
            json!({"pageId": page_id, "body": "Secret plans, revised.",
                   "expectedVersion": page["value"]["page"]["version"].as_i64()
                       .or_else(|| page["value"]["version"].as_i64()).unwrap_or(1)}),
        )
        .await,
        "page edit",
    );
    ok(
        call(
            "page_write",
            json!({"pageId": other_id, "body": "Other text, revised.", "expectedVersion": 1}),
        )
        .await,
        "other page edit",
    );
    ok(
        call(
            "sheet_write",
            json!({"projectId": "WORK", "sheet": "Budget",
                   "cells": [{"cell": "B1", "input": "1200"}], "clientRequestId": "k35-sheet-2"}),
        )
        .await,
        "sheet edit",
    );
    ok(
        call(
            "whiteboard_edit",
            json!({"whiteboardId": board_id,
                   "operations": [{"op": "add", "type": "rectangle", "id": "box", "x": 0, "y": 0}]}),
        )
        .await,
        "board edit",
    );
    // A second board edit in the same session keeps no new version.
    ok(
        call(
            "whiteboard_edit",
            json!({"whiteboardId": board_id,
                   "operations": [{"op": "add", "type": "rectangle", "id": "box2", "x": 200, "y": 0}]}),
        )
        .await,
        "second board edit",
    );
    pump(&state).await;

    // Each rule fired once, for its own page, sheet or board.
    let fired = sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM notifications WHERE kind = 'automation' ORDER BY created_at",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("notifications");
    let texts = fired.iter().map(Value::to_string).collect::<Vec<_>>();
    assert_eq!(fired.len(), 3, "{texts:#?}");
    assert!(
        texts
            .iter()
            .any(|text| text.contains("Page Roadmap at http")),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("Sheet Budget")),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("Board Flow")),
        "{texts:#?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("Notes")),
        "{texts:#?}"
    );

    // The owner's webhook heard each, with title and address and never the
    // text; the outsider's heard nothing.
    let bodies = received.lock().expect("receiver lock").clone();
    assert!(
        bodies.iter().all(|body| body["receivedBy"] == "owner"),
        "{bodies:#?}"
    );
    let mut heard = bodies
        .iter()
        .map(|body| {
            let kind = body["type"].as_str().unwrap_or_default().to_owned();
            let key = kind.split('.').next().unwrap_or_default().to_owned();
            (
                kind,
                body["data"][key]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    heard.sort();
    assert_eq!(
        heard,
        vec![
            ("page.updated".to_owned(), "Notes".to_owned()),
            ("page.updated".to_owned(), "Roadmap".to_owned()),
            ("sheet.updated".to_owned(), "Budget".to_owned()),
            ("whiteboard.updated".to_owned(), "Flow".to_owned()),
        ],
        "{bodies:#?}"
    );
    for body in &bodies {
        let text = body.to_string();
        assert!(!text.contains("Secret plans"), "{text}");
        assert!(!text.contains("Rent"), "{text}");
    }
    let roadmap = bodies
        .iter()
        .find(|body| body["data"]["page"]["title"] == "Roadmap")
        .expect("roadmap");
    assert!(
        roadmap["data"]["page"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http") && url.contains(page_id.as_str())),
        "{roadmap}"
    );

    // With Pages off for the workspace, a page edit fires nothing.
    let (status, body) = switch(
        &setup,
        &setup.browser,
        json!({"switches": {"pages": false}}),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let before = received.lock().expect("receiver lock").len();
    let mut transaction = setup.pool.begin().await.expect("transaction");
    let project_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT project_id FROM project_pages WHERE id = $1::uuid",
    )
    .bind(&page_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("project");
    agent_workspace_server::events::emit(
        &mut transaction,
        agent_workspace_server::events::NewEvent::thing(
            &agent_workspace_server::events::catalog::PAGE_UPDATED,
            project_id,
            "page",
            page_id.parse().expect("uuid"),
        )
        .version(99),
    )
    .await
    .expect("page fact");
    transaction.commit().await.expect("commit");
    pump(&state).await;
    assert_eq!(received.lock().expect("receiver lock").len(), before);
    let fired = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE kind = 'automation'",
    )
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(fired, 3);
}
