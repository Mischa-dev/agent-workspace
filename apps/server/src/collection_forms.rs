//! Forms that add a record to a collection (AWD-110), grown from bug report
//! links (`crate::bug_reports`). A form is a bug report link with a
//! collection and the form's settings: which of the collection's fields it
//! asks for, which are required, a line above them and a thank-you after.
//!
//! Anyone with the link fills it in at `/forms/{token}`, signed in or not.
//! The page reads the form here (`GET /api/v1/forms/{token}`: the
//! collection's name and the asked fields, never a record, a person or a
//! task), then posts to the link's report URL like any bug report. Each
//! answer becomes a record through `record_write`, as the person who made
//! the link, so it gets the same checks; only the asked fields are taken.
//!
//! People and links to tasks or pages are never asked on a form: their
//! pickers would show who and what is in the project.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    collections,
    fields::{FieldDefinition, FieldType},
};

/// Fields one form asks for, at most.
pub const MAX_FORM_FIELDS: usize = 30;
pub const MAX_INTRO_CHARACTERS: usize = 2_000;
pub const MAX_THANKS_CHARACTERS: usize = 500;
/// The longest answer to a text field, as a record keeps it.
const MAX_ANSWER_CHARACTERS: usize = 10_000;

/// A form's settings, as `bug_report_links.form` stores them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormConfig {
    /// Shown above the fields: what the form is for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    /// The fields it asks for, in order, after the title.
    #[serde(default)]
    pub fields: Vec<FormField>,
    /// Shown once an answer is in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thanks: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormField {
    pub field: Uuid,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// Whether a public form may ask for this type of field.
pub fn askable(field_type: FieldType) -> bool {
    !matches!(field_type, FieldType::Person | FieldType::Link)
}

fn clean(value: &str, limit: usize, what: &str) -> Result<String, String> {
    let value = value.trim();
    if value.chars().count() > limit {
        return Err(format!("The {what} is at most {limit} characters."));
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(format!("The {what} can't hold control characters."));
    }
    Ok(value.to_owned())
}

/// The form with its text trimmed, checked against the collection's
/// fields: each asked field is an active one of a type a form can ask,
/// once. The message says what to change.
pub fn check(form: &FormConfig, definitions: &[FieldDefinition]) -> Result<FormConfig, String> {
    if form.fields.len() > MAX_FORM_FIELDS {
        return Err(format!("A form asks for at most {MAX_FORM_FIELDS} fields."));
    }
    let mut seen = HashSet::new();
    for asked in &form.fields {
        let Some(field) = definitions
            .iter()
            .find(|field| field.id == asked.field && !field.archived)
        else {
            return Err("A form field names a field this collection doesn't have.".to_owned());
        };
        if !askable(field.field_type) {
            return Err(format!(
                "\"{}\" can't be on a form: people and links would show who and what is in the project.",
                field.name
            ));
        }
        if !seen.insert(field.id) {
            return Err(format!("\"{}\" is on the form twice.", field.name));
        }
    }
    Ok(FormConfig {
        intro: clean(&form.intro, MAX_INTRO_CHARACTERS, "introduction")?,
        fields: form.fields.clone(),
        thanks: clean(&form.thanks, MAX_THANKS_CHARACTERS, "thank-you note")?,
    })
}

// ---------------------------------------------------------------------------
// The form as the public page reads it

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicOption {
    pub id: Uuid,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicField {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "type")]
    pub field_type: &'static str,
    pub required: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PublicOption>,
}

/// What `/forms/{token}` shows: nothing but the form.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicForm {
    pub name: String,
    pub collection: String,
    /// What the title is called, such as Name or Summary.
    pub title_name: String,
    pub intro: String,
    pub thanks: String,
    pub fields: Vec<PublicField>,
}

/// The form a page shows: the fields still active and askable, in the
/// form's order (one archived since drops out until it's back).
pub fn public(
    name: &str,
    collection: &collections::Collection,
    form: &FormConfig,
    definitions: &[FieldDefinition],
) -> PublicForm {
    PublicForm {
        name: name.to_owned(),
        collection: collection.name.clone(),
        title_name: collection.title_name.clone(),
        intro: form.intro.clone(),
        thanks: form.thanks.clone(),
        fields: form
            .fields
            .iter()
            .filter_map(|asked| {
                let field = definitions
                    .iter()
                    .find(|field| field.id == asked.field && !field.archived)?;
                askable(field.field_type).then(|| PublicField {
                    id: field.id,
                    name: field.name.clone(),
                    description: field.description.clone(),
                    field_type: field.field_type.as_str(),
                    required: asked.required,
                    options: field
                        .config
                        .options
                        .iter()
                        .filter(|option| !option.archived)
                        .map(|option| PublicOption {
                            id: option.id,
                            name: option.name.clone(),
                            color: option.color.clone(),
                        })
                        .collect(),
                })
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Answers

/// An answer, read from whatever the page or another sender posted: a JSON
/// object, or a form whose repeated keys (a multiple choice) are lists.
pub fn parse_answer(content_type: Option<&str>, body: &[u8]) -> Result<Map<String, Value>, String> {
    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let text = std::str::from_utf8(body).map_err(|_| "Send JSON or a form.".to_owned())?;
    if content_type == "application/x-www-form-urlencoded" {
        let pairs = serde_urlencoded::from_str::<Vec<(String, String)>>(text)
            .map_err(|_| "Send JSON or a form.".to_owned())?;
        let mut answer = Map::new();
        for (key, value) in pairs {
            match answer.get_mut(&key) {
                Some(Value::Array(values)) => values.push(Value::String(value)),
                Some(existing) => {
                    let first = existing.take();
                    *existing = Value::Array(vec![first, Value::String(value)]);
                }
                None => {
                    answer.insert(key, Value::String(value));
                }
            }
        }
        return Ok(answer);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err("Send the answers as a JSON object or a form.".to_owned()),
    }
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(values) => values.iter().all(is_blank),
        _ => false,
    }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_owned()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// One answer as `record_write` takes it for the field, or a message.
fn value_for(field: &FieldDefinition, value: &Value) -> Result<Value, String> {
    let wrong = |what: &str| format!("\"{}\" takes {what}.", field.name);
    Ok(match field.field_type {
        FieldType::Text | FieldType::Url | FieldType::Date => {
            let text = text_of(value).ok_or_else(|| wrong("text"))?;
            if text.chars().count() > MAX_ANSWER_CHARACTERS {
                return Err(format!(
                    "\"{}\" is at most {MAX_ANSWER_CHARACTERS} characters.",
                    field.name
                ));
            }
            Value::String(text)
        }
        FieldType::Number => match value {
            Value::Number(_) => value.clone(),
            Value::String(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .map(|number| json!(number))
                .ok_or_else(|| wrong("a number"))?,
            _ => return Err(wrong("a number")),
        },
        FieldType::Checkbox => Value::Bool(match value {
            Value::Bool(flag) => *flag,
            other => matches!(
                text_of(other)
                    .map(|text| text.to_ascii_lowercase())
                    .as_deref(),
                Some("true" | "on" | "yes" | "1" | "checked")
            ),
        }),
        FieldType::SingleChoice => {
            let text = text_of(value).ok_or_else(|| wrong("one of its options"))?;
            Value::String(text)
        }
        FieldType::MultipleChoice => {
            let values = match value {
                Value::Array(values) => values.clone(),
                other => vec![other.clone()],
            };
            Value::Array(
                values
                    .iter()
                    .filter(|value| !is_blank(value))
                    .map(|value| text_of(value).map(Value::String))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| wrong("a list of its options"))?,
            )
        }
        FieldType::Person | FieldType::Link => return Err(wrong("nothing on a form")),
    })
}

/// A record from an answer: its title and the asked fields' values by
/// field ID, for `record_write`. Answers are found by field ID or name
/// (and the title as "title" or the title column's name); anything the
/// form doesn't ask for is left out. The message says what's missing.
pub fn record_from_answer(
    collection: &collections::Collection,
    form: &FormConfig,
    definitions: &[FieldDefinition],
    answer: &Map<String, Value>,
) -> Result<(String, Map<String, Value>), String> {
    let lookup = |keys: &[&str]| {
        answer.iter().find_map(|(key, value)| {
            keys.iter()
                .any(|wanted| key.trim().eq_ignore_ascii_case(wanted))
                .then_some(value)
        })
    };
    let title = lookup(&["title", collection.title_name.as_str()])
        .and_then(text_of)
        .unwrap_or_default();
    if title.is_empty() {
        return Err(format!("Fill in {}.", collection.title_name));
    }
    let mut values = Map::new();
    for asked in &form.fields {
        let Some(field) = definitions
            .iter()
            .find(|field| field.id == asked.field && !field.archived)
        else {
            continue;
        };
        if !askable(field.field_type) {
            continue;
        }
        let id = field.id.to_string();
        let given = lookup(&[id.as_str(), field.name.as_str()]).filter(|value| !is_blank(value));
        match given {
            Some(value) => {
                values.insert(id, value_for(field, value)?);
            }
            None if asked.required && field.field_type != FieldType::Checkbox => {
                return Err(format!("Fill in {}.", field.name));
            }
            None => {}
        }
    }
    Ok((title, values))
}

/// A stable ID for an answer, so the same answer sent twice the same day
/// (a double click, a retry) makes one record.
pub fn request_id(link_id: Uuid, title: &str, values: &Map<String, Value>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(link_id.as_bytes());
    hasher.update(title.as_bytes());
    hasher.update(Value::Object(values.clone()).to_string().as_bytes());
    hasher.update(chrono::Utc::now().format("%Y-%m-%d").to_string().as_bytes());
    format!("form-{}", hex::encode(&hasher.finalize()[..16]))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};
    use uuid::Uuid;

    use super::{FormConfig, FormField, check, parse_answer, record_from_answer};
    use crate::{
        collections::Collection,
        fields::{ChoiceOption, FieldConfig, FieldDefinition, FieldType},
    };

    fn field(name: &str, field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
        }
    }

    fn collection() -> Collection {
        Collection {
            id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            name: "Playtests".to_owned(),
            description: String::new(),
            title_name: "Finding".to_owned(),
            next_number: 1,
            position: 0,
            archived: false,
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_form_asks_only_for_the_collections_own_fields_and_never_people() {
        let notes = field("Notes", FieldType::Text);
        let tester = field("Tester", FieldType::Person);
        let task = field("Task", FieldType::Link);
        let fields = vec![notes.clone(), tester.clone(), task.clone()];
        let ask = |ids: &[Uuid]| FormConfig {
            fields: ids
                .iter()
                .map(|id| FormField {
                    field: *id,
                    required: false,
                })
                .collect(),
            ..FormConfig::default()
        };
        assert!(check(&ask(&[notes.id]), &fields).is_ok());
        assert!(check(&ask(&[tester.id]), &fields).is_err());
        assert!(check(&ask(&[task.id]), &fields).is_err());
        assert!(check(&ask(&[notes.id, notes.id]), &fields).is_err());
        assert!(check(&ask(&[Uuid::now_v7()]), &fields).is_err());
    }

    #[test]
    fn an_answer_becomes_a_record_of_only_the_asked_fields() {
        let mut severity = field("Severity", FieldType::SingleChoice);
        severity.config.options = vec![ChoiceOption {
            id: Uuid::now_v7(),
            name: "High".to_owned(),
            color: "red".to_owned(),
            archived: false,
        }];
        let score = field("Score", FieldType::Number);
        let platforms = field("Platforms", FieldType::MultipleChoice);
        let fixed = field("Fixed", FieldType::Checkbox);
        let secret = field("Internal notes", FieldType::Text);
        let fields = vec![
            severity.clone(),
            score.clone(),
            platforms.clone(),
            fixed.clone(),
            secret.clone(),
        ];
        let form = FormConfig {
            fields: [&severity, &score, &platforms, &fixed]
                .iter()
                .map(|field| FormField {
                    field: field.id,
                    required: field.id == severity.id,
                })
                .collect(),
            ..FormConfig::default()
        };
        let answer = parse_answer(
            Some("application/x-www-form-urlencoded"),
            b"finding=Falls+through+floor&Severity=High&Score=4.5&Platforms=PC&Platforms=Deck&Fixed=on&Internal+notes=sneaky",
        )
        .expect("form");
        let (title, values) =
            record_from_answer(&collection(), &form, &fields, &answer).expect("record");
        assert_eq!(title, "Falls through floor");
        assert_eq!(values[&severity.id.to_string()], json!("High"));
        assert_eq!(values[&score.id.to_string()], json!(4.5));
        assert_eq!(values[&platforms.id.to_string()], json!(["PC", "Deck"]));
        assert_eq!(values[&fixed.id.to_string()], json!(true));
        assert!(!values.contains_key(&secret.id.to_string()));

        // A required field left empty, a missing title, and a word for a
        // number are each refused with what to fix.
        let mut missing = Map::new();
        missing.insert("title".to_owned(), json!("Crash"));
        missing.insert("Severity".to_owned(), json!(" "));
        assert_eq!(
            record_from_answer(&collection(), &form, &fields, &missing),
            Err("Fill in Severity.".to_owned())
        );
        let untitled =
            parse_answer(Some("application/json"), br#"{"Severity": "High"}"#).expect("json");
        assert_eq!(
            record_from_answer(&collection(), &form, &fields, &untitled),
            Err("Fill in Finding.".to_owned())
        );
        let wordy = parse_answer(
            None,
            br#"{"title": "Crash", "Severity": "High", "Score": "lots"}"#,
        )
        .expect("json");
        assert!(record_from_answer(&collection(), &form, &fields, &wordy).is_err());
        assert!(parse_answer(None, b"plain words").is_err());
    }
}
