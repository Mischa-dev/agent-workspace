//! A2.15: an agent's scope, as one read. Where it is a member and as what,
//! the private conversations it is in that the viewer is in too, who
//! manages it, how it hears things, and whether it is also in other
//! workspaces (yes or no only, ruling R19: never which). Everything
//! private is filtered by the viewer, so the screen shows each person only
//! what they could open themselves.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// How many spaces and conversations the screen lists at most.
const MAX_LISTED: i64 = 200;

/// What an agent can reach.
pub struct AgentScopeGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentScopeGetInput {
    /// The agent (or any account in this workspace), or a link to it.
    pub account: IdRef,
}

impl Targeted for AgentScopeGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentScopeView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub display_name: Option<String>,
    pub handle: Option<String>,
    /// Its role in this workspace.
    pub workspace_role: String,
    /// The spaces of this workspace it is a member of, that you can read.
    pub spaces: Vec<ScopeSpace>,
    /// DMs, groups and private channels it is in that you are in too.
    pub private_conversations: Vec<ScopeConversation>,
    /// Whether it hears private conversations only where a participant
    /// consented (apps acting as themselves).
    pub reads_with_consent: bool,
    /// Who manages it.
    pub managers: Vec<ScopeAccount>,
    /// Its delivery connections (webhooks and readers), not removed.
    pub connections: i64,
    /// How many things it follows that you can see too.
    pub follows: i64,
    /// Whether it is also a member of another workspace. Which one is
    /// never said.
    pub also_in_other_workspaces: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSpace {
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub name: String,
    /// project, team, personal or workspace.
    pub kind: String,
    /// Its role there.
    pub role: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeConversation {
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    /// Where it lives: this workspace, or the direct scope for a DM or
    /// group (name it as `workspace` to change consent there).
    #[schemars(with = "String")]
    pub workspace_id: Uuid,
    /// dm, group or channel.
    pub kind: String,
    pub name: Option<String>,
    /// Whether it hears this conversation, for an account that reads with
    /// consent; always true for one that doesn't.
    pub heard: bool,
    /// Who else is in it (up to six, you first), for naming one that has
    /// no name.
    pub members: Vec<ScopeMember>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeMember {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub display_name: Option<String>,
    /// Whether this is you.
    pub you: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeAccount {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub display_name: Option<String>,
}

impl Operation for AgentScopeGet {
    const NAME: &'static str = "agent_scope_get";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "What an agent can reach";
    const DESCRIPTION: &'static str = "An account's scope in this workspace: the spaces it is \
        in and its role there, the DMs, groups and private channels it shares with you, who \
        manages it, its connections and follows, and whether it is in other workspaces too.";

    type Input = AgentScopeGetInput;
    type Output = AgentScopeView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentScopeGetInput,
    ) -> Result<AgentScopeView, OpError> {
        let viewer = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let account = input.account.id();
        let tx = call.tx().await?;
        let found = sqlx::query_as::<_, (String, bool, Option<String>, Option<String>)>(
            r#"
            SELECT membership.role, actor.reads_with_consent, profile.display_name, profile.handle
            FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id
            LEFT JOIN LATERAL account_profile(actor.id, $1) profile ON true
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
            "#,
        )
        .bind(workspace_id)
        .bind(account)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let Some((workspace_role, reads_with_consent, display_name, handle)) = found else {
            return Err(OpError::not_found("That account"));
        };
        let spaces = sqlx::query_as::<_, (Uuid, String, String, String)>(
            r#"
            SELECT space.id, space.name, space.kind::text, membership.role
            FROM project_memberships membership
            JOIN projects space ON space.id = membership.project_id
            WHERE membership.actor_id = $2 AND membership.enabled
              AND space.organization_id = $1 AND space.archived_at IS NULL
              AND space_readable_by(space.id, $3)
            ORDER BY lower(space.name), space.id
            LIMIT $4
            "#,
        )
        .bind(workspace_id)
        .bind(account)
        .bind(viewer)
        .bind(MAX_LISTED)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .map(|(space_id, name, kind, role)| ScopeSpace {
            space_id,
            name,
            kind,
            role,
        })
        .collect();
        let private_conversations = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                Option<String>,
                bool,
                sqlx::types::Json<Vec<ScopeMember>>,
            ),
        >(
            r#"
            SELECT conversation.id, conversation.workspace_id, conversation.kind::text,
                   conversation.name, app_may_read($2, conversation.id),
                   COALESCE((
                       SELECT json_agg(json_build_object(
                                  'accountId', other.actor_id,
                                  'displayName', other.display_name,
                                  'you', other.actor_id = $3)
                              ORDER BY other.actor_id <> $3, other.display_name)
                       FROM (
                           SELECT someone.actor_id, profile.display_name
                           FROM conversation_members someone
                           LEFT JOIN LATERAL account_profile(someone.actor_id, $1) profile ON true
                           WHERE someone.conversation_id = conversation.id
                             AND someone.actor_id <> $2
                           ORDER BY someone.actor_id <> $3, profile.display_name
                           LIMIT 6
                       ) other
                   ), '[]'::json)
            FROM conversation_members member
            JOIN conversations conversation ON conversation.id = member.conversation_id
            CROSS JOIN LATERAL app_consent_root(conversation.id) root
            WHERE member.actor_id = $2
              AND root.needs_consent AND root.root_id = conversation.id
              AND (conversation.workspace_id = $1 OR conversation.project_id IS NULL)
              AND conversation_visible_to(conversation.id, $2)
              AND conversation_visible_to(conversation.id, $3)
            ORDER BY conversation.updated_at DESC, conversation.id
            LIMIT $4
            "#,
        )
        .bind(workspace_id)
        .bind(account)
        .bind(viewer)
        .bind(MAX_LISTED)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .map(
            |(conversation_id, workspace_id, kind, name, heard, members)| ScopeConversation {
                conversation_id,
                workspace_id,
                kind,
                name,
                heard,
                members: members.0,
            },
        )
        .collect();
        let managers = sqlx::query_as::<_, (Uuid, Option<String>)>(
            r#"
            SELECT manager.manager_account_id, profile.display_name
            FROM account_managers manager
            LEFT JOIN LATERAL account_profile(manager.manager_account_id, $1) profile ON true
            WHERE manager.account_id = $2
            ORDER BY lower(profile.display_name), manager.manager_account_id
            LIMIT $3
            "#,
        )
        .bind(workspace_id)
        .bind(account)
        .bind(MAX_LISTED)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .map(|(account_id, display_name)| ScopeAccount {
            account_id,
            display_name,
        })
        .collect();
        let (connections, follows) = sqlx::query_as::<_, (i64, i64)>(
            r#"
                SELECT
                    (SELECT count(*) FROM agent_connections
                     WHERE account_id = $2 AND state <> 'removed'),
                    (SELECT count(*) FROM follows
                     WHERE workspace_id = $1 AND account_id = $2
                       AND CASE target_kind
                               WHEN 'space' THEN COALESCE(space_readable_by(target_id, $3), false)
                                   AND COALESCE(space_readable_by(target_id, $2), false)
                               ELSE COALESCE(delivery_thing_visible(target_id, $3), false)
                                   AND COALESCE(delivery_thing_visible(target_id, $2), false)
                           END)
                "#,
        )
        .bind(workspace_id)
        .bind(account)
        .bind(viewer)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let also_in_other_workspaces =
            in_other_workspaces(&call.state.pool, account, workspace_id).await?;
        Ok(AgentScopeView {
            account_id: account,
            display_name,
            handle,
            workspace_role,
            spaces,
            private_conversations,
            reads_with_consent,
            managers,
            connections,
            follows,
            also_in_other_workspaces,
        })
    }
}

/// Whether `account` is an enabled member of a workspace other than `here`.
/// Row security would hide the memberships of workspaces the viewer isn't
/// in, so this one count runs server-wide and answers only yes or no.
async fn in_other_workspaces(
    pool: &sqlx::PgPool,
    account: Uuid,
    here: Uuid,
) -> Result<bool, AppError> {
    crate::tenancy::system("agents.scope", async move {
        let mut transaction = crate::db::scoped_tx(pool).await?;
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_memberships other \
             WHERE other.actor_id = $1 AND other.enabled \
               AND other.organization_id <> $2 AND other.organization_id <> direct_scope_id())",
        )
        .bind(account)
        .bind(here)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(found)
    })
    .await
}
