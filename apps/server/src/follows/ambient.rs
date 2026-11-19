//! Ambient mode (A2.9): per account and conversation. An account in
//! ambient mode in a channel or thread hears every new message there, and
//! each message nobody prompted (chain depth 0) invites it to speak with an
//! optional reply request, unless it already has one open there or answered
//! within its cooldown. It answers with reply_send or reply_pass, so claims,
//! turns and Stop work as everywhere else.
//!
//! Only channels and threads: in a DM or group every member who takes
//! reply requests is already asked on every message.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// How long an account in ambient mode waits after answering before it is
/// invited again, when its setting doesn't say.
pub const DEFAULT_COOLDOWN_SECONDS: i32 = 300;
const MAX_COOLDOWN_SECONDS: u32 = 86_400;
/// The most accounts the settings list returns.
const MAX_LISTED: i64 = 200;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ConversationAgentSettingsSet>()
        .add::<ConversationAgentSettingsList>();
}

/// The accounts in ambient mode in `conversation` that a new message from
/// `author` (mentioning `mentioned`, who are asked anyway) invites to
/// speak: they take reply requests, may see it, have no open request there
/// and haven't answered there within their cooldown.
pub(crate) async fn invited(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    conversation_id: Uuid,
    author: Uuid,
    mentioned: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT setting.account_id
        FROM conversation_agent_settings setting
        JOIN actors actor ON actor.id = setting.account_id
        JOIN project_memberships membership
          ON membership.project_id = $1 AND membership.actor_id = setting.account_id
        WHERE setting.conversation_id = $2
          AND setting.ambient
          AND actor.accepts_reply_requests
          AND actor.status = 'active'
          AND membership.enabled
          AND actor.id <> $3
          AND NOT (actor.id = ANY($4))
          AND conversation_visible_to($2, actor.id)
          AND NOT EXISTS (
              SELECT 1 FROM reply_requests request
              WHERE request.agent_actor_id = actor.id
                AND (request.conversation_id = $2 OR request.thread_id = $2)
                AND request.status IN ('pending', 'claimed')
          )
          AND NOT EXISTS (
              SELECT 1 FROM reply_requests request
              WHERE request.agent_actor_id = actor.id
                AND (request.conversation_id = $2 OR request.thread_id = $2)
                AND request.status IN ('replied', 'passed')
                AND request.updated_at > now() - make_interval(
                    secs => COALESCE(setting.reply_cooldown_seconds, $5))
          )
        ORDER BY actor.id
        "#,
    )
    .bind(space_id)
    .bind(conversation_id)
    .bind(author)
    .bind(mentioned)
    .bind(DEFAULT_COOLDOWN_SECONDS)
    .fetch_all(&mut **transaction)
    .await?)
}

/// One account's settings in a conversation.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingsView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub display_name: String,
    pub handle: String,
    pub ambient: bool,
    /// Seconds it waits after answering before it is invited again.
    pub reply_cooldown_seconds: i32,
}

async fn kind_of(
    connection: &mut PgConnection,
    conversation_id: Uuid,
    viewer: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT kind::text FROM conversations \
         WHERE id = $1 AND conversation_visible_to(id, $2)",
    )
    .bind(conversation_id)
    .bind(viewer)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The accounts that could be invited in `conversation` (they take reply
/// requests and may see it), with their settings, or only `only`.
async fn settings(
    connection: &mut PgConnection,
    space_id: Uuid,
    conversation_id: Uuid,
    only: Option<Uuid>,
) -> Result<Vec<AgentSettingsView>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<bool>, Option<i32>)>(
        r#"
        SELECT actor.id, membership.display_name, membership.handle,
               setting.ambient, setting.reply_cooldown_seconds
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        LEFT JOIN conversation_agent_settings setting
          ON setting.conversation_id = $2 AND setting.account_id = actor.id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.accepts_reply_requests
          AND conversation_visible_to($2, actor.id)
          AND ($3::uuid IS NULL OR actor.id = $3)
        ORDER BY lower(membership.display_name), actor.id
        LIMIT $4
        "#,
    )
    .bind(space_id)
    .bind(conversation_id)
    .bind(only)
    .bind(MAX_LISTED)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(account_id, display_name, handle, ambient, cooldown)| AgentSettingsView {
                account_id,
                display_name,
                handle,
                ambient: ambient.unwrap_or(false),
                reply_cooldown_seconds: cooldown.unwrap_or(DEFAULT_COOLDOWN_SECONDS),
            },
        )
        .collect())
}

fn ambient_place(kind: Option<String>) -> Result<(), OpError> {
    match kind.as_deref() {
        Some("channel" | "thread") => Ok(()),
        Some(_) => Err(OpError::invalid(
            "conversation",
            "Ambient mode is for channels and threads: in a DM or group everyone who takes \
             replies is already asked on every message.",
        )),
        None => Err(OpError::not_found("That conversation")),
    }
}

/// Turn ambient mode on or off for an account in a conversation.
pub struct ConversationAgentSettingsSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAgentSettingsSetInput {
    /// The channel or thread, or a link to it.
    pub conversation: IdRef,
    /// The account (one that takes reply requests and can see the
    /// conversation).
    pub account: IdRef,
    /// true: invited to speak on every new message nobody prompted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<bool>,
    /// Seconds it waits after answering before it is invited again (0 to
    /// 86400). Left out: unchanged (300 at first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_cooldown_seconds: Option<u32>,
}

impl Targeted for ConversationAgentSettingsSetInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation.id())
    }
}

impl Operation for ConversationAgentSettingsSet {
    const NAME: &'static str = "conversation_agent_settings_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = Some(Perm::RepliesSteer);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set ambient mode";
    const DESCRIPTION: &'static str = "Turn ambient mode on or off for an account in a channel \
        or thread: when on, each new message nobody prompted invites it to reply (an optional \
        reply request), unless it answered within replyCooldownSeconds.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = ConversationAgentSettingsSetInput;
    type Output = AgentSettingsView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationAgentSettingsSetInput,
    ) -> Result<AgentSettingsView, OpError> {
        if input
            .reply_cooldown_seconds
            .is_some_and(|seconds| seconds > MAX_COOLDOWN_SECONDS)
        {
            return Err(OpError::invalid(
                "replyCooldownSeconds",
                "Pass 0 to 86400 seconds.",
            ));
        }
        let caller = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let conversation_id = input.conversation.id();
        let account = input.account.id();
        let tx = call.tx().await?;
        ambient_place(kind_of(tx, conversation_id, caller).await?)?;
        if settings(tx, space_id, conversation_id, Some(account))
            .await?
            .is_empty()
        {
            return Err(OpError::invalid(
                "account",
                "That account can't be asked to reply here: it must take reply requests and \
                 be able to see this conversation.",
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO conversation_agent_settings (
                workspace_id, space_id, conversation_id, account_id, ambient,
                reply_cooldown_seconds, set_by_account_id
            )
            VALUES ($1, $2, $3, $4, COALESCE($5, false), $6, $7)
            ON CONFLICT (conversation_id, account_id) DO UPDATE SET
                ambient = COALESCE($5, conversation_agent_settings.ambient),
                reply_cooldown_seconds = COALESCE(
                    $6, conversation_agent_settings.reply_cooldown_seconds),
                set_by_account_id = $7,
                updated_at = now()
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(conversation_id)
        .bind(account)
        .bind(input.ambient)
        .bind(
            input
                .reply_cooldown_seconds
                .map(|seconds| i32::try_from(seconds).unwrap_or(i32::MAX)),
        )
        .bind(caller)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let view = settings(tx, space_id, conversation_id, Some(account))
            .await?
            .pop()
            .ok_or(OpError::Unavailable)?;
        // Open copies of the conversation read who is in ambient mode again.
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM conversations \
             WHERE id = $1 AND conversation_visible_to(id, $2)",
        )
        .bind(conversation_id)
        .bind(caller)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        crate::outbox::enqueue_typed(
            tx,
            space_id,
            crate::outbox::OutboxEventPayload::ConversationUpdated(
                crate::outbox::ConversationUpdatedPayload {
                    conversation_id,
                    version,
                },
            ),
        )
        .await?;
        call.audit_target(
            "conversation",
            conversation_id,
            json!({ "account": account, "ambient": view.ambient,
                    "reply_cooldown_seconds": view.reply_cooldown_seconds }),
        );
        Ok(view)
    }
}

/// Who could be in ambient mode in a conversation, and who is.
pub struct ConversationAgentSettingsList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAgentSettingsListInput {
    /// The channel or thread, or a link to it.
    pub conversation: IdRef,
}

impl Targeted for ConversationAgentSettingsListInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAgentSettingsListOutput {
    /// The accounts that take reply requests and can see it.
    pub accounts: Vec<AgentSettingsView>,
}

impl Operation for ConversationAgentSettingsList {
    const NAME: &'static str = "conversation_agent_settings_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Ambient mode here";
    const DESCRIPTION: &'static str = "In a channel or thread: the accounts that take reply \
        requests and can see it, each with whether it is in ambient mode and its cooldown. \
        Change one with conversation_agent_settings_set.";

    type Input = ConversationAgentSettingsListInput;
    type Output = ConversationAgentSettingsListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationAgentSettingsListInput,
    ) -> Result<ConversationAgentSettingsListOutput, OpError> {
        let caller = call.account_id();
        let space_id = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let conversation_id = input.conversation.id();
        let tx = call.tx().await?;
        ambient_place(kind_of(tx, conversation_id, caller).await?)?;
        Ok(ConversationAgentSettingsListOutput {
            accounts: settings(tx, space_id, conversation_id, None).await?,
        })
    }
}
