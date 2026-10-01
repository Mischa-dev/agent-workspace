//! Labels on tasks.
//!
//! The shared task commands (`work_item_commands`) already take a label change
//! in the shape below, so the web app, `task_update` and the batch tools share
//! one path. The tables, lookups and writes arrive with migration 0044; until
//! then no caller passes a label change.

use uuid::Uuid;

/// The most active labels one task can carry.
pub const MAX_LABELS_PER_TASK: usize = 20;

/// Labels to put on a task and take off it, as IDs already resolved among the
/// task's project's active labels in the same transaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct LabelDelta<'a> {
    pub add: &'a [Uuid],
    pub remove: &'a [Uuid],
}

impl LabelDelta<'_> {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}
