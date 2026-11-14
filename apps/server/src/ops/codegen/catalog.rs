//! `catalog.json`: what each operation is, without its schemas. The web app
//! reads it to know an operation's kind and module; the automation builder
//! lists the writes it may offer from it.

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
