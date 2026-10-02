//! Conversation tools for everyone: list, open, read, start, send, mark read,
//! find people, and start or update threads. People connected through an app
//! and agents use the same tools; access always goes through
//! `conversation_visible_to` and current membership.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        MESSAGE_CHARACTERS, SNIPPET_CHARACTERS, ToolResult, content_from_text, conversation_path,
        insert_audit, parse_id, parse_message_id, project_role, public_url, resolve_people,
        role_allows, scope_projects, snippet, truncate_text,
    },
    reactions::{ReactionSummary, reaction_summaries},
};
use crate::{
    AppState,
    conversations::{
        ConversationAccessRow, MessageCommandActor, MessageMcpCredential,
        THREAD_STATUS_SUGGESTIONS, ThreadPatch, append_message_command,
        authorize_conversation_row_for, enqueue_event, fetch_conversation_access,
        lock_conversation_for, normalize_conversation_name,
        suggested_thread_name as suggested_name, update_thread,
    },
    error::AppError,
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    outbox::{
        ConversationCreatedPayload, OutboxConversationKind, OutboxEventPayload,
        ReadStateUpdatedPayload,
    },
    policy::{Action, ActorKind},
    reactions::reaction_rows,
};

const DEFAULT_MESSAGES: i64 = 50;
const MAX_MESSAGES: i64 = 200;
const DEFAULT_LIST: i64 = 50;
const MAX_LIST: i64 = 200;
/// A thread this long with no outcome gets a nudge to record one.
const LONG_THREAD_MESSAGES: i64 = 40;
pub(super) const MAX_TEXT_CHARACTERS: usize = 20_000;

// ---------------------------------------------------------------------------
// Arguments

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationListArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// Only this project. Leave it out to list everything you can see.
    pub project_id: Option<String>,
    /// channel, thread, dm, group, or direct (DMs and groups together).
    pub kind: Option<String>,
    /// Only conversations with this person: an actor ID or @handle.
    pub with_person: Option<String>,
    /// Only conversations with messages you haven't read.
    pub unread_only: Option<bool>,
    /// Threads only: a status such as Open, In progress, Decided or Done.
    pub status: Option<String>,
    /// Words in the conversation's name.
    pub query: Option<String>,
    /// How many to return (default 50, at most 200).
    pub limit: Option<u16>,
    /// The nextCursor from an earlier call.
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationOpenArguments {
    pub conversation_id: String,
    /// How many recent messages to include (default 50, at most 200).
    pub limit: Option<u16>,
    /// Mark everything up to the newest message as read.
    pub mark_read: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationReadArguments {
    pub conversation_id: String,
    /// Messages before this sequence number (older).
    pub before: Option<i64>,
    /// Messages after this sequence number (newer).
    pub after: Option<i64>,
    /// Messages around this message: its ID, or a link to it.
    pub around_message_id: Option<String>,
    /// How many messages (default 50, at most 200).
    pub limit: Option<u16>,
    /// Mark what you read as read.
    pub mark_read: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationStartArguments {
    /// Who to talk with: actor IDs or @handles. One person makes a DM; more
    /// people (or a name) make a group.
    pub people: Vec<String>,
    /// A group name. Leave it out for a DM.
    pub name: Option<String>,
    /// An optional first message. @handles become mentions.
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageSendArguments {
    pub conversation_id: String,
    /// The message as plain text. @handles of people in the project become
    /// mentions.
    pub text: Option<String>,
    /// Structured content instead of text, for exact mentions and task links.
    pub content: Option<MessageContent>,
    /// A stable ID for this message, so retrying never sends it twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MarkReadArguments {
    pub conversation_id: String,
    /// Mark read up to this sequence number. Defaults to the newest message.
    pub up_to_sequence: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PeopleSearchArguments {
    /// Part of a name or handle. Leave it out to list everyone.
    pub query: Option<String>,
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// Only people in this project.
    pub project_id: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadStartArguments {
    /// The channel, DM, or group the thread goes in.
    pub conversation_id: String,
    /// Start the thread from this message, by ID or link (the usual case
    /// when a topic starts running long). Starting again from the same
    /// message returns the same thread.
    pub from_message_id: Option<String>,
    /// The thread's name. Defaults to the start of the message.
    pub name: Option<String>,
    /// An optional first message in the thread.
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadUpdateArguments {
    pub thread_id: String,
    /// A new name.
    pub name: Option<String>,
    /// Where the thread stands. Suggestions: Open, In progress, Decided,
    /// Done, Dropped, Blocked. Any short status works.
    pub status: Option<String>,
    /// One line on where the thread landed, such as "Went with design
    /// tokens; AW-51 created".
    pub outcome: Option<String>,
    /// Remove the outcome.
    pub clear_outcome: Option<bool>,
}

// ---------------------------------------------------------------------------
// Projections

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationSummary {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub project_id: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub unread_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<LastMessage>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LastMessage {
    pub author: String,
    /// "Message removed" when it was removed.
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    pub at: String,
}

#[derive(Debug, FromRow)]
pub(super) struct ConversationRow {
    id: Uuid,
    kind: String,
    name: String,
    project_id: Uuid,
    project_name: String,
    direct: bool,
    parent_id: Option<Uuid>,
    parent_name: Option<String>,
    parent_kind: Option<String>,
    thread_status: String,
    thread_outcome: Option<String>,
    highest_sequence: i64,
    last_read_sequence: i64,
    last_activity_at: String,
    last_author: Option<String>,
    last_text: Option<String>,
    last_removed: Option<bool>,
    member_names: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationListValue {
    pub conversations: Vec<ConversationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One message (or Codex run event) in a conversation.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageItem {
    pub sequence: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// "Name (@handle)", with ", agent" for agents.
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    /// Empty when the message was removed.
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub sent_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Present once the message was edited: pass it as expectedRevision to
    /// message_edit. A message never edited is revision 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Removed for everyone; its text is gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// "Name (@handle)" of whoever removed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_by: Option<String>,
    /// Threads started from this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadRef>,
    /// Emoji reactions, in the order first added. None on a removed message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadRef {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, FromRow)]
struct MessageRow {
    sequence: i64,
    at: String,
    message_id: Option<Uuid>,
    author_id: Option<Uuid>,
    author_kind: Option<String>,
    author_name: Option<String>,
    author_handle: Option<String>,
    text: Option<String>,
    via: Option<String>,
    event_summary: Option<String>,
    threads: Option<serde_json::Value>,
    revision: Option<i32>,
    edited_at: Option<String>,
    removed: Option<bool>,
    remover_name: Option<String>,
    remover_handle: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationDetail {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub project_id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ParentRef {
    pub id: String,
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PinItem {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadItem {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub message_count: i64,
    pub unread: bool,
    pub last_activity_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationOpenValue {
    pub conversation: ConversationDetail,
    /// Pages, decisions and messages pinned here: what's true for this
    /// conversation right now.
    pub pinned: Vec<PinItem>,
    pub highest_sequence: i64,
    pub last_read_sequence: i64,
    pub unread_count: i64,
    pub messages: Vec<MessageItem>,
    /// True when older messages exist; read them with conversation_read.
    pub has_older: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_threads: Vec<ThreadItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recently_settled_threads: Vec<ThreadItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationReadValue {
    pub conversation_id: String,
    pub messages: Vec<MessageItem>,
    pub has_older: bool,
    pub has_newer: bool,
    pub highest_sequence: i64,
    pub last_read_sequence: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SentMessage {
    pub conversation_id: String,
    pub message_id: String,
    pub sequence: i64,
    pub text: String,
    pub sent_at: String,
    /// True when this clientRequestId was already used for the same message,
    /// so nothing new was sent.
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct StartedConversation {
    pub conversation: ConversationSummary,
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<SentMessage>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadMark {
    pub conversation_id: String,
    pub last_read_sequence: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct Person {
    pub id: String,
    pub name: String,
    pub handle: String,
    /// person or agent
    pub kind: String,
    pub projects: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeopleValue {
    pub people: Vec<Person>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadValue {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub parent_id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<SentMessage>,
}

// ---------------------------------------------------------------------------
// conversation_list

pub(super) async fn conversation_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationListArguments,
) -> ToolResult<ConversationListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let limit = match bounded(arguments.limit, DEFAULT_LIST, MAX_LIST) {
        Some(limit) => limit,
        None => return ToolResult::invalid("limit", "Use a limit from 1 to 200."),
    };
    let offset = match arguments
        .cursor
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
    {
        Ok(offset) => offset.unwrap_or(0).max(0),
        Err(_) => return ToolResult::invalid("cursor", "Pass the nextCursor you were given."),
    };
    let kinds: Option<Vec<&str>> = match arguments.kind.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("direct") => Some(vec!["dm", "group"]),
        Some(kind @ ("channel" | "thread" | "dm" | "group")) => Some(vec![kind]),
        Some(_) => {
            return ToolResult::invalid("kind", "Use channel, thread, dm, group, or direct.");
        }
    };
    let result = async {
        let projects = scope_projects(state, principal).await?;
        let with_person = match arguments.with_person.as_deref() {
            Some(person) => match resolve_person_anywhere(state, &projects, person).await? {
                Some(actor_id) => Some(actor_id),
                None => {
                    return Ok(ToolResult::invalid(
                        "withPerson",
                        format!("No one called {person} was found. Try people_search."),
                    ));
                }
            },
            None => None,
        };
        let rows = list_rows(
            state,
            principal.actor_id,
            &projects,
            ListFilter {
                kinds: kinds.map(|kinds| kinds.into_iter().map(str::to_owned).collect()),
                with_person,
                unread_only: arguments.unread_only.unwrap_or(false),
                status: arguments.status.clone(),
                query: arguments.query.clone().unwrap_or_default(),
                only_id: None,
            },
            offset,
            limit + 1,
        )
        .await?;
        let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        let conversations = rows
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(|row| summary(state, principal.actor_id, row))
            .collect();
        Ok::<_, AppError>(ToolResult::ok(ConversationListValue {
            conversations,
            next_cursor: has_more.then(|| (offset + limit).to_string()),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_list"))
}

pub(super) struct ListFilter {
    pub kinds: Option<Vec<String>>,
    pub with_person: Option<Uuid>,
    pub unread_only: bool,
    pub status: Option<String>,
    pub query: String,
    pub only_id: Option<Uuid>,
}

pub(super) async fn list_rows(
    state: &AppState,
    viewer: Uuid,
    projects: &[Uuid],
    filter: ListFilter,
    offset: i64,
    limit: i64,
) -> Result<Vec<ConversationRow>, AppError> {
    Ok(sqlx::query_as::<_, ConversationRow>(
        r#"
        SELECT conversation.id, conversation.kind, conversation.name,
               conversation.project_id, project.name AS project_name,
               project.kind = 'direct' AS direct,
               parent.id AS parent_id, parent.name AS parent_name, parent.kind AS parent_kind,
               conversation.thread_status, conversation.thread_outcome,
               conversation.next_entry_sequence - 1 AS highest_sequence,
               COALESCE(read_state.highest_seen_sequence, 0) AS last_read_sequence,
               COALESCE(last_entry.created_at, conversation.created_at)::text AS last_activity_at,
               last_author.display_name AS last_author,
               CASE WHEN last_message.removed_at IS NOT NULL THEN 'Message removed'
                    ELSE last_message.plain_text END AS last_text,
               last_message.removed_at IS NOT NULL AS last_removed,
               members.names AS member_names
        FROM conversations conversation
        JOIN projects project ON project.id = conversation.project_id
        JOIN project_memberships me
          ON me.project_id = conversation.project_id AND me.actor_id = $1 AND me.enabled
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = conversation.id AND read_state.actor_id = $1
        LEFT JOIN LATERAL (
            SELECT entry.created_at, entry.message_id
            FROM conversation_entries entry
            WHERE entry.conversation_id = conversation.id
            ORDER BY entry.sequence DESC
            LIMIT 1
        ) last_entry ON true
        LEFT JOIN messages last_message ON last_message.id = last_entry.message_id
        LEFT JOIN project_memberships last_author
          ON last_author.project_id = last_message.project_id
         AND last_author.actor_id = last_message.author_actor_id
        LEFT JOIN LATERAL (
            SELECT string_agg(membership.display_name, ', ' ORDER BY lower(membership.display_name))
                       AS names
            FROM conversation_members member
            JOIN project_memberships membership
              ON membership.project_id = member.project_id
             AND membership.actor_id = member.actor_id
            WHERE member.conversation_id = COALESCE(
                    CASE WHEN parent.kind IN ('dm', 'group') THEN parent.id END,
                    conversation.id)
              AND member.actor_id <> $1
        ) members ON conversation.kind IN ('dm', 'group')
                  OR parent.kind IN ('dm', 'group')
        WHERE conversation.project_id = ANY($2)
          AND conversation.archived_at IS NULL
          AND (parent.id IS NULL OR parent.archived_at IS NULL)
          AND conversation_visible_to(conversation.id, $1)
          AND ($3::text[] IS NULL OR conversation.kind = ANY($3))
          AND ($4::uuid IS NULL OR EXISTS (
                SELECT 1 FROM conversation_members member
                WHERE member.conversation_id = COALESCE(
                        CASE WHEN parent.kind IN ('dm', 'group') THEN parent.id END,
                        conversation.id)
                  AND member.actor_id = $4))
          AND (NOT $5 OR conversation.next_entry_sequence - 1
                          > COALESCE(read_state.highest_seen_sequence, 0))
          AND ($6::text IS NULL OR (conversation.kind = 'thread'
                                    AND lower(conversation.thread_status) = lower($6)))
          AND ($7 = '' OR conversation.name ILIKE '%' || $7 || '%'
                       OR members.names ILIKE '%' || $7 || '%')
          AND ($8::uuid IS NULL OR conversation.id = $8)
        ORDER BY COALESCE(last_entry.created_at, conversation.created_at) DESC, conversation.id
        OFFSET $9
        LIMIT $10
        "#,
    )
    .bind(viewer)
    .bind(projects)
    .bind(filter.kinds)
    .bind(filter.with_person)
    .bind(filter.unread_only)
    .bind(
        filter
            .status
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
    )
    .bind(filter.query.trim())
    .bind(filter.only_id)
    .bind(offset)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?)
}

pub(super) fn summary(
    state: &AppState,
    _viewer: Uuid,
    row: ConversationRow,
) -> ConversationSummary {
    let is_thread = row.kind == "thread";
    let name = if row.kind == "dm" {
        row.member_names
            .clone()
            .unwrap_or_else(|| "Direct message".to_owned())
    } else {
        row.name.clone()
    };
    let direct = row.direct;
    ConversationSummary {
        id: row.id.to_string(),
        name,
        project_id: row.project_id.to_string(),
        project_name: if direct {
            "Messages".to_owned()
        } else {
            row.project_name
        },
        parent_id: row.parent_id.map(|id| id.to_string()),
        parent_name: row.parent_name.map(|name| {
            if row.parent_kind.as_deref() == Some("dm") {
                row.member_names.clone().unwrap_or(name)
            } else {
                name
            }
        }),
        members: if matches!(row.kind.as_str(), "group") {
            row.member_names.clone()
        } else {
            None
        },
        status: is_thread.then(|| row.thread_status.clone()),
        outcome: if is_thread { row.thread_outcome } else { None },
        unread_count: (row.highest_sequence - row.last_read_sequence).max(0),
        last_message: match (row.last_author, row.last_text) {
            (Some(author), Some(text)) => Some(LastMessage {
                author,
                text: snippet(&text, SNIPPET_CHARACTERS),
                removed: row.last_removed.unwrap_or(false),
                at: row.last_activity_at.clone(),
            }),
            _ => None,
        },
        url: public_url(
            state,
            &conversation_path(row.project_id, &row.kind, row.id, direct),
        ),
        kind: row.kind,
    }
}

async fn resolve_person_anywhere(
    state: &AppState,
    projects: &[Uuid],
    person: &str,
) -> Result<Option<Uuid>, AppError> {
    let person = person.trim();
    if let Ok(actor_id) = person.parse::<Uuid>() {
        return Ok(Some(actor_id));
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.actor_id
        FROM project_memberships membership
        WHERE membership.project_id = ANY($1) AND membership.enabled
          AND lower(membership.handle) = lower($2)
        LIMIT 1
        "#,
    )
    .bind(projects)
    .bind(person.trim_start_matches('@'))
    .fetch_optional(&state.pool)
    .await?)
}

// ---------------------------------------------------------------------------
// conversation_open and conversation_read

/// Load a conversation the caller can read, or explain why not.
async fn readable_conversation(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: &str,
) -> Result<Result<ConversationAccessRow, ToolResult<()>>, AppError> {
    if !principal.can_read() {
        return Ok(Err(ToolResult::denied("This connection can't read.")));
    }
    let Some(conversation_id) = parse_id(conversation_id) else {
        return Ok(Err(ToolResult::invalid(
            "conversationId",
            "Pass a conversation ID or link.",
        )));
    };
    let Some(row) =
        fetch_conversation_access(&state.pool, principal.actor_id, conversation_id).await?
    else {
        return Ok(Err(ToolResult::not_found("That conversation")));
    };
    if row.project_id != principal.project_id
        || authorize_conversation_row_for(
            principal.actor_id,
            principal.actor_kind,
            &row,
            Action::McpRead,
            false,
        )
        .is_err()
    {
        return Ok(Err(ToolResult::not_found("That conversation")));
    }
    Ok(Ok(row))
}

fn retype<T, U>(result: ToolResult<U>) -> ToolResult<T> {
    match result {
        ToolResult::Ok { .. } => ToolResult::unavailable(),
        ToolResult::NotFound { message } => ToolResult::NotFound { message },
        ToolResult::InvalidRequest { field, message } => {
            ToolResult::InvalidRequest { field, message }
        }
        ToolResult::PermissionDenied { message } => ToolResult::PermissionDenied { message },
        ToolResult::Conflict { message } => ToolResult::Conflict { message },
        ToolResult::TemporarilyUnavailable { message } => {
            ToolResult::TemporarilyUnavailable { message }
        }
    }
}

pub(super) async fn conversation_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationOpenArguments,
) -> ToolResult<ConversationOpenValue> {
    let Some(limit) = bounded(arguments.limit, DEFAULT_MESSAGES, MAX_MESSAGES) else {
        return ToolResult::invalid("limit", "Use a limit from 1 to 200.");
    };
    let result = async {
        let conversation =
            match readable_conversation(state, principal, &arguments.conversation_id).await? {
                Ok(row) => row,
                Err(result) => return Ok(retype(result)),
            };
        let highest = conversation.next_entry_sequence - 1;
        let last_read = read_sequence(state, conversation.id, principal.actor_id).await?;
        let from = (highest - limit + 1).max(1);
        let messages =
            message_items(state, conversation.id, principal.actor_id, from, highest).await?;
        let detail = conversation_detail(state, principal, &conversation).await?;
        let pinned = pinned_items(state, conversation.id, principal.actor_id).await?;
        let (open_threads, recently_settled_threads) = if conversation.kind == "thread" {
            (Vec::new(), Vec::new())
        } else {
            thread_lists(state, conversation.id, principal.actor_id).await?
        };
        let hint = thread_hint(&conversation, highest);
        if arguments.mark_read.unwrap_or(false) && principal.can_write() {
            mark(state, principal, &conversation, highest).await?;
        }
        Ok::<_, AppError>(ToolResult::ok(ConversationOpenValue {
            conversation: detail,
            pinned,
            highest_sequence: highest,
            last_read_sequence: last_read,
            unread_count: (highest - last_read).max(0),
            has_older: from > 1,
            messages,
            open_threads,
            recently_settled_threads,
            hint,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_open"))
}

pub(super) async fn conversation_read(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationReadArguments,
) -> ToolResult<ConversationReadValue> {
    let Some(limit) = bounded(arguments.limit, DEFAULT_MESSAGES, MAX_MESSAGES) else {
        return ToolResult::invalid("limit", "Use a limit from 1 to 200.");
    };
    let result = async {
        let conversation =
            match readable_conversation(state, principal, &arguments.conversation_id).await? {
                Ok(row) => row,
                Err(result) => return Ok(retype(result)),
            };
        let highest = conversation.next_entry_sequence - 1;
        let (from, to) = if let Some(around) = arguments.around_message_id.as_deref() {
            let Some(message_id) = parse_message_id(around) else {
                return Ok(ToolResult::invalid(
                    "aroundMessageId",
                    "Pass a message ID or a link to it.",
                ));
            };
            let Some(sequence) = sqlx::query_scalar::<_, i64>(
                "SELECT sequence FROM conversation_entries WHERE conversation_id = $1 AND message_id = $2",
            )
            .bind(conversation.id)
            .bind(message_id)
            .fetch_optional(&state.pool)
            .await?
            else {
                return Ok(ToolResult::not_found("That message in this conversation"));
            };
            ((sequence - limit / 2).max(1), (sequence + limit / 2).min(highest))
        } else if let Some(after) = arguments.after {
            ((after + 1).max(1), (after + limit).min(highest))
        } else if let Some(before) = arguments.before {
            ((before - limit).max(1), (before - 1).min(highest))
        } else {
            ((highest - limit + 1).max(1), highest)
        };
        let messages = if from <= to {
            message_items(state, conversation.id, principal.actor_id, from, to).await?
        } else {
            Vec::new()
        };
        if arguments.mark_read.unwrap_or(false) && principal.can_write() && to >= 1 {
            mark(state, principal, &conversation, to).await?;
        }
        let last_read = read_sequence(state, conversation.id, principal.actor_id).await?;
        Ok::<_, AppError>(ToolResult::ok(ConversationReadValue {
            conversation_id: conversation.id.to_string(),
            has_older: from > 1,
            has_newer: to < highest,
            messages,
            highest_sequence: highest,
            last_read_sequence: last_read,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_read"))
}

/// Entries `from..=to` of a conversation the viewer can see, with each
/// message's reactions as `viewer` sees them.
pub(super) async fn message_items(
    state: &AppState,
    conversation_id: Uuid,
    viewer: Uuid,
    from: i64,
    to: i64,
) -> Result<Vec<MessageItem>, AppError> {
    let rows = sqlx::query_as::<_, MessageRow>(
        r#"
        SELECT entry.sequence, entry.created_at::text AS at,
               message.id AS message_id, message.author_actor_id AS author_id,
               author.kind AS author_kind, membership.display_name AS author_name,
               membership.handle AS author_handle,
               CASE WHEN message.removed_at IS NULL THEN message.plain_text END AS text,
               via_client.client_name AS via, event.summary AS event_summary,
               message.revision, message.edited_at::text AS edited_at,
               message.removed_at IS NOT NULL AS removed,
               remover.display_name AS remover_name, remover.handle AS remover_handle,
               (SELECT jsonb_agg(jsonb_build_object(
                        'id', thread.id, 'name', thread.name, 'status', thread.thread_status)
                        ORDER BY thread.created_at)
                FROM conversations thread
                WHERE thread.origin_message_id = message.id AND thread.kind = 'thread'
                  AND thread.archived_at IS NULL) AS threads
        FROM conversation_entries entry
        LEFT JOIN messages message ON message.id = entry.message_id
        LEFT JOIN actors author ON author.id = message.author_actor_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = message.project_id
         AND membership.actor_id = message.author_actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = message.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        LEFT JOIN project_memberships remover
          ON remover.project_id = message.project_id
         AND remover.actor_id = message.removed_by_actor_id
        LEFT JOIN agent_run_events event ON event.id = entry.agent_run_event_id
        WHERE entry.conversation_id = $1 AND entry.sequence BETWEEN $2 AND $3
        ORDER BY entry.sequence
        "#,
    )
    .bind(conversation_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await?;
    let live = rows
        .iter()
        .filter(|row| !row.removed.unwrap_or(false))
        .filter_map(|row| row.message_id)
        .collect::<Vec<_>>();
    let mut reactions = reaction_rows(&state.pool, conversation_id, &live, viewer).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let summaries = row
                .message_id
                .and_then(|id| reactions.remove(&id))
                .unwrap_or_default();
            let mut item = message_item(row);
            item.reactions = reaction_summaries(summaries);
            item
        })
        .collect())
}

/// "Name (@handle)", with ", agent" for agents.
pub(super) fn person_label(name: &str, handle: &str, kind: Option<&str>) -> String {
    if kind == Some("external_agent") {
        format!("{name} (@{handle}, agent)")
    } else {
        format!("{name} (@{handle})")
    }
}

fn message_item(row: MessageRow) -> MessageItem {
    let author = match (&row.author_name, &row.author_handle) {
        (Some(name), Some(handle)) => person_label(name, handle, row.author_kind.as_deref()),
        _ if row.event_summary.is_some() => "Codex run".to_owned(),
        _ => "Former member".to_owned(),
    };
    let removed = row.removed.unwrap_or(false);
    let raw = if removed {
        String::new()
    } else {
        row.text.or(row.event_summary).unwrap_or_default()
    };
    let (text, truncated) = truncate_text(&raw, MESSAGE_CHARACTERS);
    MessageItem {
        sequence: row.sequence,
        message_id: row.message_id.map(|id| id.to_string()),
        author,
        author_id: row.author_id.map(|id| id.to_string()),
        text,
        truncated,
        sent_at: row.at,
        via: row.via,
        revision: row.revision.filter(|revision| *revision > 1),
        edited_at: row.edited_at,
        removed,
        removed_by: if removed {
            Some(match (row.remover_name, row.remover_handle) {
                (Some(name), Some(handle)) => format!("{name} (@{handle})"),
                _ => "Former member".to_owned(),
            })
        } else {
            None
        },
        threads: row
            .threads
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        reactions: Vec::new(),
    }
}

async fn read_sequence(
    state: &AppState,
    conversation_id: Uuid,
    actor_id: Uuid,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT highest_seen_sequence FROM conversation_read_states WHERE conversation_id = $1 AND actor_id = $2",
    )
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(0))
}

async fn conversation_detail(
    state: &AppState,
    principal: &McpPrincipal,
    conversation: &ConversationAccessRow,
) -> Result<ConversationDetail, AppError> {
    let rows = list_rows(
        state,
        principal.actor_id,
        &[conversation.project_id],
        ListFilter {
            kinds: None,
            with_person: None,
            unread_only: false,
            status: None,
            query: String::new(),
            only_id: Some(conversation.id),
        },
        0,
        1,
    )
    .await?;
    let row = rows
        .into_iter()
        .next()
        .ok_or(AppError::NotFound("conversation"))?;
    let members = if matches!(conversation.effective_kind(), "dm" | "group") {
        Some(
            sqlx::query_scalar::<_, String>(
                r#"
                SELECT membership.display_name || ' (@' || membership.handle || ')'
                FROM conversation_members member
                JOIN project_memberships membership
                  ON membership.project_id = member.project_id
                 AND membership.actor_id = member.actor_id
                WHERE member.conversation_id = $1
                ORDER BY lower(membership.display_name)
                "#,
            )
            .bind(conversation.membership_conversation_id())
            .fetch_all(&state.pool)
            .await?,
        )
    } else {
        None
    };
    let summary = summary(state, principal.actor_id, row);
    Ok(ConversationDetail {
        parent: match (
            &summary.parent_id,
            &summary.parent_name,
            &conversation.parent_kind,
        ) {
            (Some(id), Some(name), Some(kind)) => Some(ParentRef {
                id: id.clone(),
                kind: kind.clone(),
                name: name.clone(),
            }),
            _ => None,
        },
        id: summary.id,
        kind: summary.kind,
        name: summary.name,
        project_id: summary.project_id,
        url: summary.url,
        members,
        status: summary.status,
        outcome: summary.outcome,
    })
}

#[derive(Debug, FromRow)]
struct PinRow {
    kind: String,
    id: Uuid,
    title: String,
    text: String,
}

pub(super) async fn pinned_items(
    state: &AppState,
    conversation_id: Uuid,
    viewer: Uuid,
) -> Result<Vec<PinItem>, AppError> {
    let rows = sqlx::query_as::<_, PinRow>(
        r#"
        SELECT pin.target_kind AS kind, pin.target_id AS id,
               COALESCE(page.title, decision.title,
                        'Message from ' || COALESCE(author.display_name, 'someone')) AS title,
               COALESCE(page.body, decision.detail, message.plain_text, '') AS text
        FROM conversation_pins pin
        LEFT JOIN project_pages page
          ON pin.target_kind = 'page' AND page.id = pin.target_id AND page.archived_at IS NULL
        LEFT JOIN project_decisions decision
          ON pin.target_kind = 'decision' AND decision.id = pin.target_id
         AND decision.status = 'active'
         AND (decision.source_conversation_id IS NULL
              OR conversation_visible_to(decision.source_conversation_id, $2))
        LEFT JOIN messages message
          ON pin.target_kind = 'message' AND message.id = pin.target_id
         AND message.removed_at IS NULL
         AND conversation_visible_to(message.conversation_id, $2)
        LEFT JOIN project_memberships author
          ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
        WHERE pin.conversation_id = $1
          AND (page.id IS NOT NULL OR decision.id IS NOT NULL OR message.id IS NOT NULL)
        ORDER BY pin.created_at
        LIMIT 20
        "#,
    )
    .bind(conversation_id)
    .bind(viewer)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PinItem {
            kind: row.kind,
            id: row.id.to_string(),
            title: row.title,
            text: truncate_text(&row.text, 4_000).0,
        })
        .collect())
}

async fn thread_lists(
    state: &AppState,
    parent_id: Uuid,
    viewer: Uuid,
) -> Result<(Vec<ThreadItem>, Vec<ThreadItem>), AppError> {
    let threads =
        crate::conversations::thread_summaries(&state.pool, parent_id, viewer, None, false, 100)
            .await?;
    let settled = |status: &str| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "done" | "dropped" | "decided"
        )
    };
    let item = |thread: &crate::conversations::ThreadSummaryView| ThreadItem {
        id: thread.id.to_string(),
        name: thread.name.clone(),
        status: thread.status.clone(),
        outcome: thread.outcome.clone(),
        message_count: thread.message_count,
        unread: thread.unread,
        last_activity_at: thread.last_activity_at.clone(),
    };
    let open = threads
        .iter()
        .filter(|thread| !settled(&thread.status))
        .take(15)
        .map(item)
        .collect();
    let mut recent = threads
        .iter()
        .filter(|thread| settled(&thread.status) && thread.status_changed_at.is_some())
        .collect::<Vec<_>>();
    recent.sort_by(|left, right| right.status_changed_at.cmp(&left.status_changed_at));
    Ok((open, recent.into_iter().take(10).map(item).collect()))
}

fn thread_hint(conversation: &ConversationAccessRow, highest: i64) -> Option<String> {
    if conversation.kind == "thread"
        && conversation.thread_outcome.is_none()
        && highest >= LONG_THREAD_MESSAGES
    {
        Some(format!(
            "This thread has {highest} messages and no outcome. If it has landed somewhere, record it with thread_update (status and outcome) so others don't have to reread it."
        ))
    } else if conversation.kind != "thread" && highest >= 200 {
        Some(
            "This conversation is long. Start a thread (thread_start with fromMessageId) when a topic will run on, and search to find older discussions.".to_owned(),
        )
    } else {
        None
    }
}

async fn mark(
    state: &AppState,
    principal: &McpPrincipal,
    conversation: &ConversationAccessRow,
    up_to: i64,
) -> Result<i64, AppError> {
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query_as::<_, (i64, i64)>(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence, version
        )
        VALUES ($1, $2, $3, $4, 1)
        ON CONFLICT (project_id, conversation_id, actor_id) DO UPDATE
        SET highest_seen_sequence = EXCLUDED.highest_seen_sequence,
            version = conversation_read_states.version + 1,
            updated_at = now()
        WHERE EXCLUDED.highest_seen_sequence > conversation_read_states.highest_seen_sequence
        RETURNING highest_seen_sequence, version
        "#,
    )
    .bind(conversation.project_id)
    .bind(conversation.id)
    .bind(principal.actor_id)
    .bind(up_to)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some((highest_seen_sequence, version)) = row {
        enqueue_event(
            &mut transaction,
            conversation.project_id,
            OutboxEventPayload::ReadStateUpdated(ReadStateUpdatedPayload {
                conversation_id: conversation.id,
                actor_id: principal.actor_id,
                highest_seen_sequence,
                version,
            }),
        )
        .await?;
    }
    transaction.commit().await?;
    read_sequence(state, conversation.id, principal.actor_id).await
}

pub(super) async fn mark_read(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MarkReadArguments,
) -> ToolResult<ReadMark> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let result = async {
        let conversation =
            match readable_conversation(state, principal, &arguments.conversation_id).await? {
                Ok(row) => row,
                Err(result) => return Ok(retype(result)),
            };
        let highest = conversation.next_entry_sequence - 1;
        let up_to = arguments.up_to_sequence.unwrap_or(highest);
        if up_to < 0 || up_to > highest {
            return Ok(ToolResult::invalid(
                "upToSequence",
                format!("Use a sequence from 0 to {highest}."),
            ));
        }
        let last_read = mark(state, principal, &conversation, up_to).await?;
        Ok::<_, AppError>(ToolResult::ok(ReadMark {
            conversation_id: conversation.id.to_string(),
            last_read_sequence: last_read,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "mark_read"))
}

// ---------------------------------------------------------------------------
// message_send and conversation_start

pub(super) async fn message_send(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageSendArguments,
) -> ToolResult<SentMessage> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let content = match message_content(
        state,
        principal,
        arguments.text.as_deref(),
        arguments.content,
        "message_send",
    )
    .await
    {
        Ok(content) => content,
        Err(refused) => return refused,
    };
    send(
        state,
        principal,
        conversation_id,
        content,
        arguments.client_request_id,
    )
    .await
}

/// Message content from a tool's `text` or `content` argument (exactly one):
/// @handles of people in the project in text become mentions. The content is
/// validated when the message is written.
pub(super) async fn message_content<T>(
    state: &AppState,
    principal: &McpPrincipal,
    text: Option<&str>,
    content: Option<MessageContent>,
    operation: &'static str,
) -> Result<serde_json::Value, ToolResult<T>> {
    match (text, content) {
        (Some(text), None) => {
            let text = text.trim();
            if text.is_empty() || text.chars().count() > MAX_TEXT_CHARACTERS {
                return Err(ToolResult::invalid(
                    "text",
                    "Send between 1 and 20,000 characters.",
                ));
            }
            content_from_text(state, principal.project_id, text)
                .await
                .map_err(|error| ToolResult::from_error(error, operation))
        }
        (None, Some(content)) => serde_json::to_value(content)
            .map_err(|_| ToolResult::invalid("content", "The content isn't valid.")),
        _ => Err(ToolResult::invalid("text", "Pass either text or content.")),
    }
}

/// Who a message command acts as: an agent as itself (its credential is
/// checked against the conversation), a person as themselves, through the
/// app they connected. Anything else is refused with this message.
pub(super) fn message_actor(principal: &McpPrincipal) -> Result<MessageCommandActor, &'static str> {
    match principal.actor_kind {
        ActorKind::ExternalAgent => {
            let Some(connection_id) = principal.external_agent_connection_id else {
                return Err("This agent connection can't send messages.");
            };
            Ok(MessageCommandActor {
                actor_id: principal.actor_id,
                actor_kind: "external_agent".to_owned(),
                mcp_credential: Some(MessageMcpCredential {
                    credential_id: principal.credential_id,
                    connection_id,
                }),
                via_grant_id: None,
            })
        }
        ActorKind::Human => Ok(MessageCommandActor {
            actor_id: principal.actor_id,
            actor_kind: "human".to_owned(),
            mcp_credential: None,
            via_grant_id: principal.via_grant_id(),
        }),
        _ => Err("This connection can't send messages."),
    }
}

async fn send(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: Uuid,
    content: serde_json::Value,
    client_request_id: Option<String>,
) -> ToolResult<SentMessage> {
    let key = client_request_id.unwrap_or_else(|| Uuid::now_v7().to_string());
    let Ok(idempotency_key) = IdempotencyKey::parse(&key) else {
        return ToolResult::invalid(
            "clientRequestId",
            "Use letters, numbers, dashes or dots, up to 128 characters.",
        );
    };
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(message) => return ToolResult::denied(message),
    };
    match append_message_command(
        state,
        actor,
        conversation_id,
        None,
        idempotency_key,
        content,
        None,
    )
    .await
    {
        Ok(outcome) => ToolResult::ok(SentMessage {
            conversation_id: outcome.body.conversation_id.to_string(),
            message_id: outcome.body.message.id.to_string(),
            sequence: outcome.body.entry.sequence,
            text: snippet(&outcome.body.message.plain_text, SNIPPET_CHARACTERS),
            sent_at: outcome.body.message.created_at,
            idempotent_replay: outcome.idempotent_replay,
        }),
        Err(AppError::Conflict(..)) => ToolResult::conflict(
            "That clientRequestId was already used for a different message. Use a new one.",
        ),
        Err(AppError::NotFound(_)) => ToolResult::not_found("That conversation"),
        Err(error) => ToolResult::from_error(error, "message_send"),
    }
}

pub(super) async fn conversation_start(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationStartArguments,
) -> ToolResult<StartedConversation> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if arguments.people.is_empty() || arguments.people.len() > 19 {
        return ToolResult::invalid("people", "Name between 1 and 19 people.");
    }
    if arguments
        .text
        .as_deref()
        .is_some_and(|text| text.trim().chars().count() > MAX_TEXT_CHARACTERS)
    {
        return ToolResult::invalid("text", "Send between 1 and 20,000 characters.");
    }
    let name = match arguments
        .name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(name) => match normalize_conversation_name(name) {
            Ok(name) => Some(name),
            Err(_) => return ToolResult::invalid("name", "Use a name up to 120 characters."),
        },
        None => None,
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let (space_id, role) = crate::direct::space_for_actor(
            &mut transaction,
            principal.actor_id,
            principal.external_agent_connection_id,
        )
        .await?;
        let subject = crate::policy::Subject::new(
            principal.actor_id,
            principal.actor_kind,
            true,
            Some(crate::policy::Membership::project(space_id, role, true)),
            None,
        );
        if !matches!(
            crate::policy::authorize(
                &subject,
                Action::ChannelCreate,
                crate::policy::Resource::Project(crate::policy::ProjectScope::project(space_id)),
            ),
            crate::policy::Decision::Allow
        ) {
            return Ok(ToolResult::denied("You can't start conversations."));
        }
        let people = match resolve_people(state, space_id, &arguments.people).await? {
            Ok(people) => people
                .into_iter()
                .filter(|id| *id != principal.actor_id)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
            Err(person) => {
                return Ok(ToolResult::invalid(
                    "people",
                    format!("{person} isn't someone you can message. Try people_search."),
                ));
            }
        };
        if people.is_empty() {
            return Ok(ToolResult::invalid(
                "people",
                "Name someone other than yourself.",
            ));
        }
        let (conversation_id, created, kind) = if people.len() == 1 && name.is_none() {
            let (id, created) = crate::direct::open_dm_in(
                &mut transaction,
                space_id,
                principal.actor_id,
                people[0],
            )
            .await?;
            (id, created, OutboxConversationKind::Dm)
        } else {
            let id = crate::direct::create_group_in(
                &mut transaction,
                space_id,
                principal.actor_id,
                &people,
                name.clone(),
            )
            .await?;
            (id, true, OutboxConversationKind::Group)
        };
        if created {
            enqueue_event(
                &mut transaction,
                space_id,
                OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
                    conversation_id,
                    kind,
                    version: 1,
                }),
            )
            .await?;
            insert_audit(
                &mut transaction,
                principal,
                space_id,
                "conversation.mcp_started",
                "conversation",
                conversation_id,
                serde_json::json!({ "members": people.len() + 1 }),
            )
            .await?;
        }
        transaction.commit().await?;
        let message = match arguments
            .text
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(text) => {
                let content = content_from_text(state, space_id, text).await?;
                match send(state, principal, conversation_id, content, None).await {
                    ToolResult::Ok { value } => Some(value),
                    failed => return Ok(retype(failed)),
                }
            }
            None => None,
        };
        let row = list_rows(
            state,
            principal.actor_id,
            &[space_id],
            ListFilter {
                kinds: None,
                with_person: None,
                unread_only: false,
                status: None,
                query: String::new(),
                only_id: Some(conversation_id),
            },
            0,
            1,
        )
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound("conversation"))?;
        Ok::<_, AppError>(ToolResult::ok(StartedConversation {
            conversation: summary(state, principal.actor_id, row),
            created,
            message,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_start"))
}

// ---------------------------------------------------------------------------
// people_search

#[derive(Debug, FromRow)]
struct PersonRow {
    id: Uuid,
    name: String,
    handle: String,
    kind: String,
    projects: Vec<String>,
}

pub(super) async fn people_search(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PeopleSearchArguments,
) -> ToolResult<PeopleValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(limit) = bounded(arguments.limit, 50, 200) else {
        return ToolResult::invalid("limit", "Use a limit from 1 to 200.");
    };
    let query = arguments.query.unwrap_or_default();
    if query.chars().count() > 120 {
        return ToolResult::invalid("query", "Use a shorter search.");
    }
    let result = async {
        let projects = scope_projects(state, principal).await?;
        let rows = sqlx::query_as::<_, PersonRow>(
            r#"
            SELECT actor.id,
                   min(membership.display_name) AS name,
                   min(membership.handle) AS handle,
                   CASE WHEN actor.kind = 'external_agent' THEN 'agent' ELSE 'person' END AS kind,
                   array_agg(DISTINCT CASE WHEN project.kind = 'direct' THEN 'Messages'
                                           ELSE project.name END) AS projects
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            JOIN projects project ON project.id = membership.project_id
            WHERE membership.project_id = ANY($1) AND membership.enabled
              AND actor.status = 'active' AND actor.kind IN ('human', 'external_agent')
              AND ($2 = '' OR membership.display_name ILIKE '%' || $2 || '%'
                           OR membership.handle ILIKE '%' || $2 || '%')
            GROUP BY actor.id, actor.kind
            ORDER BY (actor.kind = 'external_agent') DESC, lower(min(membership.display_name))
            LIMIT $3
            "#,
        )
        .bind(&projects)
        .bind(query.trim().trim_start_matches('@'))
        .bind(limit)
        .fetch_all(&state.pool)
        .await?;
        Ok::<_, AppError>(ToolResult::ok(PeopleValue {
            people: rows
                .into_iter()
                .map(|row| Person {
                    id: row.id.to_string(),
                    name: row.name,
                    handle: row.handle,
                    kind: row.kind,
                    projects: row.projects,
                })
                .collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "people_search"))
}

// ---------------------------------------------------------------------------
// Threads

pub(super) async fn thread_start(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ThreadStartArguments,
) -> ToolResult<ThreadValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(parent_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass the channel, DM, or group ID.");
    };
    let origin_id = match arguments.from_message_id.as_deref() {
        Some(value) => match parse_message_id(value) {
            Some(id) => Some(id),
            None => {
                return ToolResult::invalid("fromMessageId", "Pass a message ID or a link to it.");
            }
        },
        None => None,
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let parent = match lock_conversation_for(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            parent_id,
            Action::ThreadCreate,
            false,
        )
        .await
        {
            Ok(parent) => parent,
            Err(AppError::NotFound(_)) => {
                return Ok(ToolResult::not_found("That conversation"));
            }
            Err(error) => return Err(error),
        };
        if parent.project_id != principal.project_id {
            return Ok(ToolResult::not_found("That conversation"));
        }
        if !matches!(parent.kind.as_str(), "channel" | "dm" | "group") {
            return Ok(ToolResult::invalid(
                "conversationId",
                "Threads go in a channel, DM, or group, not inside another thread.",
            ));
        }
        let origin_text = match origin_id {
            Some(origin_id) => {
                // Whether it was removed comes first: a removed message's
                // text is never read, and no new thread starts from it.
                let Some((removed, text)) = sqlx::query_as::<_, (bool, String)>(
                    r#"
                    SELECT removed_at IS NOT NULL,
                           CASE WHEN removed_at IS NULL THEN plain_text ELSE '' END
                    FROM messages
                    WHERE id = $1 AND conversation_id = $2
                    "#,
                )
                .bind(origin_id)
                .bind(parent.id)
                .fetch_optional(&mut *transaction)
                .await?
                else {
                    return Ok(ToolResult::not_found("That message in this conversation"));
                };
                if let Some((id, name, status, outcome)) =
                    sqlx::query_as::<_, (Uuid, String, String, Option<String>)>(
                        r#"
                        SELECT id, name, thread_status, thread_outcome FROM conversations
                        WHERE origin_message_id = $1 AND kind = 'thread' AND archived_at IS NULL
                        ORDER BY created_at LIMIT 1
                        "#,
                    )
                    .bind(origin_id)
                    .fetch_optional(&mut *transaction)
                    .await?
                {
                    transaction.commit().await?;
                    let message = post_optional(
                        state,
                        principal,
                        id,
                        arguments.text.as_deref(),
                        Some(origin_id),
                    )
                    .await?;
                    return Ok(ToolResult::ok(ThreadValue {
                        url: public_url(state, &thread_path(&parent, id)),
                        id: id.to_string(),
                        name,
                        status,
                        outcome,
                        parent_id: parent.id.to_string(),
                        created: Some(false),
                        message,
                    }));
                }
                if removed {
                    return Ok(ToolResult::invalid(
                        "fromMessageId",
                        "That message was removed. Start the thread without fromMessageId.",
                    ));
                }
                Some(text)
            }
            None => None,
        };
        let base_name = match arguments
            .name
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .or_else(|| origin_text.as_deref().map(suggested_name))
        {
            Some(name) if !name.is_empty() => name,
            _ => {
                return Ok(ToolResult::invalid(
                    "name",
                    "Give the thread a name, or start it from a message.",
                ));
            }
        };
        let base_name = match normalize_conversation_name(&base_name) {
            Ok(name) => name,
            Err(_) => return Ok(ToolResult::invalid("name", "Use a name up to 120 characters.")),
        };
        let name = unique_thread_name(&mut transaction, parent.id, &base_name).await?;
        let thread_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, origin_message_id, name)
            VALUES ($1, $2, 'thread', $3, $4, $5)
            "#,
        )
        .bind(thread_id)
        .bind(parent.project_id)
        .bind(parent.id)
        .bind(origin_id)
        .bind(&name)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversation_read_states (project_id, conversation_id, actor_id, highest_seen_sequence)
            VALUES ($1, $2, $3, 0)
            "#,
        )
        .bind(parent.project_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;
        enqueue_event(
            &mut transaction,
            parent.project_id,
            OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
                conversation_id: thread_id,
                kind: OutboxConversationKind::Thread,
                version: 1,
            }),
        )
        .await?;
        insert_audit(
            &mut transaction,
            principal,
            parent.project_id,
            "conversation.thread_created",
            "conversation",
            thread_id,
            serde_json::json!({ "parent_channel_id": parent.id }),
        )
        .await?;
        transaction.commit().await?;
        let message = post_optional(
            state,
            principal,
            thread_id,
            arguments.text.as_deref(),
            origin_id,
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(ThreadValue {
            url: public_url(state, &thread_path(&parent, thread_id)),
            id: thread_id.to_string(),
            name,
            status: THREAD_STATUS_SUGGESTIONS[0].to_owned(),
            outcome: None,
            parent_id: parent.id.to_string(),
            created: Some(true),
            message,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "thread_start"))
}

async fn post_optional(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: Uuid,
    text: Option<&str>,
    origin_id: Option<Uuid>,
) -> Result<Option<SentMessage>, AppError> {
    let Some(text) = text.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    // A thread started from a message is found again on retry, so its opening
    // message gets a key from that message and the text: retrying replays it
    // instead of posting it twice.
    let key = origin_id.map(|origin_id| {
        let digest = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes());
        format!("thread-start.{origin_id}.{}", hex::encode(&digest[..16]))
    });
    let content = content_from_text(state, principal.project_id, text).await?;
    Ok(
        match send(state, principal, conversation_id, content, key).await {
            ToolResult::Ok { value } => Some(value),
            _ => None,
        },
    )
}

fn thread_path(parent: &ConversationAccessRow, thread_id: Uuid) -> String {
    if matches!(parent.kind.as_str(), "dm" | "group") {
        format!("/messages/{}?thread={thread_id}", parent.id)
    } else {
        format!("/projects/{}/threads/{thread_id}", parent.project_id)
    }
}

async fn unique_thread_name(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
    base: &str,
) -> Result<String, AppError> {
    let taken = sqlx::query_scalar::<_, String>(
        r#"
        SELECT lower(name) FROM conversations
        WHERE parent_channel_id = $1 AND kind = 'thread' AND archived_at IS NULL
        "#,
    )
    .bind(parent_id)
    .fetch_all(&mut **transaction)
    .await?;
    if !taken.contains(&base.to_lowercase()) {
        return Ok(base.to_owned());
    }
    for number in 2..1000 {
        let suffix = format!(" ({number})");
        let stem = base
            .chars()
            .take(120 - suffix.chars().count())
            .collect::<String>();
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict(
        "conversation_name",
        "an active Conversation already uses that name",
    ))
}

pub(super) async fn thread_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ThreadUpdateArguments,
) -> ToolResult<ThreadValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(thread_id) = parse_id(&arguments.thread_id) else {
        return ToolResult::invalid("threadId", "Pass the thread's ID or link.");
    };
    let patch = ThreadPatch {
        name: arguments.name,
        status: arguments.status,
        outcome: arguments.outcome,
        clear_outcome: arguments.clear_outcome.unwrap_or(false),
    };
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let view = match update_thread(
            &mut transaction,
            principal.actor_id,
            principal.actor_kind,
            thread_id,
            &patch,
        )
        .await
        {
            Ok(view) => view,
            Err(AppError::Validation("thread change")) => {
                return Ok(ToolResult::invalid(
                    "status",
                    "Change at least one of name, status, or outcome.",
                ));
            }
            Err(AppError::Validation(field)) => {
                return Ok(ToolResult::invalid(
                    field,
                    "Statuses are up to 40 characters; outcomes up to 1,000.",
                ));
            }
            Err(error) => return Err(error),
        };
        insert_audit(
            &mut transaction,
            principal,
            view.project_id,
            "conversation.thread_updated",
            "conversation",
            thread_id,
            serde_json::json!({ "status": view.thread_status }),
        )
        .await?;
        transaction.commit().await?;
        let parent_id = view.parent_channel_id.unwrap_or(view.id);
        let url = if matches!(view.parent_kind.as_deref(), Some("dm" | "group")) {
            format!("/messages/{parent_id}?thread={}", view.id)
        } else {
            format!("/projects/{}/threads/{}", view.project_id, view.id)
        };
        Ok::<_, AppError>(ToolResult::ok(ThreadValue {
            id: view.id.to_string(),
            name: view.name,
            status: view.thread_status.unwrap_or_default(),
            outcome: view.thread_outcome,
            parent_id: parent_id.to_string(),
            url: public_url(state, &url),
            created: None,
            message: None,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "thread_update"))
}

// ---------------------------------------------------------------------------

fn bounded(limit: Option<u16>, default: i64, max: i64) -> Option<i64> {
    let limit = limit.map_or(default, i64::from);
    (1..=max).contains(&limit).then_some(limit)
}

/// Whether the caller may write in a Project (used before project-level writes).
pub(super) async fn can_write_in(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
    action: Action,
) -> Result<bool, AppError> {
    Ok(principal.can_write()
        && project_role(state, principal, project_id)
            .await?
            .is_some_and(|role| role_allows(principal, project_id, &role, action)))
}

#[cfg(test)]
mod tests {
    use super::suggested_name;

    #[test]
    fn suggested_thread_names_are_short_first_lines() {
        assert_eq!(suggested_name("Theming\nmore"), "Theming");
        let long = "word ".repeat(30);
        let name = suggested_name(&long);
        assert!(name.ends_with('…'));
        assert!(name.chars().count() <= 61);
    }
}
