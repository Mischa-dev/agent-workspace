//! Memberships that end by themselves (K1.27): a guest, or anyone given a
//! space or the workspace for a while. `expires_at` on a workspace or space
//! membership; `workspace_member_update` sets it on a workspace membership
//! (`space_member_add` and `space_member_update` on a space's), and the
//! `memberships.expire` job turns a membership off once its time passes, as
//! removing someone does.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::AppError,
    jobs::JobRegistry,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted},
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<WorkspaceMemberUpdate>();
}

const EXPIRE_KIND: &str = "memberships.expire";

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(EXPIRE_KIND, |context| async move {
        while expire_due(&context.state.pool, 100).await? == 100 {}
        Ok(())
    });
    registry.every(EXPIRE_KIND, std::time::Duration::from_secs(60));
}

/// Turns off up to `limit` memberships whose time has come, each in its own
/// transaction; returns how many it looked at.
pub async fn expire_due(pool: &sqlx::PgPool, limit: i64) -> Result<u64, AppError> {
    // Due memberships are found across every workspace.
    crate::tenancy::system("memberships.expire", Box::pin(expire_due_here(pool, limit))).await
}

async fn expire_due_here(pool: &sqlx::PgPool, limit: i64) -> Result<u64, AppError> {
    let mut looked = 0;
    for _ in 0..limit {
        let mut tx = crate::db::scoped_tx(pool).await?;
        // An owner's membership lasts (it can't be given a time, and taking
        // ownership clears one), so owners are never picked.
        // A space membership first: the space alone.
        let space = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT project_id, actor_id FROM project_memberships \
             WHERE enabled AND NOT via_group AND expires_at <= now() AND role <> $1 \
             ORDER BY expires_at LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .bind(crate::policy::MembershipRole::Owner.key())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((space, account)) = space {
            looked += 1;
            super::expire_member(&mut tx, space, account).await?;
            tx.commit().await?;
            continue;
        }
        let workspace = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT organization_id, actor_id FROM organization_memberships \
             WHERE enabled AND expires_at <= now() AND role <> $1 \
             ORDER BY expires_at LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .bind(crate::policy::MembershipRole::Owner.key())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((workspace, account)) = workspace else {
            break;
        };
        looked += 1;
        match crate::workspace::remove_from_workspace(&mut tx, workspace, account, account).await {
            Ok(()) => tx.commit().await?,
            Err(AppError::Conflict(..)) => {
                // The only owner of a project stays: their time is cleared
                // rather than tried again every minute.
                tx.rollback().await?;
                let mut tx = crate::db::scoped_tx(pool).await?;
                sqlx::query(
                    "UPDATE organization_memberships SET expires_at = NULL \
                     WHERE organization_id = $1 AND actor_id = $2",
                )
                .bind(workspace)
                .bind(account)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                tracing::warn!(
                    workspace_id = %workspace,
                    account_id = %account,
                    "a membership's time passed, but it holds a project's only owner"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(looked)
}

// ---------------------------------------------------------------------------
// workspace_member_update

pub struct WorkspaceMemberUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMemberUpdateInput {
    /// Who: an account's id or @handle in this workspace.
    pub account: String,
    /// When their membership of the workspace ends by itself (RFC 3339, in
    /// the future); an empty string makes it last.
    pub expires_at: String,
}

impl Targeted for WorkspaceMemberUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Someone's membership of the workspace.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMemberView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub version: i64,
}

impl Operation for WorkspaceMemberUpdate {
    const NAME: &'static str = "workspace_member_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::MembersManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["account.profile_changed"];
    const TITLE: &'static str = "Change someone's workspace membership";
    const DESCRIPTION: &'static str = "Set when someone's membership of the workspace ends by \
        itself, such as a guest's, or make it last. Their role is changed with role_assign. \
        Nobody changes their own, and an owner's lasts.";

    type Input = WorkspaceMemberUpdateInput;
    type Output = WorkspaceMemberView;

    async fn run(
        call: &mut OpCall<'_>,
        input: WorkspaceMemberUpdateInput,
    ) -> Result<WorkspaceMemberView, OpError> {
        let expires_at = crate::spaces::members::expiry(&input.expires_at)?;
        let workspace = call.ctx.workspace_id;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let text = input.account.trim();
        let (account, role) = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT membership.actor_id, membership.role FROM organization_memberships membership \
             JOIN organization_actor_profiles profile \
               ON profile.organization_id = membership.organization_id \
              AND profile.actor_id = membership.actor_id \
             WHERE membership.organization_id = $1 AND membership.enabled \
               AND (membership.actor_id::text = $2 OR lower(profile.handle) = lower($3)) \
             LIMIT 1 FOR UPDATE OF membership",
        )
        .bind(workspace)
        .bind(text)
        .bind(text.trim_start_matches('@'))
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That account in this workspace"))?;
        if account == caller {
            return Err(OpError::invalid(
                "account",
                "You can't change your own membership. Ask another owner or admin.",
            ));
        }
        if crate::access::tier::stored_owner(&role) && expires_at.is_some() {
            return Err(OpError::invalid("account", "An owner's membership lasts."));
        }
        // A membership that ends is a removal on a timer: only someone who
        // may remove this member now may set one (an admin can't end
        // another admin's).
        if expires_at.is_some() {
            let standing = crate::workspace::standing_of(tx, caller, workspace, true).await?;
            let tier = crate::policy::MembershipRole::parse(&role)
                .ok_or(AppError::Internal("stored workspace role"))?;
            if !crate::workspace::member_capabilities(standing, false, tier, false, true).remove {
                return Err(OpError::Denied {
                    reason: crate::ops::DenyReason::Rule,
                    message: "Only someone who may remove this member can set when their \
                              membership ends."
                        .into(),
                });
            }
        }
        let view = sqlx::query_as::<_, WorkspaceMemberView>(
            "UPDATE organization_memberships SET expires_at = $3, version = version + 1, \
                 updated_at = now() \
             WHERE organization_id = $1 AND actor_id = $2 \
             RETURNING actor_id AS account_id, enabled AS active, \
                 to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') \
                   AS expires_at, version",
        )
        .bind(workspace)
        .bind(account)
        .bind(expires_at)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let mut event = crate::events::NewEvent::thing(
            &crate::events::catalog::ACCOUNT_PROFILE_CHANGED,
            Uuid::nil(),
            "account",
            account,
        )
        .workspace(workspace)
        .actor(caller)
        .audience(crate::events::Audience::Workspace)
        .payload(json!({ "expires": view.expires_at.is_some() }));
        event.space_id = None;
        crate::events::emit(tx, event).await?;
        call.audit_target("account", account, json!({ "expiresAt": view.expires_at }));
        Ok(view)
    }
}
