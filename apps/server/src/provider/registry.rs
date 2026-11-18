//! The registered harness adapters and the server-wide state they share:
//! shutdown, and sign-in ceremonies (which are the same for every harness).
//! Stored adapter kinds are validated against this registry rather than by
//! database checks (H1), so adding a harness is adding an adapter here.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::sync::Notify;
use uuid::Uuid;

use super::{
    adapter::{AccountHandle, HarnessAdapter, HarnessError, HarnessSession, ProfileHealth},
    claude_cloud::ClaudeCloudAdapter,
    claude_code::ClaudeCodeAdapter,
    codex_app_server::CodexAdapter,
    codex_cloud::CodexCloudAdapter,
    demo::{DEMO_KIND, DemoAdapter, DemoHarness},
    host::{HarnessHost, HostCell},
    manifest::AdapterManifest,
};
use crate::config::AppConfig;

pub use agent_workspace_builtin_host::harness::shutdown::ShutdownSignal;

/// The harness adapters the server can run agents on. The name is kept from
/// when it only supervised Codex processes.
#[derive(Clone)]
pub struct SupervisorRegistry {
    inner: Arc<RegistryInner>,
}

/// A harness someone registered in a workspace (X2).
pub struct CustomAdapter {
    pub organization_id: Uuid,
    pub enabled: bool,
    /// Set for a connected runner: launches go to this agent as a tag.
    pub runner_actor_id: Option<Uuid>,
    pub adapter: Arc<dyn HarnessAdapter>,
}

/// A manifest that stays valid while the registry changes.
#[derive(Clone)]
pub struct ManifestRef(Arc<dyn HarnessAdapter>);

impl std::ops::Deref for ManifestRef {
    type Target = AdapterManifest;

    fn deref(&self) -> &AdapterManifest {
        self.0.manifest()
    }
}

impl std::fmt::Debug for ManifestRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ManifestRef")
            .field(&self.kind)
            .finish()
    }
}

struct RegistryInner {
    adapters: BTreeMap<String, Arc<dyn HarnessAdapter>>,
    custom: std::sync::RwLock<BTreeMap<String, CustomAdapter>>,
    host: HostCell,
    demo: Option<Arc<DemoAdapter>>,
    codex_cloud: Arc<CodexCloudAdapter>,
    /// Development only: every run goes to the demo harness whatever its
    /// connection's kind.
    route_all_to_demo: bool,
    shutdown: Arc<ShutdownSignal>,
    /// This instance among the others (K5.3): local turns run only on the
    /// holder of the built-in host's lease. Unset in tests that build a
    /// state without a server.
    cluster: std::sync::OnceLock<crate::cluster::Cluster>,
    /// Turns this process is driving, so losing the built-in host's lease
    /// can wait for them to stop before anyone else recovers them.
    active_turns: AtomicUsize,
    turns_idle: Notify,
}

/// How long losing the built-in host's lease waits for this process's turns
/// to stop.
const LOCAL_TURNS_STOP_WAIT: Duration = Duration::from_secs(10);

/// One turn this process drives ([`SupervisorRegistry::turn_started`]);
/// dropping it says the turn has stopped.
pub struct ActiveTurn {
    inner: Arc<RegistryInner>,
}

impl Drop for ActiveTurn {
    fn drop(&mut self) {
        if self.inner.active_turns.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.turns_idle.notify_waiters();
        }
    }
}

const CONTROL_SLOT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const CONTROL_SLOT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

impl SupervisorRegistry {
    pub fn new(config: &AppConfig) -> Result<Self, HarnessError> {
        let config = &config.harness();
        let shutdown = Arc::new(ShutdownSignal::default());
        let host = HostCell::default();
        let mut adapters: BTreeMap<String, Arc<dyn HarnessAdapter>> = BTreeMap::new();
        let codex: Arc<dyn HarnessAdapter> =
            Arc::new(CodexAdapter::new(config, Arc::clone(&shutdown))?);
        adapters.insert(codex.manifest().kind.clone(), Arc::clone(&codex));
        let codex_cloud = Arc::new(CodexCloudAdapter::new(
            config,
            Arc::clone(&shutdown),
            codex,
        )?);
        adapters.insert(
            codex_cloud.manifest().kind.clone(),
            Arc::clone(&codex_cloud) as Arc<dyn HarnessAdapter>,
        );
        let claude_code: Arc<dyn HarnessAdapter> = Arc::new(ClaudeCodeAdapter::new(
            config,
            Arc::clone(&shutdown),
            Arc::clone(&host),
        ));
        adapters.insert(claude_code.manifest().kind.clone(), claude_code);
        let claude_cloud: Arc<dyn HarnessAdapter> =
            Arc::new(ClaudeCloudAdapter::new(config, Arc::clone(&host))?);
        adapters.insert(claude_cloud.manifest().kind.clone(), claude_cloud);
        let demo = config.demo_executor.then(|| {
            Arc::new(DemoAdapter::new(
                Arc::clone(&shutdown),
                config.codex_host_concurrency_limit,
            ))
        });
        if let Some(demo) = &demo {
            adapters.insert(
                DEMO_KIND.to_owned(),
                Arc::new(DemoHarness(Arc::clone(demo))),
            );
        }
        Ok(Self {
            inner: Arc::new(RegistryInner {
                adapters,
                custom: std::sync::RwLock::new(BTreeMap::new()),
                host,
                route_all_to_demo: demo.is_some(),
                demo,
                codex_cloud,
                shutdown,
                cluster: std::sync::OnceLock::new(),
                active_turns: AtomicUsize::new(0),
                turns_idle: Notify::new(),
            }),
        })
    }

    /// The adapter that runs connections of `kind`.
    pub fn adapter(&self, kind: &str) -> Result<Arc<dyn HarnessAdapter>, HarnessError> {
        if self.inner.route_all_to_demo
            && let Some(demo) = self.inner.adapters.get(DEMO_KIND)
        {
            return Ok(Arc::clone(demo));
        }
        if let Some(adapter) = self.inner.adapters.get(kind) {
            return Ok(Arc::clone(adapter));
        }
        self.custom_read()
            .get(kind)
            .map(|custom| Arc::clone(&custom.adapter))
            .ok_or(HarnessError::UnknownAdapter)
    }

    /// The manifest people see for connections of `kind` (never rerouted).
    /// Registered harnesses (X2) are found whichever workspace owns them;
    /// listing and creating go through `manifests_in` and `manifest_in`.
    pub fn manifest(&self, kind: &str) -> Option<ManifestRef> {
        if let Some(adapter) = self.inner.adapters.get(kind) {
            return Some(ManifestRef(Arc::clone(adapter)));
        }
        self.custom_read()
            .get(kind)
            .map(|custom| ManifestRef(Arc::clone(&custom.adapter)))
    }

    /// The built-in harnesses and every registered one, for server-wide
    /// questions (which kinds run in the cloud). Never shown to people.
    pub fn manifests(&self) -> Vec<ManifestRef> {
        let mut manifests = self
            .inner
            .adapters
            .values()
            .map(|adapter| ManifestRef(Arc::clone(adapter)))
            .collect::<Vec<_>>();
        manifests.extend(
            self.custom_read()
                .values()
                .map(|custom| ManifestRef(Arc::clone(&custom.adapter))),
        );
        manifests
    }

    /// The harnesses a workspace can use: the built-in ones and its own
    /// registered ones, never another workspace's.
    pub fn manifests_in(&self, organization_id: Uuid) -> Vec<ManifestRef> {
        let mut manifests = self
            .inner
            .adapters
            .values()
            .map(|adapter| ManifestRef(Arc::clone(adapter)))
            .collect::<Vec<_>>();
        manifests.extend(
            self.custom_read()
                .values()
                .filter(|custom| custom.organization_id == organization_id && custom.enabled)
                .map(|custom| ManifestRef(Arc::clone(&custom.adapter))),
        );
        manifests
    }

    /// A harness a workspace may create connections or environments of.
    pub fn manifest_in(&self, organization_id: Uuid, kind: &str) -> Option<ManifestRef> {
        if let Some(adapter) = self.inner.adapters.get(kind) {
            return Some(ManifestRef(Arc::clone(adapter)));
        }
        self.custom_read()
            .get(kind)
            .filter(|custom| custom.organization_id == organization_id && custom.enabled)
            .map(|custom| ManifestRef(Arc::clone(&custom.adapter)))
    }

    /// Replaces the registered harnesses with the ones stored now.
    pub fn replace_custom(&self, harnesses: Vec<CustomAdapter>) {
        let map = harnesses
            .into_iter()
            .map(|custom| (custom.adapter.manifest().kind.clone(), custom))
            .collect();
        *self
            .inner
            .custom
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = map;
    }

    fn custom_read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, CustomAdapter>> {
        self.inner
            .custom
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What adapters need from the rest of the server (secrets, repository
    /// tokens). Set once when the server starts.
    pub fn host(&self) -> Option<&Arc<dyn HarnessHost>> {
        self.inner.host.get()
    }

    pub fn set_host(&self, host: Arc<dyn HarnessHost>) {
        let _ = self.inner.host.set(host);
    }

    /// The instance whose lease decides where local turns run (K5.3). Set
    /// once when the server starts.
    pub fn set_cluster(&self, cluster: crate::cluster::Cluster) {
        let _ = self.inner.cluster.set(cluster);
    }

    pub fn cluster(&self) -> Option<&crate::cluster::Cluster> {
        self.inner.cluster.get()
    }

    /// The host cell registered adapters share with the built-in ones.
    pub(crate) fn host_cell(&self) -> HostCell {
        Arc::clone(&self.inner.host)
    }

    /// The agent a connected-runner harness hands its launches to.
    pub fn runner_for(&self, kind: &str) -> Option<Uuid> {
        self.custom_read()
            .get(kind)
            .and_then(|custom| custom.runner_actor_id)
    }

    /// The Codex Cloud adapter, for checking environments.
    pub(crate) fn codex_cloud(&self) -> &Arc<CodexCloudAdapter> {
        &self.inner.codex_cloud
    }

    /// The kinds of harness whose runs execute somewhere other than this
    /// host, so the host-wide run limit doesn't count them.
    pub fn cloud_kinds(&self) -> Vec<String> {
        self.manifests()
            .into_iter()
            .filter(|manifest| manifest.runs_on == super::manifest::RunLocation::Cloud)
            .map(|manifest| manifest.kind.clone())
            .collect()
    }

    /// The demo harness, when this server runs it.
    pub fn demo(&self) -> Option<&Arc<DemoAdapter>> {
        self.inner.demo.as_ref()
    }

    pub async fn acquire_run(
        &self,
        kind: &str,
        account: AccountHandle,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        self.adapter(kind)?.acquire_run(account).await
    }

    pub async fn acquire_control(
        &self,
        kind: &str,
        account: AccountHandle,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        // Control sessions are short (a status read, a sign-in step), so a
        // full pool frees up quickly: wait for a slot rather than report the
        // account as failing.
        let adapter = self.adapter(kind)?;
        let mut waited = std::time::Duration::ZERO;
        loop {
            match adapter.acquire_control(account.clone()).await {
                Err(HarnessError::ControlCapacity) if waited < CONTROL_SLOT_WAIT => {
                    tokio::time::sleep(CONTROL_SLOT_RETRY).await;
                    waited += CONTROL_SLOT_RETRY;
                }
                result => return result,
            }
        }
    }

    pub async fn health(&self, kind: &str, account_id: Uuid) -> Option<ProfileHealth> {
        self.adapter(kind).ok()?.health(account_id).await
    }

    pub async fn shutdown_account(&self, kind: &str, account_id: Uuid) {
        if let Ok(adapter) = self.adapter(kind) {
            adapter.shutdown_account(account_id).await;
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        self.inner.shutdown.is_shutting_down()
    }

    pub fn request_shutdown(&self) {
        self.inner.shutdown.request();
    }

    pub async fn shutdown_requested(&self) {
        self.inner.shutdown.requested().await;
    }

    pub async fn shutdown_all(&self) {
        self.request_shutdown();
        self.stop_local_turns().await;
    }

    /// Stop every turn running here, as when this instance loses the
    /// built-in host's lease: nobody may recover them while they run. Each
    /// turn watches the lease itself ([`Self::host_lease_lost`]) and stops
    /// its session; this stops what the adapters hold and then waits, for a
    /// while, until every turn has.
    pub async fn stop_local_turns(&self) {
        let custom = self
            .custom_read()
            .values()
            .map(|custom| Arc::clone(&custom.adapter))
            .collect::<Vec<_>>();
        for adapter in self.inner.adapters.values().chain(custom.iter()) {
            adapter.shutdown_all().await;
        }
        let stopped = tokio::time::timeout(LOCAL_TURNS_STOP_WAIT, async {
            loop {
                let idle = self.inner.turns_idle.notified();
                if self.inner.active_turns.load(Ordering::Acquire) == 0 {
                    return;
                }
                idle.await;
            }
        })
        .await;
        if stopped.is_err() {
            tracing::warn!(
                turns = self.inner.active_turns.load(Ordering::Acquire),
                "local turns still running after the stop wait"
            );
        }
    }

    /// Count a turn this process drives until the returned guard drops.
    pub fn turn_started(&self) -> ActiveTurn {
        self.inner.active_turns.fetch_add(1, Ordering::AcqRel);
        ActiveTurn {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Cancelled when this process doesn't hold, or loses, the built-in
    /// host's lease (K5.3): a turn driven here must then stop, since the next
    /// holder recovers it. Without a cluster (tests of the queue alone) it is
    /// never cancelled.
    pub fn host_lease_lost(&self) -> tokio_util::sync::CancellationToken {
        let Some(cluster) = self.cluster() else {
            return tokio_util::sync::CancellationToken::new();
        };
        match cluster.held_lease(&crate::cluster::LeaseName::BuiltinHost) {
            Some(lease) => lease.lost.clone(),
            None => {
                let lost = tokio_util::sync::CancellationToken::new();
                lost.cancel();
                lost
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::SupervisorRegistry;
    use crate::config::AppConfig;

    fn registry() -> SupervisorRegistry {
        let config = AppConfig::from_values(&BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://example.invalid/database".to_owned(),
        )]))
        .expect("registry test config");
        SupervisorRegistry::new(&config).expect("registry")
    }

    #[test]
    fn unknown_adapter_kinds_are_refused() {
        let registry = registry();
        assert!(registry.adapter("codex_app_server").is_ok());
        assert!(registry.adapter("nobody").is_err());
        assert!(
            registry.manifest("demo").is_none(),
            "demo runs only in development"
        );
    }
}
