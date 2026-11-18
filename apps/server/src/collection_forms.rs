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
/// Answers a form takes in one UTC day before it pauses itself, unless its
/// settings say otherwise (T1.29).
pub const DEFAULT_DAILY_LIMIT: u32 = 500;
pub const MAX_DAILY_LIMIT: u32 = 10_000;
/// Answers one sender address may give one form in an hour.
pub const ANSWERS_PER_ADDRESS_PER_HOUR: u32 = 20;
/// An answer must come at least this long after its page was rendered, and
/// within a day of it.
pub const MIN_RENDER_AGE_MS: i64 = 3_000;
const MAX_RENDER_AGE_MS: i64 = 24 * 3_600 * 1_000;
/// The answer key that carries the page's render token.
pub const RENDER_TOKEN_KEY: &str = "_aw_rendered";
/// A field the page hides from people; anything in it means a script.
pub const HONEYPOT_KEY: &str = "_aw_homepage";

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
    /// Answers it takes in a UTC day before pausing itself and telling its
    /// maker; [`DEFAULT_DAILY_LIMIT`] when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_limit: Option<u32>,
}

impl FormConfig {
    pub fn daily_limit(&self) -> u32 {
        self.daily_limit.unwrap_or(DEFAULT_DAILY_LIMIT)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormField {
    pub field: Uuid,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// Whether a public form may ask for this type of field: not people or
/// links (the person filling it in can't see who or what there is), and
/// not the record's own columns, which nobody fills in.
pub fn askable(field_type: FieldType) -> bool {
    !matches!(field_type, FieldType::Person | FieldType::Link) && !field_type.is_read_only()
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
    if let Some(limit) = form.daily_limit
        && !(1..=MAX_DAILY_LIMIT).contains(&limit)
    {
        return Err(format!(
            "A form takes from 1 to {MAX_DAILY_LIMIT} answers a day."
        ));
    }
    Ok(FormConfig {
        intro: clean(&form.intro, MAX_INTRO_CHARACTERS, "introduction")?,
        fields: form.fields.clone(),
        thanks: clean(&form.thanks, MAX_THANKS_CHARACTERS, "thank-you note")?,
        daily_limit: form.daily_limit,
    })
}

// ---------------------------------------------------------------------------
// The render token: proof the page was loaded a moment before

/// `<milliseconds since 1970>.<signature>`, signed with the form's own key,
/// which only the server holds.
pub fn render_token(key: &[u8], link_id: Uuid, now_ms: i64) -> String {
    let signature = render_mac(key, link_id, now_ms)
        .map(|mac| hex::encode(hmac::Mac::finalize(mac).into_bytes()))
        .unwrap_or_default();
    format!("{now_ms}.{signature}")
}

fn render_mac(key: &[u8], link_id: Uuid, issued_ms: i64) -> Option<hmac::Hmac<Sha256>> {
    use hmac::Mac;
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(key).ok()?;
    mac.update(link_id.as_bytes());
    mac.update(&issued_ms.to_be_bytes());
    Some(mac)
}

/// Why an answer's render token doesn't let it in, in words for the page.
#[derive(Debug, Eq, PartialEq)]
pub enum RenderRefusal {
    /// Missing, forged, or more than a day old: load the form again.
    Reload,
    /// Sent sooner after the page loaded than a person could.
    TooSoon,
}

pub fn check_render_token(
    key: &[u8],
    link_id: Uuid,
    token: Option<&str>,
    now_ms: i64,
) -> Result<(), RenderRefusal> {
    use hmac::Mac;
    let (issued, signature) = token
        .and_then(|token| token.split_once('.'))
        .ok_or(RenderRefusal::Reload)?;
    let issued = issued.parse::<i64>().map_err(|_| RenderRefusal::Reload)?;
    let signature = hex::decode(signature).map_err(|_| RenderRefusal::Reload)?;
    render_mac(key, link_id, issued)
        .ok_or(RenderRefusal::Reload)?
        .verify_slice(&signature)
        .map_err(|_| RenderRefusal::Reload)?;
    let age = now_ms - issued;
    if !(0..=MAX_RENDER_AGE_MS).contains(&age) {
        return Err(RenderRefusal::Reload);
    }
    if age < MIN_RENDER_AGE_MS {
        return Err(RenderRefusal::TooSoon);
    }
    Ok(())
}

/// Whether the hidden field a person never sees was filled in.
pub fn caught_by_honeypot(answer: &Map<String, Value>) -> bool {
    answer
        .get(HONEYPOT_KEY)
        .is_some_and(|value| !is_blank(value))
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
    /// Sent back with the answer under [`RENDER_TOKEN_KEY`].
    pub render_token: String,
    /// The key of a field the page keeps out of sight; left empty.
    pub honeypot: &'static str,
}

/// The form a page shows: the fields still active and askable, in the
/// form's order (one archived since drops out until it's back).
pub fn public(
    name: &str,
    collection: &collections::Collection,
    form: &FormConfig,
    definitions: &[FieldDefinition],
    render_token: String,
) -> PublicForm {
    PublicForm {
        render_token,
        honeypot: HONEYPOT_KEY,
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
        FieldType::Text
        | FieldType::LongText
        | FieldType::Url
        | FieldType::Email
        | FieldType::Phone
        | FieldType::Date => {
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
        FieldType::SingleChoice | FieldType::Status => {
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
        FieldType::Person
        | FieldType::Link
        | FieldType::People
        | FieldType::Labels
        | FieldType::CreatedTime
        | FieldType::EditedTime
        | FieldType::CreatedBy
        | FieldType::EditedBy
        | FieldType::AutoId
        | FieldType::Formula
        | FieldType::Relation
        | FieldType::Rollup
        | FieldType::Button => return Err(wrong("nothing on a form")),
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
        // The title's keys answer the title only: a field named "Title" is
        // answered by its ID.
        let by_name = !(field.name.trim().eq_ignore_ascii_case("title")
            || field
                .name
                .trim()
                .eq_ignore_ascii_case(collection.title_name.trim()));
        let given = if by_name {
            lookup(&[id.as_str(), field.name.as_str()])
        } else {
            lookup(&[id.as_str()])
        }
        .filter(|value| !is_blank(value));
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

    use super::{
        FormConfig, FormField, MIN_RENDER_AGE_MS, RenderRefusal, caught_by_honeypot, check,
        check_render_token, parse_answer, record_from_answer, render_token,
    };
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
            accessor: crate::fields::Accessor::Stored,
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
            builtin: None,
            acts_like_task: false,
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
            category: None,
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

    #[test]
    fn a_field_named_title_never_takes_the_records_title() {
        let named_title = field("Title", FieldType::Number);
        let fields = vec![named_title.clone()];
        let form = FormConfig {
            fields: vec![FormField {
                field: named_title.id,
                required: false,
            }],
            ..FormConfig::default()
        };
        let answer = parse_answer(None, br#"{"title": "Crash"}"#).expect("json");
        let (title, values) =
            record_from_answer(&collection(), &form, &fields, &answer).expect("record");
        assert_eq!(title, "Crash");
        assert!(values.is_empty(), "{values:?}");
        let required = FormConfig {
            fields: vec![FormField {
                field: named_title.id,
                required: true,
            }],
            ..FormConfig::default()
        };
        assert_eq!(
            record_from_answer(&collection(), &required, &fields, &answer),
            Err("Fill in Title.".to_owned())
        );
        let by_id = parse_answer(
            None,
            format!(r#"{{"title": "Crash", "{}": 3}}"#, named_title.id).as_bytes(),
        )
        .expect("json");
        let (_, values) =
            record_from_answer(&collection(), &form, &fields, &by_id).expect("record");
        assert_eq!(values[&named_title.id.to_string()], json!(3));
    }

    #[test]
    fn a_render_token_lets_in_an_answer_after_a_moment_and_only_for_its_form() {
        let key = [7_u8; 32];
        let link = Uuid::now_v7();
        let issued = 1_800_000_000_000;
        let token = render_token(&key, link, issued);
        let at = |ms| check_render_token(&key, link, Some(&token), issued + ms);
        assert_eq!(at(MIN_RENDER_AGE_MS - 1), Err(RenderRefusal::TooSoon));
        assert_eq!(at(MIN_RENDER_AGE_MS), Ok(()));
        assert_eq!(at(25 * 3_600 * 1_000), Err(RenderRefusal::Reload));
        assert_eq!(at(-1), Err(RenderRefusal::Reload));
        // Another form's key, another form, a changed time, or none at all.
        let later = issued + MIN_RENDER_AGE_MS;
        assert_eq!(
            check_render_token(&[8; 32], link, Some(&token), later),
            Err(RenderRefusal::Reload)
        );
        assert_eq!(
            check_render_token(&key, Uuid::now_v7(), Some(&token), later),
            Err(RenderRefusal::Reload)
        );
        let forged = token.replacen(&issued.to_string(), &(issued - 60_000).to_string(), 1);
        assert_eq!(
            check_render_token(&key, link, Some(&forged), later),
            Err(RenderRefusal::Reload)
        );
        assert_eq!(
            check_render_token(&key, link, None, later),
            Err(RenderRefusal::Reload)
        );
    }

    #[test]
    fn the_honeypot_catches_only_a_filled_in_hidden_field() {
        let mut answer = Map::new();
        assert!(!caught_by_honeypot(&answer));
        answer.insert("_aw_homepage".to_owned(), json!(""));
        assert!(!caught_by_honeypot(&answer));
        answer.insert("_aw_homepage".to_owned(), json!("https://spam.example"));
        assert!(caught_by_honeypot(&answer));
    }

    #[test]
    fn a_daily_limit_is_from_one_to_ten_thousand() {
        for (limit, ok) in [
            (Some(0), false),
            (Some(1), true),
            (Some(10_000), true),
            (Some(10_001), false),
            (None, true),
        ] {
            let form = FormConfig {
                daily_limit: limit,
                ..FormConfig::default()
            };
            assert_eq!(check(&form, &[]).is_ok(), ok, "{limit:?}");
        }
        assert_eq!(FormConfig::default().daily_limit(), 500);
    }
}
