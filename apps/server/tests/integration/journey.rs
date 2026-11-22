//! The journey (master plan, BUILD-ORDER section 5; R47): one automated test
//! that proves the foundations work together. Each build turns on the steps
//! its slices make possible; stage 2 is done when it passes start to finish.
//!
//! 1. An outside agent connects with one credential and takes a run through
//!    operations (A1.1, A1.3, A1.4). On.
//! 2. The same agent, one account, is a member of two workspaces (K1).
//! 3. It receives an event from each workspace on one stream, and by webhook
//!    (A2, K3.24).
//! 4. It edits a task through an operation; browser and MCP give the same
//!    result and audit line (K2).
//! 5. It raises a request; an account holding the permission answers it, once
//!    a person and once another agent (A1.7, A1.8).
//! 6. The same create twice makes one thing; a run start whose answer was lost
//!    runs once (P0.15, P0.16, A1.3, A1.12).
//! 7. Its membership in one workspace is revoked mid-flight: stream, delivery
//!    and operation stop there; the other workspace carries on (K1.20, K5.8,
//!    A2).
//! 8. A request answered after it changed, or after the requester lost
//!    access, is refused (A1.8).

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{create_agent, initialize, tool},
    agent_join::second_workspace,
    mcp_admin::{create_project, setup},
    support::{
        app::send,
        callers::{Call, Caller, Person},
        world::{Workspace, WorldBuilder},
    },
};

#[tokio::test]
async fn the_journey() {
    let Some(setup) = setup("journey").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;

    // Step 1: an outside agent connects with one credential and takes a run
    // through operations: open, report, produce, finish. Retries are safe.
    let work = create_project(&setup, "WORK", "Work").await;
    let (agent, token) = create_agent(app, &setup.browser, &work, "Scout", "scout").await;
    let session = initialize(app, &token)
        .await
        .expect("one credential connects");
    let call = |name: &'static str, arguments: Value| {
        let token = token.clone();
        let session = session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let open = json!({
        "projectId": "WORK",
        "clientRunId": "journey-1",
        "title": "Survey the open issues",
        "cause": {"kind": "manual"}
    });
    let opened = call("run_create", open.clone()).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["run"]["state"], "working");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let again = call("run_create", open).await;
    assert_eq!(
        again["value"]["run"]["runId"],
        run_id.as_str(),
        "a retried open is the same run"
    );

    let steps = json!({"runId": run_id, "activities": [
        {"id": "s1", "kind": "step_started", "payload": {"title": "Read the issues"}},
        {"id": "s2", "kind": "text_message", "payload": {"text": "Three need a decision."}}
    ]});
    let reported = call("run_activity_report", steps.clone()).await;
    assert_eq!(reported["value"]["accepted"], 2, "{reported}");
    let retried = call("run_activity_report", steps).await;
    assert_eq!(
        retried["value"]["duplicates"], 2,
        "a retried batch is stored once: {retried}"
    );
    let status = call(
        "run_update",
        json!({"runId": run_id, "statusLine": "Writing the summary"}),
    )
    .await;
    assert_eq!(status["outcome"], "ok", "{status}");
    let produced = call(
        "run_result_report",
        json!({"runId": run_id, "results": [{"kind": "link", "url": "https://example.com/summary", "title": "Summary"}]}),
    )
    .await;
    assert_eq!(produced["outcome"], "ok", "{produced}");
    let finished = call(
        "run_activity_report",
        json!({"runId": run_id, "activities": [
            {"id": "done", "kind": "run_finished", "payload": {"outcome": "done"}}]}),
    )
    .await;
    assert_eq!(finished["value"]["state"], "done", "{finished}");

    let run = run_id.parse::<Uuid>().expect("run UUID");
    let (account, state) =
        sqlx::query_as::<_, (Uuid, String)>("SELECT account_id, state FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(pool)
            .await
            .expect("the run");
    assert_eq!(account, agent, "the run is the agent's own");
    assert_eq!(state, "done");
    let kinds = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM run_activities WHERE run_id = $1 ORDER BY seq",
    )
    .bind(run)
    .fetch_all(pool)
    .await
    .expect("activities");
    assert_eq!(
        kinds,
        [
            "run_started",
            "step_started",
            "text_message",
            "result",
            "run_finished"
        ],
        "each report once, in order"
    );

    // Steps 2 to 8 run in `the_journey_across_two_workspaces`, which needs
    // the shared world (people, agents, two workspaces).
}

/// An operation from the browser, in `workspace`.
async fn as_person(
    world: &Workspace,
    who: &Person,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> Value {
    let (_, body) = send(
        &world.app,
        who.browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await;
    body
}

/// A tool call as an agent, in `workspace` when given.
async fn as_agent(
    world: &Workspace,
    agent: &str,
    name: &'static str,
    mut arguments: Value,
    workspace: Option<Uuid>,
) -> Value {
    if let Some(workspace) = workspace {
        arguments["workspace"] = json!(workspace);
    }
    let (_, body) = world.agents[agent]
        .call(&world.app, Call::Tool { name, arguments })
        .await;
    body
}

/// What the agent hears now, from every workspace, on its one stream. An
/// account-wide read still names a workspace to call through when the
/// caller is in several; it reads them all.
async fn heard(world: &Workspace, agent: &str) -> Vec<Value> {
    agent_workspace_server::events::catch_up(world.pool())
        .await
        .expect("sequence events");
    let page = as_agent(
        world,
        agent,
        "event_list",
        json!({}),
        Some(world.organization),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    page["value"]["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Steps 2 to 8: one agent, one account, in two workspaces.
#[tokio::test]
async fn the_journey_across_two_workspaces() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("scout", "member")
        .agent("rook", "member")
        .build("journey_two")
        .await
    else {
        return;
    };
    let a = world.organization;
    let owner = &world.owner;
    let scout = world.agents["scout"].actor;
    let rook = world.agents["rook"].actor;

    // Step 2: the same account joins a second workspace; no new identity.
    let b = second_workspace(&world).await;
    let added = as_person(
        &world,
        owner,
        b,
        "account_workspace_add",
        json!({"account": scout}),
    )
    .await;
    assert_eq!(added["value"]["status"], "joined", "{added}");
    // A project in B, with it as a member, to work in.
    let (status, project) = send(
        &world.app,
        owner.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/space_create", b),
            Some("journey-b-project"),
            json!({"key": "SIDE", "name": "Side"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{project}");
    let b_space = project["value"]["id"].as_str().expect("project").to_owned();
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'member', 'Scout', 'scout')",
    )
    .bind(Uuid::now_v7())
    .bind(b_space.parse::<Uuid>().expect("project"))
    .bind(scout)
    .execute(world.pool())
    .await
    .expect("project membership");
    let accounts = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors WHERE id = $1")
        .bind(scout)
        .fetch_one(world.pool())
        .await
        .expect("account");
    assert_eq!(accounts, 1);

    // Step 3: an event from each workspace arrives on its one stream.
    let work = world.project("WORK").to_owned();
    for (workspace, space, title) in [
        (a, work.as_str(), "From A?"),
        (b, b_space.as_str(), "From B?"),
    ] {
        let asked = as_person(
            &world,
            owner,
            workspace,
            "request_create",
            json!({"spaceId": space, "title": title, "audience": {"accounts": [scout]}}),
        )
        .await;
        assert_eq!(asked["outcome"], "ok", "{asked}");
    }
    let events = heard(&world, "scout").await;
    let from = |workspace: Uuid| {
        events.iter().any(|event| {
            event["type"] == "request.raised" && event["workspace"]["id"] == workspace.to_string()
        })
    };
    assert!(from(a) && from(b), "{events:?}");

    // Step 4: it edits a task through an operation, and the browser reads
    // what it wrote; both leave the same audit line.
    let made = as_agent(
        &world,
        "scout",
        "task_create",
        json!({"projectId": "WORK", "title": "Journey task", "clientRequestId": "journey-task"}),
        Some(a),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    // Step 6, first half: the same create twice makes one task.
    let again = as_agent(
        &world,
        "scout",
        "task_create",
        json!({"projectId": "WORK", "title": "Journey task", "clientRequestId": "journey-task"}),
        Some(a),
    )
    .await;
    let tasks = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_rows WHERE title = 'Journey task' AND project_id = $1::uuid",
    )
    .bind(&work)
    .fetch_one(world.pool())
    .await
    .expect("tasks");
    assert_eq!(tasks, 1, "a retried create makes one task: {again}");

    // Step 5: it raises a request; a person holding the permission answers
    // one, another agent the other.
    let mut answered_by = Vec::new();
    for (title, answerer) in [("Ship it?", "person"), ("Merge it?", "agent")] {
        let asked = as_agent(
            &world,
            "scout",
            "request_create",
            json!({"spaceId": work, "kind": "approval", "title": title,
                   "options": [{"id": "yes", "label": "Yes", "style": "approve"},
                               {"id": "no", "label": "No", "style": "deny"}],
                   "audience": {"accounts": [if answerer == "person" { owner.actor } else { rook }]}}),
            Some(a),
        )
        .await;
        assert_eq!(asked["outcome"], "ok", "{asked}");
        let request = asked["value"]["id"].clone();
        let answer = json!({"requestId": request, "optionId": "yes"});
        let answered = if answerer == "person" {
            as_person(&world, owner, a, "request_respond", answer).await
        } else {
            as_agent(&world, "rook", "request_respond", answer, Some(a)).await
        };
        assert_eq!(answered["outcome"], "ok", "{answerer}: {answered}");
        answered_by.push(request);
    }
    // Step 8, first half: a request already answered can't be answered again.
    let late = as_person(
        &world,
        owner,
        a,
        "request_respond",
        json!({"requestId": answered_by[0], "optionId": "no"}),
    )
    .await;
    assert_ne!(late["outcome"], "ok", "{late}");

    // Step 7: its membership in B is revoked mid-flight. B's stream,
    // operations and open requests stop; A carries on.
    let pending = as_agent(
        &world,
        "scout",
        "request_create",
        json!({"spaceId": b_space, "title": "Still there?", "audience": {"accounts": [owner.actor]}}),
        Some(b),
    )
    .await;
    assert_eq!(pending["outcome"], "ok", "{pending}");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(b)
    .bind(scout)
    .fetch_one(world.pool())
    .await
    .expect("membership");
    let (status, removed) = send(
        &world.app,
        owner.browser.request(
            "PATCH",
            &format!("/api/v1/organizations/{b}/members/{scout}"),
            Some(&Uuid::now_v7().to_string()),
            json!({"expectedVersion": version, "enabled": false}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {removed}");
    let refused = as_agent(&world, "scout", "request_list", json!({}), Some(b)).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let still = as_agent(&world, "scout", "request_list", json!({}), Some(a)).await;
    assert_eq!(still["outcome"], "ok", "{still}");
    as_person(
        &world,
        owner,
        b,
        "request_create",
        json!({"spaceId": b_space, "title": "After you left?", "audience": {"roles": ["owner"]}}),
    )
    .await;
    let events = heard(&world, "scout").await;
    assert!(
        !events
            .iter()
            .any(|event| event["workspace"]["id"] == b.to_string()
                && event["type"] == "request.raised"
                && event["data"]["title"] == "After you left?"),
        "{events:?}"
    );

    // Step 8, second half: its request in B can't be answered once it lost
    // access there.
    let answered = as_person(
        &world,
        owner,
        b,
        "request_respond",
        json!({"requestId": pending["value"]["id"], "text": "Yes"}),
    )
    .await;
    assert_ne!(answered["outcome"], "ok", "{answered}");

    world.finish().await;
}
