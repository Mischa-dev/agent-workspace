//! The tasks an account could run now (N3, U3): open and not yet started,
//! not blocked, with a repository, in every project the account serves
//! that the viewer can still open. A capacity alert's Launch opens this
//! list; W13 turns it into a one-tap batch.

use serde::Serialize;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{error::AppError, work_item_planning::HAS_OPEN_BLOCKER};

const MAX_READY_TASKS: i64 = 100;

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub key: String,
    pub title: String,
    pub priority: String,
    /// How many unfinished tasks wait on this one.
    pub unblocks: i64,
    #[sqlx(skip)]
    pub href: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyTasks {
    pub tasks: Vec<ReadyTask>,
    /// The projects the account serves that the viewer can open.
    pub project_count: i64,
}

/// Ready tasks for `account_id`, as `viewer` may see them, most urgent and
/// most unblocking first. `limit` caps the list (at most 100).
pub async fn ready_tasks(
    connection: &mut PgConnection,
    viewer: Uuid,
    account_id: Uuid,
    limit: Option<i64>,
) -> Result<ReadyTasks, AppError> {
    ready_tasks_in(connection, viewer, account_id, limit, false).await
}

/// `ready_tasks`; with `runs_only` (the tools, AWD-75) only in projects
/// where Runs is on for the viewer, in the list and the count.
pub async fn ready_tasks_in(
    connection: &mut PgConnection,
    viewer: Uuid,
    account_id: Uuid,
    limit: Option<i64>,
    runs_only: bool,
) -> Result<ReadyTasks, AppError> {
    let off = if runs_only {
        runs_off(connection, viewer, account_id).await?
    } else {
        Vec::new()
    };
    let limit = limit.unwrap_or(MAX_READY_TASKS).clamp(1, MAX_READY_TASKS);
    let mut tasks = sqlx::query_as::<_, ReadyTask>(&format!(
        r#"
        WITH served AS (
            SELECT DISTINCT profile.project_id
            FROM execution_profiles profile
            JOIN projects project ON project.id = profile.project_id
            WHERE profile.provider_account_id = $1
              AND profile.status <> 'disabled'
              AND project.archived_at IS NULL
              AND project.kind = 'standard'
              AND actor_can_access_project($2, NULL, project.id)
              AND NOT (project.id = ANY($4))
        )
        SELECT item.id, item.project_id, project.name AS project_name,
               project.project_key || '-' || item.number::text AS key,
               item.title, item.priority,
               (SELECT count(*) FROM work_item_dependencies dependency
                  JOIN work_items waiting
                    ON waiting.project_id = dependency.project_id
                   AND waiting.id = dependency.blocked_work_item_id
                 WHERE dependency.blocking_project_id = item.project_id
                   AND dependency.blocking_work_item_id = item.id
                   AND waiting.archived_at IS NULL) AS unblocks
        FROM served
        JOIN projects project ON project.id = served.project_id
        JOIN work_items item ON item.project_id = served.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.archived_at IS NULL
          AND status.semantic_category IN ('backlog', 'unstarted')
          AND item.blocked_reason IS NULL
          AND NOT {HAS_OPEN_BLOCKER}
          AND (EXISTS (SELECT 1 FROM work_item_repository_links link
                        WHERE link.work_item_id = item.id)
               OR EXISTS (SELECT 1 FROM repositories repository
                           WHERE repository.project_id = item.project_id
                             AND repository.archived_at IS NULL))
        ORDER BY CASE item.priority
                   WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2
                   WHEN 'low' THEN 3 ELSE 4 END,
                 unblocks DESC, item.updated_at DESC, item.id
        LIMIT $3
        "#
    ))
    .bind(account_id)
    .bind(viewer)
    .bind(limit)
    .bind(&off)
    .fetch_all(&mut *connection)
    .await?;
    for task in &mut tasks {
        task.href = format!("/projects/{}/work-items/{}", task.project_id, task.id);
    }
    let project_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(DISTINCT profile.project_id)
        FROM execution_profiles profile
        JOIN projects project ON project.id = profile.project_id
        WHERE profile.provider_account_id = $1
          AND profile.status <> 'disabled'
          AND project.archived_at IS NULL
          AND project.kind = 'standard'
          AND actor_can_access_project($2, NULL, project.id)
          AND NOT (project.id = ANY($3))
        "#,
    )
    .bind(account_id)
    .bind(viewer)
    .bind(&off)
    .fetch_one(&mut *connection)
    .await?;
    Ok(ReadyTasks {
        tasks,
        project_count,
    })
}

/// AWD-75: the projects the account serves where Runs is off for `viewer`.
async fn runs_off(
    connection: &mut PgConnection,
    viewer: Uuid,
    account_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let served = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT profile.project_id
        FROM execution_profiles profile
        JOIN projects project ON project.id = profile.project_id
        WHERE profile.provider_account_id = $1
          AND profile.status <> 'disabled'
          AND project.archived_at IS NULL
          AND project.kind = 'standard'
          AND actor_can_access_project($2, NULL, project.id)
        "#,
    )
    .bind(account_id)
    .bind(viewer)
    .fetch_all(&mut *connection)
    .await?;
    let on = crate::modules::state::ProjectModules::load(connection, viewer, &served)
        .await?
        .keep(&served, |modules, _| modules.contains("runs"));
    Ok(served
        .into_iter()
        .filter(|project| !on.contains(project))
        .collect())
}
