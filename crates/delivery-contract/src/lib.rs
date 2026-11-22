//! The contract for what agents are told (A2.7), in one place an agent's
//! author can test against:
//!
//! - **Fixtures** ([`fixtures`]): an example of every shape the server
//!   sends: the event envelope, a poll page, a catalogue entry, a stream,
//!   and a signed webhook with the secret that signed it.
//!   They live in `sdks/fixtures/delivery/` for SDKs in any language.
//! - **Verification** ([`webhook`]): checking a pushed webhook's Standard
//!   Webhooks signature, written from the published scheme, not from the
//!   server's code.
//! - **The suite** ([`check_server`], and [`webhook::check_webhook`] for a
//!   received push): reads a real server's catalogue, polls, long-polls,
//!   streams and resumes, and checks every event it sees against the
//!   fixtures and for text it must not carry.
//!
//! A shape conforms to a fixture when it has every key the fixture has,
//! each of the same JSON type (a key may be `null` where the fixture is
//! not required to be set, see [`conforms`]); extra keys are allowed, so the
//! server can add fields without breaking anyone.

mod server;
pub mod webhook;

pub use server::check_server;

use std::fmt;

use serde_json::Value;

/// The fixtures, as the files in `sdks/fixtures/delivery/` hold them.
pub mod fixtures {
    pub const ENVELOPE: &str = include_str!("../../../sdks/fixtures/delivery/envelope.json");
    pub const POLL_PAGE: &str = include_str!("../../../sdks/fixtures/delivery/poll-page.json");
    pub const CATALOGUE_ENTRY: &str =
        include_str!("../../../sdks/fixtures/delivery/catalogue-entry.json");
    pub const STREAM: &str = include_str!("../../../sdks/fixtures/delivery/stream.txt");
    pub const WEBHOOK_STANDARD: &str =
        include_str!("../../../sdks/fixtures/delivery/webhook-standard.json");

    /// A fixture parsed; they are checked to parse by this crate's tests.
    pub fn parse(fixture: &str) -> serde_json::Value {
        serde_json::from_str(fixture).unwrap_or(serde_json::Value::Null)
    }
}

/// Envelope keys that are always set, never `null`.
pub const ENVELOPE_REQUIRED: &[&str] = &[
    "type",
    "timestamp",
    "id",
    "cursor",
    "workspace",
    "workspace.id",
    "recipient",
    "recipient.accountId",
    "reason",
    "data",
];

/// Keys whose contents depend on the event's type: only their JSON type is
/// checked.
pub const ENVELOPE_OPEN: &[&str] = &["data"];

/// Keys that would carry what someone wrote. Events carry ids, kinds and
/// field names; readers fetch the text live, with their own access.
pub const TEXT_KEYS: &[&str] = &[
    "text", "body", "content", "note", "markdown", "html", "snippet", "preview", "excerpt",
];

/// Whether `actual` has the shape of `fixture`: every key of the fixture, of
/// the same JSON type. A key may be `null` unless its dotted path is in
/// `required`; a key in `open` is checked only for its type. Arrays are
/// checked element by element against the fixture's first element.
pub fn conforms(
    fixture: &Value,
    actual: &Value,
    required: &[&str],
    open: &[&str],
) -> Result<(), String> {
    conforms_at("", fixture, actual, required, open)
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn conforms_at(
    path: &str,
    fixture: &Value,
    actual: &Value,
    required: &[&str],
    open: &[&str],
) -> Result<(), String> {
    let shown = if path.is_empty() { "the value" } else { path };
    if actual.is_null() {
        return if fixture.is_null() || !required.contains(&path) {
            Ok(())
        } else {
            Err(format!("{shown} is null"))
        };
    }
    if fixture.is_null() {
        return Ok(());
    }
    if type_name(fixture) != type_name(actual) {
        return Err(format!(
            "{shown} is {}, expected {}",
            type_name(actual),
            type_name(fixture)
        ));
    }
    if open.contains(&path) {
        return Ok(());
    }
    match (fixture, actual) {
        (Value::Object(expected), Value::Object(found)) => {
            for (key, value) in expected {
                let at = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match found.get(key) {
                    Some(found) => conforms_at(&at, value, found, required, open)?,
                    None => return Err(format!("{at} is missing")),
                }
            }
            Ok(())
        }
        (Value::Array(expected), Value::Array(found)) => match expected.first() {
            Some(first) => found.iter().enumerate().try_for_each(|(index, item)| {
                conforms_at(&format!("{path}[{index}]"), first, item, required, open)
            }),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Every key in `value`, at any depth, that would carry text (see
/// [`TEXT_KEYS`]), by dotted path.
pub fn text_keys(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_text_keys("", value, &mut found);
    found
}

fn collect_text_keys(path: &str, value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let at = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if TEXT_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    found.push(at.clone());
                }
                collect_text_keys(&at, value, found);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_text_keys(&format!("{path}[{index}]"), item, found);
            }
        }
        _ => {}
    }
}

/// Checks one event envelope: its shape against the fixture, and no text.
pub fn check_envelope(event: &Value) -> Result<(), String> {
    conforms(
        &fixtures::parse(fixtures::ENVELOPE),
        event,
        ENVELOPE_REQUIRED,
        ENVELOPE_OPEN,
    )?;
    let id = event["id"].as_str().unwrap_or_default();
    if !id.starts_with("evt_") {
        return Err(format!("id {id:?} doesn't start with evt_"));
    }
    match text_keys(event).as_slice() {
        [] => Ok(()),
        keys => Err(format!("carries text under {}", keys.join(", "))),
    }
}

/// How one check came out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Pass,
    Fail,
    /// Not checked: nothing was there to check it with.
    Skip,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
    pub detail: String,
}

/// Every check, in the order they ran.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    fn record(&mut self, name: &'static str, passed: bool, detail: impl Into<String>) -> bool {
        self.checks.push(Check {
            name,
            outcome: if passed { Outcome::Pass } else { Outcome::Fail },
            detail: detail.into(),
        });
        passed
    }

    fn result(&mut self, name: &'static str, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => self.record(name, true, ""),
            Err(detail) => self.record(name, false, detail),
        }
    }

    fn skip(&mut self, name: &'static str, detail: impl Into<String>) {
        self.checks.push(Check {
            name,
            outcome: Outcome::Skip,
            detail: detail.into(),
        });
    }

    /// No check failed.
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.outcome != Outcome::Fail)
    }

    /// The check called `name`, if it ran.
    pub fn check(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.name == name)
    }

    /// What failed, by name.
    pub fn failures(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|check| check.outcome == Outcome::Fail)
            .map(|check| check.name)
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for check in &self.checks {
            let mark = match check.outcome {
                Outcome::Pass => "pass",
                Outcome::Fail => "FAIL",
                Outcome::Skip => "skip",
            };
            write!(formatter, "{mark}  {}", check.name)?;
            if !check.detail.is_empty() {
                write!(formatter, ": {}", check.detail)?;
            }
            writeln!(formatter)?;
        }
        let failed = self.failures().len();
        if failed == 0 {
            writeln!(formatter, "\nEvery check passed.")
        } else {
            writeln!(formatter, "\n{failed} check(s) failed.")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_fixture_parses_and_the_envelope_passes_its_own_check() {
        for fixture in [
            fixtures::ENVELOPE,
            fixtures::POLL_PAGE,
            fixtures::CATALOGUE_ENTRY,
            fixtures::WEBHOOK_STANDARD,
        ] {
            assert!(!fixtures::parse(fixture).is_null(), "{fixture}");
        }
        check_envelope(&fixtures::parse(fixtures::ENVELOPE)).expect("envelope");
        let page = fixtures::parse(fixtures::POLL_PAGE);
        check_envelope(&page["events"][0]).expect("page's envelope");
    }

    #[test]
    fn the_stream_fixture_carries_the_envelope_with_its_cursor_as_id() {
        let events = server::sse_events(fixtures::STREAM);
        assert_eq!(events.len(), 1, "{events:?}");
        let (id, kind, data) = &events[0];
        let envelope = fixtures::parse(fixtures::ENVELOPE);
        assert_eq!(id.as_deref(), envelope["cursor"].as_str());
        assert_eq!(kind.as_deref(), envelope["type"].as_str());
        assert_eq!(serde_json::from_str::<Value>(data).expect("data"), envelope);
    }

    #[test]
    fn shapes_must_match_but_may_grow() {
        let fixture = json!({"a": "x", "b": {"c": 1}, "d": [{"e": true}]});
        let grown = json!({"a": "y", "b": {"c": 2, "z": 0}, "d": [{"e": false}], "new": 1});
        assert!(conforms(&fixture, &grown, &[], &[]).is_ok());
        let missing = json!({"a": "y", "b": {}, "d": []});
        assert_eq!(
            conforms(&fixture, &missing, &[], &[]),
            Err("b.c is missing".to_owned())
        );
        let retyped = json!({"a": 1, "b": {"c": 1}, "d": []});
        assert!(conforms(&fixture, &retyped, &[], &[]).is_err());
        let nulled = json!({"a": null, "b": {"c": 1}, "d": []});
        assert!(conforms(&fixture, &nulled, &[], &[]).is_ok());
        assert!(conforms(&fixture, &nulled, &["a"], &[]).is_err());
    }

    #[test]
    fn text_anywhere_is_found() {
        let event = json!({"data": {"thing": {"id": "1"}, "items": [{"note": "hi"}]}});
        assert_eq!(text_keys(&event), vec!["data.items[0].note".to_owned()]);
    }
}
