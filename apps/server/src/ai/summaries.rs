//! AI6: "catch me up" on a thread or a task, in up to five lines, with the
//! outcome or decision when there is one.
//!
//! A summary is made for the person asking, from what they can see right
//! now (`conversation_visible_to` for a thread, project access for both),
//! and isn't stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    gateway::{self, AiError, Scope, Tier},
    prompts,
};
use crate::{AppState, error::AppError};

/// Most entries one summary reads; the newest are kept.
const MAX_ENTRIES: i64 = 300;
/// Most characters of one entry.
const ENTRY_CHARS: i32 = 1500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub lines: Vec<String>,
    pub outcome: Option<String>,
    pub status: Option<String>,
    /// How many messages or history entries it read.
    #[serde(default)]
    pub entries: usize,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug)]
pub enum SummaryRefusal {
    NotFound,
    Empty,
    Ai(AiError),
}

impl From<AiError> for SummaryRefusal {
    fn from(error: AiError) -> Self {
        Self::Ai(error)
    }
}

impl From<AppError> for SummaryRefusal {
    fn from(error: AppError) -> Self {
        Self::Ai(AiError::App(error))
    }
}

impl From<sqlx::Error> for SummaryRefusal {
    fn from(error: sqlx::Error) -> Self {
        Self::Ai(AiError::App(error.into()))
    }
}

#[derive(FromRow, Serialize)]
struct Entry {
    author: String,
    at: DateTime<Utc>,
    text: String,
}

async fn summarize(
    state: &AppState,
    scope: Scope,
    kind: &str,
    title: &str,
    mut entries: Vec<Entry>,
) -> Result<Summary, SummaryRefusal> {
    if entries.is_empty() {
        return Err(SummaryRefusal::Empty);
    }
    entries.reverse();
    let count = entries.len();
    let answer = gateway::run::<Summary>(
        state,
        scope,
        &prompts::SUMMARY,
        &json!({"kind": kind, "title": title, "entries": entries}),
        Tier::Quick,
    )
    .await?;
    let mut summary = answer.value;
    summary.lines.truncate(5);
    summary.entries = count;
    summary.provider = answer.provider.to_owned();
    summary.model = answer.model;
    Ok(summary)
}

/// Summarizes a thread or channel in `project_id` for `actor_id`, from the
/// messages since `since` (all of them when unset, newest 300 at most).
pub async fn thread(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    connection_id: Option<Uuid>,
    conversation_id: Uuid,
    since: Option<DateTime<Utc>>,
) -> Result<Summary, SummaryRefusal> {
    let Some(title) = sqlx::query_scalar::<_, String>(
        r#"
        SELECT conversation.name FROM conversations conversation
        WHERE conversation.id = $1 AND conversation.project_id = $2
          AND conversation_visible_to(conversation.id, $3)
          AND actor_can_access_project($3, $4, conversation.project_id)
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(connection_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(SummaryRefusal::NotFound);
    };
    let entries = sqlx::query_as::<_, Entry>(
        r#"
        SELECT COALESCE(membership.display_name, 'Someone') AS author,
               message.created_at AS at,
               left(message.plain_text, $4) AS text
        FROM messages message
        JOIN conversations conversation ON conversation.id = message.conversation_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = conversation.project_id
         AND membership.actor_id = message.author_actor_id
        WHERE message.conversation_id = $1 AND message.removed_at IS NULL
          AND ($2::timestamptz IS NULL OR message.created_at > $2)
          AND btrim(message.plain_text) <> ''
        ORDER BY message.created_at DESC
        LIMIT $3
        "#,
    )
    .bind(conversation_id)
    .bind(since)
    .bind(MAX_ENTRIES)
    .bind(ENTRY_CHARS)
    .fetch_all(&state.pool)
    .await?;
    summarize(
        state,
        Scope {
            organization_id,
            project_id: Some(project_id),
            actor_id: Some(actor_id),
        },
        "thread",
        &title,
        entries,
    )
    .await
}

/// Summarizes a task's history and comments for `actor_id`.
pub async fn task(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    connection_id: Option<Uuid>,
    task_id: Uuid,
) -> Result<Summary, SummaryRefusal> {
    let Some((key, title, description)) = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT project.project_key || '-' || item.number, item.title, left(item.description, 3000)
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.id = $1 AND item.project_id = $2
          AND actor_can_access_project($3, $4, item.project_id)
        "#,
    )
    .bind(task_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(connection_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(SummaryRefusal::NotFound);
    };
    let mut entries = sqlx::query_as::<_, Entry>(
        r#"
        SELECT author, at, text FROM (
            SELECT COALESCE(membership.display_name, 'Someone') AS author,
                   activity.created_at AS at, activity.summary AS text
            FROM work_item_activity activity
            LEFT JOIN project_memberships membership
              ON membership.project_id = activity.project_id
             AND membership.actor_id = activity.actor_id
            WHERE activity.project_id = $1 AND activity.work_item_id = $2
            UNION ALL
            SELECT COALESCE(membership.display_name, 'Someone'),
                   comment.created_at, left(comment.plain_text, $4)
            FROM work_item_comments comment
            LEFT JOIN project_memberships membership
              ON membership.project_id = comment.project_id
             AND membership.actor_id = comment.author_actor_id
            WHERE comment.project_id = $1 AND comment.work_item_id = $2
              AND comment.removed_at IS NULL
        ) history
        ORDER BY at DESC
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .bind(MAX_ENTRIES)
    .bind(ENTRY_CHARS)
    .fetch_all(&state.pool)
    .await?;
    if !description.trim().is_empty() {
        entries.push(Entry {
            author: "Description".to_owned(),
            at: entries.last().map_or_else(Utc::now, |entry| entry.at),
            text: description,
        });
    }
    summarize(
        state,
        Scope {
            organization_id,
            project_id: Some(project_id),
            actor_id: Some(actor_id),
        },
        "task",
        &format!("{key} {title}"),
        entries,
    )
    .await
}
