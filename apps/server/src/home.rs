//! P3: what Home shows that no other page gathers: the runs working now
//! across every project the reader can open, and how many launches wait in
//! their queues. The rest of Home (what needs you, capacity, your tasks,
//! recent projects) comes from the inbox, usage, my-work and project lists.

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, auth::AuthenticatedActor, error::AppError};

/// The most runs Home lists.
const MAX_RUNS: i64 = 20;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/home", get(home_http))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeTask {
    pub id: Uuid,
    pub key: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingRun {
    pub id: Uuid,
    /// queued, starting, working, blocked or recovering.
    pub state: String,
    pub project_id: Uuid,
    pub project_name: String,
    pub thread_id: Uuid,
    pub thread_name: String,
    pub task: Option<HomeTask>,
    /// The harness it runs on and its display name, such as `codex` and
    /// Codex: data, never assumed.
    pub harness: Option<String>,
    pub harness_name: Option<String>,
    pub model: Option<String>,
    pub started_by: String,
    pub mine: bool,
    /// Approvals it waits on.
    pub approvals_waiting: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub href: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Home {
    pub working: Vec<WorkingRun>,
    /// Runs working now, beyond the ones listed.
    pub working_total: i64,
    /// Launches waiting to start in the projects the reader can open.
    pub queued_launches: i64,
}

#[derive(FromRow)]
struct RunRow {
    id: Uuid,
    state: String,
    project_id: Uuid,
    project_name: String,
    thread_id: Uuid,
    thread_name: String,
    task_id: Option<Uuid>,
    task_key: Option<String>,
    task_title: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    started_by: Option<String>,
    requesting_actor_id: Uuid,
    approvals_waiting: i64,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
}

const RUNS_FROM: &str = r#"
    FROM agent_runs run
    JOIN conversations thread ON thread.id = run.conversation_id
    JOIN projects project ON project.id = thread.project_id
"#;

/// Active runs the reader can see: in threads `conversation_visible_to`
/// shows them, in standard projects they can still open.
const VISIBLE_ACTIVE: &str = r#"
    WHERE run.state IN ('queued', 'starting', 'working', 'blocked', 'recovering')
      AND project.archived_at IS NULL AND project.kind = 'standard'
      AND conversation_visible_to(thread.id, $1)
      AND actor_can_access_project($1, NULL, project.id)
"#;

pub async fn home_for(state: &AppState, viewer: Uuid) -> Result<Home, AppError> {
    let rows = sqlx::query_as::<_, RunRow>(&format!(
        r#"
        SELECT run.id, run.state, project.id AS project_id, project.name AS project_name,
               thread.id AS thread_id, thread.name AS thread_name,
               item.id AS task_id,
               CASE WHEN item.id IS NOT NULL
                    THEN project.project_key || '-' || item.number::text END AS task_key,
               item.title AS task_title,
               COALESCE(run.config->>'harness', account.adapter_kind) AS harness,
               run.config->>'model' AS model,
               starter.display_name AS started_by, run.requesting_actor_id,
               (SELECT count(*) FROM agent_approvals approval
                 WHERE approval.agent_run_id = run.id AND approval.status = 'pending')
                 AS approvals_waiting,
               run.created_at, run.started_at
        {RUNS_FROM}
        LEFT JOIN LATERAL (
            SELECT work_item.id, work_item.number, work_item.title
            FROM launches launch
            JOIN task_rows work_item
              ON work_item.project_id = launch.project_id AND work_item.id = launch.work_item_id
            WHERE launch.agent_run_id = run.id AND work_item.archived_at IS NULL
              -- The task's key and title only where the reader sees tasks.
              AND EXISTS (
                  SELECT 1 FROM project_memberships sees_task
                  WHERE sees_task.project_id = launch.project_id AND sees_task.actor_id = $1
                    AND sees_task.enabled
                    AND custom_role_allows(sees_task.role_id, 'tasks.view'))
            LIMIT 1
        ) item ON true
        LEFT JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        LEFT JOIN provider_accounts account ON account.id = profile.provider_account_id
        LEFT JOIN project_memberships starter
          ON starter.project_id = project.id AND starter.actor_id = run.requesting_actor_id
        {VISIBLE_ACTIVE}
        ORDER BY run.created_at DESC, run.id
        LIMIT {MAX_RUNS}
        "#
    ))
    .bind(viewer)
    .fetch_all(&state.pool)
    .await?;
    let working_total =
        sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) {RUNS_FROM} {VISIBLE_ACTIVE}"))
            .bind(viewer)
            .fetch_one(&state.pool)
            .await?;
    let queued_launches = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM launches launch
        JOIN projects project ON project.id = launch.project_id
        WHERE launch.state = 'queued'
          AND project.archived_at IS NULL AND project.kind = 'standard'
          AND actor_can_access_project($1, NULL, project.id)
        "#,
    )
    .bind(viewer)
    .fetch_one(&state.pool)
    .await?;
    let working = rows
        .into_iter()
        .map(|row| {
            let harness_name = row.harness.as_deref().map(|kind| {
                state
                    .supervisors
                    .manifest(kind)
                    .map_or_else(|| kind.to_owned(), |manifest| manifest.display_name.clone())
            });
            WorkingRun {
                href: format!("/projects/{}/threads/{}", row.project_id, row.thread_id),
                id: row.id,
                state: row.state,
                project_id: row.project_id,
                project_name: row.project_name,
                thread_id: row.thread_id,
                thread_name: row.thread_name,
                task: match (row.task_id, row.task_key, row.task_title) {
                    (Some(id), Some(key), Some(title)) => Some(HomeTask { id, key, title }),
                    _ => None,
                },
                harness: row.harness,
                harness_name,
                model: row.model,
                started_by: row.started_by.unwrap_or_else(|| "Someone".to_owned()),
                mine: row.requesting_actor_id == viewer,
                approvals_waiting: row.approvals_waiting,
                created_at: row.created_at,
                started_at: row.started_at,
            }
        })
        .collect();
    Ok(Home {
        working,
        working_total,
        queued_launches,
    })
}

async fn home_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<Home>, AppError> {
    Ok(Json(home_for(&state, principal.actor_id).await?))
}
