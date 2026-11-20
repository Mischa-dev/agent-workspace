//! K1.24: narrow tokens. A short-lived bearer token for one account in one
//! workspace, holding at most what the account holds and optionally less:
//! a subset of its permissions, a subset of its spaces. A run's token (what
//! a host is handed for a run's agent) is one use; anything that needs a
//! short, narrow credential for an account issues one here.
//!
//! The token is shown once, to whoever asked; only its digest is kept. It
//! stops working when it expires or is revoked, when the account stops being
//! active, and for a run's token also when the run ends or the host lets the
//! agent go. What it holds is read again on every call, so it never reaches
//! further than the account does at that moment.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{Caller, Scope};
use crate::{
    error::AppError,
    policy::{AgentLimits, PermissionSet},
};

pub const TOKEN_PREFIX: &str = "aw_at_";
const TOKEN_PREFIX_LENGTH: usize = 12;
/// The longest a token may live.
pub const MAX_MINUTES: i64 = 24 * 60;

/// The run a token is for, when it is a run's token.
#[derive(Clone, Copy, Debug)]
pub struct RunBinding {
    pub run_id: Uuid,
    pub host_id: Uuid,
    pub command_id: Uuid,
}

/// What to issue.
#[derive(Clone, Debug)]
pub struct ScopedTokenRequest {
    pub account_id: Uuid,
    pub workspace_id: Uuid,
    /// The space the token starts in (where a call naming none acts).
    pub home_space: Uuid,
    /// `None`: everything the account holds where the token reaches.
    pub permissions: Option<PermissionSet>,
    /// `None`: every space of the workspace the account is in. Otherwise
    /// only these; the home space is always among them.
    pub spaces: Option<Vec<Uuid>>,
    pub minutes: i64,
    /// The account that asked for it, when not the account itself.
    pub requested_by: Option<Uuid>,
    pub run: Option<RunBinding>,
    /// The secret it was exchanged from: revoking that stops it at once.
    pub credential_id: Option<Uuid>,
    /// `read`, and `write` when it may change things; `None` is both.
    pub scopes: Option<Vec<String>>,
}

/// A token just issued: the token itself is in no row.
#[derive(Clone, Debug)]
pub struct Issued {
    pub id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Issue a token, refusing one that would hold more than the account does:
/// a space it isn't in, or a permission it holds in none of the token's
/// spaces nor at workspace level.
pub async fn issue_scoped(
    transaction: &mut Transaction<'_, Postgres>,
    request: ScopedTokenRequest,
) -> Result<Issued, AppError> {
    if !(1..=MAX_MINUTES).contains(&request.minutes) {
        return Err(AppError::Validation(
            "a token lives between a minute and a day",
        ));
    }
    let mut spaces = request.spaces.clone();
    if let Some(spaces) = spaces.as_mut() {
        if !spaces.contains(&request.home_space) {
            spaces.insert(0, request.home_space);
        }
        spaces.sort_unstable();
        spaces.dedup();
        if spaces.len() > 200 {
            return Err(AppError::Validation("a token reaches at most 200 spaces"));
        }
    }
    let kind = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM actors WHERE id = $1 AND status = 'active'",
    )
    .bind(request.account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .and_then(|kind| match kind.as_str() {
        // The built-in host's agents are execution workers (A1.16); a run's
        // token signs them in like any host's agent.
        "execution_worker" => Some(crate::policy::ActorKind::ExecutionWorker), // kind-check: mechanical (which account a run's token signs in)
        other => crate::mcp::parse_mcp_actor_kind(other).ok(),
    })
    .ok_or(AppError::Forbidden)?;
    let mut account = Caller::person(request.account_id, kind);
    // A run's token holds what the run delegates in its space: a worker
    // membership there is enough, as it is when the token is used.
    account.for_run = request.run.is_some();
    // An agent reaches spaces through its connection in the workspace, as
    // the token will when it's used.
    account.connection_id =
        connection_in(transaction, request.account_id, request.workspace_id).await?;
    let checked = spaces.clone().unwrap_or_else(|| vec![request.home_space]);
    let mut held = PermissionSet::EMPTY;
    for space in &checked {
        let holding = super::holding::load(transaction, &account, Scope::Space(*space), true)
            .await?
            .filter(|holding| holding.workspace_id == request.workspace_id)
            .ok_or(AppError::Refused {
                field: "spaces",
                message: "The account isn't in every space the token would reach.",
            })?;
        held = held.union(holding.permissions);
    }
    if let Some(holding) = super::holding::load(
        transaction,
        &account,
        Scope::Workspace(request.workspace_id),
        true,
    )
    .await?
    {
        held = held.union(holding.permissions);
    }
    if let Some(wanted) = request.permissions
        && !wanted.is_subset_of(held)
    {
        return Err(AppError::Refused {
            field: "permissions",
            message: "A token can't hold more than its account does.",
        });
    }
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let id = Uuid::now_v7();
    let expires_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        INSERT INTO access_tokens (
            id, workspace_id, space_id, run_id, account_id, host_id, command_id,
            token_prefix, token_digest, permissions, spaces, requested_by_actor_id, expires_at,
            credential_id, scopes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                now() + make_interval(mins => $13::int), $14,
                COALESCE($15, ARRAY['read', 'write']::TEXT[]))
        RETURNING expires_at
        "#,
    )
    .bind(id)
    .bind(request.workspace_id)
    .bind(request.home_space)
    .bind(request.run.map(|run| run.run_id))
    .bind(request.account_id)
    .bind(request.run.map(|run| run.host_id))
    .bind(request.run.map(|run| run.command_id))
    .bind(&token[..TOKEN_PREFIX_LENGTH])
    .bind(digest.as_slice())
    .bind(request.permissions.map(PermissionSet::keys))
    .bind(spaces)
    .bind(request.requested_by)
    .bind(request.minutes as i32)
    .bind(request.credential_id)
    .bind(request.scopes.as_deref())
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Issued {
        id,
        token,
        expires_at,
    })
}

/// The account's own active connection in the workspace, when it has one
/// (an agent's): what a token for it reaches through.
async fn connection_in(
    connection: &mut PgConnection,
    account_id: Uuid,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(CONNECTION_IN)
        .bind(account_id)
        .bind(workspace_id)
        .fetch_optional(&mut *connection)
        .await?)
}

const CONNECTION_IN: &str = r#"
    SELECT connection.id FROM external_agent_connections connection
    JOIN projects home ON home.id = connection.project_id
    WHERE connection.actor_id = $1 AND connection.status = 'active'
      AND home.organization_id = $2
    ORDER BY connection.id LIMIT 1
"#;

/// A run's tokens stop working: it ended.
pub async fn revoke_for_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "UPDATE access_tokens SET revoked_at = now() WHERE run_id = $1 AND revoked_at IS NULL",
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

/// One token stops working.
pub async fn revoke(connection: &mut PgConnection, token_id: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query(
        "UPDATE access_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        > 0)
}

/// Who a token speaks for.
#[derive(Clone, Debug, FromRow)]
pub struct Holder {
    pub token_id: Uuid,
    pub account_id: Uuid,
    pub account_kind: String,
    pub workspace_id: Uuid,
    pub space_id: Uuid,
    pub run_id: Option<Uuid>,
    /// The host it was handed to, for a run's token.
    pub host_name: Option<String>,
    /// The agent's own connection in the workspace, when it has one: the
    /// token reaches what that connection reaches, no more.
    pub connection_id: Option<Uuid>,
    pub scopes: Vec<String>,
    /// Whether it was narrowed to listed spaces, rather than reaching every
    /// space of the account in its workspace.
    pub listed_spaces: bool,
    token_digest: Vec<u8>,
}

/// The account a token is for, while it still works.
pub async fn resolve(pool: &PgPool, token: &str) -> Result<Option<Holder>, AppError> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        return Ok(None);
    }
    let Some(prefix) = token.get(..TOKEN_PREFIX_LENGTH) else {
        return Ok(None);
    };
    let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let holder = sqlx::query_as::<_, Holder>(
        r#"
        SELECT token.id AS token_id, token.account_id, account.kind AS account_kind,
               token.workspace_id, token.space_id, token.run_id,
               host.name AS host_name, token.token_digest, token.scopes,
               token.spaces IS NOT NULL AS listed_spaces,
               (SELECT connection.id FROM external_agent_connections connection
                JOIN projects home ON home.id = connection.project_id
                WHERE connection.actor_id = token.account_id AND connection.status = 'active'
                  AND home.organization_id = token.workspace_id
                ORDER BY connection.id LIMIT 1) AS connection_id
        FROM access_tokens token
        JOIN actors account ON account.id = token.account_id
        LEFT JOIN organization_memberships workspace_membership
          ON workspace_membership.organization_id = token.workspace_id
         AND workspace_membership.actor_id = token.account_id
        JOIN projects space ON space.id = token.space_id
        JOIN project_memberships membership
          ON membership.project_id = token.space_id AND membership.actor_id = token.account_id
        LEFT JOIN hosts host ON host.id = token.host_id
        LEFT JOIN host_agents binding
          ON binding.host_id = token.host_id AND binding.account_id = token.account_id
        LEFT JOIN account_credentials credential ON credential.id = token.credential_id
        WHERE token.token_prefix = $1
          AND token.revoked_at IS NULL AND token.expires_at > now()
          AND account.status = 'active'
          AND space.archived_at IS NULL AND membership.enabled
          -- An enabled workspace membership; a run's agent may have none
          -- (a worker on the built-in host), though a disabled one stops it.
          AND (workspace_membership.enabled
               OR (token.run_id IS NOT NULL AND workspace_membership.actor_id IS NULL))
          AND (token.credential_id IS NULL
               OR (credential.status = 'active' AND credential.expires_at > now()))
          AND (token.host_id IS NULL
               OR (host.status <> 'revoked' AND binding.status <> 'gone'))
        "#,
    )
    .bind(prefix);
    let holder = crate::db::scoped_one(pool, async |connection| {
        holder.fetch_optional(connection).await
    })
    .await?;
    let Some(holder) = holder.filter(|holder| {
        holder.token_digest.len() == candidate.len()
            && bool::from(holder.token_digest.as_slice().ct_eq(&candidate))
    }) else {
        return Ok(None);
    };
    // A run's token stops when its run does, even before it is revoked.
    if let Some(run_id) = holder.run_id
        && !crate::runs::credentials::run_open(pool, run_id).await?
    {
        return Ok(None);
    }
    Ok(Some(holder))
}

/// How far a token reaches: its workspace, and in it the spaces and
/// permissions it was narrowed to.
#[derive(Clone, Debug)]
pub struct TokenScope {
    pub workspace_id: Uuid,
    pub permissions: Option<PermissionSet>,
    /// Every space the token reaches.
    pub spaces: Vec<Uuid>,
    /// Whether it was narrowed to listed spaces (and so holds nothing that
    /// reaches the whole workspace).
    pub listed_spaces: bool,
}

impl TokenScope {
    /// The account's reach and limits narrowed to the token: nothing outside
    /// its workspace or spaces, nothing beyond its permissions. A token
    /// narrowed to listed spaces holds no workspace-wide permission.
    pub fn narrow(
        &self,
        account_id: Uuid,
        reach: PermissionSet,
        limits: Option<&AgentLimits>,
    ) -> (PermissionSet, AgentLimits) {
        let allowed = self.permissions.map_or(PermissionSet::all(), |wanted| {
            wanted.union(PermissionSet::required())
        });
        let mut narrowed = AgentLimits {
            actor_id: account_id,
            ..AgentLimits::default()
        };
        let mut at_workspace = allowed;
        if self.listed_spaces {
            at_workspace = at_workspace
                .without_all(PermissionSet::workspace_only())
                .union(PermissionSet::required());
        }
        let existing = |found: Option<&PermissionSet>| -> PermissionSet {
            match limits {
                Some(_) => found.copied().unwrap_or(PermissionSet::EMPTY),
                None => PermissionSet::all(),
            }
        };
        narrowed.organizations.insert(
            self.workspace_id,
            at_workspace.intersect(existing(
                limits.and_then(|limits| limits.organizations.get(&self.workspace_id)),
            )),
        );
        for space in &self.spaces {
            narrowed.projects.insert(
                *space,
                allowed.intersect(existing(
                    limits.and_then(|limits| limits.projects.get(space)),
                )),
            );
        }
        (reach.intersect(allowed), narrowed)
    }
}

/// The run a token is for and who asked for it (K1.25), so what it writes
/// says so.
pub async fn acting_of(
    connection: &mut PgConnection,
    token_id: Uuid,
) -> Result<(Option<Uuid>, Option<Uuid>), AppError> {
    Ok(sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>)>(
        "SELECT run_id, requested_by_actor_id FROM access_tokens WHERE id = $1",
    )
    .bind(token_id)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or_default())
}

#[derive(FromRow)]
struct ScopeRow {
    workspace_id: Uuid,
    permissions: Option<Vec<String>>,
    spaces: Option<Vec<Uuid>>,
}

/// The scope of a token, read again on every call.
pub async fn scope_of(
    connection: &mut PgConnection,
    token_id: Uuid,
) -> Result<Option<TokenScope>, AppError> {
    let Some(row) = sqlx::query_as::<_, ScopeRow>(
        "SELECT workspace_id, permissions, spaces FROM access_tokens WHERE id = $1",
    )
    .bind(token_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    let listed_spaces = row.spaces.is_some();
    let spaces = match row.spaces {
        Some(spaces) => spaces,
        None => {
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects WHERE organization_id = $1")
                .bind(row.workspace_id)
                .fetch_all(&mut *connection)
                .await?
        }
    };
    Ok(Some(TokenScope {
        workspace_id: row.workspace_id,
        permissions: row.permissions.as_deref().map(PermissionSet::from_keys),
        spaces,
        listed_spaces,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Permission;

    #[test]
    fn a_listed_space_token_holds_nothing_workspace_wide_and_nothing_elsewhere() {
        let workspace = Uuid::now_v7();
        let home = Uuid::now_v7();
        let elsewhere = Uuid::now_v7();
        let scope = TokenScope {
            workspace_id: workspace,
            permissions: None,
            spaces: vec![home],
            listed_spaces: true,
        };
        let (reach, limits) = scope.narrow(Uuid::now_v7(), PermissionSet::all(), None);
        assert_eq!(reach, PermissionSet::all());
        assert_eq!(limits.projects.get(&home), Some(&PermissionSet::all()));
        assert!(!limits.projects.contains_key(&elsewhere));
        let at_workspace = limits.organizations[&workspace];
        assert!(
            PermissionSet::workspace_only()
                .without_all(PermissionSet::required())
                .iter()
                .all(|permission| !at_workspace.contains(permission))
        );
    }

    #[test]
    fn a_token_narrows_an_apps_limit_further_never_wider() {
        let workspace = Uuid::now_v7();
        let home = Uuid::now_v7();
        let some = PermissionSet::EMPTY.with(Permission::ProjectsView);
        let scope = TokenScope {
            workspace_id: workspace,
            permissions: None,
            spaces: vec![home],
            listed_spaces: true,
        };
        let app = AgentLimits {
            actor_id: Uuid::now_v7(),
            organizations: [(workspace, some)].into(),
            projects: [(home, some)].into(),
        };
        let (_, limits) = scope.narrow(app.actor_id, PermissionSet::all(), Some(&app));
        assert!(limits.projects[&home].is_subset_of(some));
        assert!(limits.organizations[&workspace].is_subset_of(some));
    }
}
