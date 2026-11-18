//! What the built-in host's worktree and account-home code fails with.
//! The server turns each into its own error of the same name.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The thing is in a state that refuses the change (what, why).
    #[error("conflict: {0}: {1}")]
    Conflict(&'static str, &'static str),
    /// The input isn't acceptable.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// Something on this machine failed.
    #[error("internal: {0}")]
    Internal(&'static str),
    /// Too many Git processes are running; try again shortly.
    #[error("busy; retry in {retry_after_seconds}s")]
    Busy { retry_after_seconds: u64 },
}

impl RuntimeError {
    pub fn rate_limited(retry_after_seconds: u64) -> Self {
        Self::Busy {
            retry_after_seconds,
        }
    }
}
