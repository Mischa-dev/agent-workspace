//! `follow_set` and `follow_list` (A2.9): following a thing or a space, for
//! people and agents alike, through the web, the tool of the same name, the
//! v2 API and automations.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{FollowKind, FollowLevel, FollowView};
use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FollowSet>().add::<FollowList>();
}

fn target(kind: FollowKind, id: Uuid) -> Target {
    match kind {
        FollowKind::Space => Target::Space(id),
        FollowKind::Task => Target::thing("task", id),
        FollowKind::Page => Target::thing("page", id),
        FollowKind::Sheet => Target::thing("sheet", id),
        FollowKind::Record => Target::thing("record", id),
        FollowKind::Whiteboard => Target::thing("whiteboard", id),
        FollowKind::Conversation => Target::thing("conversation", id),
    }
}

/// Follow something, change how much, or stop.
pub struct FollowSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowSetInput {
    /// What it is: space, task, page, sheet, record, whiteboard or
    /// conversation (a channel or thread).
    pub kind: FollowKind,
    /// Its id, or a link to it.
    pub id: IdRef,
    /// changes (its own updates) or everything (its comments and messages
    /// too). Left out: changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<FollowLevel>,
    /// false stops following. Left out: true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

impl Targeted for FollowSetInput {
    fn target(&self) -> Target {
        target(self.kind, self.id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FollowSetOutput {
    pub kind: FollowKind,
    #[schemars(with = "String")]
    pub id: Uuid,
    /// How you follow it now; null when you don't.
    pub level: Option<FollowLevel>,
    /// Whether this call changed anything.
    pub changed: bool,
}

impl Operation for FollowSet {
    const NAME: &'static str = "follow_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Follow";
    const DESCRIPTION: &'static str = "Follow a space, task, page, sheet, record, whiteboard or \
        conversation to hear about it in your events and notifications: level changes hears \
        its updates, everything adds its comments and messages. on: false stops. Following \
        never shows you more than you can already see. follow_list lists what you follow.";
    const EMITS: &'static [&'static str] = &["follows.changed"];

    type Input = FollowSetInput;
    type Output = FollowSetOutput;

    async fn run(call: &mut OpCall<'_>, input: FollowSetInput) -> Result<FollowSetOutput, OpError> {
        let account = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let space_id = call.ctx.space_id;
        let id = input.id.id();
        let level = input
            .on
            .unwrap_or(true)
            .then(|| input.level.unwrap_or_default());
        let tx = call.tx().await?;
        if level.is_some() && !super::visible(tx, input.kind, id, account).await? {
            return Err(OpError::not_found("That"));
        }
        let changed =
            super::set(tx, workspace_id, space_id, account, input.kind, id, level).await?;
        if changed {
            // The caller's other tabs read what they follow again.
            let space = crate::spaces::workspace::ensure(tx, workspace_id).await?;
            crate::outbox::enqueue_typed(
                tx,
                space,
                crate::outbox::OutboxEventPayload::FollowsChanged(
                    crate::outbox::FollowsChangedPayload { actor_id: account },
                ),
            )
            .await?;
        }
        call.audit_target(
            input.kind.key(),
            id,
            json!({ "level": level.map(FollowLevel::key) }),
        );
        Ok(FollowSetOutput {
            kind: input.kind,
            id,
            level,
            changed,
        })
    }
}

/// What the caller follows.
pub struct FollowList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowListInput {
    /// Only what you follow in this space. Left out: the whole workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
}

impl Targeted for FollowListInput {
    fn target(&self) -> Target {
        match self.space {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FollowListOutput {
    pub follows: Vec<FollowView>,
}

impl Operation for FollowList {
    const NAME: &'static str = "follow_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "What you follow";
    const DESCRIPTION: &'static str = "The spaces and things you follow in this workspace (or \
        one space), newest first, with how much of each (changes or everything). Change one \
        with follow_set.";

    type Input = FollowListInput;
    type Output = FollowListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: FollowListInput,
    ) -> Result<FollowListOutput, OpError> {
        let account = call.account_id();
        let workspace_id = call.ctx.workspace_id;
        let space = input.space.map(IdRef::id);
        let tx = call.tx().await?;
        Ok(FollowListOutput {
            follows: super::list(tx, workspace_id, space, account).await?,
        })
    }
}
