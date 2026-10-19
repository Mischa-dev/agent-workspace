//! What a run did (H5) and what it cost (U2), for the run page, the Runs
//! page across projects and the usage totals.
//!
//! Every read checks the reader can open the run's project
//! (`actor_can_access_project`) and see its thread (`conversation_visible_to`):
//! a run is visible exactly when its thread is. Steps were redacted and
//! capped when they were stored (`run_log`); the diff is read live from the
//! run's worktree, or from the mirror once the worktree is gone.

use std::path::PathBuf;

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    git_runtime::{ChangedFile, RunDiff},
    run_log::{TOTALS, UsageTotals},
};

/// The most steps the run page shows; long runs show their last ones.
const MAX_STEPS: i64 = 2_000;
/// The largest diff the run page shows inline.
const MAX_PATCH_BYTES: usize = 768 * 1024;
const MAX_LIST: i64 = 100;
const MAX_USAGE_DAYS: i64 = 400;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/runs", get(list_http))
        .route("/api/v1/runs/usage", get(usage_http))
        .route("/api/v1/agent-runs/{run_id}", get(detail_http))
        .route("/api/v1/agent-runs/{run_id}/diff", get(diff_http))
}

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
        SELECT link.work_item_id FROM work_item_conversation_links link
        WHERE link.project_id = project.id AND link.conversation_id = run.conversation_id
        ORDER BY link.created_at, link.work_item_id
        LIMIT 1
    ) task_link ON true
    LEFT JOIN work_items task
      ON task.id = task_link.work_item_id AND task.archived_at IS NULL
    LEFT JOIN LATERAL (
        SELECT {TOTALS} FROM agent_run_usage WHERE agent_run_id = run.id
    ) usage ON true
"#;

fn summary_select() -> String {
    SUMMARY_SELECT.replace("{TOTALS}", TOTALS)
}

fn named(state: &AppState, mut run: RunSummary) -> RunSummary {
    run.harness_name = run
        .harness
        .as_deref()
        .map(|harness| crate::agent_sessions::harness_name(state, harness));
    run
}

// ---------------------------------------------------------------------------
// Runs page

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunListQuery {
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// active, finished, failed or needs_you; all when left out.
    pub status: Option<String>,
    pub harness: Option<String>,
    /// Only runs this person started.
    pub mine: Option<bool>,
    /// The `nextCursor` of the previous page.
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunList {
    pub runs: Vec<RunSummary>,
    pub next_cursor: Option<String>,
}

fn status_states(status: Option<&str>) -> Result<Option<&'static [&'static str]>, AppError> {
    Ok(match status {
        None | Some("" | "all") => None,
        Some("active") => Some(&[
            "created",
            "queued",
            "starting",
            "working",
            "blocked",
            "recovering",
        ]),
        Some("needs_you") => Some(&["blocked"]),
        Some("finished") => Some(&["completed"]),
        Some("failed") => Some(&["failed", "canceled", "interrupted"]),
        Some(_) => return Err(AppError::Validation("status")),
    })
}

fn decode_cursor(cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (time, id) = cursor
        .split_once('_')
        .ok_or(AppError::Validation("cursor"))?;
    let micros = time
        .parse::<i64>()
        .map_err(|_| AppError::Validation("cursor"))?;
    let time = DateTime::from_timestamp_micros(micros).ok_or(AppError::Validation("cursor"))?;
    let id = id.parse().map_err(|_| AppError::Validation("cursor"))?;
    Ok((time, id))
}

fn encode_cursor(run: &RunSummary) -> String {
    format!("{}_{}", run.created_at.timestamp_micros(), run.id)
}

/// Runs the viewer can see, newest first, across their projects (or one
/// workspace's or project's).
pub async fn list_runs(
    state: &AppState,
    viewer: Uuid,
    query: &RunListQuery,
) -> Result<RunList, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_LIST);
    let states = status_states(query.status.as_deref())?.map(|states| {
        states
            .iter()
            .map(|state| (*state).to_owned())
            .collect::<Vec<_>>()
    });
    let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
    if query
        .harness
        .as_deref()
        .is_some_and(|harness| harness.len() > 64)
    {
        return Err(AppError::Validation("harness"));
    }
    let rows = sqlx::query_as::<_, RunSummary>(&format!(
        r#"{}
        WHERE conversation_visible_to(run.conversation_id, $1)
          AND actor_can_access_project($1, NULL, project.id)
          AND ($2::uuid IS NULL OR project.organization_id = $2)
          AND ($3::uuid IS NULL OR project.id = $3)
          AND ($4::text[] IS NULL OR run.state = ANY($4))
          AND ($5::text IS NULL OR run.config->>'harness' = $5)
          AND (NOT $6 OR run.requesting_actor_id = $1)
          AND ($7::timestamptz IS NULL OR (run.created_at, run.id) < ($7, $8))
        ORDER BY run.created_at DESC, run.id DESC
        LIMIT $9
        "#,
        summary_select()
    ))
    .bind(viewer)
    .bind(query.organization_id)
    .bind(query.project_id)
    .bind(states)
    .bind(query.harness.as_deref())
    .bind(query.mine.unwrap_or(false))
    .bind(cursor.map(|(time, _)| time))
    .bind(cursor.map(|(_, id)| id))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(0);
    let runs = rows
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(0))
        .map(|run| named(state, run))
        .collect::<Vec<_>>();
    let next_cursor = more.then(|| runs.last().map(encode_cursor)).flatten();
    Ok(RunList { runs, next_cursor })
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<RunListQuery>,
) -> Result<Json<RunList>, AppError> {
    Ok(Json(list_runs(&state, principal.actor_id, &query).await?))
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
    /// The files the run's branch changed against its base, when the
    /// worktree or the published commit is still there to read.
    pub files: Option<Vec<ChangedFile>>,
}

async fn visible_run(state: &AppState, viewer: Uuid, run_id: Uuid) -> Result<RunSummary, AppError> {
    sqlx::query_as::<_, RunSummary>(&format!(
        "{} WHERE run.id = $1 AND conversation_visible_to(run.conversation_id, $2) AND actor_can_access_project($2, NULL, project.id)",
        summary_select()
    ))
    .bind(run_id)
    .bind(viewer)
    .fetch_optional(&state.pool)
    .await?
    .map(|run| named(state, run))
    .ok_or(AppError::NotFound("agent_run"))
}

pub async fn run_detail(
    state: &AppState,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<RunDetail, AppError> {
    let run = visible_run(state, viewer, run_id).await?;
    let prompt = sqlx::query_scalar::<_, String>("SELECT prompt FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&state.pool)
        .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_run_events WHERE agent_run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let mut steps = sqlx::query_as::<_, RunStep>(
        r#"
        SELECT id, kind, summary, payload, created_at FROM agent_run_events
        WHERE agent_run_id = $1
        ORDER BY created_at DESC, id DESC
        LIMIT $2
        "#,
    )
    .bind(run_id)
    .bind(MAX_STEPS)
    .fetch_all(&state.pool)
    .await?;
    steps.reverse();
    let tests = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT count(*),
               COALESCE(sum((payload->'tests'->>'passed')::bigint), 0)::bigint,
               COALESCE(sum((payload->'tests'->>'failed')::bigint), 0)::bigint,
               COALESCE(sum((payload->'tests'->>'skipped')::bigint), 0)::bigint
        FROM agent_run_events
        WHERE agent_run_id = $1 AND kind = 'test_result'
        "#,
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let last_failed = sqlx::query_scalar::<_, Option<i64>>(
        r#"
        SELECT (payload->'tests'->>'failed')::bigint FROM agent_run_events
        WHERE agent_run_id = $1 AND kind = 'test_result'
        ORDER BY created_at DESC, id DESC LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let usage_by_model = sqlx::query_as::<_, ModelUsage>(
        r#"
        SELECT model, input_tokens, output_tokens, cached_input_tokens,
               COALESCE(estimated_cost, reported_cost_usd) AS cost,
               CASE WHEN estimated_cost IS NOT NULL THEN currency
                    WHEN reported_cost_usd IS NOT NULL THEN 'USD' END AS currency
        FROM agent_run_usage WHERE agent_run_id = $1
        ORDER BY model
        "#,
    )
    .bind(run_id)
    .fetch_all(&state.pool)
    .await?;
    let files = match read_diff(state, &run, 0).await {
        Ok(Some(diff)) => Some(diff.files),
        Ok(None) => None,
        Err(error) => {
            tracing::debug!(
                error_category = error.log_category(),
                "run diff unavailable"
            );
            None
        }
    };
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

async fn detail_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunDetail>, AppError> {
    Ok(Json(run_detail(&state, principal.actor_id, run_id).await?))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDiffView {
    /// ready, or unavailable when neither the worktree nor a published
    /// commit is left to read.
    pub state: &'static str,
    #[serde(flatten)]
    pub diff: Option<RunDiff>,
}

/// Where the run's changes can be read now: its live worktree (including
/// uncommitted changes), else the mirror at the published commit.
async fn read_diff(
    state: &AppState,
    run: &RunSummary,
    max_patch_bytes: usize,
) -> Result<Option<RunDiff>, AppError> {
    let Some(base) = run.base_commit.clone() else {
        return Ok(None);
    };
    let location = sqlx::query_as::<_, (Option<String>, String)>(
        r#"
        SELECT worktree.path, repository.mirror_path
        FROM agent_runs run
        JOIN repositories repository ON repository.id = run.repository_id
        LEFT JOIN git_worktrees worktree
          ON worktree.repository_workspace_id = run.repository_workspace_id
         AND worktree.state IN ('ready', 'dirty')
        WHERE run.id = $1
        "#,
    )
    .bind(run.id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((worktree, mirror)) = location else {
        return Ok(None);
    };
    let (path, bare, head) = match (worktree, &run.published_commit) {
        (Some(worktree), _) if PathBuf::from(&worktree).is_dir() => {
            (PathBuf::from(worktree), false, None)
        }
        (_, Some(published)) => (PathBuf::from(mirror), true, Some(published.clone())),
        _ => return Ok(None),
    };
    let diff = tokio::task::spawn_blocking(move || {
        crate::git_runtime::run_diff(&path, bare, &base, head.as_deref(), max_patch_bytes)
    })
    .await
    .map_err(|_| AppError::Internal("read the run's diff"))??;
    Ok(Some(diff))
}

pub async fn run_diff(
    state: &AppState,
    viewer: Uuid,
    run_id: Uuid,
) -> Result<RunDiffView, AppError> {
    let run = visible_run(state, viewer, run_id).await?;
    let diff = read_diff(state, &run, MAX_PATCH_BYTES).await?;
    Ok(RunDiffView {
        state: if diff.is_some() {
            "ready"
        } else {
            "unavailable"
        },
        diff,
    })
}

async fn diff_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunDiffView>, AppError> {
    Ok(Json(run_diff(&state, principal.actor_id, run_id).await?))
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
        FROM agent_run_usage usage
        JOIN agent_runs run ON run.id = usage.agent_run_id
        JOIN projects project ON project.id = usage.project_id
        LEFT JOIN provider_accounts account ON account.id = usage.provider_account_id
        WHERE conversation_visible_to(run.conversation_id, $1)
          AND actor_can_access_project($1, NULL, project.id)
          AND usage.usage_date >= $2
          AND ($3::uuid IS NULL OR usage.organization_id = $3)
          AND ($4::uuid IS NULL OR usage.project_id = $4)
    "#;
    let rows = sqlx::query_as::<_, UsageRow>(&format!(
        r#"
        SELECT {key} AS key, COALESCE({label}, 'Unknown') AS label,
               count(DISTINCT usage.agent_run_id) AS runs,
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
    .fetch_all(&state.pool)
    .await?;
    let (runs, totals) = {
        #[derive(FromRow)]
        struct Total {
            runs: i64,
            #[sqlx(flatten)]
            usage: UsageTotals,
        }
        let total = sqlx::query_as::<_, Total>(&format!(
            "SELECT count(DISTINCT usage.agent_run_id) AS runs, {TOTALS} {filter}"
        ))
        .bind(viewer)
        .bind(since)
        .bind(query.organization_id)
        .bind(query.project_id)
        .fetch_one(&state.pool)
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

async fn usage_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<UsageQuery>,
) -> Result<Json<UsageReport>, AppError> {
    Ok(Json(
        usage_report(&state, principal.actor_id, &query).await?,
    ))
}
