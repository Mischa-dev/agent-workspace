//! settings_get: a project's settings as they apply to the caller, with
//! where each value came from (S2). Reading only: settings change through
//! people in the app, never through MCP.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::{
    McpPrincipal,
    common::{ToolResult, parse_id},
};
use crate::{AppState, error::AppError, memory, policy::Action, settings};

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SettingsGetArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Only these keys, such as ["run.model"]. Left out, every setting.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SettingValue {
    pub key: String,
    pub label: String,
    /// The value that applies, as JSON; null means "not chosen" (the
    /// harness or provider decides).
    pub value: serde_json::Value,
    /// Where it came from: project, user, workspace or default.
    pub source: String,
    /// What it overrides, if anything.
    pub overrides: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SettingsValue {
    pub settings: Vec<SettingValue>,
}

fn source_name(source: settings::Source) -> String {
    serde_json::to_value(source)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(super) async fn settings_get(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SettingsGetArguments,
) -> ToolResult<SettingsValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    if let Some(unknown) = arguments
        .keys
        .iter()
        .flatten()
        .find(|key| settings::definition(key).is_none())
    {
        return ToolResult::invalid("keys", format!("There's no setting called {unknown}."));
    }
    let result = async {
        // Errs (as "not found") unless the caller may see the project.
        memory::project_access(
            &state.pool,
            principal.actor_id,
            principal.actor_kind,
            principal.project_id,
            Action::ProjectView,
        )
        .await?;
        // The canonical check also covers this credential's agent
        // connection, which may have been turned off since sign-in.
        let organization_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT organization_id FROM projects WHERE id = $1 AND actor_can_access_project($2, $3, id)",
        )
        .bind(principal.project_id)
        .bind(principal.actor_id)
        .bind(principal.external_agent_connection_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound("project"))?;
        let mut connection = state.pool.acquire().await?;
        let effective = settings::resolve(
            &mut connection,
            settings::SettingsContext {
                organization_id,
                project_id: Some(principal.project_id),
                actor_id: Some(principal.actor_id),
            },
            &BTreeMap::new(),
        )
        .await?;
        let settings = effective
            .settings
            .into_iter()
            .filter(|resolved| {
                arguments
                    .keys
                    .as_ref()
                    .is_none_or(|keys| keys.iter().any(|key| key == resolved.key))
            })
            .map(|resolved| SettingValue {
                key: resolved.key.to_owned(),
                label: settings::definition(resolved.key)
                    .map(|definition| definition.label.to_owned())
                    .unwrap_or_default(),
                value: resolved.value,
                source: source_name(resolved.source),
                overrides: resolved.overrides.map(source_name),
            })
            .collect();
        Ok::<_, AppError>(ToolResult::ok(SettingsValue { settings }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "settings_get"))
}
