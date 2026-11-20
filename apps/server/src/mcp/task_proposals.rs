//! Proposals on tasks (T1.41): someone whose role only suggests on tasks
//! (`access::write_mode` answers `Propose`) changes one as usual, through
//! `task_update`, and the change is kept as a proposal instead.
//!
//! The change is resolved once, when it's proposed: each part it sets, with
//! the value it read (`was`) and the one it writes (`is`), as stored (IDs
//! for a status, a type, an assignee or labels) and as shown. It's tried
//! inside a savepoint and rolled back, so a proposal the workflow, the
//! assignee rules or a label limit would refuse is refused at once.
//! Accepting writes it as its author's change, one version, with the audit
//! row naming who accepted it (`AuditAttribution::Proposal`), and is
//! refused when any part it sets changed since, so it never undoes a change
//! the author hadn't seen. Its request card asks the people who edit the
//! task to decide it (`proposals::raise_request`).
//!
//! A proposal takes what a task's own fields hold: title, description,
//! status, type, priority, assignee, labels, dates, estimate and custom
//! fields. Moving, archiving, blocking, planning and handing a task on need
//! edit access.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use sqlx::{Connection, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{McpPrincipal, workflow::Workflow};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    fields, memory,
    page_docs::suggestions::{Decided, Decision, Refusal},
    work_item_commands::{
        self, AuditAttribution, AuditVersionMetadata, CommandActor, LockedWorkItem,
        TaskFieldChanges, UpdateWorkItemCommand, UpdateWorkItemOutcome,
    },
    work_item_fields::{self, FieldValueChange, FieldValueChanges},
    work_item_labels::LabelDelta,
};

/// The most proposals one author keeps waiting on one task.
const MAX_PENDING_PER_AUTHOR: i64 = 10;

/// What a proposal can't take, and so needs edit access.
pub const EDIT_ONLY: &str = "Your role here suggests changes to tasks: you can propose its \
    title, description, status, type, priority, assignee, labels, dates, estimate and fields. \
    Moving, archiving, blocking, planning or handing it on needs edit access.";

/// A change to a task as `task_update` names it, before it's resolved.
#[derive(Debug, Default)]
pub struct Asked<'a> {
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub status: Option<&'a str>,
    pub item_type: Option<&'a str>,
    /// "none" (or clearAssignee) for nobody.
    pub assignee: Option<&'a str>,
    pub add_labels: &'a [String],
    pub remove_labels: &'a [String],
    pub start_date: Option<&'a str>,
    pub due_date: Option<&'a str>,
    pub estimate: Option<Option<f64>>,
    pub fields: Option<&'a Map<String, Value>>,
}

/// What proposing came to.
#[derive(Debug)]
pub enum Proposed {
    Made {
        id: Uuid,
        changes: Vec<Value>,
    },
    /// The change wouldn't change anything.
    Unchanged,
    /// The change itself is wrong: the argument and what would work.
    Problem(String, String),
    Refused(String),
}

fn problem(field: &str, message: impl Into<String>) -> Proposed {
    Proposed::Problem(field.to_owned(), message.into())
}

/// The task's labels, by ID, sorted.
async fn label_ids(
    connection: &mut PgConnection,
    space_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let mut ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT label_id FROM work_item_active_labels WHERE project_id = $1 AND work_item_id = $2",
    )
    .bind(space_id)
    .bind(task_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|id| id.to_string())
    .collect::<Vec<_>>();
    ids.sort();
    Ok(ids)
}

async fn label_names(
    connection: &mut PgConnection,
    space_id: Uuid,
    ids: &[String],
) -> Result<Value, AppError> {
    let ids = ids
        .iter()
        .filter_map(|id| id.parse::<Uuid>().ok())
        .collect::<Vec<_>>();
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM project_labels WHERE project_id = $1 AND id = ANY($2) ORDER BY lower(name)",
    )
    .bind(space_id)
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?;
    Ok(json!(names))
}

async fn person_name(
    connection: &mut PgConnection,
    space_id: Uuid,
    actor: Option<Uuid>,
) -> Result<Value, AppError> {
    let Some(actor) = actor else {
        return Ok(Value::Null);
    };
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(space_id)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await?
    .map_or(Value::Null, Value::String))
}

fn date(value: Option<NaiveDate>) -> Value {
    value.map_or(Value::Null, |date| Value::String(date.to_string()))
}

fn id(value: Option<Uuid>) -> Value {
    value.map_or(Value::Null, |id| Value::String(id.to_string()))
}

/// How one part of the task reads now, as a proposal stores it.
async fn now_of(
    connection: &mut PgConnection,
    space_id: Uuid,
    task: &LockedWorkItem,
    field: &str,
) -> Result<Value, AppError> {
    Ok(match field {
        "title" => Value::String(task.title.clone()),
        "description" => Value::String(task.description.clone()),
        "priority" => Value::String(task.priority.clone()),
        "status" => id(Some(task.status_id)),
        "type" => id(Some(task.type_id)),
        "assignee" => id(task.assignee_actor_id),
        "labels" => json!(label_ids(connection, space_id, task.id).await?),
        "startDate" => date(task.start_date),
        "dueDate" => date(task.due_date),
        "estimate" => task
            .estimate
            .map_or(Value::Null, |estimate| json!(estimate)),
        custom => task
            .field_values
            .get(custom)
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// One part of the change, if it changes anything.
fn entry(
    field: &str,
    name: &str,
    was: Value,
    is: Value,
    shown: Option<(Value, Value)>,
) -> Option<Value> {
    if was == is {
        return None;
    }
    let mut entry = json!({"field": field, "name": name, "was": was, "is": is});
    if let Some((was_shown, is_shown)) = shown {
        entry["wasShown"] = was_shown;
        entry["isShown"] = is_shown;
    }
    Some(entry)
}

/// The task's parts `asked` sets, resolved, each with how it reads now.
async fn resolve(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    space_id: Uuid,
    task: &LockedWorkItem,
    asked: &Asked<'_>,
) -> Result<Result<Vec<Value>, Proposed>, AppError> {
    let mut changes = Vec::new();
    if let Some(title) = asked.title {
        let Ok(title) = work_item_commands::normalize_single_line(
            title,
            work_item_commands::MAX_TITLE_CHARACTERS,
            false,
        ) else {
            return Ok(Err(problem(
                "title",
                "Give a title of one line, up to 240 characters.",
            )));
        };
        changes.extend(entry(
            "title",
            "Title",
            json!(task.title),
            json!(title),
            None,
        ));
    }
    if let Some(description) = asked.description {
        let Ok(description) = work_item_commands::normalize_description(description) else {
            return Ok(Err(problem(
                "description",
                "That description is too long or has control characters.",
            )));
        };
        changes.extend(entry(
            "description",
            "Description",
            json!(task.description),
            json!(description),
            None,
        ));
    }
    if let Some(priority) = asked.priority {
        if !super::writes::valid_priority(priority) {
            return Ok(Err(problem(
                "priority",
                "Use no_priority, low, medium, high or urgent.",
            )));
        }
        changes.extend(entry(
            "priority",
            "Priority",
            json!(task.priority),
            json!(priority),
            None,
        ));
    }
    if asked.status.is_some() || asked.item_type.is_some() {
        let workflow = Workflow::load(transaction, space_id).await?;
        if let Some(status) = asked.status {
            let to = match workflow.status(status) {
                Ok(status) => status.id,
                Err(message) => return Ok(Err(problem("status", message))),
            };
            if let Some(message) = workflow.transition_problem(task.status_id, to) {
                return Ok(Err(problem("status", message)));
            }
            let shown = (
                json!(workflow.status_name(task.status_id)),
                json!(workflow.status_name(to)),
            );
            changes.extend(entry(
                "status",
                "Status",
                id(Some(task.status_id)),
                id(Some(to)),
                Some(shown),
            ));
        }
        if let Some(kind) = asked.item_type {
            let to = match workflow.work_type(kind) {
                Ok(kind) => kind.id,
                Err(message) => return Ok(Err(problem("type", message))),
            };
            let shown = (
                json!(workflow.type_name(task.type_id)),
                json!(workflow.type_name(to)),
            );
            changes.extend(entry(
                "type",
                "Type",
                id(Some(task.type_id)),
                id(Some(to)),
                Some(shown),
            ));
        }
    }
    if let Some(assignee) = asked.assignee {
        let to = match super::tasks::resolve_assignee(transaction, principal, space_id, assignee)
            .await?
        {
            Ok(to) => to,
            Err(super::tasks::AssigneeProblem::NotInProject(message))
            | Err(super::tasks::AssigneeProblem::CantBeAssigned(message)) => {
                return Ok(Err(problem("assignee", message)));
            }
        };
        let shown = (
            person_name(transaction, space_id, task.assignee_actor_id).await?,
            person_name(transaction, space_id, to).await?,
        );
        changes.extend(entry(
            "assignee",
            "Assignee",
            id(task.assignee_actor_id),
            id(to),
            Some(shown),
        ));
    }
    if !asked.add_labels.is_empty() || !asked.remove_labels.is_empty() {
        let mut ids = BTreeSet::new();
        for (field, references) in [
            ("addLabels", asked.add_labels),
            ("removeLabels", asked.remove_labels),
        ] {
            let resolution =
                crate::work_item_labels::resolve(transaction, space_id, references).await?;
            if !resolution.is_complete() {
                let message =
                    crate::work_item_labels::unknown_message(transaction, space_id, &resolution)
                        .await?;
                return Ok(Err(problem(field, message)));
            }
            ids.extend(
                resolution
                    .ids()
                    .into_iter()
                    .map(|id| (field == "addLabels", id.to_string())),
            );
        }
        let was = label_ids(transaction, space_id, task.id).await?;
        let mut is = was.iter().cloned().collect::<BTreeSet<_>>();
        for (add, id) in ids {
            if add {
                is.insert(id);
            } else {
                is.remove(&id);
            }
        }
        let is = is.into_iter().collect::<Vec<_>>();
        let shown = (
            label_names(transaction, space_id, &was).await?,
            label_names(transaction, space_id, &is).await?,
        );
        changes.extend(entry(
            "labels",
            "Labels",
            json!(was),
            json!(is),
            Some(shown),
        ));
    }
    for (field, name, value, now) in [
        ("startDate", "Start date", asked.start_date, task.start_date),
        ("dueDate", "Due date", asked.due_date, task.due_date),
    ] {
        if let Some(value) = value {
            let Some(to) = super::planning::date_change(value) else {
                return Ok(Err(problem(field, super::planning::DATE_MESSAGE)));
            };
            changes.extend(entry(field, name, date(now), date(to), None));
        }
    }
    if let Some(estimate) = asked.estimate {
        if estimate.is_some_and(|estimate| !estimate.is_finite() || estimate < 0.0) {
            return Ok(Err(problem("estimate", "Give an estimate of 0 or more.")));
        }
        changes.extend(entry(
            "estimate",
            "Estimate",
            task.estimate
                .map_or(Value::Null, |estimate| json!(estimate)),
            estimate.map_or(Value::Null, |estimate| json!(estimate)),
            None,
        ));
    }
    if let Some(written) = asked.fields.filter(|fields| !fields.is_empty()) {
        let values = match super::field_tools::written_values(
            transaction,
            principal,
            space_id,
            written,
            super::field_tools::FieldsOf::Task(task.id),
        )
        .await?
        {
            Ok(values) => values,
            Err(trouble) => return Ok(Err(Proposed::Problem(trouble.field, trouble.message))),
        };
        for change in values.entries {
            let key = change.field.id.to_string();
            let was = task.field_values.get(&key).cloned().unwrap_or(Value::Null);
            let is = change.value.clone().unwrap_or(Value::Null);
            changes.extend(entry(
                &key,
                &change.field.name,
                was,
                is,
                Some((Value::Null, change.shown)),
            ));
        }
    }
    Ok(Ok(changes))
}

/// The resolved change as the task command takes it.
struct Planned {
    title: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    status_id: Option<Uuid>,
    type_id: Option<Uuid>,
    assignee: Option<Option<Uuid>>,
    add: Vec<Uuid>,
    remove: Vec<Uuid>,
    fields: TaskFieldChanges,
    values: FieldValueChanges,
}

fn uuid_of(value: &Value) -> Option<Uuid> {
    value.as_str().and_then(|id| id.parse().ok())
}

fn date_of(value: &Value) -> Option<NaiveDate> {
    value.as_str().and_then(|date| date.parse().ok())
}

fn ids_of(value: &Value) -> BTreeSet<Uuid> {
    value
        .as_array()
        .map(|ids| ids.iter().filter_map(uuid_of).collect())
        .unwrap_or_default()
}

async fn planned(
    connection: &mut PgConnection,
    space_id: Uuid,
    task_id: Uuid,
    changes: &[Value],
) -> Result<Planned, AppError> {
    let mut plan = Planned {
        title: None,
        description: None,
        priority: None,
        status_id: None,
        type_id: None,
        assignee: None,
        add: Vec::new(),
        remove: Vec::new(),
        fields: TaskFieldChanges::default(),
        values: FieldValueChanges::default(),
    };
    let mut definitions = None;
    for change in changes {
        let field = change
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is = change.get("is").unwrap_or(&Value::Null);
        match field {
            "title" => plan.title = is.as_str().map(str::to_owned),
            "description" => plan.description = is.as_str().map(str::to_owned),
            "priority" => plan.priority = is.as_str().map(str::to_owned),
            "status" => plan.status_id = uuid_of(is),
            "type" => plan.type_id = uuid_of(is),
            "assignee" => plan.assignee = Some(uuid_of(is)),
            "labels" => {
                let was = ids_of(change.get("was").unwrap_or(&Value::Null));
                let is = ids_of(is);
                plan.add = is.difference(&was).copied().collect();
                plan.remove = was.difference(&is).copied().collect();
            }
            "startDate" => plan.fields.start_date = Some(date_of(is)),
            "dueDate" => plan.fields.due_date = Some(date_of(is)),
            "estimate" => plan.fields.estimate = Some(is.as_f64()),
            custom => {
                let Some(field_id) = custom.parse::<Uuid>().ok() else {
                    continue;
                };
                if definitions.is_none() {
                    let collection =
                        work_item_fields::task_collection(&mut *connection, space_id, task_id)
                            .await?;
                    definitions = Some(
                        fields::list(
                            &mut *connection,
                            space_id,
                            work_item_fields::kind_of(collection),
                            true,
                        )
                        .await?,
                    );
                }
                let Some(definition) = definitions
                    .as_ref()
                    .and_then(|found| found.iter().find(|definition| definition.id == field_id))
                else {
                    return Err(AppError::Conflict("proposal", "field removed"));
                };
                plan.values.entries.push(FieldValueChange {
                    field: definition.clone(),
                    value: (!is.is_null()).then(|| is.clone()),
                    shown: change.get("isShown").cloned().unwrap_or(Value::Null),
                });
            }
        }
    }
    Ok(plan)
}

/// Runs `changes` on the locked task as `actor`.
async fn apply(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task: LockedWorkItem,
    changes: &[Value],
) -> Result<UpdateWorkItemOutcome, AppError> {
    let plan = planned(transaction, actor.project_id, task.id, changes).await?;
    let labels = (!plan.add.is_empty() || !plan.remove.is_empty()).then_some(LabelDelta {
        add: &plan.add,
        remove: &plan.remove,
    });
    let version = task.version;
    work_item_commands::update_locked_work_item(
        transaction,
        actor,
        task,
        UpdateWorkItemCommand {
            expected_version: version,
            title: plan.title.as_deref(),
            description: plan.description.as_deref(),
            type_id: plan.type_id,
            status_id: plan.status_id,
            priority: plan.priority.as_deref(),
            assignee_actor_id: plan.assignee,
            parent_work_item_id: None,
            board_placement: None,
            labels,
            fields: plan.fields,
            field_values: (!plan.values.is_empty()).then_some(&plan.values),
            summary: "Proposed change accepted",
            audit_action: "work_item.proposal_accepted",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await
}

/// Why the task command refused a change, in plain words.
fn refusal(outcome: &UpdateWorkItemOutcome) -> Option<(&'static str, String)> {
    match outcome {
        UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_) => None,
        UpdateWorkItemOutcome::InvalidAssignee => Some((
            "assignee",
            "They can't be assigned tasks here: only active people and agents in the space can."
                .to_owned(),
        )),
        UpdateWorkItemOutcome::InvalidTransition { .. } => Some((
            "status",
            "The workflow doesn't let the task move there from its status.".to_owned(),
        )),
        UpdateWorkItemOutcome::TooManyLabels { limit, .. } => {
            Some(("addLabels", format!("A task holds at most {limit} labels.")))
        }
        UpdateWorkItemOutcome::InvalidDates => Some((
            "dueDate",
            "The due date can't come before the start date.".to_owned(),
        )),
        UpdateWorkItemOutcome::Conflict(_) => Some((
            "expectedVersion",
            "The task changed meanwhile. Read it again and retry.".to_owned(),
        )),
        UpdateWorkItemOutcome::NotFound => {
            Some(("taskId", "That task isn't here any more.".to_owned()))
        }
        _ => Some((
            "update",
            "That change can't be made to this task.".to_owned(),
        )),
    }
}

/// Keeps `asked` (a change to `task_id`) as a proposal by `author`. The
/// caller has checked the author may suggest on tasks in the space.
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    transaction: &mut Transaction<'_, Postgres>,
    author: &CommandActor,
    principal: &McpPrincipal,
    via_grant_id: Option<Uuid>,
    task_id: Uuid,
    expected_version: i64,
    asked: &Asked<'_>,
    note: Option<&str>,
) -> Result<Proposed, AppError> {
    let space_id = author.project_id;
    let Some(task) = work_item_commands::lock_work_item(transaction, space_id, task_id).await?
    else {
        return Err(AppError::NotFound("task"));
    };
    if task.archived {
        return Ok(Proposed::Refused(
            "This task is archived. Someone who edits tasks can restore it first.".to_owned(),
        ));
    }
    if expected_version != task.version {
        return Ok(problem(
            "expectedVersion",
            format!(
                "The task changed since it was read (now version {}). Read it again and retry.",
                task.version
            ),
        ));
    }
    let changes = match resolve(transaction, principal, space_id, &task, asked).await? {
        Ok(changes) => changes,
        Err(refused) => return Ok(refused),
    };
    if changes.is_empty() {
        return Ok(Proposed::Unchanged);
    }
    // Try it for real, as the author, and roll it back: whatever the task
    // command would refuse is refused now, not when someone accepts it.
    {
        let mut trial = (**transaction).begin().await?;
        let task = work_item_commands::lock_work_item(&mut trial, space_id, task_id)
            .await?
            .ok_or(AppError::NotFound("task"))?;
        let outcome = apply(&mut trial, author, task, &changes).await?;
        trial.rollback().await?;
        if let Some((field, message)) = refusal(&outcome) {
            return Ok(problem(field, message));
        }
        if matches!(outcome, UpdateWorkItemOutcome::Noop(_)) {
            return Ok(Proposed::Unchanged);
        }
    }
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals \
         WHERE thing_kind = 'task' AND thing_id = $1 AND author_actor_id = $2 AND status = 'pending'",
    )
    .bind(task_id)
    .bind(author.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if pending >= MAX_PENDING_PER_AUTHOR {
        return Ok(Proposed::Refused(
            "You have too many proposals waiting on this task. Wait for some to be decided, or \
             withdraw them."
                .to_owned(),
        ));
    }
    let id = Uuid::now_v7();
    let workspace_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO proposals (
            id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
            via_oauth_grant_id, note, base_version, change, preview
        )
        SELECT $1, project.organization_id, project.id, 'task', $3, 'task_update', $4, $5,
               $6, $7, jsonb_build_object('changes', $8::jsonb),
               jsonb_build_object('fields', jsonb_array_length($8::jsonb))
        FROM projects project WHERE project.id = $2
        RETURNING workspace_id
        "#,
    )
    .bind(id)
    .bind(space_id)
    .bind(task_id)
    .bind(author.actor_id)
    .bind(via_grant_id)
    .bind(note)
    .bind(task.version)
    .bind(Value::Array(changes.clone()))
    .fetch_one(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        task_id,
        author.actor_id,
    )
    .await?;
    Ok(Proposed::Made { id, changes })
}

/// Accepts, rejects or withdraws a proposal on task `task_id`. `may_edit`
/// is whether `decider` edits the task directly; accepting and rejecting
/// need it, and only the author withdraws.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    decider: memory::Writer,
    may_edit: bool,
    task_id: Uuid,
    proposal_id: Uuid,
    decision: Decision,
) -> Result<Result<Decided, Refusal>, AppError> {
    let Some((space_id, workspace_id, author_id, status, change)) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Value)>(
            "SELECT space_id, workspace_id, author_actor_id, status, change \
             FROM proposals WHERE id = $1 AND thing_kind = 'task' AND thing_id = $2 \
             FOR UPDATE",
        )
        .bind(proposal_id)
        .bind(task_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if status != "pending" {
        return Ok(Err(Refusal::AlreadyDecided(status)));
    }
    match decision {
        Decision::Withdraw if decider.actor_id != author_id => {
            return Ok(Err(Refusal::NotYours(
                "Only its author can withdraw a proposal. Reject it instead.",
            )));
        }
        Decision::Accept | Decision::Reject if !may_edit => {
            return Ok(Err(Refusal::NotYours(
                "Deciding a proposal needs edit access to the task. Ask someone who edits it.",
            )));
        }
        _ => {}
    }
    let Some(task) = work_item_commands::lock_work_item(transaction, space_id, task_id).await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    let mut version = task.version;
    let status = match decision {
        Decision::Accept => {
            if task.archived {
                return Ok(Err(Refusal::Archived));
            }
            let changes = change
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for entry in &changes {
                let field = entry
                    .get("field")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = entry.get("name").and_then(Value::as_str).unwrap_or(field);
                let was = entry.get("was").unwrap_or(&Value::Null);
                if &now_of(transaction, space_id, &task, field).await? != was {
                    return Ok(Err(Refusal::Stale(format!(
                        "\"{name}\" has changed since it was proposed."
                    ))));
                }
            }
            let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
                .bind(author_id)
                .fetch_one(&mut **transaction)
                .await?;
            // The change is the author's; the audit row says who accepted it.
            let author = CommandActor {
                actor_id: author_id,
                actor_kind: crate::accounts::actor_kind(&kind)?,
                project_id: space_id,
                audit: AuditAttribution::Proposal {
                    proposal_id,
                    accepted_by: decider.actor_id,
                },
            };
            let outcome = apply(transaction, &author, task, &changes).await?;
            if let Some((_, message)) = refusal(&outcome) {
                return Ok(Err(Refusal::Stale(message)));
            }
            if let UpdateWorkItemOutcome::Applied {
                version: applied, ..
            } = outcome
            {
                version = applied;
            }
            "accepted"
        }
        Decision::Reject => "rejected",
        Decision::Withdraw => "withdrawn",
    };
    sqlx::query(
        "UPDATE proposals SET status = $2, decided_by_actor_id = $3, decided_at = now(), \
                applied_version = $4, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proposal_id)
    .bind(status)
    .bind(decider.actor_id)
    .bind((status == "accepted").then_some(version))
    .execute(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        task_id,
        decider.actor_id,
    )
    .await?;
    Ok(Ok(Decided { status, version }))
}

/// Tells the task's readers its proposals changed (ids only).
async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    space_id: Uuid,
    task_id: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    events::emit(
        transaction,
        NewEvent::thing(&catalog::TASK_PROPOSALS_CHANGED, space_id, "task", task_id)
            .actor(actor)
            .workspace(workspace_id)
            .payload(json!({})),
    )
    .await?;
    Ok(())
}
