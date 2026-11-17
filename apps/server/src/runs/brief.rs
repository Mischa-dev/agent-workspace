//! A1.6: a run in brief, as the run tools show it (`run_get`, `run_list`,
//! and the run `run_start`, `run_stop` and `run_retry` return): any run,
//! on the built-in host or reported by its own agent, with the stage it is
//! at and what to do next, in words.
//!
//! Who sees a run is `run_visible_to` (its conversation's rule, else its
//! project's members, else the accounts that do and asked for it), and, for
//! a run in a project, `actor_can_access_project` for the caller's
//! connection.

use sqlx::{FromRow, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use super::state::RunState;
use crate::error::AppError;

/// One run as the run tools read it. `detail_state` is set for a run on the
/// built-in host (its own, older states); the code fields only for those.
#[derive(Clone, Debug, FromRow)]
pub struct BriefRow {
    pub id: Uuid,
    pub state: String,
    pub detail_state: Option<String>,
    pub liveness: String,
    pub prompt: String,
    pub status_line: Option<String>,
    pub outcome_summary: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub started_by: String,
    pub queue_position: Option<i64>,
    pub branch: Option<String>,
    pub published: bool,
    pub failure_reason: Option<String>,
    pub pull_request_request_state: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    pub pr_title: Option<String>,
    pub pr_state: Option<String>,
    pub pr_checks: Option<String>,
    pub pr_review: Option<String>,
    pub approvals_waiting: i64,
    pub created_at: String,
    pub updated_at: String,
    pub conversation_kind: Option<String>,
    pub parent_conversation_id: Option<Uuid>,
    pub parent_kind: Option<String>,
}

/// `$1` is the viewer, `$2` their agent connection.
const SELECT: &str = r#"
    SELECT run.id, run.state, detail.state AS detail_state, run.liveness,
           COALESCE(detail.prompt, run.title) AS prompt,
           run.status_line, run.outcome_summary,
           run.conversation_id, run.space_id AS project_id,
           COALESCE(starter.display_name, 'Someone') AS started_by,
           CASE WHEN detail.state = 'queued' THEN (
               SELECT count(*) FROM agent_runs queued
               WHERE queued.state = 'queued'
                 AND (queued.created_at, queued.id) <= (detail.created_at, detail.id)
           ) END AS queue_position,
           workspace.branch_name AS branch,
           workspace.published_at IS NOT NULL AS published,
           run.failure_reason,
           request.state AS pull_request_request_state,
           pr.pull_request_number AS pr_number, pr.html_url AS pr_url, pr.title AS pr_title,
           pr.state AS pr_state, pr.checks_state AS pr_checks, pr.review_state AS pr_review,
           CASE WHEN detail.id IS NOT NULL
                THEN (SELECT count(*) FROM requests request
                      WHERE request.run_id = detail.id AND request.status = 'open')
                ELSE (SELECT count(*) FROM requests open_request
                      WHERE open_request.run_id = run.id AND open_request.status = 'open')
           END AS approvals_waiting,
           run.created_at::text AS created_at, run.updated_at::text AS updated_at,
           thread.kind AS conversation_kind,
           thread.parent_channel_id AS parent_conversation_id,
           parent.kind AS parent_kind
    FROM runs run
    LEFT JOIN agent_runs detail ON detail.id = run.id
    LEFT JOIN conversations thread ON thread.id = run.conversation_id
    LEFT JOIN conversations parent ON parent.id = thread.parent_channel_id
    LEFT JOIN project_memberships starter
      ON starter.project_id = run.space_id AND starter.actor_id = run.requested_by_account_id
    LEFT JOIN repository_workspaces workspace ON workspace.id = detail.repository_workspace_id
    LEFT JOIN github_pull_request_requests request ON request.agent_run_id = detail.id
    LEFT JOIN github_pull_requests pr ON pr.id = request.github_pull_request_id
    WHERE run_visible_to(run.id, $1)
      AND (thread.id IS NULL OR conversation_visible_to(thread.id, $1))
      AND (run.space_id IS NULL OR actor_can_access_project($1, $2, run.space_id))
"#;

/// One run the viewer may see, through the connection they call from.
pub async fn get(
    executor: impl PgExecutor<'_>,
    viewer: Uuid,
    connection: Option<Uuid>,
    run_id: Uuid,
) -> Result<Option<BriefRow>, AppError> {
    Ok(
        sqlx::query_as::<_, BriefRow>(&format!("{SELECT} AND run.id = $3"))
            .bind(viewer)
            .bind(connection)
            .bind(run_id)
            .fetch_optional(executor)
            .await?,
    )
}

/// Which runs `list` returns.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub project_id: Option<Uuid>,
    /// Run states (`RunState` keys) to keep; every state when empty.
    pub states: Vec<&'static str>,
    /// Only runs the viewer asked for.
    pub mine: bool,
    /// Projects to leave out (where the Runs module is off for the viewer).
    pub without_projects: Vec<Uuid>,
    pub limit: i64,
}

/// Runs the viewer may see, newest first.
pub async fn list(
    executor: impl PgExecutor<'_>,
    viewer: Uuid,
    connection: Option<Uuid>,
    filter: &ListFilter,
) -> Result<Vec<BriefRow>, AppError> {
    Ok(sqlx::query_as::<_, BriefRow>(&format!(
        r#"{SELECT}
          AND ($3::uuid IS NULL OR run.space_id = $3)
          AND NOT COALESCE(run.space_id = ANY($7), false)
          AND (cardinality($4::text[]) = 0 OR run.state = ANY($4))
          AND (NOT $5 OR run.requested_by_account_id = $1)
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT $6"#
    ))
    .bind(viewer)
    .bind(connection)
    .bind(filter.project_id)
    .bind(&filter.states)
    .bind(filter.mine)
    .bind(filter.limit.clamp(1, 100))
    .bind(&filter.without_projects)
    .fetch_all(executor)
    .await?)
}

/// The states `run_list`'s words stand for.
pub fn states_named(name: &str) -> Option<Vec<&'static str>> {
    Some(match name {
        "active" => RunState::active_keys(),
        "finished" => vec![RunState::Done.as_str()],
        "failed" => vec![RunState::Failed.as_str()],
        "all" => Vec::new(),
        _ => return None,
    })
}

/// Where a run is, for someone following it: a stage key, what to do next
/// in words, and when to ask again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage {
    pub key: &'static str,
    pub next: String,
    pub poll_after_seconds: Option<u32>,
}

fn stage_of(key: &'static str, next: impl Into<String>, poll: Option<u32>) -> Stage {
    Stage {
        key,
        next: next.into(),
        poll_after_seconds: poll,
    }
}

/// The stage of a run, from its pull request when it has one, else its
/// state.
pub fn stage(row: &BriefRow) -> Stage {
    if let Some(checks) = row.pr_checks.as_deref().filter(|_| row.pr_number.is_some()) {
        return match checks {
            "passing" | "success" | "passed" => {
                stage_of("checks_passed", "The pull request's checks passed.", None)
            }
            "failing" | "failure" | "failed" => stage_of(
                "checks_failed",
                "The pull request's checks failed; open it to see which.",
                None,
            ),
            _ => stage_of(
                "checks_pending",
                "Checks are running on the pull request; ask again shortly.",
                Some(60),
            ),
        };
    }
    if row.pr_number.is_some() {
        return stage_of(
            "checks_pending",
            "The pull request is open; its checks haven't reported yet.",
            Some(60),
        );
    }
    if row.pull_request_request_state.is_some() {
        return stage_of(
            "opening_pull_request",
            "The pull request is being opened.",
            Some(30),
        );
    }
    match row.detail_state.as_deref() {
        Some(detail) => builtin_stage(row, detail),
        None => reported_stage(row),
    }
}

/// A run on the built-in host, by its own states.
fn builtin_stage(row: &BriefRow, detail: &str) -> Stage {
    match detail {
        "created" | "queued" => {
            stage_of("queued", "Waiting for the harness to pick it up.", Some(60))
        }
        "blocked" if row.approvals_waiting > 0 => stage_of(
            "waiting_for_approval",
            "The run is waiting on an approval; answer it in the thread (request_list shows it).",
            Some(60),
        ),
        "starting" | "working" | "blocked" | "recovering" => {
            stage_of("working", "The run is working.", Some(60))
        }
        "failed" => stage_of(
            "failed",
            "The run failed; see failure. Fix the cause and start a new run.",
            None,
        ),
        "interrupted" | "canceled" => stage_of("interrupted", "The run was stopped.", None),
        _ if row.published => stage_of(
            "published",
            "The run's branch is published. Open a pull request from the run in the web app.",
            None,
        ),
        _ => stage_of(
            "finished",
            "The run finished. Publish its branch and open a pull request from the run in the web app.",
            None,
        ),
    }
}

/// A run its own agent reports, by the run's state, liveness and what it
/// last said it was doing.
fn reported_stage(row: &BriefRow) -> Stage {
    let Ok(state) = row.state.parse::<RunState>() else {
        return stage_of("working", "The run is working.", Some(60));
    };
    let doing = row
        .status_line
        .as_deref()
        .filter(|line| !line.trim().is_empty());
    match state {
        RunState::Queued | RunState::Starting | RunState::StartUncertain => {
            stage_of("queued", "Waiting for its agent to pick it up.", Some(60))
        }
        RunState::Accepted | RunState::Working if row.liveness == "not_responding" => stage_of(
            "not_responding",
            "Its agent hasn't reported for a while; it may have stopped. Ask again later, or stop the run.",
            Some(300),
        ),
        RunState::Accepted | RunState::Working => stage_of(
            "working",
            match doing {
                Some(line) => format!("The run is working: {line}"),
                None => "The run is working.".to_owned(),
            },
            Some(60),
        ),
        RunState::Waiting => stage_of(
            "waiting_for_answer",
            "The run is waiting on an answer to a request it raised; answer it where it was asked.",
            Some(60),
        ),
        RunState::CancelRequested => stage_of(
            "stopping",
            "Stopping; waiting for its agent to confirm.",
            Some(30),
        ),
        RunState::Done => stage_of(
            "finished",
            match row.outcome_summary.as_deref() {
                Some(summary) if !summary.trim().is_empty() => {
                    format!("The run finished: {summary}")
                }
                _ => "The run finished.".to_owned(),
            },
            None,
        ),
        RunState::Failed => stage_of(
            "failed",
            "The run failed; see failure. Fix the cause and start a new run.",
            None,
        ),
        RunState::Refused => stage_of("interrupted", "Its host refused the run.", None),
        RunState::Cancelled | RunState::Interrupted => {
            stage_of("interrupted", "The run was stopped.", None)
        }
    }
}

/// The failure notice a failed run left in its thread, for dismissing it:
/// its event, project and conversation. `None` when the run hasn't failed.
pub async fn failure_notice(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<(Uuid, Uuid, Uuid)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"
        SELECT event.id, run.project_id, run.conversation_id
          FROM agent_runs run
          JOIN run_activities event ON event.run_id = run.id
          JOIN conversation_entries entry
            ON entry.run_activity_id = event.id
           AND entry.conversation_id = run.conversation_id
         WHERE run.id = $1
           AND run.state = 'failed'
           AND event.payload->>'step' = 'run_failed'
         ORDER BY event.seq DESC
         LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, detail: Option<&str>) -> BriefRow {
        BriefRow {
            id: Uuid::nil(),
            state: state.to_owned(),
            detail_state: detail.map(str::to_owned),
            liveness: "ok".to_owned(),
            prompt: String::new(),
            status_line: None,
            outcome_summary: None,
            conversation_id: None,
            project_id: None,
            started_by: String::new(),
            queue_position: None,
            branch: None,
            published: false,
            failure_reason: None,
            pull_request_request_state: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            pr_checks: None,
            pr_review: None,
            approvals_waiting: 0,
            created_at: String::new(),
            updated_at: String::new(),
            conversation_kind: None,
            parent_conversation_id: None,
            parent_kind: None,
        }
    }

    #[test]
    fn a_reported_run_says_what_it_is_doing() {
        let mut working = row("working", None);
        working.status_line = Some("Running the tests".to_owned());
        let stage = stage(&working);
        assert_eq!(stage.key, "working");
        assert_eq!(stage.next, "The run is working: Running the tests");

        let mut silent = row("working", None);
        silent.liveness = "not_responding".to_owned();
        assert_eq!(super::stage(&silent).key, "not_responding");
        assert_eq!(
            super::stage(&row("waiting", None)).key,
            "waiting_for_answer"
        );
        assert_eq!(super::stage(&row("cancel_requested", None)).key, "stopping");
        let mut done = row("done", None);
        done.outcome_summary = Some("Opened the fix".to_owned());
        assert_eq!(super::stage(&done).next, "The run finished: Opened the fix");
    }

    #[test]
    fn a_builtin_run_keeps_its_stages() {
        assert_eq!(stage(&row("queued", Some("queued"))).key, "queued");
        let mut blocked = row("waiting", Some("blocked"));
        blocked.approvals_waiting = 1;
        assert_eq!(stage(&blocked).key, "waiting_for_approval");
        assert_eq!(stage(&row("done", Some("completed"))).key, "finished");
        assert_eq!(
            stage(&row("interrupted", Some("canceled"))).key,
            "interrupted"
        );
    }

    #[test]
    fn list_words_name_run_states() {
        assert_eq!(states_named("active"), Some(RunState::active_keys()));
        assert_eq!(states_named("finished"), Some(vec!["done"]));
        assert_eq!(states_named("all"), Some(Vec::new()));
        assert_eq!(states_named("soon"), None);
    }
}
