//! Shaping conversations: create a channel, rename or archive any channel,
//! thread or group (and set a thread's status and outcome), and pin things to
//! a conversation. Each works the same for people and agents, limited by
//! their role in the project.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, conversation_path, insert_audit, mark_via, parse_id, parse_message_id,
        public_url,
    },
    memory_tools::writer,
};
use crate::{
    AppState,
    conversations::{
        ConversationView, ThreadPatch, insert_channel, normalize_conversation_name,
        rename_conversation, set_conversation_archived, update_thread,
    },
    error::AppError,
    memory,
    policy::Action,
};

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationValue {
    pub id: String,
    /// channel, thread, or group.
    pub kind: String,
    pub name: String,
    pub archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Pass as expectedVersion next time.
    pub version: i64,
    pub url: String,
    /// channel_create only: false when a channel with that name already
    /// existed and was returned instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
}

async fn conversation_value(
    state: &AppState,
    view: &ConversationView,
    created: Option<bool>,
) -> Result<ConversationValue, AppError> {
    let parent_kind = match view.parent_channel_id {
        Some(parent_id) => {
            sqlx::query_scalar::<_, String>("SELECT kind FROM conversations WHERE id = $1")
                .bind(parent_id)
                .fetch_optional(&state.pool)
                .await?
        }
        None => None,
    };
    let path = match (
        view.kind.as_str(),
        parent_kind.as_deref(),
        view.parent_channel_id,
    ) {
        ("thread", Some("dm" | "group"), Some(parent_id)) => {
            format!("/messages/{parent_id}?thread={}", view.id)
        }
        (kind, _, _) => conversation_path(view.project_id, kind, view.id, kind == "group"),
    };
    Ok(ConversationValue {
        id: view.id.to_string(),
        kind: view.kind.clone(),
        name: view.name.clone(),
        archived: view.archived_at.is_some(),
        status: view.thread_status.clone(),
        outcome: view.thread_outcome.clone(),
        version: view.version,
        url: public_url(state, &path),
        created,
    })
}

/// Turn a conversation error into words an agent can act on.
fn conversation_error(error: AppError, operation: &'static str) -> ToolResult<ConversationValue> {
    match error {
        AppError::NotFound(_) => ToolResult::not_found("That conversation"),
        AppError::Conflict("conversation_name", _) => ToolResult::conflict(
            "Another active conversation here already has that name. Pick another, or archive or rename that one first.",
        ),
        AppError::Conflict("conversation", "non-terminal Agent Run prevents archive") => {
            ToolResult::conflict(
                "A run is still going in it. Wait for the run to finish, or interrupt it, then archive.",
            )
        }
        AppError::Conflict("conversation", message) => ToolResult::conflict(message),
        AppError::Validation(field) => ToolResult::invalid(field, format!("{field} isn't valid.")),
        error => ToolResult::from_error(error, operation),
    }
}

// ---------------------------------------------------------------------------
// channel_create

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ChannelCreateArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The channel's name, such as "design" or "releases".
    pub name: String,
}

pub(super) async fn channel_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ChannelCreateArguments,
) -> ToolResult<ConversationValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let Ok(name) = normalize_conversation_name(&arguments.name) else {
        return ToolResult::invalid("name", "Use a name up to 120 characters, on one line.");
    };
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let messages_space = match memory::project_access(
            &mut *transaction,
            principal.actor_id,
            principal.actor_kind,
            project_id,
            Action::ChannelCreate,
        )
        .await
        {
            Ok(messages_space) => messages_space,
            Err(AppError::Forbidden) => {
                return Ok(ToolResult::denied(
                    "You can't create channels in that project.",
                ));
            }
            Err(error) => return Err(error),
        };
        if messages_space {
            return Ok(ToolResult::invalid(
                "projectId",
                "The Messages space has DMs and groups, not channels. Use conversation_start.",
            ));
        }
        // Creating a channel that already exists returns it, so retries and
        // "make sure there's a #design channel" are both safe.
        if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM conversations
            WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL
              AND lower(name) = lower($2)
            "#,
        )
        .bind(project_id)
        .bind(&name)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            let view = existing_view(state, principal, existing).await?;
            return Ok(ToolResult::ok(
                conversation_value(state, &view, Some(false)).await?,
            ));
        }
        mark_via(&mut transaction, principal).await?;
        let channel_id =
            insert_channel(&mut transaction, project_id, principal.actor_id, &name).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "conversation.channel_created",
            "conversation",
            channel_id,
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        let view = existing_view(state, principal, channel_id).await?;
        Ok::<_, AppError>(ToolResult::ok(
            conversation_value(state, &view, Some(true)).await?,
        ))
    }
    .await;
    result.unwrap_or_else(|error| conversation_error(error, "channel_create"))
}

async fn existing_view(
    state: &AppState,
    principal: &McpPrincipal,
    conversation_id: Uuid,
) -> Result<ConversationView, AppError> {
    let row = crate::conversations::fetch_conversation_access(
        &state.pool,
        principal.actor_id,
        conversation_id,
    )
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    Ok(ConversationView::from(&row))
}

// ---------------------------------------------------------------------------
// conversation_update

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConversationUpdateArguments {
    /// The channel, thread, or group.
    pub conversation_id: String,
    /// A new name.
    pub name: Option<String>,
    /// Threads only: where it stands. Suggestions: Open, In progress,
    /// Decided, Done, Dropped, Blocked. Any short status works.
    pub status: Option<String>,
    /// Threads only: one line on where it landed, such as "Went with design
    /// tokens; AW-51 created".
    pub outcome: Option<String>,
    /// Threads only: remove the outcome.
    pub clear_outcome: Option<bool>,
    /// Channels and threads: true to archive, false to restore. Archiving a
    /// channel archives its threads with it; nothing is deleted.
    pub archived: Option<bool>,
    /// The version you last read. Leave it out to change the current one.
    pub expected_version: Option<i64>,
}

pub(super) async fn conversation_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ConversationUpdateArguments,
) -> ToolResult<ConversationValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let thread_fields = arguments.status.is_some()
        || arguments.outcome.is_some()
        || arguments.clear_outcome.unwrap_or(false);
    if arguments.name.is_none() && !thread_fields && arguments.archived.is_none() {
        return ToolResult::invalid(
            "name",
            "Say what to change: name, status, outcome, or archived.",
        );
    }
    let result = async {
        let Some((kind, project_id)) = sqlx::query_as::<_, (String, Uuid)>(
            "SELECT kind, project_id FROM conversations WHERE id = $1 AND conversation_visible_to(id, $2)",
        )
        .bind(conversation_id)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
        else {
            return Ok(ToolResult::not_found("That conversation"));
        };
        if project_id != principal.project_id {
            return Ok(ToolResult::not_found("That conversation"));
        }
        if kind == "dm" {
            return Ok(ToolResult::invalid(
                "conversationId",
                "DMs are named after the people in them and can't be renamed or archived.",
            ));
        }
        if thread_fields && kind != "thread" {
            return Ok(ToolResult::invalid(
                "status",
                "Only threads have a status and outcome. Start one with thread_start.",
            ));
        }
        if arguments.archived.is_some() && kind == "group" {
            return Ok(ToolResult::invalid(
                "archived",
                "Groups can't be archived. Leave them out of your briefing with mark_read instead.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let mut changes = Vec::new();
        let mut view = None;
        // Restore first, so a restored conversation can also be renamed.
        if arguments.archived == Some(false) {
            let (_, restored) = set_conversation_archived(
                &mut transaction,
                principal.actor_id,
                principal.actor_kind,
                conversation_id,
                &["channel", "thread"],
                false,
                arguments.expected_version,
            )
            .await?;
            if let Some(restored) = restored {
                changes.push("restored");
                view = Some(ConversationView::from(&restored));
            }
        }
        let version_checked = !changes.is_empty();
        if arguments.name.is_some() || thread_fields {
            if !version_checked
                && let Some(expected) = arguments.expected_version
            {
                let current = sqlx::query_scalar::<_, i64>(
                    "SELECT version FROM conversations WHERE id = $1 FOR UPDATE",
                )
                .bind(conversation_id)
                .fetch_one(&mut *transaction)
                .await?;
                if current != expected {
                    return Ok(ToolResult::conflict(format!(
                        "Someone changed this conversation first (now version {current}). Read it again, then retry."
                    )));
                }
            }
            let updated = if kind == "thread" {
                update_thread(
                    &mut transaction,
                    principal.actor_id,
                    principal.actor_kind,
                    conversation_id,
                    &ThreadPatch {
                        name: arguments.name.clone(),
                        status: arguments.status.clone(),
                        outcome: arguments.outcome.clone(),
                        clear_outcome: arguments.clear_outcome.unwrap_or(false),
                    },
                )
                .await?
            } else {
                let Some(name) = arguments.name.as_deref() else {
                    unreachable!("non-thread changes without a name were rejected above");
                };
                rename_conversation(
                    &mut transaction,
                    principal.actor_id,
                    principal.actor_kind,
                    conversation_id,
                    name,
                )
                .await?
            };
            changes.push("updated");
            view = Some(updated);
        }
        if arguments.archived == Some(true) {
            let (current, archived) = set_conversation_archived(
                &mut transaction,
                principal.actor_id,
                principal.actor_kind,
                conversation_id,
                &["channel", "thread"],
                true,
                if changes.is_empty() {
                    arguments.expected_version
                } else {
                    None
                },
            )
            .await?;
            if let Some(archived) = archived {
                changes.push("archived");
                view = Some(ConversationView::from(&archived));
            } else if view.is_none() {
                view = Some(ConversationView::from(&current));
            }
        }
        if !changes.is_empty() {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                "conversation.mcp_updated",
                "conversation",
                conversation_id,
                serde_json::json!({ "changes": changes }),
            )
            .await?;
        }
        transaction.commit().await?;
        let view = match view {
            Some(view) => view,
            None => existing_view(state, principal, conversation_id).await?,
        };
        Ok::<_, AppError>(ToolResult::ok(conversation_value(state, &view, None).await?))
    }
    .await;
    result.unwrap_or_else(|error| conversation_error(error, "conversation_update"))
}

// ---------------------------------------------------------------------------
// pin_set

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PinSetArguments {
    /// The conversation to pin to. Its pins are its brief: conversation_open
    /// shows them first.
    pub conversation_id: String,
    /// What to pin: a messageId (an ID or a link to the message), pageId, or
    /// decisionId. Pass exactly one.
    pub message_id: Option<String>,
    pub page_id: Option<String>,
    pub decision_id: Option<String>,
    /// False to unpin. Defaults to true.
    pub pinned: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct PinValue {
    pub conversation_id: String,
    pub kind: String,
    pub id: String,
    pub pinned: bool,
    /// Everything pinned to the conversation now, oldest first.
    pub pins: Vec<super::talk::PinItem>,
}

pub(super) async fn pin_set(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: PinSetArguments,
) -> ToolResult<PinValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let targets = [
        ("message", arguments.message_id.as_deref()),
        ("page", arguments.page_id.as_deref()),
        ("decision", arguments.decision_id.as_deref()),
    ]
    .into_iter()
    .filter_map(|(kind, value)| value.map(|value| (kind, value)))
    .collect::<Vec<_>>();
    let [(kind, target)] = targets.as_slice() else {
        return ToolResult::invalid(
            "messageId",
            "Pass exactly one of messageId, pageId, or decisionId.",
        );
    };
    let parsed = if *kind == "message" {
        parse_message_id(target)
    } else {
        parse_id(target)
    };
    let Some(target_id) = parsed else {
        return ToolResult::invalid(&format!("{kind}Id"), "Pass an ID or link.");
    };
    let pinned = arguments.pinned.unwrap_or(true);
    let result = async {
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let project_id = match memory::set_pin(
            &mut transaction,
            writer(principal),
            conversation_id,
            kind,
            target_id,
            pinned,
        )
        .await
        {
            Ok(project_id) => project_id,
            Err(AppError::NotFound("conversation")) => {
                return Ok(ToolResult::not_found("That conversation"));
            }
            Err(AppError::NotFound("pin target")) => {
                return Ok(ToolResult::not_found(&format!(
                    "That {kind} in this conversation's project"
                )));
            }
            Err(error) => return Err(error),
        };
        if project_id != principal.project_id {
            return Ok(ToolResult::not_found("That conversation"));
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            if pinned {
                "conversation.mcp_pinned"
            } else {
                "conversation.mcp_unpinned"
            },
            "conversation",
            conversation_id,
            serde_json::json!({ "kind": kind, "target_id": target_id }),
        )
        .await?;
        transaction.commit().await?;
        let pins = super::talk::pinned_items(state, conversation_id, principal.actor_id).await?;
        Ok::<_, AppError>(ToolResult::ok(PinValue {
            conversation_id: conversation_id.to_string(),
            kind: (*kind).to_owned(),
            id: target_id.to_string(),
            pinned,
            pins,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "pin_set"))
}
