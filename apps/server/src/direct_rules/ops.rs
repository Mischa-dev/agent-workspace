//! Who may message whom, as operations (C2.39): `dm_standing_get`,
//! `dm_request_respond`, `account_block_set` and `account_block_list`. The
//! DM setting itself (`dmPolicy`) is part of `profile_update`.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// The most accounts one account blocks.
const MAX_BLOCKS: i64 = 1000;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<DmStandingGet>()
        .add::<DmRequestRespond>()
        .add::<AccountBlockSet>()
        .add::<AccountBlockList>();
}

async fn tell(call: &mut OpCall<'_>, accounts: &[Uuid]) -> Result<(), OpError> {
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    // A DM's call lands in the direct scope (K4.12), which has no space:
    // its notice goes there as it is.
    let space = if workspace == crate::spaces::DIRECT {
        workspace
    } else {
        crate::spaces::workspace::ensure(tx, workspace).await?
    };
    for account in accounts {
        crate::outbox::enqueue_typed(
            tx,
            space,
            crate::outbox::OutboxEventPayload::DmRulesChanged(
                crate::outbox::DmRulesChangedPayload { actor_id: *account },
            ),
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dm_standing_get

pub struct DmStandingGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DmStandingGetInput {
    /// The DM: its id or a link to it.
    pub conversation_id: IdRef,
}

impl Targeted for DmStandingGetInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

/// Whether you can send in a DM, and why not.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DmStanding {
    /// Whether a message you send now goes through.
    pub can_send: bool,
    /// Why not, when you can't: `blocked` (you blocked them), `closed`,
    /// `not_shared` (you no longer share a workspace) or `waiting` (they
    /// haven't accepted your request).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The other person, for a DM between two.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub other_account_id: Option<Uuid>,
    /// You blocked them.
    pub you_blocked: bool,
    /// A message request in this DM: `incoming` (to you, to accept or
    /// decline) or `outgoing` (yours, waiting), with its state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<DmRequestView>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DmRequestView {
    pub direction: String,
    /// pending, accepted or declined.
    pub state: String,
}

async fn readable_dm(call: &mut OpCall<'_>, conversation: Uuid) -> Result<String, OpError> {
    let caller = call.account_id();
    let tx = call.tx().await?;
    sqlx::query_scalar::<_, String>(
        "SELECT kind FROM conversations \
         WHERE id = $1 AND archived_at IS NULL AND conversation_visible_to(id, $2)",
    )
    .bind(conversation)
    .bind(caller)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That conversation"))
}

pub(crate) async fn standing(
    call: &mut OpCall<'_>,
    conversation: Uuid,
) -> Result<DmStanding, OpError> {
    let caller = call.account_id();
    let kind = readable_dm(call, conversation).await?;
    let open = DmStanding {
        can_send: true,
        reason: None,
        other_account_id: None,
        you_blocked: false,
        request: None,
    };
    if kind != "dm" {
        return Ok(open);
    }
    let tx = call.tx().await?;
    let Some(other) = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM conversation_members \
         WHERE conversation_id = $1 AND actor_id <> $2 ORDER BY actor_id LIMIT 1",
    )
    .bind(conversation)
    .bind(caller)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    else {
        return Ok(open);
    };
    let you_blocked = super::has_blocked(tx, caller, other).await?;
    let blocked = you_blocked || super::has_blocked(tx, other, caller).await?;
    let shared = super::share_a_workspace(tx, caller, other).await?;
    let request = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT requester_id, state FROM dm_requests WHERE conversation_id = $1",
    )
    .bind(conversation)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let waiting = request
        .as_ref()
        .is_some_and(|(requester, state)| *requester == caller && state != "accepted");
    let reason = if you_blocked {
        Some("blocked")
    } else if blocked {
        Some("closed")
    } else if !shared {
        Some("not_shared")
    } else if waiting {
        Some("waiting")
    } else {
        None
    };
    Ok(DmStanding {
        can_send: reason.is_none(),
        reason: reason.map(str::to_owned),
        other_account_id: Some(other),
        you_blocked,
        // A declined request looks like a pending one to the person who sent
        // it: they aren't told.
        request: request.map(|(requester, state)| {
            let outgoing = requester == caller;
            DmRequestView {
                direction: if outgoing { "outgoing" } else { "incoming" }.to_owned(),
                state: if outgoing && state == "declined" {
                    "pending".to_owned()
                } else {
                    state
                },
            }
        }),
    })
}

impl Operation for DmStandingGet {
    const NAME: &'static str = "dm_standing_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get DM standing";
    const DESCRIPTION: &'static str = "Whether you can send in a DM now, and why not: you blocked \
        them, you no longer share a workspace, or your message request is waiting. Also says when \
        the DM holds a message request to you, to accept or decline with dm_request_respond.";

    type Input = DmStandingGetInput;
    type Output = DmStanding;

    async fn run(call: &mut OpCall<'_>, input: DmStandingGetInput) -> Result<DmStanding, OpError> {
        standing(call, input.conversation_id.id()).await
    }
}

// ---------------------------------------------------------------------------
// dm_request_respond

pub struct DmRequestRespond;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DmRequestRespondInput {
    /// The DM holding the request: its id or a link to it.
    pub conversation_id: IdRef,
    /// true accepts it: their messages reach you from now on. false declines
    /// it: the DM leaves your list, and they can't send more. Writing back
    /// accepts it too.
    pub accept: bool,
}

impl Targeted for DmRequestRespondInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for DmRequestRespond {
    const NAME: &'static str = "dm_request_respond";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Answer a message request";
    const DESCRIPTION: &'static str = "Accept or decline a message request: the first message \
        from someone, to you while you ask people first (dmPolicy ask). Accepting lets their \
        messages reach you; declining takes the DM out of your list and stops their messages. \
        They aren't told you declined.";
    const EMITS: &'static [&'static str] = &["dm_rules.changed"];

    type Input = DmRequestRespondInput;
    type Output = DmStanding;

    async fn run(
        call: &mut OpCall<'_>,
        input: DmRequestRespondInput,
    ) -> Result<DmStanding, OpError> {
        let caller = call.account_id();
        let conversation = input.conversation_id.id();
        readable_dm(call, conversation).await?;
        let tx = call.tx().await?;
        let requester = sqlx::query_scalar::<_, Uuid>(
            "SELECT requester_id FROM dm_requests \
             WHERE conversation_id = $1 AND recipient_id = $2 FOR UPDATE",
        )
        .bind(conversation)
        .bind(caller)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("A message request to you there"))?;
        if super::answer(tx, conversation, caller, input.accept).await? {
            // The person who asked hears only of an acceptance.
            let told: &[Uuid] = if input.accept {
                &[caller, requester]
            } else {
                &[caller]
            };
            tell(call, told).await?;
        }
        standing(call, conversation).await
    }
}

// ---------------------------------------------------------------------------
// account_block_set

pub struct AccountBlockSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountBlockSetInput {
    /// The account (a person's or an agent's): its id.
    pub account: IdRef,
    /// true blocks them; false unblocks them.
    pub blocked: bool,
}

impl Targeted for AccountBlockSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// Accounts you blocked.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountBlocks {
    pub blocked: Vec<BlockedAccount>,
}

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BlockedAccount {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// Their name in this workspace, when they are in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub blocked_at: String,
}

async fn blocks(call: &mut OpCall<'_>) -> Result<AccountBlocks, OpError> {
    let (caller, workspace) = (call.account_id(), call.ctx.workspace_id);
    let tx = call.tx().await?;
    let blocked = sqlx::query_as::<_, BlockedAccount>(
        r#"
        SELECT block.blocked_account_id AS account_id,
               (SELECT min(membership.display_name) FROM project_memberships membership
                JOIN projects space ON space.id = membership.project_id
                WHERE membership.actor_id = block.blocked_account_id
                  AND space.organization_id = $2) AS name,
               block.created_at::text AS blocked_at
        FROM account_blocks block
        WHERE block.account_id = $1
        ORDER BY block.created_at DESC
        "#,
    )
    .bind(caller)
    .bind(workspace)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    Ok(AccountBlocks { blocked })
}

impl Operation for AccountBlockSet {
    const NAME: &'static str = "account_block_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Block or unblock";
    const DESCRIPTION: &'static str = "Block someone in this workspace (blocked: true), or \
        unblock them (false). A block takes your DM with them out of your list, stops new \
        messages between you both ways, and keeps their mentions and other notifications out of \
        your inbox. They aren't told. Returns everyone you blocked.";
    const EMITS: &'static [&'static str] = &["dm_rules.changed"];

    type Input = AccountBlockSetInput;
    type Output = AccountBlocks;

    async fn run(
        call: &mut OpCall<'_>,
        input: AccountBlockSetInput,
    ) -> Result<AccountBlocks, OpError> {
        let (caller, workspace) = (call.account_id(), call.ctx.workspace_id);
        let account = input.account.id();
        if account == caller {
            return Err(OpError::invalid("account", "You can't block yourself."));
        }
        let tx = call.tx().await?;
        let changed = if input.blocked {
            // Only someone in this workspace: an account elsewhere is none
            // of this workspace's business.
            let member = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM organization_memberships \
                 WHERE organization_id = $1 AND actor_id = $2)",
            )
            .bind(workspace)
            .bind(account)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if !member {
                return Err(OpError::not_found("That account"));
            }
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_blocks WHERE account_id = $1",
            )
            .bind(caller)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_BLOCKS {
                return Err(OpError::invalid(
                    "account",
                    "You have blocked 1000 accounts already; unblock one first.",
                ));
            }
            sqlx::query(
                "INSERT INTO account_blocks (account_id, blocked_account_id) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(caller)
            .bind(account)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?
            .rows_affected()
        } else {
            sqlx::query(
                "DELETE FROM account_blocks WHERE account_id = $1 AND blocked_account_id = $2",
            )
            .bind(caller)
            .bind(account)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?
            .rows_affected()
        };
        if changed > 0 {
            tell(call, &[caller]).await?;
        }
        blocks(call).await
    }
}

// ---------------------------------------------------------------------------
// account_block_list

pub struct AccountBlockList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountBlockListInput {}

impl Targeted for AccountBlockListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for AccountBlockList {
    const NAME: &'static str = "account_block_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List blocked accounts";
    const DESCRIPTION: &'static str = "Everyone you blocked, newest first, with their name here \
        when they are in this workspace. Blocks are your own and hold in every workspace.";

    type Input = AccountBlockListInput;
    type Output = AccountBlocks;

    async fn run(
        call: &mut OpCall<'_>,
        _input: AccountBlockListInput,
    ) -> Result<AccountBlocks, OpError> {
        blocks(call).await
    }
}
