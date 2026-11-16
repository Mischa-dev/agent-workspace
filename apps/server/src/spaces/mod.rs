//! Spaces (K4): the places inside a workspace where work lives. A space is
//! a project, a team's space, someone's personal space, or the workspace's
//! own space. Stored in `projects` (seen through the `spaces` view) until
//! K4.22.
//!
//! - `kind`: `SpaceKind` and `SpaceVisibility`, read from storage in one
//!   place.
//! - `access`: who is in a space and who may read it (`get`, `readable`,
//!   `membership`, `readable_sql`), over the SQL helpers of K4.2.
//! - The workspace's own space: `workspace_space` (find, or make).

pub mod access;
#[cfg(test)]
mod guards;
pub mod kind;

pub use access::{Space, SpaceMembership, get, membership, readable, readable_by, readable_sql};
pub use kind::{SpaceKind, SpaceVisibility};

use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::AppError;

/// The workspace's own space, if it has been made.
pub async fn workspace_space_id(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?)
}
