use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

pub const EVENT_RETENTION_DAYS: i32 = 30;
const MAX_CURSOR_BYTES: usize = 512;

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEvent {
    pub id: Uuid,
    pub sequence: i64,
    pub kind: String,
    pub summary: String,
    pub resource_kind: String,
    pub resource_id: Uuid,
    pub canonical_path: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CursorPayload {
    version: u8,
    connection_id: Uuid,
    project_id: Uuid,
    credential_id: Uuid,
    after_sequence: i64,
}

#[allow(clippy::too_many_arguments)]
pub async fn enqueue_external_event(
    transaction: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    project_id: Uuid,
    kind: &str,
    summary: &str,
    resource_kind: &str,
    resource_id: Uuid,
    canonical_path: &str,
) -> Result<i64, AppError> {
    let sequence = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO external_agent_event_counters (connection_id, next_sequence)
        VALUES ($1, 2)
        ON CONFLICT (connection_id) DO UPDATE
        SET next_sequence = external_agent_event_counters.next_sequence + 1,
            updated_at = now()
        RETURNING next_sequence - 1
        "#,
    )
    .bind(connection_id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO external_agent_events (
            id, project_id, connection_id, sequence, kind, summary,
            resource_kind, resource_id, canonical_path,
            expires_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9,
            now() + make_interval(days => $10)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(connection_id)
    .bind(sequence)
    .bind(kind)
    .bind(summary)
    .bind(resource_kind)
    .bind(resource_id)
    .bind(canonical_path)
    .bind(EVENT_RETENTION_DAYS)
    .execute(&mut **transaction)
    .await?;
    Ok(sequence)
}

pub async fn enqueue_external_work_item_event(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    explicit_actor_id: Option<Uuid>,
    kind: &str,
    summary: &str,
) -> Result<(), AppError> {
    let available =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('external_agent_events') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?;
    if !available {
        return Ok(());
    }
    let connection_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT connection.id
        FROM work_items item
        JOIN external_agent_connections connection
          ON agent_connection_can_access(connection.id, item.project_id)
         AND connection.actor_id = COALESCE($3, item.assignee_actor_id)
         AND connection.status = 'active'
        JOIN actors actor ON actor.id = connection.actor_id
         AND actor.kind = 'external_agent' AND actor.status = 'active'
        JOIN project_memberships membership
          ON membership.project_id = $1
         AND membership.actor_id = connection.actor_id
         AND membership.enabled
        WHERE item.project_id = $1 AND item.id = $2
        ORDER BY connection.id
        FOR SHARE OF connection, actor, membership
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(explicit_actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    for connection_id in connection_ids {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            kind,
            summary,
            "work_item",
            work_item_id,
            &format!("/projects/{project_id}/work-items/{work_item_id}"),
        )
        .await?;
    }
    Ok(())
}

pub async fn enqueue_external_approval_requested(
    transaction: &mut Transaction<'_, Postgres>,
    approval_id: Uuid,
) -> Result<(), AppError> {
    let available =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('external_agent_events') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?;
    if !available {
        return Ok(());
    }
    let recipients = external_approval_recipients(transaction, approval_id, "pending").await?;
    for (connection_id, project_id, conversation_id) in recipients {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            "approval.requested",
            "An approval requires your decision",
            "agent_approval",
            approval_id,
            &format!("/projects/{project_id}/threads/{conversation_id}"),
        )
        .await?;
    }
    Ok(())
}

pub async fn enqueue_external_approval_resolved(
    transaction: &mut Transaction<'_, Postgres>,
    approval_id: Uuid,
) -> Result<(), AppError> {
    let available =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('external_agent_events') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?;
    if !available {
        return Ok(());
    }
    let recipients = external_approval_recipients(transaction, approval_id, "resolved").await?;
    for (connection_id, project_id, conversation_id) in recipients {
        enqueue_external_event(
            transaction,
            connection_id,
            project_id,
            "approval.resolved",
            "An addressed approval was resolved",
            "agent_approval",
            approval_id,
            &format!("/projects/{project_id}/threads/{conversation_id}"),
        )
        .await?;
    }
    Ok(())
}

async fn external_approval_recipients(
    transaction: &mut Transaction<'_, Postgres>,
    approval_id: Uuid,
    required_status: &str,
) -> Result<Vec<(Uuid, Uuid, Uuid)>, AppError> {
    let recipients = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"
        SELECT connection.id, conversation.project_id, run.conversation_id
        FROM agent_approvals approval
        JOIN agent_runs run ON run.id = approval.agent_run_id
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN projects project ON project.id = conversation.project_id
         AND project.archived_at IS NULL
        JOIN external_agent_connections connection
          ON agent_connection_can_access(connection.id, conversation.project_id)
          AND connection.status = 'active'
         AND connection.actor_id <> approval.requesting_actor_id
        JOIN actors actor ON actor.id = connection.actor_id
         AND actor.kind = 'external_agent' AND actor.status = 'active'
        JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = connection.actor_id
         AND membership.enabled
         AND membership.role IN ('owner', 'admin', 'member')
         AND custom_role_allows(membership.role_id, 'runs.approve')
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = connection.actor_id
         AND organization_membership.enabled
        WHERE approval.id = $1
          AND approval.status = $2
          AND NOT approval.human_required
          AND (
              approval.eligible_actor_id = connection.actor_id
              OR (
                  approval.eligible_actor_id IS NULL
                  AND approval.eligible_role IN (membership.role, 'any_authorized')
              )
          )
        ORDER BY connection.id
        FOR SHARE OF approval, run, connection, actor, membership, organization_membership
        "#,
    )
    .bind(approval_id)
    .bind(required_status)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(recipients)
}

pub fn encode_cursor(
    signing_key: &[u8],
    connection_id: Uuid,
    project_id: Uuid,
    credential_id: Uuid,
    after_sequence: i64,
) -> Result<String, AppError> {
    let payload = serde_json::to_vec(&CursorPayload {
        version: 1,
        connection_id,
        project_id,
        credential_id,
        after_sequence,
    })
    .map_err(|_| AppError::Internal("serialize external event cursor"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key)
        .map_err(|_| AppError::Internal("external event cursor signing key"))?;
    mac.update(&payload);
    let signature = mac.finalize().into_bytes();
    let mut envelope = Vec::with_capacity(payload.len() + signature.len());
    envelope.extend_from_slice(&payload);
    envelope.extend_from_slice(&signature);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

pub fn decode_cursor(
    signing_key: &[u8],
    cursor: &str,
    connection_id: Uuid,
    project_id: Uuid,
    credential_id: Uuid,
) -> Result<i64, AppError> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES {
        return Err(AppError::Invalid("external event cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| AppError::Invalid("external event cursor"))?;
    let payload_length = envelope
        .len()
        .checked_sub(32)
        .ok_or(AppError::Invalid("external event cursor"))?;
    let (payload, signature) = envelope.split_at(payload_length);
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key)
        .map_err(|_| AppError::Internal("external event cursor signing key"))?;
    mac.update(payload);
    mac.verify_slice(signature)
        .map_err(|_| AppError::Invalid("external event cursor"))?;
    let payload: CursorPayload =
        serde_json::from_slice(payload).map_err(|_| AppError::Invalid("external event cursor"))?;
    if payload.version != 1
        || payload.connection_id != connection_id
        || payload.project_id != project_id
        || payload.credential_id != credential_id
        || payload.after_sequence < 0
    {
        return Err(AppError::Invalid("external event cursor"));
    }
    Ok(payload.after_sequence)
}

#[cfg(test)]
mod tests {
    use super::{decode_cursor, encode_cursor};
    use uuid::Uuid;

    #[test]
    fn cursor_is_signed_and_bound_to_connection_project_and_credential() {
        let key = [7_u8; 32];
        let connection = Uuid::now_v7();
        let project = Uuid::now_v7();
        let credential = Uuid::now_v7();
        let cursor = encode_cursor(&key, connection, project, credential, 42).expect("cursor");
        assert_eq!(
            decode_cursor(&key, &cursor, connection, project, credential).expect("valid cursor"),
            42
        );
        assert!(decode_cursor(&key, &cursor, Uuid::now_v7(), project, credential).is_err());
        let mut tampered = cursor.into_bytes();
        let index = tampered.len() / 2;
        tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
        assert!(
            decode_cursor(
                &key,
                std::str::from_utf8(&tampered).expect("ASCII cursor"),
                connection,
                project,
                credential
            )
            .is_err()
        );
    }
}
