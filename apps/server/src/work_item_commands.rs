//! Canonical application primitives for Work Item mutations.
//!
//! HTTP and MCP keep their wire validation, idempotency replay encoding, and
//! response mapping. This module owns the domain operations that must not
//! diverge: normalized values, Work Item locking and conflicts, assignee and
//! workflow checks, version advancement, and exactly-once mutation side
//! effects inside the caller's transaction.

use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    external_events::enqueue_external_work_item_event,
    outbox::{OutboxEventPayload, WorkItemUpdatedPayload, enqueue_typed},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
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
}

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
    ParentArchived(LockedWorkItem),
    /// The subtask would sit deeper than MAX_SUBTASK_LEVELS.
    ParentTooDeep,
    InvalidPlacement(PlacementProblem),
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
    Ok(sqlx::query_as::<_, LockedWorkItem>(
        r#"
        SELECT id, number, version, title, description, type_id, status_id, priority,
               assignee_actor_id, parent_work_item_id, board_rank,
               archived_at IS NOT NULL AS archived, blocked_reason
        FROM work_items
        WHERE project_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
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
    Ok(sqlx::query_as::<_, LockedWorkItem>(
        r#"
        SELECT id, number, version, title, description, type_id, status_id, priority,
               assignee_actor_id, parent_work_item_id, board_rank,
               archived_at IS NOT NULL AS archived, blocked_reason
        FROM work_items
        WHERE project_id = $1 AND id = ANY($2)
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(work_item_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

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
    if let Some(parent_id) = command.parent_work_item_id {
        let Some(parent) = lock_work_item(transaction, actor.project_id, parent_id).await? else {
            return Ok(CreateWorkItemOutcome::ParentNotFound);
        };
        if parent.archived {
            return Ok(CreateWorkItemOutcome::ParentArchived(parent));
        }
        let parent_levels = ancestor_chain(transaction, actor.project_id, parent_id)
            .await?
            .len();
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
            parent_work_item_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
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
    .execute(&mut **transaction)
    .await?;
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
    if changed_fields.is_empty() {
        return Ok(UpdateWorkItemOutcome::Noop(current));
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
            board_rank = $11, version = version + 1, updated_at = now()
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
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal(
            "locked Work Item update lost its version",
        ));
    }
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
/// locked the project (`lock_membership`) and the task, and refused an
/// archived task. A call that changes nothing passes whatever version it
/// names, so a retry answers `Unchanged`; `expected_version` is checked only
/// when something would change.
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
    let ids = criteria.iter().map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let texts = criteria
        .iter()
        .map(|(text, _)| text.clone())
        .collect::<Vec<_>>();
    let completed = criteria
        .iter()
        .map(|(_, completed)| *completed)
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO work_item_acceptance_criteria (
            id, project_id, work_item_id, text, position, completed,
            completed_by_actor_id, completed_at, created_by_actor_id
        )
        SELECT item.id, $1, $2, item.text, (item.ord - 1 + $4)::int, item.completed,
               CASE WHEN item.completed THEN $3::uuid END,
               CASE WHEN item.completed THEN now() END, $3
        FROM unnest($5::uuid[], $6::text[], $7::bool[])
             WITH ORDINALITY AS item(id, text, completed, ord)
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(actor_id)
    .bind(start_position)
    .bind(&ids)
    .bind(&texts)
    .bind(&completed)
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
    enqueue_typed(
        transaction,
        project_id,
        OutboxEventPayload::WorkItemUpdated(WorkItemUpdatedPayload {
            work_item_id,
            version,
        }),
    )
    .await?;
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
                ) VALUES ($1, $2, $3, $4, 'password_session', $5, 'work_item', $6, 'succeeded', $7, $8)
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
                ) VALUES ($1, $2, $3, $4, $5, 'work_item', $6, 'succeeded', $7, $8, 'mcp', $9, $10, $11, $12)
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
        BOARD_RANK_STEP, BoardPlacement, ChangePayload, LinkChange, PlacementProblem,
        allocate_board_ranks, normalize_criterion_text, normalize_description,
        normalize_single_line, normalize_title, target_index, validate_priority,
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
}
