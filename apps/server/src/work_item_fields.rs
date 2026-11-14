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
/// The most a task's values may take as PostgreSQL stores them
/// (`work_item_history::jsonb_text_len`: numbers in full, spaces after
/// ":" and ","), under the column's 64 KB limit.
pub const MAX_VALUES_BYTES: usize = 60_000;

/// Whether a task's values fit `MAX_VALUES_BYTES` as PostgreSQL stores them.
pub fn values_fit(values: &Value) -> bool {
    crate::work_item_history::jsonb_text_len(values) <= MAX_VALUES_BYTES
}

/// What a task's values too large together are refused with.
pub const TOO_LARGE: &str =
    "These field values are too large together. Shorten the longest ones and try again.";

/// The refusal for values that don't fit `MAX_VALUES_BYTES`: the web's
/// `fieldValues` (MCP's `fields`) with `TOO_LARGE`.
pub fn too_large() -> AppError {
    AppError::Refused {
        field: "fieldValues",
        message: TOO_LARGE,
    }
}

/// The project's task fields.
pub async fn definitions(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    fields::list(executor, project_id, KIND, include_archived).await
}

/// Whose fields a task's values follow: the project's task fields, or the
/// fields of the collection that acts like a task it was made in
/// (`work_items.collection_id`, AWD-110).
pub fn kind_of(collection_id: Option<Uuid>) -> RecordKind {
    collection_id.map_or(KIND, RecordKind::Collection)
}

/// The collection that acts like a task a task was made in (None for the
/// project's Tasks collection, or when there's no such task).
pub async fn task_collection(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT collection_id FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_optional(executor)
    .await?
    .flatten())
}

/// The fields one task's values follow, with `plain` (the project's task
/// fields, already read) for a plain task, so a bulk change reads each
/// collection's only for its own tasks.
pub async fn definitions_for_task_or(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
    plain: &[FieldDefinition],
) -> Result<Vec<FieldDefinition>, AppError> {
    match task_collection(&mut *connection, project_id, task_id).await? {
        Some(collection) => {
            fields::list(
                &mut *connection,
                project_id,
                RecordKind::Collection(collection),
                true,
            )
            .await
        }
        None => Ok(plain.to_vec()),
    }
}

/// The fields one task's values follow (see `kind_of`).
pub async fn definitions_for_task(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    let collection = task_collection(&mut *connection, project_id, task_id).await?;
    fields::list(
        &mut *connection,
        project_id,
        kind_of(collection),
        include_archived,
    )
    .await
}

/// Every field a task of the project may have values for: the task fields,
/// then those of each collection that acts like a task. Field IDs are
/// unique, so a task's values read against these whatever its collection.
pub async fn all_task_definitions(
    connection: &mut PgConnection,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    let mut found = definitions(&mut *connection, project_id, include_archived).await?;
    let collections = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections \
         WHERE project_id = $1 AND acts_like_task AND builtin IS NULL ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    for collection in collections {
        found.extend(
            fields::list(
                &mut *connection,
                project_id,
                RecordKind::Collection(collection),
                include_archived,
            )
            .await?,
        );
    }
    Ok(found)
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
    resolve_for(
        connection,
        project_id,
        writer,
        definitions,
        argument,
        written,
        None,
    )
    .await
}

/// A task's stored values (`field_values`), for `resolve_for`. None when no
/// such task is in the project; the write that follows refuses it then.
pub async fn current_values(
    connection: &mut PgConnection,
    project_id: Uuid,
    task: Uuid,
) -> Result<Option<Value>, AppError> {
    Ok(sqlx::query_scalar::<_, Value>(
        "SELECT field_values FROM task_rows WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(task)
    .fetch_optional(&mut *connection)
    .await?)
}

/// `resolve` for an existing task whose values are `current` (its
/// `field_values`): an archived choice option it already holds may stay
/// chosen while the rest of the selection changes.
pub async fn resolve_for(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    definitions: &[FieldDefinition],
    argument: &str,
    written: &[(String, Value)],
    current: Option<&Value>,
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
        let kept = current.and_then(|values| values.get(field.id.to_string()));
        let normalized = match fields::normalize_value_keeping(field, input, kept) {
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
                match find_link(connection, project_id, writer, field, &written).await? {
                    Some((id, label)) => FieldValueChange {
                        field: field.clone(),
                        value: Some(Value::String(id.to_string())),
                        shown: Value::String(label),
                    },
                    None => {
                        let what = match target {
                            LinkTarget::Task => "a task",
                            LinkTarget::Page => "a page",
                            LinkTarget::Record => "a record of its collection",
                        };
                        return Ok(Err(value_problem(
                            &at,
                            format!(
                                "\"{}\" links to {what} in {}, by ID, link{}. {} isn't one.",
                                field.name,
                                writer.project_key,
                                match target {
                                    LinkTarget::Task => " or key (such as AWD-12)",
                                    LinkTarget::Record => ", #number or title",
                                    LinkTarget::Page => "",
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
    field: &FieldDefinition,
    written: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    match field.config.target.unwrap_or(LinkTarget::Task) {
        // A record of the field's collection (AWD-110): its ID or link, its
        // #number, or its title (the oldest, when two share one).
        LinkTarget::Record => {
            let Some(collection) = field.config.collection else {
                return Ok(None);
            };
            let written = written.trim();
            let number = written
                .strip_prefix('#')
                .and_then(|number| number.parse::<i64>().ok());
            Ok(sqlx::query_as::<_, (Uuid, String)>(
                r#"
                SELECT record.id,
                       CASE WHEN record.title = '' THEN 'Untitled #' || record.number::text
                            ELSE record.title END
                FROM collection_records record
                WHERE record.project_id = $1 AND record.collection_id = $2
                  AND record.archived_at IS NULL
                  AND (record.id = $3 OR record.number = $4
                       OR ($3 IS NULL AND $4 IS NULL AND lower(record.title) = lower($5)))
                ORDER BY record.id = $3 DESC, record.number = $4 DESC, record.number
                LIMIT 1
                "#,
            )
            .bind(project_id)
            .bind(collection)
            .bind(uuid_in(written))
            .bind(number)
            .bind(written)
            .fetch_optional(&mut *connection)
            .await?)
        }
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
                FROM task_rows item JOIN projects project ON project.id = item.project_id
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
    /// "person", "task", "page" or "record".
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
    let mut records = HashSet::new();
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
            match (field.field_type.behaves_as(), field.config.target) {
                (FieldType::Person, _) => people.insert(id),
                (FieldType::Link, Some(LinkTarget::Page)) => pages.insert(id),
                (FieldType::Link, Some(LinkTarget::Record)) => records.insert(id),
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
            FROM task_rows item JOIN projects project ON project.id = item.project_id
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
    if !records.is_empty() {
        let ids = records.into_iter().collect::<Vec<_>>();
        for (id, label, number) in sqlx::query_as::<_, (Uuid, String, i64)>(
            r#"
            SELECT record.id,
                   CASE WHEN record.title = '' THEN 'Untitled #' || record.number::text
                        ELSE record.title END,
                   record.number
            FROM collection_records record
            WHERE record.project_id = $1 AND record.id = ANY($2) AND record.archived_at IS NULL
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
                    kind: "record",
                    label,
                    key: Some(format!("#{number}")),
                },
            );
        }
    }
    Ok(found)
}

/// A page link as an export writes it: the page's title, then its link, so
/// importing the file into the same project finds the page again (`find_link`
/// reads the last ID in a link).
pub fn exported_page(project_id: Uuid, page_id: Uuid, title: &str) -> String {
    format!("{title} (/projects/{project_id}/pages/{page_id})")
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

    use super::{exported_page, task_key, uuid_in};

    /// An exported page link reads back as the same page, even when its
    /// title holds an ID of its own.
    #[test]
    fn exported_page_links_import_as_the_same_page() {
        let (project, page) = (Uuid::now_v7(), Uuid::now_v7());
        let title = format!("Notes on {}", Uuid::now_v7());
        let written = exported_page(project, page, &title);
        assert!(written.starts_with(&title));
        assert_eq!(uuid_in(&written), Some(page));
    }

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
