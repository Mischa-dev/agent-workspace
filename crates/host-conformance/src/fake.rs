//! A stand-in workspace that serves the host's side of the protocol, and
//! the script that drives a host through it.

use std::{collections::HashSet, sync::Arc, time::Duration};

use agent_workspace_host_protocol::{PROTOCOL_HEADER, PROTOCOL_VERSION};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::Report;

const HOST_TOKEN: &str = "conformance-host-token";
/// How long a host has to answer each step.
const STEP: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
struct FakeCommand {
    id: Uuid,
    seq: i64,
    kind: &'static str,
    run_id: Option<Uuid>,
    request_id: Option<Uuid>,
    start_key: Option<String>,
    body: Map<String, Value>,
    offer: i32,
    acked: bool,
}

/// What the host did, as the stand-in saw it.
#[derive(Debug, Default)]
struct Seen {
    enrolled: Option<Value>,
    descriptions: Vec<Value>,
    heartbeats: usize,
    notices: Vec<Value>,
    commands: Vec<FakeCommand>,
    next_seq: i64,
    /// Acknowledgements taken, by command.
    acks: Vec<(Uuid, Value)>,
    /// Commands whose next acknowledgement is answered 503 and lost.
    lose_ack: HashSet<Uuid>,
    /// Run reports: (run, operation, idempotency key, body).
    reports: Vec<(Uuid, String, String, Value)>,
    /// Calls that broke the protocol, said in a sentence.
    faults: Vec<String>,
}

struct Shared {
    code: String,
    seen: Mutex<Seen>,
    changed: Notify,
}

/// A workspace stand-in on a local port. Give a host its `base` and `code`.
pub struct FakeWorkspace {
    pub base: String,
    pub code: String,
    shared: Arc<Shared>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for FakeWorkspace {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl FakeWorkspace {
    /// Serve on `address` (`127.0.0.1:0` picks a free port).
    pub async fn start(address: &str) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(address).await?;
        let base = format!("http://{}", listener.local_addr()?);
        let code = format!("conformance-{}", Uuid::now_v7().simple());
        let shared = Arc::new(Shared {
            code: code.clone(),
            seen: Mutex::new(Seen::default()),
            changed: Notify::new(),
        });
        let app = Router::new()
            .route("/api/v1/hosts/enrol", post(enrol))
            .route("/api/v1/host/describe", post(describe))
            .route("/api/v1/host/heartbeat", post(heartbeat))
            .route("/api/v1/host/notices", post(notice))
            .route("/api/v1/host/commands", get(commands))
            .route("/api/v1/host/commands/{id}", get(command))
            .route("/api/v1/host/commands/{id}/ack", post(ack))
            .route("/api/v2/ops/{name}", post(report))
            .with_state(shared.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            base,
            code,
            shared,
            server,
        })
    }

    async fn offer(
        &self,
        kind: &'static str,
        run_id: Option<Uuid>,
        request_id: Option<Uuid>,
        start_key: Option<String>,
        body: Value,
    ) -> Uuid {
        let mut seen = self.shared.seen.lock().await;
        seen.next_seq += 1;
        let id = Uuid::now_v7();
        let command = FakeCommand {
            id,
            seq: seen.next_seq,
            kind,
            run_id,
            request_id,
            start_key,
            body: match body {
                Value::Object(body) => body,
                _ => Map::new(),
            },
            offer: 1,
            acked: false,
        };
        seen.commands.push(command);
        drop(seen);
        self.shared.changed.notify_waiters();
        id
    }

    async fn start_run(&self, agent: &str, prompt: &str, lose_first_ack: bool) -> (Uuid, Uuid) {
        let run_id = Uuid::now_v7();
        let body = json!({"agent": {"accountId": Uuid::now_v7(), "hostAgentRef": agent},
                          "input": {"prompt": prompt}, "title": prompt,
                          "cause": {"kind": "manual"}});
        let command = self
            .offer(
                "start",
                Some(run_id),
                None,
                Some(format!("start-{run_id}")),
                body,
            )
            .await;
        if lose_first_ack {
            self.shared.seen.lock().await.lose_ack.insert(command);
        }
        (run_id, command)
    }

    /// Wait until `test` holds, up to `limit`.
    async fn until<T>(&self, limit: Duration, test: impl Fn(&Seen) -> Option<T>) -> Option<T> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if let Some(found) = test(&*self.shared.seen.lock().await) {
                return Some(found);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn acked(&self, command: Uuid, count: usize) -> Option<Vec<Value>> {
        self.until(STEP, |seen| {
            let acks = seen
                .acks
                .iter()
                .filter(|(id, _)| *id == command)
                .map(|(_, ack)| ack.clone())
                .collect::<Vec<_>>();
            (acks.len() >= count).then_some(acks)
        })
        .await
    }
}

/// Drive the host that was given `fake.base` and `fake.code` through the
/// protocol, and say what it lacks.
pub async fn check_host(fake: &FakeWorkspace) -> Report {
    let mut report = Report::default();
    let enrolled = fake.until(STEP, |seen| seen.enrolled.clone()).await;
    let offered = enrolled.as_ref().is_some_and(|body| {
        body["protocolVersions"]
            .as_array()
            .is_some_and(|versions| versions.iter().any(|v| v == PROTOCOL_VERSION))
    });
    if !report.record(
        "enrols with a one-time code",
        enrolled.is_some() && offered,
        if enrolled.is_none() {
            "no enrolment arrived".to_owned()
        } else if offered {
            String::new()
        } else {
            format!("it didn't offer protocol {PROTOCOL_VERSION}")
        },
    ) {
        return report;
    }
    let description = fake
        .until(STEP, |seen| seen.descriptions.last().cloned())
        .await
        .unwrap_or(Value::Null);
    let agents = description["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let agent = agents
        .first()
        .and_then(|agent| agent["hostAgentRef"].as_str())
        .map(str::to_owned);
    if !report.record(
        "describes itself and at least one agent",
        agent.is_some(),
        if description.is_null() {
            "no description arrived"
        } else {
            ""
        },
    ) {
        return report;
    }
    let agent = agent.unwrap_or_default();
    let beat = fake
        .until(STEP, |seen| (seen.heartbeats > 0).then_some(()))
        .await;
    report.record("sends heartbeats", beat.is_some(), "");

    // A start, acknowledged with the host's own reference, then reported
    // on with the credential it carried.
    let (first_run, first) = fake.start_run(&agent, "Say hello", false).await;
    let acks = fake.acked(first, 1).await.unwrap_or_default();
    let reference = acks
        .first()
        .and_then(|ack| ack["hostRunRef"].as_str().map(str::to_owned));
    report.record(
        "starts a run and acknowledges it with its own reference",
        acks.first().is_some_and(|ack| ack["outcome"] == "accepted") && reference.is_some(),
        acks.first()
            .map(Value::to_string)
            .unwrap_or_else(|| "no acknowledgement".to_owned()),
    );
    let reported = fake
        .until(STEP, |seen| {
            seen.reports
                .iter()
                .any(|(run, ..)| *run == first_run)
                .then_some(())
        })
        .await;
    report.record(
        "reports on a run with the credential its start carried",
        reported.is_some(),
        "",
    );

    // The acknowledgement of a start is lost: the start is offered again
    // and must be answered with the same run, never started twice.
    let (second_run, second) = fake.start_run(&agent, "Say hello and wait", true).await;
    let acks = fake.acked(second, 2).await.unwrap_or_default();
    let references = acks
        .iter()
        .filter_map(|ack| ack["hostRunRef"].as_str())
        .collect::<HashSet<_>>();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let starts = fake
        .shared
        .seen
        .lock()
        .await
        .reports
        .iter()
        .filter(|(run, operation, _, body)| {
            *run == second_run
                && operation == "run_activity_report"
                && body["activities"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["kind"] == "run_started"))
        })
        .map(|(_, _, key, _)| key.clone())
        .collect::<HashSet<_>>();
    let idempotent = acks.len() >= 2
        && acks.iter().all(|ack| ack["outcome"] == "accepted")
        && references.len() == 1
        && starts.len() <= 1;
    report.record(
        "a start offered again is not started twice",
        idempotent,
        if idempotent {
            String::new()
        } else if acks.len() < 2 {
            "start not idempotent: the re-offered start was not acknowledged".to_owned()
        } else {
            format!(
                "start not idempotent: {} reference(s) and {} start report(s) for one run",
                references.len(),
                starts.len()
            )
        },
    );

    let cancel = fake
        .offer(
            "cancel",
            Some(second_run),
            None,
            None,
            json!({"reason": "conformance"}),
        )
        .await;
    let answered = fake.acked(cancel, 1).await;
    report.record("acknowledges a cancel", answered.is_some(), "");

    let steer = fake
        .offer(
            "steer",
            Some(first_run),
            None,
            None,
            json!({"hostRunRef": reference, "messageId": Uuid::now_v7()}),
        )
        .await;
    report.record(
        "acknowledges a steer",
        fake.acked(steer, 1).await.is_some(),
        "",
    );

    let request = Uuid::now_v7();
    let answer = fake
        .offer("answer", Some(first_run), Some(request), None,
               json!({"answer": {"status": "answered", "answer": {"optionId": "yes", "label": "Yes"}}}))
        .await;
    report.record(
        "acknowledges an answer",
        fake.acked(answer, 1).await.is_some(),
        "",
    );

    let inspect = fake
        .offer(
            "inspect",
            Some(first_run),
            None,
            None,
            json!({"hostRunRef": reference}),
        )
        .await;
    let inspected = fake.acked(inspect, 1).await.unwrap_or_default();
    report.record(
        "answers an inspect with whether it knows the run",
        inspected
            .first()
            .is_some_and(|ack| ack["outcome"] == "done" && ack["result"]["known"].is_boolean()),
        inspected.first().map(Value::to_string).unwrap_or_default(),
    );

    match description["actions"]
        .as_array()
        .and_then(|actions| actions.first())
    {
        Some(action) => {
            let id = action["id"].as_str().unwrap_or_default();
            let command = fake
                .offer(
                    "action",
                    None,
                    None,
                    None,
                    json!({"action": id, "input": {}}),
                )
                .await;
            let acks = fake.acked(command, 1).await.unwrap_or_default();
            report.record(
                "carries out a host action",
                acks.first().is_some_and(|ack| ack["outcome"] == "done"),
                acks.first()
                    .map(Value::to_string)
                    .unwrap_or_else(|| "no acknowledgement".to_owned()),
            );
        }
        None => report.skip("carries out a host action", "it describes no host action"),
    }

    // Its agent's actions, in the order described; a host that has one per
    // notice sends each.
    let agent_actions = agents
        .first()
        .and_then(|agent| agent["actions"].as_array().cloned())
        .unwrap_or_default();
    for action in &agent_actions {
        let id = action["id"].as_str().unwrap_or_default();
        let command = fake
            .offer(
                "action",
                None,
                None,
                None,
                json!({"hostAgentRef": agent, "action": id, "input": {}}),
            )
            .await;
        let _ = fake.acked(command, 1).await;
    }
    let kinds = fake
        .shared
        .seen
        .lock()
        .await
        .notices
        .iter()
        .filter_map(|notice| notice["kind"].as_str().map(str::to_owned))
        .collect::<HashSet<_>>();
    for (kind, name) in [
        ("agent_profile_changed", "sends agent_profile_changed"),
        ("agent_ref_changed", "sends agent_ref_changed"),
        ("agent_retired", "sends agent_retired"),
    ] {
        if kinds.contains(kind) {
            report.record(name, true, "");
        } else {
            report.skip(name, "not sent (a host sends it when that happens)");
        }
    }

    let faults = fake.shared.seen.lock().await.faults.clone();
    report.record(
        "keeps to the protocol on every call",
        faults.is_empty(),
        faults.join("; "),
    );
    report
}

// ------------------------------------------------------------ the routes

fn refuse(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": {"code": code}}))).into_response()
}

/// A host call: the protocol header and the host's token.
async fn host_call(shared: &Shared, headers: &HeaderMap, what: &str) -> Result<(), Response> {
    if headers.get(PROTOCOL_HEADER).and_then(|v| v.to_str().ok()) != Some(PROTOCOL_VERSION) {
        shared
            .seen
            .lock()
            .await
            .faults
            .push(format!("{what} without the {PROTOCOL_HEADER} header"));
        return Err(refuse(StatusCode::BAD_REQUEST, "protocol_version"));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if token != Some(HOST_TOKEN) {
        shared
            .seen
            .lock()
            .await
            .faults
            .push(format!("{what} without the host's token"));
        return Err(refuse(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(())
}

async fn enrol(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Response {
    if body["code"] != shared.code.as_str() {
        return refuse(StatusCode::UNAUTHORIZED, "code_invalid");
    }
    let mut seen = shared.seen.lock().await;
    if seen.enrolled.is_some() {
        return refuse(StatusCode::CONFLICT, "code_used");
    }
    seen.enrolled = Some(body);
    Json(
        json!({"hostId": Uuid::now_v7(), "owner": {"kind": "workspace"},
                "protocol": PROTOCOL_VERSION,
                "credential": {"kind": "bearer", "token": HOST_TOKEN}}),
    )
    .into_response()
}

async fn describe(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "describe").await {
        return refused;
    }
    shared.seen.lock().await.descriptions.push(body);
    Json(
        json!({"hostId": Uuid::nil(), "protocol": PROTOCOL_VERSION, "deprecated": false,
                "unboundAgents": []}),
    )
    .into_response()
}

async fn heartbeat(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "a heartbeat").await {
        return refused;
    }
    shared.seen.lock().await.heartbeats += 1;
    Json(
        json!({"protocol": PROTOCOL_VERSION, "deprecated": false, "describeWanted": false,
                "runsRenewed": 0}),
    )
    .into_response()
}

async fn notice(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "a notice").await {
        return refused;
    }
    shared.seen.lock().await.notices.push(body);
    Json(json!({"noticeId": Uuid::now_v7(), "outcome": "applied"})).into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    after: i64,
    #[serde(default)]
    wait: u64,
}

async fn commands(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "reading commands").await {
        return refused;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(query.wait.min(30));
    loop {
        let changed = shared.changed.notified();
        {
            let seen = shared.seen.lock().await;
            let pending = seen
                .commands
                .iter()
                .filter(|command| command.seq > query.after && !command.acked)
                .map(|command| {
                    json!({"commandId": command.id, "seq": command.seq, "kind": command.kind,
                           "runId": command.run_id, "offer": command.offer})
                })
                .collect::<Vec<_>>();
            if !pending.is_empty() || tokio::time::Instant::now() >= deadline {
                let next = seen.next_seq.max(query.after);
                return Json(json!({"commands": pending, "next": next})).into_response();
            }
        }
        let _ = tokio::time::timeout_at(deadline, changed).await;
    }
}

async fn command(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "fetching a command").await {
        return refused;
    }
    let seen = shared.seen.lock().await;
    let Some(command) = seen.commands.iter().find(|command| command.id == id) else {
        return refuse(StatusCode::NOT_FOUND, "not_found");
    };
    let mut body = command.body.clone();
    body.insert("commandId".into(), json!(command.id));
    body.insert("seq".into(), json!(command.seq));
    body.insert("kind".into(), json!(command.kind));
    body.insert("issuedAt".into(), json!("2026-11-17T00:00:00Z"));
    body.insert("expiresAt".into(), json!("2099-01-01T00:00:00Z"));
    if let Some(run_id) = command.run_id {
        body.insert("runId".into(), json!(run_id));
        if command.kind == "start" {
            body.insert(
                "workspaceCredential".into(),
                json!({"token": format!("run:{run_id}"), "expiresAt": "2099-01-01T00:00:00Z",
                       "scope": "run"}),
            );
        }
    }
    if let Some(request_id) = command.request_id {
        body.insert("requestId".into(), json!(request_id));
    }
    if let Some(start_key) = &command.start_key {
        body.insert("startKey".into(), json!(start_key));
    }
    Json(Value::Object(body)).into_response()
}

async fn ack(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(refused) = host_call(&shared, &headers, "an acknowledgement").await {
        return refused;
    }
    let mut seen = shared.seen.lock().await;
    let Some(index) = seen.commands.iter().position(|command| command.id == id) else {
        return refuse(StatusCode::NOT_FOUND, "not_found");
    };
    seen.acks.push((id, body.clone()));
    if seen.lose_ack.remove(&id) {
        // Lost on the way: offered again, with a new place in the list.
        seen.next_seq += 1;
        let seq = seen.next_seq;
        let command = &mut seen.commands[index];
        command.seq = seq;
        command.offer += 1;
        drop(seen);
        shared.changed.notify_waiters();
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    }
    seen.commands[index].acked = true;
    let state = match body["outcome"].as_str() {
        Some("refused") => "refused",
        Some("uncertain") => "uncertain",
        _ => "acked",
    };
    Json(json!({"commandId": id, "state": state})).into_response()
}

async fn report(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let run_id = body["runId"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok());
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut seen = shared.seen.lock().await;
    let Some(run_id) = run_id.filter(|run| token.as_deref() == Some(&format!("run:{run}"))) else {
        seen.faults
            .push(format!("{name} without its run's credential"));
        return refuse(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let Some(key) = key else {
        seen.faults
            .push(format!("{name} without an Idempotency-Key"));
        return refuse(StatusCode::BAD_REQUEST, "idempotency_key_required");
    };
    seen.reports.push((run_id, name, key, body));
    Json(json!({"outcome": "ok", "value": {"runId": run_id}})).into_response()
}
