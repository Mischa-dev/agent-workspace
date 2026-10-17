//! Agents' edits: operations (see `scene`) applied to the stored scene in
//! one transaction, written through the same merge as browsers' changes.

use std::collections::HashSet;

use rmcp::schemars;
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    Caller, elements,
    scene::{self, Operation, OperationResult, Scene},
    store,
};
use crate::error::AppError;

/// Most operations one call may run.
pub const MAX_OPERATIONS: usize = 500;
/// Removing more live elements than this in one call needs `confirmDelete`.
pub const CONFIRM_DELETE_OVER: usize = 25;

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    /// The board's change counter after the edit. Pass it as
    /// expectedSceneSeq next time to be told when someone else changed what
    /// you're about to touch.
    pub scene_seq: i64,
    pub results: Vec<OperationResult>,
    /// Elements created or changed, including labels and re-routed arrows.
    pub changed: Vec<String>,
    /// The revision saved before this edit, if one was: restore it to undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_revision: Option<i32>,
}

/// Why an edit was refused, in words for the agent.
#[derive(Debug)]
pub enum EditError {
    Invalid(String),
    Conflict(String),
    App(AppError),
}

impl From<AppError> for EditError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for EditError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

/// Runs operations against a board the caller may edit. With
/// `expected_scene_seq`, an operation that touches an element someone else
/// changed after that point is refused instead of applied on top.
pub async fn apply(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    operations: &[Operation],
    expected_scene_seq: Option<i64>,
    confirm_delete: bool,
) -> Result<EditOutcome, EditError> {
    if operations.is_empty() {
        return Err(EditError::Invalid(
            "Pass at least one operation.".to_owned(),
        ));
    }
    if operations.len() > MAX_OPERATIONS {
        return Err(EditError::Invalid(format!(
            "Pass at most {MAX_OPERATIONS} operations at once."
        )));
    }
    let scene_seq = store::lock(transaction, whiteboard_id).await?;
    let rows = store::load_rows(transaction, whiteboard_id).await?;
    let changed_since = match expected_scene_seq {
        Some(expected) if expected < scene_seq => sqlx::query_scalar::<_, String>(
            "SELECT element_id FROM whiteboard_elements WHERE whiteboard_id = $1 AND seq > $2",
        )
        .bind(whiteboard_id)
        .bind(expected)
        .fetch_all(&mut **transaction)
        .await?
        .into_iter()
        .collect::<HashSet<_>>(),
        _ => HashSet::new(),
    };
    let live_before = rows.iter().filter(|row| !scene::is_deleted(row)).count();
    let mut board = Scene::new(rows, chrono::Utc::now().timestamp_millis());
    let results = board.apply(operations).map_err(EditError::Invalid)?;
    let changed = board.changed();
    let named = operations
        .iter()
        .flat_map(|operation| {
            operation
                .id
                .iter()
                .chain(operation.ids.iter().flatten())
                .chain(operation.from.iter())
                .chain(operation.to.iter())
                .cloned()
        })
        .collect::<HashSet<_>>();
    if let Some(conflict) = named.iter().find(|id| changed_since.contains(*id)) {
        return Err(EditError::Conflict(format!(
            "Someone changed {conflict} after scene {}. Read the board again (whiteboard_read), then retry with expectedSceneSeq {scene_seq}.",
            expected_scene_seq.unwrap_or_default()
        )));
    }
    let removed = changed
        .iter()
        .filter(|element| scene::is_deleted(element))
        .count();
    if removed > CONFIRM_DELETE_OVER && !confirm_delete {
        return Err(EditError::Conflict(format!(
            "This would remove {removed} elements. Pass confirmDelete: true if that's what you mean; the board is saved first so it can be restored."
        )));
    }
    let big_removal = removed >= 10 || (live_before > 0 && removed * 2 >= live_before);
    let saved_revision = store::save_revision(
        transaction,
        caller,
        whiteboard_id,
        big_removal,
        big_removal.then_some("Before an agent removed elements"),
    )
    .await?;
    let ids = changed
        .iter()
        .map(|element| scene::id(element).to_owned())
        .collect::<Vec<_>>();
    let parsed = changed
        .into_iter()
        .map(|element| elements::parse(serde_json::Value::Object(element)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(EditError::Invalid)?;
    let outcome =
        store::write_elements(transaction, caller, whiteboard_id, scene_seq, parsed).await?;
    Ok(EditOutcome {
        scene_seq: outcome.scene_seq,
        results,
        changed: ids,
        saved_revision,
    })
}
