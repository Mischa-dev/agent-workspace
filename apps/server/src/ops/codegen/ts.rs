//! TypeScript for the web app, from the operations' JSON Schemas: one type
//! per input and output, one per shared definition, and the `Operations`
//! map the generic client is typed by.
//!
//! It reads the subset schemars emits here: objects with `properties`,
//! `required` and `additionalProperties`, `$ref` into the definitions,
//! string enums, `const`, `oneOf`/`anyOf` unions (serde's tagged enums and
//! nullable values), arrays and primitives. Any other keyword fails with the
//! operation's name, so a new shape is noticed rather than mistranslated.

use std::fmt::Write as _;

use serde_json::Value;

use super::super::{Registry, error::ERROR_OUTCOMES};

const HEADER: &str = "// Generated from the operation registry by `ops::codegen` in the server.\n\
// Do not edit: change the operation, then run\n\
// OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current\n";

/// Keywords that only describe or bound a value: no change to its type.
const DESCRIPTIVE: &[&str] = &[
    "description",
    "title",
    "format",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "$schema",
    "$defs",
    "definitions",
];

/// Keywords the translation reads.
const READ: &[&str] = &[
    "$ref",
    "oneOf",
    "anyOf",
    "allOf",
    "const",
    "enum",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "nullable",
];

/// `decision_record` -> `DecisionRecord`; `Some_type` -> `SomeType`.
pub fn pascal(name: &str) -> String {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn property_key(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '$'
        });
    if plain {
        name.to_owned()
    } else {
        Value::String(name.to_owned()).to_string()
    }
}

fn union(parts: Vec<String>) -> String {
    let mut unique = Vec::new();
    for part in parts {
        if !unique.contains(&part) {
            unique.push(part);
        }
    }
    match unique.len() {
        0 => "never".to_owned(),
        1 => unique.remove(0),
        _ => unique
            .into_iter()
            .map(|part| {
                if part.contains(" | ") || part.contains(" & ") {
                    format!("({part})")
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

/// The TypeScript type of `schema`. `op` names the operation in errors.
pub fn type_of(schema: &Value, op: &str) -> Result<String, String> {
    let object = match schema {
        Value::Bool(true) => return Ok("unknown".to_owned()),
        Value::Bool(false) => return Ok("never".to_owned()),
        Value::Object(object) => object,
        _ => return Err(format!("{op}: a schema must be an object")),
    };
    if let Some(keyword) = object
        .keys()
        .find(|key| !DESCRIPTIVE.contains(&key.as_str()) && !READ.contains(&key.as_str()))
    {
        return Err(format!(
            "{op}: the schema keyword `{keyword}` is outside what the TypeScript generator reads"
        ));
    }
    let mut parts = Vec::new();
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .or_else(|| reference.strip_prefix("#/definitions/"))
            .ok_or_else(|| format!("{op}: the reference {reference} isn't to a definition"))?;
        parts.push(pascal(name));
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(branches) = object.get(key) {
            let branches = branches
                .as_array()
                .ok_or_else(|| format!("{op}: {key} must be a list"))?;
            let types = branches
                .iter()
                .map(|branch| type_of(branch, op))
                .collect::<Result<Vec<_>, _>>()?;
            parts.push(union(types));
        }
    }
    if let Some(branches) = object.get("allOf") {
        let branches = branches
            .as_array()
            .ok_or_else(|| format!("{op}: allOf must be a list"))?;
        let types = branches
            .iter()
            .map(|branch| type_of(branch, op))
            .collect::<Result<Vec<_>, _>>()?;
        parts.push(types.join(" & "));
    }
    if let Some(value) = object.get("const") {
        parts.push(value.to_string());
    } else if let Some(values) = object.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| format!("{op}: enum must be a list"))?;
        parts.push(union(values.iter().map(Value::to_string).collect()));
    } else if let Some(kinds) = object.get("type") {
        let kinds = match kinds {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => return Err(format!("{op}: type must be a name or a list")),
        };
        let mut types = Vec::new();
        for kind in kinds {
            types.push(match kind {
                "string" => "string".to_owned(),
                "integer" | "number" => "number".to_owned(),
                "boolean" => "boolean".to_owned(),
                "null" => "null".to_owned(),
                "array" => {
                    let items = object
                        .get("items")
                        .map_or(Ok("unknown".to_owned()), |items| type_of(items, op))?;
                    if items.contains(' ') {
                        format!("Array<{items}>")
                    } else {
                        format!("{items}[]")
                    }
                }
                "object" => object_type(object, op)?,
                other => return Err(format!("{op}: the type {other} isn't supported")),
            });
        }
        parts.push(union(types));
    } else if object.contains_key("properties") {
        parts.push(object_type(object, op)?);
    }
    if object.get("nullable").and_then(Value::as_bool) == Some(true) {
        parts = vec![union(vec![
            if parts.is_empty() {
                "unknown".to_owned()
            } else {
                parts.join(" & ")
            },
            "null".to_owned(),
        ])];
    }
    Ok(match parts.len() {
        0 => "unknown".to_owned(),
        1 => parts.remove(0),
        _ => parts
            .into_iter()
            .map(|part| {
                if part.contains(" | ") {
                    format!("({part})")
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" & "),
    })
}

fn object_type(object: &serde_json::Map<String, Value>, op: &str) -> Result<String, String> {
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let properties = object.get("properties").and_then(Value::as_object);
    let extra = match object.get("additionalProperties") {
        None | Some(Value::Bool(true)) => properties.is_none().then(|| "unknown".to_owned()),
        Some(Value::Bool(false)) => None,
        Some(schema) => Some(type_of(schema, op)?),
    };
    let mut fields = Vec::new();
    for (name, schema) in properties.into_iter().flatten() {
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        fields.push(format!(
            "{}{optional}: {}",
            property_key(name),
            type_of(schema, op)?
        ));
    }
    let shape = (!fields.is_empty()).then(|| format!("{{ {} }}", fields.join("; ")));
    Ok(match (shape, extra) {
        (Some(shape), None) => shape,
        (Some(shape), Some(extra)) => format!("{shape} & Record<string, {extra}>"),
        (None, Some(extra)) => format!("Record<string, {extra}>"),
        (None, None) => "Record<string, never>".to_owned(),
    })
}

/// `operations.ts`: types only, so importing it adds nothing to a bundle.
pub fn operations(registry: &Registry) -> Result<String, String> {
    let mut out = String::from(HEADER);
    out.push('\n');
    let outcomes = ERROR_OUTCOMES
        .iter()
        .map(|outcome| format!("'{outcome}'"))
        .collect::<Vec<_>>()
        .join(" | ");
    let _ = writeln!(out, "export type OpErrorOutcome = {outcomes}\n");
    out.push_str(
        "export type OpError = {\n  outcome: OpErrorOutcome\n  message: string\n  retryCategory: string\n  field?: string\n  code?: string\n  module?: string\n  details?: Record<string, unknown>\n}\n\n",
    );
    out.push_str(
        "export type Envelope<T> =\n  | { outcome: 'ok'; value: T; requestId?: string; replayed?: boolean; preview?: boolean }\n  | OpError\n",
    );
    for (name, definition) in super::shared_definitions(registry)? {
        let _ = write!(
            out,
            "\nexport type {} = {}\n",
            pascal(&name),
            type_of(&definition, &name)?
        );
    }
    let mut map = Vec::new();
    for entry in registry.entries() {
        let name = entry.meta.name;
        let type_name = pascal(name);
        let input = type_of(&super::bare((entry.meta.input_schema)()), name)?;
        let output = type_of(&super::bare((entry.meta.output_schema)()), name)?;
        let _ = write!(
            out,
            "\nexport type {type_name}Input = {input}\n\nexport type {type_name}Output = {output}\n"
        );
        map.push(format!(
            "  {name}: {{ input: {type_name}Input; output: {type_name}Output }}\n"
        ));
    }
    if map.is_empty() {
        out.push_str("\nexport type Operations = Record<never, { input: never; output: never }>\n");
    } else {
        let _ = write!(out, "\nexport type Operations = {{\n{}}}\n", map.concat());
    }
    out.push_str("\nexport type OpName = keyof Operations\n");
    Ok(out)
}

/// `client.ts`: one generic call. Its runtime is this function alone.
pub fn client() -> String {
    format!(
        "{HEADER}
import {{ requestJson }} from '../client'
import {{ ApiError }} from '../errors'
import type {{ Envelope, OpName, Operations }} from './operations'

export type CallOptions = {{
  /** The workspace to act in, by id or slug; left out, the target says. */
  workspaceId?: string
  /** A write's request id: a retry with the same one is answered once. */
  requestId?: string
  /** Run it and roll it back (operations that have a preview). */
  preview?: boolean
  signal?: AbortSignal
}}

/** Run an operation. A refusal throws `ApiError` with the outcome as its code. */
export async function callOp<N extends OpName>(
  name: N,
  input: Operations[N]['input'],
  options: CallOptions = {{}},
): Promise<Operations[N]['output']> {{
  const base = options.workspaceId
    ? `/api/v2/w/${{encodeURIComponent(options.workspaceId)}}/ops/${{name}}`
    : `/api/v2/ops/${{name}}`
  const envelope = await requestJson<Envelope<Operations[N]['output']>>(
    options.preview ? `${{base}}?preview=true` : base,
    {{
      method: 'POST',
      json: input,
      idempotencyKey: options.requestId,
      signal: options.signal,
    }},
  )
  if (envelope.outcome !== 'ok') {{
    throw new ApiError({{
      status: 200,
      code: envelope.outcome,
      message: envelope.message,
      details: {{ ...envelope.details, field: envelope.field, reason: envelope.code }},
    }})
  }}
  return envelope.value
}}
"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_subset_translates() {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "note": { "type": ["string", "null"] },
                "kind": { "enum": ["a", "b"] },
                "tags": { "type": "array", "items": { "type": "string" } },
                "extra": { "type": "object", "additionalProperties": { "type": "integer" } },
                "link": { "$ref": "#/$defs/some_link" },
                "shape": { "oneOf": [
                    { "type": "object", "properties": { "kind": { "const": "dot" } }, "required": ["kind"] },
                    { "type": "object", "properties": { "kind": { "const": "line" }, "length": { "type": "number" } }, "required": ["kind", "length"] }
                ] }
            },
            "required": ["id", "kind"]
        });
        let typed = type_of(&schema, "thing_create").expect("in the subset");
        for field in [
            "id: string",
            "note?: string | null",
            "kind: \"a\" | \"b\"",
            "tags?: string[]",
            "extra?: Record<string, number>",
            "link?: SomeLink",
            "shape?: { kind: \"dot\" } | { kind: \"line\"; length: number }",
        ] {
            assert!(typed.contains(field), "{field} in {typed}");
        }
    }

    #[test]
    fn a_schema_outside_the_subset_names_its_operation() {
        let schema = json!({ "type": "object", "patternProperties": { "^x": {} } });
        let error = type_of(&schema, "thing_frobnicate").expect_err("outside the subset");
        assert!(error.starts_with("thing_frobnicate:"), "{error}");
        assert!(error.contains("patternProperties"), "{error}");
    }
}
