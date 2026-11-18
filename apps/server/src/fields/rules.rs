//! The rules a collection's field holds its values to (T1.19): a value is
//! required, unique among the collection's records, within a range or
//! matching a pattern; a new record starts with a default; only people who
//! may build the collection change it.
//!
//! Record writes (`collections::records::write`) apply them, so the web
//! app, agents, forms and automations meet the same rules. Fields of tasks
//! and of collections whose records are tasks don't take rules yet: their
//! writes go through the task pipeline.

use regex_lite::Regex;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{FieldDefinition, FieldType, Normalized};

/// Characters a pattern holds, at most.
pub const MAX_PATTERN_CHARACTERS: usize = 200;

fn is_false(value: &bool) -> bool {
    !*value
}

/// A field's rules, as `FieldConfig::rules` stores them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldRules {
    /// Every record has a value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// No two live records have the same value (text ignoring case).
    #[serde(default, skip_serializing_if = "is_false")]
    pub unique: bool,
    /// A new record's value when it's given none, as written: "me" names
    /// whoever adds the record and "today" the day it's added (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Numbers: the smallest and largest allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Text: a pattern the whole value matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Only people who may build the collection change the value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub builders_only: bool,
}

impl FieldRules {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A field's rules as someone writes them. Every rule is set at once: one
/// left out is off.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulesInput {
    /// Every record needs a value.
    #[serde(default)]
    pub required: bool,
    /// No two records may have the same value (text, email, URL, phone or
    /// number fields).
    #[serde(default)]
    pub unique: bool,
    /// A new record's value when it's given none, written as a value
    /// would be; "me" for a person field, "today" for a date field.
    #[serde(default)]
    pub default: Option<Value>,
    /// Number fields: the smallest value allowed.
    #[serde(default)]
    pub min: Option<serde_json::Number>,
    /// Number fields: the largest value allowed.
    #[serde(default)]
    pub max: Option<serde_json::Number>,
    /// Text fields: a pattern (regular expression) the whole value must
    /// match, at most 200 characters.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Only people who may build the collection may change the value.
    #[serde(default)]
    pub builders_only: bool,
}

impl From<&FieldRules> for RulesInput {
    fn from(rules: &FieldRules) -> Self {
        Self {
            required: rules.required,
            unique: rules.unique,
            default: rules.default.clone(),
            min: rules.min.and_then(serde_json::Number::from_f64),
            max: rules.max.and_then(serde_json::Number::from_f64),
            pattern: rules.pattern.clone(),
            builders_only: rules.builders_only,
        }
    }
}

/// Whether a field of this type takes rules at all: one whose value
/// someone writes.
pub fn takes_rules(field_type: FieldType) -> bool {
    !field_type.is_read_only()
        && !matches!(
            field_type,
            FieldType::Button | FieldType::People | FieldType::Labels
        )
}

fn takes_unique(field_type: FieldType) -> bool {
    matches!(
        field_type,
        FieldType::Text | FieldType::Email | FieldType::Url | FieldType::Phone | FieldType::Number
    )
}

fn takes_pattern(field_type: FieldType) -> bool {
    matches!(field_type, FieldType::Text | FieldType::LongText)
}

fn compile(pattern: &str) -> Option<Regex> {
    Regex::new(&format!("^(?:{pattern})$")).ok()
}

/// Checks rules written for `field` (its type and settings, the rules not
/// yet on it). The message says what to fix.
pub fn normalize(
    field: &FieldDefinition,
    input: &RulesInput,
) -> Result<Option<FieldRules>, String> {
    let name = &field.name;
    let field_type = field.field_type;
    if !takes_rules(field_type) {
        return Err(format!(
            "\"{name}\" is worked out or set by the app, so it takes no rules."
        ));
    }
    if input.unique && !takes_unique(field_type) {
        return Err("Only text, email, URL, phone and number fields can be unique.".to_owned());
    }
    if (input.min.is_some() || input.max.is_some()) && field_type != FieldType::Number {
        return Err("Only number fields have a smallest or largest value.".to_owned());
    }
    let min = input.min.as_ref().and_then(serde_json::Number::as_f64);
    let max = input.max.as_ref().and_then(serde_json::Number::as_f64);
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(format!(
            "The smallest value ({min}) is above the largest ({max})."
        ));
    }
    let pattern = input
        .pattern
        .as_deref()
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty());
    if let Some(pattern) = pattern {
        if !takes_pattern(field_type) {
            return Err("Only text fields have a pattern.".to_owned());
        }
        if pattern.chars().count() > MAX_PATTERN_CHARACTERS {
            return Err(format!(
                "A pattern is at most {MAX_PATTERN_CHARACTERS} characters."
            ));
        }
        if compile(pattern).is_none() {
            return Err(format!(
                "{pattern} isn't a pattern this app reads: write a regular expression, such as [A-Z]{{3}}-[0-9]+."
            ));
        }
    }
    let mut rules = FieldRules {
        required: input.required,
        unique: input.unique,
        default: None,
        min,
        max,
        pattern: pattern.map(str::to_owned),
        builders_only: input.builders_only,
    };
    if let Some(default) = input.default.as_ref().filter(|value| !value.is_null()) {
        rules.default = Some(normalize_default(field, &rules, default)?);
    }
    Ok((!rules.is_empty()).then_some(rules))
}

/// A default as stored: checked as a value would be, people and dates
/// that are worked out at each write kept as written.
fn normalize_default(
    field: &FieldDefinition,
    rules: &FieldRules,
    default: &Value,
) -> Result<Value, String> {
    let name = &field.name;
    match field.field_type {
        FieldType::Relation | FieldType::Link => {
            return Err(format!(
                "\"{name}\" can't have a default: what it links to is picked each time."
            ));
        }
        FieldType::Person => {
            return match default.as_str().map(str::trim) {
                Some(written) if written.eq_ignore_ascii_case("me") => {
                    Ok(Value::String("me".to_owned()))
                }
                _ => Err(format!(
                    "\"{name}\"'s default can be \"me\": whoever adds the record."
                )),
            };
        }
        FieldType::Date
            if default
                .as_str()
                .is_some_and(|text| text.trim().eq_ignore_ascii_case("today")) =>
        {
            return Ok(Value::String("today".to_owned()));
        }
        _ => {}
    }
    if rules.unique {
        return Err(format!(
            "\"{name}\" is unique, so a default would be the same on every record."
        ));
    }
    match super::normalize_value(field, default) {
        Ok(Normalized::Value(value)) => {
            check(field, rules, &value).map_err(|message| format!("The default: {message}"))?;
            Ok(value)
        }
        Ok(Normalized::Clear) => Err(format!("\"{name}\"'s default is empty.")),
        Ok(_) => Err(format!("\"{name}\" can't have a default.")),
        Err(message) => Err(format!("The default: {message}")),
    }
}

/// The default a new record starts with, as it would be written:
/// "today" is today's date.
pub fn default_written(rules: &FieldRules) -> Option<Value> {
    let default = rules.default.as_ref()?;
    Some(match default.as_str() {
        Some("today") => Value::String(chrono::Utc::now().date_naive().to_string()),
        _ => default.clone(),
    })
}

/// Checks a stored value against a field's range and pattern.
pub fn check(field: &FieldDefinition, rules: &FieldRules, value: &Value) -> Result<(), String> {
    let name = &field.name;
    if let Some(number) = value.as_f64() {
        if let Some(min) = rules.min
            && number < min
        {
            return Err(format!("\"{name}\" is at least {min}."));
        }
        if let Some(max) = rules.max
            && number > max
        {
            return Err(format!("\"{name}\" is at most {max}."));
        }
    }
    if let (Some(pattern), Some(text)) = (rules.pattern.as_deref(), value.as_str())
        && !compile(pattern).is_some_and(|pattern| pattern.is_match(text))
    {
        return Err(format!("\"{name}\" must match {pattern}."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{FieldRules, RulesInput, check, normalize};
    use crate::fields::{Accessor, FieldConfig, FieldDefinition, FieldType};

    fn field(field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: "Code".to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
            accessor: Accessor::Stored,
        }
    }

    #[test]
    fn rules_fit_their_types() {
        let number = field(FieldType::Number);
        let ranged = normalize(
            &number,
            &RulesInput {
                min: Some(1.into()),
                max: Some(5.into()),
                default: Some(json!(3)),
                ..RulesInput::default()
            },
        )
        .expect("normalized")
        .expect("rules");
        assert_eq!(ranged.default, Some(json!(3)));
        assert!(check(&number, &ranged, &json!(6)).is_err());
        assert!(check(&number, &ranged, &json!(5)).is_ok());
        // A default outside the range, a pattern on a number, a backwards
        // range: each refused.
        for input in [
            RulesInput {
                max: Some(2.into()),
                default: Some(json!(3)),
                ..RulesInput::default()
            },
            RulesInput {
                pattern: Some("x".into()),
                ..RulesInput::default()
            },
            RulesInput {
                min: Some(3.into()),
                max: Some(1.into()),
                ..RulesInput::default()
            },
        ] {
            assert!(normalize(&number, &input).is_err(), "{input:?}");
        }
        let text = field(FieldType::Text);
        let coded = normalize(
            &text,
            &RulesInput {
                pattern: Some("[A-Z]{3}-[0-9]+".into()),
                unique: true,
                ..RulesInput::default()
            },
        )
        .expect("normalized")
        .expect("rules");
        assert!(check(&text, &coded, &json!("ABC-12")).is_ok());
        assert!(check(&text, &coded, &json!("ABC-12 and more")).is_err());
        assert!(
            normalize(
                &text,
                &RulesInput {
                    pattern: Some("(".into()),
                    ..RulesInput::default()
                }
            )
            .is_err()
        );
        assert!(
            normalize(
                &field(FieldType::Date),
                &RulesInput {
                    unique: true,
                    ..RulesInput::default()
                }
            )
            .is_err()
        );
        assert_eq!(
            normalize(&text, &RulesInput::default()).expect("normalized"),
            None::<FieldRules>
        );
        let person = field(FieldType::Person);
        assert_eq!(
            normalize(
                &person,
                &RulesInput {
                    default: Some(json!("Me")),
                    ..RulesInput::default()
                }
            )
            .expect("normalized")
            .expect("normalized")
            .default,
            Some(json!("me"))
        );
        assert!(
            normalize(
                &person,
                &RulesInput {
                    default: Some(json!("@ada")),
                    ..RulesInput::default()
                }
            )
            .is_err()
        );
        assert!(
            normalize(
                &field(FieldType::Formula),
                &RulesInput {
                    required: true,
                    ..RulesInput::default()
                }
            )
            .is_err()
        );
    }
}
