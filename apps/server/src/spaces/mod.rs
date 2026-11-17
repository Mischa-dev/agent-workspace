//! Spaces (K4): the places inside a workspace where work lives. A space is
//! a project, a team's space, someone's personal space, or the workspace's
//! own space. Stored in `projects` (seen through the `spaces` view) until
//! K4.22.
//!
//! - `kind`: `SpaceKind` and `SpaceVisibility`, read from storage in one
//!   place.
//! - `access`: who is in a space and who may read it (`get`, `readable`,
//!   `membership`, `readable_sql`), over the SQL helpers of K4.2.
//! - `workspace`: the workspace's own space (find, make, join).

pub mod access;
#[cfg(test)]
mod guards;
pub mod invite_links;
pub mod kind;
pub mod workspace;

pub use access::{Space, SpaceMembership, get, membership, readable, readable_by, readable_sql};
pub use kind::{SpaceKind, SpaceVisibility};
