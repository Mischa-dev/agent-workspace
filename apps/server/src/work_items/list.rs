//! Task lists, group counts and board columns at project scale.
//!
//! A page is read in two steps: the first query finds the page's task IDs in
//! order (filters, keyset and sort only, so the planner can use the indexes),
//! the second reads the cards for just those IDs. Cursors are keysets over
//! the full sort, group included, so every sort and grouping pages without
//! OFFSET and without ranking the whole project. A cursor remembers a
//! fingerprint of the filters it was made for and is refused under others.
//!
//! Most filters take several values, comma separated: `statusId=a,b` lists
//! tasks in either status and `notStatusId=a,b` leaves both out. "none"
//! stands for no value where a field can be empty (`assigneeActorId=none,x`
//! is unassigned or assigned to x). Labels have three: `labelIds` (all of
//! them, as it always was), `anyLabelIds` and `notLabelIds`.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{Json, extract::State};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{
    AssigneeView, FIELD_VALUES_COLUMNS, WORK_ITEM_LABELS_COLUMN, WorkItemCapabilities,
    WorkItemLabelView, WorkItemStatusView, WorkItemTypeView, WorkflowStatusRow,
    authorize_work_item, capabilities, normalize_search, project_membership, validate_priority,
    validate_semantic_category,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    fields::{self, FieldClause, FieldDefinition, FieldType},
    policy::Action,
    work_item_fields,
    work_item_planning::{HAS_OPEN_BLOCKER, OPEN_BLOCKER_COUNT},
};

const DEFAULT_LIMIT: u16 = 50;
const MAX_LIMIT: u16 = 200;
/// The most values one filter names, such as 20 labels or 20 people.
pub(crate) const MAX_FILTER_VALUES: usize = 20;
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
    type_id: Option<String>,
    not_type_id: Option<String>,
    status_id: Option<String>,
    not_status_id: Option<String>,
    semantic_category: Option<String>,
    not_semantic_category: Option<String>,
    priority: Option<String>,
    not_priority: Option<String>,
    assignee_actor_id: Option<String>,
    not_assignee_actor_id: Option<String>,
    /// Older links' way of saying `assigneeActorId=none`.
    unassigned: Option<bool>,
    archived: Option<bool>,
    label_ids: Option<String>,
    any_label_ids: Option<String>,
    not_label_ids: Option<String>,
    milestone_id: Option<String>,
    not_milestone_id: Option<String>,
    parent_id: Option<String>,
    blocked: Option<bool>,
    ready: Option<bool>,
    claimed: Option<bool>,
    due_before: Option<String>,
    due_after: Option<String>,
    overdue: Option<bool>,
    include_done: Option<bool>,
    /// Custom field filters (AWD-89): a JSON array of
    /// `{"field", "op", "values"}` (crate::fields::FieldClause).
    field_filters: Option<String>,
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

/// The values one filter names, each once and in order, and whether it
/// names "none": no value at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Values<T> {
    values: Vec<T>,
    none: bool,
}

impl<T> Default for Values<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            none: false,
        }
    }
}

impl<T: Ord> Values<T> {
    /// `values`, each once and in order, and with `none` no value at all.
    pub(crate) fn new(mut values: Vec<T>, none: bool) -> Self {
        values.sort();
        values.dedup();
        Self { values, none }
    }
}

impl<T: PartialEq> Values<T> {
    /// Whether a task whose value is `value` (`None`: it has none) is named.
    fn names(&self, value: Option<&T>) -> bool {
        value.map_or(self.none, |value| self.values.contains(value))
    }
}

impl<T: ToString> Values<T> {
    /// As a query parameter: the values, then "none".
    fn param(&self) -> String {
        self.values
            .iter()
            .map(T::to_string)
            .chain(self.none.then(|| "none".to_owned()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// What one field must be: any of `any`, and none of `not`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FieldFilter<T> {
    any: Option<Values<T>>,
    not: Option<Values<T>>,
}

impl<T> Default for FieldFilter<T> {
    fn default() -> Self {
        Self {
            any: None,
            not: None,
        }
    }
}

impl<T> FieldFilter<T> {
    pub(crate) fn new(any: Option<Values<T>>, not: Option<Values<T>>) -> Self {
        Self { any, not }
    }
}

impl<T: Ord> FieldFilter<T> {
    fn parse(
        any: Option<&str>,
        not: Option<&str>,
        fields: (&'static str, &'static str),
        none_allowed: bool,
        parse: impl Fn(&str) -> Option<T> + Copy,
    ) -> Result<Self, AppError> {
        Ok(Self {
            any: parse_values(any, fields.0, none_allowed, parse)?,
            not: parse_values(not, fields.1, none_allowed, parse)?,
        })
    }

    /// Just `value` (`None`: no value), as a board column narrows its field.
    fn only(value: Option<T>) -> Self {
        Self {
            any: Some(Values {
                none: value.is_none(),
                values: value.into_iter().collect(),
            }),
            not: None,
        }
    }

    /// Whether a task whose value is `value` can match.
    fn allows(&self, value: Option<&T>) -> bool {
        self.any.as_ref().is_none_or(|any| any.names(value))
            && !self.not.as_ref().is_some_and(|not| not.names(value))
    }
}

impl<T: ToString> FieldFilter<T> {
    fn put(&self, params: &mut BTreeMap<String, String>, keys: (&str, &str)) {
        for (key, values) in [(keys.0, &self.any), (keys.1, &self.not)] {
            if let Some(values) = values {
                params.insert(key.to_owned(), values.param());
            }
        }
    }
}

/// A comma-separated filter value: each part read by `parse`, or "none"
/// where the field can be empty. At most 20, each kept once.
fn parse_values<T: Ord>(
    value: Option<&str>,
    field: &'static str,
    none_allowed: bool,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<Values<T>>, AppError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let mut values = Values::default();
    for part in value.split(',').map(str::trim) {
        if none_allowed && part == "none" {
            values.none = true;
        } else {
            values
                .values
                .push(parse(part).ok_or(AppError::Validation(field))?);
        }
    }
    values.values.sort();
    values.values.dedup();
    if values.values.len() + usize::from(values.none) > MAX_FILTER_VALUES {
        return Err(AppError::Validation(field));
    }
    Ok(Some(values))
}

#[derive(Clone, Debug, Default)]
pub(super) struct Filters {
    search: Option<String>,
    types: FieldFilter<Uuid>,
    statuses: FieldFilter<Uuid>,
    categories: FieldFilter<String>,
    priorities: FieldFilter<String>,
    assignees: FieldFilter<Uuid>,
    archived: bool,
    /// All of these labels (`labelIds`), or none at all.
    labels: Option<LabelFilter>,
    /// At least one of these; "none" also lets in tasks with no labels.
    any_labels: Option<Values<Uuid>>,
    /// None of these; "none" leaves out tasks with no labels.
    not_labels: Option<Values<Uuid>>,
    milestones: FieldFilter<Uuid>,
    parent: Option<OneOrNone>,
    blocked: Option<bool>,
    ready: bool,
    claimed: Option<bool>,
    due_before: Option<NaiveDate>,
    due_after: Option<NaiveDate>,
    overdue: bool,
    hide_done: bool,
    /// Custom field filters, all of which must match.
    fields: Vec<FieldClause>,
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
    /// By a custom field (`field:<id>`, or `field:<id>:desc`): tasks without
    /// a value last, then newest first.
    Field {
        id: Uuid,
        desc: bool,
    },
}

impl Sort {
    fn parse(value: Option<&str>) -> Result<Self, AppError> {
        if let Some(rest) = value.and_then(|value| value.strip_prefix("field:")) {
            let (id, desc) = match rest.strip_suffix(":desc") {
                Some(id) => (id, true),
                None => (rest.strip_suffix(":asc").unwrap_or(rest), false),
            };
            let id = Uuid::parse_str(id).map_err(|_| AppError::Validation("sort"))?;
            return Ok(Self::Field { id, desc });
        }
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
            Self::Field { .. } => "field",
        }
    }

    /// The sort as a list request writes it.
    fn param(self) -> String {
        match self {
            Self::Field { id, desc: true } => format!("field:{id}:desc"),
            Self::Field { id, desc: false } => format!("field:{id}"),
            other => other.as_str().to_owned(),
        }
    }

    /// The keys of a field sort, given the field (`ListSpec::sort_field`).
    fn field_keys(desc: bool, field: Option<&FieldDefinition>) -> Vec<Key> {
        let mut keys = field
            .map(|field| {
                fields::sort_keys(field, "item.field_values", "item.project_id")
                    .into_iter()
                    .map(|key| Key {
                        expr: key.expr,
                        ty: key.ty,
                        desc: desc && key.follows_direction,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        keys.push(Key::desc("item.number", "bigint"));
        keys
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
            Self::Field { desc, .. } => Self::field_keys(desc, None),
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
    /// By a custom field (`field:<id>`) that can group (choice, person,
    /// checkbox); its type is filled in by `resolve_fields`. Lists only.
    Field(Uuid, Option<FieldType>),
}

impl Grouping {
    fn parse(value: &str, board: bool) -> Result<Self, AppError> {
        if !board && let Some(id) = value.strip_prefix("field:") {
            let id = Uuid::parse_str(id).map_err(|_| AppError::Validation("groupBy"))?;
            return Ok(Self::Field(id, None));
        }
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
            Self::Field(..) => "field",
        }
    }

    /// A field grouping's SQL, once its type is known.
    fn field_sql(self) -> Option<fields::GroupSql> {
        match self {
            Self::Field(id, Some(field_type)) => {
                fields::group_sql(id, field_type, "item.field_values", "item.project_id")
            }
            _ => None,
        }
    }

    /// Joins the group needs beyond the base ones.
    fn join(self) -> String {
        if let Some(sql) = self.field_sql() {
            return sql.join;
        }
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
        .to_owned()
    }

    /// The group's ID (or "none"), its label and its colour, as SQL.
    fn columns(self) -> (String, String, String) {
        if let Some(sql) = self.field_sql() {
            return (sql.key, sql.label, sql.color);
        }
        let (key, label, color) = self.fixed_columns();
        (key.to_owned(), label, color.to_owned())
    }

    fn fixed_columns(self) -> (&'static str, String, &'static str) {
        match self {
            // An unresolved field groups nothing; resolve_fields refuses it.
            Self::Field(..) => ("'none'", "'No value'".into(), "NULL::text"),
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
        if let Some(sql) = self.field_sql() {
            return sql
                .order
                .into_iter()
                .map(|key| Key {
                    expr: key.expr,
                    ty: key.ty,
                    desc: false,
                })
                .collect();
        }
        match self {
            Self::Field(..) => Vec::new(),
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
            Self::Assignee | Self::Label | Self::Milestone | Self::Parent | Self::Field(..)
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
    /// The field a `Sort::Field` sorts by, from `resolve_fields`.
    pub(super) sort_field: Option<FieldDefinition>,
}

impl ListSpec {
    fn keys(&self) -> Vec<Key> {
        let mut keys = self.grouping.map(Grouping::keys).unwrap_or_default();
        match self.sort {
            Sort::Field { desc, .. } => {
                keys.extend(Sort::field_keys(desc, self.sort_field.as_ref()));
            }
            sort => keys.extend(sort.keys()),
        }
        keys.push(Key::asc("item.id", "uuid"));
        keys
    }

    /// Changes whenever anything that decides the rows or their order does.
    fn fingerprint(&self, project_id: Uuid) -> String {
        let mut hasher = Sha256::new();
        hasher.update(project_id.as_bytes());
        hasher.update(format!("{:?}", self.filters.to_params()).as_bytes());
        hasher.update(self.sort.as_str());
        if let Sort::Field { .. } = self.sort {
            hasher.update(self.sort.param());
            if let Some(field) = &self.sort_field {
                hasher.update(format!("{:?}", field.config));
            }
        }
        hasher.update(self.grouping.map(Grouping::as_str).unwrap_or(""));
        if let Some(Grouping::Field(id, _)) = self.grouping {
            hasher.update(id.as_bytes());
        }
        hex::encode(&hasher.finalize()[..12])
    }

    /// Fills in the custom fields a sort or grouping names, from the
    /// project's own fields: one in another project, archived, or unable to
    /// group is refused.
    pub(super) async fn resolve_fields(
        &mut self,
        pool: &PgPool,
        project_id: Uuid,
    ) -> Result<(), AppError> {
        let sorts_by_field = matches!(self.sort, Sort::Field { .. });
        let groups_by_field = matches!(self.grouping, Some(Grouping::Field(..)));
        if !sorts_by_field && !groups_by_field {
            return Ok(());
        }
        let definitions = work_item_fields::definitions(pool, project_id, false).await?;
        if let Sort::Field { id, .. } = self.sort {
            self.sort_field = Some(
                definitions
                    .iter()
                    .find(|field| field.id == id)
                    .cloned()
                    .ok_or(AppError::Validation("sort"))?,
            );
        }
        if let Some(grouping) = self.grouping {
            self.grouping = Some(resolve_grouping(grouping, &definitions)?);
        }
        Ok(())
    }
}

/// A field grouping with its field's type, which must be able to group.
fn resolve_grouping(
    grouping: Grouping,
    definitions: &[FieldDefinition],
) -> Result<Grouping, AppError> {
    let Grouping::Field(id, _) = grouping else {
        return Ok(grouping);
    };
    let field = definitions
        .iter()
        .find(|field| field.id == id && !field.archived && field.field_type.can_group())
        .ok_or(AppError::Validation("groupBy"))?;
    Ok(Grouping::Field(id, Some(field.field_type)))
}

/// Board swimlanes a saved view can keep beside the list query.
const LANES: [&str; 5] = ["none", "assignee", "priority", "milestone", "type"];

/// A view's parameter with "me" (alone or among other assignees) replaced
/// by `reader`, whoever opens the view.
pub(super) fn resolve_me(key: &str, value: &str, reader: Uuid) -> String {
    if key == "fieldFilters" {
        // A person field's "me" is whoever opens the view, too.
        let Some(mut clauses) = fields::parse_clauses(value) else {
            return value.to_owned();
        };
        for clause in &mut clauses {
            for value in &mut clause.values {
                if value == "me" {
                    *value = reader.to_string();
                }
            }
        }
        return fields::clauses_param(&clauses);
    }
    if !matches!(key, "assigneeActorId" | "notAssigneeActorId") {
        return value.to_owned();
    }
    value
        .split(',')
        .map(|part| {
            if part.trim() == "me" {
                reader.to_string()
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A saved view's list parameters (filters, sort, grouping, board columns
/// and lanes), read exactly as a list request reads them. "me" as an
/// assignee stands for `reader`, whoever opens the view.
pub(super) fn saved_query_spec(
    params: &BTreeMap<String, String>,
    reader: Uuid,
) -> Result<ListSpec, AppError> {
    let mut list = BTreeMap::new();
    for (key, value) in params {
        match key.as_str() {
            "columnsBy" => {
                Grouping::parse(value, true)?;
            }
            "lanes" if LANES.contains(&value.as_str()) => {}
            "lanes" => return Err(AppError::Validation("lanes")),
            "cursor" | "limit" => return Err(AppError::Validation("query")),
            _ => {
                list.insert(key.clone(), resolve_me(key, value, reader));
            }
        }
    }
    let encoded = serde_urlencoded::to_string(&list).map_err(|_| AppError::Validation("query"))?;
    let query: TaskListQuery =
        serde_urlencoded::from_str(&encoded).map_err(|_| AppError::Validation("query"))?;
    Ok(ListSpec {
        filters: Filters::parse(&query)?,
        sort: Sort::parse(query.sort.as_deref())?,
        grouping: query
            .group_by
            .as_deref()
            .map(|value| Grouping::parse(value, false))
            .transpose()?,
        sort_field: None,
    })
}

/// A saved view's filters as task_list applies them for agents: read
/// exactly as the list page reads the view, so both see the same tasks.
#[derive(Clone, Debug)]
pub(crate) struct ViewFilters {
    filters: Filters,
    /// Whether the view decides archived tasks; not when the caller does.
    archived_rule: bool,
}

impl ViewFilters {
    /// `params` as the view keeps them, "me" read as `reader`.
    pub(crate) fn parse(params: &BTreeMap<String, String>, reader: Uuid) -> Result<Self, AppError> {
        Ok(Self {
            filters: saved_query_spec(params, reader)?.filters,
            archived_rule: true,
        })
    }

    /// The caller says whether archived tasks are listed, not the view.
    pub(crate) fn leave_archived_to_caller(&mut self) {
        self.archived_rule = false;
    }

    /// The view lists only archived tasks.
    pub(crate) fn archived_only(&self) -> bool {
        self.archived_rule && self.filters.archived
    }

    /// The view's conditions on `item`, its `status` and `project`, each
    /// starting with AND.
    pub(crate) fn push_where(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        self.filters.push_conditions(builder, self.archived_rule);
    }

    /// Changes whenever the tasks the view matches could.
    pub(crate) fn fingerprint(&self) -> String {
        format!("{:?} {}", self.filters.to_params(), self.archived_rule)
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
        let id = |part: &str| Uuid::parse_str(part).ok();
        let priority = |part: &str| validate_priority(part).is_ok().then(|| part.to_owned());
        let category = |part: &str| {
            validate_semantic_category(Some(part))
                .is_ok()
                .then(|| part.to_owned())
        };
        let mut assignees = FieldFilter::parse(
            query.assignee_actor_id.as_deref(),
            query.not_assignee_actor_id.as_deref(),
            ("assigneeActorId", "notAssigneeActorId"),
            true,
            id,
        )?;
        if query.unassigned == Some(true) {
            assignees.any.get_or_insert_with(Values::default).none = true;
        }
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
                if ids.len() > MAX_FILTER_VALUES {
                    return Err(AppError::Validation("labelIds"));
                }
                Some(LabelFilter::All(ids))
            }
        };
        Ok(Self {
            search: normalize_search(query.q.as_deref())?,
            types: FieldFilter::parse(
                query.type_id.as_deref(),
                query.not_type_id.as_deref(),
                ("typeId", "notTypeId"),
                false,
                id,
            )?,
            statuses: FieldFilter::parse(
                query.status_id.as_deref(),
                query.not_status_id.as_deref(),
                ("statusId", "notStatusId"),
                false,
                id,
            )?,
            categories: FieldFilter::parse(
                query.semantic_category.as_deref(),
                query.not_semantic_category.as_deref(),
                ("semanticCategory", "notSemanticCategory"),
                false,
                category,
            )?,
            priorities: FieldFilter::parse(
                query.priority.as_deref(),
                query.not_priority.as_deref(),
                ("priority", "notPriority"),
                false,
                priority,
            )?,
            assignees,
            archived: query.archived.unwrap_or(false),
            labels,
            any_labels: parse_values(query.any_label_ids.as_deref(), "anyLabelIds", true, id)?,
            not_labels: parse_values(query.not_label_ids.as_deref(), "notLabelIds", true, id)?,
            milestones: FieldFilter::parse(
                query.milestone_id.as_deref(),
                query.not_milestone_id.as_deref(),
                ("milestoneId", "notMilestoneId"),
                true,
                id,
            )?,
            parent: parse_one_or_none(query.parent_id.as_deref(), "parentId")?,
            blocked: query.blocked,
            ready: query.ready.unwrap_or(false),
            claimed: query.claimed,
            due_before: parse_date(query.due_before.as_deref(), "dueBefore")?,
            due_after: parse_date(query.due_after.as_deref(), "dueAfter")?,
            overdue: query.overdue.unwrap_or(false),
            hide_done: query.include_done == Some(false),
            fields: match query.field_filters.as_deref() {
                Some(param) => {
                    fields::parse_clauses(param).ok_or(AppError::Validation("fieldFilters"))?
                }
                None => Vec::new(),
            },
        })
    }

    /// The filters as list query parameters, so a board column can hand the
    /// web app the exact list request that pages it.
    fn to_params(&self) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        if let Some(value) = &self.search {
            params.insert("q".to_owned(), value.clone());
        }
        self.types.put(&mut params, ("typeId", "notTypeId"));
        self.statuses.put(&mut params, ("statusId", "notStatusId"));
        self.categories
            .put(&mut params, ("semanticCategory", "notSemanticCategory"));
        self.priorities
            .put(&mut params, ("priority", "notPriority"));
        self.assignees
            .put(&mut params, ("assigneeActorId", "notAssigneeActorId"));
        self.milestones
            .put(&mut params, ("milestoneId", "notMilestoneId"));
        let mut put = |key: &str, value: String| {
            params.insert(key.to_owned(), value);
        };
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
        for (key, values) in [
            ("anyLabelIds", &self.any_labels),
            ("notLabelIds", &self.not_labels),
        ] {
            if let Some(values) = values {
                put(key, values.param());
            }
        }
        match self.parent {
            Some(OneOrNone::One(id)) => put("parentId", id.to_string()),
            Some(OneOrNone::None) => put("parentId", "none".into()),
            None => {}
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
        if !self.fields.is_empty() {
            put("fieldFilters", fields::clauses_param(&self.fields));
        }
        params
    }

    /// Whether a board column or group for `key` can hold tasks under these
    /// filters. A column the filters rule out isn't shown, so narrowing to a
    /// column (`with_group`) only ever narrows to a value the filters allow.
    fn allows_group(&self, grouping: Grouping, key: &str) -> bool {
        let id = Uuid::parse_str(key).ok();
        let value = || {
            if key == "none" {
                Ok(None)
            } else {
                id.ok_or(()).map(Some)
            }
        };
        match grouping {
            Grouping::Status => id.is_some_and(|id| self.statuses.allows(Some(&id))),
            Grouping::Type => id.is_some_and(|id| self.types.allows(Some(&id))),
            Grouping::Priority => self.priorities.allows(Some(&key.to_owned())),
            Grouping::Assignee => value().is_ok_and(|value| self.assignees.allows(value.as_ref())),
            Grouping::Milestone => {
                value().is_ok_and(|value| self.milestones.allows(value.as_ref()))
            }
            Grouping::Parent | Grouping::Field(..) => true,
            // Labels other than the ones a filter names can sit beside them,
            // so only "none of" rules a label out.
            Grouping::Label if key == "none" => {
                !matches!(self.labels, Some(LabelFilter::All(_)))
                    && self.any_labels.as_ref().is_none_or(|any| any.none)
                    && !self.not_labels.as_ref().is_some_and(|not| not.none)
            }
            Grouping::Label => {
                !matches!(self.labels, Some(LabelFilter::None))
                    && id.is_some_and(|id| {
                        !self
                            .not_labels
                            .as_ref()
                            .is_some_and(|not| not.values.contains(&id))
                    })
            }
        }
    }

    /// Narrows these filters to one board column or group whose key
    /// `allows_group` accepts: the column's own value, the other filters as
    /// they are.
    fn with_group(&self, grouping: Grouping, key: &str) -> Result<Self, AppError> {
        let mut filters = self.clone();
        let id = || Uuid::parse_str(key).map_err(|_| AppError::Internal("group key"));
        let value = || -> Result<Option<Uuid>, AppError> {
            if key == "none" {
                Ok(None)
            } else {
                id().map(Some)
            }
        };
        match grouping {
            Grouping::Status => filters.statuses = FieldFilter::only(Some(id()?)),
            Grouping::Type => filters.types = FieldFilter::only(Some(id()?)),
            Grouping::Priority => filters.priorities = FieldFilter::only(Some(key.to_owned())),
            Grouping::Assignee => filters.assignees = FieldFilter::only(value()?),
            Grouping::Milestone => filters.milestones = FieldFilter::only(value()?),
            Grouping::Parent => {
                filters.parent = Some(value()?.map_or(OneOrNone::None, OneOrNone::One));
            }
            Grouping::Field(field, _) => {
                if filters.fields.len() >= fields::MAX_CLAUSES {
                    return Err(AppError::Validation("fieldFilters"));
                }
                filters.fields.push(fields::group_clause(field, key));
            }
            Grouping::Label if key == "none" => {
                filters.labels = Some(LabelFilter::None);
                filters.any_labels = None;
                filters.not_labels = None;
            }
            Grouping::Label => {
                // A label column keeps the labels the filter already
                // requires, and adds its own.
                let column = id()?;
                let mut ids = match filters.labels.take() {
                    Some(LabelFilter::All(ids)) => ids,
                    _ => Vec::new(),
                };
                if !ids.contains(&column) {
                    ids.push(column);
                    ids.sort();
                }
                filters.labels = Some(LabelFilter::All(ids));
            }
        }
        Ok(filters)
    }

    /// Appends ` AND ...` for every filter.
    fn push_where(&self, builder: &mut QueryBuilder<'_, Postgres>) {
        self.push_conditions(builder, true);
    }

    /// The filters as conditions on `item`, its `status` and `project`, each
    /// starting with AND. `archived_rule` adds the one on archived tasks
    /// (only them, or none of them).
    fn push_conditions(&self, builder: &mut QueryBuilder<'_, Postgres>, archived_rule: bool) {
        if let Some(search) = &self.search {
            let escaped = search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            builder.push(" AND (item.title ILIKE ");
            builder.push_bind(format!("%{escaped}%"));
            if let Some((key, number)) = search_number(search) {
                builder.push(" OR (item.number = ");
                builder.push_bind(number);
                // "OTHER-12" in project AW must not find AW-12.
                if let Some(key) = key {
                    builder.push(" AND upper(project.project_key) = upper(");
                    builder.push_bind(key.to_owned());
                    builder.push(")");
                }
                builder.push(")");
            }
            builder.push(")");
        }
        push_field(builder, "item.type_id", false, &self.types);
        push_field(builder, "item.status_id", false, &self.statuses);
        push_field(builder, "status.semantic_category", false, &self.categories);
        push_field(builder, "item.priority", false, &self.priorities);
        push_field(builder, "item.assignee_actor_id", true, &self.assignees);
        if archived_rule {
            builder.push(if self.archived {
                " AND item.archived_at IS NOT NULL"
            } else {
                " AND item.archived_at IS NULL"
            });
        }
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
                builder.push(format!(" AND NOT {HAS_SOME_LABEL}"));
            }
            None => {}
        }
        if let Some(any) = &self.any_labels {
            builder.push(" AND (false");
            if !any.values.is_empty() {
                builder.push(format!(" OR {HAS_LABEL_IN}"));
                builder.push_bind(any.values.clone());
                builder.push("))");
            }
            if any.none {
                builder.push(format!(" OR NOT {HAS_SOME_LABEL}"));
            }
            builder.push(")");
        }
        if let Some(not) = &self.not_labels {
            if !not.values.is_empty() {
                builder.push(format!(" AND NOT {HAS_LABEL_IN}"));
                builder.push_bind(not.values.clone());
                builder.push("))");
            }
            if not.none {
                builder.push(format!(" AND {HAS_SOME_LABEL}"));
            }
        }
        push_field(builder, "item.milestone_id", true, &self.milestones);
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
        fields::push_clauses(builder, "item.field_values", &self.fields);
    }
}

/// The task has an active label (archived ones don't count)...
const HAS_SOME_LABEL: &str = "EXISTS (SELECT 1 FROM work_item_active_labels has_label \
     WHERE has_label.project_id = item.project_id AND has_label.work_item_id = item.id)";
/// ...or one of those bound next, closed with `))`.
const HAS_LABEL_IN: &str = "EXISTS (SELECT 1 FROM work_item_active_labels has_label \
     WHERE has_label.project_id = item.project_id AND has_label.work_item_id = item.id \
       AND has_label.label_id = ANY(";

/// Appends a field filter on `column` (which may be null when `nullable`).
/// "Is not" keeps tasks with no value: not assigned to Ana includes
/// unassigned tasks, unless "none" is one of the values left out.
pub(crate) fn push_field<'args, T>(
    builder: &mut QueryBuilder<'args, Postgres>,
    column: &str,
    nullable: bool,
    filter: &FieldFilter<T>,
) where
    T: Clone,
    Vec<T>: 'args + sqlx::Encode<'args, Postgres> + sqlx::Type<Postgres>,
{
    if let Some(any) = &filter.any {
        builder.push(" AND (false");
        if !any.values.is_empty() {
            builder.push(format!(" OR {column} = ANY("));
            builder.push_bind(any.values.clone());
            builder.push(")");
        }
        if any.none {
            builder.push(format!(" OR {column} IS NULL"));
        }
        builder.push(")");
    }
    if let Some(not) = &filter.not {
        if !not.values.is_empty() {
            builder.push(if nullable && !not.none {
                format!(" AND ({column} IS NULL OR {column} <> ALL(")
            } else {
                format!(" AND ({column} <> ALL(")
            });
            builder.push_bind(not.values.clone());
            builder.push("))");
        }
        if not.none {
            builder.push(format!(" AND {column} IS NOT NULL"));
        }
    }
}

/// "42", "#42" or "AW-42" as a task number, with the project key if given.
fn search_number(search: &str) -> Option<(Option<&str>, i64)> {
    let search = search.trim_start_matches('#');
    let (key, digits) = match search.rsplit_once('-') {
        Some((key, number)) if !key.is_empty() => (Some(key), number),
        Some(_) => return None,
        None => (None, search),
    };
    digits
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0 && digits.chars().all(|c| c.is_ascii_digit()))
        .map(|number| (key, number))
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
        "NULL::text".into(),
        "NULL::text".into(),
        "NULL::text".into(),
    ));
    let key_array = keys
        .iter()
        .map(|key| format!("({})::text", key.expr))
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT item.id, {group_key} AS group_key, {group_label} AS group_label, \
         ARRAY[{key_array}] AS keys {BASE_FROM} {} WHERE item.project_id = ",
        spec.grouping.map(Grouping::join).unwrap_or_default()
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
        sort_field: None,
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
            sort_field: None,
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

/// Which of `work_item_ids` a saved view's query (or the same query inline)
/// matches, read as `reader` would read it. Automations (X5) use a view's
/// filters as their conditions.
pub(crate) async fn matching_tasks(
    pool: &PgPool,
    project_id: Uuid,
    params: &BTreeMap<String, String>,
    reader: Uuid,
    work_item_ids: Option<&[Uuid]>,
    limit: i64,
) -> Result<Vec<Uuid>, AppError> {
    let spec = saved_query_spec(params, reader)?;
    let mut builder = QueryBuilder::<Postgres>::new(format!(
        "SELECT item.id {BASE_FROM} WHERE item.project_id = "
    ));
    builder.push_bind(project_id);
    if let Some(ids) = work_item_ids {
        builder
            .push(" AND item.id = ANY(")
            .push_bind(ids.to_vec())
            .push(")");
    }
    spec.filters.push_where(&mut builder);
    builder.push(" ORDER BY item.created_at, item.id LIMIT ");
    builder.push_bind(limit.clamp(1, 500));
    Ok(builder.build_query_scalar::<Uuid>().fetch_all(pool).await?)
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
/// board order. The board then keeps those its filters allow. A label filter
/// that asks for labels keeps the columns to those, rather than every label.
async fn column_values(
    pool: &PgPool,
    project_id: Uuid,
    grouping: Grouping,
    filters: &Filters,
) -> Result<Vec<GroupCount>, AppError> {
    let named_labels = match (&filters.labels, &filters.any_labels) {
        (None, None) => None,
        (labels, any) => Some(
            match labels {
                Some(LabelFilter::All(ids)) => ids.clone(),
                _ => Vec::new(),
            }
            .into_iter()
            .chain(any.iter().flat_map(|any| any.values.iter().copied()))
            .collect::<Vec<_>>(),
        ),
    };
    let sql = match grouping {
        Grouping::Type => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, NULL::text AS color \
             FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id::text"
                .to_owned()
        }
        Grouping::Priority => format!(
            "SELECT item.priority AS key, {PRIORITY_LABEL} AS label, 0::bigint AS count, NULL::text AS color \
             FROM (VALUES ('urgent'), ('high'), ('medium'), ('low'), ('no_priority')) AS item(priority) \
             WHERE $1::uuid IS NOT NULL ORDER BY {PRIORITY_RANK}"
        ),
        Grouping::Assignee => {
            "SELECT membership.actor_id::text AS key, membership.display_name AS label, \
                    0::bigint AS count, NULL::text AS color \
             FROM project_memberships membership JOIN actors actor ON actor.id = membership.actor_id \
             WHERE membership.project_id = $1 AND membership.enabled AND actor.status = 'active' \
               AND actor.kind IN ('human', 'external_agent') \
             ORDER BY lower(membership.display_name), membership.actor_id::text"
                .to_owned()
        }
        Grouping::Label => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, color \
             FROM project_labels WHERE project_id = $1 AND archived_at IS NULL \
               AND ($2::uuid[] IS NULL OR id = ANY($2)) \
             ORDER BY lower(name), id::text"
                .to_owned()
        }
        Grouping::Milestone => {
            "SELECT id::text AS key, name AS label, 0::bigint AS count, NULL::text AS color \
             FROM project_milestones WHERE project_id = $1 AND archived_at IS NULL \
             ORDER BY COALESCE(target_date, 'infinity'::date), lower(name), id::text"
                .to_owned()
        }
        _ => return Ok(Vec::new()),
    };
    let query = sqlx::query_as::<_, GroupCount>(&sql).bind(project_id);
    let query = if grouping == Grouping::Label {
        query.bind(named_labels)
    } else {
        query
    };
    Ok(query.fetch_all(pool).await?)
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
    /// Custom field values by field ID, and the people, tasks and pages
    /// they name (AWD-89).
    field_values: serde_json::Value,
    field_references: serde_json::Value,
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
    field_values: serde_json::Value,
    field_references: serde_json::Value,
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
            {FIELD_VALUES_COLUMNS},
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
        field_values: row.field_values.clone(),
        field_references: row.field_references.clone(),
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
    let card = TaskCard {
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
        field_values: row.field_values,
        field_references: row.field_references,
    };
    Ok(if capabilities.tasks {
        card
    } else {
        without_contents(card)
    })
}

/// For someone who may change tasks here but not see them (a bulk change's
/// response, say): the card's id, key, version, type and status, not its
/// contents, as a single task's view does.
fn without_contents(card: TaskCard) -> TaskCard {
    TaskCard {
        title: String::new(),
        priority: "no_priority".to_owned(),
        assignee: None,
        labels: Vec::new(),
        parent_id: None,
        milestone: None,
        start_date: None,
        due_date: None,
        estimate: None,
        blocked_reason: None,
        open_blocker_count: 0,
        ready: false,
        subtask_count: 0,
        subtasks_done: 0,
        claimed_by: None,
        field_values: serde_json::json!({}),
        field_references: serde_json::json!([]),
        ..card
    }
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
pub(super) async fn validate_filter_ids(
    pool: &PgPool,
    project_id: Uuid,
    filters: &Filters,
) -> Result<(), AppError> {
    // (table, its ID column, the query parameter, the IDs)
    let mut checks: Vec<(&'static str, &'static str, &'static str, Vec<Uuid>)> = Vec::new();
    let mut check = |table, column, field, values: Option<&Values<Uuid>>| {
        if let Some(values) = values.filter(|values| !values.values.is_empty()) {
            checks.push((table, column, field, values.values.clone()));
        }
    };
    check(
        "work_item_types",
        "id",
        "typeId",
        filters.types.any.as_ref(),
    );
    check(
        "work_item_types",
        "id",
        "notTypeId",
        filters.types.not.as_ref(),
    );
    check(
        "work_item_statuses",
        "id",
        "statusId",
        filters.statuses.any.as_ref(),
    );
    check(
        "work_item_statuses",
        "id",
        "notStatusId",
        filters.statuses.not.as_ref(),
    );
    check(
        "project_memberships",
        "actor_id",
        "assigneeActorId",
        filters.assignees.any.as_ref(),
    );
    check(
        "project_memberships",
        "actor_id",
        "notAssigneeActorId",
        filters.assignees.not.as_ref(),
    );
    check(
        "project_labels",
        "id",
        "anyLabelIds",
        filters.any_labels.as_ref(),
    );
    check(
        "project_labels",
        "id",
        "notLabelIds",
        filters.not_labels.as_ref(),
    );
    check(
        "project_milestones",
        "id",
        "milestoneId",
        filters.milestones.any.as_ref(),
    );
    check(
        "project_milestones",
        "id",
        "notMilestoneId",
        filters.milestones.not.as_ref(),
    );
    if let Some(LabelFilter::All(ids)) = &filters.labels {
        checks.push(("project_labels", "id", "labelIds", ids.clone()));
    }
    if let Some(OneOrNone::One(id)) = filters.parent {
        checks.push(("work_items", "id", "parentId", vec![id]));
    }
    if !filters.fields.is_empty() {
        // Every field filter names one of this project's fields, with
        // values that suit it.
        let definitions = work_item_fields::definitions(pool, project_id, true).await?;
        for clause in &filters.fields {
            let field = definitions
                .iter()
                .find(|field| field.id == clause.field)
                .ok_or(AppError::Validation("fieldFilters"))?;
            fields::check_clause(field, clause)
                .map_err(|_| AppError::Validation("fieldFilters"))?;
        }
    }
    for (table, column, field, ids) in checks {
        let found = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE project_id = $1 AND {column} = ANY($2)"
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
    ApiQuery(mut query): ApiQuery<TaskListQuery>,
) -> Result<Json<TaskListResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    // "me" in a person field's filter is whoever is reading.
    query.field_filters = query
        .field_filters
        .map(|value| resolve_me("fieldFilters", &value, principal.actor_id));
    if query.columns_by.is_some() {
        return Err(AppError::Validation("columnsBy"));
    }
    let mut spec = ListSpec {
        filters: Filters::parse(&query)?,
        sort: Sort::parse(query.sort.as_deref())?,
        grouping: query
            .group_by
            .as_deref()
            .map(|value| Grouping::parse(value, false))
            .transpose()?,
        sort_field: None,
    };
    validate_filter_ids(&state.pool, project_id, &spec.filters).await?;
    spec.resolve_fields(&state.pool, project_id).await?;
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
    ApiQuery(mut query): ApiQuery<TaskListQuery>,
) -> Result<Json<GroupsResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    // "me" in a person field's filter is whoever is reading.
    query.field_filters = query
        .field_filters
        .map(|value| resolve_me("fieldFilters", &value, principal.actor_id));
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
    let grouping = if matches!(grouping, Grouping::Field(..)) {
        resolve_grouping(
            grouping,
            &work_item_fields::definitions(&state.pool, project_id, false).await?,
        )?
    } else {
        grouping
    };
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
    ApiQuery(mut query): ApiQuery<TaskListQuery>,
) -> Result<Json<BoardResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    // "me" in a person field's filter is whoever is reading.
    query.field_filters = query
        .field_filters
        .map(|value| resolve_me("fieldFilters", &value, principal.actor_id));
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

    // Status columns are every enabled status, empty ones too; a status or
    // status group filter keeps only the columns it allows.
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
            .filter(|status| {
                filters.statuses.allows(Some(&status.id))
                    && filters.categories.allows(Some(&status.semantic_category))
            })
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
        // Every value a card can be dropped on gets a column, used or not,
        // unless the filters rule it out; values only tasks still carry (an
        // archived label, a removed member) follow them.
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
        columns.retain(|(group, _)| filters.allows_group(grouping, &group.key));
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
            sort_field: None,
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
        assert_eq!(search_number("42"), Some((None, 42)));
        assert_eq!(search_number("#42"), Some((None, 42)));
        assert_eq!(search_number("AW-42"), Some((Some("AW"), 42)));
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
            sort_field: None,
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
        assert_eq!(
            params.get("assigneeActorId").map(String::as_str),
            Some("none")
        );
        assert_eq!(params.get("includeDone").map(String::as_str), Some("false"));
        let query: TaskListQuery =
            serde_urlencoded::from_str(&serde_urlencoded::to_string(&params).expect("test value"))
                .expect("test value");
        let parsed = Filters::parse(&query).expect("test value");
        assert_eq!(format!("{:?}", parsed.to_params()), format!("{params:?}"));
    }
}
