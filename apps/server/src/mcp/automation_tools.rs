//! Outgoing webhooks and automation rules for agents and apps (W14, X4 and
//! X5). Both act as whoever made them, so the same checks as the app apply:
//! a webhook belongs to the person (or the app acting for them), a rule to
//! the person, app or agent that made it.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use crate::{
    AppState,
    automations::{self, Action, Conditions, CreateRule, RunNow, Trigger, UpdateRule},
    error::AppError,
    events::Caller,
    webhooks::{self, CreateWebhook, UpdateWebhook},
};

fn to_value(value: impl Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("serialize tool value"))
}

/// Like `ToolResult::from_error`, but keeps the sentence an `Invalid` error
/// carries, which says what to change.
fn refusal(error: AppError, operation: &'static str) -> ToolResult<Value> {
    match error {
        AppError::Invalid(message) => ToolResult::invalid("request", message),
        AppError::SecretsNotConfigured => ToolResult::conflict(
            "Webhooks need the server's secrets keyring; ask whoever runs this workspace to set it up.",
        ),
        error => ToolResult::from_error(error, operation),
    }
}

fn finish(result: Result<Value, AppError>, operation: &'static str) -> ToolResult<Value> {
    match result {
        Ok(value) => ToolResult::ok(value),
        Err(error) => refusal(error, operation),
    }
}

fn id_argument(
    value: Option<&str>,
    field: &'static str,
) -> Result<uuid::Uuid, ToolResult<Value>> {
    match value.map(parse_id) {
        Some(Some(id)) => Ok(id),
        Some(None) => Err(ToolResult::invalid(field, format!("Use an ID for {field}."))),
        None => Err(ToolResult::invalid(field, format!("{field} is required here."))),
    }
}

// ---------------------------------------------------------------------------
// Webhooks

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WebhookListArguments {}

pub(super) async fn webhook_list(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: WebhookListArguments,
) -> ToolResult<Value> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let caller = Caller::mcp(principal);
    finish(
        async { to_value(webhooks::list(state, &caller).await?) }.await,
        "webhook_list",
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum WebhookOperation {
    Create,
    Update,
    Delete,
    RotateSecret,
    Test,
    Redeliver,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WebhookWriteArguments {
    /// create, update, delete, rotate_secret, test (sends a ping) or
    /// redeliver (one delivery again, with its id).
    pub operation: WebhookOperation,
    /// The webhook, for everything but create.
    pub webhook_id: Option<String>,
    /// create and update.
    pub name: Option<String>,
    /// create and update: an https URL that receives each delivery.
    pub url: Option<String>,
    /// create and update: event types from webhook_list, such as
    /// run.finished or message.created.
    pub event_types: Option<Vec<String>>,
    /// create: hear only this project (ID, link or key); left out, every
    /// project in the workspace you can see.
    pub project_id: Option<String>,
    /// update: active or paused.
    pub state: Option<String>,
    /// update: the version you last read.
    pub expected_version: Option<i64>,
    /// redeliver: the delivery to send again.
    pub delivery_id: Option<String>,
}

pub(super) async fn webhook_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WebhookWriteArguments,
) -> ToolResult<Value> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let caller = Caller::mcp(principal);
    let operation = arguments.operation;
    if operation == WebhookOperation::Create {
        let (Some(name), Some(url), Some(event_types)) =
            (arguments.name, arguments.url, arguments.event_types)
        else {
            return ToolResult::invalid("name", "create needs name, url and eventTypes.");
        };
        let project_id = match arguments.project_id {
            Some(_) if principal.project_explicit => Some(principal.project_id),
            Some(_) => return ToolResult::not_found("That project"),
            None => None,
        };
        return finish(
            async {
                to_value(
                    webhooks::create(
                        state,
                        &caller,
                        CreateWebhook {
                            name,
                            url,
                            event_types,
                            project_id,
                            organization_id: None,
                        },
                    )
                    .await?,
                )
            }
            .await,
            "webhook_write",
        );
    }
    let webhook_id = match id_argument(arguments.webhook_id.as_deref(), "webhookId") {
        Ok(id) => id,
        Err(refused) => return refused,
    };
    let result = async {
        match operation {
            WebhookOperation::Create => unreachable!("handled above"),
            WebhookOperation::Update => {
                let Some(expected_version) = arguments.expected_version else {
                    return Err(AppError::Validation("expectedVersion"));
                };
                to_value(
                    webhooks::update(
                        state,
                        &caller,
                        webhook_id,
                        UpdateWebhook {
                            expected_version,
                            name: arguments.name,
                            url: arguments.url,
                            event_types: arguments.event_types,
                            state: arguments.state,
                        },
                    )
                    .await?,
                )
            }
            WebhookOperation::Delete => {
                webhooks::delete(state, &caller, webhook_id).await?;
                Ok(serde_json::json!({ "deleted": true }))
            }
            WebhookOperation::RotateSecret => {
                to_value(webhooks::rotate_secret(state, &caller, webhook_id).await?)
            }
            WebhookOperation::Test => to_value(webhooks::send_test(state, &caller, webhook_id).await?),
            WebhookOperation::Redeliver => {
                let delivery_id = arguments
                    .delivery_id
                    .as_deref()
                    .and_then(parse_id)
                    .ok_or(AppError::Validation("deliveryId"))?;
                to_value(webhooks::redeliver(state, &caller, webhook_id, delivery_id).await?)
            }
        }
    }
    .await;
    finish(result, "webhook_write")
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WebhookDeliveriesArguments {
    pub webhook_id: String,
    /// nextCursor from the previous page.
    pub cursor: Option<String>,
    /// 1 to 100, default 50.
    pub limit: Option<i64>,
}

pub(super) async fn webhook_deliveries(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WebhookDeliveriesArguments,
) -> ToolResult<Value> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let webhook_id = match id_argument(Some(&arguments.webhook_id), "webhookId") {
        Ok(id) => id,
        Err(refused) => return refused,
    };
    let caller = Caller::mcp(principal);
    finish(
        async {
            to_value(
                webhooks::deliveries(
                    state,
                    &caller,
                    webhook_id,
                    arguments.cursor.as_deref(),
                    arguments.limit,
                )
                .await?,
            )
        }
        .await,
        "webhook_deliveries",
    )
}

// ---------------------------------------------------------------------------
// Automations

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AutomationListArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
}

pub(super) async fn automation_list(
    state: &AppState,
    principal: &McpPrincipal,
    _arguments: AutomationListArguments,
) -> ToolResult<Value> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let caller = Caller::mcp(principal);
    finish(
        async { to_value(automations::list(state, &caller, principal.project_id).await?) }.await,
        "automation_list",
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum AutomationOperation {
    Create,
    Update,
    Delete,
    /// Fire it now, to try it.
    Run,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AutomationWriteArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project's ID, link or key.
    pub project_id: String,
    /// create, update, delete or run (fire it now to try it).
    pub operation: AutomationOperation,
    /// The rule, for everything but create.
    pub rule_id: Option<String>,
    pub name: Option<String>,
    pub trigger: Option<Trigger>,
    /// Saved view and/or list filters the task must match.
    pub conditions: Option<Conditions>,
    /// One to five actions, run in order.
    pub actions: Option<Vec<Action>>,
    pub enabled: Option<bool>,
    /// update: the version you last read.
    pub expected_version: Option<i64>,
    /// run: try it on this task (ID, link or key).
    pub task_id: Option<String>,
}

pub(super) async fn automation_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AutomationWriteArguments,
) -> ToolResult<Value> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let caller = Caller::mcp(principal);
    let project_id = principal.project_id;
    if arguments.operation == AutomationOperation::Create {
        let (Some(name), Some(trigger), Some(actions)) =
            (arguments.name, arguments.trigger, arguments.actions)
        else {
            return ToolResult::invalid("name", "create needs name, trigger and actions.");
        };
        return finish(
            async {
                to_value(
                    automations::create(
                        state,
                        &caller,
                        project_id,
                        CreateRule {
                            name,
                            trigger,
                            conditions: arguments.conditions.unwrap_or_default(),
                            actions,
                            enabled: arguments.enabled,
                        },
                    )
                    .await?,
                )
            }
            .await,
            "automation_write",
        );
    }
    let rule_id = match id_argument(arguments.rule_id.as_deref(), "ruleId") {
        Ok(id) => id,
        Err(refused) => return refused,
    };
    let result = async {
        match arguments.operation {
            AutomationOperation::Create => unreachable!("handled above"),
            AutomationOperation::Update => {
                let Some(expected_version) = arguments.expected_version else {
                    return Err(AppError::Validation("expectedVersion"));
                };
                to_value(
                    automations::update(
                        state,
                        &caller,
                        project_id,
                        rule_id,
                        UpdateRule {
                            expected_version,
                            name: arguments.name,
                            trigger: arguments.trigger,
                            conditions: arguments.conditions,
                            actions: arguments.actions,
                            enabled: arguments.enabled,
                        },
                    )
                    .await?,
                )
            }
            AutomationOperation::Delete => {
                automations::delete(state, &caller, project_id, rule_id).await?;
                Ok(serde_json::json!({ "deleted": true }))
            }
            AutomationOperation::Run => {
                let work_item_id = match arguments.task_id.as_deref() {
                    Some(value) => Some(parse_id(value).ok_or(AppError::Validation("taskId"))?),
                    None => None,
                };
                to_value(
                    automations::run_now(
                        state,
                        &caller,
                        project_id,
                        rule_id,
                        RunNow { work_item_id },
                    )
                    .await?,
                )
            }
        }
    }
    .await;
    finish(result, "automation_write")
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AutomationRunsArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project's ID, link or key.
    pub project_id: String,
    pub rule_id: String,
}

pub(super) async fn automation_runs(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AutomationRunsArguments,
) -> ToolResult<Value> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let rule_id = match id_argument(Some(&arguments.rule_id), "ruleId") {
        Ok(id) => id,
        Err(refused) => return refused,
    };
    let caller = Caller::mcp(principal);
    finish(
        async { to_value(automations::runs(state, &caller, principal.project_id, rule_id).await?) }
            .await,
        "automation_runs",
    )
}
