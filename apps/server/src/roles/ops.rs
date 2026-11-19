//! Roles and permissions as operations (K2.17): `role_list`, `role_get`,
//! `role_create`, `role_update`, `role_delete`, `role_assign`,
//! `agent_limit_set` and `permissions_get`. The Roles and Members pages,
//! agents and apps run these; each wraps the domain function in this
//! module, which decides who may do what, and audits the change with what
//! it was before and after.
//!
//! Anyone in a workspace reads its roles and what they themselves may do.
//! Owners and admins (whoever holds "Manage roles") make and change roles;
//! only owners make or change one based on Admin, and nobody gives a role
//! more than they hold. Assigning takes "Manage members".

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    Assignment, Auditor, ModuleEntry, PermissionEntry, PermissionsView, RoleChange, RoleDefaults,
    RoleInput, RoleView, StartingPoint,
};
use crate::{
    error::AppError,
    events::catalog::{ROLE_CHANGED, WORKSPACE_MEMBER_CHANGED},
    ops::{
        DenyReason, Edge, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
    policy::Permission,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<RoleList>()
        .add::<RoleGet>()
        .add::<RoleCreate>()
        .add::<RoleUpdate>()
        .add::<RoleDelete>()
        .add::<RoleAssign>()
        .add::<AgentLimitSet>()
        .add::<PermissionsGet>();
}

/// The role `text` names in the workspace: its id, key or name.
async fn role_named(
    connection: &mut PgConnection,
    workspace: Uuid,
    text: &str,
) -> Result<Uuid, OpError> {
    super::find_role(connection, workspace, text)
        .await?
        .ok_or_else(|| OpError::not_found("That role"))
}

/// The member `text` names in the workspace: their id or @handle.
async fn member_named(
    connection: &mut PgConnection,
    workspace: Uuid,
    text: &str,
) -> Result<Uuid, OpError> {
    let text = text.trim();
    sqlx::query_scalar::<_, Uuid>(
        "SELECT profile.actor_id FROM organization_actor_profiles profile \
         WHERE profile.organization_id = $1 \
           AND (profile.actor_id::text = $2 OR lower(profile.handle) = lower($3)) \
         LIMIT 1",
    )
    .bind(workspace)
    .bind(text)
    .bind(text.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That account in this workspace"))
}

/// A refusal from the role rules, said in words.
fn refused(error: AppError, message: &'static str) -> OpError {
    match error {
        AppError::Forbidden => OpError::Denied {
            reason: DenyReason::Rule,
            message: message.into(),
        },
        error => error.into(),
    }
}

/// Tells the workspace's open tabs and listeners that a role or someone's
/// standing changed (ids only).
async fn changed(
    connection: &mut PgConnection,
    ty: &'static crate::events::EventType,
    workspace: Uuid,
    kind: &'static str,
    id: Uuid,
    by: Uuid,
) -> Result<(), OpError> {
    let mut event = crate::events::NewEvent::thing(ty, workspace, kind, id)
        .audience(crate::events::Audience::Workspace)
        .workspace(workspace)
        .actor(by);
    event.space_id = None;
    crate::events::emit(connection, event).await?;
    Ok(())
}

const ROLE: &str = "role";
const ACCOUNT: &str = "account";

const MAY_NOT_DEFINE: &str = "You can't make that change. Owners and admins manage roles; only \
    owners manage roles based on admin, and nobody gives a role more than they hold.";

// ---------------------------------------------------------------------------
// role_list

/// List the workspace's roles.
pub struct RoleList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleListInput {}

impl Targeted for RoleListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleListOutput {
    pub roles: Vec<RoleView>,
    /// The modules permissions are grouped by.
    pub modules: Vec<ModuleEntry>,
    /// Every permission a role can hold.
    pub permissions: Vec<PermissionEntry>,
    /// Ready-made custom roles to start from.
    pub starting_points: Vec<StartingPoint>,
    /// Whether you may make and change roles.
    pub can_manage: bool,
    /// Whether you may also make or change roles based on admin.
    pub can_manage_admin_roles: bool,
    /// What you may do in the workspace.
    pub your_permissions: Vec<&'static str>,
}

impl Operation for RoleList {
    const NAME: &'static str = "role_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List roles";
    const DESCRIPTION: &'static str = "The workspace's roles (built-in and custom) with their \
        permissions, agent limits, defaults and holders, and every permission a role can hold.";

    type Input = RoleListInput;
    type Output = RoleListOutput;

    async fn run(call: &mut OpCall<'_>, _input: RoleListInput) -> Result<RoleListOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let roles = super::list(tx, caller, workspace).await?;
        let (modules, permissions) = super::catalogue();
        Ok(RoleListOutput {
            roles,
            modules,
            permissions,
            starting_points: super::starting_points(),
            can_manage: caller.may(Permission::RolesManage),
            can_manage_admin_roles: caller.may(Permission::RolesManage) && caller.is_owner(),
            your_permissions: caller.permissions.keys(),
        })
    }
}

// ---------------------------------------------------------------------------
// role_get

/// Read one role.
pub struct RoleGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleGetInput {
    /// The role's name, key (such as "member") or id.
    pub role: String,
}

impl Targeted for RoleGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for RoleGet {
    const NAME: &'static str = "role_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get role";
    const DESCRIPTION: &'static str = "One role: its permissions, agent limit, defaults, holders, \
        version and what you may do with it.";

    type Input = RoleGetInput;
    type Output = RoleView;

    async fn run(call: &mut OpCall<'_>, input: RoleGetInput) -> Result<RoleView, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let role = role_named(tx, workspace, &input.role).await?;
        Ok(super::read(tx, caller, workspace, role).await?)
    }
}

// ---------------------------------------------------------------------------
// role_create

/// Make a custom role.
pub struct RoleCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleCreateInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What it starts from: admin, member or viewer. It keeps only
    /// permissions its base has; only owners make roles based on admin.
    pub base_role: String,
    /// Permission keys (role_list lists them), such as "tasks.edit".
    pub permissions: Vec<String>,
    /// What apps and agents acting for holders may do, as permission keys.
    /// Left out, they may do what holders may.
    #[serde(default)]
    pub agent_limit: Option<Vec<String>>,
    /// Where holders start.
    #[serde(default)]
    pub defaults: RoleDefaults,
    /// Modules holders don't get, on top of what the workspace has off.
    #[serde(default)]
    pub modules_off: Vec<String>,
    /// Holders acting on someone's request (such as a run someone asked
    /// for) do only what that account could also do.
    #[serde(default)]
    pub act_within_requester: bool,
    /// The role new members and invitations start with.
    #[serde(default)]
    pub is_default: bool,
}

impl Targeted for RoleCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for RoleCreate {
    const NAME: &'static str = "role_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::RolesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Create role";
    const DESCRIPTION: &'static str = "Make a custom role from admin, member or viewer, holding \
        some of its base's permissions.";

    const EMITS: &'static [&'static str] = &["role.changed"];

    type Input = RoleCreateInput;
    type Output = RoleView;

    async fn run(call: &mut OpCall<'_>, input: RoleCreateInput) -> Result<RoleView, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let ctx = call.ctx;
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let view = super::create(
            tx,
            &Auditor::Op(ctx),
            caller,
            workspace,
            RoleInput {
                name: input.name,
                description: input.description,
                base_role: input.base_role,
                permissions: input.permissions,
                agent_limit: input.agent_limit,
                defaults: input.defaults,
                modules_off: input.modules_off,
                act_within_requester: input.act_within_requester,
                is_default: input.is_default,
            },
        )
        .await
        .map_err(|error| refused(error, MAY_NOT_DEFINE))?;
        changed(tx, &ROLE_CHANGED, workspace, ROLE, view.id, account).await?;
        call.audited_by_command();
        Ok(view)
    }
}

// ---------------------------------------------------------------------------
// role_update

/// Change a role.
pub struct RoleUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleUpdateInput {
    /// The role's name, key or id.
    pub role: String,
    /// Its version (role_get), so a change made meanwhile isn't undone.
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Permission keys. Built-in owner keeps everything.
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// What apps and agents acting for holders may do; an empty list leaves
    /// them reading only.
    #[serde(default)]
    pub agent_limit: Option<Vec<String>>,
    /// True takes the agent limit away (apps may do what holders may).
    #[serde(default)]
    pub clear_agent_limit: bool,
    #[serde(default)]
    pub defaults: Option<RoleDefaults>,
    /// Modules holders don't get; an empty list gives them every module the
    /// workspace has on. Never for the owner role.
    #[serde(default)]
    pub modules_off: Option<Vec<String>>,
    #[serde(default)]
    pub act_within_requester: Option<bool>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

impl Targeted for RoleUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for RoleUpdate {
    const NAME: &'static str = "role_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::RolesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update role";
    const DESCRIPTION: &'static str = "Change a role's name, description, permissions, agent \
        limit, defaults or modules. What's left out stays.";

    const EMITS: &'static [&'static str] = &["role.changed"];

    type Input = RoleUpdateInput;
    type Output = RoleView;

    async fn run(call: &mut OpCall<'_>, input: RoleUpdateInput) -> Result<RoleView, OpError> {
        if input.clear_agent_limit && input.agent_limit.is_some() {
            return Err(OpError::invalid(
                "clearAgentLimit",
                "Give agentLimit or clearAgentLimit, not both.",
            ));
        }
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let ctx = call.ctx;
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let role = role_named(tx, workspace, &input.role).await?;
        let agent_limit = if input.clear_agent_limit {
            Some(None)
        } else {
            input.agent_limit.map(Some)
        };
        let view = super::change(
            tx,
            &Auditor::Op(ctx),
            caller,
            workspace,
            role,
            RoleChange {
                expected_version: Some(input.expected_version),
                name: input.name,
                description: input.description,
                permissions: input.permissions,
                agent_limit,
                defaults: input.defaults,
                modules_off: input.modules_off,
                act_within_requester: input.act_within_requester,
                is_default: input.is_default,
            },
        )
        .await
        .map_err(|error| refused(error, MAY_NOT_DEFINE))?;
        changed(tx, &ROLE_CHANGED, workspace, ROLE, view.id, account).await?;
        call.audited_by_command();
        Ok(view)
    }
}

// ---------------------------------------------------------------------------
// role_delete

/// Remove a custom role nobody holds.
pub struct RoleDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleDeleteInput {
    /// The role's name, key or id.
    pub role: String,
}

impl Targeted for RoleDeleteInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleDeleteOutput {
    #[schemars(with = "String")]
    pub role_id: Uuid,
}

impl Operation for RoleDelete {
    const NAME: &'static str = "role_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::RolesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Delete role";
    const DESCRIPTION: &'static str =
        "Remove a custom role. Built-in roles stay, and a role someone holds can't go.";

    const EMITS: &'static [&'static str] = &["role.changed"];

    type Input = RoleDeleteInput;
    type Output = RoleDeleteOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: RoleDeleteInput,
    ) -> Result<RoleDeleteOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let ctx = call.ctx;
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let role = role_named(tx, workspace, &input.role).await?;
        super::remove(tx, &Auditor::Op(ctx), caller, workspace, role)
            .await
            .map_err(|error| refused(error, MAY_NOT_DEFINE))?;
        changed(tx, &ROLE_CHANGED, workspace, ROLE, role, account).await?;
        call.audited_by_command();
        Ok(RoleDeleteOutput { role_id: role })
    }
}

// ---------------------------------------------------------------------------
// role_assign

/// Give someone a workspace role.
pub struct RoleAssign;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleAssignInput {
    /// Who: an @handle or account id of someone in the workspace.
    pub account: String,
    /// The workspace role to give them: its name, key or id. For a role in
    /// one space only, use member_update.
    pub role: String,
    /// True gives them the role in each of their spaces too (where they
    /// aren't the owner).
    #[serde(default)]
    pub apply_to_projects: bool,
    /// Their membership's version, so a change made meanwhile isn't undone.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

impl Targeted for RoleAssignInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for RoleAssign {
    const NAME: &'static str = "role_assign";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Assign role";
    const DESCRIPTION: &'static str = "Give someone a workspace role, and with applyToProjects \
        in their spaces too. Nobody changes their own role or an owner's, and only owners give \
        roles based on admin.";

    const EMITS: &'static [&'static str] = &["workspace.member_changed"];

    type Input = RoleAssignInput;
    type Output = Assignment;

    async fn run(call: &mut OpCall<'_>, input: RoleAssignInput) -> Result<Assignment, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let ctx = call.ctx;
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let target = member_named(tx, workspace, &input.account).await?;
        let role = role_named(tx, workspace, &input.role).await?;
        let assignment = super::assign(
            tx,
            &Auditor::Op(ctx),
            caller,
            workspace,
            target,
            role,
            input.apply_to_projects,
            input.expected_version,
        )
        .await
        .map_err(|error| {
            refused(
                error,
                "You can't give that role. Owners and admins assign roles; only owners change an \
                 admin's role or give one based on admin, and nobody gives a role that holds \
                 more than they do.",
            )
        })?;
        changed(
            tx,
            &WORKSPACE_MEMBER_CHANGED,
            workspace,
            ACCOUNT,
            target,
            account,
        )
        .await?;
        call.audited_by_command();
        Ok(assignment)
    }
}

// ---------------------------------------------------------------------------
// agent_limit_set

/// Cap what someone's apps may do.
pub struct AgentLimitSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentLimitSetInput {
    /// Whose apps: an @handle or account id. Left out, your own.
    #[serde(default)]
    pub account: Option<String>,
    /// The permission keys their apps are capped to, whatever their role
    /// allows.
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// True removes the cap (instead of permissions).
    #[serde(default)]
    pub lift: bool,
}

impl Targeted for AgentLimitSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentLimitView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// The keys their apps are capped to now; absent for no cap.
    pub permissions: Option<Vec<&'static str>>,
}

impl Operation for AgentLimitSet {
    const NAME: &'static str = "agent_limit_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set agent limit";
    const DESCRIPTION: &'static str = "Cap what apps connected by a person may do, or lift the \
        cap. Your own, or anyone's if you manage members (an owner for an admin's). Through an \
        app you can only narrow the cap it runs under; lifting it takes the web.";

    const EMITS: &'static [&'static str] = &["workspace.member_changed"];

    type Input = AgentLimitSetInput;
    type Output = AgentLimitView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentLimitSetInput,
    ) -> Result<AgentLimitView, OpError> {
        let limit = match (input.permissions, input.lift) {
            (Some(keys), false) => Some(keys),
            (None, true) => None,
            _ => {
                return Err(OpError::invalid(
                    "permissions",
                    "Give the permissions to cap them to, or lift: true.",
                ));
            }
        };
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let ctx = call.ctx;
        // A call from the web is the person; any other edge is an app.
        let through_app = !matches!(ctx.edge, Edge::Web { .. });
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let target = match input.account.as_deref() {
            Some(text) => member_named(tx, workspace, text).await?,
            None => account,
        };
        if through_app && target == account {
            // Lock the membership before reading the cap, so a change that
            // commits meanwhile can't be undone by this one.
            let current = sqlx::query_scalar::<_, Option<Vec<String>>>(
                "SELECT agent_limit FROM organization_memberships \
                 WHERE organization_id = $1 AND actor_id = $2 FOR UPDATE",
            )
            .bind(workspace)
            .bind(account)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?
            .flatten();
            if let Some(current) = current {
                let widens = match &limit {
                    None => true,
                    Some(keys) => keys.iter().any(|key| !current.contains(key)),
                };
                if widens {
                    return Err(OpError::Denied {
                        reason: DenyReason::Rule,
                        message: "An app can only narrow the cap it runs under. Lift or widen \
                            it on the web, in your workspace settings."
                            .into(),
                    });
                }
            }
        }
        let permissions = super::set_agent_limit(
            tx,
            &Auditor::Op(ctx),
            caller,
            workspace,
            target,
            limit,
        )
        .await
        .map_err(|error| {
            refused(
                error,
                "You can't change that cap. Your own is yours; anyone else's takes managing \
                     members (an owner for an admin's).",
            )
        })?;
        changed(
            tx,
            &WORKSPACE_MEMBER_CHANGED,
            workspace,
            ACCOUNT,
            target,
            account,
        )
        .await?;
        call.audited_by_command();
        Ok(AgentLimitView {
            account_id: target,
            permissions,
        })
    }
}

// ---------------------------------------------------------------------------
// permissions_get

/// What someone may do.
pub struct PermissionsGet;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionsGetInput {
    /// Whose: an @handle or account id. Left out, yours. Someone else's
    /// takes seeing the member list.
    #[serde(default)]
    pub account: Option<String>,
    /// A space to check too: its id or link.
    #[serde(default)]
    pub space_id: Option<IdRef>,
}

impl Targeted for PermissionsGetInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

impl Operation for PermissionsGet {
    const NAME: &'static str = "permissions_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get permissions";
    const DESCRIPTION: &'static str = "What someone may do in the workspace and, with spaceId, \
        in a space: their role's permissions, and what apps and agents acting for them may \
        (forApps, cut down by agent limits). Left out, yours.";

    type Input = PermissionsGetInput;
    type Output = PermissionsView;

    async fn run(
        call: &mut OpCall<'_>,
        input: PermissionsGetInput,
    ) -> Result<PermissionsView, OpError> {
        let workspace = call.ctx.workspace_id;
        let space = call.ctx.space_id;
        let account = call.account_id();
        let tx = call.tx().await?;
        let caller = super::standing(tx, workspace, account).await?;
        let whose = match input.account.as_deref() {
            Some(text) => member_named(tx, workspace, text).await?,
            None => account,
        };
        if !super::may_read_permissions_of(caller, whose) {
            return Err(OpError::not_found("That account in this workspace"));
        }
        Ok(super::permissions_of(tx, workspace, whose, space).await?)
    }
}
