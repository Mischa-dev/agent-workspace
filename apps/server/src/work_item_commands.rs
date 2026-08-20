//! Canonical application primitives for Work Item mutations.
//!
//! HTTP and MCP keep their wire validation, idempotency replay encoding, and
//! response mapping. This module owns the domain operations that must not
//! diverge: normalized values, Work Item locking and conflicts, assignee and
//! workflow checks, version advancement, and exactly-once mutation side
//! effects inside the caller's transaction.

use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    external_events::enqueue_external_work_item_event,
    outbox::{OutboxEventPayload, WorkItemUpdatedPayload, enqueue_typed},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

pub const MAX_TITLE_CHARACTERS: usize = 240;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 100_000;
pub const BOARD_RANK_STEP: i64 = 1024;

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
    pub version: i64,
    pub title: String,
    pub description: String,
    pub type_id: Uuid,
    pub status_id: Uuid,
    pub priority: String,
    pub assignee_actor_id: Option<Uuid>,
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
}

pub struct UpdateWorkItemCommand<'a> {
    pub expected_version: i64,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub type_id: Option<Uuid>,
    pub status_id: Option<Uuid>,
    pub priority: Option<&'a str>,
    pub assignee_actor_id: Option<Option<Uuid>>,
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
        SELECT id, version, title, description, type_id, status_id, priority,
               assignee_actor_id, archived_at IS NOT NULL AS archived,
               blocked_reason
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

pub async fn create_work_item(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    command: CreateWorkItemCommand<'_>,
) -> Result<CreateWorkItemOutcome, AppError> {
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
    let number = sqlx::query_scalar::<_, i64>(
        "UPDATE project_counters SET next_work_item_number = next_work_item_number + 1, updated_at = now() WHERE project_id = $1 RETURNING next_work_item_number - 1",
    )
    .bind(actor.project_id)
    .fetch_one(&mut **transaction)
    .await?;
    let board_rank = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(max(board_rank), 0) + $3 FROM work_items WHERE project_id = $1 AND status_id = $2",
    )
    .bind(actor.project_id)
    .bind(status_id)
    .bind(BOARD_RANK_STEP)
    .fetch_one(&mut **transaction)
    .await?;
    let work_item_id = Uuid::now_v7();
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
    let mut activity_metadata = command.activity_metadata;
    if matches!(
        command.activity_number_metadata,
        AllocatedNumberMetadata::Include
    ) && let Some(metadata) = activity_metadata.as_object_mut()
    {
        metadata.insert("number".to_owned(), serde_json::json!(number));
    }
    let mut audit_metadata = command.audit_metadata;
    if let Some(metadata) = audit_metadata.as_object_mut() {
        metadata.insert("number".to_owned(), serde_json::json!(number));
    }
    let finalized = finalize_created(
        transaction,
        actor,
        work_item_id,
        command.assignee_actor_id,
        command.activity_summary,
        activity_metadata,
        command.audit_action,
        audit_metadata,
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

async fn update_locked_work_item(
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
    if changed_fields.is_empty() {
        return Ok(UpdateWorkItemOutcome::Noop(current));
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
            priority = $7, assignee_actor_id = $8,
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
    let activity_payload = serde_json::json!({"changedFields": changed_fields});
    let audit_metadata = match command.audit_version_metadata {
        AuditVersionMetadata::Omit => activity_payload.clone(),
        AuditVersionMetadata::Include => {
            serde_json::json!({"changedFields": changed_fields, "version": version})
        }
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
    transaction: &mut Transaction<'_, Postgres>,
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
    .fetch_one(&mut **transaction)
    .await?)
}

pub async fn resolve_status(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    value: &str,
) -> Result<Option<Uuid>, AppError> {
    let id = value.parse::<Uuid>().ok();
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM work_item_statuses
        WHERE project_id = $1 AND enabled
          AND (($2::uuid IS NOT NULL AND id = $2)
               OR lower(name) = lower($3)
               OR semantic_category = lower($3))
        ORDER BY position, id LIMIT 2
        "#,
    )
    .bind(project_id)
    .bind(id)
    .bind(value)
    .fetch_all(&mut **transaction)
    .await?;
    Ok((rows.len() == 1).then(|| rows[0]))
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
    let activity_id = insert_activity(
        transaction,
        actor.project_id,
        work_item_id,
        actor.actor_id,
        finalization.activity_kind,
        finalization.summary,
        &finalization.activity_payload,
    )
    .await?;
    enqueue_update(
        transaction,
        actor.project_id,
        work_item_id,
        finalization.version,
    )
    .await?;
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
        finalization.audit_action,
        work_item_id,
        finalization.audit_metadata,
    )
    .await?;
    Ok(FinalizedChange {
        activity_id,
        audit_id,
    })
}

pub async fn finalize_created(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    work_item_id: Uuid,
    assignee_actor_id: Option<Uuid>,
    activity_summary: &str,
    activity_metadata: serde_json::Value,
    audit_action: &str,
    audit_metadata: serde_json::Value,
) -> Result<FinalizedChange, AppError> {
    let activity_id = insert_activity(
        transaction,
        actor.project_id,
        work_item_id,
        actor.actor_id,
        "work_item_created",
        activity_summary,
        &activity_metadata,
    )
    .await?;
    enqueue_update(transaction, actor.project_id, work_item_id, 1).await?;
    enqueue_external_work_item_event(
        transaction,
        actor.project_id,
        work_item_id,
        assignee_actor_id,
        "work_item.assigned",
        "A Work Item was assigned to you",
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
    use super::{normalize_description, normalize_single_line, normalize_title, validate_priority};

    #[test]
    fn canonical_text_and_priority_validation_preserves_wire_contracts() {
        assert_eq!(normalize_title("  Build it  ").unwrap(), "Build it");
        assert_eq!(
            normalize_description("  line one\nline two  ").unwrap(),
            "line one\nline two"
        );
        assert!(normalize_single_line("line one\nline two", 100, false).is_err());
        assert!(validate_priority("urgent").is_ok());
        assert!(validate_priority("critical").is_err());
    }
}
