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
use chrono::{DateTime, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
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
    Reminders,
    Other,
}

impl Group {
    pub const ALL: [Self; 8] = [
        Self::Mentions,
        Self::Approvals,
        Self::Runs,
        Self::Tasks,
        Self::PullRequests,
        Self::Usage,
        Self::Reminders,
        Self::Other,
    ];

    pub fn of(kind: &str) -> Self {
        match kind {
            kind::MENTION | kind::COMMENT_MENTION | kind::CONVERSATION_MESSAGE => Self::Mentions,
            kind::REMINDER | kind::SCHEDULED_SEND_FAILED => Self::Reminders,
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
            Self::Reminders => "reminders",
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
            Self::Reminders => "Reminders and scheduled messages",
            Self::Other => "Everything else",
        }
    }

    /// Built-in default for the pair.
    pub fn default_for(self, channel: Channel) -> bool {
        match channel {
            Channel::InApp => true,
            Channel::Push => matches!(
                self,
                Self::Mentions | Self::Approvals | Self::Runs | Self::Usage | Self::Reminders
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
    /// Notifications paused by hand until then ("Pause for an hour"). Read
    /// with `paused_at`; a time in the past means not paused.
    pub paused_until: Option<DateTime<Utc>>,
}

impl QuietHours {
    /// When the pause ends, if one is on at `now`.
    pub fn paused_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.paused_until.filter(|until| *until > now)
    }

    /// When pushes may go out again, if `now` is inside a pause or quiet
    /// hours. A pause that ends inside quiet hours waits for them too.
    pub fn ends_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let paused = self.paused_at(now);
        self.scheduled_end_after(paused.unwrap_or(now)).or(paused)
    }

    fn scheduled_end_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
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
        // The end on the local clock, today or tomorrow: counted in wall
        // time, not minutes, so a daylight-saving change in between still
        // ends quiet hours at the end the person chose.
        let end = chrono::NaiveTime::from_hms_opt(
            u32::try_from(self.end.div_euclid(60)).ok()?,
            u32::try_from(self.end.rem_euclid(60)).ok()?,
            0,
        )?;
        let mut date = local.date_naive();
        if self.end <= minute {
            date = date.succ_opt()?;
        }
        let naive = date.and_time(end);
        zone.from_local_datetime(&naive)
            .earliest()
            // The end falls in a skipped hour: the first moment after it.
            .or_else(|| {
                zone.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                    .earliest()
            })
            .map(|ends| ends.with_timezone(&Utc))
    }
}

/// Whether a conversation's level still lets `notification` reach a phone
/// or a mailbox when it goes out: set to nothing after a mention arrived (or
/// off "every message"), a held push or a delayed email stays quiet.
pub async fn conversation_still_notifies(
    connection: &mut PgConnection,
    recipient: Uuid,
    notification: &crate::notifications::NotificationView,
) -> Result<bool, AppError> {
    use crate::notifications::kind;
    let kind = notification.kind.as_str();
    if notification.subject.kind != "conversation"
        || !matches!(kind, kind::MENTION | kind::CONVERSATION_MESSAGE)
    {
        return Ok(true);
    }
    let level = sqlx::query_scalar::<_, String>("SELECT conversation_notification_level($1, $2)")
        .bind(recipient)
        .bind(notification.subject.id)
        .fetch_one(&mut *connection)
        .await?;
    Ok(match kind {
        kind::CONVERSATION_MESSAGE => level == "all",
        _ => level != "nothing",
    })
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
            paused_until: effective
                .value::<i64>("notifications.paused_until")
                .filter(|seconds| *seconds > 0)
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
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
    /// Conversations set to every message or to nothing (W21).
    pub conversation_levels: Vec<ConversationLevelView>,
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
    let conversation_levels = sqlx::query_as::<_, ConversationLevelRow>(&format!(
        "{LEVEL_SQL} AND project.organization_id = $2 AND level.level <> 'mentions'
         ORDER BY project.name, conversation.name LIMIT 200"
    ))
    .bind(actor_id)
    .bind(organization_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(ConversationLevelView::from)
    .collect();
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
        conversation_levels,
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
// Per conversation (W21)

/// How much a person hears from one conversation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLevel {
    /// Every message, as a notification.
    All,
    /// Only mentions of you (the default).
    Mentions,
    /// Nothing, not even mentions.
    Nothing,
}

impl ConversationLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mentions => "mentions",
            Self::Nothing => "nothing",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "all" => Self::All,
            "nothing" => Self::Nothing,
            _ => Self::Mentions,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLevelView {
    pub conversation_id: Uuid,
    pub project_id: Uuid,
    pub place: String,
    /// What applies here.
    pub level: ConversationLevel,
    /// Set on this conversation itself (false: the default, or a thread
    /// following its channel).
    pub own: bool,
}

#[derive(FromRow)]
struct ConversationLevelRow {
    conversation_id: Uuid,
    project_id: Uuid,
    kind: String,
    name: String,
    level: Option<String>,
    effective: String,
}

impl From<ConversationLevelRow> for ConversationLevelView {
    fn from(row: ConversationLevelRow) -> Self {
        let place = match row.kind.as_str() {
            "dm" => "a DM".to_owned(),
            "channel" => format!("#{}", row.name),
            "group" if row.name.is_empty() => "a group".to_owned(),
            _ => row.name,
        };
        Self {
            conversation_id: row.conversation_id,
            project_id: row.project_id,
            place,
            level: ConversationLevel::parse(&row.effective),
            own: row.level.is_some(),
        }
    }
}

/// Conversations `$1` can see, with their own level when they set one.
const LEVEL_SQL: &str = r#"
    SELECT conversation.id AS conversation_id, conversation.project_id, conversation.kind,
           conversation.name, level.level,
           conversation_notification_level($1, conversation.id) AS effective
    FROM conversations conversation
    JOIN projects project ON project.id = conversation.project_id
    LEFT JOIN conversation_notification_levels level
      ON level.conversation_id = conversation.id AND level.actor_id = $1
    WHERE actor_can_access_project($1, NULL, conversation.project_id)
      AND conversation_visible_to(conversation.id, $1)
"#;

pub async fn conversation_level(
    pool: &PgPool,
    actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<ConversationLevelView, AppError> {
    sqlx::query_as::<_, ConversationLevelRow>(&format!("{LEVEL_SQL} AND conversation.id = $2"))
        .bind(actor_id)
        .bind(conversation_id)
        .fetch_optional(pool)
        .await?
        .map(ConversationLevelView::from)
        .ok_or(AppError::NotFound("conversation"))
}

/// Set a conversation the person can see to `level`, or (`None`) back to
/// what it would be by default.
pub async fn set_conversation_level(
    pool: &PgPool,
    actor_id: Uuid,
    conversation_id: Uuid,
    level: Option<ConversationLevel>,
) -> Result<ConversationLevelView, AppError> {
    let current = conversation_level(pool, actor_id, conversation_id).await?;
    match level {
        Some(level) => {
            sqlx::query(
                r#"
                INSERT INTO conversation_notification_levels (actor_id, conversation_id, project_id, level)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (actor_id, conversation_id)
                DO UPDATE SET level = EXCLUDED.level, updated_at = now()
                "#,
            )
            .bind(actor_id)
            .bind(conversation_id)
            .bind(current.project_id)
            .bind(level.as_str())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM conversation_notification_levels WHERE actor_id = $1 AND conversation_id = $2",
            )
            .bind(actor_id)
            .bind(conversation_id)
            .execute(pool)
            .await?;
        }
    }
    conversation_level(pool, actor_id, conversation_id).await
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
        .route(
            "/api/v1/conversations/{id}/notification-level",
            get(conversation_level_http).put(set_conversation_level_http),
        )
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

async fn conversation_level_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
) -> Result<Json<ConversationLevelView>, AppError> {
    Ok(Json(
        conversation_level(&state.pool, principal.actor_id, conversation_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLevelChange {
    /// all, mentions or nothing; null for the default.
    pub level: Option<ConversationLevel>,
}

async fn set_conversation_level_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(change): ApiJson<ConversationLevelChange>,
) -> Result<Json<ConversationLevelView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    Ok(Json(
        set_conversation_level(
            &state.pool,
            principal.actor_id,
            conversation_id,
            change.level,
        )
        .await?,
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
            paused_until: None,
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

    #[test]
    fn quiet_hours_end_on_the_local_clock_across_a_clock_change() {
        let quiet = QuietHours {
            enabled: true,
            start: 22 * 60,
            end: 7 * 60,
            time_zone: Some("Europe/Berlin".to_owned()),
            paused_until: None,
        };
        // Berlin leaves summer time on 2026-10-25 at 03:00. 23:30 CEST on
        // the 24th is 21:30 UTC; 07:00 CET on the 25th is 06:00 UTC.
        let late = Utc.with_ymd_and_hms(2026, 10, 24, 21, 30, 0).unwrap();
        assert_eq!(
            quiet.ends_after(late),
            Some(Utc.with_ymd_and_hms(2026, 10, 25, 6, 0, 0).unwrap())
        );
    }

    #[test]
    fn a_pause_holds_pushes_until_it_ends_and_then_waits_for_quiet_hours() {
        let noon = Utc.with_ymd_and_hms(2026, 10, 16, 10, 0, 0).unwrap();
        let in_an_hour = Utc.with_ymd_and_hms(2026, 10, 16, 11, 0, 0).unwrap();
        let paused = QuietHours {
            enabled: false,
            start: 22 * 60,
            end: 7 * 60,
            time_zone: None,
            paused_until: Some(in_an_hour),
        };
        assert_eq!(paused.paused_at(noon), Some(in_an_hour));
        assert_eq!(paused.ends_after(noon), Some(in_an_hour));
        // Over, it changes nothing.
        assert_eq!(paused.paused_at(in_an_hour), None);
        assert_eq!(paused.ends_after(in_an_hour), None);
        // A pause until 23:00 UTC ends inside quiet hours, so pushes wait for 07:00.
        let late_pause = QuietHours {
            enabled: true,
            paused_until: Some(Utc.with_ymd_and_hms(2026, 10, 16, 23, 0, 0).unwrap()),
            ..paused
        };
        assert_eq!(
            late_pause.ends_after(noon),
            Some(Utc.with_ymd_and_hms(2026, 10, 17, 7, 0, 0).unwrap())
        );
    }
}
