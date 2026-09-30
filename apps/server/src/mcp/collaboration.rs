//! Agent-only reads: the check that an agent's credential and connection are
//! still active, and the older per-connection event inbox.

use std::time::{Duration, Instant};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    external_events::{ExternalEvent, decode_cursor, encode_cursor},
    policy::{Action, ActorKind},
};

#[derive(Debug, FromRow)]
pub(super) struct ConnectionAuthorization {
    membership_role: String,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ReadFailure {
    Authorization,
    NotFound,
    Database(&'static str),
}

pub(super) async fn begin_authorized_read<'a>(
    state: &'a AppState,
    principal: &McpPrincipal,
    required_scope: &str,
) -> Result<(Transaction<'a, Postgres>, ConnectionAuthorization), ReadFailure> {
    let connection_id = principal
        .external_agent_connection_id
        .ok_or(ReadFailure::Authorization)?;
    if principal.actor_kind != ActorKind::ExternalAgent || !principal.has_scope(required_scope) {
        return Err(ReadFailure::Authorization);
    }
    let mut transaction = state
        .pool
        .begin()
        .await
        .map_err(|_| ReadFailure::Database("begin"))?;
    let authorization = sqlx::query_as::<_, ConnectionAuthorization>(
        r#"
        SELECT membership.role AS membership_role
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = credential.actor_id
        JOIN projects project ON project.id = $2
        JOIN project_memberships membership
          ON membership.project_id = $2
         AND membership.actor_id = credential.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = credential.actor_id
        WHERE credential.id = $1
          AND agent_connection_can_access(connection.id, $2)
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active' AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY[$5]::text[]
          AND connection.status = 'active'
          AND connection.project_id = credential.project_id
          AND connection.actor_id = credential.actor_id
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent' AND actor.status = 'active'
          AND project.archived_at IS NULL
          AND membership.enabled AND organization_membership.enabled
        FOR SHARE OF credential, connection, actor, project, membership,
                     organization_membership
        "#,
    )
    .bind(principal.credential_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .bind(connection_id)
    .bind(required_scope)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| ReadFailure::Database("authorize"))?
    .ok_or(ReadFailure::Authorization)?;
    if !super::mcp_policy_allows(
        principal.actor_id,
        principal.actor_kind,
        principal.project_id,
        &authorization.membership_role,
        Action::McpRead,
    ) {
        return Err(ReadFailure::Authorization);
    }
    Ok((transaction, authorization))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExternalEventPollArguments {
    pub after_cursor: Option<String>,
    pub limit: Option<u16>,
    pub wait_seconds: Option<u8>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum ExternalEventPollResult {
    Events {
        events: Vec<ExternalEventProjection>,
        next_cursor: String,
        has_more: bool,
    },
    CursorExpired {
        retry_category: String,
        resync_path: String,
    },
    Busy {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    InvalidRequest {
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExternalEventProjection {
    id: String,
    sequence: i64,
    kind: String,
    summary: String,
    resource_kind: String,
    resource_id: String,
    canonical_path: String,
    created_at: String,
    expires_at: String,
}

pub(super) async fn poll_external_events(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ExternalEventPollArguments,
) -> ExternalEventPollResult {
    let Some(connection_id) = principal.external_agent_connection_id else {
        return denied();
    };
    if !principal.has_scope("events") {
        return denied();
    }
    let limit = arguments.limit.unwrap_or(50);
    let wait_seconds = arguments.wait_seconds.unwrap_or(0);
    if !(1..=100).contains(&limit) || wait_seconds > 30 {
        return ExternalEventPollResult::InvalidRequest {
            retry_category: "do_not_retry_without_change".to_owned(),
        };
    }

    match poll(
        state,
        principal,
        connection_id,
        arguments.after_cursor.as_deref(),
        i64::from(limit),
        wait_seconds,
    )
    .await
    {
        Ok(result) => result,
        Err(PollFailure::InvalidCursor) => ExternalEventPollResult::InvalidRequest {
            retry_category: "do_not_retry_without_change".to_owned(),
        },
        Err(PollFailure::Authorization) => denied(),
        Err(PollFailure::Database(category)) => {
            tracing::error!(category, "external event poll failed");
            ExternalEventPollResult::TemporarilyUnavailable {
                retry_category: "retry_with_backoff".to_owned(),
            }
        }
    }
}

async fn poll(
    state: &AppState,
    principal: &McpPrincipal,
    connection_id: Uuid,
    cursor: Option<&str>,
    limit: i64,
    wait_seconds: u8,
) -> Result<ExternalEventPollResult, PollFailure> {
    let mut transaction = state
        .pool
        .begin()
        .await
        .map_err(|_| PollFailure::Database("begin"))?;
    let signing_key = revalidate_connection(&mut transaction, principal, connection_id)
        .await?
        .ok_or(PollFailure::Authorization)?;
    let acquired =
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("m9-external-poll:{connection_id}"))
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| PollFailure::Database("poll_lock"))?;
    if !acquired {
        return Ok(ExternalEventPollResult::Busy {
            retry_category: "retry_with_backoff".to_owned(),
        });
    }
    let after_sequence = match cursor {
        Some(cursor) => decode_cursor(
            &signing_key,
            cursor,
            connection_id,
            principal.project_id,
            principal.credential_id,
        )
        .map_err(|_| PollFailure::InvalidCursor)?,
        None => 0,
    };
    if cursor_is_expired(&mut transaction, connection_id, after_sequence).await? {
        return Ok(ExternalEventPollResult::CursorExpired {
            retry_category: "resynchronize".to_owned(),
            resync_path: format!("/projects/{}", principal.project_id),
        });
    }

    let deadline = Instant::now() + Duration::from_secs(u64::from(wait_seconds));
    loop {
        let mut rows = sqlx::query_as::<_, ExternalEvent>(
            r#"
            SELECT id, sequence, kind, summary, resource_kind, resource_id,
                   canonical_path, created_at::text AS created_at,
                   expires_at::text AS expires_at
            FROM external_agent_events
            WHERE connection_id = $1 AND sequence > $2 AND expires_at > now()
              AND agent_connection_can_access(connection_id, project_id)
              AND external_event_visible_to(connection_id, resource_kind, resource_id)
            ORDER BY sequence
            LIMIT $3
            "#,
        )
        .bind(connection_id)
        .bind(after_sequence)
        .bind(limit + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| PollFailure::Database("fetch"))?;
        if !rows.is_empty() || Instant::now() >= deadline {
            if revalidate_connection(&mut transaction, principal, connection_id)
                .await?
                .is_none()
            {
                return Err(PollFailure::Authorization);
            }
            let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
            if has_more {
                rows.pop();
            }
            let next_sequence = rows.last().map_or(after_sequence, |event| event.sequence);
            let next_cursor = encode_cursor(
                &signing_key,
                connection_id,
                principal.project_id,
                principal.credential_id,
                next_sequence,
            )
            .map_err(|_| PollFailure::Database("encode_cursor"))?;
            transaction
                .commit()
                .await
                .map_err(|_| PollFailure::Database("commit"))?;
            return Ok(ExternalEventPollResult::Events {
                events: rows
                    .into_iter()
                    .map(ExternalEventProjection::from)
                    .collect(),
                next_cursor,
                has_more,
            });
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn revalidate_connection(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    connection_id: Uuid,
) -> Result<Option<Vec<u8>>, PollFailure> {
    sqlx::query_scalar::<_, Vec<u8>>(
        r#"
        SELECT credential.token_digest
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = connection.actor_id
        JOIN projects project ON project.id = connection.project_id
        JOIN project_memberships membership
          ON membership.project_id = connection.project_id
         AND membership.actor_id = connection.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = connection.actor_id
        WHERE credential.id = $1
          AND agent_connection_can_access(connection.id, $2)
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active'
          AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY['events']::text[]
          AND connection.status = 'active'
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent'
          AND actor.status = 'active'
          AND project.archived_at IS NULL
          AND membership.enabled
          AND organization_membership.enabled
        "#,
    )
    .bind(principal.credential_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .bind(connection_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| PollFailure::Database("revalidate"))
}

async fn cursor_is_expired(
    transaction: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    after_sequence: i64,
) -> Result<bool, PollFailure> {
    let (next_sequence, minimum_available) = sqlx::query_as::<_, (i64, Option<i64>)>(
        r#"
        SELECT counter.next_sequence,
               (SELECT min(sequence) FROM external_agent_events
                WHERE connection_id = $1 AND expires_at > now())
        FROM external_agent_event_counters counter
        WHERE counter.connection_id = $1
        "#,
    )
    .bind(connection_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| PollFailure::Database("cursor_retention"))?
    .unwrap_or((1, None));
    if after_sequence >= next_sequence {
        return Ok(true);
    }
    Ok(match minimum_available {
        Some(minimum) => after_sequence + 1 < minimum,
        None => next_sequence > after_sequence + 1,
    })
}

#[derive(Clone, Copy, Debug)]
enum PollFailure {
    Authorization,
    InvalidCursor,
    Database(&'static str),
}

fn denied() -> ExternalEventPollResult {
    ExternalEventPollResult::PermissionDenied {
        retry_category: "reauthenticate_or_request_scope".to_owned(),
    }
}

impl From<ExternalEvent> for ExternalEventProjection {
    fn from(event: ExternalEvent) -> Self {
        Self {
            id: event.id.to_string(),
            sequence: event.sequence,
            kind: event.kind,
            summary: event.summary,
            resource_kind: event.resource_kind,
            resource_id: event.resource_id.to_string(),
            canonical_path: event.canonical_path,
            created_at: event.created_at,
            expires_at: event.expires_at,
        }
    }
}
