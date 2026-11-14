//! Writes to the records of a collection that acts like a task, such as
//! the built-in Tasks collection (AWD-110): each one is a task change, made by the same commands as the task page and the task
//! tools, so it gets the same checks, history, activity and events.
//!
//! A record's title is the task's title; the built-in fields (Status,
//! Assignee, Priority, Start date, Due date, Labels) are the task's own
//! columns; every other value is one of the collection's own fields (the
//! project's task fields, for Tasks). Archiving a record
//! archives the task. The task's triggers keep its record in step
//! (migration 20261114010000).

use std::collections::HashSet;

use chrono::NaiveDate;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    Collection,
    records::{ProblemKind, RecordInput, RecordRef, WriteProblem},
    tasks,
};
use crate::{
    error::AppError,
    policy::Action,
    work_item_commands::{
        AllocatedNumberMetadata, ArchiveCommand, ArchiveOutcome, AuditVersionMetadata,
        CommandActor, CreateWorkItemCommand, CreateWorkItemOutcome, TaskFieldChanges,
        UpdateWorkItemCommand, UpdateWorkItemOutcome, authorize_mutation, lock_work_item,
        normalize_title, read_membership, set_archived, update_locked_work_item,
    },
    work_item_fields::{self, FieldValueChanges, Writer},
    work_item_labels::LabelDelta,
};

/// The title of a task added from a record without one.
const UNTITLED: &str = "Untitled task";

/// What a record write asks of its task, sorted out of its values.
#[derive(Default)]
struct TaskChange {
    status_id: Option<Uuid>,
    assignee: Option<Option<Uuid>>,
    priority: Option<&'static str>,
    start_date: Option<Option<NaiveDate>>,
    due_date: Option<Option<NaiveDate>>,
    labels: Option<Vec<Uuid>>,
    custom: Vec<(String, Value)>,
}

fn invalid(field: &str, message: impl Into<String>) -> WriteProblem {
    WriteProblem {
        index: 0,
        kind: ProblemKind::Invalid,
        field: field.to_owned(),
        message: message.into(),
        current_version: None,
    }
}

fn id_value(value: &Value) -> Option<Uuid> {
    value
        .as_str()
        .and_then(|text| Uuid::parse_str(text.trim()).ok())
}

fn date_value(field: &str, value: &Value) -> Result<Option<NaiveDate>, WriteProblem> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
            .map(Some)
            .map_err(|_| invalid(field, "A date is written YYYY-MM-DD.")),
        _ => Err(invalid(field, "A date is written YYYY-MM-DD.")),
    }
}

/// Sorts a record's values into the task's columns and its field values.
/// Built-in fields are named by ID, as the web app writes them.
fn sort_values(values: &[(String, Value)]) -> Result<TaskChange, WriteProblem> {
    let mut change = TaskChange::default();
    for (reference, value) in values {
        let field = format!("values.{reference}");
        let Some(id) = Uuid::parse_str(reference.trim())
            .ok()
            .filter(|id| tasks::is_builtin_field(*id))
        else {
            change.custom.push((reference.clone(), value.clone()));
            continue;
        };
        match id {
            tasks::STATUS => {
                change.status_id = Some(
                    id_value(value)
                        .ok_or_else(|| invalid(&field, "A task always has a status."))?,
                );
            }
            tasks::ASSIGNEE => {
                change.assignee = Some(match value {
                    Value::Null => None,
                    other => Some(
                        id_value(other)
                            .ok_or_else(|| invalid(&field, "Name the assignee by their ID."))?,
                    ),
                });
            }
            tasks::PRIORITY => {
                change.priority = Some(match value {
                    Value::Null => "no_priority",
                    other => id_value(other)
                        .and_then(tasks::priority_for_option)
                        .ok_or_else(|| invalid(&field, "That isn't one of the priorities."))?,
                });
            }
            tasks::START_DATE => change.start_date = Some(date_value(&field, value)?),
            tasks::DUE_DATE => change.due_date = Some(date_value(&field, value)?),
            tasks::LABELS => {
                change.labels = Some(match value {
                    Value::Null => Vec::new(),
                    Value::Array(items) => items
                        .iter()
                        .map(|item| {
                            id_value(item)
                                .ok_or_else(|| invalid(&field, "Name labels by their IDs."))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => return Err(invalid(&field, "Labels are a list of label IDs.")),
                });
            }
            _ => change.custom.push((reference.clone(), value.clone())),
        }
    }
    Ok(change)
}

async fn allowed(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    action: Action,
    task: Option<Uuid>,
) -> Result<bool, AppError> {
    let Some(membership) = read_membership(transaction, actor.actor_id, actor.project_id).await?
    else {
        return Ok(false);
    };
    Ok(membership.enabled && authorize_mutation(actor, &membership, action, task)?.is_allowed())
}

/// Whether the actor may add, change or archive the project's task fields:
/// the same rule as the task field settings (the workflow permission).
pub async fn may_change_fields(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
) -> Result<bool, AppError> {
    allowed(transaction, actor, Action::WorkflowManage, None).await
}

async fn custom_values(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_key: &str,
    collection: &Collection,
    written: &[(String, Value)],
    task: Option<Uuid>,
) -> Result<Result<Option<FieldValueChanges>, WriteProblem>, AppError> {
    if written.is_empty() {
        return Ok(Ok(None));
    }
    let definitions = crate::fields::list(
        &mut **transaction,
        actor.project_id,
        collection.field_kind(),
        true,
    )
    .await?;
    let current = match task {
        Some(task) => work_item_fields::current_values(transaction, actor.project_id, task).await?,
        None => None,
    };
    Ok(work_item_fields::resolve_for(
        transaction,
        actor.project_id,
        Writer {
            actor_id: actor.actor_id,
            project_key,
        },
        &definitions,
        "values",
        written,
        current.as_ref(),
    )
    .await?
    .map(Some)
    .map_err(|problem| invalid(&problem.field, problem.message)))
}

async fn labels_active(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    labels: &[Uuid],
) -> Result<bool, AppError> {
    if labels.is_empty() {
        return Ok(true);
    }
    let unique = labels.iter().collect::<HashSet<_>>().len() as i64;
    let found = sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT id) FROM project_labels \
         WHERE project_id = $1 AND id = ANY($2) AND archived_at IS NULL",
    )
    .bind(project_id)
    .bind(labels)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(found == unique)
}

fn labels_problem() -> WriteProblem {
    invalid(
        &format!("values.{}", tasks::LABELS),
        "That isn't one of the project's labels.",
    )
}

fn forbidden(what: &str) -> WriteProblem {
    WriteProblem {
        index: 0,
        kind: ProblemKind::Invalid,
        field: "record".to_owned(),
        message: format!("You can't {what} tasks in this project."),
        current_version: None,
    }
}

fn conflict(current_version: i64) -> WriteProblem {
    WriteProblem {
        index: 0,
        kind: ProblemKind::Conflict,
        field: "expectedVersion".to_owned(),
        message: format!(
            "The task changed since it was read (now version {current_version}). Read it again and retry."
        ),
        current_version: Some(current_version),
    }
}

fn update_problem(outcome: UpdateWorkItemOutcome, expected: i64) -> Option<WriteProblem> {
    Some(match outcome {
        UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_) => return None,
        UpdateWorkItemOutcome::Conflict(current)
            if current.archived && current.version == expected =>
        {
            WriteProblem {
                index: 0,
                kind: ProblemKind::Conflict,
                field: "archived".to_owned(),
                message: "The task is archived. Restore it (archived: false) to change it."
                    .to_owned(),
                current_version: None,
            }
        }
        UpdateWorkItemOutcome::Conflict(current) => conflict(current.version),
        UpdateWorkItemOutcome::NotFound => WriteProblem {
            index: 0,
            kind: ProblemKind::NotFound,
            field: "record".to_owned(),
            message: "No such task.".to_owned(),
            current_version: None,
        },
        UpdateWorkItemOutcome::InvalidAssignee => invalid(
            &format!("values.{}", tasks::ASSIGNEE),
            "The assignee must be someone in the project who can hold tasks.",
        ),
        UpdateWorkItemOutcome::InvalidTransition { .. } => invalid(
            &format!("values.{}", tasks::STATUS),
            "The project's workflow doesn't allow that status change.",
        ),
        UpdateWorkItemOutcome::TooManyLabels { limit, .. } => invalid(
            &format!("values.{}", tasks::LABELS),
            format!("A task has at most {limit} labels."),
        ),
        UpdateWorkItemOutcome::InvalidDates => invalid(
            &format!("values.{}", tasks::DUE_DATE),
            "The start date can't be after the due date.",
        ),
        UpdateWorkItemOutcome::InvalidParent(_)
        | UpdateWorkItemOutcome::InvalidPlacement(_)
        | UpdateWorkItemOutcome::InvalidMilestone => {
            invalid("record", "That change can't be made here.")
        }
    })
}

/// Makes or changes one task of `collection` from a record write, and
/// answers with the task's ID. The caller holds the project lock and has
/// checked the collection may take record writes.
pub async fn write(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_key: &str,
    collection: &Collection,
    input: &RecordInput,
) -> Result<Result<Uuid, WriteProblem>, AppError> {
    let change = match sort_values(&input.values) {
        Ok(change) => change,
        Err(problem) => return Ok(Err(problem)),
    };
    // A record added without a title (the New button, a calendar day) is a
    // task to name on its page.
    let written_title = match (input.record, input.title.as_deref()) {
        (None, None) => Some(UNTITLED),
        (None, Some(title)) if title.trim().is_empty() => Some(UNTITLED),
        (_, title) => title,
    };
    let title = match written_title.map(normalize_title).transpose() {
        Ok(title) => title,
        Err(_) => {
            return Ok(Err(invalid(
                "title",
                "A task's title is one line of 1 to 240 characters.",
            )));
        }
    };
    match input.record {
        None => {
            create(
                transaction,
                actor,
                project_key,
                collection,
                input,
                change,
                title,
            )
            .await
        }
        Some(reference) => {
            update(
                transaction,
                actor,
                project_key,
                collection,
                input,
                reference,
                change,
                title,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_key: &str,
    collection: &Collection,
    input: &RecordInput,
    change: TaskChange,
    title: Option<String>,
) -> Result<Result<Uuid, WriteProblem>, AppError> {
    if !allowed(transaction, actor, Action::WorkItemCreate, None).await? {
        return Ok(Err(forbidden("add")));
    }
    if input.archived.is_some() || input.expected_version.is_some() {
        return Ok(Err(invalid(
            "record",
            "archived and expectedVersion change a task that exists; name it with record.",
        )));
    }
    let title = title.unwrap_or_else(|| UNTITLED.to_owned());
    let field_values = match custom_values(
        transaction,
        actor,
        project_key,
        collection,
        &change.custom,
        None,
    )
    .await?
    {
        Ok(values) => values,
        Err(problem) => return Ok(Err(problem)),
    };
    let labels = change.labels.unwrap_or_default();
    if !labels_active(transaction, actor.project_id, &labels).await? {
        return Ok(Err(labels_problem()));
    }
    let outcome = crate::work_item_commands::create_work_item(
        transaction,
        actor,
        CreateWorkItemCommand {
            collection_id: collection.task_collection_id(),
            field_values: field_values.as_ref(),
            title: &title,
            description: "",
            type_id: None,
            status_id: change.status_id,
            priority: change.priority.unwrap_or("no_priority"),
            assignee_actor_id: change.assignee.flatten(),
            parent_work_item_id: None,
            label_ids: &labels,
            acceptance_criteria: &[],
            board_placement: None,
            fields: crate::work_item_commands::NewTaskFields {
                start_date: change.start_date.flatten(),
                due_date: change.due_date.flatten(),
                ..Default::default()
            },
            activity_summary: "Work Item created",
            activity_metadata: serde_json::json!({ "via": "collection" }),
            activity_number_metadata: AllocatedNumberMetadata::Omit,
            audit_action: "work_item.created",
            audit_metadata: serde_json::json!({ "via": "collection" }),
        },
    )
    .await?;
    Ok(match outcome {
        CreateWorkItemOutcome::Created { work_item_id, .. } => Ok(work_item_id),
        CreateWorkItemOutcome::InvalidAssignee => Err(invalid(
            &format!("values.{}", tasks::ASSIGNEE),
            "The assignee must be someone in the project who can hold tasks.",
        )),
        CreateWorkItemOutcome::InvalidDates => Err(invalid(
            &format!("values.{}", tasks::DUE_DATE),
            "The start date can't be after the due date.",
        )),
        _ => Err(invalid("record", "That task can't be made here.")),
    })
}

#[allow(clippy::too_many_arguments)]
async fn update(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_key: &str,
    collection: &Collection,
    input: &RecordInput,
    reference: RecordRef,
    change: TaskChange,
    title: Option<String>,
) -> Result<Result<Uuid, WriteProblem>, AppError> {
    let (id, number) = match reference {
        RecordRef::Id(id) => (Some(id), None),
        RecordRef::Number(number) => (None, Some(number)),
    };
    // Only the collection's own tasks.
    let task_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_items WHERE project_id = $1 \
           AND (id = $2 OR ($2 IS NULL AND number = $3)) \
           AND collection_id IS NOT DISTINCT FROM $4",
    )
    .bind(actor.project_id)
    .bind(id)
    .bind(number)
    .bind(collection.task_collection_id())
    .fetch_optional(&mut **transaction)
    .await?;
    let current = match task_id {
        Some(id) => lock_work_item(transaction, actor.project_id, id).await?,
        None => None,
    };
    let Some(current) = current else {
        return Ok(Err(WriteProblem {
            index: 0,
            kind: ProblemKind::NotFound,
            field: "record".to_owned(),
            message: format!("{} has no record {}.", collection.name, reference.shown()),
            current_version: None,
        }));
    };
    let task_id = current.id;
    let edits = title.is_some()
        || change.status_id.is_some()
        || change.assignee.is_some()
        || change.priority.is_some()
        || change.start_date.is_some()
        || change.due_date.is_some()
        || change.labels.is_some()
        || !change.custom.is_empty();
    if edits && !allowed(transaction, actor, Action::WorkItemEdit, Some(task_id)).await? {
        return Ok(Err(forbidden("change")));
    }
    if input.archived.is_some()
        && !allowed(transaction, actor, Action::WorkItemArchive, Some(task_id)).await?
    {
        return Ok(Err(forbidden("archive")));
    }
    let mut version = current.version;
    if let Some(expected) = input.expected_version
        && expected != version
    {
        return Ok(Err(conflict(version)));
    }
    // A restore comes first, so the same write can change what it restores;
    // an archive comes last, after any change.
    if input.archived == Some(false) && current.archived {
        match archive(transaction, actor, task_id, version, false).await? {
            Ok(now) => version = now,
            Err(problem) => return Ok(Err(problem)),
        }
    }
    if edits {
        let field_values = match custom_values(
            transaction,
            actor,
            project_key,
            collection,
            &change.custom,
            Some(task_id),
        )
        .await?
        {
            Ok(values) => values,
            Err(problem) => return Ok(Err(problem)),
        };
        let (add, remove) = match &change.labels {
            Some(wanted) => {
                let held = sqlx::query_scalar::<_, Uuid>(
                    "SELECT label_id FROM work_item_labels WHERE project_id = $1 AND work_item_id = $2",
                )
                .bind(actor.project_id)
                .bind(task_id)
                .fetch_all(&mut **transaction)
                .await?;
                (
                    wanted
                        .iter()
                        .filter(|id| !held.contains(id))
                        .copied()
                        .collect::<Vec<_>>(),
                    held.iter()
                        .filter(|id| !wanted.contains(id))
                        .copied()
                        .collect::<Vec<_>>(),
                )
            }
            None => (Vec::new(), Vec::new()),
        };
        // Labels the task holds stay even when archived since; only the
        // ones it gains must be active.
        if !labels_active(transaction, actor.project_id, &add).await? {
            return Ok(Err(labels_problem()));
        }
        let current = lock_work_item(transaction, actor.project_id, task_id)
            .await?
            .ok_or(AppError::Internal("task is missing"))?;
        let outcome = update_locked_work_item(
            transaction,
            actor,
            current,
            UpdateWorkItemCommand {
                expected_version: version,
                title: title.as_deref(),
                description: None,
                type_id: None,
                status_id: change.status_id,
                priority: change.priority,
                assignee_actor_id: change.assignee,
                parent_work_item_id: None,
                board_placement: None,
                labels: change.labels.is_some().then_some(LabelDelta {
                    add: &add,
                    remove: &remove,
                }),
                fields: TaskFieldChanges {
                    start_date: change.start_date,
                    due_date: change.due_date,
                    ..Default::default()
                },
                field_values: field_values.as_ref(),
                summary: "Work Item updated",
                audit_action: "work_item.updated",
                audit_version_metadata: AuditVersionMetadata::Omit,
            },
        )
        .await?;
        if let UpdateWorkItemOutcome::Applied { version: now, .. } = &outcome {
            version = *now;
        }
        if let Some(problem) = update_problem(outcome, version) {
            return Ok(Err(problem));
        }
    }
    if input.archived == Some(true)
        && let Err(problem) = archive(transaction, actor, task_id, version, true).await?
    {
        return Ok(Err(problem));
    }
    Ok(Ok(task_id))
}

async fn archive(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    version: i64,
    archived: bool,
) -> Result<Result<i64, WriteProblem>, AppError> {
    let outcome = set_archived(
        transaction,
        actor,
        task_id,
        ArchiveCommand {
            expected_version: version,
            archived,
            summary: if archived {
                "Work Item archived"
            } else {
                "Work Item restored"
            },
            audit_action: if archived {
                "work_item.archived"
            } else {
                "work_item.restored"
            },
        },
    )
    .await?;
    Ok(match outcome {
        ArchiveOutcome::Applied { version, .. } => Ok(version),
        ArchiveOutcome::Unchanged(current) => Ok(current.version),
        ArchiveOutcome::VersionConflict(current) => Err(conflict(current.version)),
        ArchiveOutcome::MovedAway { .. } => Err(invalid(
            "archived",
            "This task moved to another project; its copy there is the one to change.",
        )),
        ArchiveOutcome::NotFound => Err(WriteProblem {
            index: 0,
            kind: ProblemKind::NotFound,
            field: "record".to_owned(),
            message: "No such task.".to_owned(),
            current_version: None,
        }),
    })
}
