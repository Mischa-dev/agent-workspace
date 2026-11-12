//! AWD-111: repository_write, a project's GitHub repositories added,
//! prepared on the server and archived through MCP by people connected
//! through an app as themselves, as the project's settings do. Agents read
//! repositories only (repository_list, repository_get). Everything goes
//! through `crate::repositories`.

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
    policy::{Action, ActorKind},
    repositories,
};

const PEOPLE_ONLY: &str = "Agents read repositories, but only people add, prepare or archive them: those who manage code in the project, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RepositoryWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// A repository of the project to prepare or archive, by ID
    /// (repository_list). Leave it out to add one.
    pub repository: Option<String>,
    /// To add: the GitHub owner and name, such as "octo-org" and "app".
    pub owner: Option<String>,
    pub name: Option<String>,
    /// To add: GitHub's numeric ID for the repository.
    pub github_repository_id: Option<i64>,
    /// To add: the branch runs start from.
    pub default_branch: Option<String>,
    /// To add: you trust this repository's code to run on this server's
    /// harnesses. Required, as on the web.
    pub trust_acknowledged: Option<bool>,
    /// Prepare it: clone it on the server, or bring its copy up to date.
    pub prepare: Option<bool>,
    /// Archive it. Refused while a run uses it.
    pub archive: Option<bool>,
    /// The version you last read, to archive. Defaults to the current one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepositoryWriteValue {
    /// "added", "prepared" or "archived".
    pub done: String,
    /// The repository as it is now.
    #[schemars(with = "Map<String, Value>")]
    pub repository: Value,
}

pub(super) async fn repository_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RepositoryWriteArguments,
) -> ToolResult<RepositoryWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(PEOPLE_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let prepare = arguments.prepare.unwrap_or(false);
    let archive = arguments.archive.unwrap_or(false);
    let adding = arguments.owner.is_some()
        || arguments.name.is_some()
        || arguments.github_repository_id.is_some()
        || arguments.default_branch.is_some()
        || arguments.trust_acknowledged.is_some();
    let existing = match arguments.repository.as_deref() {
        None => None,
        Some(text) => match parse_id(text) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("repository", "Pass the repository's ID."),
        },
    };
    match (existing, adding, prepare, archive) {
        (None, true, _, false) => {}
        (Some(_), false, true, false) | (Some(_), false, false, true) => {}
        (Some(_), false, true, true) => {
            return ToolResult::invalid("archive", "Either prepare or archive, not both.");
        }
        (Some(_), true, _, _) => {
            return ToolResult::invalid(
                "repository",
                "A repository's GitHub identity can't change. Leave out repository to add another.",
            );
        }
        _ => {
            return ToolResult::invalid(
                "repository",
                "Add a repository (owner, name, githubRepositoryId, defaultBranch, trustAcknowledged), or name one to prepare or archive.",
            );
        }
    }
    if existing.is_none() && arguments.trust_acknowledged != Some(true) {
        return ToolResult::invalid(
            "trustAcknowledged",
            "Adding a repository means trusting its code to run on this server; say so with trustAcknowledged: true.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let role = repositories::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            ActorKind::Human,
            project_id,
            Action::RepositoryManage,
        )
        .await?;
        // The answer is the repository: only for someone who also sees
        // code here.
        repositories::require_project_access_as(
            &mut transaction,
            principal.actor_id,
            ActorKind::Human,
            project_id,
            Action::RepositoryView,
        )
        .await?;
        let manage = repositories::may_manage(role);
        let (done, repository_id, action) = match existing {
            None => {
                let (Some(owner), Some(name), Some(github_repository_id), Some(branch)) = (
                    arguments.owner.as_deref(),
                    arguments.name.as_deref(),
                    arguments.github_repository_id,
                    arguments.default_branch.as_deref(),
                ) else {
                    return Ok(ToolResult::invalid(
                        "owner",
                        "Adding needs owner, name, githubRepositoryId and defaultBranch.",
                    ));
                };
                let repository_id = repositories::register_picked_repository(
                    state,
                    &mut transaction,
                    project_id,
                    github_repository_id,
                    owner,
                    name,
                    branch,
                )
                .await?;
                ("added", repository_id, "repository.registered")
            }
            Some(repository_id) if archive => {
                let current = sqlx::query_scalar::<_, i64>(
                    "SELECT version FROM repositories WHERE id = $1 AND project_id = $2",
                )
                .bind(repository_id)
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(AppError::NotFound("repository"))?;
                repositories::archive_in(
                    &mut transaction,
                    principal.actor_id,
                    project_id,
                    repository_id,
                    arguments.expected_version.unwrap_or(current),
                )
                .await?;
                ("archived", repository_id, "repository.archived")
            }
            Some(repository_id) => ("prepared", repository_id, "repository.prepared"),
        };
        if done != "prepared" {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                action,
                "repository",
                repository_id,
                serde_json::json!({}),
            )
            .await?;
            let repository =
                repositories::repository_json(&mut transaction, project_id, repository_id, manage)
                    .await?;
            transaction.commit().await?;
            return Ok(ToolResult::ok(RepositoryWriteValue {
                done: done.to_owned(),
                repository,
            }));
        }
        // Preparing runs git outside the transaction; the audit row says
        // who asked for it.
        repositories::repository_json(&mut transaction, project_id, repository_id, manage).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            action,
            "repository",
            repository_id,
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        repositories::prepare_in(state, project_id, repository_id).await?;
        let mut transaction = state.pool.begin().await?;
        let repository =
            repositories::repository_json(&mut transaction, project_id, repository_id, manage)
                .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(RepositoryWriteValue {
            done: done.to_owned(),
            repository,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(PEOPLE_ONLY),
        Err(AppError::NotFound("project")) => ToolResult::not_found("That project"),
        Err(AppError::NotFound(_)) => ToolResult::not_found("That repository"),
        Err(AppError::RepositoryHasActiveRuns) => {
            ToolResult::conflict("A run is using this repository. Stop it first.")
        }
        Err(AppError::Validation("repositoryIdentity")) => ToolResult::invalid(
            "owner",
            "That isn't a valid GitHub owner and repository name.",
        ),
        Err(AppError::Validation("githubRepositoryId")) => {
            ToolResult::invalid("githubRepositoryId", "Pass GitHub's numeric ID for it.")
        }
        Err(error) => ToolResult::from_error(error, "repository_write"),
    }
}
