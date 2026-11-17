//! A1.6: pieces of SQL the run service owns, for queries elsewhere that put
//! a run beside their own rows (a thread's run card, a timeline entry's run
//! step, a task's threads, the inbox). The run tables are named only here
//! and in the rest of `src/runs/` (guard G1), so when the built-in host's
//! detail rows move (A1.17, A1.22) these change and their callers don't.
//!
//! Each piece names the aliases it expects and the ones it brings; a piece
//! that takes a column takes it as SQL text from the caller's own query,
//! never from input.

use super::ACTIVE_RUN_STATES;
use crate::domain::RunState as BuiltinState;

/// A list of states as SQL, each quoted, in brackets. From the constants,
/// so no state list is written out twice (guard G4).
fn states_sql(states: impl IntoIterator<Item = String>) -> String {
    let quoted = states
        .into_iter()
        .map(|state| format!("'{state}'"))
        .collect::<Vec<_>>();
    format!("({})", quoted.join(", "))
}

/// The built-in states that hold or wait for harness time.
pub fn builtin_active_states() -> String {
    states_sql(ACTIVE_RUN_STATES.iter().map(|state| (*state).to_owned()))
}

/// The built-in states a run has finished in.
pub fn builtin_terminal_states() -> String {
    states_sql(
        BuiltinState::ALL
            .into_iter()
            .filter(|state| state.is_terminal())
            .map(|state| state.to_string()),
    )
}

/// A conversation entry's run step: brings `event` (`id`, `agent_run_id`,
/// `kind`, `summary`, `created_at`, `payload`) beside `entry`.
pub const ENTRY_EVENT_JOIN: &str =
    "LEFT JOIN agent_run_events event ON event.id = entry.agent_run_event_id";

/// Whether the conversation `conversation` (a column) has a built-in run
/// that hasn't finished: a boolean expression.
pub fn conversation_has_active_run(conversation: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM agent_runs active_run \
         WHERE active_run.conversation_id = {conversation} \
           AND active_run.state IN {})",
        builtin_active_states()
    )
}

/// Whether the repository workspace `workspace` (a column) has a built-in
/// run that hasn't finished: a boolean expression.
pub fn workspace_has_active_run(workspace: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM agent_runs active_run \
         WHERE active_run.repository_workspace_id = {workspace} \
           AND active_run.state IN {})",
        builtin_active_states()
    )
}

/// The state of the newest built-in run in the conversation `conversation`
/// (a column): a scalar expression, NULL when it has none.
pub fn newest_state_in(conversation: &str) -> String {
    format!(
        "(SELECT newest.state FROM agent_runs newest \
          WHERE newest.conversation_id = {conversation} \
          ORDER BY newest.created_at DESC, newest.id DESC LIMIT 1)"
    )
}

/// The newest built-in run in the conversation `conversation` (a column),
/// as a subquery to select from: `id`, `state`.
pub fn newest_in(conversation: &str) -> String {
    format!(
        "SELECT newest.id, newest.state FROM agent_runs newest \
         WHERE newest.conversation_id = {conversation} \
         ORDER BY newest.created_at DESC, newest.id DESC LIMIT 1"
    )
}

/// [`newest_state_in`] for `thread.id`, as a literal for queries built
/// with `concat!`.
macro_rules! newest_state_in_thread {
    () => {
        "(SELECT newest.state FROM agent_runs newest \
          WHERE newest.conversation_id = thread.id \
          ORDER BY newest.created_at DESC, newest.id DESC LIMIT 1)"
    };
}
pub(crate) use newest_state_in_thread;

/// The sequence of the last conversation entry the run `run` (a column)
/// wrote: a scalar expression.
pub fn last_entry_sequence(run: &str) -> String {
    format!(
        "(SELECT max(run_entry.sequence) FROM conversation_entries run_entry \
          JOIN agent_run_events run_event ON run_event.id = run_entry.agent_run_event_id \
          WHERE run_event.agent_run_id = {run})"
    )
}

/// The state of a launch's run: its built-in row's state when it has one
/// (`builtin`, a column), else the state of the run it reports for itself
/// (`run`, a column). A scalar expression.
pub fn launch_run_state(builtin: &str, run: &str) -> String {
    format!(
        "COALESCE(\
           (SELECT detail.state FROM agent_runs detail WHERE detail.id = {builtin}), \
           (SELECT reported.state FROM runs reported \
             WHERE reported.id = {run} AND {builtin} IS NULL))"
    )
}

/// Brings `run` (the built-in row of the run `run_id`, a column) and
/// `run_worker` (the worker that does it) beside the caller's rows, both
/// optional.
pub fn builtin_run_and_worker_join(run_id: &str) -> String {
    format!(
        "LEFT JOIN agent_runs run ON run.id = {run_id} \
         LEFT JOIN execution_workers run_worker ON run_worker.id = run.execution_worker_id"
    )
}

/// A thread's run card: the newest built-in run in conversation `$1` that
/// is active (`active`) or finished, with its branch and pull request, as
/// `conversations::RunRow` reads it.
pub fn thread_card(active: bool) -> String {
    let (queue_position, states) = if active {
        (
            "CASE WHEN run.state = 'queued' THEN (
                SELECT count(*) FROM agent_runs queued
                WHERE queued.state = 'queued'
                  AND (queued.created_at, queued.id) <= (run.created_at, run.id)
            ) END",
            builtin_active_states(),
        )
    } else {
        ("NULL::bigint", builtin_terminal_states())
    };
    format!(
        r#"
        SELECT
            run.id, run.state, run.repository_id, run.repository_workspace_id,
            run.provider_session_id, run.provider_turn_id,
            workspace.branch_name AS branch,
            run.config,
            workspace.base_commit,
            workspace.published_commit,
            workspace.published_at::text AS published_at,
            workspace.publish_failure_category,
            pull_request_request.state AS pull_request_request_state,
            pull_request_request.failure_category AS pull_request_failure_category,
            pull_request.pull_request_number,
            pull_request.id AS pull_request_id,
            pull_request.html_url AS pull_request_url,
            pull_request.title AS pull_request_title,
            pull_request.state AS pull_request_state,
            pull_request.draft AS pull_request_draft,
            pull_request.checks_state AS pull_request_checks_state,
            pull_request.review_state AS pull_request_review_state,
            pull_request.mergeability_state AS pull_request_mergeability_state,
            pull_request.sync_state AS pull_request_sync_state,
            pull_request.sync_failure_category AS pull_request_sync_failure_category,
            pull_request.automation_state AS pull_request_automation_state,
            pull_request.automation_reason AS pull_request_automation_reason,
            {queue_position} AS queue_position,
            run.failure_reason,
            run.created_at::text AS created_at,
            run.updated_at::text AS updated_at
        FROM agent_runs run
        LEFT JOIN repository_workspaces workspace
          ON workspace.id = run.repository_workspace_id
        LEFT JOIN github_pull_request_requests pull_request_request
          ON pull_request_request.agent_run_id = run.id
        LEFT JOIN github_pull_requests pull_request
          ON pull_request.id = pull_request_request.github_pull_request_id
        WHERE run.conversation_id = $1
          AND run.state IN {states}
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT 1
        "#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_lists_come_from_the_constants() {
        let active = builtin_active_states();
        for state in ACTIVE_RUN_STATES {
            assert!(active.contains(&format!("'{state}'")), "{active}");
        }
        let finished = builtin_terminal_states();
        for state in ["completed", "failed", "canceled", "interrupted"] {
            assert!(finished.contains(&format!("'{state}'")), "{finished}");
        }
        assert!(!finished.contains("'queued'"), "{finished}");
    }
}
