use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use codex_bridge_spike::{BridgeConfig, BridgeError, CodexBridge};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    sync::{Mutex, Notify, OwnedMutexGuard},
    time::{sleep, timeout},
};
use uuid::Uuid;

use crate::{config::AppConfig, git_runtime::absolute_runtime_path};

const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const CIRCUIT_OPEN_DURATION: Duration = Duration::from_secs(30);
const MAX_RESTART_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct SupervisorRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    config: SupervisorConfig,
    profiles: Mutex<HashMap<Uuid, Arc<ProfileSupervisor>>>,
    shutting_down: AtomicBool,
    shutdown: Notify,
}

#[derive(Clone)]
struct SupervisorConfig {
    codex_binary: PathBuf,
    neutral_cwd: PathBuf,
    max_profiles: usize,
    max_line_bytes: usize,
    max_stderr_bytes: usize,
    max_buffered_events: usize,
    request_timeout: Duration,
    shutdown_timeout: Duration,
}

struct ProfileSupervisor {
    profile_id: Uuid,
    codex_home: PathBuf,
    config: SupervisorConfig,
    state: Arc<Mutex<SupervisorState>>,
}

struct SupervisorState {
    bridge: Option<CodexBridge>,
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
    circuit_open_until: Option<Instant>,
    process_starts: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileHealth {
    pub process_running: bool,
    pub consecutive_failures: u32,
    pub circuit_open: bool,
    pub process_starts: u64,
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("the Codex host concurrency limit is reached")]
    HostCapacity,
    #[error("the Codex supervisor is shutting down")]
    ShuttingDown,
    #[error("the Codex profile circuit is temporarily open")]
    CircuitOpen,
    #[error("the Codex profile configuration changed while its supervisor was active")]
    ProfileConfigurationChanged,
    #[error("the Codex App Server transport failed")]
    Transport(#[source] BridgeError),
    #[error("the Codex App Server response omitted {0}")]
    MissingResult(&'static str),
    #[error("the Codex App Server returned an invalid account response")]
    InvalidAccountResponse,
    #[error("the Codex supervisor configuration is invalid")]
    InvalidConfiguration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    CommandExecution,
    FileChange,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalKind {
    Approval,
    Question,
    Unsupported,
}

#[derive(Debug)]
pub struct ProviderRequest {
    id: Value,
    pub kind: ApprovalKind,
    question_ids: Vec<String>,
}

impl ProviderRequest {
    pub fn durable_id(&self) -> String {
        self.id.to_string()
    }
}

#[derive(Debug)]
pub enum ProviderEvent {
    AgentMessageDelta(String),
    ItemStarted(ItemKind),
    ItemCompleted(ItemKind),
    TurnCompleted { completed: bool },
    Request(ProviderRequest),
    Error,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredTurn {
    pub status: String,
    pub final_message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSummary {
    pub authenticated: bool,
    pub account_type: Option<String>,
    pub email: Option<String>,
}

pub struct ProfileLease {
    supervisor: Arc<ProfileSupervisor>,
    guard: OwnedMutexGuard<SupervisorState>,
}

impl SupervisorRegistry {
    pub fn new(config: &AppConfig) -> Result<Self, SupervisorError> {
        let codex_binary = absolute_runtime_path(&config.codex_binary)
            .map_err(|_| SupervisorError::InvalidConfiguration)?;
        let neutral_cwd = absolute_runtime_path(&config.data_root)
            .map_err(|_| SupervisorError::InvalidConfiguration)?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                config: SupervisorConfig {
                    codex_binary,
                    neutral_cwd,
                    max_profiles: config.codex_host_concurrency_limit,
                    max_line_bytes: config.codex_max_line_bytes,
                    max_stderr_bytes: config.codex_max_stderr_bytes,
                    max_buffered_events: config.codex_max_buffered_events,
                    request_timeout: config.codex_request_timeout,
                    shutdown_timeout: config.codex_shutdown_timeout,
                },
                profiles: Mutex::new(HashMap::new()),
                shutting_down: AtomicBool::new(false),
                shutdown: Notify::new(),
            }),
        })
    }

    pub async fn acquire(
        &self,
        profile_id: Uuid,
        codex_home: PathBuf,
    ) -> Result<ProfileLease, SupervisorError> {
        if self.is_shutting_down() {
            return Err(SupervisorError::ShuttingDown);
        }
        let supervisor = {
            let mut profiles = self.inner.profiles.lock().await;
            if let Some(existing) = profiles.get(&profile_id) {
                if existing.codex_home != codex_home {
                    return Err(SupervisorError::ProfileConfigurationChanged);
                }
                Arc::clone(existing)
            } else {
                if profiles.len() >= self.inner.config.max_profiles {
                    return Err(SupervisorError::HostCapacity);
                }
                let supervisor = Arc::new(ProfileSupervisor {
                    profile_id,
                    codex_home,
                    config: self.inner.config.clone(),
                    state: Arc::new(Mutex::new(SupervisorState {
                        bridge: None,
                        consecutive_failures: 0,
                        last_failure_at: None,
                        circuit_open_until: None,
                        process_starts: 0,
                    })),
                });
                profiles.insert(profile_id, Arc::clone(&supervisor));
                supervisor
            }
        };
        let mut guard = Arc::clone(&supervisor.state).lock_owned().await;
        ensure_started(&supervisor, &mut guard).await?;
        Ok(ProfileLease { supervisor, guard })
    }

    pub async fn health(&self, profile_id: Uuid) -> Option<ProfileHealth> {
        let supervisor = {
            let profiles = self.inner.profiles.lock().await;
            profiles.get(&profile_id).cloned()
        }?;
        let state = supervisor.state.lock().await;
        Some(health_snapshot(&state))
    }

    pub fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.load(Ordering::Acquire)
    }

    pub async fn shutdown_requested(&self) {
        if self.is_shutting_down() {
            return;
        }
        self.inner.shutdown.notified().await;
    }

    pub async fn shutdown_profile(&self, profile_id: Uuid) {
        let supervisor = self.inner.profiles.lock().await.remove(&profile_id);
        if let Some(supervisor) = supervisor {
            let mut state = supervisor.state.lock().await;
            shutdown_bridge(&mut state).await;
        }
    }

    pub async fn shutdown_all(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        self.inner.shutdown.notify_waiters();
        let supervisors = {
            let mut profiles = self.inner.profiles.lock().await;
            profiles.drain().map(|(_, value)| value).collect::<Vec<_>>()
        };
        for supervisor in supervisors {
            if let Ok(mut state) = timeout(
                supervisor.config.shutdown_timeout + Duration::from_secs(1),
                supervisor.state.lock(),
            )
            .await
            {
                shutdown_bridge(&mut state).await;
            } else {
                tracing::error!(
                    profile_id = %supervisor.profile_id,
                    "timed out waiting for the Codex profile lease during shutdown"
                );
            }
        }
    }
}

impl ProfileLease {
    pub fn profile_id(&self) -> Uuid {
        self.supervisor.profile_id
    }

    pub async fn start_thread(&mut self, cwd: &Path) -> Result<String, SupervisorError> {
        let result = self
            .request(
                "thread/start",
                json!({
                    "cwd": cwd,
                    "approvalPolicy": "on-request",
                    "sandbox": "workspace-write",
                    "ephemeral": false
                }),
            )
            .await?;
        result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(SupervisorError::MissingResult("thread id"))
    }

    pub async fn resume_thread(
        &mut self,
        thread_id: &str,
        cwd: &Path,
    ) -> Result<(), SupervisorError> {
        self.request("thread/resume", json!({"threadId": thread_id, "cwd": cwd}))
            .await?;
        Ok(())
    }

    pub async fn start_turn(
        &mut self,
        thread_id: &str,
        cwd: &Path,
        prompt: &str,
    ) -> Result<String, SupervisorError> {
        let result = self
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "cwd": cwd,
                    "input": [{"type": "text", "text": prompt}]
                }),
            )
            .await?;
        result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(SupervisorError::MissingResult("turn id"))
    }

    pub async fn interrupt_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), SupervisorError> {
        self.request(
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": turn_id}),
        )
        .await?;
        Ok(())
    }

    pub async fn read_latest_turn(
        &mut self,
        thread_id: &str,
    ) -> Result<RecoveredTurn, SupervisorError> {
        let history = self
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": true}),
            )
            .await?;
        let Some(latest_turn) = history
            .pointer("/thread/turns")
            .and_then(Value::as_array)
            .and_then(|turns| turns.last())
        else {
            return Ok(RecoveredTurn {
                status: "missing".to_owned(),
                final_message: None,
            });
        };
        let status = latest_turn
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let final_message = latest_turn
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().rev().find_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("agentMessage"))
                        .then(|| item.get("text").and_then(Value::as_str))
                        .flatten()
                })
            })
            .map(str::to_owned);
        Ok(RecoveredTurn {
            status,
            final_message,
        })
    }

    pub async fn read_account(&mut self) -> Result<AccountSummary, SupervisorError> {
        let account = self
            .request("account/read", json!({"refreshToken": false}))
            .await?;
        let account_value = account.get("account");
        let authenticated = account_value.is_some_and(|value| !value.is_null());
        let account_type = account_value
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let email = account_value
            .and_then(|value| value.get("email"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if authenticated && account_type.is_none() {
            return Err(SupervisorError::InvalidAccountResponse);
        }
        Ok(AccountSummary {
            authenticated,
            account_type,
            email,
        })
    }

    pub async fn next_event(&mut self) -> Result<ProviderEvent, SupervisorError> {
        let raw = self.next_raw_event().await?;
        if raw.get("id").is_some() {
            return if raw.get("method").is_some() {
                Ok(ProviderEvent::Request(parse_provider_request(raw)?))
            } else {
                Ok(ProviderEvent::Error)
            };
        }
        let method = raw
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(match method {
            "item/agentMessage/delta" => raw
                .pointer("/params/delta")
                .and_then(Value::as_str)
                .map(|delta| ProviderEvent::AgentMessageDelta(delta.to_owned()))
                .unwrap_or(ProviderEvent::Ignored),
            "item/started" => ProviderEvent::ItemStarted(item_kind(&raw)),
            "item/completed" => ProviderEvent::ItemCompleted(item_kind(&raw)),
            "turn/completed" => ProviderEvent::TurnCompleted {
                completed: raw.pointer("/params/turn/status").and_then(Value::as_str)
                    == Some("completed"),
            },
            "error" => ProviderEvent::Error,
            _ => ProviderEvent::Ignored,
        })
    }

    pub async fn answer_request(
        &mut self,
        request: ProviderRequest,
        approved: bool,
    ) -> Result<(), SupervisorError> {
        let result = match request.kind {
            ApprovalKind::Approval => {
                json!({"decision": if approved { "accept" } else { "decline" }})
            }
            ApprovalKind::Question => {
                let answers = request
                    .question_ids
                    .into_iter()
                    .map(|id| {
                        (
                            id,
                            json!({"answers": [if approved { "approved" } else { "denied" }]}),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({"answers": answers})
            }
            ApprovalKind::Unsupported => {
                let response = self
                    .bridge_mut()?
                    .respond_error(request.id, -32601, "unsupported provider request")
                    .await;
                return self.finish_transport_result(response).await;
            }
        };
        let response = self.bridge_mut()?.respond(request.id, result).await;
        self.finish_transport_result(response).await
    }

    pub async fn shutdown_process(&mut self) {
        shutdown_bridge(&mut self.guard).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, SupervisorError> {
        let result = self.bridge_mut()?.request(method, params).await;
        match result {
            Ok(value) => {
                record_success(&mut self.guard);
                Ok(value)
            }
            Err(error) => {
                self.fail_transport().await;
                Err(SupervisorError::Transport(error))
            }
        }
    }

    async fn next_raw_event(&mut self) -> Result<Value, SupervisorError> {
        let result = self.bridge_mut()?.next_raw_event().await;
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.fail_transport().await;
                Err(SupervisorError::Transport(error))
            }
        }
    }

    fn bridge_mut(&mut self) -> Result<&mut CodexBridge, SupervisorError> {
        self.guard
            .bridge
            .as_mut()
            .ok_or(SupervisorError::ShuttingDown)
    }

    async fn finish_transport_result(
        &mut self,
        result: Result<(), BridgeError>,
    ) -> Result<(), SupervisorError> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.fail_transport().await;
                Err(SupervisorError::Transport(error))
            }
        }
    }

    async fn fail_transport(&mut self) {
        shutdown_bridge(&mut self.guard).await;
        record_failure(&mut self.guard);
    }
}

async fn ensure_started(
    supervisor: &ProfileSupervisor,
    state: &mut SupervisorState,
) -> Result<(), SupervisorError> {
    if state.bridge.is_some() {
        return Ok(());
    }
    let now = Instant::now();
    if state
        .circuit_open_until
        .is_some_and(|circuit_open_until| circuit_open_until > now)
    {
        return Err(SupervisorError::CircuitOpen);
    }
    state.circuit_open_until = None;
    if let Some(last_failure_at) = state.last_failure_at {
        let backoff = restart_backoff(state.consecutive_failures);
        let elapsed = now.saturating_duration_since(last_failure_at);
        if elapsed < backoff {
            sleep(backoff - elapsed).await;
        }
    }

    let mut environment = BTreeMap::new();
    let binary_parent = supervisor
        .config
        .codex_binary
        .parent()
        .ok_or(SupervisorError::InvalidConfiguration)?;
    environment.insert(
        "PATH".to_owned(),
        format!("{}:/usr/local/bin:/usr/bin:/bin", binary_parent.display()),
    );
    environment.insert("LANG".to_owned(), "C.UTF-8".to_owned());
    environment.insert("LC_ALL".to_owned(), "C.UTF-8".to_owned());
    let bridge_config = BridgeConfig {
        codex_binary: supervisor.config.codex_binary.clone(),
        codex_home: supervisor.codex_home.clone(),
        cwd: supervisor.config.neutral_cwd.clone(),
        safe_environment: environment,
        max_line_bytes: supervisor.config.max_line_bytes,
        max_stderr_bytes: supervisor.config.max_stderr_bytes,
        max_buffered_events: supervisor.config.max_buffered_events,
        automatic_server_request_responses: false,
        request_timeout: supervisor.config.request_timeout,
        shutdown_timeout: supervisor.config.shutdown_timeout,
    };
    let mut bridge = match CodexBridge::spawn(bridge_config).await {
        Ok(bridge) => bridge,
        Err(error) => {
            record_failure(state);
            return Err(SupervisorError::Transport(error));
        }
    };
    if let Err(error) = bridge.initialize().await {
        bridge.shutdown().await;
        record_failure(state);
        return Err(SupervisorError::Transport(error));
    }
    state.bridge = Some(bridge);
    state.process_starts = state.process_starts.saturating_add(1);
    record_success(state);
    Ok(())
}

fn parse_provider_request(raw: Value) -> Result<ProviderRequest, SupervisorError> {
    let id = raw
        .get("id")
        .cloned()
        .ok_or(SupervisorError::MissingResult("provider request id"))?;
    let method = raw
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = match method {
        "item/tool/requestUserInput" => ApprovalKind::Question,
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/permissions/requestApproval" => ApprovalKind::Approval,
        _ => ApprovalKind::Unsupported,
    };
    let question_ids = if kind == ApprovalKind::Question {
        raw.pointer("/params/questions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|question| question.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    Ok(ProviderRequest {
        id,
        kind,
        question_ids,
    })
}

fn item_kind(raw: &Value) -> ItemKind {
    match raw.pointer("/params/item/type").and_then(Value::as_str) {
        Some("commandExecution") => ItemKind::CommandExecution,
        Some("fileChange") => ItemKind::FileChange,
        _ => ItemKind::Other,
    }
}

async fn shutdown_bridge(state: &mut SupervisorState) {
    if let Some(bridge) = state.bridge.take() {
        bridge.shutdown().await;
    }
}

fn record_success(state: &mut SupervisorState) {
    state.consecutive_failures = 0;
    state.last_failure_at = None;
    state.circuit_open_until = None;
}

fn record_failure(state: &mut SupervisorState) {
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_failure_at = Some(Instant::now());
    if state.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        state.circuit_open_until = Some(Instant::now() + CIRCUIT_OPEN_DURATION);
    }
}

fn restart_backoff(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(6);
    Duration::from_millis(100_u64.saturating_mul(1_u64 << exponent)).min(MAX_RESTART_BACKOFF)
}

fn health_snapshot(state: &SupervisorState) -> ProfileHealth {
    ProfileHealth {
        process_running: state.bridge.is_some(),
        consecutive_failures: state.consecutive_failures,
        circuit_open: state
            .circuit_open_until
            .is_some_and(|circuit_open_until| circuit_open_until > Instant::now()),
        process_starts: state.process_starts,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalKind, ItemKind, ProviderEvent, SupervisorError, SupervisorRegistry,
        SupervisorState, health_snapshot, item_kind, parse_provider_request, record_failure,
        restart_backoff,
    };
    use crate::config::AppConfig;
    use serde_json::json;
    use std::{collections::BTreeMap, time::Duration};

    #[test]
    fn restart_backoff_is_bounded() {
        assert_eq!(restart_backoff(1), Duration::from_millis(100));
        assert_eq!(restart_backoff(3), Duration::from_millis(400));
        assert_eq!(restart_backoff(100), Duration::from_secs(5));
    }

    #[test]
    fn repeated_failures_open_the_profile_circuit() {
        let mut state = SupervisorState {
            bridge: None,
            consecutive_failures: 0,
            last_failure_at: None,
            circuit_open_until: None,
            process_starts: 0,
        };
        for _ in 0..5 {
            record_failure(&mut state);
        }
        let health = health_snapshot(&state);
        assert!(health.circuit_open);
        assert_eq!(health.consecutive_failures, 5);
    }

    #[test]
    fn provider_requests_are_reduced_to_bounded_adapter_types() {
        let request = parse_provider_request(json!({
            "id": 17,
            "method": "item/tool/requestUserInput",
            "params": {"questions": [{"id": "choice"}]}
        }))
        .expect("provider request");
        assert_eq!(request.kind, ApprovalKind::Question);
        assert_eq!(request.question_ids, vec!["choice"]);

        let unsupported = parse_provider_request(json!({
            "id": 18,
            "method": "account/chatgptAuthTokens/refresh",
            "params": {}
        }))
        .expect("unsupported provider request");
        assert_eq!(unsupported.kind, ApprovalKind::Unsupported);

        let item = json!({"params": {"item": {"type": "commandExecution"}}});
        assert_eq!(item_kind(&item), ItemKind::CommandExecution);
    }

    #[test]
    fn provider_event_type_does_not_expose_raw_json() {
        let event = ProviderEvent::TurnCompleted { completed: true };
        assert!(matches!(
            event,
            ProviderEvent::TurnCompleted { completed: true }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn one_profile_reuses_one_initialized_process_and_enforces_host_capacity() {
        use std::{fs, os::unix::fs::PermissionsExt};
        use uuid::Uuid;

        let temporary = tempfile::tempdir().expect("temporary supervisor root");
        let data_root = temporary.path().join("data");
        let profile_home = data_root.join("profiles/one");
        let first_worktree = data_root.join("worktrees/one");
        let second_worktree = data_root.join("worktrees/two");
        for directory in [&data_root, &profile_home, &first_worktree, &second_worktree] {
            fs::create_dir_all(directory).expect("create test runtime directory");
        }
        let evidence = temporary.path().join("process-evidence");
        let binary = temporary.path().join("fake-codex");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s|%s|%s\n' "$$" "$PWD" "$CODEX_HOME" > '{}'
while IFS= read -r line; do
  if [[ "$line" != *'"id"'* ]]; then
    continue
  fi
  request_id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  if [[ "$line" == *'"method":"initialize"'* ]]; then
    printf '{{"id":%s,"result":{{"server":"fake"}}}}\n' "$request_id"
  elif [[ "$line" == *'"method":"thread/start"'* ]]; then
    printf '{{"method":"item/started","params":{{"item":{{"type":"commandExecution"}}}}}}\n'
    printf '{{"id":%s,"result":{{"thread":{{"id":"thread-%s"}}}}}}\n' "$request_id" "$request_id"
  elif [[ "$line" == *'"method":"turn/start"'* ]]; then
    printf '{{"id":%s,"result":{{"turn":{{"id":"turn-%s"}}}}}}\n' "$request_id" "$request_id"
  else
    printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
  fi
done
"#,
            evidence.display()
        );
        fs::write(&binary, script).expect("write fake Codex binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("make fake Codex executable");

        let config = AppConfig::from_values(&BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/database".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
                data_root.to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
                binary.to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
                "2".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS".to_owned(),
                "1".to_owned(),
            ),
        ]))
        .expect("supervisor test config");
        let registry = SupervisorRegistry::new(&config).expect("supervisor registry");
        let profile_id = Uuid::now_v7();

        {
            let mut first = registry
                .acquire(profile_id, profile_home.clone())
                .await
                .expect("first profile lease");
            assert_eq!(
                first
                    .start_thread(&first_worktree)
                    .await
                    .expect("start first thread"),
                "thread-2"
            );
            assert!(matches!(
                first.next_event().await.expect("first buffered event"),
                ProviderEvent::ItemStarted(ItemKind::CommandExecution)
            ));
            assert_eq!(
                first
                    .start_turn("thread-2", &first_worktree, "first")
                    .await
                    .expect("start first turn"),
                "turn-3"
            );
        }
        {
            let mut second = registry
                .acquire(profile_id, profile_home.clone())
                .await
                .expect("second profile lease");
            assert_eq!(
                second
                    .start_thread(&second_worktree)
                    .await
                    .expect("start second thread"),
                "thread-4"
            );
            assert!(matches!(
                second.next_event().await.expect("second buffered event"),
                ProviderEvent::ItemStarted(ItemKind::CommandExecution)
            ));
            assert_eq!(
                second
                    .start_turn("thread-4", &second_worktree, "second")
                    .await
                    .expect("start second turn"),
                "turn-5"
            );
        }

        let health = registry.health(profile_id).await.expect("profile health");
        assert!(health.process_running);
        assert_eq!(health.process_starts, 1);
        assert!(matches!(
            registry
                .acquire(Uuid::now_v7(), data_root.join("profiles/two"))
                .await,
            Err(SupervisorError::HostCapacity)
        ));
        let evidence = fs::read_to_string(evidence).expect("process evidence");
        let mut fields = evidence.trim().split('|');
        assert!(fields.next().is_some_and(|pid| !pid.is_empty()));
        let expected_data_root = data_root.to_string_lossy();
        let expected_profile_home = profile_home.to_string_lossy();
        assert_eq!(fields.next(), Some(expected_data_root.as_ref()));
        assert_eq!(fields.next(), Some(expected_profile_home.as_ref()));

        registry.shutdown_all().await;
        assert!(registry.is_shutting_down());
    }
}
