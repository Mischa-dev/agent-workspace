//! A collection's records (AWD-90): their titles and values, writes and
//! history.
//!
//! A record has a number in its collection (#12), a title and values keyed
//! by field ID, as a task's `field_values` are. Values go through the same
//! checks as task fields (`crate::fields`, `crate::work_item_fields`): a
//! person is someone in the project who can hold tasks, and a link names a
//! task or page in the same project. Every change is kept in
//! `collection_record_history`, each field's before and after as it read
//! then, with the app the change came through (`current_via_grant()`).
//!
//! A write takes up to `MAX_RECORDS_PER_WRITE` records and is all or
//! nothing: the first problem stops it, and the caller drops the
//! transaction.

use std::collections::{HashMap, HashSet};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::Collection;
use crate::{
    error::AppError,
    fields::{self, FieldDefinition, LinkTarget, Normalized},
    work_item_fields::{self, FieldValueChange, FieldValueChanges, ValueReference, Writer},
    work_item_labels::echo,
};

pub const MAX_TITLE_CHARACTERS: usize = 300;
/// Records one write adds or changes, all or nothing.
pub const MAX_RECORDS_PER_WRITE: usize = 100;
/// History entries a record's read returns, newest first.
pub const HISTORY_LIMIT: i64 = 50;
/// Longest text kept on each side of a change in history.
const MAX_HISTORY_TEXT_CHARACTERS: usize = 300;

// ---------------------------------------------------------------------------
// Names

/// How someone names a record: its ID (or a link that ends in one), or its
/// number, written 12 or #12.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RecordRef {
    Id(Uuid),
    Number(i64),
}

impl RecordRef {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(id) = super::id_in(value) {
            return Some(Self::Id(id));
        }
        value
            .trim_start_matches('#')
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|number| *number > 0)
            .map(Self::Number)
    }

    /// How a message names it.
    pub fn shown(self) -> String {
        match self {
            Self::Id(id) => id.to_string(),
            Self::Number(number) => format!("#{number}"),
        }
    }
}

/// A record's title: one line of at most `MAX_TITLE_CHARACTERS`, runs of
/// whitespace collapsed. Empty is allowed: the record shows as untitled.
pub fn normalize_title(value: &str) -> Option<String> {
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return None;
    }
    let title = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (title.chars().count() <= MAX_TITLE_CHARACTERS).then_some(title)
}

fn title_message() -> String {
    format!("A title is one line of at most {MAX_TITLE_CHARACTERS} characters.")
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Clone, Debug, FromRow)]
pub struct Record {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub number: i64,
    pub title: String,
    /// Values keyed by field ID, as stored.
    pub field_values: Value,
    pub created_by_actor_id: Uuid,
    pub created_by_name: Option<String>,
    pub updated_by_actor_id: Uuid,
    pub updated_by_name: Option<String>,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// The tables a record is read from, aliased `record`.
pub(super) const RECORD_FROM: &str = "FROM collection_records record \
     LEFT JOIN project_memberships record_creator \
       ON record_creator.project_id = record.project_id \
      AND record_creator.actor_id = record.created_by_actor_id \
     LEFT JOIN project_memberships record_editor \
       ON record_editor.project_id = record.project_id \
      AND record_editor.actor_id = record.updated_by_actor_id";

pub(super) const RECORD_COLUMNS: &str = "record.id, record.collection_id, record.number, \
     record.title, record.field_values, record.created_by_actor_id, \
     record_creator.display_name AS created_by_name, record.updated_by_actor_id, \
     record_editor.display_name AS updated_by_name, record.archived_at IS NOT NULL AS archived, \
     record.version, record.created_at::text AS created_at, record.updated_at::text AS updated_at";

/// Records of a collection by ID, in the order of `ids`. IDs that aren't
/// the collection's are left out.
pub async fn by_ids(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Record>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut found = sqlx::query_as::<_, Record>(&format!(
        "SELECT {RECORD_COLUMNS} {RECORD_FROM} \
         WHERE record.project_id = $1 AND record.collection_id = $2 AND record.id = ANY($3)"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(ids)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|record| (record.id, record))
    .collect::<HashMap<_, _>>();
    Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
}

/// One of a collection's records, by ID or number.
pub async fn find(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    reference: RecordRef,
) -> Result<Option<Record>, AppError> {
    let (id, number) = match reference {
        RecordRef::Id(id) => (Some(id), None),
        RecordRef::Number(number) => (None, Some(number)),
    };
    Ok(sqlx::query_as::<_, Record>(&format!(
        "SELECT {RECORD_COLUMNS} {RECORD_FROM} \
         WHERE record.project_id = $1 AND record.collection_id = $2 \
           AND (record.id = $3 OR record.number = $4)"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(id)
    .bind(number)
    .fetch_optional(executor)
    .await?)
}

/// The IDs of the records `references` name, in order; None for any that
/// isn't one of the collection's.
pub async fn ids_of(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    collection_id: Uuid,
    references: &[RecordRef],
) -> Result<Vec<Option<Uuid>>, AppError> {
    let ids = references
        .iter()
        .filter_map(|reference| match reference {
            RecordRef::Id(id) => Some(*id),
            RecordRef::Number(_) => None,
        })
        .collect::<Vec<_>>();
    let numbers = references
        .iter()
        .filter_map(|reference| match reference {
            RecordRef::Number(number) => Some(*number),
            RecordRef::Id(_) => None,
        })
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, number FROM collection_records \
         WHERE project_id = $1 AND collection_id = $2 AND (id = ANY($3) OR number = ANY($4))",
    )
    .bind(project_id)
    .bind(collection_id)
    .bind(&ids)
    .bind(&numbers)
    .fetch_all(executor)
    .await?;
    Ok(references
        .iter()
        .map(|reference| {
            rows.iter()
                .find(|(id, number)| match reference {
                    RecordRef::Id(wanted) => id == wanted,
                    RecordRef::Number(wanted) => number == wanted,
                })
                .map(|(id, _)| *id)
        })
        .collect())
}

async fn lock(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Uuid,
    reference: RecordRef,
) -> Result<Option<Record>, AppError> {
    let (id, number) = match reference {
        RecordRef::Id(id) => (Some(id), None),
        RecordRef::Number(number) => (None, Some(number)),
    };
    Ok(sqlx::query_as::<_, Record>(&format!(
        "SELECT {RECORD_COLUMNS} {RECORD_FROM} \
         WHERE record.project_id = $1 AND record.collection_id = $2 \
           AND (record.id = $3 OR record.number = $4) \
         FOR UPDATE OF record"
    ))
    .bind(project_id)
    .bind(collection_id)
    .bind(id)
    .bind(number)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Someone a record names: who made it or changed it last.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonRef {
    pub id: Uuid,
    pub name: String,
}

/// A record as the web app sees it: values by field ID, as stored, for the
/// collection's active fields.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordView {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub values: Map<String, Value>,
    pub created_by: PersonRef,
    pub updated_by: PersonRef,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl RecordView {
    pub fn of(record: &Record, definitions: &[FieldDefinition]) -> Self {
        let stored = record.field_values.as_object();
        let values = definitions
            .iter()
            .filter(|field| !field.archived)
            .filter_map(|field| {
                let key = field.id.to_string();
                let value = stored?.get(&key)?.clone();
                Some((key, value))
            })
            .collect();
        Self {
            id: record.id,
            number: record.number,
            title: record.title.clone(),
            values,
            created_by: PersonRef {
                id: record.created_by_actor_id,
                name: record.created_by_name.clone().unwrap_or_default(),
            },
            updated_by: PersonRef {
                id: record.updated_by_actor_id,
                name: record.updated_by_name.clone().unwrap_or_default(),
            },
            archived: record.archived,
            version: record.version,
            created_at: record.created_at.clone(),
            updated_at: record.updated_at.clone(),
        }
    }
}

/// The people, tasks and pages the records' values name, for showing them
/// by name. Anything that no longer exists is left out.
pub async fn references(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    records: &[Record],
) -> Result<Vec<ValueReference>, AppError> {
    let found = work_item_fields::references(
        connection,
        project_id,
        definitions,
        records.iter().map(|record| &record.field_values),
    )
    .await?;
    let mut references = found.into_values().collect::<Vec<_>>();
    references.sort_by_key(|reference| reference.id);
    Ok(references)
}

/// A record as agents read it: values by field name, as they read (option
/// names, @handles, task keys, page titles), only the ones it has.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordLine {
    /// Its number in the collection: #12 is 12.
    pub number: i64,
    pub id: String,
    pub title: String,
    /// Values by field name; a field with no value is left out.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    /// With groupBy: the group it's in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// What record_write's expectedVersion takes.
    pub version: i64,
    pub updated_at: String,
    /// Who changed it last.
    pub updated_by: String,
}

impl RecordLine {
    /// `only` names the fields to show, by ID; None shows them all.
    pub fn of(
        record: &Record,
        definitions: &[FieldDefinition],
        references: &HashMap<Uuid, ValueReference>,
        only: Option<&HashSet<Uuid>>,
    ) -> Self {
        let fields = work_item_fields::named_values(definitions, &record.field_values, references)
            .into_iter()
            .filter(|(field, _)| only.is_none_or(|only| only.contains(&field.id)))
            .map(|(field, value)| (field.name, value))
            .collect();
        Self {
            number: record.number,
            id: record.id.to_string(),
            title: record.title.clone(),
            fields,
            group: None,
            archived: record.archived,
            version: record.version,
            updated_at: record.updated_at.clone(),
            updated_by: record.updated_by_name.clone().unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// History

/// One change to a record, newest first.
#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// created, updated, archived or restored.
    pub action: String,
    #[schemars(with = "String")]
    pub actor_id: Uuid,
    /// Who made the change.
    pub actor: String,
    /// The app they made it through, when it shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// [{"field", "name", "before", "after"}], each value as it read then.
    pub changes: Value,
    /// The record's version after the change.
    pub version: i64,
    pub at: String,
}

pub async fn history(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    record_id: Uuid,
    limit: i64,
) -> Result<Vec<HistoryEntry>, AppError> {
    Ok(sqlx::query_as::<_, HistoryEntry>(
        r#"
        SELECT history.id, history.action, history.actor_id,
               COALESCE(member.display_name, '') AS actor,
               via_client.client_name AS via,
               history.changes, history.version, history.created_at::text AS at
        FROM collection_record_history history
        LEFT JOIN project_memberships member
          ON member.project_id = history.project_id AND member.actor_id = history.actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = history.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE history.project_id = $1 AND history.record_id = $2
        ORDER BY history.created_at DESC, history.id DESC
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(record_id)
    .bind(limit.clamp(1, HISTORY_LIMIT))
    .fetch_all(executor)
    .await?)
}

/// A value as history keeps it: long text cut, a long list of choices
/// written out and cut.
fn kept(value: Value) -> Value {
    let cut = |text: &str| {
        if text.chars().count() <= MAX_HISTORY_TEXT_CHARACTERS {
            text.to_owned()
        } else {
            let mut cut = text
                .chars()
                .take(MAX_HISTORY_TEXT_CHARACTERS - 1)
                .collect::<String>();
            cut.push('…');
            cut
        }
    };
    match value {
        Value::String(text) => Value::String(cut(&text)),
        Value::Array(items) => {
            let text = fields::display_text(&Value::Array(items.clone()));
            if text.chars().count() <= MAX_HISTORY_TEXT_CHARACTERS {
                Value::Array(items)
            } else {
                Value::String(cut(&text))
            }
        }
        other => other,
    }
}

fn change_entry(field: &str, name: &str, before: Value, after: Value) -> Value {
    json!({ "field": field, "name": name, "before": kept(before), "after": kept(after) })
}

async fn insert_history(
    connection: &mut PgConnection,
    project_id: Uuid,
    record_id: Uuid,
    actor_id: Uuid,
    action: &str,
    changes: Vec<Value>,
    version: i64,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO collection_record_history (
            id, project_id, record_id, actor_id, action, changes, version
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(record_id)
    .bind(actor_id)
    .bind(action)
    .bind(Value::Array(changes))
    .bind(version)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Values

fn problem(argument: &str, message: impl Into<String>) -> work_item_fields::ValueProblem {
    work_item_fields::ValueProblem {
        field: argument.to_owned(),
        message: message.into(),
    }
}

/// Values written by field name or ID, checked and resolved for a record of
/// `collection`, as `work_item_fields::resolve_for` does for a task.
/// `current` is the record's stored values when it exists, so an archived
/// option it holds may stay chosen. An archived or unknown field, a value
/// of the wrong type, someone who isn't in the project or a link outside it
/// is refused, and nothing is set.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_values(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    writer: Writer<'_>,
    definitions: &[FieldDefinition],
    argument: &str,
    written: &[(String, Value)],
    current: Option<&Value>,
) -> Result<Result<FieldValueChanges, work_item_fields::ValueProblem>, AppError> {
    if written.len() > work_item_fields::MAX_VALUES_PER_CHANGE {
        return Ok(Err(problem(
            argument,
            format!(
                "Set at most {} fields at a time.",
                work_item_fields::MAX_VALUES_PER_CHANGE
            ),
        )));
    }
    let mut changes = FieldValueChanges::default();
    let mut seen = HashSet::new();
    for (reference, input) in written {
        let Some(field) = fields::find(definitions, reference) else {
            return Ok(Err(problem(
                argument,
                format!(
                    "{} has no field called \"{}\". Its fields: {}.",
                    collection.name,
                    echo(reference),
                    fields::active_names(definitions)
                ),
            )));
        };
        let at = format!("{argument}.{}", field.name);
        if field.archived {
            return Ok(Err(problem(
                &at,
                format!(
                    "\"{}\" is archived in {}. Restore the field to set it.",
                    field.name, collection.name
                ),
            )));
        }
        if !seen.insert(field.id) {
            return Ok(Err(problem(
                &at,
                format!("\"{}\" is set twice. Set each field once.", field.name),
            )));
        }
        let kept = current.and_then(|values| values.get(field.id.to_string()));
        let normalized = match fields::normalize_value_keeping(field, input, kept) {
            Ok(normalized) => normalized,
            Err(message) => return Ok(Err(problem(&at, message))),
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
                match work_item_fields::find_person(connection, project_id, writer, &written)
                    .await?
                {
                    Some((id, name)) => FieldValueChange {
                        field: field.clone(),
                        value: Some(Value::String(id.to_string())),
                        shown: Value::String(name),
                    },
                    None => {
                        return Ok(Err(problem(
                            &at,
                            format!(
                                "\"{}\" takes someone in {}: \"me\", an @handle or an ID. No one there is called {}.",
                                field.name,
                                writer.project_key,
                                echo(&written)
                            ),
                        )));
                    }
                }
            }
            Normalized::Link(written) => {
                let target = field.config.target.unwrap_or(LinkTarget::Task);
                match work_item_fields::find_link(connection, project_id, writer, target, &written)
                    .await?
                {
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
                        return Ok(Err(problem(
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
                                echo(&written)
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

// ---------------------------------------------------------------------------
// Writing

/// One record to add or change, as written.
#[derive(Clone, Debug, Default)]
pub struct RecordInput {
    /// The record to change; None adds one.
    pub record: Option<RecordRef>,
    pub title: Option<String>,
    /// Values by field name or ID; null clears one.
    pub values: Vec<(String, Value)>,
    /// true archives the record, false restores it.
    pub archived: Option<bool>,
    /// Checked only when something would change.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum Written {
    Created(Record),
    Changed {
        record: Record,
        /// "title", "archived", and the names of the fields that changed.
        changed: Vec<String>,
    },
    Unchanged(Record),
}

impl Written {
    pub fn record(&self) -> &Record {
        match self {
            Self::Created(record) | Self::Changed { record, .. } | Self::Unchanged(record) => {
                record
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProblemKind {
    /// Something written can't be used; the message says what would work.
    Invalid,
    /// A record named isn't in the collection.
    NotFound,
    /// The record changed since it was read, or it's archived.
    Conflict,
    /// The collection keeps `super::MAX_RECORDS` already.
    Full,
}

/// Why a write stopped. Nothing it did is kept: the caller drops the
/// transaction.
#[derive(Clone, Debug)]
pub struct WriteProblem {
    /// Which record, counting from 0 in the order written.
    pub index: usize,
    pub kind: ProblemKind,
    /// The part of that record, such as "title" or "fields.Severity".
    pub field: String,
    pub message: String,
    /// The record's version now, for a version conflict.
    pub current_version: Option<i64>,
}

/// Adds and changes records of `collection`, the row the caller locked
/// (`super::lock`) after the project lock. `values_argument` is what the
/// caller calls the values ("fields" for agents, "values" for the web app),
/// for refusals. The caller has checked the actor may edit records and
/// that the collection isn't archived.
#[allow(clippy::too_many_arguments)]
pub async fn write(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    writer: Writer<'_>,
    definitions: &[FieldDefinition],
    values_argument: &str,
    inputs: &[RecordInput],
) -> Result<Result<Vec<Written>, WriteProblem>, AppError> {
    if inputs.len() > MAX_RECORDS_PER_WRITE {
        return Ok(Err(WriteProblem {
            index: MAX_RECORDS_PER_WRITE,
            kind: ProblemKind::Invalid,
            field: "records".to_owned(),
            message: format!("Write at most {MAX_RECORDS_PER_WRITE} records at a time."),
            current_version: None,
        }));
    }
    let mut next_number = collection.next_number;
    let mut written = Vec::with_capacity(inputs.len());
    let mut touched = HashSet::new();
    for (index, input) in inputs.iter().enumerate() {
        let stop = |kind: ProblemKind, field: &str, message: String| {
            Ok(Err(WriteProblem {
                index,
                kind,
                field: field.to_owned(),
                message,
                current_version: None,
            }))
        };
        let title = match input.title.as_deref() {
            Some(value) => match normalize_title(value) {
                Some(title) => Some(title),
                None => return stop(ProblemKind::Invalid, "title", title_message()),
            },
            None => None,
        };
        let Some(reference) = input.record else {
            if input.archived.is_some() || input.expected_version.is_some() {
                return stop(
                    ProblemKind::Invalid,
                    "record",
                    "archived and expectedVersion change a record that exists; name it with record."
                        .to_owned(),
                );
            }
            if next_number > super::MAX_RECORDS {
                return stop(
                    ProblemKind::Full,
                    "records",
                    format!(
                        "{} keeps {} records already, archived ones included.",
                        collection.name,
                        super::MAX_RECORDS
                    ),
                );
            }
            let changes = match resolve_values(
                connection,
                project_id,
                collection,
                writer,
                definitions,
                values_argument,
                &input.values,
                None,
            )
            .await?
            {
                Ok(changes) => changes,
                Err(problem) => {
                    return stop(ProblemKind::Invalid, &problem.field, problem.message);
                }
            };
            let values = changes.initial();
            if !work_item_fields::values_fit(&values) {
                return stop(
                    ProblemKind::Invalid,
                    values_argument,
                    work_item_fields::TOO_LARGE.to_owned(),
                );
            }
            let title = title.unwrap_or_default();
            let record = create(
                connection,
                project_id,
                collection,
                writer.actor_id,
                next_number,
                &title,
                &values,
            )
            .await?;
            next_number += 1;
            let mut history = Vec::new();
            if !title.is_empty() {
                history.push(change_entry(
                    "title",
                    &collection.title_name,
                    Value::Null,
                    Value::String(title.clone()),
                ));
            }
            for entry in changes.entries.iter().filter(|entry| entry.value.is_some()) {
                history.push(change_entry(
                    &entry.field.id.to_string(),
                    &entry.field.name,
                    Value::Null,
                    entry.shown.clone(),
                ));
            }
            insert_history(
                connection,
                project_id,
                record.id,
                writer.actor_id,
                "created",
                history,
                record.version,
            )
            .await?;
            touched.insert(record.id);
            written.push(Written::Created(record));
            continue;
        };
        let Some(current) = lock(connection, project_id, collection.id, reference).await? else {
            return stop(
                ProblemKind::NotFound,
                "record",
                format!("{} has no record {}.", collection.name, reference.shown()),
            );
        };
        if !touched.insert(current.id) {
            return stop(
                ProblemKind::Invalid,
                "record",
                format!(
                    "#{} is named twice. Change each record once per call.",
                    current.number
                ),
            );
        }
        let restoring = current.archived && input.archived == Some(false);
        let archiving = !current.archived && input.archived == Some(true);
        let edits = title.is_some() || !input.values.is_empty();
        if current.archived && !restoring && edits {
            return stop(
                ProblemKind::Conflict,
                "archived",
                format!(
                    "#{} is archived. Restore it (archived: false) to change it.",
                    current.number
                ),
            );
        }
        let changes = match resolve_values(
            connection,
            project_id,
            collection,
            writer,
            definitions,
            values_argument,
            &input.values,
            Some(&current.field_values),
        )
        .await?
        {
            Ok(changes) => changes,
            Err(problem) => return stop(ProblemKind::Invalid, &problem.field, problem.message),
        };
        let mut changed = Vec::new();
        let mut history = Vec::new();
        let title = title.unwrap_or_else(|| current.title.clone());
        if title != current.title {
            changed.push("title".to_owned());
            history.push(change_entry(
                "title",
                &collection.title_name,
                Value::String(current.title.clone()),
                Value::String(title.clone()),
            ));
        }
        for entry in &changes.entries {
            let key = entry.field.id.to_string();
            let before = current.field_values.get(&key);
            if before == entry.value.as_ref() {
                continue;
            }
            changed.push(entry.field.name.clone());
            history.push(change_entry(
                &key,
                &entry.field.name,
                work_item_fields::shown_before(connection, project_id, &entry.field, before)
                    .await?,
                entry.shown.clone(),
            ));
        }
        if restoring || archiving {
            changed.push("archived".to_owned());
        }
        if changed.is_empty() {
            written.push(Written::Unchanged(current));
            continue;
        }
        if let Some(expected) = input.expected_version
            && expected != current.version
        {
            return Ok(Err(WriteProblem {
                index,
                kind: ProblemKind::Conflict,
                field: "expectedVersion".to_owned(),
                message: format!(
                    "#{} changed since it was read (now version {}). Read it again and retry.",
                    current.number, current.version
                ),
                current_version: Some(current.version),
            }));
        }
        let values = changes.applied_to(&current.field_values);
        if !work_item_fields::values_fit(&values) {
            return stop(
                ProblemKind::Invalid,
                values_argument,
                work_item_fields::TOO_LARGE.to_owned(),
            );
        }
        let archived = if restoring {
            false
        } else {
            current.archived || archiving
        };
        sqlx::query(
            r#"
            UPDATE collection_records
            SET title = $3, field_values = $4,
                archived_at = CASE WHEN $5 THEN COALESCE(archived_at, now()) END,
                archived_by_actor_id = CASE WHEN $5 THEN COALESCE(archived_by_actor_id, $6) END,
                updated_by_actor_id = $6, version = version + 1, updated_at = now()
            WHERE project_id = $1 AND id = $2
            "#,
        )
        .bind(project_id)
        .bind(current.id)
        .bind(&title)
        .bind(&values)
        .bind(archived)
        .bind(writer.actor_id)
        .execute(&mut *connection)
        .await?;
        let action = if archiving {
            "archived"
        } else if restoring {
            "restored"
        } else {
            "updated"
        };
        insert_history(
            connection,
            project_id,
            current.id,
            writer.actor_id,
            action,
            history,
            current.version + 1,
        )
        .await?;
        let record = find(
            &mut *connection,
            project_id,
            collection.id,
            RecordRef::Id(current.id),
        )
        .await?
        .ok_or(AppError::Internal("changed record is missing"))?;
        written.push(Written::Changed { record, changed });
    }
    if next_number != collection.next_number {
        sqlx::query("UPDATE collections SET next_number = $3 WHERE project_id = $1 AND id = $2")
            .bind(project_id)
            .bind(collection.id)
            .bind(next_number)
            .execute(&mut *connection)
            .await?;
    }
    Ok(Ok(written))
}

async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    actor_id: Uuid,
    number: i64,
    title: &str,
    values: &Value,
) -> Result<Record, AppError> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO collection_records (
            id, project_id, collection_id, number, title, field_values,
            created_by_actor_id, updated_by_actor_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(collection.id)
    .bind(number)
    .bind(title)
    .bind(values)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    find(
        &mut *connection,
        project_id,
        collection.id,
        RecordRef::Id(id),
    )
    .await?
    .ok_or(AppError::Internal("created record is missing"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{MAX_TITLE_CHARACTERS, RecordRef, kept, normalize_title};

    #[test]
    fn records_are_named_by_number_or_id() {
        assert_eq!(RecordRef::parse("#12"), Some(RecordRef::Number(12)));
        assert_eq!(RecordRef::parse(" 7 "), Some(RecordRef::Number(7)));
        assert_eq!(RecordRef::parse("#0"), None);
        assert_eq!(RecordRef::parse("twelve"), None);
        let id = Uuid::now_v7();
        assert_eq!(RecordRef::parse(&id.to_string()), Some(RecordRef::Id(id)));
        assert_eq!(
            RecordRef::parse(&format!(
                "https://example.com/projects/{}/collections/{}/records/{id}",
                Uuid::now_v7(),
                Uuid::now_v7()
            )),
            Some(RecordRef::Id(id))
        );
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(
            normalize_title("  Crash   on\nlevel 3 ").as_deref(),
            Some("Crash on level 3")
        );
        assert_eq!(normalize_title("").as_deref(), Some(""));
        assert!(normalize_title("bell\u{7}").is_none());
        assert!(normalize_title(&"a".repeat(MAX_TITLE_CHARACTERS + 1)).is_none());
    }

    #[test]
    fn history_keeps_values_short() {
        let long = "x".repeat(1_000);
        let cut = kept(json!(long));
        assert_eq!(cut.as_str().map(|text| text.chars().count()), Some(300));
        assert_eq!(kept(json!(["A", "B"])), json!(["A", "B"]));
        let many = (0..200)
            .map(|index| format!("Option {index}"))
            .collect::<Vec<_>>();
        assert!(kept(json!(many)).is_string());
        assert_eq!(kept(json!(3)), json!(3));
    }
}
