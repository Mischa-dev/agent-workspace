//! Write-time domain events (K3.7): a write emits its own fact in the log,
//! with who did it and the app they came through, and webhooks and
//! automations hear that fact as written. A task made through any way in
//! (the web, an agent's tool, an app a person connected, an import, a
//! recurring schedule) is one `task.created`, by the account that made it.

use agent_workspace_server::jobs::{self, NewJob, WorkerOptions};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

/// The `task.created` facts about a task, staged or sequenced: who acted
/// and through which app.
async fn created(pool: &PgPool, task: Uuid) -> Vec<(Option<Uuid>, Option<Uuid>)> {
    sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>)>(
        r#"
        SELECT actor_account_id, via_credential_id FROM events
        WHERE type = 'task.created' AND thing_id = $1
        UNION ALL
        SELECT actor_account_id, via_credential_id FROM event_staging
        WHERE type = 'task.created' AND thing_id = $1
        "#,
    )
    .bind(task)
    .fetch_all(pool)
    .await
    .expect("task facts")
}

async fn task_titled(pool: &PgPool, project: &str, title: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM task_rows WHERE project_id = $1::uuid AND title = $2",
    )
    .bind(project)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("task {title}: {error}"))
}

async fn run_jobs(world: &Workspace) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "domain-events-test".to_owned(),
    };
    while jobs::run_due(&world.state, &registry, &options)
        .await
        .expect("run jobs")
        > 0
    {}
}

#[tokio::test]
async fn a_task_made_any_way_is_one_task_created_by_its_maker() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .connected_app("helper")
        .build("k3_7_task_created")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let project = world.project("WORK").to_owned();
    let lena = &world.people["lena"];
    let rook = &world.agents["rook"];
    let helper = &world.apps["helper"];

    // The web.
    let (status, made) = send(
        &world.app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/work-items"),
            Some("k3-7-web-task"),
            json!({"title": "From the web"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {made}");
    let task = task_titled(&pool, &project, "From the web").await;
    assert_eq!(created(&pool, task).await, vec![(Some(lena.actor), None)]);

    // An agent's own tool.
    let (_, made) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "task_create",
                arguments: json!({"projectId": project, "title": "From an agent", "clientRequestId": "agent-1"}),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let task = task_titled(&pool, &project, "From an agent").await;
    let facts = created(&pool, task).await;
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].0, Some(rook.actor));

    // An app the owner connected acts as the owner, through its grant.
    let (_, made) = helper
        .call(
            &world.app,
            Call::Tool {
                name: "task_create",
                arguments: json!({"projectId": project, "title": "From an app", "clientRequestId": "app-1"}),
            },
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let task = task_titled(&pool, &project, "From an app").await;
    let facts = created(&pool, task).await;
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].0, Some(helper.person));
    assert!(facts[0].1.is_some(), "the app it came through: {facts:?}");

    // An import.
    let (_, imported) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "task_import",
                arguments: json!({
                    "projectId": project,
                    "source": "markdown",
                    "text": "- [ ] From an import\n",
                    "commit": true
                }),
            },
        )
        .await;
    assert_eq!(imported["outcome"], "ok", "{imported}");
    run_jobs(&world).await;
    let task = task_titled(&pool, &project, "From an import").await;
    let facts = created(&pool, task).await;
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].0, Some(rook.actor));

    // A recurring schedule acts as the person who set it up.
    let base = format!("/api/v1/projects/{project}");
    let (status, template) = send(
        &world.app,
        lena.browser.request(
            "POST",
            &format!("{base}/task-templates"),
            Some("k3-7-template"),
            json!({"name": "Weekly", "title": "From a schedule"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{template}");
    let (status, recurring) = send(
        &world.app,
        lena.browser.request(
            "POST",
            "/api/v2/ops/recurring_create",
            Some("k3-7-recurring"),
            json!({"projectId": project, "template": template["id"], "frequency": "daily", "timezone": "UTC"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recurring}");
    let recurring = &recurring["value"]["recurring"];
    sqlx::query(
        "UPDATE recurring_tasks SET next_run_at = now() - interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(recurring["id"].as_str().expect("schedule"))
    .execute(&pool)
    .await
    .expect("due");
    jobs::enqueue(&pool, NewJob::new("tasks.recurring_sweep"))
        .await
        .expect("sweep");
    run_jobs(&world).await;
    let task = task_titled(&pool, &project, "From a schedule").await;
    let facts = created(&pool, task).await;
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].0, Some(lena.actor));

    // A change is task.updated with what changed; a webhook hears both as
    // written, and nothing reconstructs them later.
    let (_, changed) = rook
        .call(
            &world.app,
            Call::Tool {
                name: "task_update",
                arguments: json!({
                    "taskId": task,
                    "priority": "high",
                    "expectedVersion": 1,
                    "clientRequestId": "change-1"
                }),
            },
        )
        .await;
    assert!(
        changed["outcome"] == "applied" || changed["outcome"] == "ok",
        "{changed}"
    );
    let changes = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT changes FROM event_staging WHERE type = 'task.updated' AND thing_id = $1 \
         UNION ALL SELECT changes FROM events WHERE type = 'task.updated' AND thing_id = $1",
    )
    .bind(task)
    .fetch_all(&pool)
    .await
    .expect("task.updated");
    assert_eq!(changes, vec![vec!["priority".to_owned()]]);
    world.finish().await;
}
