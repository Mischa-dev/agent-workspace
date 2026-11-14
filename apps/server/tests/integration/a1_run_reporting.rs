//! A1.3: run operations for any agent. The silent failures: a retried batch
//! storing steps twice, another agent writing into someone's run, a secret
//! in a tool call's output landing in the database, a run that stopped
//! reporting still showing as fine, and a stop that never reaches the agent.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::add_member,
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

    // A secret in a title is removed before it is stored, on open and on
    // update, as it is in status lines and activities.
    let titled = call(
        "run_open",
        json!({"projectId": "WORK", "title": "Deploy API_KEY=abcdef1234567890",
               "cause": {"kind": "manual"}}),
    )
    .await;
    assert_eq!(titled["outcome"], "ok", "{titled}");
    let titled_id = titled["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let retitled = call(
        "run_update",
        json!({"runId": titled_id, "title": "Rotate token=zyxw9876vuts5432"}),
    )
    .await;
    assert_eq!(retitled["outcome"], "ok", "{retitled}");
    let stored = sqlx::query_scalar::<_, String>("SELECT title FROM runs WHERE id = $1::uuid")
        .bind(&titled_id)
        .fetch_one(&setup.pool)
        .await
        .expect("title");
    for (secret, shown) in [
        ("abcdef1234567890", titled.to_string()),
        ("zyxw9876vuts5432", retitled.to_string()),
        ("zyxw9876vuts5432", stored.clone()),
    ] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    assert!(stored.starts_with("Rotate"), "{stored}");
    let ended = call("run_update", json!({"runId": titled_id, "state": "done"})).await;
    assert_eq!(ended["outcome"], "ok", "{ended}");

    // Open, then open again with the same clientRunId: one run.
    let open = json!({
        "projectId": "WORK",
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
    let stored =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_activities WHERE run_id = $1")
            .bind(run)
            .fetch_one(&setup.pool)
            .await
            .expect("count");
    assert_eq!(stored, 4, "run_started and the three reported");
    let payloads =
        sqlx::query_scalar::<_, Value>("SELECT payload FROM run_activities WHERE run_id = $1")
            .bind(run)
            .fetch_all(&setup.pool)
            .await
            .expect("payloads");
    assert!(
        !Value::Array(payloads)
            .to_string()
            .contains("abcdef1234567890abcdef"),
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
        (
            "open".to_owned(),
            Some(run),
            "One task or three?".to_owned()
        )
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
    // An interrupt after the end is kept as a step, but opens no request
    // that would stay open on a finished run.
    let late = call(
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "late-q", "kind": "interrupt",
            "payload": {"request": {"kind": "question", "title": "Anything else?"}}}]}),
    )
    .await;
    assert_eq!(late["outcome"], "ok", "{late}");
    assert_eq!(late["value"]["state"], "done", "{late}");
    let (kept, open) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM run_activities WHERE run_id = $1 AND client_id = 'late-q'),
               (SELECT count(*) FROM requests WHERE run_id = $1 AND status = 'open')
        "#,
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("late interrupt");
    assert_eq!((kept, open), (1, 0), "kept, and nothing left open");

    // The largest valid ask (8 options, 40-character ids, 200-character
    // labels of three-byte characters) is stored, not rolled back.
    // Settings carry a password under its name: it is not stored.
    let asking = call(
        "run_open",
        json!({"projectId": "WORK", "title": "Big ask", "cause": {"kind": "manual"},
               "settings": {"password": "correct-horse-battery-staple",
                            "endpoints": [{"apiKey": "plain-words-key"}]}}),
    )
    .await;
    assert_eq!(asking["outcome"], "ok", "{asking}");
    let asking_run = asking["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .parse::<Uuid>()
        .expect("run UUID");
    let options = (0..8)
        .map(|index| json!({"id": format!("{index}{}", "i".repeat(39)), "label": "漢".repeat(200)}))
        .collect::<Vec<_>>();
    let asked = call(
        "run_activity",
        json!({"runId": asking["value"]["run"]["runId"],
               "activities": [{"id": "ask", "kind": "interrupt",
                   "payload": {"request": {"kind": "approval", "title": "Which way?",
                                            "options": options}}}]}),
    )
    .await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    assert_eq!(asked["value"]["state"], "waiting", "{asked}");

    // So does a step's payload, at any depth.
    let step = call(
        "run_activity",
        json!({"runId": asking_run.to_string(), "activities": [{"id": "login", "kind": "tool_call",
            "payload": {"toolCallRef": "t9", "input": {"user": "dana",
                        "password": "correct-horse-battery-staple",
                        "headers": [{"Authorization": "Basic plain-words-key"}]}}}]}),
    )
    .await;
    assert_eq!(step["outcome"], "ok", "{step}");
    let stored = sqlx::query_scalar::<_, String>(
        r#"
        SELECT run.settings::text || (SELECT string_agg(activity.payload::text, ' ')
                                      FROM run_activities activity WHERE activity.run_id = run.id)
        FROM runs run WHERE run.id = $1
        "#,
    )
    .bind(asking_run)
    .fetch_one(&setup.pool)
    .await
    .expect("stored");
    assert!(
        !stored.contains("correct-horse-battery-staple") && !stored.contains("plain-words-key"),
        "{stored}"
    );
    assert!(stored.contains("dana"), "the rest is kept: {stored}");
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
        json!({"projectId": "WORK", "title": "Long job", "cause": {"kind": "manual"}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();

    // A person who may stop runs there asks; a retry with the same
    // clientRequestId asks once.
    // The longest reason (200 characters) still fits the status line.
    let reason = format!("Wrong branch. {}", "x".repeat(186));
    let cancel = json!({"runId": run_id, "reason": reason, "clientRequestId": "stop-1"});
    let asked = tool(app, &person, &desk, "run_cancel", cancel.clone()).await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    assert_eq!(asked["value"]["run"]["state"], "cancel_requested");
    let status_line =
        sqlx::query_scalar::<_, Option<String>>("SELECT status_line FROM runs WHERE id = $1")
            .bind(run_id.parse::<Uuid>().expect("run UUID"))
            .fetch_one(&setup.pool)
            .await
            .expect("status line")
            .expect("a status line");
    assert!(
        status_line.starts_with("Stop asked: Wrong branch.") && status_line.chars().count() == 200,
        "{status_line}"
    );
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

#[tokio::test]
async fn a_retried_open_never_answers_with_a_run_from_elsewhere() {
    use agent_workspace_server::runs::{
        ops::{self, Cause, Reporter, RunOpError, RunOpen},
        state::RunState,
    };

    let Some(setup) = setup("a1_run_replay").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let home = create_project(&setup, "HOME", "Home").await;
    let (fixer_id, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    add_member(&setup, &home, fixer_id, "member").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let open = |project: &'static str| {
        json!({"projectId": project, "clientRunId": "nightly",
               "title": "Quiet merger talks", "cause": {"kind": "manual"}})
    };
    let opened = tool(app, &fixer_token, &fixer, "run_open", open("WORK")).await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();

    // The same clientRunId from the other project: refused, and nothing
    // about the first run comes back.
    let elsewhere = tool(app, &fixer_token, &fixer, "run_open", open("HOME")).await;
    assert_ne!(elsewhere["outcome"], "ok", "{elsewhere}");
    let said = elsewhere.to_string();
    assert!(
        !said.contains(&run_id) && !said.contains("Quiet merger"),
        "{said}"
    );

    // Gone from the first project, a replay there finds nothing either,
    // even below the tool's own check.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(work.parse::<Uuid>().expect("project"))
    .bind(fixer_id)
    .execute(&setup.pool)
    .await
    .expect("leave the project");
    let mut transaction = setup.pool.begin().await.expect("transaction");
    let replay = ops::open(
        &mut transaction,
        Reporter::Account {
            account_id: fixer_id,
        },
        RunOpen {
            space_id: work.parse().expect("project"),
            client_run_id: Some("nightly".to_owned()),
            title: "Anything".to_owned(),
            cause: Cause {
                kind: "manual".to_owned(),
                conversation_id: None,
                work_item_id: None,
                message_id: None,
                reference: None,
            },
            parent_run_id: None,
            report_interval_seconds: None,
            settings: None,
            initial: RunState::Working,
            requested_by: None,
            via_app_id: None,
            launch_id: None,
        },
    )
    .await;
    assert!(
        matches!(replay, Err(RunOpError::NotFound)),
        "a replay where the account no longer belongs finds nothing"
    );
}

#[tokio::test]
async fn a_replay_rechecks_the_conversation_its_run_is_in() {
    let Some(setup) = setup("a1_run_replay_place").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (_fixer, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let fixer = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let owner_app = connect_app(app, &setup.browser, "Laptop", "full").await;
    let owner = initialize(app, &owner_app).await.expect("app MCP");
    let channel = tool(
        app,
        &owner_app,
        &owner,
        "channel_create",
        json!({"projectId": "WORK", "name": "deals"}),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let channel_id = channel["value"]["id"].as_str().expect("channel").to_owned();
    let call = |name: &'static str, arguments: Value| {
        let token = fixer_token.clone();
        let session = fixer.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    let opened = call(
        "run_open",
        json!({"projectId": "WORK", "title": "Deal notes", "cause": {"kind": "thread", "conversationId": channel_id}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    let update = json!({"runId": run_id, "statusLine": "Reading the merger terms",
                        "clientRequestId": "update-1"});
    let updated = call("run_update", update.clone()).await;
    assert_eq!(updated["outcome"], "ok", "{updated}");

    // The channel turns members-only without the agent: a replay of the
    // same update, and any other report, finds nothing.
    sqlx::query("UPDATE conversations SET visibility = 'members' WHERE id = $1")
        .bind(channel_id.parse::<Uuid>().expect("channel UUID"))
        .execute(&setup.pool)
        .await
        .expect("make the channel private");
    let replayed = call("run_update", update).await;
    assert_eq!(replayed["outcome"], "not_found", "{replayed}");
    assert!(!replayed.to_string().contains("merger"), "{replayed}");
    let step = call(
        "run_activity",
        json!({"runId": run_id, "activities": [{"id": "s1", "kind": "step_started",
            "payload": {"title": "More"}}]}),
    )
    .await;
    assert_eq!(step["outcome"], "not_found", "{step}");
}
