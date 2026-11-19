//! The harness registry lives in the built-in host's crate; the server
//! keeps the cluster its lease comes from beside it.

use std::{ops::Deref, sync::Arc};

#[cfg(test)]
pub use agent_workspace_builtin_host::harness::registry::ShutdownSignal;
pub use agent_workspace_builtin_host::harness::registry::{CustomAdapter, HostLease};

use crate::{config::AppConfig, provider::adapter::HarnessError};

/// The harness adapters the server can run agents on, and the cluster whose
/// `builtin-host` lease decides where their turns run.
#[derive(Clone)]
pub struct SupervisorRegistry {
    inner: agent_workspace_builtin_host::harness::registry::SupervisorRegistry,
    cluster: Arc<std::sync::OnceLock<crate::cluster::Cluster>>,
}

impl Deref for SupervisorRegistry {
    type Target = agent_workspace_builtin_host::harness::registry::SupervisorRegistry;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SupervisorRegistry {
    pub fn new(config: &AppConfig) -> Result<Self, HarnessError> {
        Ok(Self {
            inner: agent_workspace_builtin_host::harness::registry::SupervisorRegistry::new(
                &config.harness(),
            )?,
            cluster: Arc::new(std::sync::OnceLock::new()),
        })
    }

    /// The instance whose lease decides where local turns run (K5.3). Set
    /// once when the server starts.
    pub fn set_cluster(&self, cluster: crate::cluster::Cluster) {
        let _ = self.cluster.set(cluster.clone());
        self.inner.set_lease(Arc::new(ClusterLease(cluster)));
    }

    pub fn cluster(&self) -> Option<&crate::cluster::Cluster> {
        self.cluster.get()
    }
}

/// The built-in host's lease as the cluster holds it.
struct ClusterLease(crate::cluster::Cluster);

impl HostLease for ClusterLease {
    fn lost(&self) -> tokio_util::sync::CancellationToken {
        match self.0.held_lease(&crate::cluster::LeaseName::BuiltinHost) {
            Some(lease) => lease.lost.clone(),
            None => {
                let lost = tokio_util::sync::CancellationToken::new();
                lost.cancel();
                lost
            }
        }
    }

    fn released(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.0.released())
    }
}
