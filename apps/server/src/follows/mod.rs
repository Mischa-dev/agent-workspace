//! Following (A2.9): an account follows a thing or a whole space and hears
//! about it. `changes` hears the thing's own events (an update, a status
//! change); `everything` adds its discussion: comments, and the messages
//! of a followed conversation or space. Watching a task is following it
//! with `everything`.
//!
//! Following never widens what anyone sees: delivery and notifications
//! still check, for every event, that the follower may see it, so someone
//! who leaves the space hears nothing more from what they followed there.

pub mod ambient;
pub mod ops;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::AppError;

/// What can be followed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowKind {
    Space,
    Task,
    Page,
    Sheet,
    Record,
    Whiteboard,
    Conversation,
}

impl FollowKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::Task => "task",
            Self::Page => "page",
            Self::Sheet => "sheet",
            Self::Record => "record",
            Self::Whiteboard => "whiteboard",
            Self::Conversation => "conversation",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        [
            Self::Space,
            Self::Task,
            Self::Page,
            Self::Sheet,
            Self::Record,
            Self::Whiteboard,
            Self::Conversation,
        ]
        .into_iter()
        .find(|kind| kind.key() == key)
    }
}

/// How much of it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowLevel {
    /// Its own changes.
    #[default]
    Changes,
    /// Its changes and its discussion.
    Everything,
}

impl FollowLevel {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Everything => "everything",
        }
    }

    fn parse(key: &str) -> Self {
        if key == "everything" {
            Self::Everything
        } else {
            Self::Changes
        }
    }
}

/// One thing an account follows.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FollowView {
    pub kind: FollowKind,
    #[schemars(with = "String")]
    pub id: Uuid,
    pub level: FollowLevel,
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
}

/// Whether `account` may see what it would follow. The operation's target
/// already found it; this is the thing's own rule (a private page, a
/// space's own reading rule), the one delivery applies to each event.
pub async fn visible(
    connection: &mut PgConnection,
    kind: FollowKind,
    id: Uuid,
    account: Uuid,
) -> Result<bool, AppError> {
    let sql = match kind {
        FollowKind::Space => "SELECT space_readable_by($1, $2)",
        _ => "SELECT delivery_thing_visible($1, $2)",
    };
    Ok(sqlx::query_scalar::<_, Option<bool>>(sql)
        .bind(id)
        .bind(account)
        .fetch_one(&mut *connection)
        .await?
        .unwrap_or(false))
}

/// Follow (or change how much), or stop. Returns whether anything changed.
pub async fn set(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    account: Uuid,
    kind: FollowKind,
    id: Uuid,
    level: Option<FollowLevel>,
) -> Result<bool, AppError> {
    let changed = match level {
        Some(level) => sqlx::query(
            r#"
            INSERT INTO follows (
                workspace_id, space_id, account_id, target_kind, target_id, level,
                created_by_account_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $3)
            ON CONFLICT (account_id, target_kind, target_id)
            DO UPDATE SET level = EXCLUDED.level WHERE follows.level <> EXCLUDED.level
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(account)
        .bind(kind.key())
        .bind(id)
        .bind(level.key())
        .execute(&mut *connection)
        .await?
        .rows_affected(),
        None => sqlx::query(
            "DELETE FROM follows WHERE account_id = $1 AND target_kind = $2 AND target_id = $3",
        )
        .bind(account)
        .bind(kind.key())
        .bind(id)
        .execute(&mut *connection)
        .await?
        .rows_affected(),
    };
    Ok(changed > 0)
}

/// How `account` follows one thing, if it does.
pub async fn level_of(
    connection: &mut PgConnection,
    account: Uuid,
    kind: FollowKind,
    id: Uuid,
) -> Result<Option<FollowLevel>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT level FROM follows WHERE account_id = $1 AND target_kind = $2 AND target_id = $3",
    )
    .bind(account)
    .bind(kind.key())
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .map(|level| FollowLevel::parse(&level)))
}

/// The most follows one list returns.
pub const MAX_LISTED: i64 = 500;

/// What `account` follows in a workspace (or one space of it), newest
/// first, leaving out what it may no longer see.
pub async fn list(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    account: Uuid,
) -> Result<Vec<FollowView>, AppError> {
    let rows = sqlx::query_as::<_, (String, Uuid, String, Option<Uuid>)>(
        r#"
        SELECT target_kind, target_id, level, space_id
        FROM follows
        WHERE workspace_id = $1 AND account_id = $2
          AND ($3::uuid IS NULL OR space_id = $3)
          AND CASE target_kind
                  WHEN 'space' THEN COALESCE(space_readable_by(target_id, $2), false)
                  ELSE COALESCE(delivery_thing_visible(target_id, $2), false)
              END
        ORDER BY created_at DESC, target_id
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(account)
    .bind(space_id)
    .bind(MAX_LISTED)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(kind, id, level, space_id)| {
            Some(FollowView {
                kind: FollowKind::parse(&kind)?,
                id,
                level: FollowLevel::parse(&level),
                space_id,
            })
        })
        .collect())
}
