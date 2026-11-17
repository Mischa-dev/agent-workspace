//! Workspace lifecycle operations (H1.24): `workspace_delete` schedules the
//! workspace's deletion after a 14-day grace period, and
//! `workspace_delete_cancel` keeps it. Both need "Delete the workspace"
//! (`workspace.delete`), which only owners start with.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::lifecycle::{self, Lifecycle};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<WorkspaceDelete>();
    registry.add::<WorkspaceDeleteCancel>();
}

/// Schedule the workspace for deletion.
pub struct WorkspaceDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceDeleteInput {
    /// The workspace's address (its slug), typed out to confirm.
    pub confirm: String,
}

impl Targeted for WorkspaceDeleteInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLifecycleOutput {
    /// `active`, or `deleting` while the deletion is scheduled.
    pub lifecycle: Lifecycle,
    /// When the workspace and everything in it goes for good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<DateTime<Utc>>,
    /// Agent and app accounts the workspace owns: they are retired with it
    /// (credentials revoked, memberships in other workspaces ended).
    pub owned_accounts: usize,
}

impl Operation for WorkspaceDelete {
    const NAME: &'static str = "workspace_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::WorkspaceDelete);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Delete workspace";
    const DESCRIPTION: &'static str = "Schedule this whole workspace for deletion: its spaces, \
        conversations, tasks, pages, files and runs. Nothing changes for 14 days, during which \
        workspace_delete_cancel keeps it; until then everything but cancelling is refused with \
        workspace_suspended. Accounts stay; the agent and app accounts the workspace owns are \
        retired. Pass the workspace's address as confirm.";
    const EMITS: &'static [&'static str] = &["workspace.lifecycle.changed"];

    type Input = WorkspaceDeleteInput;
    type Output = WorkspaceLifecycleOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: WorkspaceDeleteInput,
    ) -> Result<WorkspaceLifecycleOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let by = call.account_id();
        let tx = call.tx().await?;
        let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(crate::error::AppError::from)?;
        if !input.confirm.trim().eq_ignore_ascii_case(&slug) {
            return Err(OpError::invalid(
                "confirm",
                "Type the workspace's address exactly to confirm deleting it.",
            ));
        }
        let delete_after = lifecycle::request_deletion(tx, workspace, by).await?;
        let owned_accounts = lifecycle::owned_accounts(tx, workspace).await?.len();
        call.audit_target(
            "workspace",
            workspace,
            json!({ "deleteAfter": delete_after, "ownedAccounts": owned_accounts }),
        );
        Ok(WorkspaceLifecycleOutput {
            lifecycle: Lifecycle::Deleting,
            delete_after: Some(delete_after),
            owned_accounts,
        })
    }
}

/// Keep a workspace that was going to be deleted.
pub struct WorkspaceDeleteCancel;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceDeleteCancelInput {}

impl Targeted for WorkspaceDeleteCancelInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceDeleteCancel {
    const NAME: &'static str = "workspace_delete_cancel";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::WorkspaceDelete);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Keep workspace";
    const DESCRIPTION: &'static str = "Cancel a scheduled deletion of this workspace, within its \
        14 days. Everything works again at once.";
    const EMITS: &'static [&'static str] = &["workspace.lifecycle.changed"];

    type Input = WorkspaceDeleteCancelInput;
    type Output = WorkspaceLifecycleOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceDeleteCancelInput,
    ) -> Result<WorkspaceLifecycleOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        lifecycle::cancel_deletion(tx, workspace).await?;
        let owned_accounts = lifecycle::owned_accounts(tx, workspace).await?.len();
        call.audit_target("workspace", workspace, json!({}));
        Ok(WorkspaceLifecycleOutput {
            lifecycle: Lifecycle::Active,
            delete_after: None,
            owned_accounts,
        })
    }
}
