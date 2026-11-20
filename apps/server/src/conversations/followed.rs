//! The threads you follow, across every space (C2.36): `thread_followed_list`.
//! A thread is followed when your level for it is "every message", chosen or
//! set for you when you started, posted in or were mentioned in it (C2.5).
//! Newest reply first, with each thread's status and outcome, where it
//! lives, and whether anything in it is new to you.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted},
};

/// The most threads one call lists.
const MAX_LISTED: i64 = 100;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ThreadFollowedList>();
}

/// List the threads you follow.
pub struct ThreadFollowedList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadFollowedListInput {
    /// true: leave out threads that are settled (Decided, Done or Dropped,
    /// or with an outcome). Left out, every thread you follow.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open_only: bool,
    /// At most this many (default and most 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl Targeted for ThreadFollowedListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FollowedThreadView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// Its status, such as Open, Decided or Done.
    pub status: String,
    /// The one line it settled on, when it has one.
    pub outcome: Option<String>,
    /// The space it lives in; for a thread in a DM or group, the direct
    /// scope, named Messages.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub space_name: String,
    /// The channel, DM or group it's under.
    #[schemars(with = "String")]
    pub parent_id: Uuid,
    pub parent_kind: String,
    /// The channel's name; none for a DM or group.
    pub parent_name: Option<String>,
    /// The message it was opened from, if any.
    #[schemars(with = "Option<String>")]
    pub origin_message_id: Option<Uuid>,
    pub message_count: i64,
    /// When its newest message came, or when it was made.
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    /// Something in it is newer than what you've read.
    pub unread: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadFollowedListOutput {
    pub threads: Vec<FollowedThreadView>,
}

/// The threads `viewer` follows in `workspace_id`, newest activity first,
/// each one they can still read.
pub async fn followed(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    viewer: Uuid,
    reach: crate::drafts::Reach,
    open_only: bool,
    limit: i64,
) -> Result<Vec<FollowedThreadView>, AppError> {
    Ok(sqlx::query_as::<_, FollowedThreadView>(
        r#"
        SELECT thread.id, thread.name, thread.thread_status AS status,
               thread.thread_outcome AS outcome,
               COALESCE(space.id, direct_scope_id()) AS space_id,
               COALESCE(space.name, 'Messages') AS space_name,
               parent.id AS parent_id, parent.kind AS parent_kind,
               CASE WHEN parent.kind = 'channel' THEN parent.name END AS parent_name,
               thread.origin_message_id,
               (SELECT count(*) FROM messages message
                WHERE message.conversation_id = thread.id AND message.removed_at IS NULL)
                   AS message_count,
               COALESCE((SELECT max(message.created_at) FROM messages message
                         WHERE message.conversation_id = thread.id
                           AND message.removed_at IS NULL),
                        thread.created_at) AS last_activity_at,
               thread.last_message_sequence
                   > COALESCE(read_state.highest_seen_sequence, 0) AS unread
        FROM conversation_notification_levels level
        JOIN conversations thread ON thread.id = level.conversation_id
        JOIN conversations parent ON parent.id = thread.parent_channel_id
        LEFT JOIN projects space ON space.id = thread.project_id
        LEFT JOIN project_memberships me
          ON me.project_id = thread.project_id AND me.actor_id = $2 AND me.enabled
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = thread.id AND read_state.actor_id = $2
        WHERE level.actor_id = $2
          AND level.level = 'all'
          AND thread.kind = 'thread'
          AND thread.archived_at IS NULL
          AND conversation_visible_to(thread.id, $2)
          AND (
              -- A thread in a space of this workspace.
              (space.organization_id = $1
               AND space.archived_at IS NULL
               AND me.actor_id IS NOT NULL
               AND actor_can_access_project($2, $5, thread.project_id)
               AND ($6::uuid IS NULL OR thread.project_id = $6))
              -- A thread in a DM or group (K4.15): one kept by this
              -- workspace, or a personal one, which shows in every
              -- workspace; a credential held to one space reaches none.
              OR (thread.project_id IS NULL
                  AND thread.workspace_id IN ($1, direct_scope_id())
                  AND $6::uuid IS NULL
                  AND ($5::uuid IS NULL OR agent_connection_can_reach_direct($5)))
          )
          AND (NOT $3 OR (thread.thread_outcome IS NULL
                          AND lower(thread.thread_status) <> ALL($7::text[])))
        ORDER BY last_activity_at DESC, thread.id DESC
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(viewer)
    .bind(open_only)
    .bind(limit)
    .bind(reach.connection_id)
    .bind(reach.confined_space)
    .bind(&crate::conversations::SETTLED_THREAD_STATUSES[..])
    .fetch_all(&mut *connection)
    .await?)
}

impl Operation for ThreadFollowedList {
    const NAME: &'static str = "thread_followed_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List followed threads";
    const DESCRIPTION: &'static str = "Show the threads you follow in every space of this \
        workspace, newest reply first: each one's status and outcome, where it lives, and \
        whether it has anything new for you. You follow a thread you started, posted in or \
        were mentioned in, or set to every message. Open one with conversation_open.";

    type Input = ThreadFollowedListInput;
    type Output = ThreadFollowedListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ThreadFollowedListInput,
    ) -> Result<ThreadFollowedListOutput, OpError> {
        let limit = input.limit.unwrap_or(MAX_LISTED).clamp(1, MAX_LISTED);
        let (workspace, viewer) = (call.ctx.workspace_id, call.account_id());
        let reach = crate::drafts::Reach {
            connection_id: call.ctx.caller.connection_id,
            confined_space: call.ctx.caller.confined_space,
        };
        let connection = &mut **call.tx().await?;
        Ok(ThreadFollowedListOutput {
            threads: followed(connection, workspace, viewer, reach, input.open_only, limit).await?,
        })
    }
}
