//! `message_list` (AWD-220): one read of everything said under a scope
//! after a point, oldest first, with its text, in pages measured for a
//! model's context instead of a screen. The other reads open one
//! conversation a screenful at a time, and events carry only ids (rule
//! 10), so catching up on a space used to cost a call per conversation.
//!
//! Every row is checked against the reader when it is read
//! (`conversation_visible_to`), after the dispatcher has placed the caller
//! in the space. DMs and groups live in no space, so they are never here.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef,
};

/// A page's size when the caller names none, in characters of message text.
const DEFAULT_CHARACTERS: i64 = 100_000;
const MAX_CHARACTERS: i64 = 1_000_000;
/// The most messages in one page, whatever their length.
const MAX_MESSAGES: i64 = 1_000;

pub struct MessageList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageListInput {
    /// The space to read: its ID or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Only this channel or thread, and for a channel its threads too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<IdRef>,
    /// Only the discussion of this task, page, sheet, whiteboard or record
    /// (its comments and their threads): its ID or a link to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_id: Option<IdRef>,
    /// Where the last read stopped (its nextCursor). Left out: from `since`,
    /// or from the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// For a first read: only messages after this time, such as
    /// 2026-11-27T20:00:00Z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// false leaves working notes out. They are included by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<bool>,
    /// About how much message text one page holds, in characters (default
    /// 100,000, at most 1,000,000). A message is never cut: a page ends
    /// with the message that reaches the size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_characters: Option<u32>,
}

impl Targeted for MessageListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListedMessage {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Which of `conversations` it was said in.
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    pub at: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_handle: Option<String>,
    pub text: String,
    /// A working note: a record that asked for nobody's attention.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub note: bool,
    /// The message it answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub reply_to_message_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
}

/// A conversation that messages of the page were said in.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListedConversation {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// channel, thread or comments (a thing's discussion).
    pub kind: String,
    pub name: String,
    /// A thread's channel or discussion.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent_id: Option<Uuid>,
    /// The kind of thing it discusses (task, page, ...), for a discussion
    /// and its threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about_kind: Option<String>,
    /// That thing's id: thing_open or task_get opens it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub about_id: Option<Uuid>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    /// Oldest first.
    pub messages: Vec<ListedMessage>,
    /// Each conversation the messages name, once.
    pub conversations: Vec<ListedConversation>,
    /// Pass it back as cursor to read on from here, now or later.
    pub next_cursor: String,
    /// true: more is waiting already; read again now.
    pub more: bool,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    conversation_id: Uuid,
    created_at: DateTime<Utc>,
    author: Option<String>,
    author_handle: Option<String>,
    text: String,
    note: bool,
    reply_to_message_id: Option<Uuid>,
    edited: bool,
    conversation_kind: String,
    conversation_name: String,
    parent_id: Option<Uuid>,
    about_kind: Option<String>,
    about_id: Option<Uuid>,
    has_next: bool,
}

fn decode_cursor(cursor: &str) -> Option<(DateTime<Utc>, Uuid)> {
    let (time, id) = cursor.split_once('_')?;
    Some((
        DateTime::from_timestamp_micros(time.parse().ok()?)?,
        id.parse().ok()?,
    ))
}

impl Operation for MessageList {
    const NAME: &'static str = "message_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List messages";
    const DESCRIPTION: &'static str = "Catch up on a space in one read: every message you can \
        see there after a point (channels, threads, and the discussions on tasks, pages and \
        other things; working notes too), oldest first, with its text, who wrote it and where. \
        conversationId or thingId narrows it to one channel or one thing's discussion. Pages \
        are sized in characters of text (maxCharacters), not in messages; pass nextCursor back \
        as cursor to read on. A message sent in the last two seconds waits for the next read. \
        DMs and groups aren't in a space: read those with conversation_read.";

    type Input = MessageListInput;
    type Output = MessagePage;

    async fn run(call: &mut OpCall<'_>, input: MessageListInput) -> Result<MessagePage, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let caller = call.ctx.caller.account_id;
        let after = match (&input.cursor, &input.since) {
            (Some(cursor), _) => Some(decode_cursor(cursor).ok_or(OpError::invalid(
                "cursor",
                "Pass the nextCursor of your last read, or none.",
            ))?),
            (None, Some(since)) => Some((
                DateTime::parse_from_rfc3339(since.trim())
                    .map_err(|_| {
                        OpError::invalid("since", "Pass a time such as 2026-11-27T20:00:00Z.")
                    })?
                    .with_timezone(&Utc),
                Uuid::max(),
            )),
            (None, None) => None,
        };
        let budget = input
            .max_characters
            .map_or(DEFAULT_CHARACTERS, i64::from)
            .clamp(1, MAX_CHARACTERS);
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<_, Row>(
            r#"
            SELECT * FROM (
                SELECT message.id, message.conversation_id, message.created_at,
                       author.display_name AS author, author.handle AS author_handle,
                       message.plain_text AS text, message.register = 'note' AS note,
                       message.reply_to_message_id, message.edited_at IS NOT NULL AS edited,
                       conversation.kind AS conversation_kind,
                       COALESCE(conversation.name, '') AS conversation_name,
                       conversation.parent_channel_id AS parent_id,
                       COALESCE(parent.subject_kind, conversation.subject_kind) AS about_kind,
                       COALESCE(parent.subject_id, conversation.subject_id) AS about_id,
                       COALESCE(sum(char_length(message.plain_text)) OVER earlier, 0)::bigint
                           AS before,
                       row_number() OVER ordered AS position,
                       lead(message.id) OVER ordered IS NOT NULL AS has_next
                FROM messages message
                JOIN conversations conversation ON conversation.id = message.conversation_id
                LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
                LEFT JOIN LATERAL place_profile(message.project_id, message.author_actor_id)
                    author ON true
                WHERE conversation.project_id = $1
                  AND conversation_visible_to(conversation.id, $2)
                  AND message.removed_at IS NULL
                  AND conversation.archived_at IS NULL
                  AND message.created_at < now() - interval '2 seconds'
                  AND ($3::timestamptz IS NULL
                       OR (message.created_at, message.id) > ($3::timestamptz, $4::uuid))
                  AND ($5::uuid IS NULL
                       OR conversation.id = $5 OR conversation.parent_channel_id = $5)
                  AND ($6::uuid IS NULL
                       OR COALESCE(parent.subject_id, conversation.subject_id) = $6)
                  AND ($7 OR message.register <> 'note')
                WINDOW ordered AS (ORDER BY message.created_at, message.id),
                       earlier AS (ORDER BY message.created_at, message.id
                                   ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING)
                ORDER BY message.created_at, message.id
                LIMIT $9 + 1
            ) page
            WHERE page.before < $8 AND page.position <= $9
            ORDER BY page.created_at, page.id
            "#,
        )
        .bind(space)
        .bind(caller)
        .bind(after.map(|(time, _)| time))
        .bind(after.map(|(_, id)| id))
        .bind(input.conversation_id.map(IdRef::id))
        .bind(input.thing_id.map(IdRef::id))
        .bind(input.notes != Some(false))
        .bind(budget)
        .bind(MAX_MESSAGES)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let next_cursor = match (rows.last(), after) {
            (Some(last), _) => format!("{}_{}", last.created_at.timestamp_micros(), last.id),
            (None, Some((time, id))) => format!("{}_{}", time.timestamp_micros(), id),
            (None, None) => format!("0_{}", Uuid::nil()),
        };
        let more = rows.last().is_some_and(|last| last.has_next);
        let mut conversations = Vec::<ListedConversation>::new();
        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            if !conversations
                .iter()
                .any(|known| known.id == row.conversation_id)
            {
                conversations.push(ListedConversation {
                    id: row.conversation_id,
                    kind: row.conversation_kind,
                    name: row.conversation_name,
                    parent_id: row.parent_id,
                    about_kind: row.about_kind,
                    about_id: row.about_id,
                });
            }
            messages.push(ListedMessage {
                id: row.id,
                conversation_id: row.conversation_id,
                at: row.created_at.to_rfc3339(),
                author: row.author.unwrap_or_else(|| "Someone".to_owned()),
                author_handle: row.author_handle,
                text: row.text,
                note: row.note,
                reply_to_message_id: row.reply_to_message_id,
                edited: row.edited,
            });
        }
        Ok(MessagePage {
            messages,
            conversations,
            next_cursor,
            more,
        })
    }
}
