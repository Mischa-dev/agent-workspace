//! Account operations (K1.4): naming who manages an agent or app account.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    access::{self, Caller, Need, Target as AccessTarget},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
    policy::Permission,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<AgentManagersSet>();
}

/// Name or un-name the managers of an agent or app account.
pub struct AgentManagersSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentManagersSetInput {
    /// The agent or app account: its id or a link to it.
    pub agent: IdRef,
    /// Accounts to name as managers. Each must be able to manage agents in
    /// this workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<IdRef>,
    /// Accounts to stop naming as managers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<IdRef>,
}

impl Targeted for AgentManagersSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentManagersView {
    #[schemars(with = "String")]
    pub agent: Uuid,
    /// The accounts named as its managers now.
    #[schemars(with = "Vec<String>")]
    pub managers: Vec<Uuid>,
}

impl Operation for AgentManagersSet {
    const NAME: &'static str = "agent_managers_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set agent managers";
    const DESCRIPTION: &'static str = "Name or stop naming the accounts that manage an agent or \
        app account (its profile, credentials and connections). You must be able to manage the \
        account yourself, and each account you add must be able to manage agents in this \
        workspace. Returns the managers named now.";
    // Recorded in the audit row; the outbox is per space, so the event
    // itself waits for K3's log.
    const EMITS: &'static [&'static str] = &["account.managers.changed"];

    type Input = AgentManagersSetInput;
    type Output = AgentManagersView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentManagersSetInput,
    ) -> Result<AgentManagersView, OpError> {
        if input.add.is_empty() && input.remove.is_empty() {
            return Err(OpError::invalid(
                "add",
                "Name at least one account to add or remove.",
            ));
        }
        let agent = input.agent.id();
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        // The agent must be in this workspace, and the caller must manage it.
        let member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_memberships \
             WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
        )
        .bind(workspace_id)
        .bind(agent)
        .fetch_one(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        if !member {
            return Err(OpError::not_found("That agent"));
        }
        match access::may_manage_account(&mut **tx, &caller, agent).await? {
            Ok(_) => {}
            Err(access::Denied::NotFound) => return Err(OpError::not_found("That agent")),
            Err(_) => {
                return Err(OpError::invalid(
                    "agent",
                    "You can't manage that agent. Ask its owner or one of its managers.",
                ));
            }
        }
        for manager in &input.add {
            let manager = manager.id();
            if manager == agent {
                return Err(OpError::invalid("add", "An account can't manage itself."));
            }
            let kind = sqlx::query_scalar::<_, String>(
                "SELECT kind FROM actors WHERE id = $1 AND status = 'active'",
            )
            .bind(manager)
            .fetch_optional(&mut **tx)
            .await
            .map_err(crate::error::AppError::from)?;
            let Some(kind) = kind else {
                return Err(OpError::not_found("That account"));
            };
            let as_manager = Caller::person(manager, crate::accounts::actor_kind(&kind)?);
            let held = access::check(
                &mut **tx,
                &as_manager,
                Need::permission(Permission::AgentsManage),
                AccessTarget::Workspace(workspace_id),
            )
            .await?;
            match held {
                Ok(_) => {}
                Err(access::Denied::NotFound) => {
                    return Err(OpError::not_found("That account"));
                }
                Err(_) => {
                    return Err(OpError::invalid(
                        "add",
                        "That account can't manage agents in this workspace. Give them a role \
                         that can first.",
                    ));
                }
            }
            sqlx::query(
                "INSERT INTO account_managers \
                     (account_id, manager_account_id, workspace_id, added_by_actor_id) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            )
            .bind(agent)
            .bind(manager)
            .bind(workspace_id)
            .bind(caller.account_id)
            .execute(&mut **tx)
            .await
            .map_err(crate::error::AppError::from)?;
        }
        let removed = input.remove.iter().map(|id| id.id()).collect::<Vec<_>>();
        sqlx::query(
            "DELETE FROM account_managers WHERE account_id = $1 AND manager_account_id = ANY($2)",
        )
        .bind(agent)
        .bind(&removed)
        .execute(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let managers = sqlx::query_scalar::<_, Uuid>(
            "SELECT manager_account_id FROM account_managers WHERE account_id = $1 \
             ORDER BY created_at, manager_account_id",
        )
        .bind(agent)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let added = input.add.iter().map(|id| id.id()).collect::<Vec<_>>();
        call.audit_target(
            "account",
            agent,
            json!({ "added": added, "removed": removed }),
        );
        Ok(AgentManagersView { agent, managers })
    }
}
