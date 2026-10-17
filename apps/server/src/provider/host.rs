//! What adapters may ask of the rest of the server (W12): an account's
//! stored key, a registered harness's named secrets, and access to a run's
//! GitHub repository. The server sets one host on the registry at startup;
//! adapters never touch the database or the secret store themselves, and
//! every value they get back is held as a `SecretValue` until it is written
//! to the one place that needs it (a child's private file descriptor, or an
//! HTTPS request body).

use std::path::Path;

use uuid::Uuid;

use super::adapter::HarnessFuture;
use crate::secrets::SecretValue;

/// A run's repository on GitHub and a short-lived token that can read and
/// push it.
pub struct RepositoryAccess {
    pub url: String,
    pub token: SecretValue,
}

pub trait HarnessHost: Send + Sync {
    /// The key or token stored for a harness account, if any.
    fn account_secret(&self, account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>>;

    /// Store (or replace) the key or token for a harness account.
    fn store_account_secret<'a>(
        &'a self,
        account_id: Uuid,
        value: &'a SecretValue,
    ) -> HarnessFuture<'a, ()>;

    fn delete_account_secret(&self, account_id: Uuid) -> HarnessFuture<'_, ()>;

    /// The named workspace secrets a registered harness is allowed, read
    /// for the account that runs it. Missing names are left out.
    fn harness_secrets<'a>(
        &'a self,
        account_id: Uuid,
        names: &'a [String],
    ) -> HarnessFuture<'a, Vec<(String, SecretValue)>>;

    /// The GitHub repository a run's worktree belongs to.
    fn repository_access<'a>(&'a self, worktree: &'a Path) -> HarnessFuture<'a, RepositoryAccess>;
}

/// Where adapters find the host once the server has set it.
pub type HostCell = std::sync::Arc<std::sync::OnceLock<std::sync::Arc<dyn HarnessHost>>>;

/// The host, or a refusal when the server never set one (some tests).
pub fn host_of(
    cell: &HostCell,
) -> Result<&std::sync::Arc<dyn HarnessHost>, super::adapter::HarnessError> {
    cell.get().ok_or(super::adapter::HarnessError::Unsupported(
        "stored harness secrets",
    ))
}

/// The store name of a harness account's key.
pub fn account_secret_name(account_id: Uuid) -> String {
    format!("provider_account:{account_id}:key")
}
