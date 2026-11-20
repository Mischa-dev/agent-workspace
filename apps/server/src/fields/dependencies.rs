//! What worked-out fields read, across a space's collections (T1.18): a
//! formula reads the fields it names; a rollup reads its relation and, on
//! the related records, the field it rolls up. Read from the fields'
//! settings, so it can't drift from them.
//!
//! A change that would make a field read itself, however many records and
//! relations away, is refused with the path; so is a chain through more
//! than `MAX_RELATION_DEPTH` relations.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{FieldConfig, FieldType, formula, rollup::RollupConfig};
use crate::error::AppError;

/// Relations a chain of rollups may cross.
pub const MAX_RELATION_DEPTH: usize = 5;

struct Node {
    name: String,
    collection: String,
    field_type: FieldType,
    reads: Vec<Uuid>,
}

/// The fields `config` of type `field_type` reads.
pub fn reads(field_type: FieldType, config: &FieldConfig) -> Vec<Uuid> {
    match field_type {
        FieldType::Formula => config
            .formula
            .as_ref()
            .map(formula::formula_reads)
            .unwrap_or_default(),
        FieldType::Rollup => config
            .rollup
            .as_ref()
            .map(
                |RollupConfig {
                     relation, target, ..
                 }| { std::iter::once(*relation).chain(*target).collect() },
            )
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Refuses `own` (called `own_name`, of `own_type`) reading `own_reads` if
/// that makes a loop, or a chain through too many relations; the message
/// names the path.
pub async fn check(
    connection: &mut PgConnection,
    project_id: Uuid,
    own: Uuid,
    own_name: &str,
    own_type: FieldType,
    own_reads: Vec<Uuid>,
) -> Result<Result<(), String>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, Value, String)>(
        "SELECT field.id, field.name, field.field_type, field.config, collection.name \
         FROM field_definitions field JOIN collections collection ON collection.id = field.collection_id \
         WHERE field.project_id = $1 AND field.archived_at IS NULL \
           AND field.field_type IN ('formula', 'rollup')",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut nodes = HashMap::new();
    for (id, name, field_type, config, collection) in rows {
        let Some(field_type) = FieldType::parse(&field_type) else {
            continue;
        };
        let config = serde_json::from_value::<FieldConfig>(config).unwrap_or_default();
        nodes.insert(
            id,
            Node {
                name,
                collection,
                field_type,
                reads: reads(field_type, &config),
            },
        );
    }
    let own_collection = nodes
        .get(&own)
        .map(|node| node.collection.clone())
        .unwrap_or_default();
    nodes.insert(
        own,
        Node {
            name: own_name.to_owned(),
            collection: own_collection,
            field_type: own_type,
            reads: own_reads,
        },
    );
    let name = |id: Uuid| {
        nodes.get(&id).map_or_else(
            || "a field".to_owned(),
            |node| {
                if node.collection.is_empty() {
                    format!("\"{}\"", node.name)
                } else {
                    format!("\"{}\" in {}", node.name, node.collection)
                }
            },
        )
    };
    let rollups = |path: &[Uuid]| {
        path.iter()
            .filter(|id| {
                nodes
                    .get(id)
                    .is_some_and(|node| node.field_type == FieldType::Rollup)
            })
            .count()
    };
    // Depth first from the field, keeping the path; fields with no node
    // (stored values) read nothing.
    let mut stack = vec![vec![own]];
    let mut steps = 0;
    while let Some(path) = stack.pop() {
        steps += 1;
        if steps > 10_000 {
            return Ok(Err(
                "That makes too long a chain of worked-out fields. Simplify it.".to_owned(),
            ));
        }
        let last = *path.last().unwrap_or(&own);
        if rollups(&path) > MAX_RELATION_DEPTH {
            return Ok(Err(format!(
                "That reads through more than {MAX_RELATION_DEPTH} relations: {}.",
                path.iter()
                    .map(|id| name(*id))
                    .collect::<Vec<_>>()
                    .join(" reads ")
            )));
        }
        let Some(node) = nodes.get(&last) else {
            continue;
        };
        for next in &node.reads {
            if *next == own {
                let mut names = path.iter().map(|id| name(*id)).collect::<Vec<_>>();
                names.push(name(own));
                return Ok(Err(format!(
                    "That would make a field read itself: {}.",
                    names.join(" reads ")
                )));
            }
            if path.contains(next) {
                continue;
            }
            let mut longer = path.clone();
            longer.push(*next);
            stack.push(longer);
        }
    }
    Ok(Ok(()))
}
