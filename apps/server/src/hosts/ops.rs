//! A1.11: managing hosts as operations (K2): list them, make an enrolment
//! code, bind and unbind agents, unlink. The Hosts settings pages and agents
//! call these; a host's own calls (enrol, describe, heartbeat, notices) stay
//! its routes, with its own credential (`routes`).
//!
//! Every one acts in a workspace: the one the call names, or the caller's
//! only one. A host owned by another workspace is `not_found` there. The
//! rules of who may manage a host or bind an agent are `crate::hosts`'s.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{EnrolmentCode, HostView, Owner};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<HostList>()
        .add::<HostEnrolmentCreate>()
        .add::<HostAgentBind>()
        .add::<HostAgentUnbind>()
        .add::<HostUnlink>();
}

/// A host the call may change: not one another workspace owns. The domain
/// call then checks the caller manages it.
async fn in_workspace(call: &mut OpCall<'_>, host_id: Uuid) -> Result<(), OpError> {
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    let host = super::get(tx, host_id).await?;
    match host.map(|host| host.owner()) {
        Some(Owner::Account(_)) => Ok(()),
        Some(Owner::Workspace(owner)) if owner == workspace => Ok(()),
        _ => Err(OpError::not_found("That host")),
    }
}

// ---------------------------------------------------------------- host_list

pub struct HostList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostListInput {
    /// Only the hosts you own, leaving out the workspace's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine_only: bool,
}

impl Targeted for HostListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostListOutput {
    /// Built-in first, then oldest first. Unlinked hosts are left out.
    pub hosts: Vec<HostView>,
}

impl Operation for HostList {
    const NAME: &'static str = "host_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List hosts";
    const DESCRIPTION: &'static str = "List the hosts (machines and services that run agents) \
        you own and the workspace's: their status, the agents bound to each and the ones each \
        last described, and whether you may manage it. Bind an agent with host_agent_bind.";

    type Input = HostListInput;
    type Output = HostListOutput;

    async fn run(call: &mut OpCall<'_>, input: HostListInput) -> Result<HostListOutput, OpError> {
        let mut connection = call
            .state
            .pool
            .acquire()
            .await
            .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        let workspace = (!input.mine_only).then_some(call.ctx.workspace_id);
        let hosts = super::list(&mut connection, call.account_id(), workspace).await?;
        Ok(HostListOutput { hosts })
    }
}

// ---------------------------------------------------------------- host_enrolment_create

pub struct HostEnrolmentCreate;

/// Who will own the host.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HostOwnerChoice {
    /// You.
    Person,
    /// The workspace (needs hosts.manage there).
    Workspace,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostEnrolmentCreateInput {
    /// person (a host you own) or workspace (one the workspace owns).
    pub owner: HostOwnerChoice,
    /// A note for you, such as the machine's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Targeted for HostEnrolmentCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for HostEnrolmentCreate {
    const NAME: &'static str = "host_enrolment_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    /// Never `Dispatcher`: its stored answer would keep the code. A retry
    /// makes a fresh code instead (ten unused ones at most), which is safe.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Make a host enrolment code";
    const DESCRIPTION: &'static str = "Make a one-time code that links a host to you (owner \
        person) or to the workspace (owner workspace, which needs hosts.manage). Give the code \
        to the host's own enrol step within 15 minutes; it is shown once and never stored. Each \
        call makes a new code.";
    const EMITS: &'static [&'static str] = &["host.enrolment_created"];

    type Input = HostEnrolmentCreateInput;
    type Output = EnrolmentCode;

    async fn run(
        call: &mut OpCall<'_>,
        input: HostEnrolmentCreateInput,
    ) -> Result<EnrolmentCode, OpError> {
        let caller = call.account_id();
        let caller_kind = call.ctx.caller.kind;
        let owner = match input.owner {
            HostOwnerChoice::Person => Owner::Account(caller),
            HostOwnerChoice::Workspace => Owner::Workspace(call.ctx.workspace_id),
        };
        let tx = call.tx().await?;
        let code =
            super::create_enrolment(tx, caller, caller_kind, owner, input.label.as_deref()).await?;
        call.audit_target(
            "host_enrolment",
            code.enrolment_id,
            json!({ "owner": code.owner.kind }),
        );
        Ok(code)
    }
}

// ---------------------------------------------------------------- host_agent_bind / unbind

pub struct HostAgentBind;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAgentBindInput {
    /// The host (its id).
    pub host_id: IdRef,
    /// The agent account the host may act for (its id).
    pub account_id: IdRef,
    /// The host's own name for the agent, from its described agents.
    pub host_agent_ref: String,
}

impl Targeted for HostAgentBindInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for HostAgentBind {
    const NAME: &'static str = "host_agent_bind";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Bind an agent to a host";
    const DESCRIPTION: &'static str = "Let a host act for an agent account, which the host \
        knows by hostAgentRef (from host_list's describedAgents). Needs managing the host and \
        agents.manage where the agent is a member. Binding the same agent again moves it to \
        the new name.";
    const EMITS: &'static [&'static str] = &["host.agent_bound"];

    type Input = HostAgentBindInput;
    type Output = HostView;

    async fn run(call: &mut OpCall<'_>, input: HostAgentBindInput) -> Result<HostView, OpError> {
        let host_id = input.host_id.id();
        let account_id = input.account_id.id();
        in_workspace(call, host_id).await?;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let host = super::bind(tx, caller, host_id, account_id, &input.host_agent_ref).await?;
        call.audit_target("host", host_id, json!({ "accountId": account_id }));
        Ok(host)
    }
}

pub struct HostAgentUnbind;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAgentUnbindInput {
    /// The host (its id).
    pub host_id: IdRef,
    /// The agent account to stop it acting for (its id).
    pub account_id: IdRef,
}

impl Targeted for HostAgentUnbindInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for HostAgentUnbind {
    const NAME: &'static str = "host_agent_unbind";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Unbind an agent from a host";
    const DESCRIPTION: &'static str = "Stop a host acting for an agent account. The agent's \
        runs on that host are interrupted, since nobody is left to report them. Needs managing \
        the host.";
    const EMITS: &'static [&'static str] = &["host.agent_unbound"];

    type Input = HostAgentUnbindInput;
    type Output = HostView;

    async fn run(call: &mut OpCall<'_>, input: HostAgentUnbindInput) -> Result<HostView, OpError> {
        let host_id = input.host_id.id();
        let account_id = input.account_id.id();
        in_workspace(call, host_id).await?;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let host = super::unbind(tx, caller, host_id, account_id).await?;
        call.audit_target("host", host_id, json!({ "accountId": account_id }));
        Ok(host)
    }
}

// ---------------------------------------------------------------- host_unlink

pub struct HostUnlink;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUnlinkInput {
    /// The host (its id).
    pub host_id: IdRef,
}

impl Targeted for HostUnlinkInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for HostUnlink {
    const NAME: &'static str = "host_unlink";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Unlink a host";
    const DESCRIPTION: &'static str = "Unlink a host: its credential stops working, its active \
        runs are interrupted and the requests raised through it are cancelled. Linking it again \
        needs a new enrolment code. Needs managing the host.";
    const EMITS: &'static [&'static str] = &["host.unlinked"];

    type Input = HostUnlinkInput;
    type Output = HostView;

    async fn run(call: &mut OpCall<'_>, input: HostUnlinkInput) -> Result<HostView, OpError> {
        let host_id = input.host_id.id();
        in_workspace(call, host_id).await?;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let host = super::unlink(tx, caller, host_id).await?;
        call.audit_target("host", host_id, json!({}));
        Ok(host)
    }
}
