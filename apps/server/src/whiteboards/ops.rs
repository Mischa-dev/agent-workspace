//! Whiteboards as operations (K2.12). Drawing on a board
//! (`whiteboard_update_elements`, alias `whiteboard_edit`) runs in the
//! operation's own transaction over `access` and `edit::apply`, so the web's
//! operations route and the agent tool write the same audit row. Creating,
//! changing, copying and linking a board run the commands the hand-written
//! tools ran (`mcp::whiteboard_commands`). Finding, reading and drawing a
//! board as an image stay tools until K2.22.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    access,
    edit::{self, EditOutcome},
    scene::Operation as SceneOperation,
};
use crate::{
    mcp::whiteboard_commands::{
        self, BoardUpdate, LinkArgument, NewBoard, WhiteboardChangeValue, WhiteboardLinkValue,
    },
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<WhiteboardCreate>()
        .add::<WhiteboardUpdateElements>()
        .add::<WhiteboardUpdate>()
        .add::<WhiteboardDuplicate>()
        .add::<WhiteboardLink>();
}

// ---------------------------------------------------------------------------
// whiteboard_update_elements (alias whiteboard_edit)

pub struct WhiteboardUpdateElements;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardUpdateElementsInput {
    /// The board: its id or a link to it.
    pub whiteboard_id: IdRef,
    /// Applied in order, all or nothing. Each is {op, ...}:
    /// add {type, id?, x?, y?, width?, height?, text?, styles…} draws a
    /// shape (rectangle, ellipse, diamond, frame), text, line or arrow;
    /// add {type: "image", dataBase64, mimeType?, x?, y?, width?, height?}
    /// puts an image on the board (or fileId, one already on it);
    /// connect {from, to, text?, id?} draws an arrow bound to both;
    /// update {id, text?, x?, y?, width?, height?, styles…};
    /// delete {ids} (labels and bound arrows go with their shape);
    /// move {ids, dx, dy}; align {ids, edge}; distribute {ids, axis, gap?};
    /// layout {ids?, direction?, gap?}; group/ungroup {ids};
    /// order {ids, position}. Arrows follow the shapes they connect.
    pub operations: Vec<SceneOperation>,
    /// The sceneSeq you last read. When given, an operation naming an
    /// element someone else changed since is refused instead of applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_scene_seq: Option<i64>,
    /// Needed to remove more than 25 elements at once. The board is saved
    /// first, so whiteboard_update restoreRevision can undo it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_delete: Option<bool>,
}

impl Targeted for WhiteboardUpdateElementsInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

impl Operation for WhiteboardUpdateElements {
    const NAME: &'static str = "whiteboard_update_elements";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    // Images ride along as base64 (up to 4 MiB each).
    const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;
    const TITLE: &'static str = "Draw on whiteboard";
    const DESCRIPTION: &'static str = "Draw on a whiteboard with operations: add shapes, text, \
        arrows and images, connect shapes, update, move, align, distribute, lay out, group and \
        order them, or delete them. The server turns them into elements and merges them like a \
        browser's change, so open boards update live. Pass the sceneSeq you read as \
        expectedSceneSeq to be told when someone else changed what you touch.";
    const EMITS: &'static [&'static str] = &["whiteboard.updated"];

    type Input = WhiteboardUpdateElementsInput;
    type Output = EditOutcome;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardUpdateElementsInput,
    ) -> Result<EditOutcome, OpError> {
        let id = input.whiteboard_id.id();
        let caller = whiteboard_commands::caller_of(call.ctx);
        let tx = call.tx().await?;
        let board = access(tx, &caller, id, true)
            .await
            .map_err(|error| whiteboard_commands::edit_error(edit::EditError::App(error)))?;
        if board.deleted {
            return Err(OpError::Conflict {
                reason: ConflictReason::State { code: "deleted" },
                message: "That whiteboard is in the trash. Restore it with whiteboard_update \
                          deleted: false first."
                    .into(),
                current: None,
            });
        }
        let operations =
            whiteboard_commands::store_images(tx, &caller, id, &input.operations).await?;
        let outcome = edit::apply(
            tx,
            &caller,
            id,
            &operations,
            input.expected_scene_seq,
            input.confirm_delete.unwrap_or(false),
        )
        .await
        .map_err(whiteboard_commands::edit_error)?;
        call.audit_target(
            "whiteboard",
            id,
            json!({ "changed": outcome.changed.len(), "sceneSeq": outcome.scene_seq }),
        );
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// whiteboard_create

pub struct WhiteboardCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardCreateInput {
    pub title: String,
    /// Put the board in this space: only its members see it. Left out, the
    /// board is for everyone in the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<IdRef>,
    /// The workspace the board is for, when it isn't in a space. Left out,
    /// the workspace this call is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<IdRef>,
    /// Link the new board to a task, space or page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_to: Option<LinkArgument>,
    /// Draw something straight away: the same operations as
    /// whiteboard_update_elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<SceneOperation>>,
}

impl Targeted for WhiteboardCreateInput {
    fn target(&self) -> Target {
        match (self.project_id, self.workspace_id) {
            (Some(space), _) => Target::Space(space.id()),
            (None, workspace) => Target::Workspace(workspace.map(IdRef::id)),
        }
    }
}

impl Operation for WhiteboardCreate {
    const NAME: &'static str = "whiteboard_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;
    const TITLE: &'static str = "Create whiteboard";
    const DESCRIPTION: &'static str = "Create a whiteboard in a space (only its members see it) \
        or, without projectId, for the whole workspace. Optionally link it to a task, space or \
        page, and draw on it straight away with the operations whiteboard_update_elements \
        takes.";
    const EMITS: &'static [&'static str] = &["whiteboard.created"];

    type Input = WhiteboardCreateInput;
    type Output = WhiteboardChangeValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardCreateInput,
    ) -> Result<WhiteboardChangeValue, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let created = whiteboard_commands::create(
            call.state,
            call.ctx,
            NewBoard {
                title: input.title,
                project_id: input.project_id.map(IdRef::id),
                workspace_id: input.workspace_id.map(IdRef::id),
                link_to: input.link_to,
                operations: input.operations,
            },
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        // The envelope says it was a replay; the value is the first answer.
        let mut created = created;
        call.mark_replayed(std::mem::take(&mut created.idempotent_replay));
        call.audit_target("whiteboard", created.whiteboard.whiteboard.id, json!({}));
        // whiteboards::create writes its own whiteboard.created row.
        call.audited_by_command();
        Ok(created)
    }
}

// ---------------------------------------------------------------------------
// whiteboard_update

pub struct WhiteboardUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardUpdateInput {
    /// The board: its id or a link to it.
    pub whiteboard_id: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Move the board into this space (only its members see it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<IdRef>,
    /// Make the board visible to the whole workspace instead of one space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_wide: Option<bool>,
    /// Move the board to the trash (true) or bring it back (false). Its
    /// drawing and links are kept either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    /// Put the drawing back the way this saved revision had it. The
    /// current drawing is saved first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_revision: Option<i32>,
    /// The board version you last read; refuses the change if it moved on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for WhiteboardUpdateInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

impl Operation for WhiteboardUpdate {
    const NAME: &'static str = "whiteboard_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update whiteboard";
    const DESCRIPTION: &'static str = "Rename a whiteboard, move it into a space or make it \
        workspace-wide, put it in the trash or bring it back (deleted), or restore a saved \
        revision of its drawing (the current one is saved first).";
    const EMITS: &'static [&'static str] = &["whiteboard.updated"];

    type Input = WhiteboardUpdateInput;
    type Output = WhiteboardChangeValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardUpdateInput,
    ) -> Result<WhiteboardChangeValue, OpError> {
        let id = input.whiteboard_id.id();
        let changed = whiteboard_commands::update(
            call.state,
            call.ctx,
            id,
            BoardUpdate {
                title: input.title,
                project_id: input.project_id.map(IdRef::id),
                workspace_wide: input.workspace_wide,
                deleted: input.deleted,
                restore_revision: input.restore_revision,
                expected_version: input.expected_version,
            },
        )
        .await?;
        // The change wrote its own audit rows.
        call.audited_by_command();
        Ok(changed)
    }
}

// ---------------------------------------------------------------------------
// whiteboard_duplicate

pub struct WhiteboardDuplicate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardDuplicateInput {
    /// The board to copy: its id or a link to it.
    pub whiteboard_id: IdRef,
    /// The copy's title. Default: "Copy of …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Targeted for WhiteboardDuplicateInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

impl Operation for WhiteboardDuplicate {
    const NAME: &'static str = "whiteboard_duplicate";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Copy whiteboard";
    const DESCRIPTION: &'static str = "Copy a whiteboard, with its drawing and images, into the \
        same place. The copy starts with no links and no history.";
    const EMITS: &'static [&'static str] = &["whiteboard.created"];

    type Input = WhiteboardDuplicateInput;
    type Output = WhiteboardChangeValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardDuplicateInput,
    ) -> Result<WhiteboardChangeValue, OpError> {
        let keyed = call.ctx.request_id.is_some();
        let copy = whiteboard_commands::duplicate(
            call.state,
            call.ctx,
            input.whiteboard_id.id(),
            input.title,
        )
        .await?;
        if keyed {
            call.key_honoured();
        }
        let mut copy = copy;
        call.mark_replayed(std::mem::take(&mut copy.idempotent_replay));
        call.audit_target("whiteboard", copy.whiteboard.whiteboard.id, json!({}));
        Ok(copy)
    }
}

// ---------------------------------------------------------------------------
// whiteboard_link

pub struct WhiteboardLink;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardLinkInput {
    /// The board: its id or a link to it.
    pub whiteboard_id: IdRef,
    /// task, project, or page.
    pub target_kind: String,
    /// The task, space or page: its id or a link to it.
    pub target_id: IdRef,
    /// Take the link off instead. The board itself stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
}

impl Targeted for WhiteboardLinkInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

impl Operation for WhiteboardLink {
    const NAME: &'static str = "whiteboard_link";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Link whiteboard";
    const DESCRIPTION: &'static str = "Link a whiteboard to a task, space or page, so it shows \
        there, or take the link off with remove: true. Linking twice changes nothing.";
    const EMITS: &'static [&'static str] = &["whiteboard.updated"];

    type Input = WhiteboardLinkInput;
    type Output = WhiteboardLinkValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardLinkInput,
    ) -> Result<WhiteboardLinkValue, OpError> {
        let id = input.whiteboard_id.id();
        let linked = whiteboard_commands::link(
            call.state,
            call.ctx,
            id,
            input.target_kind,
            input.target_id.id(),
            input.remove,
        )
        .await?;
        call.audit_target(
            "whiteboard",
            id,
            json!({ "linked": linked.linked, "changed": linked.changed }),
        );
        // A change writes its own whiteboard.linked or .unlinked row; an
        // unchanged call writes this one.
        if linked.changed {
            call.audited_by_command();
        }
        Ok(linked)
    }
}
