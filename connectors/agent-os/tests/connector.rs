//! The connector against a fake Agent OS controller and a fake run sink:
//! the cases the old bridge handled (receipts, uncertain acceptance, group
//! decisions, long answers), on the host protocol.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_workspace_agent_os_connector::{
    Connector, Decision, MEMORY_ACTION, PAUSE_ACTION, RunSink,
    config::{AgentEntry, AgentOs, Config, OfferEntry, Operations},
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
    /// Answers to `task-cancel`, in turn (then a plain yes).
    cancels: Arc<Mutex<Vec<Result<Value, ControllerError>>>>,
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
            "task-list" => Ok(json!({"tasks": [
                // Started in Agent OS for the configured agent: adopted.
                {"id": "own-1", "agent": "agent-helper", "title": "Nightly digest\nmore",
                 "runtime_run_id": "rt-9", "observation": {"status": "running"}},
                // The connector's own start: already a run.
                {"id": "task-1", "agent": "agent-helper", "idempotency_key": "workspace-run",
                 "observation": {"status": "running"}},
                // Over already.
                {"id": "own-2", "agent": "agent-helper", "observation": {"status": "completed"}},
                // An agent this host doesn't serve.
                {"id": "own-3", "agent": "someone-else", "observation": {"status": "running"}},
            ]})),
            "agent-pause" => Ok(json!({"paused": true})),
            "memory-show" => Ok(json!({"text": "Prefers short answers."})),
            "usage-report" => Ok(json!({"accounts": [{
                "ref": "acct-1", "name": "Team plan", "plan": "team",
                "windows": [
                    {"key": "five-hour", "label": "5 hours", "usedPercent": 42.4, "resetsAt": "2026-11-18T15:00:00Z"},
                    {"key": "Bad Key", "usedPercent": 10},
                    {"key": "week", "usedPercent": 100},
                ],
            }]})),
            "task-cancel" => {
                let mut cancels = self.cancels.lock().expect("cancels");
                if cancels.is_empty() {
                    Ok(json!({}))
                } else {
                    cancels.remove(0)
                }
            }
            other => Err(ControllerError::Rejected(format!("unknown {other}"))),
        }
    }
}

#[derive(Clone, Default)]
struct FakeSink {
    reports: Arc<Mutex<Vec<(String, Value)>>>,
    /// Updates still to fail, as when the workspace can't be reached.
    failing_updates: Arc<Mutex<usize>>,
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
        {
            let mut failing = self.failing_updates.lock().expect("failing");
            if *failing > 0 {
                *failing -= 1;
                return Err("unreachable".to_owned());
            }
        }
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
        offers: Vec::new(),
        operations: Operations::default(),
        adopt_seconds: 30,
    }
}

/// The configuration with every optional part turned on.
fn full_config(state_dir: PathBuf) -> Config {
    Config {
        offers: vec![OfferEntry {
            id: "researcher".to_owned(),
            title: "A research agent".to_owned(),
            description: Some("Reads and summarises.".to_owned()),
        }],
        operations: Operations {
            list_tasks: Some("task-list".to_owned()),
            usage: Some("usage-report".to_owned()),
            pause: Some("agent-pause".to_owned()),
            memory: Some("memory-show".to_owned()),
        },
        ..config(state_dir)
    }
}

fn action(agent: &str, action: &str) -> Command {
    serde_json::from_value(json!({
        "commandId": uuid::Uuid::now_v7(), "seq": 3, "kind": "action",
        "issuedAt": "2026-11-17T10:00:00Z", "expiresAt": "2026-11-18T10:00:00Z",
        "hostAgentRef": agent, "action": action, "input": {},
    }))
    .expect("action")
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
async fn a_restart_takes_up_its_tasks_and_a_lost_last_report_is_sent_again() {
    let fake = FakeController::default();
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector: Connector<_, FakeSink> =
        Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    let first = start(RUN, "helper");
    connector
        .command(&first, || Some(sink.clone()))
        .await
        .expect("ack");
    drop(connector);

    // After a restart it knows the task and the start to fetch again.
    let mut restarted: Connector<_, FakeSink> =
        Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    assert_eq!(restarted.running(), 0);
    let waiting = restarted.to_resume();
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        waiting[0].command_id.as_deref(),
        Some(first.command_id.to_string().as_str())
    );
    let resumed = FakeSink::default();
    restarted.resume(waiting[0].clone(), resumed.clone());
    assert!(restarted.to_resume().is_empty());
    assert_eq!(
        restarted.heartbeat().runs.len(),
        1,
        "the heartbeat names it"
    );
    // Its credential gets old: the start to fetch again is named, and a new
    // sink takes over.
    let stale = restarted.stale_credentials(std::time::Duration::ZERO);
    assert_eq!(stale, vec![(RUN.to_owned(), first.command_id.to_string())]);
    restarted.refresh(RUN, resumed.clone());
    assert!(
        restarted
            .stale_credentials(std::time::Duration::from_secs(60))
            .is_empty()
    );

    // Its last report is lost once: it stays until the report lands.
    *resumed.failing_updates.lock().expect("failing") = 1;
    fake.finish("completed", json!("{\"decision\":\"pass\"}"));
    restarted.tick().await;
    assert_eq!(restarted.running(), 1, "kept until the report lands");
    assert!(resumed.reports().is_empty());
    restarted.tick().await;
    assert_eq!(restarted.running(), 0);
    assert_eq!(
        resumed.reports().last().expect("reported").1["state"],
        "done"
    );
    assert!(restarted.to_resume().is_empty(), "finished for good");
    let _ = std::fs::remove_dir_all(dir);
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

/// A stop whose answer is lost or refused leaves the run watched: the
/// cancel is offered again, and only a confirmed stop forgets the task.
#[tokio::test]
async fn an_unconfirmed_stop_keeps_the_task() {
    let fake = FakeController::default();
    let sink = FakeSink::default();
    let dir = state_dir();
    let mut connector = Connector::new(config(dir.clone()), fake.clone()).expect("connector");
    connector
        .command(&start(RUN, "helper"), || Some(sink.clone()))
        .await
        .expect("ack");
    *fake.cancels.lock().expect("cancels") = vec![
        Err(ControllerError::Uncertain("timed out".to_owned())),
        Err(ControllerError::Rejected("busy".to_owned())),
    ];
    assert!(connector.command(&cancel(RUN), || None).await.is_none());
    let refused = connector.command(&cancel(RUN), || None).await.expect("ack");
    assert_eq!(refused.outcome, Some(AckOutcome::Refused));
    assert_eq!(connector.receipts().unfinished().len(), 1);
    assert!(
        sink.reports()
            .iter()
            .all(|(_, fields)| fields["state"] != "cancelled")
    );
    let ack = connector.command(&cancel(RUN), || None).await.expect("ack");
    assert_eq!(ack.outcome, Some(AckOutcome::Done));
    assert_eq!(fake.calls("task-cancel").len(), 3);
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

#[tokio::test]
async fn offers_and_actions_are_described_only_when_configured() {
    let plain: Connector<FakeController, FakeSink> =
        Connector::new(config(state_dir()), FakeController::default()).expect("connector");
    let described = plain.description();
    assert!(described.offers.is_empty());
    assert!(described.agents[0].actions.is_empty());

    let full: Connector<FakeController, FakeSink> =
        Connector::new(full_config(state_dir()), FakeController::default()).expect("connector");
    let described = full.description();
    assert_eq!(described.offers[0].id, "researcher");
    let actions: Vec<(&str, &str)> = described.agents[0]
        .actions
        .iter()
        .map(|action| (action.id.as_str(), action.label.as_str()))
        .collect();
    assert_eq!(
        actions,
        [
            (PAUSE_ACTION, "Pause"),
            (MEMORY_ACTION, "What it remembers")
        ]
    );
}

#[tokio::test]
async fn pause_and_what_it_remembers_call_agent_os_for_that_agent() {
    let fake = FakeController::default();
    let mut connector: Connector<FakeController, FakeSink> =
        Connector::new(full_config(state_dir()), fake.clone()).expect("connector");
    let paused = connector
        .command(&action("helper", PAUSE_ACTION), || None)
        .await
        .expect("answered");
    assert_eq!(paused.outcome, Some(AckOutcome::Done));
    assert_eq!(paused.result, Some(json!({"paused": true})));
    assert_eq!(fake.calls("agent-pause")[0]["agent"], "agent-helper");

    let memory = connector
        .command(&action("helper", MEMORY_ACTION), || None)
        .await
        .expect("answered");
    assert_eq!(
        memory.result,
        Some(json!({"text": "Prefers short answers."}))
    );

    for (agent, name) in [("helper", "rename"), ("stranger", PAUSE_ACTION)] {
        let refused = connector
            .command(&action(agent, name), || None)
            .await
            .expect("answered");
        assert_eq!(refused.outcome, Some(AckOutcome::Refused));
    }
    // Without the operation configured, there is no such action.
    let mut plain: Connector<FakeController, FakeSink> =
        Connector::new(config(state_dir()), fake.clone()).expect("connector");
    let refused = plain
        .command(&action("helper", PAUSE_ACTION), || None)
        .await
        .expect("answered");
    assert_eq!(refused.reason.as_deref(), Some("unknown_action"));
    assert_eq!(fake.calls("agent-pause").len(), 1);
}

#[tokio::test]
async fn a_task_started_in_agent_os_is_adopted_once_and_reported_like_a_start() {
    let fake = FakeController::default();
    let dir = state_dir();
    let mut connector = Connector::new(full_config(dir.clone()), fake.clone()).expect("connector");
    let found = connector.found_tasks().await;
    assert_eq!(
        found.len(),
        1,
        "only the running task of a served agent, not its own"
    );
    assert_eq!(found[0].task_id(), "own-1");
    assert_eq!(found[0].host_agent_ref, "helper");
    let request = found[0].adopt_request();
    assert_eq!(
        (request.host_run_ref.as_str(), request.title.as_str()),
        ("own-1", "Nightly digest")
    );

    let sink = FakeSink::default();
    connector
        .adopted(
            found[0].clone(),
            RUN.to_owned(),
            uuid::Uuid::now_v7().to_string(),
            sink.clone(),
        )
        .await;
    assert!(connector.found_tasks().await.is_empty(), "adopted once");
    assert_eq!(
        connector.heartbeat().runs[0].host_run_ref.as_deref(),
        Some("own-1")
    );

    fake.finish("completed", json!("Digest sent."));
    connector.tick().await;
    let reports = sink.reports();
    assert_eq!(
        reports[0].1["statusLine"],
        "Started in Agent OS; its steps aren't shown here."
    );
    assert_eq!(reports[1].1["payload"]["text"], "Digest sent.");
    assert_eq!(reports[2].1["state"], "done");
    assert_eq!(fake.calls("task-inspect")[0]["id"], "own-1");

    // After a restart it is still known: not adopted a second time.
    let restarted: Connector<FakeController, FakeSink> =
        Connector::new(full_config(dir), fake).expect("connector");
    assert!(restarted.found_tasks().await.is_empty());
}

#[tokio::test]
async fn usage_reports_the_accounts_windows_and_says_when_the_accounts_change() {
    let mut connector: Connector<FakeController, FakeSink> =
        Connector::new(full_config(state_dir()), FakeController::default()).expect("connector");
    let (changed, report) = connector.usage().await.expect("usage");
    assert!(changed);
    assert_eq!(connector.description().accounts[0].name, "Team plan");
    let windows = &report.accounts[0].windows;
    assert_eq!(
        windows.len(),
        2,
        "a window with an unusable key is left out"
    );
    assert_eq!(
        (
            windows[0].key.as_str(),
            windows[0].used_percent,
            windows[0].limit_reached
        ),
        ("five-hour", 42, false)
    );
    assert!(windows[1].limit_reached);
    let (changed, _) = connector.usage().await.expect("usage");
    assert!(!changed);

    let mut plain: Connector<FakeController, FakeSink> =
        Connector::new(config(state_dir()), FakeController::default()).expect("connector");
    assert!(plain.usage().await.is_none());
    assert!(plain.found_tasks().await.is_empty());
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
