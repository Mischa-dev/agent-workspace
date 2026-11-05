//! W21: forwarding a message to another conversation.
//!
//! A forward is a new message, sent as the forwarder with the ordinary send
//! command (so it can only go where they can post, and its reply rules and
//! mentions apply there), that points at the original. The original is
//! never copied: its quote is read live for each viewer and shown only to
//! people who can see the original's conversation now. Everyone else sees
//! that a message was forwarded and nothing of it, so forwarding out of a
//! private channel can't widen who reads it.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        AppendExtras, MessageAppendResponse, MessageCommandActor, TimelineEntryView,
        append_message_command_with,
    },
    error::{ApiJson, ApiPath, AppError},
    idempotency::IdempotencyKey,
    message_content::MessageContent,
};

const QUOTE_CHARACTERS: i32 = 2_000;
const COMMENT_CHARACTERS: usize = 4_000;
/// The text of a forward sent without a comment.
pub const DEFAULT_TEXT: &str = "Forwarded a message";

/// The forwarded message, as one viewer may see it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardedView {
    pub message_id: Uuid,
    /// False when the viewer can't see the original: nothing else is set.
    pub available: bool,
    #[serde(default)]
    pub removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_kind: Option<String>,
    /// "#general", "a DM", a group's or thread's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Up to 2,000 characters of its text, as it reads now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl ForwardedView {
    /// Before the viewer's access is checked: shows nothing.
    pub fn unread(message_id: Uuid) -> Self {
        Self {
            message_id,
            available: false,
            removed: false,
            author_name: None,
            author_kind: None,
            place: None,
            text: None,
            created_at: None,
            href: None,
        }
    }
}

#[derive(FromRow)]
struct SourceRow {
    id: Uuid,
    project_id: Uuid,
    conversation_id: Uuid,
    conversation_kind: String,
    conversation_name: String,
    parent_conversation_id: Option<Uuid>,
    direct_space: bool,
    removed: bool,
    text: String,
    author_name: Option<String>,
    author_kind: String,
    created_at: DateTime<Utc>,
}

/// Messages `$2` can see now, among `$1`.
const SOURCE_SQL: &str = r#"
    SELECT message.id, message.project_id, message.conversation_id,
           conversation.kind AS conversation_kind, conversation.name AS conversation_name,
           conversation.parent_channel_id AS parent_conversation_id,
           project.kind = 'direct' AS direct_space,
           message.removed_at IS NOT NULL AS removed,
           CASE WHEN message.removed_at IS NULL
                THEN left(message.plain_text, $3) ELSE '' END AS text,
           author.display_name AS author_name, author_actor.kind AS author_kind,
           message.created_at
    FROM messages message
    JOIN conversations conversation ON conversation.id = message.conversation_id
    JOIN projects project ON project.id = message.project_id
    JOIN actors author_actor ON author_actor.id = message.author_actor_id
    LEFT JOIN project_memberships author
      ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
    WHERE message.id = ANY($1)
      AND actor_can_access_project($2, NULL, message.project_id)
      AND conversation_visible_to(message.conversation_id, $2)
"#;

fn place(kind: &str, name: &str) -> String {
    match kind {
        "dm" => "a DM".to_owned(),
        "group" if !name.is_empty() => name.to_owned(),
        "group" => "a group".to_owned(),
        "channel" => format!("#{name}"),
        _ if !name.is_empty() => name.to_owned(),
        _ => "a conversation".to_owned(),
    }
}

fn available(row: SourceRow) -> ForwardedView {
    ForwardedView {
        message_id: row.id,
        available: true,
        removed: row.removed,
        author_name: Some(
            row.author_name
                .unwrap_or_else(|| "Former member".to_owned()),
        ),
        author_kind: Some(row.author_kind),
        place: Some(place(&row.conversation_kind, &row.conversation_name)),
        text: (!row.removed).then_some(row.text),
        created_at: Some(crate::later::timestamp(row.created_at)),
        href: Some(crate::later::message_href(
            row.project_id,
            row.direct_space,
            row.conversation_id,
            Some(&row.conversation_kind),
            row.parent_conversation_id,
            row.id,
        )),
    }
}

/// Fill in the quotes of forwarded messages, and of the messages replies
/// answer (AWD-70), among these entries, as `viewer` may see them. Both are
/// read through `conversation_visible_to` for the viewer every time, so a
/// quote never shows to someone who can't see the original now.
pub(crate) async fn attach_forwards<'e, E>(
    executor: E,
    entries: &mut [TimelineEntryView],
    viewer: Uuid,
) -> Result<(), AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let sources = entries
        .iter()
        .filter_map(|entry| entry.message.as_ref())
        .filter(|message| message.removed.is_none())
        .flat_map(|message| [message.forwarded.as_ref(), message.reply_to.as_ref()])
        .flatten()
        .map(|quoted| quoted.message_id)
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query_as::<_, SourceRow>(SOURCE_SQL)
        .bind(&sources)
        .bind(viewer)
        .bind(QUOTE_CHARACTERS)
        .fetch_all(executor)
        .await?;
    let found = rows
        .into_iter()
        .map(|row| (row.id, available(row)))
        .collect::<std::collections::HashMap<_, _>>();
    for message in entries
        .iter_mut()
        .filter_map(|entry| entry.message.as_mut())
    {
        if message.removed.is_some() {
            message.forwarded = None;
            message.reply_to = None;
            continue;
        }
        for quoted in [message.forwarded.as_mut(), message.reply_to.as_mut()]
            .into_iter()
            .flatten()
        {
            *quoted = found
                .get(&quoted.message_id)
                .cloned()
                .unwrap_or_else(|| ForwardedView::unread(quoted.message_id));
        }
    }
    Ok(())
}

/// The messages among `ids` that `viewer` can see now, quoted. One the
/// viewer can't see is simply missing.
pub(crate) async fn quotes<'e, E>(
    executor: E,
    ids: &[Uuid],
    viewer: Uuid,
) -> Result<std::collections::HashMap<Uuid, ForwardedView>, AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(sqlx::query_as::<_, SourceRow>(SOURCE_SQL)
        .bind(ids)
        .bind(viewer)
        .bind(QUOTE_CHARACTERS)
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(|row| (row.id, available(row)))
        .collect())
}

/// The message a new reply answers, as its sender sees it (AWD-70). It must
/// be in `conversation_id`, the conversation the sender was just allowed to
/// post in, and not removed. Anything else, including a message elsewhere
/// that the sender can't see, gets the same refusal, so it says nothing
/// about what's there.
pub(crate) async fn quote_for_reply(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation_id: Uuid,
    message_id: Uuid,
    sender: Uuid,
) -> Result<ForwardedView, AppError> {
    sqlx::query_as::<_, SourceRow>(SOURCE_SQL)
        .bind(vec![message_id])
        .bind(sender)
        .bind(QUOTE_CHARACTERS)
        .fetch_optional(&mut **transaction)
        .await?
        .filter(|row| row.conversation_id == conversation_id && !row.removed)
        .map(available)
        .ok_or(AppError::Refused {
            field: "replyToMessageId",
            message: "Reply to a message in this conversation that hasn't been removed.",
        })
}

// ---------------------------------------------------------------------------
// HTTP

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/messages/{id}/forward", post(forward_http))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForwardRequest {
    /// Where it goes: a channel, thread, DM or group you can post in.
    conversation_id: Uuid,
    /// An optional comment sent with it.
    #[serde(default)]
    text: Option<String>,
}

async fn forward_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(message_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ForwardRequest>,
) -> Result<(StatusCode, Json<MessageAppendResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let (status, body) = forward(
        &state,
        MessageCommandActor {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind.clone(),
            mcp_credential: None,
            via_grant_id: None,
        },
        message_id,
        request.conversation_id,
        request.text.as_deref(),
        idempotency_key,
    )
    .await?;
    Ok((status, Json(body)))
}

/// Forward `message_id`, which the sender must be able to see now, into
/// `conversation_id`, where the send command checks they can post.
pub(crate) async fn forward(
    state: &AppState,
    actor: MessageCommandActor,
    message_id: Uuid,
    conversation_id: Uuid,
    text: Option<&str>,
    idempotency_key: IdempotencyKey,
) -> Result<(StatusCode, MessageAppendResponse), AppError> {
    let source = sqlx::query_as::<_, SourceRow>(SOURCE_SQL)
        .bind(vec![message_id])
        .bind(actor.actor_id)
        .bind(QUOTE_CHARACTERS)
        .fetch_optional(&state.pool)
        .await?
        .filter(|row| !row.removed)
        .ok_or(AppError::NotFound("message"))?;
    if source.conversation_id == conversation_id {
        return Err(AppError::Validation("conversationId"));
    }
    let text = text.map(str::trim).filter(|text| !text.is_empty());
    if text.is_some_and(|text| text.chars().count() > COMMENT_CHARACTERS) {
        return Err(AppError::Validation("text"));
    }
    let content = MessageContent::from_plain_text(text.unwrap_or(DEFAULT_TEXT))
        .map_err(|_| AppError::Validation("text"))?;
    let (content, _, _) = content.into_parts();
    let outcome = append_message_command_with(
        state,
        actor,
        conversation_id,
        None,
        idempotency_key,
        serde_json::to_value(content).map_err(|_| AppError::Internal("forward content"))?,
        &[],
        None,
        AppendExtras {
            forwarded_message_id: Some(source.id),
            ..AppendExtras::default()
        },
    )
    .await?;
    let mut body = outcome.body;
    let quote = available(source);
    body.message.forwarded = Some(quote.clone());
    if let Some(message) = body.entry.message.as_mut() {
        message.forwarded = Some(quote);
    }
    Ok((outcome.status, body))
}
