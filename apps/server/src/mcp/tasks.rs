//! Task tools: list and open tasks, put them in order, claim them before
//! working on them, and link them to the conversations, pages and decisions
//! they came from. Creating and changing tasks reuses the canonical commands
//! in `writes.rs`.

use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, ToolResult, accessible_project_ids, archived_task_message,
        flag_or_text, mark_via, one_or_many, parse_id, parse_message_id, parse_since,
        project_is_chosen, public_url, scope_projects, snippet, string_or_number,
        version_conflict_message,
    },
    field_tools, history, planning,
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
    work_item_planning::{HAS_OPEN_BLOCKER, OPEN_BLOCKER_COUNT},
    work_items::{
        decode_work_item_timeline_cursor,
        list::{FieldFilter, Values, ViewFilters, push_field},
    },
};

const DEFAULT_CLAIM_MINUTES: i32 = 120;

// ---------------------------------------------------------------------------
// task_list

/// The most projects one task_list call looks at through projectIds.
const MAX_LIST_PROJECTS: usize = 50;
/// The most labels one task_list call filters by.
const MAX_FILTER_LABELS: usize = 10;
/// The most values any other list filter names, such as 20 statuses.
const MAX_FILTER_VALUES: usize = 20;
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
    /// Status names (such as "In Progress"), IDs, or groups: backlog,
    /// unstarted, started, review, verification, completed, canceled. Tasks
    /// in any of them, finished ones included when you name them.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub status: Option<Vec<String>>,
    /// Leave out tasks in any of these statuses or groups.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_status: Option<Vec<String>>,
    /// "me", "none", actor IDs, or @handles: tasks assigned to any of them.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub assignee: Option<Vec<String>>,
    /// Leave out tasks assigned to any of these; unassigned ones stay
    /// unless you list "none".
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_assignee: Option<Vec<String>>,
    /// Only tasks that have all of these labels, by name or ID (at most 10).
    /// Names match in each project you're looking at.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub labels: Option<Vec<String>>,
    /// Tasks with at least one of these labels; "none" lets in tasks with
    /// no labels too.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub any_labels: Option<Vec<String>>,
    /// Leave out tasks with any of these labels; "none" leaves out tasks
    /// with no labels.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_labels: Option<Vec<String>>,
    /// Type names such as Bug (any case), or type IDs: tasks of any of them.
    #[serde(rename = "type", default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub task_type: Option<Vec<String>>,
    /// Leave out tasks of any of these types.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_type: Option<Vec<String>>,
    /// no_priority (or none), low, medium, high, urgent: any of them.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub priority: Option<Vec<String>>,
    /// Leave out tasks with any of these priorities.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_priority: Option<Vec<String>>,
    /// Who created it: "me", an @handle, or an actor ID.
    pub created_by: Option<String>,
    /// Changed since: a date (YYYY-MM-DD), a timestamp such as
    /// 2026-09-30T14:00:00Z, or a span such as 24h or 7d.
    pub updated_since: Option<String>,
    /// true: only blocked tasks, whether marked blocked or waiting on an
    /// unfinished task (task_depend). false: leave them out.
    pub blocked: Option<bool>,
    /// true: only tasks ready to start (not finished, not blocked, nothing
    /// unfinished blocking them). false: the rest.
    pub ready: Option<bool>,
    /// Milestones by name or ID, or "none" for tasks in no milestone: any
    /// of them.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub milestone: Option<Vec<String>>,
    /// Leave out tasks in any of these milestones; "none" leaves out tasks
    /// in no milestone.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "Option<Vec<String>>")]
    pub not_milestone: Option<Vec<String>>,
    /// Only tasks due on or before this date (YYYY-MM-DD).
    pub due_before: Option<String>,
    /// Only tasks due on or after this date (YYYY-MM-DD).
    pub due_after: Option<String>,
    /// true: only unfinished tasks past their due date. false: the rest.
    pub overdue: Option<bool>,
    /// yes (someone is working on it), no, me, or an @handle. true and false
    /// work too.
    #[serde(default, deserialize_with = "flag_or_text")]
    #[schemars(with = "Option<String>")]
    pub claimed: Option<String>,
    /// Words in the title or description, or a key such as AW-12.
    pub query: Option<String>,
    /// Only subtasks of this task, or "none" for top-level tasks.
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
    /// A saved view by name or ID (see view_list), such as "Ready for
    /// agents". Needs projectId. Its filters apply as on the task list; a
    /// field you pass replaces the view's filter on that field.
    pub view: Option<String>,
    /// Filters on the project's custom fields (see field_list), all of
    /// which must match, such as [{"field": "Risk", "op": "is", "values":
    /// ["High"]}]. Needs projectId.
    pub field_filters: Option<Vec<field_tools::FieldFilterArgument>>,
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
    /// Marked blocked, or waiting on an unfinished task.
    pub blocked: bool,
    /// How many unfinished tasks it waits on.
    #[serde(default)]
    pub open_blocker_count: i64,
    /// Not finished, not blocked, and nothing unfinished blocking it.
    #[serde(default)]
    pub ready: bool,
    /// YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Past its due date and not finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overdue: bool,
    /// Its milestone, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
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
    /// Its custom field values by field name (only those it has), as
    /// task_open reads them.
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The planning columns of a task line: blocked (marked or by an unfinished
/// dependency), how many unfinished tasks it waits on, whether it's ready,
/// its due date and milestone.
fn task_planning_columns() -> String {
    format!(
        r#"
        (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) AS blocked,
        {OPEN_BLOCKER_COUNT} AS open_blocker_count,
        ({READY}) AS ready,
        item.due_date::text AS due_date,
        ({OVERDUE}) AS overdue,
        (SELECT milestone.name FROM project_milestones milestone
          WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id) AS milestone
        "#
    )
}

/// SQL for a task row aliased `item` with its `status`: ready to start.
const READY: &str = r#"
    item.archived_at IS NULL
    AND status.semantic_category NOT IN ('completed', 'canceled')
    AND item.blocked_reason IS NULL
    AND NOT EXISTS (SELECT 1
       FROM work_item_dependencies ready_dependency
       JOIN work_items ready_blocker
         ON ready_blocker.project_id = ready_dependency.blocking_project_id
        AND ready_blocker.id = ready_dependency.blocking_work_item_id
       JOIN work_item_statuses ready_status
         ON ready_status.project_id = ready_blocker.project_id
        AND ready_status.id = ready_blocker.status_id
      WHERE ready_dependency.project_id = item.project_id
        AND ready_dependency.blocked_work_item_id = item.id
        AND ready_blocker.archived_at IS NULL
        AND ready_status.semantic_category NOT IN ('completed', 'canceled'))
"#;

/// SQL for a task row aliased `item` with its `status`: past due, unfinished.
const OVERDUE: &str = r#"
    COALESCE(item.due_date < current_date
             AND status.semantic_category NOT IN ('completed', 'canceled'), false)
"#;

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

/// What a task_list call filters on, resolved. `push_where` writes the same
/// conditions for the count and the page, so `total` and the page agree.
#[derive(Debug, Default)]
struct ListFilters {
    projects: Vec<Uuid>,
    include_done: bool,
    include_archived: bool,
    /// The call names statuses to list: finished tasks in them count even
    /// without includeDone.
    names_statuses: bool,
    statuses: FieldFilter<Uuid>,
    assignees: FieldFilter<Uuid>,
    query: String,
    parent: Option<Parent>,
    types: FieldFilter<Uuid>,
    priorities: FieldFilter<String>,
    created_by: Option<Uuid>,
    updated_since: Option<DateTime<Utc>>,
    blocked: Option<bool>,
    claimed_mode: &'static str,
    claimed_id: Option<Uuid>,
    /// Label names, lowercased and each once: the task has all of them.
    labels: Vec<String>,
    /// At least one of these names, or no labels at all (true).
    any_labels: Option<(Vec<String>, bool)>,
    /// None of these names, nor no labels at all (true).
    not_labels: Option<(Vec<String>, bool)>,
    milestones: FieldFilter<Uuid>,
    ready: Option<bool>,
    due_before: Option<NaiveDate>,
    due_after: Option<NaiveDate>,
    overdue: Option<bool>,
    /// A saved view's own filters, applied as the task list applies them.
    view: Option<ViewFilters>,
    /// Custom field filters, as stored values (one project only).
    fields: Vec<crate::fields::FieldClause>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    /// Subtasks of this task.
    Of(Uuid),
    /// Tasks with no parent.
    TopLevel,
}

/// SQL for a task row aliased `item`: it has an active label...
const HAS_SOME_LABEL: &str = "EXISTS (SELECT 1 FROM work_item_active_labels active \
     WHERE active.project_id = item.project_id AND active.work_item_id = item.id)";
/// ...or one of the lowercased names bound next, closed with `))`.
const HAS_LABEL_NAMED: &str = "EXISTS (SELECT 1 FROM work_item_active_labels active \
     WHERE active.project_id = item.project_id AND active.work_item_id = item.id \
       AND lower(active.name) = ANY(";

impl ListFilters {
    /// Every filter, as the WHERE clause of a query over `TASK_FROM`.
    /// Labels match by name through the active-labels view, so an archived
    /// label never matches.
    fn push_where(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        builder
            .push(" WHERE item.project_id = ANY(")
            .push_bind(self.projects.clone())
            .push(")");
        if !self.include_archived {
            builder.push(" AND item.archived_at IS NULL");
        }
        if !self.include_done && !self.names_statuses {
            builder.push(" AND status.semantic_category NOT IN ('completed', 'canceled')");
        }
        push_field(builder, "item.status_id", false, &self.statuses);
        push_field(builder, "item.assignee_actor_id", true, &self.assignees);
        if !self.query.is_empty() {
            builder
                .push(" AND (item.search_vector @@ websearch_to_tsquery('english', ")
                .push_bind(self.query.clone())
                .push(") OR item.title ILIKE '%' || ")
                .push_bind(self.query.clone())
                .push(" || '%' OR upper(project.project_key || '-' || item.number::text) = upper(")
                .push_bind(self.query.clone())
                .push("))");
        }
        match self.parent {
            Some(Parent::Of(id)) => {
                builder
                    .push(" AND item.parent_work_item_id = ")
                    .push_bind(id);
            }
            Some(Parent::TopLevel) => {
                builder.push(" AND item.parent_work_item_id IS NULL");
            }
            None => {}
        }
        push_field(builder, "item.type_id", false, &self.types);
        push_field(builder, "item.priority", false, &self.priorities);
        if let Some(id) = self.created_by {
            builder
                .push(" AND item.created_by_actor_id = ")
                .push_bind(id);
        }
        if let Some(time) = self.updated_since {
            builder.push(" AND item.updated_at >= ").push_bind(time);
        }
        match self.claimed_mode {
            "none" => {
                builder.push(" AND claim.work_item_id IS NULL");
            }
            "some" => {
                builder.push(" AND claim.work_item_id IS NOT NULL");
            }
            "one" => {
                builder
                    .push(" AND claim.actor_id = ")
                    .push_bind(self.claimed_id);
            }
            _ => {}
        }
        if !self.labels.is_empty() {
            builder
                .push(
                    " AND (SELECT count(*) FROM work_item_active_labels active \
                       WHERE active.project_id = item.project_id \
                         AND active.work_item_id = item.id AND lower(active.name) = ANY(",
                )
                .push_bind(self.labels.clone())
                .push(")) = ")
                .push_bind(self.labels.len() as i64);
        }
        if let Some((names, none)) = &self.any_labels {
            builder.push(" AND (false");
            if !names.is_empty() {
                builder
                    .push(format!(" OR {HAS_LABEL_NAMED}"))
                    .push_bind(names.clone())
                    .push("))");
            }
            if *none {
                builder.push(format!(" OR NOT {HAS_SOME_LABEL}"));
            }
            builder.push(")");
        }
        if let Some((names, none)) = &self.not_labels {
            if !names.is_empty() {
                builder
                    .push(format!(" AND NOT {HAS_LABEL_NAMED}"))
                    .push_bind(names.clone())
                    .push("))");
            }
            if *none {
                builder.push(format!(" AND {HAS_SOME_LABEL}"));
            }
        }
        if let Some(blocked) = self.blocked {
            builder
                .push(format!(
                    " AND (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) = "
                ))
                .push_bind(blocked);
        }
        push_field(builder, "item.milestone_id", true, &self.milestones);
        if let Some(ready) = self.ready {
            builder.push(format!(" AND ({READY}) = ")).push_bind(ready);
        }
        if let Some(date) = self.due_before {
            builder.push(" AND item.due_date <= ").push_bind(date);
        }
        if let Some(date) = self.due_after {
            builder.push(" AND item.due_date >= ").push_bind(date);
        }
        if let Some(overdue) = self.overdue {
            builder
                .push(format!(" AND ({OVERDUE}) = "))
                .push_bind(overdue);
        }
        if let Some(view) = &self.view {
            view.push_where(builder);
        }
        crate::fields::push_clauses(builder, "item.field_values", &self.fields);
    }
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
    // Added later: left out when absent, so cursors made before still fit.
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<(&'a str, &'a [Uuid])>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_before: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overdue: Option<bool>,
    /// Several values, "leave out" filters, top-level only, dueAfter and a
    /// view's own filters.
    #[serde(skip_serializing_if = "Option::is_none")]
    more: Option<String>,
}

impl ListFingerprint<'_> {
    fn hash(&self) -> Result<String, AppError> {
        let encoded =
            serde_json::to_vec(self).map_err(|_| AppError::Internal("task_list fingerprint"))?;
        Ok(hex::encode(&Sha256::digest(&encoded)[..6]))
    }
}

/// The sort keys of the last task on a page: the next page starts after
/// it, whatever was added or finished in between, without counting past
/// every earlier task.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Serialize)]
struct ListPosition {
    project_name: String,
    project_id: Uuid,
    status_position: i32,
    archived: bool,
    board_rank: i64,
    id: Uuid,
    priority_rank: i32,
    updated_at: DateTime<Utc>,
    number: i64,
}

/// Where a page starts.
#[derive(Debug, PartialEq)]
enum ListCursor {
    Start,
    /// Cursors made before keyset pages, `{offset}.{fingerprint}` or a bare
    /// number, still read as an offset.
    Offset(i64),
    After(Box<ListPosition>),
}

/// Reads a cursor: `k{position}.{fingerprint}`, or an older offset.
fn read_cursor(cursor: Option<&str>, fingerprint: &str) -> Result<ListCursor, &'static str> {
    let Some(cursor) = cursor.map(str::trim).filter(|cursor| !cursor.is_empty()) else {
        return Ok(ListCursor::Start);
    };
    if cursor.len() > 1_000 {
        return Err(CURSOR_MESSAGE);
    }
    let (position, from) = match cursor.rsplit_once('.') {
        Some((position, from)) => (position, Some(from)),
        None => (cursor, None),
    };
    if from.is_some_and(|from| from != fingerprint) {
        return Err(
            "That cursor came from a different search. Leave out cursor to start from the first page.",
        );
    }
    if let Some(encoded) = position.strip_prefix('k') {
        if from.is_none() {
            return Err(CURSOR_MESSAGE);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| CURSOR_MESSAGE)?;
        let position =
            serde_json::from_slice::<ListPosition>(&bytes).map_err(|_| CURSOR_MESSAGE)?;
        return Ok(ListCursor::After(Box::new(position)));
    }
    if position.is_empty() || position.len() > 9 || !position.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CURSOR_MESSAGE);
    }
    position
        .parse::<i64>()
        .map(ListCursor::Offset)
        .map_err(|_| CURSOR_MESSAGE)
}

fn write_cursor(position: &ListPosition, fingerprint: &str) -> Result<String, AppError> {
    let encoded =
        serde_json::to_vec(position).map_err(|_| AppError::Internal("task_list cursor"))?;
    Ok(format!(
        "k{}.{fingerprint}",
        URL_SAFE_NO_PAD.encode(encoded)
    ))
}

/// How priority sorts: urgent first, no priority last.
const PRIORITY_RANK: &str = "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END";

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

/// The order of a task_list sort. Archived tasks (includeArchived) go to
/// the end of their column. Each order ends in the task's ID, so it's total
/// and a page can start right after the last task of the one before.
fn list_order(sort: &str) -> Option<String> {
    Some(match sort {
        "board" => "project.name, item.project_id, status.position, item.archived_at IS NOT NULL, item.board_rank, item.id".to_owned(),
        "priority" => format!(
            "{PRIORITY_RANK}, status.position, item.archived_at IS NOT NULL, item.board_rank, item.id"
        ),
        "updated" => "item.updated_at DESC, item.id".to_owned(),
        "number" => "project.name, item.project_id, item.number".to_owned(),
        _ => return None,
    })
}

/// Only tasks after `position` in the `sort` order.
fn push_after(builder: &mut QueryBuilder<'_, Postgres>, sort: &str, position: &ListPosition) {
    match sort {
        "priority" => {
            builder
                .push(format!(
                    " AND ({PRIORITY_RANK}, status.position, item.archived_at IS NOT NULL, item.board_rank, item.id) > ("
                ))
                .push_bind(position.priority_rank)
                .push(", ")
                .push_bind(position.status_position)
                .push(", ")
                .push_bind(position.archived)
                .push(", ")
                .push_bind(position.board_rank)
                .push(", ")
                .push_bind(position.id)
                .push(")");
        }
        "updated" => {
            builder
                .push(" AND (item.updated_at < ")
                .push_bind(position.updated_at)
                .push(" OR (item.updated_at = ")
                .push_bind(position.updated_at)
                .push(" AND item.id > ")
                .push_bind(position.id)
                .push("))");
        }
        "number" => {
            builder
                .push(" AND (project.name, item.project_id, item.number) > (")
                .push_bind(position.project_name.clone())
                .push(", ")
                .push_bind(position.project_id)
                .push(", ")
                .push_bind(position.number)
                .push(")");
        }
        _ => {
            builder
                .push(
                    " AND (project.name, item.project_id, status.position, \
                       item.archived_at IS NOT NULL, item.board_rank, item.id) > (",
                )
                .push_bind(position.project_name.clone())
                .push(", ")
                .push_bind(position.project_id)
                .push(", ")
                .push_bind(position.status_position)
                .push(", ")
                .push_bind(position.archived)
                .push(", ")
                .push_bind(position.board_rank)
                .push(", ")
                .push_bind(position.id)
                .push(")");
        }
    }
}

/// A list argument's values, trimmed, without blanks; none left reads as
/// not passed. More than 20 is refused with `field`.
fn listed(
    values: Option<&Vec<String>>,
    field: &'static str,
) -> Result<Vec<String>, (&'static str, String)> {
    let values = values
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.len() > MAX_FILTER_VALUES {
        return Err((field, format!("Pass at most {MAX_FILTER_VALUES}.")));
    }
    Ok(values)
}

/// Any of `values`, and with `none` no value at all; `None` when the call
/// names nothing.
fn chosen<T: Ord>(values: Vec<T>, none: bool) -> Option<Values<T>> {
    (!values.is_empty() || none).then(|| Values::new(values, none))
}

/// The one value a call names, as cursors made before lists had it.
fn single(values: &[String]) -> Option<&str> {
    match values {
        [only] => Some(only),
        _ => None,
    }
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

/// People named in a task_list filter: their IDs, and whether "none"
/// (no one) is among them. Refused with `field` for someone not found.
async fn filter_people(
    state: &AppState,
    principal: &McpPrincipal,
    projects: &[Uuid],
    values: &[String],
    field: &'static str,
) -> Result<Result<(Vec<Uuid>, bool), (&'static str, String)>, AppError> {
    let mut ids = Vec::new();
    let mut none = false;
    for person in values {
        match person.as_str() {
            "me" => ids.push(principal.actor_id),
            "none" => none = true,
            person => match resolve_actor(state, projects, person).await? {
                Some(id) => ids.push(id),
                None => return Ok(Err((field, format!("No one called {person} was found.")))),
            },
        }
    }
    Ok(Ok((ids, none)))
}

/// Priorities named in a task_list filter, as stored ("none" is
/// no_priority).
fn filter_priorities(
    values: &[String],
    field: &'static str,
) -> Result<Vec<String>, (&'static str, String)> {
    values
        .iter()
        .map(|value| match value.to_lowercase() {
            value if value == "none" => Ok("no_priority".to_owned()),
            value if validate_priority(&value).is_ok() => Ok(value),
            _ => Err((
                field,
                "Use no_priority, low, medium, high, or urgent.".to_owned(),
            )),
        })
        .collect()
}

/// Labels named in a task_list filter, "none" apart: their lowercased
/// names, and whether "none" (no labels) is among them.
async fn filter_label_choice(
    state: &AppState,
    projects: &[Uuid],
    values: &[String],
    field: &'static str,
) -> Result<Result<Option<(Vec<String>, bool)>, (&'static str, String)>, AppError> {
    if values.is_empty() {
        return Ok(Ok(None));
    }
    let none = values
        .iter()
        .any(|value| value.eq_ignore_ascii_case("none"));
    let named = values
        .iter()
        .filter(|value| !value.eq_ignore_ascii_case("none"))
        .cloned()
        .collect::<Vec<_>>();
    if named.is_empty() {
        return Ok(Ok(Some((Vec::new(), none))));
    }
    Ok(match filter_labels(state, projects, &named).await? {
        Ok(names) => Ok(Some((names, none))),
        Err(message) => Err((field, message)),
    })
}

/// Milestones named in a task_list filter, by name or ID: their IDs, and
/// whether "none" (no milestone) is among them.
async fn filter_milestone_choice(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    values: &[String],
    field: &'static str,
) -> Result<Result<(Vec<Uuid>, bool), (&'static str, String)>, AppError> {
    let mut ids = Vec::new();
    let mut none = false;
    for value in values {
        if value.eq_ignore_ascii_case("none") {
            none = true;
            continue;
        }
        match filter_milestones(state, projects, place, value).await? {
            Ok(found) => ids.extend(found),
            Err(message) => return Ok(Err((field, message))),
        }
    }
    Ok(Ok((ids, none)))
}

async fn task_list_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskListArguments,
) -> Result<ToolResult<TaskListValue>, AppError> {
    let (arguments, view) = match super::views_data::apply_view(state, principal, arguments).await?
    {
        Ok(applied) => applied,
        Err((field, message)) => return Ok(ToolResult::invalid(field, message)),
    };
    match list_tasks(state, principal, arguments, view).await? {
        Ok(value) => Ok(ToolResult::ok(value)),
        Err(refusal) => Ok(refusal),
    }
}

async fn list_tasks(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskListArguments,
    view: Option<ViewFilters>,
) -> Result<Result<TaskListValue, ToolResult<TaskListValue>>, AppError> {
    let invalid =
        |(field, message): (&'static str, String)| Err(ToolResult::invalid(field, message));
    let limit = i64::from(arguments.limit.unwrap_or(50));
    if !(1..=200).contains(&limit) {
        return Ok(Err(ToolResult::invalid(
            "limit",
            "Use a limit from 1 to 200.",
        )));
    }
    let sort = arguments.sort.as_deref().unwrap_or("board");
    let Some(order) = list_order(sort) else {
        return Ok(Err(ToolResult::invalid(
            "sort",
            "Use board, priority, updated, or number.",
        )));
    };
    let parent = match arguments.parent_id.as_deref().map(str::trim) {
        None => None,
        Some(value) if value.eq_ignore_ascii_case("none") => Some(Parent::TopLevel),
        Some(value) => match parse_id(value) {
            Some(id) => Some(Parent::Of(id)),
            None => {
                return Ok(Err(ToolResult::invalid(
                    "parentId",
                    "Pass the parent task's ID, or \"none\" for top-level tasks.",
                )));
            }
        },
    };
    if arguments.project_id.is_some() && arguments.project_ids.is_some() {
        return Ok(Err(ToolResult::invalid(
            "projectIds",
            "Pass projectId or projectIds, not both.",
        )));
    }
    // An empty milestone, as before lists, means none.
    let milestone = arguments.milestone.as_ref().map(|values| {
        values
            .iter()
            .map(|value| {
                if value.trim().is_empty() {
                    "none".to_owned()
                } else {
                    value.clone()
                }
            })
            .collect::<Vec<_>>()
    });
    let lists = (|| {
        Ok::<_, (&'static str, String)>([
            listed(arguments.status.as_ref(), "status")?,
            listed(arguments.not_status.as_ref(), "notStatus")?,
            listed(arguments.assignee.as_ref(), "assignee")?,
            listed(arguments.not_assignee.as_ref(), "notAssignee")?,
            listed(arguments.any_labels.as_ref(), "anyLabels")?,
            listed(arguments.not_labels.as_ref(), "notLabels")?,
            listed(arguments.task_type.as_ref(), "type")?,
            listed(arguments.not_type.as_ref(), "notType")?,
            listed(arguments.priority.as_ref(), "priority")?,
            listed(arguments.not_priority.as_ref(), "notPriority")?,
            listed(milestone.as_ref(), "milestone")?,
            listed(arguments.not_milestone.as_ref(), "notMilestone")?,
        ])
    })();
    let [
        status,
        not_status,
        assignee,
        not_assignee,
        any_labels,
        not_labels,
        task_type,
        not_type,
        priority,
        not_priority,
        milestone,
        not_milestone,
    ] = match lists {
        Ok(lists) => lists,
        Err(refusal) => return Ok(invalid(refusal)),
    };
    let (priorities, not_priorities) = match (
        filter_priorities(&priority, "priority"),
        filter_priorities(&not_priority, "notPriority"),
    ) {
        (Ok(any), Ok(not)) => (any, not),
        (Err(refusal), _) | (_, Err(refusal)) => return Ok(invalid(refusal)),
    };
    let updated_since = match arguments.updated_since.as_deref() {
        None => None,
        Some(value) => match parse_since(value) {
            Some(time) => Some(time),
            None => {
                return Ok(Err(ToolResult::invalid(
                    "updatedSince",
                    UPDATED_SINCE_MESSAGE,
                )));
            }
        },
    };
    let date = |value: Option<&str>, field: &'static str| match value {
        None => Ok(None),
        Some(value) => NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
            .map(Some)
            .map_err(|_| (field, "Use a date: YYYY-MM-DD.".to_owned())),
    };
    let (due_before, due_after) = match (
        date(arguments.due_before.as_deref(), "dueBefore"),
        date(arguments.due_after.as_deref(), "dueAfter"),
    ) {
        (Ok(before), Ok(after)) => (before, after),
        (Err(refusal), _) | (_, Err(refusal)) => return Ok(invalid(refusal)),
    };

    let projects = match &arguments.project_ids {
        Some(listed) => {
            if listed.is_empty() || listed.len() > MAX_LIST_PROJECTS {
                return Ok(Err(ToolResult::invalid(
                    "projectIds",
                    "Pass 1 to 50 projects.",
                )));
            }
            let reachable = accessible_project_ids(state, principal).await?;
            let mut projects = Vec::with_capacity(listed.len());
            for raw in listed {
                // The middleware turned keys the caller can reach into IDs; a
                // key left as written, or an ID out of reach, is named back.
                match parse_id(raw).filter(|id| reachable.contains(id)) {
                    Some(id) => projects.push(id),
                    None => {
                        return Ok(Err(ToolResult::not_found(&format!(
                            "Project {}",
                            snippet(raw, 60)
                        ))));
                    }
                }
            }
            projects
        }
        None => scope_projects(state, principal).await?,
    };
    // The tool gate takes "see tasks" in any project; list only those where
    // the caller's role (and an app's limit) keeps it.
    let projects =
        crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &projects).await?;
    // The tool gate takes Tasks being on anywhere the caller is (AWD-75); list
    // only projects where it's on for them, their role narrowing it, before
    // anything is counted or paged.
    let modules = {
        let mut connection = state.pool.acquire().await?;
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &projects)
            .await?
    };
    let mut projects = modules.with(&projects, "tasks");
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

    let (assignees, not_assignees) = match (
        filter_people(state, principal, &projects, &assignee, "assignee").await?,
        filter_people(state, principal, &projects, &not_assignee, "notAssignee").await?,
    ) {
        (Ok(any), Ok(not)) => (any, not),
        (Err(refusal), _) | (_, Err(refusal)) => return Ok(invalid(refusal)),
    };
    let mut statuses = Vec::new();
    for (values, field) in [(&status, "status"), (&not_status, "notStatus")] {
        match filter_statuses(state, &projects, &place, values).await? {
            Ok(ids) => statuses.push(ids),
            Err(message) => return Ok(Err(ToolResult::invalid(field, message))),
        }
    }
    let mut types = Vec::new();
    for (values, field) in [(&task_type, "type"), (&not_type, "notType")] {
        match filter_types(state, &projects, &place, values).await? {
            Ok(ids) => types.push(ids),
            Err(message) => return Ok(Err(ToolResult::invalid(field, message))),
        }
    }
    let labels = match &arguments.labels {
        None => Vec::new(),
        Some(labels) => match filter_labels(state, &projects, labels).await? {
            Ok(names) => names,
            Err(message) => return Ok(Err(ToolResult::invalid("labels", message))),
        },
    };
    let (any_labels, not_labels) = match (
        filter_label_choice(state, &projects, &any_labels, "anyLabels").await?,
        filter_label_choice(state, &projects, &not_labels, "notLabels").await?,
    ) {
        (Ok(any), Ok(not)) => (any, not),
        (Err(refusal), _) | (_, Err(refusal)) => return Ok(invalid(refusal)),
    };
    let created_by = match arguments.created_by.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("me") => Some(principal.actor_id),
        Some(person) => match resolve_actor(state, &projects, person).await? {
            Some(id) => Some(id),
            None => {
                return Ok(Err(ToolResult::invalid(
                    "createdBy",
                    format!("No one called {person} was found."),
                )));
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
                    return Ok(Err(ToolResult::invalid(
                        "claimed",
                        format!("No one called {word} was found."),
                    )));
                }
                None => {
                    return Ok(Err(ToolResult::invalid(
                        "claimed",
                        "Use yes, no, me, or an @handle.",
                    )));
                }
            },
        },
    };
    let (milestones, not_milestones) = match (
        filter_milestone_choice(state, &projects, &place, &milestone, "milestone").await?,
        filter_milestone_choice(state, &projects, &place, &not_milestone, "notMilestone").await?,
    ) {
        (Ok(any), Ok(not)) => (any, not),
        (Err(refusal), _) | (_, Err(refusal)) => return Ok(invalid(refusal)),
    };
    let field_clauses = match arguments.field_filters.as_deref() {
        None | Some([]) => Vec::new(),
        Some(filters) => {
            let [project] = projects.as_slice() else {
                return Ok(Err(ToolResult::invalid(
                    "fieldFilters",
                    "Fields belong to one project: pass projectId with fieldFilters.",
                )));
            };
            let mut connection = state.pool.acquire().await?;
            match field_tools::filter_clauses(&mut connection, principal, *project, &place, filters)
                .await?
            {
                Ok(clauses) => clauses,
                Err(message) => return Ok(Err(ToolResult::invalid("fieldFilters", message))),
            }
        }
    };

    // Cursors made before lists keep fitting: one value sits where it
    // always did, and anything newer goes in `more`.
    let one_person = |(ids, none): &(Vec<Uuid>, bool)| match (ids.as_slice(), *none) {
        ([], false) => Some(("any", None)),
        ([], true) => Some(("none", None)),
        ([id], false) => Some(("one", Some(*id))),
        _ => None,
    };
    let old_milestone = match (milestone.as_slice(), &milestones) {
        ([], _) => Some(None),
        ([_], (ids, false)) => Some(Some(("one", ids.as_slice()))),
        ([_], (_, true)) => Some(Some(("none", &[][..]))),
        _ => None,
    };
    let old_status = single(&status);
    let old_type = single(&task_type);
    let old_priority = single(&priorities);
    let old_assignee = one_person(&assignees);
    let expressible = (status.len() <= 1)
        && (task_type.len() <= 1)
        && (priorities.len() <= 1)
        && old_assignee.is_some()
        && old_milestone.is_some()
        && not_status.is_empty()
        && not_assignee.is_empty()
        && any_labels.is_none()
        && not_labels.is_none()
        && not_type.is_empty()
        && not_priorities.is_empty()
        && not_milestone.is_empty()
        && parent != Some(Parent::TopLevel)
        && due_after.is_none()
        && view.is_none()
        && field_clauses.is_empty();
    let more = (!expressible).then(|| {
        format!(
            "{:?}",
            (
                (&status, &not_status, &assignees, &not_assignees),
                (&any_labels, &not_labels, &task_type, &not_type),
                (&priorities, &not_priorities, &milestones, &not_milestones),
                (
                    parent,
                    due_after,
                    view.as_ref().map(ViewFilters::fingerprint),
                    crate::fields::clauses_param(&field_clauses),
                ),
            )
        )
    });
    let query = arguments
        .query
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let include_done = arguments.include_done.unwrap_or(false);
    let include_archived = arguments.include_archived.unwrap_or(false);
    let fingerprint = ListFingerprint {
        projects: &projects,
        status: old_status,
        assignee: old_assignee.unwrap_or(("any", None)),
        query: &query,
        parent: match parent {
            Some(Parent::Of(id)) => Some(id),
            _ => None,
        },
        sort,
        include_done,
        include_archived,
        labels: &labels,
        task_type: old_type,
        priority: old_priority,
        created_by,
        updated_since: arguments.updated_since.as_deref().map(str::trim),
        blocked: arguments.blocked,
        claimed: (claimed_mode, claimed_id),
        milestone: old_milestone.flatten(),
        ready: arguments.ready,
        due_before,
        overdue: arguments.overdue,
        more,
    }
    .hash()?;
    let cursor = match read_cursor(arguments.cursor.as_deref(), &fingerprint) {
        Ok(cursor) => cursor,
        Err(message) => return Ok(Err(ToolResult::invalid("cursor", message))),
    };
    let offset = match &cursor {
        ListCursor::Offset(offset) => *offset,
        _ => 0,
    };
    let position = match cursor {
        ListCursor::After(position) => Some(*position),
        _ => None,
    };

    let [any_statuses, not_statuses] = <[Vec<Uuid>; 2]>::try_from(statuses)
        .map_err(|_| AppError::Internal("task_list statuses"))?;
    let [any_types, not_types] =
        <[Vec<Uuid>; 2]>::try_from(types).map_err(|_| AppError::Internal("task_list types"))?;
    let filters = ListFilters {
        projects,
        include_done,
        include_archived,
        names_statuses: !status.is_empty(),
        statuses: FieldFilter::new(
            (!status.is_empty()).then(|| Values::new(any_statuses, false)),
            (!not_status.is_empty()).then(|| Values::new(not_statuses, false)),
        ),
        assignees: FieldFilter::new(
            chosen(assignees.0, assignees.1),
            chosen(not_assignees.0, not_assignees.1),
        ),
        query,
        parent,
        types: FieldFilter::new(
            (!task_type.is_empty()).then(|| Values::new(any_types, false)),
            (!not_type.is_empty()).then(|| Values::new(not_types, false)),
        ),
        priorities: FieldFilter::new(chosen(priorities, false), chosen(not_priorities, false)),
        created_by,
        updated_since,
        blocked: arguments.blocked,
        claimed_mode,
        claimed_id,
        labels,
        any_labels,
        not_labels,
        milestones: FieldFilter::new(
            chosen(milestones.0, milestones.1),
            chosen(not_milestones.0, not_milestones.1),
        ),
        ready: arguments.ready,
        due_before,
        due_after,
        overdue: arguments.overdue,
        view,
        fields: field_clauses,
    };

    // One snapshot for both queries, so the total and the page agree.
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let mut count = QueryBuilder::<Postgres>::new(format!("SELECT count(*) {TASK_FROM}"));
    filters.push_where(&mut count);
    let total = count
        .build_query_scalar::<i64>()
        .fetch_one(&mut *transaction)
        .await?;
    // The page is chosen first, from the sort keys alone; only its tasks
    // then get their board position, labels and planning columns. A board
    // position counts the open tasks above it in its column, through the
    // (project, status, rank) index, never by ranking a whole project.
    let planning_columns = task_planning_columns();
    let mut page = QueryBuilder::<Postgres>::new(format!(
        r#"
        SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
               item.title, status.name AS status, status.semantic_category AS status_group,
               item_type.name AS task_type, item.priority,
               assignee.display_name AS assignee,
               CASE WHEN item.archived_at IS NOT NULL THEN 0::bigint
                    ELSE 1 + (SELECT count(*) FROM work_items peer
                              WHERE peer.project_id = item.project_id
                                AND peer.status_id = item.status_id
                                AND peer.archived_at IS NULL
                                AND (peer.board_rank, peer.id) < (item.board_rank, item.id))
               END AS board_position,
               claimer.display_name AS claimed_by, project.name AS project,
               item.version, item.updated_at::text AS updated_at,
               item.archived_at IS NOT NULL AS archived,
               {TASK_LABELS_COLUMN},
               {planning_columns}
        {TASK_FROM}
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
        LEFT JOIN project_memberships claimer
          ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
        WHERE item.id IN (
            SELECT item.id
            {TASK_FROM}"#
    ));
    filters.push_where(&mut page);
    if let Some(position) = &position {
        push_after(&mut page, sort, position);
    }
    page.push(format!(" ORDER BY {order} LIMIT "))
        .push_bind(limit + 1)
        .push(" OFFSET ")
        .push_bind(offset)
        .push(format!(") ORDER BY {order}"));
    let mut tasks = page
        .build_query_as::<TaskSummary>()
        .fetch_all(&mut *transaction)
        .await?;
    let has_more = i64::try_from(tasks.len()).unwrap_or(i64::MAX) > limit;
    tasks.truncate(usize::try_from(limit).unwrap_or(200));
    let next_cursor = match tasks.last() {
        Some(last) if has_more => {
            let position = sqlx::query_as::<_, ListPosition>(&format!(
                r#"
                SELECT project.name AS project_name, item.project_id,
                       status.position AS status_position,
                       item.archived_at IS NOT NULL AS archived, item.board_rank, item.id,
                       ({PRIORITY_RANK})::int4 AS priority_rank, item.updated_at, item.number
                FROM work_items item
                JOIN projects project ON project.id = item.project_id
                JOIN work_item_statuses status
                  ON status.project_id = item.project_id AND status.id = item.status_id
                WHERE item.id = $1
                "#
            ))
            .bind(
                last.id
                    .parse::<Uuid>()
                    .map_err(|_| AppError::Internal("task_list row ID"))?,
            )
            .fetch_one(&mut *transaction)
            .await?;
            Some(write_cursor(&position, &fingerprint)?)
        }
        _ => None,
    };
    field_tools::fill_summary_fields(&mut transaction, &mut tasks).await?;
    transaction.commit().await?;
    Ok(Ok(TaskListValue {
        tasks,
        total,
        next_cursor,
    }))
}

/// The milestones a task_list call names, by ID or name (any case, archived
/// ones too) in the projects; otherwise the message listing the active ones.
async fn filter_milestones(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    value: &str,
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM project_milestones
        WHERE project_id = ANY($1) AND (id::text = lower($2) OR lower(name) = lower($2))
        LIMIT 100
        "#,
    )
    .bind(projects)
    .bind(value)
    .fetch_all(&state.pool)
    .await?;
    if !ids.is_empty() {
        return Ok(Ok(ids));
    }
    let names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM project_milestones
        WHERE project_id = ANY($1) AND archived_at IS NULL
        GROUP BY name ORDER BY min(target_date) NULLS LAST, name
        LIMIT 40
        "#,
    )
    .bind(projects)
    .fetch_all(&state.pool)
    .await?;
    Ok(Err(if names.is_empty() {
        format!(
            "{place} has no milestone called \"{}\", and no milestones yet. Create one with milestone_write.",
            echo(value)
        )
    } else {
        format!(
            "No milestone called \"{}\" in {place}. Milestones: {}. Use \"none\" for tasks in none.",
            echo(value),
            names.join(", ")
        )
    }))
}

/// The statuses `values` name in the projects, each by name or ID (retired
/// ones too, since tasks can sit in one) or a status group. For one that
/// names nothing, the message listing the enabled statuses.
async fn filter_statuses(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    values: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    if values.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let rows = sqlx::query_as::<_, (String, Option<Uuid>)>(
        r#"
        SELECT requested.value, status.id
        FROM unnest($2::text[]) WITH ORDINALITY AS requested(value, ordinality)
        LEFT JOIN work_item_statuses status
          ON status.project_id = ANY($1)
         AND (lower(status.name) = lower(requested.value)
              OR status.id::text = lower(requested.value)
              OR status.semantic_category = lower(requested.value))
        ORDER BY requested.ordinality
        "#,
    )
    .bind(projects)
    .bind(values)
    .fetch_all(&state.pool)
    .await?;
    // A group is known even where no status is in it yet.
    let unknown = rows.iter().find(|(value, id)| {
        id.is_none() && !STATUS_GROUPS.contains(&value.to_lowercase().as_str())
    });
    if let Some((value, _)) = unknown {
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
        return Ok(Err(format!(
            "No status called \"{}\" in {place}. Statuses: {}. Groups: {}.",
            echo(value),
            names.join(", "),
            STATUS_GROUPS.join(", ")
        )));
    }
    Ok(Ok(rows.into_iter().filter_map(|(_, id)| id).collect()))
}

/// The task types `values` name in the projects, by name or ID. For one
/// that names nothing, the message listing the enabled types.
async fn filter_types(
    state: &AppState,
    projects: &[Uuid],
    place: &str,
    values: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    if values.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let rows = sqlx::query_as::<_, (String, Option<Uuid>)>(
        r#"
        SELECT requested.value, item_type.id
        FROM unnest($2::text[]) WITH ORDINALITY AS requested(value, ordinality)
        LEFT JOIN work_item_types item_type
          ON item_type.project_id = ANY($1)
         AND (lower(item_type.name) = lower(requested.value)
              OR item_type.id::text = lower(requested.value))
        ORDER BY requested.ordinality
        "#,
    )
    .bind(projects)
    .bind(values)
    .fetch_all(&state.pool)
    .await?;
    if let Some((value, _)) = rows.iter().find(|(_, id)| id.is_none()) {
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
        return Ok(Err(format!(
            "No task type called \"{}\" in {place}. Types: {}.",
            echo(value),
            names.join(", ")
        )));
    }
    Ok(Ok(rows.into_iter().filter_map(|(_, id)| id).collect()))
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
    let projects = task_project_ids(state, principal).await?;
    resolve_among(state, principal, &projects, identifier).await
}

/// `resolve_task` for several references, in order, reading the caller's
/// projects once.
pub(super) async fn resolve_tasks(
    state: &AppState,
    principal: &McpPrincipal,
    references: &[String],
) -> Result<Vec<Option<(Uuid, Uuid)>>, AppError> {
    let projects = task_project_ids(state, principal).await?;
    let mut found = Vec::with_capacity(references.len());
    for reference in references {
        found.push(resolve_among(state, principal, &projects, reference).await?);
    }
    Ok(found)
}

/// The caller's projects where their role (and an app's limit) lets them
/// see tasks: a task elsewhere doesn't resolve, for any task tool.
async fn task_project_ids(
    state: &AppState,
    principal: &McpPrincipal,
) -> Result<Vec<Uuid>, AppError> {
    let projects = accessible_project_ids(state, principal).await?;
    crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &projects).await
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
    /// Notes left by whoever last let go of this task (task_release with
    /// handoff), newest first: where the work got to and what's left.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<HandoffNote>,
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
    /// Files attached to the task itself, oldest first; file_read opens one
    /// and file_upload (taskId) adds one. Comments list their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<super::file_tools::FileSummary>,
    /// Where to read how this project works: project_open or page_get.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide_page_id: Option<String>,
    /// With include: ["history"], what happened to the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<history::TaskHistory>,
    /// The task it's a subtask of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<planning::TaskRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<MilestoneRef>,
    /// YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    /// Past its due date and not finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overdue: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<f64>,
    /// What estimates count in this project: points or hours.
    #[serde(default)]
    pub estimate_unit: String,
    /// Why it's marked blocked (task_mark_blocked), if it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Tasks that must finish first, each with its status. Tasks in projects
    /// you can't reach are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<planning::TaskRef>,
    /// Tasks waiting on this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<planning::TaskRef>,
    /// Not finished, not marked blocked, and nothing unfinished blocking it
    /// (counting blockers you can't see).
    #[serde(default)]
    pub ready: bool,
    /// Related and duplicate tasks (task_relate).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<planning::RelationRef>,
    /// Who hears about its changes (task_watch).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watchers: Vec<planning::WatcherRef>,
    /// The project's custom fields with this task's values (null when it
    /// has none). Set them with task_update's fields, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<field_tools::TaskFieldValue>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MilestoneRef {
    pub id: String,
    pub name: String,
    /// YYYY-MM-DD, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<String>,
}

/// A task's planning fields, as task_open reads them.
#[derive(Debug, FromRow)]
struct PlanningRow {
    milestone_id: Option<Uuid>,
    milestone_name: Option<String>,
    milestone_target_date: Option<NaiveDate>,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    overdue: bool,
    estimate: Option<f64>,
    blocked_reason: Option<String>,
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
    pub id: String,
    pub author: String,
    pub text: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Files posted with the comment: read them with file_read.
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<super::file_tools::FileSummary>,
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
        let planning_columns = task_planning_columns();
        let subtasks = sqlx::query_as::<_, TaskSummary>(&format!(
            r#"
            SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
                   item.title, status.name AS status, status.semantic_category AS status_group,
                   item_type.name AS task_type, item.priority,
                   assignee.display_name AS assignee, 0::bigint AS board_position,
                   NULL::text AS claimed_by,
                   project.name AS project, item.version, item.updated_at::text AS updated_at,
                   item.archived_at IS NOT NULL AS archived, {TASK_LABELS_COLUMN},
                   {planning_columns}
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
            SELECT comment.id::text AS id,
                   COALESCE(author.display_name, 'Former member') AS author,
                   comment.plain_text AS text, comment.created_at::text AS at,
                   via_client.client_name AS via
            FROM work_item_comments comment
            LEFT JOIN project_memberships author
              ON author.project_id = comment.project_id AND author.actor_id = comment.author_actor_id
            LEFT JOIN mcp_oauth_grants via_grant
              ON via_grant.id = comment.via_oauth_grant_id AND via_grant.show_via
            LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
            WHERE comment.work_item_id = $1 AND comment.removed_at IS NULL
            ORDER BY comment.created_at DESC
            LIMIT 10
            "#,
        )
        .bind(task_id)
        .fetch_all(&state.pool)
        .await?;
        comments.reverse();
        // Files on the task and on its comments (file_read opens them).
        let mut files = Vec::new();
        for row in crate::files::task_files(
            &state.pool,
            super::file_tools::reader(principal),
            project_id,
            task_id,
        )
        .await?
        {
            let summary = super::file_tools::FileSummary::from(&row);
            match row.comment_id.map(|id| id.to_string()) {
                Some(comment_id) => {
                    if let Some(comment) = comments.iter_mut().find(|comment| comment.id == comment_id) {
                        comment.files.push(summary);
                    }
                }
                None => files.push(summary),
            }
        }
        let guide_page_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM project_pages WHERE project_id = $1 AND is_guide AND archived_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?
        .map(|id| id.to_string());
        let mut connection = state.pool.acquire().await?;
        let planning_row = sqlx::query_as::<_, PlanningRow>(&format!(
            r#"
            SELECT item.milestone_id, milestone.name AS milestone_name,
                   milestone.target_date AS milestone_target_date,
                   item.start_date, item.due_date, {OVERDUE} AS overdue,
                   item.estimate, item.blocked_reason
            FROM work_items item
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            LEFT JOIN project_milestones milestone
              ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
            WHERE item.project_id = $1 AND item.id = $2
            "#
        ))
        .bind(project_id)
        .bind(task_id)
        .fetch_one(&mut *connection)
        .await?;
        let (_, estimate_unit) =
            crate::work_item_workflow::task_settings(&mut connection, project_id).await?;
        let parent = match task.parent_work_item_id.as_deref().and_then(parse_id) {
            Some(parent) => planning::own_task_ref(&mut connection, project_id, parent).await?,
            None => None,
        };
        let planning_links =
            planning::visible_links(&mut connection, principal, project_id, task_id).await?;
        let watchers = planning::visible_watchers(&mut connection, project_id, task_id).await?;
        let fields = field_tools::task_fields(&mut connection, project_id, task_id).await?;
        let mut subtasks = subtasks;
        field_tools::fill_summary_fields(&mut connection, &mut subtasks).await?;
        drop(connection);
        let handoffs = handoff_notes(state, task_id, 3).await?;
        Ok::<_, AppError>(ToolResult::ok(TaskDetail {
            fields,
            handoffs,
            parent,
            milestone: planning_row.milestone_id.map(|id| MilestoneRef {
                id: id.to_string(),
                name: planning_row.milestone_name.clone().unwrap_or_default(),
                target_date: planning_row.milestone_target_date.map(|date| date.to_string()),
            }),
            start_date: planning_row.start_date.map(|date| date.to_string()),
            due_date: planning_row.due_date.map(|date| date.to_string()),
            overdue: planning_row.overdue,
            estimate: planning_row.estimate,
            estimate_unit,
            blocked_reason: planning_row.blocked_reason,
            blocked_by: planning_links.blocked_by,
            blocks: planning_links.blocks,
            ready: planning_links.ready,
            relations: planning_links.relations,
            watchers,
            task,
            claimed_by,
            subtasks,
            conversations,
            links,
            comments,
            files,
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
    /// An active milestone of the project, by name or ID (see
    /// milestone_list).
    pub milestone: Option<String>,
    /// When work starts: YYYY-MM-DD.
    pub start_date: Option<String>,
    /// When it's due: YYYY-MM-DD, on or after startDate.
    pub due_date: Option<String>,
    /// How big it is, in the project's unit (points or hours; see
    /// workflow_get), from 0 to 100000.
    pub estimate: Option<f64>,
    /// Custom field values by field name (see field_list), such as
    /// {"Risk": "High", "Owner": "@ana", "Shipped": true}.
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
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
        milestone: arguments.milestone.clone(),
        start_date: arguments.start_date.clone(),
        due_date: arguments.due_date.clone(),
        estimate: arguments.estimate,
        fields: arguments.fields.clone().filter(|fields| !fields.is_empty()),
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
    /// Put it in this active milestone, by name or ID (see milestone_list);
    /// "none" takes it out.
    pub milestone: Option<String>,
    /// When work starts: YYYY-MM-DD, or "" to clear it.
    pub start_date: Option<String>,
    /// When it's due: YYYY-MM-DD, or "" to clear it.
    pub due_date: Option<String>,
    /// How big it is, in the project's unit (points or hours), from 0 to
    /// 100000.
    pub estimate: Option<f64>,
    /// true removes the estimate.
    pub clear_estimate: Option<bool>,
    /// Custom field values to set, by field name (see field_list); null
    /// clears one. Such as {"Risk": "High", "Due to customer": "2026-12-01"}.
    pub fields: Option<serde_json::Map<String, serde_json::Value>>,
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
            || self.milestone.is_some()
            || self.start_date.is_some()
            || self.due_date.is_some()
            || self.estimate.is_some()
            || self.clear_estimate == Some(true)
            || self
                .fields
                .as_ref()
                .is_some_and(|fields| !fields.is_empty())
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
        milestone: arguments.milestone,
        start_date: arguments.start_date,
        due_date: arguments.due_date,
        estimate: arguments.estimate,
        clear_estimate: arguments.clear_estimate.filter(|clear| *clear),
        fields: arguments.fields.filter(|fields| !fields.is_empty()),
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
    // AWD-75: the tasks were named by ID, so the gate couldn't see their
    // project; Tasks must be on there.
    if !tasks_on(&mut transaction, project_id, principal.actor_id).await? {
        return Ok(ToolResult::module_off("task_reorder", "Tasks"));
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
    // Every rank is written before any event records its neighbours, so each
    // card's placement reflects the finished order.
    for (id, task) in ids.iter().zip(&tasks) {
        enqueue_update(&mut transaction, project_id, *id, task.version).await?;
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

/// AWD-75: whether Tasks is on for the actor in a project (their role there
/// included), for a tool whose project the module gate couldn't see.
pub(super) async fn tasks_on(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        crate::modules::state::for_project(connection, project_id, actor_id)
            .await?
            .unwrap_or_default()
            .contains("tasks"),
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
    /// On a claim you just took: the last handoff note left on the task.
    /// Read it before you start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffNote>,
}

#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct HandoffNote {
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub note: String,
    pub at: String,
}

/// The latest handoff notes on a task, newest first.
async fn handoff_notes(
    state: &AppState,
    task_id: Uuid,
    limit: i64,
) -> Result<Vec<HandoffNote>, AppError> {
    Ok(sqlx::query_as::<_, HandoffNote>(
        r#"
        SELECT COALESCE(member.display_name, 'Former member') AS by,
               via_client.client_name AS via, handoff.note, handoff.created_at::text AS at
        FROM work_item_handoffs handoff
        LEFT JOIN project_memberships member
          ON member.project_id = handoff.project_id AND member.actor_id = handoff.actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = handoff.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE handoff.work_item_id = $1
        ORDER BY handoff.created_at DESC, handoff.id DESC
        LIMIT $2
        "#,
    )
    .bind(task_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?)
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
        handoff: None,
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
        let mut claim = current_claim(state, task_id)
            .await?
            .ok_or(AppError::Internal("claim just written"))?;
        claim.handoff = handoff_notes(state, task_id, 1).await?.into_iter().next();
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
    /// For whoever picks the task up next: where the work got to, what's
    /// left, and anything that would trip them up (a branch, a failing test,
    /// a question you're waiting on). Up to 4,000 characters. The next
    /// task_claim returns it, and task_open shows it.
    pub handoff: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReleaseValue {
    pub task_id: String,
    pub released: bool,
    /// Whether your handoff note was saved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handoff_saved: bool,
}

const MAX_HANDOFF_CHARS: usize = 4_000;

pub(super) async fn task_release(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReleaseArguments,
) -> ToolResult<ReleaseValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let handoff = arguments
        .handoff
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if handoff.is_some_and(|note| note.chars().count() > MAX_HANDOFF_CHARS || note.contains('\0')) {
        return ToolResult::invalid("handoff", "Keep the handoff note to 4,000 characters.");
    }
    let result = async {
        let Some((task_id, project_id)) =
            resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        if handoff.is_some()
            && !can_write_in(state, principal, project_id, Action::WorkItemEdit).await?
        {
            return Ok(ToolResult::denied(
                "You can't work on tasks in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        let released =
            sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1 AND actor_id = $2")
                .bind(task_id)
                .bind(principal.actor_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected()
                == 1;
        // Only whoever held the task hands it off: a note from anyone else
        // would read to the next claimer as the last holder's word, and a
        // retried release would save it twice.
        if handoff.is_some() && !released {
            transaction.rollback().await?;
            return Ok(ToolResult::conflict(
                "You don't hold this task's claim, so there's nothing to hand off. Comment on the task instead.",
            ));
        }
        if let Some(note) = handoff {
            sqlx::query(
                r#"
                INSERT INTO work_item_handoffs (
                    id, project_id, work_item_id, actor_id, via_oauth_grant_id, note
                ) VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(project_id)
            .bind(task_id)
            .bind(principal.actor_id)
            .bind(principal.via_grant_id())
            .bind(note)
            .execute(&mut *transaction)
            .await?;
            insert_activity(
                &mut transaction,
                project_id,
                task_id,
                principal.actor_id,
                "work_item_updated",
                &format!("Handed off: {}", snippet(note, 200)),
                &serde_json::json!({ "handoff": true }),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(ReleaseValue {
            task_id: task_id.to_string(),
            released,
            handoff_saved: handoff.is_some(),
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
    /// A whiteboard: it shows on the task. One board can be on many tasks.
    pub whiteboard_id: Option<String>,
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
    /// conversation, message, page, decision, repository, pull_request, or
    /// whiteboard.
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
    if let Some(whiteboard) = arguments.whiteboard_id.as_deref() {
        return task_whiteboard_link(state, principal, &arguments, whiteboard).await;
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
            "Pass exactly one of conversationId, messageId, pageId, decisionId, repositoryId, pullRequestId, or whiteboardId.",
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

/// task_link with whiteboardId: the link lives on the board (see
/// `whiteboards::set_link`), so the task's version doesn't change.
async fn task_whiteboard_link(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: &TaskLinkArguments,
    whiteboard: &str,
) -> ToolResult<LinkValue> {
    let others = [
        &arguments.conversation_id,
        &arguments.message_id,
        &arguments.page_id,
        &arguments.decision_id,
        &arguments.repository_id,
        &arguments.pull_request_id,
    ];
    if others.iter().any(|other| other.is_some()) || arguments.role.is_some() {
        return ToolResult::invalid("whiteboardId", "Pass whiteboardId on its own.");
    }
    let Some(whiteboard_id) = parse_id(whiteboard.trim()) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let result = async {
        let Some((task_id, project_id)) =
            resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        // AWD-75: the gate checked Tasks for the task; the link also changes
        // the board, so Whiteboards must be on where the board is, as for
        // whiteboard_link. A board the caller can't reach is left to set_link.
        if super::whiteboard_tools::board_module_off(
            state,
            &super::whiteboard_tools::caller(principal),
            whiteboard_id,
        )
        .await?
        {
            return Ok(ToolResult::module_off("task_link", "Whiteboards"));
        }
        let linked = !arguments.remove.unwrap_or(false);
        let outcome = super::whiteboard_tools::set_link(
            state,
            principal,
            whiteboard_id,
            crate::whiteboards::TargetKind::WorkItem,
            task_id,
            linked,
        )
        .await?;
        let version = sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
            .bind(task_id)
            .fetch_one(&state.pool)
            .await?;
        Ok::<_, AppError>(ToolResult::ok(LinkValue {
            task_id: task_id.to_string(),
            key: task_key_by_id(state, task_id).await?,
            kind: "whiteboard".to_owned(),
            target_id: whiteboard_id.to_string(),
            linked,
            changed: outcome.changed,
            version,
            role: None,
            notes: Vec::new(),
            task_url: public_url(
                state,
                &format!("/projects/{project_id}/work-items/{task_id}"),
            ),
        }))
    }
    .await;
    result.unwrap_or_else(|error| match error {
        AppError::NotFound("whiteboard") => ToolResult::not_found("That whiteboard"),
        error => ToolResult::from_error(error, "task_link"),
    })
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
