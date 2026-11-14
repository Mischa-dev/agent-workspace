//! A collection's fields and views as one portable piece, by name (AWD-90).
//!
//! collection_describe returns a collection's definition, and
//! collection_write takes the same shape to make the collection again, here
//! or in another project, or to change one. Each field and view is matched
//! by name (or by `field` / `view` naming the one to change), so writing a
//! definition a second time changes nothing. Filter values read the way
//! agents write them: option names, "me" or @handles, task keys, page IDs,
//! numbers and YYYY-MM-DD dates.

use std::collections::{HashMap, HashSet};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    Collection,
    views::{
        self, ChangeViewOutcome, Column, CreateViewOutcome, Direction, NewView, RemoveViewOutcome,
        SortBy, SortRule, View, ViewChange, ViewConfig, ViewType,
    },
};
use crate::{
    error::AppError,
    fields::{
        self, ChangeFieldOutcome, CreateFieldOutcome, FieldChange, FieldClause, FieldDefinition,
        FieldOp, FieldType, LinkTarget, NewField, OptionInput,
    },
    work_item_fields::{self, Writer},
    work_item_labels::echo,
};

/// Fields and views one write names.
pub const MAX_WRITES: usize = 60;

// ---------------------------------------------------------------------------
// The shapes

/// A field as a definition holds it, or a change to one.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldWrite {
    /// The field to change, by its current name or ID. Left out, `name`
    /// finds it, and a name the collection doesn't have adds a field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Its name, or a new name when `field` names it: 1 to 60 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For a new field: text, number, date, single_choice,
    /// multiple_choice, person, url, checkbox or link. It never changes.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub field_type: Option<String>,
    /// What it's for, on one line. "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Choice fields: every option, in order. An option keeps its records
    /// by name; one left out is archived, never removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<OptionInput>>,
    /// For a new link field: "task", "page", or "record:<collection>" for
    /// the records of one of the project's collections, by name or ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Its place among the fields, 1 being first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// true archives it (hidden, values kept), false restores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

/// A filter by field name, as agents write one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilterWrite {
    /// The field, by name or ID.
    pub field: String,
    /// is (any of the values), isNot, empty, notEmpty, contains (text),
    /// gt, gte, lt, lte (numbers and dates).
    pub op: FieldOp,
    /// What to compare with: option names, "me" or @handles, task keys,
    /// page IDs, numbers, YYYY-MM-DD dates, true or false. Not for empty
    /// and notEmpty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SortWrite {
    /// A field's name, or title, number (the order records were added in)
    /// or updated.
    pub field: String,
    /// asc (the default) or desc.
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnWrite {
    /// The field, by name or ID.
    pub field: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// In pixels, 48 to 1200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
}

/// A view as a definition holds it, or a change to one. Each part given
/// replaces that part; the rest stays.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewWrite {
    /// The view to change, by its current name or ID. Left out, `name`
    /// finds it, and a name the collection doesn't have adds a view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Its name, or a new name when `view` names it: 1 to 80 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// "table" (the default), "board", "gallery" or "calendar". A board's
    /// columns are its groupBy field's values; a calendar puts records on
    /// its dateField. A new one without them takes the first field that
    /// fits. The type doesn't change once made.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub view_type: Option<String>,
    /// Its filters, all of which must hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<FilterWrite>>,
    /// Up to 3 sort keys, first one first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<SortWrite>>,
    /// A single choice, person or checkbox field to group by; "" stops
    /// grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// Columns in order; fields left out show after them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<ColumnWrite>>,
    /// The title column's width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_width: Option<u16>,
    /// A calendar's date field, by name or ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_field: Option<String>,
    /// Its place among the views, 1 being first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// true removes the view. Records stay; views hold none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
}

/// A collection's definition: pass its parts to collection_write to make
/// the collection again.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub title_name: String,
    pub fields: Vec<FieldWrite>,
    pub views: Vec<ViewWrite>,
}

// ---------------------------------------------------------------------------
// Describing

/// How the people, tasks and pages a view's filters name are written: a
/// person's @handle (their ID when they have none), a task's key, a page's
/// ID. "me" stays "me".
async fn written_names(
    connection: &mut PgConnection,
    project_id: Uuid,
    definitions: &[FieldDefinition],
    views: &[View],
) -> Result<HashMap<String, String>, AppError> {
    let objects = views
        .iter()
        .flat_map(|view| view.config.filters.iter())
        .flat_map(|clause| {
            clause
                .values
                .iter()
                .map(|value| json!({ clause.field.to_string(): value }))
        })
        .collect::<Vec<Value>>();
    let found = work_item_fields::references(connection, project_id, definitions, &objects).await?;
    Ok(found
        .into_values()
        .filter_map(|reference| {
            let written = match reference.kind {
                "person" => reference.key?,
                "task" => reference.key?,
                _ => return None,
            };
            Some((reference.id.to_string(), written))
        })
        .collect())
}

fn filter_written(
    clause: &FieldClause,
    field: &FieldDefinition,
    names: &HashMap<String, String>,
) -> FilterWrite {
    let values = clause
        .values
        .iter()
        .map(|value| match field.field_type {
            FieldType::SingleChoice | FieldType::MultipleChoice => field
                .option(value)
                .map_or_else(|| value.clone(), |option| option.name.clone()),
            FieldType::Person | FieldType::Link => {
                names.get(value).cloned().unwrap_or_else(|| value.clone())
            }
            _ => value.clone(),
        })
        .collect();
    FilterWrite {
        field: field.name.clone(),
        op: clause.op,
        values,
    }
}

/// A view as a definition writes it, by names.
fn view_written(
    view: &View,
    definitions: &[FieldDefinition],
    names: &HashMap<String, String>,
) -> ViewWrite {
    let config = view.config.active(definitions);
    let field = |id: Uuid| definitions.iter().find(|field| field.id == id);
    let filters = config
        .filters
        .iter()
        .filter_map(|clause| Some(filter_written(clause, field(clause.field)?, names)))
        .collect::<Vec<_>>();
    let sort = config
        .sort
        .iter()
        .filter_map(|rule| {
            let written = match SortBy::parse(&rule.field)? {
                SortBy::Field(id) => field(id)?.name.clone(),
                other => other.key(),
            };
            Some(SortWrite {
                field: written,
                direction: rule.direction,
            })
        })
        .collect::<Vec<_>>();
    let columns = config
        .columns
        .iter()
        .filter_map(|column| {
            Some(ColumnWrite {
                field: field(column.field)?.name.clone(),
                hidden: column.hidden,
                width: column.width,
            })
        })
        .collect::<Vec<_>>();
    ViewWrite {
        name: Some(view.name.clone()),
        view_type: Some(view.view_type.as_str().to_owned()),
        filters: (!filters.is_empty()).then_some(filters),
        sort: (!sort.is_empty()).then_some(sort),
        group_by: config
            .group_by
            .and_then(field)
            .map(|field| field.name.clone()),
        columns: (!columns.is_empty()).then_some(columns),
        title_width: config.title_width,
        date_field: config
            .date_field
            .and_then(field)
            .map(|field| field.name.clone()),
        ..ViewWrite::default()
    }
}

/// A collection's definition: its active fields and its views, by name.
pub async fn describe(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    definitions: &[FieldDefinition],
    views: &[View],
) -> Result<CollectionDefinition, AppError> {
    let names = written_names(connection, project_id, definitions, views).await?;
    let collection_names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM collections WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect();
    Ok(CollectionDefinition {
        name: collection.name.clone(),
        description: collection.description.clone(),
        title_name: collection.title_name.clone(),
        fields: definitions
            .iter()
            .filter(|field| !field.archived)
            .map(|field| FieldWrite {
                name: Some(field.name.clone()),
                field_type: Some(field.field_type.as_str().to_owned()),
                description: (!field.description.is_empty()).then(|| field.description.clone()),
                options: field.field_type.has_options().then(|| {
                    field
                        .config
                        .options
                        .iter()
                        .filter(|option| !option.archived)
                        .map(|option| OptionInput {
                            id: None,
                            name: option.name.clone(),
                            color: Some(option.color.clone()),
                        })
                        .collect()
                }),
                target: field.config.target.map(|target| match target {
                    // By name, so the definition reads the same in another
                    // project that has a collection of that name.
                    LinkTarget::Record => format!(
                        "record:{}",
                        field
                            .config
                            .collection
                            .and_then(|id| collection_names.get(&id).cloned())
                            .unwrap_or_default()
                    ),
                    other => other.written(None),
                }),
                ..FieldWrite::default()
            })
            .collect(),
        views: views
            .iter()
            .map(|view| view_written(view, definitions, &names))
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Reading what agents write

/// The active field `reference` names, or a message listing them.
fn active_field<'a>(
    collection: &Collection,
    definitions: &'a [FieldDefinition],
    reference: &str,
) -> Result<&'a FieldDefinition, String> {
    match fields::find(definitions, reference) {
        Some(field) if !field.archived => Ok(field),
        Some(field) => Err(format!(
            "\"{}\" is archived in {}. Restore it first.",
            field.name, collection.name
        )),
        None => Err(format!(
            "{} has no field called \"{}\". Its fields: {}.",
            collection.name,
            echo(reference),
            fields::active_names(definitions)
        )),
    }
}

/// Filters by field name as stored clauses: option IDs, people's and
/// linked records' IDs ("me" stays "me", whoever reads the view). A person
/// or record outside the project is refused.
pub async fn clauses_from_written(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    collection: &Collection,
    definitions: &[FieldDefinition],
    filters: &[FilterWrite],
) -> Result<Result<Vec<FieldClause>, String>, AppError> {
    if filters.len() > fields::MAX_CLAUSES {
        return Ok(Err(format!(
            "Filter by at most {} fields.",
            fields::MAX_CLAUSES
        )));
    }
    let mut clauses = Vec::with_capacity(filters.len());
    for filter in filters {
        let field = match active_field(collection, definitions, &filter.field) {
            Ok(field) => field,
            Err(message) => return Ok(Err(message)),
        };
        let mut values = Vec::with_capacity(filter.values.len());
        let mut me = false;
        for value in &filter.values {
            match field.field_type {
                FieldType::Person if value.trim().eq_ignore_ascii_case(views::ME) => {
                    // Checked as anyone in the project; stored as "me".
                    me = true;
                    values.push(Uuid::nil().to_string());
                }
                FieldType::Person => {
                    match work_item_fields::member_id(
                        connection,
                        project_id,
                        writer.actor_id,
                        value,
                    )
                    .await?
                    {
                        Some(id) => values.push(id.to_string()),
                        None => {
                            return Ok(Err(format!(
                                "No one called {} is in {}.",
                                echo(value),
                                writer.project_key
                            )));
                        }
                    }
                }
                FieldType::Link => {
                    let target = field.config.target.unwrap_or(LinkTarget::Task);
                    match work_item_fields::find_link(connection, project_id, writer, field, value)
                        .await?
                    {
                        Some((id, _)) => values.push(id.to_string()),
                        None => {
                            return Ok(Err(format!(
                                "\"{}\" links to a {} in {}; {} isn't one.",
                                field.name,
                                match target {
                                    LinkTarget::Page => "page",
                                    LinkTarget::Task => "task",
                                    LinkTarget::Record => "record of its collection",
                                },
                                writer.project_key,
                                echo(value)
                            )));
                        }
                    }
                }
                _ => values.push(value.clone()),
            }
        }
        let mut clause = match fields::clause_from_written(field, filter.op, &values) {
            Ok(clause) => clause,
            Err(message) => return Ok(Err(message)),
        };
        if me {
            let nil = Uuid::nil().to_string();
            for value in &mut clause.values {
                if *value == nil {
                    *value = views::ME.to_owned();
                }
            }
        }
        clauses.push(clause);
    }
    Ok(Ok(clauses))
}

/// Sort keys by name as a view stores them. A field's name wins over the
/// built-in title, number and updated.
pub fn sorts_from_written(
    collection: &Collection,
    definitions: &[FieldDefinition],
    sorts: &[SortWrite],
) -> Result<Vec<SortRule>, String> {
    if sorts.len() > views::MAX_SORTS {
        return Err(format!("Sort by at most {} things.", views::MAX_SORTS));
    }
    sorts
        .iter()
        .map(|sort| {
            let field = match fields::find(definitions, &sort.field) {
                Some(field) if !field.archived => field.id.to_string(),
                found => match SortBy::parse(&sort.field) {
                    Some(SortBy::Field(_)) | None => {
                        return Err(match found {
                            Some(field) => format!(
                                "\"{}\" is archived in {}. Restore it first.",
                                field.name, collection.name
                            ),
                            None => format!(
                                "Sort by title, number, updated, or a field: {}.",
                                fields::active_names(definitions)
                            ),
                        });
                    }
                    Some(by) => by.key(),
                },
            };
            Ok(SortRule {
                field,
                direction: sort.direction,
            })
        })
        .collect()
}

/// The field a view groups by; "" for none.
pub fn group_from_written(
    collection: &Collection,
    definitions: &[FieldDefinition],
    group_by: &str,
) -> Result<Option<Uuid>, String> {
    if group_by.trim().is_empty() {
        return Ok(None);
    }
    let field = active_field(collection, definitions, group_by)?;
    if !field.field_type.can_group() {
        return Err(format!(
            "\"{}\" can't group. Group by a single choice, person or checkbox field.",
            field.name
        ));
    }
    Ok(Some(field.id))
}

fn columns_from_written(
    collection: &Collection,
    definitions: &[FieldDefinition],
    columns: &[ColumnWrite],
) -> Result<Vec<Column>, String> {
    columns
        .iter()
        .map(|column| {
            Ok(Column {
                field: active_field(collection, definitions, &column.field)?.id,
                hidden: column.hidden,
                width: column.width,
            })
        })
        .collect()
}

/// A view's settings with the parts `write` gives replaced, by names.
async fn config_from_written(
    connection: &mut PgConnection,
    project_id: Uuid,
    writer: Writer<'_>,
    collection: &Collection,
    definitions: &[FieldDefinition],
    write: &ViewWrite,
    current: ViewConfig,
) -> Result<Result<ViewConfig, String>, AppError> {
    let mut config = current;
    if let Some(filters) = &write.filters {
        match clauses_from_written(
            connection,
            project_id,
            writer,
            collection,
            definitions,
            filters,
        )
        .await?
        {
            Ok(clauses) => config.filters = clauses,
            Err(message) => return Ok(Err(message)),
        }
    }
    if let Some(sorts) = &write.sort {
        match sorts_from_written(collection, definitions, sorts) {
            Ok(rules) => config.sort = rules,
            Err(message) => return Ok(Err(message)),
        }
    }
    if let Some(group_by) = &write.group_by {
        match group_from_written(collection, definitions, group_by) {
            Ok(group) => config.group_by = group,
            Err(message) => return Ok(Err(message)),
        }
    }
    if let Some(columns) = &write.columns {
        match columns_from_written(collection, definitions, columns) {
            Ok(columns) => config.columns = columns,
            Err(message) => return Ok(Err(message)),
        }
    }
    if write.title_width.is_some() {
        config.title_width = write.title_width;
    }
    if let Some(date_field) = &write.date_field {
        if date_field.trim().is_empty() {
            config.date_field = None;
        } else {
            match active_field(collection, definitions, date_field) {
                Ok(field) if field.field_type == FieldType::Date => {
                    config.date_field = Some(field.id);
                }
                Ok(field) => {
                    return Ok(Err(format!(
                        "\"{}\" isn't a date field. A calendar shows records by a date field.",
                        field.name
                    )));
                }
                Err(message) => return Ok(Err(message)),
            }
        }
    }
    Ok(Ok(config))
}

// ---------------------------------------------------------------------------
// Writing

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProblemKind {
    Invalid,
    NotFound,
    /// A limit: fields, options or views.
    Full,
}

/// Why a write stopped, at which entry ("fields[2].options"). Nothing it
/// did is kept: the caller drops the transaction.
#[derive(Clone, Debug)]
pub struct ApplyProblem {
    pub kind: ProblemKind,
    pub field: String,
    pub message: String,
}

fn stop<T>(
    kind: ProblemKind,
    field: String,
    message: impl Into<String>,
) -> Result<T, ApplyProblem> {
    Err(ApplyProblem {
        kind,
        field,
        message: message.into(),
    })
}

/// What a write changed, for the answer and the audit log, such as
/// "Severity added" or "By tester changed".
pub type Changes = Vec<String>;

/// Options as written, each keeping the existing option of the same name
/// (and every record that has it) when it gives no ID.
fn options_by_name(field: &FieldDefinition, options: &[OptionInput]) -> Vec<OptionInput> {
    options
        .iter()
        .map(|option| {
            if option.id.is_some() {
                return option.clone();
            }
            let name = option.name.split_whitespace().collect::<Vec<_>>().join(" ");
            let existing = field
                .config
                .options
                .iter()
                .filter(|existing| existing.name.to_lowercase() == name.to_lowercase())
                .min_by_key(|existing| existing.archived);
            OptionInput {
                id: existing.map(|existing| existing.id.to_string()),
                ..option.clone()
            }
        })
        .collect()
}

/// Adds and changes a collection's fields, then its views, as written. The
/// caller holds the project lock and the collection's row, and has checked
/// the actor may build collections. Bumps the collection's version when
/// anything changed.
pub async fn apply(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    writer: Writer<'_>,
    field_writes: &[FieldWrite],
    view_writes: &[ViewWrite],
) -> Result<Result<Changes, ApplyProblem>, AppError> {
    if field_writes.len() + view_writes.len() > MAX_WRITES {
        return Ok(stop(
            ProblemKind::Invalid,
            "fields".to_owned(),
            format!("Write at most {MAX_WRITES} fields and views at a time."),
        ));
    }
    // Tasks' fields are the project's task fields; its built-in ones
    // (Status, Assignee, …) don't change here.
    let kind = if collection.is_tasks() {
        fields::RecordKind::Task
    } else {
        super::kind(collection.id)
    };
    let mut changes = Changes::new();
    for (index, write) in field_writes.iter().enumerate() {
        let at = format!("fields[{index}]");
        let definitions =
            super::field_definitions(&mut *connection, project_id, collection.id, true).await?;
        let name = match write.name.as_deref() {
            Some(value) => match fields::normalize_name(value) {
                Some(name) => Some(name),
                None => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.name"),
                        format!(
                            "Give the field a name of 1 to {} characters on one line, that isn't an ID.",
                            fields::MAX_NAME_CHARACTERS
                        ),
                    ));
                }
            },
            None => None,
        };
        let description = match write.description.as_deref() {
            Some(value) => match fields::normalize_description(value) {
                Some(text) => Some(text),
                None => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.description"),
                        format!(
                            "Keep the description to one line of at most {} characters.",
                            fields::MAX_DESCRIPTION_CHARACTERS
                        ),
                    ));
                }
            },
            None => None,
        };
        let restoring = write.archived == Some(false);
        let found = match (write.field.as_deref(), name.as_deref()) {
            (Some(reference), _) => {
                let wanted = reference.trim().to_lowercase();
                let found = if restoring {
                    definitions
                        .iter()
                        .find(|field| {
                            field.archived
                                && (field.id.to_string() == wanted
                                    || field.name.to_lowercase() == wanted)
                        })
                        .or_else(|| fields::find(&definitions, reference))
                } else {
                    fields::find(&definitions, reference)
                };
                let Some(found) = found else {
                    return Ok(stop(
                        ProblemKind::NotFound,
                        format!("{at}.field"),
                        format!(
                            "{} has no field called \"{}\". Its fields: {}.",
                            collection.name,
                            echo(reference),
                            fields::active_names(&definitions)
                        ),
                    ));
                };
                Some(found.clone())
            }
            (None, Some(name)) => definitions
                .iter()
                .filter(|field| field.name.to_lowercase() == name.to_lowercase())
                .filter(|field| restoring || !field.archived)
                .min_by_key(|field| !field.archived)
                .cloned(),
            (None, None) => {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    "Name the field: name finds or adds it, field names one to rename.",
                ));
            }
        };
        if collection.is_tasks()
            && (found
                .as_ref()
                .is_some_and(|field| super::tasks::is_builtin_field(field.id))
                || name.as_deref().is_some_and(super::tasks::is_builtin_name))
        {
            return Ok(stop(
                ProblemKind::Invalid,
                format!("{at}.name"),
                "Status, Assignee, Priority, Start date, Due date and Labels are built into Tasks: \
                 change them on tasks, and give a new field another name.",
            ));
        }
        let Some(current) = found else {
            // Add a field.
            let Some(name) = name else {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    "Give the new field a name.",
                ));
            };
            let Some(field_type) = write.field_type.as_deref().and_then(FieldType::parse) else {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.type"),
                    format!(
                        "\"{name}\" is a new field: give its type, one of text, number, date, single_choice, multiple_choice, person, url, checkbox or link."
                    ),
                ));
            };
            if write.archived.is_some() {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.archived"),
                    format!(
                        "{} has no field called \"{name}\" to archive or restore.",
                        collection.name
                    ),
                ));
            }
            let target = match write.target.as_deref() {
                Some(target) => {
                    match crate::collections::link_target(&mut *connection, project_id, target)
                        .await?
                    {
                        Ok(target) => Some(target),
                        Err(message) => {
                            return Ok(stop(ProblemKind::Invalid, format!("{at}.target"), message));
                        }
                    }
                }
                None => None,
            };
            let config = match fields::normalize_config(
                field_type,
                None,
                write.options.as_deref(),
                target.as_deref(),
            ) {
                Ok(config) => config,
                Err(problem) => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.{}", problem.field),
                        problem.message,
                    ));
                }
            };
            match fields::create(
                connection,
                project_id,
                kind,
                writer.actor_id,
                NewField {
                    name,
                    description: description.unwrap_or_default(),
                    field_type,
                    config,
                    position: write.position,
                },
            )
            .await?
            {
                CreateFieldOutcome::Created(field) => {
                    changes.push(format!("field \"{}\" added", field.name));
                }
                CreateFieldOutcome::NameTaken(field) => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.name"),
                        format!("A field is already called \"{}\".", field.name),
                    ));
                }
                CreateFieldOutcome::ProjectFull => {
                    return Ok(stop(
                        ProblemKind::Full,
                        at,
                        format!(
                            "{} has {} fields already, or keeps {} counting archived ones. Archive or restore one instead.",
                            collection.name,
                            fields::MAX_ACTIVE_FIELDS,
                            fields::MAX_FIELD_ROWS
                        ),
                    ));
                }
            }
            continue;
        };
        // Change a field.
        if let Some(written) = write.field_type.as_deref()
            && FieldType::parse(written) != Some(current.field_type)
        {
            return Ok(stop(
                ProblemKind::Invalid,
                format!("{at}.type"),
                format!(
                    "\"{}\" is a {} field, and a field's type never changes. Add a new field with the type you want.",
                    current.name,
                    current.field_type.as_str()
                ),
            ));
        }
        // Compared as stored: a record link's collection resolves by name or
        // ID, so the definition collection_describe gives replays.
        let retargeted = match write.target.as_deref() {
            None => false,
            Some(written) => {
                match crate::collections::link_target(&mut *connection, project_id, written).await?
                {
                    Ok(resolved) => match resolved.split_once(':') {
                        Some((kind, collection)) => {
                            LinkTarget::parse(kind) != current.config.target
                                || collection.parse::<Uuid>().ok() != current.config.collection
                        }
                        None => LinkTarget::parse(&resolved) != current.config.target,
                    },
                    Err(_) => true,
                }
            }
        };
        if retargeted {
            return Ok(stop(
                ProblemKind::Invalid,
                format!("{at}.target"),
                "A link field's target never changes. Add a new field instead.",
            ));
        }
        let config = match write.options.as_deref() {
            Some(options) => match fields::normalize_config(
                current.field_type,
                Some(&current.config),
                Some(&options_by_name(&current, options)),
                None,
            ) {
                Ok(config) => Some(config),
                Err(problem) => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.{}", problem.field),
                        problem.message,
                    ));
                }
            },
            None => None,
        };
        // Matched by name, a field keeps the name it has.
        let rename = write.field.as_ref().and(name);
        match fields::change(
            connection,
            project_id,
            kind,
            current.id,
            writer.actor_id,
            FieldChange {
                name: rename,
                description,
                config,
                position: write.position,
                archived: write.archived,
                expected_version: None,
            },
        )
        .await?
        {
            ChangeFieldOutcome::Changed {
                field,
                previous,
                changed,
            } => {
                changes.push(match (previous.archived, field.archived) {
                    (false, true) => format!("field \"{}\" archived", field.name),
                    (true, false) => format!("field \"{}\" restored", field.name),
                    _ if previous.name != field.name => {
                        format!("field \"{}\" renamed to \"{}\"", previous.name, field.name)
                    }
                    _ => format!("field \"{}\" changed ({})", field.name, changed.join(", ")),
                });
            }
            ChangeFieldOutcome::Unchanged(_) => {}
            ChangeFieldOutcome::NotFound | ChangeFieldOutcome::VersionConflict(_) => {
                return Ok(stop(
                    ProblemKind::NotFound,
                    format!("{at}.field"),
                    format!(
                        "\"{}\" was just removed. Read the collection again.",
                        current.name
                    ),
                ));
            }
            ChangeFieldOutcome::NameTaken(other) => {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    format!("Another field is already called \"{other}\"."),
                ));
            }
            ChangeFieldOutcome::ProjectFull => {
                return Ok(stop(
                    ProblemKind::Full,
                    at,
                    format!(
                        "{} has {} active fields already. Archive one first.",
                        collection.name,
                        fields::MAX_ACTIVE_FIELDS
                    ),
                ));
            }
        }
    }

    let definitions =
        super::field_definitions(&mut *connection, project_id, collection.id, true).await?;
    for (index, write) in view_writes.iter().enumerate() {
        let at = format!("views[{index}]");
        let existing = views::list(&mut *connection, project_id, collection.id).await?;
        let name = match write.name.as_deref() {
            Some(value) => match super::normalize_name(value) {
                Some(name) => Some(name),
                None => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.name"),
                        format!(
                            "Give the view a name of 1 to {} characters on one line, that isn't an ID.",
                            super::MAX_NAME_CHARACTERS
                        ),
                    ));
                }
            },
            None => None,
        };
        let found = match (write.view.as_deref(), name.as_deref()) {
            (Some(reference), _) => match views::find(&existing, reference) {
                Some(view) => Some(view.clone()),
                None => {
                    return Ok(stop(
                        ProblemKind::NotFound,
                        format!("{at}.view"),
                        format!(
                            "{} has no view called \"{}\". Its views: {}.",
                            collection.name,
                            echo(reference),
                            views::names(&existing)
                        ),
                    ));
                }
            },
            (None, Some(name)) => views::find(&existing, name).cloned(),
            (None, None) => {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    "Name the view: name finds or adds it, view names one to rename.",
                ));
            }
        };
        if write.remove == Some(true) {
            let Some(view) = found else {
                return Ok(stop(
                    ProblemKind::NotFound,
                    format!("{at}.view"),
                    format!(
                        "{} has no view called \"{}\" to remove.",
                        collection.name,
                        echo(
                            write
                                .view
                                .as_deref()
                                .or(name.as_deref())
                                .unwrap_or_default()
                        )
                    ),
                ));
            };
            match views::remove(connection, project_id, collection.id, view.id).await? {
                RemoveViewOutcome::Removed(view) => {
                    changes.push(format!("view \"{}\" removed", view.name));
                }
                RemoveViewOutcome::NotFound => {}
                RemoveViewOutcome::LastView => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.remove"),
                        "A collection keeps at least one view. Add another before removing this one.",
                    ));
                }
            }
            continue;
        }
        if let Some(written) = write.view_type.as_deref()
            && ViewType::parse(written).is_none()
        {
            return Ok(stop(
                ProblemKind::Invalid,
                format!("{at}.type"),
                views::VIEW_TYPE_MESSAGE,
            ));
        }
        let current = found.as_ref().map(|view| view.config.clone());
        let config = match config_from_written(
            connection,
            project_id,
            writer,
            collection,
            &definitions,
            write,
            current.clone().unwrap_or_default(),
        )
        .await?
        {
            Ok(config) => config,
            Err(message) => return Ok(stop(ProblemKind::Invalid, at, message)),
        };
        // A new board or calendar takes the first field that fits.
        let config = match &found {
            Some(_) => config,
            None => match config.for_type(
                write
                    .view_type
                    .as_deref()
                    .and_then(ViewType::parse)
                    .unwrap_or(ViewType::Table),
                &definitions,
            ) {
                Ok(config) => config,
                Err(message) => return Ok(stop(ProblemKind::Invalid, at, message)),
            },
        };
        if let Err(message) = views::check(&config, &definitions) {
            return Ok(stop(ProblemKind::Invalid, at, message));
        }
        if let Err(message) =
            views::check_references(connection, project_id, &config, &definitions).await?
        {
            return Ok(stop(ProblemKind::Invalid, at, message));
        }
        let Some(view) = found else {
            let Some(name) = name else {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    "Give the new view a name.",
                ));
            };
            let view_type = write
                .view_type
                .as_deref()
                .and_then(ViewType::parse)
                .unwrap_or(ViewType::Table);
            match views::create(
                connection,
                project_id,
                collection.id,
                writer.actor_id,
                NewView {
                    name,
                    view_type,
                    config,
                    position: write.position,
                },
            )
            .await?
            {
                CreateViewOutcome::Created(view) => {
                    changes.push(format!("view \"{}\" added", view.name));
                }
                CreateViewOutcome::NameTaken(view) => {
                    return Ok(stop(
                        ProblemKind::Invalid,
                        format!("{at}.name"),
                        format!("A view is already called \"{}\".", view.name),
                    ));
                }
                CreateViewOutcome::Full => {
                    return Ok(stop(
                        ProblemKind::Full,
                        at,
                        format!(
                            "{} has {} views already. Remove one first.",
                            collection.name,
                            views::MAX_VIEWS
                        ),
                    ));
                }
            }
            continue;
        };
        if let Some(written) = write.view_type.as_deref()
            && ViewType::parse(written) != Some(view.view_type)
        {
            return Ok(stop(
                ProblemKind::Invalid,
                format!("{at}.type"),
                "A view's type doesn't change. Add a new view instead.",
            ));
        }
        let rename = write.view.as_ref().and(name);
        match views::change(
            connection,
            project_id,
            collection.id,
            view.id,
            ViewChange {
                name: rename,
                config: (Some(&config) != current.as_ref()).then_some(config),
                position: write.position,
                expected_version: None,
            },
        )
        .await?
        {
            ChangeViewOutcome::Changed { view: changed, .. } => {
                changes.push(if changed.name == view.name {
                    format!("view \"{}\" changed", changed.name)
                } else {
                    format!("view \"{}\" renamed to \"{}\"", view.name, changed.name)
                });
            }
            ChangeViewOutcome::Unchanged(_) => {}
            ChangeViewOutcome::NotFound | ChangeViewOutcome::VersionConflict(_) => {
                return Ok(stop(
                    ProblemKind::NotFound,
                    format!("{at}.view"),
                    format!(
                        "\"{}\" was just removed. Read the collection again.",
                        view.name
                    ),
                ));
            }
            ChangeViewOutcome::NameTaken(other) => {
                return Ok(stop(
                    ProblemKind::Invalid,
                    format!("{at}.name"),
                    format!("Another view is already called \"{other}\"."),
                ));
            }
        }
    }
    if !changes.is_empty() {
        super::touch(connection, project_id, collection.id).await?;
    }
    Ok(Ok(changes))
}

/// Field and view names a write mentions, for checking a definition before
/// anything is created: each new name once.
pub fn duplicate_name(field_writes: &[FieldWrite], view_writes: &[ViewWrite]) -> Option<String> {
    let mut fields_seen = HashSet::new();
    for write in field_writes.iter().filter(|write| write.field.is_none()) {
        if let Some(name) = write.name.as_deref().and_then(fields::normalize_name)
            && !fields_seen.insert(name.to_lowercase())
        {
            return Some(format!("Two fields are called \"{name}\". Name each once."));
        }
    }
    let mut views_seen = HashSet::new();
    for write in view_writes.iter().filter(|write| write.view.is_none()) {
        if let Some(name) = write.name.as_deref().and_then(super::normalize_name)
            && !views_seen.insert(name.to_lowercase())
        {
            return Some(format!("Two views are called \"{name}\". Name each once."));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{FieldWrite, ViewWrite, duplicate_name, options_by_name};
    use crate::fields::{ChoiceOption, FieldConfig, FieldDefinition, FieldType, OptionInput};

    #[test]
    fn options_keep_their_records_by_name() {
        let kept = Uuid::now_v7();
        let field = FieldDefinition {
            id: Uuid::now_v7(),
            name: "Severity".to_owned(),
            description: String::new(),
            field_type: FieldType::SingleChoice,
            config: FieldConfig {
                options: vec![ChoiceOption {
                    id: kept,
                    name: "High".to_owned(),
                    color: "#ef4444".to_owned(),
                    archived: false,
                }],
                target: None,
                collection: None,
            },
            position: 0,
            archived: false,
            version: 1,
        };
        let written = options_by_name(
            &field,
            &[
                OptionInput {
                    id: None,
                    name: " high ".to_owned(),
                    color: None,
                },
                OptionInput {
                    id: None,
                    name: "Low".to_owned(),
                    color: None,
                },
            ],
        );
        assert_eq!(written[0].id.as_deref(), Some(kept.to_string().as_str()));
        assert_eq!(written[1].id, None);
    }

    #[test]
    fn a_definition_names_each_new_field_and_view_once() {
        let field = |name: &str| FieldWrite {
            name: Some(name.to_owned()),
            field_type: Some("text".to_owned()),
            ..FieldWrite::default()
        };
        let view = |name: &str| ViewWrite {
            name: Some(name.to_owned()),
            ..ViewWrite::default()
        };
        assert_eq!(
            duplicate_name(&[field("Notes"), field("Tester")], &[]),
            None
        );
        assert!(duplicate_name(&[field("Notes"), field("notes")], &[]).is_some());
        assert!(duplicate_name(&[], &[view("All"), view("ALL")]).is_some());
    }
}
