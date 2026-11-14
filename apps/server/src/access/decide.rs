//! The pure core of a decision (K1 §3.5): a holding, a need and what the
//! credential allows in; allow or deny out. No database, and no account
//! kind: the same holding gives the same answer for a person, an agent or
//! an app (guard 4).

use super::{Denied, DenyReason, Grant, Holding, Need, WriteMode};

/// What the credential a call came through allows, apart from the role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Credential {
    pub reads: bool,
    pub writes: bool,
}

impl Credential {
    pub const FULL: Self = Self {
        reads: true,
        writes: true,
    };
}

/// Decide one need against what the caller holds where it acts (`None`:
/// not a member there, or the account or membership is disabled, which
/// answers `NotFound` so a refusal reveals nothing).
pub fn decide(
    holding: Option<&Holding>,
    need: &Need,
    credential: Credential,
) -> Result<Grant, Denied> {
    if need.writes && !credential.writes {
        return Err(Denied::Permission {
            permission: need.permission,
            because: DenyReason::ReadOnlyCredential,
        });
    }
    if !need.writes && !credential.reads {
        return Err(Denied::Permission {
            permission: need.permission,
            because: DenyReason::CredentialCannotRead,
        });
    }
    let Some(holding) = holding else {
        return Err(Denied::NotFound);
    };
    if let (Some(module), Some(modules)) = (need.module, holding.modules)
        && !modules.contains(module)
    {
        return Err(Denied::ModuleOff { module });
    }
    if let Some(permission) = need.permission
        && !holding.permissions.contains(permission)
    {
        return Err(Denied::Permission {
            permission: Some(permission),
            because: if holding.delegated {
                DenyReason::Delegation
            } else {
                DenyReason::Role
            },
        });
    }
    Ok(Grant {
        permission: need.permission,
        mode: WriteMode::Direct,
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::{
        access::{Scope, Tier},
        policy::{MembershipRole, PERMISSIONS, Permission, PermissionSet, RoleGrant},
    };

    fn holding(role: MembershipRole) -> Holding {
        Holding {
            scope: Scope::Space(Uuid::nil()),
            workspace_id: Uuid::nil(),
            permissions: if role == MembershipRole::Worker {
                PermissionSet::EMPTY
            } else {
                RoleGrant::built_in(role).permissions()
            },
            tier: Tier::from(role),
            modules: None,
            delegated: false,
        }
    }

    /// The built-in roles decide every catalogue key exactly as their
    /// defaults say, and a worker holds nothing of its own (the old core's
    /// matrix, minus the account kind).
    #[test]
    fn built_in_roles_follow_the_catalogue() {
        for (index, role) in [
            MembershipRole::Owner,
            MembershipRole::Admin,
            MembershipRole::Member,
            MembershipRole::Viewer,
        ]
        .into_iter()
        .enumerate()
        {
            for info in PERMISSIONS {
                let decided = decide(
                    Some(&holding(role)),
                    &Need::permission(info.permission),
                    Credential::FULL,
                );
                assert_eq!(
                    decided.is_ok(),
                    info.built_in[index],
                    "{role:?} {}",
                    info.key
                );
            }
        }
        for info in PERMISSIONS {
            assert!(
                decide(
                    Some(&holding(MembershipRole::Worker)),
                    &Need::permission(info.permission),
                    Credential::FULL
                )
                .is_err()
            );
        }
    }

    #[test]
    fn no_membership_is_not_found() {
        assert!(matches!(
            decide(
                None,
                &Need::permission(Permission::TasksView),
                Credential::FULL
            ),
            Err(Denied::NotFound)
        ));
    }

    #[test]
    fn a_read_only_credential_cannot_write() {
        let held = holding(MembershipRole::Owner);
        let read_only = Credential {
            reads: true,
            writes: false,
        };
        assert!(
            decide(
                Some(&held),
                &Need::permission(Permission::TasksView),
                read_only
            )
            .is_ok()
        );
        assert!(matches!(
            decide(
                Some(&held),
                &Need::permission(Permission::TasksEdit).writes(),
                read_only
            ),
            Err(Denied::Permission {
                because: DenyReason::ReadOnlyCredential,
                ..
            })
        ));
    }

    #[test]
    fn a_delegation_limit_is_named() {
        let mut held = holding(MembershipRole::Admin);
        held.permissions = held.permissions.without(Permission::SettingsManage);
        held.delegated = true;
        assert!(matches!(
            decide(
                Some(&held),
                &Need::permission(Permission::SettingsManage),
                Credential::FULL
            ),
            Err(Denied::Permission {
                because: DenyReason::Delegation,
                ..
            })
        ));
    }

    #[test]
    fn modules_off_refuse_before_the_role() {
        let mut held = holding(MembershipRole::Owner);
        held.modules = Some(crate::modules::ModuleSet::always_on());
        assert!(matches!(
            decide(
                Some(&held),
                &Need::permission(Permission::PagesEdit).in_module("pages"),
                Credential::FULL
            ),
            Err(Denied::ModuleOff { module: "pages" })
        ));
        held.modules = Some(crate::modules::ModuleSet::all());
        assert!(
            decide(
                Some(&held),
                &Need::permission(Permission::PagesEdit).in_module("pages"),
                Credential::FULL
            )
            .is_ok()
        );
    }
}
