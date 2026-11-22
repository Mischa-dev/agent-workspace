//! Change sets as operations (C1.9): `change_list` reads a thing's recent
//! changes (what each touched, who made it, whether it can be undone) and
//! `change_undo` takes one back.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ChangeSetView, Refusal};
use crate::{
    memory::Writer,
    ops::{
        ConflictReason, DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm,
        Registry, Target, Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ChangeList>().add::<ChangeUndo>();
}

const CHANGE: &str = "That change";

/// The kinds that record change sets today.
const KINDS: &[&str] = &["page", "sheet", "whiteboard"];

// ------------------------------------------------------------------ list

/// List a thing's recent changes.
pub struct ChangeList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeListInput {
    /// The kind of thing: page, sheet or whiteboard.
    pub thing_kind: String,
    /// The thing: its id or link.
    pub thing_id: IdRef,
    /// How far back, in hours (default 24, at most 2,160).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_hours: Option<u16>,
    /// How many, newest first (default 50, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

fn known_kind(kind: &str) -> Option<&'static str> {
    KINDS.iter().copied().find(|known| *known == kind)
}

impl Targeted for ChangeListInput {
    fn target(&self) -> Target {
        match known_kind(&self.thing_kind) {
            Some(kind) => Target::thing(kind, self.thing_id.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeListOutput {
    /// Newest first.
    pub changes: Vec<ChangeSetView>,
}

impl Operation for ChangeList {
    const NAME: &'static str = "change_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List recent changes";
    const DESCRIPTION: &'static str = "List a page's, sheet's or whiteboard's recent changes, \
        newest first: who made each, the versions it spans, what it touched (a page's blocks by \
        lasting id, a sheet's cells, a board's elements), and whether it can be undone. Undo one \
        with change_undo.";

    type Input = ChangeListInput;
    type Output = ChangeListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ChangeListInput,
    ) -> Result<ChangeListOutput, OpError> {
        let kind = known_kind(&input.thing_kind).ok_or_else(|| {
            OpError::invalid(
                "thingKind",
                "Pages, sheets and whiteboards keep changes (page, sheet, whiteboard).",
            )
        })?;
        let viewer = call.account_id();
        let hours = i32::from(input.since_hours.unwrap_or(24).clamp(1, 2_160));
        let limit = i64::from(input.limit.unwrap_or(50).clamp(1, 200));
        let tx = call.tx().await?;
        let changes = super::list(tx, viewer, kind, input.thing_id.id(), hours, limit).await?;
        Ok(ChangeListOutput { changes })
    }
}

// ------------------------------------------------------------------ undo

/// Undo a change.
pub struct ChangeUndo;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeUndoInput {
    /// The change: its id, from change_list.
    pub change_set_id: IdRef,
}

impl Targeted for ChangeUndoInput {
    fn target(&self) -> Target {
        Target::thing("change_set", self.change_set_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeUndoOutput {
    /// The undo, itself a change that can be undone.
    pub change: ChangeSetView,
    /// The thing's version after the undo.
    pub version: i64,
    /// Items the change touched that were changed again since, left as
    /// they are.
    pub skipped: Vec<String>,
}

fn refused(refusal: Refusal) -> OpError {
    match refusal {
        Refusal::NotFound => OpError::not_found(CHANGE),
        Refusal::NotYours => OpError::Denied {
            reason: DenyReason::Rule,
            message: "Undoing a change needs edit access to what it changed.".into(),
        },
        Refusal::AlreadyUndone => OpError::Conflict {
            reason: ConflictReason::State {
                code: "already_undone",
            },
            message: "That change has already been undone. Undo the undo to bring it back.".into(),
            current: None,
        },
        Refusal::NotKept { version_from } => OpError::Conflict {
            reason: ConflictReason::State { code: "not_kept" },
            message: format!(
                "That change was too large to undo in a step. Restore version {version_from} instead."
            )
            .into(),
            current: None,
        },
        Refusal::Archived => OpError::Conflict {
            reason: ConflictReason::Archived,
            message: "It's archived. Bring it back first.".into(),
            current: None,
        },
        Refusal::Refused(message) => OpError::Conflict {
            reason: ConflictReason::State {
                code: "changed_since",
            },
            message: message.into(),
            current: None,
        },
        Refusal::NothingLeft => OpError::Conflict {
            reason: ConflictReason::State {
                code: "changed_since",
            },
            message: "Everything that change touched has been changed again since, so there's nothing to undo."
                .into(),
            current: None,
        },
    }
}

impl Operation for ChangeUndo {
    const NAME: &'static str = "change_undo";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    // Undo needs edit on the thing it changed: `change_sets::undo` asks
    // `access::write_mode` for it, since no one permission covers every
    // kind.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const EMITS: &'static [&'static str] = &["page.updated", "sheet.updated", "whiteboard.updated"];
    const TITLE: &'static str = "Undo a change";
    const DESCRIPTION: &'static str = "Undo one change to a page, sheet or whiteboard (change_list \
        finds them): what it replaced comes back for every block, cell or element nobody has \
        changed since, and the rest are listed as skipped. The undo is a change of its own, so it can be undone too. Needs edit access.";

    type Input = ChangeUndoInput;
    type Output = ChangeUndoOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ChangeUndoInput,
    ) -> Result<ChangeUndoOutput, OpError> {
        let caller = call.ctx.caller.clone();
        let writer = Writer::from(call.ctx);
        let board_caller = crate::whiteboards::Caller::from(call.ctx);
        let viewer = call.account_id();
        let id = input.change_set_id.id();
        let tx = call.tx().await?;
        let undid = super::undo(tx, &caller, &board_caller, writer, id)
            .await?
            .map_err(refused)?;
        let change = super::get(tx, viewer, undid.change_set_id)
            .await?
            .ok_or(OpError::Unavailable)?;
        call.audit_target(
            "change_set",
            id,
            json!({"undoneBy": undid.change_set_id, "skipped": undid.skipped.len()}),
        );
        Ok(ChangeUndoOutput {
            change,
            version: undid.version,
            skipped: undid.skipped,
        })
    }
}
