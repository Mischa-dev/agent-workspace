//! A1.4: connected runners get runs. The silent failures: a launch to a
//! runner that leaves no run (or changes the hand-off people see), a runner
//! that only uses the reply tools showing a run that never moves, an old
//! runner's run looking fine when it never picked the work up, and a Stop
//! that never reaches the run.

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
};

async fn run_row(
    pool: &sqlx::PgPool,
    run: Uuid,
) -> (String, String, Option<String>, Option<String>) {
    sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        "SELECT state, liveness, status_line, external_url FROM runs WHERE id = $1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("run")
}

fn uuid_at(value: &Value, pointer: &str) -> Uuid {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{pointer} in {value}"))
        .parse()
        .expect("UUID")
}

#[tokio::test]
async fn a_launch_to_a_runner_gives_it_a_run_its_reply_calls_move() {
    let Some(setup) = setup("a1_launch_paths").await else {
        return;
    };
    let app = &setup.app;
    let pool = &setup.pool;
    let work = create_project(&setup, "WORK", "Work").await;
    let (runner, runner_token) = create_agent(app, &setup.browser, &work, "Runner", "runner").await;
    let runner_session = initialize(app, &runner_token).await.expect("Runner MCP");
    let (_planner, planner_token) =
        create_agent(app, &setup.browser, &work, "Planner", "planner").await;
    let planner_session = initialize(app, &planner_token).await.expect("Planner MCP");
    let runner_call = |name: &'static str, arguments: Value| {
        let token = runner_token.clone();
        let session = runner_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };

    // The runner's harness, registered by the owner.
    let (status, registered) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/harnesses", setup.organization_id),
            Some("register-runner"),
            json!({
                "name": "Night runner",
                "transport": "runner",
                "runnerActorId": runner,
                "manifest": {"description": "Runs at night", "models": [{"id": "night-model", "displayName": "Night"}]}
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {registered}");
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM custom_harnesses WHERE name = $1")
        .bind("Night runner")
        .fetch_one(pool)
        .await
        .expect("harness kind");

    let owner_app = connect_app(app, &setup.browser, "Laptop", "full").await;
    let owner_session = initialize(app, &owner_app).await.expect("app MCP");
    let owner_call = |name: &'static str, arguments: Value| {
        let token = owner_app.clone();
        let session = owner_session.clone();
        async move { tool(app, &token, &session, name, arguments).await }
    };
    // A task launches in a thread of a channel its launcher can see.
    let channel = owner_call(
        "channel_create",
        json!({"projectId": "WORK", "name": "general"}),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let task = |title: &'static str, key: &'static str| {
        let call = owner_call;
        async move {
            let made = call(
                "task_create",
                json!({"projectId": "WORK", "title": title, "clientRequestId": key}),
            )
            .await;
            assert_eq!(made["outcome"], "applied", "{made}");
            made["key"].as_str().expect("key").to_owned()
        }
    };
    let launch = |key: String, client: &'static str| {
        let call = owner_call;
        let kind = kind.clone();
        async move {
            let launched = call(
                "task_launch",
                json!({"taskIds": [key], "harness": kind, "model": "night-model",
                       "prompt": "Tidy the changelog for the release.", "clientRequestId": client}),
            )
            .await;
            assert_eq!(launched["outcome"], "ok", "{launched}");
            launched["value"]["launches"][0].clone()
        }
    };

    // A launch to the runner hands off as before, and opens the runner's run.
    let first = launch(task("Tidy the changelog", "t1").await, "l1").await;
    let run = uuid_at(&first, "/runId");
    assert_eq!(first["runState"], "accepted", "{first}");
    let launch_id = uuid_at(&first, "/id");
    let (launch_state, launch_run, message) = sqlx::query_as::<_, (String, Option<Uuid>, Uuid)>(
        "SELECT state, run_id, handoff_message_id FROM launches WHERE id = $1",
    )
    .bind(launch_id)
    .fetch_one(pool)
    .await
    .expect("launch");
    assert_eq!(launch_state, "handed_off");
    assert_eq!(launch_run, Some(run));
    let content = sqlx::query_scalar::<_, Value>("SELECT content FROM messages WHERE id = $1")
        .bind(message)
        .fetch_one(pool)
        .await
        .expect("hand-off message");
    assert_eq!(
        content["nodes"],
        json!([
            {"type": "mention", "actorId": runner},
            {"type": "text", "text": " Tidy the changelog for the release.\n\nSettings: model=night-model"}
        ]),
        "the hand-off reads as before"
    );
    let (account, cause, cause_message, run_launch) =
        sqlx::query_as::<_, (Uuid, String, Option<Uuid>, Option<Uuid>)>(
            "SELECT account_id, cause_kind, cause_message_id, launch_id FROM runs WHERE id = $1",
        )
        .bind(run)
        .fetch_one(pool)
        .await
        .expect("run");
    assert_eq!(
        (account, cause.as_str(), cause_message, run_launch),
        (runner, "task", Some(message), Some(launch_id))
    );

    // The runner only uses the reply tools: claim, progress, session, answer.
    let inbox = runner_call("inbox_list", json!({})).await;
    let request = inbox["value"]["requests"]
        .as_array()
        .expect("requests")
        .iter()
        .find(|request| request["messageId"] == message.to_string().as_str())
        .unwrap_or_else(|| panic!("the hand-off reached the runner: {inbox}"))
        .clone();
    let request_id = request["requestId"].as_str().expect("request").to_owned();
    let claimed = runner_call("reply_claim", json!({"requestId": request_id})).await;
    assert_eq!(claimed["outcome"], "ok", "{claimed}");
    assert_eq!(claimed["value"]["run"]["runId"], run.to_string().as_str());
    assert_eq!(claimed["value"]["run"]["state"], "working");
    let generation = claimed["value"]["request"]["generation"].clone();
    let answer_in = claimed["value"]["request"]["replyConversationId"]
        .as_str()
        .expect("where to answer")
        .to_owned();
    for status in [
        "Reading the changelog",
        "Grouping entries",
        "Grouping entries",
    ] {
        let progress = runner_call(
            "reply_progress",
            json!({"requestId": request_id, "generation": generation, "status": status}),
        )
        .await;
        assert_eq!(progress["outcome"], "ok", "{progress}");
    }
    let steps = sqlx::query_scalar::<_, String>(
        "SELECT summary FROM run_activities WHERE run_id = $1 AND kind = 'step_started' ORDER BY seq",
    )
    .bind(run)
    .fetch_all(pool)
    .await
    .expect("steps");
    assert_eq!(
        steps,
        ["Reading the changelog", "Grouping entries"],
        "one step per line"
    );
    let bound = runner_call(
        "session_bind",
        json!({"conversationId": answer_in, "externalRef": "night-1",
               "url": "https://runner.example.com/sessions/night-1"}),
    )
    .await;
    assert_eq!(
        bound["outcome"], "ok",
        "session_bind without a harness: {bound}"
    );
    let (state, _, status_line, url) = run_row(pool, run).await;
    assert_eq!(state, "working");
    assert_eq!(status_line.as_deref(), Some("Grouping entries"));
    assert_eq!(
        url.as_deref(),
        Some("https://runner.example.com/sessions/night-1")
    );
    let answered = runner_call(
        "reply_send",
        json!({"requestId": request_id, "generation": generation, "text": "Tidied: 12 entries."}),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    assert_eq!(
        run_row(pool, run).await.0,
        "done",
        "the final reply finishes the run"
    );

    // A runner that never claims (an old SDK): its run goes quiet, then
    // shows it isn't responding.
    let silent = launch(task("Rename the flags", "t2").await, "l2").await;
    let silent_run = uuid_at(&silent, "/runId");
    sqlx::query("UPDATE runs SET last_report_at = now() - interval '2 hours' WHERE id = $1")
        .bind(silent_run)
        .execute(pool)
        .await
        .expect("age the run");
    agent_workspace_server::runs::liveness::sweep(pool)
        .await
        .expect("sweep");
    let (state, liveness, _, _) = run_row(pool, silent_run).await;
    assert_eq!(
        (state.as_str(), liveness.as_str()),
        ("accepted", "not_responding")
    );

    // Stop on the request asks the run to stop; release fails one.
    let stopped = launch(task("Sweep the backlog", "t3").await, "l3").await;
    let stopped_run = uuid_at(&stopped, "/runId");
    let stopped_message =
        sqlx::query_scalar::<_, Uuid>("SELECT handoff_message_id FROM launches WHERE id = $1")
            .bind(uuid_at(&stopped, "/id"))
            .fetch_one(pool)
            .await
            .expect("launch");
    let stop_request = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM reply_requests WHERE message_id = $1 AND agent_actor_id = $2",
    )
    .bind(stopped_message)
    .bind(runner)
    .fetch_one(pool)
    .await
    .expect("request");
    let claimed = runner_call("reply_claim", json!({"requestId": stop_request})).await;
    assert_eq!(claimed["value"]["run"]["state"], "working", "{claimed}");
    let stop = owner_call("reply_stop", json!({"requestId": stop_request})).await;
    assert_eq!(stop["outcome"], "ok", "{stop}");
    assert_eq!(run_row(pool, stopped_run).await.0, "cancel_requested");
    // A runner that only uses the reply tools never confirms: the stop
    // completes on the server.
    agent_workspace_server::runs::liveness::settle(pool)
        .await
        .expect("settle");
    assert_eq!(run_row(pool, stopped_run).await.0, "cancelled");

    // A stop retried and claimed again before it settles: the claim gets a
    // new attempt, and settling never ends that attempt.
    let restarted = launch(task("Sort the inbox", "t7").await, "l7").await;
    let restarted_run = uuid_at(&restarted, "/runId");
    let restarted_request = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id FROM reply_requests request
        JOIN launches launch ON launch.handoff_message_id = request.message_id
        WHERE launch.id = $1 AND request.agent_actor_id = $2
        "#,
    )
    .bind(uuid_at(&restarted, "/id"))
    .bind(runner)
    .fetch_one(pool)
    .await
    .expect("request");
    let claimed = runner_call("reply_claim", json!({"requestId": restarted_request})).await;
    assert_eq!(claimed["value"]["run"]["state"], "working", "{claimed}");
    let stop = owner_call("reply_stop", json!({"requestId": restarted_request})).await;
    assert_eq!(stop["outcome"], "ok", "{stop}");
    assert_eq!(run_row(pool, restarted_run).await.0, "cancel_requested");
    let retried = owner_call("reply_retry", json!({"requestId": restarted_request})).await;
    assert_eq!(retried["outcome"], "ok", "{retried}");
    let again = runner_call("reply_claim", json!({"requestId": restarted_request})).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let fresh = again["value"]["run"]["runId"]
        .as_str()
        .expect("a new attempt")
        .parse::<Uuid>()
        .expect("run UUID");
    assert_ne!(fresh, restarted_run, "{again}");
    assert_eq!(run_row(pool, restarted_run).await.0, "cancelled");
    agent_workspace_server::runs::liveness::settle(pool)
        .await
        .expect("settle");
    assert_eq!(run_row(pool, fresh).await.0, "working");
    let still = sqlx::query_scalar::<_, String>("SELECT status FROM reply_requests WHERE id = $1")
        .bind(restarted_request)
        .fetch_one(pool)
        .await
        .expect("request");
    assert_eq!(still, "claimed");

    let released = launch(task("Write the notes", "t4").await, "l4").await;
    let released_run = uuid_at(&released, "/runId");
    let released_request = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id FROM reply_requests request
        JOIN launches launch ON launch.handoff_message_id = request.message_id
        WHERE launch.id = $1 AND request.agent_actor_id = $2
        "#,
    )
    .bind(uuid_at(&released, "/id"))
    .bind(runner)
    .fetch_one(pool)
    .await
    .expect("request");
    let claimed = runner_call("reply_claim", json!({"requestId": released_request})).await;
    let release = runner_call(
        "reply_release",
        json!({"requestId": released_request, "generation": claimed["value"]["request"]["generation"],
               "reason": "The notes need a decision first."}),
    )
    .await;
    assert_eq!(release["outcome"], "ok", "{release}");
    let (state, failure) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, failure_reason FROM runs WHERE id = $1",
    )
    .bind(released_run)
    .fetch_one(pool)
    .await
    .expect("run");
    assert_eq!(state, "failed");
    assert_eq!(failure.as_deref(), Some("The notes need a decision first."));

    // Retry: the next claim is a new attempt that can still succeed.
    let retried = owner_call("reply_retry", json!({"requestId": released_request})).await;
    assert_eq!(retried["outcome"], "ok", "{retried}");
    let again = runner_call("reply_claim", json!({"requestId": released_request})).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let attempt = again["value"]["run"]["runId"]
        .as_str()
        .expect("a new attempt")
        .parse::<Uuid>()
        .expect("run UUID");
    assert_ne!(attempt, released_run, "{again}");
    assert_eq!(again["value"]["run"]["state"], "working");
    let (predecessor, shown) = sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>)>(
        "SELECT run.predecessor_run_id, (SELECT run_id FROM launches WHERE id = $2) FROM runs run WHERE run.id = $1",
    )
    .bind(attempt)
    .bind(uuid_at(&released, "/id"))
    .fetch_one(pool)
    .await
    .expect("attempt");
    assert_eq!((predecessor, shown), (Some(released_run), Some(attempt)));
    // Stopping the old, finished attempt leaves the retry's request alone.
    let _ = owner_call(
        "run_cancel",
        json!({"runId": released_run, "clientRequestId": "stop-old-attempt"}),
    )
    .await;
    let still = sqlx::query_scalar::<_, String>("SELECT status FROM reply_requests WHERE id = $1")
        .bind(released_request)
        .fetch_one(pool)
        .await
        .expect("request");
    assert_eq!(
        still, "claimed",
        "the old attempt's stop never reaches the retry"
    );
    assert_eq!(run_row(pool, attempt).await.0, "working");
    let answered = runner_call(
        "reply_send",
        json!({"requestId": released_request, "generation": again["value"]["request"]["generation"],
               "text": "Notes written."}),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    assert_eq!(run_row(pool, attempt).await.0, "done");
    assert_eq!(run_row(pool, released_run).await.0, "failed");

    // A stop nobody confirms ends the run after the grace period.
    let unconfirmed = owner_call(
        "run_cancel",
        json!({"runId": silent_run, "clientRequestId": "stop-silent"}),
    )
    .await;
    assert_eq!(unconfirmed["outcome"], "ok", "{unconfirmed}");
    assert_eq!(run_row(pool, silent_run).await.0, "cancel_requested");
    sqlx::query(
        "UPDATE runs SET cancel_requested_at = now() - interval '11 minutes' WHERE id = $1",
    )
    .bind(silent_run)
    .execute(pool)
    .await
    .expect("age the stop");
    agent_workspace_server::runs::liveness::settle(pool)
        .await
        .expect("settle");
    assert_eq!(run_row(pool, silent_run).await.0, "cancelled");

    // Stopping a claimed launched run from the run itself goes through its
    // request: the claim is revoked and the runner hears reply.cancelled,
    // which today's SDKs stop on.
    let halted = launch(task("Rebuild the index", "t6").await, "l6").await;
    let halted_run = uuid_at(&halted, "/runId");
    let halted_request = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id FROM reply_requests request
        JOIN launches launch ON launch.handoff_message_id = request.message_id
        WHERE launch.id = $1 AND request.agent_actor_id = $2
        "#,
    )
    .bind(uuid_at(&halted, "/id"))
    .bind(runner)
    .fetch_one(pool)
    .await
    .expect("request");
    let claimed = runner_call("reply_claim", json!({"requestId": halted_request})).await;
    assert_eq!(claimed["value"]["run"]["state"], "working", "{claimed}");
    let halt = owner_call(
        "run_cancel",
        json!({"runId": halted_run, "reason": "Wrong index", "clientRequestId": "stop-6"}),
    )
    .await;
    assert_eq!(halt["outcome"], "ok", "{halt}");
    assert_eq!(halt["value"]["run"]["state"], "cancel_requested", "{halt}");
    let (request_status, heard) = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT (SELECT status FROM reply_requests WHERE id = $1),
               (SELECT count(*) FROM external_agent_events
                WHERE kind = 'reply.cancelled' AND resource_id = $1)
        "#,
    )
    .bind(halted_request)
    .fetch_one(pool)
    .await
    .expect("request");
    assert_eq!(request_status, "cancelled");
    assert!(heard >= 1, "the runner hears reply.cancelled");
    let late = runner_call(
        "reply_send",
        json!({"requestId": halted_request, "generation": claimed["value"]["request"]["generation"],
               "text": "Done anyway."}),
    )
    .await;
    assert_ne!(late["outcome"], "ok", "the claim is gone: {late}");
    agent_workspace_server::runs::liveness::settle(pool)
        .await
        .expect("settle");
    assert_eq!(run_row(pool, halted_run).await.0, "cancelled");

    // A lease that ran out fails the run, with the reason.
    let lapsed = launch(task("Check the links", "t5").await, "l5").await;
    let lapsed_run = uuid_at(&lapsed, "/runId");
    let lapsed_request = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id FROM reply_requests request
        JOIN launches launch ON launch.handoff_message_id = request.message_id
        WHERE launch.id = $1 AND request.agent_actor_id = $2
        "#,
    )
    .bind(uuid_at(&lapsed, "/id"))
    .bind(runner)
    .fetch_one(pool)
    .await
    .expect("request");
    let claimed = runner_call("reply_claim", json!({"requestId": lapsed_request})).await;
    assert_eq!(claimed["value"]["run"]["state"], "working", "{claimed}");
    sqlx::query(
        "UPDATE reply_requests SET status = 'expired', lease_expires_at = NULL, reason = 'The agent stopped checking in before replying.' WHERE id = $1",
    )
    .bind(lapsed_request)
    .execute(pool)
    .await
    .expect("lapse the lease");
    agent_workspace_server::runs::liveness::settle(pool)
        .await
        .expect("settle");
    let (state, failure) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, failure_reason FROM runs WHERE id = $1",
    )
    .bind(lapsed_run)
    .fetch_one(pool)
    .await
    .expect("run");
    assert_eq!(
        (state.as_str(), failure.as_deref()),
        (
            "failed",
            Some("The agent stopped checking in before replying.")
        )
    );

    // Nobody else's reply calls touch the runner's run.
    let foreign = tool(
        app,
        &planner_token,
        &planner_session,
        "reply_claim",
        json!({"requestId": request_id}),
    )
    .await;
    assert_ne!(foreign["outcome"], "ok", "{foreign}");
    assert_eq!(run_row(pool, run).await.0, "done");
}
