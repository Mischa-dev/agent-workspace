//! A1.3: run operations for any agent. The silent failures: a retried batch
//! storing steps twice, another agent writing into someone's run, a secret
//! in a tool call's output landing in the database, a run that stopped
//! reporting still showing as fine, and a stop that never reaches the agent.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
};

#[tokio::test]
async fn an_agent_reports_its_own_run_and_nobody_else_can() {
    let Some(setup) = setup("a1_run_reporting").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (_fixer, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let (_other, other_token) = create_agent(app, &setup.browser, &work, "Other", "other").await;
    let other = initialize(app, &other_token).await.expect("Other MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = fixer_token.clone();
        let session = fixer.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // Open, then open again with the same clientRunId: one run.
    let open = json!({
        "clientRunId": "triage-2026-11-14",
        "title": "Triage new feedback",
        "cause": {"kind": "schedule", "ref": "weekday 09:00"},
        "reportIntervalSeconds": 600
    });
    let opened = call("run_open", open.clone()).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert_eq!(opened["value"]["created"], true);
    assert_eq!(opened["value"]["run"]["state"], "working");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let again = call("run_open", open).await;
    assert_eq!(again["value"]["created"], false, "{again}");
    assert_eq!(again["value"]["run"]["runId"], run_id.as_str());

    // A batch, then the same batch again: duplicates, no new rows.
    let batch = json!({"runId": run_id, "activities": [
        {"id": "a1", "kind": "plan", "payload": {"items": [
            {"title": "Read feedback", "status": "done"},
            {"title": "File tasks", "status": "in_progress"}]}},
        {"id": "a2", "kind": "tool_call", "stepRef": "s2", "summary": "shell env",
         "payload": {"toolCallRef": "t7", "tool": "shell", "status": "completed",
                     "output": "export API_KEY=abcdef1234567890abcdef done"}},
        {"id": "a3", "kind": "text_message", "payload": {"text": "Grouped by theme."}}
    ]});
    let reported = call("run_activity", batch.clone()).await;
    assert_eq!(reported["outcome"], "ok", "{reported}");
    assert_eq!(reported["value"]["accepted"], 3);
    assert_eq!(reported["value"]["duplicates"], 0);
    let repeated = call("run_activity", batch).await;
    assert_eq!(repeated["value"]["accepted"], 0, "{repeated}");
    assert_eq!(repeated["value"]["duplicates"], 3);
    let run = run_id.parse::<Uuid>().expect("run UUID");
    let stored = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_activities WHERE run_id = $1")
        .bind(run)
        .fetch_one(&setup.pool)
        .await
        .expect("count");
    assert_eq!(stored, 4, "run_started and the three reported");
    let payloads = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM run_activities WHERE run_id = $1",
    )
    .bind(run)
    .fetch_all(&setup.pool)
    .await
    .expect("payloads");
    assert!(
        !Value::Array(payloads).to_string().contains("abcdef1234567890abcdef"),
        "secrets are removed before storing"
    );

    // Another agent in the same project can't report on it, change it or
    // see it exists.
    for (name, arguments) in [
        (
            "run_activity",
            json!({"runId": run_id, "activities": [
                {"id": "x", "kind": "text_message", "payload": {"text": "mine now"}}]}),
        ),
        ("run_update", json!({"runId": run_id, "state": "done"})),
        (
            "run_result",
            json!({"runId": run_id, "results": [{"kind": "link", "url": "https://example.com/x"}]}),
        ),
        (
            "run_usage_report",
            json!({"runId": run_id, "reportKey": "x", "model": "m", "inputTokens": 1}),
        ),
    ] {
        let refused = tool(app, &other_token, &other, name, arguments).await;
        assert_eq!(refused["outcome"], "not_found", "{name}: {refused}");
    }

    // An interrupt makes it wait; its answer puts it back to work.
    let asked = call(
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "q1", "kind": "interrupt",
            "payload": {"request": {"kind": "question", "title": "One task or three?"}}}]}),
    )
    .await;
    assert_eq!(asked["value"]["state"], "waiting", "{asked}");
    let request = asked["value"]["requests"][0]["requestId"]
        .as_str()
        .expect("the interrupt opened a request")
        .parse::<Uuid>()
        .expect("request UUID");
    let request_state = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (String, Option<Uuid>, String)>(
            "SELECT status, run_id, title FROM requests WHERE id = $1",
        )
        .bind(request)
        .fetch_one(&pool)
        .await
        .expect("request")
    };
    assert_eq!(
        request_state(setup.pool.clone()).await,
        ("open".to_owned(), Some(run), "One task or three?".to_owned())
    );

    // Silent for three intervals: not responding; the next report: ok.
    sqlx::query("UPDATE runs SET last_report_at = now() - interval '31 minutes' WHERE id = $1")
        .bind(run)
        .execute(&setup.pool)
        .await
        .expect("age the run");
    agent_workspace_server::runs::liveness::sweep(&setup.pool)
        .await
        .expect("sweep");
    let liveness = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT liveness FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .expect("liveness")
    };
    assert_eq!(liveness(setup.pool.clone()).await, "not_responding");
    let answered = call(
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "q1-answer",
            "kind": "interrupt_resolved", "payload": {"requestId": "q1"}}]}),
    )
    .await;
    assert_eq!(answered["value"]["state"], "working", "{answered}");
    assert_eq!(request_state(setup.pool.clone()).await.0, "superseded");
    assert_eq!(liveness(setup.pool.clone()).await, "ok");

    // Usage and results are kept once each.
    let usage = json!({"runId": run_id, "reportKey": "u1", "model": "model-a",
        "inputTokens": 1200, "outputTokens": 80, "cost": 0.5});
    let recorded = call("run_usage_report", usage.clone()).await;
    assert_eq!(recorded["value"]["duplicate"], false, "{recorded}");
    let recorded = call("run_usage_report", usage).await;
    assert_eq!(recorded["value"]["duplicate"], true, "{recorded}");
    let tokens = sqlx::query_scalar::<_, i64>(
        "SELECT input_tokens FROM run_usage WHERE run_id = $1 AND attribution = 'reported'",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("usage");
    assert_eq!(tokens, 1200);
    let result = json!({"runId": run_id, "results": [
        {"kind": "pull_request", "url": "https://code.example/acme/app/pull/12", "number": 12}]});
    let added = call("run_result", result.clone()).await;
    assert_eq!(added["value"]["added"], 1, "{added}");
    let added = call("run_result", result).await;
    assert_eq!(added["value"]["added"], 0, "{added}");

    // run_finished finishes it, and a finished run stays finished.
    let finished = call(
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "end", "kind": "run_finished",
            "payload": {"outcome": "done"}}]}),
    )
    .await;
    assert_eq!(finished["value"]["state"], "done", "{finished}");
    let reopened = call("run_update", json!({"runId": run_id, "state": "working"})).await;
    assert_eq!(reopened["outcome"], "invalid_request", "{reopened}");
    let events = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM outbox_events WHERE event_type = 'run.changed' AND resource_id = $1",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("events");
    assert!(events >= 5, "every change is an event: {events}");
}

#[tokio::test]
async fn a_person_stops_an_agents_run_and_the_agent_hears_it() {
    let Some(setup) = setup("a1_run_cancel").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (_fixer, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let person = connect_app(app, &setup.browser, "Desk", "full").await;
    let desk = initialize(app, &person).await.expect("Desk MCP");

    let opened = tool(
        app,
        &fixer_token,
        &fixer,
        "run_open",
        json!({"title": "Long job", "cause": {"kind": "manual"}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();

    // A person who may stop runs there asks; a retry with the same
    // clientRequestId asks once.
    let cancel = json!({"runId": run_id, "reason": "Wrong branch", "clientRequestId": "stop-1"});
    let asked = tool(app, &person, &desk, "run_cancel", cancel.clone()).await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    assert_eq!(asked["value"]["run"]["state"], "cancel_requested");
    let again = tool(app, &person, &desk, "run_cancel", cancel).await;
    assert_eq!(again["value"]["idempotentReplay"], true, "{again}");
    let heard = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM external_agent_events WHERE kind = 'run.cancel' AND resource_id = $1",
    )
    .bind(run_id.parse::<Uuid>().expect("run UUID"))
    .fetch_one(&setup.pool)
    .await
    .expect("events");
    assert_eq!(heard, 1, "the agent hears run.cancel once");

    // The agent confirms.
    let confirmed = tool(
        app,
        &fixer_token,
        &fixer,
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "stopped", "kind": "run_finished",
            "payload": {"outcome": "cancelled"}}]}),
    )
    .await;
    assert_eq!(confirmed["value"]["state"], "cancelled", "{confirmed}");

    // Only a run whose start is uncertain can be marked as not started.
    let refused = tool(
        app,
        &person,
        &desk,
        "run_mark_not_started",
        json!({"runId": run_id}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
}
