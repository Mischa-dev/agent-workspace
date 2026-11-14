//! One result shape on every edge.
//!
//! ```json
//! { "outcome": "ok", "value": { ... }, "requestId": "c-41", "replayed": false }
//! { "outcome": "conflict", "code": "version", "message": "...", "retryCategory": "reread",
//!   "details": { "currentVersion": 8 } }
//! ```
//!
//! `outcome` is the discriminator (as in the older tools' `ToolResult`),
//! names are camelCase, and `message` is always present on errors. Each
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
            "preview": { "type": "boolean" },
        },
        "required": ["outcome", "value"],
    })];
    for outcome in ERROR_OUTCOMES {
        branches.push(json!({
            "type": "object",
            "properties": {
                "outcome": { "const": outcome },
                "message": { "type": "string" },
                "retryCategory": { "type": "string" },
                "field": { "type": "string" },
                "code": { "type": "string" },
                "module": { "type": "string" },
                "details": { "type": "object" },
            },
            "required": ["outcome", "message", "retryCategory"],
        }));
    }
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
        let outcomes = schema["oneOf"]
            .as_array()
            .expect("branches")
            .iter()
            .map(|branch| branch["properties"]["outcome"]["const"].clone())
            .collect::<Vec<_>>();
        assert_eq!(outcomes[0], "ok");
        for outcome in ERROR_OUTCOMES {
            assert!(outcomes.contains(&json!(outcome)), "{outcome}");
        }
        let ok = Envelope::Ok {
            value: json!({"id": 1}),
            request_id: Some("c-1".into()),
            replayed: true,
            preview: false,
        }
        .to_json();
        assert_eq!(ok["outcome"], "ok");
        assert_eq!(ok["requestId"], "c-1");
        assert_eq!(ok["replayed"], true);
    }
}
