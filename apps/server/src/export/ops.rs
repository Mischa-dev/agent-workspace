//! A workspace export as operations (H1.9): start one, see one, list
//! yours. Each needs "Export the workspace" (`workspace.export`), which only
//! owners start with; a transfer is seen and downloaded by the account that
//! asked for it alone.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::transfer::{self, TransferView};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<WorkspaceExportStart>()
        .add::<WorkspaceTransferGet>()
        .add::<WorkspaceTransferList>();
}

/// One transfer, with where its archive downloads from once it's ready.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferOutput {
    #[serde(flatten)]
    pub transfer: TransferView,
    /// Where the archive downloads from, with your own sign-in, while it's
    /// kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

impl TransferOutput {
    fn new(workspace: uuid::Uuid, transfer: TransferView) -> Self {
        let ready = transfer.direction == "export" && transfer.state == "succeeded";
        Self {
            download_url: ready.then(|| transfer::download_path(workspace, transfer.id)),
            transfer,
        }
    }
}

/// Start exporting the whole workspace.
pub struct WorkspaceExportStart;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceExportStartInput {}

impl Targeted for WorkspaceExportStartInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceExportStart {
    const NAME: &'static str = "workspace_export_start";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::WorkspaceExport);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Export the workspace";
    const DESCRIPTION: &'static str = "Start writing the whole workspace to one archive: its \
        spaces, conversations (never DMs or groups), tasks, pages with every version, records, \
        files and settings, with a profile for each member. Secrets and connections are named, \
        never their values. It runs in the background; workspace_transfer_get shows when it's \
        ready and where only you can download it, for 7 days.";
    const EMITS: &'static [&'static str] = &["workspace.transfer_changed"];

    type Input = WorkspaceExportStartInput;
    type Output = TransferOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceExportStartInput,
    ) -> Result<TransferOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let by = call.account_id();
        let tx = call.tx().await?;
        let started = transfer::start_export(tx, workspace, by).await?;
        call.audit_target(
            "workspace",
            workspace,
            json!({ "transfer": started.id, "direction": "export" }),
        );
        Ok(TransferOutput::new(workspace, started))
    }
}

/// See one of your exports or imports.
pub struct WorkspaceTransferGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceTransferGetInput {
    /// The transfer, as workspace_export_start gave it.
    pub transfer_id: IdRef,
}

impl Targeted for WorkspaceTransferGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceTransferGet {
    const NAME: &'static str = "workspace_transfer_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::WorkspaceExport);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See a workspace export";
    const DESCRIPTION: &'static str = "One export or import you started in this workspace: \
        whether it's queued, running, done or failed, what it holds, and where its archive \
        downloads from while it's kept.";

    type Input = WorkspaceTransferGetInput;
    type Output = TransferOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: WorkspaceTransferGetInput,
    ) -> Result<TransferOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let reader = call.account_id();
        let tx = call.tx().await?;
        transfer::transfer_of(tx, workspace, reader, input.transfer_id.id())
            .await?
            .map(|found| TransferOutput::new(workspace, found))
            .ok_or(OpError::not_found("transfer"))
    }
}

/// Your exports and imports of this workspace.
pub struct WorkspaceTransferList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceTransferListInput {}

impl Targeted for WorkspaceTransferListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferList {
    /// Newest first, at most 50.
    pub transfers: Vec<TransferOutput>,
}

impl Operation for WorkspaceTransferList {
    const NAME: &'static str = "workspace_transfer_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::WorkspaceExport);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List workspace exports";
    const DESCRIPTION: &'static str = "The exports and imports of this workspace that you \
        started, newest first, each with its state and download address while it's kept.";

    type Input = WorkspaceTransferListInput;
    type Output = TransferList;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceTransferListInput,
    ) -> Result<TransferList, OpError> {
        let workspace = call.ctx.workspace_id;
        let reader = call.account_id();
        let tx = call.tx().await?;
        Ok(TransferList {
            transfers: transfer::transfers_of(tx, workspace, reader)
                .await?
                .into_iter()
                .map(|found| TransferOutput::new(workspace, found))
                .collect(),
        })
    }
}
