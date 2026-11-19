//! Metering as an operation (H1.25): the workspace's plan and its use of
//! every meter, for whoever manages its settings.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::UsageReport;
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<WorkspaceUsageGet>();
}

/// See the workspace's plan and usage.
pub struct WorkspaceUsageGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUsageGetInput {}

impl Targeted for WorkspaceUsageGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceUsageGet {
    const NAME: &'static str = "workspace_usage_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See the workspace's usage";
    const DESCRIPTION: &'static str = "The workspace's plan and its use of each meter: people \
        and stored bytes now, and this month's counted use (emails, model calls on the server's \
        key, automation runs, form submissions and the rest), each with the workspace's limit \
        where it has one and when the count resets.";

    type Input = WorkspaceUsageGetInput;
    type Output = UsageReport;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceUsageGetInput,
    ) -> Result<UsageReport, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        Ok(super::usage_for(tx, workspace).await?)
    }
}
