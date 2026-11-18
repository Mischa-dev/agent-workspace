//! One result shape on every edge.
//!
//! ```json
//! { "outcome": "ok", "value": { ... }, "requestId": "c-41", "replayed": false,
//!   "replaySafe": true }
//! { "outcome": "conflict", "code": "version", "message": "...", "retryCategory": "reread",
//!   "details": { "currentVersion": 8 } }
//! ```
//!
//! `outcome` is the discriminator (as in the older tools' `ToolResult`),
//! names are camelCase, and `message` is always present on errors. Each
//! A write's answer says `replaySafe`: whether a retry with the same request
//! id is answered once (false when it came without one, ruling R52). The
//! operation's output schema is one `oneOf` over the ok branch and every
//! error outcome, so a refusal from the dispatcher always matches it.

use serde::Serialize;
use serde_json::{Value, json};

use super::error::{ERROR_OUTCOMES, OpError};

/// A finished call: `ok` with its value, or an error.
#[derive(Clone, Debug)]
pub enum Envelope<T> {
    Ok {
        value: T,
        request_id: Option<String>,
        replayed: bool,
        /// Writes only: whether the call had a request id to replay by.
        replay_safe: Option<bool>,
        preview: bool,
    },
    Error(OpError),
}

impl<T: Serialize> Envelope<T> {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Ok {
                value,
                request_id,
                replayed,
                replay_safe,
                preview,
            } => {
                let mut body = json!({
                    "outcome": "ok",
                    "value": serde_json::to_value(value).unwrap_or(Value::Null),
                    "replayed": replayed,
                });
                if let Some(request_id) = request_id {
                    body["requestId"] = request_id.as_str().into();
                }
                if let Some(replay_safe) = replay_safe {
                    body["replaySafe"] = (*replay_safe).into();
                }
                if *preview {
                    body["preview"] = true.into();
                }
                body
            }
            Self::Error(error) => error.to_json(),
        }
    }
}

/// The schema of an operation's envelope, given its output's schema. The
/// output's definitions move to the envelope's root, where its `$ref`s
/// point.
pub fn schema(mut output: Value) -> Value {
    let mut definitions = Vec::new();
    if let Some(object) = output.as_object_mut() {
        object.remove("$schema");
        for key in ["$defs", "definitions"] {
            if let Some(found) = object.remove(key) {
                definitions.push((key, found));
            }
        }
    }
    let mut branches = vec![json!({
        "type": "object",
        "properties": {
            "outcome": { "const": "ok" },
            "value": output,
            "requestId": { "type": "string" },
            "replayed": { "type": "boolean" },
            "replaySafe": { "type": "boolean" },
            "preview": { "type": "boolean" },
            "paused": { "const": false },
            "pendingReview": { "const": false },
        },
        "required": ["outcome", "value"],
    })];
    // One branch for every refusal: they share a shape, and a branch per
    // outcome repeated in every operation's schema made the documents
    // several times their size.
    branches.push(json!({
        "type": "object",
        "properties": {
            "outcome": { "enum": ERROR_OUTCOMES },
            "message": { "type": "string" },
            "retryCategory": { "type": "string" },
            "field": { "type": "string" },
            "code": { "type": "string" },
            "module": { "type": "string" },
            "details": { "type": "object" },
        },
        "required": ["outcome", "message", "retryCategory"],
    }));
    // A call set aside is still `ok`, so a client doesn't retry, but not the
    // operation's value: a paused account's write (A2.10) does nothing and
    // says `paused`; a change a team's guard holds for review (A2.27, R37)
    // is kept with a request to its reviewers and says `pendingReview`, and
    // the same call made again once they allow it goes through. One small
    // branch for both, since every operation's tool repeats it.
    branches.push(json!({
        "properties": {
            "outcome": { "const": "ok" },
            "value": {
                "properties": {
                    "status": { "enum": ["paused", "pending_review"] },
                    "requestId": { "type": "string" },
                },
            },
        },
        "required": ["outcome", "value"],
        "anyOf": [
            { "properties": { "paused": { "const": true } }, "required": ["paused"] },
            { "properties": { "pendingReview": { "const": true } }, "required": ["pendingReview"] },
        ],
    }));
    let mut schema = json!({ "type": "object", "oneOf": branches });
    for (key, found) in definitions {
        schema[key] = found;
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_schema_lists_every_outcome() {
        let schema = schema(json!({ "type": "object" }));
        let branches = schema["oneOf"].as_array().expect("branches");
        assert_eq!(branches[0]["properties"]["outcome"]["const"], "ok");
        let refused = branches[1]["properties"]["outcome"]["enum"]
            .as_array()
            .expect("refusals");
        for outcome in ERROR_OUTCOMES {
            assert!(refused.contains(&json!(outcome)), "{outcome}");
        }
        let ok = Envelope::Ok {
            value: json!({"id": 1}),
            request_id: Some("c-1".into()),
            replayed: true,
            replay_safe: Some(true),
            preview: false,
        }
        .to_json();
        assert_eq!(ok["outcome"], "ok");
        assert_eq!(ok["requestId"], "c-1");
        assert_eq!(ok["replayed"], true);
        assert_eq!(ok["replaySafe"], true);
    }
}
