//! A formula's values while it runs, and its static types.

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::limits::MAX_TEXT_CHARACTERS;

/// A value while a formula runs. Errors are values: they flow through the
/// rest of the formula and show in the cell.
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Blank,
    Num(f64),
    Text(String),
    Bool(bool),
    Date(NaiveDate),
    /// A moment (a record's created or edited time, NOW()).
    Time(DateTime<Utc>),
    List(Vec<V>),
    Err(String),
}

/// What kind of value an expression gives, as far as can be known before
/// it runs. `Any` is a mix (an IF whose branches differ).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ty {
    Number,
    Text,
    Boolean,
    Date,
    List,
    Any,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "a number",
            Self::Text => "text",
            Self::Boolean => "true or false",
            Self::Date => "a date",
            Self::List => "a list",
            Self::Any => "a value",
        }
    }

    /// Whether a value of this type can be used where `want` is needed.
    pub fn fits(self, want: Ty) -> bool {
        self == want || self == Ty::Any || want == Ty::Any
    }

    /// The type two branches give together.
    pub fn common(self, other: Ty) -> Ty {
        if self == other { self } else { Ty::Any }
    }
}

/// What a formula field's value is, which decides how it filters, sorts
/// and groups.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResultType {
    Number,
    Text,
    Boolean,
    Date,
    List,
}

impl ResultType {
    pub fn of(ty: Ty) -> Self {
        match ty {
            Ty::Number => Self::Number,
            Ty::Boolean => Self::Boolean,
            Ty::Date => Self::Date,
            Ty::List => Self::List,
            Ty::Text | Ty::Any => Self::Text,
        }
    }

    pub fn ty(self) -> Ty {
        match self {
            Self::Number => Ty::Number,
            Self::Text => Ty::Text,
            Self::Boolean => Ty::Boolean,
            Self::Date => Ty::Date,
            Self::List => Ty::List,
        }
    }
}

fn too_long() -> V {
    V::Err(format!(
        "Text over {MAX_TEXT_CHARACTERS} characters. Use LEFT to shorten it."
    ))
}

/// A number as text: whole numbers without a point, others with up to ten
/// decimals and no trailing zeros.
pub fn number_text(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        let text = format!("{number:.10}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

impl V {
    pub fn text(text: String) -> V {
        if text.chars().count() > MAX_TEXT_CHARACTERS {
            too_long()
        } else {
            V::Text(text)
        }
    }

    pub fn is_err(&self) -> bool {
        matches!(self, V::Err(_))
    }

    pub fn is_blank(&self) -> bool {
        match self {
            V::Blank => true,
            V::Text(text) => text.is_empty(),
            V::List(items) => items.is_empty(),
            _ => false,
        }
    }

    /// As a number: blank is 0, true 1, text that reads as a number that
    /// number.
    pub fn to_num(&self) -> Result<f64, V> {
        match self {
            V::Blank => Ok(0.0),
            V::Num(number) => Ok(*number),
            V::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
            V::Text(text) if text.trim().is_empty() => Ok(0.0),
            V::Text(text) => text
                .trim()
                .replace(',', "")
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| V::Err(format!("\"{}\" isn't a number.", short(text)))),
            V::Err(_) => Err(self.clone()),
            V::List(items) if items.len() == 1 => items[0].to_num(),
            V::Date(_) | V::Time(_) => Err(V::Err(
                "A date isn't a number. Use DATEDIFF to count days.".to_owned(),
            )),
            V::List(_) => Err(V::Err(
                "A list isn't a number. Use SUM or COUNT.".to_owned(),
            )),
        }
    }

    /// As text: numbers plainly, dates as YYYY-MM-DD, lists joined with ", ".
    pub fn to_text(&self) -> Result<String, V> {
        Ok(match self {
            V::Blank => String::new(),
            V::Num(number) => number_text(*number),
            V::Text(text) => text.clone(),
            V::Bool(value) => if *value { "true" } else { "false" }.to_owned(),
            V::Date(date) => date.to_string(),
            V::Time(time) => time.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            V::List(items) => {
                let mut parts = Vec::with_capacity(items.len());
                for item in items {
                    parts.push(item.to_text()?);
                }
                parts.join(", ")
            }
            V::Err(_) => return Err(self.clone()),
        })
    }

    /// As true or false: blank is false, a number is true unless it's 0.
    pub fn to_bool(&self) -> Result<bool, V> {
        match self {
            V::Blank => Ok(false),
            V::Bool(value) => Ok(*value),
            V::Num(number) => Ok(*number != 0.0),
            V::Text(text) => match text.trim().to_ascii_lowercase().as_str() {
                "" | "false" | "no" => Ok(false),
                "true" | "yes" => Ok(true),
                _ => Err(V::Err(format!("\"{}\" isn't true or false.", short(text)))),
            },
            V::List(items) => Ok(!items.is_empty()),
            V::Date(_) | V::Time(_) => Ok(true),
            V::Err(_) => Err(self.clone()),
        }
    }

    /// As a day: a date, a moment's day (UTC), or text as YYYY-MM-DD.
    pub fn to_date(&self) -> Result<Option<NaiveDate>, V> {
        match self {
            V::Blank => Ok(None),
            V::Date(date) => Ok(Some(*date)),
            V::Time(time) => Ok(Some(time.date_naive())),
            V::Text(text) if text.trim().is_empty() => Ok(None),
            V::Text(text) => parse_date(text)
                .map(Some)
                .ok_or_else(|| V::Err(format!("\"{}\" isn't a date as YYYY-MM-DD.", short(text)))),
            V::Err(_) => Err(self.clone()),
            _ => Err(V::Err("That isn't a date.".to_owned())),
        }
    }

    /// The items of a list; anything else is a list of itself (blank, none).
    pub fn items(&self) -> Vec<V> {
        match self {
            V::List(items) => items.clone(),
            V::Blank => Vec::new(),
            other => vec![other.clone()],
        }
    }
}

pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            DateTime::parse_from_rfc3339(text)
                .ok()
                .map(|time| time.with_timezone(&Utc).date_naive())
        })
        .or_else(|| {
            text.get(..10)
                .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        })
}

/// Text quoted in a message: its first 40 characters.
pub fn short(text: &str) -> String {
    let mut cut = text.chars().take(40).collect::<String>();
    if text.chars().count() > 40 {
        cut.push('…');
    }
    cut
}

/// Joins two texts, refusing to grow past the cap.
pub fn join(left: String, right: &str) -> V {
    if left.chars().count() + right.chars().count() > MAX_TEXT_CHARACTERS {
        return too_long();
    }
    let mut joined = left;
    joined.push_str(right);
    V::Text(joined)
}
