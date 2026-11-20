//! K1.25: who a call acts for, kept for the length of one call so every
//! write inside it says so: the credential it came through, the account it
//! was made for (a run's requester) and the run. Set at the edge (the MCP
//! call, the operation runner); `events::set_context` reads it when the
//! write path didn't say. Task-local, never shared between calls.

use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Acting {
    pub credential: Option<Uuid>,
    pub requested_by: Option<Uuid>,
    pub run: Option<Uuid>,
}

tokio::task_local! {
    static ACTING: Acting;
}

/// Run `call` acting as `acting` says.
pub async fn within<F: std::future::Future>(acting: Acting, call: F) -> F::Output {
    ACTING.scope(acting, call).await
}

/// What the current call acts as; empty outside one.
pub fn current() -> Acting {
    ACTING.try_with(|acting| *acting).unwrap_or_default()
}

impl Acting {
    pub fn of(caller: &super::Caller) -> Self {
        Self {
            credential: caller.credential_id,
            requested_by: caller.requested_by,
            run: caller.run_id,
        }
    }
}
