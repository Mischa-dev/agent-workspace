//! A project's own workflow (docs/plans/roadmap.md, T4): owners and admins
//! add, rename, reorder and retire statuses and types, choose the defaults,
//! and say which status changes are allowed.
//!
//! The web settings page sends the whole workflow at once and MCP's
//! workflow_update turns its operations into the same document, so both go
//! through `replace_workflow`. Statuses keep their semantic category
//! (backlog .. canceled), which the rest of the app reads to know what is
//! open, in review or finished. A retired status or type stays in the table,
//! disabled, so history still names it; its open tasks move to a replacement
//! with an activity entry each.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    work_item_commands::{CommandActor, enqueue_update, record_audit_for},
};

pub const MAX_STATUSES: usize = 30;
pub const MAX_TYPES: usize = 30;
pub const MAX_NAME_CHARACTERS: usize = 80;
pub const SEMANTIC_CATEGORIES: [&str; 7] = [
    "backlog",
    "unstarted",
    "started",
    "review",
    "verification",
    "completed",
    "canceled",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusDraft {
    /// An existing status (active or retired). Leave it out for a new one.
    pub id: Option<Uuid>,
    /// A name for a new status that transitions and replacements can use
    /// before it has an ID.
    pub key: Option<String>,
    pub name: String,
    pub semantic_category: String,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeDraft {
    pub id: Option<Uuid>,
    pub key: Option<String>,
    pub name: String,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionDraft {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TransitionsDraft {
    /// "all": any status can move to any other.
    All(AllTransitions),
    List(Vec<TransitionDraft>),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AllTransitions {
    All,
}

/// The whole workflow as it should be after the change.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowDocument {
    pub expected_version: i64,
    pub estimate_unit: Option<String>,
    /// In board order, left to right.
    pub statuses: Vec<StatusDraft>,
    pub types: Vec<TypeDraft>,
    pub transitions: TransitionsDraft,
    /// Where the open tasks of a retired status or type go: old ID to an ID
    /// or new key in this document.
    #[serde(default)]
    pub replacements: HashMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowRefusal {
    VersionConflict {
        current_version: i64,
    },
    Invalid {
        field: &'static str,
        message: String,
    },
}

fn invalid(field: &'static str, message: impl Into<String>) -> WorkflowRefusal {
    WorkflowRefusal::Invalid {
        field,
        message: message.into(),
    }
}

#[derive(Clone, Debug, FromRow)]
struct DefinitionRow {
    id: Uuid,
    name: String,
    enabled: bool,
    open_tasks: i64,
}

/// The project's workflow version and estimate unit, with defaults when it
/// has no settings row yet.
pub async fn task_settings(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<(i64, String), AppError> {
    Ok(sqlx::query_as::<_, (i64, String)>(
        r#"
        SELECT COALESCE(settings.workflow_version, 1), COALESCE(settings.estimate_unit, 'points')
        FROM projects project
        LEFT JOIN project_task_settings settings ON settings.project_id = project.id
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or((1, "points".to_owned())))
}

/// The workflow as a document a caller can edit and send back.
pub async fn current_document(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<WorkflowDocument, AppError> {
    let (version, estimate_unit) = task_settings(connection, project_id).await?;
    let statuses = sqlx::query_as::<_, (Uuid, String, String, bool)>(
        "SELECT id, name, semantic_category, is_default FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let types = sqlx::query_as::<_, (Uuid, String, bool)>(
        "SELECT id, name, is_default FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let enabled = statuses.iter().map(|row| row.0).collect::<HashSet<_>>();
    let transitions = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT from_status_id, to_status_id FROM work_item_status_transitions WHERE project_id = $1 ORDER BY from_status_id, to_status_id",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .filter(|(from, to)| enabled.contains(from) && enabled.contains(to))
    .map(|(from, to)| TransitionDraft {
        from: from.to_string(),
        to: to.to_string(),
    })
    .collect();
    Ok(WorkflowDocument {
        expected_version: version,
        estimate_unit: Some(estimate_unit),
        statuses: statuses
            .into_iter()
            .map(|(id, name, semantic_category, is_default)| StatusDraft {
                id: Some(id),
                key: None,
                name,
                semantic_category,
                is_default,
            })
            .collect(),
        types: types
            .into_iter()
            .map(|(id, name, is_default)| TypeDraft {
                id: Some(id),
                key: None,
                name,
                is_default,
            })
            .collect(),
        transitions: TransitionsDraft::List(transitions),
        replacements: HashMap::new(),
    })
}

fn normalized_name(value: &str, field: &'static str) -> Result<String, WorkflowRefusal> {
    let name = value.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(invalid(
            field,
            format!("Use a name of 1 to {MAX_NAME_CHARACTERS} characters, on one line."),
        ));
    }
    Ok(name.to_owned())
}

/// One status or type of the document, resolved: its ID (a fresh one for a
/// new entry) and whether it already exists.
#[derive(Clone, Debug)]
struct Resolved {
    id: Uuid,
    existing: bool,
    name: String,
    category: Option<String>,
    is_default: bool,
}

/// Checks the document against itself and the project's current rows and
/// gives every entry an ID. Pure, so the rules are unit-tested.
fn resolve_entries(
    entries: &[(Option<Uuid>, Option<String>, String, Option<String>, bool)],
    existing: &HashMap<Uuid, DefinitionRow>,
    keys: &mut HashMap<String, Uuid>,
    field: &'static str,
    noun: &str,
    max: usize,
) -> Result<Vec<Resolved>, WorkflowRefusal> {
    if entries.is_empty() {
        return Err(invalid(field, format!("Keep at least one {noun}.")));
    }
    if entries.len() > max {
        return Err(invalid(
            field,
            format!("A project can have at most {max} {noun}es."),
        ));
    }
    let mut names = HashSet::new();
    let mut seen = HashSet::new();
    let mut resolved = Vec::with_capacity(entries.len());
    for (id, key, name, category, is_default) in entries {
        let name = normalized_name(name, field)?;
        if !names.insert(name.to_lowercase()) {
            return Err(invalid(field, format!("Two {noun}es are called {name}.")));
        }
        if let Some(category) = category
            && !SEMANTIC_CATEGORIES.contains(&category.as_str())
        {
            return Err(invalid(
                field,
                format!(
                    "{name} needs a category: one of {}.",
                    SEMANTIC_CATEGORIES.join(", ")
                ),
            ));
        }
        let id = match (id, key) {
            (Some(id), _) => {
                if !existing.contains_key(id) {
                    return Err(invalid(
                        field,
                        format!("{name} isn't a {noun} of this project."),
                    ));
                }
                *id
            }
            (None, Some(key)) => {
                let key = key.trim();
                if key.is_empty() || key.len() > 64 || keys.contains_key(key) {
                    return Err(invalid(field, format!("Give {name} a key of its own.")));
                }
                let id = Uuid::now_v7();
                keys.insert(key.to_owned(), id);
                id
            }
            (None, None) => Uuid::now_v7(),
        };
        if !seen.insert(id) {
            return Err(invalid(field, format!("{name} is listed twice.")));
        }
        resolved.push(Resolved {
            id,
            existing: existing.contains_key(&id),
            name,
            category: category.clone(),
            is_default: *is_default,
        });
    }
    let defaults = resolved.iter().filter(|entry| entry.is_default).count();
    if defaults > 1 {
        return Err(invalid(field, format!("Choose one default {noun}.")));
    }
    if defaults == 0 {
        resolved[0].is_default = true;
    }
    Ok(resolved)
}

fn reference(
    value: &str,
    keys: &HashMap<String, Uuid>,
    names: &HashMap<String, Uuid>,
) -> Option<Uuid> {
    let value = value.trim();
    keys.get(value)
        .copied()
        .or_else(|| value.parse::<Uuid>().ok())
        .or_else(|| names.get(&value.to_lowercase()).copied())
}

async fn definitions(
    connection: &mut PgConnection,
    table: &str,
    column: &str,
    project_id: Uuid,
) -> Result<HashMap<Uuid, DefinitionRow>, AppError> {
    let query = format!(
        r#"
        SELECT definition.id, definition.name, definition.enabled,
               (SELECT count(*) FROM work_items item
                 WHERE item.project_id = definition.project_id
                   AND item.{column} = definition.id AND item.archived_at IS NULL) AS open_tasks
        FROM {table} definition
        WHERE definition.project_id = $1
        FOR UPDATE OF definition
        "#
    );
    Ok(sqlx::query_as::<_, DefinitionRow>(&query)
        .bind(project_id)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect())
}

/// What `replace_workflow` changed, for the audit row and the reply.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowChange {
    pub version: i64,
    pub statuses_added: usize,
    pub statuses_retired: usize,
    pub types_added: usize,
    pub types_retired: usize,
    pub tasks_moved: usize,
}

/// Makes the project's workflow match `document`. The caller has locked the
/// project (`lock_membership`) and checked the actor may manage it.
pub async fn replace_workflow(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    document: &WorkflowDocument,
) -> Result<Result<WorkflowChange, WorkflowRefusal>, AppError> {
    let project_id = actor.project_id;
    let (version, current_unit) = task_settings(transaction, project_id).await?;
    if document.expected_version != version {
        return Ok(Err(WorkflowRefusal::VersionConflict {
            current_version: version,
        }));
    }
    let estimate_unit = match document.estimate_unit.as_deref() {
        None => current_unit,
        Some(unit @ ("points" | "hours")) => unit.to_owned(),
        Some(_) => return Ok(Err(invalid("estimateUnit", "Use points or hours."))),
    };
    let existing_statuses =
        definitions(transaction, "work_item_statuses", "status_id", project_id).await?;
    let existing_types = definitions(transaction, "work_item_types", "type_id", project_id).await?;

    let mut keys = HashMap::new();
    let statuses = match resolve_entries(
        &document
            .statuses
            .iter()
            .map(|status| {
                (
                    status.id,
                    status.key.clone(),
                    status.name.clone(),
                    Some(status.semantic_category.clone()),
                    status.is_default,
                )
            })
            .collect::<Vec<_>>(),
        &existing_statuses,
        &mut keys,
        "statuses",
        "status",
        MAX_STATUSES,
    ) {
        Ok(statuses) => statuses,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let mut type_keys = HashMap::new();
    let types = match resolve_entries(
        &document
            .types
            .iter()
            .map(|kind| {
                (
                    kind.id,
                    kind.key.clone(),
                    kind.name.clone(),
                    None,
                    kind.is_default,
                )
            })
            .collect::<Vec<_>>(),
        &existing_types,
        &mut type_keys,
        "types",
        "type",
        MAX_TYPES,
    ) {
        Ok(types) => types,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let status_names = statuses
        .iter()
        .map(|status| (status.name.to_lowercase(), status.id))
        .collect::<HashMap<_, _>>();
    let type_names = types
        .iter()
        .map(|kind| (kind.name.to_lowercase(), kind.id))
        .collect::<HashMap<_, _>>();
    let kept_statuses = statuses
        .iter()
        .map(|status| status.id)
        .collect::<HashSet<_>>();
    let kept_types = types.iter().map(|kind| kind.id).collect::<HashSet<_>>();

    // Where each retired status's and type's open tasks go.
    let mut status_moves = Vec::new();
    for row in existing_statuses.values().filter(|row| row.enabled) {
        if kept_statuses.contains(&row.id) || row.open_tasks == 0 {
            continue;
        }
        let target = document
            .replacements
            .get(&row.id.to_string())
            .and_then(|value| reference(value, &keys, &status_names))
            .filter(|target| kept_statuses.contains(target));
        let Some(target) = target else {
            return Ok(Err(invalid(
                "replacements",
                format!(
                    "Say where the {} open {} in {} should go before retiring it.",
                    row.open_tasks,
                    if row.open_tasks == 1 { "task" } else { "tasks" },
                    row.name
                ),
            )));
        };
        status_moves.push((row.id, target));
    }
    let mut type_moves = Vec::new();
    for row in existing_types.values().filter(|row| row.enabled) {
        if kept_types.contains(&row.id) || row.open_tasks == 0 {
            continue;
        }
        let target = document
            .replacements
            .get(&row.id.to_string())
            .and_then(|value| reference(value, &type_keys, &type_names))
            .filter(|target| kept_types.contains(target));
        let Some(target) = target else {
            return Ok(Err(invalid(
                "replacements",
                format!(
                    "Say which type the {} open {} of type {} should become before retiring it.",
                    row.open_tasks,
                    if row.open_tasks == 1 { "task" } else { "tasks" },
                    row.name
                ),
            )));
        };
        type_moves.push((row.id, target));
    }
    let transitions: Vec<(Uuid, Uuid)> = match &document.transitions {
        TransitionsDraft::All(_) => statuses
            .iter()
            .flat_map(|from| {
                statuses
                    .iter()
                    .filter(move |to| to.id != from.id)
                    .map(move |to| (from.id, to.id))
            })
            .collect(),
        TransitionsDraft::List(list) => {
            let mut pairs = Vec::with_capacity(list.len());
            let mut seen = HashSet::new();
            for transition in list {
                let from = reference(&transition.from, &keys, &status_names)
                    .filter(|id| kept_statuses.contains(id));
                let to = reference(&transition.to, &keys, &status_names)
                    .filter(|id| kept_statuses.contains(id));
                let (Some(from), Some(to)) = (from, to) else {
                    return Ok(Err(invalid(
                        "transitions",
                        format!(
                            "{} → {} names a status that isn't in the workflow.",
                            transition.from, transition.to
                        ),
                    )));
                };
                if from != to && seen.insert((from, to)) {
                    pairs.push((from, to));
                }
            }
            pairs
        }
    };

    // Clear names and defaults first, so renames and swaps never collide
    // with each other on the unique indexes.
    for table in ["work_item_statuses", "work_item_types"] {
        sqlx::query(&format!(
            "UPDATE {table} SET name = id::text, is_default = false, updated_at = now() WHERE project_id = $1 AND enabled"
        ))
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    }
    for (position, status) in statuses.iter().enumerate() {
        let position = i32::try_from(position + 1).map_err(|_| AppError::Internal("position"))?;
        let category = status.category.as_deref().unwrap_or("backlog");
        if status.existing {
            sqlx::query(
                "UPDATE work_item_statuses SET name = $3, semantic_category = $4, position = $5, enabled = true, updated_at = now() WHERE project_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(status.id)
            .bind(&status.name)
            .bind(category)
            .bind(position)
            .execute(&mut **transaction)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO work_item_statuses (id, project_id, name, semantic_category, position) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(status.id)
            .bind(project_id)
            .bind(&status.name)
            .bind(category)
            .bind(position)
            .execute(&mut **transaction)
            .await?;
        }
    }
    for (position, kind) in types.iter().enumerate() {
        let position = i32::try_from(position + 1).map_err(|_| AppError::Internal("position"))?;
        if kind.existing {
            sqlx::query(
                "UPDATE work_item_types SET name = $3, position = $4, enabled = true, updated_at = now() WHERE project_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(kind.id)
            .bind(&kind.name)
            .bind(position)
            .execute(&mut **transaction)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO work_item_types (id, project_id, name, position) VALUES ($1, $2, $3, $4)",
            )
            .bind(kind.id)
            .bind(project_id)
            .bind(&kind.name)
            .bind(position)
            .execute(&mut **transaction)
            .await?;
        }
    }
    let mut tasks_moved = 0;
    for (from, to) in &status_moves {
        tasks_moved += move_tasks(transaction, actor, "status_id", *from, *to).await?;
    }
    for (from, to) in &type_moves {
        tasks_moved += move_tasks(transaction, actor, "type_id", *from, *to).await?;
    }
    let mut statuses_retired = 0;
    for row in existing_statuses.values() {
        if row.enabled && !kept_statuses.contains(&row.id) {
            statuses_retired += 1;
            // A retired status gets its name back (unique among active ones
            // only), so history still reads it.
            sqlx::query(
                "UPDATE work_item_statuses SET enabled = false, name = $3, updated_at = now() WHERE project_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(row.id)
            .bind(&row.name)
            .execute(&mut **transaction)
            .await?;
        }
    }
    let mut types_retired = 0;
    for row in existing_types.values() {
        if row.enabled && !kept_types.contains(&row.id) {
            types_retired += 1;
            sqlx::query(
                "UPDATE work_item_types SET enabled = false, name = $3, updated_at = now() WHERE project_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(row.id)
            .bind(&row.name)
            .execute(&mut **transaction)
            .await?;
        }
    }
    if let Some(status) = statuses.iter().find(|status| status.is_default) {
        sqlx::query(
            "UPDATE work_item_statuses SET is_default = true WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(status.id)
        .execute(&mut **transaction)
        .await?;
    }
    if let Some(kind) = types.iter().find(|kind| kind.is_default) {
        sqlx::query(
            "UPDATE work_item_types SET is_default = true WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(kind.id)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query("DELETE FROM work_item_status_transitions WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    if !transitions.is_empty() {
        let (from, to): (Vec<Uuid>, Vec<Uuid>) = transitions.iter().copied().unzip();
        sqlx::query(
            r#"
            INSERT INTO work_item_status_transitions (project_id, from_status_id, to_status_id)
            SELECT $1, pair.from_id, pair.to_id FROM unnest($2::uuid[], $3::uuid[]) AS pair(from_id, to_id)
            "#,
        )
        .bind(project_id)
        .bind(&from)
        .bind(&to)
        .execute(&mut **transaction)
        .await?;
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO project_task_settings (project_id, estimate_unit, workflow_version)
        VALUES ($1, $2, 2)
        ON CONFLICT (project_id) DO UPDATE
        SET estimate_unit = EXCLUDED.estimate_unit,
            workflow_version = project_task_settings.workflow_version + 1,
            updated_at = now()
        RETURNING workflow_version
        "#,
    )
    .bind(project_id)
    .bind(&estimate_unit)
    .fetch_one(&mut **transaction)
    .await?;
    let change = WorkflowChange {
        version,
        statuses_added: statuses.iter().filter(|status| !status.existing).count(),
        statuses_retired,
        types_added: types.iter().filter(|kind| !kind.existing).count(),
        types_retired,
        tasks_moved,
    };
    record_audit_for(
        transaction,
        actor,
        "project",
        "work_item.workflow_updated",
        project_id,
        serde_json::to_value(&change).map_err(|_| AppError::Internal("workflow audit"))?,
    )
    .await?;
    Ok(Ok(change))
}

/// Moves every open task from one status (or type) to another, with an
/// activity entry and a live update each.
async fn move_tasks(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    column: &'static str,
    from: Uuid,
    to: Uuid,
) -> Result<usize, AppError> {
    let (field, summary) = match column {
        "status_id" => ("status", "Status retired; task moved"),
        _ => ("type", "Type retired; task changed type"),
    };
    let payload = if column == "status_id" {
        serde_json::json!({
            "changedFields": [field],
            "previousStatusId": from,
            "statusId": to,
        })
    } else {
        serde_json::json!({
            "changedFields": [field],
            "changes": {"type": {"from": from, "to": to}},
        })
    };
    let moved = sqlx::query_as::<_, (Uuid, i64)>(&format!(
        r#"
        WITH moved AS (
            UPDATE work_items
            SET {column} = $3, version = version + 1, updated_at = now(),
                next_timeline_sequence = next_timeline_sequence + 1
            WHERE project_id = $1 AND {column} = $2 AND archived_at IS NULL
            RETURNING id, version, next_timeline_sequence - 1 AS sequence
        ), activity AS (
            INSERT INTO work_item_activity (id, project_id, work_item_id, actor_id, kind, summary, payload)
            SELECT gen_random_uuid(), $1, moved.id, $4, 'work_item_updated', $5, $6
            FROM moved
            RETURNING id, work_item_id
        ), timeline AS (
            INSERT INTO work_item_timeline_entries (project_id, work_item_id, sequence, kind, activity_id)
            SELECT $1, moved.id, moved.sequence, 'activity', activity.id
            FROM moved JOIN activity ON activity.work_item_id = moved.id
        )
        SELECT id, version FROM moved
        "#
    ))
    .bind(actor.project_id)
    .bind(from)
    .bind(to)
    .bind(actor.actor_id)
    .bind(summary)
    .bind(&payload)
    .fetch_all(&mut **transaction)
    .await?;
    for (work_item_id, version) in &moved {
        enqueue_update(transaction, actor.project_id, *work_item_id, *version).await?;
    }
    Ok(moved.len())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::Uuid;

    use super::{DefinitionRow, WorkflowRefusal, resolve_entries};

    fn existing(ids: &[Uuid]) -> HashMap<Uuid, DefinitionRow> {
        ids.iter()
            .map(|id| {
                (
                    *id,
                    DefinitionRow {
                        id: *id,
                        name: "Old".to_owned(),
                        enabled: true,
                        open_tasks: 0,
                    },
                )
            })
            .collect()
    }

    type Entry = (Option<Uuid>, Option<String>, String, Option<String>, bool);

    fn entry(id: Option<Uuid>, key: Option<&str>, name: &str, default: bool) -> Entry {
        (
            id,
            key.map(str::to_owned),
            name.to_owned(),
            Some("started".to_owned()),
            default,
        )
    }

    #[test]
    fn new_entries_get_ids_and_keys_resolve() {
        let review = Uuid::now_v7();
        let mut keys = HashMap::new();
        let resolved = resolve_entries(
            &[
                entry(Some(review), None, "Review", false),
                entry(None, Some("qa"), "QA", false),
            ],
            &existing(&[review]),
            &mut keys,
            "statuses",
            "status",
            30,
        )
        .expect("valid workflow");
        assert_eq!(resolved.len(), 2);
        assert!(resolved[0].existing && resolved[0].is_default);
        assert!(!resolved[1].existing);
        assert_eq!(keys["qa"], resolved[1].id);
    }

    #[test]
    fn duplicate_names_and_two_defaults_are_refused() {
        let mut keys = HashMap::new();
        let refused = resolve_entries(
            &[
                entry(None, None, "Done", false),
                entry(None, None, " done ", false),
            ],
            &HashMap::new(),
            &mut keys,
            "statuses",
            "status",
            30,
        );
        assert!(matches!(refused, Err(WorkflowRefusal::Invalid { .. })));
        let refused = resolve_entries(
            &[entry(None, None, "A", true), entry(None, None, "B", true)],
            &HashMap::new(),
            &mut keys,
            "statuses",
            "status",
            30,
        );
        assert!(matches!(refused, Err(WorkflowRefusal::Invalid { .. })));
    }

    #[test]
    fn unknown_ids_and_categories_are_refused() {
        let mut keys = HashMap::new();
        assert!(
            resolve_entries(
                &[entry(Some(Uuid::now_v7()), None, "A", false)],
                &HashMap::new(),
                &mut keys,
                "statuses",
                "status",
                30,
            )
            .is_err()
        );
        assert!(
            resolve_entries(
                &[(None, None, "A".to_owned(), Some("doing".to_owned()), false)],
                &HashMap::new(),
                &mut keys,
                "statuses",
                "status",
                30,
            )
            .is_err()
        );
        assert!(
            resolve_entries(&[], &HashMap::new(), &mut keys, "statuses", "status", 30).is_err()
        );
    }
}
