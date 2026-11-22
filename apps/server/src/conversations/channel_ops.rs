//! Channels you're in (C2.7): list a space's channels to browse, read one's details,
//! join and leave, and set its topic, description and who joins it.
//!
//! A row in `conversation_members` means you're in a channel: it's in your
//! sidebar. For a public channel that's all it means. Anyone in the space
//! still finds, reads and joins it. For a private one (C2.6) it is also
//! what lets you see it, so joining one isn't possible: its managers add
//! people (`conversation_members`).

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    access::{self, Need, Target as AccessTarget},
    conversations::{enqueue_event, lock_conversation_for, read_conversation_access},
    direct::{GroupEditor, change_group_members},
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
    outbox::{ConversationUpdatedPayload, OutboxEventPayload},
    policy::Action,
};

/// The most channels `channel_list` returns.
const MAX_BROWSED: i64 = 500;
const MAX_TOPIC_CHARACTERS: usize = 250;
const MAX_DESCRIPTION_CHARACTERS: usize = 2000;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ChannelBrowse>()
        .add::<ChannelDetails>()
        .add::<ChannelJoin>()
        .add::<ChannelLeave>()
        .add::<ChannelUpdate>();
}

fn invalid(field: &'static str, message: &'static str) -> OpError {
    OpError::invalid(field, message)
}

/// A channel as `channel_list` lists it.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BrowsedChannel {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    pub topic: Option<String>,
    /// Only its members see it.
    pub private: bool,
    /// Everyone who arrives in the space joins it.
    pub auto_join: bool,
    /// You're in it: it's in your sidebar.
    pub joined: bool,
    pub member_count: i64,
}

/// One channel, as its page and details show it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub name: String,
    pub topic: Option<String>,
    pub description: Option<String>,
    pub private: bool,
    pub auto_join: bool,
    pub archived: bool,
    pub joined: bool,
    pub member_count: i64,
    /// Pass as expectedVersion to channel_update.
    pub version: i64,
    /// Whether you can set its topic and description.
    pub can_edit: bool,
    /// Whether you can change who joins it, and a private one's members.
    pub can_manage: bool,
}

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: Uuid,
    project_id: Uuid,
    name: String,
    topic: Option<String>,
    description: Option<String>,
    private: bool,
    auto_join: bool,
    archived: bool,
    joined: bool,
    manager: bool,
    member_count: i64,
    version: i64,
}

/// Reads a channel the caller can see, with what they may do to it.
async fn channel_view(call: &mut OpCall<'_>, channel_id: Uuid) -> Result<ChannelView, OpError> {
    let caller = call.ctx.caller.account_id;
    let tx = call.tx().await?;
    let row = sqlx::query_as::<_, ChannelRow>(
        r#"
        SELECT channel.id, channel.project_id, channel.name, channel.topic, channel.description,
               channel.visibility = 'members' AS private, channel.auto_join,
               channel.archived_at IS NOT NULL AS archived,
               EXISTS (SELECT 1 FROM conversation_members member
                       WHERE member.conversation_id = channel.id AND member.actor_id = $2)
                   AS joined,
               EXISTS (SELECT 1 FROM conversation_members member
                       WHERE member.conversation_id = channel.id AND member.actor_id = $2
                         AND member.manager) AS manager,
               (SELECT count(*) FROM conversation_members member
                WHERE member.conversation_id = channel.id) AS member_count,
               channel.version
        FROM conversations channel
        WHERE channel.id = $1 AND channel.kind = 'channel'
          AND conversation_visible_to(channel.id, $2)
        "#,
    )
    .bind(channel_id)
    .bind(caller)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That channel"))?;
    let access = read_conversation_access(&mut **tx, caller, channel_id)
        .await?
        .ok_or_else(|| OpError::not_found("That channel"))?;
    let can_edit = !row.archived && access.allows(caller, Action::MessageSend)?;
    let can_manage =
        !row.archived && (row.manager || access.allows(caller, Action::MessageModerate)?);
    Ok(ChannelView {
        id: row.id,
        space_id: row.project_id,
        name: row.name,
        topic: row.topic,
        description: row.description,
        private: row.private,
        auto_join: row.auto_join,
        archived: row.archived,
        joined: row.joined,
        member_count: row.member_count,
        version: row.version,
        can_edit,
        can_manage,
    })
}

/// Bumps a channel's version and tells everyone who can see it.
async fn announce_change(call: &mut OpCall<'_>, channel_id: Uuid) -> Result<(), OpError> {
    let tx = call.tx().await?;
    let (project_id, version) = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        -- visibility: the channel was locked through conversation_visible_to just before.
        UPDATE conversations SET version = version + 1, updated_at = now()
        WHERE id = $1
        RETURNING project_id, version
        "#,
    )
    .bind(channel_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)?;
    enqueue_event(
        tx,
        project_id,
        OutboxEventPayload::ConversationUpdated(ConversationUpdatedPayload {
            conversation_id: channel_id,
            version,
        }),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// channel_list

pub struct ChannelBrowse;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelBrowseInput {
    /// The project: its id, key or a link to it.
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Only channels whose name or topic holds these words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

impl Targeted for ChannelBrowseInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowsedChannels {
    pub channels: Vec<BrowsedChannel>,
}

impl Operation for ChannelBrowse {
    const NAME: &'static str = "channel_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Browse channels";
    const DESCRIPTION: &'static str = "List a project's channels you can see, joined or not, \
        with their topics and member counts: every public channel, and the private ones you're \
        in. Without \"browse channels\", only the ones you're in. Join one with channel_join.";

    type Input = ChannelBrowseInput;
    type Output = BrowsedChannels;

    async fn run(
        call: &mut OpCall<'_>,
        input: ChannelBrowseInput,
    ) -> Result<BrowsedChannels, OpError> {
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let caller = call.ctx.caller.account_id;
        let query = input
            .query
            .map(|query| query.trim().to_owned())
            .filter(|query| !query.is_empty());
        let who = call.ctx.caller.clone();
        let tx = call.tx().await?;
        // Without "browse channels" (K1.27), only the ones you're in.
        let browses = access::check(
            tx,
            &who,
            Need::permission(Perm::ChannelsBrowse),
            AccessTarget::Space(space),
        )
        .await?
        .is_ok();
        let channels = sqlx::query_as::<_, BrowsedChannel>(
            r#"
            SELECT channel.id, channel.name, channel.topic,
                   channel.visibility = 'members' AS private, channel.auto_join,
                   EXISTS (SELECT 1 FROM conversation_members member
                           WHERE member.conversation_id = channel.id AND member.actor_id = $2)
                       AS joined,
                   (SELECT count(*) FROM conversation_members member
                    WHERE member.conversation_id = channel.id) AS member_count
            FROM conversations channel
            WHERE channel.project_id = $1
              AND channel.kind = 'channel'
              AND channel.archived_at IS NULL
              AND conversation_visible_to(channel.id, $2)
              AND ($5 OR EXISTS (SELECT 1 FROM conversation_members member
                                 WHERE member.conversation_id = channel.id
                                   AND member.actor_id = $2))
              AND ($3::text IS NULL
                   OR strpos(lower(channel.name), lower($3)) > 0
                   OR strpos(lower(COALESCE(channel.topic, '')), lower($3)) > 0)
            ORDER BY lower(channel.name), channel.id
            LIMIT $4
            "#,
        )
        .bind(space)
        .bind(caller)
        .bind(query)
        .bind(MAX_BROWSED)
        .bind(browses)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(BrowsedChannels { channels })
    }
}

// ---------------------------------------------------------------------------
// channel_get

pub struct ChannelDetails;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelInput {
    /// The channel: its id or a link to it.
    pub channel_id: IdRef,
}

impl Targeted for ChannelInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.channel_id.id())
    }
}

impl Operation for ChannelDetails {
    const NAME: &'static str = "channel_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Channel details";
    const DESCRIPTION: &'static str = "A channel's topic and description, whether it's private, \
        whether you're in it, how many are, and what you can change.";

    type Input = ChannelInput;
    type Output = ChannelView;

    async fn run(call: &mut OpCall<'_>, input: ChannelInput) -> Result<ChannelView, OpError> {
        channel_view(call, input.channel_id.id()).await
    }
}

// ---------------------------------------------------------------------------
// channel_join

pub struct ChannelJoin;

impl Operation for ChannelJoin {
    const NAME: &'static str = "channel_join";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Join channel";
    const DESCRIPTION: &'static str = "Join a public channel, so it's in your sidebar. You can \
        read and post in any public channel without joining. A private channel's managers add \
        people instead (conversation_members_update). Joining twice changes nothing.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = ChannelInput;
    type Output = ChannelView;

    async fn run(call: &mut OpCall<'_>, input: ChannelInput) -> Result<ChannelView, OpError> {
        let channel_id = input.channel_id.id();
        let caller = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let channel =
            lock_conversation_for(tx, caller, channel_id, Action::ProjectView, false).await?;
        if channel.kind != "channel" {
            return Err(invalid("channelId", "Only channels are joined."));
        }
        if channel.private {
            return Err(invalid(
                "channelId",
                "A private channel's managers add people to it.",
            ));
        }
        let joined = sqlx::query(
            r#"
            INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
            VALUES ($1, $2, $3, $3)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(channel.project_id)
        .bind(channel_id)
        .bind(caller)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?
        .rows_affected()
            > 0;
        if joined {
            announce_change(call, channel_id).await?;
        }
        call.audit_target("conversation", channel_id, serde_json::json!({}));
        channel_view(call, channel_id).await
    }
}

// ---------------------------------------------------------------------------
// channel_leave

pub struct ChannelLeave;

impl Operation for ChannelLeave {
    const NAME: &'static str = "channel_leave";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Leave channel";
    const DESCRIPTION: &'static str = "Leave a channel, so it's no longer in your sidebar. A \
        public one stays open to you: you can still find it, read it and join again. Leaving a \
        private one means you no longer see it, until someone who manages it adds you back.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = ChannelInput;
    type Output = ChannelLeft;

    async fn run(call: &mut OpCall<'_>, input: ChannelInput) -> Result<ChannelLeft, OpError> {
        let channel_id = input.channel_id.id();
        let caller = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let channel =
            lock_conversation_for(tx, caller, channel_id, Action::ProjectView, true).await?;
        if channel.kind != "channel" {
            return Err(invalid("channelId", "Only channels are left this way."));
        }
        call.audit_target("conversation", channel_id, serde_json::json!({}));
        let tx = call.tx().await?;
        if channel.private {
            // The members change of C2.6, which also withdraws an agent's
            // open requests there and tells its other tabs.
            change_group_members(
                tx,
                GroupEditor { actor_id: caller },
                channel_id,
                &[],
                &[caller],
                None,
            )
            .await?;
            return Ok(ChannelLeft {
                channel_id,
                still_visible: false,
            });
        }
        let left = sqlx::query(
            "DELETE FROM conversation_members WHERE conversation_id = $1 AND actor_id = $2",
        )
        .bind(channel_id)
        .bind(caller)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?
        .rows_affected()
            > 0;
        if left {
            announce_change(call, channel_id).await?;
        }
        Ok(ChannelLeft {
            channel_id,
            still_visible: true,
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelLeft {
    #[schemars(with = "String")]
    pub channel_id: Uuid,
    /// A public channel: you can still find it, read it and join again.
    pub still_visible: bool,
}

// ---------------------------------------------------------------------------
// channel_update

pub struct ChannelUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelUpdateInput {
    /// The channel: its id or a link to it.
    pub channel_id: IdRef,
    /// The version you last read; a change made since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// One line on what it's for now, up to 250 characters; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// What it's for, up to 2,000 characters; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A public channel: true has everyone in the project join it now, and
    /// everyone who arrives later; false stops taking in newcomers. Needs
    /// someone who manages the channel or moderates the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_join: Option<bool>,
}

impl Targeted for ChannelUpdateInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.channel_id.id())
    }
}

fn cleared(value: Option<String>) -> Option<Option<String>> {
    value.map(|value| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    })
}

impl Operation for ChannelUpdate {
    const NAME: &'static str = "channel_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "channels";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change channel";
    const DESCRIPTION: &'static str = "Set a channel's topic (one line on what it's on now) or \
        description, or, for someone who manages it or moderates the project, whether everyone \
        joins it (autoJoin). Rename or archive a channel with conversation_update.";
    const EMITS: &'static [&'static str] = &["conversation.updated"];

    type Input = ChannelUpdateInput;
    type Output = ChannelView;

    async fn run(call: &mut OpCall<'_>, input: ChannelUpdateInput) -> Result<ChannelView, OpError> {
        let channel_id = input.channel_id.id();
        let topic = cleared(input.topic);
        let description = cleared(input.description);
        if let Some(Some(topic)) = &topic
            && (topic.chars().count() > MAX_TOPIC_CHARACTERS || topic.chars().any(char::is_control))
        {
            return Err(invalid(
                "topic",
                "A topic is one line of at most 250 characters.",
            ));
        }
        if let Some(Some(description)) = &description
            && description.chars().count() > MAX_DESCRIPTION_CHARACTERS
        {
            return Err(invalid(
                "description",
                "A description holds at most 2,000 characters.",
            ));
        }
        if topic.is_none() && description.is_none() && input.auto_join.is_none() {
            return Err(invalid(
                "topic",
                "Give a topic, a description or autoJoin to change.",
            ));
        }
        let caller = call.ctx.caller.account_id;
        let who = call.ctx.caller.clone();
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let tx = call.tx().await?;
        let channel =
            lock_conversation_for(tx, caller, channel_id, Action::MessageSend, false).await?;
        if channel.kind != "channel" {
            return Err(invalid("channelId", "Only channels have a topic here."));
        }
        if let Some(expected) = input.expected_version
            && expected != channel.version
        {
            return Err(AppError::VersionConflict {
                resource: "conversation",
                current_version: channel.version,
            }
            .into());
        }
        if let Some(auto_join) = input.auto_join {
            if channel.private {
                return Err(invalid(
                    "autoJoin",
                    "A private channel's managers add people to it.",
                ));
            }
            let manager = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (SELECT 1 FROM conversation_members
                               WHERE conversation_id = $1 AND actor_id = $2 AND manager)
                "#,
            )
            .bind(channel_id)
            .bind(caller)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            let moderates = access::check(
                tx,
                &who,
                Need::permission(Perm::ChannelsModerate),
                AccessTarget::Space(space),
            )
            .await?
            .is_ok();
            if !manager && !moderates {
                return Err(OpError::Denied {
                    reason: crate::ops::error::DenyReason::Role {
                        permission: Perm::ChannelsModerate,
                    },
                    message: std::borrow::Cow::Borrowed(
                        "Only someone who manages this channel or moderates the project changes \
                         who joins it.",
                    ),
                });
            }
            let tx = call.tx().await?;
            sqlx::query("UPDATE conversations SET auto_join = $2 WHERE id = $1")
                .bind(channel_id)
                .bind(auto_join)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
            if auto_join {
                sqlx::query(
                    r#"
                    INSERT INTO conversation_members
                        (project_id, conversation_id, actor_id, added_by_actor_id)
                    SELECT $1, $2, membership.actor_id, $3
                    FROM project_memberships membership
                    WHERE membership.project_id = $1 AND membership.enabled
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(channel.project_id)
                .bind(channel_id)
                .bind(caller)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
            }
        }
        let tx = call.tx().await?;
        if let Some(topic) = topic {
            sqlx::query("UPDATE conversations SET topic = $2 WHERE id = $1")
                .bind(channel_id)
                .bind(topic)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        }
        if let Some(description) = description {
            sqlx::query("UPDATE conversations SET description = $2 WHERE id = $1")
                .bind(channel_id)
                .bind(description)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        }
        announce_change(call, channel_id).await?;
        call.audit_target("conversation", channel_id, serde_json::json!({}));
        channel_view(call, channel_id).await
    }
}
