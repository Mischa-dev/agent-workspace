//! W1.42: trying an automation rule writes nothing. `automation_preview`
//! works out what a rule would do (the tasks it acts on, each action with
//! its text filled in) without firing it: no run row, no change to the task,
//! no notification. Only the rule's maker may try it, as only they may run
//! it, because the preview reads what the maker can see.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::support::{
    app::{create_agent, initialize, send, tool},
    world::{create_project, setup},
};

async fn count(pool: &sqlx::PgPool, sql: &str, id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn trying_a_rule_writes_nothing_and_is_for_its_maker() {
    let Some(setup) = setup("w1_42_preview").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "TRY", "Try").await;

    let (status, task) = crate::support::tasks::send(
        app,
        &setup.browser,
        "POST",
        &format!("/api/v1/projects/{work}/work-items"),
        Some("try-task"),
        json!({"title": "Needs a look"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = task["id"].as_str().expect("task id").to_owned();
    let task_uuid = task_id.parse::<uuid::Uuid>().expect("task uuid");

    let (status, rule) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/automation_create",
            Some("try-rule"),
            json!({
                "projectId": work,
                "name": "Raise and tell",
                "trigger": {"kind": "task_created"},
                "actions": [
                    {"kind": "set_field", "field": "priority", "value": "urgent"},
                    {"kind": "notify", "recipients": "owner", "message": "Look at {{task.title}}"},
                ],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rule}");
    let rule_id = rule["value"]["id"].as_str().expect("rule id").to_owned();
    let rule_uuid = rule_id.parse::<uuid::Uuid>().expect("rule uuid");
    let notifications = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications")
            .fetch_one(&setup.pool)
            .await
            .expect("notifications")
    };
    let notifications_before = notifications().await;

    let (status, preview) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v2/ops/automation_preview",
            None,
            json!({"projectId": work, "ruleId": rule_id, "taskId": task_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let value = &preview["value"];
    assert_eq!(value["tasks"], 1, "{preview}");
    let summaries = value["actions"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["summary"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries,
        [
            "Set priority to \"urgent\" on 1 task",
            "Notify you: \"Look at Needs a look\""
        ],
        "{preview}"
    );

    // Nothing happened: no run, the task as it was, nobody told.
    assert_eq!(
        count(
            &setup.pool,
            "SELECT count(*) FROM automation_runs WHERE rule_id = $1",
            rule_uuid
        )
        .await,
        0
    );
    let priority: String = sqlx::query_scalar("SELECT priority FROM work_items WHERE id = $1")
        .bind(task_uuid)
        .fetch_one(&setup.pool)
        .await
        .expect("priority");
    assert_ne!(priority, "urgent");
    assert_eq!(notifications().await, notifications_before);

    // Someone else in the space can't try the maker's rule.
    let (_, token) = create_agent(app, &setup.browser, &work, "Tess", "tess").await;
    let session = initialize(app, &token).await.expect("Tess MCP");
    let refused: Value = tool(
        app,
        &token,
        &session,
        "automation_preview",
        json!({"projectId": work, "ruleId": rule_id}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
}
