//! `catalog.json`: what each operation is, without its schemas. The web app
//! reads it to know an operation's kind and module; the automation builder
//! lists the writes it may offer from it.
//!
//! `writes.json` is the slice a page bundles: each write's name, title and
//! kind, so a button editor can list them without carrying the whole
//! catalogue in its chunk.

use serde_json::{Value, json};

use super::super::{Idempotency, Registry};

pub fn json(registry: &Registry) -> String {
    let operations = registry
        .entries()
        .map(|entry| {
            let meta = &entry.meta;
            json!({
                "name": meta.name,
                "title": meta.title,
                "kind": meta.kind.key(),
                "module": meta.module,
                "permission": meta.permission.map(|permission| permission.key()),
                "proposable": meta.proposable,
                "idempotent": meta.idempotency != Idempotency::None,
                "preview": meta.preview,
                "destructive": meta.destructive,
                "long": meta.long,
                "automation": meta.automation,
                "emits": meta.emits,
            })
        })
        .collect::<Vec<_>>();
    super::pretty(&Value::Array(operations))
}

/// The writes a page may offer (creates, changes and suggestions), with only
/// what a picker shows.
pub fn writes(registry: &Registry) -> String {
    let operations = registry
        .entries()
        .filter(|entry| ["create", "change", "suggest"].contains(&entry.meta.kind.key()))
        .map(|entry| {
            let meta = &entry.meta;
            json!({ "name": meta.name, "title": meta.title, "kind": meta.kind.key() })
        })
        .collect::<Vec<_>>();
    super::pretty(&Value::Array(operations))
}
