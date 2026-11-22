//! What a run did (H5) and what it cost (U2): the built-in run's record
//! (`run_step_list`) and the usage totals (`run_usage_list`).
//!
//! Every read checks the reader can open the run's project
//! (`actor_can_access_project`) and see its thread (`conversation_visible_to`):
//! a run is visible exactly when its thread is. Steps were redacted and
//! capped when they were stored (`run_log`); the changes are what the
//! run's host last reported (`runs::changes`).

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    harness_catalog::Harnesses,
    run_log::{TOTALS, UsageTotals},
    runs::changes::{self, ChangedFile, Changes},
};

/// The most steps the run page shows; long runs show their last ones.
const MAX_STEPS: i64 = 2_000;
const MAX_USAGE_DAYS: i64 = 400;

/// A run as lists and the run page's header show it.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: Uuid,
    pub state: String,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub project_name: String,
    pub conversation_id: Uuid,
    pub thread_name: Option<String>,
    pub task_id: Option<Uuid>,
    pub task_key: Option<String>,
    pub task_title: Option<String>,
    pub harness: Option<String>,
    /// The harness's name for people, filled in after the query.
    #[sqlx(default)]
    pub harness_name: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub speed: Option<String>,
    pub requested_by_actor_id: Uuid,
    pub requested_by_name: String,
    pub agent_actor_id: Uuid,
    pub agent_name: String,
    pub agent_profile_id: Option<Uuid>,
    pub branch: Option<String>,
    pub base_commit: Option<String>,
    pub published_commit: Option<String>,
    pub pull_request_number: Option<i64>,
    pub pull_request_url: Option<String>,
    pub pull_request_state: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    #[sqlx(flatten)]
    pub usage: UsageTotals,
}

const SUMMARY_SELECT: &str = r#"
    SELECT run.id, run.state, project.organization_id, project.id AS project_id,
           project.project_key, project.name AS project_name,
           run.conversation_id, conversation.name AS thread_name,
           task.id AS task_id,
           CASE WHEN task.id IS NOT NULL THEN project.project_key || '-' || task.number END AS task_key,
           task.title AS task_title,
           run.config->>'harness' AS harness, run.config->>'model' AS model,
           run.config->>'reasoning' AS reasoning, run.config->>'speed' AS speed,
           run.requesting_actor_id AS requested_by_actor_id,
           COALESCE(requester.display_name, 'Someone') AS requested_by_name,
           worker.actor_id AS agent_actor_id,
           COALESCE(agent.display_name, 'Agent') AS agent_name,
           profile.id AS agent_profile_id,
           workspace.branch_name AS branch, workspace.base_commit, workspace.published_commit,
           pull_request.pull_request_number, pull_request.html_url AS pull_request_url,
           pull_request.state AS pull_request_state,
           run.failure_reason, run.created_at, run.started_at, run.finished_at, run.updated_at,
           usage.input_tokens, usage.output_tokens, usage.cached_input_tokens,
           usage.cost, usage.currency
    FROM agent_runs run
    JOIN conversations conversation ON conversation.id = run.conversation_id
    JOIN projects project ON project.id = conversation.project_id
    JOIN execution_workers worker ON worker.id = run.execution_worker_id
    LEFT JOIN project_memberships agent
      ON agent.project_id = project.id AND agent.actor_id = worker.actor_id
    LEFT JOIN project_memberships requester
      ON requester.project_id = project.id AND requester.actor_id = run.requesting_actor_id
    LEFT JOIN agent_profiles profile ON profile.execution_worker_id = worker.id
    LEFT JOIN repository_workspaces workspace ON workspace.id = run.repository_workspace_id
    LEFT JOIN github_pull_request_requests pull_request_request
      ON pull_request_request.agent_run_id = run.id
    LEFT JOIN github_pull_requests pull_request
      ON pull_request.id = pull_request_request.github_pull_request_id
    LEFT JOIN LATERAL (
        SELECT link.work_item_id FROM task_threads link
        WHERE link.project_id = project.id AND link.conversation_id = run.conversation_id
        ORDER BY link.created_at, link.work_item_id
        LIMIT 1
    ) task_link ON true
    LEFT JOIN task_rows task
      ON task.id = task_link.work_item_id AND task.archived_at IS NULL
    LEFT JOIN LATERAL (
        SELECT {TOTALS} FROM run_usage WHERE run_id = run.id
    ) usage ON true
"#;

fn summary_select() -> String {
    SUMMARY_SELECT.replace("{TOTALS}", TOTALS)
}

fn named(harnesses: &Harnesses, mut run: RunSummary) -> RunSummary {
    run.harness_name = run
        .harness
        .as_deref()
        .map(|harness| harnesses.name(harness));
    run
}

// ---------------------------------------------------------------------------
// Run page

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStep {
    pub id: Uuid,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    /// Test commands the run ran, each with its counts.
    pub runs: i64,
    pub passed: i64,
    pub failed: i64,
    pub skipped: i64,
    /// The counts of the last test command, which is what the run left.
    pub last_failed: Option<i64>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    #[serde(flatten)]
    pub run: RunSummary,
    pub prompt: String,
    pub steps: Vec<RunStep>,
    /// Earlier steps not shown because the run had more than the page shows.
    pub hidden_steps: i64,
    pub tests: TestSummary,
    pub usage_by_model: Vec<ModelUsage>,
    /// The files the run changed against where it started, as its host last
    /// reported them; none before its first report.
    pub files: Option<Vec<ChangedFile>>,
}

/// The run as `viewer` may see it, and whether they see its code.
async fn visible_run(
    state: &AppState,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<(RunSummary, bool), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let harnesses = Harnesses::visible_on(state, &mut transaction).await?;
    let mut run = sqlx::query_as::<_, RunSummary>(&format!(
        "{} WHERE run.id = $1 AND conversation_visible_to(run.conversation_id, $2) AND actor_can_access_project($2, NULL, project.id)",
        summary_select()
    ))
    .bind(run_id)
    .bind(viewer)
    .fetch_optional(&mut *transaction)
    .await?
    .map(|run| named(&harnesses, run))
    .ok_or(AppError::NotFound("agent_run"))?;
    drop(transaction);
    let sees_code = redact(state, viewer, std::slice::from_mut(&mut run)).await?;
    let code = sees_code.contains(&run.project_id);
    Ok((run, code))
}

/// Leaves out what `viewer` may not see of each run, by their role in its
/// project and (in an app's call) its agent limit: without "see tasks", the
/// linked task's title (its key stays, as a task's view does); without "see
/// code", the branch, commits and pull request. Returns the projects where
/// they see code.
async fn redact(
    state: &AppState,
    viewer: Uuid,
    runs: &mut [RunSummary],
) -> Result<Vec<Uuid>, AppError> {
    let mut projects = runs.iter().map(|run| run.project_id).collect::<Vec<_>>();
    projects.sort_unstable();
    projects.dedup();
    if projects.is_empty() {
        return Ok(Vec::new());
    }
    let tasks = crate::search::projects_showing_tasks(&state.pool, viewer, &projects).await?;
    let code = crate::search::projects_allowing(
        &state.pool,
        viewer,
        &projects,
        crate::policy::Permission::CodeView,
    )
    .await?;
    for run in runs {
        if !tasks.contains(&run.project_id) {
            run.task_title = None;
        }
        if !code.contains(&run.project_id) {
            run.branch = None;
            run.base_commit = None;
            run.published_commit = None;
            run.pull_request_number = None;
            run.pull_request_url = None;
            run.pull_request_state = None;
        }
    }
    Ok(code)
}

pub async fn run_detail(
    state: &AppState,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<RunDetail, AppError> {
    let (run, code) = visible_run(state, viewer, run_id).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let prompt = sqlx::query_scalar::<_, String>("SELECT prompt FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut *transaction)
        .await?;
    let total =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_activities WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&mut *transaction)
            .await?;
    let mut steps = sqlx::query_as::<_, RunStep>(
        r#"
        SELECT id, COALESCE(payload->>'step', kind) AS kind, summary, payload,
               occurred_at AS created_at
          FROM run_activities
         WHERE run_id = $1
         ORDER BY seq DESC
        LIMIT $2
        "#,
    )
    .bind(run_id)
    .bind(MAX_STEPS)
    .fetch_all(&mut *transaction)
    .await?;
    steps.reverse();
    let tests = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT count(*),
               COALESCE(sum((payload->'tests'->>'passed')::bigint), 0)::bigint,
               COALESCE(sum((payload->'tests'->>'failed')::bigint), 0)::bigint,
               COALESCE(sum((payload->'tests'->>'skipped')::bigint), 0)::bigint
        FROM run_activities
        WHERE run_id = $1 AND payload->>'step' = 'test_result'
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut *transaction)
    .await?;
    let last_failed = sqlx::query_scalar::<_, Option<i64>>(
        r#"
        SELECT (payload->'tests'->>'failed')::bigint FROM run_activities
        WHERE run_id = $1 AND payload->>'step' = 'test_result'
        ORDER BY seq DESC LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await?
    .flatten();
    let usage_by_model = sqlx::query_as::<_, ModelUsage>(
        r#"
        SELECT model, sum(input_tokens)::bigint AS input_tokens,
               sum(output_tokens)::bigint AS output_tokens,
               sum(cached_input_tokens)::bigint AS cached_input_tokens,
               sum(COALESCE(estimated_cost, reported_cost)) AS cost,
               min(CASE WHEN estimated_cost IS NOT NULL THEN currency
                        WHEN reported_cost IS NOT NULL THEN COALESCE(reported_currency, 'USD')
                   END) AS currency
        FROM run_usage WHERE run_id = $1
        GROUP BY model
        ORDER BY model
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await?;
    // Changed files are code: only for a viewer who sees it.
    let files = if code {
        changes::get(&mut transaction, run_id)
            .await?
            .map(|changes| changes.files)
    } else {
        None
    };
    drop(transaction);
    Ok(RunDetail {
        run,
        prompt,
        hidden_steps: (total - i64::try_from(steps.len()).unwrap_or(0)).max(0),
        steps,
        tests: TestSummary {
            runs: tests.0,
            passed: tests.1,
            failed: tests.2,
            skipped: tests.3,
            last_failed,
        },
        usage_by_model,
        files,
    })
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDiffView {
    /// ready, or unavailable when the run's host hasn't reported its
    /// changes (or the viewer doesn't see its code).
    pub state: &'static str,
    #[serde(flatten)]
    pub diff: Option<Changes>,
}

pub async fn run_diff(
    state: &AppState,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<RunDiffView, AppError> {
    // Any run, whoever runs it: the shared run view's visibility, and its
    // code only for a viewer who sees code.
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let line = super::pages::visible_line(&mut transaction, viewer, run_id)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    let diff = if super::pages::sees_code(&mut transaction, viewer, line.project_id).await? {
        changes::get(&mut transaction, run_id).await?
    } else {
        None
    };
    Ok(RunDiffView {
        state: if diff.is_some() {
            "ready"
        } else {
            "unavailable"
        },
        diff,
    })
}

// ---------------------------------------------------------------------------
// Usage totals (U2)

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageQuery {
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// day, project, account or model.
    pub group_by: Option<String>,
    /// How many days back, today included (30 when left out).
    pub days: Option<i64>,
    /// AWD-75: projects to leave out: the MCP tool passes those where Runs
    /// is off for the caller. Never read from a request.
    #[serde(skip)]
    pub without_projects: Vec<Uuid>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    /// The day (YYYY-MM-DD), project, account or model the row totals.
    pub key: String,
    pub label: String,
    pub runs: i64,
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub usage: UsageTotals,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub group_by: String,
    pub since: NaiveDate,
    pub rows: Vec<UsageRow>,
    pub totals: UsageTotals,
    pub runs: i64,
}

/// Tokens and cost of the runs the viewer can see, totalled per day,
/// project, account or model. Accounts are named only for the viewer's
/// own; other people's show as one row.
pub async fn usage_report(
    state: &AppState,
    viewer: Uuid,
    query: &UsageQuery,
) -> Result<UsageReport, AppError> {
    let group_by = query.group_by.as_deref().unwrap_or("day");
    let (key, label) = match group_by {
        "day" => (
            "to_char(usage.usage_date, 'YYYY-MM-DD')",
            "to_char(usage.usage_date, 'YYYY-MM-DD')",
        ),
        "project" => ("project.id::text", "project.name"),
        // A run that used no provider account (a connected runner, or one
        // whose account was removed) is its own row, not someone else's.
        "account" => (
            "CASE WHEN account.id IS NULL THEN 'none' \
                  WHEN account.owner_actor_id = $1 THEN account.id::text ELSE 'others' END",
            "CASE WHEN account.id IS NULL THEN 'No account' \
                  WHEN account.owner_actor_id = $1 THEN account.name \
                  ELSE 'Other people''s accounts' END",
        ),
        "model" => ("usage.model", "usage.model"),
        _ => return Err(AppError::Validation("groupBy")),
    };
    let days = query.days.unwrap_or(30).clamp(1, MAX_USAGE_DAYS);
    let since = Utc::now().date_naive() - chrono::Duration::days(days - 1);
    let filter = r#"
        FROM run_usage usage
        JOIN runs run ON run.id = usage.run_id
        JOIN projects project ON project.id = usage.space_id
        LEFT JOIN agent_runs builtin ON builtin.id = run.id
        LEFT JOIN execution_profiles profile ON profile.id = builtin.execution_profile_id
        LEFT JOIN provider_accounts account ON account.id = profile.provider_account_id
        WHERE (run.conversation_id IS NULL OR conversation_visible_to(run.conversation_id, $1))
          AND actor_can_access_project($1, NULL, project.id)
          AND usage.usage_date >= $2
          AND ($3::uuid IS NULL OR usage.workspace_id = $3)
          AND ($4::uuid IS NULL OR usage.space_id = $4)
          AND NOT (usage.space_id = ANY($5))
    "#;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let rows = sqlx::query_as::<_, UsageRow>(&format!(
        r#"
        SELECT {key} AS key, COALESCE({label}, 'Unknown') AS label,
               count(DISTINCT usage.run_id) AS runs,
               {TOTALS}
        {filter}
        GROUP BY 1, 2
        ORDER BY {order}
        LIMIT 400
        "#,
        order = if group_by == "day" {
            "1 DESC"
        } else {
            "cost DESC NULLS LAST, 3 DESC"
        },
    ))
    .bind(viewer)
    .bind(since)
    .bind(query.organization_id)
    .bind(query.project_id)
    .bind(&query.without_projects)
    .fetch_all(&mut *transaction)
    .await?;
    let (runs, totals) = {
        #[derive(FromRow)]
        struct Total {
            runs: i64,
            #[sqlx(flatten)]
            usage: UsageTotals,
        }
        let total = sqlx::query_as::<_, Total>(&format!(
            "SELECT count(DISTINCT usage.run_id) AS runs, {TOTALS} {filter}"
        ))
        .bind(viewer)
        .bind(since)
        .bind(query.organization_id)
        .bind(query.project_id)
        .bind(&query.without_projects)
        .fetch_one(&mut *transaction)
        .await?;
        (total.runs, total.usage)
    };
    Ok(UsageReport {
        group_by: group_by.to_owned(),
        since,
        rows,
        totals,
        runs,
    })
}
