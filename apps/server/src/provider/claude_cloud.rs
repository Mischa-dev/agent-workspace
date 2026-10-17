//! Claude Code in the cloud (C3): a run in one of your Claude cloud
//! environments, from the same Launch button as Codex Cloud. Each turn is a
//! Claude Managed Agents session with the run's repository mounted at the
//! run's base commit. The agent writes its changes as a patch to the
//! session's outputs, and the patch is committed on the run branch here,
//! ready to publish like any run.
//!
//! The account's API key is kept in the secret store and only ever sent to
//! the Claude API. The repository token goes in the session's resource, and
//! Anthropic's git proxy keeps it out of the container, so the agent never
//! sees it.

use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemDetail, ItemKind,
        LoginKind, LoginStart, ProfileHealth, RateLimitSummary, RecoveredTurn, RequestKind,
        RequestOption, TokenUsage,
    },
    claude_code::claude_models,
    host::{HostCell, host_of},
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry, RunLocation,
        RunOptionSupport, VersionRange,
    },
    run_config::RunConfig,
    slots::{RunSlot, RunSlots},
};
use crate::{config::AppConfig, secrets::SecretValue};

pub const CLAUDE_CLOUD_KIND: &str = "claude_cloud";
const BETA: &str = "managed-agents-2026-04-01";
const API_VERSION: &str = "2023-06-01";
const MOUNT: &str = "/workspace/repo";
const PATCH_NAME: &str = "changes.patch";
const MAX_PATCH_BYTES: usize = 20 * 1024 * 1024;

pub fn claude_cloud_manifest() -> AdapterManifest {
    AdapterManifest {
        kind: CLAUDE_CLOUD_KIND.to_owned(),
        display_name: "Claude Code cloud".to_owned(),
        description: "Claude in one of your Claude cloud environments, with the repository mounted. The result comes back to this thread's worktree, ready to publish as a pull request.".to_owned(),
        versions: VersionRange {
            minimum: "2026.4.1".to_owned(),
            below: None,
        },
        branch_prefix: "claude-cloud/run-".to_owned(),
        runs_on: RunLocation::Cloud,
        features: AdapterFeatures {
            streaming: false,
            commands: true,
            file_changes: true,
            approvals: true,
            questions: false,
            resume: true,
            interrupt: true,
            cloud: true,
            multiple_attempts: false,
            concurrent_runs: true,
            model_listing: false,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 3,
            max_per_account: 8,
        },
        login_methods: vec![LoginMethod {
            kind: LoginKind::ApiKey,
            label: "Paste an Anthropic API key from the Claude Console. It's kept encrypted and only sent to the Claude API.".to_owned(),
        }],
        run_options: RunOptionSupport {
            sandbox_modes: Vec::new(),
            approval_policies: vec!["auto".to_owned(), "on_request".to_owned()],
            max_attempts: 1,
        },
        options_schema: json!({
            "type": "object",
            "properties": {
                "environmentId": {
                    "type": "string",
                    "title": "Claude cloud environment",
                    "description": "The environment's ID (env_…), as the Claude Console shows it.",
                    "maxLength": 200
                },
                "instructions": {
                    "type": "string",
                    "maxLength": 8000,
                    "description": "Added to the agent's system prompt."
                }
            },
            "additionalProperties": false
        }),
        default_models: claude_models(),
    }
}

#[derive(Clone)]
struct Api {
    client: reqwest::Client,
    base: String,
}

impl Api {
    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        key: &SecretValue,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("x-api-key", key.expose())
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", BETA)
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        key: &SecretValue,
        body: Option<Value>,
    ) -> Result<Value, HarnessError> {
        let mut request = self.request(method, path, key);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(HarnessError::InvalidConfiguration);
        }
        if !status.is_success() {
            tracing::debug!(status = status.as_u16(), "Claude API request refused");
            return Err(HarnessError::InvalidResponse);
        }
        response.json().await.map_err(transport)
    }

    async fn bytes(&self, path: &str, key: &SecretValue) -> Result<Vec<u8>, HarnessError> {
        let response = self
            .request(reqwest::Method::GET, path, key)
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            return Err(HarnessError::InvalidResponse);
        }
        let bytes = response.bytes().await.map_err(transport)?;
        if bytes.len() > MAX_PATCH_BYTES {
            return Err(HarnessError::InvalidResponse);
        }
        Ok(bytes.to_vec())
    }
}

fn transport(error: impl std::error::Error + Send + Sync + 'static) -> HarnessError {
    HarnessError::Transport(Box::new(error))
}

pub struct ClaudeCloudAdapter {
    manifest: AdapterManifest,
    api: Api,
    poll_interval: Duration,
    slots: RunSlots,
    host: HostCell,
    /// Agents made for an account, by model and instructions.
    agents: Arc<Mutex<HashMap<(Uuid, String), String>>>,
}

impl ClaudeCloudAdapter {
    pub fn new(config: &AppConfig, host: HostCell) -> Result<Self, HarnessError> {
        Self::with_api(
            config.anthropic_api_url.clone(),
            config.codex_cloud_poll_interval,
            host,
        )
    }

    pub fn with_api(
        base: String,
        poll_interval: Duration,
        host: HostCell,
    ) -> Result<Self, HarnessError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| HarnessError::InvalidConfiguration)?;
        Ok(Self {
            manifest: claude_cloud_manifest(),
            api: Api { client, base },
            poll_interval,
            slots: RunSlots::new(usize::from(u16::MAX)),
            host,
            agents: Arc::default(),
        })
    }

    fn session(&self, account: AccountHandle, slot: Option<RunSlot>) -> CloudSession {
        CloudSession {
            account,
            api: self.api.clone(),
            poll_interval: self.poll_interval,
            host: Arc::clone(&self.host),
            agents: Arc::clone(&self.agents),
            _slot: slot,
            events: None,
            follower: None,
            key: None,
            current: None,
        }
    }
}

impl HarnessAdapter for ClaudeCloudAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
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
        _account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        Box::pin(async { Some(ProfileHealth::default()) })
    }

    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.slots.stop_account(account_id);
        Box::pin(async {})
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

struct CloudSession {
    account: AccountHandle,
    api: Api,
    poll_interval: Duration,
    host: HostCell,
    agents: Arc<Mutex<HashMap<(Uuid, String), String>>>,
    _slot: Option<RunSlot>,
    events: Option<mpsc::Receiver<HarnessEvent>>,
    follower: Option<tokio::task::JoinHandle<()>>,
    key: Option<SecretValue>,
    /// The Managed Agents session the current turn runs in.
    current: Option<String>,
}

impl Drop for CloudSession {
    fn drop(&mut self) {
        if let Some(follower) = self.follower.take() {
            follower.abort();
        }
    }
}

impl CloudSession {
    async fn key(&mut self) -> Result<SecretValue, HarnessError> {
        if let Some(key) = &self.key {
            return Ok(SecretValue::new(key.expose()));
        }
        let key = host_of(&self.host)?
            .account_secret(self.account.id)
            .await?
            .ok_or(HarnessError::InvalidConfiguration)?;
        self.key = Some(SecretValue::new(key.expose()));
        Ok(key)
    }

    async fn agent(&mut self, config: &RunConfig) -> Result<String, HarnessError> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "claude-opus-5-5".to_owned());
        let instructions = config
            .options
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let policy = if config.approval_policy.as_deref() == Some("on_request") {
            "always_ask"
        } else {
            "auto"
        };
        let cache_key = (
            self.account.id,
            format!("{model}\u{0}{policy}\u{0}{instructions}"),
        );
        if let Some(agent) = lock(&self.agents).get(&cache_key).cloned() {
            return Ok(agent);
        }
        let key = self.key().await?;
        let mut body = json!({
            "name": "Agent Workspace",
            "model": model,
            "tools": [{
                "type": "agent_toolset_20260401",
                "default_config": { "permission_policy": { "type": policy } }
            }]
        });
        if !instructions.trim().is_empty() {
            body["system"] = json!(instructions);
        }
        let agent = self
            .api
            .json(reqwest::Method::POST, "/v1/agents", &key, Some(body))
            .await?
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(HarnessError::MissingResult("an agent id"))?;
        lock(&self.agents).insert(cache_key, agent.clone());
        Ok(agent)
    }

    fn follow(&mut self, session_id: String, cwd: PathBuf, key: SecretValue) {
        if let Some(follower) = self.follower.take() {
            follower.abort();
        }
        let (sender, receiver) = mpsc::channel(256);
        self.events = Some(receiver);
        let api = self.api.clone();
        let poll = self.poll_interval;
        self.follower = Some(tokio::spawn(async move {
            follow_session(api, key, session_id, cwd, poll, sender).await;
        }));
    }
}

async fn follow_session(
    api: Api,
    key: SecretValue,
    session_id: String,
    cwd: PathBuf,
    poll: Duration,
    sender: mpsc::Sender<HarnessEvent>,
) {
    let mut page: Option<String> = None;
    let mut seen = std::collections::HashSet::<String>::new();
    let mut tools = HashMap::<String, (ItemKind, ItemDetail)>::new();
    let mut message = String::new();
    loop {
        let path = match &page {
            Some(page) => format!("/v1/sessions/{session_id}/events?page={page}"),
            None => format!("/v1/sessions/{session_id}/events"),
        };
        let listing = match api.json(reqwest::Method::GET, &path, &key, None).await {
            Ok(listing) => listing,
            Err(_) => {
                tokio::time::sleep(poll).await;
                continue;
            }
        };
        for event in listing
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = event
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !id.is_empty() && !seen.insert(id.clone()) {
                continue;
            }
            match event_outcome(event, &mut tools, &mut message) {
                Outcome::Events(events) => {
                    for event in events {
                        if sender.send(event).await.is_err() {
                            return;
                        }
                    }
                }
                Outcome::Finished(completed) => {
                    let completed =
                        completed && bring_back(&api, &key, &session_id, &cwd).await.is_ok();
                    if !message.is_empty() {
                        let _ = sender
                            .send(HarnessEvent::MessageDelta(message.clone()))
                            .await;
                    }
                    let _ = sender.send(HarnessEvent::TurnCompleted { completed }).await;
                    return;
                }
            }
        }
        if let Some(next) = listing.get("next_page").and_then(Value::as_str) {
            page = Some(next.to_owned());
            continue;
        }
        tokio::time::sleep(poll).await;
    }
}

enum Outcome {
    Events(Vec<HarnessEvent>),
    Finished(bool),
}

fn event_outcome(
    event: &Value,
    tools: &mut HashMap<String, (ItemKind, ItemDetail)>,
    message: &mut String,
) -> Outcome {
    let id = event.get("id").and_then(Value::as_str).unwrap_or_default();
    let mut events = Vec::new();
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "agent.message" => {
            let text = event
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            if !text.trim().is_empty() {
                events.push(HarnessEvent::Note(text.clone()));
                // The last thing the agent says is the run's reply.
                *message = text;
            }
        }
        "agent.tool_use" | "agent.mcp_tool_use" => {
            let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
            let input = event.get("input").cloned().unwrap_or(Value::Null);
            let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
            let (kind, detail) = match name {
                "bash" => (
                    ItemKind::CommandExecution,
                    ItemDetail {
                        command: text("command"),
                        ..ItemDetail::default()
                    },
                ),
                "write" | "edit" => (
                    ItemKind::FileChange,
                    ItemDetail {
                        tool: Some(name.to_owned()),
                        files: text("file_path")
                            .or_else(|| text("path"))
                            .map(|path| {
                                let path = path
                                    .strip_prefix(&format!("{MOUNT}/"))
                                    .map(str::to_owned)
                                    .unwrap_or(path);
                                vec![(path, Some("modified".to_owned()))]
                            })
                            .unwrap_or_default(),
                        ..ItemDetail::default()
                    },
                ),
                _ => (
                    ItemKind::Other,
                    ItemDetail {
                        tool: Some(name.to_owned()),
                        ..ItemDetail::default()
                    },
                ),
            };
            tools.insert(id.to_owned(), (kind, detail.clone()));
            events.push(HarnessEvent::ItemStarted(HarnessItem {
                key: format!("claude-cloud:{id}"),
                kind,
                detail,
            }));
            if event.get("evaluated_permission").and_then(Value::as_str) == Some("ask") {
                let what = text("command")
                    .or_else(|| text("file_path"))
                    .unwrap_or_default();
                let summary = if what.is_empty() {
                    format!("Allow Claude to use {name}?")
                } else {
                    format!("Allow Claude to use {name}: {what}?")
                };
                events.push(HarnessEvent::Request(HarnessRequest {
                    kind: RequestKind::Approval,
                    durable_id: format!("claude-cloud-approval-{id}"),
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
                    token: json!({ "toolUseId": id }),
                }));
            }
        }
        "agent.tool_result" | "agent.mcp_tool_result" => {
            let tool_id = event
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some((kind, mut detail)) = tools.remove(tool_id) {
                detail.failed = event
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if kind == ItemKind::CommandExecution {
                    detail.output = event
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|block| block.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        });
                }
                events.push(HarnessEvent::ItemCompleted(HarnessItem {
                    key: format!("claude-cloud:{tool_id}"),
                    kind,
                    detail,
                }));
            }
        }
        "span.model_request_end" => {
            if let Some(usage) = event.get("model_usage") {
                let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
                events.push(HarnessEvent::TokensUsed(TokenUsage {
                    model: event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    input_tokens: count("input_tokens") + count("cache_creation_input_tokens"),
                    output_tokens: count("output_tokens"),
                    cached_input_tokens: count("cache_read_input_tokens"),
                    reported_cost_usd: None,
                }));
            }
        }
        "session.status_idle" => {
            let reason = event
                .pointer("/stop_reason/type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match reason {
                "end_turn" => return Outcome::Finished(true),
                "requires_action" => {}
                _ => return Outcome::Finished(false),
            }
        }
        "session.status_terminated" | "session.error" => return Outcome::Finished(false),
        _ => {}
    }
    Outcome::Events(events)
}

/// Downloads the session's patch and commits it on the run branch.
async fn bring_back(
    api: &Api,
    key: &SecretValue,
    session_id: &str,
    cwd: &Path,
) -> Result<(), HarnessError> {
    let files = api
        .json(
            reqwest::Method::GET,
            &format!("/v1/files?scope_id={session_id}"),
            key,
            None,
        )
        .await?;
    let Some(file_id) = files
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|file| file.get("filename").and_then(Value::as_str) == Some(PATCH_NAME))
        .and_then(|file| file.get("id").and_then(Value::as_str))
        .map(str::to_owned)
    else {
        // Nothing changed.
        return Ok(());
    };
    let patch = api
        .bytes(&format!("/v1/files/{file_id}/content"), key)
        .await?;
    if patch.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let cwd = cwd.to_path_buf();
    let session_id = session_id.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::git_runtime::apply_result_and_commit(&cwd, &patch, &session_id, "Claude Code cloud")
    })
    .await
    .map_err(|_| HarnessError::InvalidResponse)?
    .map(|_| ())
    .map_err(|_| HarnessError::MissingResult("an applicable patch"))
}

fn base_commit(cwd: &Path) -> Result<String, HarnessError> {
    let output = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "HEAD"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .map_err(transport)?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success()
        || sha.len() != 40
        || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(HarnessError::MissingResult("the worktree's commit"));
    }
    Ok(sha)
}

fn task_prompt(prompt: &str, base: &str) -> String {
    format!(
        "{prompt}\n\n---\nThe repository is checked out at {MOUNT} on commit {base}. Work there. \
         When you're done, save your changes as one patch, including new files: \
         `cd {MOUNT} && git add -A && git diff --cached --binary {base} > /mnt/session/outputs/{PATCH_NAME}`. \
         Don't push and don't open a pull request; the patch is brought back and published from Agent Workspace."
    )
}

impl HarnessSession for CloudSession {
    fn account_id(&self) -> Uuid {
        self.account.id
    }

    fn is_stopping(&self) -> bool {
        false
    }

    fn start_thread<'a>(
        &'a mut self,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        // Each turn is its own session; the thread is only ours.
        Box::pin(async { Ok(format!("claude-cloud-{}", Uuid::now_v7())) })
    }

    fn resume_thread<'a>(
        &'a mut self,
        _thread_id: &'a str,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn start_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        cwd: &'a Path,
        prompt: &'a str,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            let environment = config
                .options
                .get("environmentId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or(HarnessError::InvalidConfiguration)?
                .to_owned();
            let key = self.key().await?;
            let repository = host_of(&self.host)?.repository_access(cwd).await?;
            let base = base_commit(cwd)?;
            let agent = self.agent(config).await?;
            let session = self
                .api
                .json(
                    reqwest::Method::POST,
                    "/v1/sessions",
                    &key,
                    Some(json!({
                        "agent": agent,
                        "environment_id": environment,
                        "title": crate::run_log::cap(prompt.lines().next().unwrap_or("Agent Workspace run"), 120),
                        "resources": [{
                            "type": "github_repository",
                            "url": repository.url,
                            "mount_path": MOUNT,
                            "authorization_token": repository.token.expose(),
                            "checkout": { "type": "commit", "sha": base }
                        }]
                    })),
                )
                .await?;
            let session_id = session
                .get("id")
                .and_then(Value::as_str)
                .ok_or(HarnessError::MissingResult("a session id"))?
                .to_owned();
            self.api
                .json(
                    reqwest::Method::POST,
                    &format!("/v1/sessions/{session_id}/events"),
                    &key,
                    Some(json!({
                        "events": [{
                            "type": "user.message",
                            "content": [{ "type": "text", "text": task_prompt(prompt, &base) }]
                        }]
                    })),
                )
                .await?;
            self.current = Some(session_id.clone());
            self.follow(session_id.clone(), cwd.to_path_buf(), key);
            Ok(session_id)
        })
    }

    fn follow_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        turn_id: &'a str,
        cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            let key = self.key().await?;
            self.current = Some(turn_id.to_owned());
            self.follow(turn_id.to_owned(), cwd.to_path_buf(), key);
            Ok(())
        })
    }

    fn turn_link(&self, turn_id: &str) -> Option<String> {
        Some(format!(
            "https://platform.claude.com/workspaces/default/sessions/{turn_id}"
        ))
    }

    fn interrupt_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            if let Some(follower) = self.follower.take() {
                follower.abort();
            }
            self.events = None;
            let key = self.key().await?;
            self.api
                .json(
                    reqwest::Method::POST,
                    &format!("/v1/sessions/{turn_id}/events"),
                    &key,
                    Some(json!({ "events": [{ "type": "user.interrupt" }] })),
                )
                .await
                .map(|_| ())
        })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(async move {
            let Some(events) = self.events.as_mut() else {
                return std::future::pending().await;
            };
            match events.recv().await {
                Some(event) => Ok(event),
                None => std::future::pending().await,
            }
        })
    }

    fn answer(&mut self, request: HarnessRequest, answer: HarnessAnswer) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            let Some(tool_use_id) = request.token.get("toolUseId").and_then(Value::as_str) else {
                return Err(HarnessError::InvalidResponse);
            };
            let Some(turn) = self.current.clone() else {
                return Ok(());
            };
            let key = self.key().await?;
            let result = if matches!(answer, HarnessAnswer::Approve) {
                "allow"
            } else {
                "deny"
            };
            self.api
                .json(
                    reqwest::Method::POST,
                    &format!("/v1/sessions/{turn}/events"),
                    &key,
                    Some(json!({ "events": [{
                        "type": "user.tool_confirmation",
                        "tool_use_id": tool_use_id,
                        "result": result
                    }] })),
                )
                .await
                .map(|_| ())
        })
    }

    fn read_latest_turn<'a>(&'a mut self, _thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        Box::pin(async {
            Ok(RecoveredTurn {
                id: None,
                status: "inProgress".to_owned(),
                final_message: None,
            })
        })
    }

    fn read_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, RecoveredTurn> {
        Box::pin(async move {
            let key = self.key().await?;
            let session = self
                .api
                .json(
                    reqwest::Method::GET,
                    &format!("/v1/sessions/{turn_id}"),
                    &key,
                    None,
                )
                .await?;
            let status = match session.get("status").and_then(Value::as_str) {
                Some("terminated") => "failed",
                // A running or idle session is followed again; idle ends
                // the turn as soon as its events are read.
                _ => "inProgress",
            };
            Ok(RecoveredTurn {
                id: Some(turn_id.to_owned()),
                status: status.to_owned(),
                final_message: None,
            })
        })
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary> {
        Box::pin(async move {
            let stored = match self.host.get() {
                Some(host) => host.account_secret(self.account.id).await?.is_some(),
                None => false,
            };
            Ok(AccountSummary {
                authenticated: stored,
                account_type: stored.then(|| "api_key".to_owned()),
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
            if !key.starts_with("sk-ant-")
                || !(20..=400).contains(&key.len())
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(HarnessError::InvalidResponse);
            }
            host_of(&self.host)?
                .store_account_secret(self.account.id, &SecretValue::new(key))
                .await?;
            self.key = None;
            Ok(LoginStart::ApiKey)
        })
    }

    fn cancel_login<'a>(&'a mut self, _login_id: &'a str) -> HarnessFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&mut self) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            self.key = None;
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
        Box::pin(async { Ok(claude_models()) })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if let Some(follower) = self.follower.take() {
                follower.abort();
            }
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::{
        Json, Router,
        extract::{Path as UrlPath, State},
        http::HeaderMap,
        routing::{get, post},
    };

    use super::*;
    use crate::provider::{
        contract_tests::adapter_contract_with,
        host::{HarnessHost, RepositoryAccess},
    };

    const FAKE_KEY: &str = "sk-ant-api03-fakefakefakefakefakefakefakefake";
    const FAKE_TOKEN: &str = "ghs_faketokenfaketokenfaketoken";
    const PATCH: &str = "diff --git a/NOTES.md b/NOTES.md\nnew file mode 100644\n--- /dev/null\n+++ b/NOTES.md\n@@ -0,0 +1 @@\n+hi\n";

    /// A stand-in for the Managed Agents API: one session that runs a
    /// command, asks before writing a file, and leaves a patch.
    #[derive(Default)]
    struct Fake {
        requests: Mutex<Vec<(String, Value)>>,
        confirmed: AtomicBool,
        sessions: AtomicUsize,
        wrong_key: AtomicBool,
    }

    type Shared = Arc<Fake>;

    fn record(fake: &Fake, headers: &HeaderMap, what: String, body: Value) {
        if headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            != Some(FAKE_KEY)
            || headers.get("anthropic-beta").is_none()
        {
            fake.wrong_key.store(true, Ordering::SeqCst);
        }
        lock(&fake.requests).push((what, body));
    }

    async fn create_agent(
        State(fake): State<Shared>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        record(&fake, &headers, "agent".to_owned(), body);
        Json(json!({ "id": "agent_1" }))
    }

    async fn create_session(
        State(fake): State<Shared>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        record(&fake, &headers, "session".to_owned(), body);
        let number = fake.sessions.fetch_add(1, Ordering::SeqCst);
        Json(json!({ "id": format!("sesn_{number}") }))
    }

    async fn send_events(
        State(fake): State<Shared>,
        UrlPath(id): UrlPath<String>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if body.pointer("/events/0/type").and_then(Value::as_str) == Some("user.tool_confirmation")
        {
            fake.confirmed.store(true, Ordering::SeqCst);
        }
        record(&fake, &headers, format!("events:{id}"), body);
        Json(json!({}))
    }

    async fn list_events(State(fake): State<Shared>) -> Json<Value> {
        let mut data = vec![
            json!({"id": "e1", "type": "agent.message", "content": [{"type": "text", "text": "Running the tests."}]}),
            json!({"id": "e2", "type": "agent.tool_use", "name": "bash", "input": {"command": "cargo test"}}),
            json!({"id": "e3", "type": "agent.tool_result", "tool_use_id": "e2", "content": [{"type": "text", "text": "test result: ok. 3 passed; 0 failed"}]}),
            json!({"id": "e4", "type": "agent.tool_use", "name": "write", "input": {"file_path": "/workspace/repo/NOTES.md", "content": "hi"}, "evaluated_permission": "ask"}),
        ];
        if fake.confirmed.load(Ordering::SeqCst) {
            data.extend([
                json!({"id": "e5", "type": "agent.tool_result", "tool_use_id": "e4", "content": [{"type": "text", "text": "written"}]}),
                json!({"id": "e6", "type": "span.model_request_end", "model": "claude-sonnet-5-5", "model_usage": {"input_tokens": 10, "output_tokens": 20, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 5}}),
                json!({"id": "e7", "type": "agent.message", "content": [{"type": "text", "text": "Wrote NOTES.md."}]}),
                json!({"id": "e8", "type": "session.status_idle", "stop_reason": {"type": "end_turn"}}),
            ]);
        } else {
            data.push(json!({"id": "e4b", "type": "session.status_idle", "stop_reason": {"type": "requires_action"}}));
        }
        Json(json!({ "data": data, "next_page": null }))
    }

    async fn list_files() -> Json<Value> {
        Json(json!({ "data": [{ "id": "file_1", "filename": PATCH_NAME }] }))
    }

    async fn file_content() -> &'static str {
        PATCH
    }

    async fn serve(fake: Shared) -> String {
        let app = Router::new()
            .route("/v1/agents", post(create_agent))
            .route("/v1/sessions", post(create_session))
            .route(
                "/v1/sessions/{id}/events",
                post(send_events).get(list_events),
            )
            .route("/v1/files", get(list_files))
            .route("/v1/files/{id}/content", get(file_content))
            .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test step");
        let address = listener.local_addr().expect("test step");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}")
    }

    struct FakeHost {
        key: Mutex<Option<String>>,
    }

    impl HarnessHost for FakeHost {
        fn account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>> {
            let key = lock(&self.key).clone();
            Box::pin(async move { Ok(key.map(SecretValue::new)) })
        }

        fn store_account_secret<'a>(
            &'a self,
            _account_id: Uuid,
            value: &'a SecretValue,
        ) -> HarnessFuture<'a, ()> {
            *lock(&self.key) = Some(value.expose().to_owned());
            Box::pin(async { Ok(()) })
        }

        fn delete_account_secret(&self, _account_id: Uuid) -> HarnessFuture<'_, ()> {
            *lock(&self.key) = None;
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
        ) -> HarnessFuture<'a, RepositoryAccess> {
            Box::pin(async {
                Ok(RepositoryAccess {
                    url: "https://github.com/example/repo".to_owned(),
                    token: SecretValue::new(FAKE_TOKEN),
                })
            })
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    async fn setup() -> (tempfile::TempDir, ClaudeCloudAdapter, AccountHandle, Shared) {
        let root = tempfile::tempdir().expect("test step");
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("test step");
        git(&worktree, &["init", "--quiet"]);
        std::fs::write(worktree.join("README.md"), "repo\n").expect("test step");
        git(&worktree, &["add", "-A"]);
        git(&worktree, &["commit", "--quiet", "-m", "base"]);
        let fake = Shared::default();
        let base = serve(Arc::clone(&fake)).await;
        let host = Arc::new(FakeHost {
            key: Mutex::new(None),
        });
        let cell = HostCell::default();
        let _ = cell.set(host as Arc<dyn HarnessHost>);
        let adapter =
            ClaudeCloudAdapter::with_api(base, Duration::from_millis(20), cell).expect("test step");
        let account = AccountHandle {
            id: Uuid::now_v7(),
            home: root.path().join("home"),
            max_concurrent_runs: 2,
        };
        (root, adapter, account, fake)
    }

    fn launch() -> RunConfig {
        let mut config = RunConfig::default();
        config
            .options
            .insert("environmentId".to_owned(), json!("env_test"));
        config
    }

    #[tokio::test]
    async fn claude_cloud_keeps_the_adapter_contract() {
        let (root, adapter, account, _) = setup().await;
        let mut control = adapter
            .acquire_control(account.clone())
            .await
            .expect("test step");
        control
            .login_with_api_key(FAKE_KEY)
            .await
            .expect("test step");
        drop(control);
        adapter_contract_with(
            &adapter,
            account,
            &root.path().join("worktree"),
            launch(),
            true,
        )
        .await;
    }

    #[tokio::test]
    async fn a_cloud_session_becomes_neutral_events_and_its_patch_is_committed() {
        let (root, adapter, account, fake) = setup().await;
        let worktree = root.path().join("worktree");
        let mut control = adapter
            .acquire_control(account.clone())
            .await
            .expect("test step");
        assert!(
            !control
                .read_account()
                .await
                .expect("test step")
                .authenticated
        );
        assert!(control.login_with_api_key("not a key").await.is_err());
        control
            .login_with_api_key(FAKE_KEY)
            .await
            .expect("test step");
        assert!(
            control
                .read_account()
                .await
                .expect("test step")
                .authenticated
        );
        drop(control);

        let mut config = launch();
        config.model = Some("claude-sonnet-5-5".to_owned());
        config.approval_policy = Some("on_request".to_owned());
        let mut run = adapter
            .acquire_run(account.clone())
            .await
            .expect("test step");
        let thread = run
            .start_thread(&worktree, &config)
            .await
            .expect("test step");
        let turn = run
            .start_turn(&thread, &worktree, "write notes", &config)
            .await
            .expect("test step");
        assert_eq!(turn, "sesn_0");
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), run.next_event())
                .await
                .expect("events keep coming")
                .expect("test step");
            if let HarnessEvent::Request(request) = event {
                assert!(request.summary.contains("NOTES.md"));
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
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemCompleted(item)
                if item.kind == ItemKind::CommandExecution
                    && item.detail.output.as_deref().is_some_and(|out| out.contains("3 passed"))
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::ItemStarted(item)
                if item.kind == ItemKind::FileChange && item.detail.files[0].0 == "NOTES.md"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::TokensUsed(usage) if usage.input_tokens == 15 && usage.cached_input_tokens == 30
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HarnessEvent::MessageDelta(text) if text == "Wrote NOTES.md."
        )));
        // The patch is committed on the run branch.
        assert_eq!(
            std::fs::read_to_string(worktree.join("NOTES.md")).expect("test step"),
            "hi\n"
        );
        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["log", "-1", "--format=%B"])
            .output()
            .expect("test step");
        assert!(String::from_utf8_lossy(&log.stdout).contains("Harness-Result: sesn_0"));

        assert!(
            !fake.wrong_key.load(Ordering::SeqCst),
            "every call carries the key and beta"
        );
        let requests = lock(&fake.requests).clone();
        let agent = &requests
            .iter()
            .find(|(what, _)| what == "agent")
            .expect("test step")
            .1;
        assert_eq!(agent["model"], "claude-sonnet-5-5");
        assert_eq!(
            agent["tools"][0]["default_config"]["permission_policy"]["type"],
            "always_ask"
        );
        let session = &requests
            .iter()
            .find(|(what, _)| what == "session")
            .expect("test step")
            .1;
        assert_eq!(session["environment_id"], "env_test");
        assert_eq!(session["resources"][0]["authorization_token"], FAKE_TOKEN);
        assert_eq!(session["resources"][0]["checkout"]["type"], "commit");
        // The key and token never go in the prompt the agent sees.
        let message = requests
            .iter()
            .find(|(what, body)| {
                what.starts_with("events:")
                    && body.pointer("/events/0/type") == Some(&json!("user.message"))
            })
            .expect("test step");
        let text = message.1.to_string();
        assert!(!text.contains(FAKE_KEY) && !text.contains(FAKE_TOKEN));
        assert!(text.contains("changes.patch"));
        assert!(requests.iter().any(|(_, body)| {
            body.pointer("/events/0/type") == Some(&json!("user.tool_confirmation"))
                && body.pointer("/events/0/tool_use_id") == Some(&json!("e4"))
                && body.pointer("/events/0/result") == Some(&json!("allow"))
        }));
    }

    #[tokio::test]
    async fn a_run_without_an_environment_is_refused() {
        let (root, adapter, account, _) = setup().await;
        let mut control = adapter
            .acquire_control(account.clone())
            .await
            .expect("test step");
        control
            .login_with_api_key(FAKE_KEY)
            .await
            .expect("test step");
        drop(control);
        let worktree = root.path().join("worktree");
        let mut run = adapter.acquire_run(account).await.expect("test step");
        let config = RunConfig::default();
        let thread = run
            .start_thread(&worktree, &config)
            .await
            .expect("test step");
        assert!(matches!(
            run.start_turn(&thread, &worktree, "go", &config).await,
            Err(HarnessError::InvalidConfiguration)
        ));
    }
}
