//! message_react (docs/plans/mcp-tools.md, Phase 3). The command is the
//! web's: `crate::reactions::set_reaction` locks the conversation (checked
//! with `conversation_visible_to`, and for agents their credential and
//! connection) and adds or takes away one reaction. It states a result
//! rather than toggling, so retries are safe without a clientRequestId.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_message_id},
    talk::{message_actor, person_label},
};
use crate::{
    AppState,
    error::AppError,
    reactions::{
        REACTION_LIST, ReactionError, ReactionRefused, ReactionView, reaction_rows, resolve,
        set_reaction,
    },
};

/// At most this many names per emoji; `count` is always the full number.
const LISTED_PEOPLE: usize = 10;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageReactArguments {
    /// The message: its ID, or a link to it.
    pub message_id: String,
    /// One emoji, or its name: 👍 thumbsup, 👎 thumbsdown, ❤️ heart, 😄 smile,
    /// 🎉 tada, 😕 confused, 🚀 rocket, 👀 eyes, ✅ check, ❌ x, 🙏 pray,
    /// 🤔 thinking.
    pub emoji: String,
    /// False to take your reaction away. Defaults to true.
    pub reacted: Option<bool>,
}

/// One emoji on a message.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReactionSummary {
    /// The emoji, such as ✅.
    pub emoji: String,
    /// Its name, such as check. message_react takes either.
    pub name: String,
    pub count: i64,
    /// You reacted with this.
    pub by_you: bool,
    /// Who reacted, earliest first: "Name (@handle)", with ", agent" for
    /// agents. At most 10.
    pub people: Vec<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageReactionValue {
    pub conversation_id: String,
    pub message_id: String,
    pub emoji: String,
    pub name: String,
    /// Whether your reaction is on the message now.
    pub reacted: bool,
    /// False when it was already that way, so nothing changed.
    pub changed: bool,
    /// Every reaction on the message now, in the order first added.
    pub reactions: Vec<ReactionSummary>,
}

/// Reactions as conversation_open, conversation_read and message_react show
/// them.
pub(super) fn reaction_summaries(reactions: Vec<ReactionView>) -> Vec<ReactionSummary> {
    reactions
        .into_iter()
        .map(|reaction| ReactionSummary {
            emoji: reaction.emoji,
            name: reaction.name,
            count: reaction.count,
            by_you: reaction.by_viewer,
            people: reaction
                .actors
                .iter()
                .take(LISTED_PEOPLE)
                .map(|actor| {
                    person_label(
                        &actor.display_name,
                        &actor.handle,
                        Some(actor.kind.as_str()),
                    )
                })
                .collect(),
        })
        .collect()
}

fn archived() -> String {
    "That conversation is archived, so its reactions can't change. Restore it with conversation_update first.".to_owned()
}

pub(super) async fn message_react(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageReactArguments,
) -> ToolResult<MessageReactionValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(_) => return ToolResult::denied("This connection can't react to messages."),
    };
    let Some(message_id) = parse_message_id(&arguments.message_id) else {
        return ToolResult::invalid("messageId", "Pass a message ID, or a link to the message.");
    };
    let Some(choice) = resolve(&arguments.emoji) else {
        return ToolResult::invalid(
            "emoji",
            format!("Use one emoji from {REACTION_LIST}, or its name, such as eyes or check."),
        );
    };
    let reacted = arguments.reacted.unwrap_or(true);
    let result = async {
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let outcome = match set_reaction(
            &mut transaction,
            &actor,
            None,
            Some(principal.project_id),
            message_id,
            choice,
            reacted,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(ReactionError::App(AppError::NotFound(_))) => {
                return Ok(ToolResult::not_found("That message"));
            }
            Err(ReactionError::App(AppError::ProjectArchived)) => {
                return Ok(ToolResult::conflict(
                    "That project is archived. Restore it first.",
                ));
            }
            Err(ReactionError::App(error)) => return Err(error),
            Err(ReactionError::Refused(ReactionRefused::RoleCannotReact)) => {
                return Ok(ToolResult::denied(
                    "You can read this conversation but can't react in it. A project owner can give you the member role.",
                ));
            }
            Err(ReactionError::Refused(ReactionRefused::Archived)) => {
                return Ok(ToolResult::conflict(archived()));
            }
            Err(ReactionError::Refused(ReactionRefused::Removed)) => {
                return Ok(ToolResult::conflict(
                    "That message was removed, so its reactions can't change.",
                ));
            }
        };
        let conversation_id = outcome.conversation.id;
        if outcome.changed {
            insert_audit(
                &mut transaction,
                principal,
                outcome.conversation.project_id,
                if reacted {
                    "conversation.message_reacted"
                } else {
                    "conversation.message_unreacted"
                },
                "message",
                message_id,
                serde_json::json!({
                    "conversation_id": conversation_id,
                    "reaction": choice.name,
                }),
            )
            .await?;
        }
        transaction.commit().await?;
        let mut by_message =
            reaction_rows(&state.pool, conversation_id, &[message_id], principal.actor_id).await?;
        Ok::<_, AppError>(ToolResult::ok(MessageReactionValue {
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            emoji: choice.emoji.to_owned(),
            name: choice.name.to_owned(),
            reacted,
            changed: outcome.changed,
            reactions: reaction_summaries(by_message.remove(&message_id).unwrap_or_default()),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "message_react"))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::reaction_summaries;
    use crate::{conversations::ActorView, reactions::ReactionView};

    #[test]
    fn summaries_name_at_most_ten_people_and_mark_agents() {
        let actors = (0..12)
            .map(|index| ActorView {
                id: Uuid::from_u128(index),
                kind: if index == 0 {
                    "external_agent"
                } else {
                    "human"
                }
                .to_owned(),
                display_name: format!("P{index}"),
                handle: format!("p{index}"),
                avatar_url: None,
            })
            .collect::<Vec<_>>();
        let summaries = reaction_summaries(vec![ReactionView {
            emoji: "✅".to_owned(),
            name: "check".to_owned(),
            count: 12,
            by_viewer: true,
            actors,
        }]);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].count, 12);
        assert!(summaries[0].by_you);
        assert_eq!(summaries[0].people.len(), 10);
        assert_eq!(summaries[0].people[0], "P0 (@p0, agent)");
        assert_eq!(summaries[0].people[1], "P1 (@p1)");
    }
}
