//! Task lists, group counts and board columns at project scale.
//!
//! A page is read in two steps: the first query finds the page's task IDs in
//! order (filters, keyset and sort only, so the planner can use the indexes),
//! the second reads the cards for just those IDs. Cursors are keysets over
//! the full sort, group included, so every sort and grouping pages without
//! OFFSET and without ranking the whole project. A cursor remembers a
//! fingerprint of the filters it was made for and is refused under others.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{Json, extract::State};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{
    AssigneeView, WORK_ITEM_LABELS_COLUMN, WorkItemCapabilities, WorkItemLabelView,
    WorkItemStatusView, WorkItemTypeView, WorkflowStatusRow, authorize_work_item, capabilities,
    normalize_search, project_membership, validate_list_filter_ids, validate_priority,
    validate_priority_filter, validate_semantic_category,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    policy::Action,
    work_item_planning::{HAS_OPEN_BLOCKER, OPEN_BLOCKER_COUNT},
};

const DEFAULT_LIMIT: u16 = 50;
const MAX_LIMIT: u16 = 200;
const MAX_LABEL_FILTER: usize = 20;
const MAX_CURSOR_LENGTH: usize = 1_024;

/// Sort rank of a priority: urgent first, no priority last.
const PRIORITY_RANK: &str = "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 \
     WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END";
const PRIORITY_LABEL: &str = "CASE item.priority WHEN 'urgent' THEN 'Urgent' WHEN 'high' THEN 'High' \
     WHEN 'medium' THEN 'Medium' WHEN 'low' THEN 'Low' ELSE 'No priority' END";
const FINISHED: &str = "status.semantic_category IN ('completed', 'canceled')";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskListQuery {
    q: Option<String>,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    semantic_category: Option<String>,
    priority: Option<String>,
    assignee_actor_id: Option<Uuid>,
    unassigned: Option<bool>,
    archived: Option<bool>,
    label_ids: Option<String>,
    milestone_id: Option<String>,
    parent_id: Option<String>,
    blocked: Option<bool>,
    ready: Option<bool>,
    claimed: Option<bool>,
    due_before: Option<String>,
    due_after: Option<String>,
    overdue: Option<bool>,
    include_done: Option<bool>,
    sort: Option<String>,
    group_by: Option<String>,
    columns_by: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

/// A filter that names one thing or asks for "none".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OneOrNone {
    One(Uuid),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LabelFilter {
    /// The task has every one of these.
    All(Vec<Uuid>),
    None,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Filters {
    search: Option<String>,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    semantic_category: Option<String>,
    priority: Option<String>,
    assignee: Option<OneOrNone>,
    archived: bool,
    labels: Option<LabelFilter>,
    milestone: Option<OneOrNone>,
    parent: Option<OneOrNone>,
    blocked: Option<bool>,
    ready: bool,
    claimed: Option<bool>,
    due_before: Option<NaiveDate>,
    due_after: Option<NaiveDate>,
    overdue: bool,
    hide_done: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    Newest,
    Oldest,
    Updated,
    Priority,
    Due,
    Title,
    Manual,
}

impl Sort {
    fn parse(value: Option<&str>) -> Result<Self, AppError> {
        Ok(match value.unwrap_or("newest") {
            "newest" => Self::Newest,
            "oldest" => Self::Oldest,
            "updated" => Self::Updated,
            "priority" => Self::Priority,
            "due" => Self::Due,
            "title" => Self::Title,
            "manual" => Self::Manual,
            _ => return Err(AppError::Validation("sort")),
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Updated => "updated",
            Self::Priority => "priority",
            Self::Due => "due",
            Self::Title => "title",
            Self::Manual => "manual",
        }
    }

    fn keys(self) -> Vec<Key> {
        let number_desc = Key::desc("item.number", "bigint");
        match self {
            Self::Newest => vec![number_desc],
            Self::Oldest => vec![Key::asc("item.number", "bigint")],
            Self::Updated => vec![Key::desc("item.updated_at", "timestamptz"), number_desc],
            Self::Priority => vec![Key::asc(PRIORITY_RANK, "integer"), number_desc],
            Self::Due => vec![
                Key::asc("COALESCE(item.due_date, 'infinity'::date)", "date"),
                Key::asc(PRIORITY_RANK, "integer"),
                number_desc,
            ],
            Self::Title => vec![
                Key::asc("lower(item.title)", "text"),
                Key::asc("item.number", "bigint"),
            ],
            Self::Manual => vec![
                Key::asc("status.position", "integer"),
                Key::asc("item.board_rank", "bigint"),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Grouping {
    Status,
    Assignee,
    Priority,
    Label,
    Milestone,
    Parent,
    Type,
}

impl Grouping {
    fn parse(value: &str, board: bool) -> Result<Self, AppError> {
        Ok(match value {
            "status" => Self::Status,
            "assignee" => Self::Assignee,
            "priority" => Self::Priority,
            "label" => Self::Label,
            "milestone" => Self::Milestone,
            "parent" if !board => Self::Parent,
            "type" => Self::Type,
            _ if board => return Err(AppError::Validation("columnsBy")),
            _ => return Err(AppError::Validation("groupBy")),
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Assignee => "assignee",
            Self::Priority => "priority",
            Self::Label => "label",
            Self::Milestone => "milestone",
            Self::Parent => "parent",
            Self::Type => "type",
        }
    }

    /// Joins the group needs beyond the base ones.
    fn join(self) -> &'static str {
        match self {
            Self::Label => {
                " LEFT JOIN work_item_active_labels group_label \
                   ON group_label.project_id = item.project_id AND group_label.work_item_id = item.id"
            }
            Self::Milestone => {
                " LEFT JOIN project_milestones group_milestone \
                   ON group_milestone.project_id = item.project_id AND group_milestone.id = item.milestone_id"
            }
            Self::Parent => {
                " LEFT JOIN work_items group_parent \
                   ON group_parent.project_id = item.project_id AND group_parent.id = item.parent_work_item_id"
            }
            _ => "",
        }
    }

    /// The group's ID (or "none"), its label and its colour, as SQL.
    fn columns(self) -> (&'static str, String, &'static str) {
        match self {
            Self::Status => ("status.id::text", "status.name".into(), "NULL::text"),
            Self::Type => ("item_type.id::text", "item_type.name".into(), "NULL::text"),
            Self::Priority => ("item.priority", PRIORITY_LABEL.into(), "NULL::text"),
            Self::Assignee => (
                "COALESCE(item.assignee_actor_id::text, 'none')",
                "COALESCE(assignee.display_name, 'No assignee')".into(),
                "NULL::text",
            ),
            Self::Label => (
                "COALESCE(group_label.label_id::text, 'none')",
                "COALESCE(group_label.name, 'No label')".into(),
                "group_label.color",
            ),
            Self::Milestone => (
                "COALESCE(item.milestone_id::text, 'none')",
                "COALESCE(group_milestone.name, 'No milestone')".into(),
                "NULL::text",
            ),
            Self::Parent => (
                "COALESCE(item.parent_work_item_id::text, 'none')",
                "COALESCE(project.project_key || '-' || group_parent.number || ' ' || group_parent.title, 'No parent')".into(),
                "NULL::text",
            ),
        }
    }

    /// The group's place in the order; "none" always comes last.
    fn keys(self) -> Vec<Key> {
        match self {
            Self::Status => vec![
                Key::asc("status.position", "integer"),
                Key::asc("status.id::text", "text"),
            ],
            Self::Type => vec![
                Key::asc("item_type.position", "integer"),
                Key::asc("item_type.id::text", "text"),
            ],
            Self::Priority => vec![Key::asc(PRIORITY_RANK, "integer")],
            Self::Assignee => vec![
                Key::asc("(item.assignee_actor_id IS NULL)::integer", "integer"),
                Key::asc("lower(COALESCE(assignee.display_name, ''))", "text"),
                Key::asc("COALESCE(item.assignee_actor_id::text, '')", "text"),
            ],
            Self::Label => vec![
                Key::asc("(group_label.label_id IS NULL)::integer", "integer"),
                Key::asc("lower(COALESCE(group_label.name, ''))", "text"),
                Key::asc("COALESCE(group_label.label_id::text, '')", "text"),
            ],
            Self::Milestone => vec![
                Key::asc("(item.milestone_id IS NULL)::integer", "integer"),
                Key::asc(
                    "COALESCE(group_milestone.target_date, 'infinity'::date)",
                    "date",
                ),
                Key::asc("lower(COALESCE(group_milestone.name, ''))", "text"),
                Key::asc("COALESCE(item.milestone_id::text, '')", "text"),
            ],
            Self::Parent => vec![
                Key::asc("(item.parent_work_item_id IS NULL)::integer", "integer"),
                Key::asc("COALESCE(group_parent.number, 0)", "bigint"),
            ],
        }
    }

    /// Whether some tasks have no value here, so a "none" group exists.
    fn has_none(self) -> bool {
        matches!(
            self,
            Self::Assignee | Self::Label | Self::Milestone | Self::Parent
        )
    }
}

/// One sort key: a non-null SQL expression, the type its cursor value is
/// cast back to, and the direction.
#[derive(Clone, Debug)]
struct Key {
    expr: String,
    ty: &'static str,
    desc: bool,
}

impl Key {
    fn asc(expr: &str, ty: &'static str) -> Self {
        Self {
            expr: expr.into(),
            ty,
            desc: false,
        }
    }

    fn desc(expr: &str, ty: &'static str) -> Self {
        Self {
            expr: expr.into(),
            ty,
            desc: true,
        }
    }
}

/// What a list request asks for, parsed and checked.
#[derive(Clone, Debug)]
pub(super) struct ListSpec {
    pub(super) filters: Filters,
    pub(super) sort: Sort,
    pub(super) grouping: Option<Grouping>,
}

impl ListSpec {
    fn keys(&self) -> Vec<Key> {
        let mut keys = self.grouping.map(Grouping::keys).unwrap_or_default();
        keys.extend(self.sort.keys());
        keys.push(Key::asc("item.id", "uuid"));
        keys
    }

    /// Changes whenever anything that decides the rows or their order does.
    fn fingerprint(&self, project_id: Uuid) -> String {
        let mut hasher = Sha256::new();
        hasher.update(project_id.as_bytes());
        hasher.update(format!("{:?}", self.filters.to_params()).as_bytes());
        hasher.update(self.sort.as_str());
        hasher.update(self.grouping.map(Grouping::as_str).unwrap_or(""));
        hex::encode(&hasher.finalize()[..12])
    }
}

fn parse_date(value: Option<&str>, field: &'static str) -> Result<Option<NaiveDate>, AppError> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| AppError::Validation(field))
        })
        .transpose()
}

fn parse_one_or_none(
    value: Option<&str>,
    field: &'static str,
) -> Result<Option<OneOrNone>, AppError> {
    match value {
        None => Ok(None),
        Some("none") => Ok(Some(OneOrNone::None)),
        Some(value) => Uuid::parse_str(value)
            .map(|id| Some(OneOrNone::One(id)))
            .map_err(|_| AppError::Validation(field)),
    }
}

impl Filters {
    fn parse(query: &TaskListQuery) -> Result<Self, AppError> {
        validate_priority_filter(query.priority.as_deref())?;
        validate_semantic_category(query.semantic_category.as_deref())?;
        let assignee = match (query.assignee_actor_id, query.unassigned) {
            (Some(_), Some(true)) => return Err(AppError::Validation("assignee filter")),
            (Some(id), _) => Some(OneOrNone::One(id)),
            (None, Some(true)) => Some(OneOrNone::None),
            (None, _) => None,
        };
        let labels = match query.label_ids.as_deref().map(str::trim) {
            None | Some("") => None,
            Some("none") => Some(LabelFilter::None),
            Some(value) => {
                let mut ids = value
                    .split(',')
                    .map(|part| Uuid::parse_str(part.trim()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| AppError::Validation("labelIds"))?;
                ids.sort();
                ids.dedup();
                if ids.len() > MAX_LABEL_FILTER {
                    return Err(AppError::Validation("labelIds"));
                }
                Some(LabelFilter::All(ids))
            }
        };
        Ok(Self {
            search: normalize_search(query.q.as_deref())?,
            type_id: query.type_id,
            status_id: query.status_id,
            semantic_category: query.semantic_category.clone(),
            priority: query.priority.clone(),
            assignee,
            archived: query.archived.unwrap_or(false),
            labels,
            milestone: parse_one_or_none(query.milestone_id.as_deref(), "milestoneId")?,
            parent: parse_one_or_none(query.parent_id.as_deref(), "parentId")?,
            blocked: query.blocked,
            ready: query.ready.unwrap_or(false),
            claimed: query.claimed,
            due_before: parse_date(query.due_before.as_deref(), "dueBefore")?,
            due_after: parse_date(query.due_after.as_deref(), "dueAfter")?,
            overdue: query.overdue.unwrap_or(false),
            hide_done: query.include_done == Some(false),
        })
    }

    /// The filters as list query parameters, so a board column can hand the
    /// web app the exact list request that pages it.
    fn to_params(&self) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            params.insert(key.to_owned(), value);
        };
        if let Some(value) = &self.search {
            put("q", value.clone());
        }
        if let Some(value) = self.type_id {
            put("typeId", value.to_string());
        }
        if let Some(value) = self.status_id {
            put("statusId", value.to_string());
        }
        if let Some(value) = &self.semantic_category {
            put("semanticCategory", value.clone());
        }
        if let Some(value) = &self.priority {
            put("priority", value.clone());
        }
        match self.assignee {
            Some(OneOrNone::One(id)) => put("assigneeActorId", id.to_string()),
            Some(OneOrNone::None) => put("unassigned", "true".into()),
            None => {}
        }
        if self.archived {
            put("archived", "true".into());
        }
        match &self.labels {
            Some(LabelFilter::All(ids)) => put(
                "labelIds",
                ids.iter()
                    .map(Uuid::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            Some(LabelFilter::None) => put("labelIds", "none".into()),
            None => {}
        }
        for (key, value) in [("milestoneId", self.milestone), ("parentId", self.parent)] {
            match value {
                Some(OneOrNone::One(id)) => put(key, id.to_string()),
                Some(OneOrNone::None) => put(key, "none".into()),
                None => {}
            }
        }
        if let Some(value) = self.blocked {
            put("blocked", value.to_string());
        }
        if self.ready {
            put("ready", "true".into());
        }
        if let Some(value) = self.claimed {
            put("claimed", value.to_string());
        }
        if let Some(value) = self.due_before {
            put("dueBefore", value.to_string());
        }
        if let Some(value) = self.due_after {
            put("dueAfter", value.to_string());
        }
        if self.overdue {
            put("overdue", "true".into());
        }
        if self.hide_done {
            put("includeDone", "false".into());
        }
        params
    }

    /// Narrows these filters to one board column or group.
    fn with_group(&self, grouping: Grouping, key: &str) -> Result<Self, AppError> {
        let mut filters = self.clone();
        let id = || Uuid::parse_str(key).map_err(|_| AppError::Internal("group key"));
        let one_or_none = || -> Result<OneOrNone, AppError> {
            if key == "none" {
                Ok(OneOrNone::None)
            } else {
                Ok(OneOrNone::One(id()?))
            }
        };
        match grouping {
            Grouping::Status => filters.status_id = Some(id()?),
            Grouping::Type => filters.type_id = Some(id()?),
            Grouping::Priority => filters.priority = Some(key.to_owned()),
            Grouping::Assignee => filters.assignee = Some(one_or_none()?),
            Grouping::Milestone => filters.milestone = Some(one_or_none()?),
            Grouping::Parent => filters.parent = Some(one_or_none()?),
            Grouping::Label => {
                // A label column keeps the labels the filter already
                // requires. Under such a filter the "No label" column counts
                // zero, so its page is never read.
                filters.labels = Some(match (key, filters.labels.take()) {
                    ("none", _) => LabelFilter::None,
                    (_, Some(LabelFilter::All(mut ids))) => {
                        let column = id()?;
                        if !ids.contains(&column) {
                            ids.push(column);
                        }
                        LabelFilter::All(ids)
                    }
                    _ => LabelFilter::All(vec![id()?]),
                });
            }
        }
        Ok(filters)
    }

    /// Appends ` AND ...` for every filter.
    fn push_where(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        if let Some(search) = &self.search {
            let escaped = search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            builder.push(" AND (item.title ILIKE ");
            builder.push_bind(format!("%{escaped}%"));
            if let Some(number) = search_number(search) {
                builder.push(" OR item.number = ");
                builder.push_bind(number);
            }
            builder.push(")");
        }
        if let Some(value) = self.type_id {
            builder.push(" AND item.type_id = ").push_bind(value);
        }
        if let Some(value) = self.status_id {
            builder.push(" AND item.status_id = ").push_bind(value);
        }
        if let Some(value) = &self.semantic_category {
            builder
                .push(" AND status.semantic_category = ")
                .push_bind(value.clone());
        }
        if let Some(value) = &self.priority {
            builder
                .push(" AND item.priority = ")
                .push_bind(value.clone());
        }
        match self.assignee {
            Some(OneOrNone::One(id)) => {
                builder.push(" AND item.assignee_actor_id = ").push_bind(id);
            }
            Some(OneOrNone::None) => {
                builder.push(" AND item.assignee_actor_id IS NULL");
            }
            None => {}
        }
        builder.push(if self.archived {
            " AND item.archived_at IS NOT NULL"
        } else {
            " AND item.archived_at IS NULL"
        });
        match &self.labels {
            Some(LabelFilter::All(ids)) => {
                builder.push(
                    " AND (SELECT count(*) FROM work_item_active_labels has_label \
                       WHERE has_label.project_id = item.project_id \
                         AND has_label.work_item_id = item.id AND has_label.label_id = ANY(",
                );
                builder.push_bind(ids.clone());
                builder.push(")) = ");
                builder.push_bind(ids.len() as i64);
            }
            Some(LabelFilter::None) => {
                builder.push(
                    " AND NOT EXISTS (SELECT 1 FROM work_item_active_labels has_label \
                       WHERE has_label.project_id = item.project_id AND has_label.work_item_id = item.id)",
                );
            }
            None => {}
        }
        match self.milestone {
            Some(OneOrNone::One(id)) => {
                builder.push(" AND item.milestone_id = ").push_bind(id);
            }
            Some(OneOrNone::None) => {
                builder.push(" AND item.milestone_id IS NULL");
            }
            None => {}
        }
        match self.parent {
            Some(OneOrNone::One(id)) => {
                builder
                    .push(" AND item.parent_work_item_id = ")
                    .push_bind(id);
            }
            Some(OneOrNone::None) => {
                builder.push(" AND item.parent_work_item_id IS NULL");
            }
            None => {}
        }
        match self.blocked {
            Some(true) => {
                builder.push(format!(
                    " AND (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER})"
                ));
            }
            Some(false) => {
                builder.push(format!(
                    " AND item.blocked_reason IS NULL AND NOT {HAS_OPEN_BLOCKER}"
                ));
            }
            None => {}
        }
        if self.ready {
            builder.push(format!(
                " AND NOT ({FINISHED}) AND item.blocked_reason IS NULL AND NOT {HAS_OPEN_BLOCKER}"
            ));
        }
        if let Some(claimed) = self.claimed {
            builder.push(if claimed { " AND " } else { " AND NOT " });
            builder.push(
                "EXISTS (SELECT 1 FROM work_item_claims claim \
                   WHERE claim.work_item_id = item.id AND claim.expires_at > now())",
            );
        }
        if let Some(value) = self.due_before {
            builder.push(" AND item.due_date <= ").push_bind(value);
        }
        if let Some(value) = self.due_after {
            builder.push(" AND item.due_date >= ").push_bind(value);
        }
        if self.overdue {
            builder.push(format!(
                " AND item.due_date < current_date AND NOT ({FINISHED})"
            ));
        }
        if self.hide_done {
            builder.push(format!(" AND NOT ({FINISHED})"));
        }
    }
}

/// "42", "#42" or "AW-42" as a task number.
fn search_number(search: &str) -> Option<i64> {
    let search = search.trim_start_matches('#');
    let digits = match search.rsplit_once('-') {
        Some((key, number)) if !key.is_empty() => number,
        Some(_) => return None,
        None => search,
    };
    digits
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0 && digits.chars().all(|c| c.is_ascii_digit()))
}

const BASE_FROM: &str = r#"
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = item.project_id AND item_type.id = item.type_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
"#;

#[derive(Debug, FromRow)]
struct PageRow {
    id: Uuid,
    group_key: Option<String>,
    group_label: Option<String>,
    keys: Vec<String>,
}

/// One page of task IDs in order, each with its group when grouped.
pub(super) struct Page {
    pub(super) rows: Vec<(Uuid, Option<GroupRef>)>,
    pub(super) next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct GroupRef {
    key: String,
    label: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorBody {
    f: String,
    k: Vec<String>,
}

fn encode_cursor(fingerprint: String, keys: Vec<String>) -> Result<String, AppError> {
    let body = serde_json::to_vec(&CursorBody {
        f: fingerprint,
        k: keys,
    })
    .map_err(|_| AppError::Internal("encode task cursor"))?;
    Ok(URL_SAFE_NO_PAD.encode(body))
}

fn decode_cursor(
    value: &str,
    fingerprint: &str,
    key_count: usize,
) -> Result<Vec<String>, AppError> {
    if value.len() > MAX_CURSOR_LENGTH {
        return Err(AppError::Validation("cursor"));
    }
    let body: CursorBody = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(AppError::Validation("cursor"))?;
    // A cursor made for other filters, sort or grouping would skip or
    // repeat rows; the client starts over instead.
    if body.f != fingerprint || body.k.len() != key_count {
        return Err(AppError::Validation("cursor"));
    }
    Ok(body.k)
}

pub(super) async fn page(
    pool: &PgPool,
    project_id: Uuid,
    spec: &ListSpec,
    cursor: Option<&str>,
    limit: i64,
) -> Result<Page, AppError> {
    let keys = spec.keys();
    let fingerprint = spec.fingerprint(project_id);
    let after = cursor
        .map(|value| decode_cursor(value, &fingerprint, keys.len()))
        .transpose()?;
    let (group_key, group_label, _) = spec.grouping.map(Grouping::columns).unwrap_or((
        "NULL::text",
        "NULL::text".into(),
        "NULL::text",
    ));
    let key_array = keys
        .iter()
        .map(|key| format!("({})::text", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT item.id, {group_key} AS group_key, {group_label} AS group_label, \
         ARRAY[{key_array}] AS keys {BASE_FROM} {} WHERE item.project_id = ",
        spec.grouping.map(Grouping::join).unwrap_or("")
    ));
    builder.push_bind(project_id);
    spec.filters.push_where(&mut builder);
    if let Some(after) = after {
        push_after(&mut builder, &keys, after);
    }
    builder.push(" ORDER BY ");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(format!(
            "{} {}",
            key.expr,
            if key.desc { "DESC" } else { "ASC" }
        ));
    }
    builder.push(" LIMIT ").push_bind(limit + 1);
    let mut rows = builder.build_query_as::<PageRow>().fetch_all(pool).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = match (has_more, rows.last()) {
        (true, Some(last)) => Some(encode_cursor(fingerprint, last.keys.clone())?),
        _ => None,
    };
    Ok(Page {
        rows: rows
            .into_iter()
            .map(|row| {
                let group = spec
                    .grouping
                    .and(row.group_key.zip(row.group_label))
                    .map(|(key, label)| GroupRef { key, label });
                (row.id, group)
            })
            .collect(),
        next_cursor,
    })
}

/// The first page of every board column in one query: tasks ranked within
/// their column in manual order. Each page and cursor is the one `page`
/// returns for that column's own filters, so "load more" carries on from it.
async fn first_pages(
    pool: &PgPool,
    project_id: Uuid,
    filters: &Filters,
    grouping: Grouping,
    limit: i64,
) -> Result<HashMap<String, Page>, AppError> {
    let keys = ListSpec {
        filters: filters.clone(),
        sort: Sort::Manual,
        grouping: None,
    }
    .keys();
    let (group_key, _, _) = grouping.columns();
    let key_array = keys
        .iter()
        .map(|key| format!("({})::text", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let order = keys
        .iter()
        .map(|key| format!("{} {}", key.expr, if key.desc { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT id, group_key, keys FROM (SELECT item.id, {group_key} AS group_key, \
         ARRAY[{key_array}] AS keys, \
         row_number() OVER (PARTITION BY {group_key} ORDER BY {order}) AS rank \
         {BASE_FROM} {} WHERE item.project_id = ",
        grouping.join()
    ));
    builder.push_bind(project_id);
    filters.push_where(&mut builder);
    builder.push(") ranked WHERE rank <= ");
    builder.push_bind(limit + 1);
    builder.push(" ORDER BY group_key, rank");
    let rows = builder
        .build_query_as::<(Uuid, Option<String>, Vec<String>)>()
        .fetch_all(pool)
        .await?;
    let mut ranked: HashMap<String, Vec<(Uuid, Vec<String>)>> = HashMap::new();
    for (id, key, keys) in rows {
        ranked
            .entry(key.unwrap_or_else(|| "none".into()))
            .or_default()
            .push((id, keys));
    }
    let mut pages = HashMap::with_capacity(ranked.len());
    for (key, mut rows) in ranked {
        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let fingerprint = ListSpec {
            filters: filters.with_group(grouping, &key)?,
            sort: Sort::Manual,
            grouping: None,
        }
        .fingerprint(project_id);
        let next_cursor = match (has_more, rows.last()) {
            (true, Some((_, last))) => Some(encode_cursor(fingerprint, last.clone())?),
            _ => None,
        };
        pages.insert(
            key,
            Page {
                rows: rows.into_iter().map(|(id, _)| (id, None)).collect(),
                next_cursor,
            },
        );
    }
    Ok(pages)
}

/// Rows strictly after `after` in the order of `keys`: the lexicographic
/// comparison spelt out, since the directions can differ.
fn push_after(builder: &mut QueryBuilder<'_, Postgres>, keys: &[Key], after: Vec<String>) {
    builder.push(" AND (");
    for index in 0..keys.len() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder.push("(");
        for (equal, value) in keys.iter().zip(&after).take(index) {
            builder.push(format!("({}) = CAST(", equal.expr));
            builder.push_bind(value.clone());
            builder.push(format!(" AS {}) AND ", equal.ty));
        }
        let key = &keys[index];
        builder.push(format!(
            "({}) {} CAST(",
            key.expr,
            if key.desc { "<" } else { ">" }
        ));
        builder.push_bind(after[index].clone());
        builder.push(format!(" AS {}))", key.ty));
    }
    builder.push(")");
}

pub(super) async fn total(
    pool: &PgPool,
    project_id: Uuid,
    filters: &Filters,
) -> Result<i64, AppError> {
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT count(*) {BASE_FROM} WHERE item.project_id = "
    ));
    builder.push_bind(project_id);
    filters.push_where(&mut builder);
    Ok(builder.build_query_scalar::<i64>().fetch_one(pool).await?)
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupCount {
    key: String,
    label: String,
    count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

/// Every group with tasks under these filters, in list order.
pub(super) async fn group_counts(
    pool: &PgPool,
    project_id: Uuid,
    filters: &Filters,
    grouping: Grouping,
) -> Result<Vec<GroupCount>, AppError> {
    let (key, label, color) = grouping.columns();
    let order = grouping
        .keys()
        .iter()
        .map(|key| format!("min({})", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT {key} AS key, min({label}) AS label, count(*) AS count, min({color}) AS color \
         {BASE_FROM} {} WHERE item.project_id = ",
        grouping.join()
    ));
    builder.push_bind(project_id);
    filters.push_where(&mut builder);
    builder.push(format!(" GROUP BY {key} ORDER BY {order}, {key}"));
    Ok(builder
        .build_query_as::<GroupCount>()
        .fetch_all(pool)
        .await?)
}

/// The values a board column can stand for, each with no tasks yet, in
/// board order. A filter on the column's own field leaves just the columns
/// that have tasks.
async fn column_values(
    pool: &PgPool,
    project_id: Uuid,
    grouping: Grouping,
    filters: &Filters,
) -> Result<Vec<GroupCount>, AppError> {
    let sql = match grouping {
        Grouping::Type if filters.type_id.is_none() => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, NULL::text AS color \
             FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id::text"
                .to_owned()
        }
        Grouping::Priority if filters.priority.is_none() => format!(
            "SELECT item.priority AS key, {PRIORITY_LABEL} AS label, 0::bigint AS count, NULL::text AS color \
             FROM (VALUES ('urgent'), ('high'), ('medium'), ('low'), ('no_priority')) AS item(priority) \
             WHERE $1::uuid IS NOT NULL ORDER BY {PRIORITY_RANK}"
        ),
        Grouping::Assignee if filters.assignee.is_none() => {
            "SELECT membership.actor_id::text AS key, membership.display_name AS label, \
                    0::bigint AS count, NULL::text AS color \
             FROM project_memberships membership JOIN actors actor ON actor.id = membership.actor_id \
             WHERE membership.project_id = $1 AND membership.enabled AND actor.status = 'active' \
               AND actor.kind IN ('human', 'external_agent') \
             ORDER BY lower(membership.display_name), membership.actor_id::text"
                .to_owned()
        }
        Grouping::Label if filters.labels.is_none() => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, color \
             FROM project_labels WHERE project_id = $1 AND archived_at IS NULL \
             ORDER BY lower(name), id::text"
                .to_owned()
        }
        Grouping::Milestone if filters.milestone.is_none() => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, NULL::text AS color \
             FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL \
             ORDER BY COALESCE(target_date, 'infinity'::date), lower(name), id::text"
                .to_owned()
        }
        _ => return Ok(Vec::new()),
    };
    Ok(sqlx::query_as::<_, GroupCount>(&sql)
        .bind(project_id)
        .fetch_all(pool)
        .await?)
}

/// A task as list rows and board cards show it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskCard {
    id: Uuid,
    project_id: Uuid,
    key: String,
    number: i64,
    title: String,
    item_type: WorkItemTypeView,
    status: WorkItemStatusView,
    priority: String,
    assignee: Option<AssigneeView>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    labels: Vec<WorkItemLabelView>,
    capabilities: WorkItemCapabilities,
    parent_id: Option<Uuid>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked_reason: Option<String>,
    open_blocker_count: i64,
    ready: bool,
    subtask_count: i64,
    subtasks_done: i64,
    claimed_by: Option<serde_json::Value>,
    /// Where the card sits in manual order (status, then rank), as a string
    /// that sorts the same way. Boards grouped by anything but status keep
    /// this order, so a moved card can be shown where it will land.
    board_order: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<GroupRef>,
}

#[derive(Debug, FromRow)]
struct CardRow {
    id: Uuid,
    project_id: Uuid,
    project_key: String,
    number: i64,
    title: String,
    type_id: Uuid,
    type_name: String,
    status_id: Uuid,
    status_name: String,
    semantic_category: String,
    priority: String,
    assignee_actor_id: Option<Uuid>,
    assignee_display_name: Option<String>,
    assignee_handle: Option<String>,
    assignee_kind: Option<String>,
    assignee_active: Option<bool>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    labels: serde_json::Value,
    parent_work_item_id: Option<Uuid>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked_reason: Option<String>,
    open_blocker_count: i64,
    subtask_count: i64,
    subtasks_done: i64,
    claimed_by: Option<serde_json::Value>,
    status_position: i32,
    board_rank: i64,
}

/// The cards for `ids`, in that order (an ID may repeat when grouped by
/// label). IDs not in the project are skipped.
pub(super) async fn cards(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    rows: &[(Uuid, Option<GroupRef>)],
    capabilities: WorkItemCapabilities,
) -> Result<Vec<TaskCard>, AppError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids = rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let found = sqlx::query_as::<_, CardRow>(&format!(
        r#"
        SELECT
            item.id, item.project_id, project.project_key, item.number, item.title,
            item.type_id, item_type.name AS type_name,
            item.status_id, status.name AS status_name, status.semantic_category,
            status.position AS status_position, item.board_rank,
            item.priority, item.assignee_actor_id,
            assignee.display_name AS assignee_display_name,
            assignee.handle AS assignee_handle,
            assignee_actor.kind AS assignee_kind,
            CASE WHEN assignee.actor_id IS NULL THEN NULL
                 ELSE assignee.enabled AND assignee_actor.status = 'active'
            END AS assignee_active,
            item.version, item.archived_at::text AS archived_at,
            item.created_at::text AS created_at, item.updated_at::text AS updated_at,
            {WORK_ITEM_LABELS_COLUMN},
            item.parent_work_item_id,
            (SELECT jsonb_build_object('id', milestone.id, 'name', milestone.name,
                                       'targetDate', milestone.target_date::text)
               FROM project_milestones milestone
              WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id
            ) AS milestone,
            item.start_date::text AS start_date, item.due_date::text AS due_date,
            item.estimate, item.blocked_reason,
            {OPEN_BLOCKER_COUNT} AS open_blocker_count,
            (SELECT count(*) FROM work_items child
              WHERE child.project_id = item.project_id
                AND child.parent_work_item_id = item.id AND child.archived_at IS NULL
            ) AS subtask_count,
            (SELECT count(*) FROM work_items child
               JOIN work_item_statuses child_status
                 ON child_status.project_id = child.project_id AND child_status.id = child.status_id
              WHERE child.project_id = item.project_id
                AND child.parent_work_item_id = item.id AND child.archived_at IS NULL
                AND child_status.semantic_category IN ('completed', 'canceled')
            ) AS subtasks_done,
            (SELECT jsonb_build_object('id', claimer.id, 'kind', claimer.kind,
                                       'displayName', COALESCE(claimer_membership.display_name, 'Unknown'),
                                       'handle', COALESCE(claimer_membership.handle, ''),
                                       'avatarUrl', NULL)
               FROM work_item_claims claim
               JOIN actors claimer ON claimer.id = claim.actor_id
               LEFT JOIN project_memberships claimer_membership
                 ON claimer_membership.project_id = item.project_id
                AND claimer_membership.actor_id = claim.actor_id
              WHERE claim.work_item_id = item.id AND claim.expires_at > now()
            ) AS claimed_by
        {BASE_FROM}
        LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        "#
    ))
    .bind(project_id)
    .bind(&ids)
    .fetch_all(executor)
    .await?;
    let mut by_id = found
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let mut cards = Vec::with_capacity(rows.len());
    for (index, (id, group)) in rows.iter().enumerate() {
        // The last time an ID appears it can be moved out of the map.
        let last = rows[index + 1..].iter().all(|(other, _)| other != id);
        let row = if last {
            by_id.remove(id)
        } else {
            by_id.get(id).map(clone_row)
        };
        if let Some(row) = row {
            cards.push(card(row, group.clone(), capabilities)?);
        }
    }
    Ok(cards)
}

fn clone_row(row: &CardRow) -> CardRow {
    CardRow {
        id: row.id,
        project_id: row.project_id,
        project_key: row.project_key.clone(),
        number: row.number,
        title: row.title.clone(),
        type_id: row.type_id,
        type_name: row.type_name.clone(),
        status_id: row.status_id,
        status_name: row.status_name.clone(),
        semantic_category: row.semantic_category.clone(),
        priority: row.priority.clone(),
        assignee_actor_id: row.assignee_actor_id,
        assignee_display_name: row.assignee_display_name.clone(),
        assignee_handle: row.assignee_handle.clone(),
        assignee_kind: row.assignee_kind.clone(),
        assignee_active: row.assignee_active,
        version: row.version,
        archived_at: row.archived_at.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        labels: row.labels.clone(),
        parent_work_item_id: row.parent_work_item_id,
        milestone: row.milestone.clone(),
        start_date: row.start_date.clone(),
        due_date: row.due_date.clone(),
        estimate: row.estimate,
        blocked_reason: row.blocked_reason.clone(),
        open_blocker_count: row.open_blocker_count,
        subtask_count: row.subtask_count,
        subtasks_done: row.subtasks_done,
        claimed_by: row.claimed_by.clone(),
        status_position: row.status_position,
        board_rank: row.board_rank,
    }
}

fn card(
    row: CardRow,
    group: Option<GroupRef>,
    capabilities: WorkItemCapabilities,
) -> Result<TaskCard, AppError> {
    validate_priority(&row.priority)?;
    let assignee = match (
        row.assignee_actor_id,
        row.assignee_display_name,
        row.assignee_handle,
        row.assignee_kind,
        row.assignee_active,
    ) {
        (Some(actor_id), Some(display_name), Some(handle), Some(kind), Some(active)) => {
            Some(AssigneeView {
                actor_id,
                display_name,
                handle,
                kind,
                active,
            })
        }
        (None, None, None, None, None) => None,
        _ => return Err(AppError::Internal("invalid task card assignee projection")),
    };
    let labels = serde_json::from_value(row.labels)
        .map_err(|_| AppError::Internal("invalid task card label projection"))?;
    let finished = matches!(row.semantic_category.as_str(), "completed" | "canceled");
    let ready = !finished
        && row.archived_at.is_none()
        && row.blocked_reason.is_none()
        && row.open_blocker_count == 0;
    Ok(TaskCard {
        id: row.id,
        project_id: row.project_id,
        key: format!("{}-{}", row.project_key, row.number),
        number: row.number,
        title: row.title,
        item_type: WorkItemTypeView {
            id: row.type_id,
            name: row.type_name,
        },
        status: WorkItemStatusView {
            id: row.status_id,
            name: row.status_name,
            semantic_category: row.semantic_category,
        },
        priority: row.priority,
        assignee,
        version: row.version,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        labels,
        capabilities,
        parent_id: row.parent_work_item_id,
        milestone: row.milestone,
        start_date: row.start_date,
        due_date: row.due_date,
        estimate: row.estimate,
        blocked_reason: row.blocked_reason,
        open_blocker_count: row.open_blocker_count,
        ready,
        subtask_count: row.subtask_count,
        subtasks_done: row.subtasks_done,
        claimed_by: row.claimed_by,
        board_order: board_order(row.status_position, row.board_rank),
        group,
    })
}

/// Status position, then rank, as fixed-width digits so that comparing the
/// strings compares the pair. The rank is shifted to be non-negative.
fn board_order(status_position: i32, board_rank: i64) -> String {
    let position = i64::from(status_position) - i64::from(i32::MIN);
    let rank = i128::from(board_rank) - i128::from(i64::MIN);
    format!("{position:010}:{rank:020}")
}

fn limit(value: Option<u16>) -> i64 {
    i64::from(value.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
}

/// Filter IDs must belong to the project, so a list can't probe another
/// project's statuses or members.
async fn validate_filter_ids(
    pool: &PgPool,
    project_id: Uuid,
    filters: &Filters,
) -> Result<(), AppError> {
    let assignee = match filters.assignee {
        Some(OneOrNone::One(id)) => Some(id),
        _ => None,
    };
    validate_list_filter_ids(
        pool,
        project_id,
        filters.type_id,
        filters.status_id,
        assignee,
    )
    .await?;
    let mut checks: Vec<(&'static str, &'static str, Vec<Uuid>)> = Vec::new();
    if let Some(LabelFilter::All(ids)) = &filters.labels {
        checks.push(("project_labels", "labelIds", ids.clone()));
    }
    if let Some(OneOrNone::One(id)) = filters.milestone {
        checks.push(("project_milestones", "milestoneId", vec![id]));
    }
    if let Some(OneOrNone::One(id)) = filters.parent {
        checks.push(("work_items", "parentId", vec![id]));
    }
    for (table, field, ids) in checks {
        let found = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE project_id = $1 AND id = ANY($2)"
        ))
        .bind(project_id)
        .bind(&ids)
        .fetch_one(pool)
        .await?;
        if found != ids.len() as i64 {
            return Err(AppError::Validation(field));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskListResponse {
    items: Vec<TaskCard>,
    next_cursor: Option<String>,
    total: i64,
}

pub(super) async fn list_work_items(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TaskListQuery>,
) -> Result<Json<TaskListResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if query.columns_by.is_some() {
        return Err(AppError::Validation("columnsBy"));
    }
    let spec = ListSpec {
        filters: Filters::parse(&query)?,
        sort: Sort::parse(query.sort.as_deref())?,
        grouping: query
            .group_by
            .as_deref()
            .map(|value| Grouping::parse(value, false))
            .transpose()?,
    };
    validate_filter_ids(&state.pool, project_id, &spec.filters).await?;
    let page = page(
        &state.pool,
        project_id,
        &spec,
        query.cursor.as_deref(),
        limit(query.limit),
    )
    .await?;
    let total = total(&state.pool, project_id, &spec.filters).await?;
    let items = cards(
        &state.pool,
        project_id,
        &page.rows,
        capabilities(&membership)?,
    )
    .await?;
    Ok(Json(TaskListResponse {
        items,
        next_cursor: page.next_cursor,
        total,
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct GroupsResponse {
    groups: Vec<GroupCount>,
}

pub(super) async fn list_work_item_groups(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TaskListQuery>,
) -> Result<Json<GroupsResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if query.cursor.is_some() || query.columns_by.is_some() {
        return Err(AppError::Validation("groups query"));
    }
    let grouping = Grouping::parse(
        query
            .group_by
            .as_deref()
            .ok_or(AppError::Validation("groupBy"))?,
        false,
    )?;
    let filters = Filters::parse(&query)?;
    validate_filter_ids(&state.pool, project_id, &filters).await?;
    let groups = group_counts(&state.pool, project_id, &filters, grouping).await?;
    Ok(Json(GroupsResponse { groups }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BoardResponse {
    columns_by: &'static str,
    columns: Vec<BoardColumn>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BoardColumn {
    key: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    status: Option<WorkItemStatusView>,
    items: Vec<TaskCard>,
    has_more: bool,
    total: i64,
    next_cursor: Option<String>,
    filter: BTreeMap<String, String>,
    /// Whether a card can be moved here. Columns for an archived milestone
    /// or label, a disabled type or a removed member stay for the tasks that
    /// still carry them, but nothing new can be given that value.
    droppable: bool,
}

pub(super) async fn get_work_item_board(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TaskListQuery>,
) -> Result<Json<BoardResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if query.cursor.is_some() {
        return Err(AppError::Invalid("board cursor"));
    }
    if query.group_by.is_some() || query.sort.is_some() {
        return Err(AppError::Validation("board query"));
    }
    let grouping = Grouping::parse(query.columns_by.as_deref().unwrap_or("status"), true)?;
    let filters = Filters::parse(&query)?;
    validate_filter_ids(&state.pool, project_id, &filters).await?;
    let limit = limit(query.limit);
    let counts = group_counts(&state.pool, project_id, &filters, grouping).await?;

    // Status columns are every enabled status, empty ones too; a status
    // filter keeps only its own column.
    let mut columns: Vec<(GroupCount, Option<WorkItemStatusView>)> = if grouping == Grouping::Status
    {
        let statuses = sqlx::query_as::<_, WorkflowStatusRow>(
            "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position, id",
        )
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
        let count_of = counts
            .iter()
            .map(|group| (group.key.clone(), group.count))
            .collect::<HashMap<_, _>>();
        statuses
            .into_iter()
            .filter(|status| filters.status_id.is_none_or(|id| id == status.id))
            .map(|status| {
                let key = status.id.to_string();
                (
                    GroupCount {
                        count: count_of.get(&key).copied().unwrap_or(0),
                        key,
                        label: status.name.clone(),
                        color: None,
                    },
                    Some(WorkItemStatusView {
                        id: status.id,
                        name: status.name,
                        semantic_category: status.semantic_category,
                    }),
                )
            })
            .collect()
    } else {
        // Every value a card can be dropped on gets a column, used or not;
        // values only tasks still carry (an archived label, a removed
        // member) follow them.
        let mut count_of = counts
            .into_iter()
            .map(|group| (group.key.clone(), group))
            .collect::<HashMap<_, _>>();
        let mut columns = column_values(&state.pool, project_id, grouping, &filters)
            .await?
            .into_iter()
            .map(|empty| (count_of.remove(&empty.key).unwrap_or(empty), None))
            .collect::<Vec<_>>();
        let mut rest = count_of.into_values().collect::<Vec<_>>();
        rest.sort_by(|a, b| {
            (a.key == "none", a.label.to_lowercase(), &a.key).cmp(&(
                b.key == "none",
                b.label.to_lowercase(),
                &b.key,
            ))
        });
        columns.extend(rest.into_iter().map(|group| (group, None)));
        if grouping.has_none() && !columns.iter().any(|(group, _)| group.key == "none") {
            columns.push((
                GroupCount {
                    key: "none".into(),
                    label: match grouping {
                        Grouping::Assignee => "No assignee",
                        Grouping::Label => "No label",
                        _ => "No milestone",
                    }
                    .into(),
                    count: 0,
                    color: None,
                },
                None,
            ));
        }
        columns
    };

    // The values a card can still be given, whatever the filters.
    let assignable = if grouping == Grouping::Status {
        None
    } else {
        Some(
            column_values(&state.pool, project_id, grouping, &Filters::default())
                .await?
                .into_iter()
                .map(|value| value.key)
                .collect::<HashSet<_>>(),
        )
    };
    let mut first = first_pages(&state.pool, project_id, &filters, grouping, limit).await?;
    let mut pages = Vec::with_capacity(columns.len());
    let mut all_rows = Vec::new();
    for (group, _) in &columns {
        let spec = ListSpec {
            filters: filters.with_group(grouping, &group.key)?,
            sort: Sort::Manual,
            grouping: None,
        };
        let page = first.remove(&group.key).unwrap_or(Page {
            rows: Vec::new(),
            next_cursor: None,
        });
        all_rows.extend(page.rows.iter().cloned());
        let mut filter = spec.filters.to_params();
        filter.insert("sort".into(), "manual".into());
        pages.push((page, filter));
    }
    let cards = cards(
        &state.pool,
        project_id,
        &all_rows,
        capabilities(&membership)?,
    )
    .await?
    .into_iter()
    .map(|card| (card.id, card))
    .collect::<HashMap<_, _>>();
    let columns = columns
        .drain(..)
        .zip(pages)
        .map(|((group, status), (page, filter))| BoardColumn {
            items: page
                .rows
                .iter()
                .filter_map(|(id, _)| cards.get(id).cloned())
                .collect(),
            has_more: page.next_cursor.is_some(),
            next_cursor: page.next_cursor,
            total: group.count,
            label: group.label,
            color: group.color,
            droppable: group.key == "none"
                || assignable
                    .as_ref()
                    .is_none_or(|values| values.contains(&group.key)),
            key: group.key,
            status,
            filter,
        })
        .collect();
    Ok(Json(BoardResponse {
        columns_by: grouping.as_str(),
        columns,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_order_sorts_like_status_then_rank() {
        let mut pairs = [(2, -5), (1, i64::MAX), (1, i64::MIN), (2, 7), (1, 0)];
        let mut keys = pairs.map(|(position, rank)| board_order(position, rank));
        pairs.sort_unstable();
        keys.sort();
        assert_eq!(
            keys,
            pairs.map(|(position, rank)| board_order(position, rank))
        );
    }

    #[test]
    fn task_numbers_are_found_in_searches() {
        assert_eq!(search_number("42"), Some(42));
        assert_eq!(search_number("#42"), Some(42));
        assert_eq!(search_number("AW-42"), Some(42));
        assert_eq!(search_number("fix-login"), None);
        assert_eq!(search_number("-3"), None);
        assert_eq!(search_number("4a"), None);
    }

    #[test]
    fn cursors_are_refused_under_other_filters() {
        let project = Uuid::now_v7();
        let spec = ListSpec {
            filters: Filters::default(),
            sort: Sort::Due,
            grouping: Some(Grouping::Assignee),
        };
        let cursor =
            encode_cursor(spec.fingerprint(project), vec!["1".into(); 7]).expect("test value");
        assert!(decode_cursor(&cursor, &spec.fingerprint(project), spec.keys().len()).is_ok());
        let mut narrower = spec.clone();
        narrower.filters.hide_done = true;
        assert!(decode_cursor(&cursor, &narrower.fingerprint(project), 7).is_err());
        let resorted = ListSpec {
            sort: Sort::Title,
            ..spec.clone()
        };
        assert!(decode_cursor(&cursor, &resorted.fingerprint(project), 7).is_err());
        assert!(decode_cursor(&cursor, &spec.fingerprint(Uuid::now_v7()), 7).is_err());
    }

    #[test]
    fn board_columns_round_trip_as_list_filters() {
        let filters = Filters {
            hide_done: true,
            labels: Some(LabelFilter::All(vec![Uuid::nil()])),
            ..Filters::default()
        };
        let column = filters
            .with_group(Grouping::Assignee, "none")
            .expect("test value");
        let params = column.to_params();
        assert_eq!(params.get("unassigned").map(String::as_str), Some("true"));
        assert_eq!(params.get("includeDone").map(String::as_str), Some("false"));
        let query: TaskListQuery =
            serde_urlencoded::from_str(&serde_urlencoded::to_string(&params).expect("test value"))
                .expect("test value");
        let parsed = Filters::parse(&query).expect("test value");
        assert_eq!(format!("{:?}", parsed.to_params()), format!("{params:?}"));
    }
}
