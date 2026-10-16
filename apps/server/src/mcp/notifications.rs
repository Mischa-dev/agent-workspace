//! The inbox through MCP: `notification_list` and `notification_update`,
//! sharing `crate::notifications` with the web, so an app a person connects
//! sees and changes exactly what their inbox shows. Agents have no
//! notifications of their own; they use briefing and inbox_list.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, accessible_project_ids, parse_id},
};
use crate::{
    AppState,
    error::AppError,
    notifications::{self, Category, ListFilter, NotificationView, Selection, UpdateAction, View},
};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NotificationListArguments {
    /// inbox (default: not archived or snoozed), unread, snoozed or archived.
    pub view: Option<String>,
    /// all (default), mentions, tasks, runs or pull_requests.
    pub category: Option<String>,
    /// Only this project.
    pub project_id: Option<String>,
    /// From a previous call's nextCursor.
    pub cursor: Option<String>,
    /// 1 to 100, default 30.
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct NotificationLine {
    pub id: String,
    /// mention, comment_mention, assigned, approval_requested, run_waiting,
    /// run_completed, run_failed, checks_failed, review_requested,
    /// task_changed, or a kind added later.
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub project: String,
    /// What it's about: a conversation, task, run or pull request.
    pub subject: String,
    pub subject_kind: String,
    pub subject_id: String,
    /// How many occurrences collapsed into this one.
    pub count: i32,
    pub read: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
    pub at: String,
    pub url: String,
    /// Answer with the web app or the run's thread; pending approvals only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct NotificationListValue {
    pub notifications: Vec<NotificationLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub unread_count: i64,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NotificationUpdateArguments {
    /// read, unread, archive, restore, snooze or unsnooze.
    pub action: String,
    /// Notification IDs from notification_list (up to 200).
    pub ids: Option<Vec<String>>,
    /// Instead of ids: everything in the inbox. read and archive only.
    pub all: Option<bool>,
    /// With all: only this project.
    pub project_id: Option<String>,
    /// For snooze: when it comes back, as RFC 3339 (2026-10-17T09:00:00+02:00).
    pub snooze_until: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct NotificationUpdateValue {
    /// IDs that changed. Ones already that way, or not yours, are skipped.
    pub updated: Vec<String>,
    pub unread_count: i64,
}

pub(super) fn line(state: &AppState, view: NotificationView) -> NotificationLine {
    let url = if view.href.starts_with('/') {
        super::common::public_url(state, &view.href)
    } else {
        view.href
    };
    NotificationLine {
        id: view.id.to_string(),
        kind: view.kind,
        title: view.title,
        preview: view.preview,
        project: view.project_name,
        subject: view.subject.title,
        subject_kind: view.subject.kind,
        subject_id: view.subject.id.to_string(),
        count: view.count,
        read: view.read,
        snoozed_until: view.snoozed_until,
        at: view.at,
        url,
        approval_id: view
            .approval
            .filter(|approval| approval.status == "pending")
            .map(|approval| approval.id.to_string()),
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
}

/// The projects this call may read notifications in.
async fn within(state: &AppState, principal: &McpPrincipal) -> Result<Vec<Uuid>, AppError> {
    if principal.project_explicit {
        Ok(vec![principal.project_id])
    } else {
        accessible_project_ids(state, principal).await
    }
}

/// Unread inbox notifications for the briefing, newest first.
pub(super) async fn unread_lines(
    state: &AppState,
    principal: &McpPrincipal,
    projects: &[Uuid],
) -> Result<Vec<NotificationLine>, AppError> {
    let page = notifications::list(
        &state.pool,
        principal.actor_id,
        &ListFilter {
            view: View::Unread,
            within: Some(projects.to_vec()),
            limit: 20,
            ..ListFilter::default()
        },
    )
    .await?;
    Ok(page
        .notifications
        .into_iter()
        .map(|view| line(state, view))
        .collect())
}

pub(super) async fn notification_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: NotificationListArguments,
) -> ToolResult<NotificationListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let Some(view) = arguments.view.as_deref().map_or(Some(View::Inbox), parse) else {
            return Ok(ToolResult::invalid(
                "view",
                "Use inbox, unread, snoozed or archived.",
            ));
        };
        let Some(category) = arguments
            .category
            .as_deref()
            .map_or(Some(Category::All), parse)
        else {
            return Ok(ToolResult::invalid(
                "category",
                "Use all, mentions, tasks, runs or pull_requests.",
            ));
        };
        let project_id = match arguments.project_id.as_deref().map(parse_id) {
            None => None,
            Some(Some(project_id)) => Some(project_id),
            Some(None) => return Ok(ToolResult::invalid("projectId", "Use a project ID or key.")),
        };
        let page = notifications::list(
            &state.pool,
            principal.actor_id,
            &ListFilter {
                view,
                category,
                project_id,
                within: Some(within(state, principal).await?),
                cursor: arguments.cursor,
                limit: arguments.limit.unwrap_or(30),
            },
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(NotificationListValue {
            notifications: page
                .notifications
                .into_iter()
                .map(|view| line(state, view))
                .collect(),
            next_cursor: page.next_cursor,
            unread_count: page.unread_count,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "notification_list"))
}

pub(super) async fn notification_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: NotificationUpdateArguments,
) -> ToolResult<NotificationUpdateValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let result = async {
        let Some(action) = parse::<UpdateAction>(&arguments.action) else {
            return Ok(ToolResult::invalid(
                "action",
                "Use read, unread, archive, restore, snooze or unsnooze.",
            ));
        };
        let selection = match (arguments.ids, arguments.all) {
            (Some(ids), None | Some(false)) => {
                let mut parsed = Vec::with_capacity(ids.len());
                for id in ids {
                    match parse_id(&id) {
                        Some(id) => parsed.push(id),
                        None => {
                            return Ok(ToolResult::invalid(
                                "ids",
                                "Use IDs from notification_list.",
                            ));
                        }
                    }
                }
                Selection::Ids(parsed)
            }
            (None, Some(true)) => Selection::AllInInbox {
                project_id: match arguments.project_id.as_deref().map(parse_id) {
                    None => None,
                    Some(Some(project_id)) => Some(project_id),
                    Some(None) => {
                        return Ok(ToolResult::invalid("projectId", "Use a project ID or key."));
                    }
                },
            },
            _ => return Ok(ToolResult::invalid("ids", "Pass ids, or all: true.")),
        };
        let projects = within(state, principal).await?;
        let updated = notifications::update(
            &state.pool,
            principal.actor_id,
            Some(&projects),
            &selection,
            action,
            arguments.snooze_until.as_deref(),
        )
        .await?;
        Ok::<_, AppError>(ToolResult::ok(NotificationUpdateValue {
            updated: updated.updated.iter().map(Uuid::to_string).collect(),
            unread_count: updated.unread_count,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "notification_update"))
}
