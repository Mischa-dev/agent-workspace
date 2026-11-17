//! Links as operations (K3.15): link one thing to another, or take a link
//! away. Linking needs edit on the thing the link starts at and sight of
//! the other end. Dependencies, relations and duplicates between two tasks
//! keep their own tools (`task_depend`, `task_relate`), which check cycles
//! and limits and write the same link store.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
    target::Located,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<ThingLink>().add::<ThingUnlink>();
}

/// The kinds a link made by hand can have.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandLinkKind {
    /// Is about, or discusses, the other.
    About,
    /// Related, either way.
    RelatesTo,
    /// Was made from the other.
    MadeFrom,
}

impl HandLinkKind {
    fn key(self) -> &'static str {
        match self {
            Self::About => "about",
            Self::RelatesTo => "relates_to",
            Self::MadeFrom => "made_from",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThingLinkInput {
    /// The thing the link starts at: its id or link. You need to be able
    /// to edit it.
    pub from: IdRef,
    /// The thing it links to: its id or link, in the same workspace.
    pub to: IdRef,
    pub kind: HandLinkKind,
}

impl Targeted for ThingLinkInput {
    fn target(&self) -> Target {
        Target::thing("thing", self.from.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThingLinkOutput {
    /// False when the link was there already (or, for unlinking, wasn't).
    pub changed: bool,
}

/// The permission editing a thing of `kind` takes, if it can hold links.
fn edit_permission(kind: &str) -> Option<Perm> {
    Some(match kind {
        "task" => Perm::TasksEdit,
        "page" | "decision" => Perm::PagesEdit,
        "whiteboard" => Perm::WhiteboardsEdit,
        "record" => Perm::CollectionsEdit,
        "collection" => Perm::CollectionsBuild,
        "sheet" => Perm::SheetsEdit,
        _ => return None,
    })
}

/// Both ends as the caller sees them, after checking they may edit `from`.
async fn check(
    call: &OpCall<'_>,
    from: uuid::Uuid,
    to: uuid::Uuid,
    title: &'static str,
) -> Result<(String, String), OpError> {
    let from_kind = crate::history::ops::visible_kind(call, from).await?;
    let to_kind = crate::history::ops::visible_kind(call, to).await?;
    if from == to {
        return Err(OpError::invalid("to", "A thing can't link to itself."));
    }
    let Some(permission) = edit_permission(&from_kind) else {
        return Err(OpError::invalid(
            "from",
            "Links made by hand start at a task, page, decision, whiteboard, record, collection or sheet.",
        ));
    };
    let located = Located {
        workspace_id: call.ctx.workspace_id,
        space_id: call.ctx.space_id,
    };
    let mut connection = crate::db::scoped_tx(&call.state.pool)
        .await
        .map_err(crate::error::AppError::from)?;
    crate::ops::access_shim::check(
        &mut connection,
        &call.ctx.caller,
        &call.ctx.target,
        &located,
        Some(permission),
        true,
        title,
    )
    .await?;
    Ok((from_kind, to_kind))
}

/// Link two things.
pub struct ThingLink;

impl Operation for ThingLink {
    const NAME: &'static str = "thing_link";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    // Editing `from`: which permission depends on its kind, checked in `run`.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["link.created"];
    const TITLE: &'static str = "Link two things";
    const DESCRIPTION: &'static str = "Link one thing to another in the same workspace: about \
        (it discusses or concerns the other), relates_to, or made_from. Both show the link from \
        their side (thing_open). For dependencies, relations and duplicates between two tasks use \
        task_depend and task_relate.";

    type Input = ThingLinkInput;
    type Output = ThingLinkOutput;

    async fn run(call: &mut OpCall<'_>, input: ThingLinkInput) -> Result<ThingLinkOutput, OpError> {
        let (from, to) = (input.from.id(), input.to.id());
        let (from_kind, to_kind) = check(call, from, to, Self::TITLE).await?;
        if from_kind == "task" && to_kind == "task" && input.kind == HandLinkKind::RelatesTo {
            return Err(OpError::invalid(
                "kind",
                "Two tasks are related with task_relate, which keeps each task's history.",
            ));
        }
        let account = call.account_id();
        let tx = call.tx().await?;
        let changed = match crate::links::put_explicit(
            tx,
            from,
            to,
            input.kind.key(),
            Some(account),
        )
        .await?
        {
            Ok(changed) => changed,
            Err(_) => return Err(OpError::not_found("That thing")),
        };
        call.audit_target("thing", from, json!({ "kind": input.kind.key(), "to": to }));
        Ok(ThingLinkOutput { changed })
    }
}

/// Take a link away.
pub struct ThingUnlink;

impl Operation for ThingUnlink {
    const NAME: &'static str = "thing_unlink";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["link.deleted"];
    const TITLE: &'static str = "Take a link away";
    const DESCRIPTION: &'static str = "Take away a link made with thing_link (the same from, to \
        and kind; relates_to either way). Links found in text go when the text changes.";

    type Input = ThingLinkInput;
    type Output = ThingLinkOutput;

    async fn run(call: &mut OpCall<'_>, input: ThingLinkInput) -> Result<ThingLinkOutput, OpError> {
        let (from, to) = (input.from.id(), input.to.id());
        check(call, from, to, Self::TITLE).await?;
        let tx = call.tx().await?;
        let changed = crate::links::drop_explicit(
            tx,
            from,
            to,
            input.kind.key(),
            input.kind == HandLinkKind::RelatesTo,
        )
        .await?;
        call.audit_target("thing", from, json!({ "kind": input.kind.key(), "to": to }));
        Ok(ThingLinkOutput { changed })
    }
}
