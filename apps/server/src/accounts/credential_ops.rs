//! K1.23: an account's secrets as operations, so the web, agents and
//! scripts share one path. An account manages its own secrets; an agent or
//! app account's secrets are also managed by whoever may manage the account
//! (`access::may_manage_account`). A delegated or short-lived credential
//! never makes or changes a longer-lived one.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    access::{
        self, Caller,
        credentials::{self, CredentialReach, CredentialView, NewSecret},
    },
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

pub fn register_ops(registry: &mut crate::ops::Registry) {
    registry.add::<CredentialList>();
    registry.add::<CredentialCreate>();
    registry.add::<CredentialRotate>();
    registry.add::<CredentialRevoke>();
}

/// How long a secret lives when the call doesn't say.
const DEFAULT_DAYS: i32 = 90;
/// How long an old value keeps working after a rotation when the call
/// doesn't say.
const DEFAULT_GRACE_MINUTES: i64 = 60;

/// Whether `caller` may manage `account`'s secrets: its own, or an agent or
/// app account it manages. `NotFound` otherwise, as for a stranger's.
async fn may_manage_secrets(
    connection: &mut PgConnection,
    caller: &Caller,
    account: Uuid,
) -> Result<(), OpError> {
    if account != caller.account_id {
        match access::may_manage_account(connection, caller, account).await? {
            Ok(_) => {}
            Err(_) => return Err(OpError::not_found("That account")),
        }
    }
    // A short-lived token never makes a lasting secret, which would outlive
    // the token and the secret it came from.
    let through_token = match caller.credential_id {
        Some(credential) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM access_tokens WHERE id = $1)",
        )
        .bind(credential)
        .fetch_one(&mut *connection)
        .await
        .map_err(AppError::from)?,
        None => false,
    };
    if through_token || caller.limits.is_some() || caller.via_grant_id.is_some() {
        return Err(OpError::Denied {
            reason: crate::ops::DenyReason::Delegation,
            message: "Secrets are made and changed with the account's own sign-in or secret, \
                      not through an app or a short-lived token."
                .into(),
        });
    }
    Ok(())
}

/// The secret, when the caller may manage it.
async fn managed_secret(
    connection: &mut PgConnection,
    caller: &Caller,
    credential: Uuid,
) -> Result<CredentialView, OpError> {
    let found = credentials::get(connection, credential)
        .await?
        .ok_or_else(|| OpError::not_found("That secret"))?;
    may_manage_secrets(connection, caller, found.account_id)
        .await
        .map_err(|error| match error {
            OpError::NotFound { .. } => OpError::not_found("That secret"),
            other => other,
        })?;
    Ok(found)
}

// ---- credential_list ----------------------------------------------------

pub struct CredentialList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialListInput {
    /// Whose secrets: an agent or app account you manage. Leave it out for
    /// your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for CredentialListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CredentialListView {
    pub credentials: Vec<CredentialView>,
}

impl Operation for CredentialList {
    const NAME: &'static str = "credential_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List secrets";
    const DESCRIPTION: &'static str = "List an account's secrets (yours, or an agent's you \
        manage): name, how far each reaches, when it expires and when it was last used. Never \
        the values.";
    type Input = CredentialListInput;
    type Output = CredentialListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CredentialListInput,
    ) -> Result<CredentialListView, OpError> {
        let caller = call.ctx.caller.clone();
        let account = input
            .account
            .map_or(caller.account_id, |account| account.id());
        let tx = call.tx().await?;
        may_manage_secrets(tx, &caller, account).await?;
        Ok(CredentialListView {
            credentials: credentials::list_for(tx, account).await?,
        })
    }
}

// ---- credential_create --------------------------------------------------

pub struct CredentialCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialCreateInput {
    /// A name to tell it apart, such as the machine or script that holds it.
    pub name: String,
    /// Whose secret: an agent or app account you manage. Leave it out for
    /// your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
    /// `space`: only the space it is made in. `account`: every workspace the
    /// account is in, each call naming which. Defaults to `space`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<CredentialReach>,
    /// The space it is made in. Defaults to the workspace's own space, or
    /// the account's first space here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
    /// false makes it read-only. Defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<bool>,
    /// Days until it expires, 1 to 365. Defaults to 90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<i32>,
}

impl Targeted for CredentialCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMadeView {
    pub credential: CredentialView,
    /// The value, shown once: store it now. Absent when a retry answers a
    /// secret already made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// For a rotation: until when the old value still works. Absent when it
    /// stopped at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_secret_valid_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl Operation for CredentialCreate {
    const NAME: &'static str = "credential_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    // The answer carries the secret, which is never stored for a replay: the
    // secret's row keeps the request id itself.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["credential.created"];
    const TITLE: &'static str = "Make a secret";
    const DESCRIPTION: &'static str = "Make a long-lived secret for your account, or for an agent \
        or app account you manage. The value is returned once. Exchange it at the token \
        endpoint (grant type client_credentials) for an hour-long token, or send it as a bearer.";
    type Input = CredentialCreateInput;
    type Output = CredentialMadeView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CredentialCreateInput,
    ) -> Result<CredentialMadeView, OpError> {
        let caller = call.ctx.caller.clone();
        let workspace_id = call.ctx.workspace_id;
        let request_id = call.ctx.request_id.clone();
        let account = input
            .account
            .map_or(caller.account_id, |account| account.id());
        let tx = call.tx().await?;
        may_manage_secrets(tx, &caller, account).await?;
        let home_space = sqlx::query_scalar::<_, Uuid>(
            "SELECT project.id FROM projects project \
             JOIN project_memberships membership \
               ON membership.project_id = project.id AND membership.actor_id = $2 \
              AND membership.enabled \
             WHERE project.organization_id = $1 AND project.archived_at IS NULL \
               AND ($3::uuid IS NULL OR project.id = $3) \
             ORDER BY (project.kind = $4) DESC, project.created_at, project.id LIMIT 1",
        )
        .bind(workspace_id)
        .bind(account)
        .bind(input.space.map(IdRef::id))
        .bind(crate::spaces::SpaceKind::Workspace.stored())
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That space"))?;
        // An agent's secret belongs to its connection here and holds what
        // the connection holds; a person's stands alone.
        let connection = sqlx::query_as::<_, (Uuid, Vec<String>)>(
            "SELECT connection.id, connection.scopes FROM external_agent_connections connection \
             WHERE connection.actor_id = $1 AND connection.workspace_id = $2 \
               AND connection.status = 'active' \
             ORDER BY connection.created_at, connection.id LIMIT 1",
        )
        .bind(account)
        .bind(workspace_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let signs_in = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE actor_id = $1 AND status = 'active')",
        )
        .bind(account)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let (connection_id, scopes) = match connection {
            Some((id, scopes)) => (Some(id), scopes),
            None if signs_in => {
                let mut scopes = vec!["read".to_owned()];
                if input.write.unwrap_or(true) {
                    scopes.push("write".to_owned());
                }
                (None, scopes)
            }
            None => {
                return Err(OpError::invalid(
                    "account",
                    "That account has no connection in this workspace yet. Connect it first.",
                ));
            }
        };
        if connection_id.is_some() && input.write == Some(false) {
            return Err(OpError::invalid(
                "write",
                "An agent's secret holds what its connection holds. Change the connection's \
                 scopes instead.",
            ));
        }
        let made = credentials::create_secret(
            tx,
            NewSecret {
                account_id: account,
                home_space,
                client_name: &input.name,
                scopes,
                reach: input.reach.unwrap_or(CredentialReach::Space),
                days: input.days.unwrap_or(DEFAULT_DAYS),
                connection_id,
                created_by: caller.account_id,
                request_id: request_id.as_deref(),
            },
        )
        .await?;
        call.mark_replayed(made.secret.is_none());
        call.audit_target(
            "credential",
            made.credential.id,
            json!({ "reach": made.credential.reach, "account": account }),
        );
        Ok(CredentialMadeView {
            credential: made.credential,
            secret: made.secret,
            old_secret_valid_until: None,
        })
    }
}

// ---- credential_rotate --------------------------------------------------

pub struct CredentialRotate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRotateInput {
    /// The secret to replace (its id).
    pub credential: IdRef,
    /// Minutes the old value keeps working, 0 to 10080 (a week). Defaults
    /// to 60; 0 stops it at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_minutes: Option<i64>,
    /// Days the new value lives. Defaults to what the old one had left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<i32>,
}

impl Targeted for CredentialRotateInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for CredentialRotate {
    const NAME: &'static str = "credential_rotate";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    // The answer carries the new value; a repeat finds the secret already
    // replaced and says so.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["credential.rotated"];
    const TITLE: &'static str = "Rotate a secret";
    const DESCRIPTION: &'static str = "Replace a secret with a new value, returned once. The old \
        value keeps working for the grace period so whatever holds it can move over.";
    type Input = CredentialRotateInput;
    type Output = CredentialMadeView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CredentialRotateInput,
    ) -> Result<CredentialMadeView, OpError> {
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let old = managed_secret(tx, &caller, input.credential.id()).await?;
        if old.replaced_by.is_some() || old.status != "active" {
            return Err(OpError::invalid(
                "credential",
                "That secret was already replaced or revoked. Rotate the newest one.",
            ));
        }
        let rotated = credentials::rotate(
            tx,
            old.id,
            input.grace_minutes.unwrap_or(DEFAULT_GRACE_MINUTES),
            input.days,
            caller.account_id,
        )
        .await?;
        call.audit_target(
            "credential",
            old.id,
            json!({ "replacedBy": rotated.made.credential.id }),
        );
        Ok(CredentialMadeView {
            credential: rotated.made.credential,
            secret: rotated.made.secret,
            old_secret_valid_until: rotated.old_secret_valid_until,
        })
    }
}

// ---- credential_revoke --------------------------------------------------

pub struct CredentialRevoke;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRevokeInput {
    /// The secret to stop (its id).
    pub credential: IdRef,
}

impl Targeted for CredentialRevokeInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for CredentialRevoke {
    const NAME: &'static str = "credential_revoke";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["credential.revoked"];
    const TITLE: &'static str = "Revoke a secret";
    const DESCRIPTION: &'static str = "Stop a secret at once, with every token exchanged from it.";
    type Input = CredentialRevokeInput;
    type Output = CredentialView;

    async fn run(
        call: &mut OpCall<'_>,
        input: CredentialRevokeInput,
    ) -> Result<CredentialView, OpError> {
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let found = managed_secret(tx, &caller, input.credential.id()).await?;
        credentials::revoke(tx, found.id).await?;
        let revoked = credentials::get(tx, found.id)
            .await?
            .ok_or_else(|| OpError::not_found("That secret"))?;
        call.audit_target("credential", found.id, json!({}));
        Ok(revoked)
    }
}
