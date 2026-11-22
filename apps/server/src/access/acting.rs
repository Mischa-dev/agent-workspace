//! K1.25: who a call acts for, kept for the length of one call so every
//! write inside it says so: the credential it came through, the account it
//! was made for (a run's requester), the run, and the operation's request
//! id. Set at the edge (the MCP call, the operation runner);
//! `events::set_context` reads it when the write path didn't say, so a
//! command that opens its own transaction inside an operation still names
//! the call's request id on its events. Task-local, never shared between
//! calls.

use std::sync::Arc;

use uuid::Uuid;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Acting {
    pub credential: Option<Uuid>,
    pub requested_by: Option<Uuid>,
    pub run: Option<Uuid>,
    /// The operation's request id (`Idempotency-Key`, `clientRequestId`).
    pub request_id: Option<Arc<str>>,
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
    ACTING.try_with(Acting::clone).unwrap_or_default()
}

impl Acting {
    pub fn of(caller: &super::Caller) -> Self {
        Self {
            credential: caller.credential_id,
            requested_by: caller.requested_by,
            run: caller.run_id,
            request_id: None,
        }
    }

    /// The same, for the operation's request id.
    pub fn for_request(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id.map(Arc::from);
        self
    }
}
