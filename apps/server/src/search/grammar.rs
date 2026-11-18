//! The search query grammar (C2.25), read the same way here and in the web
//! app (`apps/web/src/memory/searchQuery.ts`); `query_parity.json` holds the
//! cases both must agree on.
//!
//! Free words, "quoted phrases" and -excluded words go to full-text search
//! as they are. Filters narrow it:
//!
//! - `from:@handle` (or `from:me`), `is:mine`: who wrote it.
//! - `in:#channel`, `in:@person` (your DM with them).
//! - `during:2026`, `during:2026-10`, `on:2026-10-05`, `before:`, `after:`
//!   (UTC days; `after:` starts the day after, `before:` ends the day before).
//! - `has:file|link|reaction|pin|task`, `is:thread|dm|unread|saved`.
//! - `kind:message|thread|task|page|decision|record|whiteboard|sheet|file`.
//!
//! A filter with a value it doesn't take is listed in `errors`; a word with
//! a colon that names no filter (a link, a time) stays a word.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ParsedQuery {
    /// The words for full-text search, in `websearch_to_tsquery` syntax.
    pub text: String,
    /// Handles, lowercased without the `@`, or `me`.
    pub from: Vec<String>,
    /// Channel names, lowercased without the `#`.
    pub in_channels: Vec<String>,
    /// Handles whose DM with you to search.
    pub in_people: Vec<String>,
    /// The first day searched, `YYYY-MM-DD`.
    pub after: Option<String>,
    /// The day after the last day searched, `YYYY-MM-DD`.
    pub before: Option<String>,
    pub has: Vec<String>,
    pub is: Vec<String>,
    /// Kind names as search results use them, plural (`messages`).
    pub kinds: Vec<String>,
    /// Filters whose value isn't one they take, as typed.
    pub errors: Vec<String>,
}

impl ParsedQuery {
    /// Whether anything narrows the search besides the words.
    pub fn has_filters(&self) -> bool {
        !(self.from.is_empty()
            && self.in_channels.is_empty()
            && self.in_people.is_empty()
            && self.after.is_none()
            && self.before.is_none()
            && self.has.is_empty()
            && self.is.is_empty()
            && self.kinds.is_empty())
    }
}

const KEYS: [&str; 9] = [
    "from", "in", "during", "on", "before", "after", "has", "is", "kind",
];

/// The query's tokens: runs of non-space, with a double-quoted part
/// (spaces included) kept whole.
pub fn tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in query.chars() {
        if character == '"' {
            quoted = !quoted;
            current.push(character);
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn parse(query: &str) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    let mut words = Vec::new();
    for token in tokens(query) {
        let Some((key, value)) = filter(&token) else {
            words.push(token);
            continue;
        };
        if !apply(&mut parsed, &key, &value) {
            push(&mut parsed.errors, token);
        }
    }
    parsed.text = words.join(" ");
    parsed
}

/// `key:value` when `key` is a filter, lowercased.
fn filter(token: &str) -> Option<(String, String)> {
    let (key, value) = token.split_once(':')?;
    let key = key.to_ascii_lowercase();
    KEYS.contains(&key.as_str())
        .then(|| (key, value.to_lowercase()))
}

fn push(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

fn handle(value: &str) -> Option<String> {
    let value = value.strip_prefix('@').unwrap_or(value);
    let ok = !value.is_empty()
        && value.chars().count() <= 80
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
    ok.then(|| value.to_owned())
}

fn apply(parsed: &mut ParsedQuery, key: &str, value: &str) -> bool {
    match key {
        "from" => match handle(value) {
            Some(who) => push(&mut parsed.from, who),
            None => return false,
        },
        "in" => {
            if let Some(person) = value.strip_prefix('@') {
                match handle(person) {
                    Some(who) => push(&mut parsed.in_people, who),
                    None => return false,
                }
            } else {
                let name = value.strip_prefix('#').unwrap_or(value);
                if name.is_empty() || name.chars().count() > 80 || name.contains('"') {
                    return false;
                }
                push(&mut parsed.in_channels, name.to_owned());
            }
        }
        "during" | "on" => match span(value) {
            Some((start, end)) if key == "during" || value.len() == 10 => {
                narrow_after(parsed, start);
                narrow_before(parsed, end);
            }
            _ => return false,
        },
        "before" => match span(value) {
            Some((start, _)) => narrow_before(parsed, start),
            None => return false,
        },
        "after" => match span(value) {
            Some((_, end)) => narrow_after(parsed, end),
            None => return false,
        },
        "has" => {
            let value = value.strip_suffix('s').unwrap_or(value);
            if !matches!(value, "file" | "link" | "reaction" | "pin" | "task") {
                return false;
            }
            push(&mut parsed.has, value.to_owned());
        }
        "is" => match value {
            "mine" => push(&mut parsed.from, "me".to_owned()),
            "thread" | "threads" => push(&mut parsed.is, "thread".to_owned()),
            "dm" | "dms" => push(&mut parsed.is, "dm".to_owned()),
            "unread" | "saved" => push(&mut parsed.is, value.to_owned()),
            _ => return false,
        },
        "kind" => match kind(value) {
            Some(kind) => push(&mut parsed.kinds, kind.to_owned()),
            None => return false,
        },
        _ => return false,
    }
    true
}

fn kind(value: &str) -> Option<&'static str> {
    Some(match value {
        "message" | "messages" => "messages",
        "thread" | "threads" => "threads",
        "task" | "tasks" => "tasks",
        "page" | "pages" => "pages",
        "decision" | "decisions" => "decisions",
        "record" | "records" => "records",
        "whiteboard" | "whiteboards" | "board" | "boards" => "whiteboards",
        "sheet" | "sheets" => "sheets",
        "file" | "files" => "files",
        _ => return None,
    })
}

fn narrow_after(parsed: &mut ParsedQuery, day: String) {
    if parsed.after.as_ref().is_none_or(|after| day > *after) {
        parsed.after = Some(day);
    }
}

fn narrow_before(parsed: &mut ParsedQuery, day: String) {
    if parsed.before.as_ref().is_none_or(|before| day < *before) {
        parsed.before = Some(day);
    }
}

fn days_in(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn day(year: u32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// A year, month or day as its first day and the day after its last.
fn span(value: &str) -> Option<(String, String)> {
    let parts = value.split('-').collect::<Vec<_>>();
    let number = |part: &str, width: usize| {
        (part.len() == width && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let year = number(parts.first()?, 4).filter(|year| (1970..=9998).contains(year))?;
    let month = match parts.get(1) {
        Some(part) => Some(number(part, 2).filter(|month| (1..=12).contains(month))?),
        None => None,
    };
    let date = match parts.get(2) {
        Some(part) => {
            let month = month?;
            Some(number(part, 2).filter(|date| (1..=days_in(year, month)).contains(date))?)
        }
        None => None,
    };
    if parts.len() > 3 {
        return None;
    }
    Some(match (month, date) {
        (None, _) => (day(year, 1, 1), day(year + 1, 1, 1)),
        (Some(12), None) => (day(year, 12, 1), day(year + 1, 1, 1)),
        (Some(month), None) => (day(year, month, 1), day(year, month + 1, 1)),
        (Some(month), Some(date)) => {
            let next = if date < days_in(year, month) {
                day(year, month, date + 1)
            } else if month < 12 {
                day(year, month + 1, 1)
            } else {
                day(year + 1, 1, 1)
            };
            (day(year, month, date), next)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        query: String,
        parsed: ParsedQuery,
    }

    /// The web app's parser runs the same cases (`searchQuery.test.ts`).
    #[test]
    fn parses_the_parity_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("query_parity.json")).expect("parity cases");
        assert!(cases.len() > 10);
        for case in cases {
            assert_eq!(parse(&case.query), case.parsed, "{}", case.query);
        }
    }
}
