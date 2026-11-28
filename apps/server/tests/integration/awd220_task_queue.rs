//! The task queue for workers (AWD-220): `task_claim_next` and
//! `task_finish`. The silent failures: two workers handed the same task, a
//! worker handed a task that is someone else's, a wait that never sees a
//! task arrive, and a result that reaches nobody or finishes a task another
//! worker holds.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{app::tool, callers::Agent, world::WorldBuilder};

async fn call(app: &axum::Router, agent: &Agent, name: &str, arguments: Value) -> Value {
    let answer = tool(app, &agent.token, &agent.session, name, arguments).await;
    assert_eq!(answer["outcome"], "ok", "{name}: {answer}");
    answer["value"].clone()
}

async fn new_task(
    app: &axum::Router,
    browser: &super::support::app::Browser,
    project: &str,
    body: Value,
) -> String {
    let (status, task) = crate::support::tasks::send(
        app,
        browser,
        "POST",
        &format!("/api/v1/projects/{project}/work-items"),
        Some(&Uuid::now_v7().to_string()),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task["id"].as_str().expect("task").to_owned()
}

#[tokio::test]
async fn workers_share_a_queue_without_taking_the_same_task_or_someone_elses() {
    let Some(world) = WorldBuilder::default()
        .project("QU")
        .person("ben", "member")
        .agent("codex", "member")
        .agent("rook", "member")
        .build("awd220_queue")
        .await
    else {
        return;
    };
    let app = &world.app;
    let project = world.project("QU").to_owned();
    let owner = &world.owner.browser;
    let ben = &world.people["ben"];
    let codex = &world.agents["codex"];
    let rook = &world.agents["rook"];
    let urgent = new_task(
        app,
        owner,
        &project,
        json!({"title": "Fix the importer", "priority": "urgent"}),
    )
    .await;
    let plain = new_task(app, owner, &project, json!({"title": "Tidy the readme"})).await;
    // Assigned to a person: not a worker's to take from the queue.
    let bens = new_task(
        app,
        owner,
        &project,
        json!({"title": "Pick the logo", "priority": "urgent", "assigneeActorId": ben.actor}),
    )
    .await;

    let first = call(
        app,
        codex,
        "task_claim_next",
        json!({"spaceId": project, "note": "On it"}),
    )
    .await;
    assert_eq!(first["claimed"], true, "{first}");
    assert_eq!(
        first["task"]["taskId"],
        urgent.as_str(),
        "the most urgent first: {first}"
    );
    assert_eq!(first["claim"]["note"], "On it");
    let second = call(app, rook, "task_claim_next", json!({"spaceId": project})).await;
    assert_eq!(second["task"]["taskId"], plain.as_str(), "{second}");
    // Two workers on one account, asking at once: one task, one of them.
    let lone = new_task(app, owner, &project, json!({"title": "Bump the parser"})).await;
    let (one, other) = tokio::join!(
        call(app, rook, "task_claim_next", json!({"spaceId": project})),
        call(app, rook, "task_claim_next", json!({"spaceId": project})),
    );
    let got = [&one, &other]
        .iter()
        .filter(|answer| answer["task"]["taskId"] == lone.as_str())
        .count();
    assert_eq!(got, 1, "handed out once: {one} {other}");
    // Two workers on two accounts, asking at once.
    let raced = new_task(app, owner, &project, json!({"title": "Sort the fixtures"})).await;
    let (one, other) = tokio::join!(
        call(app, codex, "task_claim_next", json!({"spaceId": project})),
        call(app, rook, "task_claim_next", json!({"spaceId": project})),
    );
    let got = [&one, &other]
        .iter()
        .filter(|answer| answer["task"]["taskId"] == raced.as_str())
        .count();
    assert_eq!(got, 1, "handed out once: {one} {other}");
    // Given back with a handoff, a started task is the next one out, with
    // the note for whoever takes it.
    let holder = if one["claimed"] == true { codex } else { rook };
    call(
        app,
        holder,
        "task_release",
        json!({"taskId": raced, "handoff": "Fixtures sorted; the snapshot is left."}),
    )
    .await;
    let again = call(app, rook, "task_claim_next", json!({"spaceId": project})).await;
    assert_eq!(again["task"]["taskId"], raced.as_str(), "{again}");
    assert_eq!(
        again["claim"]["handoff"]["note"], "Fixtures sorted; the snapshot is left.",
        "{again}"
    );
    call(
        app,
        rook,
        "task_finish",
        json!({"taskId": raced, "result": "Snapshot updated."}),
    )
    .await;
    let third = call(app, rook, "task_claim_next", json!({"spaceId": project})).await;
    assert_eq!(
        third["claimed"], false,
        "only someone else's task is left: {third}"
    );
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_claims WHERE work_item_id = $1::uuid",
    )
    .bind(&bens)
    .fetch_one(world.pool())
    .await
    .expect("claims");
    assert_eq!(held, 0);

    // Labels narrow the queue.
    call(
        app,
        codex,
        "label_create",
        json!({"projectId": project, "name": "rust"}),
    )
    .await;
    let labelled = new_task(
        app,
        owner,
        &project,
        json!({"title": "Port the parser", "labels": ["rust"]}),
    )
    .await;
    new_task(
        app,
        owner,
        &project,
        json!({"title": "Restyle the header", "priority": "high"}),
    )
    .await;
    let rust = call(
        app,
        codex,
        "task_claim_next",
        json!({"spaceId": project, "labels": ["Rust"]}),
    )
    .await;
    assert_eq!(rust["task"]["taskId"], labelled.as_str(), "{rust}");

    // A waiting worker gets a task that arrives while it waits.
    let taken = call(app, rook, "task_claim_next", json!({"spaceId": project})).await;
    assert_eq!(taken["claimed"], true, "{taken}");
    let waiting = call(
        app,
        rook,
        "task_claim_next",
        json!({"spaceId": project, "waitSeconds": 20}),
    );
    let arriving = async {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        new_task(app, owner, &project, json!({"title": "Arrives later"})).await
    };
    let (got, late) = tokio::join!(waiting, arriving);
    assert_eq!(got["task"]["taskId"], late.as_str(), "{got}");
}

#[tokio::test]
async fn finishing_a_task_posts_its_result_moves_it_and_frees_it() {
    let Some(world) = WorldBuilder::default()
        .project("FN")
        .agent("codex", "member")
        .agent("rook", "member")
        .build("awd220_finish")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let project = world.project("FN").to_owned();
    let owner = &world.owner.browser;
    let codex = &world.agents["codex"];
    let rook = &world.agents["rook"];
    let task = new_task(app, owner, &project, json!({"title": "Split the importer"})).await;
    let state = |task: String| async move {
        sqlx::query_as::<_, (String, Option<String>, i64, i64)>(
            r#"
            SELECT status.semantic_category, item.blocked_reason,
                   (SELECT count(*) FROM work_item_claims claim WHERE claim.work_item_id = item.id),
                   (SELECT count(*) FROM messages comment
                    JOIN conversations comments ON comments.id = comment.conversation_id
                    WHERE comments.kind = 'comments' AND comments.subject_id = item.id)
            FROM task_rows item
            JOIN work_item_statuses status ON status.id = item.status_id
            WHERE item.id = $1::uuid
            "#,
        )
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("task")
    };

    call(app, codex, "task_claim_next", json!({"spaceId": project})).await;
    assert_eq!(
        state(task.clone()).await.0,
        "started",
        "a claimed task shows as being worked on"
    );
    // Not the holder: refused, and nothing is posted.
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "task_finish",
        json!({"taskId": task, "result": "Done by me."}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert_eq!(state(task.clone()).await.3, 0);

    let finished = call(
        app,
        codex,
        "task_finish",
        json!({"taskId": task, "result": "Split into parse.rs and write.rs; tests pass on the branch importer-split."}),
    )
    .await;
    assert_eq!(finished["outcome"], "done", "{finished}");
    assert_eq!(finished["released"], true, "{finished}");
    let (category, blocked, claims, comments) = state(task.clone()).await;
    assert_eq!(category, "completed");
    assert_eq!(blocked, None);
    assert_eq!(claims, 0, "the claim is released");
    assert_eq!(comments, 1, "the result is on the task");
    // Whoever made the task is told what came of it.
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("catch up with the log");
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'thing_comment'",
    )
    .bind(world.owner.actor)
    .fetch_one(pool)
    .await
    .expect("notifications");
    assert_eq!(told, 1);

    // Failed: the task stays open, marked blocked with the reason.
    let stuck = new_task(
        app,
        owner,
        &project,
        json!({"title": "Publish the package"}),
    )
    .await;
    call(app, rook, "task_claim_next", json!({"spaceId": project})).await;
    let failed = call(
        app,
        rook,
        "task_finish",
        json!({"taskId": stuck, "outcome": "failed", "result": "No registry token here.\nSomeone with access needs to publish."}),
    )
    .await;
    assert_eq!(failed["outcome"], "failed", "{failed}");
    let (category, blocked, claims, comments) = state(stuck).await;
    assert_ne!(category, "completed");
    assert_eq!(blocked.as_deref(), Some("No registry token here."));
    assert_eq!((claims, comments), (0, 1));
}

/// A guard on changing tasks holds wherever a task is changed: the queue
/// operations change a task inside one call, and a worker must not get
/// past the guard by using them instead of `task_update`.
#[tokio::test]
async fn a_guard_on_task_changes_holds_inside_the_queue_operations() {
    let Some(world) = WorldBuilder::default()
        .project("QG")
        .agent("rook", "member")
        .build("awd220_queue_guard")
        .await
    else {
        return;
    };
    let app = &world.app;
    let project = world.project("QG").to_owned();
    let owner = &world.owner.browser;
    let rook = &world.agents["rook"];
    let task = new_task(app, owner, &project, json!({"title": "Ship the importer"})).await;
    let guard = |input: Value| async {
        let (status, body) = super::support::app::send(
            app,
            owner.request(
                "POST",
                &format!("/api/v2/w/{}/ops/guard_set", world.organization),
                Some(&Uuid::now_v7().to_string()),
                input,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["value"]["guard"]["id"].clone()
    };
    let claims = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_item_claims")
            .fetch_one(world.pool())
            .await
            .expect("claims")
    };

    let stop = guard(json!({"name": "Statuses stay put", "action": "deny",
                            "operations": ["task_update"], "targetKinds": ["task"]}))
    .await;
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "task_claim_next",
        json!({"spaceId": project}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(
        refused.to_string().contains("Statuses stay put"),
        "{refused}"
    );
    assert_eq!(claims().await, 0);

    // Claimed the plain way, which the guard doesn't name; finishing it
    // would move it, so that is stopped and nothing is posted.
    call(app, rook, "task_claim", json!({"taskId": task})).await;
    let refused = tool(
        app,
        &rook.token,
        &rook.session,
        "task_finish",
        json!({"taskId": task, "result": "Done."}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(
        refused.to_string().contains("Statuses stay put"),
        "{refused}"
    );
    let comments = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages comment \
         JOIN conversations comments ON comments.id = comment.conversation_id \
         WHERE comments.kind = 'comments' AND comments.subject_id = $1",
    )
    .bind(Uuid::parse_str(&task).expect("id"))
    .fetch_one(world.pool())
    .await
    .expect("comments");
    assert_eq!((claims().await, comments), (1, 0));

    // With the guard gone, the same call goes through.
    guard(json!({"guardId": stop, "remove": true})).await;
    let finished = call(
        app,
        rook,
        "task_finish",
        json!({"taskId": task, "result": "Done."}),
    )
    .await;
    assert_eq!(finished["outcome"], "done", "{finished}");
}
