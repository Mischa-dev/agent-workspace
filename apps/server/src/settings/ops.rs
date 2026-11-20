//! Settings as operations (K2.17): `setting_list` reads every setting as it
//! applies to the caller in a workspace or space, with where each value came
//! from; `setting_update` changes or resets one at the workspace, a space or
//! for the caller. Workspace values take "Manage settings" in the workspace,
//! a space's in the space; anyone keeps their own. Layouts and modules have
//! their own operations.

use std::collections::BTreeMap;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use std::borrow::Cow;

use super::{EffectiveSettings, Resolved, Scope, Target as SettingTarget};
use crate::{
    error::AppError,
    events::{Audience, NewEvent, catalog::SETTING_CHANGED},
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

fn invalid(field: &'static str, message: String) -> OpError {
    OpError::Invalid {
        field: Cow::Borrowed(field),
        message: Cow::Owned(message),
    }
}

pub fn register_ops(registry: &mut Registry) {
    registry.add::<SettingList>().add::<SettingUpdate>();
}

// ---------------------------------------------------------------------------
// setting_list

/// Read settings as they apply to you.
pub struct SettingList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingListInput {
    /// A space: its settings as they apply there. Left out, the
    /// workspace's.
    #[serde(default)]
    pub space_id: Option<IdRef>,
    /// Only these keys, such as ["run.model"]. Left out, every setting.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

impl Targeted for SettingListInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingCapabilities {
    /// You may change the workspace's values.
    pub manage_workspace: bool,
    /// You may change the space's values.
    pub manage_project: bool,
    /// You keep values of your own.
    pub manage_own: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingListOutput {
    #[serde(flatten)]
    pub effective: EffectiveSettings,
    pub capabilities: SettingCapabilities,
}

impl Operation for SettingList {
    const NAME: &'static str = "setting_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List settings";
    const DESCRIPTION: &'static str = "Settings as they apply to you in the workspace or a \
        space (the default harness, model, reasoning and speed for runs, its environment, AI \
        options), each with where its value came from: space, user, workspace or default.";

    type Input = SettingListInput;
    type Output = SettingListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SettingListInput,
    ) -> Result<SettingListOutput, OpError> {
        if let Some(unknown) = input
            .keys
            .iter()
            .flatten()
            .find(|key| super::definition(key).is_none())
        {
            return Err(invalid(
                "keys",
                format!("There's no setting called {unknown}."),
            ));
        }
        let workspace = call.ctx.workspace_id;
        let space = call.ctx.space_id;
        let account = call.account_id();
        let tx = call.tx().await?;
        let mut effective = super::resolve(
            tx,
            super::SettingsContext {
                organization_id: workspace,
                project_id: space,
                actor_id: Some(account),
            },
            &BTreeMap::new(),
        )
        .await?;
        if let Some(space) = space {
            // Settings of a module that's off here aren't shown.
            let modules = crate::modules::state::for_project(tx, space, account)
                .await?
                .unwrap_or_default();
            effective
                .settings
                .retain(|resolved| modules.allows_setting(resolved.key));
        }
        if let Some(keys) = &input.keys {
            effective
                .settings
                .retain(|resolved| keys.iter().any(|key| key == resolved.key));
        }
        let manage_workspace = super::authorize_workspace_manage_as(&mut **tx, account, workspace)
            .await
            .is_ok();
        let manage_project = match space {
            Some(space) => super::authorize_project_manage_as(&mut **tx, account, space)
                .await
                .is_ok(),
            None => false,
        };
        Ok(SettingListOutput {
            effective,
            capabilities: SettingCapabilities {
                manage_workspace,
                manage_project,
                // Any account keeps values of its own (K1.13).
                manage_own: true,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// setting_update

/// Change or reset a setting.
pub struct SettingUpdate;

/// A value that was given, even as JSON null: only a missing field is None.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingUpdateInput {
    /// The setting's key, such as "run.model" (setting_list lists them).
    pub key: String,
    /// Where to keep it: "workspace", "project" (a space's, with spaceId)
    /// or "user" (your own, in this workspace). Left out: the space's with
    /// spaceId, else the workspace's.
    #[serde(default)]
    pub scope: Option<Scope>,
    /// The space, for its value (and for how the result applies there).
    #[serde(default)]
    pub space_id: Option<IdRef>,
    /// The new value, as JSON; null means "not chosen" where a setting
    /// allows it. Leave it out with reset.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<Value>")]
    pub value: Option<Value>,
    /// Remove the value at scope, so the setting inherits again.
    #[serde(default)]
    pub reset: bool,
    /// The version you last read; 0 means it isn't set at that scope yet.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

impl Targeted for SettingUpdateInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingUpdateOutput {
    /// "changed", "reset" or "unchanged" (a reset with nothing to remove).
    pub done: &'static str,
    /// The setting as it now applies to you there.
    pub setting: Resolved,
    /// The version now stored at scope, after a change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
}

impl Operation for SettingUpdate {
    const NAME: &'static str = "setting_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update setting";
    const DESCRIPTION: &'static str = "Change a setting, or reset it so it inherits again, for \
        the workspace, a space or yourself. The workspace's and a space's take managing their \
        settings. Layouts and modules have their own operations.";
    const EMITS: &'static [&'static str] = &["setting.changed"];

    type Input = SettingUpdateInput;
    type Output = SettingUpdateOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SettingUpdateInput,
    ) -> Result<SettingUpdateOutput, OpError> {
        let key = input.key.trim();
        let Some(definition) = super::definition(key) else {
            return Err(invalid("key", format!("There's no setting called {key}.")));
        };
        if super::has_own_routes(definition.key) {
            return Err(OpError::invalid(
                "key",
                "Layouts and modules have their own operations: layout_update, and the \
                 workspace's modules in workspace_update.",
            ));
        }
        let value = match (input.reset, input.value) {
            (true, Some(_)) => {
                return Err(OpError::invalid("value", "Leave out value when resetting."));
            }
            (false, None) => {
                return Err(OpError::invalid(
                    "value",
                    "Give the new value, or reset: true.",
                ));
            }
            (true, None) => None,
            (false, Some(value)) => Some(value),
        };
        let workspace = call.ctx.workspace_id;
        let space = call.ctx.space_id;
        let scope = input.scope.unwrap_or(if space.is_some() {
            Scope::Project
        } else {
            Scope::Workspace
        });
        if scope == Scope::Project && space.is_none() {
            return Err(OpError::invalid(
                "spaceId",
                "Name the space whose setting this is.",
            ));
        }
        let account = call.account_id();
        let kind = call.ctx.caller.kind;
        let ctx = call.ctx;
        let tx = call.tx().await?;
        let target = super::authorize_target_as(
            tx,
            account,
            kind,
            scope,
            Some(workspace),
            space.filter(|_| scope == Scope::Project),
        )
        .await
        .map_err(|error| match error {
            AppError::Forbidden => OpError::Denied {
                reason: DenyReason::Role {
                    permission: Perm::SettingsManage,
                },
                message: match scope {
                    Scope::Workspace => {
                        "Only those who manage the workspace's settings change them."
                    }
                    _ => "Only those who manage this space's settings change them.",
                }
                .into(),
            },
            error => error.into(),
        })?;
        let invalid = |error: AppError| match error {
            AppError::Validation("setting scope") => invalid(
                "scope",
                format!("{} can't be set at that scope.", definition.key),
            ),
            AppError::Validation(_) | AppError::Invalid(_) => invalid(
                "value",
                format!("That isn't a valid value for {}.", definition.key),
            ),
            error => error.into(),
        };
        let (done, version) = match value {
            Some(value) => {
                let version = super::set(
                    tx,
                    target,
                    definition.key,
                    value,
                    input.expected_version,
                    account,
                )
                .await
                .map_err(invalid)?;
                ("changed", Some(version))
            }
            None => {
                let removed = super::reset(tx, target, definition.key, input.expected_version)
                    .await
                    .map_err(invalid)?;
                (if removed { "reset" } else { "unchanged" }, None)
            }
        };
        // As it now applies to the caller: in the space when one was named.
        let applies = match space {
            Some(project_id) => SettingTarget::Project {
                organization_id: workspace,
                project_id,
            },
            None => target,
        };
        let setting = super::resolved_after(tx, applies, account, definition.key).await?;
        if done != "unchanged" {
            let audience = match target {
                SettingTarget::User { actor_id, .. } => Audience::Account(actor_id),
                SettingTarget::Project { .. } => Audience::Space,
                SettingTarget::Workspace { .. } => Audience::Workspace,
            };
            let mut event = NewEvent::thing(
                &SETTING_CHANGED,
                target.project_id().unwrap_or(workspace),
                "",
                Uuid::nil(),
            )
            .audience(audience)
            .workspace(workspace)
            .actor(account)
            .payload(json!({ "key": definition.key, "scope": scope.as_str() }));
            event.thing = None;
            if target.project_id().is_none() {
                event.space_id = None;
            }
            crate::events::emit(tx, event).await?;
        }
        // The audit log keeps setting.changed and setting.reset, as the
        // settings pages always wrote; a reset with nothing to remove
        // leaves no row.
        if done != "unchanged" {
            let mut metadata = json!({
                "key": definition.key,
                "scope": scope.as_str(),
                "organizationId": workspace,
            });
            if let Some(version) = version {
                metadata["version"] = version.into();
            }
            crate::ops::dispatch::audit_row_as(
                tx,
                ctx,
                if done == "changed" {
                    "setting.changed"
                } else {
                    "setting.reset"
                },
                "setting",
                space.unwrap_or(workspace),
                metadata,
            )
            .await
            .map_err(AppError::from)?;
        }
        call.audited_by_command();
        Ok(SettingUpdateOutput {
            done,
            setting,
            version,
        })
    }
}
