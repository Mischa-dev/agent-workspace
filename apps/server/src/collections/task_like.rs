//! Collections that act like a task (AWD-110, step 6). A collection with
//! the switch on has records that are tasks, the way the built-in Tasks
//! collection's are: each record has a task with the same ID
//! (`work_items.collection_id` names the collection), so it has a status,
//! assignee, priority, dates, labels and a key like AWD-12, shows on the
//! board and in My work, and can be claimed, run and commented on. Its own
//! fields stay its own; the built-in task fields come first
//! (`tasks::fields`). Its records change as tasks (`task_writes`), and
//! `task_record_guard` refuses other writes to them.
//!
//! Switching on makes a task for each record
//! (`collection_make_task_like`, migration 20261114070000). Switching off
//! puts its tasks away with their keys and history (`task_rows` shows only
//! tasks of collections that act like one), and switching on again brings
//! them back (migration 20261118145917). A collection can also use only
//! some of the space's statuses and start new tasks in one of them
//! (`Workflow`).

use sqlx::PgConnection;
use uuid::Uuid;

use super::{Collection, tasks};
use crate::error::AppError;

/// Why `current` can't become `acts_like_task` and `archived`, or None
/// when it can.
pub async fn refusal(
    connection: &mut PgConnection,
    current: &Collection,
    acts_like_task: bool,
    archived: bool,
) -> Result<Option<String>, AppError> {
    if current.is_tasks() {
        return Ok((!acts_like_task)
            .then(|| "Tasks is the built-in collection of this project's tasks.".to_owned()));
    }
    if acts_like_task && !current.acts_like_task {
        let taken = sqlx::query_scalar::<_, String>(
            "SELECT name FROM field_definitions \
             WHERE project_id = $1 AND collection_id = $2 AND archived_at IS NULL \
             ORDER BY position, id",
        )
        .bind(current.project_id)
        .bind(current.id)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .find(|name| tasks::is_builtin_name(name));
        // Its records would change as tasks, which record triggers don't
        // follow.
        let rules = sqlx::query_scalar::<_, String>(
            "SELECT name FROM automation_rules \
             WHERE project_id = $1 AND enabled \
               AND trigger_kind IN ('record_created', 'record_changed') \
               AND trigger ->> 'collectionId' = $2::text \
             ORDER BY name LIMIT 3",
        )
        .bind(current.project_id)
        .bind(current.id)
        .fetch_all(&mut *connection)
        .await?;
        if !rules.is_empty() {
            return Ok(Some(format!(
                "Automations watch {}'s records ({}). Records that act like tasks change as tasks, so turn those off or point them at task triggers first.",
                current.name,
                rules.join(", ")
            )));
        }
        if let Some(name) = taken {
            return Ok(Some(format!(
                "Records that act like tasks have a built-in \"{name}\" field. Rename {}'s own \"{name}\" field first.",
                current.name
            )));
        }
    }
    // Switched off, its records keep their tasks hidden, so switching on
    // again loses nothing; only work in progress holds it.
    if !acts_like_task && current.acts_like_task {
        let claimed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_claims claim \
             JOIN work_items task ON task.id = claim.work_item_id \
             WHERE task.project_id = $1 AND task.collection_id = $2 AND claim.expires_at > now())",
        )
        .bind(current.project_id)
        .bind(current.id)
        .fetch_one(&mut *connection)
        .await?;
        if claimed {
            return Ok(Some(format!(
                "Someone is working on one of {}'s tasks (it is claimed). Wait for the claim to end, or release it, first.",
                current.name
            )));
        }
        if crate::runs::store::active_for_collection_tasks(
            &mut *connection,
            current.project_id,
            current.id,
        )
        .await?
        {
            return Ok(Some(format!(
                "A run is working on one of {}'s tasks. Wait for it to finish, or stop it, first.",
                current.name
            )));
        }
    }
    if archived && !current.archived && acts_like_task {
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM collection_records \
             WHERE collection_id = $1 AND archived_at IS NULL",
        )
        .bind(current.id)
        .fetch_one(&mut *connection)
        .await?;
        if active > 0 {
            return Ok(Some(format!(
                "{}'s records are tasks, and {active} of them are still open. Archive them first.",
                current.name
            )));
        }
    }
    Ok(None)
}

/// Turns the switch on (making a task for each record that has none, as
/// `actor_id`) or off (its tasks stay, hidden). `refusal` has said it may.
pub async fn switch(
    connection: &mut PgConnection,
    collection_id: Uuid,
    actor_id: Uuid,
    on: bool,
) -> Result<(), AppError> {
    if on {
        sqlx::query("SELECT collection_make_task_like($1, $2)")
            .bind(collection_id)
            .bind(actor_id)
            .execute(&mut *connection)
            .await?;
    } else {
        sqlx::query(
            // Its records keep their task numbers; new ones follow on.
            "UPDATE collections SET acts_like_task = false, version = version + 1, updated_at = now(), \
                 next_number = GREATEST(next_number, \
                     (SELECT COALESCE(max(record.number), 0) + 1 FROM collection_records record \
                      WHERE record.collection_id = collections.id)) \
             WHERE id = $1 AND builtin IS NULL",
        )
        .bind(collection_id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// A project's collection that acts like a task, by name or ID (case
/// doesn't matter): the built-in Tasks collection too. None when no active
/// one is called that.
pub async fn find(
    connection: &mut PgConnection,
    project_id: Uuid,
    reference: &str,
) -> Result<Option<Collection>, AppError> {
    Ok(super::find(&mut *connection, project_id, reference)
        .await?
        .filter(|collection| collection.acts_like_task && !collection.archived))
}

/// The names of a project's active collections that act like a task, the
/// built-in one first.
pub async fn names(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM collections \
         WHERE project_id = $1 AND acts_like_task AND archived_at IS NULL \
         ORDER BY builtin IS NULL, position, id",
    )
    .bind(project_id)
    .fetch_all(executor)
    .await?)
}

/// A task-like collection's own workflow within its space's (T1.25): the
/// statuses its tasks use (None is all) and the one they start in (None is
/// the space's default).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Workflow {
    pub statuses: Option<Vec<Uuid>>,
    pub start: Option<Uuid>,
}

/// Why a collection can't take this workflow, if it can't.
pub async fn workflow_refusal(
    connection: &mut PgConnection,
    current: &Collection,
    acts_like_task: bool,
    workflow: &Workflow,
) -> Result<Option<String>, AppError> {
    if current.is_tasks() {
        return Ok(Some(
            "Tasks follows the space's workflow; change that in workflow settings.".to_owned(),
        ));
    }
    if !acts_like_task {
        return Ok(Some(format!(
            "{}'s records don't act like tasks, so it has no statuses to choose.",
            current.name
        )));
    }
    let enabled = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1 AND enabled",
    )
    .bind(current.project_id)
    .fetch_all(&mut *connection)
    .await?;
    let chosen = workflow.statuses.clone().unwrap_or_else(|| enabled.clone());
    if chosen.is_empty() {
        return Ok(Some("Keep at least one status.".to_owned()));
    }
    if chosen.iter().any(|id| !enabled.contains(id)) {
        return Ok(Some(
            "Choose from the space's statuses (workflow_get lists them).".to_owned(),
        ));
    }
    if workflow.start.is_some_and(|start| !chosen.contains(&start)) {
        return Ok(Some(
            "New tasks start in one of the statuses it uses.".to_owned(),
        ));
    }
    Ok(None)
}

pub async fn set_workflow(
    connection: &mut PgConnection,
    collection_id: Uuid,
    workflow: &Workflow,
) -> Result<(), AppError> {
    sqlx::query("UPDATE collections SET task_statuses = $2, start_status_id = $3 WHERE id = $1")
        .bind(collection_id)
        .bind(workflow.statuses.as_deref())
        .bind(workflow.start)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// The statuses a collection's tasks use and the one they start in, read
/// for a task write; None for a task of the built-in Tasks collection or of
/// a collection that uses the whole workflow.
pub async fn workflow_of(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection_id: Option<Uuid>,
) -> Result<Option<(Option<Vec<Uuid>>, Option<Uuid>)>, AppError> {
    let Some(collection_id) = collection_id else {
        return Ok(None);
    };
    Ok(sqlx::query_as::<_, (Option<Vec<Uuid>>, Option<Uuid>)>(
        "SELECT task_statuses, start_status_id FROM collections WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(collection_id)
    .fetch_optional(&mut *connection)
    .await?
    .filter(|(statuses, start)| statuses.is_some() || start.is_some()))
}

/// Status names or IDs to IDs, among the space's enabled statuses; Err
/// names the one that isn't there.
pub async fn resolve_statuses(
    connection: &mut PgConnection,
    project_id: Uuid,
    references: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    let statuses = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM work_item_statuses WHERE project_id = $1 AND enabled ORDER BY position",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut ids = Vec::new();
    for reference in references {
        let reference = reference.trim();
        let found = statuses.iter().find(|(id, name)| {
            id.to_string() == reference.to_lowercase() || name.eq_ignore_ascii_case(reference)
        });
        match found {
            Some((id, _)) if !ids.contains(id) => ids.push(*id),
            Some(_) => {}
            None => {
                return Ok(Err(format!(
                    "There is no status \"{reference}\" here. Statuses: {}.",
                    statuses
                        .iter()
                        .map(|(_, name)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(Ok(ids))
}
