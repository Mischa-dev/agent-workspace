//! conversation_members and conversation_continue (docs/plans/mcp-tools.md,
//! Phase 3). The commands are the web's: `direct::change_group_members` and
//! `replies::continue_conversation_in` lock the conversation through
//! `lock_conversation_for` (`conversation_visible_to`); these tools also
//! check the caller can still reach its project (`project_role`, which goes
//! through `actor_can_access_project`).
//!
//! People can add and remove anyone in groups they're in; agents can only
//! leave, because whoever is added reads the whole history. Only people can
//! let agents continue.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, insert_audit, link_url, mark_via, parse_id, project_role,
        resolve_conversation_members, resolve_direct_people,
    },
};
use crate::{
    AppState,
    conversations::{ActorView, ConversationAccessRow, fetch_conversation_access},
    direct::{GroupEditor, MAX_GROUP_MEMBERS, change_group_members},
    error::AppError,
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    replies::continue_conversation_in,
};

/// At most this many people in one add or remove list: a group holds 20,
/// one of them the caller.
const MAX_CHANGED: usize = MAX_GROUP_MEMBERS - 1;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberRef {
    pub id: String,
    pub name: String,
    pub handle: String,
    /// person or agent
    pub kind: String,
}

impl From<ActorView> for MemberRef {
    fn from(actor: ActorView) -> Self {
        let agent = actor.kind == "external_agent"; // kind-check: mechanical (a display label)
        Self {
            id: actor.id.to_string(),
            name: actor.display_name,
            handle: actor.handle,
            kind: if agent { "agent" } else { "person" }.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConversationMembersArguments {
    /// The group, or a thread in it: an ID or link.
    pub conversation_id: String,
    /// People and agents to add: actor IDs or @handles. They can read the
    /// group's whole history.
    pub add: Option<Vec<String>>,
    /// People and agents to remove: actor IDs or @handles. Your own to leave.
    pub remove: Option<Vec<String>>,
    /// The group's version you last saw. Leave it out to change the current
    /// one.
    pub expected_version: Option<i64>,
    /// A stable ID for this change, so a retry never repeats it after someone
    /// else changed the group.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupMembersValue {
    /// Always the group, even when you passed a thread in it.
    pub conversation_id: String,
    pub name: String,
    /// Pass as expectedVersion next time.
    pub version: i64,
    /// Everyone in the group now. Empty when you left.
    pub members: Vec<MemberRef>,
    /// Who was actually added; empty when they were all members already.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<MemberRef>,
    /// Who was actually removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<MemberRef>,
    /// You removed yourself and can no longer see this group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub left: bool,
    pub url: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConversationContinueArguments {
    /// The group, or a thread in one: an ID or link.
    pub conversation_id: String,
    /// A stable ID for this call, so a retry doesn't grant another round.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContinueValue {
    pub conversation_id: String,
    /// Agent messages allowed before a person has to continue again.
    pub agent_turns_remaining: i32,
    /// Agents now asked to answer the newest message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked: Vec<MemberRef>,
    /// The newest message, the one the agents were asked about if an agent
    /// wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

const ONLY_LEAVE: &str = "You can leave this group, but your role doesn't let you add or remove others (conversations.members), because whoever is added can read the whole history. Ask someone in the group whose role does.";

/// A conversation the caller can see in the project this call is scoped to,
/// and can still reach through this connection.
async fn visible_conversation(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: Uuid,
) -> Result<Option<ConversationAccessRow>, AppError> {
    let Some(row) =
        fetch_conversation_access(&state.pool, principal.actor_id, conversation_id).await?
    else {
        return Ok(None);
    };
    if row.place() != principal.project_id {
        return Ok(None);
    }
    // A DM or group is reached by seeing it; a space's, through the
    // caller's place there.
    if let Some(space) = row.project_id
        && project_role(state, principal, space).await?.is_none()
    {
        return Ok(None);
    }
    Ok(Some(row))
}

pub(crate) async fn conversation_members(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationMembersArguments,
) -> ToolResult<GroupMembersValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let add = arguments.add.unwrap_or_default();
    let remove = arguments.remove.unwrap_or_default();
    if add.is_empty() && remove.is_empty() {
        return ToolResult::invalid(
            "add",
            "Say who to add or remove: add and remove take actor IDs or @handles.",
        );
    }
    if add.len() > MAX_CHANGED || remove.len() > MAX_CHANGED {
        return ToolResult::invalid(
            if add.len() > MAX_CHANGED {
                "add"
            } else {
                "remove"
            },
            "Change at most 19 people at a time.",
        );
    }
    let key = match arguments
        .client_request_id
        .as_deref()
        .map(IdempotencyKey::parse)
    {
        None => None,
        Some(Ok(key)) => Some(key),
        Some(Err(_)) => {
            return ToolResult::invalid(
                "clientRequestId",
                "Use letters, numbers, dashes or dots, up to 128 characters.",
            );
        }
    };
    let result = async {
        let Some(row) = visible_conversation(state, principal, conversation_id).await? else {
            return Ok(ToolResult::not_found("That conversation"));
        };
        // Private channels have members too (K4.11); a thread under one
        // follows it.
        let channel = match (row.kind.as_str(), row.parent_kind.as_deref()) {
            ("channel", _) => Some(row.id),
            ("thread", Some("channel")) => row.parent_channel_id,
            _ => None,
        };
        let private_channel = match channel {
            Some(channel) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT visibility FROM conversations WHERE id = $1 -- visibility: checked above",
                )
                .bind(channel)
                .fetch_one(crate::db::scoped(&state.pool))
                .await?
                    == "members"
            }
            None => false,
        };
        let group_id = match (row.kind.as_str(), row.effective_kind()) {
            ("group", _) => row.id,
            ("thread", "group") => row.membership_conversation_id(),
            _ if private_channel => channel.unwrap_or(row.id),
            (_, "dm") => {
                return Ok(ToolResult::invalid(
                    "conversationId",
                    "A DM is always between the same two people. To bring others in, start a group with conversation_start.",
                ));
            }
            _ => {
                return Ok(ToolResult::invalid(
                    "conversationId",
                    "This channel is open to everyone in the project. To add someone to the project, an owner or admin uses space_member_add.",
                ));
            }
        };
        let add_ids = match resolve_direct_people(state, principal.actor_id, &add).await? {
            Ok(ids) => ids.into_iter().collect::<BTreeSet<_>>(),
            Err(who) => {
                return Ok(ToolResult::invalid(
                    "add",
                    format!(
                        "{who} isn't someone you can add: they need to share a workspace with you. Try people_search."
                    ),
                ));
            }
        };
        let remove_ids = match resolve_conversation_members(state, group_id, &remove).await? {
            Ok(ids) => ids.into_iter().collect::<BTreeSet<_>>(),
            Err(who) => {
                return Ok(ToolResult::invalid(
                    "remove",
                    format!(
                        "{who} isn't in this group. Use an actor ID or @handle from conversation_open."
                    ),
                ));
            }
        };
        if let Some(both) = add_ids.intersection(&remove_ids).next() {
            let who = add
                .iter()
                .find(|value| value.trim().parse::<Uuid>().ok() == Some(*both))
                .cloned()
                .unwrap_or_else(|| both.to_string());
            return Ok(ToolResult::invalid(
                "remove",
                format!("{who} is in both add and remove."),
            ));
        }
        let add_ids = add_ids.into_iter().collect::<Vec<_>>();
        let remove_ids = remove_ids.into_iter().collect::<Vec<_>>();
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        mark_via(&mut transaction, principal).await?;
        let reservation = match &key {
            None => None,
            Some(key) => {
                let scope = resource_bound_scope("mcp_conversations", group_id, "members")?;
                let hash = RequestHash::for_value(&serde_json::json!({
                    "add": add_ids,
                    "remove": remove_ids,
                    "expectedVersion": arguments.expected_version,
                }))?;
                match reserve(&mut transaction, principal.actor_id, &scope, key, hash).await {
                    Ok(IdempotencyOutcome::Replay(replay)) => {
                        let mut value = replay.decode::<GroupMembersValue>()?;
                        transaction.commit().await?;
                        value.idempotent_replay = true;
                        return Ok(ToolResult::ok(value));
                    }
                    Ok(IdempotencyOutcome::Reserved(reservation)) => Some(reservation),
                    Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                        return Ok(ToolResult::conflict(
                            "That clientRequestId was already used for a different change. Use a new one.",
                        ));
                    }
                    Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                        return Ok(ToolResult::conflict(
                            "That change is still being made. Try again in a moment.",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let change = match change_group_members(
            &mut transaction,
            GroupEditor {
                actor_id: principal.actor_id,
            },
            group_id,
            &add_ids,
            &remove_ids,
            arguments.expected_version,
        )
        .await
        {
            Ok(change) => change,
            Err(AppError::NotFound("person")) => {
                return Ok(ToolResult::invalid(
                    "add",
                    "Someone you're adding can't join groups right now: they were removed or deactivated.",
                ));
            }
            Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That conversation")),
            Err(AppError::Conflict("group members", _)) => {
                return Ok(ToolResult::conflict(format!(
                    "A group can have at most {MAX_GROUP_MEMBERS} members. Remove someone first."
                )));
            }
            Err(AppError::Forbidden) => return Ok(ToolResult::denied(ONLY_LEAVE)),
            Err(AppError::VersionConflict {
                current_version, ..
            }) => {
                return Ok(ToolResult::conflict(format!(
                    "Someone changed this group first (now version {current_version}). Read it again with conversation_open, then retry."
                )));
            }
            Err(error) => return Err(error),
        };
        if !change.added.is_empty() || !change.removed.is_empty() {
            insert_audit(
                &mut transaction,
                principal,
                crate::spaces::place(change.project_id),
                "conversation.mcp_members_changed",
                "conversation",
                group_id,
                serde_json::json!({
                    "added": change.added.iter().map(|actor| actor.id).collect::<Vec<_>>(),
                    "removed": change.removed.iter().map(|actor| actor.id).collect::<Vec<_>>(),
                }),
            )
            .await?;
        }
        let left = change
            .removed
            .iter()
            .any(|actor| actor.id == principal.actor_id);
        let value = GroupMembersValue {
            conversation_id: change.conversation_id.to_string(),
            name: change.name,
            version: change.version,
            members: if left {
                Vec::new()
            } else {
                change.members.into_iter().map(MemberRef::from).collect()
            },
            added: change.added.into_iter().map(MemberRef::from).collect(),
            removed: change.removed.into_iter().map(MemberRef::from).collect(),
            left,
            url: link_url(
                state,
                None,
                &crate::addresses::Target::Direct {
                    id: group_id,
                    thread: None,
                },
            ),
            idempotent_replay: false,
        };
        if let Some(reservation) = &reservation {
            complete(&mut transaction, reservation, StatusCode::OK, &value).await?;
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_members"))
}

pub(crate) async fn conversation_continue(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationContinueArguments,
) -> ToolResult<ContinueValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let key = match arguments
        .client_request_id
        .as_deref()
        .map(IdempotencyKey::parse)
    {
        None => None,
        Some(Ok(key)) => Some(key),
        Some(Err(_)) => {
            return ToolResult::invalid(
                "clientRequestId",
                "Use letters, numbers, dashes or dots, up to 128 characters.",
            );
        }
    };
    let result = async {
        let Some(_) = visible_conversation(state, principal, conversation_id).await? else {
            return Ok(ToolResult::not_found("That conversation"));
        };
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        mark_via(&mut transaction, principal).await?;
        let reservation = match &key {
            None => None,
            Some(key) => {
                let scope = resource_bound_scope("mcp_conversations", conversation_id, "continue")?;
                let hash = RequestHash::for_value(
                    &serde_json::json!({ "conversationId": conversation_id }),
                )?;
                match reserve(&mut transaction, principal.actor_id, &scope, key, hash).await {
                    Ok(IdempotencyOutcome::Replay(replay)) => {
                        // A replay never resets the turns again.
                        let mut value = replay.decode::<ContinueValue>()?;
                        transaction.commit().await?;
                        value.idempotent_replay = true;
                        return Ok(ToolResult::ok(value));
                    }
                    Ok(IdempotencyOutcome::Reserved(reservation)) => Some(reservation),
                    Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                        return Ok(ToolResult::conflict(
                            "That is still being done. Try again in a moment.",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let outcome = match continue_conversation_in(
            &mut transaction,
            principal.actor_id,
            conversation_id,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(AppError::NotFound(_)) => {
                return Ok(ToolResult::denied(
                    "Your role here doesn't let you give the agents another round (replies.steer). Ask someone in the group.",
                ));
            }
            Err(error) => return Err(error),
        };
        insert_audit(
            &mut transaction,
            principal,
            outcome.project_id,
            "conversation.mcp_continued",
            "conversation",
            outcome.conversation_id,
            serde_json::json!({ "asked": outcome.asked.len() }),
        )
        .await?;
        let value = ContinueValue {
            conversation_id: outcome.conversation_id.to_string(),
            agent_turns_remaining: outcome.agent_turns_remaining,
            asked: outcome.asked.into_iter().map(MemberRef::from).collect(),
            latest_message_id: outcome.latest_message_id.map(|id| id.to_string()),
            idempotent_replay: false,
        };
        if let Some(reservation) = &reservation {
            complete(&mut transaction, reservation, StatusCode::OK, &value).await?;
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "conversation_continue"))
}
