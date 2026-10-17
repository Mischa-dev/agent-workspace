//! Claude Code, headless (H6): one `claude -p` process per turn in the run's
//! worktree, speaking stream-json on stdin and stdout. Its tool calls become
//! neutral command, file-change and tool events, its permission prompts
//! (`can_use_tool` control requests) go to the approvals UI, and its usage
//! is priced from the catalog.
//!
//! Sign-in is either the account's own Claude login (made once on the
//! server with `claude auth login` in the account's home) or an API key kept
//! in the secret store. The key never enters the child's environment or
//! arguments: it sits in an anonymous file the child holds as descriptor 3,
//! and Claude Code's `apiKeyHelper` reads it from there when it needs it.

use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemDetail, ItemKind,
        LoginKind, LoginStart, ProfileHealth, RateLimitSummary, RecoveredTurn, RequestKind,
        RequestOption, TokenUsage, UsageReport, UsageWindow,
    },
    host::{HostCell, host_of},
    line_process::{Line, LineProcess},
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry, ModelPrices,
        RunLocation, RunOptionSupport, VersionRange,
    },
    registry::ShutdownSignal,
    run_config::RunConfig,
    secret_fd,
    slots::{RunSlot, RunSlots},
};
use crate::{config::AppConfig, secrets::SecretValue};

pub const CLAUDE_CODE_KIND: &str = "claude_code";

/// Reads the key from the anonymous file Claude Code was started with. The
/// helper runs in a shell Claude Code starts, so its parent holds fd 3.
const API_KEY_HELPER: &str = "cat \"/proc/$PPID/fd/3\"";

const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// How each approval policy maps onto Claude Code's permission modes.
const PERMISSION_MODES: [(&str, &str); 5] = [
    ("on_request", "default"),
    ("accept_edits", "acceptEdits"),
    ("auto", "auto"),
    ("plan", "plan"),
    ("dont_ask", "dontAsk"),
];

pub fn claude_code_manifest() -> AdapterManifest {
    AdapterManifest {
        kind: CLAUDE_CODE_KIND.to_owned(),
        display_name: "Claude Code".to_owned(),
        description: "Anthropic's coding agent, run headless in the run's worktree under your own Claude login or an API key. It asks here before commands and edits it isn't allowed to make.".to_owned(),
        versions: VersionRange {
            minimum: "2.1.0".to_owned(),
            below: Some("3.0.0".to_owned()),
        },
        branch_prefix: "claude-code/run-".to_owned(),
        runs_on: RunLocation::LocalHost,
        features: AdapterFeatures {
            streaming: true,
            commands: true,
            file_changes: true,
            approvals: true,
            questions: true,
            resume: true,
            interrupt: true,
            cloud: false,
            multiple_attempts: false,
            concurrent_runs: true,
            model_listing: false,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 1,
            max_per_account: 4,
        },
        login_methods: vec![LoginMethod {
            kind: LoginKind::ApiKey,
            label: "Paste an Anthropic API key. It's kept encrypted and handed to Claude Code privately, never in its environment. To use your Claude subscription instead, sign in once on the server with claude auth login in this account's folder.".to_owned(),
        }],
        run_options: RunOptionSupport {
            sandbox_modes: vec!["workspace_write".to_owned()],
            approval_policies: PERMISSION_MODES
                .iter()
                .map(|(policy, _)| (*policy).to_owned())
                .collect(),
            max_attempts: 1,
        },
        options_schema: json!({
            "type": "object",
            "properties": {
                "instructions": {
                    "type": "string",
                    "maxLength": 8000,
                    "description": "Added to Claude Code's system prompt."
                },
                "allowedTools": {
                    "type": "string",
                    "maxLength": 2000,
                    "description": "Tools it may use without asking, such as Read Grep Bash(cargo test:*)."
                },
                "maxBudgetUsd": {
                    "type": "number",
                    "minimum": 0.01,
                    "maximum": 1000,
                    "description": "Stop the turn once it has cost this much (API keys only)."
                }
            },
            "additionalProperties": false
        }),
        default_models: claude_models(),
    }
}

pub(crate) fn claude_models() -> Vec<ModelEntry> {
    let model =
        |id: &str, name: &str, description: &str, prices: (f64, f64, Option<f64>)| ModelEntry {
            id: id.to_owned(),
            display_name: name.to_owned(),
            description: Some(description.to_owned()),
            context_window: Some(1_000_000),
            reasoning_levels: EFFORT_LEVELS
                .iter()
                .map(|level| (*level).to_owned())
                .collect(),
            default_reasoning: Some("high".to_owned()),
            prices: Some(ModelPrices {
                currency: "USD".to_owned(),
                input_per_million: prices.0,
                output_per_million: prices.1,
                cached_input_per_million: prices.2,
            }),
            ..ModelEntry::default()
        };
    vec![
        ModelEntry {
            is_default: true,
            ..model(
                "claude-opus-5-5",
                "Claude Opus 5.5",
                "The strongest all-round coding model.",
                (4.0, 20.0, Some(0.2)),
            )
        },
        model(
            "claude-fable-5-1",
            "Claude Fable 5.1",
            "The most capable model, for the hardest work.",
            (10.0, 50.0, Some(0.25)),
        ),
        model(
            "claude-sonnet-5-5",
            "Claude Sonnet 5.5",
            "Fast and capable for everyday changes.",
            (2.0, 10.0, Some(0.2)),
        ),
        model(
            "claude-haiku-5-5",
            "Claude Haiku 5.5",
            "The quickest and cheapest.",
            (0.1, 0.5, None),
        ),
    ]
}

pub struct ClaudeCodeAdapter {
    manifest: AdapterManifest,
    binary: PathBuf,
    shutdown: Arc<ShutdownSignal>,
    slots: RunSlots,
    host: HostCell,
}

impl ClaudeCodeAdapter {
    pub fn new(config: &AppConfig, shutdown: Arc<ShutdownSignal>, host: HostCell) -> Self {
        Self::with_binary(
            config.claude_binary.clone(),
            config.codex_host_concurrency_limit,
            shutdown,
            host,
        )
    }

    pub fn with_binary(
        binary: PathBuf,
        host_limit: usize,
        shutdown: Arc<ShutdownSignal>,
        host: HostCell,
    ) -> Self {
        Self {
            manifest: claude_code_manifest(),
            binary,
            shutdown,
            slots: RunSlots::new(host_limit),
            host,
        }
    }

    fn session(&self, account: AccountHandle, slot: Option<RunSlot>) -> ClaudeSession {
        ClaudeSession {
            stop_generation: self.slots.stop_generation(account.id),
            account,
            binary: self.binary.clone(),
            slots: self.slots.clone(),
            host: Arc::clone(&self.host),
            _slot: slot,
            known_threads: HashMap::new(),
            turn: None,
        }
    }
}

impl HarnessAdapter for ClaudeCodeAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async move {
            if self.shutdown.is_shutting_down() {
                return Err(HarnessError::ShuttingDown);
            }
            let slot = self.slots.acquire(&account)?;
            Ok(Box::new(self.session(account, Some(slot))) as Box<dyn HarnessSession>)
        })
    }

    fn acquire_control(
        &self,
        account: AccountHandle,
    ) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async move {
            if self.shutdown.is_shutting_down() {
                return Err(HarnessError::ShuttingDown);
            }
            Ok(Box::new(self.session(account, None)) as Box<dyn HarnessSession>)
        })
    }

    fn health(
        &self,
        account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        let running = self.slots.running(account_id);
        Box::pin(async move {
            Some(ProfileHealth {
                process_running: running > 0,
                consecutive_failures: 0,
                circuit_open: false,
                process_starts: 0,
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

/// A tool call waiting for its result.
struct PendingTool {
    kind: ItemKind,
    detail: ItemDetail,
}

struct Turn {
    process: LineProcess,
    turn_id: String,
    pending: HashMap<String, PendingTool>,
    queue: std::collections::VecDeque<HarnessEvent>,
    finished: bool,
}

struct ClaudeSession {
    account: AccountHandle,
    binary: PathBuf,
    slots: RunSlots,
    stop_generation: u64,
    host: HostCell,
    _slot: Option<RunSlot>,
    /// Threads this session started or resumed, and whether Claude Code has
    /// a conversation for them yet (it does after the first turn).
    known_threads: HashMap<String, bool>,
    turn: Option<Turn>,
}

impl ClaudeSession {
    fn base_command(&self, cwd: &Path) -> Result<tokio::process::Command, HarnessError> {
        let binary_dir = self
            .binary
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| format!("{}:", parent.display()))
            .unwrap_or_default();
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .current_dir(cwd)
            .env_clear()
            .env("HOME", &self.account.home)
            .env("CLAUDE_CONFIG_DIR", self.account.home.join(".claude"))
            .env("PATH", format!("{binary_dir}/usr/local/bin:/usr/bin:/bin"))
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("NO_COLOR", "1")
            .env("DISABLE_AUTOUPDATER", "1");
        Ok(command)
    }

    async fn stored_key(&self) -> Result<Option<SecretValue>, HarnessError> {
        match self.host.get() {
            Some(host) => host.account_secret(self.account.id).await,
            None => Ok(None),
        }
    }

    async fn spawn_turn(
        &mut self,
        thread_id: &str,
        resume: bool,
        cwd: &Path,
        config: &RunConfig,
    ) -> Result<LineProcess, HarnessError> {
        let mut command = self.base_command(cwd)?;
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources",
            "user",
            "--strict-mcp-config",
        ]);
        command.arg(if resume { "--resume" } else { "--session-id" });
        command.arg(thread_id);
        if let Some(model) = &config.model {
            command.arg("--model").arg(model);
        }
        if let Some(effort) = config
            .reasoning
            .as_deref()
            .filter(|level| EFFORT_LEVELS.contains(level))
        {
            command.arg("--effort").arg(effort);
        }
        let mode = config
            .approval_policy
            .as_deref()
            .and_then(|policy| {
                PERMISSION_MODES
                    .iter()
                    .find(|(name, _)| *name == policy)
                    .map(|(_, mode)| *mode)
            })
            .unwrap_or("default");
        command.arg("--permission-mode").arg(mode);
        if let Some(instructions) = config
            .options
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            command.arg("--append-system-prompt").arg(instructions);
        }
        if let Some(tools) = config
            .options
            .get("allowedTools")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            command.arg("--allowedTools").arg(tools);
        }
        if let Some(budget) = config.options.get("maxBudgetUsd").and_then(Value::as_f64) {
            command.arg("--max-budget-usd").arg(format!("{budget:.2}"));
        }
        if let Some(key) = self.stored_key().await? {
            command
                .arg("--settings")
                .arg(json!({ "apiKeyHelper": API_KEY_HELPER }).to_string());
            secret_fd::attach(
                &mut command,
                secret_fd::secret_file(key.expose().as_bytes())?,
            )?;
        }
        LineProcess::spawn(command)
    }

    fn turn_mut(&mut self) -> Result<&mut Turn, HarnessError> {
        self.turn
            .as_mut()
            .ok_or(HarnessError::MissingResult("an active turn"))
    }

    /// `claude auth status --json` in the account's home.
    async fn auth_status(&self) -> Result<Value, HarnessError> {
        let mut command = self.base_command(&self.account.home)?;
        command
            .args(["auth", "status", "--json"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
            .await
            .map_err(|_| HarnessError::MissingResult("a timely sign-in status"))?
            .map_err(|error| HarnessError::Transport(Box::new(error)))?;
        serde_json::from_slice(&output.stdout).map_err(|_| HarnessError::InvalidResponse)
    }
}

impl HarnessSession for ClaudeSession {
    fn account_id(&self) -> Uuid {
        self.account.id
    }

    fn is_stopping(&self) -> bool {
        self.slots.stop_generation(self.account.id) != self.stop_generation
    }

    fn start_thread<'a>(
        &'a mut self,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            // Claude Code takes the conversation's id from us.
            let thread_id = Uuid::new_v4().to_string();
            self.known_threads.insert(thread_id.clone(), false);
            Ok(thread_id)
        })
    }

    fn resume_thread<'a>(
        &'a mut self,
        thread_id: &'a str,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            Uuid::parse_str(thread_id).map_err(|_| HarnessError::MissingResult("thread"))?;
            self.known_threads.insert(thread_id.to_owned(), true);
            Ok(())
        })
    }

    fn start_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        cwd: &'a Path,
        prompt: &'a str,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            let resume = *self
                .known_threads
                .get(thread_id)
                .ok_or(HarnessError::MissingResult("thread"))?;
            if let Some(mut previous) = self.turn.take() {
                previous.process.kill().await;
            }
            let mut process = self.spawn_turn(thread_id, resume, cwd, config).await?;
            process
                .send(&json!({
                    "type": "control_request",
                    "request_id": format!("init-{}", Uuid::now_v7()),
                    "request": { "subtype": "initialize" }
                }))
                .await?;
            process
                .send(&json!({
                    "type": "user",
                    "message": { "role": "user", "content": prompt }
                }))
                .await?;
            self.known_threads.insert(thread_id.to_owned(), true);
            let turn_id = format!("claude-turn-{}", Uuid::now_v7());
            self.turn = Some(Turn {
                process,
                turn_id: turn_id.clone(),
                pending: HashMap::new(),
                queue: std::collections::VecDeque::new(),
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
            if let Some(mut turn) = self.turn.take() {
                if turn.turn_id == turn_id {
                    let _ = turn
                        .process
                        .send(&json!({
                            "type": "control_request",
                            "request_id": format!("interrupt-{}", Uuid::now_v7()),
                            "request": { "subtype": "interrupt" }
                        }))
                        .await;
                    turn.process.kill().await;
                } else {
                    self.turn = Some(turn);
                }
            }
            Ok(())
        })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(async move {
            loop {
                let turn = self.turn_mut()?;
                if let Some(event) = turn.queue.pop_front() {
                    return Ok(event);
                }
                if turn.finished {
                    return std::future::pending().await;
                }
                match turn.process.next().await {
                    Line::Json(value) => {
                        let events = parse_line(&value, &mut turn.pending);
                        if events
                            .iter()
                            .any(|event| matches!(event, HarnessEvent::TurnCompleted { .. }))
                        {
                            turn.finished = true;
                            turn.process.close_input();
                        }
                        if let Some(reply) = unsupported_control_reply(&value) {
                            turn.process.send(&reply).await?;
                        }
                        turn.queue.extend(events);
                    }
                    Line::Text => {}
                    Line::Closed => {
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
            let Some(turn) = self.turn.as_mut() else {
                return Ok(());
            };
            let Some(request_id) = request.token.get("requestId").and_then(Value::as_str) else {
                return Err(HarnessError::InvalidResponse);
            };
            let input = request.token.get("input").cloned().unwrap_or(json!({}));
            let response = match (request.kind, answer) {
                (RequestKind::Question, HarnessAnswer::Text(text)) => {
                    let answers = input
                        .get("questions")
                        .and_then(Value::as_array)
                        .map(|questions| {
                            questions
                                .iter()
                                .filter_map(|question| question.get("question")?.as_str())
                                .map(|question| (question.to_owned(), Value::String(text.clone())))
                                .collect::<serde_json::Map<_, _>>()
                        })
                        .unwrap_or_default();
                    let mut updated = input.as_object().cloned().unwrap_or_default();
                    updated.insert("answers".to_owned(), Value::Object(answers));
                    json!({ "behavior": "allow", "updatedInput": updated })
                }
                (_, HarnessAnswer::Approve) => {
                    json!({ "behavior": "allow", "updatedInput": input })
                }
                (_, HarnessAnswer::Text(text)) => json!({ "behavior": "deny", "message": text }),
                (_, HarnessAnswer::Deny) => json!({
                    "behavior": "deny",
                    "message": "The person watching this run said no."
                }),
            };
            turn.process
                .send(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": request_id,
                        "response": response
                    }
                }))
                .await
        })
    }

    fn read_latest_turn<'a>(&'a mut self, _thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        // A turn lives as long as its process; after a restart it is gone,
        // and the conversation can be resumed with a new turn.
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
            if self.stored_key().await?.is_some() {
                return Ok(AccountSummary {
                    authenticated: true,
                    account_type: Some("api_key".to_owned()),
                    email: None,
                });
            }
            let status = self.auth_status().await?;
            let authenticated = status
                .get("loggedIn")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(AccountSummary {
                authenticated,
                account_type: authenticated
                    .then(|| {
                        status
                            .get("authMethod")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten(),
                email: status
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
    }

    fn start_login(&mut self, _kind: LoginKind) -> HarnessFuture<'_, LoginStart> {
        Box::pin(async { Err(HarnessError::Unsupported("browser sign-in")) })
    }

    fn login_with_api_key<'a>(&'a mut self, api_key: &'a str) -> HarnessFuture<'a, LoginStart> {
        Box::pin(async move {
            let key = api_key.trim();
            if !valid_api_key(key) {
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
        // Claude Code reports its windows during turns (rate_limit_event).
        Box::pin(async {
            Ok(RateLimitSummary {
                primary: None,
                secondary: None,
            })
        })
    }

    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>> {
        Box::pin(async { Ok(claude_models()) })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if let Some(mut turn) = self.turn.take() {
                turn.process.kill().await;
            }
        })
    }
}

/// Anthropic API keys: `sk-ant-` and a plain token.
fn valid_api_key(key: &str) -> bool {
    key.starts_with("sk-ant-")
        && (20..=400).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A control request this adapter doesn't handle gets an error back, so
/// Claude Code doesn't wait on it.
fn unsupported_control_reply(value: &Value) -> Option<Value> {
    if value.get("type").and_then(Value::as_str) != Some("control_request") {
        return None;
    }
    let subtype = value.pointer("/request/subtype").and_then(Value::as_str)?;
    if subtype == "can_use_tool" {
        return None;
    }
    let request_id = value.get("request_id").and_then(Value::as_str)?;
    Some(json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": format!("{subtype} is not supported here")
        }
    }))
}

/// One stream-json line as neutral events.
fn parse_line(value: &Value, pending: &mut HashMap<String, PendingTool>) -> Vec<HarnessEvent> {
    match value.get("type").and_then(Value::as_str) {
        Some("assistant") => assistant_events(value, pending),
        Some("user") => tool_results(value, pending),
        Some("control_request") => permission_request(value).into_iter().collect(),
        Some("rate_limit_event") => rate_limits(value).into_iter().collect(),
        Some("result") => result_events(value),
        _ => Vec::new(),
    }
}

fn blocks(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn assistant_events(
    value: &Value,
    pending: &mut HashMap<String, PendingTool>,
) -> Vec<HarnessEvent> {
    // Subagents' own steps carry their parent's tool id; the parent's tool
    // call already stands for them.
    if value
        .get("parent_tool_use_id")
        .is_some_and(|parent| !parent.is_null())
    {
        return Vec::new();
    }
    let mut events = Vec::new();
    for block in blocks(value) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    events.push(HarnessEvent::Note(text.to_owned()));
                }
            }
            Some("tool_use") => {
                let Some(id) = block.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                let (kind, detail) = tool_detail(name, &input);
                pending.insert(
                    id.to_owned(),
                    PendingTool {
                        kind,
                        detail: detail.clone(),
                    },
                );
                events.push(HarnessEvent::ItemStarted(HarnessItem {
                    key: format!("claude:{id}"),
                    kind,
                    detail,
                }));
            }
            _ => {}
        }
    }
    events
}

fn tool_detail(name: &str, input: &Value) -> (ItemKind, ItemDetail) {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    match name {
        "Bash" => (
            ItemKind::CommandExecution,
            ItemDetail {
                command: text("command"),
                ..ItemDetail::default()
            },
        ),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let path = text("file_path").or_else(|| text("notebook_path"));
            (
                ItemKind::FileChange,
                ItemDetail {
                    tool: Some(name.to_owned()),
                    files: path
                        .map(|path| vec![(path, Some("modified".to_owned()))])
                        .unwrap_or_default(),
                    ..ItemDetail::default()
                },
            )
        }
        _ => (
            ItemKind::Other,
            ItemDetail {
                tool: Some(name.to_owned()),
                ..ItemDetail::default()
            },
        ),
    }
}

fn tool_results(value: &Value, pending: &mut HashMap<String, PendingTool>) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    for block in blocks(value) {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(tool) = pending.remove(id) else {
            continue;
        };
        let mut detail = tool.detail;
        detail.failed = block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let structured = value.get("tool_use_result");
        match tool.kind {
            ItemKind::CommandExecution => {
                let output = structured
                    .and_then(|result| {
                        let out = result.get("stdout").and_then(Value::as_str)?;
                        let err = result.get("stderr").and_then(Value::as_str).unwrap_or("");
                        Some(if err.is_empty() {
                            out.to_owned()
                        } else {
                            format!("{out}\n{err}")
                        })
                    })
                    .or_else(|| result_text(block));
                detail.output = output;
            }
            ItemKind::FileChange => {
                if structured
                    .and_then(|result| result.get("type"))
                    .and_then(Value::as_str)
                    == Some("create")
                {
                    for file in &mut detail.files {
                        file.1 = Some("added".to_owned());
                    }
                }
            }
            _ => {}
        }
        events.push(HarnessEvent::ItemCompleted(HarnessItem {
            key: format!("claude:{id}"),
            kind: tool.kind,
            detail,
        }));
    }
    events
}

fn result_text(block: &Value) -> Option<String> {
    match block.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

fn permission_request(value: &Value) -> Option<HarnessEvent> {
    let request = value.get("request")?;
    if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        return None;
    }
    let request_id = value.get("request_id").and_then(Value::as_str)?;
    let tool = request
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("a tool");
    let input = request.get("input").cloned().unwrap_or(json!({}));
    let token = json!({ "requestId": request_id, "input": input });
    if tool == "AskUserQuestion" {
        let questions = input
            .get("questions")
            .and_then(Value::as_array)
            .map(|questions| {
                questions
                    .iter()
                    .filter_map(|question| question.get("question").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "Claude Code has a question.".to_owned());
        return Some(HarnessEvent::Request(HarnessRequest {
            kind: RequestKind::Question,
            durable_id: format!("claude-question-{request_id}"),
            summary: crate::run_log::cap(&crate::run_log::redact(&questions), 1_000),
            options: Vec::new(),
            token,
        }));
    }
    let display = request
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or(tool);
    let what = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .or_else(|| request.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let summary = if what.is_empty() {
        format!("Allow Claude Code to use {display}?")
    } else {
        format!("Allow Claude Code to use {display}: {what}?")
    };
    Some(HarnessEvent::Request(HarnessRequest {
        kind: RequestKind::Approval,
        durable_id: format!("claude-approval-{request_id}"),
        summary: crate::run_log::cap(&crate::run_log::redact(&summary), 1_000),
        options: vec![
            RequestOption {
                id: "approve".to_owned(),
                label: "Allow once".to_owned(),
            },
            RequestOption {
                id: "deny".to_owned(),
                label: "Deny".to_owned(),
            },
        ],
        token,
    }))
}

fn rate_limits(value: &Value) -> Option<HarnessEvent> {
    let windows = value
        .pointer("/rate_limit_info/unifiedWindows")?
        .as_object()?;
    let report = UsageReport {
        windows: windows
            .iter()
            .filter_map(|(name, window)| {
                let used = window.get("utilization").and_then(Value::as_f64)?;
                let key = format!("claude.{name}");
                (key.len() <= 64
                    && key.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    }))
                .then(|| UsageWindow {
                    key,
                    limit_name: Some(name.replace('_', " ")),
                    used_percent: (used * 100.0).round().clamp(0.0, 100.0) as u16,
                    resets_at: window.get("resetsAt").and_then(Value::as_i64),
                    duration_minutes: match name.as_str() {
                        "five_hour" => Some(300),
                        "seven_day" => Some(7 * 24 * 60),
                        _ => None,
                    },
                    limit_reached: used >= 1.0,
                })
            })
            .collect(),
        reset_credits: None,
        plan: None,
    };
    (!report.windows.is_empty()).then_some(HarnessEvent::UsageUpdated(report))
}

fn result_events(value: &Value) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    if let Some(models) = value.get("modelUsage").and_then(Value::as_object) {
        for (model, usage) in models {
            let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            events.push(HarnessEvent::TokensUsed(TokenUsage {
                model: Some(model.clone()),
                // Writing the prompt cache is billed as input.
                input_tokens: count("inputTokens") + count("cacheCreationInputTokens"),
                output_tokens: count("outputTokens"),
                cached_input_tokens: count("cacheReadInputTokens"),
                reported_cost_usd: usage.get("costUSD").and_then(Value::as_f64),
            }));
        }
    }
    let failed = value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value.get("subtype").and_then(Value::as_str) != Some("success");
    if let Some(text) = value.get("result").and_then(Value::as_str)
        && !failed
        && !text.is_empty()
    {
        events.push(HarnessEvent::MessageDelta(text.to_owned()));
    }
    events.push(HarnessEvent::TurnCompleted { completed: !failed });
    events
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

    use uuid::Uuid;

    use super::*;
    use crate::provider::contract_tests::adapter_contract;

    /// A stand-in `claude` that answers stream-json like Claude Code does:
    /// a Bash call, a permission prompt for a Write, then the result. It
    /// records its arguments and environment, and what fd 3 holds.
    const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = "auth" ]; then
  echo '{"loggedIn": false, "authMethod": "none"}'
  exit 0
fi
printf '%s\n' "$@" > "$HOME/args.txt"
env > "$HOME/env.txt"
if [ -e /dev/fd/3 ]; then cat /dev/fd/3 > "$HOME/fd3.txt"; fi
read init
read prompt
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Looking at the repo. token=hunter2"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]},"parent_tool_use_id":null}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"test result: ok. 3 passed; 0 failed; 0 ignored","is_error":false}]},"tool_use_result":{"stdout":"test result: ok. 3 passed; 0 failed; 0 ignored","stderr":""}}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Write","input":{"file_path":"NOTES.md","content":"hi"}}]},"parent_tool_use_id":null}'
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"NOTES.md","content":"hi"}}}'
read answer
case "$answer" in
  *'"allow"'*) echo hi > NOTES.md; echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"File created"}]},"tool_use_result":{"type":"create"}}';;
  *) echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"denied","is_error":true}]}}';;
esac
echo '{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.25,"resetsAt":1792211400}}}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Wrote NOTES.md.","modelUsage":{"claude-sonnet-5-5":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":5,"costUSD":0.01}}}'
"#;

    struct FakeHost {
        key: std::sync::Mutex<Option<String>>,
    }

    impl super::super::host::HarnessHost for FakeHost {
        fn account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>> {
            let key = self.key.lock().unwrap().clone();
            Box::pin(async move { Ok(key.map(SecretValue::new)) })
        }

        fn store_account_secret<'a>(
            &'a self,
            _account_id: Uuid,
            value: &'a SecretValue,
        ) -> HarnessFuture<'a, ()> {
            *self.key.lock().unwrap() = Some(value.expose().to_owned());
            Box::pin(async { Ok(()) })
        }

        fn delete_account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, ()> {
            *self.key.lock().unwrap() = None;
            Box::pin(async { Ok(()) })
        }

        fn harness_secrets<'a>(
            &'a self,
            _account_id: Uuid,
            _names: &'a [String],
        ) -> HarnessFuture<'a, Vec<(String, SecretValue)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn repository_access<'a>(
            &'a self,
            _worktree: &'a Path,
        ) -> HarnessFuture<'a, super::super::host::RepositoryAccess> {
            Box::pin(async { Err(HarnessError::Unsupported("repositories")) })
        }
    }

    fn setup() -> (
        tempfile::TempDir,
        ClaudeCodeAdapter,
        AccountHandle,
        Arc<FakeHost>,
    ) {
        let root = tempfile::tempdir().expect("root");
        let binary = root.path().join("bin/claude");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, FAKE_CLAUDE).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let host = Arc::new(FakeHost {
            key: std::sync::Mutex::new(None),
        });
        let cell = HostCell::default();
        let _ = cell.set(Arc::clone(&host) as Arc<dyn super::super::host::HarnessHost>);
        let adapter =
            ClaudeCodeAdapter::with_binary(binary, 8, Arc::new(ShutdownSignal::default()), cell);
        let account = AccountHandle {
            id: Uuid::now_v7(),
            home,
            max_concurrent_runs: 2,
        };
        (root, adapter, account, host)
    }

    #[tokio::test]
    async fn claude_code_keeps_the_adapter_contract() {
        let (root, adapter, account, _) = setup();
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        adapter_contract(&adapter, account, &worktree).await;
    }

    #[tokio::test]
    async fn a_turn_becomes_neutral_events_and_the_key_stays_out_of_the_environment() {
        let (root, adapter, account, host) = setup();
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let fake_key = format!("sk-ant-api03-{}", "x".repeat(40));
        let mut control = adapter.acquire_control(account.clone()).await.unwrap();
        assert!(!control.read_account().await.unwrap().authenticated);
        control.login_with_api_key(&fake_key).await.unwrap();
        assert!(control.read_account().await.unwrap().authenticated);
        assert!(control.login_with_api_key("not a key").await.is_err());
        drop(control);
        assert_eq!(host.key.lock().unwrap().as_deref(), Some(fake_key.as_str()));

        let config = RunConfig {
            model: Some("claude-sonnet-5-5".to_owned()),
            reasoning: Some("high".to_owned()),
            approval_policy: Some("accept_edits".to_owned()),
            ..RunConfig::default()
        };
        let mut run = adapter.acquire_run(account.clone()).await.unwrap();
        let thread = run.start_thread(&worktree, &config).await.unwrap();
        run.start_turn(&thread, &worktree, "write notes", &config)
            .await
            .unwrap();
        let mut events = Vec::new();
        loop {
            let event = run.next_event().await.unwrap();
            if let HarnessEvent::Request(request) = event {
                assert!(request.summary.contains("NOTES.md"));
                run.answer(request, HarnessAnswer::Approve).await.unwrap();
                continue;
            }
            let done = matches!(event, HarnessEvent::TurnCompleted { .. });
            events.push(event);
            if done {
                break;
            }
        }
        assert!(matches!(
            events.last(),
            Some(HarnessEvent::TurnCompleted { completed: true })
        ));
        let command = events.iter().find_map(|event| match event {
            HarnessEvent::ItemCompleted(item) if item.kind == ItemKind::CommandExecution => {
                Some(item.detail.clone())
            }
            _ => None,
        });
        let command = command.expect("the Bash call completed");
        assert_eq!(command.command.as_deref(), Some("cargo test"));
        assert!(command.output.unwrap().contains("3 passed"));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item)
                if item.kind == ItemKind::FileChange
                    && item.detail.files == vec![("NOTES.md".to_owned(), Some("added".to_owned()))]
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::TokensUsed(usage)
                if usage.input_tokens == 15 && usage.cached_input_tokens == 30
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, HarnessEvent::UsageUpdated(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::MessageDelta(text) if text == "Wrote NOTES.md."
        )));
        assert!(worktree.join("NOTES.md").exists());

        let args = std::fs::read_to_string(account.home.join("args.txt")).unwrap();
        let env = std::fs::read_to_string(account.home.join("env.txt")).unwrap();
        let fd3 = std::fs::read_to_string(account.home.join("fd3.txt")).unwrap();
        assert_eq!(fd3, fake_key, "the key arrives on fd 3");
        assert!(!args.contains(&fake_key) && !env.contains(&fake_key));
        assert!(!env.contains("ANTHROPIC_API_KEY"));
        assert!(args.contains("--permission-mode\nacceptEdits"));
        assert!(args.contains("--effort\nhigh"));
        assert!(args.contains("--session-id"));
    }

    #[test]
    fn subagent_steps_and_unknown_lines_are_ignored() {
        let mut pending = HashMap::new();
        let line = json!({
            "type": "assistant",
            "parent_tool_use_id": "t9",
            "message": {"content": [{"type": "tool_use", "id": "x", "name": "Bash", "input": {}}]}
        });
        assert!(parse_line(&line, &mut pending).is_empty());
        assert!(parse_line(&json!({"type": "stream_event"}), &mut pending).is_empty());
        let failed = parse_line(
            &json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
            &mut pending,
        );
        assert!(matches!(
            failed.last(),
            Some(HarnessEvent::TurnCompleted { completed: false })
        ));
        let _ = PathBuf::new();
    }
}
