//! W21 through MCP: Later, reminders and scheduled messages. The commands
//! are the web's (`crate::later`, `crate::scheduled_messages`), so a person's
//! app sees and changes exactly their own list, and an agent gets reminders
//! and scheduled messages of its own, acting as itself with the same
//! checks. An agent's due reminder arrives as a `reminder.due` event.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, accessible_project_ids, confined_project, parse_id, parse_message_id,
        public_url,
    },
    talk::{message_actor, message_content},
    tasks::resolve_task,
};
use crate::{
    AppState,
    error::AppError,
    later::{self, ChangeRequest, ListView, SaveRequest, SavedItemView, Saver},
    message_content::MessageContent,
    policy::ActorKind,
    scheduled_messages::{self, ScheduledMessageView},
};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LaterListArguments {
    /// open (default: saved and not done, reminders due soonest first),
    /// done, or scheduled (your messages waiting to send).
    pub view: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaterLine {
    pub id: String,
    /// message, work_item or reminder
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<String>,
    pub done: bool,
    pub project: String,
    /// The saved message's or task's text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScheduledLine {
    pub id: String,
    pub conversation_id: String,
    pub place: String,
    pub text: String,
    pub send_at: String,
    /// scheduled or failed
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaterListValue {
    // `default` keeps each optional in the output schema: an empty list is
    // left out, and clients reject a result missing a required field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<LaterLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled: Vec<ScheduledLine>,
    pub open_count: i64,
}

fn line(state: &AppState, item: SavedItemView) -> LaterLine {
    LaterLine {
        id: item.id.to_string(),
        kind: item.kind,
        note: item.note,
        remind_at: item.remind_at,
        done: item.done,
        project: item.project_name,
        preview: item
            .message
            .as_ref()
            .map(|message| {
                if message.removed {
                    "(removed)".to_owned()
                } else {
                    format!(
                        "{} in {}: {}",
                        message.author_name, message.place, message.preview
                    )
                }
            })
            .or_else(|| {
                item.work_item
                    .as_ref()
                    .map(|task| format!("{} {}", task.key, task.title))
            }),
        message_id: item.message.as_ref().map(|message| message.id.to_string()),
        task: item.work_item.as_ref().map(|task| task.key.clone()),
        url: public_url(state, &item.href),
    }
}

fn scheduled_line(state: &AppState, message: ScheduledMessageView) -> ScheduledLine {
    ScheduledLine {
        id: message.id.to_string(),
        conversation_id: message.conversation_id.to_string(),
        place: message.place,
        text: message.plain_text,
        send_at: message.send_at,
        status: message.status,
        failure: message.failure,
        url: public_url(state, &message.href),
    }
}

/// The projects this call reads in: the one it's confined to, or (for an
/// agent) the ones its connection reaches; a person's app reads everywhere.
async fn within(state: &AppState, principal: &McpPrincipal) -> Result<Option<Vec<Uuid>>, AppError> {
    if let Some(project_id) = confined_project(principal) {
        return Ok(Some(vec![project_id]));
    }
    if principal.actor_kind == ActorKind::Human {
        return Ok(None);
    }
    accessible_project_ids(state, principal).await.map(Some)
}

pub(super) async fn later_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LaterListArguments,
) -> ToolResult<LaterListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let within = within(state, principal).await?;
        let view = arguments.view.as_deref().unwrap_or("open");
        let (items, scheduled) = match view {
            "open" | "done" => {
                let page = later::list(
                    &state.pool,
                    principal.actor_id,
                    if view == "done" {
                        ListView::Done
                    } else {
                        ListView::Open
                    },
                    within.as_deref(),
                )
                .await?;
                (page.items, Vec::new())
            }
            "scheduled" => (
                Vec::new(),
                scheduled_messages::list(&state.pool, principal.actor_id, None, within.as_deref())
                    .await?,
            ),
            _ => {
                return Ok(ToolResult::invalid("view", "Use open, done or scheduled."));
            }
        };
        let open_count =
            later::open_count(&state.pool, principal.actor_id, within.as_deref()).await?;
        Ok::<_, AppError>(ToolResult::ok(LaterListValue {
            items: items.into_iter().map(|item| line(state, item)).collect(),
            scheduled: scheduled
                .into_iter()
                .map(|message| scheduled_line(state, message))
                .collect(),
            open_count,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "later_list"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReminderSetArguments {
    /// What to be reminded of. Needed unless you save a message or task.
    pub text: Option<String>,
    /// When, as RFC 3339 (2026-10-18T09:00:00+02:00), up to a year ahead.
    /// Leave it out to save a message or task for later without a reminder.
    pub at: Option<String>,
    /// Save this message: its ID or link.
    pub message_id: Option<String>,
    /// Save this task: its ID, link or key (such as AW-12).
    pub task: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaterItemValue {
    pub item: LaterLine,
}

/// `later_save`: save a message or task for later, or set a reminder, for
/// yourself.
pub(super) async fn later_save(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ReminderSetArguments,
) -> ToolResult<LaterItemValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let human = match principal.actor_kind {
        ActorKind::Human => true,
        ActorKind::ExternalAgent => false,
        _ => return ToolResult::denied("This connection can't keep a Later list."),
    };
    let message_id = match arguments.message_id.as_deref().map(parse_message_id) {
        None => None,
        Some(Some(id)) => Some(id),
        Some(None) => {
            return ToolResult::invalid("messageId", "Pass a message ID, or a link to it.");
        }
    };
    let result = async {
        let work_item_id = match arguments.task.as_deref() {
            None => None,
            Some(task) => match resolve_task(state, principal, task).await? {
                Some((id, _)) => Some(id),
                None => return Ok(ToolResult::not_found("That task")),
            },
        };
        if message_id.is_none() && work_item_id.is_none() {
            if arguments
                .text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            {
                return Ok(ToolResult::invalid("text", "Say what to remind you of."));
            }
            if arguments.at.is_none() {
                return Ok(ToolResult::invalid("at", "Say when, as RFC 3339."));
            }
        }
        let saver = Saver {
            actor_id: principal.actor_id,
            human,
            organization_id: None,
            project_id: Some(principal.project_id),
            within: if human {
                confined_project(principal)
            } else {
                Some(principal.project_id)
            },
            connection_id: principal.external_agent_connection_id,
        };
        match later::save(
            state,
            saver,
            SaveRequest {
                message_id,
                work_item_id,
                note: arguments.text,
                remind_at: arguments.at,
                organization_id: None,
            },
        )
        .await
        {
            Ok(item) => Ok(ToolResult::ok(LaterItemValue {
                item: line(state, item),
            })),
            Err(AppError::NotFound(_)) => Ok(ToolResult::not_found("That message or task")),
            Err(AppError::Validation("remindAt")) => Ok(ToolResult::invalid(
                "at",
                "Use RFC 3339, from now up to a year ahead.",
            )),
            Err(AppError::Validation("note")) => {
                Ok(ToolResult::invalid("text", "Use up to 1,000 characters."))
            }
            Err(error) => Err(error),
        }
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "later_save"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LaterUpdateArguments {
    /// The item's ID, from later_list or later_save.
    pub id: String,
    /// true: mark it done. false: back to open.
    pub done: Option<bool>,
    /// A new reminder time (RFC 3339), or "none" to stop reminding.
    pub at: Option<String>,
    /// A new note, or a reminder's new text.
    pub text: Option<String>,
    /// true: take it off the list altogether.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LaterUpdateValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<LaterLine>,
    pub removed: bool,
}

pub(super) async fn later_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LaterUpdateArguments,
) -> ToolResult<LaterUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = parse_id(&arguments.id) else {
        return ToolResult::invalid("id", "Use an ID from later_list.");
    };
    let result = async {
        // Only items this connection can read.
        let item = match later::get_item(&state.pool, principal.actor_id, id).await {
            Ok(item) => item,
            Err(AppError::NotFound(_)) => return Ok(ToolResult::not_found("That item")),
            Err(error) => return Err(error),
        };
        if let Some(projects) = within(state, principal).await?
            && !projects.contains(&item.project_id)
        {
            return Ok(ToolResult::not_found("That item"));
        }
        if arguments.remove == Some(true) {
            later::remove(&state.pool, principal.actor_id, id).await?;
            return Ok(ToolResult::ok(LaterUpdateValue {
                item: None,
                removed: true,
            }));
        }
        let remind_at = match arguments.at.as_deref() {
            None => None,
            Some("none" | "") => Some(None),
            Some(at) => Some(Some(at.to_owned())),
        };
        match later::change(
            state,
            principal.actor_id,
            id,
            ChangeRequest {
                remind_at,
                note: arguments.text,
                done: arguments.done,
            },
        )
        .await
        {
            Ok(item) => Ok(ToolResult::ok(LaterUpdateValue {
                item: Some(line(state, item)),
                removed: false,
            })),
            Err(AppError::Validation("remindAt")) => Ok(ToolResult::invalid(
                "at",
                "Use RFC 3339, from now up to a year ahead. A reminder needs a time.",
            )),
            Err(AppError::Validation("note")) => Ok(ToolResult::invalid(
                "text",
                "Use up to 1,000 characters; a reminder needs some.",
            )),
            Err(error) => Err(error),
        }
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "later_update"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MessageScheduleArguments {
    pub conversation_id: String,
    /// The message as plain text. @handles of people in the project become
    /// mentions.
    pub text: Option<String>,
    /// Structured content instead of text.
    pub content: Option<MessageContent>,
    /// When to send it, as RFC 3339, up to a year ahead. It sends as you,
    /// checked then as if you sent it then.
    pub send_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScheduledValue {
    pub scheduled: ScheduledLine,
}

fn schedule_refusal<T>(error: AppError, operation: &'static str) -> ToolResult<T> {
    match error {
        AppError::NotFound(_) => ToolResult::not_found("That conversation or scheduled message"),
        AppError::Validation("sendAt") => {
            ToolResult::invalid("sendAt", "Use RFC 3339, from now up to a year ahead.")
        }
        AppError::Validation(_) => ToolResult::invalid(
            "text",
            "That message isn't valid there: check its length and mentions.",
        ),
        AppError::Conflict(_, message) => ToolResult::conflict(message),
        error => ToolResult::from_error(error, operation),
    }
}

pub(super) async fn message_schedule(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: MessageScheduleArguments,
) -> ToolResult<ScheduledValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(conversation_id) = parse_id(&arguments.conversation_id) else {
        return ToolResult::invalid("conversationId", "Pass a conversation ID or link.");
    };
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(message) => return ToolResult::denied(message),
    };
    let content = match message_content(
        state,
        principal,
        arguments.text.as_deref(),
        arguments.content,
        "message_schedule",
    )
    .await
    {
        Ok(content) => content,
        Err(refused) => return refused,
    };
    match scheduled_messages::schedule(
        state,
        &actor,
        conversation_id,
        content,
        &arguments.send_at,
        principal.actor_kind == ActorKind::Human,
    )
    .await
    {
        Ok(message) => ToolResult::ok(ScheduledValue {
            scheduled: scheduled_line(state, message),
        }),
        Err(error) => schedule_refusal(error, "message_schedule"),
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ScheduledUpdateArguments {
    /// The scheduled message's ID, from message_schedule or later_list.
    pub id: String,
    /// New text.
    pub text: Option<String>,
    /// A new time, as RFC 3339.
    pub send_at: Option<String>,
    /// true: send it now.
    pub send_now: Option<bool>,
    /// true: don't send it.
    pub cancel: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScheduledUpdateValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<ScheduledLine>,
    pub canceled: bool,
}

pub(super) async fn scheduled_message_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ScheduledUpdateArguments,
) -> ToolResult<ScheduledUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = parse_id(&arguments.id) else {
        return ToolResult::invalid("id", "Use an ID from later_list.");
    };
    let actor = match message_actor(principal) {
        Ok(actor) => actor,
        Err(message) => return ToolResult::denied(message),
    };
    let result = async {
        let existing =
            match scheduled_messages::get_scheduled(&state.pool, principal.actor_id, id).await {
                Ok(existing) => existing,
                Err(AppError::NotFound(_)) => {
                    return Ok(ToolResult::not_found("That scheduled message"));
                }
                Err(error) => return Err(error),
            };
        if let Some(projects) = within(state, principal).await?
            && !projects.contains(&existing.project_id)
        {
            return Ok(ToolResult::not_found("That scheduled message"));
        }
        if arguments.cancel == Some(true) {
            return match scheduled_messages::cancel(&state.pool, principal.actor_id, id).await {
                Ok(()) => Ok(ToolResult::ok(ScheduledUpdateValue {
                    scheduled: None,
                    canceled: true,
                })),
                Err(error) => Ok(schedule_refusal(error, "scheduled_message_update")),
            };
        }
        let content = match arguments.text.as_deref() {
            None => None,
            Some(text) => {
                let text = text.trim();
                if text.is_empty() {
                    return Ok(ToolResult::invalid(
                        "text",
                        "Send between 1 and 20,000 characters.",
                    ));
                }
                Some(super::common::content_from_text(state, existing.project_id, text).await?)
            }
        };
        let mut updated = existing;
        if content.is_some() || arguments.send_at.is_some() {
            updated = match scheduled_messages::update(
                state,
                &actor,
                id,
                content,
                arguments.send_at.as_deref(),
                principal.actor_kind == ActorKind::Human,
            )
            .await
            {
                Ok(updated) => updated,
                Err(error) => return Ok(schedule_refusal(error, "scheduled_message_update")),
            };
        }
        if arguments.send_now == Some(true) {
            updated = match scheduled_messages::send_now(state, principal.actor_id, id).await {
                Ok(updated) => updated,
                Err(error) => return Ok(schedule_refusal(error, "scheduled_message_update")),
            };
        }
        Ok::<_, AppError>(ToolResult::ok(ScheduledUpdateValue {
            scheduled: Some(scheduled_line(state, updated)),
            canceled: false,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "scheduled_message_update"))
}
