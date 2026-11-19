//! Spaces (K4): the places inside a workspace where work lives. A space is
//! a project, a team's space, someone's personal space, or the workspace's
//! own space. Stored in `projects` (seen through the `spaces` view) until
//! K4.22.
//!
//! - `kind`: `SpaceKind` and `SpaceVisibility`, read from storage in one
//!   place.
//! - `access`: who is in a space and who may read it (`get`, `readable`,
//!   `membership`, `readable_sql`), over the SQL helpers of K4.2.
//! - `manage`: making and changing spaces (create, update, archive,
//!   restore), with their events; the one place a space row is made.
//! - `ops`: the space operations (K4.7) over `manage`.
//! - `members`: joining, leaving, adding and removing members (K4.8).
//! - `personal`: each account's personal space, made on first use (K4.9).
//! - `workspace`: the workspace's own space (find, make, join).

pub mod access;
#[cfg(test)]
mod guards;
pub mod invite_links;
pub mod kind;
pub mod manage;
pub mod members;
pub mod ops;
pub mod personal;
pub mod shape;
pub mod workspace;

/// Where DMs and groups live (K4.12): not a space but the reserved direct
/// scope, an `organizations` row of kind `direct` that no one belongs to
/// (`direct_scope_id()` in SQL). A DM's stored space is NULL; the API, the
/// web's keys and realtime frames name it by this id, its place.
pub const DIRECT: uuid::Uuid = uuid::Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_d1ec);

/// The place of something stored in `space` (none: a DM or group).
pub fn place(space: Option<uuid::Uuid>) -> uuid::Uuid {
    space.unwrap_or(DIRECT)
}

/// The stored space of a place: none for the direct scope.
pub fn space_of(place: uuid::Uuid) -> Option<uuid::Uuid> {
    (place != DIRECT).then_some(place)
}

pub use access::{Space, SpaceMembership, get, membership, readable, readable_by, readable_sql};
pub use kind::{SpaceKind, SpaceVisibility};
