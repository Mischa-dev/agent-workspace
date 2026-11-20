//! T1.43 Project status and update posts. A space carries a health (on
//! track, at risk, off track), a target date and a lead. An update is a
//! thread in one of its channels, made by the one thread-creation function
//! (`conversations::threads::create`), whose outcome is the health it set;
//! its words stay in its messages. The latest update shows on the space's
//! home, in its members' My work and in the initiatives its milestones are
//! part of, each read with the reader's own access to the thread.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// The healths a space can have: its stored key and how it reads.
pub const HEALTHS: [(&str, &str); 3] = [
    ("on_track", "On track"),
    ("at_risk", "At risk"),
    ("off_track", "Off track"),
];

/// A health named by its key or label, in any case: `at_risk`, "At risk".
pub fn parse_health(value: &str) -> Option<&'static str> {
    let word = value
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    HEALTHS
        .iter()
        .find(|(key, label)| key.replace('_', " ") == word || label.to_lowercase() == word)
        .map(|(key, _)| *key)
}

/// How a stored health reads.
pub fn health_label(key: &str) -> &'static str {
    HEALTHS
        .iter()
        .find(|(stored, _)| *stored == key)
        .map_or("", |(_, label)| *label)
}

/// The healths, for refusals and descriptions.
pub const HEALTH_MESSAGE: &str = "Use on_track, at_risk or off_track.";

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    pub name: String,
}

/// The update that last set a space's health.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LatestUpdate {
    /// The update's thread: read it with conversation_read.
    pub thread_id: String,
    /// The channel it is in.
    pub channel_id: String,
    pub name: String,
    /// The health it set, as it reads ("At risk").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    pub posted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Person>,
}

/// A space's status.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceStatus {
    pub space_id: String,
    /// on_track, at_risk or off_track; none until someone sets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// How the health reads ("At risk").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub target_date: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<Person>,
    /// The latest update you can read; none when there is none, or its
    /// channel is closed to you.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_update: Option<LatestUpdate>,
    /// Pass it back as expectedVersion to project_status_set; 0 before
    /// anything was set.
    pub version: i64,
    /// Whether you may post an update or change the status (said by
    /// project_status_get).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_update: Option<bool>,
}

impl SpaceStatus {
    fn unset(space_id: Uuid) -> Self {
        Self {
            space_id: space_id.to_string(),
            health: None,
            health_label: None,
            target_date: None,
            lead: None,
            latest_update: None,
            version: 0,
            can_update: None,
        }
    }
}

#[derive(sqlx::FromRow)]
struct StatusRow {
    space_id: Uuid,
    health: Option<String>,
    target_date: Option<NaiveDate>,
    lead_id: Option<Uuid>,
    lead_name: Option<String>,
    version: i64,
    thread_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    thread_name: Option<String>,
    thread_outcome: Option<String>,
    posted_at: Option<DateTime<Utc>>,
    author_id: Option<Uuid>,
    author_name: Option<String>,
}

/// The status of each of `spaces`, as `reader` sees it. The caller decides
/// which spaces the reader may see; the latest update is checked here,
/// thread by thread.
pub async fn statuses_of(
    connection: &mut PgConnection,
    reader: Uuid,
    spaces: &[Uuid],
) -> Result<HashMap<Uuid, SpaceStatus>, AppError> {
    if spaces.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, StatusRow>(
        r#"
        SELECT status.space_id, status.health, status.target_date,
               status.lead_actor_id AS lead_id, lead.display_name AS lead_name,
               status.version,
               thread.id AS thread_id, thread.parent_channel_id AS channel_id,
               thread.name AS thread_name, thread.thread_outcome,
               status.latest_update_at AS posted_at,
               status.latest_update_by_actor_id AS author_id, author.display_name AS author_name
        FROM space_statuses status
        LEFT JOIN project_memberships lead
          ON lead.project_id = status.space_id AND lead.actor_id = status.lead_actor_id
        LEFT JOIN conversations thread
          ON thread.id = status.latest_update_id
         AND thread.archived_at IS NULL
         AND conversation_visible_to(thread.id, $2)
        LEFT JOIN project_memberships author
          ON author.project_id = status.space_id
         AND author.actor_id = status.latest_update_by_actor_id
        WHERE status.space_id = ANY($1)
        "#,
    )
    .bind(spaces)
    .bind(reader)
    .fetch_all(&mut *connection)
    .await?;
    let mut statuses: HashMap<Uuid, SpaceStatus> = spaces
        .iter()
        .map(|space| (*space, SpaceStatus::unset(*space)))
        .collect();
    for row in rows {
        let latest_update = match (row.thread_id, row.channel_id, row.posted_at) {
            (Some(thread), Some(channel), Some(posted_at)) => Some(LatestUpdate {
                thread_id: thread.to_string(),
                channel_id: channel.to_string(),
                name: row.thread_name.unwrap_or_default(),
                health: row.thread_outcome,
                posted_at: posted_at.to_rfc3339(),
                author: row.author_id.zip(row.author_name).map(|(id, name)| Person {
                    id: id.to_string(),
                    name,
                }),
            }),
            _ => None,
        };
        statuses.insert(
            row.space_id,
            SpaceStatus {
                space_id: row.space_id.to_string(),
                health_label: row
                    .health
                    .as_deref()
                    .map(|key| health_label(key).to_owned()),
                health: row.health,
                target_date: row.target_date,
                lead: row.lead_id.zip(row.lead_name).map(|(id, name)| Person {
                    id: id.to_string(),
                    name,
                }),
                latest_update,
                version: row.version,
                can_update: None,
            },
        );
    }
    Ok(statuses)
}

/// One space's status, as `reader` sees it.
pub async fn status_of(
    connection: &mut PgConnection,
    reader: Uuid,
    space: Uuid,
) -> Result<SpaceStatus, AppError> {
    Ok(statuses_of(connection, reader, &[space])
        .await?
        .remove(&space)
        .unwrap_or_else(|| SpaceStatus::unset(space)))
}

/// What changes. `None` leaves a value; `Some(None)` clears it.
#[derive(Debug, Default)]
pub struct StatusChange {
    pub health: Option<Option<&'static str>>,
    pub target_date: Option<Option<NaiveDate>>,
    pub lead: Option<Option<Uuid>>,
}

impl StatusChange {
    pub fn is_empty(&self) -> bool {
        self.health.is_none() && self.target_date.is_none() && self.lead.is_none()
    }
}

/// Why a change was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum StatusRefusal {
    VersionConflict { current_version: i64 },
}

/// Applies `change` to the space's status (making it if it isn't there)
/// and returns its new version.
pub async fn set(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    space: Uuid,
    actor: Uuid,
    change: &StatusChange,
    expected_version: Option<i64>,
) -> Result<Result<i64, StatusRefusal>, AppError> {
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM space_statuses WHERE space_id = $1 FOR UPDATE",
    )
    .bind(space)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(0);
    if expected_version.is_some_and(|expected| expected != current) {
        return Ok(Err(StatusRefusal::VersionConflict {
            current_version: current,
        }));
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO space_statuses (space_id, workspace_id, health, target_date, lead_actor_id,
                                    updated_by_actor_id)
        VALUES ($1, $2, $4, $6, $8, $9)
        ON CONFLICT (space_id) DO UPDATE SET
            health = CASE WHEN $3 THEN $4 ELSE space_statuses.health END,
            target_date = CASE WHEN $5 THEN $6 ELSE space_statuses.target_date END,
            lead_actor_id = CASE WHEN $7 THEN $8 ELSE space_statuses.lead_actor_id END,
            updated_by_actor_id = $9,
            version = space_statuses.version + 1,
            updated_at = now()
        RETURNING version
        "#,
    )
    .bind(space)
    .bind(workspace)
    .bind(change.health.is_some())
    .bind(change.health.flatten())
    .bind(change.target_date.is_some())
    .bind(change.target_date.flatten())
    .bind(change.lead.is_some())
    .bind(change.lead.flatten())
    .bind(actor)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Ok(version))
}

/// Records `thread` as the space's latest update, setting its health.
pub async fn record_update(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    space: Uuid,
    actor: Uuid,
    thread: Uuid,
    health: &'static str,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO space_statuses (space_id, workspace_id, health, latest_update_id,
                                    latest_update_at, latest_update_by_actor_id,
                                    updated_by_actor_id)
        VALUES ($1, $2, $3, $4, now(), $5, $5)
        ON CONFLICT (space_id) DO UPDATE SET
            health = $3,
            latest_update_id = $4,
            latest_update_at = now(),
            latest_update_by_actor_id = $5,
            updated_by_actor_id = $5,
            version = space_statuses.version + 1,
            updated_at = now()
        RETURNING version
        "#,
    )
    .bind(space)
    .bind(workspace)
    .bind(health)
    .bind(thread)
    .bind(actor)
    .fetch_one(&mut **transaction)
    .await?)
}

/// The channel an update goes in when none is named: the space's oldest
/// open channel that every reader of the space sees, else the oldest one
/// `actor` sees.
pub async fn default_channel(
    connection: &mut PgConnection,
    space: Uuid,
    actor: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT channel.id
        FROM conversations channel
        WHERE channel.project_id = $1 AND channel.kind = 'channel'
          AND channel.archived_at IS NULL
          AND conversation_visible_to(channel.id, $2)
        ORDER BY channel.visibility = 'members', channel.created_at, channel.id
        LIMIT 1
        "#,
    )
    .bind(space)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A lead named as "me", "none", an account ID or a handle in the space.
/// `Ok(None)` clears it; `Err(())` when nobody in the space matches.
pub async fn resolve_lead(
    connection: &mut PgConnection,
    space: Uuid,
    me: Uuid,
    value: &str,
) -> Result<Result<Option<Uuid>, ()>, AppError> {
    let value = value.trim();
    match value.to_lowercase().as_str() {
        "" | "none" => return Ok(Ok(None)),
        "me" => return Ok(Ok(Some(me))),
        _ => {}
    }
    let found = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.actor_id
        FROM project_memberships membership
        WHERE membership.project_id = $1 AND membership.enabled
          AND (membership.actor_id::text = $2 OR lower(membership.handle) = lower($3))
        LIMIT 1
        "#,
    )
    .bind(space)
    .bind(value)
    .bind(value.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?;
    Ok(found.map(Some).ok_or(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healths_read_by_key_or_label() {
        assert_eq!(parse_health("at_risk"), Some("at_risk"));
        assert_eq!(parse_health("At risk"), Some("at_risk"));
        assert_eq!(parse_health(" off-track "), Some("off_track"));
        assert_eq!(parse_health("ON TRACK"), Some("on_track"));
        assert_eq!(parse_health("fine"), None);
        assert_eq!(health_label("off_track"), "Off track");
    }
}
