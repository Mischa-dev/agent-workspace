//! usage_windows: your provider accounts' usage windows, banked resets and
//! how many ready tasks each could run (U1). Reading only: schedules, alert
//! rules and spending a banked reset (which can't be undone) happen in the
//! app. Accounts belong to people, so agents have none.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use crate::{AppState, error::AppError, usage};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UsageWindowsArguments {
    /// Only this provider account.
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WindowLine {
    /// Such as 5-hour, Weekly or a schedule's name.
    pub label: String,
    /// reported (by the harness) or schedule (set by you).
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unused_percent: Option<u16>,
    /// RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    pub limit_reached: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountLine {
    pub account_id: String,
    pub name: String,
    pub harness: String,
    pub connected: bool,
    pub windows: Vec<WindowLine>,
    /// Banked resets you can spend in the app; absent when the harness has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banked_resets: Option<i32>,
    /// Open, unblocked tasks with a repository in the projects this account serves.
    pub ready_tasks: usize,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageWindowsValue {
    pub accounts: Vec<AccountLine>,
}

pub(super) async fn usage_windows(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: UsageWindowsArguments,
) -> ToolResult<UsageWindowsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let account_id = match arguments.account_id.as_deref().map(parse_id) {
        None => None,
        Some(Some(id)) => Some(id),
        Some(None) => return ToolResult::invalid("accountId", "Use a provider account ID."),
    };
    let result = async {
        // The workspace of the connection's project.
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_one(&state.pool)
                .await?;
        let accounts = usage::list_for(state, principal.actor_id, Some(organization_id)).await?;
        let mut lines = Vec::new();
        let mut connection = state.pool.acquire().await?;
        for account in accounts
            .into_iter()
            .filter(|account| account_id.is_none_or(|id| id == account.account_id))
        {
            // AWD-75: counted only where Runs is on for the caller.
            let ready = usage::ready::ready_tasks_in(
                &mut connection,
                principal.actor_id,
                account.account_id,
                None,
                true,
            )
            .await?;
            lines.push(AccountLine {
                account_id: account.account_id.to_string(),
                name: account.name,
                harness: account.harness_name,
                connected: account.connected,
                windows: account
                    .windows
                    .into_iter()
                    .map(|window| WindowLine {
                        label: window.label,
                        source: match window.source {
                            usage::WindowSource::Reported => "reported",
                            usage::WindowSource::Schedule => "schedule",
                        }
                        .to_owned(),
                        used_percent: window.used_percent,
                        unused_percent: window.unused_percent,
                        resets_at: window.resets_at.map(|at| at.to_rfc3339()),
                        limit_reached: window.limit_reached,
                    })
                    .collect(),
                banked_resets: account.reset_credits.map(|credits| credits.available),
                ready_tasks: ready.tasks.len(),
            });
        }
        if account_id.is_some() && lines.is_empty() {
            return Ok::<_, AppError>(ToolResult::not_found("provider account"));
        }
        Ok(ToolResult::ok(UsageWindowsValue { accounts: lines }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "usage_windows"))
}
