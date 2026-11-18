//! A1.20: the A2A binding, against a recorded A2A agent. The silent
//! failures: a start sent twice when its answer was lost, a question the
//! agent asked that reaches nobody (or whose answer never reaches the
//! agent), an agent's result dropped, and an address a workspace chose
//! reaching this server's private network.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    AppState, config::AppConfig, hosts, provider::SupervisorRegistry, rate_limit::RateLimiters,
    realtime::RealtimeHub, router,
};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get, routing::post};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    a1_host_protocol::{dispatch, host_op, run_state},
    agent_interface::create_agent,
    support::world::{create_project, setup},
};

/// What the recorded agent heard, and where its one task is.
#[derive(Default)]
struct Agent {
    origin: String,
    sends: Vec<Value>,
    /// tasks/get calls answered so far.
    looks: usize,
    answer: Option<String>,
}

type Shared = Arc<Mutex<Agent>>;

fn task(state: &str, message: Option<&str>) -> Value {
    let mut task =
        json!({"kind": "task", "id": "t1", "contextId": "c1", "status": {"state": state}});
    if let Some(text) = message {
        task["status"]["message"] = json!({"kind": "message", "role": "agent",
            "messageId": format!("m-{state}"), "parts": [{"kind": "text", "text": text}]});
    }
    task
}

async fn card(State(agent): State<Shared>) -> Json<Value> {
    let origin = agent.lock().expect("agent").origin.clone();
    Json(json!({
        "name": "Recorded agent", "description": "Answers from a script.", "version": "1.0",
        "protocolVersion": "0.3.0", "url": format!("{origin}/rpc"), "preferredTransport": "JSONRPC",
        "capabilities": {"streaming": false},
        "skills": [{"id": "fix", "name": "Fix things", "description": "Fixes the build."}],
    }))
}

async fn rpc(State(agent): State<Shared>, Json(call): Json<Value>) -> (StatusCode, Json<Value>) {
    let id = call["id"].clone();
    let mut agent = agent.lock().expect("agent");
    let result = match call["method"].as_str() {
        Some("message/send") => {
            let message = call["params"]["message"].clone();
            agent.sends.push(message.clone());
            let text = message["parts"][0]["text"].as_str().unwrap_or_default();
            if message["taskId"] == "t1" {
                agent.answer = Some(text.to_owned());
                task("working", None)
            } else if text.contains("lose the answer") {
                // Took it, but the answer never arrives.
                return (StatusCode::BAD_GATEWAY, Json(json!({})));
            } else {
                task("submitted", None)
            }
        }
        Some("tasks/get") => {
            agent.looks += 1;
            match (agent.looks, agent.answer.as_deref()) {
                (1, _) => task("working", Some("Looking at the build.")),
                (_, None) => task("input-required", Some("Which branch?")),
                (_, Some(_)) => {
                    let mut done = task("completed", Some("Fixed it."));
                    done["artifacts"] = json!([{"artifactId": "a1", "name": "Report", "parts": [
                        {"kind": "text", "text": "All done on the branch you named."},
                        {"kind": "data", "data": {"passed": 3}}]}]);
                    done
                }
            }
        }
        Some("tasks/cancel") => task("canceled", None),
        _ => {
            return (
                StatusCode::OK,
                Json(json!({"jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "No such method."}})),
            );
        }
    };
    (
        StatusCode::OK,
        Json(json!({"jsonrpc": "2.0", "id": id, "result": result})),
    )
}

async fn recorded_agent() -> Shared {
    let agent: Shared = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    agent.lock().expect("agent").origin = origin;
    let app = Router::new()
        .route("/.well-known/agent-card.json", get(card))
        .route("/rpc", post(rpc))
        .with_state(agent.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    agent
}

/// The server, trusting the recorded agent's origin as an operator would
/// trust a private address.
fn trusting(pool: &sqlx::PgPool, origin: &str) -> AppState {
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://fixture/unused".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS".to_owned(),
            origin.to_owned(),
        ),
    ]))
    .expect("configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(32),
        supervisors,
    }
}

async fn command(pool: &sqlx::PgPool, run: Uuid, kind: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM host_commands WHERE run_id = $1 AND kind = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(run)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("command")
}

async fn look_now(state: &AppState) {
    sqlx::query("UPDATE host_a2a_tasks SET next_poll_at = now()")
        .execute(&state.pool)
        .await
        .expect("due");
    hosts::a2a::poll_due(state).await.expect("poll");
}

async fn said(pool: &sqlx::PgPool, run: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as::<_, (String, Value)>(
        "SELECT kind, payload FROM run_activities WHERE run_id = $1 ORDER BY seq",
    )
    .bind(run)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

#[tokio::test]
async fn an_a2a_agent_runs_a_task_asks_through_a_request_and_hands_back_its_result() {
    let Some(setup) = setup("a1_a2a").await else {
        return;
    };
    let agent = recorded_agent().await;
    let origin = agent.lock().expect("agent").origin.clone();
    let work = create_project(&setup, "WORK", "Work").await;
    let space = work.parse::<Uuid>().expect("space");
    let (fixer, _) = create_agent(&setup.app, &setup.browser, &work, "Fixer", "fixer").await;

    // A server that doesn't trust the address never reaches it.
    let (status, refused) = host_op(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "host_link",
        json!({"owner": "workspace", "cardUrl": origin}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{refused}");
    assert!(agent.lock().expect("agent").sends.is_empty());

    let state = trusting(&setup.pool, &origin);
    let app = router(state.clone());
    let (status, host) = host_op(
        &app,
        &setup.browser,
        setup.organization_id,
        "host_link",
        json!({"owner": "workspace", "cardUrl": origin, "accountId": fixer}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{host}");
    assert_eq!(host["name"], "Recorded agent");
    assert_eq!(host["connection"], "a2a");
    assert_eq!(host["describedAgents"][0]["hostAgentRef"], "agent");
    assert_eq!(host["agents"][0]["accountId"], fixer.to_string(), "{host}");

    // The start is one message/send, its id the start key.
    let run = dispatch(&setup.pool, space, fixer, "Fix the build").await;
    let start = command(&setup.pool, run, "start").await;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs WHERE kind = 'hosts.a2a_command' AND unique_key = $1",
    )
    .bind(format!("host-a2a:{start}"))
    .fetch_one(&setup.pool)
    .await
    .expect("job");
    assert_eq!(queued, 1, "the call waits on the job queue");
    hosts::a2a::send(&state, start).await.expect("send");
    hosts::a2a::send(&state, start).await.expect("send again");
    {
        let agent = agent.lock().expect("agent");
        assert_eq!(agent.sends.len(), 1, "a start is sent once");
        assert_eq!(agent.sends[0]["messageId"], format!("run-{run}"));
        assert_eq!(agent.sends[0]["parts"][0]["text"], "Fix the build");
    }
    assert_eq!(run_state(&setup.pool, run).await, "accepted");

    // working, with what it says.
    look_now(&state).await;
    assert_eq!(run_state(&setup.pool, run).await, "working");
    assert!(said(&setup.pool, run).await.iter().any(
        |(kind, payload)| kind == "text_message" && payload["text"] == "Looking at the build."
    ));

    // input-required: a question for a person, answered back to the task.
    look_now(&state).await;
    look_now(&state).await;
    assert_eq!(run_state(&setup.pool, run).await, "waiting");
    let (request, title): (Uuid, String) =
        sqlx::query_as("SELECT id, title FROM requests WHERE run_id = $1 AND status = 'open'")
            .bind(run)
            .fetch_one(&setup.pool)
            .await
            .expect("one open request, however often it was looked at");
    assert_eq!(title, "Which branch?");
    let (status, answered) = host_op(
        &app,
        &setup.browser,
        setup.organization_id,
        "request_respond",
        json!({"requestId": request, "text": "main"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let answer = command(&setup.pool, run, "answer").await;
    hosts::a2a::send(&state, answer)
        .await
        .expect("send the answer");
    {
        let agent = agent.lock().expect("agent");
        assert_eq!(agent.answer.as_deref(), Some("main"));
        assert_eq!(agent.sends[1]["taskId"], "t1");
    }

    // completed: its artifact is the agent's message and a result.
    look_now(&state).await;
    assert_eq!(run_state(&setup.pool, run).await, "done");
    let log = said(&setup.pool, run).await;
    assert!(log.iter().any(|(kind, payload)| kind == "text_message"
        && payload["text"] == "All done on the branch you named."));
    assert!(
        log.iter()
            .any(|(kind, payload)| kind == "result" && payload["data"]["passed"] == 3),
        "{log:?}"
    );
    let following: i64 = sqlx::query_scalar("SELECT count(*) FROM host_a2a_tasks")
        .fetch_one(&setup.pool)
        .await
        .expect("tasks");
    assert_eq!(following, 0, "a finished task is no longer looked at");

    // No answer to a start: start_uncertain, never sent again.
    let lost = dispatch(&setup.pool, space, fixer, "Please lose the answer").await;
    let start = command(&setup.pool, lost, "start").await;
    hosts::a2a::send(&state, start).await.expect("send");
    hosts::a2a::send(&state, start).await.expect("send again");
    assert_eq!(run_state(&setup.pool, lost).await, "start_uncertain");
    let tries = agent
        .lock()
        .expect("agent")
        .sends
        .iter()
        .filter(|sent| sent["messageId"] == format!("run-{lost}"))
        .count();
    assert_eq!(tries, 1);
}
