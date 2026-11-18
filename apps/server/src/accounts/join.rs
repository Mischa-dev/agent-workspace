//! Adding an account you manage to this workspace (A2.18; K1 3.11, ruling
//! R19). Accounts are global: an agent or app made in one workspace joins
//! another by a membership with a role, the same act as for anyone. The
//! adder needs `members.invite` here and a role within what they hold, and
//! must manage the account (that is the manager's agreement). The
//! workspace's `access.outside_accounts` setting decides what happens to an
//! account owned outside it: let it in, ask an admin (an approval request),
//! or refuse. Accounts owned by this workspace or by one of its members may
//! always join. Nothing here says where else an account is a member.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    access::{self, Caller, Scope},
    accounts::{self, Owner},
    error::AppError,
    invitations::engine::{self, Admission, Admitted},
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

/// The subject of the approval request an "ask an admin" join raises.
pub(crate) const JOIN_SUBJECT: &str = "account_join";
/// The workspace setting on accounts owned outside it.
pub const OUTSIDE_SETTING: &str = "access.outside_accounts";

pub fn register_ops(registry: &mut Registry) {
    registry.add::<AccountWorkspaceAdd>();
    registry.add::<AccountJoinableList>();
}

/// What the workspace says about an account owned outside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outside {
    Yes,
    AskAdmin,
    No,
}

async fn outside_rule(
    connection: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Outside, AppError> {
    let stored = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM settings WHERE organization_id = $1 AND scope = 'workspace' AND key = $2",
    )
    .bind(workspace_id)
    .bind(OUTSIDE_SETTING)
    .fetch_optional(&mut *connection)
    .await?;
    let default = crate::settings::definition(OUTSIDE_SETTING)
        .map(crate::settings::SettingDef::default_value)
        .unwrap_or_default();
    Ok(
        match stored
            .as_ref()
            .unwrap_or(&default)
            .as_str()
            .unwrap_or("yes")
        {
            "no" => Outside::No,
            "ask_admin" => Outside::AskAdmin,
            _ => Outside::Yes,
        },
    )
}

/// Whether the account is owned here: by this workspace, or by an enabled
/// member of it.
async fn owned_here(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    owner: Option<Owner>,
) -> Result<bool, AppError> {
    Ok(match owner {
        Some(Owner::Workspace(owner)) => owner == workspace_id,
        Some(Owner::Account(owner)) => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM organization_memberships \
             WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
            )
            .bind(workspace_id)
            .bind(owner)
            .fetch_one(&mut *connection)
            .await?
        }
        None => false,
    })
}

/// The account's name: its name here if it has one, else the one it uses
/// elsewhere (never which workspace that is).
async fn name_of(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<Option<(String, String)>, AppError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT display_name, handle FROM organization_actor_profiles WHERE actor_id = $2 \
         ORDER BY (organization_id = $1) DESC, created_at LIMIT 1",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?)
}

async fn handle_taken(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
    handle: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
         WHERE organization_id = $1 AND lower(handle) = lower($2) AND actor_id <> $3)",
    )
    .bind(workspace_id)
    .bind(handle)
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// Let the account in with the role, into the workspace's own space.
async fn let_in(
    transaction: &mut Transaction<'_, Postgres>,
    admission: Admission<'_>,
) -> Result<(Admitted, Option<Uuid>), AppError> {
    let admitted = engine::admit(transaction, admission).await?;
    if admitted == Admitted::AlreadyMember {
        return Ok((admitted, None));
    }
    let space =
        engine::join_workspace_space(transaction, admission.workspace_id, admission.account_id)
            .await?;
    Ok((admitted, Some(space)))
}

// ------------------------------------------------------------------- add

/// Add an agent or app you manage, from another workspace, to this one.
pub struct AccountWorkspaceAdd;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountWorkspaceAddInput {
    /// An agent or app you manage.
    pub account: IdRef,
    /// Its role here. Default: the workspace's role for new agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<IdRef>,
    /// Its handle here, if its own is taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

impl Targeted for AccountWorkspaceAddInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountJoinView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// joined, returned, already_member, or asked (see requestId).
    pub status: &'static str,
    /// The workspace's own space it joined.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    /// The request an admin here answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub request_id: Option<Uuid>,
}

impl Operation for AccountWorkspaceAdd {
    const NAME: &'static str = "account_workspace_add";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["project.membership.updated", "request.raised"];
    const TITLE: &'static str = "Add an agent you have to this workspace";
    const DESCRIPTION: &'static str = "Add an agent or app you manage elsewhere to this \
        workspace, with a role within yours. `asked` means an admin here decides.";

    type Input = AccountWorkspaceAddInput;
    type Output = AccountJoinView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AccountWorkspaceAddInput,
    ) -> Result<AccountJoinView, OpError> {
        let account_id = input.account.id();
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        manages(tx, &caller, account_id).await?;
        let account = accounts::load(tx, account_id)
            .await?
            .filter(|account| account.active)
            .ok_or_else(|| OpError::not_found("That account"))?;
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
        let (display_name, handle) = name_of(tx, workspace_id, account_id)
            .await?
            .ok_or_else(|| OpError::not_found("That account"))?;
        let handle = match input.handle {
            Some(handle) => crate::workspace::normalize_handle(&handle).map_err(|_| {
                OpError::invalid(
                    "handle",
                    "Use 2 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
                )
            })?,
            None => handle,
        };
        if handle_taken(tx, workspace_id, account_id, &handle).await? {
            return Err(OpError::invalid(
                "handle",
                "Someone here already has that handle. Give it another one here.",
            ));
        }
        let admission = Admission {
            workspace_id,
            account_id,
            role: &role,
            role_id,
            display_name: &display_name,
            handle: &handle,
        };
        let enabled_here = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_memberships \
             WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
        )
        .bind(workspace_id)
        .bind(account_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if !enabled_here && !owned_here(tx, workspace_id, account.owner).await? {
            match outside_rule(tx, workspace_id).await? {
                Outside::Yes => {}
                Outside::No => {
                    return Err(OpError::Denied {
                        reason: DenyReason::Rule,
                        message: "This workspace doesn't let in agents or apps owned outside it."
                            .into(),
                    });
                }
                Outside::AskAdmin => {
                    let request_id = ask(tx, &caller, admission).await?;
                    call.audit_target(
                        "account",
                        account_id,
                        json!({ "asked": request_id, "role": role, "roleId": role_id }),
                    );
                    return Ok(AccountJoinView {
                        account_id,
                        status: "asked",
                        space_id: None,
                        request_id: Some(request_id),
                    });
                }
            }
        }
        let (admitted, space_id) = let_in(tx, admission).await?;
        call.audit_target(
            "account",
            account_id,
            json!({ "role": role, "roleId": role_id }),
        );
        Ok(AccountJoinView {
            account_id,
            status: match admitted {
                Admitted::Joined => "joined",
                Admitted::Returned => "returned",
                Admitted::AlreadyMember => "already_member",
            },
            space_id,
            request_id: None,
        })
    }
}

/// The caller manages the account, or it is `not_found` (as one they can't
/// see).
async fn manages(
    connection: &mut PgConnection,
    caller: &Caller,
    account_id: Uuid,
) -> Result<(), OpError> {
    match access::may_manage_account(connection, caller, account_id).await? {
        Ok(_) => Ok(()),
        Err(_) => Err(OpError::not_found("That account")),
    }
}

/// Raise the approval request an admin answers, or find the one already
/// waiting for this account here.
async fn ask(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    admission: Admission<'_>,
) -> Result<Uuid, OpError> {
    let waiting = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT request.id
        FROM account_join_requests ask
        JOIN requests request
          ON request.subject_kind = $3 AND request.subject_id = ask.id AND request.status = 'open'
        WHERE ask.workspace_id = $1 AND ask.account_id = $2 AND ask.status = 'pending'
        "#,
    )
    .bind(admission.workspace_id)
    .bind(admission.account_id)
    .bind(JOIN_SUBJECT)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    if let Some(request_id) = waiting {
        return Ok(request_id);
    }
    // One left pending by a request that ended some other way is closed.
    sqlx::query(
        "UPDATE account_join_requests SET status = 'cancelled', settled_at = now() \
         WHERE workspace_id = $1 AND account_id = $2 AND status = 'pending'",
    )
    .bind(admission.workspace_id)
    .bind(admission.account_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    let join_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO account_join_requests
            (id, workspace_id, account_id, asked_by_account_id, role, role_id, display_name, handle)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(join_id)
    .bind(admission.workspace_id)
    .bind(admission.account_id)
    .bind(caller.account_id)
    .bind(admission.role)
    .bind(admission.role_id)
    .bind(admission.display_name)
    .bind(admission.handle)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    // Asked in the workspace's own space, where its owners and admins hold
    // those roles.
    let space_id = crate::spaces::workspace::find(transaction, admission.workspace_id).await?;
    let raised = crate::requests::open(
        transaction,
        crate::requests::Origin {
            account_id: caller.account_id,
            workspace_id: admission.workspace_id,
            space_id,
            run_id: None,
            host_request_ref: None,
            conversation_id: None,
            work_item_id: None,
            subject: Some((JOIN_SUBJECT, join_id)),
            field: "account",
        },
        &json!({
            "kind": "approval",
            "title": "Add an agent from outside this workspace?",
            "detail": format!(
                "@{} ({}) is owned outside this workspace. Adding it gives it the role picked.",
                admission.handle, admission.display_name
            ),
            "options": [
                {"id": "approve", "label": "Add it", "style": "approve"},
                {"id": "deny", "label": "Don't add it", "style": "deny"},
            ],
            "allowText": false,
            "audience": {"roles": ["owner", "admin"]},
            "expiresInSeconds": 7 * 24 * 3600,
        }),
    )
    .await
    .map_err(|error| match error {
        crate::requests::OpenError::App(error) => OpError::from(error),
        crate::requests::OpenError::Refused { .. } => OpError::invalid(
            "account",
            "This workspace can't be asked right now. Try again.",
        ),
    })?;
    Ok(raised.request_id)
}

/// An admin answered (or the request ended): let the account in on
/// approval, and close the ask either way.
pub(crate) async fn settled(
    transaction: &mut Transaction<'_, Postgres>,
    row: &crate::requests::RequestRow,
) -> Result<(), AppError> {
    let Some(join_id) = row.subject_id else {
        return Ok(());
    };
    let Some(ask) = sqlx::query_as::<_, PendingJoin>(
        r#"
        SELECT workspace_id, account_id, role, role_id, display_name, handle
        FROM account_join_requests WHERE id = $1 AND status = 'pending' FOR UPDATE
        "#,
    )
    .bind(join_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(());
    };
    let approved = row.status == "answered"
        && row
            .answer
            .as_ref()
            .and_then(|answer| answer.get("optionId"))
            .and_then(serde_json::Value::as_str)
            == Some("approve");
    let status = match (approved, row.status.as_str()) {
        (true, _) => "joined",
        (false, "answered") => "refused",
        (false, "expired") => "expired",
        _ => "cancelled",
    };
    if approved {
        // Still an active account, and the handle still free here.
        let active = accounts::load(transaction, ask.account_id)
            .await?
            .is_some_and(|account| account.active);
        if !active {
            return Err(AppError::NotFound("account"));
        }
        if handle_taken(transaction, ask.workspace_id, ask.account_id, &ask.handle).await? {
            return Err(AppError::Validation("handle"));
        }
        let_in(
            transaction,
            Admission {
                workspace_id: ask.workspace_id,
                account_id: ask.account_id,
                role: &ask.role,
                role_id: ask.role_id,
                display_name: &ask.display_name,
                handle: &ask.handle,
            },
        )
        .await?;
    }
    sqlx::query("UPDATE account_join_requests SET status = $2, settled_at = now() WHERE id = $1")
        .bind(join_id)
        .bind(status)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct PendingJoin {
    workspace_id: Uuid,
    account_id: Uuid,
    role: String,
    role_id: Option<Uuid>,
    display_name: String,
    handle: String,
}

// ------------------------------------------------------------------ list

/// The agents and apps you manage that aren't members here yet.
pub struct AccountJoinableList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountJoinableListInput {}

impl Targeted for AccountJoinableListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JoinableAccount {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// `agent` or `app`.
    pub kind: String,
    pub display_name: String,
    pub handle: String,
    /// Its handle is taken here: give another.
    pub handle_taken: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JoinableView {
    pub accounts: Vec<JoinableAccount>,
}

impl Operation for AccountJoinableList {
    const NAME: &'static str = "account_joinable_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::MembersInvite);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Agents you could add here";
    const DESCRIPTION: &'static str = "Agents and apps you manage that aren't here yet.";

    type Input = AccountJoinableListInput;
    type Output = JoinableView;

    async fn run(
        call: &mut OpCall<'_>,
        _input: AccountJoinableListInput,
    ) -> Result<JoinableView, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        // Candidates: owned by the caller, naming them a manager, or owned
        // by a workspace they belong to. The one rule decides each.
        let candidates = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT account.id, account.account_kind
            FROM actors account
            WHERE account.account_kind IN ('agent', 'app') AND account.status = 'active'
              AND (account.owner_account_id = $2
                   OR EXISTS (SELECT 1 FROM account_managers manager
                              WHERE manager.account_id = account.id
                                AND manager.manager_account_id = $2)
                   OR account.owner_workspace_id IN (
                        SELECT organization_id FROM organization_memberships
                        WHERE actor_id = $2 AND enabled))
              AND NOT EXISTS (SELECT 1 FROM organization_memberships here
                              WHERE here.organization_id = $1 AND here.actor_id = account.id
                                AND here.enabled)
            ORDER BY account.id
            LIMIT 200
            "#,
        )
        .bind(workspace_id)
        .bind(caller.account_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let mut accounts = Vec::new();
        for (account_id, kind) in candidates {
            if access::may_manage_account(tx, &caller, account_id)
                .await?
                .is_err()
            {
                continue;
            }
            let Some((display_name, handle)) = name_of(tx, workspace_id, account_id).await? else {
                continue;
            };
            let handle_taken = handle_taken(tx, workspace_id, account_id, &handle).await?;
            accounts.push(JoinableAccount {
                account_id,
                kind,
                display_name,
                handle,
                handle_taken,
            });
        }
        accounts.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(JoinableView { accounts })
    }
}
