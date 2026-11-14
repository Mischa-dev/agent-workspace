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
    mcp_admin::{create_project, setup},
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
        "clientRunId": "journey-1",
        "title": "Survey the open issues",
        "cause": {"kind": "manual"}
    });
    let opened = call("run_open", open.clone()).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["run"]["state"], "working");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let again = call("run_open", open).await;
    assert_eq!(
        again["value"]["run"]["runId"],
        run_id.as_str(),
        "a retried open is the same run"
    );

    let steps = json!({"runId": run_id, "activities": [
        {"id": "s1", "kind": "step_started", "payload": {"title": "Read the issues"}},
        {"id": "s2", "kind": "text_message", "payload": {"text": "Three need a decision."}}
    ]});
    let reported = call("run_activity", steps.clone()).await;
    assert_eq!(reported["value"]["accepted"], 2, "{reported}");
    let retried = call("run_activity", steps).await;
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
        "run_result",
        json!({"runId": run_id, "results": [{"kind": "link", "url": "https://example.com/summary", "title": "Summary"}]}),
    )
    .await;
    assert_eq!(produced["outcome"], "ok", "{produced}");
    let finished = call(
        "run_activity",
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

    // Steps 2 to 8 are turned on by the builds that make them possible.
}
