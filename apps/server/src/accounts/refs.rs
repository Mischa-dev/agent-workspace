//! K1.7: an agent account's outside reference, which agent elsewhere it is.
//!
//! An account may carry one active reference: an issuer (who issued the
//! outside id, such as a host installation) and the id it gave. A reference
//! is never what makes the account: rebinding it to a new pair keeps the
//! account, its roles, its name and everything it wrote, and the old pair
//! moves into the reference's lineage. Managers attach, rebind and detach it
//! (the manage rule, `access::may_manage_account`). A host that says one of
//! its agents now has another reference only proposes the new pair: it waits
//! as `pending_rebind` until a manager accepts it, because a forged lineage
//! would hand one agent's account to another.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access,
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

const MAX_PART_CHARS: usize = 200;
/// How many earlier pairs a reference remembers.
const MAX_LINEAGE: usize = 50;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<AgentRefAttach>();
    registry.add::<AgentRefRebind>();
    registry.add::<AgentRefDecide>();
    registry.add::<AgentRefDetach>();
}

/// One outside reference.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutsideRef {
    pub issuer: String,
    pub external_id: String,
    /// Earlier pairs this agent was moved or re-keyed from, newest first.
    pub lineage: Vec<Value>,
    /// The host that proposed it, for a pending rebind.
    #[schemars(with = "Option<String>")]
    pub host_id: Option<Uuid>,
    #[schemars(with = "String")]
    pub at: DateTime<Utc>,
}

/// An agent's outside reference now, and a rebind waiting for a manager.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRefView {
    #[schemars(with = "String")]
    pub agent: Uuid,
    pub active: Option<OutsideRef>,
    /// A new pair a host proposed: a manager accepts or rejects it with
    /// `agent_ref_decide`.
    pub pending: Option<OutsideRef>,
}

#[derive(sqlx::FromRow)]
struct RefRow {
    id: Uuid,
    issuer: String,
    external_id: String,
    lineage: Value,
    status: String,
    host_id: Option<Uuid>,
    attached_at: DateTime<Utc>,
}

impl RefRow {
    fn view(self) -> OutsideRef {
        OutsideRef {
            issuer: self.issuer,
            external_id: self.external_id,
            lineage: self.lineage.as_array().cloned().unwrap_or_default(),
            host_id: self.host_id,
            at: self.attached_at,
        }
    }
}

async fn rows(connection: &mut PgConnection, agent: Uuid) -> Result<Vec<RefRow>, AppError> {
    Ok(sqlx::query_as::<_, RefRow>(
        "SELECT id, issuer, external_id, lineage, status, host_id, attached_at \
         FROM agent_outside_refs \
         WHERE account_id = $1 AND status <> 'detached' ORDER BY attached_at FOR UPDATE",
    )
    .bind(agent)
    .fetch_all(&mut *connection)
    .await?)
}

/// What an agent carries now.
pub async fn view(connection: &mut PgConnection, agent: Uuid) -> Result<AgentRefView, AppError> {
    let mut active = None;
    let mut pending = None;
    for row in rows(connection, agent).await? {
        if row.status == "active" {
            active = Some(row.view());
        } else {
            pending = Some(row.view());
        }
    }
    Ok(AgentRefView {
        agent,
        active,
        pending,
    })
}

fn part(value: &str, field: &'static str) -> Result<String, OpError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_PART_CHARS || value.contains('\n') {
        return Err(OpError::invalid(
            field,
            "Give one line of 1 to 200 characters.",
        ));
    }
    Ok(value.to_owned())
}

/// Is `(issuer, external_id)` another account's, active or waiting?
async fn held_elsewhere(
    connection: &mut PgConnection,
    agent: Uuid,
    issuer: &str,
    external_id: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM agent_outside_refs \
         WHERE issuer = $1 AND external_id = $2 AND status <> 'detached' AND account_id <> $3)",
    )
    .bind(issuer)
    .bind(external_id)
    .bind(agent)
    .fetch_one(&mut *connection)
    .await?)
}

/// The agent is in this workspace and the caller manages it.
async fn require_managed(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &access::Caller,
    workspace_id: Uuid,
    agent: Uuid,
) -> Result<(), OpError> {
    let member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2)",
    )
    .bind(workspace_id)
    .bind(agent)
    .fetch_one(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    if !member {
        return Err(OpError::not_found("That agent"));
    }
    match access::may_manage_account(transaction, caller, agent).await? {
        Ok(_) => Ok(()),
        Err(access::Denied::NotFound) => Err(OpError::not_found("That agent")),
        Err(_) => Err(OpError::invalid(
            "agent",
            "You can't manage that agent. Ask its owner or one of its managers.",
        )),
    }
}

/// Move the active reference to a new pair: the old pair goes to the front
/// of its lineage, and any waiting proposal is set aside.
async fn rebind_active(
    transaction: &mut Transaction<'_, Postgres>,
    agent: Uuid,
    active: &RefRow,
    issuer: &str,
    external_id: &str,
    by: Uuid,
) -> Result<(), AppError> {
    let mut lineage = vec![json!({
        "issuer": active.issuer,
        "externalId": active.external_id,
        "until": Utc::now(),
    })];
    lineage.extend(
        active
            .lineage
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(MAX_LINEAGE - 1),
    );
    sqlx::query(
        "UPDATE agent_outside_refs SET status = 'detached', detached_at = now(), \
         version = version + 1 \
         WHERE account_id = $1 AND status = 'pending_rebind'",
    )
    .bind(agent)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE agent_outside_refs SET issuer = $2, external_id = $3, lineage = $4, \
         attached_by_actor_id = $5, attached_at = now(), host_id = NULL, version = version + 1 \
         WHERE id = $1",
    )
    .bind(active.id)
    .bind(issuer)
    .bind(external_id)
    .bind(Value::Array(lineage))
    .bind(by)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// A host says its agent `agent` is now `(issuer, external_id)`. Nothing
/// changes until a manager accepts: the pair waits as the agent's pending
/// rebind (replacing an earlier one). Returns whether a rebind now waits.
/// A pair the agent already carries, or another account's, proposes nothing.
pub async fn propose_from_host(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    agent: Uuid,
    issuer: &str,
    external_id: &str,
) -> Result<bool, AppError> {
    let (Ok(issuer), Ok(external_id)) = (part(issuer, "issuer"), part(external_id, "id")) else {
        return Ok(false);
    };
    let current = rows(transaction, agent).await?;
    if current
        .iter()
        .any(|row| row.issuer == issuer && row.external_id == external_id)
    {
        return Ok(current
            .iter()
            .any(|row| row.status == "pending_rebind" && row.external_id == external_id));
    }
    if held_elsewhere(transaction, agent, &issuer, &external_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE agent_outside_refs SET status = 'detached', detached_at = now(), \
         version = version + 1 \
         WHERE account_id = $1 AND status = 'pending_rebind'",
    )
    .bind(agent)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO agent_outside_refs (id, account_id, issuer, external_id, status, host_id) \
         VALUES ($1, $2, $3, $4, 'pending_rebind', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(&issuer)
    .bind(&external_id)
    .bind(host_id)
    .execute(&mut **transaction)
    .await?;
    Ok(true)
}

/// A manager settles the agent's waiting rebind: accepted, the pair becomes
/// its reference (the old one moves to its lineage, or it becomes the first
/// one); rejected, it is set aside. `false` when nothing was waiting.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    agent: Uuid,
    accept: bool,
    by: Uuid,
) -> Result<bool, AppError> {
    let current = rows(transaction, agent).await?;
    let Some(pending) = current.iter().find(|row| row.status == "pending_rebind") else {
        return Ok(false);
    };
    if !accept {
        sqlx::query(
            "UPDATE agent_outside_refs SET status = 'detached', detached_at = now(), \
             version = version + 1 WHERE id = $1",
        )
        .bind(pending.id)
        .execute(&mut **transaction)
        .await?;
        return Ok(true);
    }
    match current.iter().find(|row| row.status == "active") {
        Some(active) => {
            let (issuer, external_id) = (pending.issuer.clone(), pending.external_id.clone());
            rebind_active(transaction, agent, active, &issuer, &external_id, by).await?;
        }
        None => {
            sqlx::query(
                "UPDATE agent_outside_refs SET status = 'active', attached_by_actor_id = $2, \
                 attached_at = now(), version = version + 1 WHERE id = $1",
            )
            .bind(pending.id)
            .bind(by)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Operations

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRefAttachInput {
    /// The agent account: its id or a link to it.
    pub agent: IdRef,
    /// Who issued the outside id, as the agent's host or platform names
    /// itself.
    pub issuer: String,
    /// The id that issuer gave the agent.
    pub external_id: String,
}

impl Targeted for AgentRefAttachInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Say which agent elsewhere an agent account is.
pub struct AgentRefAttach;

impl Operation for AgentRefAttach {
    const NAME: &'static str = "agent_ref_set";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["account.ref_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Attach an agent's outside reference";
    const DESCRIPTION: &'static str = "Record which agent elsewhere an agent account is: the \
        issuer of its outside id and the id. An agent has at most one; to move it to a new pair \
        use agent_ref_update. No other agent may hold the same pair. You must manage the agent.";

    type Input = AgentRefAttachInput;
    type Output = AgentRefView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentRefAttachInput,
    ) -> Result<AgentRefView, OpError> {
        let agent = input.agent.id();
        let issuer = part(&input.issuer, "issuer")?;
        let external_id = part(&input.external_id, "externalId")?;
        let (workspace_id, caller) = (call.ctx.workspace_id, call.ctx.caller.clone());
        let tx = call.tx().await?;
        require_managed(tx, &caller, workspace_id, agent).await?;
        let current = rows(tx, agent).await?;
        if let Some(active) = current.iter().find(|row| row.status == "active") {
            if active.issuer == issuer && active.external_id == external_id {
                return Ok(view(tx, agent).await?);
            }
            return Err(OpError::invalid(
                "agent",
                "This agent already has an outside reference. Rebind it with agent_ref_update.",
            ));
        }
        if held_elsewhere(tx, agent, &issuer, &external_id).await? {
            return Err(OpError::invalid(
                "externalId",
                "Another agent already has that reference.",
            ));
        }
        sqlx::query(
            "INSERT INTO agent_outside_refs \
                 (id, account_id, issuer, external_id, status, attached_by_actor_id) \
             VALUES ($1, $2, $3, $4, 'active', $5)",
        )
        .bind(Uuid::now_v7())
        .bind(agent)
        .bind(&issuer)
        .bind(&external_id)
        .bind(caller.account_id)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target("account", agent, json!({ "ref": "attached" }));
        let tx = call.tx().await?;
        Ok(view(tx, agent).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRefRebindInput {
    /// The agent account: its id or a link to it.
    pub agent: IdRef,
    /// The issuer of its new outside id.
    pub issuer: String,
    /// Its new outside id.
    pub external_id: String,
}

impl Targeted for AgentRefRebindInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Move an agent's outside reference to a new pair.
pub struct AgentRefRebind;

impl Operation for AgentRefRebind {
    const NAME: &'static str = "agent_ref_update";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["account.ref_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Rebind an agent's outside reference";
    const DESCRIPTION: &'static str = "Move an agent account's outside reference to a new issuer \
        and id, when the agent was moved or re-keyed. The account, its roles, its name and \
        everything it wrote stay; the old pair is kept in the reference's lineage. A rebind a \
        host proposed is settled with agent_ref_decide instead. You must manage the agent.";

    type Input = AgentRefRebindInput;
    type Output = AgentRefView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentRefRebindInput,
    ) -> Result<AgentRefView, OpError> {
        let agent = input.agent.id();
        let issuer = part(&input.issuer, "issuer")?;
        let external_id = part(&input.external_id, "externalId")?;
        let (workspace_id, caller) = (call.ctx.workspace_id, call.ctx.caller.clone());
        let tx = call.tx().await?;
        require_managed(tx, &caller, workspace_id, agent).await?;
        let current = rows(tx, agent).await?;
        let Some(active) = current.into_iter().find(|row| row.status == "active") else {
            return Err(OpError::invalid(
                "agent",
                "This agent has no outside reference yet. Attach one with agent_ref_set.",
            ));
        };
        if active.issuer == issuer && active.external_id == external_id {
            return Ok(view(tx, agent).await?);
        }
        if held_elsewhere(tx, agent, &issuer, &external_id).await? {
            return Err(OpError::invalid(
                "externalId",
                "Another agent already has that reference.",
            ));
        }
        rebind_active(tx, agent, &active, &issuer, &external_id, caller.account_id).await?;
        call.audit_target("account", agent, json!({ "ref": "rebound" }));
        let tx = call.tx().await?;
        Ok(view(tx, agent).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRefDecideInput {
    /// The agent account: its id or a link to it.
    pub agent: IdRef,
    /// `true` makes the proposed pair its reference; `false` sets it aside.
    pub accept: bool,
}

impl Targeted for AgentRefDecideInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Accept or reject the rebind a host proposed for an agent.
pub struct AgentRefDecide;

impl Operation for AgentRefDecide {
    const NAME: &'static str = "agent_ref_decide";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["account.ref_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Decide on an agent's proposed rebind";
    const DESCRIPTION: &'static str = "Accept or reject the new outside reference a host \
        proposed for an agent account (the host's request asks its managers the same thing). \
        Accepting keeps the account and moves its old pair into the lineage; rejecting \
        leaves the reference as it was. You must manage the agent.";

    type Input = AgentRefDecideInput;
    type Output = AgentRefView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentRefDecideInput,
    ) -> Result<AgentRefView, OpError> {
        let agent = input.agent.id();
        let (workspace_id, caller) = (call.ctx.workspace_id, call.ctx.caller.clone());
        let tx = call.tx().await?;
        require_managed(tx, &caller, workspace_id, agent).await?;
        if !decide(tx, agent, input.accept, caller.account_id).await? {
            return Err(OpError::invalid(
                "agent",
                "No rebind is waiting for this agent.",
            ));
        }
        call.audit_target(
            "account",
            agent,
            json!({ "ref": if input.accept { "rebind_accepted" } else { "rebind_rejected" } }),
        );
        let tx = call.tx().await?;
        Ok(view(tx, agent).await?)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRefDetachInput {
    /// The agent account: its id or a link to it.
    pub agent: IdRef,
}

impl Targeted for AgentRefDetachInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Stop saying which agent elsewhere an agent account is.
pub struct AgentRefDetach;

impl Operation for AgentRefDetach {
    const NAME: &'static str = "agent_ref_remove";
    // Recorded in the audit row, which K3's log reads.
    const EMITS: &'static [&'static str] = &["account.ref_changed"];
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Detach an agent's outside reference";
    const DESCRIPTION: &'static str = "Remove an agent account's outside reference and any \
        rebind waiting for it. The account and everything it did stay. You must manage the agent.";

    type Input = AgentRefDetachInput;
    type Output = AgentRefView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentRefDetachInput,
    ) -> Result<AgentRefView, OpError> {
        let agent = input.agent.id();
        let (workspace_id, caller) = (call.ctx.workspace_id, call.ctx.caller.clone());
        let tx = call.tx().await?;
        require_managed(tx, &caller, workspace_id, agent).await?;
        sqlx::query(
            "UPDATE agent_outside_refs SET status = 'detached', detached_at = now(), \
             version = version + 1 WHERE account_id = $1 AND status <> 'detached'",
        )
        .bind(agent)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        call.audit_target("account", agent, json!({ "ref": "detached" }));
        let tx = call.tx().await?;
        Ok(view(tx, agent).await?)
    }
}
