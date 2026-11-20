//! K1.30: the identity provider seam. Signing in through an outside
//! identity provider is one implementation of `IdentityProvider`, present
//! only when the server is configured for it; with none configured, a
//! request makes no provider query at all. WorkOS is today's one provider:
//! its sign-in start, callback and agent token checks stay in
//! `auth::workos`, and this seam holds the check every browser request
//! makes.

use uuid::Uuid;

use crate::{AppState, error::AppError};

/// An outside identity provider people sign in through.
pub trait IdentityProvider {
    /// A stable key for logs and settings, such as `workos`.
    fn key(&self) -> &'static str;

    /// Whether the browser session `session_id` is still good with the
    /// provider. A session the provider didn't start is good.
    fn session_valid(
        &self,
        state: &AppState,
        session_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;
}

/// WorkOS, when configured.
pub struct WorkOs;

impl IdentityProvider for WorkOs {
    fn key(&self) -> &'static str {
        "workos"
    }

    async fn session_valid(&self, state: &AppState, session_id: Uuid) -> Result<bool, AppError> {
        super::workos::session_valid(state, session_id).await
    }
}

/// The provider this server is configured for, if any.
pub fn configured(state: &AppState) -> Option<WorkOs> {
    state.config.workos.is_some().then_some(WorkOs)
}
