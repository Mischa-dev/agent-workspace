//! "Mark all read" as an operation (C2.2): `conversations_mark_read` reads
//! every conversation the caller can read in the workspace (or one space)
//! through the same `mark_seen` as opening each one, so the inbox and the
//! other tabs follow. The web's command and shortcut and agents run it.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

/// The most conversations one call reads; `more` says to call again.
const MAX_MARKED: i64 = 500;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ConversationsMarkRead>();
}

/// Mark every conversation read.
pub struct ConversationsMarkRead;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationsMarkReadInput {
    /// Only this space (a project): its id, link or key. Left out, every
    /// channel, thread, DM and group you can read in the workspace.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for ConversationsMarkReadInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationsMarkedRead {
    /// How many conversations had something unread and are now read.
    pub marked: i64,
    /// True when there were more than one call reads: call again.
    pub more: bool,
}

impl Operation for ConversationsMarkRead {
    const NAME: &'static str = "conversations_mark_read";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Mark all read";
    const DESCRIPTION: &'static str = "Mark every conversation you can read as read (channels, \
        threads, DMs and groups), or only those in one space with spaceId. Their notifications \
        are read too. To read one conversation, use conversation_mark_read.";
    const EMITS: &'static [&'static str] = &["conversation.read_state.updated"];

    type Input = ConversationsMarkReadInput;
    type Output = ConversationsMarkedRead;

    async fn run(
        call: &mut OpCall<'_>,
        input: ConversationsMarkReadInput,
    ) -> Result<ConversationsMarkedRead, OpError> {
        let actor = call.account_id();
        let connection = call.ctx.caller.connection_id;
        let confined = call.ctx.caller.confined_space;
        let workspace_id = call.ctx.workspace_id;
        let space_id = input.space_id.and(call.ctx.space_id);
        if input.space_id.is_some() && space_id.is_none() {
            return Err(OpError::not_found("That space"));
        }
        let tx = call.tx().await?;
        // Every conversation with something past the caller's mark, in a
        // space they are in and that they may read (rule 7).
        let unread = sqlx::query_as::<_, (Uuid, Uuid, i64)>(
            r#"
            SELECT conversation.project_id, conversation.id, conversation.next_entry_sequence - 1
            FROM conversations conversation
            JOIN projects project ON project.id = conversation.project_id
            LEFT JOIN conversation_read_states mark
              ON mark.project_id = conversation.project_id
             AND mark.conversation_id = conversation.id
             AND mark.actor_id = $2
            WHERE project.organization_id = $1
              AND ($4::uuid IS NULL OR conversation.project_id = $4)
              AND conversation.next_entry_sequence - 1 > COALESCE(mark.highest_seen_sequence, 0)
              AND actor_can_access_project($2, $3, conversation.project_id)
              AND ($6::uuid IS NULL OR conversation.project_id = $6)
              AND conversation_readable_by(conversation.id, $2)
            ORDER BY conversation.id
            LIMIT $5
            "#,
        )
        .bind(workspace_id)
        .bind(actor)
        .bind(connection)
        .bind(space_id)
        .bind(MAX_MARKED + 1)
        .bind(confined)
        .fetch_all(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        let more = unread.len() as i64 > MAX_MARKED;
        let mut marked = 0;
        for (project_id, conversation_id, sequence) in unread.into_iter().take(MAX_MARKED as usize)
        {
            super::mark_seen(tx, project_id, conversation_id, actor, sequence).await?;
            marked += 1;
        }
        call.audit_target(
            "account",
            actor,
            json!({ "marked": marked, "space": space_id.is_some() }),
        );
        Ok(ConversationsMarkedRead { marked, more })
    }
}
