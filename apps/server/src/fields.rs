//! Custom fields (AWD-89): the field types every kind of record shares.
//!
//! A field belongs to one project and one kind of record (`RecordKind`):
//! tasks, or one collection's records (AWD-90), and modules' records later.
//! This module knows nothing about tasks or collections. It owns:
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
//! (`collection_records.field_values`, tasks included). A field with no value has no key.
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
pub const MAX_LONG_TEXT_CHARACTERS: usize = 20_000;
pub const MAX_EMAIL_CHARACTERS: usize = 320;
pub const MAX_PHONE_CHARACTERS: usize = 32;
/// Sorts on text compare this many characters of it, lowercased: plenty to
/// order by, and few enough that a list's cursor, which carries the last
/// row's sort values, stays small. Values that share them fall through to
/// the list's next key.
pub const SORT_PREFIX_CHARACTERS: usize = 100;
/// The largest number a number field holds, either side of zero.
pub const MAX_NUMBER: f64 = 1e15;
/// Field filters one list request can carry.
pub const MAX_CLAUSES: usize = 8;
/// Values one field filter can name.
pub const MAX_CLAUSE_VALUES: usize = 20;
const MAX_CLAUSE_VALUE_CHARACTERS: usize = 200;

// ---------------------------------------------------------------------------
// Types

/// A field's type: the one list every record kind, collection and module
/// manifest names types from (T1.2; `modules::contract::FieldType` is this
/// type). [`FieldType::ALL`] are the types a field can be made with; the
/// rest are named by module manifests only, until the field layer stores
/// them (`FieldType::is_stored`). System types (`FieldType::is_system`)
/// store nothing: they read the record's own columns.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    /// Paragraphs of text.
    LongText,
    Number,
    Date,
    SingleChoice,
    MultipleChoice,
    /// One of its options, each of a status category ("started",
    /// "completed"...), which lists filter, sort and group by.
    Status,
    Person,
    /// Several people. Declared by manifests only.
    People,
    /// The project's labels. Declared by manifests only.
    Labels,
    Url,
    /// An email address, kept lowercase.
    Email,
    /// A phone number as written, with its digits compared.
    Phone,
    Checkbox,
    // A link to a task or a page in the same project (`FieldConfig::target`).
    Link,
    /// When the record was made (system).
    CreatedTime,
    /// When the record last changed (system).
    EditedTime,
    /// Who made the record (system).
    CreatedBy,
    /// Who last changed the record (system).
    EditedBy,
    /// The record's number (system).
    AutoId,
}

impl FieldType {
    pub const ALL: [Self; 18] = [
        Self::Text,
        Self::LongText,
        Self::Number,
        Self::Date,
        Self::SingleChoice,
        Self::MultipleChoice,
        Self::Status,
        Self::Person,
        Self::Url,
        Self::Email,
        Self::Phone,
        Self::Checkbox,
        Self::Link,
        Self::CreatedTime,
        Self::EditedTime,
        Self::CreatedBy,
        Self::EditedBy,
        Self::AutoId,
    ];

    /// Whether fields of this type can be made.
    pub fn is_stored(self) -> bool {
        Self::ALL.contains(&self)
    }

    /// Whether it reads the record's own columns instead of a stored value:
    /// nobody writes it.
    pub fn is_system(self) -> bool {
        matches!(
            self,
            Self::CreatedTime | Self::EditedTime | Self::CreatedBy | Self::EditedBy | Self::AutoId
        )
    }

    /// Where a system type's value is read: the record's column (aliased
    /// `record`).
    pub fn system_accessor(self) -> Option<Accessor> {
        let (expr, sql_type) = match self {
            Self::CreatedTime => ("record.created_at", "timestamptz"),
            Self::EditedTime => ("record.updated_at", "timestamptz"),
            Self::CreatedBy => ("record.created_by_actor_id", "uuid"),
            Self::EditedBy => ("record.updated_by_actor_id", "uuid"),
            Self::AutoId => ("record.number", "bigint"),
            _ => return None,
        };
        Some(Accessor::Column {
            expr: expr.to_owned(),
            sql_type,
            category: None,
        })
    }

    /// The type it filters, sorts and groups like: a system type's value's
    /// own (a time is a date, a maker a person, an ID a number).
    pub fn behaves_as(self) -> Self {
        match self {
            Self::CreatedTime | Self::EditedTime => Self::Date,
            Self::CreatedBy | Self::EditedBy => Self::Person,
            Self::AutoId => Self::Number,
            other => other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::LongText => "long_text",
            Self::Status => "status",
            Self::People => "people",
            Self::Labels => "labels",
            Self::Number => "number",
            Self::Date => "date",
            Self::SingleChoice => "single_choice",
            Self::MultipleChoice => "multiple_choice",
            Self::Person => "person",
            Self::Url => "url",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Checkbox => "checkbox",
            Self::Link => "link",
            Self::CreatedTime => "created_time",
            Self::EditedTime => "edited_time",
            Self::CreatedBy => "created_by",
            Self::EditedBy => "edited_by",
            Self::AutoId => "auto_id",
        }
    }

    /// A type as stored, or as people and agents tend to write it ("select",
    /// "multi select", "user", "boolean", "relation", "paragraph").
    pub fn parse(value: &str) -> Option<Self> {
        let squashed = value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        Some(match squashed.as_str() {
            "text" | "string" => Self::Text,
            "longtext" | "paragraph" | "paragraphs" | "textarea" | "richtext" | "notes" => {
                Self::LongText
            }
            "number" | "numeric" => Self::Number,
            "date" => Self::Date,
            "singlechoice" | "choice" | "select" | "singleselect" => Self::SingleChoice,
            "multiplechoice" | "multichoice" | "multiselect" | "multipleselect" | "choices" => {
                Self::MultipleChoice
            }
            "status" | "state" => Self::Status,
            "person" | "user" | "people" | "member" => Self::Person,
            "url" | "website" => Self::Url,
            "email" | "emailaddress" | "mail" => Self::Email,
            "phone" | "phonenumber" | "telephone" | "tel" | "mobile" => Self::Phone,
            "checkbox" | "boolean" | "bool" => Self::Checkbox,
            "link" | "relation" | "reference" => Self::Link,
            "createdtime" | "createdat" | "created" | "createdon" => Self::CreatedTime,
            "editedtime" | "lasteditedtime" | "updatedat" | "updated" | "edited" | "lastedited"
            | "modified" => Self::EditedTime,
            "createdby" | "creator" | "maker" => Self::CreatedBy,
            "editedby" | "lasteditedby" | "updatedby" | "modifiedby" => Self::EditedBy,
            "autoid" | "autonumber" | "uniqueid" => Self::AutoId,
            _ => return None,
        })
    }

    /// Whether a task field can be of this type: tasks have their own
    /// status and record columns, so not those.
    pub fn suits_tasks(self) -> bool {
        self.is_stored() && !self.is_system() && self != Self::Status
    }

    pub fn has_options(self) -> bool {
        matches!(
            self,
            Self::SingleChoice | Self::MultipleChoice | Self::Status
        )
    }

    /// Whether a list can be grouped by a field of this type, one group per
    /// record (`FieldDefinition::can_group` adds choices and dates).
    pub fn can_group(self) -> bool {
        matches!(
            self.behaves_as(),
            Self::SingleChoice | Self::Status | Self::Person | Self::Checkbox
        )
    }

    /// The filter operations that make sense for it.
    pub fn ops(self) -> &'static [FieldOp] {
        use FieldOp::*;
        match self.behaves_as() {
            Self::Text | Self::Url | Self::Email | Self::Phone => {
                &[Contains, Is, IsNot, Empty, NotEmpty]
            }
            Self::LongText => &[Contains, Empty, NotEmpty],
            Self::Number => &[Is, IsNot, Gt, Gte, Lt, Lte, Empty, NotEmpty],
            Self::Date => &[Is, IsNot, Gt, Gte, Lt, Lte, Within, Empty, NotEmpty],
            Self::Status => &[Is, IsNot, CategoryIs, CategoryIsNot, Empty, NotEmpty],
            Self::SingleChoice
            | Self::MultipleChoice
            | Self::Labels
            | Self::Person
            | Self::People
            | Self::Link => &[Is, IsNot, Empty, NotEmpty],
            Self::Checkbox => &[Is, Empty, NotEmpty],
            Self::CreatedTime
            | Self::EditedTime
            | Self::CreatedBy
            | Self::EditedBy
            | Self::AutoId => &[Empty, NotEmpty],
        }
    }
}

/// How a list grouped by a date puts records together: by day, by week
/// (from Monday) or by month.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DateBucket {
    Day,
    Week,
    #[default]
    Month,
}

impl DateBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

/// The kind of record a field belongs to: a project's tasks, or the
/// records of one of its collections (AWD-90, `crate::collections`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Task,
    /// The records of the collection with this ID.
    Collection(Uuid),
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Collection(_) => "record",
        }
    }

    /// The collection the fields belong to; None for tasks.
    pub fn collection_id(self) -> Option<Uuid> {
        match self {
            Self::Task => None,
            Self::Collection(id) => Some(id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkTarget {
    Task,
    Page,
    /// A record in one collection of the project (AWD-110), named by the
    /// field's `collection`.
    Record,
}

impl LinkTarget {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "task" | "tasks" => Some(Self::Task),
            "page" | "pages" => Some(Self::Page),
            "record" | "records" => Some(Self::Record),
            _ => None,
        }
    }

    /// The target as `normalize_config` takes it: "task", "page", or
    /// "record:<collection ID>".
    pub fn written(self, collection: Option<Uuid>) -> String {
        match (self, collection) {
            (Self::Task, _) => "task".to_owned(),
            (Self::Page, _) => "page".to_owned(),
            (Self::Record, Some(collection)) => format!("record:{collection}"),
            (Self::Record, None) => "record".to_owned(),
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
    /// Status options: what kind of status it is ("started", "completed"...),
    /// which `categoryIs` filters by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
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
    /// Link fields to records: the collection whose records they name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<Uuid>,
}

/// Where a field's value comes from in SQL (T1.5), so stored values, a
/// record's own columns and a task's columns filter, sort and group through
/// one code path (`push_condition`, `sort_keys`, `group_sql`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Accessor {
    /// A stored value: `<values column> -> '<field id>'`.
    #[default]
    Stored,
    /// A column or SQL expression of the record (aliased `record`) or its
    /// task (aliased `core`); NULL is no value. `sql_type` is its type, so
    /// filters compare in it and its indexes apply; `category`, for status
    /// fields, the SQL for the status's category.
    Column {
        expr: String,
        sql_type: &'static str,
        category: Option<String>,
    },
    /// A set of values: a query giving each as text (one column), such as
    /// a task's labels.
    Elements { sql: String },
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
    /// Where its value comes from: `Accessor::Stored` for every field in
    /// `field_definitions`; built-in fields read columns.
    pub accessor: Accessor,
}

/// What kind of status a status is, in workflow order.
pub const STATUS_CATEGORIES: [&str; 7] = [
    "backlog",
    "unstarted",
    "started",
    "review",
    "verification",
    "completed",
    "canceled",
];

impl FieldDefinition {
    /// The filter operations the field takes: its type's, and
    /// `categoryIs`/`categoryIsNot` wherever a status's category is known.
    pub fn ops(&self) -> Vec<FieldOp> {
        let mut ops = self.field_type.ops().to_vec();
        if matches!(
            &self.accessor,
            Accessor::Column {
                category: Some(_),
                ..
            }
        ) && !ops.contains(&FieldOp::CategoryIs)
        {
            ops.extend([FieldOp::CategoryIs, FieldOp::CategoryIsNot]);
        }
        ops
    }

    /// Whether a list can be grouped by the field: its type can; it holds
    /// several choices (a record in each of its groups) or a date (by
    /// `DateBucket`); or it's a parent field its source groups (a task's).
    pub fn can_group(&self) -> bool {
        self.field_type.can_group()
            || matches!(
                self.field_type.behaves_as(),
                FieldType::MultipleChoice | FieldType::Date
            )
            || (self.accessor != Accessor::Stored && self.field_type == FieldType::Link)
    }

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

    /// An option someone may choose: an active one, or an archived one the
    /// record already holds (`kept`, its current value for this field), so
    /// editing the rest of a selection doesn't drop it. Newly choosing an
    /// archived option stays refused.
    fn choosable_option(&self, reference: &str, kept: Option<&Value>) -> Option<&ChoiceOption> {
        if let Some(option) = self.active_option(reference) {
            return Some(option);
        }
        let reference = reference.trim().to_ascii_lowercase();
        let by_name = collapse(&reference).to_lowercase();
        let held = |id: &str| match kept {
            Some(Value::String(kept)) => kept == id,
            Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(id)),
            _ => false,
        };
        self.config.options.iter().find(|option| {
            let id = option.id.to_string();
            option.archived
                && held(&id)
                && (id == reference || option.name.to_lowercase() == by_name)
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
        let field_type =
            FieldType::parse(&row.field_type).ok_or(AppError::Internal("stored field type"))?;
        Ok(Self {
            id: row.id,
            name: row.name,
            description: row.description,
            config: serde_json::from_value(row.config)
                .map_err(|_| AppError::Internal("stored field config"))?,
            position: row.position,
            archived: row.archived,
            version: row.version,
            accessor: field_type.system_accessor().unwrap_or_default(),
            field_type,
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
    /// Link fields: "task", "page" or "record".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LinkTarget>,
    /// Record links: the collection whose records it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
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
    /// Status options: the option's category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
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
                    category: option.category.clone(),
                })
                .collect(),
            target: field.config.target,
            collection: field.config.collection.map(|id| id.to_string()),
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
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
    /// Status fields: the option's category, one of backlog, unstarted,
    /// started, review, verification, completed or canceled. Left out, an
    /// existing option keeps its own and a new one is unstarted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// A status option's category as written, if it's one.
pub fn status_category(value: &str) -> Option<&'static str> {
    let value = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    STATUS_CATEGORIES
        .iter()
        .copied()
        .find(|category| *category == value)
}

/// The options a new status field starts with.
fn default_status_options() -> Vec<ChoiceOption> {
    [
        ("Not started", "unstarted", "gray"),
        ("In progress", "started", "blue"),
        ("Done", "completed", "green"),
    ]
    .into_iter()
    .map(|(name, category, color)| ChoiceOption {
        id: Uuid::now_v7(),
        name: name.to_owned(),
        color: work_item_labels::normalize_color(color)
            .unwrap_or_else(|| work_item_labels::default_color(name).to_owned()),
        archived: false,
        category: Some(category.to_owned()),
    })
    .collect()
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
        // A record link names its collection: "record:<collection ID>".
        let (kind, collection) = match target.split_once(':') {
            Some((kind, collection)) => (kind, Some(collection.trim())),
            None => (target, None),
        };
        let target = LinkTarget::parse(kind).ok_or_else(|| {
            problem(
                "target",
                "A link field links to \"task\", \"page\" or a collection's records.",
            )
        })?;
        let collection = match (target, collection) {
            (LinkTarget::Record, Some(collection)) => {
                Some(collection.parse::<Uuid>().map_err(|_| {
                    problem("target", "Name the collection whose records it links to.")
                })?)
            }
            (LinkTarget::Record, None) => {
                return Err(problem(
                    "target",
                    "Name the collection whose records it links to.",
                ));
            }
            (_, Some(_)) => {
                return Err(problem(
                    "target",
                    "Only a link to records names a collection.",
                ));
            }
            (_, None) => None,
        };
        if current.is_some_and(|current| {
            current.target != Some(target) || current.collection != collection
        }) {
            return Err(problem(
                "target",
                "A link field's target can't change. Add a new field instead.",
            ));
        }
        config.target = Some(target);
        config.collection = collection;
    }
    if field_type == FieldType::Link && config.target.is_none() {
        return Err(problem(
            "target",
            "Say what a link field links to: \"task\", \"page\" or a collection's records.",
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
            let category = match (field_type, input.category.as_deref()) {
                (FieldType::Status, Some(category)) => {
                    Some(status_category(category).ok_or_else(|| {
                        problem(
                            "options",
                            format!(
                                "\"{name}\" needs a category: {}.",
                                STATUS_CATEGORIES.join(", ")
                            ),
                        )
                    })?)
                }
                (FieldType::Status, None) => Some(
                    config
                        .options
                        .iter()
                        .find(|option| option.id == id)
                        .and_then(|option| option.category.as_deref())
                        .and_then(status_category)
                        .unwrap_or("unstarted"),
                ),
                (_, Some(_)) => {
                    return Err(problem("options", "Only status options have a category."));
                }
                (_, None) => None,
            };
            next.push(ChoiceOption {
                id,
                name,
                color,
                archived: false,
                category: category.map(str::to_owned),
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
    } else if field_type == FieldType::Status && current.is_none() {
        config.options = default_status_options();
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

/// A lowercased email address of a plain shape (RFC 5321's limits: a
/// local part of at most 64 characters and a domain with a dot), or None.
pub fn normalize_email(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches("mailto:").to_lowercase();
    if value.chars().count() > MAX_EMAIL_CHARACTERS {
        return None;
    }
    let (local, domain) = value.rsplit_once('@')?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.chars().all(|character| {
            character.is_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(character)
        });
    let labels = domain.split('.').collect::<Vec<_>>();
    let domain_ok = labels.len() >= 2
        && domain.len() <= 253
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '-')
        });
    (local_ok && domain_ok).then_some(value)
}

/// A phone number as written, spaces and all, if it has a plausible number
/// of digits and nothing but digits, one leading +, spaces, dots, dashes
/// and brackets. The error says what's wrong with it.
pub fn normalize_phone(value: &str) -> Result<String, &'static str> {
    let value = collapse(value.trim());
    if value.chars().count() > MAX_PHONE_CHARACTERS {
        return Err("at most 32 characters.");
    }
    if let Some(bad) = value.chars().enumerate().find(|(index, character)| {
        !(character.is_ascii_digit()
            || matches!(character, ' ' | '.' | '-' | '(' | ')')
            || (*character == '+' && *index == 0))
    }) {
        return Err(if bad.1 == '+' {
            "a + only at the start."
        } else {
            "only digits, a leading +, spaces, dots, dashes and brackets."
        });
    }
    let digits = phone_digits(&value);
    if !(4..=15).contains(&digits.trim_start_matches('+').len()) {
        return Err("4 to 15 digits.");
    }
    Ok(value)
}

/// A phone number's digits (and its leading +), as `is` compares them.
pub fn phone_digits(value: &str) -> String {
    value
        .trim()
        .chars()
        .enumerate()
        .filter(|(index, character)| {
            character.is_ascii_digit() || (*character == '+' && *index == 0)
        })
        .map(|(_, character)| character)
        .collect()
}

/// Checks a value written for `field`. The message says what would work.
pub fn normalize_value(field: &FieldDefinition, input: &Value) -> Result<Normalized, String> {
    normalize_value_keeping(field, input, None)
}

/// `normalize_value` for a record whose current value for `field` is
/// `kept`: archived choice options it already holds may stay chosen.
pub fn normalize_value_keeping(
    field: &FieldDefinition,
    input: &Value,
    kept: Option<&Value>,
) -> Result<Normalized, String> {
    if input.is_null() {
        return Ok(Normalized::Clear);
    }
    let name = &field.name;
    let blank = input.as_str().is_some_and(|text| text.trim().is_empty());
    match field.field_type {
        FieldType::People | FieldType::Labels => Err(format!(
            "\"{name}\" is a {} field, which records don't store.",
            field.field_type.as_str()
        )),
        FieldType::CreatedTime
        | FieldType::EditedTime
        | FieldType::CreatedBy
        | FieldType::EditedBy
        | FieldType::AutoId => Err(format!(
            "\"{name}\" is set by the app from the record itself, so it can't be written."
        )),
        FieldType::LongText => {
            let Value::String(text) = input else {
                return Err(format!("\"{name}\" takes text."));
            };
            let text = text.trim();
            if text.is_empty() {
                return Ok(Normalized::Clear);
            }
            if text.chars().count() > MAX_LONG_TEXT_CHARACTERS || !text_is_clean(text) {
                return Err(format!(
                    "\"{name}\" takes up to {MAX_LONG_TEXT_CHARACTERS} characters, with no control characters except line breaks and tabs."
                ));
            }
            Ok(Normalized::Value(Value::String(text.to_owned())))
        }
        FieldType::Email => match input {
            Value::String(text) if text.trim().is_empty() => Ok(Normalized::Clear),
            Value::String(text) => normalize_email(text)
                .map(|email| Normalized::Value(Value::String(email)))
                .ok_or_else(|| {
                    format!("\"{name}\" takes an email address, such as ada@example.com.")
                }),
            _ => Err(format!("\"{name}\" takes an email address.")),
        },
        FieldType::Phone => match input {
            Value::String(text) if text.trim().is_empty() => Ok(Normalized::Clear),
            Value::String(text) => normalize_phone(text)
                .map(|phone| Normalized::Value(Value::String(phone)))
                .map_err(|reason| format!("\"{name}\" takes a phone number: {reason}")),
            Value::Number(number) => normalize_phone(&number.to_string())
                .map(|phone| Normalized::Value(Value::String(phone)))
                .map_err(|reason| format!("\"{name}\" takes a phone number: {reason}")),
            _ => Err(format!(
                "\"{name}\" takes a phone number, such as +44 20 7946 0958."
            )),
        },
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
        FieldType::SingleChoice | FieldType::Status => {
            if blank {
                return Ok(Normalized::Clear);
            }
            let reference = input.as_str().unwrap_or_default();
            field
                .choosable_option(reference, kept)
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
                // One option, or several written "A, B" (rules, and plain
                // spreadsheet cells): a best effort, since an option's own
                // name may hold a comma. Exports write a JSON list instead.
                Value::String(text) if field.choosable_option(text, kept).is_none() => text
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
                    .and_then(|text| field.choosable_option(text, kept))
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
                Some(LinkTarget::Record) => {
                    format!("\"{name}\" takes a record: its title, #number, ID or link.")
                }
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
        FieldType::SingleChoice | FieldType::Status => option_name(value),
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
        FieldType::Person | FieldType::Link | FieldType::CreatedBy | FieldType::EditedBy => {
            match value.as_str() {
                Some(id) => Value::String(name_of(id).unwrap_or_else(|| id.to_owned())),
                None => Value::Null,
            }
        }
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
    matches!(
        field.field_type.behaves_as(),
        FieldType::Person | FieldType::Link
    )
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
    // A date in a range relative to today, such as "this_week" or
    // "overdue" (`WITHIN_VALUES`), read when the list is.
    Within,
    // A status whose category is any of the values (such as "started").
    CategoryIs,
    // A status whose category is none of the values.
    CategoryIsNot,
}

impl FieldOp {
    fn takes_values(self) -> bool {
        !matches!(self, Self::Empty | Self::NotEmpty)
    }

    fn takes_one_value(self) -> bool {
        matches!(
            self,
            Self::Contains | Self::Gt | Self::Gte | Self::Lt | Self::Lte | Self::Within
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
            Self::Within => "within",
            Self::CategoryIs => "categoryIs",
            Self::CategoryIsNot => "categoryIsNot",
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
    /// Whether the clause has the values its operation takes: none for
    /// empty and notEmpty, one for contains and the comparisons, and 1 to
    /// `MAX_CLAUSE_VALUES` short ones for is and isNot.
    pub fn shape_is_valid(&self) -> bool {
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
    resolve_me_in(clauses.iter_mut().collect(), definitions, reader);
}

/// `resolve_me` for clauses wherever they are, such as in a filter's groups.
pub fn resolve_me_in(
    clauses: Vec<&mut FieldClause>,
    definitions: &[FieldDefinition],
    reader: Uuid,
) {
    for clause in clauses {
        let person = definitions.iter().any(|field| {
            field.id == clause.field && field.field_type.behaves_as() == FieldType::Person
        });
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
    let ops = field.ops();
    if !ops.contains(&clause.op) {
        let ops = ops
            .iter()
            .map(|op| op.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("\"{name}\" filters with {ops}."));
    }
    match clause.op {
        FieldOp::Within => {
            return if clause
                .values
                .iter()
                .all(|value| within_range(value, today()).is_some())
            {
                Ok(())
            } else {
                Err(format!("\"{name}\" within takes one of {WITHIN_VALUES}."))
            };
        }
        FieldOp::CategoryIs | FieldOp::CategoryIsNot => {
            return if clause
                .values
                .iter()
                .all(|value| STATUS_CATEGORIES.contains(&value.trim()))
            {
                Ok(())
            } else {
                Err(format!(
                    "\"{name}\" filters by category with {}.",
                    STATUS_CATEGORIES.join(", ")
                ))
            };
        }
        _ => {}
    }
    for value in &clause.values {
        let ok = match field.field_type.behaves_as() {
            // A record's number is whole.
            FieldType::Number if field.field_type == FieldType::AutoId => {
                value.trim().parse::<i64>().is_ok()
            }
            FieldType::Number => value.trim().parse::<f64>().is_ok_and(f64::is_finite),
            FieldType::Date => NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").is_ok(),
            FieldType::SingleChoice | FieldType::MultipleChoice | FieldType::Status => {
                field.option(value).is_some()
            }
            FieldType::Person | FieldType::Link => value.parse::<Uuid>().is_ok(),
            FieldType::Checkbox => value == "true",
            FieldType::Text
            | FieldType::LongText
            | FieldType::Url
            | FieldType::Email
            | FieldType::Phone => true,
            FieldType::People
            | FieldType::Labels
            | FieldType::CreatedTime
            | FieldType::EditedTime
            | FieldType::CreatedBy
            | FieldType::EditedBy
            | FieldType::AutoId => false,
        };
        if !ok {
            return Err(match field.field_type.behaves_as() {
                FieldType::Number => format!("\"{name}\" filters by numbers."),
                FieldType::Date => format!("\"{name}\" filters by dates as YYYY-MM-DD."),
                FieldType::SingleChoice | FieldType::MultipleChoice | FieldType::Status => format!(
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
        if matches!(
            op,
            FieldOp::Within | FieldOp::CategoryIs | FieldOp::CategoryIsNot
        ) {
            // A range's or a category's name, as written.
            stored.push(value.trim().to_ascii_lowercase().replace([' ', '-'], "_"));
            continue;
        }
        match field.field_type.behaves_as() {
            FieldType::SingleChoice | FieldType::MultipleChoice | FieldType::Status => {
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
            FieldType::Email if op != FieldOp::Contains => {
                stored.push(value.trim().to_lowercase());
            }
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
    builder.push(" AND ");
    push_stored_condition(builder, column, clause, today());
}

/// Today's date, as `within` reads it when the caller doesn't say.
pub fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

/// Writes one clause as a condition (no leading AND) over where `field`'s
/// value comes from (`FieldDefinition::accessor`): its key in the JSONB
/// `column` of stored values, a column, or a set of elements. `within`
/// reads dates relative to `today`; `categoryIs` reads a status's
/// category. `field` None reads the stored value.
pub fn push_condition(
    builder: &mut QueryBuilder<'_, Postgres>,
    column: &str,
    field: Option<&FieldDefinition>,
    clause: &FieldClause,
    today: NaiveDate,
) {
    let Some(field) = field else {
        push_stored_condition(builder, column, clause, today);
        return;
    };
    match &field.accessor {
        Accessor::Stored => {
            if matches!(clause.op, FieldOp::CategoryIs | FieldOp::CategoryIsNot) {
                // A stored status holds an option ID; its options carry
                // their categories.
                let ids = field
                    .config
                    .options
                    .iter()
                    .filter(|option| {
                        option.category.as_deref().is_some_and(|category| {
                            clause.values.iter().any(|value| value == category)
                        })
                    })
                    .map(|option| option.id.to_string())
                    .collect::<Vec<_>>();
                let op = if clause.op == FieldOp::CategoryIs {
                    FieldOp::Is
                } else {
                    FieldOp::IsNot
                };
                if ids.is_empty() {
                    builder.push(if op == FieldOp::Is { "false" } else { "true" });
                    return;
                }
                let by_option = FieldClause {
                    field: clause.field,
                    op,
                    values: ids,
                };
                push_stored_condition(builder, column, &by_option, today);
            } else if clause.op == FieldOp::Is
                && clause.values.len() == 1
                && matches!(
                    field.field_type,
                    FieldType::SingleChoice | FieldType::Person | FieldType::MultipleChoice
                )
            {
                // Containment, so the values' GIN index can find it.
                let value = Value::String(clause.values[0].trim().to_owned());
                let held = if field.field_type == FieldType::MultipleChoice {
                    Value::Array(vec![value])
                } else {
                    value
                };
                let mut wanted = serde_json::Map::new();
                wanted.insert(clause.field.to_string(), held);
                builder.push(format!("{column} @> "));
                builder.push_bind(Value::Object(wanted));
            } else if field.field_type == FieldType::Phone
                && matches!(clause.op, FieldOp::Is | FieldOp::IsNot)
            {
                // Numbers match by their digits, however they're spaced.
                let digits = format!(
                    "regexp_replace({}, '[^0-9+]', '', 'g')",
                    text_at(column, clause.field)
                );
                builder.push(if clause.op == FieldOp::Is {
                    format!("{digits} = ANY(")
                } else {
                    format!("({digits} IS NULL OR NOT {digits} = ANY(")
                });
                builder.push_bind(
                    clause
                        .values
                        .iter()
                        .map(|value| phone_digits(value))
                        .collect::<Vec<_>>(),
                );
                builder.push(if clause.op == FieldOp::Is { ")" } else { "))" });
            } else {
                push_stored_condition(builder, column, clause, today);
            }
        }
        Accessor::Column {
            expr,
            sql_type,
            category,
        } if *sql_type == "timestamptz" && field.field_type.behaves_as() == FieldType::Date => {
            // A time filters by its day (UTC), as a date field does.
            let day = format!("(({expr}) AT TIME ZONE 'UTC')::date");
            push_column_condition(builder, &day, "date", category.as_deref(), clause, today);
        }
        Accessor::Column {
            expr,
            sql_type,
            category,
        } => push_column_condition(builder, expr, sql_type, category.as_deref(), clause, today),
        Accessor::Elements { sql } => push_elements_condition(builder, sql, clause),
    }
}

/// The values a clause compares with, trimmed.
fn written_values(clause: &FieldClause) -> Vec<String> {
    clause
        .values
        .iter()
        .map(|value| value.trim().to_owned())
        .collect()
}

fn like_pattern(clause: &FieldClause) -> String {
    let needle = clause.values.first().cloned().unwrap_or_default();
    let escaped = needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn comparison(op: FieldOp) -> &'static str {
    match op {
        FieldOp::Gt => ">",
        FieldOp::Gte => ">=",
        FieldOp::Lt => "<",
        _ => "<=",
    }
}

fn push_stored_condition(
    builder: &mut QueryBuilder<'_, Postgres>,
    column: &str,
    clause: &FieldClause,
    today: NaiveDate,
) {
    let field = clause.field;
    let value = value_at(column, field);
    let text = text_at(column, field);
    match clause.op {
        FieldOp::Empty => {
            builder.push(format!("{value} IS NULL"));
        }
        FieldOp::NotEmpty => {
            builder.push(format!("{value} IS NOT NULL"));
        }
        FieldOp::Is | FieldOp::IsNot => {
            builder.push(if clause.op == FieldOp::Is {
                "EXISTS (SELECT 1 FROM "
            } else {
                "NOT EXISTS (SELECT 1 FROM "
            });
            builder.push(format!(
                "{} AS element(value) WHERE element.value = ANY(",
                elements(column, field)
            ));
            builder.push_bind(written_values(clause));
            // A stored number (a number field) compares as a number, so
            // "3.0", "0012" and "1e-7" match 3, 12 and 0.0000001, however
            // PostgreSQL writes them; a text field's "0012" stays the text
            // it is. CASE keeps the cast away from anything but numbers.
            builder.push(format!(
                ") OR CASE WHEN jsonb_typeof({value}) = 'number' THEN element.value::numeric = ANY("
            ));
            builder.push_bind(filter_numbers(&clause.values));
            builder.push("::numeric[]) ELSE false END)");
        }
        FieldOp::Contains => {
            builder.push(format!(
                "(jsonb_typeof({value}) = 'string' AND {text} ILIKE "
            ));
            builder.push_bind(like_pattern(clause));
            builder.push(")");
        }
        FieldOp::Gt | FieldOp::Gte | FieldOp::Lt | FieldOp::Lte => {
            let operator = comparison(clause.op);
            let bound = clause
                .values
                .first()
                .map(|value| value.trim())
                .unwrap_or("");
            if let Ok(date) = NaiveDate::parse_from_str(bound, "%Y-%m-%d") {
                // ISO dates compare as text.
                builder.push(format!(
                    "(CASE WHEN jsonb_typeof({value}) = 'string' THEN {text} END) {operator} "
                ));
                builder.push_bind(date.to_string());
            } else {
                let number = bound
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite());
                builder.push(format!(
                    "(CASE WHEN jsonb_typeof({value}) = 'number' THEN ({text})::double precision END) {operator} "
                ));
                builder.push_bind(number);
            }
        }
        FieldOp::Within => {
            let date = format!("(CASE WHEN jsonb_typeof({value}) = 'string' THEN {text} END)");
            push_within(builder, &date, "text", clause, today);
        }
        // Only status fields have categories, and they're read through
        // their options (`push_condition`).
        FieldOp::CategoryIs | FieldOp::CategoryIsNot => {
            builder.push("false");
        }
    }
}

/// `date` (an expression of `sql_type`: a date, a time, or ISO text) is in
/// the range a `within` clause names, from `today`.
fn push_within(
    builder: &mut QueryBuilder<'_, Postgres>,
    date: &str,
    sql_type: &str,
    clause: &FieldClause,
    today: NaiveDate,
) {
    let Some((from, to)) = clause
        .values
        .first()
        .and_then(|value| within_range(value, today))
    else {
        builder.push("false");
        return;
    };
    // Times compare by their day.
    let date = match sql_type {
        "timestamptz" | "timestamp" => format!("(({date}) AT TIME ZONE 'UTC')::date"),
        _ => format!("({date})"),
    };
    let cast = if sql_type == "text" { "text" } else { "date" };
    builder.push(format!("({date} IS NOT NULL"));
    if let Some(from) = from {
        builder.push(format!(" AND {date} >= CAST("));
        builder.push_bind(from.to_string());
        builder.push(format!(" AS {cast})"));
    }
    if let Some(to) = to {
        builder.push(format!(" AND {date} < CAST("));
        builder.push_bind(to.to_string());
        builder.push(format!(" AS {cast})"));
    }
    builder.push(")");
}

fn push_column_condition(
    builder: &mut QueryBuilder<'_, Postgres>,
    expr: &str,
    sql_type: &str,
    category: Option<&str>,
    clause: &FieldClause,
    today: NaiveDate,
) {
    match clause.op {
        FieldOp::Empty => {
            builder.push(format!("({expr}) IS NULL"));
        }
        FieldOp::NotEmpty => {
            builder.push(format!("({expr}) IS NOT NULL"));
        }
        FieldOp::Is => {
            // Compared in the column's own type, so its indexes apply.
            builder.push(format!("({expr}) = ANY(CAST("));
            builder.push_bind(written_values(clause));
            builder.push(format!(" AS {sql_type}[]))"));
        }
        FieldOp::IsNot => {
            builder.push(format!("(({expr}) IS NULL OR NOT ({expr}) = ANY(CAST("));
            builder.push_bind(written_values(clause));
            builder.push(format!(" AS {sql_type}[])))"));
        }
        FieldOp::Contains => {
            builder.push(format!("({expr})::text ILIKE "));
            builder.push_bind(like_pattern(clause));
        }
        FieldOp::Gt | FieldOp::Gte | FieldOp::Lt | FieldOp::Lte => {
            let bound = clause
                .values
                .first()
                .map(|value| value.trim().to_owned())
                .unwrap_or_default();
            builder.push(format!("({expr}) {} CAST(", comparison(clause.op)));
            builder.push_bind(bound);
            builder.push(format!(" AS {sql_type})"));
        }
        FieldOp::Within => push_within(builder, expr, sql_type, clause, today),
        FieldOp::CategoryIs | FieldOp::CategoryIsNot => {
            let Some(category) = category else {
                builder.push("false");
                return;
            };
            if clause.op == FieldOp::CategoryIs {
                builder.push(format!("({category}) = ANY("));
            } else {
                builder.push(format!("(({category}) IS NULL OR NOT ({category}) = ANY("));
            }
            builder.push_bind(written_values(clause));
            builder.push(if clause.op == FieldOp::CategoryIs {
                "::text[])"
            } else {
                "::text[]))"
            });
        }
    }
}

fn push_elements_condition(
    builder: &mut QueryBuilder<'_, Postgres>,
    sql: &str,
    clause: &FieldClause,
) {
    match clause.op {
        FieldOp::Empty => {
            builder.push(format!("NOT EXISTS ({sql})"));
        }
        FieldOp::NotEmpty => {
            builder.push(format!("EXISTS ({sql})"));
        }
        FieldOp::Is | FieldOp::IsNot => {
            builder.push(if clause.op == FieldOp::Is {
                "EXISTS (SELECT 1 FROM ("
            } else {
                "NOT EXISTS (SELECT 1 FROM ("
            });
            builder.push(format!(
                "{sql}) AS element(value) WHERE element.value = ANY("
            ));
            builder.push_bind(written_values(clause));
            builder.push("))");
        }
        _ => {
            builder.push("false");
        }
    }
}

/// The days a `within` value covers from `today`: from (inclusive) and to
/// (exclusive), either open. None for a value that isn't one.
pub fn within_range(
    value: &str,
    today: NaiveDate,
) -> Option<(Option<NaiveDate>, Option<NaiveDate>)> {
    use chrono::{Datelike, Days, Months};
    let day = |offset: i64| {
        if offset >= 0 {
            today.checked_add_days(Days::new(offset as u64))
        } else {
            today.checked_sub_days(Days::new(offset.unsigned_abs()))
        }
    };
    // Weeks start on Monday.
    let monday = day(-i64::from(today.weekday().num_days_from_monday()))?;
    let week = |weeks: i64| {
        let start = if weeks >= 0 {
            monday.checked_add_days(Days::new(7 * weeks as u64))?
        } else {
            monday.checked_sub_days(Days::new(7 * weeks.unsigned_abs()))?
        };
        Some((Some(start), Some(start.checked_add_days(Days::new(7))?)))
    };
    let first = today.with_day(1)?;
    let month = |months: i32| {
        let start = if months >= 0 {
            first.checked_add_months(Months::new(months as u32))?
        } else {
            first.checked_sub_months(Months::new(months.unsigned_abs()))?
        };
        Some((Some(start), Some(start.checked_add_months(Months::new(1))?)))
    };
    let key = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    Some(match key.as_str() {
        "today" => (Some(today), day(1)),
        "yesterday" => (day(-1), Some(today)),
        "tomorrow" => (day(1), day(2)),
        "this_week" => week(0)?,
        "last_week" => week(-1)?,
        "next_week" => week(1)?,
        "this_month" => month(0)?,
        "last_month" => month(-1)?,
        "next_month" => month(1)?,
        "last_7_days" => (day(-6), day(1)),
        "last_30_days" => (day(-29), day(1)),
        "next_7_days" => (Some(today), day(7)),
        "next_30_days" => (Some(today), day(30)),
        "overdue" | "past" => (None, Some(today)),
        "future" => (day(1), None),
        _ => return None,
    })
}

/// What `within` takes, for messages.
pub const WITHIN_VALUES: &str = "today, yesterday, tomorrow, this_week, last_week, next_week, \
     this_month, last_month, next_month, last_7_days, last_30_days, next_7_days, next_30_days, \
     overdue, past or future";

/// The filter values that are numbers, written out in full as decimals
/// (Rust never writes an f64 with an exponent), so PostgreSQL reads each as
/// `numeric` and compares it with a stored number by value.
fn filter_numbers(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .filter(|number| number.is_finite())
        .map(|number| number.to_string())
        .collect()
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
/// pages by title; text, names and titles by their first
/// `SORT_PREFIX_CHARACTERS`). `column` is the JSONB column of values and
/// `project` the record's project ID column.
pub fn sort_keys(field: &FieldDefinition, column: &str, project: &str) -> Vec<SortKey> {
    let reading = Reading::of(field, column);
    let Reading {
        is_null,
        text,
        number,
        checked,
        elements,
        uuid,
    } = &reading;
    // `column = value`: in the value's own type where it has one.
    let matches = |column: &str| match uuid {
        Some(uuid) => format!("{column} = {uuid}"),
        None => format!("{column}::text = {text}"),
    };
    let empty_last = SortKey {
        expr: format!("({is_null})::integer"),
        ty: "integer",
        follows_direction: false,
    };
    let by_value = match field.field_type.behaves_as() {
        FieldType::Text
        | FieldType::Url
        | FieldType::Email
        | FieldType::Phone
        | FieldType::LongText
        | FieldType::People
        | FieldType::Labels
        | FieldType::CreatedTime
        | FieldType::EditedTime
        | FieldType::CreatedBy
        | FieldType::EditedBy
        | FieldType::AutoId => key(
            format!("left(lower(COALESCE({text}, '')), {SORT_PREFIX_CHARACTERS})"),
            "text",
        ),
        FieldType::Date => match &field.accessor {
            // A day or a time in its own type, so its index can order it.
            Accessor::Column { expr, sql_type, .. } if *sql_type == "date" => {
                key(format!("COALESCE({expr}, '-infinity'::date)"), "date")
            }
            Accessor::Column { expr, sql_type, .. } if *sql_type == "timestamptz" => key(
                format!("COALESCE({expr}, '-infinity'::timestamptz)"),
                "timestamptz",
            ),
            _ => key(format!("COALESCE({text}, '')"), "text"),
        },
        FieldType::Number => key(format!("COALESCE({number}, 0)"), "double precision"),
        FieldType::Checkbox => {
            return vec![key(format!("(NOT ({checked}))::integer"), "integer")];
        }
        FieldType::SingleChoice => key(
            format!(
                "COALESCE(array_position({}, {text}), 0)",
                uuid_array(field.config.options.iter().map(|option| option.id))
            ),
            "integer",
        ),
        // By category, in workflow order, then by option.
        FieldType::Status => key(
            format!(
                "COALESCE(array_position({}, {text}), 0)",
                uuid_array(status_order(field))
            ),
            "integer",
        ),
        FieldType::MultipleChoice => key(
            format!(
                "COALESCE((SELECT min(array_position({}, element.value)) FROM {elements}), 0)",
                uuid_array(field.config.options.iter().map(|option| option.id)),
            ),
            "integer",
        ),
        FieldType::Person => key(
            format!(
                "left(lower(COALESCE((SELECT sort_person.display_name FROM project_memberships sort_person \
                 WHERE sort_person.project_id = {project} AND {}), '')), \
                 {SORT_PREFIX_CHARACTERS})",
                matches("sort_person.actor_id")
            ),
            "text",
        ),
        FieldType::Link => match field.config.target {
            Some(LinkTarget::Record) => key(
                format!(
                    "left(lower(COALESCE((SELECT sort_record.title FROM collection_records sort_record \
                     WHERE sort_record.project_id = {project} AND {}), '')), \
                     {SORT_PREFIX_CHARACTERS})",
                    matches("sort_record.id")
                ),
                "text",
            ),
            Some(LinkTarget::Page) => key(
                format!(
                    "left(lower(COALESCE((SELECT sort_page.title FROM project_pages sort_page \
                     WHERE sort_page.project_id = {project} AND {}), '')), \
                     {SORT_PREFIX_CHARACTERS})",
                    matches("sort_page.id")
                ),
                "text",
            ),
            _ => key(
                format!(
                    "COALESCE((SELECT sort_task.number FROM work_items sort_task \
                     WHERE sort_task.project_id = {project} AND {}), 0)",
                    matches("sort_task.id")
                ),
                "bigint",
            ),
        },
    };
    vec![empty_last, by_value]
}

/// A status field's option IDs by category (workflow order), then in the
/// field's own order.
fn status_order(field: &FieldDefinition) -> impl Iterator<Item = Uuid> + '_ {
    let rank = |option: &ChoiceOption| {
        option
            .category
            .as_deref()
            .and_then(|category| {
                STATUS_CATEGORIES
                    .iter()
                    .position(|known| *known == category)
            })
            .unwrap_or(STATUS_CATEGORIES.len())
    };
    let mut options = field.config.options.iter().enumerate().collect::<Vec<_>>();
    options.sort_by_key(|(index, option)| (rank(option), *index));
    options.into_iter().map(|(_, option)| option.id)
}

/// A field's value as SQL, wherever it comes from (`Accessor`): whether it
/// has none, its first value as text, as a number, whether it's checked,
/// and its values as rows `element(value)` of text.
struct Reading {
    is_null: String,
    text: String,
    /// The value as a uuid, where it's kept as one: compared in its own
    /// type, a lookup by it can use the looked-up table's key.
    uuid: Option<String>,
    number: String,
    checked: String,
    elements: String,
}

impl Reading {
    fn of(field: &FieldDefinition, column: &str) -> Self {
        match &field.accessor {
            Accessor::Stored => {
                let value = value_at(column, field.id);
                let text = text_at(column, field.id);
                Self {
                    is_null: format!("{value} IS NULL"),
                    number: format!(
                        "CASE WHEN jsonb_typeof({value}) = 'number' THEN ({text})::double precision END"
                    ),
                    checked: format!("{value} IS NOT DISTINCT FROM 'true'::jsonb"),
                    elements: format!("{} AS element(value)", elements(column, field.id)),
                    text,
                    uuid: None,
                }
            }
            Accessor::Column { expr, sql_type, .. } => Self {
                is_null: format!("({expr}) IS NULL"),
                text: format!("(({expr})::text)"),
                uuid: (*sql_type == "uuid").then(|| format!("({expr})")),
                number: if matches!(
                    *sql_type,
                    "bigint" | "integer" | "double precision" | "numeric"
                ) {
                    format!("(({expr})::double precision)")
                } else {
                    "NULL::double precision".to_owned()
                },
                checked: if *sql_type == "boolean" {
                    format!("(({expr}) IS TRUE)")
                } else {
                    format!("(({expr}) IS NOT NULL)")
                },
                elements: format!(
                    "(SELECT ({expr})::text AS value WHERE ({expr}) IS NOT NULL) AS element"
                ),
            },
            Accessor::Elements { sql } => Self {
                is_null: format!("NOT EXISTS ({sql})"),
                text: format!("(SELECT min(first.value) FROM ({sql}) AS first(value))"),
                number: "NULL::double precision".to_owned(),
                checked: format!("EXISTS ({sql})"),
                elements: format!("({sql}) AS element(value)"),
                uuid: None,
            },
        }
    }
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

/// SQL for grouping by a field that `can_group`; a date by `bucket`. The
/// join is aliased `group_field`. A record holding several choices joins a
/// row for each, so it's in each of their groups.
pub fn group_sql(
    definition: &FieldDefinition,
    column: &str,
    project: &str,
    bucket: DateBucket,
) -> Option<GroupSql> {
    let field = definition.id;
    let Reading {
        text,
        checked,
        uuid,
        elements,
        ..
    } = Reading::of(definition, column);
    Some(match definition.field_type.behaves_as() {
        // Options of fields that aren't stored are their source's to name
        // (`views::source::RecordSource::group_sql`).
        FieldType::SingleChoice | FieldType::MultipleChoice | FieldType::Status
            if definition.accessor != Accessor::Stored =>
        {
            return None;
        }
        FieldType::MultipleChoice => GroupSql {
            join: format!(
                " LEFT JOIN LATERAL (SELECT choice.option ->> 'id' AS id, \
                   choice.option ->> 'name' AS name, \
                   choice.option ->> 'color' AS color, choice.position \
                   FROM field_definitions group_definition \
                   CROSS JOIN LATERAL jsonb_array_elements(group_definition.config -> 'options') \
                     WITH ORDINALITY AS choice(option, position) \
                   WHERE group_definition.project_id = {project} AND group_definition.id = '{field}' \
                     AND choice.option ->> 'id' IN (SELECT element.value FROM {elements})) \
                 group_field ON true"
            ),
            key: "COALESCE(group_field.id, 'none')".to_owned(),
            label: "COALESCE(group_field.name, 'No value')".to_owned(),
            color: "group_field.color".to_owned(),
            order: vec![
                key("(group_field.name IS NULL)::integer".to_owned(), "integer"),
                key("COALESCE(group_field.position, 0)".to_owned(), "bigint"),
            ],
        },
        // By category, in workflow order, then by option.
        FieldType::Status => GroupSql {
            join: format!(
                " LEFT JOIN LATERAL (SELECT choice.option ->> 'name' AS name, \
                   choice.option ->> 'color' AS color \
                   FROM field_definitions group_definition \
                   CROSS JOIN LATERAL jsonb_array_elements(group_definition.config -> 'options') \
                     AS choice(option) \
                   WHERE group_definition.project_id = {project} AND group_definition.id = '{field}' \
                     AND choice.option ->> 'id' = {text}) group_field ON true"
            ),
            key: format!("CASE WHEN group_field.name IS NULL THEN 'none' ELSE {text} END"),
            label: "COALESCE(group_field.name, 'No value')".to_owned(),
            color: "group_field.color".to_owned(),
            order: vec![
                key("(group_field.name IS NULL)::integer".to_owned(), "integer"),
                key(
                    format!(
                        "COALESCE(array_position({}, {text}), 0)",
                        uuid_array(status_order(definition))
                    ),
                    "integer",
                ),
            ],
        },
        FieldType::Date => {
            let day = match &definition.accessor {
                Accessor::Column { expr, sql_type, .. } if *sql_type == "timestamptz" => {
                    format!("((({expr}) AT TIME ZONE 'UTC')::date)")
                }
                Accessor::Column { expr, sql_type, .. } if *sql_type == "date" => {
                    format!("({expr})")
                }
                _ => format!(
                    "(CASE WHEN {text} ~ '^[0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}}$' THEN ({text})::date END)"
                ),
            };
            let start = format!("(date_trunc('{}', {day})::date)", bucket.as_str());
            let named = match bucket {
                DateBucket::Day => format!("to_char({start}, 'FMDD Mon YYYY')"),
                DateBucket::Week => format!("'Week of ' || to_char({start}, 'FMDD Mon YYYY')"),
                DateBucket::Month => format!("to_char({start}, 'FMMonth YYYY')"),
            };
            GroupSql {
                join: String::new(),
                key: format!("COALESCE(to_char({start}, 'YYYY-MM-DD'), 'none')"),
                label: format!("COALESCE({named}, 'No date')"),
                color: "NULL::text".to_owned(),
                order: vec![
                    key(format!("({day} IS NULL)::integer"), "integer"),
                    key(format!("COALESCE({start}, '-infinity'::date)"), "date"),
                ],
            }
        }
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
                   ON group_field.project_id = {project} AND {}",
                match &uuid {
                    Some(uuid) => format!("group_field.actor_id = {uuid}"),
                    None => format!("group_field.actor_id::text = {text}"),
                }
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
                    format!(
                        "left(lower(COALESCE(group_field.display_name, '')), {SORT_PREFIX_CHARACTERS})"
                    ),
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
            key: format!("CASE WHEN {checked} THEN 'true' ELSE 'none' END"),
            label: format!("CASE WHEN {checked} THEN 'Checked' ELSE 'Not checked' END"),
            color: "NULL::text".to_owned(),
            order: vec![key(format!("(NOT ({checked}))::integer"), "integer")],
        },
        _ => return None,
    })
}

/// `group_sql` for a stored field known only by its ID and type.
pub fn stored_group_sql(
    field: Uuid,
    field_type: FieldType,
    column: &str,
    project: &str,
) -> Option<GroupSql> {
    let definition = FieldDefinition {
        id: field,
        name: String::new(),
        description: String::new(),
        field_type,
        config: FieldConfig::default(),
        position: 0,
        archived: false,
        version: 0,
        accessor: Accessor::Stored,
    };
    group_sql(&definition, column, project, DateBucket::default())
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
         WHERE project_id = $1 AND record_kind = $2 AND collection_id IS NOT DISTINCT FROM $4 \
           AND ($3 OR archived_at IS NULL) \
         ORDER BY archived_at IS NOT NULL, position, id LIMIT {MAX_FIELD_ROWS}"
    ))
    .bind(project_id)
    .bind(kind.as_str())
    .bind(include_archived)
    .bind(kind.collection_id())
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
         WHERE project_id = $1 AND record_kind = $2 AND id = $3 \
           AND collection_id IS NOT DISTINCT FROM $4"
    ))
    .bind(project_id)
    .bind(kind.as_str())
    .bind(id)
    .bind(kind.collection_id())
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
    if !new.field_type.is_stored() || (kind == RecordKind::Task && !new.field_type.suits_tasks()) {
        return Err(AppError::Validation("type"));
    }
    // A link to records names a live collection of this project (AWD-110).
    if let Some(collection) = new.config.collection {
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM collections WHERE project_id = $1 AND id = $2 \
             AND archived_at IS NULL AND builtin IS NULL)",
        )
        .bind(project_id)
        .bind(collection)
        .fetch_one(&mut *connection)
        .await?;
        if !found {
            return Err(AppError::NotFound("collection"));
        }
    }
    let existing = list(&mut *connection, project_id, kind, false).await?;
    if let Some(taken) = existing
        .iter()
        .find(|field| field.name.to_lowercase() == new.name.to_lowercase())
    {
        return Ok(CreateFieldOutcome::NameTaken(taken.clone()));
    }
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM field_definitions WHERE project_id = $1 AND record_kind = $2 \
         AND collection_id IS NOT DISTINCT FROM $3",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(kind.collection_id())
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
            position, created_by_actor_id, collection_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7,
                (SELECT COALESCE(max(position) + 1, 0) FROM field_definitions
                 WHERE project_id = $2 AND record_kind = $3
                   AND collection_id IS NOT DISTINCT FROM $9), $8, $9)
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
    .bind(kind.collection_id())
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
         AND collection_id IS NOT DISTINCT FROM $3 \
         ORDER BY archived_at IS NOT NULL, position, id",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(kind.collection_id())
    .fetch_all(&mut *connection)
    .await?;
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM field_definitions WHERE project_id = $1 AND record_kind = $2 \
         AND archived_at IS NULL AND id <> $3 AND collection_id IS NOT DISTINCT FROM $4",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(id)
    .bind(kind.collection_id())
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
          AND field.collection_id IS NOT DISTINCT FROM $5
          AND field.position <> placed.position
        "#,
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(&order)
    .bind(&positions)
    .bind(kind.collection_id())
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
// Made once per call and matched at once: boxing the large variant
// buys nothing.
#[allow(clippy::large_enum_variant)]
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
        "SELECT id FROM field_definitions WHERE project_id = $1 AND record_kind = $2 AND id = $3 \
         AND collection_id IS NOT DISTINCT FROM $4 FOR UPDATE",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(field_id)
    .bind(kind.collection_id())
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
          AND collection_id IS NOT DISTINCT FROM $9
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
    .bind(kind.collection_id())
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
            accessor: crate::fields::Accessor::Stored,
        }
    }

    #[test]
    fn emails_and_phones_are_checked_and_kept_as_written() {
        assert_eq!(
            normalize_email(" Ada.Lovelace+notes@Example.CO.uk "),
            Some("ada.lovelace+notes@example.co.uk".to_owned())
        );
        for bad in [
            "ada",
            "ada@",
            "@example.com",
            "ada@example",
            "a..b@example.com",
            "ada@-x.com",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        assert_eq!(
            normalize_phone("+44  20 7946 0958"),
            Ok("+44 20 7946 0958".to_owned())
        );
        assert_eq!(
            normalize_phone("(020) 7946-0001"),
            Ok("(020) 7946-0001".to_owned())
        );
        assert!(normalize_phone("call me").is_err());
        assert!(normalize_phone("44+20").is_err());
        assert!(normalize_phone("12").is_err());
        assert_eq!(phone_digits("+44 (20) 7946-0958"), "+442079460958");
        let email = field(FieldType::Email);
        assert_eq!(
            normalize_value(&email, &json!("Ada@Example.com")),
            Ok(Normalized::Value(json!("ada@example.com")))
        );
        let phone = field(FieldType::Phone);
        assert!(
            normalize_value(&phone, &json!("ring ring"))
                .expect_err("not a phone number")
                .contains("only digits")
        );
        let long = field(FieldType::LongText);
        assert_eq!(
            normalize_value(&long, &json!("Line one\nLine two")),
            Ok(Normalized::Value(json!("Line one\nLine two")))
        );
        assert!(normalize_value(&long, &json!("x".repeat(MAX_LONG_TEXT_CHARACTERS + 1))).is_err());
    }

    #[test]
    fn system_fields_read_the_record_and_take_no_values() {
        for field_type in FieldType::ALL {
            assert_eq!(FieldType::parse(field_type.as_str()), Some(field_type));
            let definition = field(field_type);
            if field_type.is_system() {
                assert!(field_type.system_accessor().is_some());
                assert!(normalize_value(&definition, &json!("x")).is_err());
                assert!(!field_type.suits_tasks());
            }
        }
        assert_eq!(FieldType::CreatedBy.behaves_as(), FieldType::Person);
        assert_eq!(FieldType::AutoId.ops(), FieldType::Number.ops());
        let auto = field(FieldType::AutoId);
        let clause = |value: &str| FieldClause {
            field: auto.id,
            op: FieldOp::Gt,
            values: vec![value.to_owned()],
        };
        assert!(check_clause(&auto, &clause("12")).is_ok());
        assert!(check_clause(&auto, &clause("1.5")).is_err());
    }

    #[test]
    fn status_options_carry_categories() {
        let made = normalize_config(FieldType::Status, None, None, None).expect("defaults");
        assert_eq!(
            made.options
                .iter()
                .map(|option| option.category.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["unstarted", "started", "completed"]
        );
        let option = |name: &str, category: Option<&str>| OptionInput {
            id: None,
            name: name.to_owned(),
            color: None,
            category: category.map(str::to_owned),
        };
        let written = normalize_config(
            FieldType::Status,
            None,
            Some(&[option("Shipped", Some("Completed")), option("New", None)]),
            None,
        )
        .expect("written");
        assert_eq!(written.options[0].category.as_deref(), Some("completed"));
        assert_eq!(written.options[1].category.as_deref(), Some("unstarted"));
        let refused = normalize_config(
            FieldType::Status,
            None,
            Some(&[option("Odd", Some("someday"))]),
            None,
        );
        assert!(refused.is_err());
        assert!(
            normalize_config(
                FieldType::SingleChoice,
                None,
                Some(&[option("A", Some("started"))]),
                None
            )
            .is_err()
        );
        // Kept by ID, an option keeps its category.
        let kept = normalize_config(
            FieldType::Status,
            Some(&written),
            Some(&[OptionInput {
                id: Some(written.options[0].id.to_string()),
                ..option("Released", None)
            }]),
            None,
        )
        .expect("kept");
        assert_eq!(kept.options[0].category.as_deref(), Some("completed"));
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
                    category: None,
                },
                OptionInput {
                    id: None,
                    name: " High ".to_owned(),
                    color: Some("red".to_owned()),
                    category: None,
                },
            ]),
            None,
        )
        .expect("test value");
        field
    }

    /// "is" compares what was written, and compares by value only where a
    /// number is stored, so a text field's "0012" doesn't match its "12",
    /// while a number field's 1e-7 (stored, and written by PostgreSQL, as
    /// 0.0000001) matches "1e-7" and "0.00000010".
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
            sql.ends_with(&format!(
                "element.value = ANY($1) OR CASE WHEN jsonb_typeof({stored}) = 'number' \
                 THEN element.value::numeric = ANY($2::numeric[]) ELSE false END)"
            )),
            "{sql}"
        );
        // Every bracket closes (the integration test runs it for real).
        assert_eq!(sql.matches('(').count(), sql.matches(')').count(), "{sql}");
        let numbers = |values: &[&str]| {
            filter_numbers(
                &values
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            numbers(&[" 0012 ", "3.0", "abc", "NaN", "inf"]),
            vec!["12", "3"]
        );
        // Written in full, never with an exponent, so `numeric` reads them.
        assert_eq!(
            numbers(&["1e-7", "0.00000010"]),
            vec!["0.0000001", "0.0000001"]
        );
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
                category: None,
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
                    category: None,
                },
                OptionInput {
                    id: None,
                    name: "A".to_owned(),
                    color: None,
                    category: None,
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
                category: None,
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

    /// An archived option a task already holds stays chosen when the rest
    /// of the selection changes; newly choosing it is still refused.
    #[test]
    fn archived_options_stay_only_where_they_are_held() {
        let mut multiple = choice(FieldType::MultipleChoice);
        multiple.config.options[0].archived = true;
        let low = multiple.config.options[0].id.to_string();
        let high = multiple.config.options[1].id.to_string();
        let both = json!([low, high]);
        assert!(normalize_value(&multiple, &both).is_err());
        assert_eq!(
            normalize_value_keeping(&multiple, &both, Some(&json!([low]))),
            Ok(Normalized::Value(both.clone()))
        );
        assert_eq!(
            normalize_value_keeping(&multiple, &json!(["Low", "High"]), Some(&json!([low]))),
            Ok(Normalized::Value(both))
        );
        assert!(normalize_value_keeping(&multiple, &json!([low]), Some(&json!([high]))).is_err());

        let mut single = choice(FieldType::SingleChoice);
        single.config.options[0].archived = true;
        let low = single.config.options[0].id.to_string();
        assert!(normalize_value(&single, &json!(low)).is_err());
        assert_eq!(
            normalize_value_keeping(&single, &json!(low), Some(&json!(low))),
            Ok(Normalized::Value(json!(low)))
        );
    }

    #[test]
    fn sql_never_holds_what_callers_wrote() {
        let mut single = choice(FieldType::SingleChoice);
        single.config.options[0].name = "'; DROP TABLE x; --".to_owned();
        for key in sort_keys(&single, "item.field_values", "item.project_id") {
            assert!(!key.expr.contains("DROP"));
        }
        let group = group_sql(
            &single,
            "item.field_values",
            "item.project_id",
            DateBucket::Month,
        )
        .expect("test value");
        assert!(!group.join.contains("DROP"));
        assert!(stored_group_sql(single.id, FieldType::Text, "c", "p").is_none());
    }

    /// `within` reads from a given day, so a list in any time zone asks
    /// with its own today; weeks start on Monday.
    #[test]
    fn within_names_ranges_from_today() {
        let day = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("date");
        // A Saturday.
        let today = day("2026-11-14");
        assert_eq!(
            within_range("today", today),
            Some((Some(today), Some(day("2026-11-15"))))
        );
        assert_eq!(
            within_range("this week", today),
            Some((Some(day("2026-11-09")), Some(day("2026-11-16"))))
        );
        assert_eq!(
            within_range("last_7_days", today),
            Some((Some(day("2026-11-08")), Some(day("2026-11-15"))))
        );
        assert_eq!(
            within_range("next-30-days", today),
            Some((Some(today), Some(day("2026-12-14"))))
        );
        assert_eq!(
            within_range("last_month", today),
            Some((Some(day("2026-10-01")), Some(day("2026-11-01"))))
        );
        assert_eq!(within_range("overdue", today), Some((None, Some(today))));
        assert_eq!(within_range("someday", today), None);
        let date = field(FieldType::Date);
        let clause = |values: &[&str]| FieldClause {
            field: date.id,
            op: FieldOp::Within,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        };
        assert!(check_clause(&date, &clause(&["this_week"])).is_ok());
        assert!(check_clause(&date, &clause(&["fortnight"])).is_err());
        assert!(!clause(&["today", "tomorrow"]).shape_is_valid());
        assert!(check_clause(&field(FieldType::Text), &clause(&["today"])).is_err());
    }

    /// A field read from a column compiles in the column's own type, and
    /// knows a status's category only where its source says it.
    #[test]
    fn column_fields_filter_in_their_own_type() {
        let mut status = field(FieldType::SingleChoice);
        status.accessor = Accessor::Column {
            expr: "core.status_id".to_owned(),
            sql_type: "uuid",
            category: Some("status.semantic_category".to_owned()),
        };
        assert!(status.ops().contains(&FieldOp::CategoryIs));
        assert!(
            !field(FieldType::SingleChoice)
                .ops()
                .contains(&FieldOp::CategoryIs)
        );
        let sql = |field: &FieldDefinition, clause: &FieldClause| {
            let mut builder = QueryBuilder::<Postgres>::new("");
            push_condition(
                &mut builder,
                "record.field_values",
                Some(field),
                clause,
                today(),
            );
            builder.sql().to_owned()
        };
        let is = FieldClause {
            field: status.id,
            op: FieldOp::Is,
            values: vec![Uuid::now_v7().to_string()],
        };
        assert_eq!(
            sql(&status, &is),
            "(core.status_id) = ANY(CAST($1 AS uuid[]))"
        );
        let started = FieldClause {
            field: status.id,
            op: FieldOp::CategoryIs,
            values: vec!["started".to_owned()],
        };
        assert!(check_clause(&status, &started).is_ok());
        assert_eq!(
            sql(&status, &started),
            "(status.semantic_category) = ANY($1::text[])"
        );
        let keys = sort_keys(&status, "record.field_values", "record.project_id");
        assert_eq!(keys[0].expr, "((core.status_id) IS NULL)::integer");
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
