//! AWD-111: environment_write, where a project's runs execute, changed
//! through MCP by people connected through an app as themselves, as the
//! project's settings do on the web. Agents read environments only
//! (environment_list). Everything goes through `crate::environments`.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id},
    launch_tools::{EnvironmentValue, environment_value},
};
use crate::{
    AppState,
    environments::{self, EnvironmentRequest},
    error::AppError,
    policy::{Action, ActorKind},
};

const PEOPLE_ONLY: &str = "Agents read environments, but only people change them: the project's owners and admins, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EnvironmentWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// An environment to change, archive, make the default or check, by ID
    /// or name (environment_list). Leave it out to add one.
    pub environment: Option<String>,
    /// Its name: required when adding.
    pub name: Option<String>,
    /// The harness that runs in it, by kind (environment_list shows those
    /// in use): required when adding.
    pub harness: Option<String>,
    /// The provider's own environment ID, for a harness that runs in the
    /// cloud. An empty string clears it.
    pub provider_environment_id: Option<String>,
    /// A repository of the project (repository_list), by ID. An empty
    /// string clears it.
    pub repository_id: Option<String>,
    /// The branch runs start from. An empty string clears it.
    pub branch: Option<String>,
    /// Notes on setting it up, up to 4000 characters.
    pub setup_notes: Option<String>,
    /// Make it the project's default.
    pub make_default: Option<bool>,
    /// Archive it; a default that pointed here falls back to the workspace's.
    pub archive: Option<bool>,
    /// Check it with your own signed-in account for its harness, and record
    /// the result.
    pub check: Option<bool>,
    /// The version you last read (environment_list), for a change.
    /// Defaults to the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct EnvironmentWriteValue {
    /// "added", "changed", "made_default", "archived" or "checked".
    pub done: String,
    /// The environment, unless it was archived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentValue>,
    /// A check's result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_message: Option<String>,
}

fn has_fields(arguments: &EnvironmentWriteArguments) -> bool {
    arguments.name.is_some()
        || arguments.harness.is_some()
        || arguments.provider_environment_id.is_some()
        || arguments.repository_id.is_some()
        || arguments.branch.is_some()
        || arguments.setup_notes.is_some()
}

/// An active environment of the project by ID, else by name.
async fn find_environment(
    executor: &mut sqlx::PgConnection,
    project_id: Uuid,
    wanted: &str,
) -> Result<Option<Uuid>, AppError> {
    let wanted = wanted.trim();
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM project_environments
        WHERE project_id = $1 AND archived_at IS NULL
          AND (id::text = $2 OR lower(name) = lower($2))
        ORDER BY (id::text = $2) DESC, id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(wanted)
    .fetch_optional(&mut *executor)
    .await?)
}

/// An optional text argument: absent keeps `current`, empty clears it.
fn keep_or_clear(given: Option<&String>, current: Option<String>) -> Option<String> {
    match given {
        None => current,
        Some(text) if text.trim().is_empty() => None,
        Some(text) => Some(text.clone()),
    }
}

pub(super) async fn environment_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: EnvironmentWriteArguments,
) -> ToolResult<EnvironmentWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let archive = arguments.archive.unwrap_or(false);
    let check = arguments.check.unwrap_or(false);
    let make_default = arguments.make_default.unwrap_or(false);
    if (archive || check) && (has_fields(&arguments) || make_default) {
        return ToolResult::invalid(
            if archive { "archive" } else { "check" },
            "Archive or check an environment on its own, without other changes.",
        );
    }
    if archive && check {
        return ToolResult::invalid("check", "Either archive or check, not both.");
    }
    let repository_id = match arguments.repository_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => match parse_id(text) {
            Some(id) => Some(id),
            None => {
                return ToolResult::invalid("repositoryId", "Pass the repository's ID.");
            }
        },
    };
    let project_id = principal.project_id;
    let result = async {
        let organization_id = environments::authorize_project(
            &state.pool,
            principal.actor_id,
            ActorKind::Human,
            project_id,
            if check {
                Action::WorkerInvoke
            } else {
                Action::SettingsManage
            },
        )
        .await?;
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let existing = match arguments.environment.as_deref() {
            None => None,
            Some(wanted) => match find_environment(&mut transaction, project_id, wanted).await? {
                Some(id) => Some(id),
                None => return Ok(ToolResult::not_found("That environment")),
            },
        };
        let (done, environment_id, action, details) = match existing {
            None => {
                if archive || check {
                    return Ok(ToolResult::invalid(
                        "environment",
                        "Name the environment to archive or check.",
                    ));
                }
                let (Some(name), Some(harness)) = (&arguments.name, &arguments.harness) else {
                    return Ok(ToolResult::invalid(
                        "name",
                        "A new environment needs a name and a harness.",
                    ));
                };
                let row = environments::create_in(
                    state,
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    project_id,
                    EnvironmentRequest {
                        name: name.clone(),
                        harness: harness.clone(),
                        provider_environment_id: arguments.provider_environment_id.clone(),
                        repository_id,
                        branch: arguments.branch.clone(),
                        setup_notes: arguments.setup_notes.clone().unwrap_or_default(),
                        make_default,
                        expected_version: None,
                    },
                )
                .await?;
                let details = serde_json::json!({ "harness": row.harness });
                ("added", row.id, Some("environment.created"), details)
            }
            Some(environment_id) if archive => {
                environments::archive_in(
                    &mut transaction,
                    organization_id,
                    project_id,
                    environment_id,
                )
                .await?;
                (
                    "archived",
                    environment_id,
                    Some("environment.archived"),
                    serde_json::json!({}),
                )
            }
            Some(environment_id) if check => {
                // Checked outside this transaction: it calls the provider.
                ("checked", environment_id, None, serde_json::json!({}))
            }
            Some(environment_id) if !has_fields(&arguments) => {
                if !make_default {
                    return Ok(ToolResult::invalid(
                        "environment",
                        "Say what to change: fields, makeDefault, archive or check.",
                    ));
                }
                environments::make_default_in(
                    &mut transaction,
                    organization_id,
                    project_id,
                    environment_id,
                    principal.actor_id,
                )
                .await?;
                (
                    "made_default",
                    environment_id,
                    Some("environment.made_default"),
                    serde_json::json!({}),
                )
            }
            Some(environment_id) => {
                let current =
                    environments::active_environment(&mut *transaction, project_id, environment_id)
                        .await?
                        .ok_or(AppError::NotFound("environment"))?;
                let repository_id = match arguments.repository_id.as_deref() {
                    None => current.repository_id,
                    Some(_) => repository_id,
                };
                let row = environments::update_in(
                    state,
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    project_id,
                    environment_id,
                    EnvironmentRequest {
                        name: arguments.name.clone().unwrap_or(current.name),
                        harness: arguments.harness.clone().unwrap_or(current.harness),
                        provider_environment_id: keep_or_clear(
                            arguments.provider_environment_id.as_ref(),
                            current.provider_environment_id,
                        ),
                        repository_id,
                        branch: keep_or_clear(arguments.branch.as_ref(), current.branch),
                        setup_notes: arguments.setup_notes.clone().unwrap_or(current.setup_notes),
                        make_default,
                        expected_version: Some(
                            arguments.expected_version.unwrap_or(current.version),
                        ),
                    },
                )
                .await?;
                let details = serde_json::json!({ "harness": row.harness, "version": row.version });
                (
                    "changed",
                    environment_id,
                    Some("environment.updated"),
                    details,
                )
            }
        };
        if let Some(action) = action {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                action,
                "project_environment",
                environment_id,
                details,
            )
            .await?;
        }
        transaction.commit().await?;
        let (check_ok, check_message) = if check {
            let (ok, message, _) = environments::check_in(
                state,
                principal.actor_id,
                organization_id,
                project_id,
                environment_id,
            )
            .await?;
            (Some(ok), Some(message))
        } else {
            (None, None)
        };
        let environment = if archive {
            None
        } else {
            let (default, _) = environments::default_environment(
                state,
                organization_id,
                project_id,
                principal.actor_id,
            )
            .await?;
            environments::active_environment(&state.pool, project_id, environment_id)
                .await?
                .map(|row| environment_value(environments::view(state, row, default)))
        };
        Ok::<_, AppError>(ToolResult::ok(EnvironmentWriteValue {
            done: done.to_owned(),
            environment,
            check_ok,
            check_message,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(PEOPLE_ONLY),
        Err(AppError::NotFound("project")) => ToolResult::not_found("That project"),
        Err(AppError::NotFound("repository")) => ToolResult::not_found("That repository"),
        Err(AppError::NotFound(_)) => ToolResult::not_found("That environment"),
        Err(AppError::Validation("harness")) => ToolResult::invalid(
            "harness",
            "That harness isn't installed for this workspace.",
        ),
        Err(AppError::Validation("providerEnvironmentId")) => ToolResult::invalid(
            "providerEnvironmentId",
            "A harness that runs in the cloud needs the provider's environment ID (up to 200 characters).",
        ),
        Err(error) => ToolResult::from_error(error, "environment_write"),
    }
}
