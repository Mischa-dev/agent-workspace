//! Agents that speak the Agent Client Protocol (A1.21): a program a
//! workspace registers, run in the run's worktree, spoken to as an ACP
//! client over JSON-RPC lines on its stdin and stdout.
//!
//! The registration is a command-line harness's (the same allowed
//! directories, argument template, environment and secrets on descriptor
//! 3); only the conversation differs. Each run session starts the program
//! once, `initialize`s it, and opens an ACP session per thread
//! (`session/new`, or `session/load` when the agent can and the thread is
//! resumed). A turn is one `session/prompt`; while it runs the agent's
//! `session/update` notifications become events (message and thought
//! chunks, tool calls, the plan) and its `session/request_permission`
//! becomes an approval. The client offers no file system or terminal of
//! its own: the agent works in the worktree with its own tools.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemDetail, ItemKind,
        LoginKind, LoginStart, PlanItem, ProfileHealth, RateLimitSummary, RecoveredTurn,
        RequestKind, RequestOption,
    },
    command_line::{CommandSpec, spawn_child},
    host::{HostCell, host_of},
    line_process::{Line, LineProcess},
    manifest::{AdapterManifest, ModelEntry, RunLocation, VersionRange},
    run_config::RunConfig,
    slots::{RunSlot, RunSlots},
};
use agent_workspace_redaction::SecretValue;

/// The ACP version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;
/// How long `initialize`, `session/new` and `session/load` may take.
const SETUP_SECONDS: u64 = 60;
/// How long a cancelled prompt has to answer before the agent is stopped.
const CANCEL_SECONDS: u64 = 5;
const THREAD_PREFIX: &str = "acp:";

/// The manifest of an ACP registration: a command-line harness's, with
/// what the protocol always gives (streamed messages, tool calls, file
/// changes, permission requests).
pub fn build_manifest(kind: &str, name: &str, manifest: &Value) -> Result<AdapterManifest, String> {
    let mut built =
        super::command_line::build_manifest(kind, name, manifest, RunLocation::LocalHost)?;
    let described = manifest
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if !described {
        built.description =
            "An agent that speaks the Agent Client Protocol, registered in this workspace."
                .to_owned();
    }
    built.versions = VersionRange {
        minimum: format!("{PROTOCOL_VERSION}.0.0"),
        below: Some(format!("{}.0.0", PROTOCOL_VERSION + 1)),
    };
    built.features.streaming = true;
    built.features.commands = true;
    built.features.file_changes = true;
    built.features.approvals = true;
    built.features.questions = false;
    Ok(built)
}

pub struct AcpAdapter {
    spec: Arc<CommandSpec>,
    slots: RunSlots,
    host: HostCell,
}

impl AcpAdapter {
    pub fn new(spec: CommandSpec, host_limit: usize, host: HostCell) -> Self {
        Self {
            spec: Arc::new(spec),
            slots: RunSlots::new(host_limit),
            host,
        }
    }

    fn session(&self, account: AccountHandle, slot: Option<RunSlot>) -> AcpSession {
        AcpSession {
            stop_generation: self.slots.stop_generation(account.id),
            account,
            spec: Arc::clone(&self.spec),
            slots: self.slots.clone(),
            host: Arc::clone(&self.host),
            _slot: slot,
            agent: None,
            turn: None,
        }
    }
}

impl HarnessAdapter for AcpAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.spec.manifest
    }

    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async move {
            let slot = self.slots.acquire(&account)?;
            Ok(Box::new(self.session(account, Some(slot))) as Box<dyn HarnessSession>)
        })
    }

    fn acquire_control(
        &self,
        account: AccountHandle,
    ) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(
            async move { Ok(Box::new(self.session(account, None)) as Box<dyn HarnessSession>) },
        )
    }

    fn health(
        &self,
        account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        let running = self.slots.running(account_id);
        Box::pin(async move {
            Some(ProfileHealth {
                process_running: running > 0,
                ..ProfileHealth::default()
            })
        })
    }

    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.slots.stop_account(account_id);
        Box::pin(async {})
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// The running agent program and the JSON-RPC conversation with it.
struct Agent {
    process: LineProcess,
    next_id: u64,
    can_load: bool,
    /// Thread ids asked for, and the ACP session that serves each now (a
    /// thread whose session couldn't be loaded gets a fresh one).
    sessions: HashMap<String, String>,
    /// Messages read while waiting for a response, for the turn.
    stashed: VecDeque<Value>,
}

impl Agent {
    async fn start(
        session: &AcpSession,
        cwd: &Path,
        config: &RunConfig,
    ) -> Result<Self, HarnessError> {
        let process = spawn_child(
            &session.spec,
            &session.host,
            &session.account,
            config,
            cwd,
            ("AGENT_WORKSPACE_ACP_CLIENT", "1"),
        )
        .await?;
        let mut agent = Self {
            process,
            next_id: 0,
            can_load: false,
            sessions: HashMap::new(),
            stashed: VecDeque::new(),
        };
        let answer = agent
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false },
                        "terminal": false
                    },
                    "clientInfo": { "name": "agent-workspace", "version": env!("CARGO_PKG_VERSION") }
                }),
                false,
            )
            .await?;
        if answer.get("protocolVersion").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
            agent.process.kill().await;
            return Err(HarnessError::Unsupported(
                "this Agent Client Protocol version",
            ));
        }
        agent.can_load = answer
            .pointer("/agentCapabilities/loadSession")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(agent)
    }

    async fn send(&mut self, message: Value) -> Result<(), HarnessError> {
        self.process.send(&message).await
    }

    /// Sends a request and waits for its answer. Anything else the agent
    /// says meanwhile is kept for the turn, or dropped when `quiet` (a
    /// loaded session replays its history first). Requests from the agent
    /// are refused: none can come before a prompt.
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        quiet: bool,
    ) -> Result<Value, HarnessError> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        let wait = async {
            loop {
                match self.process.next().await {
                    Line::Json(message) => {
                        if is_response(&message)
                            && message.get("id").and_then(Value::as_u64) == Some(id)
                        {
                            return match message.get("result") {
                                Some(result) => Ok(result.clone()),
                                None => Err(HarnessError::InvalidResponse),
                            };
                        }
                        if message.get("method").is_some() && message.get("id").is_some() {
                            let refusal = method_not_found(&message);
                            self.send(refusal).await?;
                        } else if !quiet {
                            self.stashed.push_back(message);
                        }
                    }
                    Line::Text => {}
                    Line::Closed => {
                        return Err(HarnessError::MissingResult("an answer from the agent"));
                    }
                }
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(SETUP_SECONDS), wait).await {
            Ok(answer) => answer,
            Err(_) => Err(HarnessError::MissingResult(
                "an answer from the agent in time",
            )),
        }
    }

    async fn new_session(&mut self, cwd: &Path) -> Result<String, HarnessError> {
        let answer = self
            .call(
                "session/new",
                json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] }),
                false,
            )
            .await?;
        answer
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 200)
            .map(str::to_owned)
            .ok_or(HarnessError::MissingResult("a session id"))
    }
}

/// One tool call as reported so far.
#[derive(Default)]
struct ToolCall {
    kind: String,
    title: Option<String>,
    raw_input: Option<Value>,
    locations: Vec<String>,
    diffs: Vec<(String, bool)>,
    output: Vec<String>,
    raw_output: Option<Value>,
    started: bool,
}

struct AcpTurn {
    session_id: String,
    turn_id: String,
    prompt_id: u64,
    tools: HashMap<String, ToolCall>,
    queue: VecDeque<HarnessEvent>,
    thought: String,
    finished: bool,
}

struct AcpSession {
    account: AccountHandle,
    spec: Arc<CommandSpec>,
    slots: RunSlots,
    stop_generation: u64,
    host: HostCell,
    _slot: Option<RunSlot>,
    agent: Option<Agent>,
    turn: Option<AcpTurn>,
}

impl AcpSession {
    async fn agent(&mut self, cwd: &Path, config: &RunConfig) -> Result<&mut Agent, HarnessError> {
        if self.agent.is_none() {
            let agent = Agent::start(self, cwd, config).await?;
            self.agent = Some(agent);
        }
        self.agent
            .as_mut()
            .ok_or(HarnessError::MissingResult("the agent"))
    }

    async fn stop_agent(&mut self) {
        self.turn = None;
        if let Some(mut agent) = self.agent.take() {
            agent.process.kill().await;
        }
    }
}

impl HarnessSession for AcpSession {
    fn account_id(&self) -> Uuid {
        self.account.id
    }

    fn is_stopping(&self) -> bool {
        self.slots.stop_generation(self.account.id) != self.stop_generation
    }

    fn start_thread<'a>(
        &'a mut self,
        cwd: &'a Path,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            let agent = self.agent(cwd, config).await?;
            let session = agent.new_session(cwd).await?;
            let thread = format!("{THREAD_PREFIX}{session}");
            agent.sessions.insert(thread.clone(), session);
            Ok(thread)
        })
    }

    fn resume_thread<'a>(
        &'a mut self,
        thread_id: &'a str,
        cwd: &'a Path,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            let Some(wanted) = thread_id.strip_prefix(THREAD_PREFIX) else {
                return Err(HarnessError::MissingResult("thread"));
            };
            let agent = self.agent(cwd, config).await?;
            if agent.sessions.contains_key(thread_id) {
                return Ok(());
            }
            // The agent loads the session when it can (replaying its
            // history, which was already reported); otherwise, or if it
            // no longer has it, the thread goes on in a new session.
            let loaded = agent.can_load
                && agent
                    .call(
                        "session/load",
                        json!({
                            "sessionId": wanted,
                            "cwd": cwd.to_string_lossy(),
                            "mcpServers": []
                        }),
                        true,
                    )
                    .await
                    .is_ok();
            let session = if loaded {
                wanted.to_owned()
            } else {
                agent.new_session(cwd).await?
            };
            agent.sessions.insert(thread_id.to_owned(), session);
            Ok(())
        })
    }

    fn start_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        _cwd: &'a Path,
        prompt: &'a str,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            let agent = self
                .agent
                .as_mut()
                .ok_or(HarnessError::MissingResult("thread"))?;
            let session_id = agent
                .sessions
                .get(thread_id)
                .cloned()
                .ok_or(HarnessError::MissingResult("thread"))?;
            agent.next_id += 1;
            let prompt_id = agent.next_id;
            agent
                .send(json!({
                    "jsonrpc": "2.0",
                    "id": prompt_id,
                    "method": "session/prompt",
                    "params": {
                        "sessionId": session_id,
                        "prompt": [{ "type": "text", "text": prompt }]
                    }
                }))
                .await?;
            let turn_id = format!("acp-turn-{}", Uuid::now_v7());
            self.turn = Some(AcpTurn {
                session_id,
                turn_id: turn_id.clone(),
                prompt_id,
                tools: HashMap::new(),
                queue: VecDeque::new(),
                thought: String::new(),
                finished: false,
            });
            Ok(turn_id)
        })
    }

    fn interrupt_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            let Some(turn) = self.turn.as_ref().filter(|turn| turn.turn_id == turn_id) else {
                return Ok(());
            };
            let (session_id, prompt_id) = (turn.session_id.clone(), turn.prompt_id);
            let Some(agent) = self.agent.as_mut() else {
                return Ok(());
            };
            let _ = agent
                .send(json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": { "sessionId": session_id }
                }))
                .await;
            // The prompt answers `cancelled` once the agent has stopped;
            // one that doesn't is stopped with its program.
            let answered =
                tokio::time::timeout(std::time::Duration::from_secs(CANCEL_SECONDS), async {
                    loop {
                        match agent.process.next().await {
                            Line::Json(message)
                                if is_response(&message)
                                    && message.get("id").and_then(Value::as_u64)
                                        == Some(prompt_id) =>
                            {
                                return true;
                            }
                            Line::Json(message)
                                if message.get("method").is_some()
                                    && message.get("id").is_some() =>
                            {
                                let _ = agent.send(cancelled_permission(&message)).await;
                            }
                            Line::Closed => return false,
                            _ => {}
                        }
                    }
                })
                .await
                .unwrap_or(false);
            self.turn = None;
            if !answered {
                self.stop_agent().await;
            }
            Ok(())
        })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(async move {
            loop {
                let turn = self
                    .turn
                    .as_mut()
                    .ok_or(HarnessError::MissingResult("an active turn"))?;
                if let Some(event) = turn.queue.pop_front() {
                    return Ok(event);
                }
                if turn.finished {
                    return std::future::pending().await;
                }
                let agent = self
                    .agent
                    .as_mut()
                    .ok_or(HarnessError::MissingResult("the agent"))?;
                let line = match agent.stashed.pop_front() {
                    Some(message) => Line::Json(message),
                    None => agent.process.next().await,
                };
                match line {
                    Line::Json(message) => {
                        if message.get("method").is_some() && message.get("id").is_some() {
                            if let Some(request) = permission(&message, turn) {
                                flush_thought(turn);
                                turn.queue.push_back(HarnessEvent::Request(request));
                            } else {
                                agent.send(method_not_found(&message)).await?;
                            }
                        } else if is_response(&message) {
                            if message.get("id").and_then(Value::as_u64) == Some(turn.prompt_id) {
                                flush_thought(turn);
                                turn.finished = true;
                                let completed = message
                                    .pointer("/result/stopReason")
                                    .and_then(Value::as_str)
                                    .is_some_and(|reason| {
                                        matches!(
                                            reason,
                                            "end_turn" | "max_tokens" | "max_turn_requests"
                                        )
                                    });
                                turn.queue
                                    .push_back(HarnessEvent::TurnCompleted { completed });
                            }
                        } else if message.get("method").and_then(Value::as_str)
                            == Some("session/update")
                            && message.pointer("/params/sessionId").and_then(Value::as_str)
                                == Some(turn.session_id.as_str())
                        {
                            let events = update(
                                turn,
                                message.pointer("/params/update").unwrap_or(&Value::Null),
                            );
                            turn.queue.extend(events);
                        }
                    }
                    Line::Text => {}
                    Line::Closed => {
                        flush_thought(turn);
                        turn.finished = true;
                        turn.queue
                            .push_back(HarnessEvent::TurnCompleted { completed: false });
                    }
                }
            }
        })
    }

    fn answer(&mut self, request: HarnessRequest, answer: HarnessAnswer) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            let Some(agent) = self.agent.as_mut() else {
                return Ok(());
            };
            let id = request
                .token
                .get("rpcId")
                .cloned()
                .ok_or(HarnessError::InvalidResponse)?;
            let chosen = match answer {
                HarnessAnswer::Approve => request.token.get("allow"),
                HarnessAnswer::Deny | HarnessAnswer::Text(_) => request.token.get("reject"),
            }
            .and_then(Value::as_str);
            let outcome = match chosen {
                Some(option) => json!({ "outcome": "selected", "optionId": option }),
                None => json!({ "outcome": "cancelled" }),
            };
            agent
                .send(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }))
                .await
        })
    }

    fn read_latest_turn<'a>(&'a mut self, _thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        // A turn lives in the program this host started; after a restart
        // it is gone, and the run says so.
        Box::pin(async {
            Ok(RecoveredTurn {
                id: None,
                status: "interrupted".to_owned(),
                final_message: None,
            })
        })
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary> {
        Box::pin(async move {
            let needs_key = !self.spec.manifest.login_methods.is_empty();
            let authenticated = if needs_key {
                match self.host.get() {
                    Some(host) => host.account_secret(self.account.id).await?.is_some(),
                    None => false,
                }
            } else {
                true
            };
            Ok(AccountSummary {
                authenticated,
                account_type: authenticated
                    .then(|| if needs_key { "api_key" } else { "none" }.to_owned()),
                email: None,
            })
        })
    }

    fn start_login(&mut self, _kind: LoginKind) -> HarnessFuture<'_, LoginStart> {
        Box::pin(async { Err(HarnessError::Unsupported("browser sign-in")) })
    }

    fn login_with_api_key<'a>(&'a mut self, api_key: &'a str) -> HarnessFuture<'a, LoginStart> {
        Box::pin(async move {
            let key = api_key.trim();
            if self.spec.manifest.login_methods.is_empty()
                || !(8..=4096).contains(&key.len())
                || key.chars().any(char::is_control)
            {
                return Err(HarnessError::InvalidResponse);
            }
            host_of(&self.host)?
                .store_account_secret(self.account.id, &SecretValue::new(key))
                .await?;
            Ok(LoginStart::ApiKey)
        })
    }

    fn cancel_login<'a>(&'a mut self, _login_id: &'a str) -> HarnessFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&mut self) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            if let Some(host) = self.host.get() {
                host.delete_account_secret(self.account.id).await?;
            }
            Ok(())
        })
    }

    fn read_rate_limits(&mut self) -> HarnessFuture<'_, RateLimitSummary> {
        Box::pin(async {
            Ok(RateLimitSummary {
                primary: None,
                secondary: None,
            })
        })
    }

    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>> {
        let models = self.spec.manifest.default_models.clone();
        Box::pin(async move { Ok(models) })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.stop_agent().await })
    }
}

fn is_response(message: &Value) -> bool {
    message.get("method").is_none()
        && message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn method_not_found(request: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": { "code": -32601, "message": "Method not found" }
    })
}

/// The answer to a permission request that arrives while a turn stops.
fn cancelled_permission(request: &Value) -> Value {
    if request.get("method").and_then(Value::as_str) == Some("session/request_permission") {
        json!({
            "jsonrpc": "2.0",
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "result": { "outcome": { "outcome": "cancelled" } }
        })
    } else {
        method_not_found(request)
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Item keys are the agent's ids, kept apart per turn.
fn key(turn: &AcpTurn, kind: &str, id: &str) -> String {
    let id: String = id.chars().take(120).collect();
    format!("{}:{kind}:{id}", turn.turn_id)
}

/// What the agent thought since its last step, as one note.
fn flush_thought(turn: &mut AcpTurn) {
    let thought = std::mem::take(&mut turn.thought);
    if !thought.trim().is_empty() {
        turn.queue.push_back(HarnessEvent::Note(thought));
    }
}

/// A permission request for this turn's session, as an approval: allow
/// picks the agent's allow-once option (or its first allowing one), deny
/// its reject-once option. People answer every request themselves, so an
/// "always" option is never offered.
fn permission(message: &Value, turn: &AcpTurn) -> Option<HarnessRequest> {
    if message.get("method").and_then(Value::as_str) != Some("session/request_permission") {
        return None;
    }
    let params = message.get("params")?;
    if params.get("sessionId").and_then(Value::as_str) != Some(turn.session_id.as_str()) {
        return None;
    }
    let options = params.get("options").and_then(Value::as_array)?;
    let pick = |kinds: &[&str]| {
        kinds.iter().find_map(|kind| {
            options
                .iter()
                .find(|option| option.get("kind").and_then(Value::as_str) == Some(*kind))
                .and_then(|option| option.get("optionId").and_then(Value::as_str))
                .map(str::to_owned)
        })
    };
    let allow = pick(&["allow_once", "allow_always"]);
    let reject = pick(&["reject_once", "reject_always"]);
    let tool = params.get("toolCall").unwrap_or(&Value::Null);
    let tool_id = text(tool, "toolCallId").unwrap_or_else(|| Uuid::now_v7().to_string());
    let title = text(tool, "title")
        .or_else(|| turn.tools.get(&tool_id).and_then(|call| call.title.clone()))
        .unwrap_or_else(|| "The agent asks to go on.".to_owned());
    Some(HarnessRequest {
        kind: RequestKind::Approval,
        durable_id: key(turn, "permission", &tool_id),
        summary: agent_workspace_redaction::cap(&agent_workspace_redaction::redact(&title), 1_000),
        options: vec![
            RequestOption {
                id: "approve".to_owned(),
                label: "Allow".to_owned(),
            },
            RequestOption {
                id: "deny".to_owned(),
                label: "Deny".to_owned(),
            },
        ],
        token: json!({ "rpcId": message.get("id"), "allow": allow, "reject": reject }),
    })
}

/// One `session/update`, as events.
fn update(turn: &mut AcpTurn, update: &Value) -> Vec<HarnessEvent> {
    let content_text = |value: &Value| {
        value
            .get("content")
            .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|content| text(content, "text"))
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            let mut events = Vec::new();
            let thought = std::mem::take(&mut turn.thought);
            if !thought.trim().is_empty() {
                events.push(HarnessEvent::Note(thought));
            }
            if let Some(chunk) = content_text(update).filter(|chunk| !chunk.is_empty()) {
                events.push(HarnessEvent::MessageDelta(chunk));
            }
            events
        }
        Some("agent_thought_chunk") => {
            if let Some(chunk) = content_text(update) {
                turn.thought.push_str(&chunk);
            }
            Vec::new()
        }
        Some("plan") => {
            let items = update
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(100)
                .filter_map(|entry| {
                    Some(PlanItem {
                        title: text(entry, "content")?,
                        status: match entry.get("status").and_then(Value::as_str) {
                            Some("in_progress") => "in_progress",
                            Some("completed") => "completed",
                            _ => "pending",
                        }
                        .to_owned(),
                    })
                })
                .collect::<Vec<_>>();
            let mut events = Vec::new();
            let thought = std::mem::take(&mut turn.thought);
            if !thought.trim().is_empty() {
                events.push(HarnessEvent::Note(thought));
            }
            events.push(HarnessEvent::Plan(items));
            events
        }
        Some(kind @ ("tool_call" | "tool_call_update")) => {
            let Some(id) = text(update, "toolCallId") else {
                return Vec::new();
            };
            let mut events = Vec::new();
            let thought = std::mem::take(&mut turn.thought);
            if !thought.trim().is_empty() {
                events.push(HarnessEvent::Note(thought));
            }
            let item_key = key(turn, "tool", &id);
            let call = turn.tools.entry(id.clone()).or_default();
            if kind == "tool_call" || update.get("kind").is_some() {
                call.kind = text(update, "kind").unwrap_or_else(|| "other".to_owned());
            }
            if let Some(title) = text(update, "title") {
                call.title = Some(title);
            }
            if let Some(input) = update.get("rawInput") {
                call.raw_input = Some(input.clone());
            }
            if let Some(output) = update.get("rawOutput") {
                call.raw_output = Some(output.clone());
            }
            if let Some(locations) = update.get("locations").and_then(Value::as_array) {
                call.locations = locations
                    .iter()
                    .filter_map(|location| text(location, "path"))
                    .take(50)
                    .collect();
            }
            if let Some(content) = update.get("content").and_then(Value::as_array) {
                for part in content.iter().take(50) {
                    match part.get("type").and_then(Value::as_str) {
                        Some("diff") => {
                            if let Some(path) = text(part, "path") {
                                let added = part.get("oldText").is_none_or(Value::is_null);
                                call.diffs.push((path, added));
                            }
                        }
                        Some("content") => {
                            if let Some(chunk) = part.get("content").and_then(|inner| {
                                (inner.get("type").and_then(Value::as_str) == Some("text"))
                                    .then(|| text(inner, "text"))
                                    .flatten()
                            }) {
                                call.output.push(chunk);
                            }
                        }
                        _ => {}
                    }
                }
            }
            let status = text(update, "status").unwrap_or_else(|| "pending".to_owned());
            let finished = matches!(status.as_str(), "completed" | "failed");
            let item = tool_item(&item_key, call, status == "failed");
            if finished {
                turn.tools.remove(&id);
                events.push(HarnessEvent::ItemCompleted(item));
            } else if !call.started {
                call.started = true;
                events.push(HarnessEvent::ItemStarted(item));
            }
            events
        }
        _ => Vec::new(),
    }
}

/// A tool call as an item: a command when it ran one, a file change when
/// it edited, deleted or moved files, else a named tool.
fn tool_item(item_key: &str, call: &ToolCall, failed: bool) -> HarnessItem {
    let output = if call.output.is_empty() {
        call.raw_output.as_ref().and_then(|raw| match raw {
            Value::String(text) => Some(text.clone()),
            Value::Object(fields) => ["output", "stdout", "content"]
                .iter()
                .find_map(|name| fields.get(*name).and_then(Value::as_str))
                .map(str::to_owned),
            _ => None,
        })
    } else {
        Some(call.output.join("\n"))
    };
    let exit_code = call.raw_output.as_ref().and_then(|raw| {
        raw.get("exitCode")
            .or_else(|| raw.get("exit_code"))
            .and_then(Value::as_i64)
    });
    match call.kind.as_str() {
        "execute" => {
            let command = call
                .raw_input
                .as_ref()
                .and_then(|input| match input.get("command") {
                    Some(Value::String(command)) => Some(command.clone()),
                    Some(Value::Array(parts)) => Some(
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    _ => None,
                })
                .or_else(|| call.title.clone());
            HarnessItem {
                key: item_key.to_owned(),
                kind: ItemKind::CommandExecution,
                detail: ItemDetail {
                    command,
                    exit_code,
                    output,
                    failed: failed || exit_code.is_some_and(|code| code != 0),
                    ..ItemDetail::default()
                },
            }
        }
        "edit" | "delete" | "move" => {
            let mut files = call
                .diffs
                .iter()
                .map(|(path, added)| {
                    let change = if call.kind == "delete" {
                        "deleted"
                    } else if *added {
                        "added"
                    } else {
                        "modified"
                    };
                    (path.clone(), Some(change.to_owned()))
                })
                .collect::<Vec<_>>();
            if files.is_empty() {
                let change = if call.kind == "delete" {
                    "deleted"
                } else {
                    "modified"
                };
                files = call
                    .locations
                    .iter()
                    .map(|path| (path.clone(), Some(change.to_owned())))
                    .collect();
            }
            HarnessItem {
                key: item_key.to_owned(),
                kind: ItemKind::FileChange,
                detail: ItemDetail {
                    tool: call.title.clone(),
                    files,
                    failed,
                    ..ItemDetail::default()
                },
            }
        }
        _ => HarnessItem {
            key: item_key.to_owned(),
            kind: ItemKind::Other,
            detail: ItemDetail {
                tool: Some(call.title.clone().unwrap_or_else(|| call.kind.clone())),
                output,
                failed,
                ..ItemDetail::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::harness::contract_tests::adapter_contract;

    /// A recorded ACP agent: the lines a real agent sent for one turn (a
    /// plan, a thought, a command, a permission request answered, an edit,
    /// the answer), replayed by a shell script against the client's
    /// requests. It records the requests it got.
    const AGENT: &str = r#"#!/bin/sh
log="$HOME/requests.jsonl"
read line; echo "$line" >> "$log"
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":false},"authMethods":[]}}'
read line; echo "$line" >> "$log"
echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1"}}'
read line || exit 0
echo "$line" >> "$log"
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"plan","entries":[{"content":"Run the tests","priority":"high","status":"in_progress"},{"content":"Write the notes","priority":"medium","status":"pending"}]}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Checking the tests "}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"first."}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"make test","kind":"execute","status":"in_progress","rawInput":{"command":["make","test"]}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"4 passed"}}],"rawOutput":{"exitCode":0}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"tool_call","toolCallId":"t2","title":"Write NOTES.md","kind":"edit","status":"pending","locations":[{"path":"NOTES.md"}]}}}'
echo '{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"sess-1","toolCall":{"toolCallId":"t2","title":"Write NOTES.md"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"always","name":"Always","kind":"allow_always"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
read answer; echo "$answer" >> "$log"
case "$answer" in
  *'"yes"'*) echo note > NOTES.md
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t2","status":"completed","content":[{"type":"diff","path":"NOTES.md","oldText":null,"newText":"note"}]}}}';;
  *) echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t2","status":"failed"}}}';;
esac
echo '{"jsonrpc":"2.0","id":"x-1","method":"terminal/create","params":{"sessionId":"sess-1","command":"ls"}}'
read refused; echo "$refused" >> "$log"
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"not this session"}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wrote "}}}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"the notes."}}}}'
echo '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
while read line; do echo "$line" >> "$log"; done
"#;

    struct Host;

    impl super::super::host::HarnessHost for Host {
        fn account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>> {
            Box::pin(async { Ok(None) })
        }
        fn store_account_secret<'a>(
            &'a self,
            _: Uuid,
            _: &'a SecretValue,
        ) -> HarnessFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
        fn delete_account_secret(&self, _: Uuid) -> HarnessFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn harness_secrets<'a>(
            &'a self,
            _: Uuid,
            _: &'a [String],
        ) -> HarnessFuture<'a, Vec<(String, SecretValue)>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn repository_access<'a>(
            &'a self,
            _: &'a Path,
        ) -> HarnessFuture<'a, super::super::host::RepositoryAccess> {
            Box::pin(async { Err(HarnessError::Unsupported("repositories")) })
        }
    }

    fn setup() -> (tempfile::TempDir, AcpAdapter, AccountHandle, PathBuf) {
        let root = tempfile::tempdir().expect("root");
        let dir = root.path().join("agents");
        std::fs::create_dir_all(&dir).expect("test step");
        let program = dir.join("agent.sh");
        std::fs::write(&program, AGENT).expect("test step");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("test step");
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).expect("test step");
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("test step");
        let manifest = build_manifest(
            "x_acp_agent",
            "An ACP agent",
            &json!({"models": [{"id": "agent-default", "displayName": "Default"}], "maxConcurrentRuns": 2}),
        )
        .expect("manifest");
        let command = vec![program.to_string_lossy().into_owned()];
        super::super::command_line::validate_command(&command, std::slice::from_ref(&dir))
            .expect("allowed");
        let cell = HostCell::default();
        let _ = cell.set(Arc::new(Host) as Arc<dyn super::super::host::HarnessHost>);
        let adapter = AcpAdapter::new(
            CommandSpec {
                manifest,
                command,
                secret_names: Vec::new(),
                allowed_dirs: vec![dir],
            },
            8,
            cell,
        );
        let account = AccountHandle {
            id: Uuid::now_v7(),
            home,
            max_concurrent_runs: 2,
        };
        (root, adapter, account, worktree)
    }

    #[tokio::test]
    async fn acp_agents_keep_the_adapter_contract() {
        let (_root, adapter, account, worktree) = setup();
        adapter_contract(&adapter, account, &worktree).await;
    }

    async fn turn(
        answer: HarnessAnswer,
    ) -> (Vec<HarnessEvent>, AccountHandle, PathBuf, tempfile::TempDir) {
        let (root, adapter, account, worktree) = setup();
        let config = RunConfig::default();
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        let thread = run.start_thread(&worktree, &config).await.expect("thread");
        assert_eq!(thread, "acp:sess-1");
        run.start_turn(&thread, &worktree, "write notes", &config)
            .await
            .expect("turn");
        let mut answer = Some(answer);
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(10), run.next_event())
                .await
                .expect("the agent answers")
                .expect("event");
            if let HarnessEvent::Request(request) = event {
                assert_eq!(request.kind, RequestKind::Approval);
                assert_eq!(request.summary, "Write NOTES.md");
                assert_eq!(
                    request.options.len(),
                    2,
                    "an always option is never offered"
                );
                run.answer(request, answer.take().expect("asked once"))
                    .await
                    .expect("answer");
                continue;
            }
            let done = matches!(event, HarnessEvent::TurnCompleted { .. });
            events.push(event);
            if done {
                break;
            }
        }
        run.shutdown().await;
        (events, account, worktree, root)
    }

    #[tokio::test]
    async fn an_acp_turn_becomes_steps_a_plan_an_approval_and_a_message() {
        let (events, account, worktree, _root) = turn(HarnessAnswer::Approve).await;
        assert!(matches!(
            events.last(),
            Some(HarnessEvent::TurnCompleted { completed: true })
        ));
        assert!(worktree.join("NOTES.md").exists(), "allowed once, it wrote");
        let plan = events.iter().find_map(|event| match event {
            HarnessEvent::Plan(items) => Some(items),
            _ => None,
        });
        assert_eq!(
            plan.expect("a plan")
                .iter()
                .map(|item| (item.title.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Run the tests", "in_progress"),
                ("Write the notes", "pending")
            ]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::Note(text) if text == "Checking the tests first."
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item) if item.kind == ItemKind::CommandExecution
                && item.detail.command.as_deref() == Some("make test")
                && item.detail.exit_code == Some(0)
                && item.detail.output.as_deref() == Some("4 passed")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item) if item.kind == ItemKind::FileChange
                && item.detail.files == vec![("NOTES.md".to_owned(), Some("added".to_owned()))]
        )));
        let message = events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::MessageDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(message, "Wrote the notes.", "only this session's words");
        let requests = std::fs::read_to_string(account.home.join("requests.jsonl")).expect("log");
        let lines = requests
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json"))
            .collect::<Vec<_>>();
        assert_eq!(lines[0]["method"], "initialize");
        assert_eq!(lines[0]["params"]["clientCapabilities"]["terminal"], false);
        assert_eq!(lines[1]["method"], "session/new");
        assert_eq!(lines[2]["method"], "session/prompt");
        assert_eq!(lines[3]["result"]["outcome"]["optionId"], "yes");
        assert_eq!(
            lines[4]["error"]["code"], -32601,
            "the client has no terminal"
        );
    }

    #[tokio::test]
    async fn a_denied_permission_rejects_once() {
        let (events, account, worktree, _root) = turn(HarnessAnswer::Deny).await;
        assert!(!worktree.join("NOTES.md").exists());
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item) if item.kind == ItemKind::FileChange && item.detail.failed
        )));
        let requests = std::fs::read_to_string(account.home.join("requests.jsonl")).expect("log");
        assert!(requests.contains(r#""optionId":"no""#), "{requests}");
    }

    #[tokio::test]
    async fn a_thread_the_agent_cant_load_goes_on_in_a_new_session() {
        let (_root, adapter, account, worktree) = setup();
        let config = RunConfig::default();
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        run.resume_thread("acp:earlier", &worktree, &config)
            .await
            .expect("resumed");
        run.start_turn("acp:earlier", &worktree, "go on", &config)
            .await
            .expect("turn");
        // The first update shows the agent has the prompt.
        run.next_event().await.expect("event");
        run.shutdown().await;
        let requests = std::fs::read_to_string(account.home.join("requests.jsonl")).expect("log");
        let lines = requests
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Value>(line).expect("json"))
            .collect::<Vec<_>>();
        assert_eq!(
            lines[1]["method"], "session/new",
            "no session/load without loadSession"
        );
        assert_eq!(lines[2]["params"]["sessionId"], "sess-1");
        assert!(run_thread_refused(&adapter, &account, &worktree).await);
    }

    async fn run_thread_refused(
        adapter: &AcpAdapter,
        account: &AccountHandle,
        worktree: &Path,
    ) -> bool {
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        run.resume_thread("cli-thread-1", worktree, &RunConfig::default())
            .await
            .is_err()
    }

    #[test]
    fn manifests_say_what_the_protocol_gives() {
        let manifest = build_manifest("x_a", "A", &json!({})).expect("manifest");
        assert!(manifest.features.streaming && manifest.features.approvals);
        assert!(!manifest.features.questions);
        assert!(manifest.description.contains("Agent Client Protocol"));
        assert!(manifest.versions.contains("1.0.0"));
    }
}
