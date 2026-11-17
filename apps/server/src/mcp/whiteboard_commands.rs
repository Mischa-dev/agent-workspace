//! The whiteboard commands as operations run them (K2.12): the caller the
//! dispatcher checked, as whiteboards read it, and the commands the
//! hand-written tools ran for creating, changing, copying and linking a
//! board. Drawing (`whiteboard_update_elements`) runs in the operation's
//! own transaction over `whiteboards::edit::apply`.

use std::borrow::Cow;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub use super::whiteboard_tools::{LinkArgument, WhiteboardChangeValue, WhiteboardLinkValue};
use super::{
    common::ToolResult,
    messaging::principal_of,
    whiteboard_tools::{
        self, WhiteboardCreateArguments, WhiteboardDuplicateArguments, WhiteboardLinkArguments,
        WhiteboardUpdateArguments,
    },
};
use crate::{
    AppState,
    ops::{ConflictReason, DenyReason, Edge, OpContext, OpError},
    whiteboards::{Caller, edit::EditError, scene::Operation},
};

/// The caller as whiteboards read it, with the edge it came through for
/// the board's revisions and audit rows.
pub(crate) fn caller_of(ctx: &OpContext) -> Caller {
    let principal = principal_of(ctx);
    let mut caller = whiteboard_tools::caller(&principal);
    caller.channel = match ctx.edge {
        Edge::Web { session_id } => {
            caller.session_id = Some(session_id);
            "browser"
        }
        Edge::Mcp { .. } => "mcp",
        Edge::PublicApi { .. } => "api",
        Edge::Automation { .. } => "automation",
        Edge::Job { .. } => "job",
        Edge::Proposal { .. } => "proposal",
    };
    caller
}

/// An edit's refusal as an operation's.
pub(crate) fn edit_error(error: EditError) -> OpError {
    match error {
        EditError::Invalid(message) => OpError::Invalid {
            field: Cow::Borrowed("operations"),
            message: Cow::Owned(message),
        },
        EditError::Conflict(message) => OpError::Conflict {
            reason: ConflictReason::State { code: "scene" },
            message: Cow::Owned(message),
            current: None,
        },
        EditError::App(crate::error::AppError::Forbidden) => OpError::Denied {
            reason: DenyReason::Rule,
            message: Cow::Borrowed("You can look at this whiteboard but not change it."),
        },
        EditError::App(error) => error.into(),
    }
}

/// The images an edit carries, stored on the board as the web stores a
/// dropped image, with the operations pointing at them.
pub(crate) async fn store_images(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    operations: &[Operation],
) -> Result<Vec<Operation>, OpError> {
    whiteboard_tools::store_images(transaction, caller, whiteboard_id, operations)
        .await
        .map_err(edit_error)
}

fn answer<T>(result: ToolResult<T>) -> Result<T, OpError> {
    match result {
        ToolResult::Ok { value } => Ok(value),
        ToolResult::NotFound { .. } => Err(OpError::not_found("That whiteboard")),
        ToolResult::InvalidRequest { field, message } => Err(OpError::Invalid {
            field: Cow::Owned(field),
            message: Cow::Owned(message),
        }),
        ToolResult::PermissionDenied {
            message,
            retry_category,
        } => Err(OpError::Denied {
            reason: if retry_category.as_deref() == Some("module") {
                DenyReason::ModuleOff {
                    module: "whiteboards".to_owned(),
                }
            } else {
                DenyReason::Rule
            },
            message: Cow::Owned(message),
        }),
        ToolResult::Conflict { message } => Err(OpError::Conflict {
            reason: if message.contains("clientRequestId") {
                ConflictReason::RequestIdReused
            } else {
                ConflictReason::State { code: "state" }
            },
            message: Cow::Owned(message),
            current: None,
        }),
        ToolResult::TemporarilyUnavailable { .. } => Err(OpError::Unavailable),
    }
}

pub(crate) struct NewBoard {
    pub title: String,
    pub project_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub link_to: Option<LinkArgument>,
    pub operations: Option<Vec<Operation>>,
    pub from_template_id: Option<String>,
}

pub(crate) async fn create(
    state: &AppState,
    ctx: &OpContext,
    board: NewBoard,
) -> Result<WhiteboardChangeValue, OpError> {
    let principal = principal_of(ctx);
    // Without a space, the board is for the workspace the call located.
    let workspace_id = match (board.project_id, board.workspace_id) {
        (None, None) if ctx.caller.confined_space.is_none() => Some(ctx.workspace_id),
        (_, workspace) => workspace,
    };
    answer(
        whiteboard_tools::whiteboard_create(
            state,
            &principal,
            WhiteboardCreateArguments {
                title: board.title,
                project_id: board.project_id.map(|id| id.to_string()),
                workspace_id: workspace_id.map(|id| id.to_string()),
                link_to: board.link_to,
                operations: board.operations,
                from_template_id: board.from_template_id,
                client_request_id: ctx.request_id.clone(),
            },
        )
        .await,
    )
}

pub(crate) struct BoardUpdate {
    pub title: Option<String>,
    pub project_id: Option<Uuid>,
    pub workspace_wide: Option<bool>,
    pub deleted: Option<bool>,
    pub restore_revision: Option<i32>,
    pub expected_version: Option<i64>,
    pub is_template: Option<bool>,
}

pub(crate) async fn update(
    state: &AppState,
    ctx: &OpContext,
    whiteboard_id: Uuid,
    change: BoardUpdate,
) -> Result<WhiteboardChangeValue, OpError> {
    let principal = principal_of(ctx);
    answer(
        whiteboard_tools::whiteboard_update(
            state,
            &principal,
            WhiteboardUpdateArguments {
                whiteboard_id: whiteboard_id.to_string(),
                title: change.title,
                project_id: change.project_id.map(|id| id.to_string()),
                workspace_wide: change.workspace_wide,
                deleted: change.deleted,
                restore_revision: change.restore_revision,
                expected_version: change.expected_version,
                is_template: change.is_template,
            },
        )
        .await,
    )
}

pub(crate) async fn duplicate(
    state: &AppState,
    ctx: &OpContext,
    whiteboard_id: Uuid,
    title: Option<String>,
) -> Result<WhiteboardChangeValue, OpError> {
    let principal = principal_of(ctx);
    answer(
        whiteboard_tools::whiteboard_duplicate(
            state,
            &principal,
            WhiteboardDuplicateArguments {
                whiteboard_id: whiteboard_id.to_string(),
                title,
                client_request_id: ctx.request_id.clone(),
            },
        )
        .await,
    )
}

pub(crate) async fn link(
    state: &AppState,
    ctx: &OpContext,
    whiteboard_id: Uuid,
    target_kind: String,
    target_id: Uuid,
    remove: Option<bool>,
) -> Result<WhiteboardLinkValue, OpError> {
    let principal = principal_of(ctx);
    answer(
        whiteboard_tools::whiteboard_link(
            state,
            &principal,
            WhiteboardLinkArguments {
                whiteboard_id: whiteboard_id.to_string(),
                target_kind,
                target_id: target_id.to_string(),
                remove,
            },
        )
        .await,
    )
}
