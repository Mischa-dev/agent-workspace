//! AWD-111: github_integration_write, a project's GitHub App installation
//! linked (or re-checked) with the repositories it reaches, and its pull
//! request automation switched, through MCP by people who manage the
//! project's GitHub integration, connected through an app as themselves, as
//! the project's settings do. Everything goes through `crate::github`.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id},
};
use crate::{
    AppState,
    error::AppError,
    github,
    policy::{Action, ActorKind},
};

const PEOPLE_ONLY: &str = "Only people who manage the project's GitHub integration link it or switch its automation, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GitHubIntegrationWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Link the project to this GitHub App installation (its number on
    /// GitHub), or check it again. With repositories.
    #[serde(default)]
    pub installation_id: Option<u64>,
    /// With installationId: the project's repositories it should reach, by
    /// ID (repository_list), 1 to 50. GitHub is asked that it does.
    #[serde(default)]
    pub repositories: Option<Vec<String>>,
    /// Move a task to review when a pull request that names it opens.
    #[serde(default)]
    pub pull_request_open: Option<bool>,
    /// Move a task to done when a pull request that names it merges.
    #[serde(default)]
    pub pull_request_merge: Option<bool>,
    /// The automation's version you last read. Leave it out to change
    /// whatever is set.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct GitHubIntegrationWriteValue {
    /// "synced" or "automation".
    pub done: String,
    /// After a sync: the installation and the repositories it reaches.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Map<String, Value>>")]
    pub installation: Option<Value>,
    /// After an automation change: both switches and the version.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Map<String, Value>>")]
    pub automation: Option<Value>,
}

pub(super) async fn github_integration_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: GitHubIntegrationWriteArguments,
) -> ToolResult<GitHubIntegrationWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let syncing = arguments.installation_id.is_some() || arguments.repositories.is_some();
    let switching = arguments.pull_request_open.is_some()
        || arguments.pull_request_merge.is_some()
        || arguments.expected_version.is_some();
    if syncing == switching {
        return ToolResult::invalid(
            "installationId",
            "Link an installation (installationId with repositories) or switch the automation (pullRequestOpen, pullRequestMerge), one per call.",
        );
    }
    let project_id = principal.project_id;
    if syncing {
        let (Some(installation_id), Some(repositories)) =
            (arguments.installation_id, arguments.repositories.as_deref())
        else {
            return ToolResult::invalid(
                "repositories",
                "Give installationId and the repositories it should reach together.",
            );
        };
        let Some(repository_ids) = repositories
            .iter()
            .map(|text| parse_id(text))
            .collect::<Option<Vec<_>>>()
        else {
            return ToolResult::invalid("repositories", "Pass repository IDs (repository_list).");
        };
        if repository_ids.is_empty() || repository_ids.len() > 50 || installation_id == 0 {
            return ToolResult::invalid(
                "repositories",
                "Give an installation number and 1 to 50 different repositories.",
            );
        }
        let result = async {
            let mut synced = github::sync_installation_as(
                state,
                principal.actor_id,
                ActorKind::Human,
                project_id,
                installation_id,
                &repository_ids,
            )
            .await?;
            mark_via(&mut synced.transaction, principal).await?;
            insert_audit(
                &mut synced.transaction,
                principal,
                project_id,
                "github.installation_sync",
                "github_installation",
                synced.installation_id,
                serde_json::json!({ "repository_count": synced.repository_count }),
            )
            .await?;
            let installation =
                github::installation_value(&mut synced.transaction, project_id).await?;
            synced.transaction.commit().await?;
            Ok::<_, AppError>(GitHubIntegrationWriteValue {
                done: "synced".to_owned(),
                installation: Some(installation),
                automation: None,
            })
        }
        .await;
        return outcome(result);
    }
    if arguments
        .expected_version
        .is_some_and(|version| version < 1)
    {
        return ToolResult::invalid("expectedVersion", "A version is 1 or more.");
    }
    let result = async {
        let mut transaction = state.pool.begin().await?;
        github::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            ActorKind::Human,
            project_id,
            Action::GithubIntegrationManage,
        )
        .await?;
        let settings = github::set_automation_in(
            &mut transaction,
            project_id,
            arguments.expected_version,
            arguments.pull_request_open,
            arguments.pull_request_merge,
        )
        .await?;
        mark_via(&mut transaction, principal).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            "github.automation_settings_update",
            "project",
            project_id,
            serde_json::json!({
                "pull_request_open_enabled": settings.pull_request_open_enabled,
                "pull_request_merge_enabled": settings.pull_request_merge_enabled,
                "version": settings.version,
            }),
        )
        .await?;
        transaction.commit().await?;
        let automation = serde_json::to_value(&settings)
            .map_err(|_| AppError::Internal("serialize GitHub automation"))?;
        Ok::<_, AppError>(GitHubIntegrationWriteValue {
            done: "automation".to_owned(),
            installation: None,
            automation: Some(automation),
        })
    }
    .await;
    outcome(result)
}

fn outcome(
    result: Result<GitHubIntegrationWriteValue, AppError>,
) -> ToolResult<GitHubIntegrationWriteValue> {
    match result {
        Ok(value) => ToolResult::ok(value),
        Err(AppError::Forbidden) => ToolResult::denied(PEOPLE_ONLY),
        Err(AppError::NotFound("project")) => ToolResult::not_found("That project"),
        Err(AppError::NotFound("repository")) => {
            ToolResult::not_found("One of those repositories in this project")
        }
        Err(AppError::NotFound("github_installation")) => ToolResult::not_found(
            "That GitHub installation for this workspace (connect it to the workspace on the web first)",
        ),
        Err(AppError::Conflict("github_integration", "not_configured")) => {
            ToolResult::conflict("GitHub isn't set up on this server yet.")
        }
        Err(AppError::Conflict("github_installation", "incompatible_scope")) => {
            ToolResult::conflict(
                "That installation doesn't reach those repositories, or lacks the permissions the app needs.",
            )
        }
        Err(AppError::Conflict("repository", "changed_during_sync")) => {
            ToolResult::conflict("Those repositories changed while GitHub was checked. Try again.")
        }
        Err(AppError::Conflict("github_automation_settings", "version")) => {
            ToolResult::conflict("Someone changed the automation first. Read it again, then retry.")
        }
        Err(error) => ToolResult::from_error(error, "github_integration_write"),
    }
}
