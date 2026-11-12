//! settings_get: a project's settings as they apply to the caller, with
//! where each value came from (S2). settings_set (AWD-111): people change
//! or reset one, through an app connected as themselves, as the settings
//! pages do; agents only read them.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, project_role},
};
use crate::{
    AppState,
    error::AppError,
    memory,
    policy::{Action, ActorKind},
    settings,
};

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

// ---------------------------------------------------------------------------
// settings_set

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SettingsSetArguments {
    /// The project's ID, link or key (such as "AWD"). Workspace and
    /// personal settings are those of its workspace.
    pub project_id: String,
    /// The setting's key, such as "run.model" (settings_get lists them).
    pub key: String,
    /// Where to keep it: "project" (the default; its owners and admins),
    /// "workspace" (owners and admins of the workspace) or "user" (your
    /// own, in this workspace).
    pub scope: Option<String>,
    /// The new value, as JSON. Leave it out with reset.
    pub value: Option<serde_json::Value>,
    /// Remove the value at scope, so the setting inherits again.
    pub reset: Option<bool>,
    /// The version you last read; 0 means it isn't set at that scope yet.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SettingsSetValue {
    /// "changed", "reset" or "unchanged" (a reset with nothing to remove).
    pub done: String,
    /// The setting as it now applies to you here.
    pub setting: SettingValue,
    /// The version now stored at scope, after a change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
}

const PEOPLE_CHANGE_SETTINGS: &str = "Agents read settings, but only people change them, on the web or through an app connected as themselves.";

pub(super) async fn settings_set(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SettingsSetArguments,
) -> ToolResult<SettingsSetValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_CHANGE_SETTINGS);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let Some(definition) = settings::definition(arguments.key.trim()) else {
        return ToolResult::invalid(
            "key",
            format!("There's no setting called {}.", arguments.key.trim()),
        );
    };
    if settings::has_own_routes(definition.key) {
        return ToolResult::invalid(
            "key",
            "Layouts and modules have their own tools: project_layout_set, and the workspace's settings on the web.",
        );
    }
    let scope = match arguments.scope.as_deref().map(str::trim) {
        None | Some("project") => settings::Scope::Project,
        Some("workspace") => settings::Scope::Workspace,
        Some("user") => settings::Scope::User,
        Some(_) => {
            return ToolResult::invalid("scope", "Use project, workspace or user.");
        }
    };
    let reset = arguments.reset.unwrap_or(false);
    let value = match (reset, arguments.value) {
        (true, Some(_)) => {
            return ToolResult::invalid("value", "Leave out value when resetting.");
        }
        (false, None) => {
            return ToolResult::invalid("value", "Give the new value, or reset: true.");
        }
        (true, None) => None,
        (false, Some(value)) => Some(value),
    };
    let project_id = principal.project_id;
    let result = async {
        if project_role(state, principal, project_id).await?.is_none() {
            return Ok(ToolResult::not_found("That project"));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let organization_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT organization_id FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        let target = settings::authorize_target_as(
            &mut transaction,
            principal.actor_id,
            ActorKind::Human,
            scope,
            Some(organization_id),
            Some(project_id),
        )
        .await?;
        let (done, version) = match value {
            Some(value) => {
                let version = settings::set(
                    &mut transaction,
                    target,
                    definition.key,
                    value,
                    arguments.expected_version,
                    principal.actor_id,
                )
                .await?;
                insert_audit(
                    &mut transaction,
                    principal,
                    project_id,
                    "setting.changed",
                    "setting",
                    project_id,
                    serde_json::json!({
                        "key": definition.key,
                        "scope": scope.as_str(),
                        "organizationId": organization_id,
                        "version": version,
                    }),
                )
                .await?;
                ("changed", Some(version))
            }
            None => {
                let removed = settings::reset(
                    &mut transaction,
                    target,
                    definition.key,
                    arguments.expected_version,
                )
                .await?;
                if removed {
                    insert_audit(
                        &mut transaction,
                        principal,
                        project_id,
                        "setting.reset",
                        "setting",
                        project_id,
                        serde_json::json!({
                            "key": definition.key,
                            "scope": scope.as_str(),
                            "organizationId": organization_id,
                        }),
                    )
                    .await?;
                }
                (if removed { "reset" } else { "unchanged" }, None)
            }
        };
        // As it now applies to the caller in this project.
        let resolved = settings::resolved_after(
            &mut transaction,
            settings::Target::Project {
                organization_id,
                project_id,
            },
            principal.actor_id,
            definition.key,
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(SettingsSetValue {
            done: done.to_owned(),
            setting: SettingValue {
                key: resolved.key.to_owned(),
                label: definition.label.to_owned(),
                value: resolved.value,
                source: source_name(resolved.source),
                overrides: resolved.overrides.map(source_name),
            },
            version,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(match scope {
            settings::Scope::Workspace => {
                "Only the workspace's owners and admins change its settings."
            }
            _ => "Only the project's owners and admins change its settings.",
        }),
        Err(AppError::Validation("setting scope")) => ToolResult::invalid(
            "scope",
            format!("{} can't be set at that scope.", definition.key),
        ),
        Err(AppError::Validation(_) | AppError::Invalid(_)) => ToolResult::invalid(
            "value",
            format!("That isn't a valid value for {}.", definition.key),
        ),
        Err(error) => ToolResult::from_error(error, "settings_set"),
    }
}
