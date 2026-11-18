//! Proposals as operations (C1.7): `proposal_list` reads them and
//! `proposal_decide` accepts, rejects or withdraws one, for every kind that
//! can be proposed. The page tools (`page_suggestions`,
//! `page_suggestion_decide`) and the page routes stay as aliases.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Decision, Listing, PROPOSABLE_KINDS, ProposalView};
use crate::{
    memory::Writer,
    ops::{
        ConflictReason, DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm,
        Registry, Target, Targeted, refs::IdRef,
    },
    page_docs::suggestions::Refusal,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ProposalList>().add::<ProposalDecide>();
}

const PROPOSAL: &str = "That proposal";

// ------------------------------------------------------------------ list

/// List proposals.
pub struct ProposalList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposalListInput {
    /// page, sheet or whiteboard, with thingId.
    /// Leave both out to list the proposals you made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_kind: Option<String>,
    /// The thing: its id or link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_id: Option<IdRef>,
    /// Include decided proposals too (accepted, rejected, withdrawn).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_decided: bool,
    /// How many, newest first (default 50, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

fn known_kind(kind: &str) -> Option<&'static str> {
    PROPOSABLE_KINDS
        .iter()
        .copied()
        .find(|known| *known == kind)
}

impl Targeted for ProposalListInput {
    fn target(&self) -> Target {
        match (
            self.thing_kind.as_deref().and_then(known_kind),
            self.thing_id,
        ) {
            (Some(kind), Some(id)) => Target::thing(kind, id.id()),
            _ => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposalListOutput {
    /// Newest first. Proposals on things you can't see are left out.
    pub proposals: Vec<ProposalView>,
}

impl Operation for ProposalList {
    const NAME: &'static str = "proposal_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List proposals";
    const DESCRIPTION: &'static str = "List proposed changes waiting for someone who edits the \
        thing: pass thingKind (page, sheet or whiteboard) and thingId for one thing's, or nothing for the ones you \
        made. Each says what it changes and what it read. Decide one with proposal_decide.";

    type Input = ProposalListInput;
    type Output = ProposalListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ProposalListInput,
    ) -> Result<ProposalListOutput, OpError> {
        let listing = match (input.thing_kind.as_deref(), input.thing_id) {
            (None, None) => Listing::Mine,
            (Some(kind), Some(id)) => Listing::Thing(
                known_kind(kind).ok_or_else(|| {
                    OpError::invalid(
                        "thingKind",
                        "Pages, sheets and whiteboards take proposals (page, sheet, whiteboard).",
                    )
                })?,
                id.id(),
            ),
            _ => {
                return Err(OpError::invalid(
                    "thingId",
                    "Pass thingKind and thingId together, or neither for your own proposals.",
                ));
            }
        };
        let viewer = call.account_id();
        let limit = i64::from(input.limit.unwrap_or(50).clamp(1, 200));
        let tx = call.tx().await?;
        let proposals = super::list(tx, viewer, listing, !input.include_decided, limit).await?;
        Ok(ProposalListOutput { proposals })
    }
}

// ---------------------------------------------------------------- decide

/// Why a decision wasn't taken, as the operation's error.
fn refused(refusal: Refusal) -> OpError {
    match refusal {
        Refusal::NotFound => OpError::not_found(PROPOSAL),
        Refusal::NotYours(message) => OpError::Denied {
            reason: DenyReason::Rule,
            message: message.into(),
        },
        Refusal::AlreadyDecided(status) => OpError::Conflict {
            reason: ConflictReason::State {
                code: "already_decided",
            },
            message: format!("That proposal was already {status}.").into(),
            current: None,
        },
        Refusal::Archived => OpError::Conflict {
            reason: ConflictReason::Archived,
            message: "The thing it changes is archived. Bring it back to accept proposals.".into(),
            current: None,
        },
        Refusal::Stale(why) => OpError::Conflict {
            reason: ConflictReason::State { code: "stale" },
            message: format!(
                "{why} The proposal no longer fits; reject it, or (if it's yours) withdraw it and propose again."
            )
            .into(),
            current: None,
        },
    }
}

/// Accept, reject or withdraw a proposal.
pub struct ProposalDecide;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposalDecideInput {
    /// The proposal: its id.
    pub proposal_id: IdRef,
    /// accept (apply it, as its author's change), reject, or withdraw (the
    /// author takes it back).
    pub decision: DecisionArg,
    /// Accept only these parts of a sheet or whiteboard proposal: positions
    /// in its `change.cells` or `change.elements`, from 0. Leave out to
    /// accept all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<u16>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DecisionArg {
    Accept,
    Reject,
    Withdraw,
}

impl From<DecisionArg> for Decision {
    fn from(value: DecisionArg) -> Self {
        match value {
            DecisionArg::Accept => Self::Accept,
            DecisionArg::Reject => Self::Reject,
            DecisionArg::Withdraw => Self::Withdraw,
        }
    }
}

impl Targeted for ProposalDecideInput {
    fn target(&self) -> Target {
        Target::thing("proposal", self.proposal_id.id())
    }
}

impl Operation for ProposalDecide {
    const NAME: &'static str = "proposal_decide";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    // Accepting and rejecting need edit on the thing it changes, and only
    // the author withdraws: `proposals::decide` asks `access::write_mode`
    // for the thing, since no one permission covers every kind.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &[
        "page.discussion.updated",
        "sheet.proposals_changed",
        "whiteboard.proposals_changed",
    ];
    const TITLE: &'static str = "Decide a proposal";
    const DESCRIPTION: &'static str = "Accept, reject or withdraw a proposed change \
        (proposal_list finds them). Accepting applies it as its author's change, and is refused \
        when the thing has changed where the proposal edits it since it was made. Accepting and \
        rejecting need edit on the thing; only the author withdraws.";

    type Input = ProposalDecideInput;
    type Output = ProposalView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ProposalDecideInput,
    ) -> Result<ProposalView, OpError> {
        let caller = call.ctx.caller.clone();
        let writer = Writer::from(call.ctx);
        let viewer = call.account_id();
        let id = input.proposal_id.id();
        let tx = call.tx().await?;
        match super::decide(
            tx,
            &caller,
            writer,
            id,
            input.decision.into(),
            input.parts.as_deref(),
        )
        .await?
        {
            Ok(_) => {}
            Err(refusal) => return Err(refused(refusal)),
        }
        let view = super::get(tx, viewer, id)
            .await?
            .ok_or(OpError::not_found(PROPOSAL))?;
        call.audit_target(
            "proposal",
            id,
            json!({"status": view.status, "thingKind": view.thing_kind}),
        );
        Ok(view)
    }
}
