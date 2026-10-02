//! Emoji reactions on messages (docs/plans/mcp-tools.md, Phase 3).
//!
//! A reaction is a message, an actor (person or agent) and one emoji from a
//! fixed list of 12. Every call states the result it wants (PUT or DELETE on
//! the web, `reacted: true|false` through MCP), so retries are safe without an
//! idempotency key. Anyone who can post in a conversation can react there.
//! Archived conversations and removed messages keep their reactions frozen,
//! and a removed message's reactions are hidden everywhere. Reactions change
//! nothing else: no timeline entry, unread count, mention, reply request or
//! notification.
//!
//! The web routes here and MCP's message_react share `set_reaction`, which
//! locks the conversation through `lock_message_conversation` (checked with
//! `conversation_visible_to`, and for agents their credential and connection),
//! the same lock and order as sending. Reads go through `reaction_rows`, which
//! checks `conversation_visible_to` too.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::put,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ActorView, ConversationAccessRow, MessageCommandActor, TimelineEntryView, enqueue_event,
        lock_message_conversation, parse_actor_kind,
    },
    error::{ApiPath, AppError},
    outbox::{MessageReactionsUpdatedPayload, OutboxEventPayload},
    policy::Action,
};

// The routes take no body; this is the cap every message route uses.
const MAX_HTTP_BODY_BYTES: usize = 131_072;

/// One emoji people can react with.
#[derive(Debug)]
pub(crate) struct ReactionChoice {
    /// Canonical, as stored.
    pub(crate) emoji: &'static str,
    /// ASCII, `[a-z0-9_]`, used in URLs and results.
    pub(crate) name: &'static str,
    aliases: &'static [&'static str],
}

/// The emoji people can react with. Must match the CHECK in migration 0048
/// and REACTION_CHOICES in the web app. ❤️ is U+2764 U+FE0F; every other
/// entry is a single code point.
pub(crate) const REACTIONS: [ReactionChoice; 12] = [
    ReactionChoice {
        emoji: "👍",
        name: "thumbsup",
        aliases: &["+1", "like"],
    },
    ReactionChoice {
        emoji: "👎",
        name: "thumbsdown",
        aliases: &["-1"],
    },
    ReactionChoice {
        emoji: "\u{2764}\u{FE0F}",
        name: "heart",
        aliases: &["love"],
    },
    ReactionChoice {
        emoji: "😄",
        name: "smile",
        aliases: &["laugh"],
    },
    ReactionChoice {
        emoji: "🎉",
        name: "tada",
        aliases: &["hooray"],
    },
    ReactionChoice {
        emoji: "😕",
        name: "confused",
        aliases: &[],
    },
    ReactionChoice {
        emoji: "🚀",
        name: "rocket",
        aliases: &[],
    },
    ReactionChoice {
        emoji: "👀",
        name: "eyes",
        aliases: &["looking"],
    },
    ReactionChoice {
        emoji: "✅",
        name: "check",
        aliases: &["white_check_mark", "done"],
    },
    ReactionChoice {
        emoji: "❌",
        name: "x",
        aliases: &["cross"],
    },
    ReactionChoice {
        emoji: "🙏",
        name: "pray",
        aliases: &["thanks"],
    },
    ReactionChoice {
        emoji: "🤔",
        name: "thinking",
        aliases: &[],
    },
];

/// The list as one line, for tool descriptions and error messages.
pub(crate) const REACTION_LIST: &str = "👍 👎 ❤️ 😄 🎉 😕 🚀 👀 ✅ ❌ 🙏 🤔";

fn is_ignored_modifier(character: char) -> bool {
    matches!(
        character,
        '\u{FE0E}' | '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}'
    )
}

/// An emoji, its name, an alias or `:name:`, in any case. Variation selectors
/// and skin tones are ignored, so "❤" and "👍🏽" resolve; anything else,
/// including several emoji, is None.
pub(crate) fn resolve(input: &str) -> Option<&'static ReactionChoice> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    let word = trimmed.trim_matches(':').to_lowercase();
    if let Some(choice) = REACTIONS
        .iter()
        .find(|choice| choice.name == word || choice.aliases.contains(&word.as_str()))
    {
        return Some(choice);
    }
    let bare = trimmed
        .chars()
        .filter(|character| !is_ignored_modifier(*character))
        .collect::<String>();
    REACTIONS.iter().find(|choice| {
        choice
            .emoji
            .chars()
            .filter(|character| !is_ignored_modifier(*character))
            .eq(bare.chars())
    })
}

/// The canonical name, as the web routes take it.
pub(crate) fn by_name(name: &str) -> Option<&'static ReactionChoice> {
    REACTIONS.iter().find(|choice| choice.name == name)
}

/// A stored emoji's entry.
pub(crate) fn by_emoji(emoji: &str) -> Option<&'static ReactionChoice> {
    REACTIONS.iter().find(|choice| choice.emoji == emoji)
}

/// One emoji on a message: how many reacted with it, whether the viewer did,
/// and the first 20 who did, earliest first.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionView {
    pub emoji: String,
    pub name: String,
    pub count: i64,
    pub by_viewer: bool,
    pub actors: Vec<ActorView>,
}

/// Why a visible message's reactions can't change. Invisible messages never
/// get this far: they are NotFound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReactionRefused {
    /// The caller's role can't post here (a Viewer, for example).
    RoleCannotReact,
    /// The conversation, or a thread's parent, is archived.
    Archived,
    /// The message was removed.
    Removed,
}

#[derive(Debug)]
pub(crate) enum ReactionError {
    Refused(ReactionRefused),
    App(AppError),
}

impl From<AppError> for ReactionError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for ReactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<ReactionError> for AppError {
    fn from(error: ReactionError) -> Self {
        match error {
            // The caller can see the conversation, so saying why is no leak;
            // the same as editing a message with a role that can't send.
            ReactionError::Refused(ReactionRefused::RoleCannotReact) => AppError::Forbidden,
            ReactionError::Refused(ReactionRefused::Archived) => AppError::Conflict(
                "conversation",
                "archived conversations can't change reactions",
            ),
            ReactionError::Refused(ReactionRefused::Removed) => {
                AppError::Conflict("message", "removed messages can't change reactions")
            }
            ReactionError::App(error) => error,
        }
    }
}

/// The outcome of `set_reaction`.
pub(crate) struct ReactionOutcome {
    pub(crate) conversation: ConversationAccessRow,
    /// False when the reaction was already that way.
    pub(crate) changed: bool,
}

/// Put `choice` on a message as `actor`, or take it away. The conversation is
/// locked first (FOR UPDATE through `lock_message_conversation`, which checks
/// `conversation_visible_to` and, for agents, their credential and
/// connection), the same lock and order as sending, so a removal or archive
/// and a reaction never interleave. A message the actor can't see, that isn't
/// in `conversation_hint`, or whose project isn't `project_hint`, is NotFound.
/// Writes an outbox event only when something changed.
pub(crate) async fn set_reaction(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    conversation_hint: Option<Uuid>,
    project_hint: Option<Uuid>,
    message_id: Uuid,
    choice: &'static ReactionChoice,
    reacted: bool,
) -> Result<ReactionOutcome, ReactionError> {
    let conversation_id = match conversation_hint {
        Some(conversation_id) => conversation_id,
        None => sqlx::query_scalar::<_, Uuid>("SELECT conversation_id FROM messages WHERE id = $1")
            .bind(message_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::NotFound("message"))?,
    };
    let conversation =
        match lock_message_conversation(transaction, actor, conversation_id, Action::ProjectView)
            .await
        {
            Ok(conversation) => conversation,
            Err(AppError::NotFound(_)) => return Err(AppError::NotFound("message").into()),
            Err(error) => return Err(error.into()),
        };
    if project_hint.is_some_and(|project_id| project_id != conversation.project_id) {
        return Err(AppError::NotFound("message").into());
    }
    let removed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT removed_at IS NOT NULL
        FROM messages
        WHERE id = $1 AND conversation_id = $2 AND project_id = $3
        "#,
    )
    .bind(message_id)
    .bind(conversation.id)
    .bind(conversation.project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("message"))?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived.into());
    }
    let actor_kind = parse_actor_kind(&actor.actor_kind)?;
    if !conversation.allows(actor.actor_id, actor_kind, Action::MessageSend)? {
        return Err(ReactionError::Refused(ReactionRefused::RoleCannotReact));
    }
    if conversation.is_archived() {
        return Err(ReactionError::Refused(ReactionRefused::Archived));
    }
    if removed {
        return Err(ReactionError::Refused(ReactionRefused::Removed));
    }
    let affected = if reacted {
        sqlx::query(
            r#"
            INSERT INTO message_reactions (project_id, conversation_id, message_id, actor_id, emoji)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (message_id, actor_id, emoji) DO NOTHING
            "#,
        )
        .bind(conversation.project_id)
        .bind(conversation.id)
        .bind(message_id)
        .bind(actor.actor_id)
        .bind(choice.emoji)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "DELETE FROM message_reactions WHERE message_id = $1 AND actor_id = $2 AND emoji = $3",
        )
        .bind(message_id)
        .bind(actor.actor_id)
        .bind(choice.emoji)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    };
    let changed = affected == 1;
    if changed {
        enqueue_event(
            transaction,
            conversation.project_id,
            OutboxEventPayload::MessageReactionsUpdated(MessageReactionsUpdatedPayload {
                conversation_id: conversation.id,
                message_id,
            }),
        )
        .await?;
    }
    Ok(ReactionOutcome {
        conversation,
        changed,
    })
}

#[derive(FromRow)]
struct ReactionRow {
    message_id: Uuid,
    emoji: String,
    count: i64,
    by_viewer: bool,
    actors: sqlx::types::Json<Vec<ActorView>>,
}

/// The reactions on these messages of one conversation, as `viewer` sees
/// them: nothing unless the viewer can see the conversation, and nothing for
/// removed messages. Each message's emoji are in the order first added, each
/// with its full count and the first 20 who reacted.
pub(crate) async fn reaction_rows<'e, E>(
    executor: E,
    conversation_id: Uuid,
    message_ids: &[Uuid],
    viewer: Uuid,
) -> Result<HashMap<Uuid, Vec<ReactionView>>, AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let mut by_message = HashMap::<Uuid, Vec<ReactionView>>::new();
    if message_ids.is_empty() {
        return Ok(by_message);
    }
    let rows = sqlx::query_as::<_, ReactionRow>(
        r#"
        SELECT reaction.message_id, reaction.emoji, count(*) AS count,
               bool_or(reaction.actor_id = $3) AS by_viewer,
               to_jsonb((array_agg(jsonb_build_object(
                   'id', actor.id,
                   'kind', actor.kind,
                   'displayName', COALESCE(membership.display_name, 'Former member'),
                   'handle', COALESCE(membership.handle, 'former-member'),
                   'avatarUrl', NULL
               ) ORDER BY reaction.created_at, reaction.actor_id))[1:20]) AS actors
        FROM message_reactions reaction
        JOIN messages message
          ON message.id = reaction.message_id
         AND message.conversation_id = reaction.conversation_id
        JOIN actors actor ON actor.id = reaction.actor_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = reaction.project_id
         AND membership.actor_id = reaction.actor_id
        WHERE reaction.conversation_id = $1
          AND reaction.message_id = ANY($2)
          AND message.removed_at IS NULL
          AND conversation_visible_to($1, $3)
        GROUP BY reaction.message_id, reaction.emoji
        ORDER BY reaction.message_id, min(reaction.created_at), reaction.emoji
        "#,
    )
    .bind(conversation_id)
    .bind(message_ids)
    .bind(viewer)
    .fetch_all(executor)
    .await?;
    for row in rows {
        // The CHECK in migration 0048 keeps every stored emoji on the list.
        let Some(choice) = by_emoji(&row.emoji) else {
            continue;
        };
        by_message
            .entry(row.message_id)
            .or_default()
            .push(ReactionView {
                emoji: choice.emoji.to_owned(),
                name: choice.name.to_owned(),
                count: row.count,
                by_viewer: row.by_viewer,
                actors: row.actors.0,
            });
    }
    Ok(by_message)
}

/// Fill in the reactions of these timeline entries of one conversation, as
/// `viewer` sees them. Removed messages get none.
pub(crate) async fn attach_reactions<'e, E>(
    executor: E,
    conversation_id: Uuid,
    entries: &mut [TimelineEntryView],
    viewer: Uuid,
) -> Result<(), AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    let message_ids = entries
        .iter()
        .filter_map(|entry| entry.message.as_ref())
        .filter(|message| message.removed.is_none())
        .map(|message| message.id)
        .collect::<Vec<_>>();
    let mut by_message = reaction_rows(executor, conversation_id, &message_ids, viewer).await?;
    for message in entries
        .iter_mut()
        .filter_map(|entry| entry.message.as_mut())
    {
        message.reactions = by_message.remove(&message.id).unwrap_or_default();
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/conversations/{id}/messages/{message_id}/reactions/{name}",
            put(add_reaction).delete(remove_reaction),
        )
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
}

/// Every reaction on the message after the change, as the caller sees them.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageReactionsResponse {
    pub conversation_id: Uuid,
    pub message_id: Uuid,
    /// False when the reaction was already that way.
    pub changed: bool,
    pub reactions: Vec<ReactionView>,
}

async fn add_reaction(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id, name)): ApiPath<(Uuid, Uuid, String)>,
) -> Result<Json<MessageReactionsResponse>, AppError> {
    change_reaction(
        &state,
        &headers,
        &principal,
        conversation_id,
        message_id,
        &name,
        true,
    )
    .await
}

async fn remove_reaction(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, message_id, name)): ApiPath<(Uuid, Uuid, String)>,
) -> Result<Json<MessageReactionsResponse>, AppError> {
    change_reaction(
        &state,
        &headers,
        &principal,
        conversation_id,
        message_id,
        &name,
        false,
    )
    .await
}

async fn change_reaction(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    message_id: Uuid,
    name: &str,
    reacted: bool,
) -> Result<Json<MessageReactionsResponse>, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let choice = by_name(name).ok_or(AppError::Validation("reaction"))?;
    let actor = MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
    };
    let mut transaction = state.pool.begin().await?;
    let outcome = set_reaction(
        &mut transaction,
        &actor,
        Some(conversation_id),
        None,
        message_id,
        choice,
        reacted,
    )
    .await?;
    transaction.commit().await?;
    // The web doesn't audit reactions, just as it doesn't audit sends.
    let mut by_message = reaction_rows(
        &state.pool,
        outcome.conversation.id,
        &[message_id],
        principal.actor_id,
    )
    .await?;
    Ok(Json(MessageReactionsResponse {
        conversation_id: outcome.conversation.id,
        message_id,
        changed: outcome.changed,
        reactions: by_message.remove(&message_id).unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{REACTION_LIST, REACTIONS, by_emoji, by_name, resolve};

    fn name_of(input: &str) -> Option<&'static str> {
        resolve(input).map(|choice| choice.name)
    }

    #[test]
    fn resolve_takes_emoji_names_aliases_and_colons() {
        assert_eq!(name_of("eyes"), Some("eyes"));
        assert_eq!(name_of(":EYES:"), Some("eyes"));
        assert_eq!(name_of("  Check "), Some("check"));
        assert_eq!(name_of("white_check_mark"), Some("check"));
        assert_eq!(name_of("+1"), Some("thumbsup"));
        assert_eq!(name_of("-1"), Some("thumbsdown"));
        assert_eq!(name_of("✅"), Some("check"));
        assert_eq!(name_of("👀"), Some("eyes"));
        // Without U+FE0F, with U+FE0E, and with a skin tone.
        assert_eq!(name_of("\u{2764}"), Some("heart"));
        assert_eq!(name_of("\u{2764}\u{FE0E}"), Some("heart"));
        assert_eq!(name_of("❤️"), Some("heart"));
        assert_eq!(name_of("👍🏽"), Some("thumbsup"));
        assert_eq!(name_of("🙏🏿"), Some("pray"));
    }

    #[test]
    fn resolve_refuses_anything_else() {
        for input in [
            "",
            "   ",
            "♥",
            "🦄",
            "👀✅",
            "eyes eyes",
            "::",
            "🏽",
            &"x".repeat(65),
        ] {
            assert_eq!(name_of(input), None, "{input:?}");
        }
    }

    #[test]
    fn the_list_is_consistent() {
        let emoji = REACTIONS
            .iter()
            .map(|choice| choice.emoji)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(emoji, REACTION_LIST);
        for choice in &REACTIONS {
            assert!(
                choice
                    .name
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '_')
            );
            assert_eq!(
                by_name(choice.name).map(|found| found.emoji),
                Some(choice.emoji)
            );
            assert_eq!(
                by_emoji(choice.emoji).map(|found| found.name),
                Some(choice.name)
            );
            assert_eq!(
                resolve(choice.emoji).map(|found| found.name),
                Some(choice.name)
            );
            for alias in choice.aliases {
                assert_eq!(resolve(alias).map(|found| found.name), Some(choice.name));
            }
        }
        assert!(by_name("EYES").is_none());
    }
}
