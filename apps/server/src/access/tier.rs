//! Role tiers by name (K1.19): what "only owners give owner and admin
//! roles", ownership and the last owner read. A tier is the built-in role a
//! role is based on, never an account's kind; what a role lets someone do
//! is its permissions, asked through `access`.

use crate::policy::MembershipRole;

/// The tier a stored role key names, if it names one.
pub fn of(stored: &str) -> Option<MembershipRole> {
    MembershipRole::parse(stored)
}

/// The owner tier.
pub fn owner(role: MembershipRole) -> bool {
    role == MembershipRole::Owner
}

/// The owner or admin tier: roles only an owner gives, changes or limits.
pub fn high(role: MembershipRole) -> bool {
    matches!(role, MembershipRole::Owner | MembershipRole::Admin)
}

/// A stored role key of the owner tier.
pub fn stored_owner(stored: &str) -> bool {
    of(stored).is_some_and(owner)
}

/// A stored role key of the owner or admin tier.
pub fn stored_high(stored: &str) -> bool {
    of(stored).is_some_and(high)
}

/// A built-in role's key, which no custom role may take.
pub fn reserved(key: &str) -> bool {
    of(key).is_some() || guest(key)
}

/// The guest starting role's key (K1.27): someone who sees only the spaces
/// they're added to.
pub fn guest(key: &str) -> bool {
    key == "guest"
}

/// The stored key of the owner tier, for SQL that counts owners.
pub const OWNER: &str = "owner";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_read_the_built_in_role() {
        assert!(stored_owner("owner") && !stored_owner("admin"));
        assert!(stored_high("admin") && stored_high("owner") && !stored_high("member"));
        assert!(reserved("worker") && reserved("guest") && !reserved("designer"));
        assert_eq!(OWNER, MembershipRole::Owner.key());
    }
}
