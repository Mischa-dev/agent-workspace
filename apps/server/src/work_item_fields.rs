//! Custom fields on tasks (AWD-89): the task side of `crate::fields`.
//!
//! The field types, their checks, filters and sorts live in `crate::fields`
//! and know nothing about tasks. This module adds what does: a task's values
//! live in `work_items.field_values`; a person must be someone in the
//! task's project who can hold tasks (the assignee rule); a link names a
//! task or page in the same project. The web app, MCP, imports, templates,
//! automations and `task_move` all go through `resolve`, and every change
//! goes through `work_item_commands::update_locked_work_item`, so a task
//! gets one version bump and one history entry with each field's before
//! and after.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    error::AppError,
    fields::{self, FieldDefinition, FieldType, LinkTarget, Normalized, RecordKind},
    work_item_commands::validate_assignee,
};

pub const KIND: RecordKind = RecordKind::Task;
/// Field values one change can set.
pub const MAX_VALUES_PER_CHANGE: usize = 50;
/// The most a task's values may take, as JSON, under the column's 64 KB limit.
pub const MAX_VALUES_BYTES: usize = 60_000;

/// The project's task fields.
pub async fn definitions(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    fields::list(executor, project_id, KIND, include_archived).await
}

/// One field's new value, checked and resolved.
#[derive(Clone, Debug)]
pub struct FieldValueChange {
    pub field: FieldDefinition,
    /// The value as stored, or None to clear it.
    pub value: Option<Value>,
    /// How it reads in history: option names, a person's name, a task's key
    /// or a page's title.
    pub shown: Value,
}

/// Values to set on one task, each field once.
#[derive(Clone, Debug, Default)]
pub struct FieldValueChanges {
    pub entries: Vec<FieldValueChange>,
}

impl FieldValueChanges {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `current` with these changes made.
    pub fn applied_to(&self, current: &Value) -> Value {
        let mut values = current.as_object().cloned().unwrap_or_default();
        for entry in &self.entries {
            let key = entry.field.id.to_string();
            match &entry.value {
                Some(value) => {
                    values.insert(key, value.clone());
                }
                None => {
                    values.remove(&key);
                }
            }
        }
        Value::Object(values)
    }

    /// A new task's values.
    pub fn initial(&self) -> Value {
        self.applied_to(&Value::Object(Map::new()))
    }
}

/// Why a written value was refused: the argument (such as `fields.Risk`) and
/// what would work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueProblem {
    pub field: String,
    pub message: String,
}

fn value_problem(argument: &str, message: impl Into<String>) -> ValueProblem {
    ValueProblem {
        field: argument.to_owned(),
        message: message.into(),
    }
}

/// Who `me` is, for a person field, and what the refusals call the project.
#[derive(Clone, Copy, Debug)]
pub struct Writer<'a> {
    pub actor_id: Uuid,
    pub project_key: &'a str,
}

/// Turns values written by field name or ID into checked, stored values.
/// `argument` names the argument for refusals ("fields" for MCP,
/// "fieldValues" for the web app). An archived or unknown field, a value
/// of the wrong type, a person who can't hold tasks in the project or a
/// link outside it is refused, and nothing is set.
pub async fn resolve(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    definitions: &[FieldDefinition],
    argument: &str,
    written: &[(String, Value)],
) -> Result<Result<FieldValueChanges, ValueProblem>, AppError> {
    if written.len() > MAX_VALUES_PER_CHANGE {
        return Ok(Err(value_problem(
            argument,
            format!("Set at most {MAX_VALUES_PER_CHANGE} fields at a time."),
        )));
    }
    let mut changes = FieldValueChanges::default();
    let mut seen = HashSet::new();
    for (reference, input) in written {
        let Some(field) = fields::find(definitions, reference) else {
            return Ok(Err(value_problem(
                argument,
                format!(
                    "{} has no task field called \"{}\". Its fields: {}. See field_list.",
                    writer.project_key,
                    crate::work_item_labels::echo(reference),
                    fields::active_names(definitions)
                ),
            )));
        };
        let at = format!("{argument}.{}", field.name);
        if field.archived {
            return Ok(Err(value_problem(
                &at,
                format!(
                    "\"{}\" is archived in {}. A project admin can restore it.",
                    field.name, writer.project_key
                ),
            )));
        }
        if !seen.insert(field.id) {
            return Ok(Err(value_problem(
                &at,
                format!("\"{}\" is set twice. Set each field once.", field.name),
            )));
        }
        let normalized = match fields::normalize_value(field, input) {
            Ok(normalized) => normalized,
            Err(message) => return Ok(Err(value_problem(&at, message))),
        };
        let entry = match normalized {
            Normalized::Clear => FieldValueChange {
                field: field.clone(),
                value: None,
                shown: Value::Null,
            },
            Normalized::Value(value) => FieldValueChange {
                field: field.clone(),
                shown: fields::display_value(field, &value, &|_| None),
                value: Some(value),
            },
            Normalized::Person(written) => {
                match find_person(connection, project_id, writer, &written).await? {
                    Some((id, name)) => FieldValueChange {
                        field: field.clone(),
                        value: Some(Value::String(id.to_string())),
                        shown: Value::String(name),
                    },
                    None => {
                        return Ok(Err(value_problem(
                            &at,
                            format!(
                                "\"{}\" takes someone in {} who can hold tasks: \"me\", an @handle or an ID. No one like that is called {}.",
                                field.name,
                                writer.project_key,
                                crate::work_item_labels::echo(&written)
                            ),
                        )));
                    }
                }
            }
            Normalized::Link(written) => {
                let target = field.config.target.unwrap_or(LinkTarget::Task);
                match find_link(connection, project_id, writer, target, &written).await? {
                    Some((id, label)) => FieldValueChange {
                        field: field.clone(),
                        value: Some(Value::String(id.to_string())),
                        shown: Value::String(label),
                    },
                    None => {
                        let what = match target {
                            LinkTarget::Task => "a task",
                            LinkTarget::Page => "a page",
                        };
                        return Ok(Err(value_problem(
                            &at,
                            format!(
                                "\"{}\" links to {what} in {}, by ID, link{}. {} isn't one.",
                                field.name,
                                writer.project_key,
                                if target == LinkTarget::Task {
                                    " or key (such as AWD-12)"
                                } else {
                                    ""
                                },
                                crate::work_item_labels::echo(&written)
                            ),
                        )));
                    }
                }
            }
        };
        changes.entries.push(entry);
    }
    Ok(Ok(changes))
}

pub async fn find_person(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    written: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    let written = written.trim();
    let id = if written.eq_ignore_ascii_case("me") {
        Some(writer.actor_id)
    } else {
        written.parse::<Uuid>().ok()
    };
    let found = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT actor_id, display_name FROM project_memberships
        WHERE project_id = $1 AND enabled
          AND (actor_id = $2 OR ($2 IS NULL AND lower(handle) = lower($3)))
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(id)
    .bind(written.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?;
    let Some((actor_id, name)) = found else {
        return Ok(None);
    };
    if !validate_assignee(connection, project_id, Some(actor_id)).await? {
        return Ok(None);
    }
    Ok(Some((actor_id, name)))
}

/// Someone in the project as a filter names them ("me", an @handle or an
/// ID), whether or not they can still hold tasks: a filter finds values set
/// before someone left.
pub async fn member_id(
    connection: &mut PgConnection,
    project_id: Uuid,
    me: Uuid,
    written: &str,
) -> Result<Option<Uuid>, AppError> {
    let written = written.trim();
    let id = if written.eq_ignore_ascii_case("me") {
        Some(me)
    } else {
        written.parse::<Uuid>().ok()
    };
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor_id FROM project_memberships
        WHERE project_id = $1
          AND (actor_id = $2 OR ($2 IS NULL AND lower(handle) = lower($3)))
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(id)
    .bind(written.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?)
}

/// The first UUID in what someone wrote: an ID, or a link that holds one.
fn uuid_in(written: &str) -> Option<Uuid> {
    if let Ok(id) = written.trim().parse::<Uuid>() {
        return Some(id);
    }
    // A link's last ID is the record's; the ones before name its project.
    written
        .split(|character: char| !(character.is_ascii_hexdigit() || character == '-'))
        .rev()
        .find_map(|part| {
            (part.len() == 36)
                .then(|| part.parse::<Uuid>().ok())
                .flatten()
        })
}

/// A task key such as AWD-12, as (key, number).
fn task_key(written: &str) -> Option<(&str, i64)> {
    let (key, number) = written.trim().trim_start_matches('#').rsplit_once('-')?;
    let number = number.parse::<i64>().ok().filter(|number| *number > 0)?;
    (!key.is_empty()).then_some((key, number))
}

pub async fn find_link(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    target: LinkTarget,
    written: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    match target {
        LinkTarget::Task => {
            let (id, number) = match (uuid_in(written), task_key(written)) {
                (Some(id), _) => (Some(id), None),
                (None, Some((key, number))) if key.eq_ignore_ascii_case(writer.project_key) => {
                    (None, Some(number))
                }
                _ => return Ok(None),
            };
            Ok(sqlx::query_as::<_, (Uuid, String)>(
                r#"
                SELECT item.id, project.project_key || '-' || item.number
                FROM work_items item JOIN projects project ON project.id = item.project_id
                WHERE item.project_id = $1 AND (item.id = $2 OR item.number = $3)
                LIMIT 1
                "#,
            )
            .bind(project_id)
            .bind(id)
            .bind(number)
            .fetch_optional(&mut *connection)
            .await?)
        }
        LinkTarget::Page => {
            let Some(id) = uuid_in(written) else {
                return Ok(None);
            };
            Ok(sqlx::query_as::<_, (Uuid, String)>(
                "SELECT id, title FROM project_pages WHERE project_id = $1 AND id = $2 AND archived_at IS NULL",
            )
            .bind(project_id)
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?)
        }
    }
}

// ---------------------------------------------------------------------------
// Reading values

/// What a person or linked record in a task's values is called.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueReference {
    pub id: Uuid,
    /// "person", "task" or "page".
    pub kind: &'static str,
    /// A person's name, a task's title or a page's title.
    pub label: String,
    /// A task's key such as AWD-12; a person's @handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The people, tasks and pages that `values` (each a task's field_values)
/// name, among the project's. Anything that no longer exists is left out.
pub async fn references<'a>(
    executor: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    values: impl IntoIterator<Item = &'a Value>,
) -> Result<HashMap<Uuid, ValueReference>, AppError> {
    let mut people = HashSet::new();
    let mut tasks = HashSet::new();
    let mut pages = HashSet::new();
    let by_id = definitions
        .iter()
        .map(|field| (field.id.to_string(), field))
        .collect::<HashMap<_, _>>();
    for object in values {
        let Some(object) = object.as_object() else {
            continue;
        };
        for (key, value) in object {
            let Some(field) = by_id.get(key) else {
                continue;
            };
            let Some(id) = fields::referenced_id(field, value) else {
                continue;
            };
            match (field.field_type, field.config.target) {
                (FieldType::Person, _) => people.insert(id),
                (FieldType::Link, Some(LinkTarget::Page)) => pages.insert(id),
                _ => tasks.insert(id),
            };
        }
    }
    let mut found = HashMap::new();
    if !people.is_empty() {
        let ids = people.into_iter().collect::<Vec<_>>();
        for (id, label, handle) in sqlx::query_as::<_, (Uuid, String, Option<String>)>(
            "SELECT actor_id, display_name, handle FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)",
        )
        .bind(project_id)
        .bind(&ids)
        .fetch_all(&mut *executor)
        .await?
        {
            found.insert(
                id,
                ValueReference {
                    id,
                    kind: "person",
                    label,
                    key: handle.map(|handle| format!("@{handle}")),
                },
            );
        }
    }
    if !tasks.is_empty() {
        let ids = tasks.into_iter().collect::<Vec<_>>();
        for (id, label, key) in sqlx::query_as::<_, (Uuid, String, String)>(
            r#"
            SELECT item.id, item.title, project.project_key || '-' || item.number
            FROM work_items item JOIN projects project ON project.id = item.project_id
            WHERE item.project_id = $1 AND item.id = ANY($2)
            "#,
        )
        .bind(project_id)
        .bind(&ids)
        .fetch_all(&mut *executor)
        .await?
        {
            found.insert(
                id,
                ValueReference {
                    id,
                    kind: "task",
                    label,
                    key: Some(key),
                },
            );
        }
    }
    if !pages.is_empty() {
        let ids = pages.into_iter().collect::<Vec<_>>();
        for (id, label) in sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, title FROM project_pages WHERE project_id = $1 AND id = ANY($2) AND archived_at IS NULL",
        )
        .bind(project_id)
        .bind(&ids)
        .fetch_all(&mut *executor)
        .await?
        {
            found.insert(
                id,
                ValueReference {
                    id,
                    kind: "page",
                    label,
                    key: None,
                },
            );
        }
    }
    Ok(found)
}

/// What a person, task or page reference reads as for agents and exports:
/// a person's @handle (or name), a task's key, a page's title.
pub fn reference_text(references: &HashMap<Uuid, ValueReference>, id: &str) -> Option<String> {
    let reference = references.get(&id.parse::<Uuid>().ok()?)?;
    Some(match reference.kind {
        "person" => reference
            .key
            .clone()
            .unwrap_or_else(|| reference.label.clone()),
        "task" => reference
            .key
            .clone()
            .unwrap_or_else(|| reference.label.clone()),
        _ => reference.label.clone(),
    })
}

/// A task's values by field name, as agents and exports read them: only
/// active fields, in the project's order, each value as `display_value`
/// reads it (option names, @handles, task keys, page titles).
pub fn named_values(
    definitions: &[FieldDefinition],
    values: &Value,
    references: &HashMap<Uuid, ValueReference>,
) -> Vec<(FieldDefinition, Value)> {
    let Some(object) = values.as_object() else {
        return Vec::new();
    };
    definitions
        .iter()
        .filter(|field| !field.archived)
        .filter_map(|field| {
            let value = object.get(&field.id.to_string())?;
            let shown = fields::display_value(field, value, &|id| reference_text(references, id));
            (!shown.is_null()).then(|| (field.clone(), shown))
        })
        .collect()
}

/// How a stored value read before a change, for history: one lookup for a
/// person or link, none for anything else.
pub async fn shown_before(
    connection: &mut PgConnection,
    project_id: Uuid,
    field: &FieldDefinition,
    value: Option<&Value>,
) -> Result<Value, AppError> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    if fields::referenced_id(field, value).is_none() {
        return Ok(fields::display_value(field, value, &|_| None));
    }
    let wrapper = Value::Object(Map::from_iter([(field.id.to_string(), value.clone())]));
    let found = references(
        connection,
        project_id,
        std::slice::from_ref(field),
        [&wrapper],
    )
    .await?;
    Ok(fields::display_value(field, value, &|id| {
        let reference = found.get(&id.parse::<Uuid>().ok()?)?;
        Some(match reference.kind {
            "task" => reference
                .key
                .clone()
                .unwrap_or_else(|| reference.label.clone()),
            _ => reference.label.clone(),
        })
    }))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{task_key, uuid_in};

    #[test]
    fn links_are_read_from_ids_links_and_keys() {
        let id = Uuid::now_v7();
        assert_eq!(uuid_in(&id.to_string()), Some(id));
        assert_eq!(
            uuid_in(&format!(
                "https://example.com/projects/{}/work-items/{id}",
                Uuid::now_v7()
            )),
            Some(id)
        );
        assert_eq!(uuid_in("AWD-12"), None);
        assert_eq!(task_key("AWD-12"), Some(("AWD", 12)));
        assert_eq!(task_key("#AWD-12"), Some(("AWD", 12)));
        assert_eq!(task_key("AWD-0"), None);
        assert_eq!(task_key("12"), None);
    }
}
