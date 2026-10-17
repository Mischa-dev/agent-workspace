//! Command-line harnesses (H7): a program a workspace registers (X2) that
//! the server runs once per turn in the run's worktree, speaking the
//! `aw-harness/1` JSON-lines protocol of docs/harness-guide.md.
//!
//! The program must sit inside a directory the operator lists in
//! `AGENT_WORKSPACE_COMMAND_HARNESS_DIRS`, so a workspace member can't run
//! anything else on the host. Its arguments come from a template and go to
//! the program as an array (no shell). Its environment holds only a path, a
//! home and a locale; the secrets it is allowed arrive once as a JSON object
//! on file descriptor 3.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemDetail, ItemKind,
        LoginKind, LoginStart, ProfileHealth, RateLimitSummary, RecoveredTurn, RequestKind,
        RequestOption, TestCounts, TokenUsage, UsageReport, UsageWindow,
    },
    host::{HostCell, host_of},
    line_process::{Line, LineProcess},
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry, RunLocation,
        RunOptionSupport, VersionRange,
    },
    run_config::RunConfig,
    secret_fd,
    slots::{RunSlot, RunSlots},
};
use crate::secrets::SecretValue;

pub const PROTOCOL: u32 = 1;
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_CHARS: usize = 2_000;
const MAX_MODELS: usize = 50;

/// What a registration says about its harness, as people write it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredManifest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub options: Option<Value>,
    #[serde(default)]
    pub features: RegisteredFeatures,
    #[serde(default)]
    pub max_concurrent_runs: Option<u16>,
    #[serde(default)]
    pub api_key: bool,
    /// Accepted for the guide's examples; the registration's own name wins.
    #[serde(default)]
    #[allow(dead_code)]
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredFeatures {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub questions: bool,
    #[serde(default)]
    pub commands: bool,
}

/// Turns a registration into the manifest the rest of the server reads.
/// `runs_on` is the host for commands and the cloud for connected runners
/// (their turns never wait on this host's limit).
pub fn build_manifest(
    kind: &str,
    name: &str,
    manifest: &Value,
    runs_on: RunLocation,
) -> Result<AdapterManifest, String> {
    let registered: RegisteredManifest =
        serde_json::from_value(manifest.clone()).map_err(|error| format!("manifest: {error}"))?;
    if registered.models.len() > MAX_MODELS {
        return Err(format!("manifest: at most {MAX_MODELS} models"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for model in &registered.models {
        let id_ok = (1..=120).contains(&model.id.len())
            && model.id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
            });
        if !id_ok || !seen.insert(model.id.clone()) {
            return Err(format!(
                "manifest: model id {:?} is invalid or repeated",
                model.id
            ));
        }
        if model.display_name.trim().is_empty() || model.display_name.chars().count() > 120 {
            return Err(format!("manifest: model {} needs a display name", model.id));
        }
        if model
            .prices
            .as_ref()
            .is_some_and(|prices| prices.input_per_million < 0.0 || prices.output_per_million < 0.0)
        {
            return Err(format!("manifest: model {} has a negative price", model.id));
        }
    }
    let options = registered.options.clone().unwrap_or_else(
        || json!({ "type": "object", "properties": {}, "additionalProperties": false }),
    );
    let properties = options
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("manifest: options needs properties")?;
    for (option, schema) in properties {
        let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "string" | "boolean" | "integer" | "number")
            || option.is_empty()
            || option.len() > 64
            || !option
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!(
                "manifest: option {option} has an unsupported shape"
            ));
        }
    }
    let mut options = options;
    if let Some(object) = options.as_object_mut() {
        object.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    let concurrency = registered.max_concurrent_runs.unwrap_or(1).clamp(1, 16);
    let mut models = registered.models;
    if !models.iter().any(|model| model.is_default)
        && let Some(first) = models.first_mut()
    {
        first.is_default = true;
    }
    let description = registered
        .description
        .map(|text| text.trim().chars().take(500).collect::<String>())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| match runs_on {
            RunLocation::LocalHost => {
                "A command-line harness registered in this workspace.".to_owned()
            }
            RunLocation::Cloud => "A connected runner registered in this workspace.".to_owned(),
        });
    Ok(AdapterManifest {
        kind: kind.to_owned(),
        display_name: name.to_owned(),
        description,
        versions: VersionRange {
            minimum: format!("{PROTOCOL}.0.0"),
            below: Some(format!("{}.0.0", PROTOCOL + 1)),
        },
        branch_prefix: format!("{}/run-", kind.replace('_', "-")),
        runs_on,
        features: AdapterFeatures {
            streaming: false,
            commands: registered.features.commands,
            file_changes: true,
            approvals: registered.features.approvals,
            questions: registered.features.questions,
            resume: true,
            interrupt: true,
            cloud: runs_on == RunLocation::Cloud,
            multiple_attempts: false,
            concurrent_runs: concurrency > 1,
            model_listing: false,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 1,
            max_per_account: concurrency,
        },
        login_methods: if registered.api_key {
            vec![LoginMethod {
                kind: LoginKind::ApiKey,
                label: "Paste the key this harness signs in with. It reaches the harness as API_KEY on its secrets descriptor, never its environment.".to_owned(),
            }]
        } else {
            Vec::new()
        },
        run_options: RunOptionSupport {
            sandbox_modes: vec!["workspace_write".to_owned()],
            approval_policies: vec!["on_request".to_owned()],
            max_attempts: 1,
        },
        options_schema: options,
        default_models: models,
    })
}

/// Checks an argument template: a program inside an allowed directory, and
/// placeholders the expander knows.
pub fn validate_command(command: &[String], allowed_dirs: &[PathBuf]) -> Result<PathBuf, String> {
    if command.is_empty() || command.len() > MAX_ARGUMENTS {
        return Err(format!(
            "command: give the program and at most {} arguments",
            MAX_ARGUMENTS - 1
        ));
    }
    for argument in command {
        if argument.chars().count() > MAX_ARGUMENT_CHARS || argument.contains('\0') {
            return Err("command: an argument is too long".to_owned());
        }
        let mut rest = argument.as_str();
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}') else {
                break;
            };
            let name = &rest[start + 1..start + end];
            let known = matches!(name, "model" | "reasoning" | "speed" | "worktree")
                || name.strip_prefix("option:").is_some_and(|option| {
                    !option.is_empty()
                        && option
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                });
            if !known {
                return Err(format!("command: unknown placeholder {{{name}}}"));
            }
            rest = &rest[start + end + 1..];
        }
    }
    allowed_program(Path::new(&command[0]), allowed_dirs)
}

/// The program's real path, when it sits inside an allowed directory.
fn allowed_program(program: &Path, allowed_dirs: &[PathBuf]) -> Result<PathBuf, String> {
    if allowed_dirs.is_empty() {
        return Err("command: this server doesn't allow command-line harnesses (AGENT_WORKSPACE_COMMAND_HARNESS_DIRS is unset)".to_owned());
    }
    if !program.is_absolute() {
        return Err("command: the program must be an absolute path".to_owned());
    }
    let real = std::fs::canonicalize(program)
        .map_err(|_| "command: the program doesn't exist on this server".to_owned())?;
    let inside = allowed_dirs.iter().any(|dir| {
        std::fs::canonicalize(dir).is_ok_and(|dir| real.starts_with(&dir) && real != dir)
    });
    if !inside {
        return Err("command: the program must be inside a directory listed in AGENT_WORKSPACE_COMMAND_HARNESS_DIRS".to_owned());
    }
    Ok(real)
}

/// Expands an argument template for one turn. An element that was only a
/// placeholder and comes out empty is dropped.
pub fn expand(template: &[String], config: &RunConfig, worktree: &Path) -> Vec<String> {
    let value = |name: &str| -> String {
        match name {
            "model" => config.model.clone().unwrap_or_default(),
            "reasoning" => config.reasoning.clone().unwrap_or_default(),
            "speed" => config.speed.clone().unwrap_or_default(),
            "worktree" => worktree.to_string_lossy().into_owned(),
            other => other
                .strip_prefix("option:")
                .and_then(|option| config.options.get(option))
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                })
                .unwrap_or_default(),
        }
    };
    template
        .iter()
        .skip(1)
        .filter_map(|argument| {
            let mut out = String::new();
            let mut rest = argument.as_str();
            let mut only_placeholder = true;
            while let Some(start) = rest.find('{') {
                let Some(end) = rest[start..].find('}') else {
                    break;
                };
                if start > 0 {
                    only_placeholder = false;
                }
                out.push_str(&rest[..start]);
                out.push_str(&value(&rest[start + 1..start + end]));
                rest = &rest[start + end + 1..];
            }
            if !rest.is_empty() {
                only_placeholder = false;
            }
            out.push_str(rest);
            (!(only_placeholder && out.is_empty() && argument.contains('{'))).then_some(out)
        })
        .collect()
}

/// A registered command harness.
pub struct CommandSpec {
    pub manifest: AdapterManifest,
    pub command: Vec<String>,
    pub secret_names: Vec<String>,
    pub allowed_dirs: Vec<PathBuf>,
}

pub struct CommandAdapter {
    spec: Arc<CommandSpec>,
    slots: RunSlots,
    host: HostCell,
}

impl CommandAdapter {
    pub fn new(spec: CommandSpec, host_limit: usize, host: HostCell) -> Self {
        Self {
            spec: Arc::new(spec),
            slots: RunSlots::new(host_limit),
            host,
        }
    }

    fn session(&self, account: AccountHandle, slot: Option<RunSlot>) -> CommandSession {
        CommandSession {
            stop_generation: self.slots.stop_generation(account.id),
            account,
            spec: Arc::clone(&self.spec),
            slots: self.slots.clone(),
            host: Arc::clone(&self.host),
            _slot: slot,
            threads: HashMap::new(),
            turn: None,
        }
    }
}

impl HarnessAdapter for CommandAdapter {
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

struct Turn {
    process: LineProcess,
    turn_id: String,
    commands: HashMap<String, ItemDetail>,
    queue: VecDeque<HarnessEvent>,
    message: String,
    finished: bool,
}

struct CommandSession {
    account: AccountHandle,
    spec: Arc<CommandSpec>,
    slots: RunSlots,
    stop_generation: u64,
    host: HostCell,
    _slot: Option<RunSlot>,
    /// Thread ids, and whether each has had a turn.
    threads: HashMap<String, bool>,
    turn: Option<Turn>,
}

impl CommandSession {
    async fn secrets(&self) -> Result<Vec<(String, SecretValue)>, HarnessError> {
        let Some(host) = self.host.get() else {
            return Ok(Vec::new());
        };
        let mut secrets = host
            .harness_secrets(self.account.id, &self.spec.secret_names)
            .await?;
        if let Some(key) = host.account_secret(self.account.id).await? {
            secrets.push(("API_KEY".to_owned(), key));
        }
        Ok(secrets)
    }
}

impl HarnessSession for CommandSession {
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
            let thread_id = format!("cli-thread-{}", Uuid::now_v7());
            self.threads.insert(thread_id.clone(), false);
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
            if !thread_id.starts_with("cli-thread-") {
                return Err(HarnessError::MissingResult("thread"));
            }
            self.threads.insert(thread_id.to_owned(), true);
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
                .threads
                .get(thread_id)
                .ok_or(HarnessError::MissingResult("thread"))?;
            // Checked again at every turn: the directory list or the file
            // may have changed since the harness was registered.
            let program =
                allowed_program(Path::new(&self.spec.command[0]), &self.spec.allowed_dirs)
                    .map_err(|_| HarnessError::InvalidConfiguration)?;
            let mut command = tokio::process::Command::new(&program);
            command
                .args(expand(&self.spec.command, config, cwd))
                .current_dir(cwd)
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("HOME", &self.account.home)
                .env("LANG", "C.UTF-8")
                .env("LC_ALL", "C.UTF-8")
                .env("AGENT_WORKSPACE_HARNESS_PROTOCOL", PROTOCOL.to_string());
            let secrets = self.secrets().await?;
            if !secrets.is_empty() {
                command.env(
                    "AGENT_WORKSPACE_SECRETS_FD",
                    secret_fd::SECRET_FD.to_string(),
                );
                let bytes = secret_fd::secrets_json(&secrets);
                secret_fd::attach(&mut command, secret_fd::secret_file(&bytes)?)?;
            }
            let mut process = LineProcess::spawn(command)?;
            let turn_id = format!("cli-turn-{}", Uuid::now_v7());
            process
                .send(&json!({
                    "type": "turn",
                    "protocol": PROTOCOL,
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "prompt": prompt,
                    "model": config.model,
                    "reasoning": config.reasoning,
                    "speed": config.speed,
                    "options": config.options,
                    "resume": resume,
                }))
                .await?;
            self.threads.insert(thread_id.to_owned(), true);
            if let Some(mut previous) = self.turn.take() {
                previous.process.kill().await;
            }
            self.turn = Some(Turn {
                process,
                turn_id: turn_id.clone(),
                commands: HashMap::new(),
                queue: VecDeque::new(),
                message: String::new(),
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
                    let _ = turn.process.send(&json!({ "type": "stop" })).await;
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                        while !matches!(turn.process.next().await, Line::Closed) {}
                    })
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
                match turn.process.next().await {
                    Line::Json(value) => {
                        let events = parse_line(&value, turn);
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
            let Some(id) = request.token.get("id").and_then(Value::as_str) else {
                return Err(HarnessError::InvalidResponse);
            };
            let line = match answer {
                HarnessAnswer::Approve => {
                    json!({ "type": "answer", "requestId": id, "decision": "approve" })
                }
                HarnessAnswer::Deny => {
                    json!({ "type": "answer", "requestId": id, "decision": "deny" })
                }
                HarnessAnswer::Text(text) => {
                    json!({ "type": "answer", "requestId": id, "text": text })
                }
            };
            turn.process.send(&line).await
        })
    }

    fn read_latest_turn<'a>(&'a mut self, _thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
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
        Box::pin(async move {
            if let Some(mut turn) = self.turn.take() {
                turn.process.kill().await;
            }
        })
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Item keys are the harness's ids, kept apart per turn.
fn key(turn: &Turn, kind: &str, id: &str) -> String {
    let id: String = id.chars().take(120).collect();
    format!("{}:{kind}:{id}", turn.turn_id)
}

fn parse_line(value: &Value, turn: &mut Turn) -> Vec<HarnessEvent> {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "message" => {
            if let Some(text) = text(value, "text") {
                if !turn.message.is_empty() {
                    turn.message.push_str("\n\n");
                }
                turn.message.push_str(&text);
            }
            Vec::new()
        }
        "log" => text(value, "text")
            .map(HarnessEvent::Note)
            .into_iter()
            .collect(),
        "command" => {
            let id = text(value, "id").unwrap_or_else(|| Uuid::now_v7().to_string());
            let item_key = key(turn, "command", &id);
            if text(value, "status").as_deref() == Some("completed") {
                let mut detail = turn.commands.remove(&id).unwrap_or_default();
                if detail.command.is_none() {
                    detail.command = text(value, "command");
                }
                detail.exit_code = value.get("exitCode").and_then(Value::as_i64);
                detail.output = text(value, "output");
                detail.failed = detail.exit_code.is_some_and(|code| code != 0);
                vec![HarnessEvent::ItemCompleted(HarnessItem {
                    key: item_key,
                    kind: ItemKind::CommandExecution,
                    detail,
                })]
            } else {
                let detail = ItemDetail {
                    command: text(value, "command"),
                    ..ItemDetail::default()
                };
                turn.commands.insert(id, detail.clone());
                vec![HarnessEvent::ItemStarted(HarnessItem {
                    key: item_key,
                    kind: ItemKind::CommandExecution,
                    detail,
                })]
            }
        }
        "file" => {
            let Some(path) = text(value, "path") else {
                return Vec::new();
            };
            let change = match value.get("change").and_then(Value::as_str) {
                Some(change @ ("added" | "deleted" | "modified")) => change.to_owned(),
                _ => "modified".to_owned(),
            };
            let item = HarnessItem {
                key: key(turn, "file", &format!("{path}:{}", Uuid::now_v7())),
                kind: ItemKind::FileChange,
                detail: ItemDetail {
                    files: vec![(path, Some(change))],
                    ..ItemDetail::default()
                },
            };
            vec![HarnessEvent::ItemCompleted(item)]
        }
        "tests" => {
            let tests = TestCounts {
                passed: count(value, "passed") as u32,
                failed: count(value, "failed") as u32,
                skipped: count(value, "skipped") as u32,
            };
            vec![HarnessEvent::ItemCompleted(HarnessItem {
                key: key(turn, "tests", &Uuid::now_v7().to_string()),
                kind: ItemKind::CommandExecution,
                detail: ItemDetail {
                    command: text(value, "command"),
                    tests: Some(tests),
                    failed: tests.failed > 0,
                    ..ItemDetail::default()
                },
            })]
        }
        "approval" | "question" => {
            let Some(id) = text(value, "id") else {
                return Vec::new();
            };
            let summary = text(value, "summary").unwrap_or_else(|| "The harness asks.".to_owned());
            let question = kind == "question";
            vec![HarnessEvent::Request(HarnessRequest {
                kind: if question {
                    RequestKind::Question
                } else {
                    RequestKind::Approval
                },
                durable_id: key(turn, kind, &id),
                summary: crate::run_log::cap(&crate::run_log::redact(&summary), 1_000),
                options: if question {
                    Vec::new()
                } else {
                    vec![
                        RequestOption {
                            id: "approve".to_owned(),
                            label: "Allow".to_owned(),
                        },
                        RequestOption {
                            id: "deny".to_owned(),
                            label: "Deny".to_owned(),
                        },
                    ]
                },
                token: json!({ "id": id }),
            })]
        }
        "usage" => vec![HarnessEvent::TokensUsed(TokenUsage {
            model: text(value, "model"),
            input_tokens: count(value, "inputTokens"),
            output_tokens: count(value, "outputTokens"),
            cached_input_tokens: count(value, "cachedInputTokens"),
            reported_cost_usd: value.get("costUsd").and_then(Value::as_f64),
        })],
        "limits" => {
            let windows = value
                .get("windows")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(8)
                .filter_map(|window| {
                    let name = text(window, "key")?;
                    let key = format!("custom.{name}");
                    let valid = key.len() <= 64
                        && key.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        });
                    let used = window.get("usedPercent").and_then(Value::as_f64)?;
                    valid.then(|| UsageWindow {
                        key,
                        limit_name: Some(name),
                        used_percent: used.round().clamp(0.0, 100.0) as u16,
                        resets_at: window.get("resetsAt").and_then(Value::as_i64),
                        duration_minutes: window.get("durationMinutes").and_then(Value::as_i64),
                        limit_reached: used >= 100.0,
                    })
                })
                .collect::<Vec<_>>();
            if windows.is_empty() {
                return Vec::new();
            }
            vec![HarnessEvent::UsageUpdated(UsageReport {
                windows,
                reset_credits: None,
                plan: None,
            })]
        }
        "done" => {
            turn.finished = true;
            turn.process.close_input();
            let completed = text(value, "status").as_deref() == Some("completed");
            let mut message = std::mem::take(&mut turn.message);
            if let Some(extra) = text(value, "message") {
                if !message.is_empty() {
                    message.push_str("\n\n");
                }
                message.push_str(&extra);
            }
            let mut events = Vec::new();
            if !message.is_empty() {
                events.push(HarnessEvent::MessageDelta(message));
            }
            events.push(HarnessEvent::TurnCompleted { completed });
            events
        }
        _ => Vec::new(),
    }
}

/// The options a turn line carries, for tests.
/// A connected runner registered as a harness (X2): it shows in Settings
/// and the Launch dialog like any harness, but its work is handed to the
/// runner agent as a tag, so it never runs a turn here.
pub struct RunnerAdapter {
    manifest: AdapterManifest,
}

impl RunnerAdapter {
    pub fn new(manifest: AdapterManifest) -> Self {
        Self { manifest }
    }
}

impl HarnessAdapter for RunnerAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn acquire_run(&self, _account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async { Err(HarnessError::Unsupported("runs on a connected runner")) })
    }

    fn acquire_control(
        &self,
        _account: AccountHandle,
    ) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async { Err(HarnessError::Unsupported("accounts on a connected runner")) })
    }

    fn health(
        &self,
        _account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        Box::pin(async { None })
    }

    fn shutdown_account(&self, _account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

#[cfg(test)]
fn options(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::provider::contract_tests::adapter_contract;

    /// A 30-line shell harness speaking aw-harness/1: it reports a command,
    /// asks for approval, writes a file, reports usage and finishes. It
    /// records its environment and what fd 3 held.
    const SCRIPT: &str = r#"#!/bin/sh
env > "$HOME/env.txt"
printf '%s\n' "$@" > "$HOME/args.txt"
if [ -n "$AGENT_WORKSPACE_SECRETS_FD" ]; then cat /dev/fd/3 > "$HOME/secrets.json"; fi
read turn
echo '{"type":"log","text":"starting"}'
echo '{"type":"command","id":"c1","command":"make test","status":"started"}'
echo '{"type":"command","id":"c1","status":"completed","exitCode":0,"output":"test result: ok. 4 passed; 0 failed; 0 ignored"}'
echo '{"type":"approval","id":"a1","summary":"Write NOTES.md?"}'
read answer
case "$answer" in
  *approve*) echo note > NOTES.md; echo '{"type":"file","path":"NOTES.md","change":"added"}';;
esac
echo '{"type":"usage","model":"my-large","inputTokens":100,"outputTokens":50,"cachedInputTokens":10}'
echo '{"type":"message","text":"Wrote the notes."}'
echo '{"type":"done","status":"completed"}'
"#;

    struct Host;

    impl super::super::host::HarnessHost for Host {
        fn account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>> {
            Box::pin(async { Ok(Some(SecretValue::new("account-key-123456"))) })
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
            names: &'a [String],
        ) -> HarnessFuture<'a, Vec<(String, SecretValue)>> {
            Box::pin(async move {
                Ok(names
                    .iter()
                    .map(|name| (name.clone(), SecretValue::new(format!("value-of-{name}"))))
                    .collect())
            })
        }
        fn repository_access<'a>(
            &'a self,
            _: &'a Path,
        ) -> HarnessFuture<'a, super::super::host::RepositoryAccess> {
            Box::pin(async { Err(HarnessError::Unsupported("repositories")) })
        }
    }

    fn manifest_json() -> Value {
        json!({
            "models": [{"id": "my-large", "displayName": "Large",
                        "prices": {"currency": "USD", "inputPerMillion": 3.0, "outputPerMillion": 15.0}}],
            "options": {"type": "object", "properties": {"profile": {"type": "string", "enum": ["careful", "fast"]}}},
            "features": {"approvals": true, "commands": true},
            "maxConcurrentRuns": 2,
            "apiKey": true
        })
    }

    fn setup() -> (tempfile::TempDir, CommandAdapter, AccountHandle) {
        let root = tempfile::tempdir().expect("root");
        let dir = root.path().join("harnesses");
        std::fs::create_dir_all(&dir).expect("test step");
        let program = dir.join("script.sh");
        std::fs::write(&program, SCRIPT).expect("test step");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("test step");
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).expect("test step");
        let manifest = build_manifest(
            "x_script",
            "Script",
            &manifest_json(),
            RunLocation::LocalHost,
        )
        .expect("manifest");
        let command = vec![
            program.to_string_lossy().into_owned(),
            "--model={model}".to_owned(),
            "{reasoning}".to_owned(),
            "--profile".to_owned(),
            "{option:profile}".to_owned(),
        ];
        validate_command(&command, std::slice::from_ref(&dir)).expect("allowed");
        let cell = HostCell::default();
        let _ = cell.set(Arc::new(Host) as Arc<dyn super::super::host::HarnessHost>);
        let adapter = CommandAdapter::new(
            CommandSpec {
                manifest,
                command,
                secret_names: vec!["deploy_token".to_owned()],
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
        (root, adapter, account)
    }

    #[tokio::test]
    async fn command_harnesses_keep_the_adapter_contract() {
        let (root, adapter, account) = setup();
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("test step");
        adapter_contract(&adapter, account, &worktree).await;
    }

    #[tokio::test]
    async fn a_script_harness_takes_a_turn_and_gets_its_secrets_only_on_fd_3() {
        let (root, adapter, account) = setup();
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("test step");
        let config = RunConfig {
            model: Some("my-large".to_owned()),
            options: options(json!({"profile": "careful"})),
            ..RunConfig::default()
        };
        let mut run = adapter
            .acquire_run(account.clone())
            .await
            .expect("test step");
        let thread = run
            .start_thread(&worktree, &config)
            .await
            .expect("test step");
        run.start_turn(&thread, &worktree, "write notes", &config)
            .await
            .expect("test step");
        let mut events = Vec::new();
        loop {
            let event = run.next_event().await.expect("test step");
            if let HarnessEvent::Request(request) = event {
                assert_eq!(request.summary, "Write NOTES.md?");
                run.answer(request, HarnessAnswer::Approve)
                    .await
                    .expect("test step");
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
        assert!(worktree.join("NOTES.md").exists());
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item) if item.detail.exit_code == Some(0)
                && item.detail.command.as_deref() == Some("make test")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::MessageDelta(text) if text == "Wrote the notes."
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::TokensUsed(usage) if usage.input_tokens == 100
        )));
        let args = std::fs::read_to_string(account.home.join("args.txt")).expect("test step");
        assert_eq!(
            args, "--model=my-large\n--profile\ncareful\n",
            "the empty {{reasoning}} is dropped"
        );
        let env = std::fs::read_to_string(account.home.join("env.txt")).expect("test step");
        let secrets: Value = serde_json::from_str(
            &std::fs::read_to_string(account.home.join("secrets.json")).expect("test step"),
        )
        .expect("test step");
        assert_eq!(secrets["deploy_token"], "value-of-deploy_token");
        assert_eq!(secrets["API_KEY"], "account-key-123456");
        assert!(
            !env.contains("value-of-")
                && !env.contains("account-key")
                && !args.contains("value-of-")
        );
        assert!(env.contains("AGENT_WORKSPACE_SECRETS_FD=3"));
    }

    #[test]
    fn programs_outside_the_allowed_directories_are_refused() {
        let root = tempfile::tempdir().expect("test step");
        let allowed = root.path().join("allowed");
        std::fs::create_dir_all(&allowed).expect("test step");
        std::fs::write(allowed.join("ok"), "").expect("test step");
        std::fs::write(root.path().join("outside"), "").expect("test step");
        std::os::unix::fs::symlink(root.path().join("outside"), allowed.join("link"))
            .expect("test step");
        let ok = allowed.join("ok").to_string_lossy().into_owned();
        assert!(validate_command(std::slice::from_ref(&ok), std::slice::from_ref(&allowed)).is_ok());
        assert!(
            validate_command(std::slice::from_ref(&ok), &[]).is_err(),
            "off unless the operator allows it"
        );
        for program in [
            root.path().join("outside"),
            allowed.join("link"),
            allowed.join("../outside"),
            PathBuf::from("relative/ok"),
        ] {
            let program = program.to_string_lossy().into_owned();
            assert!(validate_command(&[program], std::slice::from_ref(&allowed)).is_err());
        }
        assert!(validate_command(&[ok, "{secret}".to_owned()], &[allowed]).is_err());
    }

    #[test]
    fn manifests_are_checked() {
        assert!(build_manifest("x_a", "A", &json!({"bogus": 1}), RunLocation::LocalHost).is_err());
        assert!(
            build_manifest(
                "x_a",
                "A",
                &json!({"options": {"type": "object", "properties": {"list": {"type": "array"}}}}),
                RunLocation::LocalHost
            )
            .is_err()
        );
        let manifest = build_manifest(
            "x_my_agent",
            "Mine",
            &manifest_json(),
            RunLocation::LocalHost,
        )
        .expect("test step");
        assert_eq!(manifest.branch_prefix, "x-my-agent/run-");
        assert!(manifest.default_models[0].is_default);
        assert_eq!(manifest.concurrency.max_per_account, 2);
    }
}
