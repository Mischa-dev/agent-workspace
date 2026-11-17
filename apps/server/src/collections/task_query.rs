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
    for part in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
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
        let id = id.parse::<Uuid>().ok()?;
        return Some(vec![
            rule(field(id), Direction::Asc),
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
    let mut query = TaskQuery::default();
    for (key, value) in params {
        let value = value.trim();
        match key.as_str() {
            "q" => {
                query.search = (!value.is_empty()).then(|| value.to_owned());
            }
            "statusId" => {
                let (values, _) = parts(value);
                if !values.is_empty() {
                    all.push(clause(tasks::STATUS, FieldOp::Is, ids("statusId", &values, false)?));
                }
            }
            "notStatusId" => {
                let (values, _) = parts(value);
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
                    all.push(clause(
                        tasks::STATUS,
                        FieldOp::CategoryIsNot,
                        vec!["completed".to_owned(), "canceled".to_owned()],
                    ));
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
                let (values, _) = parts(value);
                if !values.is_empty() {
                    all.push(clause(tasks::TYPE, FieldOp::Is, ids("typeId", &values, false)?));
                }
            }
            "notTypeId" => {
                let (values, _) = parts(value);
                if !values.is_empty() {
                    all.push(clause(tasks::TYPE, FieldOp::IsNot, ids("notTypeId", &values, false)?));
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
                FieldOp::Lt,
                vec![date("dueBefore", value)?],
            )),
            "dueAfter" => all.push(clause(
                tasks::DUE_DATE,
                FieldOp::Gt,
                vec![date("dueAfter", value)?],
            )),
            "overdue" => {
                if flag("overdue", value)? {
                    all.push(clause(tasks::DUE_DATE, FieldOp::Within, vec!["overdue".to_owned()]));
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
                return Err(problem("query", format!("\"{key}\" isn't a task list filter.")));
            }
        }
    }
    query.config.filters = Filter::And(all);
    Ok(query)
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
    let definitions =
        super::field_definitions(connection, project_id, collection_id, false).await?;
    if let Err(message) = views::check(&query.config, &definitions) {
        return Ok(Err(message));
    }
    views::check_references(connection, project_id, &query.config, &definitions).await
}

/// The tasks a filter finds as `reader` reads it ("me" is them), oldest
/// first, at most `limit`; with `only`, just those of them. Automations'
/// conditions read tasks this way.
pub async fn matching(
    connection: &mut PgConnection,
    project_id: Uuid,
    filter: &Filter,
    reader: Uuid,
    only: Option<&[Uuid]>,
    limit: i64,
) -> Result<Vec<Uuid>, AppError> {
    let Some(collection_id) = tasks_collection(connection, project_id).await? else {
        return Ok(Vec::new());
    };
    let definitions =
        super::field_definitions(connection, project_id, collection_id, false).await?;
    let config = ViewConfig {
        filters: filter.clone(),
        sort: vec![SortRule {
            field: "number".to_owned(),
            direction: Direction::Asc,
        }],
        ..ViewConfig::default()
    };
    let mut spec = Spec::from_config(&config, &definitions, reader);
    spec.only = only.map(<[Uuid]>::to_vec);
    let source = tasks::TasksSource {
        project_id,
        collection_id,
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
    fn none_and_values_read_as_either() {
        let query = from_params(&params(&[("priority", "urgent,none")])).expect("reads");
        assert!(matches!(query.config.filters, Filter::Or(_)), "{:?}", query.config.filters);
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
