//! Turning task descriptions into new tasks, for task_create_many: their
//! shape is checked without the database, names are resolved inside the
//! batch's transaction, and the tasks are created in order through
//! `work_item_commands::create_work_item`, the one create path the web app,
//! task_create and task_move take too, so each task's criteria and labels
//! land under its one created activity.
//!
//! task_create keeps its own entry point (its WriteResult, idempotency scopes
//! and request hashes are fixed) and shares the resolvers used here:
//! `Workflow` for statuses and types, `tasks::resolve_assignee`, and
//! `work_item_labels` for labels.

use std::collections::HashMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::insert_audit,
    task_batch::BatchProblem,
    tasks::resolve_assignee,
    workflow::Workflow,
    writes::{
        DESCRIPTION_MESSAGE, LABEL_COUNT_MESSAGE, PRIORITY_MESSAGE, STATUS_MESSAGE, TITLE_MESSAGE,
        TYPE_MESSAGE, normalize_criteria, parent_problem_text, valid_priority,
    },
};
use crate::{
    error::AppError,
    work_item_commands::{
        AllocatedNumberMetadata, CommandActor, CreateWorkItemCommand, CreateWorkItemOutcome,
        MAX_SUBTASK_LEVELS, MAX_TITLE_CHARACTERS, ParentProblem, create_work_item, lock_work_items,
        normalize_description, normalize_single_line, subtask_level,
    },
    work_item_labels::{
        self, LabelLimit, LabelRef, MAX_ACTIVE_LABELS_PER_PROJECT, MAX_LABELS_PER_TASK, Resolution,
        SkipReason, echo,
    },
};

const MAX_REFERENCE_CHARACTERS: usize = 64;
const MAX_ASSIGNEE_CHARACTERS: usize = 200;
const NO_ONE_MESSAGE: &str = "Leave assignee out for an unassigned task.";

/// One task to create, as an agent describes it.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NewTask {
    /// Your name for this task within the list, so subtasks can point at it
    /// with parentRef: 1 to 64 letters, digits, dashes, dots or underscores.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    /// Markdown.
    pub description: Option<String>,
    /// no_priority (the default), low, medium, high, or urgent.
    pub priority: Option<String>,
    /// A status by name, such as "Ready" (see workflow_get). Any status works,
    /// Done included. Left out, the project's first status.
    pub status: Option<String>,
    /// A type by name, such as "Bug". Left out, the project's default type.
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    /// "me", an @handle, or an actor ID. Leave it out for an unassigned task.
    pub assignee: Option<String>,
    /// Label names (at most 20). They must exist in the project unless the
    /// call passes createMissingLabels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// What done looks like, one criterion per entry, in order (at most 50).
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    /// The ref of an EARLIER task in this list, to make this a subtask of it.
    pub parent_ref: Option<String>,
    /// An existing task in the same project (ID, link, or key such as
    /// AWD-12), to make this a subtask of it.
    pub parent_id: Option<String>,
}

/// Where a new task goes in the task tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ParentChoice {
    None,
    /// An earlier task in the same list, by its index.
    InBatch(usize),
    /// An existing task, already resolved in the batch's project.
    Existing(Uuid),
}

/// A task with its text normalized and its parent worked out. It is also
/// what a batch's request hash covers, so a retry that spells a parent as a
/// key instead of an ID still matches.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Draft {
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub status: Option<String>,
    pub task_type: Option<String>,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub parent: ParentChoice,
    /// parentId as the call wrote it, until it's resolved into `parent`.
    #[serde(skip)]
    pub parent_written: Option<String>,
}

impl Draft {
    /// The argument a parent problem points at.
    pub(super) fn parent_field(&self) -> &'static str {
        if self.parent_written.is_some() {
            "parentId"
        } else {
            "parentRef"
        }
    }
}

fn valid_reference(value: &str) -> bool {
    (1..=MAX_REFERENCE_CHARACTERS).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

/// Pass 1, without the database: text, priority, refs and parentRef order,
/// and acceptance criteria. Every problem is collected, not just the first;
/// each task still gets a draft, so later passes can check its other fields.
/// A parentRef must name an earlier task, which keeps the tree free of
/// cycles and the numbering in list order.
pub(super) fn check_shapes(tasks: &[NewTask]) -> (Vec<Draft>, Vec<BatchProblem>) {
    let mut drafts = Vec::with_capacity(tasks.len());
    let mut problems = Vec::new();
    // Refs of the tasks so far, lowercased, with where each is.
    let mut earlier: HashMap<String, usize> = HashMap::new();
    for (index, task) in tasks.iter().enumerate() {
        let mut found: Vec<(&'static str, String)> = Vec::new();
        let written_reference = task
            .reference
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let reference = match written_reference {
            Some(value) if valid_reference(value) => Some(value.to_owned()),
            Some(_) => {
                found.push((
                    "ref",
                    "Use a ref of 1 to 64 letters, digits, dashes, dots or underscores, such as api or step-2.".to_owned(),
                ));
                None
            }
            None => None,
        };
        let parent_written = task
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let parent_ref = task
            .parent_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut parent = ParentChoice::None;
        match (parent_ref, &parent_written) {
            (Some(_), Some(_)) => {
                found.push(("parentRef", "Use parentRef or parentId, not both.".to_owned()));
            }
            (Some(name), None) => match earlier.get(&name.to_lowercase()) {
                Some(at) => parent = ParentChoice::InBatch(*at),
                None => found.push((
                    "parentRef",
                    format!(
                        "parentRef {} doesn't match an earlier task in this list. Put parents before their subtasks.",
                        echo(name)
                    ),
                )),
            },
            _ => {}
        }
        if let Some(reference) = &reference {
            match earlier.get(&reference.to_lowercase()) {
                Some(first) => found.push((
                    "ref",
                    format!(
                        "Two tasks use ref {reference} (at {first} and {index}). Each ref must be unique in the list."
                    ),
                )),
                None => {
                    earlier.insert(reference.to_lowercase(), index);
                }
            }
        }
        let title = match normalize_single_line(&task.title, MAX_TITLE_CHARACTERS, false) {
            Ok(title) => title,
            Err(_) => {
                found.push(("title", TITLE_MESSAGE.to_owned()));
                task.title.trim().to_owned()
            }
        };
        let description = match task.description.as_deref().map(normalize_description) {
            Some(Ok(description)) => description,
            Some(Err(_)) => {
                found.push(("description", DESCRIPTION_MESSAGE.to_owned()));
                String::new()
            }
            None => String::new(),
        };
        let priority = task
            .priority
            .as_deref()
            .map(str::trim)
            .unwrap_or("no_priority")
            .to_owned();
        if !valid_priority(&priority) {
            found.push(("priority", PRIORITY_MESSAGE.to_owned()));
        }
        let name =
            |value: &Option<String>, field: &'static str, message: &str, found: &mut Vec<_>| {
                let value = value.as_deref().map(str::trim)?;
                if value.is_empty() || value.len() > 80 {
                    found.push((field, message.to_owned()));
                }
                Some(value.to_owned())
            };
        let status = name(&task.status, "status", STATUS_MESSAGE, &mut found);
        let task_type = name(&task.task_type, "type", TYPE_MESSAGE, &mut found);
        let assignee = task
            .assignee
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        match assignee.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("none") => {
                found.push(("assignee", NO_ONE_MESSAGE.to_owned()));
            }
            Some(value) if value.len() > MAX_ASSIGNEE_CHARACTERS => {
                found.push((
                    "assignee",
                    "Pass the assignee as \"me\", an @handle, or an actor ID.".to_owned(),
                ));
            }
            _ => {}
        }
        if task.labels.len() > MAX_LABELS_PER_TASK {
            found.push(("labels", LABEL_COUNT_MESSAGE.to_owned()));
        }
        let acceptance_criteria = match normalize_criteria(&task.acceptance_criteria) {
            Ok(criteria) => criteria,
            Err(message) => {
                found.push(("acceptanceCriteria", message));
                Vec::new()
            }
        };
        for (field, message) in found {
            problems.push(BatchProblem::new(index, field, message).with_ref(reference.clone()));
        }
        drafts.push(Draft {
            reference,
            title,
            description,
            priority,
            status,
            task_type,
            assignee,
            labels: task
                .labels
                .iter()
                .map(|label| label.trim().to_owned())
                .collect(),
            acceptance_criteria,
            parent,
            parent_written,
        });
    }
    (drafts, problems)
}

/// A task ready to create: every name resolved in the batch's transaction.
#[derive(Clone, Debug)]
pub(super) struct PreparedTask {
    pub index: usize,
    pub reference: Option<String>,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub status_id: Uuid,
    pub type_id: Uuid,
    pub assignee: Option<Uuid>,
    pub labels: Vec<LabelRef>,
    pub acceptance_criteria: Vec<String>,
    pub parent: ParentChoice,
    pub parent_field: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Prepared {
    pub tasks: Vec<PreparedTask>,
    /// Labels this batch created (createMissingLabels), by name.
    pub labels_created: Vec<String>,
    /// Keys of the existing parents.
    pub parent_keys: HashMap<Uuid, String>,
}

/// Pass 2, inside the batch's transaction after the project lock: statuses
/// and types against the workflow, assignees, labels (creating missing ones
/// when asked), and existing parents, which are locked, must be open, and
/// must leave room for the subtasks below them. Problems go to `problems`;
/// a subtask of a task with problems gets none of its own for that, so
/// errors don't cascade. `principal` acts in the batch's project.
pub(super) async fn prepare(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    workflow: &Workflow,
    drafts: &[Draft],
    create_missing_labels: bool,
    problems: &mut Vec<BatchProblem>,
) -> Result<Prepared, AppError> {
    let project_id = principal.project_id;
    let project_key = workflow.project_key.clone();
    let problem = |draft: &Draft, index: usize, field: &str, message: String| {
        BatchProblem::new(index, field, message).with_ref(draft.reference.clone())
    };

    let mut assignees: HashMap<String, Result<Uuid, String>> = HashMap::new();
    for value in drafts.iter().filter_map(|draft| draft.assignee.as_deref()) {
        let key = value.to_lowercase();
        if assignees.contains_key(&key) || value.eq_ignore_ascii_case("none") {
            continue;
        }
        let resolved = match resolve_assignee(transaction, principal, project_id, value).await? {
            Ok(Some(actor_id)) => Ok(actor_id),
            Ok(None) => Err(NO_ONE_MESSAGE.to_owned()),
            Err(problem) => Err(problem.message().to_owned()),
        };
        assignees.insert(key, resolved);
    }

    let mut existing = drafts
        .iter()
        .filter_map(|draft| match draft.parent {
            ParentChoice::Existing(id) => Some(id),
            _ => None,
        })
        .collect::<Vec<_>>();
    existing.sort();
    existing.dedup();
    // (key, archived, level) of each existing parent, locked in id order.
    let mut parents: HashMap<Uuid, (String, bool, usize)> = HashMap::new();
    for parent in lock_work_items(transaction, project_id, &existing).await? {
        let level = subtask_level(transaction, project_id, parent.id).await?;
        parents.insert(
            parent.id,
            (
                format!("{project_key}-{}", parent.number),
                parent.archived,
                level,
            ),
        );
    }

    let labels = resolve_labels(
        transaction,
        principal,
        drafts,
        create_missing_labels,
        problems,
    )
    .await?;

    let default_status = workflow.default_status().map(|status| status.id);
    let default_type = workflow.default_type().map(|kind| kind.id);
    let mut levels = vec![1_usize; drafts.len()];
    let mut tasks = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.iter().enumerate() {
        let status_id = match draft.status.as_deref() {
            Some(value) => match workflow.status(value) {
                Ok(status) => status.id,
                Err(message) => {
                    problems.push(problem(draft, index, "status", message));
                    Uuid::nil()
                }
            },
            None => default_status.unwrap_or_else(|| {
                problems.push(problem(
                    draft,
                    index,
                    "status",
                    format!("{project_key} has no starting status. Pass one (see workflow_get)."),
                ));
                Uuid::nil()
            }),
        };
        let type_id = match draft.task_type.as_deref() {
            Some(value) => match workflow.work_type(value) {
                Ok(kind) => kind.id,
                Err(message) => {
                    problems.push(problem(draft, index, "type", message));
                    Uuid::nil()
                }
            },
            None => default_type.unwrap_or_else(|| {
                problems.push(problem(
                    draft,
                    index,
                    "type",
                    format!("{project_key} has no default type. Pass one (see workflow_get)."),
                ));
                Uuid::nil()
            }),
        };
        let assignee = match draft.assignee.as_deref() {
            Some(value) => match assignees.get(&value.to_lowercase()) {
                Some(Ok(actor_id)) => Some(*actor_id),
                Some(Err(message)) => {
                    problems.push(problem(draft, index, "assignee", message.clone()));
                    None
                }
                // "none", already refused by check_shapes.
                None => None,
            },
            None => None,
        };
        let field = draft.parent_field();
        levels[index] = match draft.parent {
            ParentChoice::None => 1,
            ParentChoice::InBatch(at) => levels.get(at).copied().unwrap_or(1) + 1,
            ParentChoice::Existing(id) => match parents.get(&id) {
                Some((key, archived, level)) => {
                    if *archived {
                        problems.push(problem(
                            draft,
                            index,
                            field,
                            parent_problem_text(
                                &ParentProblem::Archived(id),
                                &HashMap::from([(id, key.clone())]),
                            ),
                        ));
                    }
                    level + 1
                }
                None => {
                    problems.push(problem(
                        draft,
                        index,
                        field,
                        format!(
                            "Parent {} wasn't found in {project_key}. A subtask must be in the same project as its parent.",
                            echo(draft.parent_written.as_deref().unwrap_or_default())
                        ),
                    ));
                    1
                }
            },
        };
        // Only the first task past the limit is named; those under it
        // follow from it.
        if levels[index] == MAX_SUBTASK_LEVELS + 1 {
            problems.push(problem(
                draft,
                index,
                field,
                parent_problem_text(&ParentProblem::TooDeep, &HashMap::new()),
            ));
        }
        tasks.push(PreparedTask {
            index,
            reference: draft.reference.clone(),
            title: draft.title.clone(),
            description: draft.description.clone(),
            priority: draft.priority.clone(),
            status_id,
            type_id,
            assignee,
            labels: labels.per_task.get(index).cloned().unwrap_or_default(),
            acceptance_criteria: draft.acceptance_criteria.clone(),
            parent: draft.parent,
            parent_field: field,
        });
    }
    Ok(Prepared {
        tasks,
        labels_created: labels.created,
        parent_keys: parents
            .into_iter()
            .map(|(id, (key, _, _))| (id, key))
            .collect(),
    })
}

/// Each task's labels, and the labels created for them.
#[derive(Debug, Default)]
struct BatchLabels {
    per_task: Vec<Vec<LabelRef>>,
    created: Vec<String>,
}

/// Resolves every task's labels among the project's active ones. With
/// `create_missing`, names the project doesn't have are created first
/// (`ensure_by_name`, the same rule task_move follows); an archived name is
/// never recreated, and a full project leaves the name out with a problem.
async fn resolve_labels(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    drafts: &[Draft],
    create_missing: bool,
    problems: &mut Vec<BatchProblem>,
) -> Result<BatchLabels, AppError> {
    let project_id = principal.project_id;
    let mut cache: HashMap<Vec<String>, Resolution> = HashMap::new();
    let mut resolutions = Vec::with_capacity(drafts.len());
    for draft in drafts {
        if draft.labels.is_empty() {
            resolutions.push(Resolution::default());
            continue;
        }
        let resolution = match cache.get(&draft.labels) {
            Some(resolution) => resolution.clone(),
            None => {
                let resolution =
                    work_item_labels::resolve(transaction, project_id, &draft.labels).await?;
                cache.insert(draft.labels.clone(), resolution.clone());
                resolution
            }
        };
        resolutions.push(resolution);
    }
    let mut created = Vec::new();
    let mut full: HashMap<String, LabelLimit> = HashMap::new();
    if create_missing {
        let mut wanted: Vec<(String, String)> = Vec::new();
        for name in resolutions
            .iter()
            .flat_map(|resolution| resolution.unknown.iter())
            .filter_map(|value| work_item_labels::normalize_name(value))
        {
            if !wanted
                .iter()
                .any(|(other, _)| other.to_lowercase() == name.to_lowercase())
            {
                // No color: each gets the one its name picks.
                wanted.push((name, String::new()));
            }
        }
        if !wanted.is_empty() {
            let ensured = work_item_labels::ensure_by_name(
                transaction,
                project_id,
                principal.actor_id,
                &wanted,
            )
            .await?;
            // Each new label gets its own audit row, as label_write's and
            // task_move's do. A preview rolls these back with the rest.
            for label in &ensured.created {
                insert_audit(
                    transaction,
                    principal,
                    project_id,
                    "work_item_label.mcp_created",
                    "work_item_label",
                    label.id,
                    serde_json::json!({
                        "changedFields": ["label"],
                        "name": label.name,
                        "batch": "task_create_many",
                    }),
                )
                .await?;
            }
            created = ensured
                .created
                .into_iter()
                .map(|label| label.name)
                .collect();
            for (name, reason) in ensured.skipped {
                if let SkipReason::ProjectFull(limit) = reason {
                    full.insert(name.to_lowercase(), limit);
                }
            }
            // Resolve again: the names that were missing exist now.
            cache.clear();
            for (draft, resolution) in drafts.iter().zip(resolutions.iter_mut()) {
                if resolution.unknown.is_empty() {
                    continue;
                }
                *resolution = match cache.get(&draft.labels) {
                    Some(resolution) => resolution.clone(),
                    None => {
                        let fresh =
                            work_item_labels::resolve(transaction, project_id, &draft.labels)
                                .await?;
                        cache.insert(draft.labels.clone(), fresh.clone());
                        fresh
                    }
                };
            }
        }
    }
    let mut per_task = Vec::with_capacity(drafts.len());
    for (index, (draft, resolution)) in drafts.iter().zip(resolutions).enumerate() {
        if !resolution.is_complete() {
            let message = match resolution
                .unknown
                .iter()
                .find_map(|name| full.get(&name.to_lowercase()).map(|limit| (name, limit)))
            {
                Some((name, limit)) => format!(
                    "There's no room for a label called \"{}\": {}. Archive labels you don't use with label_write, or leave it out.",
                    echo(name),
                    match limit {
                        LabelLimit::Active => format!(
                            "a project has at most {MAX_ACTIVE_LABELS_PER_PROJECT} active labels"
                        ),
                        LabelLimit::Total =>
                            "the project has as many labels as it can keep".to_owned(),
                    }
                ),
                None => {
                    let mut message =
                        work_item_labels::unknown_message(transaction, project_id, &resolution)
                            .await?;
                    if !create_missing && !resolution.unknown.is_empty() {
                        message.push_str(
                            " Or pass createMissingLabels: true to create them with these tasks.",
                        );
                    }
                    message
                }
            };
            problems.push(
                BatchProblem::new(index, "labels", message).with_ref(draft.reference.clone()),
            );
        }
        per_task.push(resolution.found);
    }
    Ok(BatchLabels { per_task, created })
}

/// A task as created.
#[derive(Clone, Copy, Debug)]
pub(super) struct CreatedRow {
    pub work_item_id: Uuid,
    pub number: i64,
}

/// Creates prepared tasks in list order, each at the bottom of its status
/// column, so the board keeps the list's order. Everything was checked in
/// `prepare` under the same project lock; if a task is refused anyway, the
/// problem comes back and the caller rolls the whole batch back.
pub(super) async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    tasks: &[PreparedTask],
) -> Result<Result<Vec<CreatedRow>, BatchProblem>, AppError> {
    let mut created: Vec<CreatedRow> = Vec::with_capacity(tasks.len());
    for task in tasks {
        let parent = match task.parent {
            ParentChoice::None => None,
            ParentChoice::Existing(id) => Some(id),
            ParentChoice::InBatch(at) => Some(
                created
                    .get(at)
                    .ok_or(AppError::Internal("a batch parent comes first"))?
                    .work_item_id,
            ),
        };
        let label_ids = task.labels.iter().map(|label| label.id).collect::<Vec<_>>();
        let mut activity_metadata = serde_json::json!({"batchIndex": task.index});
        let mut audit_metadata =
            serde_json::json!({"batch": "task_create_many", "batchIndex": task.index});
        if let Some(reference) = &task.reference {
            audit_metadata["ref"] = serde_json::json!(reference);
        }
        if let Some(parent) = parent {
            activity_metadata["parentWorkItemId"] = serde_json::json!(parent);
            audit_metadata["parentWorkItemId"] = serde_json::json!(parent);
        }
        let (summary, audit_action) = match parent {
            Some(_) => (
                "Subtask created through MCP",
                "work_item.mcp_subtask_created",
            ),
            None => ("Work Item created through MCP", "work_item.mcp_created"),
        };
        let outcome = create_work_item(
            transaction,
            actor,
            CreateWorkItemCommand {
                field_values: None,
                title: &task.title,
                description: &task.description,
                type_id: Some(task.type_id),
                status_id: Some(task.status_id),
                priority: &task.priority,
                assignee_actor_id: task.assignee,
                parent_work_item_id: parent,
                label_ids: &label_ids,
                acceptance_criteria: &task.acceptance_criteria,
                board_placement: None,
                fields: Default::default(),
                activity_summary: summary,
                activity_metadata,
                activity_number_metadata: AllocatedNumberMetadata::Include,
                audit_action,
                audit_metadata,
            },
        )
        .await?;
        let refused = |field: &str,
                       message: String|
         -> Result<Result<Vec<CreatedRow>, BatchProblem>, AppError> {
            Ok(Err(
                BatchProblem::new(task.index, field, message).with_ref(task.reference.clone())
            ))
        };
        match outcome {
            CreateWorkItemOutcome::Created {
                work_item_id,
                number,
                ..
            } => created.push(CreatedRow {
                work_item_id,
                number,
            }),
            CreateWorkItemOutcome::InvalidMilestone => {
                return refused("milestone", super::writes::MILESTONE_MESSAGE.to_owned());
            }
            CreateWorkItemOutcome::InvalidDates => {
                return refused("dueDate", super::writes::DATES_MESSAGE.to_owned());
            }
            CreateWorkItemOutcome::InvalidAssignee => {
                return refused(
                    "assignee",
                    "That person can't be assigned tasks here: only active people and agents in the project can.".to_owned(),
                );
            }
            CreateWorkItemOutcome::ParentNotFound => {
                return refused(
                    task.parent_field,
                    parent_problem_text(&ParentProblem::NotFound, &HashMap::new()),
                );
            }
            CreateWorkItemOutcome::ParentArchived(parent) => {
                return refused(
                    task.parent_field,
                    parent_problem_text(&ParentProblem::Archived(parent.id), &HashMap::new()),
                );
            }
            CreateWorkItemOutcome::ParentTooDeep => {
                return refused(
                    task.parent_field,
                    parent_problem_text(&ParentProblem::TooDeep, &HashMap::new()),
                );
            }
            CreateWorkItemOutcome::InvalidPlacement(_) => {
                return Err(AppError::Internal("a batch task asked for no board place"));
            }
        }
    }
    Ok(Ok(created))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{NewTask, ParentChoice, check_shapes};

    fn tasks(value: serde_json::Value) -> Vec<NewTask> {
        serde_json::from_value(value).expect("tasks")
    }

    #[test]
    fn every_shape_problem_is_reported_where_it_is() {
        let (drafts, problems) = check_shapes(&tasks(json!([
            {"ref": "api", "title": "API"},
            {"ref": "c", "parentRef": "later", "title": "Too early"},
            {"ref": "later", "title": "Later"},
            {"ref": "API", "title": "Same ref"},
            {"parentRef": "api", "parentId": "AWD-1", "title": "Both"},
            {"parentRef": "api", "title": "", "priority": "soon", "acceptanceCriteria": ["ok", " "]},
            {"title": "Nobody", "assignee": "none"},
        ])));
        let found = problems
            .iter()
            .map(|problem| (problem.index, problem.field.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                (1, "parentRef"),
                (3, "ref"),
                (4, "parentRef"),
                (5, "title"),
                (5, "priority"),
                (5, "acceptanceCriteria"),
                (6, "assignee"),
            ],
            "{problems:?}"
        );
        assert!(problems[0].message.contains("earlier"), "{problems:?}");
        assert_eq!(problems[0].reference.as_deref(), Some("c"));
        assert!(problems[1].message.contains("unique"), "{problems:?}");
        assert_eq!(drafts.len(), 7);
        assert_eq!(drafts[5].parent, ParentChoice::InBatch(0));
        assert_eq!(drafts[4].parent_written.as_deref(), Some("AWD-1"));
        assert_eq!(drafts[0].priority, "no_priority");
    }

    #[test]
    fn a_clean_plan_has_no_problems_and_keeps_its_tree() {
        let (drafts, problems) = check_shapes(&tasks(json!([
            {"ref": "epic", "title": "  Import plans  ", "acceptanceCriteria": [" Every plan is a task "]},
            {"ref": "a", "parentRef": "EPIC", "title": "Parse Markdown"},
            {"parentRef": "a", "title": "Handle nested lists"},
        ])));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(drafts[0].title, "Import plans");
        assert_eq!(drafts[0].acceptance_criteria, ["Every plan is a task"]);
        assert_eq!(drafts[1].parent, ParentChoice::InBatch(0));
        assert_eq!(drafts[2].parent, ParentChoice::InBatch(1));
    }
}
