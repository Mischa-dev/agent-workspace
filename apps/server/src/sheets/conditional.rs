//! Conditional formatting (C1.17): rules on a tab that change how cells
//! look by what they hold, such as "less than 0 is red". A tab keeps its
//! rules in order (the first that applies to a cell wins); the engine works
//! them out with the values, so every reader sees the same colours, and
//! they go into an `.xlsx` as Excel's own conditional formats.

use ironcalc_base::{
    cf_types::{CfRuleInput, Cfvo, ColorScaleThreshold, TextOperator, ValueOperator},
    types::{Color, Dxf, DxfFont, Fill},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::address;

/// At most this many rules on one tab.
pub const MAX_RULES_PER_TAB: usize = 25;
const MAX_TEXT: usize = 200;
const MAX_FORMULA: usize = 1_000;

/// When a rule applies to a cell.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "when", deny_unknown_fields)]
pub enum RuleTest {
    /// The value is less than `value` (a number, text, or a formula such as `=$B$1`).
    LessThan {
        value: Json,
    },
    GreaterThan {
        value: Json,
    },
    AtLeast {
        value: Json,
    },
    AtMost {
        value: Json,
    },
    Equals {
        value: Json,
    },
    NotEquals {
        value: Json,
    },
    /// From `value` to `value2`, both included.
    Between {
        value: Json,
        value2: Json,
    },
    NotBetween {
        value: Json,
        value2: Json,
    },
    /// The text holds `text`, ignoring case.
    TextContains {
        text: String,
    },
    TextDoesNotContain {
        text: String,
    },
    TextBeginsWith {
        text: String,
    },
    TextEndsWith {
        text: String,
    },
    Empty {},
    NotEmpty {},
    /// The cell shows an error such as #DIV/0!.
    Errors {},
    /// A formula that is TRUE for the cell, written for the range's first
    /// cell, such as `=$C2="Late"`.
    Formula {
        formula: String,
    },
    /// The `rank` largest values (or the top `rank` percent).
    Top {
        rank: u32,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        percent: bool,
    },
    Bottom {
        rank: u32,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        percent: bool,
    },
    AboveAverage {},
    BelowAverage {},
    /// Values that appear more than once in the range.
    Duplicates {},
    /// A colour from `low` at the smallest value to `high` at the largest
    /// (through `mid` at the middle, if given); the rule's style is unused.
    ColorScale {
        low: String,
        high: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mid: Option<String>,
    },
}

/// What a rule does to the cells it applies to.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleStyle {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    /// The text's colour, `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The background, `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
}

impl RuleStyle {
    /// Changes nothing (a colour scale's rule has no style of its own).
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// One conditional format: the cells it covers, when it applies, and how
/// they look then.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConditionalRule {
    /// The cells it covers, such as `B2:B40`.
    pub range: String,
    #[serde(flatten)]
    pub test: RuleTest,
    #[serde(default, skip_serializing_if = "RuleStyle::is_plain")]
    pub style: RuleStyle,
}

/// A rule's value as Excel reads it in a comparison: a number, quoted text,
/// or a formula (from `=`) as written.
fn operand(value: &Json) -> Result<String, String> {
    match value {
        Json::Number(number) => Ok(number.to_string()),
        Json::Bool(value) => Ok(if *value { "TRUE" } else { "FALSE" }.to_owned()),
        Json::String(text) if text.starts_with('=') => Ok(text[1..].to_owned()),
        Json::String(text) => match text.trim().parse::<f64>() {
            Ok(number) if number.is_finite() => Ok(text.trim().to_owned()),
            _ => Ok(format!("\"{}\"", text.replace('"', "\"\""))),
        },
        _ => Err("A rule's value is a number, text or a formula.".to_owned()),
    }
}

fn hex(value: &str, what: &str) -> Result<String, String> {
    super::engine::color(value).ok_or_else(|| format!("{what} is a colour such as #C62828."))
}

impl ConditionalRule {
    /// Why this rule can't be kept, if it can't.
    pub fn check(&self) -> Result<(), String> {
        if address::range(&self.range, super::MAX_ROWS, super::MAX_COLUMNS).is_none() {
            return Err(format!(
                "\"{}\" isn't a range such as B2:B40.",
                self.range.chars().take(40).collect::<String>()
            ));
        }
        for color in [&self.style.color, &self.style.fill].into_iter().flatten() {
            hex(color, "A rule's colour")?;
        }
        match &self.test {
            RuleTest::LessThan { value }
            | RuleTest::GreaterThan { value }
            | RuleTest::AtLeast { value }
            | RuleTest::AtMost { value }
            | RuleTest::Equals { value }
            | RuleTest::NotEquals { value } => {
                checked_value(value)?;
            }
            RuleTest::Between { value, value2 } | RuleTest::NotBetween { value, value2 } => {
                checked_value(value)?;
                checked_value(value2)?;
            }
            RuleTest::TextContains { text }
            | RuleTest::TextDoesNotContain { text }
            | RuleTest::TextBeginsWith { text }
            | RuleTest::TextEndsWith { text } => {
                if text.is_empty() || text.chars().count() > MAX_TEXT {
                    return Err(format!("A rule's text is 1 to {MAX_TEXT} characters."));
                }
            }
            RuleTest::Formula { formula } => {
                if !formula.starts_with('=') || formula.chars().count() > MAX_FORMULA {
                    return Err(format!(
                        "A rule's formula starts with = and is up to {MAX_FORMULA} characters."
                    ));
                }
            }
            RuleTest::Top { rank, .. } | RuleTest::Bottom { rank, .. } => {
                if !(1..=1_000).contains(rank) {
                    return Err("A top or bottom rule ranks 1 to 1,000.".to_owned());
                }
            }
            RuleTest::ColorScale { low, high, mid } => {
                hex(low, "A colour scale's low colour")?;
                hex(high, "A colour scale's high colour")?;
                if let Some(mid) = mid {
                    hex(mid, "A colour scale's middle colour")?;
                }
            }
            RuleTest::Empty {}
            | RuleTest::NotEmpty {}
            | RuleTest::Errors {}
            | RuleTest::AboveAverage {}
            | RuleTest::BelowAverage {}
            | RuleTest::Duplicates {} => {}
        }
        if !matches!(self.test, RuleTest::ColorScale { .. }) && self.style == RuleStyle::default() {
            return Err("Say how the cells look when the rule applies.".to_owned());
        }
        Ok(())
    }

    /// The rule as IronCalc takes it.
    pub(super) fn input(&self) -> Result<CfRuleInput, String> {
        let format = self.dxf();
        let stop_if_true = false;
        let cell_is = |operator, value: &Json, value2: Option<&Json>| {
            Ok::<_, String>(CfRuleInput::CellIs {
                operator,
                formula: operand(value)?,
                formula2: value2.map(operand).transpose()?,
                format: format.clone(),
                stop_if_true,
            })
        };
        let text = |operator, value: &String| CfRuleInput::Text {
            operator,
            value: value.clone(),
            format: format.clone(),
            stop_if_true,
        };
        Ok(match &self.test {
            RuleTest::LessThan { value } => cell_is(ValueOperator::LessThan, value, None)?,
            RuleTest::GreaterThan { value } => cell_is(ValueOperator::GreaterThan, value, None)?,
            RuleTest::AtLeast { value } => cell_is(ValueOperator::GreaterThanOrEqual, value, None)?,
            RuleTest::AtMost { value } => cell_is(ValueOperator::LessThanOrEqual, value, None)?,
            RuleTest::Equals { value } => cell_is(ValueOperator::Equal, value, None)?,
            RuleTest::NotEquals { value } => cell_is(ValueOperator::NotEqual, value, None)?,
            RuleTest::Between { value, value2 } => {
                cell_is(ValueOperator::Between, value, Some(value2))?
            }
            RuleTest::NotBetween { value, value2 } => {
                cell_is(ValueOperator::NotBetween, value, Some(value2))?
            }
            RuleTest::TextContains { text: value } => text(TextOperator::Contains, value),
            RuleTest::TextDoesNotContain { text: value } => {
                text(TextOperator::DoesNotContain, value)
            }
            RuleTest::TextBeginsWith { text: value } => text(TextOperator::BeginsWith, value),
            RuleTest::TextEndsWith { text: value } => text(TextOperator::EndsWith, value),
            RuleTest::Empty {} => CfRuleInput::Blanks {
                format,
                stop_if_true,
            },
            RuleTest::NotEmpty {} => CfRuleInput::NotBlanks {
                format,
                stop_if_true,
            },
            RuleTest::Errors {} => CfRuleInput::Errors {
                format,
                stop_if_true,
            },
            RuleTest::Formula { formula } => CfRuleInput::Formula {
                formula: formula[1..].to_owned(),
                format,
                stop_if_true,
            },
            RuleTest::Top { rank, percent } => CfRuleInput::Top10 {
                rank: *rank,
                percent: *percent,
                format,
                stop_if_true,
            },
            RuleTest::Bottom { rank, percent } => CfRuleInput::Bottom10 {
                rank: *rank,
                percent: *percent,
                format,
                stop_if_true,
            },
            RuleTest::AboveAverage {} => CfRuleInput::AboveAverage {
                format,
                stop_if_true,
            },
            RuleTest::BelowAverage {} => CfRuleInput::BelowAverage {
                format,
                stop_if_true,
            },
            RuleTest::Duplicates {} => CfRuleInput::DuplicateValues {
                format,
                stop_if_true,
            },
            RuleTest::ColorScale { low, high, mid } => {
                let stop = |cfvo, color: &String| ColorScaleThreshold {
                    cfvo,
                    color: Color::Rgb(color.clone()),
                };
                let mut thresholds = vec![stop(Cfvo::Min, low)];
                if let Some(mid) = mid {
                    thresholds.push(stop(Cfvo::Percentile(50.0), mid));
                }
                thresholds.push(stop(Cfvo::Max, high));
                CfRuleInput::ColorScale { thresholds }
            }
        })
    }

    fn dxf(&self) -> Dxf {
        let style = &self.style;
        let font = DxfFont {
            b: style.bold.then_some(true),
            i: style.italic.then_some(true),
            u: style.underline.then_some(true),
            strike: style.strike.then_some(true),
            color: style.color.clone().map(Color::Rgb).unwrap_or(Color::None),
            ..DxfFont::default()
        };
        Dxf {
            font: (font != DxfFont::default()).then_some(font),
            fill: style.fill.clone().map(|fill| Fill {
                color: Color::Rgb(fill),
            }),
            ..Dxf::default()
        }
    }
}

fn checked_value(value: &Json) -> Result<(), String> {
    let text = operand(value)?;
    if text.chars().count() > MAX_FORMULA {
        return Err(format!("A rule's value is up to {MAX_FORMULA} characters."));
    }
    Ok(())
}

/// A tab's rules as kept, each range written out (`b2:b40` → `B2:B40`),
/// or why they can't be: (which rule, why).
pub fn checked(
    mut rules: Vec<ConditionalRule>,
) -> Result<Vec<ConditionalRule>, (Option<usize>, String)> {
    if rules.len() > MAX_RULES_PER_TAB {
        return Err((
            None,
            format!("A tab keeps up to {MAX_RULES_PER_TAB} conditional formats."),
        ));
    }
    for (index, rule) in rules.iter_mut().enumerate() {
        rule.check().map_err(|message| (Some(index), message))?;
        if let Some(range) = address::range(&rule.range, super::MAX_ROWS, super::MAX_COLUMNS) {
            rule.range = range.name();
        }
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_read_and_write_as_flat_objects() {
        let rule: ConditionalRule = serde_json::from_value(json!({
            "range": "B2:B40", "when": "lessThan", "value": 0, "style": {"color": "#C62828"}
        }))
        .expect("rule");
        assert_eq!(rule.test, RuleTest::LessThan { value: json!(0) });
        assert!(rule.check().is_ok());
        assert_eq!(
            serde_json::to_value(&rule).expect("json"),
            json!({"range": "B2:B40", "when": "lessThan", "value": 0, "style": {"color": "#C62828"}})
        );
        let plain: ConditionalRule =
            serde_json::from_value(json!({"range": "A1:A3", "when": "empty"})).expect("rule");
        assert!(plain.check().is_err(), "a rule that changes nothing");
        let bad: ConditionalRule = serde_json::from_value(
            json!({"range": "nowhere", "when": "notEmpty", "style": {"bold": true}}),
        )
        .expect("rule");
        assert!(bad.check().is_err());
        assert_eq!(operand(&json!("Late")).expect("text"), "\"Late\"");
        assert_eq!(operand(&json!("=$B$1")).expect("formula"), "$B$1");
    }
}
