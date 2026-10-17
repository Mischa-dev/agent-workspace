//! N4: notification settings. Per group of kinds and channel (in app,
//! push, email), mutes per project and group, and quiet hours and digest
//! frequency, which are ordinary inheriting settings (`notifications.*` in
//! the S2 registry), so a workspace can set its default and each person
//! their own.
//!
//! Notifications are decided when written: a muted project's group writes
//! nothing; with in-app off but push on, the row is written straight to the
//! archive so the push has something to open. Push also checks quiet hours
//! when it sends. Email sends once the workspace can send mail and the
//! person has confirmed an address (`notification_email`).

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, put},
};
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiQuery, AppError},
    notifications::kind,
    settings::{self, SettingsContext},
    usage::alerts,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    InApp,
    Push,
    Email,
}

impl Channel {
    pub const ALL: [Self; 3] = [Self::InApp, Self::Push, Self::Email];

    fn as_str(self) -> &'static str {
        match self {
            Self::InApp => "in_app",
            Self::Push => "push",
            Self::Email => "email",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == value)
    }
}

/// Groups of kinds people choose channels for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Mentions,
    Approvals,
    Runs,
    Tasks,
    PullRequests,
    Usage,
    Other,
}

impl Group {
    pub const ALL: [Self; 7] = [
        Self::Mentions,
        Self::Approvals,
        Self::Runs,
        Self::Tasks,
        Self::PullRequests,
        Self::Usage,
        Self::Other,
    ];

    pub fn of(kind: &str) -> Self {
        match kind {
            kind::MENTION | kind::COMMENT_MENTION => Self::Mentions,
            kind::APPROVAL_REQUESTED | kind::RUN_WAITING => Self::Approvals,
            kind::RUN_COMPLETED | kind::RUN_FAILED => Self::Runs,
            kind::ASSIGNED | kind::TASK_CHANGED => Self::Tasks,
            kind::CHECKS_FAILED | kind::REVIEW_REQUESTED => Self::PullRequests,
            other if alerts::kind::ALL.contains(&other) => Self::Usage,
            _ => Self::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mentions => "mentions",
            Self::Approvals => "approvals",
            Self::Runs => "runs",
            Self::Tasks => "tasks",
            Self::PullRequests => "pull_requests",
            Self::Usage => "usage",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.as_str() == value)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mentions => "Mentions",
            Self::Approvals => "Approvals and questions",
            Self::Runs => "Runs finishing or stopping",
            Self::Tasks => "Tasks assigned or changed",
            Self::PullRequests => "Pull request checks and reviews",
            Self::Usage => "Capacity alerts",
            Self::Other => "Everything else",
        }
    }

    /// Built-in default for the pair.
    pub fn default_for(self, channel: Channel) -> bool {
        match channel {
            Channel::InApp => true,
            Channel::Push => matches!(
                self,
                Self::Mentions | Self::Approvals | Self::Runs | Self::Usage
            ),
            Channel::Email => false,
        }
    }
}

/// How one notification reaches its recipient.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Decision {
    pub in_app: bool,
    pub push: bool,
    pub email: bool,
}

impl Decision {
    pub fn nothing(self) -> bool {
        !self.in_app && !self.push && !self.email
    }
}

/// What `recipient` wants for a `kind` notification in `project_id`.
pub async fn decide(
    connection: &mut PgConnection,
    recipient: Uuid,
    project_id: Uuid,
    kind: &str,
) -> Result<Decision, AppError> {
    let group = Group::of(kind);
    let rows = sqlx::query_as::<_, (String, bool)>(
        r#"
        SELECT preference.channel, preference.enabled
        FROM notification_channel_preferences preference
        JOIN projects project ON project.organization_id = preference.organization_id
        WHERE project.id = $1 AND preference.actor_id = $2 AND preference.kind_group = $3
        "#,
    )
    .bind(project_id)
    .bind(recipient)
    .bind(group.as_str())
    .fetch_all(&mut *connection)
    .await?;
    let muted = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM notification_project_mutes
            WHERE actor_id = $1 AND project_id = $2 AND kind_group IN ($3, 'all')
        )
        "#,
    )
    .bind(recipient)
    .bind(project_id)
    .bind(group.as_str())
    .fetch_one(&mut *connection)
    .await?;
    if muted {
        return Ok(Decision::default());
    }
    let enabled = |channel: Channel| {
        rows.iter()
            .find(|(stored, _)| stored == channel.as_str())
            .map_or_else(|| group.default_for(channel), |(_, enabled)| *enabled)
    };
    Ok(Decision {
        in_app: enabled(Channel::InApp),
        push: enabled(Channel::Push),
        email: enabled(Channel::Email),
    })
}

// ---------------------------------------------------------------------------
// Quiet hours

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuietHours {
    pub enabled: bool,
    /// Minutes after midnight, local.
    pub start: i64,
    pub end: i64,
    pub time_zone: Option<String>,
}

impl QuietHours {
    /// When quiet hours end, if `now` is inside them.
    pub fn ends_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if !self.enabled || self.start == self.end {
            return None;
        }
        let zone = self
            .time_zone
            .as_deref()
            .and_then(crate::usage::schedule::parse_time_zone)
            .unwrap_or(chrono_tz::UTC);
        let local = now.with_timezone(&zone);
        let minute = i64::from(local.hour() * 60 + local.minute());
        let inside = if self.start < self.end {
            (self.start..self.end).contains(&minute)
        } else {
            minute >= self.start || minute < self.end
        };
        if !inside {
            return None;
        }
        let mut wait = self.end - minute;
        if wait <= 0 {
            wait += 24 * 60;
        }
        Some(
            now + chrono::Duration::minutes(wait)
                - chrono::Duration::seconds(i64::from(local.second())),
        )
    }
}

pub async fn quiet_hours(
    connection: &mut PgConnection,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<(QuietHours, String), AppError> {
    let effective = settings::resolve(
        connection,
        SettingsContext {
            organization_id,
            project_id: None,
            actor_id: Some(actor_id),
        },
        &BTreeMap::new(),
    )
    .await?;
    Ok((
        QuietHours {
            enabled: effective.bool("notifications.quiet_hours").unwrap_or(false),
            start: effective
                .value::<i64>("notifications.quiet_hours_start")
                .unwrap_or(22 * 60),
            end: effective
                .value::<i64>("notifications.quiet_hours_end")
                .unwrap_or(7 * 60),
            time_zone: effective.string("notifications.time_zone"),
        },
        effective
            .string("notifications.digest")
            .unwrap_or_else(|| "off".to_owned()),
    ))
}

// ---------------------------------------------------------------------------
// Reading and changing

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub group: Group,
    pub label: &'static str,
    pub in_app: bool,
    pub push: bool,
    pub email: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MuteView {
    pub project_id: Uuid,
    pub project_name: String,
    pub group: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettingsView {
    pub organization_id: Uuid,
    pub groups: Vec<GroupView>,
    pub mutes: Vec<MuteView>,
    pub quiet_hours: QuietHours,
    /// off, daily or weekly; for email.
    pub digest: String,
    /// Whether email reaches this person here: the workspace sends mail
    /// and they confirmed an address.
    pub email_available: bool,
    /// Whether the workspace sends mail, so the page can say what's missing.
    pub mail_configured: bool,
    /// The person's confirmed address, if any.
    pub email_address: Option<String>,
}

/// The workspace notification settings apply to: `wanted` if the person is
/// an enabled member there, else the one they chose as their default (as
/// their direct messages do).
pub async fn home_workspace(
    pool: &PgPool,
    actor_id: Uuid,
    wanted: Option<Uuid>,
) -> Result<Uuid, AppError> {
    let Some(wanted) = wanted else {
        return crate::workspace::default_workspace(pool, actor_id)
            .await?
            .map(|(organization_id, _)| organization_id)
            .ok_or(AppError::NotFound("organization"));
    };
    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT organization_id FROM organization_memberships
        WHERE actor_id = $1 AND enabled AND organization_id = $2
        "#,
    )
    .bind(actor_id)
    .bind(wanted)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("organization"))
}

pub async fn read(
    pool: &PgPool,
    actor_id: Uuid,
    organization_id: Uuid,
) -> Result<NotificationSettingsView, AppError> {
    let rows = sqlx::query_as::<_, (String, String, bool)>(
        r#"
        SELECT kind_group, channel, enabled FROM notification_channel_preferences
        WHERE organization_id = $1 AND actor_id = $2
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_all(pool)
    .await?;
    let enabled = |group: Group, channel: Channel| {
        rows.iter()
            .find(|(stored_group, stored_channel, _)| {
                stored_group == group.as_str() && stored_channel == channel.as_str()
            })
            .map_or_else(|| group.default_for(channel), |(_, _, enabled)| *enabled)
    };
    let groups = Group::ALL
        .into_iter()
        .map(|group| GroupView {
            group,
            label: group.label(),
            in_app: enabled(group, Channel::InApp),
            push: enabled(group, Channel::Push),
            email: enabled(group, Channel::Email),
        })
        .collect();
    // Only projects the person can still open.
    let mutes = sqlx::query_as::<_, MuteView>(
        r#"
        SELECT mute.project_id,
               CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END
                   AS project_name,
               mute.kind_group AS "group"
        FROM notification_project_mutes mute
        JOIN projects project ON project.id = mute.project_id
        WHERE mute.actor_id = $1 AND project.organization_id = $2
          AND actor_can_access_project($1, NULL, project.id)
        ORDER BY project.name, mute.kind_group
        "#,
    )
    .bind(actor_id)
    .bind(organization_id)
    .fetch_all(pool)
    .await?;
    let mut connection = pool.acquire().await?;
    let (quiet_hours, digest) = quiet_hours(&mut connection, organization_id, actor_id).await?;
    let mail_configured = crate::mail::configured(&mut *connection, organization_id).await?;
    let email_address = crate::mail::verified_address_for_actor(&mut *connection, actor_id).await?;
    Ok(NotificationSettingsView {
        organization_id,
        groups,
        mutes,
        quiet_hours,
        digest,
        email_available: mail_configured && email_address.is_some(),
        mail_configured,
        email_address,
    })
}

pub async fn set_channel(
    pool: &PgPool,
    actor_id: Uuid,
    organization_id: Uuid,
    group: Group,
    channel: Channel,
    enabled: bool,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO notification_channel_preferences (
            organization_id, actor_id, kind_group, channel, enabled
        ) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (organization_id, actor_id, kind_group, channel)
        DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(group.as_str())
    .bind(channel.as_str())
    .bind(enabled)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mute or unmute `group` (or `None` for every kind) in a project the
/// person can open.
pub async fn set_mute(
    pool: &PgPool,
    actor_id: Uuid,
    project_id: Uuid,
    group: Option<Group>,
    muted: bool,
) -> Result<(), AppError> {
    let visible = sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
        .bind(actor_id)
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    if !visible {
        return Err(AppError::NotFound("project"));
    }
    let group = group.map_or("all", Group::as_str);
    if muted {
        sqlx::query(
            r#"
            INSERT INTO notification_project_mutes (project_id, actor_id, kind_group)
            VALUES ($1, $2, $3) ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(actor_id)
        .bind(group)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM notification_project_mutes WHERE project_id = $1 AND actor_id = $2 AND kind_group = $3",
        )
        .bind(project_id)
        .bind(actor_id)
        .bind(group)
        .execute(pool)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/notification-settings", get(read_http))
        .route(
            "/api/v1/notification-settings/channels",
            put(set_channel_http),
        )
        .route("/api/v1/notification-settings/mutes", put(set_mute_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceQuery {
    organization_id: Option<Uuid>,
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<WorkspaceQuery>,
) -> Result<Json<NotificationSettingsView>, AppError> {
    let organization_id =
        home_workspace(&state.pool, principal.actor_id, query.organization_id).await?;
    Ok(Json(
        read(&state.pool, principal.actor_id, organization_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelChange {
    pub organization_id: Option<Uuid>,
    pub group: String,
    pub channel: String,
    pub enabled: bool,
}

async fn set_channel_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(change): ApiJson<ChannelChange>,
) -> Result<Json<NotificationSettingsView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let organization_id =
        home_workspace(&state.pool, principal.actor_id, change.organization_id).await?;
    let group = Group::parse(&change.group).ok_or(AppError::Validation("group"))?;
    let channel = Channel::parse(&change.channel).ok_or(AppError::Validation("channel"))?;
    set_channel(
        &state.pool,
        principal.actor_id,
        organization_id,
        group,
        channel,
        change.enabled,
    )
    .await?;
    Ok(Json(
        read(&state.pool, principal.actor_id, organization_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MuteChange {
    pub project_id: Uuid,
    /// A group, or `all`.
    pub group: String,
    pub muted: bool,
}

async fn set_mute_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(change): ApiJson<MuteChange>,
) -> Result<Json<NotificationSettingsView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let group = match change.group.as_str() {
        "all" => None,
        other => Some(Group::parse(other).ok_or(AppError::Validation("group"))?),
    };
    set_mute(
        &state.pool,
        principal.actor_id,
        change.project_id,
        group,
        change.muted,
    )
    .await?;
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(change.project_id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(
        read(&state.pool, principal.actor_id, organization_id).await?,
    ))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{Channel, Group, QuietHours};
    use crate::notifications::kind;

    #[test]
    fn every_kind_has_a_group_and_defaults_keep_the_inbox() {
        assert_eq!(Group::of(kind::MENTION), Group::Mentions);
        assert_eq!(Group::of(kind::RUN_FAILED), Group::Runs);
        assert_eq!(Group::of("capacity_reset_soon"), Group::Usage);
        assert_eq!(Group::of("something_new"), Group::Other);
        for group in Group::ALL {
            assert!(group.default_for(Channel::InApp));
            assert!(!group.default_for(Channel::Email));
            assert_eq!(Group::parse(group.as_str()), Some(group));
        }
    }

    #[test]
    fn quiet_hours_wrap_midnight_in_the_persons_zone() {
        let quiet = QuietHours {
            enabled: true,
            start: 22 * 60,
            end: 7 * 60,
            time_zone: Some("Europe/Berlin".to_owned()),
        };
        // 23:30 in Berlin (CEST, UTC+2) is 21:30 UTC.
        let late = Utc.with_ymd_and_hms(2026, 10, 16, 21, 30, 0).unwrap();
        assert_eq!(
            quiet.ends_after(late),
            Some(Utc.with_ymd_and_hms(2026, 10, 17, 5, 0, 0).unwrap())
        );
        let noon = Utc.with_ymd_and_hms(2026, 10, 16, 10, 0, 0).unwrap();
        assert_eq!(quiet.ends_after(noon), None);
        let off = QuietHours {
            enabled: false,
            ..quiet
        };
        assert_eq!(off.ends_after(late), None);
    }
}
