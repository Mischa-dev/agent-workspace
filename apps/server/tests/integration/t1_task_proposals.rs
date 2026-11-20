//! T1.41: tasks can be proposed. The silent, costly failures: a
//! suggest-only role changing a task directly (or a role that can't even
//! suggest getting a proposal in), a proposal's author deciding it
//! themselves, an accepted proposal written as whoever accepted it, one
//! that undoes a change made since, and a retried call making two.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

fn ok(answer: &Value) -> &Value {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    &answer["value"]
}

#[tokio::test]
async fn a_suggested_task_change_waits_for_someone_who_edits_tasks() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("Codex")
        .agent("sol", "member")
        .agent("vee", "member")
        .custom_role(
            "Member, suggests tasks",
            &[
                "projects.view",
                "tasks.view",
                "tasks.suggest",
                "requests.raise",
                "directory.view",
            ],
        )
        .custom_role(
            "Member, reads tasks",
            &["projects.view", "tasks.view", "directory.view"],
        )
        .build("t1_task_proposals")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let (codex, sol, vee) = (
        &world.apps["Codex"],
        &world.agents["sol"],
        &world.agents["vee"],
    );
    let as_codex = |name: &'static str, arguments: Value| async move {
        codex.call(app, Call::Tool { name, arguments }).await.1
    };
    let as_sol = |name: &'static str, arguments: Value| async move {
        sol.call(app, Call::Tool { name, arguments }).await.1
    };
    for (who, role) in [
        (sol.actor, "Member, suggests tasks"),
        (vee.actor, "Member, reads tasks"),
    ] {
        sqlx::query(
            "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
        )
        .bind(Uuid::parse_str(&world.roles[role]).expect("role id"))
        .bind(work)
        .bind(who)
        .execute(pool)
        .await
        .expect("narrow the role");
    }

    let made = as_codex(
        "task_create",
        json!({"projectId": "WORK", "title": "Fix the login", "priority": "low",
               "clientRequestId": "fix-login"}),
    )
    .await;
    let task = Uuid::parse_str(ok(&made)["workItemId"].as_str().expect("id")).expect("task id");
    let read = || async move {
        sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT task.version, task.title, task.priority, status.name FROM task_rows task \
             JOIN work_item_statuses status ON status.id = task.status_id WHERE task.id = $1",
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("task")
    };
    let proposals = || async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM proposals WHERE thing_kind = 'task' AND thing_id = $1",
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("proposals")
    };
    let (version, title, priority, status) = read().await;

    // Vee only reads tasks: no change, and no proposal either.
    let refused = vee
        .call(
            app,
            Call::Tool {
                name: "task_update",
                arguments: json!({"taskId": task.to_string(), "expectedVersion": version,
                                  "priority": "high"}),
            },
        )
        .await
        .1;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    assert_eq!(proposals().await, 0);

    // Sol only suggests: task_update keeps the change as a proposal and
    // leaves the task as it was. Moving or archiving needs edit access.
    let archive = as_sol(
        "task_update",
        json!({"taskId": task.to_string(), "expectedVersion": version, "archived": true}),
    )
    .await;
    assert_eq!(archive["outcome"], "permission_denied", "{archive}");
    let asked = json!({"taskId": task.to_string(), "expectedVersion": version,
                       "title": "Fix the login page", "priority": "high",
                       "status": "Ready", "note": "Seen on the phone too",
                       "clientRequestId": "sol-propose-1"});
    let proposed = as_sol("task_update", asked.clone()).await;
    let proposal = ok(&proposed)["proposalId"]
        .as_str()
        .expect("proposal")
        .to_owned();
    assert_eq!(ok(&proposed)["version"], json!(version), "{proposed}");
    assert_eq!(
        ok(&proposed)["proposed"].as_array().map(Vec::len),
        Some(3),
        "{proposed}"
    );
    assert_eq!(read().await, (version, title, priority, status.clone()));
    // A retry of the same call answers with the same proposal.
    let again = as_sol("task_update", asked).await;
    assert_eq!(ok(&again)["proposalId"], json!(proposal), "{again}");
    assert_eq!(proposals().await, 1);

    // Its author can't decide it; its card asks the people who edit it.
    let own = as_sol(
        "proposal_decide",
        json!({"proposalId": proposal, "decision": "accept"}),
    )
    .await;
    assert_eq!(own["outcome"], "permission_denied", "{own}");
    let listed = as_codex(
        "proposal_list",
        json!({"thingKind": "task", "thingId": task.to_string()}),
    )
    .await;
    assert_eq!(
        ok(&listed)["proposals"][0]["thingTitle"],
        json!("Fix the login"),
        "{listed}"
    );
    let request = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM requests WHERE subject_kind = 'proposal' AND subject_id = $1::uuid \
           AND status = 'open'",
    )
    .bind(&proposal)
    .fetch_one(pool)
    .await
    .expect("the proposal's request card");

    // The owner accepts it from the card: one new version, written as
    // Sol's change, and the audit row says who accepted it.
    ok(&as_codex(
        "request_respond",
        json!({"requestId": request.to_string(), "optionId": "accept"}),
    )
    .await);
    let (accepted, title, priority, status) = read().await;
    assert_eq!(accepted, version + 1);
    assert_eq!(
        (title.as_str(), priority.as_str(), status.as_str()),
        ("Fix the login page", "high", "Ready")
    );
    let (status_now, applied) = sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT status, applied_version FROM proposals WHERE id = $1::uuid",
    )
    .bind(&proposal)
    .fetch_one(pool)
    .await
    .expect("proposal");
    assert_eq!((status_now.as_str(), applied), ("accepted", Some(accepted)));
    let writer = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM work_item_activity WHERE work_item_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("activity");
    assert_eq!(writer, sol.actor, "written as its author");
    let accepted_by = sqlx::query_scalar::<_, Option<String>>(
        "SELECT metadata->>'acceptedBy' FROM audit_events \
         WHERE target_id = $1 AND authentication_class = 'proposal' AND actor_id = $2",
    )
    .bind(task)
    .bind(sol.actor)
    .fetch_one(pool)
    .await
    .expect("audit row");
    assert_eq!(accepted_by, Some(world.owner.actor.to_string()));

    // A proposal over a part someone changed since is refused, never
    // undoing that change.
    let second = as_sol(
        "task_update",
        json!({"taskId": task.to_string(), "expectedVersion": accepted, "priority": "urgent"}),
    )
    .await;
    let second = ok(&second)["proposalId"]
        .as_str()
        .expect("second proposal")
        .to_owned();
    ok(&as_codex(
        "task_update",
        json!({"taskId": task.to_string(), "expectedVersion": accepted, "priority": "medium"}),
    )
    .await);
    let stale = as_codex(
        "proposal_decide",
        json!({"proposalId": second, "decision": "accept"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert_eq!(read().await.2, "medium");

    // The author withdraws one; it never lands.
    let (now, ..) = read().await;
    let third = as_sol(
        "task_update",
        json!({"taskId": task.to_string(), "expectedVersion": now, "title": "Drop it"}),
    )
    .await;
    let third = ok(&third)["proposalId"]
        .as_str()
        .expect("third proposal")
        .to_owned();
    ok(&as_sol(
        "proposal_decide",
        json!({"proposalId": third, "decision": "withdraw"}),
    )
    .await);
    assert_eq!(read().await.1, "Fix the login page");
}
