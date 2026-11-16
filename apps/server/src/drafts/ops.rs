//! Drafts as operations (C2.28): `message_draft_set` saves or clears the
//! caller's draft in a conversation; `message_draft_list` lists theirs. The
//! web composer, agents and apps acting for a person run these.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::DraftView;
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<MessageDraftSet>().add::<MessageDraftList>();
}

/// Save or clear a draft.
pub struct MessageDraftSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDraftSetInput {
    /// The conversation or thread the draft is for.
    pub conversation_id: IdRef,
    /// The draft: an object with its `text` (and the composer's own
    /// fields). Left out, or with no text, the draft is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<Value>,
    /// When your device saved it (an ISO 8601 time). A draft saved since
    /// on another device stays, and this one is kept as a copy for a day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

impl Targeted for MessageDraftSetInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftSaved {
    /// The draft kept now; none when it was cleared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftView>,
    /// True when a newer draft from another device stayed: yours is its
    /// `previous`.
    pub kept_newer: bool,
}

impl Operation for MessageDraftSet {
    const NAME: &'static str = "message_draft_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Save draft";
    const DESCRIPTION: &'static str = "Save what you are writing in a conversation so it waits \
        on your other devices, or clear it (leave draft out). Only you ever see your drafts.";
    const EMITS: &'static [&'static str] = &["message_draft.changed"];

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageDraftSetInput,
    ) -> Result<DraftSaved, OpError> {
        let draft = match &input.draft {
            Some(draft) => {
                super::checked(draft).map_err(|message| OpError::invalid("draft", message))?
            }
            None => None,
        };
        let saved_at = input
            .saved_at
            .as_deref()
            .map(|value| {
                DateTime::parse_from_rfc3339(value.trim())
                    .map(|at| at.with_timezone(&Utc))
                    .map_err(|_| {
                        OpError::invalid("savedAt", "Pass a time such as 2026-11-15T17:00:00Z.")
                    })
            })
            .transpose()?;
        let account = call.account_id();
        let conversation_id = input.conversation_id.id();
        let tx = call.tx().await?;
        // Rule 7: the conversation is one the writer may read, in a space
        // they are in (the dispatcher located it the same way).
        let readable = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations conversation
                WHERE conversation.id = $1
                  AND actor_can_access_project($2, NULL, conversation.project_id)
                  AND conversation_readable_by(conversation.id, $2)
            )
            "#,
        )
        .bind(conversation_id)
        .bind(account)
        .fetch_one(&mut **tx)
        .await
        .map_err(crate::error::AppError::from)?;
        if !readable {
            return Err(OpError::not_found("That conversation"));
        }
        let saved = super::save(tx, account, conversation_id, draft, saved_at).await?;
        call.audit_target(
            "conversation",
            conversation_id,
            json!({ "cleared": saved.view.is_none(), "kept_newer": saved.kept_newer }),
        );
        Ok(DraftSaved {
            draft: saved.view,
            kept_newer: saved.kept_newer,
        })
    }

    type Input = MessageDraftSetInput;
    type Output = DraftSaved;
}

/// List your drafts.
pub struct MessageDraftList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDraftListInput {
    /// Only the draft in this conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<IdRef>,
}

impl Targeted for MessageDraftListInput {
    fn target(&self) -> Target {
        match self.conversation_id {
            Some(conversation) => Target::thing("conversation", conversation.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageDraftListOutput {
    /// Newest first, in conversations you can still read.
    pub drafts: Vec<DraftView>,
}

impl Operation for MessageDraftList {
    const NAME: &'static str = "message_draft_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List drafts";
    const DESCRIPTION: &'static str = "List the messages you have started and not sent in this \
        workspace, newest first, or the one draft in a conversation (conversationId).";

    type Input = MessageDraftListInput;
    type Output = MessageDraftListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: MessageDraftListInput,
    ) -> Result<MessageDraftListOutput, OpError> {
        let account = call.account_id();
        let workspace_id: Uuid = call.ctx.workspace_id;
        let mut connection = call
            .state
            .pool
            .acquire()
            .await
            .map_err(crate::error::AppError::from)?;
        let drafts = super::listed(
            &mut connection,
            account,
            Some(workspace_id),
            input.conversation_id.map(IdRef::id),
        )
        .await?;
        Ok(MessageDraftListOutput { drafts })
    }
}
