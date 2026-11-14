//! A1.11: hosts for people and their apps, matching the Hosts settings
//! page: list them, make an enrolment code, bind and unbind agents, unlink.
//! The rules are `crate::hosts`; a host's own calls (enrol, describe,
//! heartbeat, notices) are its routes, with its own credential.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use crate::{
    AppState,
    error::AppError,
    hosts::{self, EnrolmentCode, HostView, Owner},
};

/// The workspace a call means: the one named, or the one the connection's
/// project is in. None when the caller isn't in the one named.
async fn workspace_of(
    state: &AppState,
    principal: &McpPrincipal,
    named: Option<&str>,
) -> Result<Option<Uuid>, AppError> {
    match named {
        Some(value) => {
            let Some(id) = parse_id(value) else {
                return Ok(None);
            };
            Ok(sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT organization_id FROM organization_memberships
                WHERE organization_id = $1 AND actor_id = $2 AND enabled
                "#,
            )
            .bind(id)
            .bind(principal.actor_id)
            .fetch_optional(&state.pool)
            .await?)
        }
        None => Ok(sqlx::query_scalar::<_, Uuid>(
            "SELECT organization_id FROM projects WHERE id = $1",
        )
        .bind(principal.project_id)
        .fetch_optional(&state.pool)
        .await?),
    }
}

fn finish<T>(result: Result<T, AppError>, operation: &'static str) -> ToolResult<T> {
    match result {
        Ok(value) => ToolResult::ok(value),
        Err(error) => ToolResult::from_error(error, operation),
    }
}

// ---------------------------------------------------------------- host_list

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostListArguments {
    /// The workspace whose hosts to include beside your own (ID). Defaults
    /// to the one your connection is in.
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct HostListValue {
    pub hosts: Vec<HostView>,
}

pub(super) async fn host_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: HostListArguments,
) -> ToolResult<HostListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    finish(
        async {
            let workspace =
                workspace_of(state, principal, arguments.workspace_id.as_deref()).await?;
            let mut connection = state.pool.acquire().await?;
            Ok(HostListValue {
                hosts: hosts::list(&mut connection, principal.actor_id, workspace).await?,
            })
        }
        .await,
        "host_list",
    )
}

// ---------------------------------------------------------------- host_enrolment_create

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostEnrolmentArguments {
    /// person (a host you own) or workspace (one the workspace owns, which
    /// needs hosts.manage there).
    pub owner: String,
    /// For owner workspace: which one (ID). Defaults to the one your
    /// connection is in.
    pub workspace_id: Option<String>,
    /// A note for you, such as the machine's name.
    pub label: Option<String>,
}

pub(super) async fn host_enrolment_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: HostEnrolmentArguments,
) -> ToolResult<EnrolmentCode> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let owner = match arguments.owner.as_str() {
        "person" => Owner::Account(principal.actor_id),
        "workspace" => {
            match workspace_of(state, principal, arguments.workspace_id.as_deref()).await {
                Ok(Some(workspace)) => Owner::Workspace(workspace),
                Ok(None) => return ToolResult::not_found("That workspace"),
                Err(error) => return ToolResult::from_error(error, "host_enrolment_create"),
            }
        }
        _ => return ToolResult::invalid("owner", "Give owner as person or workspace."),
    };
    finish(
        async {
            let mut transaction = state.pool.begin().await?;
            let code = hosts::create_enrolment(
                &mut transaction,
                principal.actor_id,
                principal.actor_kind,
                owner,
                arguments.label.as_deref(),
            )
            .await?;
            transaction.commit().await?;
            Ok(code)
        }
        .await,
        "host_enrolment_create",
    )
}

// ---------------------------------------------------------------- host_agent_bind / unbind

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostAgentBindArguments {
    pub host_id: String,
    /// The agent account the host may act for (ID).
    pub account_id: String,
    /// The host's own name for the agent, from its described agents.
    pub host_agent_ref: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct HostValue {
    pub host: HostView,
}

pub(super) async fn host_agent_bind(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: HostAgentBindArguments,
) -> ToolResult<HostValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let (Some(host_id), Some(account_id)) = (
        parse_id(&arguments.host_id),
        parse_id(&arguments.account_id),
    ) else {
        return ToolResult::not_found("That host or agent");
    };
    finish(
        async {
            let mut transaction = state.pool.begin().await?;
            let host = hosts::bind(
                &mut transaction,
                principal.actor_id,
                host_id,
                account_id,
                &arguments.host_agent_ref,
            )
            .await?;
            transaction.commit().await?;
            Ok(HostValue { host })
        }
        .await,
        "host_agent_bind",
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostAgentUnbindArguments {
    pub host_id: String,
    pub account_id: String,
}

pub(super) async fn host_agent_unbind(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: HostAgentUnbindArguments,
) -> ToolResult<HostValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let (Some(host_id), Some(account_id)) = (
        parse_id(&arguments.host_id),
        parse_id(&arguments.account_id),
    ) else {
        return ToolResult::not_found("That host or agent");
    };
    finish(
        async {
            let mut transaction = state.pool.begin().await?;
            let host =
                hosts::unbind(&mut transaction, principal.actor_id, host_id, account_id).await?;
            transaction.commit().await?;
            Ok(HostValue { host })
        }
        .await,
        "host_agent_unbind",
    )
}

// ---------------------------------------------------------------- host_unlink

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostUnlinkArguments {
    pub host_id: String,
}

pub(super) async fn host_unlink(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: HostUnlinkArguments,
) -> ToolResult<HostValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(host_id) = parse_id(&arguments.host_id) else {
        return ToolResult::not_found("That host");
    };
    finish(
        async {
            let mut transaction = state.pool.begin().await?;
            let host = hosts::unlink(&mut transaction, principal.actor_id, host_id).await?;
            transaction.commit().await?;
            Ok(HostValue { host })
        }
        .await,
        "host_unlink",
    )
}
