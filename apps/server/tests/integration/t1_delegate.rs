//! T1.22: a task's delegate, who it's handed to to do, beside its assignee.
//! The silent, costly failures: a person and an agent not being able to
//! hold either role in any mix, a delegate who isn't in the project, the
//! delegate not being told, and a launch not handing the task to its agent.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn people_and_agents_hold_either_role_and_launches_hand_tasks_over() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_delegate")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let ana = world.people["ana"].actor;
    let rook = &world.agents["rook"];
    let laptop = &world.apps["laptop"];
    let as_app = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };
    let as_rook = |name: &'static str, arguments: Value| async move {
        rook.call(app, Call::Tool { name, arguments }).await.1
    };
    let delegate_of = |task: Uuid| async move {
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT delegate_actor_id FROM work_items WHERE id = $1",
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("delegate")
    };
    let made = as_app(
        "task_create",
        json!({"projectId": "WORK", "title": "Ship notes", "clientRequestId": "t-1"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let key = made["value"]["key"].as_str().expect("key").to_owned();
    let task = Uuid::parse_str(made["value"]["workItemId"].as_str().expect("id")).expect("id");
    let version = |body: &Value| body["value"]["version"].as_i64().expect("version");

    // A person assigned, an agent delegated; then the other way round.
    let mut current = version(&made);
    for (assignee, delegate) in [
        (format!("@{}", "ana"), "@rook".to_owned()),
        ("@rook".to_owned(), "@ana".to_owned()),
    ] {
        let changed = as_app(
            "task_update",
            json!({"taskId": key, "expectedVersion": current,
                   "assignee": assignee, "delegate": delegate}),
        )
        .await;
        assert_eq!(changed["outcome"], "ok", "{changed}");
        current = version(&changed);
    }
    assert_eq!(delegate_of(task).await, Some(ana));
    // An agent delegates a task to itself, and task_open shows it.
    let mine = as_rook(
        "task_update",
        json!({"taskId": key, "expectedVersion": current, "delegate": "me"}),
    )
    .await;
    assert_eq!(mine["outcome"], "ok", "{mine}");
    current = version(&mine);
    assert_eq!(delegate_of(task).await, Some(rook.actor));
    let opened = as_rook("task_open", json!({"taskId": key})).await;
    assert_eq!(
        opened["value"]["task"]["delegate"]["actorId"],
        json!(rook.actor.to_string()),
        "{opened}"
    );
    // "Delegated to me".
    let listed = as_rook("task_list", json!({"projectId": "WORK", "delegate": "me"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(
        listed["value"]["tasks"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    // The delegate is told, and only them.
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("sequence events");
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM events WHERE type = 'task.delegated' \
         AND audience_account_id = $1 AND thing_id = $2",
    )
    .bind(rook.actor)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("events");
    assert!(told >= 1);

    // Someone outside the project can't be the delegate, over MCP or the web.
    let outsider = Uuid::now_v7();
    let refused = as_app(
        "task_update",
        json!({"taskId": key, "expectedVersion": current, "delegate": outsider.to_string()}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (status, body) = crate::support::tasks::send(
        app,
        &world.owner.browser,
        "PATCH",
        &format!("/api/v1/projects/{work}/work-items/{task}"),
        Some(&Uuid::now_v7().to_string()),
        json!({"expectedVersion": current, "delegateActorId": outsider}),
    )
    .await;
    assert!(!status.is_success(), "{status} {body}");
    // The web takes it back.
    let (status, body) = crate::support::tasks::send(
        app,
        &world.owner.browser,
        "PATCH",
        &format!("/api/v1/projects/{work}/work-items/{task}"),
        Some(&Uuid::now_v7().to_string()),
        json!({"expectedVersion": current, "delegateActorId": null}),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(delegate_of(task).await, None);

    // A launch naming an agent hands the task to it.
    let launched = as_app(
        "task_launch",
        json!({"taskIds": [key], "prompt": "Write the notes.", "agentAccountId": rook.actor,
               "startWhen": "blockers_done", "clientRequestId": "launch-1"}),
    )
    .await;
    assert_eq!(launched["outcome"], "ok", "{launched}");
    assert_eq!(delegate_of(task).await, Some(rook.actor));

    world.finish().await;
}
