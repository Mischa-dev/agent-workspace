//! The triage queue (T1.23): work that arrived and hasn't been looked at.
//!
//! A space uses triage once its workflow has a status of the `triage`
//! category (`enable` adds one, with a move from it to every other
//! status). Tasks made through a form or a bug report link land there, and
//! so does any task made in that status; the space's triage rules then run
//! over it in position order, and the first whose filter matches acts:
//! accept it (out of triage), give it to someone, or keep it in triage.
//!
//! `triage` is the queue's one write: accept moves a task to the space's
//! first unstarted status (or the one named), decline cancels it with the
//! reason as a comment, duplicate links it to its original and cancels it,
//! and snooze hides it from the queue until a time. The built-in Triage
//! view (status category triage, not snoozed) is the queue; the wake job
//! clears snoozes that are due and tells whoever snoozed.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    collections::tasks::TasksAcross,
    error::AppError,
    views::{definition::ViewConfig, filter::Filter, query::Spec},
    work_item_commands::{
        self, AuditVersionMetadata, CommandActor, LockedWorkItem, TaskFieldChanges,
        UpdateWorkItemCommand, UpdateWorkItemOutcome,
    },
    work_item_planning::{self, RelationKind},
    work_item_workflow::{self, StatusDraft, TransitionDraft, TransitionsDraft, WorkflowRefusal},
};

/// Tasks one `triage` call acts on at most.
pub const MAX_TASKS: usize = 50;
/// Rules one space keeps at most (the position CHECK).
pub const MAX_RULES: i64 = 50;
/// The furthest ahead a snooze can run.
pub const MAX_SNOOZE_DAYS: i64 = 365;
/// Characters a decline's reason has at most.
pub const MAX_REASON_CHARACTERS: usize = 2000;
/// The status `enable` adds.
pub const STATUS_NAME: &str = "Triage";

/// What a refused triage call says: the input field and what to do.
pub type Refusal = (&'static str, String);

fn refused<T>(
    field: &'static str,
    message: impl Into<String>,
) -> Result<Result<T, Refusal>, AppError> {
    Ok(Err((field, message.into())))
}

// ---------------------------------------------------------------------------
// Statuses

/// The space's triage status, when it uses one: the first by position.
pub async fn triage_status(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    first_status(connection, project_id, "triage").await
}

/// The name of the space's triage status, when it uses one.
pub async fn triage_status_name(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM work_item_statuses \
         WHERE project_id = $1 AND enabled AND semantic_category = 'triage' \
         ORDER BY position, id LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?)
}

async fn first_status(
    connection: &mut PgConnection,
    project_id: Uuid,
    category: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses \
         WHERE project_id = $1 AND enabled AND semantic_category = $2 \
         ORDER BY position, id LIMIT 1",
    )
    .bind(project_id)
    .bind(category)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A status of the space by its ID or name, with its category.
async fn named_status(
    connection: &mut PgConnection,
    project_id: Uuid,
    reference: &str,
) -> Result<Option<(Uuid, String, String)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, name, semantic_category FROM work_item_statuses \
         WHERE project_id = $1 AND enabled AND (id::text = $2 OR lower(name) = lower($2)) \
         ORDER BY position, id LIMIT 1",
    )
    .bind(project_id)
    .bind(reference.trim())
    .fetch_optional(&mut *connection)
    .await?)
}

/// Where accepting a task moves it: the status named, else the space's
/// first unstarted status, else its first backlog status.
async fn accepted_status(
    connection: &mut PgConnection,
    project_id: Uuid,
    named: Option<&str>,
) -> Result<Result<Uuid, Refusal>, AppError> {
    if let Some(reference) = named {
        return match named_status(connection, project_id, reference).await? {
            Some((_, name, category)) if category == "triage" => refused(
                "status",
                format!("{name} is a triage status. Name the status the task moves on to."),
            ),
            Some((id, _, _)) => Ok(Ok(id)),
            None => refused(
                "status",
                "This space has no such status. Name one of its statuses (workflow_get lists them).",
            ),
        };
    }
    for category in ["unstarted", "backlog"] {
        if let Some(id) = first_status(connection, project_id, category).await? {
            return Ok(Ok(id));
        }
    }
    refused(
        "status",
        "This space's workflow has no status to accept tasks into. Name one.",
    )
}

/// Adds a Triage status first in the workflow, with a move from it to
/// every other status. A space that has a triage status already keeps it.
/// The caller has locked the project and checked the actor may change its
/// workflow. Returns the triage status.
pub async fn enable(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
) -> Result<Result<Uuid, Refusal>, AppError> {
    // The Triage view is the Tasks collection's.
    crate::collections::tasks::ensure(transaction, actor.project_id, actor.actor_id).await?;
    if let Some(found) = triage_status(transaction, actor.project_id).await? {
        return Ok(Ok(found));
    }
    let mut document = work_item_workflow::current_document(transaction, actor.project_id).await?;
    if document
        .statuses
        .iter()
        .any(|status| status.name.eq_ignore_ascii_case(STATUS_NAME))
    {
        return refused(
            "spaceId",
            "This space already has a status called Triage. Rename it, or make it a triage \
             status in the workflow settings.",
        );
    }
    let key = "triage".to_owned();
    if let TransitionsDraft::List(transitions) = &mut document.transitions {
        transitions.extend(document.statuses.iter().filter_map(|status| {
            status.id.map(|id| TransitionDraft {
                from: key.clone(),
                to: id.to_string(),
            })
        }));
    }
    document.statuses.insert(
        0,
        StatusDraft {
            id: None,
            key: Some(key),
            name: STATUS_NAME.to_owned(),
            semantic_category: "triage".to_owned(),
            is_default: false,
        },
    );
    match work_item_workflow::replace_workflow(transaction, actor, &document).await? {
        Ok(_) => {}
        Err(WorkflowRefusal::VersionConflict { .. }) => {
            return Err(AppError::Internal("triage workflow version"));
        }
        Err(WorkflowRefusal::Invalid { message, .. }) => return refused("spaceId", message),
    }
    triage_status(transaction, actor.project_id)
        .await?
        .map(Ok)
        .ok_or(AppError::Internal("triage status added"))
}

// ---------------------------------------------------------------------------
// Triage

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriageAction {
    /// Move it out of triage, to `status` or the space's first unstarted
    /// status.
    Accept,
    /// Cancel it, with `reason` as a comment.
    Decline,
    /// Mark it a duplicate of `of` and cancel it.
    Duplicate,
    /// Hide it from the triage queue until `until`.
    Snooze,
}

impl TriageAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Duplicate => "duplicate",
            Self::Snooze => "snooze",
        }
    }
}

pub struct TriageRequest<'a> {
    pub action: TriageAction,
    pub status: Option<&'a str>,
    pub of: Option<Uuid>,
    pub until: Option<DateTime<Utc>>,
    pub reason: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TriagedTask {
    #[schemars(with = "String")]
    pub task_id: Uuid,
    pub version: i64,
    /// Its status now.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<DateTime<Utc>>,
}

/// Triages each task in turn, in the caller's transaction: all of them, or
/// none when one is refused.
pub async fn triage(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_ids: &[Uuid],
    request: &TriageRequest<'_>,
) -> Result<Result<Vec<TriagedTask>, Refusal>, AppError> {
    if task_ids.is_empty() || task_ids.len() > MAX_TASKS {
        return refused("taskIds", format!("Name 1 to {MAX_TASKS} tasks."));
    }
    let reason = match request
        .reason
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        Some(text) if text.chars().count() > MAX_REASON_CHARACTERS => {
            return refused(
                "reason",
                format!("Keep the reason to {MAX_REASON_CHARACTERS} characters."),
            );
        }
        other => other,
    };
    let snooze = match (request.action, request.until) {
        (TriageAction::Snooze, None) => {
            return refused(
                "until",
                "Say when the snooze ends, such as 2026-12-01T09:00:00Z.",
            );
        }
        (TriageAction::Snooze, Some(until)) => {
            let now = Utc::now();
            if until <= now || until > now + ChronoDuration::days(MAX_SNOOZE_DAYS) {
                return refused(
                    "until",
                    format!("Snooze until a time in the next {MAX_SNOOZE_DAYS} days."),
                );
            }
            Some(until)
        }
        _ => None,
    };
    let target = match request.action {
        TriageAction::Accept => {
            match accepted_status(transaction, actor.project_id, request.status).await? {
                Ok(id) => Some(id),
                Err(refusal) => return Ok(Err(refusal)),
            }
        }
        TriageAction::Decline => {
            match first_status(transaction, actor.project_id, "canceled").await? {
                Some(id) => Some(id),
                None => {
                    return refused(
                        "action",
                        "This space's workflow has no canceled status to decline into.",
                    );
                }
            }
        }
        TriageAction::Duplicate => {
            if request.of.is_none() {
                return refused("of", "Name the task this one duplicates.");
            }
            None
        }
        TriageAction::Snooze => None,
    };
    let mut seen = std::collections::HashSet::new();
    let mut done = Vec::with_capacity(task_ids.len());
    for &task_id in task_ids {
        if !seen.insert(task_id) {
            continue;
        }
        let Some(current) =
            work_item_commands::lock_work_item(transaction, actor.project_id, task_id).await?
        else {
            return refused(
                "taskIds",
                "A task named isn't in this space, or you can't see it.",
            );
        };
        if current.archived {
            return refused(
                "taskIds",
                "A task named is archived. Restore it before triaging it.",
            );
        }
        if request.action == TriageAction::Duplicate {
            let of = request.of.unwrap_or_default();
            match work_item_planning::change_relation(
                transaction,
                actor,
                task_id,
                of,
                RelationKind::Duplicates,
                false,
                true,
            )
            .await?
            {
                Ok(_) => {}
                Err(refusal) => return refused("of", refusal.message("the original")),
            }
        } else if let Some(status_id) = target {
            match move_to(transaction, actor, current, status_id, request.action).await? {
                Ok(()) => {}
                Err(refusal) => return Ok(Err(refusal)),
            }
            if let (TriageAction::Decline, Some(reason)) = (request.action, reason) {
                comment(transaction, actor, task_id, reason).await?;
            }
        }
        work_item_commands::mark_triaged(transaction, actor, task_id, snooze).await?;
        crate::events::emit(
            transaction,
            crate::events::NewEvent::thing(
                &crate::events::catalog::TASK_TRIAGED,
                actor.project_id,
                "task",
                task_id,
            )
            .actor(actor.actor_id)
            .payload(json!({ "taskId": task_id, "action": request.action.as_str() })),
        )
        .await?;
        done.push(read_back(transaction, actor.project_id, task_id).await?);
    }
    Ok(Ok(done))
}

/// Moves a locked task to `status_id` as a triage decision.
async fn move_to(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    current: LockedWorkItem,
    status_id: Uuid,
    action: TriageAction,
) -> Result<Result<(), Refusal>, AppError> {
    if current.status_id == status_id {
        return Ok(Ok(()));
    }
    let expected_version = current.version;
    let (summary, audit_action) = match action {
        TriageAction::Decline => ("Declined in triage", "work_item.triage_declined"),
        _ => ("Accepted from triage", "work_item.triage_accepted"),
    };
    let outcome = work_item_commands::update_locked_work_item(
        transaction,
        actor,
        current,
        UpdateWorkItemCommand {
            expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id: Some(status_id),
            priority: None,
            assignee_actor_id: None,
            parent_work_item_id: None,
            board_placement: None,
            labels: None,
            fields: TaskFieldChanges::default(),
            field_values: None,
            summary,
            audit_action,
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    match outcome {
        UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_) => Ok(Ok(())),
        UpdateWorkItemOutcome::InvalidTransition { .. } => refused(
            "status",
            "The workflow doesn't let a task move there from its status. Add that move in the \
             workflow settings, or name another status.",
        ),
        _ => Err(AppError::Internal("triage move")),
    }
}

/// Posts `text` on the task as the actor's comment.
async fn comment(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    text: &str,
) -> Result<(), AppError> {
    let content = crate::message_content::MessageContent::from_plain_text(text)
        .map_err(|_| AppError::Validation("reason"))?
        .into_parts()
        .0;
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(task_id)
        .fetch_one(&mut **transaction)
        .await?;
    work_item_commands::insert_task_comment(
        transaction,
        actor,
        work_item_commands::NewTaskComment {
            work_item_id: task_id,
            version,
            content,
            reply_to: None,
            attachments: &[],
            audit_action: "work_item.triage_comment_posted",
        },
    )
    .await?;
    Ok(())
}

async fn read_back(
    connection: &mut PgConnection,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<TriagedTask, AppError> {
    let (version, status, snoozed_until) =
        sqlx::query_as::<_, (i64, String, Option<DateTime<Utc>>)>(
            "SELECT task.version, status.name, task.snoozed_until FROM work_items task \
         JOIN work_item_statuses status \
           ON status.project_id = task.project_id AND status.id = task.status_id \
         WHERE task.project_id = $1 AND task.id = $2",
        )
        .bind(project_id)
        .bind(task_id)
        .fetch_one(&mut *connection)
        .await?;
    Ok(TriagedTask {
        task_id,
        version,
        status,
        snoozed_until,
    })
}

// ---------------------------------------------------------------------------
// Rules

/// What a rule does to a task it matches.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleAction {
    /// accept (out of triage, to `status` or the first unstarted status),
    /// assign (give it to `to`, leaving it in triage) or keep (leave it in
    /// triage; later rules don't run).
    pub kind: RuleActionKind,
    /// accept: the status, by its ID or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// assign: the person or agent, by account ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub to: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuleActionKind {
    Accept,
    Assign,
    Keep,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Its place: rules run from 1 up, and the first that matches acts.
    pub position: i32,
    pub name: String,
    /// A view filter over the arriving task (a list of clauses, or "and"
    /// and "or" groups), as views take.
    pub filter: Value,
    pub action: RuleAction,
    pub enabled: bool,
    pub version: i64,
}

type RuleRow = (Uuid, i32, String, Value, Value, bool, i64);

fn rule_of(row: RuleRow) -> Rule {
    let (id, position, name, filter, action, enabled, version) = row;
    Rule {
        id,
        position,
        name,
        filter,
        action: serde_json::from_value(action).unwrap_or(RuleAction {
            kind: RuleActionKind::Keep,
            status: None,
            to: None,
        }),
        enabled,
        version,
    }
}

const RULE_COLUMNS: &str = "id, position, name, filter, action, enabled, version";

/// The space's rules in the order they run.
pub async fn rules(connection: &mut PgConnection, project_id: Uuid) -> Result<Vec<Rule>, AppError> {
    Ok(sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {RULE_COLUMNS} FROM triage_rules WHERE space_id = $1 ORDER BY position, id"
    ))
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(rule_of)
    .collect())
}

/// A rule's new or changed parts; None keeps what it has.
#[derive(Default)]
pub struct RuleChange {
    pub name: Option<String>,
    pub filter: Option<Value>,
    pub action: Option<RuleAction>,
    pub enabled: Option<bool>,
    /// 1-based; past the end goes last.
    pub position: Option<i32>,
}

fn rule_name(value: &str) -> Result<String, Refusal> {
    let name = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 80 {
        return Err((
            "name",
            "Give the rule a name of 1 to 80 characters.".to_owned(),
        ));
    }
    Ok(name)
}

/// Checks a rule's filter against the space's task fields, as a view's.
async fn checked_filter(
    connection: &mut PgConnection,
    project_id: Uuid,
    value: Value,
) -> Result<Result<Value, Refusal>, AppError> {
    let filter = match serde_json::from_value::<Filter>(value) {
        Ok(filter) => filter,
        Err(error) => {
            return refused(
                "filter",
                format!(
                    "A rule's filter is a view's: a list of clauses, or \"and\" and \"or\" groups ({error})."
                ),
            );
        }
    };
    let Some(collection) =
        crate::collections::task_query::tasks_collection(connection, project_id).await?
    else {
        return Err(AppError::Internal("triage rule without a Tasks collection"));
    };
    let definitions =
        crate::collections::view_fields(connection, project_id, collection, true).await?;
    let config = ViewConfig {
        filters: filter,
        ..ViewConfig::default()
    };
    if let Err(message) = crate::views::definition::check(&config, &definitions) {
        return refused("filter", message);
    }
    if let Err(message) =
        crate::views::definition::check_references(connection, project_id, &config, &definitions)
            .await?
    {
        return refused("filter", message);
    }
    Ok(Ok(
        serde_json::to_value(&config.filters).unwrap_or(Value::Null)
    ))
}

async fn checked_action(
    connection: &mut PgConnection,
    project_id: Uuid,
    action: RuleAction,
) -> Result<Result<Value, Refusal>, AppError> {
    match action.kind {
        RuleActionKind::Accept => {
            if let Err(refusal) =
                accepted_status(connection, project_id, action.status.as_deref()).await?
            {
                return Ok(Err(("action", refusal.1)));
            }
        }
        RuleActionKind::Assign => {
            let Some(to) = action.to else {
                return refused("action", "Say who an assign rule gives tasks to (to).");
            };
            if !work_item_commands::validate_assignee(connection, project_id, Some(to)).await? {
                return refused("action", "Give tasks to an active member of this space.");
            }
        }
        RuleActionKind::Keep => {}
    }
    let action = RuleAction {
        status: action
            .status
            .filter(|_| action.kind == RuleActionKind::Accept),
        to: action.to.filter(|_| action.kind == RuleActionKind::Assign),
        kind: action.kind,
    };
    Ok(Ok(serde_json::to_value(action).unwrap_or(Value::Null)))
}

/// Makes a rule, or changes one (`rule`) at `expected_version`. The caller
/// has locked the space.
pub async fn write_rule(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    rule: Option<(Uuid, Option<i64>)>,
    change: RuleChange,
) -> Result<Result<Rule, Refusal>, AppError> {
    let project_id = actor.project_id;
    // Rules filter the Tasks collection's fields; a space with no task yet
    // gets the collection here, as its first task would make it.
    crate::collections::tasks::ensure(transaction, project_id, actor.actor_id).await?;
    let name = match change.name.as_deref().map(rule_name).transpose() {
        Ok(name) => name,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let filter = match change.filter {
        Some(value) => match checked_filter(transaction, project_id, value).await? {
            Ok(filter) => Some(filter),
            Err(refusal) => return Ok(Err(refusal)),
        },
        None => None,
    };
    let action = match change.action {
        Some(action) => match checked_action(transaction, project_id, action).await? {
            Ok(action) => Some(action),
            Err(refusal) => return Ok(Err(refusal)),
        },
        None => None,
    };
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM triage_rules WHERE space_id = $1")
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    let id = match rule {
        None => {
            let (Some(name), Some(action)) = (name, action) else {
                return refused("name", "A new rule needs a name and an action.");
            };
            if count >= MAX_RULES {
                return refused(
                    "spaceId",
                    format!("A space keeps at most {MAX_RULES} triage rules. Remove one first."),
                );
            }
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO triage_rules \
                   (id, workspace_id, space_id, position, name, filter, action, enabled, created_by_actor_id) \
                 SELECT $1, project.organization_id, project.id, $3, $4, $5, $6, $7, $8 \
                 FROM projects project WHERE project.id = $2",
            )
            .bind(id)
            .bind(project_id)
            .bind(i32::try_from(count + 1).unwrap_or(i32::MAX))
            .bind(name)
            .bind(filter.unwrap_or_else(|| json!([])))
            .bind(action)
            .bind(change.enabled.unwrap_or(true))
            .bind(actor.actor_id)
            .execute(&mut **transaction)
            .await?;
            id
        }
        Some((id, expected_version)) => {
            let Some(version) = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM triage_rules WHERE space_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(project_id)
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            else {
                return refused("rule", "This space has no such triage rule.");
            };
            if expected_version.is_some_and(|expected| expected != version) {
                return Err(AppError::VersionConflict {
                    resource: "triage_rule",
                    current_version: version,
                });
            }
            sqlx::query(
                "UPDATE triage_rules SET name = COALESCE($3, name), filter = COALESCE($4, filter), \
                   action = COALESCE($5, action), enabled = COALESCE($6, enabled), \
                   version = version + 1, updated_at = now() \
                 WHERE space_id = $1 AND id = $2",
            )
            .bind(project_id)
            .bind(id)
            .bind(name)
            .bind(filter)
            .bind(action)
            .bind(change.enabled)
            .execute(&mut **transaction)
            .await?;
            id
        }
    };
    if let Some(position) = change.position {
        place(transaction, project_id, id, position).await?;
    }
    let row = sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {RULE_COLUMNS} FROM triage_rules WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Ok(rule_of(row)))
}

/// Puts a rule at `position` (1-based) and numbers the others around it.
async fn place(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    rule_id: Uuid,
    position: i32,
) -> Result<(), AppError> {
    let mut order = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM triage_rules WHERE space_id = $1 AND id <> $2 ORDER BY position, id",
    )
    .bind(project_id)
    .bind(rule_id)
    .fetch_all(&mut **transaction)
    .await?;
    let at = usize::try_from(position.max(1) - 1)
        .unwrap_or(0)
        .min(order.len());
    order.insert(at, rule_id);
    renumber(transaction, project_id, &order).await
}

async fn renumber(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    order: &[Uuid],
) -> Result<(), AppError> {
    // Each rule takes its new place by its index; one statement keeps the
    // positions whole between rules.
    let positions = (1..=order.len())
        .map(|index| i32::try_from(index).unwrap_or(i32::MAX))
        .collect::<Vec<_>>();
    sqlx::query(
        "UPDATE triage_rules rule SET position = placed.position, updated_at = now() \
         FROM unnest($2::uuid[], $3::int[]) AS placed(id, position) \
         WHERE rule.space_id = $1 AND rule.id = placed.id AND rule.position <> placed.position",
    )
    .bind(project_id)
    .bind(order)
    .bind(positions)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Removes a rule; the ones after it move up.
pub async fn remove_rule(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    rule_id: Uuid,
) -> Result<bool, AppError> {
    let removed = sqlx::query("DELETE FROM triage_rules WHERE space_id = $1 AND id = $2")
        .bind(project_id)
        .bind(rule_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
        > 0;
    let order = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM triage_rules WHERE space_id = $1 ORDER BY position, id",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    renumber(transaction, project_id, &order).await?;
    Ok(removed)
}

/// Runs the space's enabled rules over a task that just landed in triage,
/// in position order: the first whose filter matches acts, as `actor`.
/// A rule whose action no longer fits (its status was retired, its person
/// left) is passed over.
pub async fn run_rules(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
) -> Result<(), AppError> {
    let found = rules(transaction, actor.project_id).await?;
    if !found.iter().any(|rule| rule.enabled) {
        return Ok(());
    }
    let Some(collection) =
        crate::collections::task_query::tasks_collection(transaction, actor.project_id).await?
    else {
        return Ok(());
    };
    let definitions =
        crate::collections::view_fields(transaction, actor.project_id, collection, true).await?;
    let source = TasksAcross {
        projects: vec![actor.project_id],
        collection: None,
    };
    for rule in found.into_iter().filter(|rule| rule.enabled) {
        let Ok(filter) = serde_json::from_value::<Filter>(rule.filter.clone()) else {
            continue;
        };
        let config = ViewConfig {
            filters: filter,
            ..ViewConfig::default()
        };
        let mut spec = Spec::from_config(&config, &definitions, actor.actor_id);
        spec.only = Some(vec![task_id]);
        if crate::views::query::total(transaction, &source, &spec).await? == 0 {
            continue;
        }
        if apply_rule(transaction, actor, task_id, &rule).await? {
            return Ok(());
        }
    }
    Ok(())
}

/// Does what a matching rule says. False when it can't (and the next rule
/// is tried).
async fn apply_rule(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    rule: &Rule,
) -> Result<bool, AppError> {
    let status_id = match rule.action.kind {
        RuleActionKind::Keep => return Ok(true),
        RuleActionKind::Accept => {
            match accepted_status(transaction, actor.project_id, rule.action.status.as_deref())
                .await?
            {
                Ok(id) => Some(id),
                Err(_) => return Ok(false),
            }
        }
        RuleActionKind::Assign => None,
    };
    let Some(current) =
        work_item_commands::lock_work_item(transaction, actor.project_id, task_id).await?
    else {
        return Ok(false);
    };
    let expected_version = current.version;
    let outcome = work_item_commands::update_locked_work_item(
        transaction,
        actor,
        current,
        UpdateWorkItemCommand {
            expected_version,
            title: None,
            description: None,
            type_id: None,
            status_id,
            priority: None,
            assignee_actor_id: match rule.action.kind {
                RuleActionKind::Assign => Some(rule.action.to),
                _ => None,
            },
            parent_work_item_id: None,
            board_placement: None,
            labels: None,
            fields: TaskFieldChanges::default(),
            field_values: None,
            summary: "Triage rule",
            audit_action: "work_item.triage_rule_applied",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    let applied = matches!(
        outcome,
        UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_)
    );
    if applied && rule.action.kind == RuleActionKind::Accept {
        work_item_commands::mark_triaged(transaction, actor, task_id, None).await?;
    }
    Ok(applied)
}

// ---------------------------------------------------------------------------
// Snoozes

/// The job that ends due snoozes.
pub const WAKE_KIND: &str = "tasks.triage_wake";
/// Snoozes one run ends at most; the next run takes the rest.
const WAKE_BATCH: i64 = 200;

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(WAKE_KIND, |context: crate::jobs::JobContext| async move {
        wake_snoozed(&context.state.pool).await?;
        Ok(())
    });
    registry.every(WAKE_KIND, std::time::Duration::from_secs(60));
}

/// Ends the snoozes that are due: each task is back in the queue, and
/// whoever snoozed it is told.
pub async fn wake_snoozed(pool: &sqlx::PgPool) -> Result<usize, AppError> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let woken = work_item_commands::end_due_snoozes(&mut transaction, WAKE_BATCH).await?;
    for task in &woken {
        if let Some(snoozer) = task.snoozed_by {
            crate::events::emit(
                &mut transaction,
                crate::events::NewEvent::thing(
                    &crate::events::catalog::TASK_SNOOZE_ENDED,
                    task.project_id,
                    "task",
                    task.task_id,
                )
                .audience(crate::events::Audience::Account(snoozer))
                .payload(json!({ "taskId": task.task_id })),
            )
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(woken.len())
}
