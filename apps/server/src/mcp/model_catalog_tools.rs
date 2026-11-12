//! AWD-111: model_catalog_write, a workspace's model list per harness,
//! changed through MCP by its owners and admins connected through an app as
//! themselves, as its Runs and models settings do on the web. Everything
//! goes through `crate::execution`'s catalog functions.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, project_role},
};
use crate::{
    AppState, error::AppError, execution, policy::ActorKind, provider::manifest::ModelOverrides,
    settings,
};

const MANAGERS_ONLY: &str = "Only the workspace's owners and admins who manage its settings change its model lists, as people, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ModelCatalogWriteArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
    /// The harness whose model list to change, by kind (such as the
    /// `harness` environment_list shows).
    pub harness: String,
    /// One model, by its ID: edit it (with `overrides`), add it by hand
    /// (with `overrides` when the list doesn't have it), or `remove` it.
    #[serde(default)]
    pub model: Option<String>,
    /// The fields to set on `model`, replacing its earlier edits:
    /// displayName, description, contextWindow, reasoningLevels,
    /// defaultReasoning, speeds, defaultSpeed, prices
    /// ({inputPerMillion, outputPerMillion, cachedInputPerMillion}), hidden,
    /// isDefault. A refresh never changes them.
    #[serde(default)]
    pub overrides: Option<serde_json::Value>,
    /// Remove a model added by hand, or clear the edits to one the harness
    /// reports.
    #[serde(default)]
    pub remove: Option<bool>,
    /// Ask the harness for its current models through your own signed-in
    /// account for it, and store them. People's edits are kept.
    #[serde(default)]
    pub refresh: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelCatalogWriteValue {
    /// "saved", "removed" or "refreshed".
    pub done: String,
    /// The harness with its manifest and the workspace's model list now.
    pub harness: serde_json::Value,
}

enum Change {
    Save(String, Box<ModelOverrides>),
    Remove(String),
    Refresh,
}

fn change_of(
    arguments: &ModelCatalogWriteArguments,
) -> Result<Change, ToolResult<ModelCatalogWriteValue>> {
    let remove = arguments.remove == Some(true);
    let refresh = arguments.refresh == Some(true);
    if refresh {
        if remove || arguments.model.is_some() || arguments.overrides.is_some() {
            return Err(ToolResult::invalid(
                "refresh",
                "A refresh goes on its own, without a model, overrides or remove.",
            ));
        }
        return Ok(Change::Refresh);
    }
    let Some(model) = arguments.model.clone() else {
        return Err(ToolResult::invalid(
            "model",
            "Name a model to edit or remove, or pass refresh: true.",
        ));
    };
    if execution::valid_model_id(&model).is_err() {
        return Err(ToolResult::invalid(
            "model",
            "A model ID is 1 to 200 letters, digits and - _ . / : characters.",
        ));
    }
    if remove {
        if arguments.overrides.is_some() {
            return Err(ToolResult::invalid(
                "remove",
                "Remove a model or give its overrides, not both.",
            ));
        }
        return Ok(Change::Remove(model));
    }
    let Some(overrides) = arguments.overrides.clone() else {
        return Err(ToolResult::invalid(
            "overrides",
            "Give the model's overrides, or remove: true.",
        ));
    };
    let overrides = match serde_json::from_value::<ModelOverrides>(overrides) {
        Ok(overrides) => overrides,
        Err(error) => {
            return Err(ToolResult::invalid(
                "overrides",
                format!("Those overrides aren't valid: {error}."),
            ));
        }
    };
    if let Err(field) = overrides.validate() {
        return Err(ToolResult::invalid(
            "overrides",
            format!("The override {field} isn't valid."),
        ));
    }
    Ok(Change::Save(model, Box::new(overrides)))
}

pub(super) async fn model_catalog_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ModelCatalogWriteArguments,
) -> ToolResult<ModelCatalogWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(MANAGERS_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let change = match change_of(&arguments) {
        Ok(change) => change,
        Err(result) => return result,
    };
    let project_id = principal.project_id;
    let harness = arguments.harness.clone();
    let result = async {
        if project_role(state, principal, project_id).await?.is_none() {
            return Ok(ToolResult::not_found("That project"));
        }
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&state.pool)
                .await?;
        settings::authorize_personal_as(
            &state.pool,
            principal.actor_id,
            ActorKind::Human,
            organization_id,
        )
        .await?;
        settings::authorize_workspace_manage_as(
            &state.pool,
            principal.actor_id,
            ActorKind::Human,
            organization_id,
        )
        .await?;
        let (done, action, model, view) = match change {
            Change::Save(model, overrides) => {
                let view = execution::save_overrides_in(
                    state,
                    principal.actor_id,
                    organization_id,
                    &harness,
                    &model,
                    &overrides,
                )
                .await?;
                ("saved", "harness_model.saved", Some(model), view)
            }
            Change::Remove(model) => {
                let view =
                    execution::remove_model_in(state, organization_id, &harness, &model).await?;
                ("removed", "harness_model.removed", Some(model), view)
            }
            Change::Refresh => {
                let view = execution::refresh_in(
                    state,
                    principal.actor_id,
                    organization_id,
                    Some(project_id),
                    &harness,
                    None,
                )
                .await?;
                ("refreshed", "harness_catalog.refreshed", None, view)
            }
        };
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            action,
            "organization",
            organization_id,
            serde_json::json!({
                "organizationId": organization_id,
                "harness": harness,
                "model": model,
            }),
        )
        .await?;
        transaction.commit().await?;
        let harness = serde_json::to_value(&view)
            .map_err(|_| AppError::Internal("serialize harness view"))?;
        Ok::<_, AppError>(ToolResult::ok(ModelCatalogWriteValue {
            done: done.to_owned(),
            harness,
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(MANAGERS_ONLY),
        Err(AppError::NotFound("harness")) => {
            ToolResult::not_found("That harness in this workspace")
        }
        Err(AppError::NotFound("harness_model")) => ToolResult::not_found("That model"),
        Err(AppError::ExecutionProfileRequired) => ToolResult::conflict(
            "Sign in to an account for that harness first: a refresh lists its models through your own account.",
        ),
        Err(error) => ToolResult::from_error(error, "model_catalog_write"),
    }
}
