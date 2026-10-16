//! W10: AI planning (docs/plans/roadmap.md, AI1 to AI6 and V7).
//!
//! - [`gateway`]: short AI calls with any provider, on keys from settings,
//!   with structured output, a monthly budget and usage per call (AI1).
//! - [`suggestions`]: the review queue. Every suggestion keeps its sources
//!   and is shown or accepted only for someone who can still see them all
//!   (`ai_suggestion_visible_to`). Accepting goes through the same task
//!   commands as a person's own change, marked "via AI" (AI2).
//! - [`detect`]: finds dependencies, blockers and duplicates. Cheap signals
//!   pick candidate pairs, the model judges them in batches. Runs with no
//!   person behind them read only what every project member can see (AI2),
//!   and triage new tasks (AI4).
//! - [`ready`]: readiness and the ranking of what to do next (AI5), and the
//!   dependency graph (V7).
//! - [`plans`]: plan to tasks, resumable in batches (AI3).
//! - [`summaries`]: catch me up on a thread or a task (AI6).
//!
//! The server never puts prompt or reply text in logs or usage rows, and
//! suggestions store a one-line reason, not the model's reply.

pub mod detect;
pub mod gateway;
pub mod http;
pub mod prompts;
pub mod plans;
pub mod providers;
pub mod ready;
pub mod schema;
pub mod summaries;
pub mod suggestions;

/// The AI jobs, for `jobs::default_registry`.
pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    detect::register_jobs(registry);
    plans::register_jobs(registry);
}
