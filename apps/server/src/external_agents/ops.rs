//! Making an agent at workspace level (K1.6). An agent is an account of the
//! workspace, like a person: it holds a workspace role, is first added to
//! the workspace's own space, and joins further spaces the way anyone does
//! (`project_member_set`). Its creator is named as one of its managers, and
//! the workspace owns it unless an owner is named. What it may do is decided
//! by its role, never by its kind.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    access::Scope,
    accounts::{NewAgent, Owner, create_agent},
    error::AppError,
    invitations::engine,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

pub fn register_ops(registry: &mut crate::ops::Registry) {
    registry.add::<AgentCreate>();
}

/// Make an agent account in this workspace, with its first credential.
pub struct AgentCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCreateInput {
    /// Its name here.
    pub display_name: String,
    /// Its handle here, for mentions.
    pub handle: String,
    /// Its credential's label. Defaults to the display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_name: Option<String>,
    /// Its role here. Default: the workspace's default role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<IdRef>,
    /// Credential scopes, with `read`. Default: read, write, events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// Its owner's account. Default: the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_account_id: Option<IdRef>,
}

impl Targeted for AgentCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentCreateView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    #[schemars(with = "String")]
    pub connection_id: Uuid,
    /// The workspace's own space.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    /// The bearer credential, shown only in the answer that made the agent;
    /// absent when a retry found the agent already made. A lost credential
    /// can't be shown again: rotate it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_expires_at: Option<String>,
}

impl Operation for AgentCreate {
    const NAME: &'static str = "agent_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsCreate);
    // The answer carries the credential, which must not be stored for a
    // replay: the connection keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["project.membership.updated"];
    const TITLE: &'static str = "Make an agent in this workspace";
    const DESCRIPTION: &'static str = "Make an agent in this workspace with a role within \
        yours. It starts in the workspace's own space and joins projects like anyone. You \
        manage it. Returns its credential once.";

    type Input = AgentCreateInput;
    type Output = AgentCreateView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentCreateInput,
    ) -> Result<AgentCreateView, OpError> {
        let display_name = input.display_name.trim().to_owned();
        if display_name.is_empty()
            || display_name.chars().count() > 80
            || display_name.chars().any(char::is_control)
        {
            return Err(OpError::invalid(
                "displayName",
                "Give a name of 1 to 80 characters on one line.",
            ));
        }
        let handle = crate::workspace::normalize_handle(&input.handle).map_err(|_| {
            OpError::invalid(
                "handle",
                "Use 2 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
            )
        })?;
        let connection_name = input
            .connection_name
            .as_deref()
            .unwrap_or(&display_name)
            .trim()
            .to_owned();
        if connection_name.is_empty()
            || connection_name.chars().count() > 80
            || !connection_name
                .chars()
                .enumerate()
                .all(|(index, character)| {
                    character.is_ascii_alphanumeric()
                        || (index > 0 && matches!(character, '.' | '_' | '-' | ' '))
                })
        {
            return Err(OpError::invalid(
                "connectionName",
                "Use 1 to 80 letters, digits, spaces, ., _ or -, starting with a letter or digit.",
            ));
        }
        let scopes = super::normalize_scopes(
            input
                .scopes
                .unwrap_or_else(|| ["read", "write", "events"].map(str::to_owned).to_vec()),
        )
        .map_err(|_| {
            OpError::invalid(
                "scopes",
                "Include read, and only read, write, events, approval_respond or worker_request.",
            )
        })?;
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let request_id = call.ctx.request_id.clone();
        let tx = call.tx().await?;
        if let Some(request_id) = request_id.as_deref() {
            let made = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
                "SELECT id, actor_id, project_id FROM external_agent_connections \
                 WHERE workspace_id = $1 AND created_by_actor_id = $2 AND request_id = $3",
            )
            .bind(workspace_id)
            .bind(caller.account_id)
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if let Some((connection_id, account_id, space_id)) = made {
                call.mark_replayed(true);
                call.audit_target("account", account_id, json!({ "replayed": true }));
                return Ok(AgentCreateView {
                    account_id,
                    connection_id,
                    space_id,
                    credential: None,
                    credential_expires_at: None,
                });
            }
        }
        let (role, role_id) = engine::agent_role(tx, workspace_id, input.role_id.map(IdRef::id))
            .await?
            .ok_or_else(|| OpError::not_found("That role"))?;
        if engine::is_ownership(&role) {
            return Err(OpError::invalid(
                "roleId",
                "Ownership is handed over with Transfer ownership, not given on arrival.",
            ));
        }
        if !engine::issuer_may_grant(tx, &caller, Scope::Workspace(workspace_id), &role, role_id)
            .await?
        {
            return Err(OpError::invalid(
                "roleId",
                "You can only give a role within what you hold yourself.",
            ));
        }
        let owner = match input.owner_account_id {
            None => Owner::Workspace(workspace_id),
            Some(owner) => {
                // The owner is someone here: an enabled member of this
                // workspace with an active account.
                let member = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS (
                        SELECT 1 FROM organization_memberships membership
                        JOIN actors account ON account.id = membership.actor_id
                          AND account.status = 'active'
                        WHERE membership.organization_id = $1 AND membership.actor_id = $2
                          AND membership.enabled
                    )
                    "#,
                )
                .bind(workspace_id)
                .bind(owner.id())
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?;
                if !member {
                    return Err(OpError::not_found("That owner"));
                }
                Owner::Account(owner.id())
            }
        };
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
             WHERE organization_id = $1 AND lower(handle) = lower($2))",
        )
        .bind(workspace_id)
        .bind(&handle)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if taken {
            return Err(OpError::invalid(
                "handle",
                "Someone here already has that handle. Choose another.",
            ));
        }
        let account_id = create_agent(
            tx,
            NewAgent {
                workspace_id,
                role: &role,
                role_id,
                owner,
                created_by: caller.account_id,
            },
        )
        .await
        .map_err(AppError::from)?;
        sqlx::query(
            "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(&display_name)
        .bind(&handle)
        .execute(&mut **tx)
        .await
        .map_err(super::map_database_error)?;
        let space_id = engine::join_workspace_space(tx, workspace_id, account_id).await?;
        let (connection_id, credential) = super::connect(
            tx,
            workspace_id,
            space_id,
            account_id,
            caller.account_id,
            &connection_name,
            &scopes,
        )
        .await?;
        if let Some(request_id) = request_id.as_deref() {
            sqlx::query("UPDATE external_agent_connections SET request_id = $2 WHERE id = $1")
                .bind(connection_id)
                .bind(request_id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        }
        call.audit_target(
            "account",
            account_id,
            json!({
                "role": role,
                "roleId": role_id,
                "connectionId": connection_id,
                "scopes": scopes,
            }),
        );
        Ok(AgentCreateView {
            account_id,
            connection_id,
            space_id,
            credential: Some(credential.token),
            credential_expires_at: Some(credential.expires_at),
        })
    }
}
