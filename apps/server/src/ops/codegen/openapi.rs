//! The v2 OpenAPI document (ruling R4): one path per operation on the one
//! generated route, its input as the body and its envelope as the answer.
//! Served at `/api/v2/openapi.json` and kept in `docs/api/v2.openapi.json`.
//! v1 (`/api/public/v1`) keeps its own document, frozen.

use serde_json::{Map, Value, json};

use super::super::{Registry, envelope};

pub const VERSION: &str = "2.0.0";

/// `#/$defs/X` becomes `#/components/schemas/X`.
fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference
                    .strip_prefix("#/$defs/")
                    .or_else(|| reference.strip_prefix("#/definitions/"))
            {
                *reference = format!("#/components/schemas/{name}");
            }
            for child in object.values_mut() {
                rewrite_refs(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

fn component(mut schema: Value) -> Value {
    rewrite_refs(&mut schema);
    schema
}

pub fn document(registry: &Registry) -> Result<Value, String> {
    let schemas = super::shared_definitions(registry)?
        .into_iter()
        .map(|(name, schema)| (name, component(schema)))
        .collect::<Map<_, _>>();
    let mut paths = Map::new();
    for entry in registry.entries() {
        let meta = &entry.meta;
        let mut parameters = vec![json!({
            "name": "workspace",
            "in": "path",
            "required": true,
            "description": "The workspace the operation acts in, by id or slug. A target in another workspace is not found.",
            "schema": { "type": "string" },
        })];
        if meta.kind.writes() {
            parameters.push(json!({
                "name": "Idempotency-Key",
                "in": "header",
                "required": false,
                "description": "Your id for this request: a retry with the same id is answered once, with replayed: true.",
                "schema": { "type": "string", "maxLength": 128 },
            }));
        }
        if meta.preview {
            parameters.push(json!({
                "name": "preview",
                "in": "query",
                "required": false,
                "description": "Run it and roll it back.",
                "schema": { "type": "boolean" },
            }));
        }
        let answer = json!({
            "application/json": {
                "schema": component(envelope::schema(super::bare((meta.output_schema)()))),
            },
        });
        let description = match meta.permission {
            Some(permission) => format!(
                "{} Needs \"{}\" where it acts.",
                meta.description,
                permission.info().label
            ),
            None => meta.description.to_owned(),
        };
        paths.insert(
            format!("/api/v2/w/{{workspace}}/ops/{}", meta.name),
            json!({
                "post": {
                    "operationId": meta.name,
                    "summary": meta.title,
                    "description": description,
                    "tags": [meta.module],
                    "x-operation-kind": meta.kind.key(),
                    "parameters": parameters,
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": component(super::bare((meta.input_schema)())),
                            },
                        },
                    },
                    "responses": {
                        "200": { "description": "Done: outcome ok, with the value.", "content": answer.clone() },
                        "default": { "description": "Refused or failed: the outcome says why, and the status follows it.", "content": answer },
                    },
                },
            }),
        );
    }
    Ok(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Agent Workspace API",
            "version": VERSION,
            "description": "Every operation of the app, as the web app and agents call them. POST the operation's input to its path; the answer is an envelope whose outcome sets the status. Authenticate with a bearer token: a personal API token, a connected app's token or an agent's token.",
        },
        "servers": [{ "url": "/" }],
        "security": [{ "bearer": [] }],
        "paths": paths,
        "components": {
            "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer" } },
            "schemas": schemas,
        },
    }))
}
