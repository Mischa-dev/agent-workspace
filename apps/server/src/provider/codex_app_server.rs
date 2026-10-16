//! The Codex App Server adapter. Everything Codex-specific (its JSON-RPC
//! methods and payloads, its process and CODEX_HOME) stays in this module.
//!
//! Each account gets a small pool of `codex app-server` processes sharing its
//! private home. A run holds one process for its whole turn, so an account
//! can run as many turns at once as its limit allows (H8). Account pages and
//! model listing borrow an idle process, or start one that never counts
//! against run capacity, and idle processes are shut down after a while, so
//! looking at accounts can never block a run (F8).

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use codex_bridge_spike::{BridgeConfig, BridgeError, CodexBridge};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{Mutex, OwnedMutexGuard},
    time::{sleep, timeout},
};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemKind, LoginKind,
        LoginStart, ProfileHealth, RateLimitSummary, RateLimitWindow, RecoveredTurn, RequestKind,
        RequestOption, ResetCredit, ResetCredits, ResetOutcome, UsageReport, UsageWindow,
    },
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry, RunLocation,
        RunOptionSupport, SpeedOption, VersionRange,
    },
    registry::ShutdownSignal,
    run_config::RunConfig,
};
use crate::{config::AppConfig, git_runtime::absolute_runtime_path};

pub const CODEX_KIND: &str = "codex_app_server";
/// The speed that sends no service tier.
const STANDARD_SPEED: &str = "standard";
const MAX_CONSECUTIVE_FAILURES: u32 = 5;
const CIRCUIT_OPEN_DURATION: Duration = Duration::from_secs(30);
const MAX_RESTART_BACKOFF: Duration = Duration::from_secs(5);
/// An unused process is shut down after this long.
const IDLE_PROCESS_TTL: Duration = Duration::from_secs(120);
/// Processes not running a turn (idle, or serving an account page) on top
/// of the host run limit.
const MAX_IDLE_PROCESSES: usize = 4;
/// A process that started a browser or device sign-in stays up this long so
/// the sign-in can finish.
const LOGIN_PIN: Duration = Duration::from_secs(10 * 60);
const MAX_MODEL_PAGES: usize = 5;

pub fn codex_manifest() -> AdapterManifest {
    AdapterManifest {
        kind: CODEX_KIND.to_owned(),
        display_name: "Codex".to_owned(),
        description:
            "OpenAI Codex through its App Server, running in an isolated worktree on this host."
                .to_owned(),
        versions: VersionRange {
            minimum: "0.146.0".to_owned(),
            below: Some("1.0.0".to_owned()),
        },
        branch_prefix: "codex/run-".to_owned(),
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
            model_listing: true,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 2,
            max_per_account: 4,
        },
        login_methods: vec![
            LoginMethod {
                kind: LoginKind::Browser,
                label: "Sign in with ChatGPT".to_owned(),
            },
            LoginMethod {
                kind: LoginKind::DeviceCode,
                label: "Use a device code".to_owned(),
            },
            LoginMethod {
                kind: LoginKind::ApiKey,
                label: "OpenAI API key".to_owned(),
            },
        ],
        run_options: RunOptionSupport {
            sandbox_modes: vec!["workspace_write".to_owned(), "read_only".to_owned()],
            approval_policies: vec!["on_request".to_owned(), "untrusted".to_owned()],
            max_attempts: 1,
        },
        options_schema: json!({
            "type": "object",
            "properties": {
                "reasoningSummary": {
                    "type": "string",
                    "title": "Reasoning summary",
                    "enum": ["auto", "concise", "detailed", "none"]
                }
            },
            "additionalProperties": false
        }),
        default_models: Vec::new(),
    }
}

pub struct CodexAdapter {
    pool: Arc<Pool>,
    manifest: AdapterManifest,
}

struct Pool {
    config: SupervisorConfig,
    shutdown: Arc<ShutdownSignal>,
    accounts: StdMutex<HashMap<Uuid, Arc<AccountProcesses>>>,
    running: AtomicUsize,
}

#[derive(Clone)]
struct SupervisorConfig {
    codex_binary: PathBuf,
    neutral_cwd: PathBuf,
    host_run_limit: usize,
    max_idle_processes: usize,
    idle_ttl: Duration,
    max_line_bytes: usize,
    max_stderr_bytes: usize,
    max_buffered_events: usize,
    request_timeout: Duration,
    shutdown_timeout: Duration,
}

struct AccountProcesses {
    home: PathBuf,
    stopping: Arc<AtomicBool>,
    slots: StdMutex<Vec<Arc<Slot>>>,
}

struct Slot {
    state: Arc<Mutex<SupervisorState>>,
    run_leased: AtomicBool,
    last_released: StdMutex<Instant>,
    pinned_until: StdMutex<Option<Instant>>,
}

struct SupervisorState {
    bridge: Option<CodexBridge>,
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
    circuit_open_until: Option<Instant>,
    process_starts: u64,
}

impl SupervisorState {
    fn new() -> Self {
        Self {
            bridge: None,
            consecutive_failures: 0,
            last_failure_at: None,
            circuit_open_until: None,
            process_starts: 0,
        }
    }
}

impl Slot {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(SupervisorState::new())),
            run_leased: AtomicBool::new(false),
            last_released: StdMutex::new(Instant::now()),
            pinned_until: StdMutex::new(None),
        })
    }

    fn pinned(&self, now: Instant) -> bool {
        lock(&self.pinned_until).is_some_and(|until| until > now)
    }

    fn idle_expired(&self, now: Instant, ttl: Duration) -> bool {
        !self.pinned(now) && now.saturating_duration_since(*lock(&self.last_released)) >= ttl
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Purpose {
    Run,
    Control,
}

impl CodexAdapter {
    pub fn new(config: &AppConfig, shutdown: Arc<ShutdownSignal>) -> Result<Self, HarnessError> {
        let codex_binary = absolute_runtime_path(&config.codex_binary)
            .map_err(|_| HarnessError::InvalidConfiguration)?;
        let neutral_cwd = absolute_runtime_path(&config.data_root)
            .map_err(|_| HarnessError::InvalidConfiguration)?;
        Ok(Self {
            pool: Arc::new(Pool {
                config: SupervisorConfig {
                    codex_binary,
                    neutral_cwd,
                    host_run_limit: config.codex_host_concurrency_limit,
                    max_idle_processes: MAX_IDLE_PROCESSES,
                    idle_ttl: IDLE_PROCESS_TTL,
                    max_line_bytes: config.codex_max_line_bytes,
                    max_stderr_bytes: config.codex_max_stderr_bytes,
                    max_buffered_events: config.codex_max_buffered_events,
                    request_timeout: config.codex_request_timeout,
                    shutdown_timeout: config.codex_shutdown_timeout,
                },
                shutdown,
                accounts: StdMutex::new(HashMap::new()),
                running: AtomicUsize::new(0),
            }),
            manifest: codex_manifest(),
        })
    }

    #[cfg(test)]
    fn with_idle_ttl(mut self, idle_ttl: Duration) -> Self {
        if let Some(pool) = Arc::get_mut(&mut self.pool) {
            pool.config.idle_ttl = idle_ttl;
        }
        self
    }

    async fn acquire(
        &self,
        account: AccountHandle,
        purpose: Purpose,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        if self.pool.shutdown.is_shutting_down() {
            return Err(HarnessError::ShuttingDown);
        }
        let (processes, slot, guard, evicted) = self.pool.select(&account, purpose)?;
        let mut session = CodexSession {
            pool: Arc::clone(&self.pool),
            account_id: account.id,
            processes,
            slot,
            guard,
            run: purpose == Purpose::Run,
        };
        for mut evicted in evicted {
            shutdown_bridge(&mut evicted).await;
        }
        ensure_started(
            &self.pool.config,
            &session.processes.home,
            &mut session.guard,
        )
        .await?;
        Ok(Box::new(session))
    }
}

type Selection = (
    Arc<AccountProcesses>,
    Arc<Slot>,
    OwnedMutexGuard<SupervisorState>,
    Vec<OwnedMutexGuard<SupervisorState>>,
);

impl Pool {
    /// Picks (or makes) a process for the account without waiting: an idle
    /// one when there is one, else a new one if the limits allow it.
    fn select(&self, account: &AccountHandle, purpose: Purpose) -> Result<Selection, HarnessError> {
        let now = Instant::now();
        let mut accounts = lock(&self.accounts);
        let mut evicted = self.reap_idle(&mut accounts, now);
        let processes = Arc::clone(accounts.entry(account.id).or_insert_with(|| {
            Arc::new(AccountProcesses {
                home: account.home.clone(),
                stopping: Arc::new(AtomicBool::new(false)),
                slots: StdMutex::new(Vec::new()),
            })
        }));
        if processes.home != account.home {
            return Err(HarnessError::ConfigurationChanged);
        }
        if processes.stopping.load(Ordering::Acquire) {
            return Err(HarnessError::ShuttingDown);
        }
        let mut slots = lock(&processes.slots);
        if purpose == Purpose::Run {
            if self.running.load(Ordering::Acquire) >= self.config.host_run_limit {
                return Err(HarnessError::HostCapacity);
            }
            let leased = slots
                .iter()
                .filter(|slot| slot.run_leased.load(Ordering::Acquire))
                .count();
            if leased >= usize::from(account.max_concurrent_runs.max(1)) {
                return Err(HarnessError::AccountCapacity);
            }
        }
        // A control session goes to the process holding a sign-in first, so
        // the sign-in can be checked or cancelled where it started; runs
        // leave that process free.
        let mut order = slots.iter().collect::<Vec<_>>();
        order.sort_by_key(|slot| slot.pinned(now) == (purpose == Purpose::Run));
        let reused = order.into_iter().find_map(|slot| {
            Arc::clone(&slot.state)
                .try_lock_owned()
                .ok()
                .map(|guard| (Arc::clone(slot), guard))
        });
        let (slot, guard) = match reused {
            Some(found) => found,
            None => {
                drop(slots);
                if purpose == Purpose::Control {
                    let idle = accounts
                        .values()
                        .flat_map(|account| lock(&account.slots).clone())
                        .filter(|slot| !slot.run_leased.load(Ordering::Acquire))
                        .count();
                    if idle >= self.config.max_idle_processes {
                        match evict_one_idle(&mut accounts, now) {
                            Some(guard) => evicted.push(guard),
                            None => return Err(HarnessError::ControlCapacity),
                        }
                    }
                }
                let slot = Slot::new();
                let guard = Arc::clone(&slot.state)
                    .try_lock_owned()
                    .map_err(|_| HarnessError::InvalidConfiguration)?;
                slots = lock(&processes.slots);
                slots.push(Arc::clone(&slot));
                (slot, guard)
            }
        };
        drop(slots);
        if purpose == Purpose::Run {
            slot.run_leased.store(true, Ordering::Release);
            self.running.fetch_add(1, Ordering::AcqRel);
        }
        Ok((processes, slot, guard, evicted))
    }

    /// Removes processes nobody has used for `idle_ttl` (and that aren't
    /// holding a sign-in), returning their locks so the caller can stop them
    /// outside the map lock.
    fn reap_idle(
        &self,
        accounts: &mut HashMap<Uuid, Arc<AccountProcesses>>,
        now: Instant,
    ) -> Vec<OwnedMutexGuard<SupervisorState>> {
        let mut evicted = Vec::new();
        for processes in accounts.values() {
            lock(&processes.slots).retain(|slot| {
                if !slot.idle_expired(now, self.config.idle_ttl) {
                    return true;
                }
                match Arc::clone(&slot.state).try_lock_owned() {
                    Ok(guard) => {
                        evicted.push(guard);
                        false
                    }
                    Err(_) => true,
                }
            });
        }
        accounts.retain(|_, processes| !lock(&processes.slots).is_empty());
        evicted
    }
}

/// Frees one process that isn't running a turn or holding a sign-in, the
/// least recently used first.
fn evict_one_idle(
    accounts: &mut HashMap<Uuid, Arc<AccountProcesses>>,
    now: Instant,
) -> Option<OwnedMutexGuard<SupervisorState>> {
    let mut candidates = accounts
        .values()
        .flat_map(|processes| {
            lock(&processes.slots)
                .iter()
                .map(|slot| (Arc::clone(processes), Arc::clone(slot)))
                .collect::<Vec<_>>()
        })
        .filter(|(_, slot)| !slot.run_leased.load(Ordering::Acquire) && !slot.pinned(now))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, slot)| *lock(&slot.last_released));
    for (processes, slot) in candidates {
        if let Ok(guard) = Arc::clone(&slot.state).try_lock_owned() {
            lock(&processes.slots).retain(|candidate| !Arc::ptr_eq(candidate, &slot));
            return Some(guard);
        }
    }
    None
}

impl HarnessAdapter for CodexAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(self.acquire(account, Purpose::Run))
    }

    fn acquire_control(
        &self,
        account: AccountHandle,
    ) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(self.acquire(account, Purpose::Control))
    }

    fn health(
        &self,
        account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        Box::pin(async move {
            let slots = {
                let accounts = lock(&self.pool.accounts);
                lock(&accounts.get(&account_id)?.slots).clone()
            };
            let mut health = ProfileHealth::default();
            for slot in slots {
                match slot.state.try_lock() {
                    Ok(state) => {
                        let snapshot = health_snapshot(&state);
                        health.process_running |= snapshot.process_running;
                        health.circuit_open |= snapshot.circuit_open;
                        health.consecutive_failures = health
                            .consecutive_failures
                            .max(snapshot.consecutive_failures);
                        health.process_starts = health
                            .process_starts
                            .saturating_add(snapshot.process_starts);
                    }
                    // In use right now, so its process is up.
                    Err(_) => health.process_running = true,
                }
            }
            Some(health)
        })
    }

    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let processes = lock(&self.pool.accounts).get(&account_id).cloned();
            let Some(processes) = processes else {
                return;
            };
            processes.stopping.store(true, Ordering::Release);
            let slots = lock(&processes.slots).clone();
            for slot in slots {
                let mut state = slot.state.lock().await;
                shutdown_bridge(&mut state).await;
            }
            let mut accounts = lock(&self.pool.accounts);
            if accounts
                .get(&account_id)
                .is_some_and(|current| Arc::ptr_eq(current, &processes))
            {
                accounts.remove(&account_id);
            }
        })
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let all = lock(&self.pool.accounts)
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for processes in &all {
                processes.stopping.store(true, Ordering::Release);
            }
            for processes in all {
                let slots = lock(&processes.slots).clone();
                for slot in slots {
                    if let Ok(mut state) = timeout(
                        self.pool.config.shutdown_timeout + Duration::from_secs(1),
                        slot.state.lock(),
                    )
                    .await
                    {
                        shutdown_bridge(&mut state).await;
                    } else {
                        tracing::error!(
                            "timed out waiting for a Codex process lease during shutdown"
                        );
                    }
                }
            }
            lock(&self.pool.accounts).clear();
        })
    }
}

pub struct CodexSession {
    pool: Arc<Pool>,
    account_id: Uuid,
    processes: Arc<AccountProcesses>,
    slot: Arc<Slot>,
    guard: OwnedMutexGuard<SupervisorState>,
    run: bool,
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        if self.run {
            self.slot.run_leased.store(false, Ordering::Release);
            self.pool.running.fetch_sub(1, Ordering::AcqRel);
        }
        *lock(&self.slot.last_released) = Instant::now();
    }
}

impl CodexSession {
    fn pin_for_login(&self) {
        *lock(&self.slot.pinned_until) = Some(Instant::now() + LOGIN_PIN);
    }

    fn unpin_account(&self) {
        for slot in lock(&self.processes.slots).iter() {
            *lock(&slot.pinned_until) = None;
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, HarnessError> {
        let result = self.bridge_mut()?.request(method, params).await;
        match result {
            Ok(value) => {
                record_success(&mut self.guard);
                Ok(value)
            }
            Err(error) => {
                self.fail_transport().await;
                Err(transport(error))
            }
        }
    }

    fn bridge_mut(&mut self) -> Result<&mut CodexBridge, HarnessError> {
        self.guard.bridge.as_mut().ok_or(HarnessError::ShuttingDown)
    }

    async fn finish_transport_result(
        &mut self,
        result: Result<(), BridgeError>,
    ) -> Result<(), HarnessError> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.fail_transport().await;
                Err(transport(error))
            }
        }
    }

    async fn fail_transport(&mut self) {
        shutdown_bridge(&mut self.guard).await;
        record_failure(&mut self.guard);
    }

    async fn next_codex_event(&mut self) -> Result<HarnessEvent, HarnessError> {
        let raw = match self.bridge_mut()?.next_raw_event().await {
            Ok(value) => value,
            Err(error) => {
                self.fail_transport().await;
                return Err(transport(error));
            }
        };
        parse_event(raw)
    }

    async fn read_models(&mut self) -> Result<Vec<ModelEntry>, HarnessError> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_MODEL_PAGES {
            let response = self
                .request(
                    "model/list",
                    json!({"cursor": cursor, "includeHidden": false, "limit": 100}),
                )
                .await?;
            let page = response
                .get("data")
                .and_then(Value::as_array)
                .ok_or(HarnessError::InvalidResponse)?;
            models.extend(page.iter().filter_map(parse_model));
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(models)
    }
}

impl HarnessSession for CodexSession {
    fn account_id(&self) -> Uuid {
        self.account_id
    }

    fn is_stopping(&self) -> bool {
        self.processes.stopping.load(Ordering::Acquire)
    }

    fn start_thread<'a>(
        &'a mut self,
        cwd: &'a Path,
        config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            let result = self
                .request("thread/start", thread_start_params(cwd, config))
                .await?;
            result
                .pointer("/thread/id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(HarnessError::MissingResult("thread id"))
        })
    }

    fn resume_thread<'a>(
        &'a mut self,
        thread_id: &'a str,
        cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            self.request("thread/resume", json!({"threadId": thread_id, "cwd": cwd}))
                .await?;
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
            let result = self
                .request(
                    "turn/start",
                    turn_start_params(thread_id, cwd, prompt, config),
                )
                .await?;
            result
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(HarnessError::MissingResult("turn id"))
        })
    }

    fn interrupt_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            self.request(
                "turn/interrupt",
                json!({"threadId": thread_id, "turnId": turn_id}),
            )
            .await?;
            Ok(())
        })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(self.next_codex_event())
    }

    fn answer(&mut self, request: HarnessRequest, answer: HarnessAnswer) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            let id = request
                .token
                .get("id")
                .cloned()
                .ok_or(HarnessError::MissingResult("request id"))?;
            let result = match request.kind {
                RequestKind::Approval => {
                    let decision = match answer {
                        HarnessAnswer::Approve => "accept",
                        HarnessAnswer::Deny => "decline",
                        HarnessAnswer::Text(_) => return Err(HarnessError::InvalidResponse),
                    };
                    json!({"decision": decision})
                }
                RequestKind::Question => {
                    let answer = match answer {
                        HarnessAnswer::Approve => "approved".to_owned(),
                        HarnessAnswer::Deny => "denied".to_owned(),
                        HarnessAnswer::Text(answer) => bounded_provider_text(&answer, 4_000)?,
                    };
                    let answers = request
                        .token
                        .get("questionIds")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|id| (id.to_owned(), json!({"answers": [answer]})))
                        .collect::<serde_json::Map<_, _>>();
                    json!({"answers": answers})
                }
                RequestKind::Unsupported => {
                    let response = self
                        .bridge_mut()?
                        .respond_error(id, -32601, "unsupported provider request")
                        .await;
                    return self.finish_transport_result(response).await;
                }
            };
            let response = self.bridge_mut()?.respond(id, result).await;
            self.finish_transport_result(response).await
        })
    }

    fn read_latest_turn<'a>(&'a mut self, thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        Box::pin(async move {
            let history = self
                .request(
                    "thread/read",
                    json!({"threadId": thread_id, "includeTurns": true}),
                )
                .await?;
            Ok(parse_latest_turn(&history))
        })
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary> {
        Box::pin(async move {
            let account = self
                .request("account/read", json!({"refreshToken": false}))
                .await?;
            let summary = parse_account(&account)?;
            if summary.authenticated {
                self.unpin_account();
            }
            Ok(summary)
        })
    }

    fn start_login(&mut self, kind: LoginKind) -> HarnessFuture<'_, LoginStart> {
        Box::pin(async move {
            let started = match kind {
                LoginKind::Browser => {
                    let response = self
                        .request(
                            "account/login/start",
                            json!({
                                "type": "chatgpt",
                                "appBrand": "codex",
                                "codexStreamlinedLogin": false,
                                "useHostedLoginSuccessPage": true
                            }),
                        )
                        .await?;
                    parse_login_start(response, "chatgpt")?
                }
                LoginKind::DeviceCode => {
                    let response = self
                        .request("account/login/start", json!({"type": "chatgptDeviceCode"}))
                        .await?;
                    parse_login_start(response, "chatgptDeviceCode")?
                }
                LoginKind::ApiKey => return Err(HarnessError::Unsupported("key sign-in here")),
            };
            self.pin_for_login();
            Ok(started)
        })
    }

    fn login_with_api_key<'a>(&'a mut self, api_key: &'a str) -> HarnessFuture<'a, LoginStart> {
        Box::pin(async move {
            if !(20..=4096).contains(&api_key.len()) || api_key.chars().any(char::is_control) {
                return Err(HarnessError::InvalidResponse);
            }
            let response = self
                .request(
                    "account/login/start",
                    json!({"type": "apiKey", "apiKey": api_key}),
                )
                .await?;
            if response.get("type").and_then(Value::as_str) != Some("apiKey") {
                return Err(HarnessError::InvalidResponse);
            }
            Ok(LoginStart::ApiKey)
        })
    }

    fn cancel_login<'a>(&'a mut self, login_id: &'a str) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            let login_id = bounded_provider_text(login_id, 256)?;
            self.request("account/login/cancel", json!({"loginId": login_id}))
                .await?;
            self.unpin_account();
            Ok(())
        })
    }

    fn logout(&mut self) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            self.request("account/logout", Value::Null).await?;
            self.unpin_account();
            Ok(())
        })
    }

    fn read_rate_limits(&mut self) -> HarnessFuture<'_, RateLimitSummary> {
        Box::pin(async move {
            let response = self.request("account/rateLimits/read", Value::Null).await?;
            let rate_limits = response
                .get("rateLimits")
                .ok_or(HarnessError::InvalidResponse)?;
            Ok(RateLimitSummary {
                primary: parse_rate_limit_window(rate_limits.get("primary"))?,
                secondary: parse_rate_limit_window(rate_limits.get("secondary"))?,
            })
        })
    }

    fn read_usage(&mut self) -> HarnessFuture<'_, UsageReport> {
        Box::pin(async move {
            let response = self.request("account/rateLimits/read", Value::Null).await?;
            parse_usage_report(&response)
        })
    }

    fn spend_reset_credit<'a>(
        &'a mut self,
        credit_id: Option<&'a str>,
        idempotency_key: &'a str,
    ) -> HarnessFuture<'a, ResetOutcome> {
        Box::pin(async move {
            let credit_id = credit_id
                .map(|id| bounded_provider_text(id, 200))
                .transpose()?;
            let idempotency_key = bounded_provider_text(idempotency_key, 200)?;
            let response = self
                .request(
                    "account/rateLimitResetCredit/consume",
                    json!({"creditId": credit_id, "idempotencyKey": idempotency_key}),
                )
                .await?;
            match response.get("outcome").and_then(Value::as_str) {
                Some("reset") => Ok(ResetOutcome::Reset),
                Some("nothingToReset") => Ok(ResetOutcome::NothingToReset),
                Some("noCredit") => Ok(ResetOutcome::NoCredit),
                Some("alreadyRedeemed") => Ok(ResetOutcome::AlreadyRedeemed),
                _ => Err(HarnessError::InvalidResponse),
            }
        })
    }

    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>> {
        Box::pin(self.read_models())
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(shutdown_bridge(&mut self.guard))
    }
}

fn thread_start_params(cwd: &Path, config: &RunConfig) -> Value {
    let mut params = json!({
        "cwd": cwd,
        "approvalPolicy": approval_policy(config),
        "sandbox": sandbox_mode(config),
        "ephemeral": false
    });
    if let Some(model) = &config.model {
        params["model"] = json!(model);
    }
    if let Some(tier) = service_tier(config) {
        params["serviceTier"] = json!(tier);
    }
    params
}

fn turn_start_params(thread_id: &str, cwd: &Path, prompt: &str, config: &RunConfig) -> Value {
    // Isolation is set on every turn, not only when the thread starts: a
    // resumed thread otherwise keeps the policy of the run that created it.
    let mut params = json!({
        "threadId": thread_id,
        "cwd": cwd,
        "input": [{"type": "text", "text": prompt}],
        "approvalPolicy": approval_policy(config),
        "sandboxPolicy": sandbox_policy(cwd, config)
    });
    if let Some(model) = &config.model {
        params["model"] = json!(model);
    }
    if let Some(effort) = &config.reasoning {
        params["effort"] = json!(effort);
    }
    if let Some(tier) = service_tier(config) {
        params["serviceTier"] = json!(tier);
    }
    if let Some(summary) = config
        .options
        .get("reasoningSummary")
        .and_then(Value::as_str)
    {
        params["summary"] = json!(summary);
    }
    params
}

fn approval_policy(config: &RunConfig) -> &'static str {
    match config.approval_policy.as_deref() {
        Some("untrusted") => "untrusted",
        _ => "on-request",
    }
}

fn sandbox_mode(config: &RunConfig) -> &'static str {
    match config.sandbox.as_deref() {
        Some("read_only") => "read-only",
        _ => "workspace-write",
    }
}

/// The per-turn form of `sandbox_mode`: a read-only turn, or writes limited
/// to the run's worktree with no network.
fn sandbox_policy(cwd: &Path, config: &RunConfig) -> Value {
    match sandbox_mode(config) {
        "read-only" => json!({"type": "readOnly"}),
        _ => json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd],
            "networkAccess": false
        }),
    }
}

/// Codex calls speed its service tier; the standard speed sends none.
fn service_tier(config: &RunConfig) -> Option<&str> {
    config
        .speed
        .as_deref()
        .filter(|speed| *speed != STANDARD_SPEED)
}

async fn ensure_started(
    config: &SupervisorConfig,
    codex_home: &Path,
    state: &mut SupervisorState,
) -> Result<(), HarnessError> {
    if state.bridge.is_some() {
        return Ok(());
    }
    let now = Instant::now();
    if state
        .circuit_open_until
        .is_some_and(|circuit_open_until| circuit_open_until > now)
    {
        return Err(HarnessError::CircuitOpen);
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
    let binary_parent = config
        .codex_binary
        .parent()
        .ok_or(HarnessError::InvalidConfiguration)?;
    environment.insert(
        "PATH".to_owned(),
        format!("{}:/usr/local/bin:/usr/bin:/bin", binary_parent.display()),
    );
    environment.insert("LANG".to_owned(), "C.UTF-8".to_owned());
    environment.insert("LC_ALL".to_owned(), "C.UTF-8".to_owned());
    let bridge_config = BridgeConfig {
        codex_binary: config.codex_binary.clone(),
        codex_home: codex_home.to_path_buf(),
        cwd: config.neutral_cwd.clone(),
        safe_environment: environment,
        max_line_bytes: config.max_line_bytes,
        max_stderr_bytes: config.max_stderr_bytes,
        max_buffered_events: config.max_buffered_events,
        automatic_server_request_responses: false,
        request_timeout: config.request_timeout,
        shutdown_timeout: config.shutdown_timeout,
    };
    let mut bridge = match CodexBridge::spawn(bridge_config).await {
        Ok(bridge) => bridge,
        Err(error) => {
            record_failure(state);
            return Err(transport(error));
        }
    };
    if let Err(error) = bridge.initialize().await {
        bridge.shutdown().await;
        record_failure(state);
        return Err(transport(error));
    }
    state.bridge = Some(bridge);
    state.process_starts = state.process_starts.saturating_add(1);
    record_success(state);
    Ok(())
}

fn transport(error: BridgeError) -> HarnessError {
    HarnessError::Transport(Box::new(error))
}

fn parse_event(raw: Value) -> Result<HarnessEvent, HarnessError> {
    if raw.get("id").is_some() {
        return if raw.get("method").is_some() {
            Ok(HarnessEvent::Request(parse_provider_request(raw)?))
        } else {
            Ok(HarnessEvent::Error)
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
            .map(|delta| HarnessEvent::MessageDelta(delta.to_owned()))
            .unwrap_or(HarnessEvent::Ignored),
        "item/started" => HarnessEvent::ItemStarted(HarnessItem {
            key: provider_event_key(method, &raw),
            kind: item_kind(&raw),
        }),
        "item/completed" => HarnessEvent::ItemCompleted(HarnessItem {
            key: provider_event_key(method, &raw),
            kind: item_kind(&raw),
        }),
        "turn/completed" => HarnessEvent::TurnCompleted {
            completed: raw.pointer("/params/turn/status").and_then(Value::as_str)
                == Some("completed"),
        },
        "error" => HarnessEvent::Error,
        // Usage readings are a side note to the turn; one that doesn't parse
        // is dropped, never the turn.
        "account/rateLimits/updated" => raw
            .pointer("/params/rateLimits")
            .and_then(|snapshot| parse_usage_snapshot(None, snapshot).ok())
            .map(|windows| {
                HarnessEvent::UsageUpdated(UsageReport {
                    windows,
                    reset_credits: None,
                    plan: None,
                })
            })
            .unwrap_or(HarnessEvent::Ignored),
        _ => HarnessEvent::Ignored,
    })
}

/// The most windows one account read may report.
const MAX_USAGE_WINDOWS: usize = 16;
const MAX_RESET_CREDITS: usize = 50;

/// `account/rateLimits/read`: the multi-bucket view when present (one
/// bucket per metered limit, such as `codex`), else the single snapshot.
fn parse_usage_report(response: &Value) -> Result<UsageReport, HarnessError> {
    let mut windows = Vec::new();
    match response
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|buckets| !buckets.is_empty())
    {
        Some(buckets) => {
            for (limit_id, snapshot) in buckets {
                windows.extend(parse_usage_snapshot(Some(limit_id), snapshot)?);
            }
        }
        None => {
            let snapshot = response
                .get("rateLimits")
                .ok_or(HarnessError::InvalidResponse)?;
            windows.extend(parse_usage_snapshot(None, snapshot)?);
        }
    }
    windows.truncate(MAX_USAGE_WINDOWS);
    let plan = response
        .pointer("/rateLimits/planType")
        .and_then(Value::as_str)
        .and_then(|plan| bounded_provider_text(plan, 40).ok());
    let reset_credits = match response
        .get("rateLimitResetCredits")
        .filter(|value| !value.is_null())
    {
        None => None,
        Some(summary) => Some(parse_reset_credits(summary)?),
    };
    Ok(UsageReport {
        windows,
        reset_credits,
        plan,
    })
}

fn parse_usage_snapshot(
    limit_id: Option<&str>,
    snapshot: &Value,
) -> Result<Vec<UsageWindow>, HarnessError> {
    let limit_id = limit_id
        .or_else(|| snapshot.get("limitId").and_then(Value::as_str))
        .map(usage_key_segment)
        .unwrap_or_else(|| "codex".to_owned());
    let limit_name = snapshot
        .get("limitName")
        .and_then(Value::as_str)
        .and_then(|name| bounded_provider_text(name, 80).ok());
    let reached = snapshot
        .get("rateLimitReachedType")
        .is_some_and(|value| !value.is_null());
    let mut windows = Vec::new();
    for slot in ["primary", "secondary"] {
        let Some(value) = snapshot.get(slot).filter(|value| !value.is_null()) else {
            continue;
        };
        let used_percent = value
            .get("usedPercent")
            .and_then(Value::as_i64)
            .ok_or(HarnessError::InvalidResponse)?
            .clamp(0, 100);
        let bounded = |name: &str, maximum: i64| {
            value
                .get(name)
                .and_then(Value::as_i64)
                .filter(|value| (0..=maximum).contains(value))
        };
        windows.push(UsageWindow {
            key: format!("{limit_id}.{slot}"),
            limit_name: limit_name.clone(),
            used_percent: u16::try_from(used_percent).unwrap_or(100),
            resets_at: bounded("resetsAt", 4_102_444_800),
            duration_minutes: bounded("windowDurationMins", 525_600),
            limit_reached: used_percent >= 100,
        });
    }
    // The snapshot says a limit is reached without saying which window: the
    // fullest one.
    if reached
        && !windows.iter().any(|window| window.limit_reached)
        && let Some(fullest) = windows.iter_mut().max_by_key(|window| window.used_percent)
    {
        fullest.limit_reached = true;
    }
    Ok(windows)
}

/// A limit id as part of a window key: lower case letters, digits, `_`
/// and `-`, at most 40 characters.
fn usage_key_segment(limit_id: &str) -> String {
    let segment = limit_id
        .chars()
        .filter_map(|c| {
            let c = c.to_ascii_lowercase();
            (c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).then_some(c)
        })
        .take(40)
        .collect::<String>();
    if segment.is_empty() {
        "codex".to_owned()
    } else {
        segment
    }
}

fn parse_reset_credits(summary: &Value) -> Result<ResetCredits, HarnessError> {
    let available = summary
        .get("availableCount")
        .and_then(Value::as_i64)
        .filter(|count| (0..=10_000).contains(count))
        .ok_or(HarnessError::InvalidResponse)?;
    let mut credits = Vec::new();
    for credit in summary
        .get("credits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if credit.get("status").and_then(Value::as_str) != Some("available") {
            continue;
        }
        let Some(id) = credit
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| bounded_provider_text(id, 200).ok())
        else {
            continue;
        };
        credits.push(ResetCredit {
            id,
            title: credit
                .get("title")
                .and_then(Value::as_str)
                .and_then(|title| bounded_provider_text(title, 120).ok()),
            expires_at: credit.get("expiresAt").and_then(Value::as_i64),
            granted_at: credit.get("grantedAt").and_then(Value::as_i64),
        });
        if credits.len() == MAX_RESET_CREDITS {
            break;
        }
    }
    Ok(ResetCredits {
        available: u32::try_from(available).unwrap_or(0),
        credits,
    })
}

fn parse_latest_turn(history: &Value) -> RecoveredTurn {
    let Some(latest_turn) = history
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.last())
    else {
        return RecoveredTurn {
            id: None,
            status: "missing".to_owned(),
            final_message: None,
        };
    };
    let id = latest_turn
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
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
    RecoveredTurn {
        id,
        status,
        final_message,
    }
}

fn parse_account(account: &Value) -> Result<AccountSummary, HarnessError> {
    let account_value = account.get("account");
    let authenticated = account_value.is_some_and(|value| !value.is_null());
    let account_type = account_value
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .map(|value| bounded_provider_text(value, 32))
        .transpose()?;
    let email = account_value
        .and_then(|value| value.get("email"))
        .and_then(Value::as_str)
        .map(|value| bounded_provider_text(value, 320))
        .transpose()?;
    if authenticated && account_type.is_none() {
        return Err(HarnessError::InvalidResponse);
    }
    Ok(AccountSummary {
        authenticated,
        account_type,
        email,
    })
}

/// One `model/list` entry as a catalog model. Entries with unusable
/// identifiers are dropped rather than failing the whole list.
fn parse_model(value: &Value) -> Option<ModelEntry> {
    let text = |name: &str, max: usize| {
        value
            .get(name)
            .and_then(Value::as_str)
            .and_then(|text| bounded_provider_text(text, max).ok())
    };
    let identifier = |text: &str| {
        (1..=200).contains(&text.len())
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
            })
    };
    let id = text("model", 200).or_else(|| text("id", 200))?;
    if !identifier(&id) {
        return None;
    }
    let reasoning_levels = value
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| option.get("reasoningEffort").and_then(Value::as_str))
        .filter(|level| identifier(level) && level.len() <= 64)
        .map(str::to_owned)
        .take(16)
        .collect::<Vec<_>>();
    let mut speeds = vec![SpeedOption {
        id: STANDARD_SPEED.to_owned(),
        name: "Standard".to_owned(),
    }];
    speeds.extend(
        value
            .get("serviceTiers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tier| {
                let id = tier.get("id").and_then(Value::as_str)?;
                let name = tier.get("name").and_then(Value::as_str)?;
                (identifier(id) && id != STANDARD_SPEED && id.len() <= 64)
                    .then(|| bounded_provider_text(name, 120).ok())
                    .flatten()
                    .map(|name| SpeedOption {
                        id: id.to_owned(),
                        name,
                    })
            })
            .take(15),
    );
    let default_reasoning =
        text("defaultReasoningEffort", 64).filter(|level| reasoning_levels.contains(level));
    let default_speed = text("defaultServiceTier", 64)
        .filter(|tier| speeds.iter().any(|speed| &speed.id == tier))
        .or_else(|| Some(STANDARD_SPEED.to_owned()));
    Some(ModelEntry {
        display_name: text("displayName", 120).unwrap_or_else(|| id.clone()),
        description: text("description", 2000),
        id,
        context_window: None,
        reasoning_levels,
        default_reasoning,
        speeds,
        default_speed,
        prices: None,
        hidden: value
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_default: value
            .get("isDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_provider_request(raw: Value) -> Result<HarnessRequest, HarnessError> {
    let id = raw
        .get("id")
        .cloned()
        .ok_or(HarnessError::MissingResult("provider request id"))?;
    let method = raw
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = match method {
        "item/tool/requestUserInput" => RequestKind::Question,
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/permissions/requestApproval" => RequestKind::Approval,
        _ => RequestKind::Unsupported,
    };
    let questions = raw.pointer("/params/questions").and_then(Value::as_array);
    let question_ids = if kind == RequestKind::Question {
        questions
            .into_iter()
            .flatten()
            .filter_map(|question| question.get("id").and_then(Value::as_str))
            .filter_map(|id| bounded_provider_text(id, 128).ok())
            .take(20)
            .collect()
    } else {
        Vec::new()
    };
    if kind == RequestKind::Question && question_ids.is_empty() {
        return Err(HarnessError::InvalidResponse);
    }
    let summary = if kind == RequestKind::Question {
        questions
            .and_then(|questions| questions.first())
            .and_then(|question| question.get("question"))
            .and_then(Value::as_str)
            .and_then(|summary| bounded_provider_text(summary, 2_000).ok())
            .unwrap_or_else(|| "The agent needs an answer before it can continue.".to_owned())
    } else {
        "The agent needs permission before it can continue.".to_owned()
    };
    let options = if kind == RequestKind::Question {
        questions
            .into_iter()
            .flatten()
            .flat_map(|question| {
                question
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|option| {
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .and_then(|label| bounded_provider_text(label, 200).ok())?;
                Some(RequestOption {
                    id: label.clone(),
                    label,
                })
            })
            .take(40)
            .collect()
    } else {
        vec![
            RequestOption {
                id: "approve".to_owned(),
                label: "Allow once".to_owned(),
            },
            RequestOption {
                id: "deny".to_owned(),
                label: "Deny".to_owned(),
            },
        ]
    };
    Ok(HarnessRequest {
        kind,
        durable_id: bounded_provider_identity("request", &id),
        summary,
        options,
        token: json!({"id": id, "questionIds": question_ids}),
    })
}

fn provider_event_key(method: &str, raw: &Value) -> String {
    let identity = [
        "/params/item/id",
        "/params/itemId",
        "/params/turn/id",
        "/params/turnId",
    ]
    .into_iter()
    .find_map(|pointer| raw.pointer(pointer))
    .map(Value::to_string)
    .filter(|value| value.len() <= 512 && !value.chars().any(char::is_control));
    match identity {
        Some(identity) => format!("provider:{method}:{identity}"),
        None => format!(
            "provider:{method}:{}",
            bounded_provider_identity("event", raw)
        ),
    }
}

fn bounded_provider_identity(namespace: &str, value: &Value) -> String {
    let encoded = value.to_string();
    if encoded.len() <= 512 && !encoded.chars().any(char::is_control) {
        return encoded;
    }
    let digest = Sha256::digest(encoded.as_bytes());
    format!("{namespace}:sha256:{digest:x}")
}

fn parse_login_start(response: Value, expected_type: &str) -> Result<LoginStart, HarnessError> {
    if response.get("type").and_then(Value::as_str) != Some(expected_type) {
        return Err(HarnessError::InvalidResponse);
    }
    let login_id = bounded_provider_text(
        response
            .get("loginId")
            .and_then(Value::as_str)
            .ok_or(HarnessError::InvalidResponse)?,
        256,
    )?;
    match expected_type {
        "chatgpt" => Ok(LoginStart::Browser {
            login_id,
            auth_url: validated_provider_url(
                response
                    .get("authUrl")
                    .and_then(Value::as_str)
                    .ok_or(HarnessError::InvalidResponse)?,
            )?,
        }),
        "chatgptDeviceCode" => Ok(LoginStart::DeviceCode {
            login_id,
            verification_url: validated_provider_url(
                response
                    .get("verificationUrl")
                    .and_then(Value::as_str)
                    .ok_or(HarnessError::InvalidResponse)?,
            )?,
            user_code: bounded_provider_text(
                response
                    .get("userCode")
                    .and_then(Value::as_str)
                    .ok_or(HarnessError::InvalidResponse)?,
                128,
            )?,
        }),
        _ => Err(HarnessError::InvalidResponse),
    }
}

fn parse_rate_limit_window(value: Option<&Value>) -> Result<Option<RateLimitWindow>, HarnessError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let used_percent = value
        .get("usedPercent")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value <= 100)
        .ok_or(HarnessError::InvalidResponse)?;
    let bounded_optional_i64 = |name: &str, maximum: i64| {
        value
            .get(name)
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_i64()
                    .filter(|value| (0..=maximum).contains(value))
                    .ok_or(HarnessError::InvalidResponse)
            })
            .transpose()
    };
    Ok(Some(RateLimitWindow {
        used_percent,
        resets_at: bounded_optional_i64("resetsAt", 4_102_444_800)?,
        window_duration_minutes: bounded_optional_i64("windowDurationMins", 525_600)?,
    }))
}

fn bounded_provider_text(value: &str, maximum_characters: usize) -> Result<String, HarnessError> {
    let character_count = value.chars().count();
    if character_count == 0
        || character_count > maximum_characters
        || value.chars().any(char::is_control)
    {
        return Err(HarnessError::InvalidResponse);
    }
    Ok(value.to_owned())
}

fn validated_provider_url(value: &str) -> Result<String, HarnessError> {
    let value = bounded_provider_text(value, 2048)?;
    let uri = value
        .parse::<http::Uri>()
        .map_err(|_| HarnessError::InvalidResponse)?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err(HarnessError::InvalidResponse);
    }
    Ok(value)
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

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path, sync::Arc, time::Duration};

    use serde_json::json;
    use uuid::Uuid;

    use super::{
        CodexAdapter, SupervisorState, health_snapshot, item_kind, parse_event, parse_login_start,
        parse_model, parse_provider_request, parse_rate_limit_window, parse_usage_report,
        provider_event_key, record_failure, restart_backoff, turn_start_params,
    };
    use crate::{
        config::AppConfig,
        provider::{
            adapter::{
                AccountHandle, HarnessAdapter, HarnessError, HarnessEvent, ItemKind, LoginStart,
                RecoveredTurn, RequestKind,
            },
            contract_tests::adapter_contract,
            registry::ShutdownSignal,
            run_config::RunConfig,
        },
    };

    #[test]
    fn restart_backoff_is_bounded() {
        assert_eq!(restart_backoff(1), Duration::from_millis(100));
        assert_eq!(restart_backoff(3), Duration::from_millis(400));
        assert_eq!(restart_backoff(100), Duration::from_secs(5));
    }

    #[test]
    fn repeated_failures_open_the_circuit() {
        let mut state = SupervisorState::new();
        for _ in 0..5 {
            record_failure(&mut state);
        }
        let health = health_snapshot(&state);
        assert!(health.circuit_open);
        assert_eq!(health.consecutive_failures, 5);
    }

    #[test]
    fn provider_requests_become_neutral_requests() {
        let request = parse_provider_request(json!({
            "id": 17,
            "method": "item/tool/requestUserInput",
            "params": {"questions": [{
                "id": "choice",
                "question": "Which release track?",
                "options": [{"label": "Stable"}, {"label": "Preview"}]
            }]}
        }))
        .expect("provider request");
        assert_eq!(request.kind, RequestKind::Question);
        assert_eq!(request.summary, "Which release track?");
        assert_eq!(request.public_options()[0]["label"], "Stable");
        assert_eq!(request.token["questionIds"], json!(["choice"]));
        assert!(!request.summary.contains("Codex"));

        let approval = parse_provider_request(json!({
            "id": 19,
            "method": "item/commandExecution/requestApproval",
            "params": {"command": "rm -rf /"}
        }))
        .expect("approval request");
        assert_eq!(approval.kind, RequestKind::Approval);
        assert!(
            !approval.summary.contains("rm -rf"),
            "raw provider params stay private"
        );

        let unsupported = parse_provider_request(json!({
            "id": 18,
            "method": "account/chatgptAuthTokens/refresh",
            "params": {}
        }))
        .expect("unsupported provider request");
        assert_eq!(unsupported.kind, RequestKind::Unsupported);

        let item = json!({"params": {"item": {"type": "commandExecution"}}});
        assert_eq!(item_kind(&item), ItemKind::CommandExecution);
    }

    #[test]
    fn provider_event_identities_are_stable_and_bounded() {
        let first = json!({
            "method": "item/completed",
            "params": {"item": {"id": "item-7", "type": "commandExecution"}}
        });
        assert_eq!(
            provider_event_key("item/completed", &first),
            "provider:item/completed:\"item-7\""
        );
        let oversized = json!({"params": {"opaque": "x".repeat(4096)}});
        let key = provider_event_key("item/started", &oversized);
        assert!(key.starts_with("provider:item/started:event:sha256:"));
        assert!(key.len() < 160);
    }

    #[test]
    fn login_and_rate_limit_shapes_are_bounded() {
        let login = parse_login_start(
            json!({
                "type": "chatgptDeviceCode",
                "loginId": "login-1",
                "verificationUrl": "https://example.com/device",
                "userCode": "ABCD-EFGH"
            }),
            "chatgptDeviceCode",
        )
        .expect("device login response");
        assert!(matches!(login, LoginStart::DeviceCode { .. }));
        assert!(
            parse_login_start(
                json!({"type": "chatgpt", "loginId": "login-2", "authUrl": "javascript:alert(1)"}),
                "chatgpt"
            )
            .is_err()
        );
        let window = parse_rate_limit_window(Some(&json!({
            "usedPercent": 42,
            "resetsAt": 2_000_000_000_i64,
            "windowDurationMins": 300
        })))
        .expect("rate limit window")
        .expect("present rate limit window");
        assert_eq!(window.used_percent, 42);
        assert!(parse_rate_limit_window(Some(&json!({"usedPercent": 101}))).is_err());
    }

    #[test]
    fn usage_reads_every_bucket_window_and_banked_reset() {
        let report = parse_usage_report(&json!({
            "rateLimits": {"planType": "pro", "primary": {"usedPercent": 10}},
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 62, "resetsAt": 2_000_000_000_i64, "windowDurationMins": 300},
                    "secondary": {"usedPercent": 140, "resetsAt": 2_000_500_000_i64, "windowDurationMins": 10080}
                },
                "Other Model!": {"secondary": {"usedPercent": 5, "windowDurationMins": 10080}}
            },
            "rateLimitResetCredits": {
                "availableCount": 2,
                "credits": [
                    {"id": "credit-1", "grantedAt": 1_900_000_000_i64, "expiresAt": 2_000_100_000_i64,
                     "resetType": "codexRateLimits", "status": "available", "title": "Bonus reset"},
                    {"id": "credit-0", "grantedAt": 1_800_000_000_i64, "resetType": "codexRateLimits",
                     "status": "redeemed"}
                ]
            }
        }))
        .expect("usage report");
        let mut keys = report
            .windows
            .iter()
            .map(|window| window.key.as_str())
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["codex.primary", "codex.secondary", "othermodel.secondary"]
        );
        let weekly = report
            .windows
            .iter()
            .find(|window| window.key == "codex.secondary")
            .expect("weekly window");
        assert_eq!(weekly.used_percent, 100);
        assert!(weekly.limit_reached);
        assert_eq!(report.plan.as_deref(), Some("pro"));
        let credits = report.reset_credits.expect("banked resets");
        assert_eq!(credits.available, 2);
        assert_eq!(credits.credits.len(), 1);
        assert_eq!(credits.credits[0].id, "credit-1");

        // A plan with only a weekly window and the older single snapshot.
        let weekly = parse_usage_report(&json!({
            "rateLimits": {"primary": null, "secondary": {"usedPercent": 30, "windowDurationMins": 10080},
                           "rateLimitReachedType": null}
        }))
        .expect("weekly only");
        assert_eq!(weekly.windows.len(), 1);
        assert_eq!(weekly.windows[0].key, "codex.secondary");
        assert!(weekly.reset_credits.is_none());
    }

    #[test]
    fn rolling_usage_updates_become_usage_events() {
        let event = parse_event(json!({
            "method": "account/rateLimits/updated",
            "params": {"rateLimits": {"limitId": "codex", "primary": {"usedPercent": 99},
                                      "rateLimitReachedType": "rate_limit_reached"}}
        }))
        .expect("event");
        let HarnessEvent::UsageUpdated(report) = event else {
            panic!("expected a usage update");
        };
        assert_eq!(report.windows[0].key, "codex.primary");
        assert!(report.windows[0].limit_reached);
        let broken = parse_event(json!({
            "method": "account/rateLimits/updated",
            "params": {"rateLimits": {"primary": {"usedPercent": "lots"}}}
        }))
        .expect("event");
        assert!(matches!(broken, HarnessEvent::Ignored));
    }

    #[test]
    fn model_list_entries_become_catalog_models() {
        let model = parse_model(&json!({
            "id": "gpt-x",
            "model": "gpt-x",
            "displayName": "GPT X",
            "description": "Fast",
            "supportedReasoningEfforts": [
                {"reasoningEffort": "low"}, {"reasoningEffort": "high"}
            ],
            "defaultReasoningEffort": "low",
            "serviceTiers": [{"id": "priority", "name": "Fast"}],
            "hidden": false,
            "isDefault": true
        }))
        .expect("model");
        assert_eq!(model.id, "gpt-x");
        assert_eq!(model.reasoning_levels, vec!["low", "high"]);
        assert_eq!(model.default_reasoning.as_deref(), Some("low"));
        assert!(model.speeds.iter().any(|speed| speed.id == "standard"));
        assert!(model.speeds.iter().any(|speed| speed.id == "priority"));
        assert!(model.is_default);
        assert!(parse_model(&json!({"id": "bad id with spaces"})).is_none());
    }

    #[test]
    fn run_settings_reach_the_turn() {
        let config = RunConfig {
            model: Some("gpt-x".to_owned()),
            reasoning: Some("high".to_owned()),
            speed: Some("priority".to_owned()),
            options: json!({"reasoningSummary": "concise"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ..RunConfig::default()
        };
        let params = turn_start_params("thread-1", Path::new("/work"), "go", &config);
        assert_eq!(params["model"], "gpt-x");
        assert_eq!(params["effort"], "high");
        assert_eq!(params["serviceTier"], "priority");
        assert_eq!(params["summary"], "concise");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["sandboxPolicy"]["type"], "workspaceWrite");
        assert_eq!(params["sandboxPolicy"]["writableRoots"], json!(["/work"]));
        let read_only = RunConfig {
            sandbox: Some("read_only".to_owned()),
            approval_policy: Some("untrusted".to_owned()),
            ..RunConfig::default()
        };
        let params = turn_start_params("thread-1", Path::new("/work"), "go", &read_only);
        assert_eq!(params["sandboxPolicy"], json!({"type": "readOnly"}));
        assert_eq!(params["approvalPolicy"], "untrusted");
        let standard = RunConfig {
            speed: Some("standard".to_owned()),
            ..RunConfig::default()
        };
        let params = turn_start_params("thread-1", Path::new("/work"), "go", &standard);
        assert!(params.get("serviceTier").is_none());
        assert!(params.get("model").is_none());
    }

    struct FakeCodex {
        _temporary: tempfile::TempDir,
        data_root: std::path::PathBuf,
        worktree: std::path::PathBuf,
        requests: std::path::PathBuf,
        config: AppConfig,
    }

    /// A stand-in `codex app-server` that answers the methods the adapter
    /// uses and logs every request it receives.
    #[cfg(unix)]
    fn fake_codex(host_limit: usize) -> FakeCodex {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary root");
        let data_root = temporary.path().join("data");
        let worktree = data_root.join("worktrees/one");
        fs::create_dir_all(&worktree).expect("create worktree");
        let requests = temporary.path().join("requests.log");
        let binary = temporary.path().join("fake-codex");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  if [[ "$line" != *'"id"'* ]]; then
    continue
  fi
  request_id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  if [[ "$line" == *'"method":"initialize"'* ]]; then
    printf '{{"id":%s,"result":{{"server":"fake"}}}}\n' "$request_id"
  elif [[ "$line" == *'"method":"thread/start"'* ]]; then
    printf '{{"id":%s,"result":{{"thread":{{"id":"thread-%s"}}}}}}\n' "$request_id" "$request_id"
  elif [[ "$line" == *'"method":"turn/start"'* ]]; then
    printf '{{"id":%s,"result":{{"turn":{{"id":"turn-%s"}}}}}}\n' "$request_id" "$request_id"
  elif [[ "$line" == *'"method":"account/read"'* ]]; then
    printf '{{"id":%s,"result":{{"account":{{"type":"chatgpt","email":"person@example.com"}}}}}}\n' "$request_id"
  elif [[ "$line" == *'"method":"model/list"'* ]]; then
    printf '{{"id":%s,"result":{{"data":[{{"id":"gpt-x","model":"gpt-x","displayName":"GPT X","supportedReasoningEfforts":[{{"reasoningEffort":"low"}}],"defaultReasoningEffort":"low","isDefault":true}}],"nextCursor":null}}}}\n' "$request_id"
  elif [[ "$line" == *'"method":"thread/read"'* ]]; then
    printf '{{"id":%s,"result":{{"thread":{{"turns":[{{"id":"turn-5","status":"completed","items":[{{"type":"agentMessage","text":"recovered proof"}}]}}]}}}}}}\n' "$request_id"
  else
    printf '{{"id":%s,"result":{{}}}}\n' "$request_id"
  fi
done
"#,
            log = requests.display()
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
                "AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT".to_owned(),
                host_limit.to_string(),
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
        .expect("fake Codex config");
        FakeCodex {
            _temporary: temporary,
            data_root,
            worktree,
            requests,
            config,
        }
    }

    fn account(fake: &FakeCodex, name: &str, max_concurrent_runs: u16) -> AccountHandle {
        let home = fake.data_root.join("homes").join(name);
        fs::create_dir_all(&home).expect("create account home");
        AccountHandle {
            id: Uuid::now_v7(),
            home,
            max_concurrent_runs,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_adapter_keeps_the_adapter_contract() {
        let fake = fake_codex(4);
        let adapter = CodexAdapter::new(&fake.config, Arc::new(ShutdownSignal::default()))
            .expect("Codex adapter");
        let account = account(&fake, "contract", 2);
        adapter_contract(&adapter, account, &fake.worktree).await;
        adapter.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn account_pages_never_take_a_run_slot_and_idle_processes_are_freed() {
        let fake = fake_codex(1);
        let adapter = CodexAdapter::new(&fake.config, Arc::new(ShutdownSignal::default()))
            .expect("Codex adapter")
            .with_idle_ttl(Duration::ZERO);
        let first = account(&fake, "first", 1);

        // Many account pages open at once (up to the idle process cap) leave
        // the host's only run slot free.
        let mut pages = Vec::new();
        for _ in 0..super::MAX_IDLE_PROCESSES {
            let mut page = adapter
                .acquire_control(account(&fake, "page", 1))
                .await
                .expect("account page");
            page.read_account().await.expect("read account");
            pages.push(page);
        }
        let mut run = adapter
            .acquire_run(first.clone())
            .await
            .expect("a run starts while account pages are open");
        let thread = run
            .start_thread(&fake.worktree, &RunConfig::default())
            .await
            .expect("thread");
        assert!(matches!(
            adapter.acquire_run(account(&fake, "second", 1)).await,
            Err(HarnessError::HostCapacity)
        ));
        assert!(matches!(
            adapter.acquire_control(account(&fake, "extra", 1)).await,
            Err(HarnessError::ControlCapacity)
        ));
        drop(pages);

        // With the pages closed and their processes idle past the TTL, the
        // next request reaps them.
        let mut page = adapter
            .acquire_control(first.clone())
            .await
            .expect("page after the others closed");
        page.read_account().await.expect("read account");
        drop(page);
        run.resume_thread(&thread, &fake.worktree, &RunConfig::default())
            .await
            .expect("resume");
        assert_eq!(
            run.read_latest_turn(&thread).await.expect("latest turn"),
            RecoveredTurn {
                id: Some("turn-5".to_owned()),
                status: "completed".to_owned(),
                final_message: Some("recovered proof".to_owned()),
            }
        );
        drop(run);
        let total_processes = adapter
            .pool
            .accounts
            .lock()
            .map(|accounts| {
                accounts
                    .values()
                    .map(|processes| processes.slots.lock().map(|slots| slots.len()).unwrap_or(0))
                    .sum::<usize>()
            })
            .unwrap_or(0);
        assert!(
            total_processes <= 2,
            "idle account-page processes were shut down, {total_processes} left"
        );
        adapter.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_settings_and_account_homes_reach_codex() {
        let fake = fake_codex(4);
        let adapter = CodexAdapter::new(&fake.config, Arc::new(ShutdownSignal::default()))
            .expect("Codex adapter");
        let account = account(&fake, "settings", 2);
        let config = RunConfig {
            model: Some("gpt-x".to_owned()),
            reasoning: Some("low".to_owned()),
            ..RunConfig::default()
        };
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        let thread = run
            .start_thread(&fake.worktree, &config)
            .await
            .expect("thread");
        run.start_turn(&thread, &fake.worktree, "go", &config)
            .await
            .expect("turn");
        drop(run);
        let mut control = adapter.acquire_control(account).await.expect("control");
        let models = control.list_models().await.expect("models");
        assert_eq!(models.len(), 1);
        drop(control);
        let log = fs::read_to_string(&fake.requests).expect("request log");
        let turn = log
            .lines()
            .find(|line| line.contains("\"method\":\"turn/start\""))
            .expect("turn/start request");
        assert!(turn.contains("\"model\":\"gpt-x\""));
        assert!(turn.contains("\"effort\":\"low\""));
        adapter.shutdown_all().await;
    }
}
