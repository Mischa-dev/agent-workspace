//! Who is in a space and who may read it (K4 §3.4), over the SQL helpers
//! `space_membership`, `space_readable_by` and `spaces_readable_by`. These
//! answer membership and readability only; what a role allows is K1's
//! `access`, which composes on top.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{SpaceKind, SpaceVisibility};
use crate::error::AppError;

/// A space as its readers see it.
#[derive(Clone, Debug)]
pub struct Space {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: SpaceKind,
    pub key: String,
    pub name: String,
    pub description: String,
    pub visibility: SpaceVisibility,
    pub owner_actor_id: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub version: i64,
}

/// An account's membership of a space.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceMembership {
    pub role: String,
    pub role_id: Option<Uuid>,
    /// True in the workspace space for a workspace member with no row of
    /// their own there.
    pub implicit: bool,
}

#[derive(sqlx::FromRow)]
struct SpaceRow {
    id: Uuid,
    workspace_id: Uuid,
    kind: String,
    key: String,
    name: String,
    description: String,
    visibility: String,
    owner_actor_id: Option<Uuid>,
    archived_at: Option<DateTime<Utc>>,
    version: i64,
}

impl SpaceRow {
    fn into_space(self) -> Option<Space> {
        Some(Space {
            id: self.id,
            workspace_id: self.workspace_id,
            kind: SpaceKind::from_stored(&self.kind)?,
            key: self.key,
            name: self.name,
            description: self.description,
            visibility: SpaceVisibility::from_stored(&self.visibility)?,
            owner_actor_id: self.owner_actor_id,
            archived_at: self.archived_at,
            version: self.version,
        })
    }
}

const SPACE_COLUMNS: &str = "space.id, space.workspace_id, space.kind, space.key, space.name, \
    space.description, space.visibility, space.owner_actor_id, space.archived_at, space.version";

/// The SQL that keeps only spaces `account_param` may read, for list
/// queries, so nobody rewrites the rule: `space_readable_by(p.id, $1)`.
pub fn readable_sql(space_column: &str, account_param: &str) -> String {
    format!("space_readable_by({space_column}, {account_param})")
}

/// The space if `account` may read it; `None` otherwise (callers answer
/// 404, never 403).
pub async fn get(
    connection: &mut PgConnection,
    account: Uuid,
    space: Uuid,
) -> Result<Option<Space>, AppError> {
    let row = sqlx::query_as::<_, SpaceRow>(&format!(
        "SELECT {SPACE_COLUMNS} FROM spaces space \
         WHERE space.id = $1 AND space_readable_by(space.id, $2)"
    ))
    .bind(space)
    .bind(account)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.and_then(SpaceRow::into_space))
}

/// The spaces of `kinds` in a workspace that `account` may read, by name.
pub async fn readable(
    connection: &mut PgConnection,
    account: Uuid,
    workspace: Uuid,
    kinds: &[SpaceKind],
) -> Result<Vec<Space>, AppError> {
    let kinds = kinds.iter().map(|kind| kind.stored()).collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, SpaceRow>(&format!(
        "SELECT {SPACE_COLUMNS} FROM spaces space \
         WHERE space.id IN (SELECT spaces_readable_by($1, $2)) AND space.kind = ANY($3) \
         ORDER BY lower(space.name), space.id"
    ))
    .bind(account)
    .bind(workspace)
    .bind(&kinds)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows.into_iter().filter_map(SpaceRow::into_space).collect())
}

/// `account`'s membership of `space`, explicit or implicit.
pub async fn membership(
    connection: &mut PgConnection,
    account: Uuid,
    space: Uuid,
) -> Result<Option<SpaceMembership>, AppError> {
    Ok(sqlx::query_as::<_, (String, Option<Uuid>, bool)>(
        "SELECT role, role_id, implicit FROM space_membership($1, $2) LIMIT 1",
    )
    .bind(space)
    .bind(account)
    .fetch_optional(&mut *connection)
    .await?
    .map(|(role, role_id, implicit)| SpaceMembership {
        role,
        role_id,
        implicit,
    }))
}

/// Whether `account` may read `space`.
pub async fn readable_by(
    connection: &mut PgConnection,
    account: Uuid,
    space: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT space_readable_by($1, $2)")
            .bind(space)
            .bind(account)
            .fetch_one(&mut *connection)
            .await?,
    )
}
