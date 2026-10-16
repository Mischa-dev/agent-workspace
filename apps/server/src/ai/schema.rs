//! Checks a model's reply against the prompt's JSON Schema.
//!
//! Providers are asked for structured output, but not all of them enforce
//! it, so the gateway checks every reply itself. This covers the part of
//! JSON Schema the prompts use: `type` (one or a list), `properties`,
//! `required`, `additionalProperties: false`, `items`, `enum`, `minimum`,
//! `maximum`, `minLength`, `maxLength`, `minItems` and `maxItems`. A schema
//! keyword outside that list is a mistake in the prompt, and the prompt
//! tests catch it.

use serde_json::Value;

/// Keywords [`validate`] understands. Anything else in a prompt's schema
/// fails the prompt tests.
pub const KNOWN_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "description",
];

/// The first place `value` breaks `schema`, as a JSON pointer and a short
/// reason, or `None` when it fits.
pub fn validate(schema: &Value, value: &Value) -> Option<String> {
    check(schema, value, "")
}

fn check(schema: &Value, value: &Value, path: &str) -> Option<String> {
    let Some(schema) = schema.as_object() else {
        return Some(format!("{path}: the schema isn't an object"));
    };
    if let Some(expected) = schema.get("type") {
        let types: Vec<&str> = match expected {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !types.iter().any(|kind| is_type(value, kind)) {
            return Some(format!("{path}: expected {}", types.join(" or ")));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(value)
    {
        return Some(format!("{path}: not one of the allowed values"));
    }
    match value {
        Value::Object(object) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(Value::Array(required)) = schema.get("required") {
                for name in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(name) {
                        return Some(format!("{path}/{name}: missing"));
                    }
                }
            }
            for (name, item) in object {
                match properties.and_then(|properties| properties.get(name)) {
                    Some(property) => {
                        if let Some(problem) = check(property, item, &format!("{path}/{name}")) {
                            return Some(problem);
                        }
                    }
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return Some(format!("{path}/{name}: not allowed"));
                    }
                    None => {}
                }
            }
        }
        Value::Array(items) => {
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
                && (items.len() as u64) < min
            {
                return Some(format!("{path}: fewer than {min} items"));
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
                && (items.len() as u64) > max
            {
                return Some(format!("{path}: more than {max} items"));
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    if let Some(problem) = check(item_schema, item, &format!("{path}/{index}")) {
                        return Some(problem);
                    }
                }
            }
        }
        Value::String(text) => {
            let length = text.chars().count() as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && length < min
            {
                return Some(format!("{path}: shorter than {min} characters"));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && length > max
            {
                return Some(format!("{path}: longer than {max} characters"));
            }
        }
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or(f64::NAN);
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && (number.is_nan() || number < min)
            {
                return Some(format!("{path}: below {min}"));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && (number.is_nan() || number > max)
            {
                return Some(format!("{path}: above {max}"));
            }
        }
        _ => {}
    }
    None
}

fn is_type(value: &Value, kind: &str) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => false,
    }
}

/// Every keyword in `schema` that [`validate`] doesn't understand, for the
/// prompt tests.
pub fn unknown_keywords(schema: &Value) -> Vec<String> {
    let mut unknown = Vec::new();
    collect_unknown(schema, &mut unknown);
    unknown
}

fn collect_unknown(schema: &Value, unknown: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    for (keyword, value) in object {
        if !KNOWN_KEYWORDS.contains(&keyword.as_str()) {
            unknown.push(keyword.clone());
        }
        match keyword.as_str() {
            "properties" => {
                for property in value.as_object().into_iter().flat_map(|map| map.values()) {
                    collect_unknown(property, unknown);
                }
            }
            "items" => collect_unknown(value, unknown),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn replies_are_checked_against_the_schema() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["items"],
            "properties": {
                "items": {
                    "type": "array",
                    "maxItems": 2,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "confidence", "note"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["blocks", "none"]},
                            "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                            "note": {"type": ["string", "null"], "maxLength": 5}
                        }
                    }
                }
            }
        });
        let good = json!({"items": [{"kind": "blocks", "confidence": 0.5, "note": null}]});
        assert_eq!(validate(&schema, &good), None);
        for bad in [
            json!({}),
            json!({"items": [], "extra": 1}),
            json!({"items": [{"kind": "maybe", "confidence": 0.5, "note": null}]}),
            json!({"items": [{"kind": "none", "confidence": 1.5, "note": null}]}),
            json!({"items": [{"kind": "none", "confidence": 0.5, "note": "too long"}]}),
            json!({"items": [{"kind": "none", "confidence": 0.5}]}),
            json!({"items": [1, 2, 3]}),
        ] {
            assert!(validate(&schema, &bad).is_some(), "{bad}");
        }
        assert!(unknown_keywords(&schema).is_empty());
        assert_eq!(
            unknown_keywords(&json!({"type": "object", "oneOf": []})),
            vec!["oneOf".to_owned()]
        );
    }
}
