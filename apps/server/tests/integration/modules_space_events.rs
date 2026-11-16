//! AWD-186: a space that turns Automations off stops webhooks for its own
//! events, and only its own. Event routing read the workspace's switches
//! alone, so a space's narrowing still sent its events out.

use agent_workspace_server::jobs::{self, JobRegistry, WorkerOptions};
use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, drain_outbox},
    world::WorldBuilder,
};

#[tokio::test]
async fn automations_off_in_one_space_stops_its_webhooks_only() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .agent("rook", "member")
        .build("awd186_space_events")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let mut hooks = Vec::new();
    for key in ["WORK", "OTHER"] {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO webhook_subscriptions (
                id, organization_id, project_id, owner_actor_id, name, url, event_types
            ) VALUES ($1, $2, $3::uuid, $4, $5, 'https://example.com/hook',
                      '{task.created,task.updated}')
            "#,
        )
        .bind(id)
        .bind(world.organization)
        .bind(world.project(key))
        .bind(world.owner.actor)
        .bind(format!("{key} hook"))
        .execute(&pool)
        .await
        .expect("webhook");
        hooks.push(id);
    }

    // Automations off in OTHER only.
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "PUT",
            &format!("/api/v1/organizations/{}/modules", world.organization),
            None,
            json!({"switches": {"automations": false}, "space": world.project("OTHER")}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let rook = &world.agents["rook"];
    for key in ["WORK", "OTHER"] {
        let (_, made) = rook
            .call(
                &world.app,
                Call::Tool {
                    name: "task_create",
                    arguments: json!({"projectId": world.project(key), "title": "Hooked",
                                      "clientRequestId": format!("hooked-{key}")}),
                },
            )
            .await;
        assert!(
            matches!(made["outcome"].as_str(), Some("ok" | "applied")),
            "{made}"
        );
    }

    // Publish the outbox and route its events (only routing: deliveries
    // stay queued, so nothing leaves the test).
    let mut registry = JobRegistry::new();
    agent_workspace_server::events::register_jobs(&mut registry);
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "awd186".to_owned(),
    };
    for _ in 0..10 {
        drain_outbox(&world.state).await;
        let ran = jobs::run_due(&world.state, &registry, &options)
            .await
            .expect("route events");
        if ran == 0 {
            break;
        }
    }
    let delivered = |hook: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM webhook_deliveries WHERE subscription_id = $1",
            )
            .bind(hook)
            .fetch_one(&pool)
            .await
            .expect("deliveries")
        }
    };
    assert!(delivered(hooks[0]).await > 0, "WORK's webhook still hears WORK");
    assert_eq!(
        delivered(hooks[1]).await,
        0,
        "OTHER turned Automations off, so its webhook hears nothing"
    );
    world.finish().await;
}
