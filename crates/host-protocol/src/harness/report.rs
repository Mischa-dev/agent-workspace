//! What a harness reports as it works and about its account: the steps of
//! a turn, the tokens it used, an account's usage windows and banked
//! resets, and the health of its process. The host fills these in; the
//! server stores and shows them.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ItemKind {
    CommandExecution,
    FileChange,
    #[default]
    Other,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct HarnessItem {
    pub key: String,
    pub kind: ItemKind,
    /// What the item did, for the run page (H5). The run redacts and caps
    /// it before storing; adapters fill in what their harness reports.
    pub detail: ItemDetail,
}

impl HarnessItem {
    pub fn new(key: impl Into<String>, kind: ItemKind) -> Self {
        Self {
            key: key.into(),
            kind,
            detail: ItemDetail::default(),
        }
    }
}

/// The neutral detail of one step.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemDetail {
    /// A tool's name, for steps that are neither commands nor file changes.
    pub tool: Option<String>,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    /// The step's output, as the harness reported it (capped when stored).
    pub output: Option<String>,
    /// Files a change touched, with `added`, `modified` or `deleted`.
    pub files: Vec<(String, Option<String>)>,
    /// Tests a step ran, when the harness counts them.
    pub tests: Option<TestCounts>,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TestCounts {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

/// Tokens a turn used (U2). Adapters report each model call's usage, or a
/// running total split into increments; the run adds them up per model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenUsage {
    /// `None` when the harness doesn't say; the run's model is used.
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    /// What the harness itself says this usage cost, in US dollars.
    pub reported_cost_usd: Option<f64>,
}

/// One usage window of an account (U1): a five-hour session, a week, a
/// month of credits. Plans differ, so an account has whichever windows its
/// harness reports, possibly only a weekly one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageWindow {
    /// Stable for the account across reads, such as `codex.secondary`:
    /// letters, digits and `._-`, at most 64 characters.
    pub key: String,
    /// The harness's own name for the limit, when it gives one.
    pub limit_name: Option<String>,
    pub used_percent: u16,
    /// Unix seconds.
    pub resets_at: Option<i64>,
    pub duration_minutes: Option<i64>,
    /// The harness says this limit is reached.
    pub limit_reached: bool,
}

/// A banked reset (U6): spending one refills the account's windows now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetCredit {
    /// The harness's opaque id for it.
    pub id: String,
    pub title: Option<String>,
    /// Unix seconds; `None` when it doesn't expire.
    pub expires_at: Option<i64>,
    pub granted_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResetCredits {
    pub available: u32,
    /// Detail for some or all of the available ones.
    pub credits: Vec<ResetCredit>,
}

/// Everything a harness says about an account's usage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageReport {
    pub windows: Vec<UsageWindow>,
    /// `None` when the harness has no banked resets.
    pub reset_credits: Option<ResetCredits>,
    /// The plan's name, such as `pro`, when the harness says.
    pub plan: Option<String>,
}

/// How a harness's process for one account is doing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProfileHealth {
    pub process_running: bool,
    pub consecutive_failures: u32,
    pub circuit_open: bool,
    pub process_starts: u64,
}
