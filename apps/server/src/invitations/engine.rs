//! One invitation engine (K1.29): what every way of letting someone into a
//! workspace shares. A project invitation (`/api/v1/invitations/...`), a
//! workspace invitation (`/api/v1/workspace-invitations/...`), a claim link
//! for an unclaimed account and an invitation link (K4.26) each keep their
//! own token family and address, and all of them:
//!
//! - make and read their one-time token here (`issue`, `consume`): only its
//!   digest is stored, compared in constant time, and the token is shown
//!   once;
//! - check the issuer through `access::may_grant` (`issuer_may_grant`):
//!   nobody lets someone in with more than they hold;
//! - make a new person through `accounts::create_person`;
//! - admit the account to the workspace through `admit`: a membership with
//!   the role, or a disabled one turned back on, and the account's name
//!   there.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    access::{self, Caller, Scope},
    error::AppError,
    outbox::{OutboxEventPayload, ProjectMembershipUpdatedPayload, enqueue_typed},
    policy::{MembershipRole, PermissionSet, RoleGrant},
};

const TOKEN_BYTES: usize = 32;
/// The stored, indexed start of a token: its family tag and a few
/// characters, never enough to use it.
pub const PREFIX_LENGTH: usize = 16;

/// The token families. Each tag is seven characters, so a token is always
/// 50 characters long.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    /// A project invitation.
    Project,
    /// A workspace invitation.
    Workspace,
    /// A claim link for an unclaimed account (K1.29).
    Claim,
    /// An invitation link anyone holding it may use, up to its limit (K4.26).
    Link,
}

impl Family {
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Project => "aw_inv_",
            Self::Workspace => "aw_wsi_",
            Self::Claim => "aw_clm_",
            Self::Link => "aw_lnk_",
        }
    }
}

/// A new token: shown once, stored only as `prefix` and `digest`.
pub struct Issued {
    pub token: String,
    pub prefix: String,
    pub digest: [u8; 32],
}

pub fn issue(family: Family) -> Issued {
    let mut bytes = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{}{}", family.tag(), URL_SAFE_NO_PAD.encode(bytes));
    bytes.zeroize();
    let prefix = token[..PREFIX_LENGTH].to_owned();
    let digest = Sha256::digest(token.as_bytes()).into();
    Issued {
        token,
        prefix,
        digest,
    }
}

/// Read a presented token of `family` and forget it: its prefix (to find
/// the row) and digest (to compare). A malformed token is `not_found` as
/// `resource`, the same answer as an unknown one; both count against the
/// invitation rate limit.
pub async fn consume(
    token: &mut String,
    family: Family,
    state: &AppState,
    resource: &'static str,
) -> Result<(String, [u8; 32]), AppError> {
    let valid = token.len() == family.tag().len() + 43
        && token.starts_with(family.tag())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    let result = valid.then(|| {
        (
            token[..PREFIX_LENGTH].to_owned(),
            Sha256::digest(token.as_bytes()).into(),
        )
    });
    token.zeroize();
    let Some((prefix, digest)) = result else {
        state.rate_limits.check_invitation("invalid").await?;
        return Err(AppError::NotFound(resource));
    };
    state.rate_limits.check_invitation(&prefix).await?;
    Ok((prefix, digest))
}

/// Whether a stored digest is the presented one, in constant time.
pub fn digest_matches(stored: &[u8], candidate: &[u8; 32]) -> bool {
    stored.len() == candidate.len() && bool::from(stored.ct_eq(candidate))
}

/// The permissions a stored role means: the built-in `role`, narrowed by
/// the custom role `role_id` when there is one.
pub async fn role_permissions(
    connection: &mut PgConnection,
    role: &str,
    role_id: Option<Uuid>,
) -> Result<PermissionSet, AppError> {
    let custom = match role_id {
        Some(role_id) => {
            sqlx::query_scalar::<_, Option<Vec<String>>>("SELECT workspace_role_permissions($1)")
                .bind(role_id)
                .fetch_one(&mut *connection)
                .await?
        }
        None => None,
    };
    Ok(RoleGrant::stored(role, custom.as_deref())
        .map_err(|_| AppError::Validation("role"))?
        .permissions())
}

/// The stored form of a workspace role picked by id: the built-in key and,
/// for a custom role, its id. With no id, the workspace's default role for
/// newcomers (`member` when none is marked). `None` when the id names no
/// role of this workspace.
pub async fn workspace_role(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    role_id: Option<Uuid>,
) -> Result<Option<(String, Option<Uuid>)>, AppError> {
    let Some(role_id) = role_id else {
        return Ok(Some(
            sqlx::query_as::<_, (String, Option<Uuid>)>(
                "SELECT CASE WHEN built_in THEN key ELSE base_role END, \
                        CASE WHEN built_in THEN NULL ELSE id END \
                 FROM workspace_roles WHERE organization_id = $1 AND is_default",
            )
            .bind(workspace_id)
            .fetch_optional(&mut *connection)
            .await?
            .unwrap_or_else(|| ("member".to_owned(), None)),
        ));
    };
    Ok(sqlx::query_as::<_, (String, bool, String)>(
        "SELECT key, built_in, base_role FROM workspace_roles \
         WHERE id = $1 AND organization_id = $2",
    )
    .bind(role_id)
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?
    .map(|(key, built_in, base)| {
        if built_in {
            (key, None)
        } else {
            (base, Some(role_id))
        }
    }))
}

/// The stored form of the role a new agent gets: `role_id` when named, else
/// the workspace's `access.default_agent_role_id` setting when it names a
/// role of this workspace, else the role for newcomers (`workspace_role`).
/// `None` when a named id is no role of this workspace.
pub async fn agent_role(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    role_id: Option<Uuid>,
) -> Result<Option<(String, Option<Uuid>)>, AppError> {
    if role_id.is_some() {
        return workspace_role(connection, workspace_id, role_id).await;
    }
    let chosen = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM settings WHERE organization_id = $1 AND scope = 'workspace' \
         AND key = 'access.default_agent_role_id'",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?
    .and_then(|value| value.as_str().and_then(|id| Uuid::parse_str(id).ok()));
    if let Some(chosen) = chosen
        && let Some(role) = workspace_role(connection, workspace_id, Some(chosen)).await?
        && !is_ownership(&role.0)
    {
        return Ok(Some(role));
    }
    workspace_role(connection, workspace_id, None).await
}

/// Nobody lets someone in with more than they hold: the role's permissions
/// are within the issuer's holding in `scope`, and a role of the Admin tier
/// comes only from an owner (as assigning one does, `roles::assign`).
pub async fn issuer_may_grant(
    connection: &mut PgConnection,
    issuer: &Caller,
    scope: Scope,
    role: &str,
    role_id: Option<Uuid>,
) -> Result<bool, AppError> {
    // A starting role is stored by its key alone, but an admin may have
    // edited it: compare what its row holds now, not the catalogue's
    // defaults for the key.
    let role_id = match (role_id, scope) {
        (Some(role_id), _) => Some(role_id),
        (None, Scope::Workspace(workspace_id)) => {
            starting_role_id(connection, workspace_id, role).await?
        }
        (None, Scope::Space(_)) => None,
    };
    if MembershipRole::parse(role) == Some(MembershipRole::Admin)
        && !access::holding(connection, issuer, scope)
            .await?
            .is_some_and(|holding| holding.tier == access::Tier::Owner)
    {
        return Ok(false);
    }
    let grants = role_permissions(connection, role, role_id).await?;
    access::may_grant(connection, issuer, scope, grants).await
}

/// The row of the workspace's starting role `key`, so a check reads the role
/// as it is now, an admin's edits included, not the catalogue's defaults.
pub async fn starting_role_id(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    key: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM workspace_roles WHERE organization_id = $1 AND built_in AND key = $2",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Whether `role` is ownership, which is handed over (Transfer ownership)
/// and never given on arrival.
pub fn is_ownership(role: &str) -> bool {
    MembershipRole::parse(role) == Some(MembershipRole::Owner)
}

/// Put a newly admitted account in the workspace's own space and tell the
/// space (`project.membership.updated`), so lists and pickers there pick it
/// up. Returns the space.
pub async fn join_workspace_space(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<Uuid, AppError> {
    let space = crate::spaces::workspace::join(transaction, workspace_id, account_id)
        .await?
        .ok_or(AppError::Internal(
            "admitted account is not a workspace member",
        ))?;
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(space)
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?;
    enqueue_typed(
        transaction,
        space,
        OutboxEventPayload::ProjectMembershipUpdated(ProjectMembershipUpdatedPayload {
            project_id: space,
            actor_id: account_id,
            version,
        }),
    )
    .await?;
    Ok(space)
}

/// Someone being let into a workspace.
#[derive(Clone, Copy, Debug)]
pub struct Admission<'a> {
    pub workspace_id: Uuid,
    pub account_id: Uuid,
    /// The built-in role (`admin`, `member`, ...), and the custom role when
    /// the role is one.
    pub role: &'a str,
    pub role_id: Option<Uuid>,
    pub display_name: &'a str,
    pub handle: &'a str,
}

/// What `admit` found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admitted {
    /// A new membership.
    Joined,
    /// A disabled membership turned back on.
    Returned,
    /// Already an enabled member: nothing changed.
    AlreadyMember,
}

/// Admit an account to a workspace: a membership with the role, or a
/// disabled one turned back on with it (an owner who left comes back as an
/// admin at most), and their name there if they have none yet.
pub async fn admit(
    connection: &mut PgConnection,
    admission: Admission<'_>,
) -> Result<Admitted, AppError> {
    let existing = sqlx::query_as::<_, (String, bool)>(
        "SELECT role, enabled FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2 FOR UPDATE",
    )
    .bind(admission.workspace_id)
    .bind(admission.account_id)
    .fetch_optional(&mut *connection)
    .await?;
    let admin = MembershipRole::parse(admission.role) == Some(MembershipRole::Admin);
    let explicit = admin || admission.role_id.is_some();
    let admitted = match existing {
        Some((_, true)) => return Ok(Admitted::AlreadyMember),
        Some((role, false)) => {
            let next = if admin || is_ownership(&role) {
                "admin"
            } else {
                admission.role
            };
            sqlx::query(
                r#"
                UPDATE organization_memberships
                SET enabled = true, role = $3, role_explicit = $4, role_id = $5,
                    version = version + 1, updated_at = now()
                WHERE organization_id = $1 AND actor_id = $2
                "#,
            )
            .bind(admission.workspace_id)
            .bind(admission.account_id)
            .bind(next)
            .bind(explicit)
            .bind(admission.role_id)
            .execute(&mut *connection)
            .await?;
            Admitted::Returned
        }
        None => {
            sqlx::query(
                r#"
                INSERT INTO organization_memberships
                    (organization_id, actor_id, role, role_explicit, role_id)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(admission.workspace_id)
            .bind(admission.account_id)
            .bind(admission.role)
            .bind(explicit)
            .bind(admission.role_id)
            .execute(&mut *connection)
            .await?;
            Admitted::Joined
        }
    };
    sqlx::query(
        r#"
        INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (organization_id, actor_id) DO NOTHING
        "#,
    )
    .bind(admission.workspace_id)
    .bind(admission.account_id)
    .bind(admission.display_name)
    .bind(admission.handle)
    .execute(&mut *connection)
    .await?;
    Ok(admitted)
}

#[cfg(test)]
mod tests {
    use super::{Family, PREFIX_LENGTH, digest_matches, issue};

    #[test]
    fn tokens_are_fifty_characters_and_differ() {
        for family in [
            Family::Project,
            Family::Workspace,
            Family::Claim,
            Family::Link,
        ] {
            let first = issue(family);
            assert_eq!(first.token.len(), 50);
            assert!(first.token.starts_with(family.tag()));
            assert_eq!(first.prefix.len(), PREFIX_LENGTH);
            assert!(digest_matches(&first.digest, &first.digest));
            assert_ne!(issue(family).token, first.token);
        }
    }
}
