//! Decisions as operations (K2.5): the worked example every later module
//! copies (`.claude/skills/add-operation/SKILL.md` walks through it).
//! `decision_record` writes in the dispatcher's transaction; `decision_list`
//! reads. The web routes (`POST` and `GET /api/v1/projects/{id}/decisions`),
//! the MCP tools of the same names, the v2 route and automation rules all
//! run these.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{DecisionInput, DecisionView, Writer};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
    refs::{IdRef, MessageRef},
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<DecisionRecord>().add::<DecisionList>();
}

/// Record a decision.
pub struct DecisionRecord;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionRecordInput {
    /// The space (a project): its id, link or key. Not needed when you pass
    /// sourceMessageId or sourceConversationId.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// What was decided, in one line.
    pub title: String,
    /// Why, the options considered, and anything a newcomer should know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The message where it was decided: its id, or a link to it.
    #[serde(
        default,
        alias = "fromMessageId",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_message_id: Option<MessageRef>,
    /// The conversation or thread where it was decided.
    #[serde(
        default,
        alias = "conversationId",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_conversation_id: Option<IdRef>,
    /// An earlier decision this one replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<IdRef>,
    /// Also pin the decision to its conversation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin: bool,
}

impl Targeted for DecisionRecordInput {
    /// The space named, else the space of the message or conversation it
    /// came from. A message or conversation the caller can't see is
    /// `not_found`, so a private conversation stays private.
    fn target(&self) -> Target {
        match (
            self.space_id,
            self.source_message_id,
            self.source_conversation_id,
        ) {
            (Some(space), _, _) => Target::Space(space.id()),
            (None, Some(message), _) => Target::thing("message", message.id()),
            (None, None, Some(conversation)) => Target::thing("conversation", conversation.id()),
            (None, None, None) => Target::Workspace(None),
        }
    }
}

impl Operation for DecisionRecord {
    const NAME: &'static str = "decision_record";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Record decision";
    const DESCRIPTION: &'static str = "Record a decision: what was chosen and why, linked to the \
        message or conversation where it happened. Pass supersedes to replace an older decision. \
        A decision from a private conversation stays as private as it.";
    const EMITS: &'static [&'static str] = &["decision.recorded"];

    type Input = DecisionRecordInput;
    type Output = DecisionView;

    async fn run(
        call: &mut OpCall<'_>,
        input: DecisionRecordInput,
    ) -> Result<DecisionView, OpError> {
        let space_id = call.ctx.space_id.ok_or_else(|| {
            OpError::invalid(
                "spaceId",
                "Say which space (spaceId), or pass the message or conversation it came from.",
            )
        })?;
        let writer = Writer::from(call.ctx);
        let viewer = call.account_id();
        let decision = DecisionInput {
            title: input.title,
            detail: input.detail,
            source_message_id: input.source_message_id.map(MessageRef::id),
            source_conversation_id: input.source_conversation_id.map(IdRef::id),
            supersedes: input.supersedes.map(IdRef::id),
        };
        let tx = call.tx().await?;
        let id = super::record_decision(tx, writer, space_id, &decision).await?;
        if input.pin
            && let Some(conversation_id) = super::decision_conversation(tx, &decision).await?
        {
            super::set_pin(tx, writer, conversation_id, "decision", id, true).await?;
        }
        let view = super::get_decision(&mut **tx, id, viewer)
            .await?
            .ok_or(OpError::Unavailable)?;
        call.audit_target("decision", id, json!({}));
        Ok(view)
    }
}

/// List a space's decisions.
pub struct DecisionList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionListInput {
    /// The space (a project): its id, link or key (such as "AWD").
    #[serde(alias = "projectId")]
    pub space_id: IdRef,
    /// Include decisions that were later replaced.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_superseded: bool,
    /// How many, newest first (default 50, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

impl Targeted for DecisionListInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionListOutput {
    /// Newest first. Decisions from conversations you can't see are left out.
    pub decisions: Vec<DecisionView>,
}

impl Operation for DecisionList {
    const NAME: &'static str = "decision_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List decisions";
    const DESCRIPTION: &'static str = "List a space's decisions, newest first: what was chosen, \
        why, and where. Superseded ones are left out unless includeSuperseded.";

    type Input = DecisionListInput;
    type Output = DecisionListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: DecisionListInput,
    ) -> Result<DecisionListOutput, OpError> {
        let decisions = super::list_decisions(
            &call.state.pool,
            input.space_id.id(),
            call.account_id(),
            input.include_superseded,
            i64::from(input.limit.unwrap_or(50).clamp(1, 200)),
        )
        .await?;
        Ok(DecisionListOutput { decisions })
    }
}
