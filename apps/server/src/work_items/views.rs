//! Ways to look at tasks beyond one list (docs/plans/roadmap.md, W11): My
//! work across every project (V6) and a project's timeline (V8). Saved
//! views are views of the Tasks collection (`crate::views`).

use axum::{Json, extract::State};
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgExecutor, PgPool};
use uuid::Uuid;

use super::{authorize_work_item, capabilities, project_membership};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    policy::Action,
    work_item_planning::{self as planning, HAS_OPEN_BLOCKER, MilestoneView},
};

// ---------------------------------------------------------------------------
// My work

/// Most tasks each My work section shows; the section's total says how many
/// there are, and its link opens the rest.
const MY_WORK_LIMIT: i64 = 100;

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWorkTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub project_name: String,
    pub key: String,
    pub title: String,
    pub status_id: Uuid,
    pub status_name: String,
    pub status_category: String,
    pub priority: String,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub overdue: bool,
    pub blocked: bool,
    pub milestone_name: Option<String>,
    pub assignee_name: Option<String>,
    pub claimed_by_id: Option<Uuid>,
    pub claimed_by_name: Option<String>,
    pub claimed_by_kind: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWorkSection {
    pub items: Vec<MyWorkTask>,
    pub total: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWork {
    /// Unfinished tasks assigned to you, soonest due first.
    pub assigned: MyWorkSection,
    /// Tasks you or your agents are working on right now.
    pub claimed: MyWorkSection,
    /// Tasks in review that you asked for or watch, assigned to someone else.
    pub review: MyWorkSection,
}

/// The tasks the My work sections read. `$1` is the reader, `$2` the
/// projects to look in.
const MY_WORK_SELECT: &str = r#"
    SELECT item.id, item.project_id, project.project_key, project.name AS project_name,
           project.project_key || '-' || item.number::text AS key, item.title,
           status.id AS status_id, status.name AS status_name,
           status.semantic_category AS status_category, item.priority,
           item.start_date, item.due_date,
           COALESCE(item.due_date < current_date
                    AND status.semantic_category NOT IN ('completed', 'canceled'), false) AS overdue,
           (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) AS blocked,
           (SELECT milestone.name FROM project_milestones milestone
             WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id
           ) AS milestone_name,
           assignee.display_name AS assignee_name,
           claim.actor_id AS claimed_by_id,
           COALESCE(claimer.display_name, CASE WHEN claim.actor_id IS NULL THEN NULL ELSE 'Unknown' END)
               AS claimed_by_name,
           claimer_actor.kind AS claimed_by_kind,
           item.updated_at::text AS updated_at
    FROM task_rows item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
    LEFT JOIN work_item_claims claim
      ON claim.work_item_id = item.id AND claim.expires_at > now()
    LEFT JOIN project_memberships claimer
      ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
    LEFT JOIN actors claimer_actor ON claimer_actor.id = claim.actor_id
    WHERE item.project_id = ANY($2) AND item.archived_at IS NULL
"#;

/// Unfinished, in review that you asked for or watch, and assigned to someone
/// else (yours is under Assigned).
const REVIEW_WHERE: &str = r#"
    AND status.semantic_category IN ('review', 'verification')
    AND item.assignee_actor_id IS DISTINCT FROM $1
    AND (item.created_by_actor_id = $1
         OR EXISTS (SELECT 1 FROM follows watcher
                     WHERE watcher.target_kind = 'task' AND watcher.target_id = item.id
                       AND watcher.account_id = $1))
"#;

/// Claimed by the reader or by an agent they connected.
const CLAIMED_WHERE: &str = r#"
    AND (claim.actor_id = $1
         OR claim.actor_id IN (SELECT connection.actor_id FROM external_agent_connections connection
                                WHERE connection.created_by_actor_id = $1
                                  AND connection.status <> 'removed'))
"#;

const ASSIGNED_WHERE: &str = r#"
    AND item.assignee_actor_id = $1
    AND status.semantic_category NOT IN ('completed', 'canceled')
"#;

/// The projects a person reads tasks in: enabled memberships in workspaces
/// they're still in, archived projects left out.
pub(crate) async fn my_projects(
    executor: impl PgExecutor<'_>,
    actor_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1 AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $1
         AND organization_membership.enabled
        WHERE project.archived_at IS NULL
        ORDER BY project.id
        LIMIT 500
        "#,
    )
    .bind(actor_id)
    .fetch_all(executor)
    .await?)
}

async fn my_work_section(
    pool: &PgPool,
    reader: Uuid,
    projects: &[Uuid],
    condition: &str,
    order: &str,
) -> Result<MyWorkSection, AppError> {
    let select = MY_WORK_SELECT.replace("{HAS_OPEN_BLOCKER}", HAS_OPEN_BLOCKER);
    let items =
        sqlx::query_as::<_, MyWorkTask>(&format!("{select} {condition} ORDER BY {order} LIMIT $3"))
            .bind(reader)
            .bind(projects)
            .bind(MY_WORK_LIMIT)
            .fetch_all(crate::db::scoped(pool))
            .await?;
    let total = sqlx::query_scalar::<_, i64>(&format!(
        r#"SELECT count(*) FROM task_rows item
           JOIN work_item_statuses status
             ON status.project_id = item.project_id AND status.id = item.status_id
           LEFT JOIN work_item_claims claim
             ON claim.work_item_id = item.id AND claim.expires_at > now()
           WHERE item.project_id = ANY($2) AND item.archived_at IS NULL {condition}
             AND $1::uuid IS NOT NULL"#
    ))
    .bind(reader)
    .bind(projects)
    .fetch_one(crate::db::scoped(pool))
    .await?;
    Ok(MyWorkSection { items, total })
}

const PRIORITY_RANK: &str = "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 \
     WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END";

/// Everything that's yours across `projects`.
pub(crate) async fn my_work(
    pool: &PgPool,
    reader: Uuid,
    projects: &[Uuid],
) -> Result<MyWork, AppError> {
    let projects = &crate::search::projects_showing_tasks(pool, reader, projects).await?[..];
    let due_first = format!(
        "item.due_date IS NULL, item.due_date, {PRIORITY_RANK}, item.updated_at DESC, item.id"
    );
    Ok(MyWork {
        assigned: my_work_section(pool, reader, projects, ASSIGNED_WHERE, &due_first).await?,
        claimed: my_work_section(
            pool,
            reader,
            projects,
            CLAIMED_WHERE,
            "claim.claimed_at DESC, item.id",
        )
        .await?,
        review: my_work_section(
            pool,
            reader,
            projects,
            REVIEW_WHERE,
            "item.updated_at, item.id",
        )
        .await?,
    })
}

pub(super) async fn get_my_work(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<MyWork>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)
        .await?;
    let projects = my_projects(&state.pool, principal.actor_id).await?;
    Ok(Json(
        my_work(&state.pool, principal.actor_id, &projects).await?,
    ))
}

// ---------------------------------------------------------------------------
// Timeline

/// The longest stretch one timeline request covers, and the most tasks it
/// draws; a busier project narrows by milestone or dates.
const MAX_TIMELINE_DAYS: i64 = 731;
const MAX_TIMELINE_TASKS: i64 = 1_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TimelineQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    milestone_id: Option<Uuid>,
    include_done: Option<bool>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineTask {
    id: Uuid,
    key: String,
    title: String,
    status_id: Uuid,
    status_name: String,
    status_category: String,
    priority: String,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    estimate: Option<f64>,
    milestone_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    assignee_name: Option<String>,
    blocked: bool,
    version: i64,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineLink {
    blocking_id: Uuid,
    blocked_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineResponse {
    from: NaiveDate,
    to: NaiveDate,
    estimate_unit: String,
    milestones: Vec<MilestoneView>,
    tasks: Vec<TimelineTask>,
    /// Blocks between the tasks shown: the blocking task must finish first.
    dependencies: Vec<TimelineLink>,
    /// Dependencies AI planning suggests and nobody has accepted yet, drawn
    /// dashed. Only those the reader may see (W10).
    suggested_dependencies: Vec<TimelineLink>,
    /// Tasks in range beyond the most one request draws.
    truncated: bool,
    /// Tasks (in the chosen milestone, or the project) with no dates at all.
    undated: i64,
    can_edit: bool,
}

pub(super) async fn get_timeline(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TimelineQuery>,
) -> Result<Json<TimelineResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)
        .await?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let today = Utc::now().date_naive();
    let from = query.from.unwrap_or(today - Duration::days(14));
    let to = query.to.unwrap_or(today + Duration::days(120));
    if to < from || (to - from).num_days() > MAX_TIMELINE_DAYS {
        return Err(AppError::Validation("to"));
    }
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    if let Some(milestone_id) = query.milestone_id
        && planning::milestone(&mut connection, project_id, milestone_id)
            .await?
            .is_none()
    {
        return Err(AppError::Validation("milestoneId"));
    }
    let include_done = query.include_done.unwrap_or(true);
    let mut tasks = sqlx::query_as::<_, TimelineTask>(&format!(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key, item.title,
               status.id AS status_id, status.name AS status_name,
               status.semantic_category AS status_category, item.priority,
               item.start_date, item.due_date, item.estimate, item.milestone_id,
               item.parent_work_item_id AS parent_id,
               assignee.display_name AS assignee_name,
               (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) AS blocked,
               item.version
        FROM task_rows item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL
          AND (item.start_date IS NOT NULL OR item.due_date IS NOT NULL)
          AND COALESCE(item.start_date, item.due_date) <= $3
          AND COALESCE(item.due_date, item.start_date) >= $2
          AND ($4::uuid IS NULL OR item.milestone_id = $4)
          AND ($5 OR status.semantic_category NOT IN ('completed', 'canceled'))
        ORDER BY COALESCE(item.start_date, item.due_date), item.number
        LIMIT $6
        "#
    ))
    .bind(project_id)
    .bind(from)
    .bind(to)
    .bind(query.milestone_id)
    .bind(include_done)
    .bind(MAX_TIMELINE_TASKS + 1)
    .fetch_all(&mut *connection)
    .await?;
    let truncated = tasks.len() as i64 > MAX_TIMELINE_TASKS;
    tasks.truncate(MAX_TIMELINE_TASKS as usize);
    let ids = tasks.iter().map(|task| task.id).collect::<Vec<_>>();
    let dependencies = sqlx::query_as::<_, TimelineLink>(
        r#"
        SELECT blocking_work_item_id AS blocking_id, blocked_work_item_id AS blocked_id
        FROM work_item_dependencies
        WHERE project_id = $1 AND blocking_project_id = $1
          AND blocked_work_item_id = ANY($2) AND blocking_work_item_id = ANY($2)
        ORDER BY blocking_work_item_id, blocked_work_item_id
        "#,
    )
    .bind(project_id)
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?;
    // Suggestions are checked against what this person may see, as on the graph.
    let suggested_dependencies = crate::ai::ready::suggested_blocks(
        &mut *connection,
        crate::work_item_planning::Reader {
            actor_id: principal.actor_id,
            connection_id: None,
            addresses: crate::addresses::Addresses::of(&state),
        },
        project_id,
        &ids,
    )
    .await?
    .into_iter()
    .map(|(blocking_id, blocked_id)| TimelineLink {
        blocking_id,
        blocked_id,
    })
    .collect();
    let undated = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM task_rows item
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL
          AND item.start_date IS NULL AND item.due_date IS NULL
          AND ($2::uuid IS NULL OR item.milestone_id = $2)
          AND status.semantic_category NOT IN ('completed', 'canceled')
        "#,
    )
    .bind(project_id)
    .bind(query.milestone_id)
    .fetch_one(&mut *connection)
    .await?;
    let mut milestones = planning::list_milestones(&mut connection, project_id, false).await?;
    if let Some(milestone_id) = query.milestone_id {
        milestones.retain(|milestone| milestone.id == milestone_id);
    }
    let (_, estimate_unit) =
        crate::work_item_workflow::task_settings(&mut connection, project_id).await?;
    Ok(Json(TimelineResponse {
        from,
        to,
        estimate_unit,
        milestones,
        tasks,
        dependencies,
        suggested_dependencies,
        truncated,
        undated,
        can_edit: capabilities(&membership)?.edit && membership.project_archived_at.is_none(),
    }))
}
