//! Agent-only reads: the check that an agent's credential and connection are
//! still active, and the older per-connection event inbox.

use std::time::Duration;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{
    AppState,
    addresses::{Place, Target},
    error::AppError,
    policy::{Action, ActorKind},
};

#[derive(Debug, FromRow)]
pub(super) struct ConnectionAuthorization {
    membership_role: String,
    membership_role_permissions: Option<Vec<String>>,
    membership_group_permissions: Option<Vec<String>>,
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
    let through_connection = principal.actor_kind == ActorKind::ExternalAgent; // kind-check: mechanical (agents sign in through connections)
    if !through_connection || !principal.has_scope(required_scope) {
        return Err(ReadFailure::Authorization);
    }
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(|_| ReadFailure::Database("begin"))?;
    let authorization = sqlx::query_as::<_, ConnectionAuthorization>(
        r#"
        SELECT membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions,
               space_group_permissions(credential.actor_id, project.id)
                 AS membership_group_permissions
        FROM account_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = credential.actor_id
        -- In a DM or group (the direct scope) the agent answers as a member
        -- of its connection's own space.
        JOIN projects project
          ON project.id = COALESCE(NULLIF($2::uuid, direct_scope_id()), credential.project_id)
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = credential.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = credential.actor_id
        WHERE credential.id = $1
          AND agent_connection_can_access(connection.id, project.id)
          AND ($2 <> direct_scope_id() OR agent_connection_can_reach_direct(connection.id))
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active' AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY[$5]::text[]
          AND connection.status = 'active'
          AND connection.project_id = credential.project_id
          AND connection.actor_id = credential.actor_id
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent' AND actor.status = 'active' -- // kind-check: mechanical (agents sign in through connections)
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
    if !super::common::role_allows(
        principal,
        principal.project_id,
        &super::common::StoredRole::of(
            &authorization.membership_role,
            authorization.membership_role_permissions.as_deref(),
        )
        .with_groups(authorization.membership_group_permissions.clone()),
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

/// The older event feed, read through the caller's connection on the
/// delivery side (A2.6): the same events `GET /api/v1/events` serves, under
/// the names this feed always used. A cursor from before is expired.
pub(super) async fn poll_external_events(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ExternalEventPollArguments,
) -> ExternalEventPollResult {
    if principal.external_agent_connection_id.is_none() || !principal.has_scope("events") {
        return denied();
    }
    let limit = arguments.limit.unwrap_or(50);
    let wait_seconds = arguments.wait_seconds.unwrap_or(0);
    if !(1..=100).contains(&limit) || wait_seconds > 30 {
        return invalid();
    }
    let cursor = arguments
        .after_cursor
        .as_deref()
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty());
    if cursor.is_some_and(|cursor| !cursor.starts_with("c2.")) {
        return expired();
    }
    let caller = crate::access::Caller::of_principal(principal, None);
    match crate::delivery::poll(
        state,
        &caller,
        None,
        cursor,
        i64::from(limit),
        Duration::from_secs(u64::from(wait_seconds)),
    )
    .await
    {
        Ok(page) => ExternalEventPollResult::Events {
            events: page
                .events
                .iter()
                .zip(&page.positions)
                .map(|(event, position)| project(event, *position))
                .collect(),
            next_cursor: page.next_cursor,
            has_more: page.has_more,
        },
        Err(AppError::Conflict("cursor_expired", _)) => expired(),
        Err(AppError::Validation(_)) => invalid(),
        Err(AppError::NotFound(_) | AppError::Forbidden) => denied(),
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "external event poll failed"
            );
            ExternalEventPollResult::TemporarilyUnavailable {
                retry_category: "retry_with_backoff".to_owned(),
            }
        }
    }
}

/// One delivered event in the older feed's shape.
fn project(event: &Value, position: i64) -> ExternalEventProjection {
    let kind = event["type"].as_str().unwrap_or_default();
    let entry = crate::delivery::catalogue::get(kind);
    let data = &event["data"];
    let id_of = |value: &Value| value.as_str().and_then(|id| id.parse::<Uuid>().ok());
    let thing_kind = data["thing"]["kind"].as_str().unwrap_or_default();
    let thing = id_of(&data["thing"]["id"]);
    let space = id_of(&data["spaceId"]);
    let conversation = id_of(&data["threadId"]).or_else(|| id_of(&data["conversationId"]));
    let target = match (thing_kind, thing, space, conversation) {
        ("task", Some(id), Some(space), _) => Target::task(space, id),
        ("page", Some(id), Some(space), _) => Target::page(space, id),
        ("run", Some(id), _, _) => Target::Run(id),
        ("request", ..) => Target::Inbox,
        ("saved_item", ..) => Target::Later,
        (_, _, Some(space), Some(conversation)) => {
            Target::space(space, Place::Thread(conversation))
        }
        _ => Target::Home,
    };
    let created_at = event["timestamp"].as_str().unwrap_or_default().to_owned();
    ExternalEventProjection {
        id: event["id"].as_str().unwrap_or_default().to_owned(),
        sequence: position,
        kind: entry.map_or(kind, |entry| entry.feed_name).to_owned(),
        summary: entry.map_or("", |entry| entry.summary).to_owned(),
        resource_kind: thing_kind.to_owned(),
        resource_id: thing.map(|id| id.to_string()).unwrap_or_default(),
        canonical_path: target.legacy_path(),
        expires_at: created_at.clone(),
        created_at,
    }
}

fn denied() -> ExternalEventPollResult {
    ExternalEventPollResult::PermissionDenied {
        retry_category: "reauthenticate_or_request_scope".to_owned(),
    }
}

fn invalid() -> ExternalEventPollResult {
    ExternalEventPollResult::InvalidRequest {
        retry_category: "do_not_retry_without_change".to_owned(),
    }
}

fn expired() -> ExternalEventPollResult {
    ExternalEventPollResult::CursorExpired {
        retry_category: "resynchronize".to_owned(),
        resync_path: Target::Home.legacy_path(),
    }
}
