//! Formula fields (T1.17): a small expression language of our own, run in
//! Rust, with each record's results kept in `collection_records
//! .computed_values` so filters, sorts and groups read them as they read
//! stored values (`fields::Accessor::Computed`).
//!
//! A formula names fields as `{Name}`; it's stored with IDs, `{#<id>}`
//! (`{#title}` for the record's title), so renaming a field never breaks
//! it, and shown with today's names (`shown`). It's checked when saved
//! (`compile`: syntax, functions, types, limits, and that no formula uses
//! itself through others) and run per record when the record is written
//! (`compute`). Errors at run time are values (`{"error": "Divide by zero"}`):
//! they show in the cell and filter as empty.
//!
//! Rollups (T1.18, `super::rollup`) are worked out here too
//! ([`rollup`]), from the related records' values, and kept beside the
//! formulas' results, where formulas read them.

pub mod limits;

mod eval;
mod functions;
mod parse;
mod value;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use self::{
    eval::{Fields, Run},
    limits::{
        MAX_COMPUTED_BYTES, MAX_REFERENCES, MAX_SOURCE_CHARACTERS, MAX_TEXT_RESULT_CHARACTERS,
    },
    parse::{Expr, print, stored_name, stored_reference},
    value::{Ty, V},
};
pub use self::{functions::NAMES as FUNCTIONS, parse::FieldRef, value::ResultType};
use super::{
    FieldDefinition, FieldType,
    rollup::{Aggregate, MAX_ROLLUP_RECORDS, RollupConfig},
};

/// A formula field's settings, in `FieldConfig::formula`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaConfig {
    /// As stored: fields named by ID (`{#<id>}`).
    pub source: String,
    /// What its value is: decides how it filters, sorts and groups.
    pub result: ResultType,
    /// It reads the time (TODAY, NOW), so its value changes on its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub volatile: bool,
}

/// A formula as a field's view shows it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormulaView {
    /// As stored: fields as `{#<field id>}`, the title as `{#title}`, so a
    /// rename never breaks it. Show it with today's names.
    pub source: String,
    /// number, text, boolean, date or list: how it filters and sorts.
    pub result: ResultType,
}

impl From<&FormulaConfig> for FormulaView {
    fn from(config: &FormulaConfig) -> Self {
        Self {
            source: config.source.clone(),
            result: config.result,
        }
    }
}

/// The type a field's value has inside a formula, or why a formula can't
/// use it.
fn field_ty(field: &FieldDefinition) -> Result<Ty, String> {
    Ok(match field.field_type {
        FieldType::Number | FieldType::AutoId => Ty::Number,
        FieldType::Date | FieldType::CreatedTime | FieldType::EditedTime => Ty::Date,
        FieldType::Checkbox => Ty::Boolean,
        FieldType::MultipleChoice | FieldType::People | FieldType::Labels | FieldType::Relation => {
            Ty::List
        }
        FieldType::Formula | FieldType::Rollup => match field.result() {
            Some(result) => result.ty(),
            None => Ty::Any,
        },
        FieldType::Button => {
            return Err(format!(
                "\"{}\" is a button, which has no value for a formula to use.",
                field.name
            ));
        }
        FieldType::Text
        | FieldType::LongText
        | FieldType::Url
        | FieldType::Email
        | FieldType::Phone
        | FieldType::SingleChoice
        | FieldType::Status
        | FieldType::Person
        | FieldType::Link
        | FieldType::CreatedBy
        | FieldType::EditedBy => Ty::Text,
    })
}

/// What a formula is checked against: the fields of its collection, the
/// collection's name for the title, and the formula field itself (None
/// while it's being made).
pub struct Scope<'a> {
    pub fields: &'a [FieldDefinition],
    pub title_name: &'a str,
    pub own: Option<Uuid>,
}

impl Scope<'_> {
    fn by_id(&self, id: Uuid) -> Option<&FieldDefinition> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// What `{ ... }` names, as written: a field by name (or `#<id>`), or
    /// the title.
    fn resolve(&self, inside: &str) -> Result<FieldRef, String> {
        let reference = if inside.starts_with('#') {
            stored_reference(inside)?
        } else if inside.eq_ignore_ascii_case(self.title_name)
            || inside.eq_ignore_ascii_case("title")
        {
            if let Some(field) = super::find(self.fields, inside).filter(|field| !field.archived) {
                FieldRef::Field(field.id)
            } else {
                FieldRef::Title
            }
        } else {
            match super::find(self.fields, inside) {
                Some(field) => FieldRef::Field(field.id),
                None => {
                    return Err(format!(
                        "There's no field called \"{}\". Fields: {}{}.",
                        value::short(inside),
                        self.title_name,
                        self.fields
                            .iter()
                            .filter(|field| !field.archived && Some(field.id) != self.own)
                            .map(|field| format!(", {}", field.name))
                            .collect::<String>()
                    ));
                }
            }
        };
        if let FieldRef::Field(id) = reference {
            let Some(field) = self.by_id(id) else {
                return Err(format!("{{{inside}}} names no field."));
            };
            if Some(id) == self.own {
                return Err("A formula can't use its own field.".to_owned());
            }
            if field.archived {
                return Err(format!(
                    "\"{}\" is archived. Restore it to use it in a formula.",
                    field.name
                ));
            }
            field_ty(field)?;
        }
        Ok(reference)
    }
}

/// Checks a formula as someone wrote it and gives its settings: fields by
/// ID, its result type, and whether it reads the time. The message says
/// what to fix.
pub fn compile(source: &str, scope: &Scope<'_>) -> Result<FormulaConfig, String> {
    let source = source.trim();
    if source.chars().count() > MAX_SOURCE_CHARACTERS {
        return Err(format!(
            "A formula is at most {MAX_SOURCE_CHARACTERS} characters."
        ));
    }
    let expr = parse::parse(source, &|inside| scope.resolve(inside))?;
    let references = expr.references();
    if references.len() > MAX_REFERENCES {
        return Err(format!(
            "A formula uses at most {MAX_REFERENCES} fields (each mention counts)."
        ));
    }
    let ty = eval::check(&expr, &|reference| match reference {
        FieldRef::Title => Ok(Ty::Text),
        FieldRef::Field(id) => scope
            .by_id(id)
            .ok_or_else(|| "A field it uses is gone.".to_owned())
            .and_then(field_ty),
    })?;
    if let Some(own) = scope.own {
        refuse_cycle(scope, own, &references)?;
    }
    Ok(FormulaConfig {
        source: print(&expr, &stored_name),
        result: ResultType::of(ty),
        volatile: expr
            .calls()
            .iter()
            .any(|name| functions::VOLATILE.contains(name)),
    })
}

/// The formula fields a stored formula reads.
pub(crate) fn formula_reads(config: &FormulaConfig) -> Vec<Uuid> {
    parse::parse(&config.source, &stored_reference)
        .map(|expr| {
            expr.references()
                .into_iter()
                .filter_map(|reference| match reference {
                    FieldRef::Field(id) => Some(id),
                    FieldRef::Title => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Refuses a formula for `own` that reads `references` if, through other
/// formulas, it would read itself; the message names the path.
fn refuse_cycle(scope: &Scope<'_>, own: Uuid, references: &[FieldRef]) -> Result<(), String> {
    let reads = |id: Uuid| -> Vec<Uuid> {
        scope
            .by_id(id)
            .and_then(|field| field.config.formula.as_ref())
            .map(formula_reads)
            .unwrap_or_default()
    };
    let name = |id: Uuid| {
        scope.by_id(id).map_or_else(
            || "a field".to_owned(),
            |field| format!("\"{}\"", field.name),
        )
    };
    // Depth first from each field it reads, keeping the path.
    let mut stack = references
        .iter()
        .filter_map(|reference| match reference {
            FieldRef::Field(id) => Some(vec![*id]),
            FieldRef::Title => None,
        })
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    while let Some(path) = stack.pop() {
        let last = *path.last().unwrap_or(&own);
        if last == own {
            let mut names = vec!["this formula".to_owned()];
            names.extend(path.iter().take(path.len() - 1).map(|id| name(*id)));
            names.push("this formula".to_owned());
            return Err(format!(
                "That would make the formula use itself: {}.",
                names.join(" uses ")
            ));
        }
        if !seen.insert(last) {
            continue;
        }
        for next in reads(last) {
            let mut longer = path.clone();
            longer.push(next);
            stack.push(longer);
        }
    }
    Ok(())
}

/// The formula as people read it: fields by today's names.
pub fn shown(config: &FormulaConfig, fields: &[FieldDefinition], title_name: &str) -> String {
    let Ok(expr) = parse::parse(&config.source, &stored_reference) else {
        return config.source.clone();
    };
    print(&expr, &|reference| match reference {
        FieldRef::Title => format!("{{{title_name}}}"),
        FieldRef::Field(id) => match fields.iter().find(|field| field.id == id) {
            Some(field) if !field.name.contains(['{', '}']) => format!("{{{}}}", field.name),
            _ => stored_name(reference),
        },
    })
}

/// Whether any active field is worked out: a formula or a rollup.
pub fn has_computed(definitions: &[FieldDefinition]) -> bool {
    definitions
        .iter()
        .any(|field| field.is_computed() && !field.archived)
}

// ---------------------------------------------------------------------------
// Running

/// One record, as its formulas read it.
pub struct RecordData<'a> {
    pub title: &'a str,
    pub number: i64,
    /// Its stored values, keyed by field ID.
    pub values: &'a Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    /// What the people, tasks, pages and records it names are called.
    pub labels: &'a HashMap<Uuid, String>,
    /// Its worked-out values known so far, keyed by field ID: a related
    /// record's stored ones, or the rollups of the record being worked out.
    pub computed: &'a Value,
}

struct Reader<'a> {
    definitions: &'a [FieldDefinition],
    record: &'a RecordData<'a>,
    computed: HashMap<Uuid, V>,
}

impl Reader<'_> {
    fn label(&self, id: &str) -> V {
        id.parse::<Uuid>()
            .ok()
            .and_then(|id| self.record.labels.get(&id))
            .map_or(V::Blank, |label| V::Text(label.clone()))
    }

    fn read(&self, field: &FieldDefinition) -> V {
        let record = self.record;
        match field.field_type {
            FieldType::Formula | FieldType::Rollup => {
                if let Some(value) = self.computed.get(&field.id) {
                    return value.clone();
                }
                return match (record.computed.get(field.id.to_string()), field.result()) {
                    (Some(stored), Some(result)) => from_stored(stored, result),
                    _ => V::Blank,
                };
            }
            FieldType::CreatedTime => return V::Time(record.created_at),
            FieldType::EditedTime => return V::Time(record.updated_at),
            FieldType::AutoId => return V::Num(record.number as f64),
            FieldType::CreatedBy => return self.label(&record.created_by.to_string()),
            FieldType::EditedBy => return self.label(&record.updated_by.to_string()),
            _ => {}
        }
        let Some(stored) = record.values.get(field.id.to_string()) else {
            return if field.field_type == FieldType::Checkbox {
                V::Bool(false)
            } else {
                V::Blank
            };
        };
        let option_name = |id: &Value| {
            id.as_str()
                .and_then(|id| field.option(id))
                .map_or(V::Blank, |option| V::Text(option.name.clone()))
        };
        match field.field_type {
            FieldType::Number => stored.as_f64().map_or(V::Blank, V::Num),
            FieldType::Date => stored
                .as_str()
                .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
                .map_or(V::Blank, V::Date),
            FieldType::Checkbox => V::Bool(stored.as_bool().unwrap_or(false)),
            FieldType::SingleChoice | FieldType::Status => option_name(stored),
            FieldType::MultipleChoice => V::List(
                stored
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .map(option_name)
                            .filter(|name| !name.is_blank())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            FieldType::Person | FieldType::Link => {
                stored.as_str().map_or(V::Blank, |id| self.label(id))
            }
            FieldType::Relation => V::List(
                stored
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(Value::as_str)
                            .map(|id| self.label(id))
                            .filter(|label| !label.is_blank())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            _ => match stored {
                Value::String(text) => V::Text(text.clone()),
                Value::Number(number) => number.as_f64().map_or(V::Blank, V::Num),
                Value::Bool(value) => V::Bool(*value),
                Value::Array(items) => V::List(
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|text| V::Text(text.to_owned())))
                        .collect(),
                ),
                _ => V::Blank,
            },
        }
    }
}

impl Fields for Reader<'_> {
    fn value(&self, reference: FieldRef) -> V {
        match reference {
            FieldRef::Title => V::Text(self.record.title.to_owned()),
            FieldRef::Field(id) => self
                .definitions
                .iter()
                .find(|field| field.id == id)
                .map_or(V::Blank, |field| self.read(field)),
        }
    }
}

/// A kept result as a formula reads it.
fn from_stored(stored: &Value, result: ResultType) -> V {
    if let Some(message) = stored.get("error").and_then(Value::as_str) {
        return V::Err(message.to_owned());
    }
    match (result, stored) {
        (ResultType::Number, Value::Number(number)) => number.as_f64().map_or(V::Blank, V::Num),
        (ResultType::Boolean, Value::Bool(value)) => V::Bool(*value),
        (ResultType::Date, Value::String(text)) => {
            NaiveDate::parse_from_str(text, "%Y-%m-%d").map_or(V::Blank, V::Date)
        }
        (ResultType::List, Value::Array(items)) => V::List(
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| V::Text(text.to_owned())))
                .collect(),
        ),
        (_, Value::String(text)) => V::Text(text.clone()),
        _ => V::Blank,
    }
}

/// A rollup's value over `related` (the active records its relation names,
/// each read with `related_fields`), as `computed_values` keeps it.
pub fn rollup(
    config: &RollupConfig,
    related_fields: &[FieldDefinition],
    related: &[RecordData<'_>],
) -> Option<Value> {
    if related.len() > MAX_ROLLUP_RECORDS {
        return Some(serde_json::json!({ "error": "Too many records to roll up." }));
    }
    let target = config
        .target
        .and_then(|id| related_fields.iter().find(|field| field.id == id));
    if config.target.is_some() && target.is_none() {
        return Some(serde_json::json!({ "error": "The field it rolls up is gone." }));
    }
    let values = related
        .iter()
        .map(|record| {
            target.map_or(V::Blank, |field| {
                Reader {
                    definitions: related_fields,
                    record,
                    computed: HashMap::new(),
                }
                .read(field)
            })
        })
        .collect::<Vec<_>>();
    if let Some(error) = values.iter().find(|value| value.is_err()) {
        return stored(error.clone(), config.result);
    }
    let filled = |value: &V| !value.is_blank() && *value != V::Bool(false);
    let total = related.len() as f64;
    let share = |count: usize| {
        if related.is_empty() {
            V::Blank
        } else {
            V::Num((count as f64 / total * 1000.0).round() / 10.0)
        }
    };
    let numbers = || {
        values.iter().filter_map(|value| match value {
            V::Num(number) => Some(*number),
            _ => None,
        })
    };
    let dates = || {
        values.iter().filter_map(|value| match value {
            V::Date(date) => Some(*date),
            V::Time(time) => Some(time.date_naive()),
            _ => None,
        })
    };
    let value = match config.aggregate {
        Aggregate::Count => V::Num(total),
        Aggregate::CountValues => {
            V::Num(values.iter().filter(|value| filled(value)).count() as f64)
        }
        Aggregate::CountUnique => {
            let mut seen = HashSet::new();
            for value in values.iter().filter(|value| filled(value)) {
                for item in value.items() {
                    if let Ok(text) = item.to_text() {
                        seen.insert(text.to_lowercase());
                    }
                }
            }
            V::Num(seen.len() as f64)
        }
        Aggregate::Sum => V::Num(numbers().sum()),
        Aggregate::Average => {
            let all = numbers().collect::<Vec<_>>();
            if all.is_empty() {
                V::Blank
            } else {
                V::Num(all.iter().sum::<f64>() / all.len() as f64)
            }
        }
        Aggregate::Min => numbers().reduce(f64::min).map_or(V::Blank, V::Num),
        Aggregate::Max => numbers().reduce(f64::max).map_or(V::Blank, V::Num),
        Aggregate::Earliest => dates().min().map_or(V::Blank, V::Date),
        Aggregate::Latest => dates().max().map_or(V::Blank, V::Date),
        Aggregate::PercentChecked => share(
            values
                .iter()
                .filter(|value| **value == V::Bool(true))
                .count(),
        ),
        Aggregate::PercentEmpty => share(values.iter().filter(|value| !filled(value)).count()),
        Aggregate::List => V::List(
            values
                .iter()
                .flat_map(|value| value.items())
                .filter(|item| !item.is_blank())
                .collect(),
        ),
    };
    stored(value, config.result)
}

/// A result as `computed_values` keeps it: None for no value (blank,
/// false, nothing in a list), `{"error": ...}` for an error.
fn stored(value: V, result: ResultType) -> Option<Value> {
    let error = |message: String| Some(serde_json::json!({ "error": message }));
    if let V::Err(message) = value {
        return error(message);
    }
    if value.is_blank() {
        return None;
    }
    match result {
        ResultType::Number => match value.to_num() {
            Ok(number) => match eval::number(number) {
                V::Num(number) => serde_json::Number::from_f64(number).map(|number| {
                    if number.as_f64().is_some_and(|value| value.fract() == 0.0) {
                        Value::from(number.as_f64().unwrap_or_default() as i64)
                    } else {
                        Value::Number(number)
                    }
                }),
                V::Err(message) => error(message),
                _ => None,
            },
            Err(V::Err(message)) => error(message),
            Err(_) => None,
        },
        ResultType::Text => match value.to_text() {
            Ok(text) if text.chars().count() > MAX_TEXT_RESULT_CHARACTERS => error(format!(
                "Text over {MAX_TEXT_RESULT_CHARACTERS} characters. Use LEFT to shorten it."
            )),
            Ok(text) if text.is_empty() => None,
            Ok(text) => Some(Value::String(text)),
            Err(V::Err(message)) => error(message),
            Err(_) => None,
        },
        ResultType::Boolean => match value.to_bool() {
            Ok(true) => Some(Value::Bool(true)),
            Ok(false) => None,
            Err(V::Err(message)) => error(message),
            Err(_) => None,
        },
        ResultType::Date => match value.to_date() {
            Ok(Some(date)) => Some(Value::String(date.to_string())),
            Ok(None) => None,
            Err(V::Err(message)) => error(message),
            Err(_) => None,
        },
        ResultType::List => {
            let mut items = Vec::new();
            for item in value.items() {
                // Items as text, as lists of names are: they filter and
                // show alike.
                match item {
                    V::Err(message) => return error(message),
                    other if other.is_blank() => {}
                    other => match other.to_text() {
                        Ok(text) => items.push(Value::String(text)),
                        Err(V::Err(message)) => return error(message),
                        Err(_) => {}
                    },
                }
            }
            (!items.is_empty()).then_some(Value::Array(items))
        }
    }
}

/// Every active formula of `definitions`, run on `record` in an order where
/// a formula comes after the formulas it reads: what the record's
/// `computed_values` should hold.
pub fn compute(
    definitions: &[FieldDefinition],
    record: &RecordData<'_>,
    now: DateTime<Utc>,
) -> Value {
    let formulas = definitions
        .iter()
        .filter(|field| field.field_type == FieldType::Formula && !field.archived)
        .filter_map(|field| {
            field
                .config
                .formula
                .as_ref()
                .map(|formula| (field, formula))
        })
        .collect::<Vec<_>>();
    let mut reader = Reader {
        definitions,
        record,
        computed: HashMap::new(),
    };
    let mut out = Map::new();
    // The record's rollups, worked out by the caller, kept as they are.
    for field in definitions
        .iter()
        .filter(|field| field.field_type == FieldType::Rollup && !field.archived)
    {
        if let Some(value) = record.computed.get(field.id.to_string()) {
            out.insert(field.id.to_string(), value.clone());
        }
    }
    // Kahn's order over the formulas reading each other; whatever is left in
    // a loop (only possible if fields changed under a saved formula) is an error.
    let ids = formulas
        .iter()
        .map(|(field, _)| field.id)
        .collect::<HashSet<_>>();
    let mut waiting = formulas
        .iter()
        .map(|(field, formula)| {
            let reads = formula_reads(formula)
                .into_iter()
                .filter(|id| ids.contains(id) && *id != field.id)
                .collect::<HashSet<_>>();
            (field.id, reads)
        })
        .collect::<HashMap<_, _>>();
    let mut done = HashSet::new();
    loop {
        let ready = formulas
            .iter()
            .filter(|(field, _)| {
                !done.contains(&field.id)
                    && waiting
                        .get(&field.id)
                        .is_some_and(|reads| reads.iter().all(|id| done.contains(id)))
            })
            .map(|(field, formula)| (*field, *formula))
            .collect::<Vec<_>>();
        if ready.is_empty() {
            break;
        }
        for (field, formula) in ready {
            let value = match parse::parse(&formula.source, &stored_reference) {
                Ok(expr) => run(&reader, &expr, now),
                Err(_) => V::Err("The formula can't be read. Edit it to fix it.".to_owned()),
            };
            reader.computed.insert(field.id, value.clone());
            if let Some(kept) = stored(value, formula.result) {
                out.insert(field.id.to_string(), kept);
            }
            done.insert(field.id);
            waiting.remove(&field.id);
        }
    }
    for (field, _) in &formulas {
        if !done.contains(&field.id) {
            out.insert(
                field.id.to_string(),
                serde_json::json!({ "error": "The formula uses itself through other formulas." }),
            );
        }
    }
    let computed = Value::Object(out);
    if serde_json::to_vec(&computed).map_or(0, |bytes| bytes.len()) > MAX_COMPUTED_BYTES {
        let mut cut = Map::new();
        for field in definitions
            .iter()
            .filter(|field| field.is_computed() && !field.archived)
        {
            cut.insert(
                field.id.to_string(),
                serde_json::json!({ "error": "This record's formula results are too large." }),
            );
        }
        return Value::Object(cut);
    }
    computed
}

fn run(reader: &Reader<'_>, expr: &Expr, now: DateTime<Utc>) -> V {
    Run::new(reader, now).eval(expr)
}

#[cfg(test)]
mod tests;
