//! AWD-111: agent_profile_write, the agents people make in a project
//! (A9), made, changed, turned off and removed through MCP by people
//! connected through an app as themselves, as the project's Agents page
//! does. Agents read them only (agent_profile_list). Everything goes
//! through `crate::agent_profiles`.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id},
};
use crate::{
    AppState,
    agent_profiles::{self, ProfileRequest},
    environments,
    error::AppError,
    policy::{Action, ActorKind},
};

const PEOPLE_ONLY: &str = "Agents read the project's agents, but only people make or change them: the project's owners and admins, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentProfileWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// An agent to change or remove, by ID or @handle (agent_profile_list).
    /// Leave it out to make one.
    pub profile: Option<String>,
    /// For a new agent: start from a template (agent_profile_list's
    /// templates, such as "reviewer"); what you give replaces its own.
    pub template: Option<String>,
    pub name: Option<String>,
    /// The @handle people tag it by: lower case letters, digits, - and _.
    pub handle: Option<String>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    /// The harness it runs on, by kind; an empty string goes back to the
    /// project's default.
    pub harness: Option<String>,
    /// Model settings its runs take, such as {"reasoning": "high"}.
    pub run_defaults: Option<Map<String, Value>>,
    pub instructions: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    /// An environment of the project (environment_list), by ID; an empty
    /// string clears it.
    pub environment_id: Option<String>,
    /// Turn it on or off. Off takes it out of mentions and stops new runs;
    /// what it did stays.
    pub enabled: Option<bool>,
    /// Remove it: deleted if it never did anything, hidden otherwise.
    pub remove: Option<bool>,
    /// The version you last read, for a change. Defaults to the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentProfileWriteValue {
    /// "made", "changed", "deleted" or "hidden".
    pub done: String,
    /// The agent as it is now, unless it was removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Map<String, Value>>")]
    pub profile: Option<Value>,
}

/// An agent of the project by ID, else by @handle.
async fn find_profile(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    wanted: &str,
) -> Result<Option<Uuid>, AppError> {
    let wanted = wanted.trim();
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT profile.id
        FROM agent_profiles profile
        JOIN project_memberships membership
          ON membership.project_id = profile.project_id AND membership.actor_id = profile.actor_id
        WHERE profile.project_id = $1 AND profile.hidden_at IS NULL
          AND (profile.id::text = $2 OR lower(membership.handle) = lower($3))
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(wanted)
    .bind(wanted.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?)
}

/// A text argument that clears with an empty string.
fn nullable<T>(
    given: Option<&str>,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<Option<T>>, ()> {
    match given.map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(text) => parse(text).map(|value| Some(Some(value))).ok_or(()),
    }
}

pub(super) async fn agent_profile_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: AgentProfileWriteArguments,
) -> ToolResult<AgentProfileWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let remove = arguments.remove.unwrap_or(false);
    let Ok(environment_id) = nullable(arguments.environment_id.as_deref(), parse_id) else {
        return ToolResult::invalid("environmentId", "Pass the environment's ID.");
    };
    let Ok(harness) = nullable(arguments.harness.as_deref(), |text| Some(text.to_owned())) else {
        return ToolResult::invalid("harness", "Pass the harness's kind.");
    };
    let request = ProfileRequest {
        template: arguments.template.clone(),
        name: arguments.name.clone(),
        handle: arguments
            .handle
            .as_deref()
            .map(|handle| handle.trim().trim_start_matches('@').to_owned()),
        avatar: arguments.avatar.clone(),
        color: arguments.color.clone(),
        description: arguments.description.clone(),
        harness,
        run_defaults: arguments.run_defaults.clone(),
        instructions: arguments.instructions.clone(),
        allowed_tools: arguments.allowed_tools.clone(),
        environment_id,
        enabled: arguments.enabled,
        version: None,
    };
    let project_id = principal.project_id;
    let result = async {
        let organization_id = environments::authorize_project(
            &state.pool,
            principal.actor_id,
            ActorKind::Human,
            project_id,
            Action::SettingsManage,
        )
        .await?;
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let existing = match arguments.profile.as_deref() {
            None => None,
            Some(wanted) => match find_profile(&mut transaction, project_id, wanted).await? {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That agent")),
            },
        };
        let (done, profile_id, action, metadata) = match existing {
            None => {
                if remove {
                    return Ok(ToolResult::invalid("profile", "Name the agent to remove."));
                }
                if arguments.expected_version.is_some() {
                    return Ok(ToolResult::invalid(
                        "expectedVersion",
                        "expectedVersion goes with a change to an agent.",
                    ));
                }
                let (profile_id, metadata) = agent_profiles::create_in(
                    state,
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    project_id,
                    request,
                )
                .await?;
                (
                    "made",
                    profile_id,
                    "agent_profile.created".to_owned(),
                    metadata,
                )
            }
            Some(profile_id) if remove => {
                let (outcome, handle) = agent_profiles::remove_in(
                    &mut transaction,
                    principal.actor_id,
                    project_id,
                    profile_id,
                )
                .await?;
                (
                    outcome,
                    profile_id,
                    format!("agent_profile.{outcome}"),
                    serde_json::json!({ "handle": handle }),
                )
            }
            Some(profile_id) => {
                if arguments.template.is_some() {
                    return Ok(ToolResult::invalid(
                        "template",
                        "A template only starts a new agent.",
                    ));
                }
                let (action, metadata) = agent_profiles::update_in(
                    state,
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    project_id,
                    profile_id,
                    ProfileRequest {
                        version: arguments.expected_version,
                        ..request
                    },
                )
                .await?;
                ("changed", profile_id, action.to_owned(), metadata)
            }
        };
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            &action,
            "agent_profile",
            profile_id,
            metadata,
        )
        .await?;
        let profile = if matches!(done, "deleted" | "hidden") {
            None
        } else {
            let view =
                agent_profiles::load(&mut *transaction, state, project_id, profile_id).await?;
            Some(serde_json::to_value(view).unwrap_or(Value::Null))
        };
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(AgentProfileWriteValue {
            done: done.to_owned(),
            profile,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(PEOPLE_ONLY),
        Err(AppError::NotFound("project")) => ToolResult::not_found("That project"),
        Err(AppError::NotFound(_)) => ToolResult::not_found("That agent"),
        Err(error) => ToolResult::from_error(error, "agent_profile_write"),
    }
}
