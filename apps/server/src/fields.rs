//! Custom fields (AWD-89): the field types every kind of record shares.
//!
//! A field belongs to one project and one kind of record (`RecordKind`):
//! tasks now, collections' records (AWD-90) and modules' records later. This
//! module knows nothing about tasks. It owns:
//!
//! * the types (`FieldType`) and their settings (`FieldConfig`: choice
//!   options, a link's target);
//! * the definitions table (migration 20261105041500): list, create, change;
//! * checking a value someone wrote (`normalize_value`) and how a stored
//!   value reads (`display_value`);
//! * filters (`FieldClause`), sorts and groups as SQL over a JSONB column of
//!   values keyed by field ID, so every record kind filters the same way.
//!
//! A record kind stores its values as a JSONB object keyed by field ID
//! (`work_items.field_values` for tasks). A field with no value has no key.
//! Values that name something hold its ID: an option ID, an actor ID, or a
//! task's or page's ID. People and links are looked up by the record's own
//! module (`Normalized::Person` and `Normalized::Link`), since who and what
//! a record may point at depends on where it lives.
//!
//! Seam for modules (AWD-75): when the module manifests and shared contract
//! types land, the manifest's field and view pieces should point here.

use std::collections::HashSet;

use chrono::NaiveDate;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{error::AppError, work_item_labels};

/// Active fields one record kind of a project can have.
pub const MAX_ACTIVE_FIELDS: i64 = 50;
/// Every field row one record kind of a project keeps, archived ones too.
pub const MAX_FIELD_ROWS: i64 = 200;
pub const MAX_NAME_CHARACTERS: usize = 60;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 300;
/// Options one choice field keeps, archived ones too.
pub const MAX_OPTIONS: usize = 100;
pub const MAX_OPTION_NAME_CHARACTERS: usize = 60;
pub const MAX_TEXT_CHARACTERS: usize = 5_000;
pub const MAX_URL_CHARACTERS: usize = 2_048;
/// The largest number a number field holds, either side of zero.
pub const MAX_NUMBER: f64 = 1e15;
/// Field filters one list request can carry.
pub const MAX_CLAUSES: usize = 8;
/// Values one field filter can name.
pub const MAX_CLAUSE_VALUES: usize = 20;
const MAX_CLAUSE_VALUE_CHARACTERS: usize = 200;

// ---------------------------------------------------------------------------
// Types

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Number,
    Date,
    SingleChoice,
    MultipleChoice,
    Person,
    Url,
    Checkbox,
    // A link to a task or a page in the same project (`FieldConfig::target`).
    Link,
}

impl FieldType {
    pub const ALL: [Self; 9] = [
        Self::Text,
        Self::Number,
        Self::Date,
        Self::SingleChoice,
        Self::MultipleChoice,
        Self::Person,
        Self::Url,
        Self::Checkbox,
        Self::Link,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Date => "date",
            Self::SingleChoice => "single_choice",
            Self::MultipleChoice => "multiple_choice",
            Self::Person => "person",
            Self::Url => "url",
            Self::Checkbox => "checkbox",
            Self::Link => "link",
        }
    }

    /// A type as stored, or as people and agents tend to write it ("select",
    /// "multi select", "user", "boolean", "relation").
    pub fn parse(value: &str) -> Option<Self> {
        let squashed = value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        Some(match squashed.as_str() {
            "text" | "string" => Self::Text,
            "number" | "numeric" => Self::Number,
            "date" => Self::Date,
            "singlechoice" | "choice" | "select" | "singleselect" => Self::SingleChoice,
            "multiplechoice" | "multichoice" | "multiselect" | "multipleselect" | "choices" => {
                Self::MultipleChoice
            }
            "person" | "user" | "people" | "member" => Self::Person,
            "url" | "website" => Self::Url,
            "checkbox" | "boolean" | "bool" => Self::Checkbox,
            "link" | "relation" | "reference" => Self::Link,
            _ => return None,
        })
    }

    pub fn has_options(self) -> bool {
        matches!(self, Self::SingleChoice | Self::MultipleChoice)
    }

    /// Whether a list can be grouped by a field of this type.
    pub fn can_group(self) -> bool {
        matches!(self, Self::SingleChoice | Self::Person | Self::Checkbox)
    }

    /// The filter operations that make sense for it.
    pub fn ops(self) -> &'static [FieldOp] {
        use FieldOp::*;
        match self {
            Self::Text | Self::Url => &[Contains, Is, IsNot, Empty, NotEmpty],
            Self::Number | Self::Date => &[Is, IsNot, Gt, Gte, Lt, Lte, Empty, NotEmpty],
            Self::SingleChoice | Self::MultipleChoice | Self::Person | Self::Link => {
                &[Is, IsNot, Empty, NotEmpty]
            }
            Self::Checkbox => &[Is, Empty, NotEmpty],
        }
    }
}

/// The kind of record a field belongs to. Collections (AWD-90) add theirs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Task,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkTarget {
    Task,
    Page,
}

impl LinkTarget {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "task" | "tasks" => Some(Self::Task),
            "page" | "pages" => Some(Self::Page),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceOption {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    /// Hidden from pickers; records that have it keep it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

/// A field's settings, as `field_definitions.config` stores them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConfig {
    /// Choice fields: every option, in order, archived ones too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ChoiceOption>,
    /// Link fields: what they link to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LinkTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDefinition {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub field_type: FieldType,
    pub config: FieldConfig,
    /// 0-based order among the record kind's fields.
    pub position: i32,
    pub archived: bool,
    pub version: i64,
}

impl FieldDefinition {
    /// The option with this ID, archived or not.
    pub fn option(&self, id: &str) -> Option<&ChoiceOption> {
        self.config
            .options
            .iter()
            .find(|option| option.id.to_string() == id)
    }

    /// An active option by ID or name, ignoring case.
    fn active_option(&self, reference: &str) -> Option<&ChoiceOption> {
        let reference = reference.trim();
        let by_name = collapse(reference);
        self.config.options.iter().find(|option| {
            !option.archived
                && (option.id.to_string() == reference.to_ascii_lowercase()
                    || option.name.to_lowercase() == by_name.to_lowercase())
        })
    }

    fn active_option_names(&self) -> String {
        let names = self
            .config
            .options
            .iter()
            .filter(|option| !option.archived)
            .map(|option| option.name.as_str())
            .collect::<Vec<_>>();
        if names.is_empty() {
            "it has no options yet".to_owned()
        } else {
            names.join(", ")
        }
    }
}

#[derive(FromRow)]
struct DefinitionRow {
    id: Uuid,
    name: String,
    description: String,
    field_type: String,
    config: Value,
    position: i32,
    archived: bool,
    version: i64,
}

impl TryFrom<DefinitionRow> for FieldDefinition {
    type Error = AppError;

    fn try_from(row: DefinitionRow) -> Result<Self, AppError> {
        Ok(Self {
            id: row.id,
            name: row.name,
            description: row.description,
            field_type: FieldType::parse(&row.field_type)
                .ok_or(AppError::Internal("stored field type"))?,
            config: serde_json::from_value(row.config)
                .map_err(|_| AppError::Internal("stored field config"))?,
            position: row.position,
            archived: row.archived,
            version: row.version,
        })
    }
}

/// A field as the web app and agents see it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldView {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// Choice fields: the options, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionView>,
    /// Link fields: "task" or "page".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LinkTarget>,
    /// 1 is first.
    pub position: i32,
    pub archived: bool,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptionView {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

impl From<&FieldDefinition> for FieldView {
    fn from(field: &FieldDefinition) -> Self {
        Self {
            id: field.id.to_string(),
            name: field.name.clone(),
            description: field.description.clone(),
            field_type: field.field_type,
            options: field
                .config
                .options
                .iter()
                .map(|option| OptionView {
                    id: option.id.to_string(),
                    name: option.name.clone(),
                    color: option.color.clone(),
                    archived: option.archived,
                })
                .collect(),
            target: field.config.target,
            position: field.position + 1,
            archived: field.archived,
            version: field.version,
        }
    }
}

// ---------------------------------------------------------------------------
// Names and settings

fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A field or option name on one line: trimmed, runs of whitespace collapsed,
/// 1 to `max` characters, no control characters, and not shaped like an ID,
/// so "ID or name" is never ambiguous.
fn normalize_one_line(value: &str, max: usize) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let name = collapse(value);
    let length = name.chars().count();
    if length == 0 || length > max || name.parse::<Uuid>().is_ok() {
        return None;
    }
    Some(name)
}

pub fn normalize_name(value: &str) -> Option<String> {
    normalize_one_line(value, MAX_NAME_CHARACTERS)
}

/// What a field is for, on one line. Empty clears it.
pub fn normalize_description(value: &str) -> Option<String> {
    let value = value.trim();
    if value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
        || value.chars().count() > MAX_DESCRIPTION_CHARACTERS
    {
        return None;
    }
    Some(value.to_owned())
}

/// A choice option as someone writes it: an existing option keeps its ID.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionInput {
    /// An existing option's ID, to keep it (and every value that uses it)
    /// through a rename. Leave it out for a new option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// A color: #rrggbb, or red, orange, yellow, green, teal, blue, purple,
    /// pink or gray. Left out, it's picked from the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Why a field's settings were refused: the argument and what would work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigProblem {
    pub field: &'static str,
    pub message: String,
}

fn problem(field: &'static str, message: impl Into<String>) -> ConfigProblem {
    ConfigProblem {
        field,
        message: message.into(),
    }
}

/// The settings a field gets: `current` for a field being changed (its
/// type never changes), `options` replacing the option list when given, and
/// `target` for a new link field. An existing option left out of `options`
/// is archived, never removed, so records that have it keep it.
pub fn normalize_config(
    field_type: FieldType,
    current: Option<&FieldConfig>,
    options: Option<&[OptionInput]>,
    target: Option<&str>,
) -> Result<FieldConfig, ConfigProblem> {
    let mut config = current.cloned().unwrap_or_default();
    if let Some(target) = target {
        if field_type != FieldType::Link {
            return Err(problem("target", "Only link fields have a target."));
        }
        let target = LinkTarget::parse(target)
            .ok_or_else(|| problem("target", "A link field links to \"task\" or \"page\"."))?;
        if current.is_some_and(|current| current.target != Some(target)) {
            return Err(problem(
                "target",
                "A link field's target can't change. Add a new field instead.",
            ));
        }
        config.target = Some(target);
    }
    if field_type == FieldType::Link && config.target.is_none() {
        return Err(problem(
            "target",
            "Say what a link field links to: \"task\" or \"page\".",
        ));
    }
    if let Some(options) = options {
        if !field_type.has_options() {
            return Err(problem("options", "Only choice fields have options."));
        }
        let mut next = Vec::with_capacity(options.len());
        let mut names = HashSet::new();
        for input in options {
            let name = normalize_one_line(&input.name, MAX_OPTION_NAME_CHARACTERS).ok_or_else(
                || {
                    problem(
                        "options",
                        format!(
                            "Give each option a name of 1 to {MAX_OPTION_NAME_CHARACTERS} characters, on one line."
                        ),
                    )
                },
            )?;
            if !names.insert(name.to_lowercase()) {
                return Err(problem(
                    "options",
                    format!("Two options are called \"{name}\". Give each its own name."),
                ));
            }
            let color = match input.color.as_deref() {
                Some(color) => work_item_labels::normalize_color(color).ok_or_else(|| {
                    problem(
                        "options",
                        "Use a color such as #3b82f6, or red, orange, yellow, green, teal, blue, purple, pink or gray.",
                    )
                })?,
                None => None
                    .or_else(|| {
                        input.id.as_deref().and_then(|id| {
                            config
                                .options
                                .iter()
                                .find(|option| option.id.to_string() == id.trim().to_ascii_lowercase())
                                .map(|option| option.color.clone())
                        })
                    })
                    .unwrap_or_else(|| work_item_labels::default_color(&name).to_owned()),
            };
            let id = match input.id.as_deref() {
                Some(id) => {
                    let id = id
                        .trim()
                        .parse::<Uuid>()
                        .ok()
                        .filter(|id| config.options.iter().any(|option| option.id == *id));
                    id.ok_or_else(|| {
                        problem(
                            "options",
                            format!("\"{name}\" names an option ID this field doesn't have. Leave the ID out for a new option."),
                        )
                    })?
                }
                None => Uuid::now_v7(),
            };
            if next.iter().any(|option: &ChoiceOption| option.id == id) {
                return Err(problem("options", "Each option can appear once."));
            }
            next.push(ChoiceOption {
                id,
                name,
                color,
                archived: false,
            });
        }
        // Options left out are archived, after the active ones.
        for option in &config.options {
            if !next.iter().any(|kept| kept.id == option.id) {
                if names.contains(&option.name.to_lowercase()) && !option.archived {
                    return Err(problem(
                        "options",
                        format!(
                            "\"{}\" is already an option. Pass its ID to keep it.",
                            option.name
                        ),
                    ));
                }
                next.push(ChoiceOption {
                    archived: true,
                    ..option.clone()
                });
            }
        }
        if next.len() > MAX_OPTIONS {
            return Err(problem(
                "options",
                format!("A field keeps at most {MAX_OPTIONS} options, archived ones included."),
            ));
        }
        config.options = next;
    }
    Ok(config)
}

// ---------------------------------------------------------------------------
// Values

/// A value someone wrote, checked against its field.
#[derive(Clone, Debug, PartialEq)]
pub enum Normalized {
    /// No value: null, "", false for a checkbox, or no choices.
    Clear,
    /// The value as stored.
    Value(Value),
    /// A person, as written (an ID, an @handle, "me"); the record's module
    /// finds them among those who can be named there.
    Person(String),
    /// A task or page, as written (an ID, a key such as AWD-12, a link);
    /// the record's module finds it where the record lives.
    Link(String),
}

fn text_is_clean(value: &str) -> bool {
    !value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
}

fn number_value(value: f64) -> Option<Value> {
    if !value.is_finite() || value.abs() > MAX_NUMBER {
        return None;
    }
    // Whole numbers are stored as integers, so 3 reads back as 3, not 3.0.
    if value.fract() == 0.0 {
        return Some(Value::Number(Number::from(value as i64)));
    }
    Number::from_f64(value).map(Value::Number)
}

fn as_number(input: &Value) -> Option<f64> {
    match input {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().replace([',', '_'], "").parse::<f64>().ok(),
        _ => None,
    }
}

fn as_bool(input: &Value) -> Option<bool> {
    match input {
        Value::Bool(value) => Some(*value),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "checked" | "on" | "1" => Some(true),
            "false" | "no" | "n" | "unchecked" | "off" | "0" | "" => Some(false),
            _ => None,
        },
        Value::Number(number) => number.as_i64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }),
        _ => None,
    }
}

/// An http or https address, given a scheme when it has none.
pub fn normalize_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_URL_CHARACTERS || !text_is_clean(value) {
        return None;
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let parsed = url::Url::parse(&candidate).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return None;
    }
    (candidate.chars().count() <= MAX_URL_CHARACTERS).then_some(candidate)
}

/// Checks a value written for `field`. The message says what would work.
pub fn normalize_value(field: &FieldDefinition, input: &Value) -> Result<Normalized, String> {
    if input.is_null() {
        return Ok(Normalized::Clear);
    }
    let name = &field.name;
    let blank = input.as_str().is_some_and(|text| text.trim().is_empty());
    match field.field_type {
        FieldType::Text => {
            let Value::String(text) = input else {
                return Err(format!("\"{name}\" takes text."));
            };
            let text = text.trim();
            if text.is_empty() {
                return Ok(Normalized::Clear);
            }
            if text.chars().count() > MAX_TEXT_CHARACTERS || !text_is_clean(text) {
                return Err(format!(
                    "\"{name}\" takes up to {MAX_TEXT_CHARACTERS} characters, with no control characters except line breaks and tabs."
                ));
            }
            Ok(Normalized::Value(Value::String(text.to_owned())))
        }
        FieldType::Number => {
            if blank {
                return Ok(Normalized::Clear);
            }
            as_number(input)
                .and_then(number_value)
                .map(Normalized::Value)
                .ok_or_else(|| format!("\"{name}\" takes a number, such as 3 or 2.5."))
        }
        FieldType::Date => {
            if blank {
                return Ok(Normalized::Clear);
            }
            input
                .as_str()
                .and_then(|text| NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok())
                .map(|date| Normalized::Value(Value::String(date.to_string())))
                .ok_or_else(|| format!("\"{name}\" takes a date as YYYY-MM-DD."))
        }
        FieldType::SingleChoice => {
            if blank {
                return Ok(Normalized::Clear);
            }
            let reference = input.as_str().unwrap_or_default();
            field
                .active_option(reference)
                .map(|option| Normalized::Value(Value::String(option.id.to_string())))
                .ok_or_else(|| {
                    format!(
                        "\"{name}\" takes one of its options: {}.",
                        field.active_option_names()
                    )
                })
        }
        FieldType::MultipleChoice => {
            let references = match input {
                Value::Array(items) => items.clone(),
                Value::String(text) if text.trim().is_empty() => Vec::new(),
                // One option, or several written "A, B" (imports, rules).
                Value::String(text) if field.active_option(text).is_none() => text
                    .split([',', ';', '|'])
                    .map(|part| Value::String(part.trim().to_owned()))
                    .filter(|part| part.as_str().is_some_and(|part| !part.is_empty()))
                    .collect(),
                Value::String(_) => vec![input.clone()],
                _ => {
                    return Err(format!(
                        "\"{name}\" takes a list of its options: {}.",
                        field.active_option_names()
                    ));
                }
            };
            let mut chosen = HashSet::new();
            for reference in &references {
                let option = reference
                    .as_str()
                    .and_then(|text| field.active_option(text))
                    .ok_or_else(|| {
                        format!(
                            "\"{name}\" takes a list of its options: {}.",
                            field.active_option_names()
                        )
                    })?;
                chosen.insert(option.id);
            }
            if chosen.is_empty() {
                return Ok(Normalized::Clear);
            }
            // Stored in the field's option order, so equal sets compare equal.
            Ok(Normalized::Value(Value::Array(
                field
                    .config
                    .options
                    .iter()
                    .filter(|option| chosen.contains(&option.id))
                    .map(|option| Value::String(option.id.to_string()))
                    .collect(),
            )))
        }
        FieldType::Person => match input {
            Value::String(text) if text.trim().is_empty() => Ok(Normalized::Clear),
            Value::String(text) => Ok(Normalized::Person(text.trim().to_owned())),
            _ => Err(format!(
                "\"{name}\" takes a person: \"me\", an @handle, or an ID."
            )),
        },
        FieldType::Url => match input {
            Value::String(text) if text.trim().is_empty() => Ok(Normalized::Clear),
            Value::String(text) => normalize_url(text)
                .map(|url| Normalized::Value(Value::String(url)))
                .ok_or_else(|| {
                    format!("\"{name}\" takes a web address, such as https://example.com.")
                }),
            _ => Err(format!("\"{name}\" takes a web address.")),
        },
        FieldType::Checkbox => match as_bool(input) {
            Some(true) => Ok(Normalized::Value(Value::Bool(true))),
            Some(false) => Ok(Normalized::Clear),
            None => Err(format!("\"{name}\" takes true or false.")),
        },
        FieldType::Link => match input {
            Value::String(text) if text.trim().is_empty() => Ok(Normalized::Clear),
            Value::String(text) => Ok(Normalized::Link(text.trim().to_owned())),
            _ => Err(match field.config.target {
                Some(LinkTarget::Page) => format!("\"{name}\" takes a page: its ID or link."),
                _ => format!("\"{name}\" takes a task: its key (such as AWD-12), ID or link."),
            }),
        },
    }
}

/// How a stored value reads: option names instead of IDs, and whatever
/// `name_of` says a person, task or page is called (or the ID, when it
/// doesn't know).
pub fn display_value(
    field: &FieldDefinition,
    value: &Value,
    name_of: &dyn Fn(&str) -> Option<String>,
) -> Value {
    let option_name = |id: &Value| -> Value {
        id.as_str()
            .and_then(|id| field.option(id))
            .map_or(Value::Null, |option| Value::String(option.name.clone()))
    };
    match field.field_type {
        FieldType::SingleChoice => option_name(value),
        FieldType::MultipleChoice => Value::Array(
            value
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .map(option_name)
                        .filter(|name| !name.is_null())
                        .collect()
                })
                .unwrap_or_default(),
        ),
        FieldType::Person | FieldType::Link => match value.as_str() {
            Some(id) => Value::String(name_of(id).unwrap_or_else(|| id.to_owned())),
            None => Value::Null,
        },
        _ => value.clone(),
    }
}

/// A value as one line of text, for exports and history.
pub fn display_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(true) => "Yes".to_owned(),
        Value::Bool(false) => "No".to_owned(),
        Value::Array(items) => items
            .iter()
            .map(display_text)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// The IDs of people and linked records a stored value names, for the
/// record's module to look up in one go.
pub fn referenced_id(field: &FieldDefinition, value: &Value) -> Option<Uuid> {
    matches!(field.field_type, FieldType::Person | FieldType::Link)
        .then(|| value.as_str()?.parse().ok())
        .flatten()
}

// ---------------------------------------------------------------------------
// Filters

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FieldOp {
    // Has any of the values (for a multiple choice, any of them is among its
    // choices).
    Is,
    // Has none of the values; records without a value count.
    IsNot,
    // Has no value.
    Empty,
    // Has a value.
    NotEmpty,
    // The text has the value in it, ignoring case.
    Contains,
    // Greater than the value (a number or a YYYY-MM-DD date).
    Gt,
    Gte,
    Lt,
    Lte,
}

impl FieldOp {
    fn takes_values(self) -> bool {
        !matches!(self, Self::Empty | Self::NotEmpty)
    }

    fn takes_one_value(self) -> bool {
        matches!(
            self,
            Self::Contains | Self::Gt | Self::Gte | Self::Lt | Self::Lte
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Is => "is",
            Self::IsNot => "isNot",
            Self::Empty => "empty",
            Self::NotEmpty => "notEmpty",
            Self::Contains => "contains",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
        }
    }
}

/// One field filter, with its values as stored: option IDs, actor IDs,
/// task or page IDs, "true" for a checked checkbox, numbers and
/// YYYY-MM-DD dates as text. The SQL doesn't need the field's type, so a
/// saved view's filters read the same everywhere.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldClause {
    pub field: Uuid,
    pub op: FieldOp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

impl FieldClause {
    fn shape_is_valid(&self) -> bool {
        if self.op.takes_values() {
            !self.values.is_empty()
                && self.values.len() <= MAX_CLAUSE_VALUES
                && (!self.op.takes_one_value() || self.values.len() == 1)
                && self.values.iter().all(|value| {
                    !value.trim().is_empty() && value.chars().count() <= MAX_CLAUSE_VALUE_CHARACTERS
                })
        } else {
            self.values.is_empty()
        }
    }
}

/// Field filters as a list request carries them: a JSON array of
/// `{"field", "op", "values"}`. None when it isn't one.
pub fn parse_clauses(param: &str) -> Option<Vec<FieldClause>> {
    let param = param.trim();
    if param.is_empty() {
        return Some(Vec::new());
    }
    let clauses: Vec<FieldClause> = serde_json::from_str(param).ok()?;
    (clauses.len() <= MAX_CLAUSES && clauses.iter().all(FieldClause::shape_is_valid))
        .then_some(clauses)
}

/// The clauses as a list request carries them.
pub fn clauses_param(clauses: &[FieldClause]) -> String {
    serde_json::to_string(clauses).unwrap_or_else(|_| "[]".to_owned())
}

/// "me" in a person field's clause as `reader`. Other fields keep "me" as
/// written: in a text or URL field it's the word.
pub fn resolve_me(clauses: &mut [FieldClause], definitions: &[FieldDefinition], reader: Uuid) {
    for clause in clauses {
        let person = definitions
            .iter()
            .any(|field| field.id == clause.field && field.field_type == FieldType::Person);
        if !person {
            continue;
        }
        for value in &mut clause.values {
            if value == "me" {
                *value = reader.to_string();
            }
        }
    }
}

/// Checks a clause against its field: the operation suits the type, and
/// the values are what the type stores. The message says what would work.
pub fn check_clause(field: &FieldDefinition, clause: &FieldClause) -> Result<(), String> {
    let name = &field.name;
    if !field.field_type.ops().contains(&clause.op) {
        let ops = field
            .field_type
            .ops()
            .iter()
            .map(|op| op.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("\"{name}\" filters with {ops}."));
    }
    for value in &clause.values {
        let ok = match (field.field_type, clause.op) {
            (FieldType::Number, _) => value.trim().parse::<f64>().is_ok_and(f64::is_finite),
            (FieldType::Date, _) => NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").is_ok(),
            (FieldType::SingleChoice | FieldType::MultipleChoice, _) => {
                field.option(value).is_some()
            }
            (FieldType::Person | FieldType::Link, _) => value.parse::<Uuid>().is_ok(),
            (FieldType::Checkbox, _) => value == "true",
            (FieldType::Text | FieldType::Url, _) => true,
        };
        if !ok {
            return Err(match field.field_type {
                FieldType::Number => format!("\"{name}\" filters by numbers."),
                FieldType::Date => format!("\"{name}\" filters by dates as YYYY-MM-DD."),
                FieldType::SingleChoice | FieldType::MultipleChoice => format!(
                    "\"{name}\" filters by its options: {}.",
                    field.active_option_names()
                ),
                FieldType::Checkbox => format!("\"{name}\" filters with is true, or empty."),
                _ => format!("\"{name}\" can't filter by that."),
            });
        }
    }
    Ok(())
}

/// Turns what an agent wrote in a filter into stored values: option names
/// into option IDs, yes/no into "true" (a checkbox's "is false" becomes
/// "empty"). People and links are left for the record's module.
pub fn clause_from_written(
    field: &FieldDefinition,
    op: FieldOp,
    values: &[String],
) -> Result<FieldClause, String> {
    let mut op = op;
    let mut stored = Vec::with_capacity(values.len());
    for value in values {
        match field.field_type {
            FieldType::SingleChoice | FieldType::MultipleChoice => {
                let option = field
                    .config
                    .options
                    .iter()
                    .find(|option| {
                        option.id.to_string() == value.trim().to_ascii_lowercase()
                            || option.name.to_lowercase() == collapse(value).to_lowercase()
                    })
                    .ok_or_else(|| {
                        format!(
                            "\"{}\" has no option \"{value}\". Its options: {}.",
                            field.name,
                            field.active_option_names()
                        )
                    })?;
                stored.push(option.id.to_string());
            }
            FieldType::Checkbox => match as_bool(&Value::String(value.clone())) {
                Some(true) => stored.push("true".to_owned()),
                Some(false) if op == FieldOp::Is => op = FieldOp::Empty,
                Some(false) => op = FieldOp::NotEmpty,
                None => return Err(format!("\"{}\" filters with true or false.", field.name)),
            },
            FieldType::Date => stored.push(value.trim().to_owned()),
            FieldType::Number => stored.push(value.trim().replace([',', '_'], "")),
            _ => stored.push(value.clone()),
        }
    }
    if !op.takes_values() {
        stored.clear();
    }
    let clause = FieldClause {
        field: field.id,
        op,
        values: stored,
    };
    if !clause.shape_is_valid() {
        return Err(if op.takes_one_value() {
            format!("\"{}\" {} takes one value.", field.name, op.as_str())
        } else if op.takes_values() {
            format!(
                "\"{}\" {} takes 1 to {MAX_CLAUSE_VALUES} values.",
                field.name,
                op.as_str()
            )
        } else {
            format!("\"{}\" {} takes no values.", field.name, op.as_str())
        });
    }
    check_clause(field, &clause)?;
    Ok(clause)
}

/// `column -> 'field id'`. Field IDs are UUIDs, so they're safe to write
/// into the SQL; everything a caller wrote is bound.
fn value_at(column: &str, field: Uuid) -> String {
    format!("({column} -> '{field}')")
}

fn text_at(column: &str, field: Uuid) -> String {
    format!("({column} ->> '{field}')")
}

/// The stored value's elements as text: a list's items, or the value alone.
fn elements(column: &str, field: Uuid) -> String {
    let value = value_at(column, field);
    format!(
        "jsonb_array_elements_text(CASE WHEN jsonb_typeof({value}) = 'array' THEN {value} \
         WHEN {value} IS NULL THEN '[]'::jsonb ELSE jsonb_build_array({value}) END)"
    )
}

/// Appends ` AND ...` for one clause over the JSONB `column` of values.
pub fn push_clause(builder: &mut QueryBuilder<'_, Postgres>, column: &str, clause: &FieldClause) {
    let field = clause.field;
    let value = value_at(column, field);
    let text = text_at(column, field);
    match clause.op {
        FieldOp::Empty => {
            builder.push(format!(" AND {value} IS NULL"));
        }
        FieldOp::NotEmpty => {
            builder.push(format!(" AND {value} IS NOT NULL"));
        }
        FieldOp::Is | FieldOp::IsNot => {
            let written = clause
                .values
                .iter()
                .map(|value| value.trim().to_owned())
                .collect::<Vec<_>>();
            builder.push(if clause.op == FieldOp::Is {
                " AND EXISTS (SELECT 1 FROM "
            } else {
                " AND NOT EXISTS (SELECT 1 FROM "
            });
            builder.push(format!(
                "{} AS element(value) WHERE element.value = ANY(",
                elements(column, field)
            ));
            builder.push_bind(written);
            // "3.0" is 3 only where a number is stored (a number field):
            // a text field's "0012" stays the text it is.
            builder.push(format!(
                ") OR (jsonb_typeof({value}) = 'number' AND element.value = ANY("
            ));
            builder.push_bind(match_values(&clause.values));
            builder.push(")))");
        }
        FieldOp::Contains => {
            let needle = clause.values.first().cloned().unwrap_or_default();
            let escaped = needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            builder.push(format!(
                " AND jsonb_typeof({value}) = 'string' AND {text} ILIKE "
            ));
            builder.push_bind(format!("%{escaped}%"));
        }
        FieldOp::Gt | FieldOp::Gte | FieldOp::Lt | FieldOp::Lte => {
            let operator = match clause.op {
                FieldOp::Gt => ">",
                FieldOp::Gte => ">=",
                FieldOp::Lt => "<",
                _ => "<=",
            };
            let bound = clause
                .values
                .first()
                .map(|value| value.trim())
                .unwrap_or("");
            if let Ok(date) = NaiveDate::parse_from_str(bound, "%Y-%m-%d") {
                // ISO dates compare as text.
                builder.push(format!(
                    " AND (CASE WHEN jsonb_typeof({value}) = 'string' THEN {text} END) {operator} "
                ));
                builder.push_bind(date.to_string());
            } else {
                let number = bound
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite());
                builder.push(format!(
                    " AND (CASE WHEN jsonb_typeof({value}) = 'number' THEN ({text})::double precision END) {operator} "
                ));
                builder.push_bind(number);
            }
        }
    }
}

/// What `Is` compares a stored element with. A number filter's "3" must
/// match a stored 3 or 3.0, so numbers are matched as JSON writes them.
fn match_values(values: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim();
        out.push(value.to_owned());
        if let Some(number) = value
            .parse::<f64>()
            .ok()
            .and_then(number_value)
            .map(|number| number.to_string())
            && number != value
        {
            out.push(number);
        }
    }
    out
}

pub fn push_clauses(
    builder: &mut QueryBuilder<'_, Postgres>,
    column: &str,
    clauses: &[FieldClause],
) {
    for clause in clauses {
        push_clause(builder, column, clause);
    }
}

// ---------------------------------------------------------------------------
// Sorting and grouping

/// One sort key in SQL: a non-null expression, the type a cursor value is
/// cast back to, and whether it follows the sort's direction (the key that
/// puts records without a value last never does).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortKey {
    pub expr: String,
    pub ty: &'static str,
    pub follows_direction: bool,
}

fn key(expr: String, ty: &'static str) -> SortKey {
    SortKey {
        expr,
        ty,
        follows_direction: true,
    }
}

fn uuid_array(ids: impl Iterator<Item = Uuid>) -> String {
    let items = ids.map(|id| format!("'{id}'")).collect::<Vec<_>>();
    if items.is_empty() {
        "ARRAY[]::text[]".to_owned()
    } else {
        format!("ARRAY[{}]::text[]", items.join(", "))
    }
}

/// How records sort by `field`: records without a value last, then by value
/// (choices in option order, people by name, linked tasks by number, linked
/// pages by title). `column` is the JSONB column of values and `project`
/// the record's project ID column.
pub fn sort_keys(field: &FieldDefinition, column: &str, project: &str) -> Vec<SortKey> {
    let value = value_at(column, field.id);
    let text = text_at(column, field.id);
    let empty_last = SortKey {
        expr: format!("({value} IS NULL)::integer"),
        ty: "integer",
        follows_direction: false,
    };
    let by_value = match field.field_type {
        FieldType::Text | FieldType::Url => key(format!("lower(COALESCE({text}, ''))"), "text"),
        FieldType::Date => key(format!("COALESCE({text}, '')"), "text"),
        FieldType::Number => key(
            format!(
                "COALESCE(CASE WHEN jsonb_typeof({value}) = 'number' THEN ({text})::double precision END, 0)"
            ),
            "double precision",
        ),
        FieldType::Checkbox => {
            return vec![key(format!("({value} IS NULL)::integer"), "integer")];
        }
        FieldType::SingleChoice => key(
            format!(
                "COALESCE(array_position({}, {text}), 0)",
                uuid_array(field.config.options.iter().map(|option| option.id))
            ),
            "integer",
        ),
        FieldType::MultipleChoice => key(
            format!(
                "COALESCE((SELECT min(array_position({}, element.value)) FROM {} AS element(value)), 0)",
                uuid_array(field.config.options.iter().map(|option| option.id)),
                elements(column, field.id)
            ),
            "integer",
        ),
        FieldType::Person => key(
            format!(
                "lower(COALESCE((SELECT sort_person.display_name FROM project_memberships sort_person \
                 WHERE sort_person.project_id = {project} AND sort_person.actor_id::text = {text}), ''))"
            ),
            "text",
        ),
        FieldType::Link => match field.config.target {
            Some(LinkTarget::Page) => key(
                format!(
                    "lower(COALESCE((SELECT sort_page.title FROM project_pages sort_page \
                     WHERE sort_page.project_id = {project} AND sort_page.id::text = {text}), ''))"
                ),
                "text",
            ),
            _ => key(
                format!(
                    "COALESCE((SELECT sort_task.number FROM work_items sort_task \
                     WHERE sort_task.project_id = {project} AND sort_task.id::text = {text}), 0)"
                ),
                "bigint",
            ),
        },
    };
    vec![empty_last, by_value]
}

/// A list grouped by a field: the join it needs, the group's key ("none"
/// for no value), label and color, and the keys that order the groups.
#[derive(Clone, Debug)]
pub struct GroupSql {
    pub join: String,
    pub key: String,
    pub label: String,
    pub color: String,
    pub order: Vec<SortKey>,
}

/// SQL for grouping by a field whose type `can_group`. The join is aliased
/// `group_field`.
pub fn group_sql(
    field: Uuid,
    field_type: FieldType,
    column: &str,
    project: &str,
) -> Option<GroupSql> {
    let value = value_at(column, field);
    let text = text_at(column, field);
    Some(match field_type {
        FieldType::SingleChoice => GroupSql {
            join: format!(
                " LEFT JOIN LATERAL (SELECT choice.option ->> 'name' AS name, \
                   choice.option ->> 'color' AS color, choice.position \
                   FROM field_definitions group_definition \
                   CROSS JOIN LATERAL jsonb_array_elements(group_definition.config -> 'options') \
                     WITH ORDINALITY AS choice(option, position) \
                   WHERE group_definition.project_id = {project} AND group_definition.id = '{field}' \
                     AND choice.option ->> 'id' = {text}) group_field ON true"
            ),
            key: format!("CASE WHEN group_field.name IS NULL THEN 'none' ELSE {text} END"),
            label: "COALESCE(group_field.name, 'No value')".to_owned(),
            color: "group_field.color".to_owned(),
            order: vec![
                key("(group_field.name IS NULL)::integer".to_owned(), "integer"),
                key("COALESCE(group_field.position, 0)".to_owned(), "bigint"),
            ],
        },
        FieldType::Person => GroupSql {
            join: format!(
                " LEFT JOIN project_memberships group_field \
                   ON group_field.project_id = {project} AND group_field.actor_id::text = {text}"
            ),
            key: "COALESCE(group_field.actor_id::text, 'none')".to_owned(),
            label: "COALESCE(group_field.display_name, 'No one')".to_owned(),
            color: "NULL::text".to_owned(),
            order: vec![
                key(
                    "(group_field.actor_id IS NULL)::integer".to_owned(),
                    "integer",
                ),
                key(
                    "lower(COALESCE(group_field.display_name, ''))".to_owned(),
                    "text",
                ),
                key(
                    "COALESCE(group_field.actor_id::text, '')".to_owned(),
                    "text",
                ),
            ],
        },
        FieldType::Checkbox => GroupSql {
            join: String::new(),
            key: format!("CASE WHEN {value} = 'true'::jsonb THEN 'true' ELSE 'none' END"),
            label: format!(
                "CASE WHEN {value} = 'true'::jsonb THEN 'Checked' ELSE 'Not checked' END"
            ),
            color: "NULL::text".to_owned(),
            order: vec![key(
                format!("({value} IS DISTINCT FROM 'true'::jsonb)::integer"),
                "integer",
            )],
        },
        _ => return None,
    })
}

/// The clause that narrows a list to one group of a field.
pub fn group_clause(field: Uuid, key: &str) -> FieldClause {
    if key == "none" {
        FieldClause {
            field,
            op: FieldOp::Empty,
            values: Vec::new(),
        }
    } else {
        FieldClause {
            field,
            op: FieldOp::Is,
            values: vec![key.to_owned()],
        }
    }
}

// ---------------------------------------------------------------------------
// Definitions

const DEFINITION_COLUMNS: &str = "id, name, description, field_type, config, position, \
     archived_at IS NOT NULL AS archived, version";

/// A project's fields for one kind of record: active ones in order, then
/// archived ones.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    kind: RecordKind,
    include_archived: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    sqlx::query_as::<_, DefinitionRow>(&format!(
        "SELECT {DEFINITION_COLUMNS} FROM field_definitions \
         WHERE project_id = $1 AND record_kind = $2 AND ($3 OR archived_at IS NULL) \
         ORDER BY archived_at IS NOT NULL, position, id LIMIT {MAX_FIELD_ROWS}"
    ))
    .bind(project_id)
    .bind(kind.as_str())
    .bind(include_archived)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(FieldDefinition::try_from)
    .collect()
}

async fn definition(
    connection: &mut PgConnection,
    project_id: Uuid,
    kind: RecordKind,
    id: Uuid,
) -> Result<Option<FieldDefinition>, AppError> {
    sqlx::query_as::<_, DefinitionRow>(&format!(
        "SELECT {DEFINITION_COLUMNS} FROM field_definitions \
         WHERE project_id = $1 AND record_kind = $2 AND id = $3"
    ))
    .bind(project_id)
    .bind(kind.as_str())
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .map(FieldDefinition::try_from)
    .transpose()
}

/// Finds a field among `fields` by ID or name (ignoring case). A name finds
/// the active field first.
pub fn find<'a>(fields: &'a [FieldDefinition], reference: &str) -> Option<&'a FieldDefinition> {
    let reference = reference.trim();
    if let Ok(id) = reference.parse::<Uuid>() {
        return fields.iter().find(|field| field.id == id);
    }
    let name = collapse(reference).to_lowercase();
    fields
        .iter()
        .filter(|field| field.name.to_lowercase() == name)
        .min_by_key(|field| field.archived)
}

/// The names of the active fields, for messages.
pub fn active_names(fields: &[FieldDefinition]) -> String {
    let names = fields
        .iter()
        .filter(|field| !field.archived)
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    if names.is_empty() {
        "none yet".to_owned()
    } else {
        names.join(", ")
    }
}

/// A field to create, its name and description already normalized.
#[derive(Clone, Debug)]
pub struct NewField {
    pub name: String,
    pub description: String,
    pub field_type: FieldType,
    pub config: FieldConfig,
    /// 1-based place among the active fields; the end when None.
    pub position: Option<i32>,
}

#[derive(Clone, Debug)]
pub enum CreateFieldOutcome {
    Created(FieldDefinition),
    /// An active field already has the name, ignoring case.
    NameTaken(FieldDefinition),
    /// MAX_ACTIVE_FIELDS active fields, or MAX_FIELD_ROWS in all.
    ProjectFull,
}

/// Creates a field. The caller holds the project lock
/// (`work_item_commands::lock_membership`) and has checked the actor may
/// manage the project's fields.
pub async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    kind: RecordKind,
    actor_id: Uuid,
    new: NewField,
) -> Result<CreateFieldOutcome, AppError> {
    if normalize_name(&new.name).as_deref() != Some(new.name.as_str()) {
        return Err(AppError::Validation("name"));
    }
    if normalize_description(&new.description).as_deref() != Some(new.description.as_str()) {
        return Err(AppError::Validation("description"));
    }
    let existing = list(&mut *connection, project_id, kind, false).await?;
    if let Some(taken) = existing
        .iter()
        .find(|field| field.name.to_lowercase() == new.name.to_lowercase())
    {
        return Ok(CreateFieldOutcome::NameTaken(taken.clone()));
    }
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM field_definitions WHERE project_id = $1 AND record_kind = $2",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .fetch_one(&mut *connection)
    .await?;
    if existing.len() as i64 >= MAX_ACTIVE_FIELDS || total >= MAX_FIELD_ROWS {
        return Ok(CreateFieldOutcome::ProjectFull);
    }
    let id = Uuid::now_v7();
    let config =
        serde_json::to_value(&new.config).map_err(|_| AppError::Internal("encode field config"))?;
    sqlx::query(
        r#"
        INSERT INTO field_definitions (
            id, project_id, record_kind, name, description, field_type, config,
            position, created_by_actor_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7,
                (SELECT COALESCE(max(position) + 1, 0) FROM field_definitions
                 WHERE project_id = $2 AND record_kind = $3), $8)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(kind.as_str())
    .bind(&new.name)
    .bind(&new.description)
    .bind(new.field_type.as_str())
    .bind(config)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if let Some(position) = new.position {
        move_to(connection, project_id, kind, id, position).await?;
    }
    let created = definition(connection, project_id, kind, id)
        .await?
        .ok_or(AppError::Internal("created field is missing"))?;
    Ok(CreateFieldOutcome::Created(created))
}

/// Puts a field at a 1-based place among the active fields (archived ones
/// keep their places after them) and renumbers the rest.
async fn move_to(
    connection: &mut PgConnection,
    project_id: Uuid,
    kind: RecordKind,
    id: Uuid,
    position: i32,
) -> Result<(), AppError> {
    let mut order = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM field_definitions WHERE project_id = $1 AND record_kind = $2 \
         ORDER BY archived_at IS NOT NULL, position, id",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .fetch_all(&mut *connection)
    .await?;
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM field_definitions WHERE project_id = $1 AND record_kind = $2 \
         AND archived_at IS NULL AND id <> $3",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(id)
    .fetch_one(&mut *connection)
    .await?;
    order.retain(|other| *other != id);
    let index = usize::try_from(position.saturating_sub(1))
        .unwrap_or(0)
        .min(usize::try_from(active).unwrap_or(0));
    order.insert(index, id);
    let positions = (0..order.len() as i32).collect::<Vec<_>>();
    sqlx::query(
        r#"
        UPDATE field_definitions field
        SET position = placed.position
        FROM unnest($3::uuid[], $4::int[]) AS placed(id, position)
        WHERE field.project_id = $1 AND field.record_kind = $2 AND field.id = placed.id
          AND field.position <> placed.position
        "#,
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(&order)
    .bind(&positions)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Changes to one field, already normalized. None leaves a part as it is.
#[derive(Clone, Debug, Default)]
pub struct FieldChange {
    pub name: Option<String>,
    pub description: Option<String>,
    /// Settings from `normalize_config` against the field's current ones.
    pub config: Option<FieldConfig>,
    /// 1-based place among the active fields.
    pub position: Option<i32>,
    pub archived: Option<bool>,
    /// Checked only when something would change.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum ChangeFieldOutcome {
    Changed {
        field: FieldDefinition,
        previous: FieldDefinition,
        /// Any of name, description, options, position, archived.
        changed: Vec<&'static str>,
    },
    Unchanged(FieldDefinition),
    NotFound,
    VersionConflict(FieldDefinition),
    /// Another active field has the name.
    NameTaken(String),
    /// Restoring would pass MAX_ACTIVE_FIELDS.
    ProjectFull,
}

/// Renames, describes, reorders, archives or restores a field, or changes
/// its options. Archiving keeps every record's value, so a restore brings
/// them back; records' versions don't change. The caller holds the project
/// lock and has checked the actor may manage fields.
pub async fn change(
    connection: &mut PgConnection,
    project_id: Uuid,
    kind: RecordKind,
    field_id: Uuid,
    actor_id: Uuid,
    change: FieldChange,
) -> Result<ChangeFieldOutcome, AppError> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM field_definitions WHERE project_id = $1 AND record_kind = $2 AND id = $3 FOR UPDATE",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(field_id)
    .fetch_optional(&mut *connection)
    .await?;
    if locked.is_none() {
        return Ok(ChangeFieldOutcome::NotFound);
    }
    let current = definition(connection, project_id, kind, field_id)
        .await?
        .ok_or(AppError::Internal("locked field is missing"))?;
    let name = change.name.clone().unwrap_or_else(|| current.name.clone());
    let description = change
        .description
        .clone()
        .unwrap_or_else(|| current.description.clone());
    let config = change
        .config
        .clone()
        .unwrap_or_else(|| current.config.clone());
    let archived = change.archived.unwrap_or(current.archived);
    if normalize_name(&name).as_deref() != Some(name.as_str())
        || normalize_description(&description).as_deref() != Some(description.as_str())
    {
        return Err(AppError::Validation("field"));
    }
    let active = list(&mut *connection, project_id, kind, false).await?;
    let current_place = active
        .iter()
        .position(|field| field.id == field_id)
        .map(|index| index as i32 + 1);
    let moves = change
        .position
        .filter(|_| !archived)
        .is_some_and(|position| {
            Some(position.clamp(1, active.len().max(1) as i32)) != current_place
        });
    let mut changed = Vec::new();
    if name != current.name {
        changed.push("name");
    }
    if description != current.description {
        changed.push("description");
    }
    if config != current.config {
        changed.push("options");
    }
    if moves {
        changed.push("position");
    }
    if archived != current.archived {
        changed.push("archived");
    }
    if changed.is_empty() {
        return Ok(ChangeFieldOutcome::Unchanged(current));
    }
    if change
        .expected_version
        .is_some_and(|expected| expected != current.version)
    {
        return Ok(ChangeFieldOutcome::VersionConflict(current));
    }
    let restoring = current.archived && !archived;
    if !archived
        && (restoring || name != current.name)
        && let Some(other) = active
            .iter()
            .find(|field| field.id != field_id && field.name.to_lowercase() == name.to_lowercase())
    {
        return Ok(ChangeFieldOutcome::NameTaken(other.name.clone()));
    }
    if restoring && active.len() as i64 >= MAX_ACTIVE_FIELDS {
        return Ok(ChangeFieldOutcome::ProjectFull);
    }
    let config_value =
        serde_json::to_value(&config).map_err(|_| AppError::Internal("encode field config"))?;
    sqlx::query(
        r#"
        UPDATE field_definitions
        SET name = $4, description = $5, config = $6,
            archived_at = CASE WHEN $7 THEN COALESCE(archived_at, now()) END,
            archived_by_actor_id = CASE WHEN $7 THEN COALESCE(archived_by_actor_id, $8) END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND record_kind = $2 AND id = $3
        "#,
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(field_id)
    .bind(&name)
    .bind(&description)
    .bind(config_value)
    .bind(archived)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    if moves && let Some(position) = change.position {
        move_to(connection, project_id, kind, field_id, position).await?;
    } else if restoring {
        // A restored field comes back at the end of the active ones.
        move_to(connection, project_id, kind, field_id, i32::MAX).await?;
    }
    let field = definition(connection, project_id, kind, field_id)
        .await?
        .ok_or(AppError::Internal("changed field is missing"))?;
    Ok(ChangeFieldOutcome::Changed {
        field,
        previous: current,
        changed,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn field(field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: "Risk".to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
        }
    }

    fn choice(field_type: FieldType) -> FieldDefinition {
        let mut field = field(field_type);
        field.config = normalize_config(
            field_type,
            None,
            Some(&[
                OptionInput {
                    id: None,
                    name: "Low".to_owned(),
                    color: None,
                },
                OptionInput {
                    id: None,
                    name: " High ".to_owned(),
                    color: Some("red".to_owned()),
                },
            ]),
            None,
        )
        .expect("test value");
        field
    }

    /// "is" compares what was written; "0012" also matches 12 only where a
    /// number is stored, so a text field's "0012" doesn't match its "12".
    #[test]
    fn numbers_match_as_numbers_only_where_numbers_are_stored() {
        let clause = FieldClause {
            field: Uuid::now_v7(),
            op: FieldOp::Is,
            values: vec![" 0012 ".to_owned()],
        };
        let mut builder = QueryBuilder::<Postgres>::new("SELECT 1 FROM work_items item WHERE true");
        push_clause(&mut builder, "item.field_values", &clause);
        let sql = builder.sql();
        let stored = format!("(item.field_values -> '{}')", clause.field);
        assert!(
            sql.contains(&format!(
                "element.value = ANY($1) OR (jsonb_typeof({stored}) = 'number' AND element.value = ANY($2)))"
            )),
            "{sql}"
        );
        assert_eq!(match_values(&clause.values), vec!["0012", "12"]);
    }

    /// "me" is the reader only in a person field; a text or URL field's
    /// filter keeps the word.
    #[test]
    fn me_is_the_reader_only_in_person_fields() {
        let reader = Uuid::now_v7();
        let person = field(FieldType::Person);
        let text = field(FieldType::Text);
        let url = field(FieldType::Url);
        let clause = |field: &FieldDefinition, op| FieldClause {
            field: field.id,
            op,
            values: vec!["me".to_owned()],
        };
        let mut clauses = vec![
            clause(&person, FieldOp::Is),
            clause(&text, FieldOp::Is),
            clause(&text, FieldOp::Contains),
            clause(&url, FieldOp::Contains),
        ];
        resolve_me(&mut clauses, &[person, text, url], reader);
        assert_eq!(clauses[0].values, vec![reader.to_string()]);
        for clause in &clauses[1..] {
            assert_eq!(clause.values, vec!["me".to_owned()]);
        }
    }

    /// The modules contract (AWD-75) names field types as task fields
    /// store them; every stored type must be one it reads.
    #[test]
    fn every_field_type_is_a_contract_field_type() {
        for field_type in FieldType::ALL {
            let read = serde_json::from_value::<crate::modules::contract::FieldType>(json!(
                field_type.as_str()
            ));
            assert!(
                read.is_ok(),
                "{} isn't a contract field type",
                field_type.as_str()
            );
        }
    }

    #[test]
    fn types_read_the_way_people_write_them() {
        assert_eq!(
            FieldType::parse("Multi select"),
            Some(FieldType::MultipleChoice)
        );
        assert_eq!(
            FieldType::parse("single_choice"),
            Some(FieldType::SingleChoice)
        );
        assert_eq!(FieldType::parse("boolean"), Some(FieldType::Checkbox));
        assert_eq!(FieldType::parse("relation"), Some(FieldType::Link));
        assert_eq!(FieldType::parse("formula"), None);
        for field_type in FieldType::ALL {
            assert_eq!(FieldType::parse(field_type.as_str()), Some(field_type));
        }
    }

    #[test]
    fn options_keep_their_ids_and_archive_when_left_out() {
        let field = choice(FieldType::SingleChoice);
        let [low, high] = [&field.config.options[0], &field.config.options[1]];
        assert_eq!(high.name, "High");
        assert_eq!(high.color, "#e5484d");
        let renamed = normalize_config(
            FieldType::SingleChoice,
            Some(&field.config),
            Some(&[OptionInput {
                id: Some(high.id.to_string()),
                name: "Very high".to_owned(),
                color: None,
            }]),
            None,
        )
        .expect("test value");
        assert_eq!(renamed.options[0].id, high.id);
        assert_eq!(renamed.options[0].color, "#e5484d");
        assert_eq!(renamed.options[1].id, low.id);
        assert!(renamed.options[1].archived);
        let duplicate = normalize_config(
            FieldType::SingleChoice,
            None,
            Some(&[
                OptionInput {
                    id: None,
                    name: "a".to_owned(),
                    color: None,
                },
                OptionInput {
                    id: None,
                    name: "A".to_owned(),
                    color: None,
                },
            ]),
            None,
        );
        assert_eq!(duplicate.expect_err("test value").field, "options");
        let stranger = normalize_config(
            FieldType::SingleChoice,
            Some(&field.config),
            Some(&[OptionInput {
                id: Some(Uuid::now_v7().to_string()),
                name: "x".to_owned(),
                color: None,
            }]),
            None,
        );
        assert!(stranger.is_err());
    }

    #[test]
    fn links_need_a_target_that_never_changes() {
        assert!(normalize_config(FieldType::Link, None, None, None).is_err());
        let page = normalize_config(FieldType::Link, None, None, Some("page")).expect("test value");
        assert_eq!(page.target, Some(LinkTarget::Page));
        assert!(normalize_config(FieldType::Link, Some(&page), None, Some("task")).is_err());
        assert!(normalize_config(FieldType::Text, None, None, Some("task")).is_err());
        assert!(normalize_config(FieldType::Text, None, Some(&[]), None).is_err());
    }

    #[test]
    fn values_are_checked_against_their_type() {
        let number = field(FieldType::Number);
        assert_eq!(
            normalize_value(&number, &json!("3")),
            Ok(Normalized::Value(json!(3)))
        );
        assert_eq!(
            normalize_value(&number, &json!(2.5)),
            Ok(Normalized::Value(json!(2.5)))
        );
        assert_eq!(normalize_value(&number, &json!("")), Ok(Normalized::Clear));
        assert!(normalize_value(&number, &json!("three")).is_err());
        assert!(normalize_value(&number, &json!(1e16)).is_err());

        let date = field(FieldType::Date);
        assert_eq!(
            normalize_value(&date, &json!("2026-11-04")),
            Ok(Normalized::Value(json!("2026-11-04")))
        );
        assert!(normalize_value(&date, &json!("04/11/2026")).is_err());

        let checkbox = field(FieldType::Checkbox);
        assert_eq!(
            normalize_value(&checkbox, &json!(true)),
            Ok(Normalized::Value(json!(true)))
        );
        assert_eq!(
            normalize_value(&checkbox, &json!("no")),
            Ok(Normalized::Clear)
        );
        assert!(normalize_value(&checkbox, &json!("maybe")).is_err());

        let url = field(FieldType::Url);
        assert_eq!(
            normalize_value(&url, &json!("example.com/a")),
            Ok(Normalized::Value(json!("https://example.com/a")))
        );
        assert!(normalize_value(&url, &json!("javascript:alert(1)")).is_err());
        assert!(normalize_value(&url, &json!("ftp://example.com")).is_err());

        let text = field(FieldType::Text);
        assert_eq!(
            normalize_value(&text, &json!("  hi\nthere ")),
            Ok(Normalized::Value(json!("hi\nthere")))
        );
        assert!(normalize_value(&text, &json!(3)).is_err());
        assert!(normalize_value(&text, &json!("bell\u{7}")).is_err());

        let person = field(FieldType::Person);
        assert_eq!(
            normalize_value(&person, &json!("@ana")),
            Ok(Normalized::Person("@ana".to_owned()))
        );
        let link = field(FieldType::Link);
        assert_eq!(
            normalize_value(&link, &json!("AWD-12")),
            Ok(Normalized::Link("AWD-12".to_owned()))
        );
    }

    #[test]
    fn choices_take_names_or_ids_and_store_ids_in_option_order() {
        let single = choice(FieldType::SingleChoice);
        let high = single.config.options[1].id.to_string();
        assert_eq!(
            normalize_value(&single, &json!("high")),
            Ok(Normalized::Value(json!(high)))
        );
        assert_eq!(
            normalize_value(&single, &json!(high.clone())),
            Ok(Normalized::Value(json!(high)))
        );
        let refused = normalize_value(&single, &json!("Medium")).expect_err("test value");
        assert!(refused.contains("Low, High"), "{refused}");

        let multiple = choice(FieldType::MultipleChoice);
        let ids = multiple
            .config
            .options
            .iter()
            .map(|option| option.id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            normalize_value(&multiple, &json!(["High", "low", "High"])),
            Ok(Normalized::Value(json!([ids[0], ids[1]])))
        );
        assert_eq!(
            normalize_value(&multiple, &json!([])),
            Ok(Normalized::Clear)
        );
        assert_eq!(
            normalize_value(&multiple, &json!("High, low")),
            Ok(Normalized::Value(json!([ids[0], ids[1]])))
        );
        assert_eq!(
            display_value(&multiple, &json!([ids[1]]), &|_| None),
            json!(["High"])
        );
    }

    #[test]
    fn filters_parse_and_check_against_the_field() {
        let single = choice(FieldType::SingleChoice);
        let high = single.config.options[1].id;
        let param = format!(
            r#"[{{"field":"{}","op":"is","values":["{high}"]}}]"#,
            single.id
        );
        let clauses = parse_clauses(&param).expect("test value");
        assert_eq!(clauses[0].op, FieldOp::Is);
        assert!(check_clause(&single, &clauses[0]).is_ok());
        assert_eq!(parse_clauses(&clauses_param(&clauses)), Some(clauses));
        assert!(parse_clauses("not json").is_none());
        assert!(
            parse_clauses(&format!(
                r#"[{{"field":"{}","op":"empty","values":["x"]}}]"#,
                single.id
            ))
            .is_none()
        );
        assert!(
            parse_clauses(&format!(
                r#"[{{"field":"{}","op":"gt","values":["1","2"]}}]"#,
                single.id
            ))
            .is_none()
        );

        let written =
            clause_from_written(&single, FieldOp::Is, &["high".to_owned()]).expect("test value");
        assert_eq!(written.values, vec![high.to_string()]);
        assert!(clause_from_written(&single, FieldOp::Contains, &["h".to_owned()]).is_err());

        let checkbox = field(FieldType::Checkbox);
        let unchecked =
            clause_from_written(&checkbox, FieldOp::Is, &["false".to_owned()]).expect("test value");
        assert_eq!(unchecked.op, FieldOp::Empty);
        assert!(unchecked.values.is_empty());

        let number = field(FieldType::Number);
        assert!(clause_from_written(&number, FieldOp::Gt, &["1,000".to_owned()]).is_ok());
        assert!(clause_from_written(&number, FieldOp::Gt, &["lots".to_owned()]).is_err());
    }

    #[test]
    fn numbers_match_however_json_writes_them() {
        assert_eq!(match_values(&["3.0".to_owned()]), vec!["3.0", "3"]);
        assert_eq!(match_values(&["3".to_owned()]), vec!["3"]);
    }

    #[test]
    fn sql_never_holds_what_callers_wrote() {
        let mut single = choice(FieldType::SingleChoice);
        single.config.options[0].name = "'; DROP TABLE x; --".to_owned();
        for key in sort_keys(&single, "item.field_values", "item.project_id") {
            assert!(!key.expr.contains("DROP"));
        }
        let group = group_sql(
            single.id,
            FieldType::SingleChoice,
            "item.field_values",
            "item.project_id",
        )
        .expect("test value");
        assert!(!group.join.contains("DROP"));
        assert!(group_sql(single.id, FieldType::Text, "c", "p").is_none());
    }

    #[test]
    fn names_are_one_line_and_never_an_id() {
        assert_eq!(
            normalize_name("  Story   points ").as_deref(),
            Some("Story points")
        );
        assert!(normalize_name("").is_none());
        assert!(normalize_name(&"x".repeat(61)).is_none());
        assert!(normalize_name(&Uuid::now_v7().to_string()).is_none());
        assert!(normalize_description("two\nlines").is_none());
    }
}
