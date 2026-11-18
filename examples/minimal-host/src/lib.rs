//! The smallest host a workspace can run agents on, written only against
//! the public host protocol (`agent-workspace-host-protocol`): it proves the
//! protocol is enough (A1.23). One agent, `echo`, says back what it was
//! asked; a prompt with a question mark first asks the requester to go on,
//! and one containing "wait" keeps working until it is stopped or steered.
//!
//! It enrols with a one-time code, describes itself (the agent, one offer,
//! a host action and three agent actions, one per notice), reads its
//! commands by long-poll, acknowledges each once, and reports on its runs
//! with the credential each start carried. A start offered again (its
//! acknowledgement was lost) is acknowledged with the run it already
//! started. See `docs/hosts/writing-a-host.md`.

use std::collections::HashMap;

use agent_workspace_host_protocol::{
    Ack, AckOutcome, ActionNeeds, ClientError, Command, CommandKind, DescribedAgent, Description,
    EnrolRequest, Heartbeat, HeartbeatRun, HostAction, HostClient, HostSoftware, Notice,
    NoticeKind, Offer, PROTOCOL_VERSION, RunReporter,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// The agent this host runs, by the reference it is bound with.
pub const AGENT: &str = "echo";

/// Link a new host with a one-time code.
pub async fn enrol(server: &str, code: &str, name: &str) -> Result<HostClient, ClientError> {
    let enrolled = HostClient::enrol(
        server,
        &EnrolRequest {
            code: code.to_owned(),
            host: HostSoftware {
                name: name.to_owned(),
                software: Some("minimal-host".to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            },
            protocol_versions: vec![PROTOCOL_VERSION.to_owned()],
            connection: None,
            push: None,
        },
    )
    .await?;
    HostClient::new(server, enrolled.credential.token)
}

/// What the host says it is.
pub fn description(agent: &str) -> Description {
    let action = |id: &str, label: &str| HostAction {
        id: id.to_owned(),
        label: label.to_owned(),
        needs: ActionNeeds::Confirm,
    };
    Description {
        software: Some(json!({"name": "minimal-host", "version": env!("CARGO_PKG_VERSION")})),
        capabilities: [("steer", true), ("answer", true), ("inspect", true)]
            .into_iter()
            .map(|(name, on)| (name.to_owned(), Value::Bool(on)))
            .collect(),
        report_interval_seconds: Some(15),
        offers: vec![Offer {
            id: "echo-agent".to_owned(),
            title: "Add an echo agent".to_owned(),
            description: Some("An agent that says back what it is asked.".to_owned()),
        }],
        actions: vec![action("ping", "Check the host answers")],
        agents: vec![DescribedAgent {
            host_agent_ref: agent.to_owned(),
            name: Some("Echo".to_owned()),
            actions: vec![
                action("rename", "Rename it"),
                action("move", "Give it a new reference"),
                action("retire", "Retire it"),
            ],
        }],
        accounts: Vec::new(),
    }
}

/// One run this host is doing.
struct Run {
    host_run_ref: String,
    reporter: RunReporter,
    state: &'static str,
    prompt: String,
}

/// The host's memory: its runs, and the starts it has seen.
pub struct MinimalHost {
    client: HostClient,
    agent: String,
    runs: HashMap<Uuid, Run>,
    /// Start key → (run, host run reference): a start is never run twice.
    started: HashMap<String, (Uuid, String)>,
    cursor: i64,
    /// How many runs it has started.
    count: u64,
    forgetful: bool,
}

impl MinimalHost {
    pub fn new(client: HostClient) -> Self {
        Self {
            client,
            agent: AGENT.to_owned(),
            runs: HashMap::new(),
            started: HashMap::new(),
            cursor: 0,
            count: 0,
            forgetful: false,
        }
    }

    /// For the conformance suite's own test: a broken host that forgets
    /// every start it ran, so a start offered again is run again.
    #[doc(hidden)]
    pub fn forget_starts(&mut self) {
        self.forgetful = true;
    }

    pub async fn describe(&self) -> Result<Value, ClientError> {
        self.client.describe(&description(&self.agent)).await
    }

    pub async fn heartbeat(&self) -> Result<Value, ClientError> {
        let runs = self
            .runs
            .iter()
            .map(|(run_id, run)| HeartbeatRun {
                run_id: *run_id,
                host_run_ref: Some(run.host_run_ref.clone()),
                state: Some(run.state.to_owned()),
            })
            .collect();
        self.client
            .heartbeat(&Heartbeat {
                capacity: Some(json!({"runs": 8})),
                runs,
            })
            .await
    }

    /// Wait up to `wait` seconds for commands and carry out each.
    pub async fn step(&mut self, wait: u64) -> Result<usize, ClientError> {
        let list = self.client.commands(self.cursor, wait).await?;
        self.cursor = list.next;
        let count = list.commands.len();
        for summary in list.commands {
            let command = match self.client.command(summary.command_id).await {
                Ok(command) => command,
                // Gone or expired: nothing to do.
                Err(ClientError::Refused(404 | 410, _)) => continue,
                Err(error) => return Err(error),
            };
            let ack = self.carry_out(&command).await;
            // A lost acknowledgement is fine: the command is offered again
            // and answered the same way.
            let _ = self.client.ack(command.command_id, &ack).await;
        }
        Ok(count)
    }

    async fn carry_out(&mut self, command: &Command) -> Ack {
        match command.kind {
            CommandKind::Start => self.start(command).await,
            CommandKind::Cancel => self.cancel(command).await,
            CommandKind::Steer => self.steer(command).await,
            CommandKind::Answer => self.answer(command).await,
            CommandKind::Inspect => self.inspect(command),
            CommandKind::Action => self.action(command).await,
        }
    }

    async fn start(&mut self, command: &Command) -> Ack {
        let (Some(run_id), Some(start_key)) = (command.run_id, command.start_key.as_deref()) else {
            return refused("invalid", "A start needs its run and start key.");
        };
        if self.forgetful {
            self.started.clear();
        }
        if let Some((_, host_run_ref)) = self.started.get(start_key) {
            return accepted(host_run_ref);
        }
        let Some(credential) = &command.workspace_credential else {
            return refused("no_credential", "The start carried no credential.");
        };
        let prompt = command.detail["input"]["prompt"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.count += 1;
        let host_run_ref = format!("echo-{}", self.count);
        self.started
            .insert(start_key.to_owned(), (run_id, host_run_ref.clone()));
        let reporter = self.client.reporter(run_id, credential);
        let mut run = Run {
            host_run_ref: host_run_ref.clone(),
            reporter,
            state: "working",
            prompt,
        };
        run.state = begin(&run).await;
        if run.state == "working" || run.state == "waiting" {
            self.runs.insert(run_id, run);
        }
        accepted(&host_run_ref)
    }

    async fn cancel(&mut self, command: &Command) -> Ack {
        if let Some(run) = command.run_id.and_then(|run_id| self.runs.remove(&run_id)) {
            let _ = run
                .reporter
                .update(
                    json!({"state": "cancelled"}),
                    &format!("{}-cancelled", run.host_run_ref),
                )
                .await;
        }
        Ack::new(AckOutcome::Done)
    }

    async fn steer(&mut self, command: &Command) -> Ack {
        let Some(run) = command.run_id.and_then(|run_id| self.runs.get(&run_id)) else {
            return refused("unknown_run", "This host isn't doing that run.");
        };
        let message = command.detail["messageId"].as_str().unwrap_or_default();
        let _ = say(run, &format!("steer-{message}"), "Noted: I was steered.").await;
        Ack::new(AckOutcome::Accepted)
    }

    async fn answer(&mut self, command: &Command) -> Ack {
        let Some(run) = command.run_id.and_then(|run_id| self.runs.remove(&run_id)) else {
            return refused("unknown_run", "This host isn't doing that run.");
        };
        let answer = &command.detail["answer"]["answer"];
        let said = answer["text"]
            .as_str()
            .or_else(|| answer["label"].as_str())
            .unwrap_or("nothing");
        let request = command.request_id.unwrap_or_default();
        let _ = say(
            &run,
            &format!("answer-{request}"),
            &format!("You said: {said}"),
        )
        .await;
        if answer["optionId"] == "no" {
            let _ = finish(&run, "cancelled").await;
        } else {
            let _ = say(&run, "echo", &format!("Echo: {}", run.prompt)).await;
            let _ = finish(&run, "done").await;
        }
        Ack::new(AckOutcome::Accepted)
    }

    fn inspect(&self, command: &Command) -> Ack {
        let found = command.run_id.and_then(|run_id| self.runs.get(&run_id));
        let known = self
            .started
            .values()
            .any(|(run_id, _)| Some(*run_id) == command.run_id);
        let mut ack = Ack::new(AckOutcome::Done);
        ack.result = Some(match found {
            Some(run) => json!({"known": true, "hostRunRef": run.host_run_ref, "state": run.state}),
            None => json!({"known": known}),
        });
        ack
    }

    async fn action(&mut self, command: &Command) -> Ack {
        let action = command.detail["action"].as_str().unwrap_or_default();
        let notice = match action {
            "ping" => {
                let mut ack = Ack::new(AckOutcome::Done);
                ack.result = Some(json!({"message": "pong"}));
                return ack;
            }
            "rename" => {
                let mut notice = Notice::new(NoticeKind::AgentProfileChanged, &self.agent);
                notice.profile = Some(json!({"name": "Echo (renamed)"}));
                notice
            }
            "move" => {
                let next = format!("{AGENT}-{}", Uuid::now_v7().simple());
                let mut notice = Notice::new(NoticeKind::AgentRefChanged, &self.agent);
                notice.new_host_agent_ref = Some(next.clone());
                notice.reference = Some(json!({"lineage": AGENT}));
                self.agent = next;
                notice
            }
            "retire" => {
                let mut notice = Notice::new(NoticeKind::AgentRetired, &self.agent);
                notice.reason = Some("Retired from the host".to_owned());
                notice
            }
            _ => return refused("unknown_action", "This host has no such action."),
        };
        match self.client.notice(&notice).await {
            Ok(answer) => {
                let mut ack = Ack::new(AckOutcome::Done);
                ack.result = Some(answer);
                ack
            }
            Err(_) => refused("notice_failed", "The server didn't take the notice."),
        }
    }
}

/// Begin a run: say what it was asked, then finish, ask, or keep working.
async fn begin(run: &Run) -> &'static str {
    let started = run
        .reporter
        .activities(
            vec![json!({"id": "started", "kind": "run_started"})],
            &format!("{}-started", run.host_run_ref),
        )
        .await;
    if started.is_err() {
        return "failed";
    }
    let words = run.prompt.split_whitespace().count() as u64;
    let _ = run
        .reporter
        .usage(&format!("{}-1", run.host_run_ref), "echo", words, words)
        .await;
    if run.prompt.contains('?') {
        let request = json!({"kind": "approval", "clientRequestId": format!("{}-go-on", run.host_run_ref),
            "title": "Echo this back?", "options": [
                {"id": "yes", "label": "Yes", "style": "approve"},
                {"id": "no", "label": "No", "style": "deny"}]});
        let asked = run
            .reporter
            .activities(
                vec![
                    json!({"id": "go-on", "kind": "interrupt", "summary": "Asking to go on",
                            "payload": {"request": request}}),
                ],
                &format!("{}-go-on", run.host_run_ref),
            )
            .await;
        return if asked.is_ok() { "waiting" } else { "failed" };
    }
    if run.prompt.to_lowercase().contains("wait") {
        let _ = say(run, "waiting", "Working until I'm stopped.").await;
        return "working";
    }
    let _ = say(run, "echo", &format!("Echo: {}", run.prompt)).await;
    finish(run, "done").await
}

async fn say(run: &Run, id: &str, text: &str) -> Result<Value, ClientError> {
    run.reporter
        .activities(
            vec![json!({"id": id, "kind": "text_message", "payload": {"text": text}})],
            &format!("{}-{id}", run.host_run_ref),
        )
        .await
}

async fn finish(run: &Run, state: &'static str) -> &'static str {
    match run
        .reporter
        .update(
            json!({"state": state}),
            &format!("{}-{state}", run.host_run_ref),
        )
        .await
    {
        Ok(_) => state,
        Err(_) => "failed",
    }
}

fn accepted(host_run_ref: &str) -> Ack {
    let mut ack = Ack::new(AckOutcome::Accepted);
    ack.host_run_ref = Some(host_run_ref.to_owned());
    ack
}

fn refused(reason: &str, message: &str) -> Ack {
    let mut ack = Ack::new(AckOutcome::Refused);
    ack.reason = Some(reason.to_owned());
    ack.message = Some(message.to_owned());
    ack
}

/// Run until `stop` resolves: describe, then read commands and send a
/// heartbeat every ten seconds (and whenever the server asks for a new
/// description, send it).
pub async fn serve(
    mut host: MinimalHost,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), ClientError> {
    host.describe().await?;
    host.heartbeat().await?;
    let mut last_beat = tokio::time::Instant::now();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            () = &mut stop => return Ok(()),
            stepped = host.step(5) => {
                if let Err(error) = stepped {
                    eprintln!("minimal-host: reading commands failed: {error:?}");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
        if last_beat.elapsed() >= std::time::Duration::from_secs(10) {
            last_beat = tokio::time::Instant::now();
            if let Ok(answer) = host.heartbeat().await
                && answer["describeWanted"] == true
            {
                let _ = host.describe().await;
            }
        }
    }
}
