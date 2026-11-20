//! K3.26: incoming webhooks start automations. The silent, costly
//! failures: a retried delivery that runs a rule twice, a forged one that
//! runs it at all, a body that leaks into the log, and a secret that is
//! shown twice or to someone who may not manage automations, and a hook
//! that keeps every body it is sent.

use agent_workspace_server::{
    incoming_hooks::{DELIVERY_HEADER, SIGNATURE_HEADER, TIMESTAMP_HEADER, sign},
    jobs::{self, WorkerOptions},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::support::{app::tool, world::WorldBuilder};

/// Posts `body` to the hook, signed with `secret` unless `forged`.
async fn deliver(
    app: &axum::Router,
    hook: &str,
    secret: &str,
    delivery: &str,
    body: &str,
) -> (StatusCode, Value) {
    let now = chrono::Utc::now().timestamp();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/incoming-hooks/{hook}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(DELIVERY_HEADER, delivery)
                .header(TIMESTAMP_HEADER, now.to_string())
                .header(
                    SIGNATURE_HEADER,
                    sign(secret, now, delivery, body.as_bytes()),
                )
                .body(Body::from(body.to_owned()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Sequences the log and runs due jobs until both are quiet.
async fn pump(state: &agent_workspace_server::AppState) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "k3-26-test".to_owned(),
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

#[tokio::test]
async fn a_signed_delivery_runs_its_rule_once_and_a_forged_one_never() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("helper")
        .build("k3_26_incoming_hooks")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let helper = &world.apps["helper"];
    let call = |name: &'static str, arguments: Value| async move {
        tool(app, &helper.token, &helper.session, name, arguments).await
    };

    let made = call(
        "incoming_hook_create",
        json!({"projectId": "WORK", "name": "Billing alerts", "clientRequestId": "k3-26-hook"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let hook = made["value"]["hook"]["id"]
        .as_str()
        .expect("hook")
        .to_owned();
    let secret = made["value"]["secret"].as_str().expect("secret").to_owned();
    assert!(
        made["value"]["hook"]["url"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("/api/v1/incoming-hooks/{hook}"))),
        "{made}"
    );
    // A retry finds the same hook, and never shows the secret again.
    let again = call(
        "incoming_hook_create",
        json!({"projectId": "WORK", "name": "Billing alerts", "clientRequestId": "k3-26-hook"}),
    )
    .await;
    assert_eq!(again["value"]["hook"]["id"], hook.as_str(), "{again}");
    assert!(again["value"].get("secret").is_none(), "{again}");
    assert!(!again.to_string().contains(&secret), "{again}");
    // Nor is it stored anywhere a replay could read it.
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM idempotency_keys WHERE response_body::text LIKE '%' || $1 || '%'",
    )
    .bind(&secret)
    .fetch_one(pool)
    .await
    .expect("idempotency keys");
    assert_eq!(stored, 0, "the secret was kept for a replay");

    let rule = call(
        "automation_write",
        json!({"projectId": "WORK", "operation": "create", "name": "On billing",
               "trigger": {"kind": "incoming_hook", "hookId": hook},
               "actions": [{"kind": "notify", "recipients": "owner",
                            "message": "{{hook.name}} {{delivery.id}}: {{delivery.text}}"}]}),
    )
    .await;
    assert_eq!(rule["outcome"], "ok", "{rule}");
    // A hook that isn't in the space can't be named.
    let unknown = call(
        "automation_write",
        json!({"projectId": "WORK", "operation": "create", "name": "Nowhere",
               "trigger": {"kind": "incoming_hook", "hookId": Uuid::now_v7()},
               "actions": [{"kind": "notify", "recipients": "owner", "message": "x"}]}),
    )
    .await;
    assert_ne!(unknown["outcome"], "ok", "{unknown}");

    let body = r#"{"invoice":"INV-77","note":"overdue-marker"}"#;
    let (status, answer) = deliver(app, &hook, &secret, "evt-1", body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    pump(&world.state).await;
    // The sender retries the same delivery: taken once, run once.
    let (status, answer) = deliver(app, &hook, &secret, "evt-1", body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "duplicate", "{answer}");
    // A forged one and one signed for another delivery id are refused.
    let (status, answer) = deliver(app, &hook, "awh_wrong", "evt-2", body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    pump(&world.state).await;

    let fired = sqlx::query_scalar::<_, Value>(
        "SELECT detail FROM notifications WHERE kind = 'automation' ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .expect("notifications");
    assert_eq!(fired.len(), 1, "{fired:#?}");
    let text = fired[0].to_string();
    assert!(text.contains("Billing alerts evt-1"), "{text}");
    assert!(text.contains("INV-77"), "the rule reads the body: {text}");

    // The log names the hook and the delivery, never the body.
    let logged = sqlx::query_as::<_, (Value, Option<Uuid>)>(
        "SELECT payload, thing_id FROM events WHERE type = 'incoming_hook.received'",
    )
    .fetch_all(pool)
    .await
    .expect("events");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].1.map(|id| id.to_string()), Some(hook.clone()));
    let jobs_text = sqlx::query_scalar::<_, String>("SELECT payload::text FROM background_jobs")
        .fetch_all(pool)
        .await
        .expect("jobs")
        .join(" ");
    for text in [logged[0].0.to_string(), jobs_text] {
        assert!(!text.contains("overdue-marker"), "{text}");
    }
    // The refusal is kept with its reason and without its body.
    let refused = sqlx::query_as::<_, (String, Option<String>, Option<Vec<u8>>)>(
        "SELECT delivery_key, reason, body FROM incoming_hook_deliveries WHERE outcome = 'refused'",
    )
    .fetch_all(pool)
    .await
    .expect("refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].0, "evt-2");
    assert_eq!(refused[0].1.as_deref(), Some("bad_signature"));
    assert!(refused[0].2.is_none());
    let listed = call("incoming_hook_list", json!({"projectId": "WORK"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let recent = listed["value"]["hooks"][0]["recent"]
        .as_array()
        .expect("recent");
    assert_eq!(recent.len(), 2, "{listed}");
    assert!(!listed.to_string().contains(&secret), "{listed}");
    assert!(!listed.to_string().contains("overdue-marker"), "{listed}");

    // The body it took is held against the hook and the workspace's
    // storage; a hook that holds all it keeps answers 429 and stores nothing.
    let held = sqlx::query_as::<_, (i32, i64)>(
        "SELECT held_deliveries, held_bytes FROM incoming_hooks WHERE id = $1::uuid",
    )
    .bind(&hook)
    .fetch_one(pool)
    .await
    .expect("held");
    assert_eq!(held, (1, body.len() as i64));
    sqlx::query("UPDATE incoming_hooks SET held_deliveries = 50000 WHERE id = $1::uuid")
        .bind(&hook)
        .execute(pool)
        .await
        .expect("fill the hook");
    let (status, answer) = deliver(app, &hook, &secret, "evt-4", body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{answer}");
    assert_eq!(answer["reason"], "hook_full", "{answer}");
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM incoming_hook_deliveries WHERE delivery_key = 'evt-4'",
    )
    .fetch_one(pool)
    .await
    .expect("stored");
    assert_eq!(stored, 0, "a full hook stores nothing");

    // Deleted, its address stops taking deliveries.
    let deleted = call(
        "incoming_hook_delete",
        json!({"projectId": "WORK", "hookId": hook, "clientRequestId": "k3-26-delete"}),
    )
    .await;
    assert_eq!(deleted["outcome"], "ok", "{deleted}");
    let (status, _) = deliver(app, &hook, &secret, "evt-3", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    world.finish().await;
}
