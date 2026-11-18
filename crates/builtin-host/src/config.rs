//! What the built-in host is configured with: where its data lives, the
//! harness programs it runs and the limits it runs them under. The server
//! reads these from its environment; tests build them directly.

use std::{path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub struct HarnessConfig {
    /// Where mirrors, worktrees and account homes live.
    pub data_root: PathBuf,
    pub codex_binary: PathBuf,
    /// Runs one harness process may hold at once.
    pub codex_host_concurrency_limit: usize,
    pub codex_max_line_bytes: usize,
    pub codex_max_stderr_bytes: usize,
    pub codex_max_buffered_events: usize,
    pub codex_request_timeout: Duration,
    pub codex_shutdown_timeout: Duration,
    pub codex_cloud_poll_interval: Duration,
    pub claude_binary: PathBuf,
    pub anthropic_api_url: String,
    /// Development only: every run goes to the demo harness.
    pub demo_executor: bool,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            data_root: PathBuf::from(".agent-workspace"),
            codex_binary: PathBuf::from("codex"),
            codex_host_concurrency_limit: 1,
            codex_max_line_bytes: 1_048_576,
            codex_max_stderr_bytes: 262_144,
            codex_max_buffered_events: 64,
            codex_request_timeout: Duration::from_secs(120),
            codex_shutdown_timeout: Duration::from_secs(5),
            codex_cloud_poll_interval: Duration::from_millis(15_000),
            claude_binary: PathBuf::from("claude"),
            anthropic_api_url: "https://api.anthropic.com".to_owned(),
            demo_executor: false,
        }
    }
}
