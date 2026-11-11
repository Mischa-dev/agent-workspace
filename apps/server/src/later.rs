//! W21: Later. Save a message or a task to come back to, or write yourself a
//! reminder, optionally with a time to be reminded at.
//!
//! Saved items belong to one person (or one agent) and are read only by
//! them. Every read rechecks their current access (principle 9): a message
//! from a conversation they've left, or a task in a project they've lost,
//! drops out of the list without being deleted, and comes back if access
//! does. A due reminder is a job on the queue (at least once, keyed by the
//! item and its time) that rechecks the same access when it fires, then
//! delivers a notification through the inbox, push and email paths, which
//! keep honouring quiet hours and channel settings. An agent's reminder is
//! an external event on its connections instead, since agents have no
//! inbox of their own.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, patch},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::TimelineEntryView,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    jobs::{self, JobError, JobRegistry, NewJob},
    notifications::{self, NewNotification, kind},
};

pub const REMIND_KIND: &str = "later.remind";
/// How far ahead a reminder may be set.
const MAX_AHEAD_DAYS: i64 = 366;
const MAX_NOTE_CHARACTERS: usize = 1_000;
/// Open items one person may keep, so the list stays a list.
const MAX_OPEN_ITEMS: i64 = 500;
const LIST_LIMIT: i64 = 200;
const PREVIEW_CHARACTERS: i32 = 280;
/// How long Snooze on a reminder's notification puts it off.
pub const SNOOZE_MINUTES: i64 = 60;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        REMIND_KIND,
        std::time::Duration::from_secs(60),
        |context| async move {
            let job: RemindJob = context.job.payload_as()?;
            remind(&context.state, &job).await
        },
    );
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedItemView {
    pub id: Uuid,
    /// message, work_item or reminder
    pub kind: String,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<String>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_at: Option<String>,
    pub created_at: String,
    pub project_id: Uuid,
    pub project_name: String,
    /// Where it opens in the web app.
    pub href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<SavedMessageView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_item: Option<SavedWorkItemView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedMessageView {
    pub id: Uuid,
    pub conversation_id: Uuid,
    /// "#general", "a DM", a group's or thread's name.
    pub place: String,
    pub author_name: String,
    /// Up to 280 characters, read live: empty once the message is removed.
    pub preview: String,
    pub removed: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedWorkItemView {
    pub id: Uuid,
    pub key: String,
    pub title: String,
    pub status_category: String,
}

#[derive(FromRow)]
struct SavedRow {
    id: Uuid,
    kind: String,
    note: String,
    remind_at: Option<DateTime<Utc>>,
    done_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    project_id: Uuid,
    project_name: String,
    direct_space: bool,
    message_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    conversation_kind: Option<String>,
    conversation_name: Option<String>,
    parent_conversation_id: Option<Uuid>,
    author_name: Option<String>,
    message_preview: Option<String>,
    message_removed: Option<bool>,
    message_created_at: Option<DateTime<Utc>>,
    work_item_id: Option<Uuid>,
    work_item_key: Option<String>,
    work_item_title: Option<String>,
    work_item_status: Option<String>,
}

pub(crate) fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The rows `$1` may still see. Callers add conditions after it.
fn select_sql(extra: &str) -> String {
    // A saved task only while their role there still lets them see tasks;
    // an app's agent limit is applied to the rows (`hidden_by_limit`).
    let sees_task = crate::roles::role_allows_sql("$1", "item.project_id", "tasks.view");
    format!(
        r#"
        SELECT item.id, item.kind, item.note, item.remind_at, item.done_at, item.created_at,
               item.project_id,
               CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END
                   AS project_name,
               project.kind = 'direct' AS direct_space,
               message.id AS message_id, message.conversation_id,
               conversation.kind AS conversation_kind, conversation.name AS conversation_name,
               conversation.parent_channel_id AS parent_conversation_id,
               author.display_name AS author_name,
               CASE WHEN message.removed_at IS NULL
                    THEN left(message.plain_text, {PREVIEW_CHARACTERS}) END AS message_preview,
               message.removed_at IS NOT NULL AS message_removed,
               message.created_at AS message_created_at,
               work.id AS work_item_id,
               project.project_key || '-' || work.number::text AS work_item_key,
               work.title AS work_item_title,
               status.semantic_category AS work_item_status
        FROM saved_items item
        JOIN projects project ON project.id = item.project_id
        LEFT JOIN messages message ON message.id = item.message_id
        LEFT JOIN conversations conversation ON conversation.id = message.conversation_id
        LEFT JOIN project_memberships author
          ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
        LEFT JOIN work_items work ON work.id = item.work_item_id
        LEFT JOIN work_item_statuses status
          ON status.project_id = work.project_id AND status.id = work.status_id
        WHERE item.actor_id = $1
          AND actor_can_access_project($1, item.external_agent_connection_id, item.project_id)
          AND (item.message_id IS NULL OR conversation_visible_to(message.conversation_id, $1))
          AND (item.work_item_id IS NULL OR {sees_task})
          {extra}
        "#
    )
}

fn place(row: &SavedRow) -> String {
    match (
        row.conversation_kind.as_deref(),
        row.conversation_name.as_deref(),
    ) {
        (Some("dm"), _) => "a DM".to_owned(),
        (Some("group"), Some(name)) if !name.is_empty() => name.to_owned(),
        (Some("group"), _) => "a group".to_owned(),
        (Some("channel"), Some(name)) => format!("#{name}"),
        (_, Some(name)) if !name.is_empty() => name.to_owned(),
        _ => "a conversation".to_owned(),
    }
}

/// Where a message opens: its conversation, scrolled to it.
pub(crate) fn message_href(
    project_id: Uuid,
    direct_space: bool,
    conversation_id: Uuid,
    conversation_kind: Option<&str>,
    parent_conversation_id: Option<Uuid>,
    message_id: Uuid,
) -> String {
    if direct_space {
        // A thread in a DM or group opens in its parent's page.
        let id = parent_conversation_id.unwrap_or(conversation_id);
        return format!("/messages/{id}#message-{message_id}");
    }
    let segment = if conversation_kind == Some("thread") {
        "threads"
    } else {
        "channels"
    };
    format!("/projects/{project_id}/{segment}/{conversation_id}#message-{message_id}")
}

fn view(row: SavedRow) -> SavedItemView {
    let message = match (row.message_id, row.conversation_id) {
        (Some(id), Some(conversation_id)) => Some(SavedMessageView {
            id,
            conversation_id,
            place: place(&row),
            author_name: row
                .author_name
                .clone()
                .unwrap_or_else(|| "Former member".to_owned()),
            preview: row.message_preview.clone().unwrap_or_default(),
            removed: row.message_removed.unwrap_or(false),
            created_at: row.message_created_at.map(timestamp).unwrap_or_default(),
        }),
        _ => None,
    };
    let work_item = match (row.work_item_id, &row.work_item_key) {
        (Some(id), Some(key)) => Some(SavedWorkItemView {
            id,
            key: key.clone(),
            title: row.work_item_title.clone().unwrap_or_default(),
            status_category: row.work_item_status.clone().unwrap_or_default(),
        }),
        _ => None,
    };
    let href = match (&message, &work_item) {
        (Some(message), _) => message_href(
            row.project_id,
            row.direct_space,
            message.conversation_id,
            row.conversation_kind.as_deref(),
            row.parent_conversation_id,
            message.id,
        ),
        (None, Some(item)) => format!("/projects/{}/work-items/{}", row.project_id, item.id),
        (None, None) => "/later".to_owned(),
    };
    SavedItemView {
        id: row.id,
        kind: row.kind,
        note: row.note,
        remind_at: row.remind_at.map(timestamp),
        done: row.done_at.is_some(),
        done_at: row.done_at.map(timestamp),
        created_at: timestamp(row.created_at),
        project_id: row.project_id,
        project_name: row.project_name,
        href,
        message,
        work_item,
    }
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListView {
    /// Not done: reminders due soonest first, then the newest saves.
    #[default]
    Open,
    Done,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaterPage {
    pub items: Vec<SavedItemView>,
    /// Open items you can still see, everywhere (or within the projects).
    pub open_count: i64,
}

/// `viewer`'s saved items they can still see, within `within` when given.
pub async fn list(
    pool: &PgPool,
    viewer: Uuid,
    view_: ListView,
    within: Option<&[Uuid]>,
) -> Result<LaterPage, AppError> {
    let extra = match view_ {
        ListView::Open => {
            "AND item.done_at IS NULL AND ($2::uuid[] IS NULL OR item.project_id = ANY($2))
             ORDER BY item.remind_at ASC NULLS LAST, item.created_at DESC, item.id DESC LIMIT $3"
        }
        ListView::Done => {
            "AND item.done_at IS NOT NULL AND ($2::uuid[] IS NULL OR item.project_id = ANY($2))
             ORDER BY item.done_at DESC, item.id DESC LIMIT $3"
        }
    };
    let rows = sqlx::query_as::<_, SavedRow>(&select_sql(extra))
        .bind(viewer)
        .bind(within)
        .bind(LIST_LIMIT)
        .fetch_all(pool)
        .await?;
    let open_count = open_count(pool, viewer, within).await?;
    Ok(LaterPage {
        items: rows
            .into_iter()
            .filter(|row| !hidden_by_limit(viewer, row))
            .map(view)
            .collect(),
        open_count,
    })
}

/// A saved task an app acting for `viewer` may not see: its agent limit
/// leaves out "see tasks" there.
fn hidden_by_limit(viewer: Uuid, row: &SavedRow) -> bool {
    row.work_item_id.is_some()
        && crate::policy::agent_limit_hides(
            viewer,
            row.project_id,
            crate::policy::Permission::TasksView,
        )
}

pub async fn open_count(
    pool: &PgPool,
    viewer: Uuid,
    within: Option<&[Uuid]>,
) -> Result<i64, AppError> {
    // $3: while an app acts for them, the projects its limit lets see tasks.
    let sees_task = crate::roles::role_allows_sql("$1", "item.project_id", "tasks.view");
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        r#"
        SELECT count(*) FROM saved_items item
        LEFT JOIN messages message ON message.id = item.message_id
        WHERE item.actor_id = $1 AND item.done_at IS NULL
          AND ($2::uuid[] IS NULL OR item.project_id = ANY($2))
          AND actor_can_access_project($1, item.external_agent_connection_id, item.project_id)
          AND (item.message_id IS NULL OR conversation_visible_to(message.conversation_id, $1))
          AND (item.work_item_id IS NULL OR ({sees_task}
               AND ($3::uuid[] IS NULL OR item.project_id = ANY($3))))
        "#
    ))
    .bind(viewer)
    .bind(within)
    .bind(crate::policy::agent_limit_projects_allowing(
        viewer,
        crate::policy::Permission::TasksView,
    ))
    .fetch_one(pool)
    .await?)
}

/// One of `viewer`'s items, if they can still see it.
pub async fn get_item(pool: &PgPool, viewer: Uuid, id: Uuid) -> Result<SavedItemView, AppError> {
    sqlx::query_as::<_, SavedRow>(&select_sql("AND item.id = $2"))
        .bind(viewer)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .filter(|row| !hidden_by_limit(viewer, row))
        .map(view)
        .ok_or(AppError::NotFound("saved_item"))
}

/// Mark which of these entries' messages `viewer` has saved and not done,
/// and when they'll be reminded.
pub(crate) async fn attach_saved<'e, E>(
    executor: E,
    entries: &mut [TimelineEntryView],
    viewer: Uuid,
) -> Result<(), AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let ids = entries
        .iter()
        .filter_map(|entry| entry.message.as_ref())
        .map(|message| message.id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Option<DateTime<Utc>>)>(
        r#"
        SELECT message_id, id, remind_at FROM saved_items
        WHERE actor_id = $1 AND message_id = ANY($2) AND done_at IS NULL
        "#,
    )
    .bind(viewer)
    .bind(&ids)
    .fetch_all(executor)
    .await?;
    for message in entries
        .iter_mut()
        .filter_map(|entry| entry.message.as_mut())
    {
        message.saved = rows
            .iter()
            .find(|(message_id, _, _)| *message_id == message.id)
            .map(|(_, id, remind_at)| crate::conversations::SavedMark {
                id: *id,
                remind_at: remind_at.map(timestamp),
            });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Changing

/// Who saves: a person, or an agent acting as itself in one project.
#[derive(Clone, Copy, Debug)]
pub struct Saver {
    pub actor_id: Uuid,
    pub human: bool,
    /// For a bare reminder: the workspace a person files it in, or the
    /// project an agent files it in.
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// Only messages and tasks in this project (agents, and connections
    /// confined to one project).
    pub within: Option<Uuid>,
    /// The connection an agent saves through; its rules decide what the
    /// agent can see, now and when a reminder fires.
    pub connection_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveRequest {
    pub message_id: Option<Uuid>,
    pub work_item_id: Option<Uuid>,
    /// A reminder's text, or a note on a saved message or task.
    #[serde(default)]
    pub note: Option<String>,
    /// RFC 3339; when to be reminded.
    #[serde(default)]
    pub remind_at: Option<String>,
    /// The workspace a bare reminder goes in; your default when absent.
    #[serde(default)]
    pub organization_id: Option<Uuid>,
}

fn clean_note(note: Option<&str>) -> Result<Option<String>, AppError> {
    match note.map(str::trim) {
        None => Ok(None),
        Some(note) if note.chars().count() > MAX_NOTE_CHARACTERS => {
            Err(AppError::Validation("note"))
        }
        Some(note) => Ok(Some(note.to_owned())),
    }
}

/// A reminder time: now or later (a minute late is taken as now), at most a
/// year ahead.
pub fn parse_remind_at(value: &str) -> Result<DateTime<Utc>, AppError> {
    let at = DateTime::parse_from_rfc3339(value)
        .map_err(|_| AppError::Validation("remindAt"))?
        .with_timezone(&Utc);
    let now = Utc::now();
    if at < now - Duration::minutes(1) || at > now + Duration::days(MAX_AHEAD_DAYS) {
        return Err(AppError::Validation("remindAt"));
    }
    Ok(at.max(now))
}

/// Save a message or task (again: a done one comes back), or write a
/// reminder. A message must be one the saver can see now; a task one in a
/// project they can open.
pub async fn save(
    state: &AppState,
    saver: Saver,
    request: SaveRequest,
) -> Result<SavedItemView, AppError> {
    let note = clean_note(request.note.as_deref())?;
    let remind_at = request
        .remind_at
        .as_deref()
        .map(parse_remind_at)
        .transpose()?;
    let mut transaction = state.pool.begin().await?;
    let open = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_items WHERE actor_id = $1 AND done_at IS NULL",
    )
    .bind(saver.actor_id)
    .fetch_one(&mut *transaction)
    .await?;
    if open >= MAX_OPEN_ITEMS {
        return Err(AppError::Conflict(
            "saved_item",
            "you have too many saved items; mark some done first",
        ));
    }
    let id = match (request.message_id, request.work_item_id) {
        (Some(message_id), None) => {
            let (project_id, organization_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
                r#"
                SELECT message.project_id, project.organization_id
                FROM messages message
                JOIN projects project ON project.id = message.project_id
                WHERE message.id = $2 AND message.removed_at IS NULL
                  AND actor_can_access_project($1, $4, message.project_id)
                  AND conversation_visible_to(message.conversation_id, $1)
                  AND ($3::uuid IS NULL OR message.project_id = $3)
                "#,
            )
            .bind(saver.actor_id)
            .bind(message_id)
            .bind(saver.within)
            .bind(saver.connection_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(AppError::NotFound("message"))?;
            upsert(
                &mut transaction,
                saver.actor_id,
                saver.connection_id,
                organization_id,
                project_id,
                Target::Message(message_id),
                note.as_deref(),
                remind_at,
            )
            .await?
        }
        (None, Some(work_item_id)) => {
            let (project_id, organization_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
                r#"
                SELECT work.project_id, project.organization_id
                FROM work_items work
                JOIN projects project ON project.id = work.project_id
                WHERE work.id = $2 AND actor_can_access_project($1, $4, work.project_id)
                  AND ($3::uuid IS NULL OR work.project_id = $3)
                "#,
            )
            .bind(saver.actor_id)
            .bind(work_item_id)
            .bind(saver.within)
            .bind(saver.connection_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(AppError::NotFound("work_item"))?;
            upsert(
                &mut transaction,
                saver.actor_id,
                saver.connection_id,
                organization_id,
                project_id,
                Target::WorkItem(work_item_id),
                note.as_deref(),
                remind_at,
            )
            .await?
        }
        (None, None) => {
            let note = note
                .filter(|note| !note.is_empty())
                .ok_or(AppError::Validation("note"))?;
            let (organization_id, project_id) = if saver.human {
                let organization_id = crate::notification_settings::home_workspace(
                    &state.pool,
                    saver.actor_id,
                    request.organization_id.or(saver.organization_id),
                )
                .await?;
                let project_id = crate::direct::space_for_member(
                    &mut transaction,
                    organization_id,
                    saver.actor_id,
                )
                .await?
                .ok_or(AppError::NotFound("organization"))?;
                (organization_id, project_id)
            } else {
                let project_id = saver.project_id.ok_or(AppError::Validation("projectId"))?;
                let organization_id = sqlx::query_scalar::<_, Uuid>(
                    "SELECT organization_id FROM projects WHERE id = $2 AND actor_can_access_project($1, $3, id)",
                )
                .bind(saver.actor_id)
                .bind(project_id)
                .bind(saver.connection_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(AppError::NotFound("project"))?;
                (organization_id, project_id)
            };
            upsert(
                &mut transaction,
                saver.actor_id,
                saver.connection_id,
                organization_id,
                project_id,
                Target::Reminder,
                Some(&note),
                remind_at,
            )
            .await?
        }
        (Some(_), Some(_)) => return Err(AppError::Validation("messageId")),
    };
    transaction.commit().await?;
    get_item(&state.pool, saver.actor_id, id).await
}

enum Target {
    Message(Uuid),
    WorkItem(Uuid),
    Reminder,
}

#[allow(clippy::too_many_arguments)]
async fn upsert(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    connection_id: Option<Uuid>,
    organization_id: Uuid,
    project_id: Uuid,
    target: Target,
    note: Option<&str>,
    remind_at: Option<DateTime<Utc>>,
) -> Result<Uuid, AppError> {
    let (kind, message_id, work_item_id, conflict) = match target {
        Target::Message(id) => (
            "message",
            Some(id),
            None,
            "ON CONFLICT (actor_id, message_id) WHERE message_id IS NOT NULL",
        ),
        Target::WorkItem(id) => (
            "work_item",
            None,
            Some(id),
            "ON CONFLICT (actor_id, work_item_id) WHERE work_item_id IS NOT NULL",
        ),
        Target::Reminder => ("reminder", None, None, ""),
    };
    let previous = match (message_id, work_item_id) {
        (Some(message_id), _) => sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(
            "SELECT id, remind_at FROM saved_items WHERE actor_id = $1 AND message_id = $2 FOR UPDATE",
        )
        .bind(actor_id)
        .bind(message_id)
        .fetch_optional(&mut **transaction)
        .await?,
        (_, Some(work_item_id)) => sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(
            "SELECT id, remind_at FROM saved_items WHERE actor_id = $1 AND work_item_id = $2 FOR UPDATE",
        )
        .bind(actor_id)
        .bind(work_item_id)
        .fetch_optional(&mut **transaction)
        .await?,
        _ => None,
    };
    let sql = format!(
        r#"
        INSERT INTO saved_items (
            id, organization_id, project_id, actor_id, kind, message_id, work_item_id,
            note, remind_at, external_agent_connection_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, ''), $9, $10)
        {conflict} {update}
        RETURNING id, remind_at
        "#,
        update = if conflict.is_empty() {
            ""
        } else {
            "DO UPDATE SET done_at = NULL,
                note = COALESCE($8, saved_items.note),
                remind_at = COALESCE($9, saved_items.remind_at),
                external_agent_connection_id = EXCLUDED.external_agent_connection_id,
                updated_at = now()"
        },
    );
    let (id, scheduled) = sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(&sql)
        .bind(Uuid::now_v7())
        .bind(organization_id)
        .bind(project_id)
        .bind(actor_id)
        .bind(kind)
        .bind(message_id)
        .bind(work_item_id)
        .bind(note)
        .bind(remind_at)
        .bind(connection_id)
        .fetch_one(&mut **transaction)
        .await?;
    if let Some((_, Some(old))) = previous
        && Some(old) != scheduled
    {
        unschedule(&mut **transaction, id, old).await?;
    }
    if let Some(at) = scheduled
        && at > Utc::now() - Duration::minutes(1)
    {
        schedule(&mut **transaction, organization_id, id, at).await?;
    }
    Ok(id)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeRequest {
    /// RFC 3339, or null to stop reminding.
    #[serde(default, deserialize_with = "optional_field")]
    pub remind_at: Option<Option<String>>,
    pub note: Option<String>,
    pub done: Option<bool>,
}

/// A field that may be absent (leave it) or null (clear it).
fn optional_field<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(FromRow)]
struct LockedItem {
    id: Uuid,
    organization_id: Uuid,
    kind: String,
    remind_at: Option<DateTime<Utc>>,
    done_at: Option<DateTime<Utc>>,
}

/// Change one of `actor_id`'s items: its time, its note, done or not.
pub async fn change(
    state: &AppState,
    actor_id: Uuid,
    id: Uuid,
    request: ChangeRequest,
) -> Result<SavedItemView, AppError> {
    // Only one they can still see.
    get_item(&state.pool, actor_id, id).await?;
    let note = clean_note(request.note.as_deref())?;
    let remind_at = match request.remind_at.as_ref() {
        None => None,
        Some(None) => Some(None),
        Some(Some(value)) => Some(Some(parse_remind_at(value)?)),
    };
    let mut transaction = state.pool.begin().await?;
    let item = sqlx::query_as::<_, LockedItem>(
        "SELECT id, organization_id, kind, remind_at, done_at FROM saved_items WHERE id = $1 AND actor_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("saved_item"))?;
    if item.kind == "reminder" {
        if note.as_deref().is_some_and(str::is_empty) {
            return Err(AppError::Validation("note"));
        }
        if remind_at == Some(None) {
            return Err(AppError::Validation("remindAt"));
        }
    }
    let next_remind = remind_at.unwrap_or(item.remind_at);
    let next_done = match request.done {
        Some(true) => Some(item.done_at.unwrap_or_else(Utc::now)),
        Some(false) => None,
        None => item.done_at,
    };
    sqlx::query(
        r#"
        UPDATE saved_items
        SET note = COALESCE($2, note), remind_at = $3, done_at = $4,
            -- A new time is a new reminder.
            reminded_for = CASE WHEN $3 IS DISTINCT FROM remind_at THEN NULL ELSE reminded_for END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(item.id)
    .bind(&note)
    .bind(next_remind)
    .bind(next_done)
    .execute(&mut *transaction)
    .await?;
    if let Some(old) = item.remind_at
        && (Some(old) != next_remind || next_done.is_some())
    {
        unschedule(&mut *transaction, item.id, old).await?;
    }
    if let Some(at) = next_remind
        && next_done.is_none()
        && (Some(at) != item.remind_at || item.done_at.is_some())
    {
        schedule(&mut *transaction, item.organization_id, item.id, at).await?;
    }
    transaction.commit().await?;
    get_item(&state.pool, actor_id, id).await
}

pub async fn remove(pool: &PgPool, actor_id: Uuid, id: Uuid) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    let removed = sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(
        "DELETE FROM saved_items WHERE id = $1 AND actor_id = $2 RETURNING id, remind_at",
    )
    .bind(id)
    .bind(actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("saved_item"))?;
    if let (id, Some(at)) = removed {
        unschedule(&mut *transaction, id, at).await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Snooze from a reminder's notification: remind again in an hour.
pub(crate) async fn snooze(
    state: &AppState,
    actor_id: Uuid,
    id: Uuid,
) -> Result<SavedItemView, AppError> {
    change(
        state,
        actor_id,
        id,
        ChangeRequest {
            remind_at: Some(Some(timestamp(
                Utc::now() + Duration::minutes(SNOOZE_MINUTES),
            ))),
            note: None,
            done: Some(false),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Reminding

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemindJob {
    item_id: Uuid,
    /// The time this job was queued for; a moved reminder has its own job.
    at_micros: i64,
}

fn job_key(id: Uuid, at: DateTime<Utc>) -> String {
    format!("{REMIND_KIND}:{id}:{}", at.timestamp_micros())
}

async fn schedule<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    jobs::enqueue(
        executor,
        NewJob::new(REMIND_KIND)
            .workspace(organization_id)
            .payload(RemindJob {
                item_id: id,
                at_micros: at.timestamp_micros(),
            })
            .unique(job_key(id, at))
            .run_at(at)
            .max_attempts(5),
    )
    .await?;
    Ok(())
}

async fn unschedule<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    jobs::cancel_queued(executor, &job_key(id, at)).await?;
    Ok(())
}

#[derive(FromRow)]
struct DueRow {
    id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    project_id: Uuid,
    kind: String,
    note: String,
    message_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    work_item_id: Option<Uuid>,
    remind_at: Option<DateTime<Utc>>,
    reminded_for: Option<DateTime<Utc>>,
    done_at: Option<DateTime<Utc>>,
    visible: bool,
}

async fn remind(state: &AppState, job: &RemindJob) -> Result<(), JobError> {
    let mut transaction = state.pool.begin().await?;
    // Access is checked now, as the item's owner, with an active account.
    let Some(row) = sqlx::query_as::<_, DueRow>(
        r#"
        SELECT item.id, item.actor_id, actor.kind AS actor_kind, item.project_id, item.kind,
               item.note, item.message_id, message.conversation_id, item.work_item_id,
               item.remind_at, item.reminded_for, item.done_at,
               (actor.status = 'active'
                AND actor_can_access_project(item.actor_id, item.external_agent_connection_id, item.project_id)
                AND (item.message_id IS NULL
                     OR (message.removed_at IS NULL
                         AND conversation_visible_to(message.conversation_id, item.actor_id)))
                -- A saved task only while their role there lets them see tasks.
                AND (item.work_item_id IS NULL OR EXISTS (
                    SELECT 1 FROM project_memberships sees_task
                    WHERE sees_task.project_id = item.project_id
                      AND sees_task.actor_id = item.actor_id AND sees_task.enabled
                      AND custom_role_allows(sees_task.role_id, 'tasks.view'))))
                   AS visible
        FROM saved_items item
        JOIN actors actor ON actor.id = item.actor_id
        LEFT JOIN messages message ON message.id = item.message_id
        WHERE item.id = $1
        FOR UPDATE OF item
        "#,
    )
    .bind(job.item_id)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        return Ok(());
    };
    let Some(at) = row.remind_at else {
        return Ok(());
    };
    if at.timestamp_micros() != job.at_micros
        || row.done_at.is_some()
        || row.reminded_for == Some(at)
    {
        return Ok(());
    }
    if row.visible {
        if row.actor_kind == "human" {
            notify(&mut transaction, &row, at).await?;
        } else {
            tell_agent(&mut transaction, &row).await?;
        }
    }
    // Reminded (or quietly dropped, for one they can no longer see), once.
    sqlx::query("UPDATE saved_items SET reminded_for = $2 WHERE id = $1")
        .bind(row.id)
        .bind(at)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn notify(
    transaction: &mut Transaction<'_, Postgres>,
    row: &DueRow,
    at: DateTime<Utc>,
) -> Result<(), AppError> {
    let (subject_kind, subject_id) = match (row.conversation_id, row.work_item_id) {
        (Some(conversation_id), _) => ("conversation", conversation_id),
        (None, Some(work_item_id)) => ("work_item", work_item_id),
        _ => ("saved_item", row.id),
    };
    let notification = NewNotification {
        project_id: row.project_id,
        recipient_actor_id: row.actor_id,
        kind: kind::REMINDER,
        subject_kind,
        subject_id,
        actor_id: None,
        conversation_id: row.conversation_id,
        message_id: row.message_id,
        work_item_id: row.work_item_id,
        comment_id: None,
        agent_run_id: None,
        approval_id: None,
        pull_request_id: None,
        detail: serde_json::json!({
            "note": row.note,
            "savedItemId": row.id,
            "savedKind": row.kind,
            "href": "/later",
        }),
        group_key: format!("reminder:{}", row.id),
        dedupe_key: format!("reminder:{}:{}", row.id, at.timestamp_micros()),
    };
    if notifications::deliver(transaction, &notification).await? {
        notifications::announce(transaction, row.project_id, row.actor_id).await?;
    }
    Ok(())
}

async fn tell_agent(
    transaction: &mut Transaction<'_, Postgres>,
    row: &DueRow,
) -> Result<(), AppError> {
    let connections = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT connection.id FROM external_agent_connections connection
        WHERE connection.actor_id = $1 AND connection.status = 'active'
          AND agent_connection_can_access(connection.id, $2)
        ORDER BY connection.id
        "#,
    )
    .bind(row.actor_id)
    .bind(row.project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let summary: String = if row.note.is_empty() {
        "A reminder you set is due".to_owned()
    } else {
        format!(
            "Reminder: {}",
            row.note.chars().take(200).collect::<String>()
        )
    };
    for connection_id in connections {
        crate::external_events::enqueue_external_event(
            transaction,
            connection_id,
            row.project_id,
            "reminder.due",
            &summary,
            "saved_item",
            row.id,
            "/later",
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/later", get(list_http).post(save_http))
        .route("/api/v1/later/{id}", patch(change_http).delete(remove_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    view: Option<ListView>,
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<LaterPage>, AppError> {
    Ok(Json(
        list(
            &state.pool,
            principal.actor_id,
            query.view.unwrap_or_default(),
            None,
        )
        .await?,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemResponse {
    item: SavedItemView,
}

async fn save_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<SaveRequest>,
) -> Result<(StatusCode, Json<ItemResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let item = save(
        &state,
        Saver {
            actor_id: principal.actor_id,
            human: true,
            organization_id: None,
            project_id: None,
            within: None,
            connection_id: None,
        },
        request,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(ItemResponse { item })))
}

async fn change_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ChangeRequest>,
) -> Result<Json<ItemResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let item = change(&state, principal.actor_id, id, request).await?;
    Ok(Json(ItemResponse { item }))
}

async fn remove_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    remove(&state.pool, principal.actor_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_times_must_be_ahead_and_within_a_year() {
        let soon = timestamp(Utc::now() + Duration::minutes(20));
        assert!(parse_remind_at(&soon).is_ok());
        let late_by_seconds = timestamp(Utc::now() - Duration::seconds(20));
        assert!(parse_remind_at(&late_by_seconds).is_ok());
        let past = timestamp(Utc::now() - Duration::hours(1));
        assert!(parse_remind_at(&past).is_err());
        let far = timestamp(Utc::now() + Duration::days(400));
        assert!(parse_remind_at(&far).is_err());
        assert!(parse_remind_at("tomorrow").is_err());
    }

    #[test]
    fn a_message_in_a_dm_thread_opens_in_its_parent() {
        let project = Uuid::now_v7();
        let thread = Uuid::now_v7();
        let parent = Uuid::now_v7();
        let message = Uuid::now_v7();
        assert_eq!(
            message_href(project, true, thread, Some("thread"), Some(parent), message),
            format!("/messages/{parent}#message-{message}")
        );
        assert_eq!(
            message_href(project, false, thread, Some("thread"), None, message),
            format!("/projects/{project}/threads/{thread}#message-{message}")
        );
    }
}
