//! The development harness: an in-process adapter that behaves like a small
//! coding agent. Each turn asks for permission, then appends a proof line to
//! `KNOWN_FILE.txt` in its worktree, reports the file change and finishes
//! with a message. It runs through exactly the same run bookkeeping as Codex,
//! so scheduler, approval and recovery tests need no Codex binary, and it
//! passes the same adapter contract tests.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemKind, LoginKind,
        LoginStart, ProfileHealth, RateLimitSummary, RateLimitWindow, RecoveredTurn, RequestKind,
        RequestOption, ResetOutcome, UsageReport,
    },
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry, RunLocation,
        RunOptionSupport, SpeedOption, VersionRange,
    },
    registry::ShutdownSignal,
    run_config::RunConfig,
};

pub const DEMO_KIND: &str = "demo";
pub const DEMO_APPROVAL_SUMMARY: &str =
    "Allow the development executor to append its proof line to KNOWN_FILE.txt?";
pub const DEMO_PROOF_LINE: &str = "M0 walking skeleton browser proof.";
pub const DEMO_FINAL_MESSAGE: &str = "The development walking skeleton completed its isolated file change and preserved the worktree.";

pub fn demo_manifest() -> AdapterManifest {
    AdapterManifest {
        kind: DEMO_KIND.to_owned(),
        display_name: "Demo harness".to_owned(),
        description: "A built-in harness for development and tests. It edits one file and asks before it does.".to_owned(),
        versions: VersionRange {
            minimum: "1.0.0".to_owned(),
            below: Some("2.0.0".to_owned()),
        },
        branch_prefix: "demo/run-".to_owned(),
        runs_on: RunLocation::LocalHost,
        features: AdapterFeatures {
            streaming: true,
            commands: false,
            file_changes: true,
            approvals: true,
            questions: false,
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
            max_per_account: 8,
        },
        login_methods: vec![LoginMethod {
            kind: LoginKind::ApiKey,
            label: "Demo key".to_owned(),
        }],
        run_options: RunOptionSupport {
            sandbox_modes: vec!["workspace_write".to_owned()],
            approval_policies: vec!["on_request".to_owned()],
            max_attempts: 1,
        },
        options_schema: json!({
            "type": "object",
            "properties": {
                "proofLine": {"type": "string", "maxLength": 200}
            },
            "additionalProperties": false
        }),
        default_models: demo_models(),
    }
}

fn demo_models() -> Vec<ModelEntry> {
    let speeds = vec![
        SpeedOption {
            id: "standard".to_owned(),
            name: "Standard".to_owned(),
        },
        SpeedOption {
            id: "fast".to_owned(),
            name: "Fast".to_owned(),
        },
    ];
    vec![
        ModelEntry {
            id: "demo-small".to_owned(),
            display_name: "Demo small".to_owned(),
            description: Some("Quick and cheap.".to_owned()),
            context_window: Some(32_000),
            reasoning_levels: vec!["low".to_owned(), "medium".to_owned()],
            default_reasoning: Some("low".to_owned()),
            speeds: speeds.clone(),
            default_speed: Some("standard".to_owned()),
            ..ModelEntry::default()
        },
        ModelEntry {
            id: "demo-large".to_owned(),
            display_name: "Demo large".to_owned(),
            description: Some("Thinks harder.".to_owned()),
            context_window: Some(200_000),
            reasoning_levels: vec!["low".to_owned(), "medium".to_owned(), "high".to_owned()],
            default_reasoning: Some("medium".to_owned()),
            speeds,
            default_speed: Some("standard".to_owned()),
            is_default: true,
            ..ModelEntry::default()
        },
    ]
}

/// What the demo harness received for one turn, so tests can check that run
/// settings reached the adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoTurnRecord {
    pub thread_id: String,
    pub turn_id: String,
    pub prompt: String,
    pub config: RunConfig,
    pub status: String,
    pub final_message: Option<String>,
}

#[derive(Default)]
struct DemoAccount {
    stopping: Arc<AtomicBool>,
    /// Demo accounts start signed in; signing out sticks until a key is used.
    logged_out: bool,
    threads: HashMap<String, Vec<DemoTurnRecord>>,
    /// What `read_usage` reports, when a test set it.
    usage: Option<UsageReport>,
}

pub struct DemoAdapter {
    manifest: AdapterManifest,
    shutdown: Arc<ShutdownSignal>,
    accounts: Mutex<HashMap<Uuid, DemoAccount>>,
    run_slots: Mutex<HashMap<Uuid, usize>>,
    running: Arc<AtomicUsize>,
    host_limit: usize,
}

impl DemoAdapter {
    pub fn new(shutdown: Arc<ShutdownSignal>, host_limit: usize) -> Self {
        Self {
            manifest: demo_manifest(),
            shutdown,
            accounts: Mutex::new(HashMap::new()),
            run_slots: Mutex::new(HashMap::new()),
            running: Arc::new(AtomicUsize::new(0)),
            host_limit,
        }
    }

    /// Make the account report `usage` from now on, as if its plan changed.
    pub fn set_usage(&self, account_id: Uuid, usage: UsageReport) {
        lock(&self.accounts).entry(account_id).or_default().usage = Some(usage);
    }

    /// Every turn the account started, oldest first.
    pub fn turns(&self, account_id: Uuid) -> Vec<DemoTurnRecord> {
        let accounts = lock(&self.accounts);
        let mut turns = accounts
            .get(&account_id)
            .map(|account| {
                account
                    .threads
                    .values()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        turns.sort_by(|left, right| left.turn_id.cmp(&right.turn_id));
        turns
    }

    fn session(
        self: &Arc<Self>,
        account: AccountHandle,
        run_slot: bool,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        if self.shutdown.is_shutting_down() {
            return Err(HarnessError::ShuttingDown);
        }
        let stopping = {
            let mut accounts = lock(&self.accounts);
            let entry = accounts.entry(account.id).or_default();
            Arc::clone(&entry.stopping)
        };
        if run_slot {
            let mut slots = lock(&self.run_slots);
            if self.running.load(Ordering::Acquire) >= self.host_limit {
                return Err(HarnessError::HostCapacity);
            }
            let used = slots.entry(account.id).or_default();
            if *used >= usize::from(account.max_concurrent_runs.max(1)) {
                return Err(HarnessError::AccountCapacity);
            }
            *used += 1;
            self.running.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Box::new(DemoSession {
            adapter: Arc::clone(self),
            account_id: account.id,
            stopping,
            run_slot,
            queue: VecDeque::new(),
            active: None,
        }))
    }
}

/// Keeps the adapter itself behind an `Arc` so sessions can reach shared
/// account state.
pub struct DemoHarness(pub Arc<DemoAdapter>);

impl HarnessAdapter for DemoHarness {
    fn manifest(&self) -> &AdapterManifest {
        &self.0.manifest
    }

    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        let result = self.0.session(account, true);
        Box::pin(async move { result })
    }

    fn acquire_control(
        &self,
        account: AccountHandle,
    ) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        let result = self.0.session(account, false);
        Box::pin(async move { result })
    }

    fn health(
        &self,
        account_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Option<ProfileHealth>> + Send + '_>> {
        let known = lock(&self.0.accounts).contains_key(&account_id);
        Box::pin(async move {
            known.then_some(ProfileHealth {
                process_running: true,
                consecutive_failures: 0,
                circuit_open: false,
                process_starts: 1,
            })
        })
    }

    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        // Stops the account's sessions; its sign-in state stays, as a real
        // harness keeps it on disk.
        if let Some(account) = lock(&self.0.accounts).get_mut(&account_id) {
            account.stopping.store(true, Ordering::Release);
            account.stopping = Default::default();
        }
        Box::pin(async {})
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        for account in lock(&self.0.accounts).values() {
            account.stopping.store(true, Ordering::Release);
        }
        Box::pin(async {})
    }
}

struct ActiveTurn {
    thread_id: String,
    turn_id: String,
    cwd: PathBuf,
    proof_line: String,
}

struct DemoSession {
    adapter: Arc<DemoAdapter>,
    account_id: Uuid,
    stopping: Arc<AtomicBool>,
    run_slot: bool,
    queue: VecDeque<HarnessEvent>,
    active: Option<ActiveTurn>,
}

impl Drop for DemoSession {
    fn drop(&mut self) {
        if self.run_slot {
            let mut slots = lock(&self.adapter.run_slots);
            if let Some(used) = slots.get_mut(&self.account_id) {
                *used = used.saturating_sub(1);
            }
            self.adapter.running.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl DemoSession {
    fn with_account<T>(&self, apply: impl FnOnce(&mut DemoAccount) -> T) -> T {
        let mut accounts = lock(&self.adapter.accounts);
        apply(accounts.entry(self.account_id).or_default())
    }

    fn set_turn_status(&self, thread_id: &str, turn_id: &str, status: &str, message: Option<&str>) {
        self.with_account(|account| {
            if let Some(turn) = account
                .threads
                .get_mut(thread_id)
                .and_then(|turns| turns.iter_mut().find(|turn| turn.turn_id == turn_id))
            {
                turn.status = status.to_owned();
                turn.final_message = message.map(str::to_owned);
            }
        });
    }

    fn finish_turn(&mut self) -> Result<(), HarnessError> {
        let Some(turn) = self.active.take() else {
            return Err(HarnessError::MissingResult("active turn"));
        };
        let known_file = turn.cwd.join("KNOWN_FILE.txt");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&known_file)
            .map_err(|error| HarnessError::Transport(Box::new(error)))?;
        writeln!(file, "{}", turn.proof_line)
            .map_err(|error| HarnessError::Transport(Box::new(error)))?;
        let item = |kind| HarnessItem {
            key: format!("demo:{}:file", turn.turn_id),
            kind,
        };
        self.queue
            .push_back(HarnessEvent::ItemStarted(item(ItemKind::FileChange)));
        self.queue
            .push_back(HarnessEvent::ItemCompleted(item(ItemKind::FileChange)));
        self.queue
            .push_back(HarnessEvent::MessageDelta(DEMO_FINAL_MESSAGE.to_owned()));
        self.queue
            .push_back(HarnessEvent::TurnCompleted { completed: true });
        self.set_turn_status(
            &turn.thread_id,
            &turn.turn_id,
            "completed",
            Some(DEMO_FINAL_MESSAGE),
        );
        Ok(())
    }
}

impl HarnessSession for DemoSession {
    fn account_id(&self) -> Uuid {
        self.account_id
    }

    fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    fn start_thread<'a>(
        &'a mut self,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        let thread_id = format!("demo-thread-{}", Uuid::now_v7());
        self.with_account(|account| {
            account.threads.insert(thread_id.clone(), Vec::new());
        });
        Box::pin(async move { Ok(thread_id) })
    }

    fn resume_thread<'a>(
        &'a mut self,
        thread_id: &'a str,
        _cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        let known = self.with_account(|account| account.threads.contains_key(thread_id));
        Box::pin(async move {
            if known {
                Ok(())
            } else {
                Err(HarnessError::MissingResult("thread"))
            }
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
            let turn_id = format!("demo-turn-{}", Uuid::now_v7());
            let known = self.with_account(|account| {
                let Some(turns) = account.threads.get_mut(thread_id) else {
                    return false;
                };
                turns.push(DemoTurnRecord {
                    thread_id: thread_id.to_owned(),
                    turn_id: turn_id.clone(),
                    prompt: prompt.to_owned(),
                    config: config.clone(),
                    status: "inProgress".to_owned(),
                    final_message: None,
                });
                true
            });
            if !known {
                return Err(HarnessError::MissingResult("thread"));
            }
            let proof_line = config
                .options
                .get("proofLine")
                .and_then(Value::as_str)
                .unwrap_or(DEMO_PROOF_LINE)
                .to_owned();
            self.active = Some(ActiveTurn {
                thread_id: thread_id.to_owned(),
                turn_id: turn_id.clone(),
                cwd: cwd.to_path_buf(),
                proof_line,
            });
            self.queue.push_back(HarnessEvent::Request(HarnessRequest {
                kind: RequestKind::Approval,
                durable_id: format!("demo-approval-{turn_id}"),
                summary: DEMO_APPROVAL_SUMMARY.to_owned(),
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
                token: json!({"turnId": turn_id}),
            }));
            Ok(turn_id)
        })
    }

    fn interrupt_turn<'a>(
        &'a mut self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, ()> {
        self.set_turn_status(thread_id, turn_id, "interrupted", None);
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
        {
            self.active = None;
            self.queue.clear();
        }
        Box::pin(async { Ok(()) })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(async move {
            if self.is_stopping() {
                return Err(HarnessError::ShuttingDown);
            }
            match self.queue.pop_front() {
                Some(event) => Ok(event),
                // Nothing more will happen until someone answers or the turn
                // is interrupted; the caller polls for both.
                None => std::future::pending().await,
            }
        })
    }

    fn answer(&mut self, request: HarnessRequest, answer: HarnessAnswer) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            let turn_id = request.token.get("turnId").and_then(Value::as_str);
            if turn_id.is_none()
                || self.active.as_ref().map(|active| active.turn_id.as_str()) != turn_id
            {
                return Ok(());
            }
            match answer {
                // An answered question is a go-ahead, as an approval is.
                HarnessAnswer::Approve | HarnessAnswer::Text(_) => self.finish_turn(),
                HarnessAnswer::Deny => {
                    if let Some(turn) = self.active.take() {
                        self.set_turn_status(&turn.thread_id, &turn.turn_id, "interrupted", None);
                    }
                    Ok(())
                }
            }
        })
    }

    fn read_latest_turn<'a>(&'a mut self, thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        let latest = self.with_account(|account| {
            account
                .threads
                .get(thread_id)
                .and_then(|turns| turns.last().cloned())
        });
        Box::pin(async move {
            Ok(match latest {
                Some(turn) => RecoveredTurn {
                    id: Some(turn.turn_id),
                    status: turn.status,
                    final_message: turn.final_message,
                },
                None => RecoveredTurn {
                    id: None,
                    status: "missing".to_owned(),
                    final_message: None,
                },
            })
        })
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary> {
        let authenticated = self.with_account(|account| !account.logged_out);
        Box::pin(async move {
            Ok(AccountSummary {
                authenticated,
                account_type: authenticated.then(|| "demo".to_owned()),
                email: None,
            })
        })
    }

    fn start_login(&mut self, _kind: LoginKind) -> HarnessFuture<'_, LoginStart> {
        Box::pin(async { Err(HarnessError::Unsupported("browser sign-in")) })
    }

    fn login_with_api_key<'a>(&'a mut self, api_key: &'a str) -> HarnessFuture<'a, LoginStart> {
        let valid = (8..=4096).contains(&api_key.len()) && !api_key.chars().any(char::is_control);
        if valid {
            self.with_account(|account| account.logged_out = false);
        }
        Box::pin(async move {
            if valid {
                Ok(LoginStart::ApiKey)
            } else {
                Err(HarnessError::InvalidResponse)
            }
        })
    }

    fn cancel_login<'a>(&'a mut self, _login_id: &'a str) -> HarnessFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn logout(&mut self) -> HarnessFuture<'_, ()> {
        self.with_account(|account| account.logged_out = true);
        Box::pin(async { Ok(()) })
    }

    fn read_rate_limits(&mut self) -> HarnessFuture<'_, RateLimitSummary> {
        Box::pin(async {
            Ok(RateLimitSummary {
                primary: Some(RateLimitWindow {
                    used_percent: 0,
                    resets_at: None,
                    window_duration_minutes: Some(300),
                }),
                secondary: None,
            })
        })
    }

    fn read_usage(&mut self) -> HarnessFuture<'_, UsageReport> {
        let usage = self.with_account(|account| account.usage.clone());
        Box::pin(async move {
            match usage {
                Some(usage) => Ok(usage),
                None => Ok(UsageReport::from_rate_limits(
                    self.read_rate_limits().await?,
                )),
            }
        })
    }

    /// Spending a banked reset empties every window, like Codex's does.
    fn spend_reset_credit<'a>(
        &'a mut self,
        credit_id: Option<&'a str>,
        _idempotency_key: &'a str,
    ) -> HarnessFuture<'a, ResetOutcome> {
        let outcome = self.with_account(|account| {
            let Some(usage) = account.usage.as_mut() else {
                return ResetOutcome::NoCredit;
            };
            let Some(credits) = usage.reset_credits.as_mut().filter(|c| c.available > 0) else {
                return ResetOutcome::NoCredit;
            };
            if let Some(id) = credit_id {
                if !credits.credits.iter().any(|credit| credit.id == id) {
                    return ResetOutcome::NoCredit;
                }
                credits.credits.retain(|credit| credit.id != id);
            } else if !credits.credits.is_empty() {
                credits.credits.remove(0);
            }
            credits.available -= 1;
            for window in &mut usage.windows {
                window.used_percent = 0;
                window.limit_reached = false;
            }
            ResetOutcome::Reset
        });
        Box::pin(async move { Ok(outcome) })
    }

    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>> {
        Box::pin(async { Ok(demo_models()) })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.queue.clear();
        Box::pin(async {})
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use uuid::Uuid;

    use super::{DemoAdapter, DemoHarness};
    use crate::provider::{
        adapter::{AccountHandle, HarnessAnswer, HarnessEvent},
        contract_tests::adapter_contract,
        registry::ShutdownSignal,
        run_config::RunConfig,
    };

    fn account(max_concurrent_runs: u16) -> AccountHandle {
        AccountHandle {
            id: Uuid::now_v7(),
            home: PathBuf::from("/nonexistent/demo-home"),
            max_concurrent_runs,
        }
    }

    #[tokio::test]
    async fn demo_adapter_keeps_the_adapter_contract() {
        let worktree = tempfile::tempdir().expect("worktree");
        let adapter = DemoHarness(Arc::new(DemoAdapter::new(
            Arc::new(ShutdownSignal::default()),
            8,
        )));
        adapter_contract(&adapter, account(2), worktree.path()).await;
    }

    #[tokio::test]
    async fn approving_the_demo_turn_writes_its_proof_and_records_the_run_settings() {
        let worktree = tempfile::tempdir().expect("worktree");
        std::fs::write(worktree.path().join("KNOWN_FILE.txt"), "start\n").expect("known file");
        let demo = Arc::new(DemoAdapter::new(Arc::new(ShutdownSignal::default()), 8));
        let adapter = DemoHarness(Arc::clone(&demo));
        let account = account(1);
        let config = RunConfig {
            model: Some("demo-small".to_owned()),
            reasoning: Some("high".to_owned()),
            ..RunConfig::default()
        };
        let mut run =
            crate::provider::adapter::HarnessAdapter::acquire_run(&adapter, account.clone())
                .await
                .expect("run session");
        let thread = run
            .start_thread(worktree.path(), &config)
            .await
            .expect("thread");
        run.start_turn(&thread, worktree.path(), "prove it", &config)
            .await
            .expect("turn");
        let HarnessEvent::Request(request) = run.next_event().await.expect("approval") else {
            panic!("the demo turn asks for approval first");
        };
        run.answer(request, HarnessAnswer::Approve)
            .await
            .expect("approve");
        let mut completed = false;
        for _ in 0..10 {
            if let HarnessEvent::TurnCompleted { completed: done } =
                run.next_event().await.expect("event")
            {
                completed = done;
                break;
            }
        }
        assert!(completed);
        let proof = std::fs::read_to_string(worktree.path().join("KNOWN_FILE.txt")).expect("proof");
        assert!(proof.contains(super::DEMO_PROOF_LINE));
        let turns = demo.turns(account.id);
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].config, config,
            "the adapter received the run's settings"
        );
    }
}
