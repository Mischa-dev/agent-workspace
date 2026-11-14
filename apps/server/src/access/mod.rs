//! The access module (K1): the one place that reads roles, memberships,
//! delegation limits, credential scopes, module switches and account status
//! to decide what a caller may do (ruling R2: allow, suggest only, deny).
//!
//! - `caller`: who is calling, resolved at the edge (`Caller`, `Via`,
//!   `Reach`).
//! - `decide`: the pure core, with no database and no account kind.
//! - `holding`: what a caller holds in one scope.
//! - `compare`: old and new checks side by side while areas move (K1 §4.3).
//! - `http`: the operators' view of recorded disagreements.
//!
//! How the functions relate: `decide` is the pure core, called only here.
//! `check` and `check_locked` answer allow or deny for a need on a target.
//! `holding` says what the caller holds in a scope (tool lists, "my
//! permissions"). `write_mode` (K1.12) answers the three-way question for
//! writes on kinds with levels. Everything else calls these.

pub mod caller;
pub mod compare;
pub mod decide;
pub mod holding;
pub mod http;

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

pub use caller::{Caller, Reach, Via};
pub use decide::{Credential, decide};

use crate::{
    error::AppError,
    modules::ModuleSet,
    policy::{MembershipRole, Permission, PermissionSet},
};

/// Where a holding applies.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Scope {
    Workspace(Uuid),
    Space(Uuid),
}

/// The tier of the role a holding comes from: what "only owners give
/// owner" and ownership read. Never the account's kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    Owner,
    Admin,
    Member,
    Viewer,
    Guest,
    Worker,
}

impl From<MembershipRole> for Tier {
    fn from(role: MembershipRole) -> Self {
        match role {
            MembershipRole::Owner => Self::Owner,
            MembershipRole::Admin => Self::Admin,
            MembershipRole::Member => Self::Member,
            MembershipRole::Viewer => Self::Viewer,
            MembershipRole::Worker => Self::Worker,
        }
    }
}

/// What the caller holds in one scope after every narrowing.
#[derive(Clone, Copy, Debug)]
pub struct Holding {
    pub scope: Scope,
    pub workspace_id: Uuid,
    pub permissions: PermissionSet,
    pub tier: Tier,
    /// The modules on there for this caller; `None` when the space has no
    /// module set of its own.
    pub modules: Option<ModuleSet>,
    /// The holding was narrowed by a delegation limit (an app acting for
    /// the account).
    pub delegated: bool,
}

/// What a call needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Need {
    /// The permission; `None` means "may see the target".
    pub permission: Option<Permission>,
    /// The module that must be on where it acts.
    pub module: Option<&'static str>,
    /// Whether the call changes anything.
    pub writes: bool,
}

impl Need {
    pub const SEE: Self = Self {
        permission: None,
        module: None,
        writes: false,
    };

    pub const fn permission(permission: Permission) -> Self {
        Self {
            permission: Some(permission),
            module: None,
            writes: false,
        }
    }

    pub const fn maybe(permission: Option<Permission>) -> Self {
        Self {
            permission,
            module: None,
            writes: false,
        }
    }

    #[must_use]
    pub const fn writes(mut self) -> Self {
        self.writes = true;
        self
    }

    #[must_use]
    pub const fn writing(mut self, writes: bool) -> Self {
        self.writes = writes;
        self
    }

    #[must_use]
    pub const fn in_module(mut self, module: &'static str) -> Self {
        self.module = Some(module);
        self
    }
}

/// What a call acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// The caller's own things.
    Account,
    Workspace(Uuid),
    Space(Uuid),
}

impl Target {
    /// The record's target kind and id.
    pub fn parts(&self) -> (&'static str, Option<Uuid>) {
        match self {
            Self::Account => ("account", None),
            Self::Workspace(id) => ("workspace", Some(*id)),
            Self::Space(id) => ("space", Some(*id)),
        }
    }
}

/// How a write may be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    Direct,
    /// Held as a proposal for someone who may edit (C1).
    Propose,
}

/// Proof that a check passed.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Grant {
    pub permission: Option<Permission>,
    pub mode: WriteMode,
}

/// Why a permission was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    Role,
    Delegation,
    ReadOnlyCredential,
    CredentialCannotRead,
}

/// A refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Denied {
    /// The caller can't see the thing: answer 404.
    NotFound,
    /// Visible, but the role, a delegation limit or the credential leaves
    /// this out.
    Permission {
        permission: Option<Permission>,
        because: DenyReason,
    },
    ModuleOff {
        module: &'static str,
    },
}

impl Denied {
    /// A short key for logs and records.
    pub fn key(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Permission {
                because: DenyReason::Role,
                ..
            } => "role",
            Self::Permission {
                because: DenyReason::Delegation,
                ..
            } => "delegation",
            Self::Permission {
                because: DenyReason::ReadOnlyCredential,
                ..
            } => "read_only_credential",
            Self::Permission {
                because: DenyReason::CredentialCannotRead,
                ..
            } => "credential_cannot_read",
            Self::ModuleOff { .. } => "module_off",
        }
    }
}

fn credential_of(caller: &Caller) -> Credential {
    Credential {
        reads: caller.can_read,
        writes: caller.can_write,
    }
}

/// What the caller holds in `scope`.
pub async fn holding(
    connection: &mut PgConnection,
    caller: &Caller,
    scope: Scope,
) -> Result<Option<Holding>, AppError> {
    holding::load(connection, caller, scope, false).await
}

/// The one question: may this caller do this to this target?
pub async fn check(
    connection: &mut PgConnection,
    caller: &Caller,
    need: Need,
    target: Target,
) -> Result<Result<Grant, Denied>, AppError> {
    check_with(connection, caller, need, target, false).await
}

/// Same as `check`, taking the membership rows `FOR SHARE` inside a write
/// transaction.
pub async fn check_locked(
    tx: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    need: Need,
    target: Target,
) -> Result<Result<Grant, Denied>, AppError> {
    check_with(&mut **tx, caller, need, target, true).await
}

async fn check_with(
    connection: &mut PgConnection,
    caller: &Caller,
    need: Need,
    target: Target,
    lock: bool,
) -> Result<Result<Grant, Denied>, AppError> {
    let scope = match target {
        Target::Account => {
            // The caller's own things: only the credential matters.
            let own = decide(
                None,
                &Need {
                    permission: None,
                    module: None,
                    writes: need.writes,
                },
                credential_of(caller),
            );
            return Ok(match own {
                Err(Denied::NotFound) => Ok(Grant {
                    permission: None,
                    mode: WriteMode::Direct,
                }),
                other => other,
            });
        }
        Target::Workspace(id) => Scope::Workspace(id),
        Target::Space(id) => Scope::Space(id),
    };
    let started = std::time::Instant::now();
    let held = holding::load(connection, caller, scope, lock).await?;
    let decided = decide(held.as_ref(), &need, credential_of(caller));
    tracing::trace!(
        target: "access.check",
        elapsed_us = started.elapsed().as_micros() as u64,
        allowed = decided.is_ok(),
        "access check"
    );
    Ok(decided)
}
/// A list filter: SQL true for rows whose space (`space_column`) the
/// caller may `need` in, with its binds starting at `$first`.
pub struct SqlFilter {
    pub sql: String,
    pub account_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub permission: &'static str,
    /// The spaces a delegation limit leaves `need` in; `None` when no
    /// limit applies.
    pub limited_to: Option<Vec<Uuid>>,
}

/// The rule for lists, from the same data as `check`: the caller reaches
/// the space (`actor_can_access_project`), their role there holds the
/// permission (`role_allows`), and a delegation limit, if any, allows it.
/// Bind, in order: `account_id`, `connection_id`, `permission`, then
/// `limited_to` when it is `Some`.
pub fn sql_filter(
    caller: &Caller,
    need: Permission,
    space_column: &str,
    first: usize,
) -> SqlFilter {
    let (account, connection, permission) = (first, first + 1, first + 2);
    let limited_to = caller.limits.as_ref().map(|limits| {
        limits
            .projects
            .iter()
            .filter(|(_, allowed)| allowed.contains(need))
            .map(|(space, _)| *space)
            .collect::<Vec<_>>()
    });
    let mut sql = format!(
        "(actor_can_access_project(${account}, ${connection}, {space_column}) \
         AND EXISTS (SELECT 1 FROM project_memberships access_membership \
             WHERE access_membership.project_id = {space_column} \
               AND access_membership.actor_id = ${account} AND access_membership.enabled \
               AND role_allows(access_membership.role, access_membership.role_id, ${permission}))"
    );
    if limited_to.is_some() {
        sql.push_str(&format!(" AND {space_column} = ANY(${})", first + 3));
    }
    sql.push(')');
    SqlFilter {
        sql,
        account_id: caller.account_id,
        connection_id: caller.connection_id,
        permission: need.key(),
        limited_to,
    }
}
