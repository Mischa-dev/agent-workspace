//! task_create_many and task_update_many (docs/plans/mcp-tools.md, Phase 2):
//! many tasks in one call, all or nothing. Either every task is created or
//! changed, or nothing is and each problem is listed with the task it's
//! about and what to do. A preview runs the same code in a transaction that
//! is then rolled back, so it can't disagree with the real run, and the keys
//! it shows come from the real counter.
//!
//! Neither tool takes a top-level argument the MCP middleware resolves
//! besides projectId. Task lists, parents and expectedVersions keys are
//! resolved here with `tasks::resolve_tasks`, which only finds tasks in
//! projects the caller can reach, and the project is taken from projectId or
//! from the tasks themselves, never from `principal.project_id` alone.
//!
//! Both are composed from the shared commands: tasks are created through
//! `work_item_commands::create_work_item`, changed through
//! `update_locked_work_item`, and archived or restored through `set_archived`,
//! so each task gets one version bump, one activity, one audit row and one
//! outbox event per call, exactly as a single write would.

use std::collections::{BTreeMap, HashMap};

use axum::http::StatusCode;
use chrono::NaiveDate;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, READ_ONLY_MESSAGE, ToolResult, accessible_project_ids,
        archived_task_message, insert_audit, parse_id, project_is_chosen, public_url,
        version_conflict_message,
    },
    task_drafts::{self, NewTask, ParentChoice},
    tasks::{resolve_assignee, resolve_task, resolve_tasks, scoped},
    workflow::Workflow,
    writes::{
        LABEL_COUNT_MESSAGE, PRIORITY_MESSAGE, REQUEST_ID_MESSAGE, STATUS_MESSAGE, TYPE_MESSAGE,
        WriteAccess, check_access_for, check_write_access, join_and, mcp_actor, moved_away_message,
        parse_request_id, valid_priority,
    },
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope},
    policy::Action,
    work_item_commands::{
        ArchiveCommand, ArchiveOutcome, AuditVersionMetadata, CommandActor, LockedWorkItem,
        TaskFieldChanges, UpdateWorkItemCommand, UpdateWorkItemOutcome, lock_work_items,
        set_archived, update_locked_work_item,
    },
    work_item_labels::{self, LabelDelta, MAX_LABELS_PER_TASK, echo},
};

/// The most tasks one task_create_many call creates.
const MAX_CREATE: usize = 100;
/// The most tasks one task_update_many call changes.
const MAX_UPDATE: usize = 200;
/// The most problems one rejected batch lists.
const MAX_LISTED_PROBLEMS: usize = 100;
/// The most notes one result carries.
const MAX_NOTES: usize = 20;

// ---------------------------------------------------------------------------
// Results

/// What a batch did. Batches need a list of problems, which the shared
/// ToolResult has no room for, so they get their own result, as replies and
/// approvals do. Only the value is stored for replays, never this.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum BatchResult<T> {
    /// Done; with preview, what would be done.
    Ok {
        value: T,
    },
    /// Nothing changed. Each problem names the task and what to do; fix them
    /// and send the whole batch again (the same clientRequestId is fine).
    Rejected {
        message: String,
        problems: Vec<BatchProblem>,
    },
    NotFound {
        message: String,
    },
    InvalidRequest {
        field: String,
        message: String,
    },
    PermissionDenied {
        message: String,
    },
    Conflict {
        message: String,
    },
    TemporarilyUnavailable {
        message: String,
    },
}

/// Why one task in a batch can't go ahead.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchProblem {
    /// Where the task is in the list you sent, counting from 0.
    pub index: u32,
    /// The task's ref, when you gave it one.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The task's key, for an existing task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The argument to fix.
    pub field: String,
    pub message: String,
    /// The task's version now, when it changed since you read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<i64>,
}

impl BatchProblem {
    pub(super) fn new(index: usize, field: &str, message: impl Into<String>) -> Self {
        Self {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            reference: None,
            task: None,
            field: field.to_owned(),
            message: message.into(),
            current_version: None,
        }
    }

    pub(super) fn with_ref(mut self, reference: Option<String>) -> Self {
        self.reference = reference;
        self
    }

    fn with_task(mut self, key: &str) -> Self {
        self.task = Some(key.to_owned());
        self
    }
}

impl<T> BatchResult<T> {
    pub(super) fn denied(message: impl Into<String>) -> Self {
        Self::PermissionDenied {
            message: message.into(),
        }
    }

    fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    /// The same mapping as `ToolResult::from_error`, plus what a reused
    /// clientRequestId means for a batch.
    fn from_error(error: AppError, operation: &'static str) -> Self {
        match error {
            AppError::Conflict("idempotency_key", "payload mismatch") => Self::conflict(
                "That clientRequestId was already used for a different batch. Send that batch unchanged to get its result, or use a new clientRequestId.",
            ),
            AppError::Conflict("idempotency_key", "command in progress") => {
                Self::conflict("That batch is still being applied. Try again in a moment.")
            }
            error => match ToolResult::<()>::from_error(error, operation) {
                ToolResult::Ok { .. } | ToolResult::TemporarilyUnavailable { .. } => {
                    Self::TemporarilyUnavailable {
                        message: "Agent Workspace couldn't finish that, and nothing changed. Try again shortly.".to_owned(),
                    }
                }
                ToolResult::NotFound { message } => Self::NotFound { message },
                ToolResult::InvalidRequest { field, message } => {
                    Self::InvalidRequest { field, message }
                }
                ToolResult::PermissionDenied { message } => Self::PermissionDenied { message },
                ToolResult::Conflict { message } => Self::Conflict { message },
            },
        }
    }

    /// What check_write_access answered, for a batch in `project_key`.
    fn refused_access(access: WriteAccess, project_key: &str, doing: &str) -> Option<Self> {
        match access {
            WriteAccess::Allowed => None,
            WriteAccess::ReadOnly => Some(Self::denied(READ_ONLY_MESSAGE)),
            WriteAccess::NotFound => Some(Self::not_found(
                "That project wasn't found, or you aren't a member.",
            )),
            WriteAccess::ProjectArchived => Some(Self::conflict(ARCHIVED_PROJECT_MESSAGE)),
            WriteAccess::Denied => Some(Self::denied(format!(
                "Your role in {project_key} doesn't allow {doing} tasks. Ask a project owner to make you a member."
            ))),
        }
    }
}

/// "tasks at 1, 3 and 4", counting from 0.
fn indexes(problems: &[BatchProblem]) -> (usize, String) {
    let mut seen = Vec::new();
    for problem in problems {
        if !seen.contains(&problem.index) {
            seen.push(problem.index);
        }
    }
    let count = seen.len();
    let listed = seen
        .iter()
        .take(MAX_LISTED_PROBLEMS)
        .map(u32::to_string)
        .collect::<Vec<_>>();
    (count, join_and(&listed))
}

/// Keeps the first MAX_LISTED_PROBLEMS problems; the message says how many
/// more there were.
fn capped(mut problems: Vec<BatchProblem>) -> (Vec<BatchProblem>, String) {
    if problems.len() <= MAX_LISTED_PROBLEMS {
        return (problems, String::new());
    }
    let more = problems.len() - MAX_LISTED_PROBLEMS;
    problems.truncate(MAX_LISTED_PROBLEMS);
    (problems, format!(" …and {more} more problems not listed."))
}

fn create_rejected<T>(total: usize, mut problems: Vec<BatchProblem>) -> BatchResult<T> {
    problems.sort_by_key(|problem| problem.index);
    let (count, at) = indexes(&problems);
    let (problems, more) = capped(problems);
    let (tasks, have) = if count == 1 {
        ("task", "has a problem")
    } else {
        ("tasks", "have problems")
    };
    BatchResult::Rejected {
        message: format!(
            "Nothing was created: {count} of {total} {tasks} {have} (at {at}, counting from 0). Fix them and send the whole batch again; the same clientRequestId is fine.{more}"
        ),
        problems,
    }
}

/// Tasks a status change can't reach in one step, grouped by where they are.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TransitionGroup {
    from: String,
    to: String,
    /// Every status on the shortest way, `from` first and `to` last; None
    /// when `to` can't be reached.
    path: Option<Vec<String>>,
    keys: Vec<String>,
}

/// The message for an update nothing came of: one line per group of tasks
/// in the same status the change can't reach directly, then one line per
/// other problem.
fn update_rejected_message(
    total: usize,
    problems: &[BatchProblem],
    groups: &[TransitionGroup],
) -> String {
    let (count, _) = indexes(problems);
    let tasks = if count == 1 { "task" } else { "tasks" };
    let mut lines = vec![format!(
        "Nothing changed: {count} of {total} {tasks} can't take this change."
    )];
    for group in groups {
        let names = join_and(&group.keys);
        let (verb, them) = if group.keys.len() == 1 {
            ("is", "it")
        } else {
            ("are", "them")
        };
        lines.push(match &group.path {
            Some(path) => format!(
                "{names} {verb} in {}. The shortest way to {} is {}: run task_update_many once per step, or leave {them} out.",
                group.from,
                group.to,
                path.join(" → ")
            ),
            None => format!(
                "{names} {verb} in {}, and {} can't be reached from there (see workflow_get). Leave {them} out.",
                group.from, group.to
            ),
        });
    }
    for problem in problems.iter().filter(|problem| problem.field != "status") {
        lines.push(match &problem.task {
            Some(key) if !problem.message.starts_with(key.as_str()) => {
                format!("{key}: {}", problem.message)
            }
            _ => problem.message.clone(),
        });
    }
    let listed = lines.len().min(MAX_LISTED_PROBLEMS + 1);
    let more = lines.len() - listed;
    lines.truncate(listed);
    if more > 0 {
        lines.push(format!("…and {more} more."));
    }
    lines.join("\n")
}

fn update_rejected<T>(
    total: usize,
    problems: Vec<BatchProblem>,
    groups: &[TransitionGroup],
) -> BatchResult<T> {
    let message = update_rejected_message(total, &problems, groups);
    let (problems, _) = capped(problems);
    BatchResult::Rejected { message, problems }
}

// ---------------------------------------------------------------------------
// task_create_many

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCreateManyArguments {
    /// The project: ID, link, or key such as "AWD" (see projects_list).
    /// Required for apps connected for a whole account; agents and
    /// single-project connections default to their own project.
    pub project_id: Option<String>,
    /// 1 to 100 tasks, created in this order: parents before their subtasks.
    pub tasks: Vec<NewTask>,
    /// true checks everything and shows what would be created, keys
    /// included, without creating anything.
    pub preview: Option<bool>,
    /// true creates labels the tasks name that the project doesn't have yet
    /// (listed in labelsCreated). Left out, an unknown label is a problem.
    pub create_missing_labels: Option<bool>,
    /// Required unless preview, such as plan-import-2026-10-01. Sending the
    /// same tasks with it again within 24 hours returns the first result
    /// instead of creating them twice.
    pub client_request_id: Option<String>,
}

/// What task_create_many made. It is stored for replays, so every field
/// added later must be optional.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateManyValue {
    /// True when nothing was created: this is what would be.
    pub preview: bool,
    pub project_key: String,
    /// In the order you sent them.
    pub tasks: Vec<CreatedTask>,
    /// Labels that didn't exist and were (or would be) created.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels_created: Vec<String>,
    /// Worth a look, but didn't stop the batch, such as an open task that
    /// already has the same title.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// This clientRequestId was already used for these tasks; these are the
    /// tasks it created then.
    #[serde(default)]
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreatedTask {
    /// Where it was in the list you sent, counting from 0.
    pub index: u32,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Left out in a preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Such as AWD-12. In a preview, the key it gets if nobody else creates
    /// a task first.
    pub key: String,
    pub title: String,
    pub status: String,
    #[serde(rename = "type")]
    pub task_type: String,
    /// The parent's key, for a subtask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    /// Who it's assigned to, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    pub criteria_count: u32,
    pub version: i64,
    /// Where people open it. Left out in a preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What a task_create_many request hash covers: the project and the tasks,
/// normalized, with existing parents resolved to IDs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateManyFingerprint<'a> {
    project_id: Uuid,
    create_missing_labels: bool,
    tasks: &'a [task_drafts::Draft],
}

pub(super) async fn task_create_many(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCreateManyArguments,
) -> BatchResult<CreateManyValue> {
    if !principal.can_write() {
        return BatchResult::denied(READ_ONLY_MESSAGE);
    }
    create_many(state, principal, arguments)
        .await
        .unwrap_or_else(|error| BatchResult::from_error(error, "task_create_many"))
}

async fn create_many(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCreateManyArguments,
) -> Result<BatchResult<CreateManyValue>, AppError> {
    let preview = arguments.preview == Some(true);
    let create_missing_labels = arguments.create_missing_labels == Some(true);
    let total = arguments.tasks.len();
    if !(1..=MAX_CREATE).contains(&total) {
        return Ok(BatchResult::invalid(
            "tasks",
            "Pass 1 to 100 tasks. Split a bigger plan into several calls, each with its own clientRequestId; later calls can use parentId with keys from earlier ones.",
        ));
    }
    let key = match (arguments.client_request_id.as_deref(), preview) {
        (Some(value), _) => match parse_request_id(value) {
            Some(key) => Some(key),
            None => return Ok(BatchResult::invalid("clientRequestId", REQUEST_ID_MESSAGE)),
        },
        (None, true) => None,
        (None, false) => {
            return Ok(BatchResult::invalid(
                "clientRequestId",
                "Pass a clientRequestId, such as plan-import-2026-10-01, so a retry can't create these tasks twice. Use up to 128 letters, digits, dashes, dots or underscores.",
            ));
        }
    };
    if arguments
        .project_id
        .as_deref()
        .is_some_and(|project| parse_id(project) != Some(principal.project_id))
    {
        return Ok(BatchResult::not_found(
            "That project wasn't found, or you can't see it.",
        ));
    }
    const PROJECT_MESSAGE: &str = "Say which project with projectId (see projects_list).";
    if !project_is_chosen(principal) {
        return Ok(BatchResult::invalid("projectId", PROJECT_MESSAGE));
    }
    let project_id = principal.project_id;
    let Some((project_key, direct)) = sqlx::query_as::<_, (String, bool)>(
        "SELECT project_key, kind = 'direct' FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(BatchResult::not_found(
            "That project wasn't found, or you can't see it.",
        ));
    };
    // Tasks never go in the hidden Messages space, where nobody would see
    // them.
    if direct {
        return Ok(BatchResult::invalid("projectId", PROJECT_MESSAGE));
    }

    let (mut drafts, mut problems) = task_drafts::check_shapes(&arguments.tasks);
    // Existing parents, found only among projects the caller can reach, and
    // only in this project. Another project's task is never named.
    let mut written = drafts
        .iter()
        .filter_map(|draft| draft.parent_written.clone())
        .collect::<Vec<_>>();
    written.sort();
    written.dedup();
    if !written.is_empty() {
        let found = resolve_tasks(state, principal, &written).await?;
        let parents = written.iter().zip(found).collect::<HashMap<_, _>>();
        for (index, draft) in drafts.iter_mut().enumerate() {
            let Some(parent) = draft.parent_written.clone() else {
                continue;
            };
            match parents.get(&parent) {
                Some(Some((parent_id, parent_project))) if *parent_project == project_id => {
                    draft.parent = ParentChoice::Existing(*parent_id);
                }
                _ => problems.push(
                    BatchProblem::new(
                        index,
                        "parentId",
                        format!(
                            "Parent {} wasn't found in {project_key}. A subtask must be in the same project as its parent.",
                            echo(&parent)
                        ),
                    )
                    .with_ref(draft.reference.clone()),
                ),
            }
        }
    }

    let mut transaction = state.pool.begin().await?;
    if let Some(refused) = BatchResult::refused_access(
        check_access_for(&mut transaction, principal, Action::WorkItemCreate).await?,
        &project_key,
        "creating",
    ) {
        return Ok(refused);
    }
    // A batch with problems already is refused below without being stored,
    // so it reserves nothing; its names are still checked, so every problem
    // comes back at once.
    let reservation = match &key {
        Some(key) if !preview && problems.is_empty() => {
            let scope = resource_bound_scope("mcp_work_items", project_id, "create_many")?;
            let hash = RequestHash::for_value(&CreateManyFingerprint {
                project_id,
                create_missing_labels,
                tasks: &drafts,
            })?;
            match reserve(&mut transaction, principal.actor_id, &scope, key, hash).await? {
                IdempotencyOutcome::Replay(replay) => {
                    transaction.commit().await?;
                    let mut value = replay.decode::<CreateManyValue>()?;
                    value.idempotent_replay = true;
                    return Ok(BatchResult::Ok { value });
                }
                IdempotencyOutcome::Reserved(reservation) => Some(reservation),
            }
        }
        _ => None,
    };

    // Every refusal from here returns before the commit, so nothing is
    // created, no task numbers are used up, and the reservation rolls back:
    // the corrected batch can reuse the clientRequestId.
    let workflow = Workflow::load(&mut transaction, project_id).await?;
    let prepared = task_drafts::prepare(
        &mut transaction,
        principal,
        &workflow,
        &drafts,
        create_missing_labels,
        &mut problems,
    )
    .await?;
    if !problems.is_empty() {
        transaction.rollback().await?;
        return Ok(create_rejected(total, problems));
    }
    let notes = title_notes(&mut transaction, project_id, &prepared.tasks).await?;
    let request_id = key
        .as_ref()
        .map_or_else(|| Uuid::now_v7().to_string(), |key| key.as_str().to_owned());
    let actor = mcp_actor(principal, request_id);
    let created = match task_drafts::create(&mut transaction, &actor, &prepared.tasks).await? {
        Ok(created) => created,
        Err(problem) => {
            transaction.rollback().await?;
            return Ok(create_rejected(total, vec![problem]));
        }
    };

    let assignee_ids = prepared
        .tasks
        .iter()
        .filter_map(|task| task.assignee)
        .collect::<Vec<_>>();
    let assignee_names = if assignee_ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT actor_id, display_name FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)",
        )
        .bind(project_id)
        .bind(&assignee_ids)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>()
    };
    let keys = created
        .iter()
        .map(|row| format!("{project_key}-{}", row.number))
        .collect::<Vec<_>>();
    let mut tasks = Vec::with_capacity(created.len());
    for (task, row) in prepared.tasks.iter().zip(&created) {
        let mut labels = task
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>();
        labels.sort_by_key(|name| name.to_lowercase());
        tasks.push(CreatedTask {
            index: u32::try_from(task.index).unwrap_or(u32::MAX),
            reference: task.reference.clone(),
            id: Some(row.work_item_id.to_string()),
            key: keys[task.index].clone(),
            title: task.title.clone(),
            status: workflow.status_name(task.status_id).to_owned(),
            task_type: workflow.type_name(task.type_id).to_owned(),
            parent_key: match task.parent {
                ParentChoice::None => None,
                ParentChoice::InBatch(at) => keys.get(at).cloned(),
                ParentChoice::Existing(id) => prepared.parent_keys.get(&id).cloned(),
            },
            assignee: task
                .assignee
                .and_then(|id| assignee_names.get(&id).cloned()),
            labels,
            criteria_count: u32::try_from(task.acceptance_criteria.len()).unwrap_or(u32::MAX),
            version: 1,
            url: Some(public_url(
                state,
                &format!("/projects/{project_id}/work-items/{}", row.work_item_id),
            )),
        });
    }
    let mut value = CreateManyValue {
        preview,
        project_key,
        tasks,
        labels_created: prepared.labels_created,
        notes,
        idempotent_replay: false,
    };
    insert_audit(
        &mut transaction,
        principal,
        project_id,
        "work_item.mcp_batch_created",
        "project",
        project_id,
        serde_json::json!({
            "clientRequestId": key.as_ref().map(|key| key.as_str()),
            "count": created.len(),
            "workItemIds": created.iter().map(|row| row.work_item_id).collect::<Vec<_>>(),
            "labelsCreated": value.labels_created,
        }),
    )
    .await?;
    if preview {
        // Everything above was written for real so the preview matches the
        // real run; none of it stays.
        transaction.rollback().await?;
        for task in &mut value.tasks {
            task.id = None;
            task.url = None;
        }
        return Ok(BatchResult::Ok { value });
    }
    if let Some(reservation) = &reservation {
        complete(&mut transaction, reservation, StatusCode::CREATED, &value).await?;
    }
    transaction.commit().await?;
    Ok(BatchResult::Ok { value })
}

/// One task a plan import creates (roadmap W10, AI3).
#[derive(Clone, Debug)]
pub(crate) struct ImportTask {
    pub reference: String,
    pub title: String,
    pub description: String,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub criteria: Vec<String>,
    /// An earlier task in the same call, by its reference.
    pub parent_in_batch: Option<String>,
    /// A task an earlier call created.
    pub parent_existing: Option<Uuid>,
}

/// Creates up to 100 tasks for a plan import as `actor`, in the caller's
/// transaction, through the same checks and commands as task_create_many,
/// creating missing labels. The caller has locked the actor's membership
/// and checked they may edit tasks. Returns the new tasks' ids in order, or
/// the first problems as one sentence; on a problem the caller rolls back.
pub(crate) async fn create_for_import(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    tasks: &[ImportTask],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    if !(1..=MAX_CREATE).contains(&tasks.len()) {
        return Ok(Err("A batch holds 1 to 100 tasks.".to_owned()));
    }
    let principal = McpPrincipal {
        credential_id: Uuid::nil(),
        mcp_credential_id: None,
        oauth_grant_id: None,
        actor_id: actor.actor_id,
        actor_kind: actor.actor_kind,
        project_id: actor.project_id,
        external_agent_connection_id: None,
        client_name: "Agent Workspace AI".to_owned(),
        scopes: Vec::new(),
        authentication_class: "ai_background",
        account_wide: false,
        project_explicit: true,
    };
    let new_tasks = tasks
        .iter()
        .map(|task| NewTask {
            reference: Some(task.reference.clone()),
            title: task.title.clone(),
            description: Some(task.description.clone()),
            priority: task.priority.clone(),
            status: None,
            task_type: None,
            assignee: None,
            labels: task.labels.clone(),
            acceptance_criteria: task.criteria.clone(),
            parent_ref: task.parent_in_batch.clone(),
            parent_id: None,
        })
        .collect::<Vec<_>>();
    let (mut drafts, mut problems) = task_drafts::check_shapes(&new_tasks);
    for (draft, task) in drafts.iter_mut().zip(tasks) {
        if let Some(parent) = task.parent_existing {
            draft.parent = ParentChoice::Existing(parent);
        }
    }
    if problems.is_empty() {
        let workflow = Workflow::load(transaction, actor.project_id).await?;
        let prepared = task_drafts::prepare(
            transaction,
            &principal,
            &workflow,
            &drafts,
            true,
            &mut problems,
        )
        .await?;
        if problems.is_empty() {
            return Ok(
                match task_drafts::create(transaction, actor, &prepared.tasks).await? {
                    Ok(created) => Ok(created.into_iter().map(|row| row.work_item_id).collect()),
                    Err(problem) => Err(problem.message),
                },
            );
        }
    }
    Ok(Err(problems
        .iter()
        .take(3)
        .map(|problem| match &problem.reference {
            Some(reference) => format!("{reference}: {}", problem.message),
            None => problem.message.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")))
}

/// Notes on titles: tasks in the list with the same title, and open tasks
/// in the project that already have one of the titles.
async fn title_notes(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    tasks: &[task_drafts::PreparedTask],
) -> Result<Vec<String>, AppError> {
    let mut notes = Vec::new();
    let mut first: HashMap<String, usize> = HashMap::new();
    for task in tasks {
        let title = task.title.to_lowercase();
        match first.get(&title) {
            Some(at) => notes.push(format!(
                "The tasks at {at} and {} have the same title, \"{}\".",
                task.index,
                echo(&task.title)
            )),
            None => {
                first.insert(title, task.index);
            }
        }
    }
    let titles = first.into_keys().collect::<Vec<_>>();
    let existing = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT project.project_key || '-' || item.number::text, item.title
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL AND lower(item.title) = ANY($2)
        ORDER BY item.number
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(&titles)
    .bind(i64::try_from(MAX_NOTES).unwrap_or(20))
    .fetch_all(&mut **transaction)
    .await?;
    for (key, title) in existing {
        notes.push(format!("{key} already has the title \"{}\".", echo(&title)));
    }
    notes.truncate(MAX_NOTES);
    Ok(notes)
}

// ---------------------------------------------------------------------------
// task_update_many

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskUpdateManyArguments {
    /// Optional: the project the tasks are all in (ID, link, or key such as
    /// "AWD"). Left out, it's the project of the tasks.
    pub project_id: Option<String>,
    /// 1 to 200 tasks, all in one project: IDs, links, or keys such as
    /// AWD-12.
    pub task_ids: Vec<String>,
    /// The version you last read for some or all of them, keyed as you wrote
    /// them in taskIds (or by ID). If one has changed since, nothing does.
    pub expected_versions: Option<BTreeMap<String, i64>>,
    /// Move them to this status, by name (see workflow_get). Each must be
    /// allowed to move there directly; they go to the bottom of its column in
    /// the order given.
    pub status: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    pub priority: Option<String>,
    /// "me", "none", an @handle, or an actor ID.
    pub assignee: Option<String>,
    /// true leaves them unassigned.
    pub clear_assignee: Option<bool>,
    /// A type by name, such as "Bug".
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    /// Labels to put on them, by name or ID (at most 20). They must exist;
    /// create one with label_write.
    pub add_labels: Option<Vec<String>>,
    /// Labels to take off them, by name or ID (at most 20).
    pub remove_labels: Option<Vec<String>>,
    /// Put them in this active milestone, by name or ID (see
    /// milestone_list); "none" takes them out of theirs.
    pub milestone: Option<String>,
    /// Their due date: YYYY-MM-DD, or "" to clear it.
    pub due_date: Option<String>,
    /// true archives them (their claims are released); false restores them.
    /// Pass it on its own.
    pub archived: Option<bool>,
    /// true checks everything and shows the result without changing
    /// anything.
    pub preview: Option<bool>,
    /// A stable ID, so a retry within 24 hours returns the first result
    /// instead of a conflict with its own first attempt.
    pub client_request_id: Option<String>,
}

/// What task_update_many did. It is stored for replays, so every field
/// added later must be optional.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateManyValue {
    /// True when nothing was changed: this is what would be.
    pub preview: bool,
    pub changed_count: u32,
    pub unchanged_count: u32,
    /// In the order you sent them.
    pub tasks: Vec<UpdatedTask>,
    /// Worth knowing, such as a task someone else has claimed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// This clientRequestId was already used for this change; this is what
    /// it did then.
    #[serde(default)]
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdatedTask {
    /// Where it was in taskIds, counting from 0.
    pub index: u32,
    pub id: String,
    pub key: String,
    /// Its version now (in a preview, what it would be).
    pub version: i64,
    /// Empty when it was already like that.
    #[serde(default)]
    pub changed_fields: Vec<String>,
    /// Its status, by name.
    pub status: String,
    pub archived: bool,
    pub url: String,
}

/// What a task_update_many request hash covers: the project, the tasks as
/// resolved IDs (so a retry that spells a task as a key instead of an ID
/// still matches), and the change, normalized.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManyFingerprint<'a> {
    project_id: Uuid,
    task_ids: &'a [Uuid],
    expected: &'a BTreeMap<Uuid, i64>,
    status: Option<&'a str>,
    priority: Option<&'a str>,
    assignee: Option<&'a str>,
    clear_assignee: bool,
    task_type: Option<&'a str>,
    add_labels: &'a [String],
    remove_labels: &'a [String],
    archived: Option<bool>,
    // Added later: left out when absent, so older calls hash as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<Option<NaiveDate>>,
}

/// The change one task_update_many call asks for, checked.
struct Change {
    status: Option<String>,
    priority: Option<String>,
    assignee: Option<String>,
    clear_assignee: bool,
    task_type: Option<String>,
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    archived: Option<bool>,
    /// As written; "none" or "" takes them out of their milestone.
    milestone: Option<String>,
    due_date: Option<Option<NaiveDate>>,
}

impl Change {
    /// The request checks that need no database.
    fn read(arguments: &TaskUpdateManyArguments) -> Result<Self, (&'static str, String)> {
        let trimmed = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let labels = |value: &Option<Vec<String>>| {
            value
                .iter()
                .flatten()
                .map(|label| label.trim().to_owned())
                .filter(|label| !label.is_empty())
                .collect::<Vec<_>>()
        };
        let change = Self {
            status: trimmed(&arguments.status),
            priority: trimmed(&arguments.priority),
            assignee: trimmed(&arguments.assignee),
            clear_assignee: arguments.clear_assignee == Some(true),
            task_type: trimmed(&arguments.task_type),
            add_labels: labels(&arguments.add_labels),
            remove_labels: labels(&arguments.remove_labels),
            archived: arguments.archived,
            milestone: arguments
                .milestone
                .as_deref()
                .map(|value| value.trim().to_owned()),
            due_date: match arguments.due_date.as_deref() {
                Some(value) => match super::planning::date_change(value) {
                    Some(date) => Some(date),
                    None => return Err(("dueDate", super::planning::DATE_MESSAGE.to_owned())),
                },
                None => None,
            },
        };
        let changes_fields = change.status.is_some()
            || change.priority.is_some()
            || change.assignee.is_some()
            || change.clear_assignee
            || change.task_type.is_some()
            || !change.add_labels.is_empty()
            || !change.remove_labels.is_empty()
            || change.milestone.is_some()
            || change.due_date.is_some();
        if !changes_fields && change.archived.is_none() {
            return Err((
                "status",
                "Say what to change: status, priority, assignee, clearAssignee, type, addLabels, removeLabels, milestone, dueDate, or archived.".to_owned(),
            ));
        }
        if change
            .milestone
            .as_deref()
            .is_some_and(|value| value.len() > 200)
        {
            return Err(("milestone", super::writes::MILESTONE_MESSAGE.to_owned()));
        }
        if changes_fields && change.archived.is_some() {
            return Err((
                "archived",
                "Pass archived on its own: archive or restore the tasks first, then send the other changes in a second call.".to_owned(),
            ));
        }
        if change.assignee.is_some() && change.clear_assignee {
            return Err((
                "assignee",
                "Use assignee or clearAssignee, not both.".to_owned(),
            ));
        }
        if change
            .priority
            .as_deref()
            .is_some_and(|priority| !valid_priority(priority))
        {
            return Err(("priority", PRIORITY_MESSAGE.to_owned()));
        }
        if change
            .status
            .as_deref()
            .is_some_and(|value| value.len() > 80)
        {
            return Err(("status", STATUS_MESSAGE.to_owned()));
        }
        if change
            .task_type
            .as_deref()
            .is_some_and(|value| value.len() > 80)
        {
            return Err(("type", TYPE_MESSAGE.to_owned()));
        }
        if change
            .assignee
            .as_deref()
            .is_some_and(|value| value.len() > 200)
        {
            return Err((
                "assignee",
                "Pass the assignee as \"me\", \"none\", an @handle, or an actor ID.".to_owned(),
            ));
        }
        for (field, list) in [
            ("addLabels", &change.add_labels),
            ("removeLabels", &change.remove_labels),
        ] {
            if list.len() > MAX_LABELS_PER_TASK {
                return Err((field, LABEL_COUNT_MESSAGE.to_owned()));
            }
        }
        let removed = change
            .remove_labels
            .iter()
            .map(|label| label_key(label))
            .collect::<Vec<_>>();
        if let Some(both) = change
            .add_labels
            .iter()
            .find(|label| removed.contains(&label_key(label)))
        {
            return Err((
                "removeLabels",
                format!(
                    "\"{}\" is in both addLabels and removeLabels. Pick one.",
                    echo(both)
                ),
            ));
        }
        Ok(change)
    }

    fn touches_claims(&self) -> bool {
        self.status.is_some()
            || self.assignee.is_some()
            || self.clear_assignee
            || self.archived == Some(true)
    }
}

pub(super) async fn task_update_many(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskUpdateManyArguments,
) -> BatchResult<UpdateManyValue> {
    if !principal.can_write() {
        return BatchResult::denied(READ_ONLY_MESSAGE);
    }
    update_many(state, principal, arguments)
        .await
        .unwrap_or_else(|error| BatchResult::from_error(error, "task_update_many"))
}

/// A task's status, archived state and version as the batch leaves it.
struct Outcome {
    version: i64,
    changed_fields: Vec<String>,
    status_id: Uuid,
    archived: bool,
}

async fn update_many(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskUpdateManyArguments,
) -> Result<BatchResult<UpdateManyValue>, AppError> {
    let preview = arguments.preview == Some(true);
    let total = arguments.task_ids.len();
    if !(1..=MAX_UPDATE).contains(&total) {
        return Ok(BatchResult::invalid(
            "taskIds",
            "Pass 1 to 200 tasks. Split a bigger change into several calls.",
        ));
    }
    let change = match Change::read(&arguments) {
        Ok(change) => change,
        Err((field, message)) => return Ok(BatchResult::invalid(field, message)),
    };
    let written_versions = arguments.expected_versions.clone().unwrap_or_default();
    if written_versions.len() > MAX_UPDATE {
        return Ok(BatchResult::invalid(
            "expectedVersions",
            "Pass at most one expected version per task.",
        ));
    }
    if let Some((written, _)) = written_versions.iter().find(|(_, version)| **version < 1) {
        return Ok(BatchResult::invalid(
            "expectedVersions",
            format!(
                "The version for {} must be the one you last read, from 1 up (see task_open or task_list).",
                echo(written)
            ),
        ));
    }
    let key = match arguments.client_request_id.as_deref() {
        Some(value) => match parse_request_id(value) {
            Some(key) => Some(key),
            None => return Ok(BatchResult::invalid("clientRequestId", REQUEST_ID_MESSAGE)),
        },
        None => None,
    };
    if arguments
        .project_id
        .as_deref()
        .is_some_and(|project| parse_id(project) != Some(principal.project_id))
    {
        return Ok(BatchResult::not_found(
            "That project wasn't found, or you can't see it.",
        ));
    }

    // The tasks, among projects the caller can reach.
    let found = resolve_tasks(state, principal, &arguments.task_ids).await?;
    let found_ids = found
        .iter()
        .flatten()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let keys = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text
        FROM work_items item JOIN projects project ON project.id = item.project_id
        WHERE item.id = ANY($1)
        "#,
    )
    .bind(&found_ids)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();
    let key_of = |id: &Uuid| keys.get(id).cloned().unwrap_or_else(|| id.to_string());
    let named_project = if principal.project_explicit {
        Some((
            principal.project_id,
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_optional(&state.pool)
                .await?
                .unwrap_or_else(|| "that project".to_owned()),
        ))
    } else {
        None
    };
    let mut project = named_project.as_ref().map(|(id, _)| *id);
    let mut problems = Vec::new();
    let mut first_at: HashMap<Uuid, usize> = HashMap::new();
    for (index, (written, found)) in arguments.task_ids.iter().zip(&found).enumerate() {
        let Some((task_id, task_project)) = found else {
            problems.push(BatchProblem::new(
                index,
                "taskIds",
                format!(
                    "Task {} wasn't found, or you can't see it.",
                    echo(written.trim())
                ),
            ));
            continue;
        };
        let key = key_of(task_id);
        if let Some(first) = first_at.get(task_id) {
            problems.push(
                BatchProblem::new(
                    index,
                    "taskIds",
                    format!("{key} is listed twice (also at {first})."),
                )
                .with_task(&key),
            );
            continue;
        }
        first_at.insert(*task_id, index);
        let here = *project.get_or_insert(*task_project);
        if *task_project != here {
            let message = match &named_project {
                Some((_, project_key)) => format!("{key} isn't in {project_key}."),
                None => format!(
                    "All tasks must be in one project; {key} is in {}.",
                    key.rsplit_once('-')
                        .map_or("another project", |(prefix, _)| prefix)
                ),
            };
            problems.push(BatchProblem::new(index, "taskIds", message).with_task(&key));
        }
    }
    if !problems.is_empty() {
        return Ok(update_rejected(total, problems, &[]));
    }
    let project_id = project.ok_or(AppError::Internal("a batch has a task"))?;
    let task_ids = found
        .iter()
        .flatten()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();

    // expectedVersions, keyed as the call wrote taskIds or by any reference
    // to a listed task.
    let mut expected: BTreeMap<Uuid, i64> = BTreeMap::new();
    for (written, version) in &written_versions {
        let task_id = match arguments.task_ids.iter().position(|entry| entry == written) {
            Some(index) => Some(task_ids[index]),
            None => resolve_task(state, principal, written)
                .await?
                .map(|(id, _)| id)
                .filter(|id| task_ids.contains(id)),
        };
        let Some(task_id) = task_id else {
            return Ok(BatchResult::invalid(
                "expectedVersions",
                format!(
                    "expectedVersions names {}, which isn't in taskIds.",
                    echo(written)
                ),
            ));
        };
        if expected
            .insert(task_id, *version)
            .is_some_and(|other| other != *version)
        {
            return Ok(BatchResult::invalid(
                "expectedVersions",
                format!(
                    "expectedVersions gives {} two different versions.",
                    key_of(&task_id)
                ),
            ));
        }
    }
    // Who may hear where a moved task went: only callers who can reach it.
    let reachable = if change.archived == Some(false) {
        accessible_project_ids(state, principal).await?
    } else {
        Vec::new()
    };

    let scoped = scoped(principal, project_id);
    let project_key = keys
        .get(&task_ids[0])
        .and_then(|key| key.rsplit_once('-'))
        .map_or_else(
            || "this project".to_owned(),
            |(prefix, _)| prefix.to_owned(),
        );
    let mut transaction = state.pool.begin().await?;
    // Archiving is its own permission.
    let access = if change.archived.is_some() {
        check_access_for(&mut transaction, &scoped, Action::WorkItemArchive).await?
    } else {
        check_write_access(&mut transaction, &scoped).await?
    };
    if let Some(refused) = BatchResult::refused_access(access, &project_key, "changing") {
        return Ok(refused);
    }
    let reservation = match &key {
        Some(key) if !preview => {
            let scope = resource_bound_scope("mcp_work_items", project_id, "update_many")?;
            let lower = |value: &Option<String>| value.as_deref().map(str::to_lowercase);
            let normalized = |labels: &[String]| {
                labels
                    .iter()
                    .map(|label| label_key(label))
                    .collect::<Vec<_>>()
            };
            let (status, assignee, task_type, milestone) = (
                lower(&change.status),
                lower(&change.assignee),
                lower(&change.task_type),
                lower(&change.milestone),
            );
            let (add_labels, remove_labels) = (
                normalized(&change.add_labels),
                normalized(&change.remove_labels),
            );
            let hash = RequestHash::for_value(&UpdateManyFingerprint {
                project_id,
                task_ids: &task_ids,
                expected: &expected,
                status: status.as_deref(),
                priority: change.priority.as_deref(),
                assignee: assignee.as_deref(),
                clear_assignee: change.clear_assignee,
                task_type: task_type.as_deref(),
                add_labels: &add_labels,
                remove_labels: &remove_labels,
                archived: change.archived,
                milestone: milestone.as_deref(),
                due_date: change.due_date,
            })?;
            match reserve(&mut transaction, principal.actor_id, &scope, key, hash).await? {
                IdempotencyOutcome::Replay(replay) => {
                    transaction.commit().await?;
                    let mut value = replay.decode::<UpdateManyValue>()?;
                    value.idempotent_replay = true;
                    return Ok(BatchResult::Ok { value });
                }
                IdempotencyOutcome::Reserved(reservation) => Some(reservation),
            }
        }
        _ => None,
    };

    // Every refusal from here returns before the commit, so nothing changes
    // and a reserved clientRequestId can be reused for the corrected call.
    // The change, resolved once for every task.
    let workflow = Workflow::load(&mut transaction, project_id).await?;
    let status_id = match change.status.as_deref().map(|value| workflow.status(value)) {
        Some(Ok(status)) => Some(status.id),
        Some(Err(message)) => return Ok(BatchResult::invalid("status", message)),
        None => None,
    };
    let type_id = match change
        .task_type
        .as_deref()
        .map(|value| workflow.work_type(value))
    {
        Some(Ok(kind)) => Some(kind.id),
        Some(Err(message)) => return Ok(BatchResult::invalid("type", message)),
        None => None,
    };
    let assignee = match change.assignee.as_deref() {
        Some(value) => {
            match resolve_assignee(&mut transaction, &scoped, project_id, value).await? {
                Ok(assignee) => Some(assignee),
                Err(problem) => return Ok(BatchResult::invalid("assignee", problem.message())),
            }
        }
        None if change.clear_assignee => Some(None),
        None => None,
    };
    let add_ids = match batch_labels(&mut transaction, project_id, &change.add_labels).await? {
        Ok(ids) => ids,
        Err(message) => return Ok(BatchResult::invalid("addLabels", message)),
    };
    let remove_ids = match batch_labels(&mut transaction, project_id, &change.remove_labels).await?
    {
        Ok(ids) => ids,
        Err(message) => return Ok(BatchResult::invalid("removeLabels", message)),
    };
    if let Some(both) = add_ids.iter().find(|id| remove_ids.contains(id)) {
        let name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM project_labels WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(both)
        .fetch_one(&mut *transaction)
        .await?;
        return Ok(BatchResult::invalid(
            "removeLabels",
            format!("\"{name}\" is in both addLabels and removeLabels. Pick one."),
        ));
    }
    let delta = LabelDelta {
        add: &add_ids,
        remove: &remove_ids,
    };
    let milestone_id = match change.milestone.as_deref() {
        Some(value) => {
            match super::planning::milestone_change(&mut transaction, project_id, value).await? {
                Some(milestone) => Some(milestone),
                None => {
                    return Ok(BatchResult::invalid(
                        "milestone",
                        super::writes::MILESTONE_MESSAGE,
                    ));
                }
            }
        }
        None => None,
    };

    // Lock every task, in id order so two batches never deadlock, then check
    // each against the change.
    let mut locked = lock_work_items(&mut transaction, project_id, &task_ids)
        .await?
        .into_iter()
        .map(|task| (task.id, task))
        .collect::<HashMap<_, _>>();
    let moved_to = if change.archived == Some(false) {
        sqlx::query_as::<_, (Uuid, Uuid, String)>(
            r#"
            SELECT alias.source_work_item_id, item.project_id,
                   project.project_key || '-' || item.number::text
            FROM work_item_key_aliases alias
            JOIN work_items item ON item.id = alias.work_item_id
            JOIN projects project ON project.id = item.project_id
            WHERE alias.source_work_item_id = ANY($1)
            "#,
        )
        .bind(&task_ids)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|(source, project, key)| (source, reachable.contains(&project).then_some(key)))
        .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let mut groups: Vec<TransitionGroup> = Vec::new();
    for (index, task_id) in task_ids.iter().enumerate() {
        let key = key_of(task_id);
        let Some(current) = locked.get(task_id) else {
            problems.push(
                BatchProblem::new(
                    index,
                    "taskIds",
                    format!("{key} wasn't found, or you can't see it."),
                )
                .with_task(&key),
            );
            continue;
        };
        // A task already archived (or restored) as asked is left as it is,
        // whatever version was expected, as the single archive answers.
        if change.archived == Some(current.archived) {
            continue;
        }
        if let Some(version) = expected.get(task_id)
            && *version != current.version
        {
            let mut problem = BatchProblem::new(
                index,
                "expectedVersions",
                version_conflict_message(&key, current.version),
            )
            .with_task(&key);
            problem.current_version = Some(current.version);
            problems.push(problem);
            continue;
        }
        if let Some(archived) = change.archived {
            if !archived && let Some(moved) = moved_to.get(task_id) {
                problems.push(
                    BatchProblem::new(
                        index,
                        "archived",
                        moved_away_message(&key, moved.as_deref()),
                    )
                    .with_task(&key),
                );
            }
            continue;
        }
        if current.archived {
            problems.push(
                BatchProblem::new(
                    index,
                    "archived",
                    format!(
                        "{} Restore it with task_update_many (archived: false) on its own, or leave it out.",
                        archived_task_message(&key)
                    ),
                )
                .with_task(&key),
            );
            continue;
        }
        if let Some(target) = status_id
            && let Some(message) = workflow.transition_problem(current.status_id, target)
        {
            problems.push(BatchProblem::new(index, "status", message).with_task(&key));
            let from = workflow.status_name(current.status_id).to_owned();
            let to = workflow.status_name(target).to_owned();
            match groups
                .iter_mut()
                .find(|group| group.from == from && group.to == to)
            {
                Some(group) => group.keys.push(key.clone()),
                None => groups.push(TransitionGroup {
                    path: workflow
                        .shortest_path(current.status_id, target)
                        .map(|path| {
                            std::iter::once(from.clone())
                                .chain(path.iter().map(|id| workflow.status_name(*id).to_owned()))
                                .collect()
                        }),
                    from,
                    to,
                    keys: vec![key.clone()],
                }),
            }
        }
        // Every task whose start date would fall after the new due date is
        // listed, not just the first one reached while applying.
        if let Some(Some(due)) = change.due_date
            && current.start_date.is_some_and(|start| start > due)
        {
            problems.push(
                BatchProblem::new(
                    index,
                    "dueDate",
                    format!(
                        "{key} starts after that due date. {}",
                        super::writes::DATES_MESSAGE
                    ),
                )
                .with_task(&key),
            );
        }
        if !delta.add.is_empty() {
            let plan =
                work_item_labels::plan(&mut transaction, project_id, *task_id, &delta).await?;
            if !plan.added.is_empty() && plan.resulting_count > MAX_LABELS_PER_TASK {
                problems.push(
                    BatchProblem::new(
                        index,
                        "addLabels",
                        format!(
                            "A task can have at most {MAX_LABELS_PER_TASK} labels; this change would give {key} {}. Remove some with removeLabels first, or leave it out.",
                            plan.resulting_count
                        ),
                    )
                    .with_task(&key),
                );
            }
        }
    }
    if !problems.is_empty() {
        transaction.rollback().await?;
        return Ok(update_rejected(total, problems, &groups));
    }

    let mut notes = Vec::new();
    if change.touches_claims() {
        let claims = sqlx::query_as::<_, (Uuid, String, String)>(
            r#"
            SELECT claim.work_item_id, membership.display_name,
                   to_char(claim.expires_at AT TIME ZONE 'UTC', 'HH24:MI')
            FROM work_item_claims claim
            JOIN project_memberships membership
              ON membership.project_id = claim.project_id AND membership.actor_id = claim.actor_id
            WHERE claim.project_id = $1 AND claim.work_item_id = ANY($2)
              AND claim.actor_id <> $3 AND claim.expires_at > now()
            "#,
        )
        .bind(project_id)
        .bind(&task_ids)
        .bind(principal.actor_id)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|(id, name, until)| (id, (name, until)))
        .collect::<HashMap<_, _>>();
        for task_id in &task_ids {
            if let Some((name, until)) = claims.get(task_id) {
                let key = key_of(task_id);
                notes.push(if change.archived == Some(true) {
                    format!("{key} was claimed by {name}; archiving released the claim.")
                } else {
                    format!(
                        "{key} is claimed by {name} until {until} UTC; they may still be working on it."
                    )
                });
            }
        }
    }
    let labels_before = if delta.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, Vec<Uuid>)>(
            r#"
            SELECT work_item_id, array_agg(label_id ORDER BY label_id)
            FROM work_item_labels
            WHERE project_id = $1 AND work_item_id = ANY($2)
            GROUP BY work_item_id
            "#,
        )
        .bind(project_id)
        .bind(&task_ids)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>()
    };

    // Apply, in the order given: tasks that change status go to the bottom
    // of the new column one after another, so they keep this order.
    let request_id = key
        .as_ref()
        .map_or_else(|| Uuid::now_v7().to_string(), |key| key.as_str().to_owned());
    let actor = mcp_actor(&scoped, request_id);
    let mut outcomes = Vec::with_capacity(task_ids.len());
    let mut previous = BTreeMap::new();
    for (index, task_id) in task_ids.iter().enumerate() {
        let current = locked
            .remove(task_id)
            .ok_or(AppError::Internal("a batch task is locked once"))?;
        let key = key_of(task_id);
        let before = serde_json::json!({
            "statusId": current.status_id,
            "priority": current.priority,
            "assigneeActorId": current.assignee_actor_id,
            "typeId": current.type_id,
            "archived": current.archived,
            "milestoneId": current.milestone_id,
            "dueDate": current.due_date,
        });
        let outcome = match change.archived {
            Some(archived) => {
                apply_archive(&mut transaction, &actor, current, archived, index, &key).await?
            }
            None => {
                apply_update(
                    &mut transaction,
                    &actor,
                    current,
                    UpdateWorkItemCommand {
                        expected_version: 0,
                        title: None,
                        description: None,
                        type_id,
                        status_id,
                        priority: change.priority.as_deref(),
                        assignee_actor_id: assignee,
                        parent_work_item_id: None,
                        board_placement: None,
                        labels: (!delta.is_empty()).then_some(delta),
                        fields: TaskFieldChanges {
                            milestone_id,
                            due_date: change.due_date,
                            ..TaskFieldChanges::default()
                        },
                        summary: "Work Item updated through MCP",
                        audit_action: "work_item.mcp_bulk_updated",
                        audit_version_metadata: AuditVersionMetadata::Include,
                    },
                    &workflow,
                    index,
                    &key,
                )
                .await?
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(problem) => {
                transaction.rollback().await?;
                return Ok(update_rejected(total, vec![problem], &[]));
            }
        };
        if !outcome.changed_fields.is_empty() {
            let mut before = before;
            if !delta.is_empty() {
                before["labelIds"] =
                    serde_json::json!(labels_before.get(task_id).cloned().unwrap_or_default());
            }
            previous.insert(task_id.to_string(), before);
        }
        outcomes.push(outcome);
    }

    if change.archived == Some(true) {
        let open = sqlx::query_as::<_, (Uuid, i64)>(
            r#"
            SELECT parent_work_item_id, count(*)
            FROM work_items
            WHERE project_id = $1 AND parent_work_item_id = ANY($2) AND archived_at IS NULL
            GROUP BY parent_work_item_id
            "#,
        )
        .bind(project_id)
        .bind(&task_ids)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
        for task_id in &task_ids {
            match open.get(task_id) {
                Some(1) => notes.push(format!(
                    "{} has an open subtask; it stays on the board.",
                    key_of(task_id)
                )),
                Some(count) => notes.push(format!(
                    "{} has {count} open subtasks; they stay on the board.",
                    key_of(task_id)
                )),
                None => {}
            }
        }
    }
    notes.truncate(MAX_NOTES);

    let changed_count = outcomes
        .iter()
        .filter(|outcome| !outcome.changed_fields.is_empty())
        .count();
    let tasks = task_ids
        .iter()
        .zip(outcomes)
        .enumerate()
        .map(|(index, (task_id, outcome))| UpdatedTask {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            id: task_id.to_string(),
            key: key_of(task_id),
            version: outcome.version,
            changed_fields: outcome.changed_fields,
            status: workflow.status_name(outcome.status_id).to_owned(),
            archived: outcome.archived,
            url: public_url(
                state,
                &format!("/projects/{project_id}/work-items/{task_id}"),
            ),
        })
        .collect::<Vec<_>>();
    let mut value = UpdateManyValue {
        preview,
        changed_count: u32::try_from(changed_count).unwrap_or(u32::MAX),
        unchanged_count: u32::try_from(total - changed_count).unwrap_or(u32::MAX),
        tasks,
        notes,
        idempotent_replay: false,
    };
    // One row for the whole call, holding what each changed task was, so a
    // mistaken bulk change can be undone by hand.
    insert_audit(
        &mut transaction,
        &scoped,
        project_id,
        "work_item.mcp_batch_updated",
        "project",
        project_id,
        serde_json::json!({
            "clientRequestId": key.as_ref().map(|key| key.as_str()),
            "count": total,
            "changed": changed_count,
            "change": {
                "statusId": status_id,
                "priority": change.priority,
                "assigneeActorId": assignee.flatten(),
                "clearAssignee": matches!(assignee, Some(None)),
                "typeId": type_id,
                "addLabelIds": add_ids,
                "removeLabelIds": remove_ids,
                "archived": change.archived,
                "milestoneId": milestone_id.flatten(),
                "clearMilestone": matches!(milestone_id, Some(None)),
                "dueDate": change.due_date.flatten(),
                "clearDueDate": matches!(change.due_date, Some(None)),
            },
            "previous": previous,
        }),
    )
    .await?;
    if preview {
        transaction.rollback().await?;
        value.preview = true;
        return Ok(BatchResult::Ok { value });
    }
    if let Some(reservation) = &reservation {
        complete(&mut transaction, reservation, StatusCode::OK, &value).await?;
    }
    transaction.commit().await?;
    Ok(BatchResult::Ok { value })
}

/// How two label names are compared: normalized, ignoring case.
fn label_key(label: &str) -> String {
    work_item_labels::normalize_name(label)
        .unwrap_or_else(|| label.to_owned())
        .to_lowercase()
}

/// Label IDs among the project's active labels, or the message naming what's
/// unknown or archived. task_update_many never creates labels: an unknown
/// name there is most likely a typo.
async fn batch_labels(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    references: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    if references.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let resolution = work_item_labels::resolve(transaction, project_id, references).await?;
    if !resolution.is_complete() {
        return Ok(Err(work_item_labels::unknown_message(
            transaction,
            project_id,
            &resolution,
        )
        .await?));
    }
    Ok(Ok(resolution.ids()))
}

/// Archives or restores one task of a batch through the one archive command
/// the web app and task_update use, which releases claims and refuses to
/// restore a moved task's original. Its caller has checked the version.
async fn apply_archive(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &crate::work_item_commands::CommandActor,
    current: LockedWorkItem,
    archived: bool,
    index: usize,
    key: &str,
) -> Result<Result<Outcome, BatchProblem>, AppError> {
    let (summary, audit_action) = if archived {
        (
            "Work Item archived through MCP",
            "work_item.mcp_bulk_archived",
        )
    } else {
        (
            "Work Item restored through MCP",
            "work_item.mcp_bulk_restored",
        )
    };
    let status_id = current.status_id;
    let outcome = set_archived(
        transaction,
        actor,
        current.id,
        ArchiveCommand {
            expected_version: current.version,
            archived,
            summary,
            audit_action,
        },
    )
    .await?;
    Ok(match outcome {
        ArchiveOutcome::Applied { version, .. } => Ok(Outcome {
            version,
            changed_fields: vec!["archived".to_owned()],
            status_id,
            archived,
        }),
        ArchiveOutcome::Unchanged(current) => Ok(Outcome {
            version: current.version,
            changed_fields: Vec::new(),
            status_id: current.status_id,
            archived: current.archived,
        }),
        ArchiveOutcome::VersionConflict(current) => Err(BatchProblem {
            current_version: Some(current.version),
            ..BatchProblem::new(
                index,
                "expectedVersions",
                version_conflict_message(key, current.version),
            )
            .with_task(key)
        }),
        ArchiveOutcome::MovedAway { .. } => {
            Err(BatchProblem::new(index, "archived", moved_away_message(key, None)).with_task(key))
        }
        ArchiveOutcome::NotFound => {
            return Err(AppError::Internal("a locked batch task vanished"));
        }
    })
}

/// Changes one task of a batch through the shared update command. Its
/// caller has checked the version, the archived state, the transition and
/// the label cap under the same locks, so a refusal here means the batch is
/// refused as a whole rather than half applied.
#[allow(clippy::too_many_arguments)]
async fn apply_update(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &crate::work_item_commands::CommandActor,
    current: LockedWorkItem,
    mut command: UpdateWorkItemCommand<'_>,
    workflow: &Workflow,
    index: usize,
    key: &str,
) -> Result<Result<Outcome, BatchProblem>, AppError> {
    command.expected_version = current.version;
    let (status_before, archived) = (current.status_id, current.archived);
    let requested_status = command.status_id;
    let problem =
        |field: &str, message: String| -> Result<Result<Outcome, BatchProblem>, AppError> {
            Ok(Err(BatchProblem::new(index, field, message).with_task(key)))
        };
    match update_locked_work_item(transaction, actor, current, command).await? {
        UpdateWorkItemOutcome::Applied {
            version,
            changed_fields,
            ..
        } => Ok(Ok(Outcome {
            version,
            changed_fields,
            status_id: requested_status.unwrap_or(status_before),
            archived,
        })),
        UpdateWorkItemOutcome::Noop(current) => Ok(Ok(Outcome {
            version: current.version,
            changed_fields: Vec::new(),
            status_id: current.status_id,
            archived: current.archived,
        })),
        UpdateWorkItemOutcome::Conflict(current) if current.archived => {
            problem("archived", archived_task_message(key))
        }
        UpdateWorkItemOutcome::Conflict(current) => problem(
            "expectedVersions",
            version_conflict_message(key, current.version),
        ),
        UpdateWorkItemOutcome::InvalidTransition {
            current_status_id,
            requested_status_id,
        } => problem(
            "status",
            workflow
                .transition_problem(current_status_id, requested_status_id)
                .unwrap_or_else(|| "That status change isn't allowed.".to_owned()),
        ),
        UpdateWorkItemOutcome::InvalidAssignee => problem(
            "assignee",
            "That person can't be assigned tasks here: only active people and agents in the project can.".to_owned(),
        ),
        UpdateWorkItemOutcome::TooManyLabels { limit, would_have } => problem(
            "addLabels",
            format!(
                "A task can have at most {limit} labels; this change would give {key} {would_have}. Remove some with removeLabels first, or leave it out."
            ),
        ),
        UpdateWorkItemOutcome::InvalidMilestone => {
            problem("milestone", super::writes::MILESTONE_MESSAGE.to_owned())
        }
        UpdateWorkItemOutcome::InvalidDates => {
            problem("dueDate", super::writes::DATES_MESSAGE.to_owned())
        }
        UpdateWorkItemOutcome::NotFound
        | UpdateWorkItemOutcome::InvalidParent(_)
        | UpdateWorkItemOutcome::InvalidPlacement(_) => {
            Err(AppError::Internal("a batch update asked for no parent or place"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use uuid::Uuid;

    use super::{
        BatchProblem, CreateManyValue, TransitionGroup, UpdateManyFingerprint, UpdateManyValue,
        create_rejected, update_rejected_message,
    };
    use crate::idempotency::RequestHash;

    #[test]
    fn expected_versions_hash_the_same_in_any_order() {
        let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
        let hash = |pairs: &[(Uuid, i64)]| {
            let expected = pairs.iter().copied().collect::<BTreeMap<_, _>>();
            RequestHash::for_value(&UpdateManyFingerprint {
                project_id: Uuid::nil(),
                task_ids: &[first, second],
                expected: &expected,
                status: Some("done"),
                priority: None,
                assignee: None,
                clear_assignee: false,
                task_type: None,
                add_labels: &[],
                remove_labels: &[],
                archived: None,
                milestone: None,
                due_date: None,
            })
            .expect("hash")
        };
        assert_eq!(
            hash(&[(first, 2), (second, 1)]),
            hash(&[(second, 1), (first, 2)])
        );
        assert_ne!(hash(&[(first, 2)]), hash(&[(first, 3)]));
    }

    #[test]
    fn stored_batch_results_still_decode() {
        // Only the fields every stored result has had since the first
        // release; anything added later must be optional.
        let created = serde_json::from_value::<CreateManyValue>(json!({
            "preview": false,
            "projectKey": "AWD",
            "tasks": [{
                "index": 0, "key": "AWD-2", "title": "Import plans", "status": "Ready",
                "type": "Feature", "criteriaCount": 0, "version": 1
            }]
        }))
        .expect("create value");
        assert!(!created.idempotent_replay);
        assert!(created.tasks[0].id.is_none());
        let round_trip = serde_json::to_value(&created).expect("encode");
        assert_eq!(round_trip["tasks"][0]["type"], "Feature");
        assert!(round_trip.get("labelsCreated").is_none());
        let updated = serde_json::from_value::<UpdateManyValue>(json!({
            "preview": false,
            "changedCount": 1,
            "unchangedCount": 0,
            "tasks": [{
                "index": 0, "id": Uuid::nil(), "key": "AWD-2", "version": 2,
                "status": "Done", "archived": false, "url": "https://example.test/x"
            }]
        }))
        .expect("update value");
        assert!(updated.tasks[0].changed_fields.is_empty());
        assert!(updated.notes.is_empty());
    }

    #[test]
    fn rejected_updates_group_transitions_and_list_the_rest() {
        let problems = vec![
            BatchProblem::new(0, "status", "Backlog can't move straight to Done.")
                .with_task("AWD-1"),
            BatchProblem::new(1, "status", "Backlog can't move straight to Done.")
                .with_task("AWD-2"),
            BatchProblem::new(2, "archived", "AWD-3 is archived. Restore it first.")
                .with_task("AWD-3"),
            BatchProblem::new(3, "status", "Canceled can't move straight to Done.")
                .with_task("AWD-4"),
        ];
        let groups = [
            TransitionGroup {
                from: "Backlog".to_owned(),
                to: "Done".to_owned(),
                path: Some(
                    ["Backlog", "Ready", "In Progress", "Review", "Done"]
                        .map(str::to_owned)
                        .to_vec(),
                ),
                keys: vec!["AWD-1".to_owned(), "AWD-2".to_owned()],
            },
            TransitionGroup {
                from: "Canceled".to_owned(),
                to: "Done".to_owned(),
                path: None,
                keys: vec!["AWD-4".to_owned()],
            },
        ];
        let message = update_rejected_message(8, &problems, &groups);
        let lines = message.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "Nothing changed: 4 of 8 tasks can't take this change."
        );
        assert_eq!(
            lines[1],
            "AWD-1 and AWD-2 are in Backlog. The shortest way to Done is Backlog → Ready → In Progress → Review → Done: run task_update_many once per step, or leave them out."
        );
        assert!(
            lines[2].starts_with("AWD-4 is in Canceled, and Done can't be reached"),
            "{message}"
        );
        assert_eq!(lines[3], "AWD-3 is archived. Restore it first.");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn rejected_batches_list_at_most_a_hundred_problems() {
        let problems = (0..150)
            .map(|index| BatchProblem::new(index, "title", "Give it a title."))
            .collect::<Vec<_>>();
        let super::BatchResult::<()>::Rejected { message, problems } =
            create_rejected(150, problems)
        else {
            panic!("rejected");
        };
        assert_eq!(problems.len(), 100);
        assert!(
            message.starts_with("Nothing was created: 150 of 150 tasks have problems"),
            "{message}"
        );
        assert!(
            message.ends_with("…and 50 more problems not listed."),
            "{message}"
        );
        let message = update_rejected_message(
            150,
            &(0..150)
                .map(|index| {
                    BatchProblem::new(index, "archived", "is archived.")
                        .with_task(&format!("AWD-{index}"))
                })
                .collect::<Vec<_>>(),
            &[],
        );
        assert_eq!(message.lines().count(), 102, "{message}");
        assert!(message.ends_with("…and 50 more."), "{message}");
    }
}
