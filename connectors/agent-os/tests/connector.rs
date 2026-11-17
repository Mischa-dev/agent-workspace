//! The connector against a fake Agent OS controller and a fake run sink:
//! the cases the old bridge handled (receipts, uncertain acceptance, group
//! decisions, long answers), on the host protocol.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_workspace_agent_os_connector::{
    Connector, Decision, RunSink,
    config::{AgentEntry, AgentOs, Config},
    controller::{Controller, ControllerError, read_envelope},
    output_text, participation,
};
use agent_workspace_host_protocol::{AckOutcome, Command};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeController {
    calls: Arc<Mutex<Vec<Value>>>,
    /// Answers to `task-submit`, in turn (then the last one again).
    submits: Arc<Mutex<Vec<Result<Value, ControllerError>>>>,
    /// The task's status `task-inspect` reports, and its output.
    status: Arc<Mutex<(String, Value)>>,
}

impl FakeController {
    fn calls(&self, operation: &str) -> Vec<Value> {
        let calls = self.calls.lock().expect("calls");
        calls
            .iter()
            .filter(|call| call["operation"] == operation)
            .cloned()
            .collect()
    }

    fn answer_submits(&self, answers: Vec<Result<Value, ControllerError>>) {
        *self.submits.lock().expect("submits") = answers;
    }

    fn finish(&self, status: &str, output: Value) {
        *self.status.lock().expect("status") = (status.to_owned(), output);
    }
}

impl Controller for FakeController {
    async fn call(&self, command: Value) -> Result<Value, ControllerError> {
        self.calls.lock().expect("calls").push(command.clone());
        match command["operation"].as_str().unwrap_or_default() {
            "plan" => Ok(json!({"source_revision": "rev-1"})),
            "task-submit" => {
                let mut submits = self.submits.lock().expect("submits");
                if submits.len() > 1 {
                    submits.remove(0)
                } else {
                    submits
                        .first()
                        .cloned()
                        .unwrap_or(Ok(json!({"id": "task-1", "runtime_run_id": "rt-1"})))
                }
            }
            "task-inspect" => {
                let (status, output) = self.status.lock().expect("status").clone();
                Ok(
                    json!({"task": {"id": command["id"], "runtime_run_id": "rt-1",
                    "observation": {"status": status, "output": output}}}),
                )
            }
            "task-cancel" => Ok(json!({})),
            other => Err(ControllerError::Rejected(format!("unknown {other}"))),
        }
    }
}

#[derive(Clone, Default)]
struct FakeSink {
    reports: Arc<Mutex<Vec<(String, Value)>>>,
}

impl FakeSink {
    fn reports(&self) -> Vec<(String, Value)> {
        self.reports.lock().expect("reports").clone()
    }
}

impl RunSink for FakeSink {
    async fn activities(&self, activities: Vec<Value>, _key: &str) -> Result<(), String> {
        let mut reports = self.reports.lock().expect("reports");
        for activity in activities {
            reports.push(("activity".to_owned(), activity));
        }
        Ok(())
    }

    async fn update(&self, fields: Value, _key: &str) -> Result<(), String> {
        self.reports
            .lock()
            .expect("reports")
            .push(("update".to_owned(), fields));
        Ok(())
    }
}

fn state_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "aw-agentos-connector-test-{}",
        uuid::Uuid::now_v7().simple()
    ))
}

fn config(state_dir: PathBuf) -> Config {
    let mut agents = BTreeMap::new();
    agents.insert(
        "helper".to_owned(),
        AgentEntry {
            agentos_id: "agent-helper".to_owned(),
            name: "Helper".to_owned(),
            instructions: None,
        },
    );
    Config {
        workspace_origin: "https://workspace.example".to_owned(),
        state_dir,
        host_name: "Agent OS".to_owned(),
        agentos: AgentOs {
            context: "example-context".to_owned(),
            command: vec![
                "agentos".to_owned(),
                "controller".to_owned(),
                "call".to_owned(),
            ],
            timeout_seconds: 5,
        },
        agents,
        poll_seconds: 1,
        task_deadline_seconds: 3600,
        max_reply_characters: 40,
    }
}

fn start(run: &str, agent: &str) -> Command {
    serde_json::from_value(json!({
        "commandId": uuid::Uuid::now_v7(), "seq": 1, "kind": "start",
        "runId": run, "startKey": format!("run-{run}"),
        "issuedAt": "2026-11-17T10:00:00Z", "expiresAt": "2026-11-18T10:00:00Z",
        "agent": {"accountId": "0199c000-0000-7000-8000-0000000000a1", "hostAgentRef": agent},
        "input": {"prompt": "What changed today?"},
        "cause": {"kind": "mention", "conversationId": "0199c000-0000-7000-8000-0000000000c1"},
    }))
    .expect("start")
}

fn cancel(run: &str) -> Command {
    serde_json::from_value(json!({
        "commandId": uuid::Uuid::now_v7(), "seq": 2, "kind": "cancel", "runId": run,
        "issuedAt": "2026-11-17T10:00:00Z", "expiresAt": "2026-11-18T10:00:00Z",
    }))
    .expect("cancel")
}

const RUN: &str = "0199c000-0000-7000-8000-000000000001";

#[tokio::test]
async fn a_start_is_one_task_however_often_it_is_offered() {
    let fake = FakeController::default();
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    let ack = connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("acknowledged");
    assert_eq!(ack.outcome, Some(AckOutcome::Accepted));
    assert_eq!(ack.host_run_ref.as_deref(), Some("task-1"));
    let submitted = fake.calls("task-submit");
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0]["idempotency_key"], format!("workspace-{RUN}"));
    assert_eq!(submitted[0]["submission"]["agent"], "agent-helper");
    assert_eq!(submitted[0]["submission"]["source_revision"], "rev-1");
    let prompt = submitted[0]["submission"]["prompt"]
        .as_str()
        .expect("prompt");
    assert!(
        prompt.contains("You are Helper") && prompt.contains("What changed today?"),
        "{prompt}"
    );

    // The acknowledgement was lost: the same start again, after a restart.
    let mut restarted: Connector<_, FakeSink> =
        Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    let again = restarted
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("acknowledged");
    assert_eq!(again.host_run_ref.as_deref(), Some("task-1"));
    assert_eq!(fake.calls("task-submit").len(), 1, "never submitted twice");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_lost_answer_keeps_the_receipt_and_sends_the_same_submission_again() {
    let fake = FakeController::default();
    fake.answer_submits(vec![
        Err(ControllerError::Uncertain("timed out".to_owned())),
        Ok(json!({"id": "task-7"})),
        Ok(json!({"id": "task-7", "runtime_run_id": "rt-7"})),
    ]);
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    assert!(
        connector
            .command(&start(RUN, "helper"), || Some(sink.clone()))
            .await
            .is_none(),
        "not acknowledged while Agent OS's answer is unknown"
    );
    assert!(
        connector
            .command(&start(RUN, "helper"), || Some(sink.clone()))
            .await
            .is_none(),
        "taken without a runtime run is still uncertain"
    );
    let ack = connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("acknowledged");
    assert_eq!(ack.host_run_ref.as_deref(), Some("task-7"));
    let submitted = fake.calls("task-submit");
    assert_eq!(submitted.len(), 3);
    assert!(
        submitted.windows(2).all(|pair| pair[0] == pair[1]),
        "the very same submission"
    );
    assert_eq!(fake.calls("plan").len(), 1, "the revision is read once");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_refused_task_or_an_unknown_agent_is_refused() {
    let fake = FakeController::default();
    fake.answer_submits(vec![Err(ControllerError::Rejected(
        "agent_unavailable".to_owned(),
    ))]);
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    let ack = connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("ack");
    assert_eq!(ack.outcome, Some(AckOutcome::Refused));
    let again = connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("ack");
    assert_eq!(again.outcome, Some(AckOutcome::Refused));
    assert_eq!(
        fake.calls("task-submit").len(),
        1,
        "a refusal is remembered"
    );
    let other = connector
        .command(
            &start("0199c000-0000-7000-8000-000000000002", "stranger"),
            || Some(sink.clone()),
        )
        .await
        .expect("ack");
    assert_eq!(other.reason.as_deref(), Some("unknown_agent"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_ended_task_becomes_the_agents_answer_a_pass_or_a_failure() {
    for (status, output, expected) in [
        ("completed", json!("Three pull requests merged."), "answer"),
        ("completed", json!("{\"decision\":\"pass\"}"), "pass"),
        ("failed", Value::Null, "failed"),
    ] {
        let fake = FakeController::default();
        let sink = FakeSink::default();
        let dir = state_dir();
        let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
        connector
            .command(&start(RUN, "helper"), || Some(sink.clone()))
            .await
            .expect("ack");
        fake.finish("running", Value::Null);
        connector.tick().await;
        assert_eq!(connector.running(), 1);
        fake.finish(status, output);
        connector.tick().await;
        assert_eq!(connector.running(), 0);
        let reports = sink.reports();
        let last = &reports.last().expect("reported").1;
        match expected {
            "answer" => {
                let said = &reports[reports.len() - 2].1;
                assert_eq!(said["kind"], "text_message");
                assert_eq!(said["payload"]["text"], "Three pull requests merged.");
                assert_eq!(last["state"], "done");
            }
            "pass" => {
                assert!(
                    reports
                        .iter()
                        .all(|(_, report)| report["kind"] != "text_message")
                );
                assert_eq!(last["state"], "done");
                assert_eq!(last["outcomeSummary"], "Nothing to add.");
            }
            _ => {
                assert_eq!(last["state"], "failed");
                assert!(
                    last["failureReason"]
                        .as_str()
                        .is_some_and(|why| why.contains("failed"))
                );
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test]
async fn stop_cancels_the_task() {
    let fake = FakeController::default();
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("ack");
    let ack = connector.command(&cancel(RUN), || None).await.expect("ack");
    assert_eq!(ack.outcome, Some(AckOutcome::Done));
    assert_eq!(fake.calls("task-cancel")[0]["id"], "task-1");
    assert_eq!(
        sink.reports().last().expect("reported").1["state"],
        "cancelled"
    );
    assert!(connector.receipts().unfinished().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn decisions_and_long_answers_read_as_the_bridge_read_them() {
    assert_eq!(participation(""), Ok(Decision::Pass));
    assert_eq!(participation("Pass."), Ok(Decision::Pass));
    assert_eq!(participation("{\"decision\":\"pass\"}"), Ok(Decision::Pass));
    assert_eq!(
        participation("```json\n{\"decision\":\"reply\",\"body\":\" hi \"}\n```"),
        Ok(Decision::Reply("hi".to_owned()))
    );
    assert_eq!(
        participation("Just text"),
        Ok(Decision::Reply("Just text".to_owned()))
    );
    assert!(participation("{\"decision\":\"reply\"}").is_err());
    assert!(participation("{\"decision\":\"maybe\"}").is_err());
    let long = output_text(&json!({"output": "x".repeat(100)}), 40);
    assert!(
        long.starts_with(&"x".repeat(40))
            && long.ends_with("[Cut short by the Agent OS connector.]")
    );
    assert_eq!(output_text(&json!({"output": {"a": 1}}), 40), "{\"a\":1}");
}

#[test]
fn the_controller_envelope_says_ok_refused_or_unknown() {
    assert_eq!(
        read_envelope(true, br#"{"status":"ok","result":{"id":"t"}}"#),
        Ok(json!({"id": "t"}))
    );
    assert_eq!(
        read_envelope(
            false,
            br#"{"status":"error","issue":{"code":"controller_peer_mismatch"}}"#
        ),
        Err(ControllerError::Rejected(
            "controller_peer_mismatch".to_owned()
        ))
    );
    assert!(matches!(
        read_envelope(true, b"not json"),
        Err(ControllerError::Uncertain(_))
    ));
}

#[test]
fn nothing_about_one_installation_is_in_the_code() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(dir).expect("src") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).expect("source");
        for needle in ["/home/", "/Users/", "~/", ".local/share", "\"context\": \""] {
            assert!(!text.contains(needle), "{} names {needle}", path.display());
        }
    }
    let example = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/connector.example.json"),
    )
    .expect("example");
    let parsed = Config::parse(&example).expect("the example parses");
    assert!(
        parsed.agentos.context.starts_with('<'),
        "the example uses placeholders"
    );
}
