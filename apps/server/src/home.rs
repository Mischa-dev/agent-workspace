//! P3: what Home shows that no other page gathers: the runs working now
//! across every project the reader can open, and how many launches wait in
//! their queues. The rest of Home (what needs you, capacity, your tasks,
//! recent projects) comes from the inbox, usage, my-work and project lists.

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
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

pub async fn home_for(state: &AppState, viewer: Uuid) -> Result<Home, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let (rows, working_total) =
        crate::runs::reads::working_for(&mut transaction, viewer, MAX_RUNS).await?;
    transaction.commit().await?;
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
    .fetch_one(crate::db::scoped(&state.pool))
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
                href: crate::addresses::link(
                    state,
                    None,
                    &crate::addresses::Target::space(
                        row.project_id,
                        crate::addresses::Place::Thread(row.thread_id),
                    ),
                ),
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
