//! Agent-only reads: the check that an agent's credential and connection are
//! still active.

use sqlx::{FromRow, Postgres, Transaction};

use super::McpPrincipal;
use crate::{
    AppState,
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
