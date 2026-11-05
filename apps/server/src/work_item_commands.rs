//! Canonical application primitives for Work Item mutations.
//!
//! HTTP and MCP keep their wire validation, idempotency replay encoding, and
//! response mapping. This module owns the domain operations that must not
//! diverge: normalized values, Work Item locking and conflicts, assignee and
//! workflow checks, version advancement, and exactly-once mutation side
//! effects inside the caller's transaction.

use std::collections::HashMap;

use chrono::NaiveDate;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    external_events::enqueue_external_work_item_event,
    outbox::enqueue_work_item_updated,
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
    work_item_fields::FieldValueChanges,
    work_item_history::FieldChanges,
    work_item_labels::{self, LabelDelta, LabelPlan, MAX_LABELS_PER_TASK},
};

pub const MAX_TITLE_CHARACTERS: usize = 240;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 100_000;
pub const BOARD_RANK_STEP: i64 = 1024;
/// The deepest a task tree can go, counting the top-level task as level 1.
/// `task_move` walks this many levels, so nothing is left behind.
pub const MAX_SUBTASK_LEVELS: usize = 20;
/// Matches the position CHECK on `work_item_acceptance_criteria`.
pub const MAX_CRITERIA_PER_WORK_ITEM: usize = 1000;
pub const MAX_CRITERION_CHARACTERS: usize = 1000;

#[derive(Clone, Debug)]
pub struct CommandActor {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    pub project_id: Uuid,
    pub audit: AuditAttribution,
}

#[derive(Clone, Debug)]
pub enum AuditAttribution {
    Browser {
        session_id: Uuid,
        request_id: Option<String>,
    },
    Mcp {
        authentication_class: &'static str,
        request_id: String,
        client_name: String,
        mcp_credential_id: Option<Uuid>,
        oauth_grant_id: Option<Uuid>,
        external_agent_connection_id: Option<Uuid>,
    },
    /// Background work acting later for the person who set it up, such as
    /// a recurring task. It rechecks their access before it acts.
    Job {
        job_id: Uuid,
        /// What set it up, such as "recurring_task", as the audit row's client.
        source: &'static str,
    },
}

#[derive(Debug, FromRow)]
pub struct ProjectMembership {
    pub project_id: Uuid,
    pub project_archived_at: Option<String>,
    pub role: String,
    pub enabled: bool,
}

#[derive(Debug, FromRow)]
pub struct LockedWorkItem {
    pub id: Uuid,
    pub number: i64,
    pub version: i64,
    pub title: String,
    pub description: String,
    pub type_id: Uuid,
    pub status_id: Uuid,
    pub priority: String,
    pub assignee_actor_id: Option<Uuid>,
    pub parent_work_item_id: Option<Uuid>,
    pub board_rank: i64,
    pub archived: bool,
    pub blocked_reason: Option<String>,
    pub milestone_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub estimate: Option<f64>,
    /// Custom field values by field ID (work_item_fields).
    pub field_values: serde_json::Value,
}

/// The columns `LockedWorkItem` reads, for every query that locks a task.
pub const LOCKED_WORK_ITEM_COLUMNS: &str = "id, number, version, title, description, type_id, status_id, priority, \
    assignee_actor_id, parent_work_item_id, board_rank, archived_at IS NOT NULL AS archived, \
    blocked_reason, milestone_id, start_date, due_date, estimate, field_values";

/// Planning fields a new task can start with.
#[derive(Clone, Copy, Debug, Default)]
pub struct NewTaskFields {
    pub milestone_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub estimate: Option<f64>,
}

/// Planning fields a change can set: `Some(None)` clears one.
#[derive(Clone, Debug, Default)]
pub struct TaskFieldChanges {
    pub milestone_id: Option<Option<Uuid>>,
    pub start_date: Option<Option<NaiveDate>>,
    pub due_date: Option<Option<NaiveDate>>,
    pub estimate: Option<Option<f64>>,
    /// Marks the task blocked with this reason, or clears it.
    pub blocked_reason: Option<Option<String>>,
}

impl TaskFieldChanges {
    pub fn is_empty(&self) -> bool {
        self.milestone_id.is_none()
            && self.start_date.is_none()
            && self.due_date.is_none()
            && self.estimate.is_none()
            && self.blocked_reason.is_none()
    }
}

/// Most characters a blocked reason may have.
pub const MAX_BLOCKED_REASON_CHARACTERS: usize = 2000;
/// The largest estimate, in points or hours.
pub const MAX_ESTIMATE: f64 = 100_000.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConflictKind {
    Version,
    Archived,
}

#[derive(Clone, Debug)]
pub struct FinalizedChange {
    pub activity_id: Uuid,
    pub audit_id: Uuid,
}

pub struct ChangeFinalization<'a> {
    pub activity_kind: &'a str,
    pub summary: &'a str,
    pub audit_action: &'a str,
    pub activity_payload: serde_json::Value,
    pub audit_metadata: serde_json::Value,
    pub version: i64,
}

pub struct CreateWorkItemCommand<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub type_id: Option<Uuid>,
    pub status_id: Option<Uuid>,
    pub priority: &'a str,
    pub assignee_actor_id: Option<Uuid>,
    pub parent_work_item_id: Option<Uuid>,
    /// Active labels of the project, already resolved in this transaction.
    /// They land with the task, under its one created activity.
    pub label_ids: &'a [Uuid],
    /// Acceptance criteria, already normalized, in order.
    pub acceptance_criteria: &'a [String],
    /// Where it goes in its status column. None puts it at the bottom without
    /// locking the column, as creates always have.
    pub board_placement: Option<BoardPlacement>,
    pub fields: NewTaskFields,
    /// Custom field values, already resolved in this transaction
    /// (work_item_fields::resolve).
    pub field_values: Option<&'a FieldValueChanges>,
    pub activity_summary: &'a str,
    pub activity_metadata: serde_json::Value,
    pub activity_number_metadata: AllocatedNumberMetadata,
    pub audit_action: &'a str,
    pub audit_metadata: serde_json::Value,
}

#[derive(Clone, Copy, Debug)]
pub enum AllocatedNumberMetadata {
    Omit,
    Include,
}

#[derive(Debug)]
pub enum CreateWorkItemOutcome {
    Created {
        work_item_id: Uuid,
        number: i64,
        finalized: FinalizedChange,
    },
    InvalidAssignee,
    ParentNotFound,
    ParentArchived(Box<LockedWorkItem>),
    /// The subtask would sit deeper than MAX_SUBTASK_LEVELS.
    ParentTooDeep,
    InvalidPlacement(PlacementProblem),
    /// The milestone isn't an active milestone of the project.
    InvalidMilestone,
    /// The start date is after the due date.
    InvalidDates,
}

pub struct UpdateWorkItemCommand<'a> {
    pub expected_version: i64,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub type_id: Option<Uuid>,
    pub status_id: Option<Uuid>,
    pub priority: Option<&'a str>,
    pub assignee_actor_id: Option<Option<Uuid>>,
    /// `Some(Some(id))` puts the task under another task in its project;
    /// `Some(None)` makes it top-level.
    pub parent_work_item_id: Option<Option<Uuid>>,
    /// Where it goes in its (new) status column. A status change without one
    /// puts the task at the bottom of its new column.
    pub board_placement: Option<BoardPlacement>,
    pub labels: Option<LabelDelta<'a>>,
    pub fields: TaskFieldChanges,
    /// Custom field values to set or clear, already resolved in this
    /// transaction (work_item_fields::resolve).
    pub field_values: Option<&'a FieldValueChanges>,
    pub summary: &'a str,
    pub audit_action: &'a str,
    pub audit_version_metadata: AuditVersionMetadata,
}

#[derive(Clone, Copy, Debug)]
pub enum AuditVersionMetadata {
    Omit,
    Include,
}

#[derive(Debug)]
pub enum UpdateWorkItemOutcome {
    Applied {
        work_item_id: Uuid,
        version: i64,
        changed_fields: Vec<String>,
        finalized: FinalizedChange,
    },
    Noop(LockedWorkItem),
    Conflict(LockedWorkItem),
    NotFound,
    InvalidAssignee,
    InvalidTransition {
        current_status_id: Uuid,
        requested_status_id: Uuid,
    },
    TooManyLabels {
        limit: usize,
        would_have: usize,
    },
    InvalidParent(ParentProblem),
    InvalidPlacement(PlacementProblem),
    InvalidMilestone,
    InvalidDates,
}

/// Why a task can't go under the parent it was given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParentProblem {
    /// No such task in this project.
    NotFound,
    IsSelf,
    Archived(Uuid),
    /// The parent is already under the task: `path` runs from the task down
    /// to the parent it was given.
    Cycle {
        path: Vec<Uuid>,
    },
    TooDeep,
}

pub fn normalize_title(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_TITLE_CHARACTERS
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation("title"));
    }
    Ok(value.to_owned())
}

pub fn normalize_description(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.chars().count() > MAX_DESCRIPTION_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("description"));
    }
    Ok(value.to_owned())
}

pub fn normalize_single_line(
    value: &str,
    max_characters: usize,
    allow_empty: bool,
) -> Result<String, AppError> {
    let value = value.trim();
    if (!allow_empty && value.is_empty())
        || value.chars().count() > max_characters
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation("text"));
    }
    Ok(value.to_owned())
}

pub fn validate_priority(value: &str) -> Result<(), AppError> {
    if matches!(value, "no_priority" | "low" | "medium" | "high" | "urgent") {
        Ok(())
    } else {
        Err(AppError::Validation("priority"))
    }
}

pub async fn lock_membership(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembership>, AppError> {
    Ok(sqlx::query_as::<_, ProjectMembership>(
        r#"
        SELECT project.id AS project_id,
               project.archived_at::text AS project_archived_at,
               membership.role,
               membership.enabled
        FROM projects project
        JOIN project_memberships membership ON membership.project_id = project.id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = membership.actor_id
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE project.id = $1 AND membership.actor_id = $2
          AND actor.status = 'active'
        FOR UPDATE OF project, membership
        FOR SHARE OF organization_membership, actor
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub fn authorize_mutation(
    actor: &CommandActor,
    membership: &ProjectMembership,
    action: Action,
    work_item_id: Option<Uuid>,
) -> Result<Decision, AppError> {
    let scope = work_item_id.map_or_else(
        || ProjectScope::project(membership.project_id),
        |id| ProjectScope::work_item(membership.project_id, id),
    );
    Ok(authorize(
        &Subject::new(
            actor.actor_id,
            actor.actor_kind,
            true,
            Some(Membership::project(
                membership.project_id,
                membership_role(&membership.role)?,
                membership.enabled,
            )),
            None,
        ),
        action,
        Resource::Project(scope),
    ))
}

pub async fn lock_work_item(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<LockedWorkItem>, AppError> {
    Ok(sqlx::query_as::<_, LockedWorkItem>(&format!(
        r#"
        SELECT {LOCKED_WORK_ITEM_COLUMNS}
        FROM work_items
        WHERE project_id = $1 AND id = $2
        FOR UPDATE
        "#,
    ))
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Locks tasks in id order, so two writers that lock several never deadlock.
/// Tasks that aren't in the project are left out.
pub async fn lock_work_items(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_ids: &[Uuid],
) -> Result<Vec<LockedWorkItem>, AppError> {
    Ok(sqlx::query_as::<_, LockedWorkItem>(&format!(
        r#"
        SELECT {LOCKED_WORK_ITEM_COLUMNS}
        FROM work_items
        WHERE project_id = $1 AND id = ANY($2)
        ORDER BY id
        FOR UPDATE
        "#,
    ))
    .bind(project_id)
    .bind(work_item_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

/// Creates a task with its criteria and labels, under one created activity.
///
/// task_create_many previews run this and then roll back, so every side
/// effect here (and in `finalize_created`) must stay a write in this
/// transaction: never a NOTIFY, an in-process broadcast or a network call.
pub async fn create_work_item(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    command: CreateWorkItemCommand<'_>,
) -> Result<CreateWorkItemOutcome, AppError> {
    if command.label_ids.len() > MAX_LABELS_PER_TASK {
        return Err(AppError::Validation("labels"));
    }
    if !validate_assignee(transaction, actor.project_id, command.assignee_actor_id).await? {
        return Ok(CreateWorkItemOutcome::InvalidAssignee);
    }
    if let Some(milestone_id) = command.fields.milestone_id
        && !milestone_is_active(transaction, actor.project_id, milestone_id).await?
    {
        return Ok(CreateWorkItemOutcome::InvalidMilestone);
    }
    if let (Some(start), Some(due)) = (command.fields.start_date, command.fields.due_date)
        && start > due
    {
        return Ok(CreateWorkItemOutcome::InvalidDates);
    }
    if command
        .fields
        .estimate
        .is_some_and(|estimate| !valid_estimate(estimate))
    {
        return Err(AppError::Validation("estimate"));
    }
    if let Some(parent_id) = command.parent_work_item_id {
        let Some(parent) = lock_work_item(transaction, actor.project_id, parent_id).await? else {
            return Ok(CreateWorkItemOutcome::ParentNotFound);
        };
        if parent.archived {
            return Ok(CreateWorkItemOutcome::ParentArchived(Box::new(parent)));
        }
        let parent_levels = subtask_level(transaction, actor.project_id, parent_id).await?;
        if parent_levels + 1 > MAX_SUBTASK_LEVELS {
            return Ok(CreateWorkItemOutcome::ParentTooDeep);
        }
    }
    let type_id = select_definition(
        transaction,
        actor.project_id,
        DefinitionKind::Type,
        command.type_id,
    )
    .await?;
    let status_id = select_definition(
        transaction,
        actor.project_id,
        DefinitionKind::Status,
        command.status_id,
    )
    .await?;
    let work_item_id = Uuid::now_v7();
    let board_rank = match command.board_placement {
        None => {
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(max(board_rank), 0) + $3 FROM work_items WHERE project_id = $1 AND status_id = $2",
            )
            .bind(actor.project_id)
            .bind(status_id)
            .bind(BOARD_RANK_STEP)
            .fetch_one(&mut **transaction)
            .await?
        }
        Some(placement) => {
            match place_in_column(
                transaction,
                actor.project_id,
                status_id,
                work_item_id,
                None,
                placement,
            )
            .await?
            {
                Ok(placed) => placed.board_rank,
                Err(problem) => return Ok(CreateWorkItemOutcome::InvalidPlacement(problem)),
            }
        }
    };
    let number = sqlx::query_scalar::<_, i64>(
        "UPDATE project_counters SET next_work_item_number = next_work_item_number + 1, updated_at = now() WHERE project_id = $1 RETURNING next_work_item_number - 1",
    )
    .bind(actor.project_id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO work_items (
            id, project_id, number, title, description, type_id, status_id,
            priority, assignee_actor_id, created_by_actor_id, board_rank,
            parent_work_item_id, milestone_id, start_date, due_date, estimate,
            field_values
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
        "#,
    )
    .bind(work_item_id)
    .bind(actor.project_id)
    .bind(number)
    .bind(command.title)
    .bind(command.description)
    .bind(type_id)
    .bind(status_id)
    .bind(command.priority)
    .bind(command.assignee_actor_id)
    .bind(actor.actor_id)
    .bind(board_rank)
    .bind(command.parent_work_item_id)
    .bind(command.fields.milestone_id)
    .bind(command.fields.start_date)
    .bind(command.fields.due_date)
    .bind(command.fields.estimate)
    .bind({
        let values = command
            .field_values
            .map_or_else(|| serde_json::json!({}), FieldValueChanges::initial);
        if values.to_string().len() > crate::work_item_fields::MAX_VALUES_BYTES {
            return Err(AppError::Validation("fieldValues"));
        }
        values
    })
    .execute(&mut **transaction)
    .await?;
    add_watcher(transaction, actor.project_id, work_item_id, actor.actor_id).await?;
    if let Some(assignee) = command.assignee_actor_id {
        add_watcher(transaction, actor.project_id, work_item_id, assignee).await?;
    }
    if !command.acceptance_criteria.is_empty() {
        let criteria = command
            .acceptance_criteria
            .iter()
            .map(|text| (text.clone(), false))
            .collect::<Vec<_>>();
        insert_criteria(
            transaction,
            actor.project_id,
            work_item_id,
            actor.actor_id,
            1,
            &criteria,
        )
        .await?;
    }
    // Labels are stored by ID in the payloads, like a later change's
    // labelsAdded, so history can leave out labels archived since.
    let labels_added = work_item_labels::attach_to_new(
        transaction,
        actor.project_id,
        work_item_id,
        actor.actor_id,
        command.label_ids,
    )
    .await?;
    let mut activity_metadata = command.activity_metadata;
    if let Some(metadata) = activity_metadata.as_object_mut() {
        if matches!(
            command.activity_number_metadata,
            AllocatedNumberMetadata::Include
        ) {
            metadata.insert("number".to_owned(), serde_json::json!(number));
        }
        if !command.acceptance_criteria.is_empty() {
            metadata.insert(
                "acceptanceCriteria".to_owned(),
                serde_json::json!(command.acceptance_criteria.len()),
            );
        }
        if !labels_added.is_empty() {
            metadata.insert("labelsAdded".to_owned(), serde_json::json!(labels_added));
        }
        if let Some(milestone_id) = command.fields.milestone_id {
            metadata.insert("milestoneId".to_owned(), serde_json::json!(milestone_id));
        }
        if let Some(values) = command.field_values.filter(|values| !values.is_empty()) {
            metadata.insert(
                "fieldsSet".to_owned(),
                serde_json::json!(
                    values
                        .entries
                        .iter()
                        .filter(|entry| entry.value.is_some())
                        .map(|entry| entry.field.id)
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    let mut audit_metadata = command.audit_metadata;
    if let Some(metadata) = audit_metadata.as_object_mut() {
        metadata.insert("number".to_owned(), serde_json::json!(number));
        if !command.acceptance_criteria.is_empty() {
            metadata.insert(
                "acceptanceCriteria".to_owned(),
                serde_json::json!(command.acceptance_criteria.len()),
            );
        }
        if !labels_added.is_empty() {
            metadata.insert("labelsAdded".to_owned(), serde_json::json!(labels_added));
        }
    }
    let finalized = finalize_created(
        transaction,
        actor,
        CreatedFinalization {
            work_item_id,
            assignee_actor_id: command.assignee_actor_id,
            activity_summary: command.activity_summary,
            activity_metadata,
            audit_action: command.audit_action,
            audit_metadata,
        },
    )
    .await?;
    Ok(CreateWorkItemOutcome::Created {
        work_item_id,
        number,
        finalized,
    })
}

pub async fn update_work_item_by_id(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    command: UpdateWorkItemCommand<'_>,
) -> Result<UpdateWorkItemOutcome, AppError> {
    let Some(current) = lock_work_item(transaction, actor.project_id, work_item_id).await? else {
        return Ok(UpdateWorkItemOutcome::NotFound);
    };
    update_locked_work_item(transaction, actor, current, command).await
}

/// Changes a task the caller has locked. Whatever mix of fields changes, the
/// task gets one version bump, one activity, one audit row and one outbox
/// event.
///
/// task_update_many previews run this and then roll back, so every side
/// effect must stay a write in this transaction.
pub async fn update_locked_work_item(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    current: LockedWorkItem,
    command: UpdateWorkItemCommand<'_>,
) -> Result<UpdateWorkItemOutcome, AppError> {
    if conflict_kind(&current, command.expected_version).is_some() {
        return Ok(UpdateWorkItemOutcome::Conflict(current));
    }
    let type_id = match command.type_id {
        Some(type_id) => {
            select_definition(
                transaction,
                actor.project_id,
                DefinitionKind::Type,
                Some(type_id),
            )
            .await?
        }
        None => current.type_id,
    };
    let status_id = match command.status_id {
        Some(status_id) => {
            select_definition(
                transaction,
                actor.project_id,
                DefinitionKind::Status,
                Some(status_id),
            )
            .await?
        }
        None => current.status_id,
    };
    if status_id != current.status_id
        && !transition_allowed(transaction, actor.project_id, current.status_id, status_id).await?
    {
        return Ok(UpdateWorkItemOutcome::InvalidTransition {
            current_status_id: current.status_id,
            requested_status_id: status_id,
        });
    }
    if let Some(assignee) = command.assignee_actor_id
        && !validate_assignee(transaction, actor.project_id, assignee).await?
    {
        return Ok(UpdateWorkItemOutcome::InvalidAssignee);
    }
    let fields = &command.fields;
    let milestone_id = fields.milestone_id.unwrap_or(current.milestone_id);
    if milestone_id != current.milestone_id
        && let Some(milestone_id) = milestone_id
        && !milestone_is_active(transaction, actor.project_id, milestone_id).await?
    {
        return Ok(UpdateWorkItemOutcome::InvalidMilestone);
    }
    let start_date = fields.start_date.unwrap_or(current.start_date);
    let due_date = fields.due_date.unwrap_or(current.due_date);
    if let (Some(start), Some(due)) = (start_date, due_date)
        && start > due
    {
        return Ok(UpdateWorkItemOutcome::InvalidDates);
    }
    let estimate = fields.estimate.unwrap_or(current.estimate);
    if estimate.is_some_and(|estimate| !valid_estimate(estimate)) {
        return Err(AppError::Validation("estimate"));
    }
    let blocked_reason = fields
        .blocked_reason
        .clone()
        .unwrap_or_else(|| current.blocked_reason.clone());
    let label_plan: Option<LabelPlan> = match command.labels {
        Some(delta) if !delta.is_empty() => {
            Some(work_item_labels::plan(transaction, actor.project_id, current.id, &delta).await?)
        }
        _ => None,
    };
    if let Some(plan) = &label_plan
        && !plan.added.is_empty()
        && plan.resulting_count > MAX_LABELS_PER_TASK
    {
        return Ok(UpdateWorkItemOutcome::TooManyLabels {
            limit: MAX_LABELS_PER_TASK,
            would_have: plan.resulting_count,
        });
    }
    let field_values = match command.field_values.filter(|values| !values.is_empty()) {
        Some(values) => values.applied_to(&current.field_values),
        None => current.field_values.clone(),
    };
    if field_values.to_string().len() > crate::work_item_fields::MAX_VALUES_BYTES {
        return Err(AppError::Validation("fieldValues"));
    }
    let parent_work_item_id = match command.parent_work_item_id {
        Some(parent) if parent != current.parent_work_item_id => {
            if let Some(parent_id) = parent
                && let Some(problem) =
                    parent_problem(transaction, actor.project_id, &current, parent_id).await?
            {
                return Ok(UpdateWorkItemOutcome::InvalidParent(problem));
            }
            parent
        }
        _ => current.parent_work_item_id,
    };
    let mut position = None;
    let board_rank = match command.board_placement {
        Some(placement) => {
            match place_in_column(
                transaction,
                actor.project_id,
                status_id,
                current.id,
                Some((current.status_id, current.board_rank)),
                placement,
            )
            .await?
            {
                Ok(placed) => {
                    if placed.moved {
                        position = Some(placed.position);
                    }
                    placed.board_rank
                }
                Err(problem) => return Ok(UpdateWorkItemOutcome::InvalidPlacement(problem)),
            }
        }
        // A task whose status changes goes to the bottom of its new column,
        // where new tasks go, rather than wherever its old rank lands.
        None if status_id != current.status_id => {
            place_in_column(
                transaction,
                actor.project_id,
                status_id,
                current.id,
                Some((current.status_id, current.board_rank)),
                BoardPlacement::Bottom,
            )
            .await?
            .map_err(|_| AppError::Internal("place a task at the bottom of its column"))?
            .board_rank
        }
        None => current.board_rank,
    };
    let title = command.title.unwrap_or(&current.title);
    let description = command.description.unwrap_or(&current.description);
    let priority = command.priority.unwrap_or(&current.priority);
    let assignee_actor_id = command
        .assignee_actor_id
        .unwrap_or(current.assignee_actor_id);
    let mut changed_fields = Vec::new();
    if title != current.title {
        changed_fields.push("title".to_owned());
    }
    if description != current.description {
        changed_fields.push("description".to_owned());
    }
    if type_id != current.type_id {
        changed_fields.push("type".to_owned());
    }
    if status_id != current.status_id {
        changed_fields.push("status".to_owned());
    }
    if priority != current.priority {
        changed_fields.push("priority".to_owned());
    }
    if assignee_actor_id != current.assignee_actor_id {
        changed_fields.push("assignee".to_owned());
    }
    if parent_work_item_id != current.parent_work_item_id {
        changed_fields.push("parent".to_owned());
    }
    if position.is_some() {
        changed_fields.push("position".to_owned());
    }
    let label_plan = label_plan.filter(LabelPlan::changes);
    if label_plan.is_some() {
        changed_fields.push("labels".to_owned());
    }
    if milestone_id != current.milestone_id {
        changed_fields.push("milestone".to_owned());
    }
    if start_date != current.start_date {
        changed_fields.push("startDate".to_owned());
    }
    if due_date != current.due_date {
        changed_fields.push("dueDate".to_owned());
    }
    if estimate != current.estimate {
        changed_fields.push("estimate".to_owned());
    }
    if blocked_reason != current.blocked_reason {
        changed_fields.push("blockedReason".to_owned());
    }
    if field_values != current.field_values {
        changed_fields.push("fields".to_owned());
    }
    if changed_fields.is_empty() {
        return Ok(UpdateWorkItemOutcome::Noop(current));
    }
    let mut changes = FieldChanges::default();
    if let Some(values) = command.field_values {
        for entry in &values.entries {
            let before = current.field_values.get(entry.field.id.to_string());
            if before == entry.value.as_ref() {
                continue;
            }
            let from = crate::work_item_fields::shown_before(
                transaction,
                actor.project_id,
                &entry.field,
                before,
            )
            .await?;
            changes.custom_field(entry.field.id, &entry.field.name, from, entry.shown.clone());
        }
    }
    if let Some(plan) = &label_plan {
        work_item_labels::apply(
            transaction,
            actor.project_id,
            current.id,
            actor.actor_id,
            plan,
        )
        .await?;
    }
    if assignee_actor_id != current.assignee_actor_id
        && let Some(assignee) = assignee_actor_id
    {
        add_watcher(transaction, actor.project_id, current.id, assignee).await?;
    }
    if assignee_actor_id != current.assignee_actor_id {
        enqueue_external_work_item_event(
            transaction,
            actor.project_id,
            current.id,
            current.assignee_actor_id,
            "work_item.changed",
            "A Work Item assignment changed",
        )
        .await?;
    }
    let updated = sqlx::query(
        r#"
        UPDATE work_items
        SET title = $3, description = $4, type_id = $5, status_id = $6,
            priority = $7, assignee_actor_id = $8, parent_work_item_id = $10,
            board_rank = $11, milestone_id = $12, start_date = $13, due_date = $14,
            estimate = $15, field_values = $18,
            blocked_reason = $16,
            blocked_at = CASE WHEN $16::text IS NULL THEN NULL
                              WHEN blocked_reason IS NOT DISTINCT FROM $16 THEN blocked_at
                              ELSE now() END,
            blocked_by_actor_id = CASE WHEN $16::text IS NULL THEN NULL
                                       WHEN blocked_reason IS NOT DISTINCT FROM $16 THEN blocked_by_actor_id
                                       ELSE $17::uuid END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $9
        "#,
    )
    .bind(actor.project_id)
    .bind(current.id)
    .bind(title)
    .bind(description)
    .bind(type_id)
    .bind(status_id)
    .bind(priority)
    .bind(assignee_actor_id)
    .bind(command.expected_version)
    .bind(parent_work_item_id)
    .bind(board_rank)
    .bind(milestone_id)
    .bind(start_date)
    .bind(due_date)
    .bind(estimate)
    .bind(blocked_reason.as_deref())
    .bind(actor.actor_id)
    .bind(&field_values)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal(
            "locked Work Item update lost its version",
        ));
    }
    changes.text("title", Some(&current.title), Some(title));
    changes.description(&current.description, description);
    changes.id("type", Some(current.type_id), Some(type_id));
    changes.value(
        "priority",
        serde_json::json!(current.priority),
        serde_json::json!(priority),
    );
    changes.id("assignee", current.assignee_actor_id, assignee_actor_id);
    changes.id("milestone", current.milestone_id, milestone_id);
    changes.value(
        "startDate",
        serde_json::json!(current.start_date),
        serde_json::json!(start_date),
    );
    changes.value(
        "dueDate",
        serde_json::json!(current.due_date),
        serde_json::json!(due_date),
    );
    changes.value(
        "estimate",
        serde_json::json!(current.estimate),
        serde_json::json!(estimate),
    );
    changes.text(
        "blockedReason",
        current.blocked_reason.as_deref(),
        blocked_reason.as_deref(),
    );
    let version = command
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    let payload = ChangePayload {
        changed_fields: changed_fields.clone(),
        status: (status_id != current.status_id).then_some((current.status_id, status_id)),
        position,
        parent: (parent_work_item_id != current.parent_work_item_id)
            .then_some((current.parent_work_item_id, parent_work_item_id)),
        labels_added: label_plan
            .iter()
            .flat_map(|plan| plan.added.iter().map(|label| label.id))
            .collect(),
        labels_removed: label_plan
            .iter()
            .flat_map(|plan| plan.removed.iter().map(|label| label.id))
            .collect(),
        changes,
        ..ChangePayload::default()
    };
    let activity_payload = payload.activity();
    let audit_metadata = match command.audit_version_metadata {
        AuditVersionMetadata::Omit => activity_payload.clone(),
        AuditVersionMetadata::Include => payload.audit(version),
    };
    let finalized = finalize_change(
        transaction,
        actor,
        current.id,
        ChangeFinalization {
            activity_kind: "work_item_updated",
            summary: command.summary,
            audit_action: command.audit_action,
            activity_payload,
            audit_metadata,
            version,
        },
    )
    .await?;
    Ok(UpdateWorkItemOutcome::Applied {
        work_item_id: current.id,
        version,
        changed_fields,
        finalized,
    })
}

/// The task's ancestors, starting with `start` itself and going up.
async fn ancestor_chain(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    start: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH RECURSIVE chain(id, parent_id, depth) AS (
            SELECT id, parent_work_item_id, 1 FROM work_items
            WHERE project_id = $1 AND id = $2
            UNION ALL
            SELECT item.id, item.parent_work_item_id, chain.depth + 1
            FROM work_items item JOIN chain ON item.id = chain.parent_id
            WHERE item.project_id = $1 AND chain.depth < 64
        )
        SELECT id FROM chain ORDER BY depth
        "#,
    )
    .bind(project_id)
    .bind(start)
    .fetch_all(&mut **transaction)
    .await?)
}

/// How deep a task sits: 1 for a top-level task, 2 for its subtasks, and so
/// on. A subtask of it would sit one level deeper, at most
/// MAX_SUBTASK_LEVELS.
pub async fn subtask_level(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<usize, AppError> {
    Ok(ancestor_chain(transaction, project_id, work_item_id)
        .await?
        .len())
}

/// How many levels of subtasks sit under a task (0 when it has none).
async fn levels_below(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    root: Uuid,
) -> Result<usize, AppError> {
    let depth = sqlx::query_scalar::<_, i32>(
        r#"
        WITH RECURSIVE tree(id, depth) AS (
            SELECT id, 0 FROM work_items WHERE project_id = $1 AND id = $2
            UNION ALL
            SELECT child.id, tree.depth + 1
            FROM work_items child JOIN tree ON child.parent_work_item_id = tree.id
            WHERE child.project_id = $1 AND tree.depth < 64
        )
        SELECT COALESCE(max(depth), 0) FROM tree
        "#,
    )
    .bind(project_id)
    .bind(root)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(usize::try_from(depth).unwrap_or(0))
}

/// Whether `task` can go under `parent_id`. Writes in a project take turns on
/// its row (`lock_membership`), so the tree can't change under this check.
async fn parent_problem(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task: &LockedWorkItem,
    parent_id: Uuid,
) -> Result<Option<ParentProblem>, AppError> {
    if parent_id == task.id {
        return Ok(Some(ParentProblem::IsSelf));
    }
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(parent_id)
    .fetch_optional(&mut **transaction)
    .await?;
    match archived {
        None => return Ok(Some(ParentProblem::NotFound)),
        Some(true) => return Ok(Some(ParentProblem::Archived(parent_id))),
        Some(false) => {}
    }
    let chain = ancestor_chain(transaction, project_id, parent_id).await?;
    if let Some(index) = chain.iter().position(|id| *id == task.id) {
        let mut path = chain[..=index].to_vec();
        path.reverse();
        return Ok(Some(ParentProblem::Cycle { path }));
    }
    let below = levels_below(transaction, project_id, task.id).await?;
    if chain.len() + 1 + below > MAX_SUBTASK_LEVELS {
        return Ok(Some(ParentProblem::TooDeep));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Archive and restore

pub struct ArchiveCommand<'a> {
    pub expected_version: i64,
    pub archived: bool,
    pub summary: &'a str,
    pub audit_action: &'a str,
}

#[derive(Debug)]
pub enum ArchiveOutcome {
    Applied {
        version: i64,
        finalized: FinalizedChange,
        /// Subtasks still open under an archived task (at most 20).
        open_subtasks: Vec<Uuid>,
    },
    /// Already archived or restored; nothing changed and the version wasn't
    /// checked, as the web app has always answered.
    Unchanged(LockedWorkItem),
    VersionConflict(LockedWorkItem),
    /// The archived original of a task `task_move` carried to another
    /// project. Restoring it would make a live duplicate whose old key opens
    /// the copy.
    MovedAway {
        moved_to: Uuid,
    },
    NotFound,
}

/// The one way to archive or restore a task, for the web app and MCP.
/// Archiving releases the task's claim.
pub async fn set_archived(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    command: ArchiveCommand<'_>,
) -> Result<ArchiveOutcome, AppError> {
    let Some(current) = lock_work_item(transaction, actor.project_id, work_item_id).await? else {
        return Ok(ArchiveOutcome::NotFound);
    };
    if current.archived == command.archived {
        return Ok(ArchiveOutcome::Unchanged(current));
    }
    if current.version != command.expected_version {
        return Ok(ArchiveOutcome::VersionConflict(current));
    }
    if !command.archived
        && let Some(moved_to) = sqlx::query_scalar::<_, Uuid>(
            "SELECT work_item_id FROM work_item_key_aliases WHERE source_work_item_id = $1 LIMIT 1",
        )
        .bind(work_item_id)
        .fetch_optional(&mut **transaction)
        .await?
    {
        return Ok(ArchiveOutcome::MovedAway { moved_to });
    }
    let updated = sqlx::query(
        r#"
        UPDATE work_items
        SET archived_at = CASE WHEN $3 THEN now() END,
            archived_by_actor_id = CASE WHEN $3 THEN $4::uuid END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $5
        "#,
    )
    .bind(actor.project_id)
    .bind(work_item_id)
    .bind(command.archived)
    .bind(actor.actor_id)
    .bind(command.expected_version)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal(
            "locked Work Item archive lost its version",
        ));
    }
    let open_subtasks = if command.archived {
        sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1")
            .bind(work_item_id)
            .execute(&mut **transaction)
            .await?;
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM work_items
            WHERE project_id = $1 AND parent_work_item_id = $2 AND archived_at IS NULL
            ORDER BY number
            LIMIT 20
            "#,
        )
        .bind(actor.project_id)
        .bind(work_item_id)
        .fetch_all(&mut **transaction)
        .await?
    } else {
        Vec::new()
    };
    let version = command
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    let payload = serde_json::json!({"previousVersion": command.expected_version});
    let finalized = finalize_change(
        transaction,
        actor,
        work_item_id,
        ChangeFinalization {
            activity_kind: if command.archived {
                "work_item_archived"
            } else {
                "work_item_restored"
            },
            summary: command.summary,
            audit_action: command.audit_action,
            activity_payload: payload.clone(),
            audit_metadata: payload,
            version,
        },
    )
    .await?;
    Ok(ArchiveOutcome::Applied {
        version,
        finalized,
        open_subtasks,
    })
}

// ---------------------------------------------------------------------------
// Board order

/// Where a task goes in a status column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardPlacement {
    Top,
    Bottom,
    /// 0-based among the column's other open tasks.
    Index(usize),
    Before(Uuid),
    After(Uuid),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementProblem {
    AnchorIsSelf,
    /// The anchor isn't an open task in that column.
    AnchorNotInColumn {
        anchor: Uuid,
    },
    OutOfRange {
        open_tasks: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placed {
    pub board_rank: i64,
    /// 1-based, among the column's open tasks.
    pub position: i64,
    /// False when the task already sat there.
    pub moved: bool,
}

/// The index a task goes to among `others`, the rest of the column in board
/// order without the task itself.
pub fn target_index(
    others: &[(Uuid, i64)],
    work_item_id: Uuid,
    placement: BoardPlacement,
) -> Result<usize, PlacementProblem> {
    let anchor_index = |anchor: Uuid| {
        if anchor == work_item_id {
            return Err(PlacementProblem::AnchorIsSelf);
        }
        others
            .iter()
            .position(|(id, _)| *id == anchor)
            .ok_or(PlacementProblem::AnchorNotInColumn { anchor })
    };
    match placement {
        BoardPlacement::Top => Ok(0),
        BoardPlacement::Bottom => Ok(others.len()),
        BoardPlacement::Index(index) if index <= others.len() => Ok(index),
        BoardPlacement::Index(_) => Err(PlacementProblem::OutOfRange {
            open_tasks: others.len(),
        }),
        BoardPlacement::Before(anchor) => anchor_index(anchor),
        BoardPlacement::After(anchor) => anchor_index(anchor).map(|index| index + 1),
    }
}

/// Board ranks for `count` tasks placed at `index` among `rows` (the rest of
/// the column in board order, without the tasks being placed). None when
/// there's no room between the neighbours.
pub fn allocate_board_ranks(
    rows: &[(Uuid, i64)],
    index: usize,
    count: usize,
) -> Result<Option<Vec<i64>>, AppError> {
    if index > rows.len() || count == 0 {
        return Err(AppError::Internal("board placement out of range"));
    }
    let low = index.checked_sub(1).map_or(0, |previous| rows[previous].1);
    let Some(&(_, next)) = rows.get(index) else {
        let mut ranks = Vec::with_capacity(count);
        for step in 1..=count {
            let Some(rank) = i64::try_from(step)
                .ok()
                .and_then(|step| step.checked_mul(BOARD_RANK_STEP))
                .and_then(|offset| low.checked_add(offset))
            else {
                return Ok(None);
            };
            ranks.push(rank);
        }
        return Ok(Some(ranks));
    };
    let gap = i128::from(next) - i128::from(low);
    let slots = i128::try_from(count).map_err(|_| AppError::Internal("board placement count"))?;
    if gap <= slots {
        return Ok(None);
    }
    (1..=slots)
        .map(|step| {
            i64::try_from(i128::from(low) + gap * step / (slots + 1))
                .map_err(|_| AppError::Internal("board rank"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Renumbers `rows` to BOARD_RANK_STEP multiples, keeping their order and
/// writing only the rows whose rank changes. Nobody sees a different order,
/// so no version changes.
pub async fn rebalance_column(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    rows: &mut [(Uuid, i64)],
) -> Result<(), AppError> {
    for (index, (id, rank)) in rows.iter_mut().enumerate() {
        let balanced = i64::try_from(index + 1)
            .ok()
            .and_then(|value| value.checked_mul(BOARD_RANK_STEP))
            .ok_or(AppError::Internal("rebalance Work Item board rank"))?;
        if *rank != balanced {
            sqlx::query("UPDATE work_items SET board_rank = $3 WHERE project_id = $1 AND id = $2")
                .bind(project_id)
                .bind(*id)
                .bind(balanced)
                .execute(&mut **transaction)
                .await?;
            *rank = balanced;
        }
    }
    Ok(())
}

/// Ranks for `count` tasks at `index` among `rows`, rebalancing the column
/// first when there's no room.
pub async fn ranks_for_placement(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    rows: &mut [(Uuid, i64)],
    index: usize,
    count: usize,
) -> Result<Vec<i64>, AppError> {
    if let Some(ranks) = allocate_board_ranks(rows, index, count)? {
        return Ok(ranks);
    }
    rebalance_column(transaction, project_id, rows).await?;
    allocate_board_ranks(rows, index, count)?
        .ok_or(AppError::Internal("allocate Work Item board rank"))
}

/// Ranks `work_item_id` in the `status_id` column without writing the task's
/// own row. Locks the column's other open tasks (callers hold the project
/// lock first) and rebalances them when there's no gap, as the board's drag
/// always has. `current` is the task's present (status, rank), or None for a
/// task being created.
pub async fn place_in_column(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    status_id: Uuid,
    work_item_id: Uuid,
    current: Option<(Uuid, i64)>,
    placement: BoardPlacement,
) -> Result<Result<Placed, PlacementProblem>, AppError> {
    let mut others = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT id, board_rank FROM work_items
        WHERE project_id = $1 AND status_id = $2 AND id <> $3 AND archived_at IS NULL
        ORDER BY board_rank, id
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(status_id)
    .bind(work_item_id)
    .fetch_all(&mut **transaction)
    .await?;
    let index = match target_index(&others, work_item_id, placement) {
        Ok(index) => index,
        Err(problem) => return Ok(Err(problem)),
    };
    let position = i64::try_from(index + 1).map_err(|_| AppError::Internal("board position"))?;
    if let Some((current_status, current_rank)) = current
        && current_status == status_id
    {
        let current_index = others
            .iter()
            .filter(|(id, rank)| (*rank, *id) < (current_rank, work_item_id))
            .count();
        if current_index == index {
            return Ok(Ok(Placed {
                board_rank: current_rank,
                position,
                moved: false,
            }));
        }
    }
    let board_rank = ranks_for_placement(transaction, project_id, &mut others, index, 1).await?[0];
    Ok(Ok(Placed {
        board_rank,
        position,
        moved: true,
    }))
}

// ---------------------------------------------------------------------------
// Links to threads and repositories

/// How a link change is written down. The web app and MCP name the same
/// change differently in the audit log and the activity summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkWording {
    /// Prefix of every audit action, such as "work_item.mcp_" for
    /// "work_item.mcp_thread_linked".
    pub audit_prefix: &'static str,
    /// Appended to every activity summary, such as " through MCP".
    pub summary_suffix: &'static str,
}

impl LinkWording {
    pub const WEB: Self = Self {
        audit_prefix: "work_item.",
        summary_suffix: "",
    };
    pub const MCP: Self = Self {
        audit_prefix: "work_item.mcp_",
        summary_suffix: " through MCP",
    };

    fn audit_action(self, kind: &str) -> String {
        format!("{}{kind}", self.audit_prefix)
    }

    fn summary(self, summary: &str) -> String {
        format!("{summary}{}", self.summary_suffix)
    }
}

#[derive(Debug)]
pub enum ThreadLinkOutcome {
    /// No thread in the task's project that the actor can see.
    ThreadNotFound,
    Unchanged,
    /// The change would apply, but the task isn't at the version given.
    VersionConflict,
    Changed {
        version: i64,
        finalized: FinalizedChange,
    },
}

/// Links a task to a thread in its project, or unlinks it. The caller has
/// locked the project (`lock_membership`) and the task, and decided whether
/// an archived task may change: task_link refuses it, while the web app
/// still unlinks one (the thread page offers Unlink on archived tasks). A
/// call that changes nothing passes whatever version it names, so a retry
/// answers `Unchanged`; `expected_version` is checked only when something
/// would change.
pub async fn change_thread_link(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    current: &LockedWorkItem,
    expected_version: Option<i64>,
    conversation_id: Uuid,
    link: bool,
    wording: LinkWording,
) -> Result<ThreadLinkOutcome, AppError> {
    // No conversation query skips conversation_visible_to (AGENTS.md): a
    // thread in a private channel stays out of reach for non-members.
    let thread = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM conversations
            WHERE project_id = $1 AND id = $2 AND kind = 'thread'
              AND conversation_visible_to(id, $3)
        )
        "#,
    )
    .bind(actor.project_id)
    .bind(conversation_id)
    .bind(actor.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !thread {
        return Ok(ThreadLinkOutcome::ThreadNotFound);
    }
    let linked = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM work_item_conversation_links
            WHERE project_id = $1 AND work_item_id = $2 AND conversation_id = $3
        )
        "#,
    )
    .bind(actor.project_id)
    .bind(current.id)
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    if linked == link {
        return Ok(ThreadLinkOutcome::Unchanged);
    }
    if expected_version.is_some_and(|expected| expected != current.version) {
        return Ok(ThreadLinkOutcome::VersionConflict);
    }
    if link {
        sqlx::query(
            r#"
            INSERT INTO work_item_conversation_links (
                project_id, work_item_id, conversation_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(actor.project_id)
        .bind(current.id)
        .bind(conversation_id)
        .bind(actor.actor_id)
        .execute(&mut **transaction)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM work_item_conversation_links WHERE project_id = $1 AND work_item_id = $2 AND conversation_id = $3",
        )
        .bind(actor.project_id)
        .bind(current.id)
        .bind(conversation_id)
        .execute(&mut **transaction)
        .await?;
    }
    let version =
        advance_version(transaction, actor.project_id, current.id, current.version).await?;
    let (kind, summary) = if link {
        ("thread_linked", "Thread linked")
    } else {
        ("thread_unlinked", "Thread unlinked")
    };
    let payload = serde_json::json!({"conversationId": conversation_id});
    let finalized = finalize_change(
        transaction,
        actor,
        current.id,
        ChangeFinalization {
            activity_kind: kind,
            summary: &wording.summary(summary),
            audit_action: &wording.audit_action(kind),
            activity_payload: payload.clone(),
            audit_metadata: payload,
            version,
        },
    )
    .await?;
    Ok(ThreadLinkOutcome::Changed { version, finalized })
}

#[derive(Debug)]
pub enum RepositoryLinkOutcome {
    /// No such repository in the task's project.
    RepositoryNotFound,
    /// Only an enabled, trusted, ready and active repository can be primary.
    NotEligibleForPrimary {
        enabled: bool,
        trusted: bool,
        preparation_state: String,
        archived: bool,
    },
    /// A new link to an archived repository. Unlinking one, or making it
    /// related, still works.
    RepositoryArchived,
    Unchanged,
    /// The change would apply, but the task isn't at the version given.
    VersionConflict,
    Changed {
        version: i64,
        /// The repository that was primary until this made another one
        /// primary; it's related now.
        demoted_primary: Option<Uuid>,
        finalized: FinalizedChange,
    },
}

/// Links a task to a repository in its project as `primary` or `related`, or
/// unlinks it (`desired_role` None). Making one primary makes the old
/// primary related, in the same change. The caller has locked the project
/// and the task and refused an archived task; `expected_version` is checked
/// only when something would change, as in `change_thread_link`.
pub async fn change_repository_link(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    current: &LockedWorkItem,
    expected_version: Option<i64>,
    repository_id: Uuid,
    desired_role: Option<&str>,
    wording: LinkWording,
) -> Result<RepositoryLinkOutcome, AppError> {
    if desired_role.is_some_and(|role| !matches!(role, "primary" | "related")) {
        return Err(AppError::Validation("repository role"));
    }
    let Some((enabled, trusted, preparation_state, archived)) =
        sqlx::query_as::<_, (bool, bool, String, bool)>(
            r#"
            SELECT enabled, trusted, preparation_state, archived_at IS NOT NULL
            FROM repositories WHERE project_id = $1 AND id = $2
            "#,
        )
        .bind(actor.project_id)
        .bind(repository_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(RepositoryLinkOutcome::RepositoryNotFound);
    };
    if desired_role == Some("primary")
        && (!enabled || !trusted || preparation_state != "ready" || archived)
    {
        return Ok(RepositoryLinkOutcome::NotEligibleForPrimary {
            enabled,
            trusted,
            preparation_state,
            archived,
        });
    }
    let existing_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM work_item_repository_links WHERE project_id = $1 AND work_item_id = $2 AND repository_id = $3",
    )
    .bind(actor.project_id)
    .bind(current.id)
    .bind(repository_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if desired_role.is_some() && existing_role.is_none() && archived {
        return Ok(RepositoryLinkOutcome::RepositoryArchived);
    }
    if existing_role.as_deref() == desired_role {
        return Ok(RepositoryLinkOutcome::Unchanged);
    }
    if expected_version.is_some_and(|expected| expected != current.version) {
        return Ok(RepositoryLinkOutcome::VersionConflict);
    }
    let mut demoted_primary = None;
    match desired_role {
        None => {
            sqlx::query(
                "DELETE FROM work_item_repository_links WHERE project_id = $1 AND work_item_id = $2 AND repository_id = $3",
            )
            .bind(actor.project_id)
            .bind(current.id)
            .bind(repository_id)
            .execute(&mut **transaction)
            .await?;
        }
        Some(role) => {
            if role == "primary" {
                demoted_primary = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    UPDATE work_item_repository_links
                    SET role = 'related', updated_at = now()
                    WHERE project_id = $1 AND work_item_id = $2 AND role = 'primary'
                      AND repository_id <> $3
                    RETURNING repository_id
                    "#,
                )
                .bind(actor.project_id)
                .bind(current.id)
                .bind(repository_id)
                .fetch_optional(&mut **transaction)
                .await?;
            }
            sqlx::query(
                r#"
                INSERT INTO work_item_repository_links (
                    project_id, work_item_id, repository_id, role, created_by_actor_id
                ) VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (work_item_id, repository_id)
                DO UPDATE SET role = EXCLUDED.role, updated_at = now()
                "#,
            )
            .bind(actor.project_id)
            .bind(current.id)
            .bind(repository_id)
            .bind(role)
            .bind(actor.actor_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    let version =
        advance_version(transaction, actor.project_id, current.id, current.version).await?;
    let (kind, summary) = match desired_role {
        None => ("repository_unlinked", "Repository unlinked"),
        Some("primary") if existing_role.is_some() => {
            ("repository_primary_changed", "Primary repository changed")
        }
        _ => ("repository_linked", "Repository linked"),
    };
    let payload = serde_json::json!({"repositoryId": repository_id, "role": desired_role});
    let finalized = finalize_change(
        transaction,
        actor,
        current.id,
        ChangeFinalization {
            activity_kind: kind,
            summary: &wording.summary(summary),
            audit_action: &wording.audit_action(kind),
            activity_payload: payload.clone(),
            audit_metadata: payload,
            version,
        },
    )
    .await?;
    Ok(RepositoryLinkOutcome::Changed {
        version,
        demoted_primary,
        finalized,
    })
}

// ---------------------------------------------------------------------------
// Acceptance criteria

/// Trims a criterion and checks it: 1 to 1,000 characters, with newlines and
/// tabs allowed but no other control characters.
pub fn normalize_criterion_text(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_CRITERION_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("criterion text"));
    }
    Ok(value.to_owned())
}

/// Inserts criteria (already normalized) in order, the first at
/// `start_position` (1-based). Whoever writes them is recorded as creating
/// them, and as completing the ones marked done.
pub async fn insert_criteria(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    start_position: i32,
    criteria: &[(String, bool)],
) -> Result<(), AppError> {
    if criteria.is_empty() {
        return Ok(());
    }
    let last = usize::try_from(start_position)
        .ok()
        .filter(|start| *start >= 1)
        .and_then(|start| start.checked_add(criteria.len() - 1));
    if last.is_none_or(|last| last > MAX_CRITERIA_PER_WORK_ITEM) {
        return Err(AppError::Validation("acceptanceCriteria"));
    }
    let mut rows = Vec::with_capacity(criteria.len());
    for (offset, (text, completed)) in criteria.iter().enumerate() {
        let position = i32::try_from(offset)
            .ok()
            .and_then(|offset| start_position.checked_add(offset))
            .ok_or(AppError::Validation("acceptanceCriteria"))?;
        rows.push(NewCriterionRow {
            id: Uuid::now_v7(),
            text: text.clone(),
            position,
            completed: *completed,
        });
    }
    insert_criterion_rows(transaction, project_id, work_item_id, actor_id, &rows).await
}

/// A criterion row to insert, with its ID and place already chosen.
struct NewCriterionRow {
    id: Uuid,
    text: String,
    position: i32,
    completed: bool,
}

/// The one INSERT for acceptance criteria, used by creates, moves and
/// criteria changes alike.
async fn insert_criterion_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    rows: &[NewCriterionRow],
) -> Result<(), AppError> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let texts = rows.iter().map(|row| row.text.clone()).collect::<Vec<_>>();
    let positions = rows.iter().map(|row| row.position).collect::<Vec<_>>();
    let completed = rows.iter().map(|row| row.completed).collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO work_item_acceptance_criteria (
            id, project_id, work_item_id, text, position, completed,
            completed_by_actor_id, completed_at, created_by_actor_id
        )
        SELECT item.id, $1, $2, item.text, item.position, item.completed,
               CASE WHEN item.completed THEN $3::uuid END,
               CASE WHEN item.completed THEN now() END, $3
        FROM unnest($4::uuid[], $5::text[], $6::int[], $7::bool[])
             AS item(id, text, position, completed)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .bind(&ids)
    .bind(&texts)
    .bind(&positions)
    .bind(&completed)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A criterion as it stands. A list of them is in position order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CriterionState {
    pub id: Uuid,
    pub text: String,
    pub completed: bool,
}

/// Which criterion an operation names, as of the version the caller read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CriterionTarget {
    Id(Uuid),
    /// 1-based, in the list at the version the caller read. Every criteria
    /// change bumps the task's version, so this is unambiguous.
    Position(i32),
}

/// One change to a task's acceptance criteria. `position` on add and update
/// counts in the list as it stands after the operations before it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CriteriaOperation {
    /// Text already normalized. None puts it at the end.
    Add {
        text: String,
        position: Option<i32>,
    },
    Update {
        target: CriterionTarget,
        /// Already normalized.
        text: Option<String>,
        position: Option<i32>,
        completed: Option<bool>,
    },
    Remove {
        target: CriterionTarget,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CriterionChangeKind {
    Created,
    Updated,
    Completed,
    Reopened,
    Removed,
}

impl CriterionChangeKind {
    /// The activity kind, as the web app has always written it.
    pub fn activity_kind(self) -> &'static str {
        match self {
            Self::Created => "criterion_created",
            Self::Updated => "criterion_updated",
            Self::Completed => "criterion_completed",
            Self::Reopened => "criterion_reopened",
            Self::Removed => "criterion_removed",
        }
    }

    /// The activity summary, identical whichever app made the change.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Created => "Acceptance criterion added",
            Self::Updated => "Acceptance criterion updated",
            Self::Completed => "Acceptance criterion completed",
            Self::Reopened => "Acceptance criterion reopened",
            Self::Removed => "Acceptance criterion removed",
        }
    }
}

/// What one operation does. Plans have exactly one per operation, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedCriterionChange {
    pub operation_index: usize,
    pub criterion_id: Uuid,
    pub kind: CriterionChangeKind,
    /// False when the criterion was already that way, or a later operation
    /// in the call set its completion back: no activity is written.
    pub changed: bool,
    /// 1-based, after the operation; for a removal, where it was.
    pub position: i32,
    /// For an update: "text" and/or "position".
    pub fields: Vec<&'static str>,
}

impl PlannedCriterionChange {
    /// The activity payload: IDs and positions only, never the text, so
    /// taking down pasted text also takes it out of the history.
    pub fn activity_payload(&self) -> serde_json::Value {
        match self.kind {
            CriterionChangeKind::Created | CriterionChangeKind::Removed => {
                serde_json::json!({"criterionId": self.criterion_id, "position": self.position})
            }
            CriterionChangeKind::Updated => serde_json::json!({
                "criterionId": self.criterion_id,
                "position": self.position,
                "fields": self.fields,
            }),
            CriterionChangeKind::Completed | CriterionChangeKind::Reopened => {
                serde_json::json!({"criterionId": self.criterion_id})
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CriteriaPlanError {
    /// The operation names a criterion that isn't in the snapshot.
    UnknownCriterion { operation_index: usize },
    /// The operation names a criterion an earlier operation removed.
    RemovedEarlier {
        operation_index: usize,
        removed_by: usize,
    },
    /// The position is outside 1..=max.
    Position { operation_index: usize, max: i32 },
    /// The task would have more than MAX_CRITERIA_PER_WORK_ITEM.
    TooMany { count: usize },
}

/// Applies `operations` in order to `snapshot` (the list at the version the
/// caller read) and returns the resulting list and what each operation did.
/// Targets name criteria in the snapshot; new criteria can't be targeted,
/// since callers don't know their IDs yet. Nothing is written.
pub fn plan_criteria(
    snapshot: &[CriterionState],
    operations: &[CriteriaOperation],
    mut new_id: impl FnMut() -> Uuid,
) -> Result<(Vec<CriterionState>, Vec<PlannedCriterionChange>), CriteriaPlanError> {
    let mut list = snapshot.to_vec();
    let mut removed: HashMap<Uuid, usize> = HashMap::new();
    let mut changes = Vec::with_capacity(operations.len());
    let length = |list: &[CriterionState]| i32::try_from(list.len()).unwrap_or(i32::MAX);
    let resolve = |operation_index: usize,
                   target: CriterionTarget,
                   removed: &HashMap<Uuid, usize>|
     -> Result<Uuid, CriteriaPlanError> {
        let found = match target {
            CriterionTarget::Id(id) => snapshot.iter().find(|criterion| criterion.id == id),
            CriterionTarget::Position(position) => usize::try_from(position)
                .ok()
                .and_then(|position| position.checked_sub(1))
                .and_then(|index| snapshot.get(index)),
        };
        let id = found
            .map(|criterion| criterion.id)
            .ok_or(CriteriaPlanError::UnknownCriterion { operation_index })?;
        match removed.get(&id) {
            Some(&removed_by) => Err(CriteriaPlanError::RemovedEarlier {
                operation_index,
                removed_by,
            }),
            None => Ok(id),
        }
    };
    let index_of = |list: &[CriterionState], id: Uuid, operation_index: usize| {
        list.iter()
            .position(|criterion| criterion.id == id)
            .ok_or(CriteriaPlanError::UnknownCriterion { operation_index })
    };
    for (operation_index, operation) in operations.iter().enumerate() {
        match operation {
            CriteriaOperation::Add { text, position } => {
                let max = length(&list).saturating_add(1);
                let position = position.unwrap_or(max);
                if !(1..=max).contains(&position) {
                    return Err(CriteriaPlanError::Position {
                        operation_index,
                        max,
                    });
                }
                let id = new_id();
                list.insert(
                    usize::try_from(position - 1).unwrap_or(list.len()),
                    CriterionState {
                        id,
                        text: text.clone(),
                        completed: false,
                    },
                );
                changes.push(PlannedCriterionChange {
                    operation_index,
                    criterion_id: id,
                    kind: CriterionChangeKind::Created,
                    changed: true,
                    position,
                    fields: Vec::new(),
                });
            }
            CriteriaOperation::Update {
                target,
                text,
                position,
                completed,
            } => {
                let id = resolve(operation_index, *target, &removed)?;
                let index = index_of(&list, id, operation_index)?;
                let max = length(&list);
                if let Some(position) = position
                    && !(1..=max).contains(position)
                {
                    return Err(CriteriaPlanError::Position {
                        operation_index,
                        max,
                    });
                }
                let current_position = i32::try_from(index + 1).unwrap_or(i32::MAX);
                let target_position = position.unwrap_or(current_position);
                let mut criterion = list.remove(index);
                let mut fields = Vec::new();
                if let Some(text) = text
                    && *text != criterion.text
                {
                    criterion.text = text.clone();
                    fields.push("text");
                }
                if target_position != current_position {
                    fields.push("position");
                }
                let completion = completed.filter(|completed| *completed != criterion.completed);
                if let Some(completed) = completion {
                    criterion.completed = completed;
                }
                list.insert(
                    usize::try_from(target_position - 1).unwrap_or(list.len()),
                    criterion,
                );
                // As in the web app, a change of completion is what the one
                // activity records, even when the text or place changed too.
                let kind = match completion {
                    Some(true) => CriterionChangeKind::Completed,
                    Some(false) => CriterionChangeKind::Reopened,
                    None => CriterionChangeKind::Updated,
                };
                changes.push(PlannedCriterionChange {
                    operation_index,
                    criterion_id: id,
                    kind,
                    changed: completion.is_some() || !fields.is_empty(),
                    position: target_position,
                    fields,
                });
            }
            CriteriaOperation::Remove { target } => {
                let id = resolve(operation_index, *target, &removed)?;
                let index = index_of(&list, id, operation_index)?;
                list.remove(index);
                removed.insert(id, operation_index);
                changes.push(PlannedCriterionChange {
                    operation_index,
                    criterion_id: id,
                    kind: CriterionChangeKind::Removed,
                    changed: true,
                    position: i32::try_from(index + 1).unwrap_or(i32::MAX),
                    fields: Vec::new(),
                });
            }
        }
    }
    if list.len() > MAX_CRITERIA_PER_WORK_ITEM {
        return Err(CriteriaPlanError::TooMany { count: list.len() });
    }
    record_net_completion(snapshot, &list, &mut changes);
    Ok((list, changes))
}

/// Records each kept criterion's completion once, as the call's net effect,
/// which is also what `persist_criteria` writes: unchecking a criterion and
/// checking it again leaves it as it was, completer included. An operation
/// whose check or uncheck a later one in the call undid, or that a later one
/// repeated, records no completion: it records its text or place change
/// instead, or nothing. So history never credits a check to someone who
/// left the criterion as they found it. A criterion the call removes keeps
/// what each operation did.
fn record_net_completion(
    snapshot: &[CriterionState],
    planned: &[CriterionState],
    changes: &mut [PlannedCriterionChange],
) {
    let completion = |kind| {
        matches!(
            kind,
            CriterionChangeKind::Completed | CriterionChangeKind::Reopened
        )
    };
    let before = snapshot
        .iter()
        .map(|criterion| (criterion.id, criterion.completed))
        .collect::<HashMap<_, _>>();
    let after = planned
        .iter()
        .map(|criterion| (criterion.id, criterion.completed))
        .collect::<HashMap<_, _>>();
    let mut last = HashMap::new();
    for (index, change) in changes.iter().enumerate() {
        if completion(change.kind) {
            last.insert(change.criterion_id, index);
        }
    }
    for (index, change) in changes.iter_mut().enumerate() {
        let Some(&completed) = after.get(&change.criterion_id) else {
            continue;
        };
        if completion(change.kind)
            && (before.get(&change.criterion_id) == Some(&completed)
                || last.get(&change.criterion_id) != Some(&index))
        {
            change.kind = CriterionChangeKind::Updated;
            change.changed = !change.fields.is_empty();
        }
    }
}

pub struct CriteriaCommand<'a> {
    pub expected_version: i64,
    pub operations: &'a [CriteriaOperation],
    pub audit_action: &'a str,
}

#[derive(Debug)]
pub enum CriteriaOutcome {
    Applied {
        version: i64,
        changes: Vec<PlannedCriterionChange>,
        finalized: FinalizedBatch,
    },
    /// Every operation found the criteria already that way: no version bump,
    /// no activity, and no audit row (the caller may write one).
    Unchanged {
        current: LockedWorkItem,
        changes: Vec<PlannedCriterionChange>,
    },
    /// The version is stale or the task is archived (`conflict_kind`).
    Conflict(LockedWorkItem),
    NotFound,
    /// Nothing was written. `criteria` is how many the task has.
    Plan {
        error: CriteriaPlanError,
        criteria: usize,
    },
}

/// Changes a task's acceptance criteria, all operations or none, for the
/// web app and MCP alike. The caller has locked the project
/// (`lock_membership`) and checked the actor may edit tasks there. However
/// many operations apply, the task gets one version bump, one outbox event
/// and one audit row, with an activity for each operation that changed
/// something.
pub async fn change_criteria(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    command: CriteriaCommand<'_>,
) -> Result<CriteriaOutcome, AppError> {
    let Some(current) = lock_work_item(transaction, actor.project_id, work_item_id).await? else {
        return Ok(CriteriaOutcome::NotFound);
    };
    if conflict_kind(&current, command.expected_version).is_some() {
        return Ok(CriteriaOutcome::Conflict(current));
    }
    let snapshot = sqlx::query_as::<_, (Uuid, String, bool)>(
        r#"
        SELECT id, text, completed FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY position
        FOR UPDATE
        "#,
    )
    .bind(actor.project_id)
    .bind(work_item_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, text, completed)| CriterionState {
        id,
        text,
        completed,
    })
    .collect::<Vec<_>>();
    let (planned, changes) = match plan_criteria(&snapshot, command.operations, Uuid::now_v7) {
        Ok(plan) => plan,
        Err(error) => {
            return Ok(CriteriaOutcome::Plan {
                error,
                criteria: snapshot.len(),
            });
        }
    };
    if !changes.iter().any(|change| change.changed) {
        return Ok(CriteriaOutcome::Unchanged { current, changes });
    }
    persist_criteria(transaction, actor, work_item_id, &snapshot, &planned).await?;
    let version = advance_version(
        transaction,
        actor.project_id,
        work_item_id,
        command.expected_version,
    )
    .await?;
    let changed = changes
        .iter()
        .filter(|change| change.changed)
        .collect::<Vec<_>>();
    let activities = changed
        .iter()
        .map(|change| ActivityRecord {
            kind: change.kind.activity_kind(),
            summary: change.kind.summary(),
            payload: change.activity_payload(),
        })
        .collect::<Vec<_>>();
    let audit_changes = changed
        .iter()
        .map(|change| {
            let mut payload = change.activity_payload();
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "kind".to_owned(),
                    serde_json::json!(change.kind.activity_kind()),
                );
            }
            payload
        })
        .collect::<Vec<_>>();
    let finalized = finalize_changes(
        transaction,
        actor,
        work_item_id,
        &activities,
        command.audit_action,
        serde_json::json!({"changes": audit_changes, "version": version}),
        version,
    )
    .await?;
    Ok(CriteriaOutcome::Applied {
        version,
        changes,
        finalized,
    })
}

/// Writes the net difference between `snapshot` and `planned`: removed rows
/// go, changed rows take their final text, place and completion, and new
/// rows go in at their final places. Writing final positions directly keeps
/// every row inside the position CHECK; the unique position constraint is
/// checked again before returning, so a clash surfaces here, not at commit.
async fn persist_criteria(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    snapshot: &[CriterionState],
    planned: &[CriterionState],
) -> Result<(), AppError> {
    let before = snapshot
        .iter()
        .enumerate()
        .map(|(index, criterion)| (criterion.id, (index, criterion)))
        .collect::<HashMap<_, _>>();
    let kept = planned
        .iter()
        .map(|criterion| criterion.id)
        .collect::<std::collections::HashSet<_>>();
    let removed = snapshot
        .iter()
        .filter(|criterion| !kept.contains(&criterion.id))
        .map(|criterion| criterion.id)
        .collect::<Vec<_>>();
    let mut updated = Vec::new();
    let mut inserted = Vec::new();
    for (index, criterion) in planned.iter().enumerate() {
        let position = i32::try_from(index + 1).map_err(|_| AppError::Validation("position"))?;
        match before.get(&criterion.id) {
            Some((was, old)) => {
                if *was != index
                    || old.text != criterion.text
                    || old.completed != criterion.completed
                {
                    updated.push((criterion, position));
                }
            }
            None => inserted.push(NewCriterionRow {
                id: criterion.id,
                text: criterion.text.clone(),
                position,
                completed: criterion.completed,
            }),
        }
    }
    if removed.is_empty() && updated.is_empty() && inserted.is_empty() {
        return Ok(());
    }
    sqlx::query("SET CONSTRAINTS work_item_acceptance_criteria_position_unique DEFERRED")
        .execute(&mut **transaction)
        .await?;
    if !removed.is_empty() {
        sqlx::query(
            "DELETE FROM work_item_acceptance_criteria WHERE project_id = $1 AND work_item_id = $2 AND id = ANY($3)",
        )
        .bind(actor.project_id)
        .bind(work_item_id)
        .bind(&removed)
        .execute(&mut **transaction)
        .await?;
    }
    if !updated.is_empty() {
        let ids = updated
            .iter()
            .map(|(criterion, _)| criterion.id)
            .collect::<Vec<_>>();
        let texts = updated
            .iter()
            .map(|(criterion, _)| criterion.text.clone())
            .collect::<Vec<_>>();
        let positions = updated
            .iter()
            .map(|(_, position)| *position)
            .collect::<Vec<_>>();
        let completed = updated
            .iter()
            .map(|(criterion, _)| criterion.completed)
            .collect::<Vec<_>>();
        // Someone who checks a criterion is its completer; one already
        // checked keeps whoever checked it.
        sqlx::query(
            r#"
            UPDATE work_item_acceptance_criteria criterion
            SET text = change.text, position = change.position, completed = change.completed,
                completed_by_actor_id = CASE WHEN change.completed THEN
                    CASE WHEN criterion.completed THEN criterion.completed_by_actor_id ELSE $3 END
                END,
                completed_at = CASE WHEN change.completed THEN
                    CASE WHEN criterion.completed THEN criterion.completed_at ELSE now() END
                END,
                updated_at = now()
            FROM unnest($4::uuid[], $5::text[], $6::int[], $7::bool[])
                 AS change(id, text, position, completed)
            WHERE criterion.project_id = $1 AND criterion.work_item_id = $2
              AND criterion.id = change.id
            "#,
        )
        .bind(actor.project_id)
        .bind(work_item_id)
        .bind(actor.actor_id)
        .bind(&ids)
        .bind(&texts)
        .bind(&positions)
        .bind(&completed)
        .execute(&mut **transaction)
        .await?;
    }
    insert_criterion_rows(
        transaction,
        actor.project_id,
        work_item_id,
        actor.actor_id,
        &inserted,
    )
    .await?;
    sqlx::query("SET CONSTRAINTS work_item_acceptance_criteria_position_unique IMMEDIATE")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Activity payloads

/// What one change did to a task, in the activity payload shape history
/// reads. Keys appear only when they carry something. Labels and links are
/// stored by ID, never by name or text.
#[derive(Clone, Debug, Default)]
pub struct ChangePayload {
    pub changed_fields: Vec<String>,
    /// (previous status, new status).
    pub status: Option<(Uuid, Uuid)>,
    /// The task's new 1-based place in its column, when it was placed.
    pub position: Option<i64>,
    /// (previous parent, new parent).
    pub parent: Option<(Option<Uuid>, Option<Uuid>)>,
    pub labels_added: Vec<Uuid>,
    pub labels_removed: Vec<Uuid>,
    pub link: Option<LinkChange>,
    /// Before and after of each changed field (work_item_history).
    pub changes: FieldChanges,
}

#[derive(Clone, Debug)]
pub struct LinkChange {
    pub kind: String,
    pub id: Uuid,
    pub linked: bool,
}

impl ChangePayload {
    pub fn activity(&self) -> serde_json::Value {
        let mut payload = serde_json::Map::new();
        if !self.changed_fields.is_empty() {
            payload.insert(
                "changedFields".to_owned(),
                serde_json::json!(self.changed_fields),
            );
        }
        if let Some((previous, new)) = self.status {
            payload.insert("previousStatusId".to_owned(), serde_json::json!(previous));
            payload.insert("statusId".to_owned(), serde_json::json!(new));
        }
        if let Some(position) = self.position {
            payload.insert("position".to_owned(), serde_json::json!(position));
        }
        if let Some((from, to)) = self.parent {
            payload.insert(
                "parent".to_owned(),
                serde_json::json!({"from": from, "to": to}),
            );
        }
        if !self.labels_added.is_empty() {
            payload.insert(
                "labelsAdded".to_owned(),
                serde_json::json!(self.labels_added),
            );
        }
        if !self.labels_removed.is_empty() {
            payload.insert(
                "labelsRemoved".to_owned(),
                serde_json::json!(self.labels_removed),
            );
        }
        if let Some(link) = &self.link {
            payload.insert(
                "link".to_owned(),
                serde_json::json!({"kind": link.kind, "id": link.id}),
            );
            payload.insert("linked".to_owned(), serde_json::json!(link.linked));
        }
        if !self.changes.is_empty() {
            payload.insert(
                "changes".to_owned(),
                serde_json::Value::Object(self.changes.clone().into_map()),
            );
        }
        serde_json::Value::Object(payload)
    }

    /// The activity payload plus the version the change produced.
    pub fn audit(&self, version: i64) -> serde_json::Value {
        let mut metadata = self.activity();
        if let Some(metadata) = metadata.as_object_mut() {
            metadata.insert("version".to_owned(), serde_json::json!(version));
        }
        metadata
    }
}

#[derive(Clone, Copy)]
enum DefinitionKind {
    Type,
    Status,
}

async fn select_definition(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    kind: DefinitionKind,
    requested: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let query = match kind {
        DefinitionKind::Type => {
            "SELECT id FROM work_item_types WHERE project_id = $1 AND enabled AND (($2::uuid IS NOT NULL AND id = $2) OR ($2::uuid IS NULL AND is_default))"
        }
        DefinitionKind::Status => {
            "SELECT id FROM work_item_statuses WHERE project_id = $1 AND enabled AND (($2::uuid IS NOT NULL AND id = $2) OR ($2::uuid IS NULL AND is_default))"
        }
    };
    sqlx::query_scalar(query)
        .bind(project_id)
        .bind(requested)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::Validation("Work Item definition"))
}

/// Whether `milestone_id` is an active milestone of the project.
pub async fn milestone_is_active(
    connection: &mut PgConnection,
    project_id: Uuid,
    milestone_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM project_milestones WHERE project_id = $1 AND id = $2 AND archived_at IS NULL)",
    )
    .bind(project_id)
    .bind(milestone_id)
    .fetch_one(&mut *connection)
    .await?)
}

pub fn valid_estimate(estimate: f64) -> bool {
    estimate.is_finite() && (0.0..=MAX_ESTIMATE).contains(&estimate)
}

/// Starts `actor_id` watching a task, if they're an enabled member. People
/// and agents who aren't (anymore) are skipped rather than refused, since
/// watching follows from another change.
pub async fn add_watcher(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO work_item_watchers (project_id, work_item_id, actor_id)
        SELECT $1, $2, membership.actor_id
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1 AND membership.actor_id = $3
          AND membership.enabled AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Tells agents watching a task that it changed, through their connection's
/// events. The assignee hears already, and nobody hears about their own
/// change. People hear through notifications, which read the outbox.
pub async fn notify_watching_agents(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
) -> Result<(), AppError> {
    let watchers = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT watcher.actor_id
        FROM work_item_watchers watcher
        JOIN actors actor ON actor.id = watcher.actor_id AND actor.kind = 'external_agent'
        JOIN work_items item ON item.id = watcher.work_item_id
        WHERE watcher.work_item_id = $1 AND watcher.actor_id <> $2
          AND item.assignee_actor_id IS DISTINCT FROM watcher.actor_id
        ORDER BY watcher.actor_id
        "#,
    )
    .bind(work_item_id)
    .bind(actor.actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    for watcher in watchers {
        enqueue_external_work_item_event(
            transaction,
            actor.project_id,
            work_item_id,
            Some(watcher),
            "work_item.changed",
            "A Work Item you watch changed",
        )
        .await?;
    }
    Ok(())
}

pub fn conflict_kind(current: &LockedWorkItem, expected_version: i64) -> Option<ConflictKind> {
    if current.version != expected_version {
        Some(ConflictKind::Version)
    } else if current.archived {
        Some(ConflictKind::Archived)
    } else {
        None
    }
}

pub async fn validate_assignee(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let Some(actor_id) = actor_id else {
        return Ok(true);
    };
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            WHERE membership.project_id = $1
              AND membership.actor_id = $2
              AND membership.enabled
              AND actor.status = 'active'
              AND actor.kind IN ('human', 'external_agent')
        )
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_one(&mut *connection)
    .await?)
}

pub async fn transition_allowed(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    from: Uuid,
    to: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_item_status_transitions WHERE project_id = $1 AND from_status_id = $2 AND to_status_id = $3)",
    )
    .bind(project_id)
    .bind(from)
    .bind(to)
    .fetch_one(&mut **transaction)
    .await?)
}

pub async fn advance_version(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    expected: i64,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        "UPDATE work_items SET version = version + 1, updated_at = now() WHERE project_id = $1 AND id = $2 AND version = $3 RETURNING version",
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(expected)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Internal("locked Work Item version changed"))
}

/// Records a change: its activity, the outbox event, a note for an assigned
/// agent, and the audit row. Batch previews roll all of it back, so it must
/// only ever write in this transaction.
pub async fn finalize_change(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    finalization: ChangeFinalization<'_>,
) -> Result<FinalizedChange, AppError> {
    let finalized = finalize_changes(
        transaction,
        actor,
        work_item_id,
        &[ActivityRecord {
            kind: finalization.activity_kind,
            summary: finalization.summary,
            payload: finalization.activity_payload,
        }],
        finalization.audit_action,
        finalization.audit_metadata,
        finalization.version,
    )
    .await?;
    Ok(FinalizedChange {
        activity_id: finalized.activity_ids[0],
        audit_id: finalized.audit_id,
    })
}

pub struct ActivityRecord<'a> {
    pub kind: &'a str,
    pub summary: &'a str,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct FinalizedBatch {
    pub activity_ids: Vec<Uuid>,
    pub audit_id: Uuid,
}

/// Records one change to a task that shows as several activity entries (such
/// as several acceptance criteria at once): an activity row each, then one
/// outbox event, one note for an assigned agent, and one audit row.
#[allow(clippy::too_many_arguments)]
pub async fn finalize_changes(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    activities: &[ActivityRecord<'_>],
    audit_action: &str,
    audit_metadata: serde_json::Value,
    version: i64,
) -> Result<FinalizedBatch, AppError> {
    if activities.is_empty() {
        return Err(AppError::Internal("a task change needs an activity"));
    }
    let mut activity_ids = Vec::with_capacity(activities.len());
    for activity in activities {
        activity_ids.push(
            insert_activity(
                transaction,
                actor.project_id,
                work_item_id,
                actor.actor_id,
                activity.kind,
                activity.summary,
                &activity.payload,
            )
            .await?,
        );
    }
    enqueue_update(transaction, actor.project_id, work_item_id, version).await?;
    enqueue_external_work_item_event(
        transaction,
        actor.project_id,
        work_item_id,
        None,
        "work_item.changed",
        "An assigned Work Item changed",
    )
    .await?;
    notify_watching_agents(transaction, actor, work_item_id).await?;
    let audit_id = record_audit(
        transaction,
        actor,
        audit_action,
        work_item_id,
        audit_metadata,
    )
    .await?;
    Ok(FinalizedBatch {
        activity_ids,
        audit_id,
    })
}

struct CreatedFinalization<'a> {
    work_item_id: Uuid,
    assignee_actor_id: Option<Uuid>,
    activity_summary: &'a str,
    activity_metadata: serde_json::Value,
    audit_action: &'a str,
    audit_metadata: serde_json::Value,
}

async fn finalize_created(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    finalization: CreatedFinalization<'_>,
) -> Result<FinalizedChange, AppError> {
    let activity_id = insert_activity(
        transaction,
        actor.project_id,
        finalization.work_item_id,
        actor.actor_id,
        "work_item_created",
        finalization.activity_summary,
        &finalization.activity_metadata,
    )
    .await?;
    enqueue_update(transaction, actor.project_id, finalization.work_item_id, 1).await?;
    enqueue_external_work_item_event(
        transaction,
        actor.project_id,
        finalization.work_item_id,
        finalization.assignee_actor_id,
        "work_item.assigned",
        "A Work Item was assigned to you",
    )
    .await?;
    let audit_id = record_audit(
        transaction,
        actor,
        finalization.audit_action,
        finalization.work_item_id,
        finalization.audit_metadata,
    )
    .await?;
    Ok(FinalizedChange {
        activity_id,
        audit_id,
    })
}

pub async fn finalize_comment(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    comment_id: Uuid,
    version: i64,
    audit_action: &str,
) -> Result<(i64, Uuid), AppError> {
    let sequence =
        insert_comment_timeline(transaction, actor.project_id, work_item_id, comment_id).await?;
    enqueue_update(transaction, actor.project_id, work_item_id, version).await?;
    enqueue_external_work_item_event(
        transaction,
        actor.project_id,
        work_item_id,
        None,
        "work_item.changed",
        "An assigned Work Item changed",
    )
    .await?;
    notify_watching_agents(transaction, actor, work_item_id).await?;
    let audit_id = record_audit(
        transaction,
        actor,
        audit_action,
        work_item_id,
        serde_json::json!({"commentId": comment_id, "sequence": sequence}),
    )
    .await?;
    Ok((sequence, audit_id))
}

/// Records a comment copied in bulk (a task move carrying its history over)
/// without telling anyone about each one. The caller's own change, committed
/// in the same transaction, already sends the update and the agent events.
pub async fn finalize_copied_comment(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    comment_id: Uuid,
    audit_action: &str,
) -> Result<(), AppError> {
    let sequence =
        insert_comment_timeline(transaction, actor.project_id, work_item_id, comment_id).await?;
    record_audit(
        transaction,
        actor,
        audit_action,
        work_item_id,
        serde_json::json!({"commentId": comment_id, "sequence": sequence}),
    )
    .await?;
    Ok(())
}

pub async fn insert_activity(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    kind: &str,
    summary: &str,
    payload: &serde_json::Value,
) -> Result<Uuid, AppError> {
    let activity_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO work_item_activity (
            id, project_id, work_item_id, actor_id, kind, summary, payload
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(activity_id)
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    let sequence = allocate_timeline_sequence(transaction, project_id, work_item_id).await?;
    sqlx::query(
        "INSERT INTO work_item_timeline_entries (project_id, work_item_id, sequence, kind, activity_id) VALUES ($1, $2, $3, 'activity', $4)",
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(sequence)
    .bind(activity_id)
    .execute(&mut **transaction)
    .await?;
    Ok(activity_id)
}

pub async fn enqueue_update(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    version: i64,
) -> Result<(), AppError> {
    enqueue_work_item_updated(transaction, project_id, work_item_id, version).await?;
    Ok(())
}

async fn insert_comment_timeline(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<i64, AppError> {
    let sequence = allocate_timeline_sequence(transaction, project_id, work_item_id).await?;
    sqlx::query(
        "INSERT INTO work_item_timeline_entries (project_id, work_item_id, sequence, kind, comment_id) VALUES ($1, $2, $3, 'comment', $4)",
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(sequence)
    .bind(comment_id)
    .execute(&mut **transaction)
    .await?;
    Ok(sequence)
}

async fn allocate_timeline_sequence(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        "UPDATE work_items SET next_timeline_sequence = next_timeline_sequence + 1 WHERE project_id = $1 AND id = $2 RETURNING next_timeline_sequence - 1",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("work_item"))
}

pub async fn record_audit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    action: &str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<Uuid, AppError> {
    record_audit_for(transaction, actor, "work_item", action, target_id, metadata).await
}

/// One audit row for a change, attributed the way the actor came in: a
/// browser session, or an MCP credential or grant. Shared cores (runs,
/// approvals, GitHub jobs) write theirs through this for both surfaces, with
/// their own `target_kind`. Never put untrusted text (prompts, Codex output,
/// titles) in `metadata`.
pub async fn record_audit_for(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    target_kind: &str,
    action: &str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<Uuid, AppError> {
    let audit_id = Uuid::now_v7();
    match &actor.audit {
        AuditAttribution::Browser {
            session_id,
            request_id,
        } => {
            sqlx::query(
                r#"
                INSERT INTO audit_events (
                    id, actor_id, project_id, session_id, authentication_class,
                    action, target_kind, target_id, outcome, request_id, metadata
                ) VALUES ($1, $2, $3, $4, 'password_session', $5, $9, $6, 'succeeded', $7, $8)
                "#,
            )
            .bind(audit_id)
            .bind(actor.actor_id)
            .bind(actor.project_id)
            .bind(session_id)
            .bind(action)
            .bind(target_id)
            .bind(request_id)
            .bind(metadata)
            .bind(target_kind)
            .execute(&mut **transaction)
            .await?;
        }
        AuditAttribution::Mcp {
            authentication_class,
            request_id,
            client_name,
            mcp_credential_id,
            oauth_grant_id,
            external_agent_connection_id,
        } => {
            sqlx::query(
                r#"
                INSERT INTO audit_events (
                    id, actor_id, project_id, authentication_class, action,
                    target_kind, target_id, outcome, request_id, metadata,
                    channel, client_name, mcp_credential_id, mcp_oauth_grant_id,
                    external_agent_connection_id
                ) VALUES ($1, $2, $3, $4, $5, $13, $6, 'succeeded', $7, $8, 'mcp', $9, $10, $11, $12)
                "#,
            )
            .bind(audit_id)
            .bind(actor.actor_id)
            .bind(actor.project_id)
            .bind(authentication_class)
            .bind(action)
            .bind(target_id)
            .bind(request_id)
            .bind(metadata)
            .bind(client_name)
            .bind(mcp_credential_id)
            .bind(oauth_grant_id)
            .bind(external_agent_connection_id)
            .bind(target_kind)
            .execute(&mut **transaction)
            .await?;
        }
        AuditAttribution::Job { job_id, source } => {
            sqlx::query(
                r#"
                INSERT INTO audit_events (
                    id, actor_id, project_id, authentication_class, action,
                    target_kind, target_id, outcome, request_id, metadata,
                    channel, client_name
                ) VALUES ($1, $2, $3, 'background_job', $4, $5, $6, 'succeeded', $7, $8, 'system', $9)
                "#,
            )
            .bind(audit_id)
            .bind(actor.actor_id)
            .bind(actor.project_id)
            .bind(action)
            .bind(target_kind)
            .bind(target_id)
            .bind(job_id.to_string())
            .bind(metadata)
            .bind(*source)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(audit_id)
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("invalid membership role")),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        BOARD_RANK_STEP, BoardPlacement, ChangePayload, CriteriaOperation, CriteriaPlanError,
        CriterionChangeKind, CriterionState, CriterionTarget, LinkChange, PlacementProblem,
        PlannedCriterionChange, allocate_board_ranks, normalize_criterion_text,
        normalize_description, normalize_single_line, normalize_title, plan_criteria, target_index,
        validate_priority,
    };

    #[test]
    fn canonical_text_and_priority_validation_preserves_wire_contracts() {
        assert_eq!(
            normalize_title("  Build it  ").expect("normalize valid title"),
            "Build it"
        );
        assert_eq!(
            normalize_description("  line one\nline two  ")
                .expect("normalize valid multiline description"),
            "line one\nline two"
        );
        assert!(normalize_single_line("line one\nline two", 100, false).is_err());
        assert!(validate_priority("urgent").is_ok());
        assert!(validate_priority("critical").is_err());
    }

    fn column(ranks: &[i64]) -> Vec<(Uuid, i64)> {
        ranks.iter().map(|rank| (Uuid::now_v7(), *rank)).collect()
    }

    /// The board's single-task ranker before it was shared.
    fn previous_allocate(rows: &[(Uuid, i64)], index: usize) -> Option<i64> {
        let previous = index
            .checked_sub(1)
            .and_then(|previous| rows.get(previous))
            .map(|row| row.1);
        let next = rows.get(index).map(|row| row.1);
        match (previous, next) {
            (None, None) => Some(BOARD_RANK_STEP),
            (Some(previous), None) => previous.checked_add(BOARD_RANK_STEP),
            (None, Some(next)) if next > 1 => Some(next / 2),
            (Some(previous), Some(next)) if next - previous > 1 => {
                Some(previous + (next - previous) / 2)
            }
            _ => None,
        }
    }

    #[test]
    fn board_ranks_fit_between_neighbours_or_ask_for_a_rebalance() {
        assert_eq!(
            allocate_board_ranks(&[], 0, 2).expect("ranks"),
            Some(vec![1024, 2048])
        );
        let top = allocate_board_ranks(&column(&[10]), 0, 3)
            .expect("ranks")
            .expect("room at the top");
        assert!(top.windows(2).all(|pair| pair[0] < pair[1]), "{top:?}");
        assert!(top.iter().all(|rank| (1..10).contains(rank)), "{top:?}");
        assert_eq!(
            allocate_board_ranks(&column(&[1, 3]), 1, 2).expect("ranks"),
            None
        );
        assert_eq!(
            allocate_board_ranks(&column(&[1, 4]), 1, 2).expect("ranks"),
            Some(vec![2, 3])
        );
        assert_eq!(
            allocate_board_ranks(&column(&[i64::MAX - 5]), 1, 1).expect("ranks"),
            None
        );
        assert!(allocate_board_ranks(&column(&[1]), 2, 1).is_err());
        for ranks in [
            &[][..],
            &[1][..],
            &[2][..],
            &[1024, 2048][..],
            &[1024, 1025, 4096][..],
            &[5, 6, 7][..],
        ] {
            let rows = column(ranks);
            for index in 0..=rows.len() {
                assert_eq!(
                    allocate_board_ranks(&rows, index, 1)
                        .expect("ranks")
                        .map(|ranks| ranks[0]),
                    previous_allocate(&rows, index),
                    "{ranks:?} at {index}"
                );
            }
        }
    }

    #[test]
    fn placements_name_an_index_or_the_problem() {
        let rows = column(&[1024, 2048, 3072]);
        let task = Uuid::now_v7();
        assert_eq!(target_index(&rows, task, BoardPlacement::Top), Ok(0));
        assert_eq!(target_index(&rows, task, BoardPlacement::Bottom), Ok(3));
        assert_eq!(target_index(&rows, task, BoardPlacement::Index(3)), Ok(3));
        assert_eq!(
            target_index(&rows, task, BoardPlacement::Index(4)),
            Err(PlacementProblem::OutOfRange { open_tasks: 3 })
        );
        assert_eq!(
            target_index(&rows, task, BoardPlacement::Before(rows[0].0)),
            Ok(0)
        );
        assert_eq!(
            target_index(&rows, task, BoardPlacement::After(rows[0].0)),
            Ok(1)
        );
        assert_eq!(
            target_index(&rows, task, BoardPlacement::Before(rows[2].0)),
            Ok(2)
        );
        assert_eq!(
            target_index(&rows, task, BoardPlacement::After(rows[2].0)),
            Ok(3)
        );
        let missing = Uuid::now_v7();
        assert_eq!(
            target_index(&rows, task, BoardPlacement::After(missing)),
            Err(PlacementProblem::AnchorNotInColumn { anchor: missing })
        );
        assert_eq!(
            target_index(&rows, task, BoardPlacement::Before(task)),
            Err(PlacementProblem::AnchorIsSelf)
        );
    }

    #[test]
    fn change_payloads_carry_only_what_changed() {
        let status = (Uuid::now_v7(), Uuid::now_v7());
        let fields = ChangePayload {
            changed_fields: vec!["title".to_owned()],
            ..ChangePayload::default()
        };
        assert_eq!(
            fields.activity(),
            serde_json::json!({"changedFields": ["title"]})
        );
        assert_eq!(
            fields.audit(3),
            serde_json::json!({"changedFields": ["title"], "version": 3})
        );
        let parent = Uuid::now_v7();
        let label = Uuid::now_v7();
        let moved = ChangePayload {
            changed_fields: vec!["status".to_owned(), "parent".to_owned()],
            status: Some(status),
            position: Some(2),
            parent: Some((None, Some(parent))),
            labels_added: vec![label],
            ..ChangePayload::default()
        };
        assert_eq!(
            moved.activity(),
            serde_json::json!({
                "changedFields": ["status", "parent"],
                "previousStatusId": status.0,
                "statusId": status.1,
                "position": 2,
                "parent": {"from": null, "to": parent},
                "labelsAdded": [label],
            })
        );
        let link = ChangePayload {
            link: Some(LinkChange {
                kind: "page".to_owned(),
                id: parent,
                linked: false,
            }),
            ..ChangePayload::default()
        };
        assert_eq!(
            link.activity(),
            serde_json::json!({"link": {"kind": "page", "id": parent}, "linked": false})
        );
    }

    #[test]
    fn criteria_keep_newlines_but_not_other_control_characters() {
        assert_eq!(
            normalize_criterion_text("  Works offline\nand syncs  ").expect("criterion"),
            "Works offline\nand syncs"
        );
        assert!(normalize_criterion_text("   ").is_err());
        assert!(normalize_criterion_text("bell\u{7}").is_err());
        assert!(normalize_criterion_text(&"x".repeat(1001)).is_err());
    }

    fn criteria(texts: &[&str]) -> Vec<CriterionState> {
        texts
            .iter()
            .map(|text| CriterionState {
                id: Uuid::now_v7(),
                text: (*text).to_owned(),
                completed: false,
            })
            .collect()
    }

    fn texts(list: &[CriterionState]) -> Vec<&str> {
        list.iter()
            .map(|criterion| criterion.text.as_str())
            .collect()
    }

    fn add(text: &str, position: Option<i32>) -> CriteriaOperation {
        CriteriaOperation::Add {
            text: text.to_owned(),
            position,
        }
    }

    fn update(
        target: CriterionTarget,
        position: Option<i32>,
        completed: Option<bool>,
    ) -> CriteriaOperation {
        CriteriaOperation::Update {
            target,
            text: None,
            position,
            completed,
        }
    }

    #[test]
    fn criteria_added_anywhere_keep_positions_without_gaps() {
        let snapshot = criteria(&["a", "b"]);
        let (list, changes) = plan_criteria(
            &snapshot,
            &[
                add("end", None),
                add("first", Some(1)),
                add("middle", Some(3)),
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert_eq!(texts(&list), ["first", "a", "middle", "b", "end"]);
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.kind, change.position, change.changed))
                .collect::<Vec<_>>(),
            [
                (CriterionChangeKind::Created, 3, true),
                (CriterionChangeKind::Created, 1, true),
                (CriterionChangeKind::Created, 3, true),
            ]
        );
        assert_eq!(changes[1].criterion_id, list[0].id);
        assert_eq!(
            plan_criteria(&snapshot, &[add("far", Some(4))], Uuid::now_v7),
            Err(CriteriaPlanError::Position {
                operation_index: 0,
                max: 3
            })
        );
        assert_eq!(
            plan_criteria(&snapshot, &[add("zero", Some(0))], Uuid::now_v7),
            Err(CriteriaPlanError::Position {
                operation_index: 0,
                max: 3
            })
        );
    }

    #[test]
    fn criteria_move_down_and_up() {
        let snapshot = criteria(&["a", "b", "c", "d"]);
        let (list, changes) = plan_criteria(
            &snapshot,
            &[update(CriterionTarget::Position(1), Some(3), None)],
            Uuid::now_v7,
        )
        .expect("down");
        assert_eq!(texts(&list), ["b", "c", "a", "d"]);
        assert_eq!(changes[0].kind, CriterionChangeKind::Updated);
        assert_eq!(changes[0].fields, ["position"]);
        assert_eq!(changes[0].position, 3);
        let (list, _) = plan_criteria(
            &snapshot,
            &[update(CriterionTarget::Id(snapshot[3].id), Some(1), None)],
            Uuid::now_v7,
        )
        .expect("up");
        assert_eq!(texts(&list), ["d", "a", "b", "c"]);
        let (_, changes) = plan_criteria(
            &snapshot,
            &[update(CriterionTarget::Position(2), Some(2), None)],
            Uuid::now_v7,
        )
        .expect("same place");
        assert!(!changes[0].changed);
        assert_eq!(
            plan_criteria(
                &snapshot,
                &[update(CriterionTarget::Position(1), Some(5), None)],
                Uuid::now_v7,
            ),
            Err(CriteriaPlanError::Position {
                operation_index: 0,
                max: 4
            })
        );
    }

    #[test]
    fn criteria_targets_name_the_list_as_it_was_read() {
        let snapshot = criteria(&["a", "b", "c"]);
        let (list, changes) = plan_criteria(
            &snapshot,
            &[
                CriteriaOperation::Remove {
                    target: CriterionTarget::Position(1),
                },
                update(CriterionTarget::Position(2), None, Some(true)),
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        // Position 2 is the original #2 ("b"), not what's second after the
        // removal.
        assert_eq!(texts(&list), ["b", "c"]);
        assert!(list[0].completed && !list[1].completed);
        assert_eq!(changes[0].position, 1);
        assert_eq!(changes[1].kind, CriterionChangeKind::Completed);
        assert_eq!(changes[1].criterion_id, snapshot[1].id);

        let a = CriterionTarget::Id(snapshot[0].id);
        assert_eq!(
            plan_criteria(
                &snapshot,
                &[
                    CriteriaOperation::Remove { target: a },
                    update(a, None, Some(true)),
                ],
                Uuid::now_v7,
            ),
            Err(CriteriaPlanError::RemovedEarlier {
                operation_index: 1,
                removed_by: 0
            })
        );
        for target in [
            CriterionTarget::Id(Uuid::now_v7()),
            CriterionTarget::Position(4),
            CriterionTarget::Position(0),
        ] {
            assert_eq!(
                plan_criteria(
                    &snapshot,
                    &[CriteriaOperation::Remove { target }],
                    Uuid::now_v7
                ),
                Err(CriteriaPlanError::UnknownCriterion { operation_index: 0 })
            );
        }
    }

    #[test]
    fn criteria_already_that_way_change_nothing() {
        let mut snapshot = criteria(&["a", "b"]);
        snapshot[0].completed = true;
        let (_, changes) = plan_criteria(
            &snapshot,
            &[
                update(CriterionTarget::Position(1), None, Some(true)),
                CriteriaOperation::Update {
                    target: CriterionTarget::Position(2),
                    text: Some("b".to_owned()),
                    position: None,
                    completed: Some(false),
                },
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert!(changes.iter().all(|change| !change.changed), "{changes:?}");

        // Text and completion together are one change, recorded as the
        // completion, as the web app records it.
        let (list, changes) = plan_criteria(
            &snapshot,
            &[CriteriaOperation::Update {
                target: CriterionTarget::Position(2),
                text: Some("b, reworded".to_owned()),
                position: Some(1),
                completed: Some(true),
            }],
            Uuid::now_v7,
        )
        .expect("plan");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, CriterionChangeKind::Completed);
        assert_eq!(changes[0].fields, ["text", "position"]);
        assert_eq!(texts(&list), ["b, reworded", "a"]);
        assert_eq!(
            changes[0].activity_payload(),
            serde_json::json!({"criterionId": snapshot[1].id})
        );
    }

    #[test]
    fn criteria_completion_records_the_net_effect() {
        let mut snapshot = criteria(&["a", "b"]);
        snapshot[0].completed = true;
        let kinds = |changes: &[PlannedCriterionChange]| {
            changes
                .iter()
                .map(|change| (change.kind, change.changed))
                .collect::<Vec<_>>()
        };

        // Unchecking a criterion someone else checked and checking it again
        // leaves it (and its completer) as it was: nothing to record.
        let (list, changes) = plan_criteria(
            &snapshot,
            &[
                update(CriterionTarget::Position(1), None, Some(false)),
                update(CriterionTarget::Position(1), None, Some(true)),
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert!(list[0].completed);
        assert!(changes.iter().all(|change| !change.changed), "{changes:?}");

        // Checked, unchecked and checked again: one completion, the last.
        let (_, changes) = plan_criteria(
            &snapshot,
            &[
                update(CriterionTarget::Position(2), None, Some(true)),
                update(CriterionTarget::Position(2), None, Some(false)),
                update(CriterionTarget::Position(2), None, Some(true)),
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert_eq!(
            kinds(&changes),
            [
                (CriterionChangeKind::Updated, false),
                (CriterionChangeKind::Updated, false),
                (CriterionChangeKind::Completed, true),
            ]
        );

        // A reworded criterion whose uncheck was undone records the rewording.
        let (_, changes) = plan_criteria(
            &snapshot,
            &[
                CriteriaOperation::Update {
                    target: CriterionTarget::Position(1),
                    text: Some("a, reworded".to_owned()),
                    position: None,
                    completed: Some(false),
                },
                update(CriterionTarget::Position(1), None, Some(true)),
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert_eq!(
            kinds(&changes),
            [
                (CriterionChangeKind::Updated, true),
                (CriterionChangeKind::Updated, false),
            ]
        );
        assert_eq!(changes[0].fields, ["text"]);

        // A criterion the call removes keeps what each operation did.
        let (_, changes) = plan_criteria(
            &snapshot,
            &[
                update(CriterionTarget::Position(2), None, Some(true)),
                CriteriaOperation::Remove {
                    target: CriterionTarget::Position(2),
                },
            ],
            Uuid::now_v7,
        )
        .expect("plan");
        assert_eq!(
            kinds(&changes),
            [
                (CriterionChangeKind::Completed, true),
                (CriterionChangeKind::Removed, true),
            ]
        );
    }

    #[test]
    fn criteria_stop_at_one_thousand() {
        let snapshot = criteria(&["x"; 1000]);
        assert_eq!(
            plan_criteria(&snapshot, &[add("one more", None)], Uuid::now_v7),
            Err(CriteriaPlanError::TooMany { count: 1001 })
        );
        let (list, _) = plan_criteria(
            &snapshot,
            &[
                CriteriaOperation::Remove {
                    target: CriterionTarget::Position(1),
                },
                add("one more", None),
            ],
            Uuid::now_v7,
        )
        .expect("room after a removal");
        assert_eq!(list.len(), 1000);
    }
}
