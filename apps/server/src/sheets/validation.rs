//! Validation and dropdowns (C1.15): a tab's rules for what its cells may
//! hold, such as "Yes or No" for a column. A strict rule refuses a write
//! that breaks it, naming the rule; a lenient one keeps the input and marks
//! the cell. A list rule gives the grid a dropdown, a checkbox rule a box.
//!
//! Rules live on the tab (`sheet_tabs.validations`), are checked against
//! what people and agents type (formulas are left alone: their value isn't
//! known until it's worked out), and follow inserted and deleted rows and
//! columns like the tab's other ranges.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::address;

/// The most rules one tab keeps.
pub const MAX_RULES_PER_TAB: usize = 50;
/// The most choices a list offers.
pub const MAX_CHOICES: usize = 200;
const MAX_CHOICE: usize = 100;
const MAX_HINT: usize = 200;

/// What a rule allows.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "allow", rename_all = "camelCase")]
pub enum Allowed {
    /// One of these choices (a dropdown), matched without minding case.
    List { choices: Vec<String> },
    /// TRUE or FALSE (a checkbox).
    Checkbox,
    /// A number, optionally whole and within min and max.
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        whole: bool,
    },
    /// A date (YYYY-MM-DD), optionally from and to.
    Date {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Text of at most (and at least) this many characters.
    TextLength {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
}

fn strict_by_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// One rule: `range` (such as C2:C200), what it allows, `strict` (true
/// by default: refuse what breaks it; false: keep it and mark the cell)
/// and a `hint` shown with the cell.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationRule {
    pub range: String,
    #[serde(flatten)]
    pub allowed: Allowed,
    #[serde(default = "strict_by_default", skip_serializing_if = "is_true")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

// Kept rules never hold NaN (`check` refuses limits that aren't finite),
// so equality is total.
impl Eq for Allowed {}
impl Eq for ValidationRule {}

/// A date written YYYY-MM-DD, as days for comparing.
fn date(text: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// A typed number as the sheet reads one: `1,200`, `$5`, `10%`.
fn number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let (body, scale) = match trimmed.strip_suffix('%') {
        Some(body) => (body, 0.01),
        None => (trimmed, 1.0),
    };
    let cleaned = body
        .trim()
        .trim_start_matches(['$', '€', '£'])
        .replace(',', "");
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value * scale)
}

fn shown(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

impl ValidationRule {
    /// Why this rule can't be kept, if it can't.
    pub fn check(&self) -> Result<(), String> {
        if address::range(&self.range, super::MAX_ROWS, super::MAX_COLUMNS).is_none() {
            return Err(format!(
                "\"{}\" isn't a range such as C2:C200.",
                self.range.chars().take(40).collect::<String>()
            ));
        }
        if self
            .hint
            .as_ref()
            .is_some_and(|hint| hint.chars().count() > MAX_HINT)
        {
            return Err(format!("A rule's hint is up to {MAX_HINT} characters."));
        }
        match &self.allowed {
            Allowed::List { choices } => {
                if choices.is_empty() || choices.len() > MAX_CHOICES {
                    return Err(format!("A list offers 1 to {MAX_CHOICES} choices."));
                }
                if choices
                    .iter()
                    .any(|choice| choice.trim().is_empty() || choice.chars().count() > MAX_CHOICE)
                {
                    return Err(format!("Each choice is 1 to {MAX_CHOICE} characters."));
                }
            }
            Allowed::Checkbox => {}
            Allowed::Number { min, max, .. } => {
                if let (Some(min), Some(max)) = (min, max)
                    && min > max
                {
                    return Err("A number rule's min is at most its max.".to_owned());
                }
                if [min, max]
                    .into_iter()
                    .flatten()
                    .any(|value| !value.is_finite())
                {
                    return Err("A number rule's limits are numbers.".to_owned());
                }
            }
            Allowed::Date { from, to } => {
                let parsed =
                    [from, to].map(|value| value.as_deref().map(|text| (text, date(text))));
                for (text, value) in parsed.iter().flatten() {
                    if value.is_none() {
                        return Err(format!("\"{text}\" isn't a date written YYYY-MM-DD."));
                    }
                }
                if let [Some((_, Some(from))), Some((_, Some(to)))] = parsed
                    && from > to
                {
                    return Err("A date rule's start is on or before its end.".to_owned());
                }
            }
            Allowed::TextLength { min, max } => {
                if min.is_none() && max.is_none() {
                    return Err("A text length rule needs a min or a max.".to_owned());
                }
                if let (Some(min), Some(max)) = (min, max)
                    && min > max
                {
                    return Err("A text length rule's min is at most its max.".to_owned());
                }
            }
        }
        Ok(())
    }

    /// What it allows, in words: "Yes, No", "a whole number from 1 to 5".
    pub fn describe(&self) -> String {
        match &self.allowed {
            Allowed::List { choices } => {
                let mut listed = choices.iter().take(8).cloned().collect::<Vec<_>>();
                if choices.len() > 8 {
                    listed.push(format!("and {} more", choices.len() - 8));
                }
                format!("one of {}", listed.join(", "))
            }
            Allowed::Checkbox => "TRUE or FALSE".to_owned(),
            Allowed::Number { min, max, whole } => {
                let kind = if *whole { "a whole number" } else { "a number" };
                match (min, max) {
                    (Some(min), Some(max)) => {
                        format!("{kind} from {} to {}", shown(*min), shown(*max))
                    }
                    (Some(min), None) => format!("{kind} of at least {}", shown(*min)),
                    (None, Some(max)) => format!("{kind} of at most {}", shown(*max)),
                    (None, None) => kind.to_owned(),
                }
            }
            Allowed::Date { from, to } => match (from, to) {
                (Some(from), Some(to)) => format!("a date from {from} to {to}"),
                (Some(from), None) => format!("a date on or after {from}"),
                (None, Some(to)) => format!("a date on or before {to}"),
                (None, None) => "a date (YYYY-MM-DD)".to_owned(),
            },
            Allowed::TextLength { min, max } => match (min, max) {
                (Some(min), Some(max)) => format!("text of {min} to {max} characters"),
                (Some(min), None) => format!("text of at least {min} characters"),
                (None, Some(max)) => {
                    format!("text of at most {max} characters")
                }
                (None, None) => "text".to_owned(),
            },
        }
    }

    /// Whether the rule covers a cell (rows and columns from 1).
    pub fn covers(&self, row: i32, column: i32) -> bool {
        address::range(&self.range, super::MAX_ROWS, super::MAX_COLUMNS).is_some_and(|range| {
            (range.rows.0..=range.rows.1).contains(&row)
                && (range.columns.0..=range.columns.1).contains(&column)
        })
    }

    /// Whether `input` (as typed) is allowed. Empty cells and formulas are.
    pub fn allows(&self, input: &str) -> bool {
        let text = input.trim();
        if text.is_empty() || input.starts_with('=') {
            return true;
        }
        match &self.allowed {
            Allowed::List { choices } => choices
                .iter()
                .any(|choice| choice.trim().eq_ignore_ascii_case(text) || choice.trim() == text),
            Allowed::Checkbox => {
                text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false")
            }
            Allowed::Number { min, max, whole } => number(text).is_some_and(|value| {
                (!whole || value.fract() == 0.0)
                    && min.is_none_or(|min| value >= min)
                    && max.is_none_or(|max| value <= max)
            }),
            Allowed::Date { from, to } => date(text).is_some_and(|value| {
                from.as_deref()
                    .and_then(date)
                    .is_none_or(|from| value >= from)
                    && to.as_deref().and_then(date).is_none_or(|to| value <= to)
            }),
            Allowed::TextLength { min, max } => {
                let length = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
                min.is_none_or(|min| length >= min) && max.is_none_or(|max| length <= max)
            }
        }
    }
}

/// A tab's rules as kept, each range written out (`c2:c9` → `C2:C9`), or
/// why they can't be.
pub fn checked(mut rules: Vec<ValidationRule>) -> Result<Vec<ValidationRule>, String> {
    if rules.len() > MAX_RULES_PER_TAB {
        return Err(format!(
            "A tab keeps up to {MAX_RULES_PER_TAB} validation rules."
        ));
    }
    for (index, rule) in rules.iter_mut().enumerate() {
        rule.check()
            .map_err(|message| format!("Validation rule {index}: {message}"))?;
        if let Some(range) = address::range(&rule.range, super::MAX_ROWS, super::MAX_COLUMNS) {
            rule.range = range.name();
        }
        if let Some(hint) = &rule.hint {
            let hint = hint.trim();
            rule.hint = (!hint.is_empty()).then(|| hint.to_owned());
        }
    }
    Ok(rules)
}

/// The first strict rule `input` breaks at a cell, if any.
pub fn refusing<'a>(
    rules: &'a [ValidationRule],
    row: i32,
    column: i32,
    input: &str,
) -> Option<&'a ValidationRule> {
    rules
        .iter()
        .find(|rule| rule.strict && rule.covers(row, column) && !rule.allows(input))
}

/// Whether a cell's input breaks any rule on it (strict or not).
pub fn breaks(rules: &[ValidationRule], row: i32, column: i32, input: &str) -> bool {
    rules
        .iter()
        .any(|rule| rule.covers(row, column) && !rule.allows(input))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rule(value: serde_json::Value) -> ValidationRule {
        serde_json::from_value(value).expect("rule")
    }

    #[test]
    fn rules_read_flat_and_strict_by_default() {
        let yes_no = rule(json!({"range": "c2:c9", "allow": "list", "choices": ["Yes", "No"]}));
        assert!(yes_no.strict);
        let kept = checked(vec![yes_no]).expect("kept");
        assert_eq!(kept[0].range, "C2:C9");
        assert_eq!(
            serde_json::to_value(&kept[0]).expect("json"),
            json!({"range": "C2:C9", "allow": "list", "choices": ["Yes", "No"]})
        );
        assert!(kept[0].allows("yes") && kept[0].allows("") && kept[0].allows("=A1"));
        assert!(!kept[0].allows("Maybe"));
        assert_eq!(
            refusing(&kept, 3, 3, "Maybe").map(ValidationRule::describe),
            Some("one of Yes, No".to_owned())
        );
        assert!(
            refusing(&kept, 3, 4, "Maybe").is_none(),
            "outside the range"
        );
        assert!(refusing(&kept, 10, 3, "Maybe").is_none(), "below the range");
    }

    #[test]
    fn numbers_dates_and_lengths_are_read_as_typed() {
        let score =
            rule(json!({"range": "B2:B9", "allow": "number", "min": 1, "max": 5, "whole": true}));
        assert!(score.allows("3") && score.allows(" 5 "));
        assert!(!score.allows("2.5") && !score.allows("6") && !score.allows("three"));
        let share = rule(json!({"range": "B2:B9", "allow": "number", "max": 1}));
        assert!(share.allows("50%") && !share.allows("150%") && share.allows("$0.5"));
        let due = rule(json!({"range": "D2:D9", "allow": "date", "from": "2026-01-01"}));
        assert!(due.allows("2026-11-18") && !due.allows("2025-12-31") && !due.allows("soon"));
        let short =
            rule(json!({"range": "E2:E9", "allow": "textLength", "max": 3, "strict": false}));
        assert!(short.allows("abc") && !short.allows("abcd"));
        assert!(
            refusing(std::slice::from_ref(&short), 2, 5, "abcd").is_none(),
            "lenient"
        );
        assert!(breaks(std::slice::from_ref(&short), 2, 5, "abcd"));
    }

    #[test]
    fn bad_rules_are_refused_with_a_reason() {
        for bad in [
            json!({"range": "nowhere", "allow": "checkbox"}),
            json!({"range": "A1:A2", "allow": "list", "choices": []}),
            json!({"range": "A1:A2", "allow": "number", "min": 5, "max": 1}),
            json!({"range": "A1:A2", "allow": "date", "from": "18/11/2026"}),
            json!({"range": "A1:A2", "allow": "textLength"}),
        ] {
            assert!(rule(bad.clone()).check().is_err(), "{bad}");
        }
    }
}
