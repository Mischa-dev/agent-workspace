//! A run finished: whoever cares hears it here (A1.10). The run's writer
//! (the built-in host's `transition`, and from A1.14 every run) only
//! announces it; the features that react are listed once, below, instead
//! of being called from the state machine. Each runs in the writer's
//! transaction, so nothing is lost if the process stops right after.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// A run reached a terminal state.
pub async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    // Its agent session may be resumed by the next run in the thread.
    crate::agent_sessions::idle_run_session(&mut **transaction, run_id).await?;
    // Its account slot may let a queued launch start.
    crate::launch::kick(&mut **transaction).await?;
    Ok(())
}
