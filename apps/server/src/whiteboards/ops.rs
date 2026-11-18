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
    proposals::{self, Proposed},
    scene::Operation as SceneOperation,
};
use crate::{
    access::WriteMode,
    mcp::whiteboard_commands::{
        self, BoardUpdate, LinkArgument, NewBoard, WhiteboardChangeValue, WhiteboardLinkValue,
    },
    ops::{
        ConflictReason, DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm,
        Registry, Target, Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<WhiteboardCreate>()
        .add::<WhiteboardUpdateElements>()
        .add::<WhiteboardPropose>()
        .add::<WhiteboardUpdate>()
        .add::<WhiteboardDuplicate>()
        .add::<WhiteboardLink>()
        .add::<WhiteboardTemplateList>()
        .add::<super::to_tasks::WhiteboardToTasks>();
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
    // Suggest is the floor: `run` asks `access::write_mode` whether the
    // drawing lands (edit, or edit-own on your own board) or waits as a
    // proposal (suggest only).
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsSuggest);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    // Images ride along as base64 (up to 4 MiB each).
    const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;
    const TITLE: &'static str = "Draw on whiteboard";
    const DESCRIPTION: &'static str = "Draw on a whiteboard with operations: add shapes, text, \
        arrows and images, connect shapes, update, move, align, distribute, lay out, group and \
        order them, or delete them. The server turns them into elements and merges them like a \
        browser's change, so open boards update live. Pass the sceneSeq you read as \
        expectedSceneSeq to be told when someone else changed what you touch. If you only \
        suggest here, it becomes a proposal (proposalId).";
    const EMITS: &'static [&'static str] = &["whiteboard.updated", "whiteboard.proposals_changed"];

    type Input = WhiteboardUpdateElementsInput;
    type Output = EditOutcome;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardUpdateElementsInput,
    ) -> Result<EditOutcome, OpError> {
        let id = input.whiteboard_id.id();
        let caller = whiteboard_commands::caller_of(call.ctx);
        let access_caller = call.ctx.caller.clone();
        let state = call.state;
        let tx = call.tx().await?;
        let (board, mode) = board_for_writing(tx, &caller, &access_caller, id).await?;
        if mode == WriteMode::Propose {
            let planned = edit::plan(tx, id, &input.operations, input.expected_scene_seq)
                .await
                .map_err(whiteboard_commands::edit_error)?;
            let changed = edit::parsed(planned.changed).map_err(whiteboard_commands::edit_error)?;
            let (proposal_id, _) = propose(
                tx,
                &caller,
                &board,
                planned.scene_seq,
                changed,
                Self::NAME,
                None,
            )
            .await?;
            if let Some(proposal) = proposal_id {
                call.audit_target("proposal", proposal, json!({ "whiteboard": id }));
            }
            return Ok(EditOutcome {
                scene_seq: planned.scene_seq,
                results: planned.results,
                changed: Vec::new(),
                saved_revision: None,
                proposal_id: proposal_id.map(|proposal| proposal.to_string()),
            });
        }
        let operations =
            whiteboard_commands::store_images(state, tx, &caller, id, &input.operations).await?;
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

/// The board, refused when it's in the trash, and how the caller may write
/// on it: directly (edit, or edit-own on their own board) or as a proposal
/// (suggest only). Anyone else is refused.
async fn board_for_writing(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    caller: &super::Caller,
    access_caller: &crate::access::Caller,
    id: uuid::Uuid,
) -> Result<(super::BoardAccess, WriteMode), OpError> {
    let board = access(tx, caller, id, false)
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
    if board.can_edit {
        return Ok((board, WriteMode::Direct));
    }
    let own = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT created_by_actor_id FROM whiteboards WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?
        == caller.actor_id;
    let mode = crate::access::write_mode(
        tx,
        access_caller,
        "whiteboard",
        board.project_id,
        own,
        Some("whiteboards"),
    )
    .await?
    .ok()
    .map(|grant| grant.mode);
    match mode {
        Some(mode) => Ok((board, mode)),
        None => Err(whiteboard_commands::edit_error(edit::EditError::App(
            crate::error::AppError::Forbidden,
        ))),
    }
}

/// Keeps elements as a proposal: its id (none when nothing would change)
/// and how many elements it sets.
async fn propose(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    caller: &super::Caller,
    board: &super::BoardAccess,
    scene_seq: i64,
    elements: Vec<super::elements::Element>,
    operation: &str,
    note: Option<&str>,
) -> Result<(Option<uuid::Uuid>, usize), OpError> {
    match proposals::propose(tx, caller, board, scene_seq, elements, operation, note).await? {
        Proposed::Made { id, elements } => Ok((Some(id), elements.len())),
        Proposed::Unchanged => Ok((None, 0)),
        Proposed::Refused(message) => Err(OpError::Denied {
            reason: DenyReason::Rule,
            message: message.into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_propose

/// A browser's suggest-mode drawing, sent as one proposal.
pub struct WhiteboardPropose;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardProposeInput {
    pub whiteboard_id: IdRef,
    /// Changed elements as Excalidraw writes them.
    #[schemars(with = "Vec<serde_json::Value>")]
    pub elements: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Targeted for WhiteboardProposeInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardProposeOutput {
    /// Absent when nothing would change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    pub elements: usize,
}

impl Operation for WhiteboardPropose {
    const NAME: &'static str = "whiteboard_propose";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = Some(Perm::WhiteboardsSuggest);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;
    const TITLE: &'static str = "Propose a whiteboard change";
    const DESCRIPTION: &'static str = "Propose changed elements for someone who edits the \
        board to decide. whiteboard_update_elements proposes by itself when you only suggest.";
    const EMITS: &'static [&'static str] = &["whiteboard.proposals_changed"];

    type Input = WhiteboardProposeInput;
    type Output = WhiteboardProposeOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardProposeInput,
    ) -> Result<WhiteboardProposeOutput, OpError> {
        let id = input.whiteboard_id.id();
        if input.elements.is_empty() {
            return Err(OpError::invalid("elements", "Send at least one element."));
        }
        if input.elements.len() > proposals::MAX_PROPOSED_ELEMENTS {
            return Err(OpError::invalid(
                "elements",
                "Propose up to 500 elements at a time.",
            ));
        }
        let note = input
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty());
        if note.is_some_and(|note| note.chars().count() > 500) {
            return Err(OpError::invalid("note", "Keep the note to 500 characters."));
        }
        let elements = input
            .elements
            .into_iter()
            .map(super::elements::parse)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| OpError::Invalid {
                field: "elements".into(),
                message: message.into(),
            })?;
        let caller = whiteboard_commands::caller_of(call.ctx);
        let access_caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let (board, _) = board_for_writing(tx, &caller, &access_caller, id).await?;
        let scene_seq = super::store::lock(tx, id).await?;
        let (proposal_id, count) =
            propose(tx, &caller, &board, scene_seq, elements, Self::NAME, note).await?;
        if let Some(proposal) = proposal_id {
            call.audit_target(
                "proposal",
                proposal,
                json!({ "whiteboard": id, "elements": count }),
            );
        }
        Ok(WhiteboardProposeOutput {
            proposal_id: proposal_id.map(|proposal| proposal.to_string()),
            elements: count,
        })
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
    /// Start from a template: a template board's ID, or a built-in one
    /// (retrospective, user-journey, flowchart, two-by-two; see
    /// whiteboard_template_list). Its drawing is copied in before any
    /// operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_template_id: Option<String>,
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
                from_template_id: input.from_template_id,
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
    /// Make the board a template (true), offered when anyone starts a board
    /// from one, or an ordinary board again (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_template: Option<bool>,
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
                is_template: input.is_template,
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

// ---------------------------------------------------------------------------
// whiteboard_template_list

pub struct WhiteboardTemplateList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardTemplateListInput {
    /// A space (a project) in the workspace: its id, link or key. Leave it
    /// out to ask about the workspace you're in.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for WhiteboardTemplateListInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardTemplateListOutput {
    /// The built-in templates, then boards marked as templates in the
    /// workspace that you can see.
    pub templates: Vec<super::templates::TemplateSummary>,
}

impl Operation for WhiteboardTemplateList {
    const NAME: &'static str = "whiteboard_template_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List whiteboard templates";
    const DESCRIPTION: &'static str = "List what a new whiteboard in a workspace can start \
        from: the built-in templates and boards marked as templates. Pass a template's id to \
        whiteboard_create as fromTemplateId.";

    type Input = WhiteboardTemplateListInput;
    type Output = WhiteboardTemplateListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WhiteboardTemplateListInput,
    ) -> Result<WhiteboardTemplateListOutput, OpError> {
        let caller = super::Caller::from(call.ctx);
        let workspace_id = call.ctx.workspace_id;
        let templates = super::templates::list(call.tx().await?, &caller, workspace_id).await?;
        Ok(WhiteboardTemplateListOutput { templates })
    }
}
