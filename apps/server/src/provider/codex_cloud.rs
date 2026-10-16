//! The Codex Cloud adapter (C2): runs a turn as a Codex Cloud task in one of
//! the project's cloud environments, under the person's own Codex sign-in.
//!
//! It drives the `codex` command line rather than an API: `codex cloud exec`
//! starts the task, `codex cloud status` follows it, and `codex cloud diff`
//! brings the result back, which is applied to the run's worktree and
//! committed so the run publishes and opens a pull request like any other.
//! The command line's output formats stay inside this module. Sign-in,
//! account status and sign-out are the Codex App Server's, on the same
//! account home, so a Codex Cloud account signs in exactly like Codex.
//!
//! Checked against codex-cli 0.163.0: `exec` prints the task's URL, `status`
//! prints `[READY] title`, a meta line and a diff summary (and exits 1 until
//! the task is ready), and `diff` prints the unified diff.

use std::{
    collections::HashMap,
    ffi::OsString,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use serde_json::json;
use tokio::{io::AsyncReadExt, io::AsyncWriteExt, sync::mpsc, task::JoinHandle, time::timeout};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, AccountSummary, HarnessAdapter, HarnessAnswer, HarnessError, HarnessEvent,
        HarnessFuture, HarnessItem, HarnessRequest, HarnessSession, ItemKind, LoginKind,
        LoginStart, ProfileHealth, RateLimitSummary, RecoveredTurn,
    },
    manifest::{
        AdapterFeatures, AdapterManifest, ConcurrencyLimits, LoginMethod, ModelEntry,
        RunLocation, RunOptionSupport, VersionRange,
    },
    registry::ShutdownSignal,
    run_config::RunConfig,
};
use crate::{config::AppConfig, git_runtime::absolute_runtime_path};

pub const CODEX_CLOUD_KIND: &str = "codex_cloud";

/// How long one command may take. `exec` uploads the prompt and resolves
/// the environment; `diff` downloads the result.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;
/// Status reads that may fail in a row (network blips, a sign-in refresh)
/// before the run gives up on the task.
const MAX_STATUS_FAILURES: u32 = 20;
const MAX_PROMPT_BYTES: usize = 256 * 1024;

pub fn codex_cloud_manifest() -> AdapterManifest {
    AdapterManifest {
        kind: CODEX_CLOUD_KIND.to_owned(),
        display_name: "Codex Cloud".to_owned(),
        description: "OpenAI Codex in one of your Codex Cloud environments. The result comes back to this thread's worktree, ready to publish as a pull request.".to_owned(),
        versions: VersionRange {
            minimum: "0.146.0".to_owned(),
            below: Some("1.0.0".to_owned()),
        },
        branch_prefix: "codex-cloud/run-".to_owned(),
        runs_on: RunLocation::Cloud,
        features: AdapterFeatures {
            streaming: false,
            commands: false,
            file_changes: true,
            approvals: false,
            questions: false,
            resume: true,
            interrupt: false,
            cloud: true,
            multiple_attempts: true,
            concurrent_runs: true,
            model_listing: false,
            pull_request_creation: false,
        },
        concurrency: ConcurrencyLimits {
            default_per_account: 3,
            max_per_account: 8,
        },
        // Codex Cloud needs a ChatGPT sign-in; an API key can't start tasks.
        login_methods: vec![
            LoginMethod {
                kind: LoginKind::Browser,
                label: "Sign in with ChatGPT".to_owned(),
            },
            LoginMethod {
                kind: LoginKind::DeviceCode,
                label: "Use a device code".to_owned(),
            },
        ],
        run_options: RunOptionSupport {
            sandbox_modes: Vec::new(),
            approval_policies: Vec::new(),
            max_attempts: 4,
        },
        options_schema: json!({
            "type": "object",
            "properties": {
                "environmentId": {
                    "type": "string",
                    "title": "Codex Cloud environment",
                    "description": "The environment's ID or label, as Codex Cloud shows it.",
                    "maxLength": 200
                },
                "branch": {
                    "type": "string",
                    "title": "Branch",
                    "description": "The branch the task starts from. Unset, the repository's default branch.",
                    "maxLength": 200
                }
            },
            "additionalProperties": false
        }),
        default_models: Vec::new(),
    }
}

/// Where a cloud task stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TaskState {
    Pending,
    Ready,
    Applied,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskStatus {
    pub state: TaskState,
    pub title: String,
    /// "+12/-3 • 2 files", or "no diff".
    pub summary: Option<String>,
}

/// Runs the `codex cloud` subcommands for one account home.
#[derive(Clone)]
struct CloudCli {
    binary: PathBuf,
    neutral_cwd: PathBuf,
}

struct CommandOutput {
    success: bool,
    stdout: Vec<u8>,
}

impl CloudCli {
    async fn run(
        &self,
        home: &Path,
        arguments: Vec<OsString>,
        input: Option<&[u8]>,
    ) -> Result<CommandOutput, HarnessError> {
        let binary_parent = self
            .binary
            .parent()
            .ok_or(HarnessError::InvalidConfiguration)?;
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .arg("cloud")
            .args(arguments)
            .current_dir(&self.neutral_cwd)
            .env_clear()
            .env("CODEX_HOME", home)
            .env("HOME", home)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", binary_parent.display()),
            )
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("NO_COLOR", "1")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(transport)?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or(HarnessError::InvalidResponse)?;
        let mut stderr = child.stderr.take().ok_or(HarnessError::InvalidResponse)?;
        let input = input.map(<[u8]>::to_vec);
        let work = async move {
            if let (Some(input), Some(mut pipe)) = (input, stdin.take()) {
                pipe.write_all(&input).await.map_err(transport)?;
                drop(pipe);
            }
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut limited_out = (&mut stdout).take(MAX_OUTPUT_BYTES as u64 + 1);
            let mut limited_err = (&mut stderr).take(MAX_ERROR_BYTES as u64);
            let (read_out, read_err) = tokio::join!(
                limited_out.read_to_end(&mut out),
                limited_err.read_to_end(&mut err),
            );
            read_out.map_err(transport)?;
            read_err.map_err(transport)?;
            if out.len() > MAX_OUTPUT_BYTES {
                return Err(HarnessError::InvalidResponse);
            }
            let status = child.wait().await.map_err(transport)?;
            if !status.success() {
                tracing::debug!(
                    stderr_bytes = err.len(),
                    "codex cloud command exited unsuccessfully"
                );
            }
            Ok(CommandOutput {
                success: status.success(),
                stdout: out,
            })
        };
        timeout(COMMAND_TIMEOUT, work)
            .await
            .map_err(|_| HarnessError::MissingResult("a timely command"))?
    }

    /// Starts a task and returns its ID and URL.
    async fn exec(
        &self,
        home: &Path,
        environment: &str,
        branch: Option<&str>,
        attempts: u8,
        prompt: &str,
    ) -> Result<(String, String), HarnessError> {
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err(HarnessError::InvalidConfiguration);
        }
        let mut arguments = vec![
            OsString::from("exec"),
            OsString::from("--env"),
            OsString::from(environment),
            OsString::from("--attempts"),
            OsString::from(attempts.to_string()),
        ];
        if let Some(branch) = branch {
            arguments.push(OsString::from("--branch"));
            arguments.push(OsString::from(branch));
        }
        // The prompt goes through stdin ("-"), never the argument list.
        arguments.push(OsString::from("-"));
        let output = self.run(home, arguments, Some(prompt.as_bytes())).await?;
        if !output.success {
            return Err(HarnessError::MissingResult("a started cloud task"));
        }
        let url = String::from_utf8(output.stdout)
            .map_err(|_| HarnessError::InvalidResponse)?
            .lines()
            .map(str::trim)
            .rfind(|line| line.starts_with("https://"))
            .ok_or(HarnessError::MissingResult("the task's URL"))?
            .to_owned();
        let id = task_id_from_url(&url).ok_or(HarnessError::InvalidResponse)?;
        Ok((id, url))
    }

    async fn status(&self, home: &Path, task_id: &str) -> Result<TaskStatus, HarnessError> {
        // `status` exits 1 for any task that isn't ready, so the output, not
        // the exit code, says where the task stands.
        let output = self
            .run(
                home,
                vec![OsString::from("status"), OsString::from(task_id)],
                None,
            )
            .await?;
        let text = String::from_utf8(output.stdout).map_err(|_| HarnessError::InvalidResponse)?;
        parse_status(&text).ok_or(if output.success {
            HarnessError::InvalidResponse
        } else {
            HarnessError::MissingResult("the task's status")
        })
    }

    async fn diff(&self, home: &Path, task_id: &str) -> Result<Vec<u8>, HarnessError> {
        let output = self
            .run(
                home,
                vec![OsString::from("diff"), OsString::from(task_id)],
                None,
            )
            .await?;
        if !output.success {
            return Err(HarnessError::MissingResult("the task's diff"));
        }
        Ok(output.stdout)
    }

    /// Checks that the account can see the environment, by listing at most
    /// one of its tasks (the command resolves the ID or label first).
    async fn check_environment(&self, home: &Path, environment: &str) -> Result<(), HarnessError> {
        let output = self
            .run(
                home,
                vec![
                    OsString::from("list"),
                    OsString::from("--env"),
                    OsString::from(environment),
                    OsString::from("--limit"),
                    OsString::from("1"),
                    OsString::from("--json"),
                ],
                None,
            )
            .await?;
        if !output.success {
            return Err(HarnessError::MissingResult("the environment"));
        }
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .ok()
            .filter(|value| value.get("tasks").is_some_and(serde_json::Value::is_array))
            .map(|_| ())
            .ok_or(HarnessError::InvalidResponse)
    }
}

/// The task ID is the last path segment of its URL.
pub(crate) fn task_id_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let id = path.trim_end_matches('/').rsplit('/').next()?.trim();
    (!id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| id.to_owned())
}

pub(crate) fn parse_status(text: &str) -> Option<TaskStatus> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next()?;
    let rest = first.strip_prefix('[')?;
    let (label, title) = rest.split_once(']')?;
    let state = match label.trim() {
        "PENDING" => TaskState::Pending,
        "READY" => TaskState::Ready,
        "APPLIED" => TaskState::Applied,
        "ERROR" => TaskState::Error,
        _ => return None,
    };
    // The meta line (environment and age) is skipped; the last line is the
    // diff summary.
    let summary = lines.last().map(str::to_owned);
    Some(TaskStatus {
        state,
        title: title.trim().chars().take(200).collect(),
        summary,
    })
}

/// What a finished task looks like in the thread.
fn finished_message(status: &TaskStatus, url: &str, committed: Option<&str>) -> String {
    let summary = status
        .summary
        .as_deref()
        .filter(|summary| !summary.is_empty())
        .unwrap_or("no diff");
    match committed {
        Some(commit) => format!(
            "Codex Cloud finished \"{}\" ({summary}). The changes are committed on this thread's run branch ({}), ready to publish as a pull request.\n\nTask: {url}",
            status.title,
            &commit[..commit.len().min(12)],
        ),
        None => format!(
            "Codex Cloud finished \"{}\" with no changes to the code.\n\nTask: {url}",
            status.title
        ),
    }
}

fn transport(error: impl std::error::Error + Send + Sync + 'static) -> HarnessError {
    HarnessError::Transport(Box::new(error))
}

/// Run slots per account, held by run sessions until they drop.
#[derive(Default)]
struct Slots {
    in_use: StdMutex<HashMap<Uuid, u16>>,
}

struct SlotGuard {
    slots: Arc<Slots>,
    account_id: Uuid,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut in_use = self
            .slots
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = in_use.get_mut(&self.account_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_use.remove(&self.account_id);
            }
        }
    }
}

pub struct CodexCloudAdapter {
    manifest: AdapterManifest,
    cli: CloudCli,
    poll_interval: Duration,
    slots: Arc<Slots>,
    shutdown: Arc<ShutdownSignal>,
    /// Sign-in, account status and sign-out go through Codex itself.
    codex: Arc<dyn HarnessAdapter>,
}

impl CodexCloudAdapter {
    pub fn new(
        config: &AppConfig,
        shutdown: Arc<ShutdownSignal>,
        codex: Arc<dyn HarnessAdapter>,
    ) -> Result<Self, HarnessError> {
        Ok(Self {
            manifest: codex_cloud_manifest(),
            cli: CloudCli {
                binary: absolute_runtime_path(&config.codex_binary)
                    .map_err(|_| HarnessError::InvalidConfiguration)?,
                neutral_cwd: absolute_runtime_path(&config.data_root)
                    .map_err(|_| HarnessError::InvalidConfiguration)?,
            },
            poll_interval: config.codex_cloud_poll_interval,
            slots: Arc::new(Slots::default()),
            shutdown,
            codex,
        })
    }

    fn session(&self, account: AccountHandle, slot: Option<SlotGuard>) -> CloudSession {
        CloudSession {
            account,
            cli: self.cli.clone(),
            poll_interval: self.poll_interval,
            codex: Arc::clone(&self.codex),
            control: None,
            cwd: None,
            urls: HashMap::new(),
            events: None,
            follower: None,
            _slot: slot,
        }
    }

    /// Checks that `environment` exists for the account (C1's "test it").
    pub async fn check_environment(
        &self,
        account: &AccountHandle,
        environment: &str,
    ) -> Result<(), HarnessError> {
        self.cli.check_environment(&account.home, environment).await
    }
}

impl HarnessAdapter for CodexCloudAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn acquire_run(&self, account: AccountHandle) -> HarnessFuture<'_, Box<dyn HarnessSession>> {
        Box::pin(async move {
            if self.shutdown.is_shutting_down() {
                return Err(HarnessError::ShuttingDown);
            }
            {
                let mut in_use = self
                    .slots
                    .in_use
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let count = in_use.entry(account.id).or_insert(0);
                if *count >= account.max_concurrent_runs.max(1) {
                    return Err(HarnessError::AccountCapacity);
                }
                *count += 1;
            }
            let guard = SlotGuard {
                slots: Arc::clone(&self.slots),
                account_id: account.id,
            };
            Ok(Box::new(self.session(account, Some(guard))) as Box<dyn HarnessSession>)
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
        Box::pin(async move {
            // Runs live in the cloud; only sign-in uses a local process.
            Some(self.codex.health(account_id).await.unwrap_or_default())
        })
    }

    fn shutdown_account(&self, account_id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.codex.shutdown_account(account_id).await })
    }

    fn shutdown_all(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        // Codex's own processes are stopped by its adapter; cloud tasks keep
        // running and are followed again after a restart.
        Box::pin(async {})
    }
}

struct CloudSession {
    account: AccountHandle,
    cli: CloudCli,
    poll_interval: Duration,
    codex: Arc<dyn HarnessAdapter>,
    /// A Codex control session for sign-in and account pages, opened on
    /// first use.
    control: Option<Box<dyn HarnessSession>>,
    /// The run's worktree, where the result is applied.
    cwd: Option<PathBuf>,
    urls: HashMap<String, String>,
    events: Option<mpsc::Receiver<HarnessEvent>>,
    follower: Option<JoinHandle<()>>,
    _slot: Option<SlotGuard>,
}

impl Drop for CloudSession {
    fn drop(&mut self) {
        if let Some(follower) = self.follower.take() {
            follower.abort();
        }
    }
}

impl CloudSession {
    async fn control(&mut self) -> Result<&mut Box<dyn HarnessSession>, HarnessError> {
        if self.control.is_none() {
            self.control = Some(self.codex.acquire_control(self.account.clone()).await?);
        }
        self.control.as_mut().ok_or(HarnessError::ShuttingDown)
    }

    fn follow(&mut self, task_id: String, url: String, cwd: PathBuf) {
        if let Some(previous) = self.follower.take() {
            previous.abort();
        }
        // Events go through a channel so `next_event` can be dropped and
        // called again (the run loop does, every half second) without losing
        // a status read halfway through.
        let (sender, receiver) = mpsc::channel(16);
        self.events = Some(receiver);
        self.urls.insert(task_id.clone(), url.clone());
        let follower = Follower {
            cli: self.cli.clone(),
            home: self.account.home.clone(),
            poll_interval: self.poll_interval,
            task_id,
            url,
            cwd,
            sender,
        };
        self.follower = Some(tokio::spawn(follower.run()));
    }
}

struct Follower {
    cli: CloudCli,
    home: PathBuf,
    poll_interval: Duration,
    task_id: String,
    url: String,
    cwd: PathBuf,
    sender: mpsc::Sender<HarnessEvent>,
}

impl Follower {
    async fn run(self) {
        let started = HarnessItem {
            key: format!("cloud-task:{}", self.task_id),
            kind: ItemKind::Other,
        };
        if self
            .sender
            .send(HarnessEvent::ItemStarted(started))
            .await
            .is_err()
        {
            return;
        }
        let mut failures = 0;
        loop {
            tokio::time::sleep(self.poll_interval).await;
            let status = match self.cli.status(&self.home, &self.task_id).await {
                Ok(status) => {
                    failures = 0;
                    status
                }
                Err(error) => {
                    failures += 1;
                    tracing::warn!(?error, failures, "Codex Cloud status read failed");
                    if failures >= MAX_STATUS_FAILURES {
                        let _ = self.sender.send(HarnessEvent::Error).await;
                        return;
                    }
                    continue;
                }
            };
            let events = match status.state {
                TaskState::Pending => continue,
                TaskState::Error => vec![
                    HarnessEvent::MessageDelta(format!(
                        "Codex Cloud couldn't finish \"{}\". Open the task for details: {}",
                        status.title, self.url
                    )),
                    HarnessEvent::TurnCompleted { completed: false },
                ],
                TaskState::Ready | TaskState::Applied => {
                    match bring_back(&self.cli, &self.home, &self.task_id, &self.cwd, &status)
                        .await
                    {
                        Ok(commit) => vec![
                            HarnessEvent::ItemCompleted(HarnessItem {
                                key: format!("cloud-result:{}", self.task_id),
                                kind: ItemKind::FileChange,
                            }),
                            HarnessEvent::MessageDelta(finished_message(
                                &status,
                                &self.url,
                                commit.as_deref(),
                            )),
                            HarnessEvent::TurnCompleted { completed: true },
                        ],
                        Err(error) => {
                            tracing::warn!(?error, "Codex Cloud result could not be applied");
                            failures += 1;
                            if failures >= MAX_STATUS_FAILURES {
                                vec![
                                    HarnessEvent::MessageDelta(format!(
                                        "Codex Cloud finished, but its changes couldn't be applied here. Open the task to review them: {}",
                                        self.url
                                    )),
                                    HarnessEvent::TurnCompleted { completed: false },
                                ]
                            } else {
                                continue;
                            }
                        }
                    }
                }
            };
            for event in events {
                if self.sender.send(event).await.is_err() {
                    return;
                }
            }
            return;
        }
    }
}

/// Downloads a finished task's diff and commits it on the run branch.
async fn bring_back(
    cli: &CloudCli,
    home: &Path,
    task_id: &str,
    cwd: &Path,
    status: &TaskStatus,
) -> Result<Option<String>, HarnessError> {
    let diff = cli.diff(home, task_id).await?;
    let cwd = cwd.to_path_buf();
    let task_id = task_id.to_owned();
    let subject = status.title.clone();
    tokio::task::spawn_blocking(move || {
        crate::git_runtime::apply_result_and_commit(&cwd, &diff, &task_id, &subject)
    })
    .await
    .map_err(|_| HarnessError::InvalidResponse)?
    .map_err(|_| HarnessError::MissingResult("an applicable diff"))
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
        cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, String> {
        Box::pin(async move {
            self.cwd = Some(cwd.to_path_buf());
            // Each turn is its own cloud task; the thread is only ours.
            Ok(format!("cloud-{}", Uuid::now_v7()))
        })
    }

    fn resume_thread<'a>(
        &'a mut self,
        _thread_id: &'a str,
        cwd: &'a Path,
        _config: &'a RunConfig,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            self.cwd = Some(cwd.to_path_buf());
            Ok(())
        })
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
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or(HarnessError::InvalidConfiguration)?;
            let branch = config
                .options
                .get("branch")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let attempts = config.attempts.unwrap_or(1).clamp(1, 4);
            let (task_id, url) = self
                .cli
                .exec(&self.account.home, environment, branch, attempts, prompt)
                .await?;
            self.cwd = Some(cwd.to_path_buf());
            self.follow(task_id.clone(), url, cwd.to_path_buf());
            Ok(task_id)
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
            let url = self
                .urls
                .get(turn_id)
                .cloned()
                .unwrap_or_else(|| task_url(turn_id));
            self.cwd = Some(cwd.to_path_buf());
            self.follow(turn_id.to_owned(), url, cwd.to_path_buf());
            Ok(())
        })
    }

    fn turn_link(&self, turn_id: &str) -> Option<String> {
        Some(
            self.urls
                .get(turn_id)
                .cloned()
                .unwrap_or_else(|| task_url(turn_id)),
        )
    }

    fn interrupt_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        _turn_id: &'a str,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            // The command line can't cancel a cloud task; stop following it
            // so nothing it does comes back here.
            if let Some(follower) = self.follower.take() {
                follower.abort();
            }
            self.events = None;
            Err(HarnessError::Unsupported("cancelling a Codex Cloud task"))
        })
    }

    fn next_event(&mut self) -> HarnessFuture<'_, HarnessEvent> {
        Box::pin(async move {
            let receiver = self
                .events
                .as_mut()
                .ok_or(HarnessError::MissingResult("a running turn"))?;
            receiver
                .recv()
                .await
                .ok_or(HarnessError::MissingResult("the task's outcome"))
        })
    }

    fn answer(&mut self, _request: HarnessRequest, _answer: HarnessAnswer) -> HarnessFuture<'_, ()> {
        Box::pin(async { Err(HarnessError::Unsupported("answering requests")) })
    }

    fn read_latest_turn<'a>(&'a mut self, _thread_id: &'a str) -> HarnessFuture<'a, RecoveredTurn> {
        Box::pin(async { Err(HarnessError::Unsupported("reading a thread without its task")) })
    }

    fn read_turn<'a>(
        &'a mut self,
        _thread_id: &'a str,
        turn_id: &'a str,
    ) -> HarnessFuture<'a, RecoveredTurn> {
        Box::pin(async move {
            let status = self.cli.status(&self.account.home, turn_id).await?;
            let url = self
                .urls
                .get(turn_id)
                .cloned()
                .unwrap_or_else(|| task_url(turn_id));
            let (status_name, final_message) = match status.state {
                TaskState::Pending => ("inProgress", None),
                TaskState::Error => ("failed", None),
                TaskState::Ready | TaskState::Applied => {
                    let cwd = self
                        .cwd
                        .clone()
                        .ok_or(HarnessError::MissingResult("the run's worktree"))?;
                    let commit =
                        bring_back(&self.cli, &self.account.home, turn_id, &cwd, &status).await?;
                    (
                        "completed",
                        Some(finished_message(&status, &url, commit.as_deref())),
                    )
                }
            };
            Ok(RecoveredTurn {
                id: Some(turn_id.to_owned()),
                status: status_name.to_owned(),
                final_message,
            })
        })
    }

    fn read_account(&mut self) -> HarnessFuture<'_, AccountSummary> {
        Box::pin(async move { self.control().await?.read_account().await })
    }

    fn start_login(&mut self, kind: LoginKind) -> HarnessFuture<'_, LoginStart> {
        Box::pin(async move {
            if kind == LoginKind::ApiKey {
                return Err(HarnessError::Unsupported("API keys for Codex Cloud"));
            }
            self.control().await?.start_login(kind).await
        })
    }

    fn login_with_api_key<'a>(&'a mut self, _api_key: &'a str) -> HarnessFuture<'a, LoginStart> {
        Box::pin(async { Err(HarnessError::Unsupported("API keys for Codex Cloud")) })
    }

    fn cancel_login<'a>(&'a mut self, login_id: &'a str) -> HarnessFuture<'a, ()> {
        Box::pin(async move { self.control().await?.cancel_login(login_id).await })
    }

    fn logout(&mut self) -> HarnessFuture<'_, ()> {
        Box::pin(async move { self.control().await?.logout().await })
    }

    fn read_rate_limits(&mut self) -> HarnessFuture<'_, RateLimitSummary> {
        Box::pin(async move { self.control().await?.read_rate_limits().await })
    }

    fn list_models(&mut self) -> HarnessFuture<'_, Vec<ModelEntry>> {
        Box::pin(async { Err(HarnessError::Unsupported("listing models")) })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if let Some(follower) = self.follower.take() {
                follower.abort();
            }
            if let Some(mut control) = self.control.take() {
                control.shutdown().await;
            }
        })
    }
}

/// The task's page when the command line didn't say (after a restart).
fn task_url(task_id: &str) -> String {
    format!("https://chatgpt.com/codex/tasks/{task_id}")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};

    use uuid::Uuid;

    use super::{
        CodexCloudAdapter, TaskState, finished_message, parse_status, task_id_from_url,
    };
    use crate::{
        config::AppConfig,
        provider::{
            adapter::{AccountHandle, HarnessAdapter, HarnessEvent},
            codex_app_server::CodexAdapter,
            contract_tests::adapter_contract_with,
            registry::ShutdownSignal,
            run_config::RunConfig,
        },
    };

    #[test]
    fn statuses_are_read_from_the_command_line_text() {
        let ready = parse_status(
            "[READY] Fix the flaky test\nCodex env  •  3m ago\n+12/-3 • 2 files\n",
        )
        .expect("ready");
        assert_eq!(ready.state, TaskState::Ready);
        assert_eq!(ready.title, "Fix the flaky test");
        assert_eq!(ready.summary.as_deref(), Some("+12/-3 • 2 files"));
        assert_eq!(
            parse_status("[PENDING] Something\nenv  •  just now\nno diff")
                .map(|status| status.state),
            Some(TaskState::Pending)
        );
        assert_eq!(
            parse_status("[ERROR] x").map(|status| status.state),
            Some(TaskState::Error)
        );
        assert!(parse_status("Not signed in. Please run 'codex login'").is_none());
        assert!(parse_status("[WEIRD] x").is_none());
    }

    #[test]
    fn task_ids_come_from_the_url() {
        assert_eq!(
            task_id_from_url("https://chatgpt.com/codex/tasks/task_e_123abc").as_deref(),
            Some("task_e_123abc")
        );
        assert_eq!(
            task_id_from_url("https://chatgpt.com/codex/tasks/task_e_1?x=1#y").as_deref(),
            Some("task_e_1")
        );
        assert!(task_id_from_url("https://chatgpt.com/codex/tasks/bad id").is_none());
    }

    #[test]
    fn the_finished_message_links_the_task() {
        let status = parse_status("[READY] Fix it\nenv\n+1/-0 • 1 file").expect("status");
        let message = finished_message(
            &status,
            "https://chatgpt.com/codex/tasks/t1",
            Some("0123456789abcdef0123456789abcdef01234567"),
        );
        assert!(message.contains("0123456789ab"));
        assert!(message.contains("https://chatgpt.com/codex/tasks/t1"));
        assert!(message.contains("+1/-0"));
    }

    struct FakeCloud {
        _temporary: tempfile::TempDir,
        data_root: std::path::PathBuf,
        worktree: std::path::PathBuf,
        log: std::path::PathBuf,
        config: AppConfig,
    }

    /// A stand-in `codex` whose `cloud` commands behave like codex-cli
    /// 0.163: tasks are ready on the second status read.
    #[cfg(unix)]
    fn fake_cloud() -> FakeCloud {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary root");
        let data_root = temporary.path().join("data");
        let worktree = data_root.join("worktrees/one");
        fs::create_dir_all(&worktree).expect("worktree");
        let log = temporary.path().join("calls.log");
        let state = temporary.path().join("state");
        fs::create_dir_all(&state).expect("state");
        let binary = temporary.path().join("fake-codex");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s|CODEX_HOME=%s\n' "$*" "$CODEX_HOME" >> '{log}'
[ "$1" = "cloud" ] || exit 2
shift
case "$1" in
  exec)
    prompt=$(cat)
    printf '%s' "$prompt" > '{state}/prompt'
    n=$(ls '{state}' | wc -l)
    echo "https://chatgpt.com/codex/tasks/task_e_$n"
    ;;
  status)
    count_file='{state}/count-'"$2"
    count=$(cat "$count_file" 2>/dev/null || echo 0)
    count=$((count + 1))
    echo "$count" > "$count_file"
    if [ "$count" -ge 2 ]; then
      printf '[READY] Fix the flaky test\nenv  •  now\nno diff\n'
    else
      printf '[PENDING] Fix the flaky test\nenv  •  now\nno diff\n'
      exit 1
    fi
    ;;
  diff)
    ;;
  list)
    if [ "$3" = "missing" ]; then
      echo "environment 'missing' not found" >&2
      exit 1
    fi
    printf '{{"tasks":[],"cursor":null}}\n'
    ;;
  *) exit 2 ;;
esac
"#,
            log = log.display(),
            state = state.display(),
        );
        fs::write(&binary, script).expect("write fake codex");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("chmod");
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
                "AGENT_WORKSPACE_CODEX_CLOUD_POLL_MILLISECONDS".to_owned(),
                "20".to_owned(),
            ),
        ]))
        .expect("config");
        FakeCloud {
            _temporary: temporary,
            data_root,
            worktree,
            log,
            config,
        }
    }

    fn adapter(fake: &FakeCloud) -> CodexCloudAdapter {
        let shutdown = Arc::new(ShutdownSignal::default());
        let codex = CodexAdapter::new(&fake.config, Arc::clone(&shutdown)).expect("codex");
        CodexCloudAdapter::new(&fake.config, shutdown, Arc::new(codex)).expect("cloud adapter")
    }

    fn account(fake: &FakeCloud, max: u16) -> AccountHandle {
        let home = fake.data_root.join("homes").join(Uuid::now_v7().to_string());
        fs::create_dir_all(&home).expect("home");
        AccountHandle {
            id: Uuid::now_v7(),
            home,
            max_concurrent_runs: max,
        }
    }

    fn with_environment() -> RunConfig {
        let mut config = RunConfig::default();
        config
            .options
            .insert("environmentId".to_owned(), "env_abc".into());
        config
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_cloud_adapter_keeps_the_adapter_contract() {
        let fake = fake_cloud();
        let adapter = adapter(&fake);
        // Account pages go through Codex's own App Server, which the fake
        // doesn't speak; the contract's run half is what this adapter adds.
        adapter_contract_with(&adapter, account(&fake, 2), &fake.worktree, with_environment(), false)
            .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_task_is_started_in_the_environment_and_followed_to_the_end() {
        let fake = fake_cloud();
        let adapter = adapter(&fake);
        let account = account(&fake, 1);
        let mut config = with_environment();
        config.attempts = Some(2);
        config
            .options
            .insert("branch".to_owned(), "main".into());
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        let thread = run
            .start_thread(&fake.worktree, &config)
            .await
            .expect("thread");
        let turn = run
            .start_turn(&thread, &fake.worktree, "Fix the flaky test", &config)
            .await
            .expect("turn");
        assert!(turn.starts_with("task_e_"));
        assert_eq!(
            run.turn_link(&turn).as_deref(),
            Some(format!("https://chatgpt.com/codex/tasks/{turn}").as_str())
        );
        let mut completed = None;
        let mut message = String::new();
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_secs(5), run.next_event())
                .await
                .expect("event in time")
                .expect("event")
            {
                HarnessEvent::MessageDelta(delta) => message.push_str(&delta),
                HarnessEvent::TurnCompleted { completed: done } => {
                    completed = Some(done);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(completed, Some(true));
        assert!(message.contains("no changes"), "{message}");
        let log = fs::read_to_string(&fake.log).expect("log");
        assert!(log.contains("cloud exec --env env_abc --attempts 2 --branch main -"));
        assert!(log.contains(&format!("CODEX_HOME={}", account.home.display())));
        // The prompt went through stdin, never the argument list.
        assert!(!log.contains("Fix the flaky test"));
        // The account's single slot is taken until the run session drops.
        assert!(adapter.acquire_run(account.clone()).await.is_err());
        drop(run);
        assert!(adapter.acquire_run(account).await.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_without_an_environment_is_refused_and_environments_are_checked() {
        let fake = fake_cloud();
        let adapter = adapter(&fake);
        let account = account(&fake, 1);
        let mut run = adapter.acquire_run(account.clone()).await.expect("run");
        let defaults = RunConfig::default();
        let thread = run
            .start_thread(&fake.worktree, &defaults)
            .await
            .expect("thread");
        assert!(
            run.start_turn(&thread, &fake.worktree, "x", &defaults)
                .await
                .is_err()
        );
        adapter
            .check_environment(&account, "env_abc")
            .await
            .expect("known environment");
        assert!(adapter.check_environment(&account, "missing").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_task_started_before_a_restart_is_read_by_its_id() {
        let fake = fake_cloud();
        let adapter = adapter(&fake);
        let account = account(&fake, 1);
        let mut control = adapter
            .acquire_control(account)
            .await
            .expect("control");
        control
            .resume_thread("cloud-1", &fake.worktree, &RunConfig::default())
            .await
            .expect("resume");
        let first = control
            .read_turn("cloud-1", "task_e_9")
            .await
            .expect("first read");
        assert_eq!(first.status, "inProgress");
        assert_eq!(first.id.as_deref(), Some("task_e_9"));
        let second = control
            .read_turn("cloud-1", "task_e_9")
            .await
            .expect("second read");
        assert_eq!(second.status, "completed");
        assert!(
            second
                .final_message
                .is_some_and(|message| message.contains("task_e_9"))
        );
    }
}
