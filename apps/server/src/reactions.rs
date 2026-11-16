//! Emoji reactions on messages (docs/plans/mcp-tools.md, Phase 3).
//!
//! A reaction is a message, an actor (person or agent) and one emoji: any
//! standard emoji on the shared list (`reactions/emoji.json`, which the web's
//! picker reads too; the quick 12 come first), or one of the workspace's own
//! (`custom_emoji`, C2.19). Every call states the result it wants (PUT or DELETE on
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

use std::{collections::HashMap, sync::LazyLock};

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

/// The quick 12, which lead the shared list in this order, as the web's
/// REACTION_CHOICES does (a test holds the list to them). ❤️ is U+2764 U+FE0F; every other
/// entry is a single code point.
#[cfg(test)]
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

/// Every standard emoji people can react with (C2.19): the shared list,
/// read once, the quick 12 first. A parity test holds the web to the same
/// file.
static ALL: LazyLock<Vec<ReactionChoice>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Entry {
        emoji: &'static str,
        name: &'static str,
        #[serde(default)]
        aliases: Vec<&'static str>,
    }
    serde_json::from_str::<Vec<Entry>>(EMOJI_LIST)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| ReactionChoice {
            emoji: entry.emoji,
            name: entry.name,
            aliases: Vec::leak(entry.aliases),
        })
        .collect()
});

/// The shared emoji list, as JSON.
pub(crate) const EMOJI_LIST: &str = include_str!("reactions/emoji.json");

fn all() -> &'static [ReactionChoice] {
    &ALL
}

/// The list as one line, for tool descriptions and error messages.
pub(crate) const REACTION_LIST: &str = "👍 👎 ❤️ 😄 🎉 😕 🚀 👀 ✅ ❌ 🙏 🤔";

fn is_ignored_modifier(character: char) -> bool {
    matches!(
        character,
        '\u{FE0E}' | '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}'
    )
}

/// A standard emoji, its name, an alias or `:name:`, in any case. Variation
/// selectors and skin tones are ignored, so "❤" and "👍🏽" resolve; anything
/// else, including several emoji, is None.
pub(crate) fn resolve(input: &str) -> Option<&'static ReactionChoice> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    let word = trimmed.trim_matches(':').to_lowercase();
    if let Some(choice) = all()
        .iter()
        .find(|choice| choice.name == word || choice.aliases.contains(&word.as_str()))
    {
        return Some(choice);
    }
    let bare = trimmed
        .chars()
        .filter(|character| !is_ignored_modifier(*character))
        .collect::<String>();
    all().iter().find(|choice| {
        choice
            .emoji
            .chars()
            .filter(|character| !is_ignored_modifier(*character))
            .eq(bare.chars())
    })
}

/// The canonical name.
#[cfg(test)]
pub(crate) fn by_name(name: &str) -> Option<&'static ReactionChoice> {
    all().iter().find(|choice| choice.name == name)
}

/// A stored emoji's entry.
pub(crate) fn by_emoji(emoji: &str) -> Option<&'static ReactionChoice> {
    all().iter().find(|choice| choice.emoji == emoji)
}

/// A custom emoji's name as written: `:party_parrot:` or `party_parrot`.
pub(crate) fn custom_name(input: &str) -> Option<String> {
    let word = input.trim().trim_matches(':').to_lowercase();
    ((2..=32).contains(&word.len())
        && word.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        }))
    .then_some(word)
}

/// Whether `input` could name a reaction at all, before any lookup.
pub(crate) fn could_be_reaction(input: &str) -> bool {
    resolve(input).is_some() || custom_name(input).is_some()
}

/// The reaction a call named, for a message in `project_id`: a standard
/// emoji, or a custom one of that project's workspace. Taking a reaction
/// away finds a retired custom emoji too.
#[derive(Clone, Debug)]
pub(crate) struct Picked {
    /// As stored: the emoji, or `:name:` for a custom one.
    pub(crate) emoji: String,
    pub(crate) name: String,
    pub(crate) custom_emoji_id: Option<Uuid>,
}

async fn pick(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    input: &str,
    adding: bool,
) -> Result<Option<Picked>, AppError> {
    if let Some(choice) = resolve(input) {
        return Ok(Some(Picked {
            emoji: choice.emoji.to_owned(),
            name: choice.name.to_owned(),
            custom_emoji_id: None,
        }));
    }
    let Some(name) = custom_name(input) else {
        return Ok(None);
    };
    let found = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT emoji.id
        FROM custom_emojis emoji
        JOIN projects project ON project.organization_id = emoji.workspace_id
        WHERE project.id = $1 AND emoji.name = $2
          AND ($3 OR emoji.retired_at IS NULL)
        ORDER BY emoji.retired_at IS NULL DESC, emoji.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(&name)
    .bind(!adding)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(found.map(|id| Picked {
        emoji: format!(":{name}:"),
        name,
        custom_emoji_id: Some(id),
    }))
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
    /// A workspace's own emoji: its image, shown in place of `emoji`. A
    /// retired one still shows on the reactions that use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_emoji_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
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
    /// The reaction as stored.
    pub(crate) picked: Picked,
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
    reaction: &str,
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
    let picked = pick(transaction, conversation.project_id, reaction, reacted)
        .await?
        .ok_or(AppError::Validation("reaction"))?;
    let affected = if reacted {
        sqlx::query(
            r#"
            INSERT INTO message_reactions (
                project_id, conversation_id, message_id, actor_id, emoji, custom_emoji_id
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (message_id, actor_id, emoji) DO NOTHING
            "#,
        )
        .bind(conversation.project_id)
        .bind(conversation.id)
        .bind(message_id)
        .bind(actor.actor_id)
        .bind(&picked.emoji)
        .bind(picked.custom_emoji_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "DELETE FROM message_reactions WHERE message_id = $1 AND actor_id = $2 AND emoji = $3",
        )
        .bind(message_id)
        .bind(actor.actor_id)
        .bind(&picked.emoji)
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
        picked,
    })
}

#[derive(FromRow)]
struct ReactionRow {
    message_id: Uuid,
    emoji: String,
    custom_emoji_id: Option<Uuid>,
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
        SELECT reaction.message_id, reaction.emoji, reaction.custom_emoji_id, count(*) AS count,
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
        GROUP BY reaction.message_id, reaction.emoji, reaction.custom_emoji_id
        ORDER BY reaction.message_id, min(reaction.created_at), reaction.emoji
        "#,
    )
    .bind(conversation_id)
    .bind(message_ids)
    .bind(viewer)
    .fetch_all(executor)
    .await?;
    for row in rows {
        // A custom emoji shows by its image; a standard one is on the list
        // (`pick` stores nothing else).
        let (name, image_url) = match row.custom_emoji_id {
            Some(id) => (
                row.emoji.trim_matches(':').to_owned(),
                Some(format!("/api/v1/custom-emoji/{id}")),
            ),
            None => match by_emoji(&row.emoji) {
                Some(choice) => (choice.name.to_owned(), None),
                None => continue,
            },
        };
        by_message
            .entry(row.message_id)
            .or_default()
            .push(ReactionView {
                emoji: row.emoji,
                name,
                count: row.count,
                by_viewer: row.by_viewer,
                actors: row.actors.0,
                custom_emoji_id: row.custom_emoji_id,
                image_url,
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
    if !could_be_reaction(name) {
        return Err(AppError::Validation("reaction"));
    }
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
        name,
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
    use super::{EMOJI_LIST, REACTION_LIST, REACTIONS, all, by_emoji, by_name, resolve};

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
        for input in ["", "   ", "👀✅", "eyes eyes", "::", "🏽", &"x".repeat(65)] {
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

    /// C2.19: the shared list reads, every name and emoji is there once, the
    /// quick 12 lead it in order, and any emoji on it resolves.
    #[test]
    fn the_shared_list_holds_any_emoji_once() {
        assert!(all().len() > 1000, "{}", all().len());
        let mut names = std::collections::HashSet::new();
        let mut emoji = std::collections::HashSet::new();
        for choice in all() {
            assert!(names.insert(choice.name), "{} twice", choice.name);
            assert!(emoji.insert(choice.emoji), "{} twice", choice.emoji);
            assert!(
                choice
                    .name
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '_'),
                "{}",
                choice.name
            );
            assert_eq!(
                resolve(choice.emoji).map(|found| found.name),
                Some(choice.name)
            );
        }
        for (quick, listed) in REACTIONS.iter().zip(all()) {
            assert_eq!((quick.emoji, quick.name), (listed.emoji, listed.name));
        }
        assert_eq!(resolve("🦄").map(|found| found.name), Some("unicorn_face"));
        assert_eq!(
            resolve(":unicorn_face:").map(|found| found.emoji),
            Some("🦄")
        );
        assert!(EMOJI_LIST.starts_with('['));
        for refused in ["<script>", "a b", "🦄🦄", "\u{7}"] {
            assert!(resolve(refused).is_none(), "{refused}");
        }
    }
}
