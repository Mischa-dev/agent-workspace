//! A1.6: run reads that other features make about one run or one person,
//! moved here from where they were written inline. Each says who has already
//! checked access: these take ids a caller is entitled to, and return only
//! what the caller asked.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// The thread and repository of a built-in run (an automation starting the
/// next run where the last one worked).
pub async fn thread_and_repository(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT conversation_id, repository_id FROM agent_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(executor)
    .await?)
}

/// Who asked for a built-in run, and the task its launch was for.
pub async fn requester_and_task(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<(Uuid, Option<Uuid>)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        r#"
        SELECT run.requesting_actor_id,
               (SELECT launch.work_item_id FROM launches launch
                WHERE launch.agent_run_id = run.id LIMIT 1)
        FROM agent_runs run WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(executor)
    .await?)
}

/// Who asked for a built-in run.
pub async fn requester(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT requesting_actor_id FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(executor)
            .await?,
    )
}

/// What a webhook delivery says about a built-in run.
#[derive(Clone, Debug, FromRow)]
pub struct WebhookRun {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub state: String,
    pub harness: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub async fn for_webhook(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Option<WebhookRun>, AppError> {
    Ok(sqlx::query_as::<_, WebhookRun>(
        r#"
        SELECT run.id, run.conversation_id, run.state, run.config->>'harness' AS harness,
               run.failure_reason, run.created_at, run.finished_at
        FROM agent_runs run WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(executor)
    .await?)
}

/// A built-in run's state, when it is in that conversation.
pub async fn builtin_state_in(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    conversation_id: Uuid,
) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT state FROM agent_runs WHERE id = $1 AND conversation_id = $2",
    )
    .bind(run_id)
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Whether the person asked for any run in the project (onboarding's
/// "started a run" step).
pub async fn requested_any_in_space(
    executor: impl PgExecutor<'_>,
    space_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM runs run
            WHERE run.space_id = $1 AND run.requested_by_account_id = $2
        )
        "#,
    )
    .bind(space_id)
    .bind(actor_id)
    .fetch_one(executor)
    .await?)
}

/// Built-in runs by state: every active one, and the rest that changed in
/// the last day (the metrics page).
pub async fn builtin_state_counts(
    executor: impl PgExecutor<'_>,
) -> Result<Vec<(String, i64)>, AppError> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT state, count(*) FROM agent_runs
        WHERE state = ANY($1) OR updated_at > now() - interval '1 day'
        GROUP BY state ORDER BY state
        "#,
    )
    .bind(&super::ACTIVE_RUN_STATES[..])
    .fetch_all(executor)
    .await?)
}

/// Where a built-in run's notifications go: its project and thread, who
/// asked for it, and whether they can still reach the thread.
#[derive(Clone, Debug, FromRow)]
pub struct NotifyContext {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub requesting_actor_id: Uuid,
    pub requester_reachable: bool,
}

pub async fn notify_context(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<NotifyContext>, AppError> {
    Ok(sqlx::query_as::<_, NotifyContext>(
        r#"
        SELECT conversation.project_id, run.conversation_id, run.requesting_actor_id,
               actor_can_access_project(run.requesting_actor_id, NULL, conversation.project_id)
               AND conversation_visible_to(run.conversation_id, run.requesting_actor_id)
                   AS requester_reachable
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Active built-in runs a viewer can see across their standard projects
/// (Home's "working now"): the newest `limit`, and how many there are.
#[derive(Clone, Debug, FromRow)]
pub struct WorkingRun {
    pub id: Uuid,
    pub state: String,
    pub project_id: Uuid,
    pub project_name: String,
    pub thread_id: Uuid,
    pub thread_name: String,
    pub task_id: Option<Uuid>,
    pub task_key: Option<String>,
    pub task_title: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub started_by: Option<String>,
    pub requesting_actor_id: Uuid,
    pub approvals_waiting: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
}

const WORKING_FROM: &str = r#"
    -- visibility: every query adds WORKING_VISIBLE, which keeps only runs in
    -- conversations conversation_visible_to shows the viewer.
    FROM agent_runs run
    JOIN conversations thread ON thread.id = run.conversation_id
    JOIN projects project ON project.id = thread.project_id
"#;

/// Active runs the reader can see: in threads `conversation_visible_to`
/// shows them, in standard projects they can still open. `$1` the viewer,
/// `$2` the active states.
const WORKING_VISIBLE: &str = r#"
    WHERE run.state = ANY($2) AND run.state <> 'created'
      AND project.archived_at IS NULL AND project.kind = 'standard'
      AND conversation_visible_to(thread.id, $1)
      AND actor_can_access_project($1, NULL, project.id)
"#;

pub async fn working_for(
    transaction: &mut Transaction<'_, Postgres>,
    viewer: Uuid,
    limit: i64,
) -> Result<(Vec<WorkingRun>, i64), AppError> {
    let rows = sqlx::query_as::<_, WorkingRun>(&format!(
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
               (SELECT count(*) FROM requests request
                 WHERE request.run_id = run.id AND request.status = 'open')
                 AS approvals_waiting,
               run.created_at, run.started_at
        {WORKING_FROM}
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
                    AND role_allows(sees_task.role, sees_task.role_id, 'tasks.view'))
            LIMIT 1
        ) item ON true
        LEFT JOIN execution_profiles profile ON profile.id = run.execution_profile_id
        LEFT JOIN provider_accounts account ON account.id = profile.provider_account_id
        LEFT JOIN project_memberships starter
          ON starter.project_id = project.id AND starter.actor_id = run.requesting_actor_id
        {WORKING_VISIBLE}
        ORDER BY run.created_at DESC, run.id
        LIMIT {limit}
        "#
    ))
    .bind(viewer)
    .bind(&super::ACTIVE_RUN_STATES[..])
    .fetch_all(&mut **transaction)
    .await?;
    let total =
        sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) {WORKING_FROM} {WORKING_VISIBLE}"))
            .bind(viewer)
            .bind(&super::ACTIVE_RUN_STATES[..])
            .fetch_one(&mut **transaction)
            .await?;
    Ok((rows, total))
}
