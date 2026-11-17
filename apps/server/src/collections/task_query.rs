//! The task list's query parameters as a view of the Tasks collection
//! (T1.8, T1.9): the one translation from what the list page's address, a
//! saved automation condition or a task tool's arguments say
//! (`statusId=...&includeDone=false&sort=due`) into filters, sorts and a
//! grouping on the Tasks source's fields, so every task list runs on the
//! one query compiler (`views::query`).
//!
//! Each list filter is a built-in field (`tasks::STATUS`, `ASSIGNEE`, ...),
//! so this is a table, not a second engine. "me" stays "me": the compiler
//! reads it as whoever reads the list.

use std::collections::BTreeMap;

use sqlx::PgConnection;
use uuid::Uuid;

use super::tasks;
use crate::{
    error::AppError,
    fields::{FieldClause, FieldOp},
    views::{
        definition::{self as views, Direction, SortRule, ViewConfig},
        filter::Filter,
        query::{self, Archived, Spec},
    },
};

/// A task list query as a view: what `views::query::Spec::from_config`
/// takes, with the words and the archived rule beside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskQuery {
    pub config: ViewConfig,
    /// The `q` words.
    pub search: Option<String>,
    pub archived: Archived,
    /// A board's columns (`columnsBy`), when it isn't the grouping.
    pub columns_by: Option<Uuid>,
}

/// What a parameter got wrong, as `(parameter, message)`.
pub type Problem = (&'static str, String);

fn problem(key: &'static str, message: impl Into<String>) -> Problem {
    (key, message.into())
}

/// The parts of a comma-separated value, and whether "none" was one.
fn parts(value: &str) -> (Vec<String>, bool) {
    let mut values = Vec::new();
    let mut none = false;
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if part == "none" {
            none = true;
        } else if !values.iter().any(|known| known == part) {
            values.push(part.to_owned());
        }
    }
    (values, none)
}

fn ids(key: &'static str, values: &[String], me: bool) -> Result<Vec<String>, Problem> {
    values
        .iter()
        .map(|value| {
            if me && value == "me" {
                return Ok(value.clone());
            }
            value
                .parse::<Uuid>()
                .map(|id| id.to_string())
                .map_err(|_| problem(key, format!("\"{value}\" isn't an id.")))
        })
        .collect()
}

fn clause(field: Uuid, op: FieldOp, values: Vec<String>) -> Filter {
    Filter::Clause(FieldClause { field, op, values })
}

/// "Any of these, or none": `is` the values, `empty` for none, or both.
fn is_any(field: Uuid, values: Vec<String>, none: bool) -> Option<Filter> {
    let mut either = Vec::new();
    if !values.is_empty() {
        either.push(clause(field, FieldOp::Is, values));
    }
    if none {
        either.push(clause(field, FieldOp::Empty, Vec::new()));
    }
    match either.len() {
        0 => None,
        1 => either.pop(),
        _ => Some(Filter::Or(either)),
    }
}

/// "None of these, and not none": `isNot` the values, `notEmpty` for none.
fn is_none_of(field: Uuid, values: Vec<String>, none: bool, into: &mut Vec<Filter>) {
    if !values.is_empty() {
        into.push(clause(field, FieldOp::IsNot, values));
    }
    if none {
        into.push(clause(field, FieldOp::NotEmpty, Vec::new()));
    }
}

fn flag(key: &'static str, value: &str) -> Result<bool, Problem> {
    match value.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(problem(key, format!("\"{other}\" isn't true or false."))),
    }
}

fn date(key: &'static str, value: &str) -> Result<String, Problem> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map(|day| day.to_string())
        .map_err(|_| problem(key, "Pass a date as YYYY-MM-DD."))
}

fn priorities(key: &'static str, values: &[String]) -> Result<Vec<String>, Problem> {
    values
        .iter()
        .map(|value| {
            tasks::priority_option(value)
                .map(|id| id.to_string())
                .ok_or_else(|| problem(key, "A priority is urgent, high, medium, low or none."))
        })
        .collect()
}

/// The grouping a `groupBy` or `columnsBy` names, as a field.
pub fn grouping_field(value: &str) -> Option<Uuid> {
    if let Some(id) = value.strip_prefix("field:") {
        return id.parse().ok();
    }
    Some(match value {
        "status" => tasks::STATUS,
        "assignee" => tasks::ASSIGNEE,
        "priority" => tasks::PRIORITY,
        "label" | "labels" => tasks::LABELS,
        "milestone" => tasks::MILESTONE,
        "parent" => tasks::PARENT,
        "type" => tasks::TYPE,
        _ => return None,
    })
}

/// The sort a `sort` names, most important key first.
pub fn sorts(value: &str) -> Option<Vec<SortRule>> {
    let rule = |field: String, direction: Direction| SortRule { field, direction };
    let field = |id: Uuid| id.to_string();
    if let Some(id) = value.strip_prefix("field:") {
        let (id, direction) = match id.strip_suffix(":desc") {
            Some(id) => (id, Direction::Desc),
            None => (id, Direction::Asc),
        };
        let id = id.parse::<Uuid>().ok()?;
        return Some(vec![
            rule(field(id), direction),
            rule("number".to_owned(), Direction::Desc),
        ]);
    }
    Some(match value {
        "newest" => vec![rule("number".to_owned(), Direction::Desc)],
        "oldest" => vec![rule("number".to_owned(), Direction::Asc)],
        "updated" => vec![rule("updated".to_owned(), Direction::Desc)],
        "priority" => vec![
            rule(field(tasks::PRIORITY), Direction::Asc),
            rule("number".to_owned(), Direction::Desc),
        ],
        "due" => vec![
            rule(field(tasks::DUE_DATE), Direction::Asc),
            rule(field(tasks::PRIORITY), Direction::Asc),
        ],
        "title" => vec![rule("title".to_owned(), Direction::Asc)],
        "manual" => vec![rule(tasks::RANK.to_owned(), Direction::Asc)],
        _ => return None,
    })
}

/// Reads the list's parameters. Unknown parameters are refused, so a typo
/// never silently lists everything; `cursor` and `limit` aren't part of a
/// query and are left to the caller.
pub fn from_params(params: &BTreeMap<String, String>) -> Result<TaskQuery, Problem> {
    let mut all = Vec::<Filter>::new();
    let mut overdue = false;
    let mut query = TaskQuery::default();
    for (key, value) in params {
        let value = value.trim();
        match key.as_str() {
            "q" => {
                query.search = (!value.is_empty()).then(|| value.to_owned());
            }
            "statusId" => {
                let (values, none) = parts(value);
                if none {
                    return Err(problem("statusId", "A task always has a status."));
                }
                if !values.is_empty() {
                    all.push(clause(
                        tasks::STATUS,
                        FieldOp::Is,
                        ids("statusId", &values, false)?,
                    ));
                }
            }
            "notStatusId" => {
                let (values, none) = parts(value);
                if none {
                    return Err(problem("notStatusId", "A task always has a status."));
                }
                if !values.is_empty() {
                    all.push(clause(
                        tasks::STATUS,
                        FieldOp::IsNot,
                        ids("notStatusId", &values, false)?,
                    ));
                }
            }
            "semanticCategory" => {
                let (values, _) = parts(value);
                if !values.is_empty() {
                    all.push(clause(tasks::STATUS, FieldOp::CategoryIs, values));
                }
            }
            "notSemanticCategory" => {
                let (values, _) = parts(value);
                if !values.is_empty() {
                    all.push(clause(tasks::STATUS, FieldOp::CategoryIsNot, values));
                }
            }
            "includeDone" => {
                if !flag("includeDone", value)? {
                    all.push(finished_excluded());
                }
            }
            "priority" => {
                let (values, none) = parts(value);
                let mut values = values;
                let none = none || values.iter().any(|value| value == "no_priority");
                values.retain(|value| value != "no_priority");
                all.extend(is_any(
                    tasks::PRIORITY,
                    priorities("priority", &values)?,
                    none,
                ));
            }
            "notPriority" => {
                let (mut values, none) = parts(value);
                let none = none || values.iter().any(|value| value == "no_priority");
                values.retain(|value| value != "no_priority");
                is_none_of(
                    tasks::PRIORITY,
                    priorities("notPriority", &values)?,
                    none,
                    &mut all,
                );
            }
            "assigneeActorId" => {
                let (values, none) = parts(value);
                all.extend(is_any(
                    tasks::ASSIGNEE,
                    ids("assigneeActorId", &values, true)?,
                    none,
                ));
            }
            "notAssigneeActorId" => {
                let (values, none) = parts(value);
                is_none_of(
                    tasks::ASSIGNEE,
                    ids("notAssigneeActorId", &values, true)?,
                    none,
                    &mut all,
                );
            }
            "unassigned" => {
                if flag("unassigned", value)? {
                    all.push(clause(tasks::ASSIGNEE, FieldOp::Empty, Vec::new()));
                }
            }
            "labelIds" => {
                let (values, none) = parts(value);
                if none && !values.is_empty() {
                    return Err(problem(
                        "labelIds",
                        "\"none\" stands alone: a task with all of these labels has some.",
                    ));
                }
                for label in ids("labelIds", &values, false)? {
                    all.push(clause(tasks::LABELS, FieldOp::Is, vec![label]));
                }
                if none {
                    all.push(clause(tasks::LABELS, FieldOp::Empty, Vec::new()));
                }
            }
            "anyLabelIds" => {
                let (values, none) = parts(value);
                all.extend(is_any(
                    tasks::LABELS,
                    ids("anyLabelIds", &values, false)?,
                    none,
                ));
            }
            "notLabelIds" => {
                let (values, none) = parts(value);
                is_none_of(
                    tasks::LABELS,
                    ids("notLabelIds", &values, false)?,
                    none,
                    &mut all,
                );
            }
            "typeId" => {
                let (values, none) = parts(value);
                if none {
                    return Err(problem("typeId", "A task always has a type."));
                }
                if !values.is_empty() {
                    all.push(clause(
                        tasks::TYPE,
                        FieldOp::Is,
                        ids("typeId", &values, false)?,
                    ));
                }
            }
            "notTypeId" => {
                let (values, none) = parts(value);
                if none {
                    return Err(problem("notTypeId", "A task always has a type."));
                }
                if !values.is_empty() {
                    all.push(clause(
                        tasks::TYPE,
                        FieldOp::IsNot,
                        ids("notTypeId", &values, false)?,
                    ));
                }
            }
            "milestoneId" => {
                let (values, none) = parts(value);
                all.extend(is_any(
                    tasks::MILESTONE,
                    ids("milestoneId", &values, false)?,
                    none,
                ));
            }
            "notMilestoneId" => {
                let (values, none) = parts(value);
                is_none_of(
                    tasks::MILESTONE,
                    ids("notMilestoneId", &values, false)?,
                    none,
                    &mut all,
                );
            }
            "parentId" => {
                let (values, none) = parts(value);
                all.extend(is_any(
                    tasks::PARENT,
                    ids("parentId", &values, false)?,
                    none,
                ));
            }
            "blocked" => all.push(if flag("blocked", value)? {
                clause(tasks::BLOCKED, FieldOp::Is, vec!["true".to_owned()])
            } else {
                clause(tasks::BLOCKED, FieldOp::Empty, Vec::new())
            }),
            "ready" => all.push(if flag("ready", value)? {
                clause(tasks::READY, FieldOp::Is, vec!["true".to_owned()])
            } else {
                clause(tasks::READY, FieldOp::Empty, Vec::new())
            }),
            "claimed" => all.push(if flag("claimed", value)? {
                clause(tasks::CLAIMED_BY, FieldOp::NotEmpty, Vec::new())
            } else {
                clause(tasks::CLAIMED_BY, FieldOp::Empty, Vec::new())
            }),
            "dueBefore" => all.push(clause(
                tasks::DUE_DATE,
                FieldOp::Lte,
                vec![date("dueBefore", value)?],
            )),
            "dueAfter" => all.push(clause(
                tasks::DUE_DATE,
                FieldOp::Gte,
                vec![date("dueAfter", value)?],
            )),
            "overdue" => {
                if flag("overdue", value)? {
                    all.push(clause(
                        tasks::DUE_DATE,
                        FieldOp::Within,
                        vec!["overdue".to_owned()],
                    ));
                    overdue = true;
                }
            }
            "archived" => {
                query.archived = if flag("archived", value)? {
                    Archived::Only
                } else {
                    Archived::Exclude
                };
            }
            "fieldFilters" | "groupField" => {
                if value.is_empty() {
                    continue;
                }
                let parsed = if key == "groupField" {
                    serde_json::from_str::<FieldClause>(value).map(|clause| vec![clause])
                } else {
                    serde_json::from_str::<Vec<FieldClause>>(value)
                };
                let clauses = parsed.map_err(|_| {
                    problem(
                        if key == "groupField" {
                            "groupField"
                        } else {
                            "fieldFilters"
                        },
                        "Pass field filters as a JSON array of {\"field\", \"op\", \"values\"}.",
                    )
                })?;
                all.extend(clauses.into_iter().map(Filter::Clause));
            }
            "sort" => {
                if !value.is_empty() {
                    query.config.sort = sorts(value).ok_or_else(|| {
                        problem(
                            "sort",
                            "Sort by newest, oldest, updated, priority, due, title, manual or \
                             field:<id>.",
                        )
                    })?;
                }
            }
            "groupBy" => {
                if !value.is_empty() && value != "none" {
                    query.config.group_by = Some(grouping_field(value).ok_or_else(|| {
                        problem(
                            "groupBy",
                            "Group by status, assignee, priority, label, milestone, parent, \
                             type or field:<id>.",
                        )
                    })?);
                }
            }
            "columnsBy" => {
                if !value.is_empty() {
                    query.columns_by = Some(grouping_field(value).ok_or_else(|| {
                        problem(
                            "columnsBy",
                            "Board columns are by status, assignee, priority, label, milestone, \
                             type or field:<id>.",
                        )
                    })?);
                }
            }
            // A board's lanes and a page's place are the screen's, not the
            // query's.
            "lanes" | "cursor" | "limit" | "layout" => {}
            _ => {
                return Err(problem(
                    "query",
                    format!("\"{key}\" isn't a task list filter."),
                ));
            }
        }
    }
    // A finished task is never overdue, as in task_list: "overdue" brings
    // "not done" with it (once, if includeDone=false said so already).
    if overdue {
        let unfinished = finished_excluded();
        if !all.contains(&unfinished) {
            all.push(unfinished);
        }
    }
    query.config.filters = Filter::And(all);
    Ok(query)
}

/// Leaves out completed and canceled tasks (`includeDone=false`).
fn finished_excluded() -> Filter {
    clause(
        tasks::STATUS,
        FieldOp::CategoryIsNot,
        vec!["completed".to_owned(), "canceled".to_owned()],
    )
}

/// The `groupBy` or `columnsBy` value that names a field: the other way
/// round from [`grouping_field`].
pub fn grouping_name(field: Uuid) -> String {
    match field {
        tasks::STATUS => "status".to_owned(),
        tasks::ASSIGNEE => "assignee".to_owned(),
        tasks::PRIORITY => "priority".to_owned(),
        tasks::LABELS => "label".to_owned(),
        tasks::MILESTONE => "milestone".to_owned(),
        tasks::PARENT => "parent".to_owned(),
        tasks::TYPE => "type".to_owned(),
        other => format!("field:{other}"),
    }
}

/// A group's key as the task screens use it: a priority's group is its
/// stored name ("urgent"), every other group its option's or value's ID.
pub fn shown_key(field: Uuid, key: &str) -> String {
    if field == tasks::PRIORITY {
        if key == "none" {
            return "no_priority".to_owned();
        }
        if let Some(stored) = key.parse().ok().and_then(tasks::priority_for_option) {
            return stored.to_owned();
        }
    }
    key.to_owned()
}

/// The other way round from [`shown_key`]: the key the compiler groups by.
pub fn stored_key(field: Uuid, key: &str) -> String {
    if field == tasks::PRIORITY {
        if key == "no_priority" {
            return "none".to_owned();
        }
        if let Some(option) = tasks::priority_option(key) {
            return option.to_string();
        }
    }
    key.to_owned()
}

/// Whether a filter leaves room for a group of `field` keyed `key`
/// (`none` for no value; `category` is a status's kind), judging only the
/// clauses on that field that every task must meet. A board keeps an empty
/// column only when a task in it could still be listed. On a field holding
/// several values (`many`: labels, people) a task kept for one value may
/// still sit in another's column, so "is" only rules out "none".
pub fn allows_group(
    filter: &Filter,
    field: Uuid,
    key: &str,
    category: Option<&str>,
    many: bool,
) -> bool {
    fn clause_allows(clause: &FieldClause, key: &str, category: Option<&str>, many: bool) -> bool {
        let none = key == "none";
        match clause.op {
            FieldOp::Is => !none && (many || clause.values.iter().any(|value| value == key)),
            FieldOp::IsNot => none || !clause.values.iter().any(|value| value == key),
            FieldOp::Empty => none,
            FieldOp::NotEmpty => !none,
            FieldOp::CategoryIs => {
                category.is_none_or(|category| clause.values.iter().any(|value| value == category))
            }
            FieldOp::CategoryIsNot => {
                category.is_none_or(|category| !clause.values.iter().any(|value| value == category))
            }
            _ => true,
        }
    }
    fn on_field(filter: &Filter, field: Uuid) -> bool {
        match filter {
            Filter::Clause(clause) => clause.field == field,
            Filter::Or(parts) => {
                !parts.is_empty() && parts.iter().all(|part| on_field(part, field))
            }
            Filter::And(_) => false,
        }
    }
    fn allows(filter: &Filter, key: &str, category: Option<&str>, many: bool) -> bool {
        match filter {
            Filter::Clause(clause) => clause_allows(clause, key, category, many),
            Filter::Or(parts) => parts.iter().any(|part| allows(part, key, category, many)),
            Filter::And(parts) => parts.iter().all(|part| allows(part, key, category, many)),
        }
    }
    let parts = match filter {
        Filter::And(parts) => parts.as_slice(),
        other => std::slice::from_ref(other),
    };
    parts
        .iter()
        .filter(|part| on_field(part, field))
        .all(|part| allows(part, key, category, many))
}

/// A view of the Tasks collection as the list's parameters, the other way
/// round from [`from_params`], so the list page can show its filters in
/// its own controls. None when the view says something the parameters
/// can't (an "or" across fields, a sort the list has no name for).
pub fn to_params(config: &ViewConfig) -> Option<BTreeMap<String, String>> {
    let mut params = BTreeMap::<String, String>::new();
    let mut custom = Vec::<FieldClause>::new();
    let add = |params: &mut BTreeMap<String, String>, key: &str, values: &[String]| {
        let entry = params.entry(key.to_owned()).or_default();
        for value in values {
            if !entry.split(',').any(|known| known == value) {
                if !entry.is_empty() {
                    entry.push(',');
                }
                entry.push_str(value);
            }
        }
    };
    let named = |field: Uuid, values: &[String]| -> Option<Vec<String>> {
        if field == tasks::PRIORITY {
            values
                .iter()
                .map(|value| {
                    value
                        .parse()
                        .ok()
                        .and_then(tasks::priority_for_option)
                        .map(str::to_owned)
                })
                .collect()
        } else {
            Some(values.to_vec())
        }
    };
    // The "any of these, or none" fields and their parameters.
    let either = |field: Uuid| match field {
        tasks::PRIORITY => Some(("priority", "notPriority")),
        tasks::ASSIGNEE => Some(("assigneeActorId", "notAssigneeActorId")),
        tasks::MILESTONE => Some(("milestoneId", "notMilestoneId")),
        tasks::LABELS => Some(("anyLabelIds", "notLabelIds")),
        tasks::PARENT => Some(("parentId", "")),
        _ => None,
    };
    let parts = match &config.filters {
        Filter::And(parts) => parts.clone(),
        other => vec![other.clone()],
    };
    for part in &parts {
        match part {
            Filter::Or(options) => {
                // Only "any of these, or none" on one field: is + empty.
                let [Filter::Clause(first), Filter::Clause(second)] = options.as_slice() else {
                    return None;
                };
                let (is, empty) = match (first.op, second.op) {
                    (FieldOp::Is, FieldOp::Empty) => (first, second),
                    (FieldOp::Empty, FieldOp::Is) => (second, first),
                    _ => return None,
                };
                if is.field != empty.field {
                    return None;
                }
                let (key, _) = either(is.field)?;
                let mut values = named(is.field, &is.values)?;
                values.push("none".to_owned());
                add(&mut params, key, &values);
            }
            Filter::And(_) => return None,
            Filter::Clause(clause) => {
                let field = clause.field;
                let values = &clause.values;
                match (field, clause.op) {
                    (tasks::STATUS, FieldOp::Is) => add(&mut params, "statusId", values),
                    (tasks::STATUS, FieldOp::IsNot) => add(&mut params, "notStatusId", values),
                    (tasks::STATUS, FieldOp::CategoryIs) => {
                        add(&mut params, "semanticCategory", values);
                    }
                    (tasks::STATUS, FieldOp::CategoryIsNot) => {
                        let mut sorted = values.clone();
                        sorted.sort();
                        if sorted == ["canceled", "completed"]
                            && !params.contains_key("includeDone")
                        {
                            params.insert("includeDone".to_owned(), "false".to_owned());
                        } else {
                            add(&mut params, "notSemanticCategory", values);
                        }
                    }
                    (tasks::LABELS, FieldOp::Is) if values.len() == 1 => {
                        add(&mut params, "labelIds", values);
                    }
                    (tasks::TYPE, FieldOp::Is) => add(&mut params, "typeId", values),
                    (tasks::TYPE, FieldOp::IsNot) => add(&mut params, "notTypeId", values),
                    (tasks::BLOCKED | tasks::READY, FieldOp::Is | FieldOp::Empty) => {
                        let key = if field == tasks::BLOCKED {
                            "blocked"
                        } else {
                            "ready"
                        };
                        if params.contains_key(key) {
                            return None;
                        }
                        let on = clause.op == FieldOp::Is && values == &["true".to_owned()];
                        params.insert(key.to_owned(), on.to_string());
                    }
                    (tasks::CLAIMED_BY, FieldOp::NotEmpty | FieldOp::Empty) => {
                        if params.contains_key("claimed") {
                            return None;
                        }
                        let on = clause.op == FieldOp::NotEmpty;
                        params.insert("claimed".to_owned(), on.to_string());
                    }
                    // "Due by" and "due on or after" take the day itself.
                    (tasks::DUE_DATE, FieldOp::Lte | FieldOp::Gte) if values.len() == 1 => {
                        let key = if clause.op == FieldOp::Lte {
                            "dueBefore"
                        } else {
                            "dueAfter"
                        };
                        if params.contains_key(key) {
                            return None;
                        }
                        params.insert(key.to_owned(), values[0].clone());
                    }
                    (tasks::DUE_DATE, FieldOp::Within) if values == &["overdue".to_owned()] => {
                        params.insert("overdue".to_owned(), "true".to_owned());
                    }
                    (field, op) if either(field).is_some() => {
                        let (is, not) = either(field)?;
                        match op {
                            FieldOp::Is => add(&mut params, is, &named(field, values)?),
                            FieldOp::Empty => add(&mut params, is, &["none".to_owned()]),
                            FieldOp::IsNot if !not.is_empty() => {
                                add(&mut params, not, &named(field, values)?);
                            }
                            FieldOp::NotEmpty if !not.is_empty() => {
                                add(&mut params, not, &["none".to_owned()]);
                            }
                            _ => return None,
                        }
                    }
                    _ if tasks::is_builtin_field(field) || builtin_system(field) => return None,
                    _ => custom.push(clause.clone()),
                }
            }
        }
    }
    // Two clauses that read as one parameter ("is a" and "is b" on one
    // field) mean something else together: refuse rather than widen.
    if params.values().any(String::is_empty) {
        return None;
    }
    // "overdue" says "not done" already.
    if params.get("overdue").is_some_and(|on| on == "true")
        && params.get("includeDone").is_some_and(|on| on == "false")
    {
        params.remove("includeDone");
    }
    if !custom.is_empty() {
        params.insert(
            "fieldFilters".to_owned(),
            serde_json::to_string(&custom).ok()?,
        );
    }
    if !config.sort.is_empty() {
        let name = SORT_NAMES
            .iter()
            .find(|name| sorts(name).as_deref() == Some(config.sort.as_slice()))
            .map(|name| (*name).to_owned())
            .or_else(|| {
                let [first, second] = config.sort.as_slice() else {
                    return None;
                };
                let id = first.field.parse::<Uuid>().ok()?;
                let named = match first.direction {
                    Direction::Desc => format!("field:{id}:desc"),
                    Direction::Asc => format!("field:{id}"),
                };
                (sorts(&named)? == [first.clone(), second.clone()]).then_some(named)
            })?;
        params.insert("sort".to_owned(), name);
    }
    if let Some(field) = config.group_by {
        params.insert("groupBy".to_owned(), grouping_name(field));
    }
    // Round trip: what reads back must be the view itself.
    let back = from_params(&params).ok()?;
    (canonical(&back.config.filters) == canonical(&config.filters)).then_some(params)
}

/// A filter written so that the order of its parts doesn't matter.
fn canonical(filter: &Filter) -> String {
    let joined = |parts: &[Filter]| {
        let mut parts = parts.iter().map(canonical).collect::<Vec<_>>();
        parts.sort();
        parts.dedup();
        parts.join(",")
    };
    match filter {
        Filter::Clause(clause) => serde_json::to_string(clause).unwrap_or_default(),
        Filter::And(parts) if parts.len() == 1 => canonical(&parts[0]),
        Filter::And(parts) => format!("and[{}]", joined(parts)),
        Filter::Or(parts) => format!("or[{}]", joined(parts)),
    }
}

/// The sorts the list names.
const SORT_NAMES: [&str; 7] = [
    "newest", "oldest", "updated", "priority", "due", "title", "manual",
];

/// The built-in task fields beyond the six every task collection lists.
fn builtin_system(field: Uuid) -> bool {
    [
        tasks::TYPE,
        tasks::MILESTONE,
        tasks::PARENT,
        tasks::BLOCKED,
        tasks::READY,
        tasks::CLAIMED_BY,
    ]
    .contains(&field)
}

/// A space's Tasks collection, if it has one yet.
pub async fn tasks_collection(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections WHERE project_id = $1 AND builtin = 'tasks'",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A view of the space's Tasks collection that `reader` may use: a shared
/// one, or their own.
pub async fn usable_view(
    connection: &mut PgConnection,
    project_id: Uuid,
    view_id: Uuid,
    reader: Uuid,
) -> Result<Option<views::View>, AppError> {
    let Some(collection_id) = tasks_collection(connection, project_id).await? else {
        return Ok(None);
    };
    Ok(views::one(connection, project_id, collection_id, view_id)
        .await?
        .filter(|view| view.owner_actor_id.is_none_or(|owner| owner == reader)))
}

/// Checks a query against the Tasks collection's fields: each filter suits
/// its field, and the ids it names are this space's. The message says what
/// would work.
pub async fn check(
    connection: &mut PgConnection,
    project_id: Uuid,
    query: &TaskQuery,
) -> Result<Result<(), String>, AppError> {
    let Some(collection_id) = tasks_collection(connection, project_id).await? else {
        return Ok(Ok(()));
    };
    let definitions = super::view_fields(connection, project_id, collection_id, false).await?;
    if let Err(message) = views::check(&query.config, &definitions) {
        return Ok(Err(message));
    }
    views::check_references(connection, project_id, &query.config, &definitions).await
}

/// The tasks a query finds as `reader` reads it ("me" is them), oldest
/// first, at most `limit`; with `only`, just those of them. Automations'
/// conditions read tasks this way.
pub async fn matching(
    connection: &mut PgConnection,
    project_id: Uuid,
    query: &TaskQuery,
    reader: Uuid,
    only: Option<&[Uuid]>,
    limit: i64,
) -> Result<Vec<Uuid>, AppError> {
    let Some(collection_id) = tasks_collection(connection, project_id).await? else {
        return Ok(Vec::new());
    };
    let definitions = super::view_fields(connection, project_id, collection_id, false).await?;
    let config = ViewConfig {
        filters: query.config.filters.clone(),
        sort: vec![SortRule {
            field: "number".to_owned(),
            direction: Direction::Asc,
        }],
        ..ViewConfig::default()
    };
    let words = query
        .search
        .as_deref()
        .map(|words| words.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let mut spec = Spec::from_config(&config, &definitions, reader).searching(words, &definitions);
    spec.archived = query.archived;
    spec.only = only.map(<[Uuid]>::to_vec);
    // Every task in the space, a task-like collection's too, as the list
    // reads them.
    let source = tasks::TasksAcross {
        projects: vec![project_id],
        collection: None,
    };
    let page = query::page(connection, &source, &spec, None, limit)
        .await?
        .map_err(|()| AppError::Internal("matching tasks cursor"))?;
    Ok(page.rows.into_iter().map(|(id, _)| id).collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{from_params, tasks};
    use crate::views::{filter::Filter, query::Archived};

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn my_tasks_reads_as_fields() {
        let query = from_params(&params(&[
            ("assigneeActorId", "me"),
            ("includeDone", "false"),
            ("sort", "due"),
        ]))
        .expect("reads");
        let json = serde_json::to_value(&query.config.filters).expect("json");
        let text = json.to_string();
        assert!(text.contains(&tasks::ASSIGNEE.to_string()), "{text}");
        assert!(text.contains("\"me\""), "{text}");
        assert!(text.contains("categoryIsNot"), "{text}");
        assert_eq!(query.config.sort[0].field, tasks::DUE_DATE.to_string());
        assert_eq!(query.archived, Archived::default());
    }

    #[test]
    fn overdue_leaves_out_finished_tasks() {
        for pairs in [
            &[("overdue", "true")][..],
            &[("overdue", "true"), ("includeDone", "false")],
        ] {
            let query = from_params(&params(pairs)).expect("reads");
            let Filter::And(parts) = &query.config.filters else {
                panic!("{:?}", query.config.filters);
            };
            assert_eq!(parts.len(), 2, "{pairs:?}: {parts:?}");
            assert!(parts.contains(&super::finished_excluded()), "{parts:?}");
            assert_eq!(
                super::to_params(&query.config),
                Some(params(&[("overdue", "true")]))
            );
        }
    }

    #[test]
    fn none_and_values_read_as_either() {
        let query = from_params(&params(&[("priority", "urgent,none")])).expect("reads");
        assert!(
            matches!(&query.config.filters, Filter::And(parts) if matches!(parts.as_slice(), [Filter::Or(_)])),
            "{:?}",
            query.config.filters
        );
    }

    #[test]
    fn list_parameters_read_back_as_themselves() {
        for pairs in [
            &[
                ("assigneeActorId", "me"),
                ("includeDone", "false"),
                ("sort", "due"),
            ][..],
            &[
                ("priority", "urgent,none"),
                ("labelIds", "0192f5c0-0000-7000-8000-0000000000a1"),
            ],
            &[
                ("blocked", "true"),
                ("notSemanticCategory", "canceled"),
                ("groupBy", "assignee"),
            ],
            &[
                ("claimed", "false"),
                ("ready", "true"),
                ("sort", "priority"),
            ],
            &[
                ("dueBefore", "2026-12-01"),
                ("overdue", "true"),
                ("milestoneId", "none"),
            ],
        ] {
            let asked = params(pairs);
            let query = from_params(&asked).expect("reads");
            assert_eq!(super::to_params(&query.config), Some(asked), "{pairs:?}");
        }
    }

    #[test]
    fn a_view_the_list_cant_say_has_no_parameters() {
        let either_field = crate::views::definition::ViewConfig {
            filters: Filter::Or(vec![
                Filter::Clause(crate::fields::FieldClause {
                    field: tasks::STATUS,
                    op: crate::fields::FieldOp::Is,
                    values: vec![uuid::Uuid::nil().to_string()],
                }),
                Filter::Clause(crate::fields::FieldClause {
                    field: tasks::ASSIGNEE,
                    op: crate::fields::FieldOp::Empty,
                    values: Vec::new(),
                }),
            ]),
            ..Default::default()
        };
        assert_eq!(super::to_params(&either_field), None);
    }

    #[test]
    fn priority_groups_are_keyed_by_name_on_task_screens() {
        let urgent = tasks::priority_option("urgent")
            .expect("urgent")
            .to_string();
        assert_eq!(super::shown_key(tasks::PRIORITY, &urgent), "urgent");
        assert_eq!(super::stored_key(tasks::PRIORITY, "urgent"), urgent);
        assert_eq!(super::shown_key(tasks::PRIORITY, "none"), "no_priority");
        assert_eq!(super::stored_key(tasks::PRIORITY, "no_priority"), "none");
        assert_eq!(super::shown_key(tasks::STATUS, &urgent), urgent);
    }

    #[test]
    fn a_status_filter_keeps_only_its_columns() {
        let query = from_params(&params(&[("includeDone", "false")])).expect("reads");
        let filter = &query.config.filters;
        assert!(super::allows_group(
            filter,
            tasks::STATUS,
            "a",
            Some("started"),
            false
        ));
        assert!(!super::allows_group(
            filter,
            tasks::STATUS,
            "b",
            Some("completed"),
            false
        ));
        let query = from_params(&params(&[("assigneeActorId", "none")])).expect("reads");
        assert!(super::allows_group(
            &query.config.filters,
            tasks::ASSIGNEE,
            "none",
            None,
            false
        ));
        assert!(!super::allows_group(
            &query.config.filters,
            tasks::ASSIGNEE,
            "x",
            None,
            false
        ));
        assert!(super::allows_group(
            &query.config.filters,
            tasks::STATUS,
            "x",
            None,
            false
        ));
    }

    #[test]
    fn unknown_parameters_are_refused() {
        assert_eq!(
            from_params(&params(&[("statusIds", "x")])).map_err(|(key, _)| key),
            Err("query")
        );
        assert_eq!(
            from_params(&params(&[("priority", "soon")])).map_err(|(key, _)| key),
            Err("priority")
        );
    }
}
