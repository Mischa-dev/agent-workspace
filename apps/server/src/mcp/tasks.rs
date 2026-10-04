//! Task tools: list and open tasks, put them in order, claim them before
//! working on them, and link them to the conversations, pages and decisions
//! they came from. Creating and changing tasks reuses the canonical commands
//! in `writes.rs`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, ToolResult, accessible_project_ids, archived_task_message,
        flag_or_text, mark_via, parse_id, parse_message_id, parse_since, project_is_chosen,
        public_url, scope_projects, snippet, string_or_number, version_conflict_message,
    },
    history,
    reads::{self, ReadResult, WorkItemProjection},
    talk::can_write_in,
    writes::{self, PullRequestLinkChange, WriteAccess, WriteResult},
};
use crate::{
    AppState,
    error::AppError,
    policy::Action,
    work_item_commands::{
        ChangeFinalization, ChangePayload, CommandActor, LinkChange, LinkWording, LockedWorkItem,
        RepositoryLinkOutcome, ThreadLinkOutcome, advance_version, change_repository_link,
        change_thread_link, enqueue_update, finalize_change, insert_activity, lock_work_item,
        lock_work_items, ranks_for_placement, record_audit, validate_assignee, validate_priority,
    },
    work_item_labels::{self, echo},
    work_items::decode_work_item_timeline_cursor,
};

const DEFAULT_CLAIM_MINUTES: i32 = 120;

// ---------------------------------------------------------------------------
// task_list

/// The most projects one task_list call looks at through projectIds.
const MAX_LIST_PROJECTS: usize = 50;
/// The most labels one task_list call filters by.
const MAX_FILTER_LABELS: usize = 10;
/// Status groups, in board order.
const STATUS_GROUPS: [&str; 7] = [
    "backlog",
    "unstarted",
    "started",
    "review",
    "verification",
    "completed",
    "canceled",
];
const UPDATED_SINCE_MESSAGE: &str = "Use a date (YYYY-MM-DD), a timestamp such as 2026-09-30T14:00:00Z, or a span such as 24h or 7d.";
const CURSOR_MESSAGE: &str = "Pass the nextCursor you were given, as cursor.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskListArguments {
    /// Only this project (ID, link, or key such as AWD). Leave it out for
    /// tasks across all your projects. The MCP middleware also scopes the
    /// call to it.
    pub project_id: Option<String>,
    /// Several projects at once (IDs, links, or keys), 1 to 50. Not together
    /// with projectId.
    pub project_ids: Option<Vec<String>>,
    /// A status name (such as "In Progress") or group: backlog, unstarted,
    /// started, review, verification, completed, canceled.
    pub status: Option<String>,
    /// "me", "none", an actor ID, or an @handle.
    pub assignee: Option<String>,
    /// Only tasks that have all of these labels, by name or ID (at most 10).
    /// Names match in each project you're looking at.
    pub labels: Option<Vec<String>>,
    /// A type name such as Bug (any case), or a type ID.
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    /// no_priority (or none), low, medium, high, or urgent.
    pub priority: Option<String>,
    /// Who created it: "me", an @handle, or an actor ID.
    pub created_by: Option<String>,
    /// Changed since: a date (YYYY-MM-DD), a timestamp such as
    /// 2026-09-30T14:00:00Z, or a span such as 24h or 7d.
    pub updated_since: Option<String>,
    /// true: only blocked tasks. false: leave blocked tasks out.
    pub blocked: Option<bool>,
    /// yes (someone is working on it), no, me, or an @handle. true and false
    /// work too.
    #[serde(default, deserialize_with = "flag_or_text")]
    #[schemars(with = "Option<String>")]
    pub claimed: Option<String>,
    /// Words in the title or description, or a key such as AW-12.
    pub query: Option<String>,
    /// Only subtasks of this task.
    pub parent_id: Option<String>,
    /// board (the order people set, the default), priority, updated, or number.
    pub sort: Option<String>,
    /// Include completed and canceled tasks.
    pub include_done: Option<bool>,
    /// Also list archived tasks, each marked archived.
    pub include_archived: Option<bool>,
    /// How many (default 50, at most 200).
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskSummary {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: String,
    pub status_group: String,
    /// The task's type, such as Task or Bug.
    #[serde(rename = "type")]
    pub task_type: String,
    pub priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// 1 is the top of its status column on the board; 0 for an archived task.
    pub board_position: i64,
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    pub project: String,
    pub version: i64,
    pub updated_at: String,
    /// Only on archived tasks (includeArchived).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// Its labels, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

/// The `labels` column of a task line, read through the active-labels view
/// so archived labels never show. There's no `#[sqlx(default)]` on the
/// field (nor on `task_type` and `archived`): a query that forgets a column
/// fails loudly.
const TASK_LABELS_COLUMN: &str = r#"
    COALESCE((SELECT array_agg(active.name ORDER BY lower(active.name), active.label_id)
              FROM work_item_active_labels active
              WHERE active.project_id = item.project_id AND active.work_item_id = item.id),
             ARRAY[]::text[]) AS labels
"#;

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskListValue {
    pub tasks: Vec<TaskSummary>,
    /// Every task that matches, across all pages.
    pub total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// What a task_list call filters on, resolved: the same values bind as $1 to
/// $16 in both the count and the page, so `total` and the page agree.
#[derive(Debug, Default)]
struct ListFilters {
    projects: Vec<Uuid>,
    include_done: bool,
    status: Option<String>,
    assignee_mode: &'static str,
    assignee_id: Option<Uuid>,
    query: String,
    parent_id: Option<Uuid>,
    include_archived: bool,
    task_type: Option<String>,
    priority: Option<String>,
    created_by: Option<Uuid>,
    updated_since: Option<DateTime<Utc>>,
    blocked: Option<bool>,
    claimed_mode: &'static str,
    claimed_id: Option<Uuid>,
    /// Label names, lowercased and each once.
    labels: Vec<String>,
}

/// The filters a cursor was made for. A cursor carries a short hash of it,
/// so passing a cursor from one search to a different one is refused rather
/// than silently skipping or repeating tasks.
#[derive(Serialize)]
struct ListFingerprint<'a> {
    projects: &'a [Uuid],
    status: Option<&'a str>,
    assignee: (&'a str, Option<Uuid>),
    query: &'a str,
    parent: Option<Uuid>,
    sort: &'a str,
    include_done: bool,
    include_archived: bool,
    labels: &'a [String],
    task_type: Option<&'a str>,
    priority: Option<&'a str>,
    created_by: Option<Uuid>,
    updated_since: Option<&'a str>,
    blocked: Option<bool>,
    claimed: (&'a str, Option<Uuid>),
}

impl ListFingerprint<'_> {
    fn hash(&self) -> Result<String, AppError> {
        let encoded =
            serde_json::to_vec(self).map_err(|_| AppError::Internal("task_list fingerprint"))?;
        Ok(hex::encode(&Sha256::digest(&encoded)[..6]))
    }
}

/// Where the next page starts. Cursors read `{offset}.{fingerprint}`; a bare
/// number, as cursors were before, is still taken as an offset.
fn cursor_offset(cursor: Option<&str>, fingerprint: &str) -> Result<i64, &'static str> {
    let Some(cursor) = cursor.map(str::trim).filter(|cursor| !cursor.is_empty()) else {
        return Ok(0);
    };
    let (offset, from) = match cursor.split_once('.') {
        Some((offset, from)) => (offset, Some(from)),
        None => (cursor, None),
    };
    if offset.is_empty() || offset.len() > 9 || !offset.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CURSOR_MESSAGE);
    }
    if from.is_some_and(|from| from != fingerprint) {
        return Err(
            "That cursor came from a different search. Leave out cursor to start from the first page.",
        );
    }
    offset.parse::<i64>().map_err(|_| CURSOR_MESSAGE)
}

/// The tables every filter reads. Only one claim per task can be live (the
/// claims primary key), so the join never repeats a task.
const TASK_FROM: &str = r#"
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    JOIN work_item_types item_type
      ON item_type.project_id = item.project_id AND item_type.id = item.type_id
    LEFT JOIN work_item_claims claim
      ON claim.work_item_id = item.id AND claim.expires_at > now()
"#;

/// Every filter, bound by `bind_task_filters`. Labels match by name through
/// the active-labels view, so an archived label never matches.
const TASK_WHERE: &str = r#"
    WHERE item.project_id = ANY($1)
      AND ($8 OR item.archived_at IS NULL)
      AND ($2 OR status.semantic_category NOT IN ('completed', 'canceled'))
      AND ($3::text IS NULL OR lower(status.name) = lower($3)
           OR status.semantic_category = lower($3) OR status.id::text = lower($3))
      AND ($4 = 'any' OR ($4 = 'none' AND item.assignee_actor_id IS NULL)
           OR ($4 = 'one' AND item.assignee_actor_id = $5))
      AND ($6 = '' OR item.search_vector @@ websearch_to_tsquery('english', $6)
           OR item.title ILIKE '%' || $6 || '%'
           OR upper(project.project_key || '-' || item.number::text) = upper($6))
      AND ($7::uuid IS NULL OR item.parent_work_item_id = $7)
      AND ($9::text IS NULL OR lower(item_type.name) = lower($9)
           OR item_type.id::text = lower($9))
      AND ($10::text IS NULL OR item.priority = $10)
      AND ($11::uuid IS NULL OR item.created_by_actor_id = $11)
      AND ($12::timestamptz IS NULL OR item.updated_at >= $12)
      AND ($13::boolean IS NULL OR (item.blocked_reason IS NOT NULL) = $13)
      AND ($14 = 'any' OR ($14 = 'none' AND claim.work_item_id IS NULL)
           OR ($14 = 'some' AND claim.work_item_id IS NOT NULL)
           OR ($14 = 'one' AND claim.actor_id = $15))
      AND (cardinality($16::text[]) = 0 OR (
            SELECT count(*) FROM work_item_active_labels active
            WHERE active.project_id = item.project_id AND active.work_item_id = item.id
              AND lower(active.name) = ANY($16::text[])
          ) = cardinality($16::text[]))
"#;

/// Binds $1 to $16 of `TASK_WHERE`, identically for the count and the page.
macro_rules! bind_task_filters {
    ($query:expr, $filters:expr) => {
        $query
            .bind(&$filters.projects)
            .bind($filters.include_done)
            .bind($filters.status.as_deref())
            .bind($filters.assignee_mode)
            .bind($filters.assignee_id)
            .bind($filters.query.as_str())
            .bind($filters.parent_id)
            .bind($filters.include_archived)
            .bind($filters.task_type.as_deref())
            .bind($filters.priority.as_deref())
            .bind($filters.created_by)
            .bind($filters.updated_since)
            .bind($filters.blocked)
            .bind($filters.claimed_mode)
            .bind($filters.claimed_id)
            .bind(&$filters.labels)
    };
}

pub(super) async fn task_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskListArguments,
) -> ToolResult<TaskListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    task_list_inner(state, principal, arguments)
        .await
        .unwrap_or_else(|error| ToolResult::from_error(error, "task_list"))
}

async fn task_list_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskListArguments,
) -> Result<ToolResult<TaskListValue>, AppError> {
    let limit = i64::from(arguments.limit.unwrap_or(50));
    if !(1..=200).contains(&limit) {
        return Ok(ToolResult::invalid("limit", "Use a limit from 1 to 200."));
    }
    let sort = arguments.sort.as_deref().unwrap_or("board");
    // Archived tasks (includeArchived) go to the end of their column.
    let order = match sort {
        "board" => {
            "project.name, item.project_id, status.position, item.archived_at IS NOT NULL, item.board_rank, item.id"
        }
        "priority" => {
            "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END, status.position, item.archived_at IS NOT NULL, item.board_rank, item.id"
        }
        "updated" => "item.updated_at DESC, item.id",
        "number" => "project.name, item.project_id, item.number",
        _ => {
            return Ok(ToolResult::invalid(
                "sort",
                "Use board, priority, updated, or number.",
            ));
        }
    };
    let parent_id = match arguments.parent_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => {
                return Ok(ToolResult::invalid(
                    "parentId",
                    "Pass the parent task's ID.",
                ));
            }
        },
        None => None,
    };
    if arguments.project_id.is_some() && arguments.project_ids.is_some() {
        return Ok(ToolResult::invalid(
            "projectIds",
            "Pass projectId or projectIds, not both.",
        ));
    }
    let priority = match arguments
        .priority
        .as_deref()
        .map(|value| value.trim().to_lowercase())
    {
        None => None,
        Some(value) if value == "none" => Some("no_priority".to_owned()),
        Some(value) if validate_priority(&value).is_ok() => Some(value),
        Some(_) => {
            return Ok(ToolResult::invalid(
                "priority",
                "Use no_priority, low, medium, high, or urgent.",
            ));
        }
    };
    let updated_since = match arguments.updated_since.as_deref() {
        None => None,
        Some(value) => match parse_since(value) {
            Some(time) => Some(time),
            None => return Ok(ToolResult::invalid("updatedSince", UPDATED_SINCE_MESSAGE)),
        },
    };

    let projects = match &arguments.project_ids {
        Some(listed) => {
            if listed.is_empty() || listed.len() > MAX_LIST_PROJECTS {
                return Ok(ToolResult::invalid("projectIds", "Pass 1 to 50 projects."));
            }
            let reachable = accessible_project_ids(state, principal).await?;
            let mut projects = Vec::with_capacity(listed.len());
            for raw in listed {
                // The middleware turned keys the caller can reach into IDs; a
                // key left as written, or an ID out of reach, is named back.
                match parse_id(raw).filter(|id| reachable.contains(id)) {
                    Some(id) => projects.push(id),
                    None => {
                        return Ok(ToolResult::not_found(&format!(
                            "Project {}",
                            snippet(raw, 60)
                        )));
                    }
                }
            }
            projects
        }
        None => scope_projects(state, principal).await?,
    };
    let mut projects = projects;
    projects.sort_unstable();
    projects.dedup();
    let place = if let [only] = projects.as_slice() {
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(only)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_else(|| "this project".to_owned())
    } else {
        "these projects".to_owned()
    };

    let (assignee_mode, assignee_id) = match arguments.assignee.as_deref().map(str::trim) {
        None | Some("") => ("any", None),
        Some("me") => ("one", Some(principal.actor_id)),
        Some("none") => ("none", None),
        Some(person) => match resolve_actor(state, &projects, person).await? {
            Some(id) => ("one", Some(id)),
            None => {
                return Ok(ToolResult::invalid(
                    "assignee",
                    format!("No one called {person} was found."),
                ));
            }
        },
    };
    let status = arguments
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(status) = &status
        && let Err(message) = known_status(state, &projects, &place, status).await?
    {
        return Ok(ToolResult::invalid("status", message));
    }
    let task_type = arguments
        .task_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(task_type) = &task_type
        && let Err(message) = known_type(state, &projects, &place, task_type).await?
    {
        return Ok(ToolResult::invalid("type", message));
    }
    let labels = match &arguments.labels {
        None => Vec::new(),
        Some(labels) => match filter_labels(state, &projects, labels).await? {
            Ok(names) => names,
            Err(message) => return Ok(ToolResult::invalid("labels", message)),
        },
    };
    let created_by = match arguments.created_by.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("me") => Some(principal.actor_id),
        Some(person) => match resolve_actor(state, &projects, person).await? {
            Some(id) => Some(id),
            None => {
                return Ok(ToolResult::invalid(
                    "createdBy",
                    format!("No one called {person} was found."),
                ));
            }
        },
    };
    let (claimed_mode, claimed_id) = match arguments.claimed.as_deref().map(str::trim) {
        None | Some("") => ("any", None),
        Some(word) => match word.to_lowercase().as_str() {
            "yes" | "true" => ("some", None),
            "no" | "false" => ("none", None),
            "me" => ("one", Some(principal.actor_id)),
            _ => match resolve_actor(state, &projects, word).await? {
                Some(id) => ("one", Some(id)),
                None if word.starts_with('@') || word.parse::<Uuid>().is_ok() => {
                    return Ok(ToolResult::invalid(
                        "claimed",
                        format!("No one called {word} was found."),
                    ));
                }
                None => {
                    return Ok(ToolResult::invalid(
                        "claimed",
                        "Use yes, no, me, or an @handle.",
                    ));
                }
            },
        },
    };
    let filters = ListFilters {
        projects,
        include_done: arguments.include_done.unwrap_or(false),
        status,
        assignee_mode,
        assignee_id,
        query: arguments
            .query
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        parent_id,
        include_archived: arguments.include_archived.unwrap_or(false),
        task_type,
        priority,
        created_by,
        updated_since,
        blocked: arguments.blocked,
        claimed_mode,
        claimed_id,
        labels,
    };
    let fingerprint = ListFingerprint {
        projects: &filters.projects,
        status: filters.status.as_deref(),
        assignee: (filters.assignee_mode, filters.assignee_id),
        query: &filters.query,
        parent: filters.parent_id,
        sort,
        include_done: filters.include_done,
        include_archived: filters.include_archived,
        labels: &filters.labels,
        task_type: filters.task_type.as_deref(),
        priority: filters.priority.as_deref(),
        created_by: filters.created_by,
        updated_since: arguments.updated_since.as_deref().map(str::trim),
        blocked: filters.blocked,
        claimed: (filters.claimed_mode, filters.claimed_id),
    }
    .hash()?;
    let offset = match cursor_offset(arguments.cursor.as_deref(), &fingerprint) {
        Ok(offset) => offset,
        Err(message) => return Ok(ToolResult::invalid("cursor", message)),
    };

    // One snapshot for both queries, so the total and the page agree.
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let total = bind_task_filters!(
        sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) {TASK_FROM} {TASK_WHERE}")),
        filters
    )
    .fetch_one(&mut *transaction)
    .await?;
    // Board positions count every open task in a column, so they're numbered
    // before the filters and the page apply.
    let mut tasks = bind_task_filters!(
        sqlx::query_as::<_, TaskSummary>(&format!(
            r#"
            WITH numbered AS (
                SELECT item.id,
                       row_number() OVER (PARTITION BY item.project_id, item.status_id
                                          ORDER BY item.board_rank, item.id) AS board_position
                FROM work_items item
                WHERE item.project_id = ANY($1) AND item.archived_at IS NULL
            )
            SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
                   item.title, status.name AS status, status.semantic_category AS status_group,
                   item_type.name AS task_type, item.priority,
                   assignee.display_name AS assignee,
                   COALESCE(numbered.board_position, 0) AS board_position,
                   item.blocked_reason IS NOT NULL AS blocked,
                   claimer.display_name AS claimed_by, project.name AS project,
                   item.version, item.updated_at::text AS updated_at,
                   item.archived_at IS NOT NULL AS archived,
                   {TASK_LABELS_COLUMN}
            {TASK_FROM}
            LEFT JOIN numbered ON numbered.id = item.id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
            LEFT JOIN project_memberships claimer
              ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
            {TASK_WHERE}
            ORDER BY {order}
            LIMIT $17 OFFSET $18
            "#
        )),
        filters
    )
    .bind(limit + 1)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let has_more = i64::try_from(tasks.len()).unwrap_or(i64::MAX) > limit;
    tasks.truncate(usize::try_from(limit).unwrap_or(200));
    Ok(ToolResult::ok(TaskListValue {
        tasks,
        total,
        next_cursor: has_more.then(|| format!("{}.{fingerprint}", offset + limit)),
    }))
}

/// Ok when `value` names a status (by name or ID) or a status group in the
/// projects; otherwise the message listing what does. Retired statuses
/// still filter, since tasks can sit in one, but only enabled ones are
/// listed.
async fn known_status(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    value: &str,
) -> Result<Result<(), String>, AppError> {
    if STATUS_GROUPS.contains(&value.to_lowercase().as_str()) {
        return Ok(Ok(()));
    }
    let known = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM work_item_statuses
            WHERE project_id = ANY($1) AND (lower(name) = lower($2) OR id::text = lower($2))
        )
        "#,
    )
    .bind(projects)
    .bind(value)
    .fetch_one(&state.pool)
    .await?;
    if known {
        return Ok(Ok(()));
    }
    let names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM work_item_statuses
        WHERE project_id = ANY($1) AND enabled
        GROUP BY name ORDER BY min(position), name
        LIMIT 40
        "#,
    )
    .bind(projects)
    .fetch_all(&state.pool)
    .await?;
    Ok(Err(format!(
        "No status called \"{}\" in {place}. Statuses: {}. Groups: {}.",
        echo(value),
        names.join(", "),
        STATUS_GROUPS.join(", ")
    )))
}

/// Ok when `value` names a task type (by name or ID) in the projects;
/// otherwise the message listing the enabled ones.
async fn known_type(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    value: &str,
) -> Result<Result<(), String>, AppError> {
    let known = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM work_item_types
            WHERE project_id = ANY($1) AND (lower(name) = lower($2) OR id::text = lower($2))
        )
        "#,
    )
    .bind(projects)
    .bind(value)
    .fetch_one(&state.pool)
    .await?;
    if known {
        return Ok(Ok(()));
    }
    let names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM work_item_types
        WHERE project_id = ANY($1) AND enabled
        GROUP BY name ORDER BY min(position), name
        LIMIT 40
        "#,
    )
    .bind(projects)
    .fetch_all(&state.pool)
    .await?;
    Ok(Err(format!(
        "No task type called \"{}\" in {place}. Types: {}.",
        echo(value),
        names.join(", ")
    )))
}

/// The label names a task_list filter matches, lowercased and each once.
/// Every entry must name an active label (by name or ID) in one of the
/// projects. With one project the refusal lists its labels; across several
/// it names no project and lists nothing, so it can't show one project's
/// labels to someone looking at another.
async fn filter_labels(
    state: &AppState,
    projects: &[Uuid],
    requested: &[String],
) -> Result<Result<Vec<String>, String>, AppError> {
    if requested.is_empty() || requested.len() > MAX_FILTER_LABELS {
        return Ok(Err("Filter by 1 to 10 labels.".to_owned()));
    }
    let mut values = Vec::with_capacity(requested.len());
    for reference in requested {
        let trimmed = reference.trim();
        let value = match trimmed.parse::<Uuid>() {
            Ok(id) => id.to_string(),
            Err(_) => match work_item_labels::normalize_name(trimmed) {
                Some(name) => name,
                None => {
                    return Ok(Err(format!(
                        "\"{}\" isn't a label name or ID.",
                        echo(trimmed)
                    )));
                }
            },
        };
        values.push(value);
    }
    // The page query compares lower(active.name) with these, so they're
    // lowercased by the same lower() here, never in Rust (Unicode rules such
    // as a final sigma differ from the database's).
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<bool>)>(
        r#"
        SELECT requested.value, label.name, lower(label.name),
               label.archived_at IS NOT NULL
        FROM unnest($2::text[]) WITH ORDINALITY AS requested(value, ordinality)
        LEFT JOIN project_labels label
          ON label.project_id = ANY($1)
         AND (label.id::text = lower(requested.value) OR lower(label.name) = lower(requested.value))
        ORDER BY requested.ordinality
        "#,
    )
    .bind(projects)
    .bind(&values)
    .fetch_all(&state.pool)
    .await?;
    let mut names = Vec::new();
    let mut resolution = work_item_labels::Resolution::default();
    for value in &values {
        let matches = rows
            .iter()
            .filter(|(requested, ..)| requested == value)
            .filter_map(|(_, name, lowered, archived)| {
                Some((name.as_deref()?, lowered.as_deref()?, (*archived)?))
            })
            .collect::<Vec<_>>();
        let active = matches
            .iter()
            .filter(|(_, _, archived)| !archived)
            .map(|(_, lowered, _)| (*lowered).to_owned())
            .collect::<Vec<_>>();
        if !active.is_empty() {
            for name in active {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        } else if let Some((name, ..)) = matches.first() {
            resolution.archived.push((*name).to_owned());
        } else {
            resolution.unknown.push(value.clone());
        }
    }
    if resolution.is_complete() {
        names.sort();
        return Ok(Ok(names));
    }
    if let [project] = projects {
        let mut connection = state.pool.acquire().await?;
        return Ok(Err(work_item_labels::unknown_message(
            &mut connection,
            *project,
            &resolution,
        )
        .await?));
    }
    Ok(Err(
        match (resolution.unknown.first(), resolution.archived.first()) {
            (Some(unknown), _) => format!(
                "No label called \"{}\" in the projects you're looking at. label_list shows a project's labels.",
                echo(unknown)
            ),
            (None, Some(archived)) => format!(
                "\"{}\" is archived. Restore it with label_write (archived: false) to filter by it.",
                echo(archived)
            ),
            (None, None) => "Name the labels to filter by.".to_owned(),
        },
    ))
}

async fn resolve_actor(
    state: &AppState,
    projects: &[Uuid],
    person: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Ok(id) = person.parse::<Uuid>() {
        return Ok(Some(id));
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = ANY($1) AND lower(handle) = lower($2) AND enabled LIMIT 1",
    )
    .bind(projects)
    .bind(person.trim_start_matches('@'))
    .fetch_optional(&state.pool)
    .await?)
}

/// Find a task by ID, link, or key (such as AW-12) among the caller's
/// projects. Returns the task and its project. A moved task's old key leads
/// to where it went when the caller can reach that project; anyone else
/// gets the archived original they could always see.
pub(super) async fn resolve_task(
    state: &AppState,
    principal: &McpPrincipal,
    identifier: &str,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let projects = accessible_project_ids(state, principal).await?;
    resolve_among(state, principal, &projects, identifier).await
}

/// `resolve_task` for several references, in order, reading the caller's
/// projects once.
pub(super) async fn resolve_tasks(
    state: &AppState,
    principal: &McpPrincipal,
    references: &[String],
) -> Result<Vec<Option<(Uuid, Uuid)>>, AppError> {
    let projects = accessible_project_ids(state, principal).await?;
    let mut found = Vec::with_capacity(references.len());
    for reference in references {
        found.push(resolve_among(state, principal, &projects, reference).await?);
    }
    Ok(found)
}

async fn resolve_among(
    state: &AppState,
    principal: &McpPrincipal,
    projects: &[Uuid],
    identifier: &str,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let reachable = |id: Uuid| {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, project_id FROM work_items WHERE id = $1 AND project_id = ANY($2)",
        )
        .bind(id)
        .bind(projects)
        .fetch_optional(&state.pool)
    };
    if let Some(id) = parse_id(identifier) {
        return Ok(reachable(id).await?);
    }
    let Some((project_key, number)) = super::split_task_key(identifier) else {
        return Ok(None);
    };
    // task_by_key keeps to the caller's reach; its answer is checked
    // against `projects` all the same, the list this lookup was given.
    if let Some(id) = super::task_by_key(state, principal, project_key, number).await?
        && let Some(found) = reachable(id).await?
    {
        return Ok(Some(found));
    }
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT item.id, item.project_id
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = ANY($3)
          AND upper(project.project_key) = upper($1) AND item.number = $2
        LIMIT 1
        "#,
    )
    .bind(project_key)
    .bind(number)
    .bind(projects)
    .fetch_optional(&state.pool)
    .await?)
}

/// Why an assignee can't be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AssigneeProblem {
    /// Nobody by that handle or ID is in the project.
    NotInProject(String),
    /// They're in the project but can't hold tasks (inactive, or not a
    /// person or agent).
    CantBeAssigned(String),
}

impl AssigneeProblem {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::NotInProject(message) | Self::CantBeAssigned(message) => message,
        }
    }
}

/// An assignee as agents write one: "me", "none" (nobody), an @handle or an
/// actor ID of someone in the project who can hold tasks. `Ok(None)` means
/// unassigned.
pub(super) async fn resolve_assignee(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    value: &str,
) -> Result<Result<Option<Uuid>, AssigneeProblem>, AppError> {
    let value = value.trim();
    match value.to_lowercase().as_str() {
        "me" => return Ok(Ok(Some(principal.actor_id))),
        "none" => return Ok(Ok(None)),
        _ => {}
    }
    let found = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT membership.actor_id, membership.display_name, project.project_key
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE membership.project_id = $1 AND membership.enabled
          AND (membership.actor_id = $2 OR lower(membership.handle) = lower($3))
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(value.parse::<Uuid>().ok())
    .bind(value.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?;
    let Some((actor_id, name, project_key)) = found else {
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_optional(&mut *connection)
                .await?
                .unwrap_or_else(|| "that project".to_owned());
        return Ok(Err(AssigneeProblem::NotInProject(format!(
            "No one called {value} is in {project_key}. Find people with people_search."
        ))));
    };
    if !validate_assignee(connection, project_id, Some(actor_id)).await? {
        return Ok(Err(AssigneeProblem::CantBeAssigned(format!(
            "{name} can't be assigned tasks in {project_key}: only active people and agents in the project can."
        ))));
    }
    Ok(Ok(Some(actor_id)))
}

/// The two shapes task_create and task_update have always given for an
/// assignee they can't use: an unknown handle names the person in
/// retryCategory, and an ID that can't be assigned is a correct_input.
fn assignee_error(person: &str, problem: AssigneeProblem) -> WriteResult {
    match problem {
        AssigneeProblem::NotInProject(message) if person.parse::<Uuid>().is_err() => {
            WriteResult::InvalidRequest {
                field: "assignee".to_owned(),
                retry_category: format!("No one called {person} is in that project."),
                message: Some(message),
            }
        }
        problem => WriteResult::invalid("assigneeActorId", problem.message()),
    }
}

pub(super) fn scoped(principal: &McpPrincipal, project_id: Uuid) -> McpPrincipal {
    let mut scoped = principal.clone();
    scoped.project_id = project_id;
    scoped.project_explicit = true;
    scoped
}

// ---------------------------------------------------------------------------
// task_open

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskOpenArguments {
    /// The task's ID, link, or key such as AW-12.
    #[serde(alias = "identifier")]
    pub task_id: String,
    /// Extra parts to return. "history": what happened to the task (status
    /// changes, edits, criteria, links and comments), with who, when, and
    /// through which app.
    pub include: Option<Vec<String>>,
    /// With include history: how many entries (default 30, at most 100).
    pub history_limit: Option<u16>,
    /// With include history: the olderCursor from a previous call, for
    /// earlier entries.
    pub history_before: Option<String>,
}

/// What task_open's include asks for, checked before anything is read.
struct HistoryRequest {
    limit: u16,
    before: Option<String>,
}

/// The include arguments, checked before anything is read: what to return,
/// or the field at fault and what would work.
fn history_request(
    arguments: &TaskOpenArguments,
) -> Result<Option<HistoryRequest>, (&'static str, String)> {
    let mut wants_history = false;
    for part in arguments.include.iter().flatten() {
        if part.trim().eq_ignore_ascii_case("history") {
            wants_history = true;
        } else {
            return Err((
                "include",
                "Use include: [\"history\"]. Nothing else can be included yet.".to_owned(),
            ));
        }
    }
    if !wants_history {
        for (field, given) in [
            ("historyBefore", arguments.history_before.is_some()),
            ("historyLimit", arguments.history_limit.is_some()),
        ] {
            if given {
                return Err((field, format!("Pass include: [\"history\"] with {field}.")));
            }
        }
        return Ok(None);
    }
    let limit = arguments
        .history_limit
        .unwrap_or(history::DEFAULT_HISTORY_LIMIT);
    if !(1..=history::MAX_HISTORY_LIMIT).contains(&limit) {
        return Err((
            "historyLimit",
            "Use a historyLimit from 1 to 100.".to_owned(),
        ));
    }
    Ok(Some(HistoryRequest {
        limit,
        before: arguments.history_before.clone(),
    }))
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskDetail {
    pub task: WorkItemProjection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<ClaimValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskSummary>,
    /// Threads and conversations about this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<LinkedItem>,
    /// Messages, pages and decisions linked to this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkedItem>,
    /// The latest comments and updates, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentItem>,
    /// Where to read how this project works: project_open or page_get.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide_page_id: Option<String>,
    /// With include: ["history"], what happened to the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<history::TaskHistory>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkedItem {
    pub kind: String,
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommentItem {
    pub author: String,
    pub text: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

pub(super) async fn task_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskOpenArguments,
) -> ToolResult<TaskDetail> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let history_request = match history_request(&arguments) {
        Ok(request) => request,
        Err((field, message)) => return ToolResult::invalid(field, message),
    };
    let result = async {
        let Some((task_id, project_id)) = resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        let scoped = scoped(principal, project_id);
        let task = match reads::work_item_get(state, &scoped, &task_id.to_string()).await {
            ReadResult::Found { value } => value,
            ReadResult::NotFound { .. } => return Ok(ToolResult::not_found("That task")),
            ReadResult::PermissionDenied { .. } => {
                return Ok(ToolResult::denied("You can't read tasks in that project."));
            }
            _ => return Ok(ToolResult::unavailable()),
        };
        // History comes only once the task was found for this reader. Its
        // cursor is bound to the task, so it's checked now.
        let history = match history_request {
            Some(request) => {
                let before = match request.before.as_deref() {
                    Some(cursor) => match decode_work_item_timeline_cursor(
                        cursor.trim(),
                        project_id,
                        task_id,
                    ) {
                        Ok(sequence) => Some(sequence),
                        Err(_) => {
                            return Ok(ToolResult::invalid(
                                "historyBefore",
                                "Pass the olderCursor you were given for this same task.",
                            ));
                        }
                    },
                    None => None,
                };
                Some(
                    history::task_history(
                        state,
                        principal,
                        project_id,
                        task_id,
                        request.limit,
                        before,
                    )
                    .await?,
                )
            }
            None => None,
        };
        let claimed_by = current_claim(state, task_id).await?;
        let subtasks = sqlx::query_as::<_, TaskSummary>(&format!(
            r#"
            SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
                   item.title, status.name AS status, status.semantic_category AS status_group,
                   item_type.name AS task_type, item.priority,
                   assignee.display_name AS assignee, 0::bigint AS board_position,
                   item.blocked_reason IS NOT NULL AS blocked, NULL::text AS claimed_by,
                   project.name AS project, item.version, item.updated_at::text AS updated_at,
                   item.archived_at IS NOT NULL AS archived, {TASK_LABELS_COLUMN}
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            JOIN work_item_types item_type
              ON item_type.project_id = item.project_id AND item_type.id = item.type_id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
            WHERE item.parent_work_item_id = $1 AND item.archived_at IS NULL
            ORDER BY item.number
            LIMIT 50
            "#
        ))
        .bind(task_id)
        .fetch_all(&state.pool)
        .await?;
        let conversations = sqlx::query_as::<_, LinkedItem>(
            r#"
            SELECT 'conversation' AS kind, conversation.id::text AS id,
                   CASE WHEN conversation.kind = 'dm' THEN 'DM' ELSE conversation.name END AS title,
                   CASE WHEN conversation.kind = 'thread' THEN conversation.thread_status END AS status,
                   COALESCE(conversation.thread_outcome, '') AS text
            FROM (
                SELECT conversation_id AS id FROM work_item_conversation_links WHERE work_item_id = $1
                UNION
                SELECT target_id FROM work_item_links
                WHERE work_item_id = $1 AND target_kind = 'conversation'
            ) linked
            JOIN conversations conversation ON conversation.id = linked.id
            WHERE conversation.archived_at IS NULL
              AND conversation.project_id = $3
              AND conversation_visible_to(conversation.id, $2)
            ORDER BY conversation.created_at
            "#,
        )
        .bind(task_id)
        .bind(principal.actor_id)
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
        let links = sqlx::query_as::<_, LinkedItem>(
            r#"
            SELECT link.target_kind AS kind, link.target_id::text AS id,
                   COALESCE(page.title, decision.title,
                            'Message from ' || COALESCE(author.display_name, 'someone')) AS title,
                   decision.status AS status,
                   CASE WHEN message.removed_at IS NOT NULL THEN 'Message removed'
                        ELSE left(COALESCE(page.body, decision.detail, message.plain_text, ''), 400)
                   END AS text
            FROM work_item_links link
            LEFT JOIN project_pages page
              ON link.target_kind = 'page' AND page.id = link.target_id AND page.archived_at IS NULL
             AND page.project_id = link.project_id
            LEFT JOIN project_decisions decision
              ON link.target_kind = 'decision' AND decision.id = link.target_id
             AND decision.project_id = link.project_id
             AND (decision.source_conversation_id IS NULL
                  OR conversation_visible_to(decision.source_conversation_id, $2))
            LEFT JOIN messages message
              ON link.target_kind = 'message' AND message.id = link.target_id
             AND message.project_id = link.project_id
             AND conversation_visible_to(message.conversation_id, $2)
            LEFT JOIN project_memberships author
              ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
            WHERE link.work_item_id = $1 AND link.target_kind <> 'conversation'
              AND (page.id IS NOT NULL OR decision.id IS NOT NULL OR message.id IS NOT NULL)
            ORDER BY link.created_at
            "#,
        )
        .bind(task_id)
        .bind(principal.actor_id)
        .fetch_all(&state.pool)
        .await?;
        let mut comments = sqlx::query_as::<_, CommentItem>(
            r#"
            SELECT COALESCE(author.display_name, 'Former member') AS author,
                   comment.plain_text AS text, comment.created_at::text AS at,
                   via_client.client_name AS via
            FROM work_item_comments comment
            LEFT JOIN project_memberships author
              ON author.project_id = comment.project_id AND author.actor_id = comment.author_actor_id
            LEFT JOIN mcp_oauth_grants via_grant
              ON via_grant.id = comment.via_oauth_grant_id AND via_grant.show_via
            LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
            WHERE comment.work_item_id = $1
            ORDER BY comment.created_at DESC
            LIMIT 10
            "#,
        )
        .bind(task_id)
        .fetch_all(&state.pool)
        .await?;
        comments.reverse();
        let guide_page_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM project_pages WHERE project_id = $1 AND is_guide AND archived_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?
        .map(|id| id.to_string());
        Ok::<_, AppError>(ToolResult::ok(TaskDetail {
            task,
            claimed_by,
            subtasks,
            conversations,
            links,
            comments,
            guide_page_id,
            history,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_open"))
}

// ---------------------------------------------------------------------------
// task_create and task_update, over the canonical commands

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCreateArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project to create it in. Not needed for a subtask.
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    pub priority: Option<String>,
    /// "me", an actor ID, or an @handle.
    #[serde(alias = "assigneeActorId")]
    pub assignee: Option<String>,
    /// Make it a subtask of this task (ID, link, or key such as AWD-3). It
    /// goes in its parent's project.
    #[serde(alias = "parentWorkItemId")]
    pub parent_id: Option<String>,
    /// Its status by name, such as "Ready" (see workflow_get). Left out, it
    /// starts in the project's first status.
    pub status: Option<String>,
    /// Its type by name, such as "Bug" (see workflow_get).
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    /// Labels by name or ID, at most 20. They must exist already; create one
    /// with label_write.
    pub labels: Option<Vec<String>>,
    /// What done looks like, one criterion per entry, in order (at most 50).
    pub acceptance_criteria: Option<Vec<String>>,
    /// Where it goes in its status column: top, bottom (the default), or a
    /// number, where 1 is the top.
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub position: Option<String>,
    /// Put it just above this task (ID, link, or key) in its column.
    pub before_task_id: Option<String>,
    /// Put it just below this task (ID, link, or key) in its column.
    pub after_task_id: Option<String>,
    /// A stable ID so retrying never creates it twice.
    pub client_request_id: Option<String>,
}

pub(super) async fn task_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCreateArguments,
) -> WriteResult {
    let client_request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let details = writes::NewTaskDetails {
        status: arguments.status.clone(),
        item_type: arguments.item_type.clone(),
        labels: arguments.labels.clone().filter(|labels| !labels.is_empty()),
        acceptance_criteria: arguments
            .acceptance_criteria
            .clone()
            .filter(|criteria| !criteria.is_empty()),
        position: arguments.position.clone(),
        before_task_id: arguments.before_task_id.clone(),
        after_task_id: arguments.after_task_id.clone(),
    };
    if arguments.parent_id.is_none() {
        // Tasks never go in the hidden Messages space, where nobody would
        // see them. A subtask goes where its parent is.
        let in_messages_space =
            sqlx::query_scalar::<_, bool>("SELECT kind = 'direct' FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(true);
        if !project_is_chosen(principal) || in_messages_space {
            const PROJECT_MESSAGE: &str = "Say which project with projectId (see projects_list).";
            return writes::WriteResult::InvalidRequest {
                field: "projectId".to_owned(),
                retry_category: PROJECT_MESSAGE.to_owned(),
                message: Some(PROJECT_MESSAGE.to_owned()),
            };
        }
    }
    let assignee_actor_id =
        match assignee_for(state, principal, arguments.assignee.as_deref()).await {
            Ok(assignee) => assignee.flatten().map(|id| id.to_string()),
            Err(result) => return result,
        };
    match arguments.parent_id {
        Some(parent) => {
            writes::create_subtask(
                state,
                principal,
                writes::CreateSubtaskArguments {
                    parent_work_item_id: parent,
                    title: arguments.title,
                    description: arguments.description,
                    client_request_id,
                    priority: arguments.priority,
                    assignee_actor_id,
                    details,
                },
            )
            .await
        }
        None => {
            writes::create_work_item(
                state,
                principal,
                writes::CreateWorkItemArguments {
                    title: arguments.title,
                    description: arguments.description,
                    priority: arguments.priority,
                    assignee_actor_id,
                    client_request_id,
                    details,
                },
            )
            .await
        }
    }
}

/// The assignee a task_create or task_update call named, in the project it
/// acts in: None when it named none, `Some(None)` for "none".
async fn assignee_for(
    state: &AppState,
    principal: &McpPrincipal,
    assignee: Option<&str>,
) -> Result<Option<Option<Uuid>>, WriteResult> {
    let Some(person) = assignee.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let resolved = async {
        let mut connection = state.pool.acquire().await?;
        resolve_assignee(&mut connection, principal, principal.project_id, person).await
    }
    .await;
    match resolved {
        Ok(Ok(assignee)) => Ok(Some(assignee)),
        Ok(Err(problem)) => Err(assignee_error(person, problem)),
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "MCP assignee lookup failed"
            );
            Err(WriteResult::TemporarilyUnavailable {
                retry_category: "retry_with_backoff".to_owned(),
            })
        }
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskUpdateArguments {
    // No parentWorkItemId alias here, ever: the middleware takes the
    // project from parentWorkItemId before taskId, and a task_update must
    // act in the task's own project.
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// The version you last read (from task_open or task_list).
    pub expected_version: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    pub priority: Option<String>,
    /// "me", "none", an actor ID, or an @handle.
    #[serde(alias = "assigneeActorId")]
    pub assignee: Option<String>,
    pub clear_assignee: Option<bool>,
    /// Move to this status, by name (such as "In Progress").
    pub status: Option<String>,
    /// Change its type, by name (such as "Bug").
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    /// Put it under this task (ID, link, or key) in the same project.
    pub parent_id: Option<String>,
    /// true makes it a top-level task again.
    pub clear_parent: Option<bool>,
    /// Labels to put on it, by name or ID (at most 20).
    pub add_labels: Option<Vec<String>>,
    /// Labels to take off it, by name or ID (at most 20).
    pub remove_labels: Option<Vec<String>>,
    /// Its place in its column (the new one, when status changes too): top,
    /// bottom, or a number, where 1 is the top. A status change without one
    /// puts it at the bottom.
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub position: Option<String>,
    /// Put it just above this task (ID, link, or key) in its column.
    pub before_task_id: Option<String>,
    /// Put it just below this task (ID, link, or key) in its column.
    pub after_task_id: Option<String>,
    /// true archives it (its claim is released), false restores it. Pass it
    /// on its own, then make other changes with the new version.
    pub archived: Option<bool>,
    /// A stable ID so retrying is safe.
    pub client_request_id: Option<String>,
}

impl TaskUpdateArguments {
    /// Whether the call changes anything besides the status and archived.
    fn changes_more_than_status(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.priority.is_some()
            || self
                .assignee
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self.clear_assignee == Some(true)
            || self.item_type.is_some()
            || self.parent_id.is_some()
            || self.clear_parent == Some(true)
            || self
                .add_labels
                .as_ref()
                .is_some_and(|labels| !labels.is_empty())
            || self
                .remove_labels
                .as_ref()
                .is_some_and(|labels| !labels.is_empty())
            || self.position.is_some()
            || self.before_task_id.is_some()
            || self.after_task_id.is_some()
    }
}

pub(super) async fn task_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskUpdateArguments,
) -> WriteResult {
    let client_request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    if let Some(archived) = arguments.archived {
        if arguments.changes_more_than_status() || arguments.status.is_some() {
            return WriteResult::invalid(
                "archived",
                "Pass archived on its own. Archive or restore first, then make other changes with the new version.",
            );
        }
        return writes::set_archived(
            state,
            principal,
            writes::SetArchivedArguments {
                work_item_id: arguments.task_id,
                expected_version: arguments.expected_version,
                archived,
                client_request_id,
            },
        )
        .await;
    }
    let mut clear_assignee = arguments.clear_assignee;
    let assignee_actor_id =
        match assignee_for(state, principal, arguments.assignee.as_deref()).await {
            Ok(Some(Some(id))) => Some(id.to_string()),
            // "none" leaves it unassigned.
            Ok(Some(None)) => {
                clear_assignee = Some(true);
                None
            }
            Ok(None) => None,
            Err(result) => return result,
        };
    // Every field must count here: a status call with nothing else goes to
    // set_status, which would drop any other change silently.
    let changes_fields = arguments.changes_more_than_status() || clear_assignee == Some(true);
    let status = arguments.status.clone();
    if let Some(status) = status.clone()
        && !changes_fields
    {
        return writes::set_status(
            state,
            principal,
            writes::SetStatusArguments {
                work_item_id: arguments.task_id,
                expected_version: arguments.expected_version,
                status,
                client_request_id,
            },
        )
        .await;
    }
    let update = writes::UpdateWorkItemArguments {
        work_item_id: arguments.task_id,
        expected_version: arguments.expected_version,
        title: arguments.title,
        description: arguments.description,
        priority: arguments.priority,
        assignee_actor_id,
        clear_assignee,
        client_request_id,
        item_type: arguments.item_type,
        parent_id: arguments.parent_id,
        clear_parent: arguments.clear_parent.filter(|clear| *clear),
        add_labels: arguments.add_labels.filter(|labels| !labels.is_empty()),
        remove_labels: arguments.remove_labels.filter(|labels| !labels.is_empty()),
        position: arguments.position,
        before_task_id: arguments.before_task_id,
        after_task_id: arguments.after_task_id,
    };
    match status {
        Some(status) => writes::update_work_item_and_status(state, principal, update, status).await,
        None => writes::update_work_item(state, principal, update).await,
    }
}

// ---------------------------------------------------------------------------
// task_reorder

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReorderArguments {
    /// Tasks in the order you want them, all in one status column (IDs,
    /// links, or keys such as AWD-12).
    pub task_ids: Vec<String>,
    /// top (the default) or bottom of their column. Leave it out when you
    /// pass beforeTaskId or afterTaskId.
    pub position: Option<String>,
    /// Put them just above this task, which must be in the same column.
    pub before_task_id: Option<String>,
    /// Put them just below this task, which must be in the same column.
    pub after_task_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderValue {
    pub status: String,
    /// The whole column after the change, top first.
    pub column: Vec<String>,
    /// False when they were already in that order and nothing changed.
    pub changed: bool,
    /// The tasks you placed, with their versions and places now.
    pub tasks: Vec<ReorderedTask>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderedTask {
    pub id: String,
    pub key: String,
    pub version: i64,
    /// 1 is the top of the column.
    pub board_position: i64,
}

/// Where task_reorder puts the tasks it moves.
#[derive(Clone, Copy, Debug)]
enum ReorderPlace {
    Top,
    Bottom,
    Before(Uuid),
    After(Uuid),
}

pub(super) async fn task_reorder(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReorderArguments,
) -> ToolResult<ReorderValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if arguments.task_ids.is_empty() || arguments.task_ids.len() > 100 {
        return ToolResult::invalid("taskIds", "Pass between 1 and 100 tasks.");
    }
    let anchor = match (
        arguments.before_task_id.as_deref(),
        arguments.after_task_id.as_deref(),
    ) {
        (Some(_), Some(_)) => {
            return ToolResult::invalid(
                "afterTaskId",
                "Pass beforeTaskId or afterTaskId, not both.",
            );
        }
        (Some(anchor), None) => Some(("beforeTaskId", anchor, true)),
        (None, Some(anchor)) => Some(("afterTaskId", anchor, false)),
        (None, None) => None,
    };
    if anchor.is_some() && arguments.position.is_some() {
        return ToolResult::invalid(
            "position",
            "Use position, or beforeTaskId/afterTaskId, not both.",
        );
    }
    let at_top = match arguments
        .position
        .as_deref()
        .map(str::trim)
        .unwrap_or("top")
    {
        "top" => true,
        "bottom" => false,
        _ => {
            return ToolResult::invalid(
                "position",
                "Use top or bottom, or put them next to a task with beforeTaskId or afterTaskId.",
            );
        }
    };
    task_reorder_inner(state, principal, &arguments.task_ids, anchor, at_top)
        .await
        .unwrap_or_else(|error| ToolResult::from_error(error, "task_reorder"))
}

async fn task_reorder_inner(
    state: &AppState,
    principal: &McpPrincipal,
    references: &[String],
    anchor: Option<(&'static str, &str, bool)>,
    at_top: bool,
) -> Result<ToolResult<ReorderValue>, AppError> {
    let mut ordered: Vec<(Uuid, Uuid)> = Vec::with_capacity(references.len());
    for (reference, found) in references
        .iter()
        .zip(resolve_tasks(state, principal, references).await?)
    {
        match found {
            Some(found) if !ordered.contains(&found) => ordered.push(found),
            Some(_) => return Ok(ToolResult::invalid("taskIds", "Each task can appear once.")),
            None => {
                return Ok(ToolResult::not_found(&format!(
                    "Task {}",
                    snippet(reference, 60)
                )));
            }
        }
    }
    let project_id = ordered[0].1;
    if ordered.iter().any(|(_, project)| *project != project_id) {
        return Ok(ToolResult::invalid(
            "taskIds",
            "All the tasks must be in one project.",
        ));
    }
    let ids = ordered.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let mut field = "position";
    let place = match anchor {
        None if at_top => ReorderPlace::Top,
        None => ReorderPlace::Bottom,
        Some((anchor_field, reference, before)) => {
            field = anchor_field;
            let Some((anchor_id, anchor_project)) =
                resolve_task(state, principal, reference).await?
            else {
                return Ok(ToolResult::not_found(&format!(
                    "Task {}",
                    snippet(reference, 60)
                )));
            };
            // resolve_task only finds tasks the caller can reach, so naming
            // the anchor back leaks nothing.
            let anchor_key = task_key_by_id(state, anchor_id).await?;
            if anchor_project != project_id {
                return Ok(ToolResult::invalid(
                    field,
                    format!(
                        "{anchor_key} is in another project. Pick a task in the same column as the ones you're moving."
                    ),
                ));
            }
            if ids.contains(&anchor_id) {
                return Ok(ToolResult::invalid(
                    field,
                    format!(
                        "{anchor_key} is one of the tasks you're moving. Pick a task next to where they should go."
                    ),
                ));
            }
            if before {
                ReorderPlace::Before(anchor_id)
            } else {
                ReorderPlace::After(anchor_id)
            }
        }
    };

    let mut transaction = state.pool.begin().await?;
    if let Some(refused) = lock_for_task_write(&mut transaction, principal, project_id).await? {
        return Ok(refused);
    }
    let locked = lock_work_items(&mut transaction, project_id, &ids).await?;
    if locked.len() != ids.len() {
        return Ok(ToolResult::not_found("One of those tasks"));
    }
    let names = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM work_item_statuses WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();
    let status_name = |id: Uuid| {
        names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "another status".to_owned())
    };
    // Answer in the order the caller gave the tasks.
    let moving = ids
        .iter()
        .filter_map(|id| locked.iter().find(|task| task.id == *id))
        .collect::<Vec<_>>();
    let status_id = moving[0].status_id;
    let mut named = ids.clone();
    if let ReorderPlace::Before(anchor) | ReorderPlace::After(anchor) = place {
        named.push(anchor);
    }
    let keys = column_keys(&mut transaction, project_id, status_id, &named).await?;
    let key_of = |id: Uuid| keys.get(&id).cloned().unwrap_or_else(|| id.to_string());
    if let Some(archived) = moving.iter().find(|task| task.archived) {
        return Ok(ToolResult::invalid(
            "taskIds",
            format!(
                "{} is archived. Restore it before reordering.",
                key_of(archived.id)
            ),
        ));
    }
    let mut statuses = Vec::new();
    for task in &moving {
        if !statuses.contains(&task.status_id) {
            statuses.push(task.status_id);
        }
    }
    if statuses.len() > 1 {
        let listed = statuses
            .iter()
            .map(|id| status_name(*id))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(ToolResult::invalid(
            "taskIds",
            format!(
                "These tasks are in different columns ({listed}). Reorder one column at a time, or move them with task_update first."
            ),
        ));
    }
    let status = status_name(status_id);
    if let ReorderPlace::Before(anchor) | ReorderPlace::After(anchor) = place {
        let (anchor_status, anchor_archived) = sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT status_id, archived_at IS NOT NULL FROM work_items WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(anchor)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("task"))?;
        let anchor_key = key_of(anchor);
        if anchor_archived {
            return Ok(ToolResult::invalid(
                field,
                format!("{anchor_key} is archived. Pick an open task in {status}."),
            ));
        }
        if anchor_status != status_id {
            return Ok(ToolResult::invalid(
                field,
                format!(
                    "{anchor_key} is in {}, but these tasks are in {status}. Pick a task in {status}, or move them with task_update first.",
                    status_name(anchor_status)
                ),
            ));
        }
    }

    // The column's open tasks, locked in board order. Every writer of a
    // column, and task_claim, takes its turn on the project row first, so
    // this can't deadlock.
    let column = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT id, board_rank FROM work_items
        WHERE project_id = $1 AND status_id = $2 AND archived_at IS NULL
        ORDER BY board_rank, id
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(status_id)
    .fetch_all(&mut *transaction)
    .await?;
    let current = column.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let mut rest = column
        .iter()
        .filter(|(id, _)| !ids.contains(id))
        .copied()
        .collect::<Vec<_>>();
    let position_of = |anchor: Uuid| {
        rest.iter()
            .position(|(id, _)| *id == anchor)
            .ok_or(AppError::Internal("reorder anchor left its column"))
    };
    let index = match place {
        ReorderPlace::Top => 0,
        ReorderPlace::Bottom => rest.len(),
        ReorderPlace::Before(anchor) => position_of(anchor)?,
        ReorderPlace::After(anchor) => position_of(anchor)? + 1,
    };
    let desired = rest[..index]
        .iter()
        .map(|(id, _)| *id)
        .chain(ids.iter().copied())
        .chain(rest[index..].iter().map(|(id, _)| *id))
        .collect::<Vec<_>>();
    let place_of = |id: Uuid| {
        desired
            .iter()
            .position(|placed| *placed == id)
            .and_then(|index| i64::try_from(index + 1).ok())
            .unwrap_or(0)
    };
    let column_keys = desired.iter().map(|id| key_of(*id)).collect::<Vec<_>>();

    // Already in that order: nothing is written, so a retry is harmless.
    if desired == current {
        transaction.commit().await?;
        return Ok(ToolResult::ok(ReorderValue {
            status,
            column: column_keys,
            changed: false,
            tasks: moving
                .iter()
                .map(|task| ReorderedTask {
                    id: task.id.to_string(),
                    key: key_of(task.id),
                    version: task.version,
                    board_position: place_of(task.id),
                })
                .collect(),
        }));
    }
    let ranks =
        ranks_for_placement(&mut transaction, project_id, &mut rest, index, ids.len()).await?;
    let actor = command_actor(principal, project_id);
    let mut tasks = Vec::with_capacity(ids.len());
    for (id, rank) in ids.iter().zip(ranks) {
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE work_items SET board_rank = $3, version = version + 1, updated_at = now()
            WHERE project_id = $1 AND id = $2
            RETURNING version
            "#,
        )
        .bind(project_id)
        .bind(id)
        .bind(rank)
        .fetch_one(&mut *transaction)
        .await?;
        let position = place_of(*id);
        let payload = ChangePayload {
            changed_fields: vec!["position".to_owned()],
            position: Some(position),
            ..ChangePayload::default()
        };
        // A reorder isn't news for an assignee, so unlike other changes it
        // sends no event to an assigned agent.
        insert_activity(
            &mut transaction,
            project_id,
            *id,
            principal.actor_id,
            "work_item_updated",
            "Task reordered on the board through MCP",
            &payload.activity(),
        )
        .await?;
        enqueue_update(&mut transaction, project_id, *id, version).await?;
        let mut metadata = payload.audit(version);
        if let (Some(object), ReorderPlace::Before(anchor) | ReorderPlace::After(anchor)) =
            (metadata.as_object_mut(), place)
        {
            object.insert(field.to_owned(), serde_json::json!(anchor));
        }
        record_audit(
            &mut transaction,
            &actor,
            "work_item.mcp_reordered",
            *id,
            metadata,
        )
        .await?;
        tasks.push(ReorderedTask {
            id: id.to_string(),
            key: key_of(*id),
            version,
            board_position: position,
        });
    }
    transaction.commit().await?;
    Ok(ToolResult::ok(ReorderValue {
        status,
        column: column_keys,
        changed: true,
        tasks,
    }))
}

/// Keys such as AWD-12, by ID, for a column's open tasks and the `named`
/// ones.
async fn column_keys(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    status_id: Uuid,
    named: &[Uuid],
) -> Result<HashMap<Uuid, String>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1
          AND (item.id = ANY($3) OR (item.status_id = $2 AND item.archived_at IS NULL))
        "#,
    )
    .bind(project_id)
    .bind(status_id)
    .bind(named)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect())
}

/// A task's key, such as AWD-12, for a task the caller has already reached.
async fn task_key_by_id(state: &AppState, task_id: Uuid) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT project.project_key || '-' || item.number::text
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.id = $1
        "#,
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or_else(|| task_id.to_string()))
}

/// Takes the project lock every task write takes first (`lock_membership`,
/// through `check_write_access`), and checks the caller may still change
/// tasks there. None means go ahead.
async fn lock_for_task_write<T>(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    project_id: Uuid,
) -> Result<Option<ToolResult<T>>, AppError> {
    Ok(
        match writes::check_write_access(transaction, &scoped(principal, project_id)).await? {
            WriteAccess::Allowed => None,
            WriteAccess::ReadOnly => Some(ToolResult::read_only()),
            WriteAccess::NotFound => Some(ToolResult::not_found("That project")),
            WriteAccess::ProjectArchived => Some(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE)),
            WriteAccess::Denied => Some(ToolResult::denied(
                "You can't change tasks in that project.",
            )),
        },
    )
}

/// The command actor for a write in `project_id` that takes no
/// clientRequestId; the audit row gets a fresh request ID.
pub(super) fn command_actor(principal: &McpPrincipal, project_id: Uuid) -> CommandActor {
    writes::mcp_actor(&scoped(principal, project_id), Uuid::now_v7().to_string())
}

// ---------------------------------------------------------------------------
// Claims

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskClaimArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// What you're doing, shown to others.
    pub note: Option<String>,
    /// How long the claim lasts, in minutes (default 120, at most 1440).
    /// Claiming again renews it.
    pub minutes: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClaimValue {
    pub task_id: String,
    pub claimed_by: String,
    pub note: Option<String>,
    pub claimed_at: String,
    pub expires_at: String,
}

#[derive(Debug, FromRow)]
struct ClaimRow {
    work_item_id: Uuid,
    claimed_by: String,
    note: Option<String>,
    claimed_at: String,
    expires_at: String,
}

async fn current_claim(state: &AppState, task_id: Uuid) -> Result<Option<ClaimValue>, AppError> {
    Ok(sqlx::query_as::<_, ClaimRow>(
        r#"
        SELECT claim.work_item_id, COALESCE(member.display_name, 'Someone') AS claimed_by,
               claim.note, claim.claimed_at::text AS claimed_at,
               claim.expires_at::text AS expires_at
        FROM work_item_claims claim
        LEFT JOIN project_memberships member
          ON member.project_id = claim.project_id AND member.actor_id = claim.actor_id
        WHERE claim.work_item_id = $1 AND claim.expires_at > now()
        "#,
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await?
    .map(|row| ClaimValue {
        task_id: row.work_item_id.to_string(),
        claimed_by: row.claimed_by,
        note: row.note,
        claimed_at: row.claimed_at,
        expires_at: row.expires_at,
    }))
}

pub(super) async fn task_claim(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskClaimArguments,
) -> ToolResult<ClaimValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let minutes = arguments.minutes.map_or(DEFAULT_CLAIM_MINUTES, i32::from);
    if !(5..=1440).contains(&minutes) {
        return ToolResult::invalid("minutes", "Claim for 5 to 1440 minutes.");
    }
    let note = arguments
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| snippet(value, 500));
    let result = async {
        let Some((task_id, project_id)) =
            resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied(
                "You can't work on tasks in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        // The claim takes its turn on the project row before the task, as
        // every task write does (lock_membership) before it locks tasks and
        // the board column a status change or reorder lands in. Locked the
        // other way round, such a write waited for this task while this
        // claim's insert waited on the project row for its foreign key. NO
        // KEY UPDATE waits for those writes but never for a foreign key's
        // share lock.
        sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR NO KEY UPDATE")
            .bind(project_id)
            .fetch_optional(&mut *transaction)
            .await?;
        // Claimers take turns on the task row. With a shared lock here, two
        // agents claiming at once deadlocked: the winner's activity write
        // waited on the loser's lock while the loser waited on the claim.
        let archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM work_items WHERE id = $1 FOR NO KEY UPDATE",
        )
        .bind(task_id)
        .fetch_one(&mut *transaction)
        .await?;
        if archived {
            return Ok(ToolResult::conflict("That task is archived."));
        }
        // The second claimer runs after the first commits and finds a live
        // claim it doesn't own, so the update below changes nothing.
        let claimed = sqlx::query_as::<_, (Uuid, bool)>(
            r#"
            INSERT INTO work_item_claims (work_item_id, project_id, actor_id, note, expires_at)
            VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
            ON CONFLICT (work_item_id) DO UPDATE
            SET actor_id = EXCLUDED.actor_id,
                note = COALESCE(EXCLUDED.note, work_item_claims.note),
                claimed_at = CASE WHEN work_item_claims.actor_id = EXCLUDED.actor_id
                                  THEN work_item_claims.claimed_at ELSE now() END,
                expires_at = EXCLUDED.expires_at
            WHERE work_item_claims.actor_id = EXCLUDED.actor_id
               OR work_item_claims.expires_at <= now()
            RETURNING actor_id, (xmax = 0) AS inserted
            "#,
        )
        .bind(task_id)
        .bind(project_id)
        .bind(principal.actor_id)
        .bind(&note)
        .bind(minutes)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((_, fresh)) = claimed else {
            transaction.rollback().await?;
            let holder = current_claim(state, task_id).await?;
            return Ok(ToolResult::conflict(match holder {
                Some(holder) => format!(
                    "{} is working on this until {}{}. Pick another task, or ask them.",
                    holder.claimed_by,
                    holder.expires_at,
                    holder
                        .note
                        .map(|note| format!(" ({note})"))
                        .unwrap_or_default()
                ),
                None => "Someone else just claimed this task. Try again.".to_owned(),
            }));
        };
        if fresh {
            insert_activity(
                &mut transaction,
                project_id,
                task_id,
                principal.actor_id,
                "work_item_updated",
                "Started working on this",
                &serde_json::json!({ "claim": true }),
            )
            .await?;
        }
        transaction.commit().await?;
        let claim = current_claim(state, task_id)
            .await?
            .ok_or(AppError::Internal("claim just written"))?;
        Ok::<_, AppError>(ToolResult::ok(claim))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_claim"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReleaseArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReleaseValue {
    pub task_id: String,
    pub released: bool,
}

pub(super) async fn task_release(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReleaseArguments,
) -> ToolResult<ReleaseValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let result = async {
        let Some((task_id, _)) = resolve_task(state, principal, &arguments.task_id).await? else {
            return Ok(ToolResult::not_found("That task"));
        };
        let released =
            sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1 AND actor_id = $2")
                .bind(task_id)
                .bind(principal.actor_id)
                .execute(&state.pool)
                .await?
                .rows_affected()
                == 1;
        Ok::<_, AppError>(ToolResult::ok(ReleaseValue {
            task_id: task_id.to_string(),
            released,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_release"))
}

// ---------------------------------------------------------------------------
// task_link

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskLinkArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// A thread or conversation about this task.
    pub conversation_id: Option<String>,
    /// A message this task came from: its ID, or a link to it.
    pub message_id: Option<String>,
    /// A page in the task's project.
    pub page_id: Option<String>,
    /// A decision in the task's project.
    pub decision_id: Option<String>,
    /// A repository in the task's project: its ID, link, or owner/name (or
    /// just its name when only one repository there has it).
    pub repository_id: Option<String>,
    /// A pull request Agent Workspace knows about: its ID, link, or GitHub
    /// URL. With the project's GitHub automation on, a linked pull request
    /// moves the task when it's opened or merged.
    pub pull_request_id: Option<String>,
    /// For repositoryId only: related or primary, the repository Codex runs
    /// on this task's thread use. Making one primary makes the old primary
    /// related. Left out, a new link is related and an existing link keeps
    /// its role.
    pub role: Option<String>,
    /// The version you last read. Optional; when given, a change is refused
    /// if the task has changed since. Asking for a link that's already there
    /// (or already gone) changes nothing and always succeeds.
    pub expected_version: Option<i64>,
    /// Remove the link instead.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkValue {
    pub task_id: String,
    /// The task's key, such as AWD-12.
    pub key: String,
    /// conversation, message, page, decision, repository, or pull_request.
    pub kind: String,
    pub target_id: String,
    pub linked: bool,
    /// False when it was already that way and nothing changed.
    pub changed: bool,
    /// The task's version after the call.
    pub version: i64,
    /// A linked repository's role: primary or related.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Anything else worth knowing, such as what a pull request link sets in
    /// motion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub task_url: String,
}

/// What a task_link call points at, found in the task's own project.
#[derive(Clone, Debug)]
enum LinkTarget {
    /// A conversation, message, page or decision (work_item_links), or a
    /// thread in the task's project (work_item_conversation_links).
    Record {
        kind: &'static str,
        id: Uuid,
    },
    Repository {
        id: Uuid,
        name: String,
    },
    PullRequest {
        id: Uuid,
    },
}

pub(super) async fn task_link(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskLinkArguments,
) -> ToolResult<LinkValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let targets = [
        (
            "conversation",
            "conversationId",
            arguments.conversation_id.as_deref(),
        ),
        ("message", "messageId", arguments.message_id.as_deref()),
        ("page", "pageId", arguments.page_id.as_deref()),
        ("decision", "decisionId", arguments.decision_id.as_deref()),
        (
            "repository",
            "repositoryId",
            arguments.repository_id.as_deref(),
        ),
        (
            "pull_request",
            "pullRequestId",
            arguments.pull_request_id.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(kind, field, value)| value.map(|value| (kind, field, value.trim())))
    .collect::<Vec<_>>();
    let [(kind, field, target)] = targets.as_slice() else {
        return ToolResult::invalid(
            "taskId",
            "Pass exactly one of conversationId, messageId, pageId, decisionId, repositoryId, or pullRequestId.",
        );
    };
    let remove = arguments.remove.unwrap_or(false);
    let role = match arguments.role.as_deref().map(str::trim) {
        None => None,
        Some(_) if *kind != "repository" => {
            return ToolResult::invalid("role", "role only applies to repositoryId.");
        }
        Some(_) if remove => {
            return ToolResult::invalid("role", "Leave out role when removing a link.");
        }
        Some(role) => match role.to_lowercase().as_str() {
            "primary" => Some("primary"),
            "related" => Some("related"),
            _ => return ToolResult::invalid("role", "Use primary or related."),
        },
    };
    if arguments
        .expected_version
        .is_some_and(|version| version < 1)
    {
        return ToolResult::invalid(
            "expectedVersion",
            "Pass the version you last read (from task_open or task_list), or leave it out.",
        );
    }
    if target.is_empty() {
        return ToolResult::invalid(field, "Pass an ID or link.");
    }
    let record_id = match *kind {
        "repository" | "pull_request" => None,
        "message" => match parse_message_id(target) {
            Some(id) => Some(id),
            None => return ToolResult::invalid(field, "Pass an ID or link."),
        },
        _ => match parse_id(target) {
            Some(id) => Some(id),
            None => return ToolResult::invalid(field, "Pass an ID or link."),
        },
    };
    let request = LinkRequest {
        kind,
        target,
        record_id,
        role,
        remove,
        expected_version: arguments.expected_version,
    };
    task_link_inner(state, principal, &arguments.task_id, request)
        .await
        .unwrap_or_else(|error| ToolResult::from_error(error, "task_link"))
}

struct LinkRequest<'a> {
    kind: &'static str,
    target: &'a str,
    record_id: Option<Uuid>,
    role: Option<&'static str>,
    remove: bool,
    expected_version: Option<i64>,
}

async fn task_link_inner(
    state: &AppState,
    principal: &McpPrincipal,
    task: &str,
    request: LinkRequest<'_>,
) -> Result<ToolResult<LinkValue>, AppError> {
    // The task decides the project, never principal.project_id: the
    // middleware takes the project from a repositoryId or conversationId
    // before taskId.
    let Some((task_id, project_id)) = resolve_task(state, principal, task).await? else {
        return Ok(ToolResult::not_found("That task"));
    };
    let key = task_key_by_id(state, task_id).await?;
    let target = match request.record_id {
        Some(id) => {
            if !record_visible(
                state,
                principal,
                project_id,
                request.kind,
                id,
                request.remove,
            )
            .await?
            {
                return Ok(ToolResult::not_found(&format!("That {}", request.kind)));
            }
            LinkTarget::Record {
                kind: request.kind,
                id,
            }
        }
        None if request.kind == "repository" => {
            match find_repository(state, project_id, request.target).await? {
                Ok((id, name)) => LinkTarget::Repository { id, name },
                Err(refused) => return Ok(refused),
            }
        }
        None => {
            match find_pull_request(state, principal, project_id, &key, request.target).await? {
                Ok(id) => LinkTarget::PullRequest { id },
                Err(refused) => return Ok(refused),
            }
        }
    };

    let mut transaction = state.pool.begin().await?;
    if let Some(refused) = lock_for_task_write(&mut transaction, principal, project_id).await? {
        return Ok(refused);
    }
    let Some(current) = lock_work_item(&mut transaction, project_id, task_id).await? else {
        return Ok(ToolResult::not_found("That task"));
    };
    if current.archived {
        return Ok(ToolResult::conflict(archived_task_message(&key)));
    }
    let stale = || ToolResult::conflict(version_conflict_message(&key, current.version));
    let actor = command_actor(principal, project_id);
    let linked = !request.remove;
    let mut notes = Vec::new();
    let mut role = None;
    let (target_id, version) = match target {
        LinkTarget::Repository { id, name } => {
            // Without a role, a repository that's already linked keeps the
            // role it has; only a new link defaults to related. Otherwise
            // relinking the primary would quietly make it related and move
            // where Codex runs.
            let kept = if linked && request.role.is_none() {
                sqlx::query_scalar::<_, String>(
                    "SELECT role FROM work_item_repository_links WHERE project_id = $1 AND work_item_id = $2 AND repository_id = $3",
                )
                .bind(project_id)
                .bind(task_id)
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await?
            } else {
                None
            };
            let desired = linked.then(|| request.role.unwrap_or("related"));
            let outcome = if kept.is_some() {
                RepositoryLinkOutcome::Unchanged
            } else {
                change_repository_link(
                    &mut transaction,
                    &actor,
                    &current,
                    request.expected_version,
                    id,
                    desired,
                    LinkWording::MCP,
                )
                .await?
            };
            let version = match outcome {
                RepositoryLinkOutcome::RepositoryNotFound => {
                    return Ok(ToolResult::not_found("That repository"));
                }
                RepositoryLinkOutcome::NotEligibleForPrimary {
                    enabled,
                    trusted,
                    preparation_state,
                    archived,
                } => {
                    let mut reasons = Vec::new();
                    if !enabled {
                        reasons.push("it's turned off".to_owned());
                    }
                    if !trusted {
                        reasons.push("it isn't trusted".to_owned());
                    }
                    if preparation_state != "ready" {
                        reasons.push(format!("it isn't ready (state: {preparation_state})"));
                    }
                    if archived {
                        reasons.push("it's archived".to_owned());
                    }
                    return Ok(ToolResult::conflict(format!(
                        "{name} can't be the primary repository: {}. Link it as related, or finish setting it up in project settings.",
                        reasons.join(", ")
                    )));
                }
                RepositoryLinkOutcome::RepositoryArchived => {
                    return Ok(ToolResult::conflict(format!(
                        "{name} is archived. Restore it in project settings first."
                    )));
                }
                RepositoryLinkOutcome::VersionConflict => return Ok(stale()),
                RepositoryLinkOutcome::Unchanged => current.version,
                RepositoryLinkOutcome::Changed {
                    version,
                    demoted_primary,
                    ..
                } => {
                    if let Some(demoted) = demoted_primary {
                        let demoted = sqlx::query_scalar::<_, String>(
                            "SELECT owner || '/' || name FROM repositories WHERE project_id = $1 AND id = $2",
                        )
                        .bind(project_id)
                        .bind(demoted)
                        .fetch_one(&mut *transaction)
                        .await?;
                        notes.push(format!("{demoted} was primary; it's now related."));
                    }
                    version
                }
            };
            role = kept.or_else(|| desired.map(str::to_owned));
            (id, version)
        }
        LinkTarget::PullRequest { id } => {
            let version = match writes::change_pull_request_link(
                &mut transaction,
                project_id,
                task_id,
                current.version,
                request.expected_version,
                id,
                principal.actor_id,
                linked,
                if linked {
                    "Pull request linked through MCP"
                } else {
                    "Pull request unlinked through MCP"
                },
            )
            .await?
            {
                PullRequestLinkChange::NotInProject => {
                    return Ok(ToolResult::not_found("That pull request"));
                }
                PullRequestLinkChange::VersionConflict => return Ok(stale()),
                PullRequestLinkChange::Unchanged => current.version,
                PullRequestLinkChange::Changed { version, .. } => {
                    record_audit(
                        &mut transaction,
                        &actor,
                        if linked {
                            "work_item.mcp_pull_request_linked"
                        } else {
                            "work_item.mcp_pull_request_unlinked"
                        },
                        task_id,
                        serde_json::json!({"pullRequestId": id, "linked": linked, "version": version}),
                    )
                    .await?;
                    version
                }
            };
            if linked {
                notes.push(automation_note(&mut transaction, project_id, &key).await?);
            }
            (id, version)
        }
        LinkTarget::Record { kind, id } => {
            let thread = kind == "conversation"
                && sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM conversations WHERE project_id = $1 AND id = $2 AND kind = 'thread')",
                )
                .bind(project_id)
                .bind(id)
                .fetch_one(&mut *transaction)
                .await?;
            let version = if thread {
                // Threads in the task's own project use the link the web app
                // shows, with the same version bump, activity and audit.
                if request.remove {
                    sqlx::query(
                        "DELETE FROM work_item_links WHERE work_item_id = $1 AND target_kind = 'conversation' AND target_id = $2",
                    )
                    .bind(task_id)
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?;
                }
                match change_thread_link(
                    &mut transaction,
                    &actor,
                    &current,
                    request.expected_version,
                    id,
                    linked,
                    LinkWording::MCP,
                )
                .await?
                {
                    ThreadLinkOutcome::ThreadNotFound => {
                        return Ok(ToolResult::not_found("That conversation"));
                    }
                    ThreadLinkOutcome::VersionConflict => return Ok(stale()),
                    ThreadLinkOutcome::Unchanged => current.version,
                    ThreadLinkOutcome::Changed { version, .. } => version,
                }
            } else {
                match change_record_link(
                    &mut transaction,
                    &actor,
                    &current,
                    request.expected_version,
                    kind,
                    id,
                    linked,
                )
                .await?
                {
                    Some(version) => version,
                    None => return Ok(stale()),
                }
            };
            (id, version)
        }
    };
    transaction.commit().await?;
    Ok(ToolResult::ok(LinkValue {
        task_id: task_id.to_string(),
        key,
        kind: request.kind.to_owned(),
        target_id: target_id.to_string(),
        linked,
        changed: version != current.version,
        version,
        role,
        notes,
        task_url: public_url(
            state,
            &format!("/projects/{project_id}/work-items/{task_id}"),
        ),
    }))
}

/// Whether a conversation, message, page or decision is in the task's
/// project and the caller can see it, so a link never shows another
/// project's (or a private conversation's) content to the task's readers.
/// A removed message can be unlinked but not linked.
async fn record_visible(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
    kind: &str,
    id: Uuid,
    unlinking: bool,
) -> Result<bool, AppError> {
    Ok(match kind {
        "conversation" => sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND project_id = $3
               AND conversation_visible_to(id, $2) AND actor_can_access_project($2, $4, $3))"#,
        ),
        "message" if unlinking => sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (SELECT 1 FROM messages WHERE id = $1 AND project_id = $3
               AND conversation_visible_to(conversation_id, $2) AND actor_can_access_project($2, $4, $3))"#,
        ),
        "message" => sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (SELECT 1 FROM messages WHERE id = $1 AND project_id = $3
               AND removed_at IS NULL
               AND conversation_visible_to(conversation_id, $2) AND actor_can_access_project($2, $4, $3))"#,
        ),
        "page" => sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (SELECT 1 FROM project_pages WHERE id = $1 AND project_id = $3
               AND archived_at IS NULL AND actor_can_access_project($2, $4, $3))"#,
        ),
        _ => sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (SELECT 1 FROM project_decisions WHERE id = $1 AND project_id = $3
               AND (source_conversation_id IS NULL OR conversation_visible_to(source_conversation_id, $2))
               AND actor_can_access_project($2, $4, $3))"#,
        ),
    }
    .bind(id)
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.external_agent_connection_id)
    .fetch_one(&state.pool)
    .await?)
}

/// Links a task to a conversation (other than a thread in its project), a
/// message, a page or a decision, or unlinks it. Returns the task's version
/// afterwards (unchanged when it was already that way), or None when the
/// change would apply but the task isn't at `expected_version`. A change is
/// one version bump, activity, audit row and outbox event, like any other.
async fn change_record_link(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    current: &LockedWorkItem,
    expected_version: Option<i64>,
    kind: &str,
    target_id: Uuid,
    link: bool,
) -> Result<Option<i64>, AppError> {
    let linked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_item_links WHERE work_item_id = $1 AND target_kind = $2 AND target_id = $3)",
    )
    .bind(current.id)
    .bind(kind)
    .bind(target_id)
    .fetch_one(&mut **transaction)
    .await?;
    if linked == link {
        return Ok(Some(current.version));
    }
    if expected_version.is_some_and(|expected| expected != current.version) {
        return Ok(None);
    }
    if link {
        sqlx::query(
            r#"
            INSERT INTO work_item_links (work_item_id, project_id, target_kind, target_id, created_by_actor_id)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(current.id)
        .bind(actor.project_id)
        .bind(kind)
        .bind(target_id)
        .bind(actor.actor_id)
        .execute(&mut **transaction)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM work_item_links WHERE work_item_id = $1 AND target_kind = $2 AND target_id = $3",
        )
        .bind(current.id)
        .bind(kind)
        .bind(target_id)
        .execute(&mut **transaction)
        .await?;
    }
    let version =
        advance_version(transaction, actor.project_id, current.id, current.version).await?;
    let payload = ChangePayload {
        link: Some(LinkChange {
            kind: kind.to_owned(),
            id: target_id,
            linked: link,
        }),
        ..ChangePayload::default()
    };
    let noun = match kind {
        "conversation" => "Conversation",
        "message" => "Message",
        "page" => "Page",
        _ => "Decision",
    };
    let summary = if link {
        format!("{noun} linked through MCP")
    } else {
        format!("{noun} unlinked through MCP")
    };
    finalize_change(
        transaction,
        actor,
        current.id,
        ChangeFinalization {
            activity_kind: "work_item_updated",
            summary: &summary,
            audit_action: if link {
                "work_item.mcp_linked"
            } else {
                "work_item.mcp_unlinked"
            },
            activity_payload: payload.activity(),
            audit_metadata: payload.audit(version),
            version,
        },
    )
    .await?;
    Ok(Some(version))
}

/// A repository in the task's project, by ID, link, owner/name, a GitHub
/// URL, or a name only one of its repositories has. Returns its ID and
/// owner/name, or the answer to give.
async fn find_repository(
    state: &AppState,
    project_id: Uuid,
    reference: &str,
) -> Result<Result<(Uuid, String), ToolResult<LinkValue>>, AppError> {
    if let Some(id) = parse_id(reference) {
        return Ok(sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, owner || '/' || name FROM repositories WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ToolResult::not_found("That repository")));
    }
    let name = reference
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let found = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, owner || '/' || name FROM repositories
        WHERE project_id = $1
          AND (lower(owner || '/' || name) = lower($2) OR lower(name) = lower($2))
        ORDER BY lower(owner), lower(name), id
        LIMIT 10
        "#,
    )
    .bind(project_id)
    .bind(name)
    .fetch_all(&state.pool)
    .await?;
    match found.as_slice() {
        [only] => Ok(Ok(only.clone())),
        [] => {
            let (project_key, here) = sqlx::query_as::<_, (String, Vec<String>)>(
                r#"
                SELECT project.project_key,
                       COALESCE((SELECT array_agg(listed.full_name ORDER BY listed.sort_key)
                                 FROM (SELECT owner || '/' || name AS full_name,
                                              lower(owner || '/' || name) AS sort_key
                                       FROM repositories
                                       WHERE project_id = project.id AND archived_at IS NULL
                                       ORDER BY lower(owner || '/' || name)
                                       LIMIT 10) listed), ARRAY[]::text[])
                FROM projects project WHERE project.id = $1
                "#,
            )
            .bind(project_id)
            .fetch_one(&state.pool)
            .await?;
            let message = if here.is_empty() {
                format!(
                    "No repository called \"{}\" in {project_key}, which has no repositories connected.",
                    echo(name)
                )
            } else {
                format!(
                    "No repository called \"{}\" in {project_key}. Repositories here: {}.",
                    echo(name),
                    here.join(", ")
                )
            };
            Ok(Err(ToolResult::NotFound { message }))
        }
        several => Ok(Err(ToolResult::invalid(
            "repositoryId",
            format!(
                "Several repositories here are called \"{}\": {}. Use owner/name.",
                echo(name),
                several
                    .iter()
                    .map(|(_, full)| full.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))),
    }
}

/// A pull request Agent Workspace knows about in the task's project, by ID,
/// link, or GitHub URL, that the caller can see (one opened from a private
/// thread stays private, migration 0050).
async fn find_pull_request(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
    task_key: &str,
    reference: &str,
) -> Result<Result<Uuid, ToolResult<LinkValue>>, AppError> {
    let found = if let Some(id) = parse_id(reference) {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM github_pull_requests WHERE project_id = $1 AND id = $2 AND pull_request_visible_to(id, $3)",
        )
        .bind(project_id)
        .bind(id)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
    } else if reference.starts_with("https://github.com/") && reference.len() <= 2048 {
        let url = reference
            .split(['?', '#'])
            .next()
            .unwrap_or(reference)
            .trim_end_matches('/');
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM github_pull_requests WHERE project_id = $1 AND html_url = $2 AND pull_request_visible_to(id, $3)",
        )
        .bind(project_id)
        .bind(url)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        return Ok(Err(ToolResult::invalid(
            "pullRequestId",
            "Pass the pull request's ID, link, or GitHub URL (see pull_request_get).",
        )));
    };
    Ok(found.ok_or_else(|| ToolResult::NotFound {
        message: format!(
            "That pull request isn't in {task_key}'s project. Agent Workspace only knows pull requests from repositories connected to the project; pull_request_get looks one up."
        ),
    }))
}

/// What the project's GitHub automation will do to a task once a pull
/// request is linked, in plain words.
async fn automation_note(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    key: &str,
) -> Result<String, AppError> {
    let (on_open, on_merge) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT pull_request_open_enabled, pull_request_merge_enabled FROM project_github_automation_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or((false, false));
    let opened = automation_target(transaction, project_id, on_open, "review").await?;
    let merged = automation_target(transaction, project_id, on_merge, "verification").await?;
    Ok(match (opened, merged) {
        (Some(opened), Some(merged)) => format!(
            "With this project's GitHub automation, {key} moves to {opened} when this pull request is opened for review, and to {merged} when it's merged."
        ),
        (Some(opened), None) => format!(
            "With this project's GitHub automation, {key} moves to {opened} when this pull request is opened for review."
        ),
        (None, Some(merged)) => format!(
            "With this project's GitHub automation, {key} moves to {merged} when this pull request is merged."
        ),
        (None, None) => format!(
            "GitHub automation is off in this project, so this pull request won't move {key}."
        ),
    })
}

/// The status a GitHub automation rule moves tasks to: the one enabled
/// status in the group it targets. None when the rule is off, or when the
/// group has no status or several (the automation then skips the task).
async fn automation_target(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    enabled: bool,
    category: &str,
) -> Result<Option<String>, AppError> {
    if !enabled {
        return Ok(None);
    }
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM work_item_statuses WHERE project_id = $1 AND semantic_category = $2 AND enabled LIMIT 2",
    )
    .bind(project_id)
    .bind(category)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(match names.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    })
}
