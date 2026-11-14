//! Files generated from the operation registry: the web app's types and
//! client (`ts`), the catalogue the web app and the automation builder read
//! (`catalog`), and the v2 OpenAPI document (`openapi`).
//!
//! `generated_files_are_current` regenerates them in memory and fails on any
//! difference, so a changed operation can't ship with stale types. Run it
//! with `OPS_BLESS=1` to write them:
//!
//! ```sh
//! OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current
//! ```

use std::collections::BTreeMap;

use serde_json::Value;

use super::Registry;

pub mod catalog;
pub mod openapi;
pub mod ts;

/// One generated file: its path from the repository root, and its text.
#[derive(Debug)]
pub struct Generated {
    pub path: &'static str,
    pub content: String,
}

/// Every generated file for `registry`. An error names the operation whose
/// schema the generators can't read.
pub fn generate(registry: &Registry) -> Result<Vec<Generated>, String> {
    Ok(vec![
        Generated {
            path: "apps/web/src/api/generated/operations.ts",
            content: ts::operations(registry)?,
        },
        Generated {
            path: "apps/web/src/api/generated/client.ts",
            content: ts::client(),
        },
        Generated {
            path: "apps/web/src/api/generated/catalog.json",
            content: catalog::json(registry),
        },
        Generated {
            path: "docs/api/v2.openapi.json",
            content: pretty(&openapi::document(registry)?),
        },
    ])
}

pub(crate) fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// A schema's definitions (`$defs`, or `definitions` in older dialects).
fn definitions(schema: &Value) -> impl Iterator<Item = (&String, &Value)> {
    ["$defs", "definitions"]
        .into_iter()
        .filter_map(|key| schema.get(key).and_then(Value::as_object))
        .flatten()
}

/// Every operation's definitions, merged into one map: one name means one
/// shape, or the generators would mix two types up.
fn shared_definitions(registry: &Registry) -> Result<BTreeMap<String, Value>, String> {
    let mut shared = BTreeMap::<String, Value>::new();
    for entry in registry.entries() {
        for schema in [(entry.meta.input_schema)(), (entry.meta.output_schema)()] {
            for (name, definition) in definitions(&schema) {
                match shared.get(name) {
                    Some(known) if known != definition => {
                        return Err(format!(
                            "{}: two different types are both called {name}; rename one",
                            entry.meta.name
                        ));
                    }
                    Some(_) => {}
                    None => {
                        shared.insert(name.clone(), definition.clone());
                    }
                }
            }
        }
    }
    Ok(shared)
}

/// A schema without its definitions and dialect marker.
fn bare(mut schema: Value) -> Value {
    if let Some(object) = schema.as_object_mut() {
        for key in ["$defs", "definitions", "$schema"] {
            object.remove(key);
        }
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_files_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bless = std::env::var_os("OPS_BLESS").is_some();
        let files = generate(&crate::ops::registry()).unwrap_or_else(|error| panic!("{error}"));
        let mut stale = Vec::new();
        for file in files {
            let path = root.join(file.path);
            let current = std::fs::read_to_string(&path).unwrap_or_default();
            if current == file.content {
                continue;
            }
            if bless {
                if let Some(folder) = path.parent() {
                    std::fs::create_dir_all(folder).expect("generated folder");
                }
                std::fs::write(&path, &file.content).expect("write generated file");
            } else {
                stale.push(file.path);
            }
        }
        assert!(
            stale.is_empty(),
            "these generated files are out of date: {stale:?}. Regenerate them with \
             OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current"
        );
    }
}
