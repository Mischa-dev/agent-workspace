//! The registered harness adapters and the server-wide state they share:
//! shutdown, and sign-in ceremonies (which are the same for every harness).
//! Stored adapter kinds are validated against this registry rather than by
//! database checks (H1), so adding a harness is adding an adapter here.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use super::{
    adapter::{
        AccountHandle, HarnessAdapter, HarnessError, HarnessSession, LoginKind, LoginStart,
        ProfileHealth,
    },
    claude_code::ClaudeCodeAdapter,
    codex_app_server::CodexAdapter,
    codex_cloud::CodexCloudAdapter,
    demo::{DEMO_KIND, DemoAdapter, DemoHarness},
    host::{HarnessHost, HostCell},
    manifest::AdapterManifest,
};
use crate::config::AppConfig;

/// How many sign-in ceremonies may be open on the host at once.
const MAX_LOGIN_CEREMONIES: usize = 32;
const LOGIN_CEREMONY_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct ShutdownSignal {
    shutting_down: AtomicBool,
    notify: Notify,
}

impl ShutdownSignal {
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn request(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub async fn requested(&self) {
        let notified = self.notify.notified();
        if self.is_shutting_down() {
            return;
        }
        notified.await;
    }
}

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
    ceremonies: Mutex<HashMap<Uuid, LoginCeremony>>,
}

#[derive(Clone)]
pub struct LoginCeremonySnapshot {
    pub ceremony_id: Uuid,
    pub kind: LoginKind,
    pub response: Option<LoginStart>,
    pub expires_at_unix_seconds: u64,
}

pub enum LoginReservation {
    Reserved(Uuid),
    Replay(LoginCeremonySnapshot),
}

#[derive(Debug, thiserror::Error)]
pub enum LoginCeremonyError {
    #[error("another login ceremony is active for this account")]
    Active,
    #[error("the host login ceremony limit is reached")]
    Capacity,
    #[error("the login ceremony is not available to this session")]
    NotFound,
}

struct LoginCeremony {
    ceremony_id: Uuid,
    actor_id: Uuid,
    session_id: Uuid,
    idempotency_key: String,
    kind: LoginKind,
    response: Option<LoginStart>,
    expires_at: Instant,
    expires_at_unix_seconds: u64,
}

impl LoginCeremony {
    fn snapshot(&self) -> LoginCeremonySnapshot {
        LoginCeremonySnapshot {
            ceremony_id: self.ceremony_id,
            kind: self.kind,
            response: self.response.clone(),
            expires_at_unix_seconds: self.expires_at_unix_seconds,
        }
    }
}

const CONTROL_SLOT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const CONTROL_SLOT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

impl SupervisorRegistry {
    pub fn new(config: &AppConfig) -> Result<Self, HarnessError> {
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
                ceremonies: Mutex::new(HashMap::new()),
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

    pub(crate) fn shutdown_signal(&self) -> Arc<ShutdownSignal> {
        Arc::clone(&self.inner.shutdown)
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
        let custom = self
            .custom_read()
            .values()
            .map(|custom| Arc::clone(&custom.adapter))
            .collect::<Vec<_>>();
        for adapter in self.inner.adapters.values().chain(custom.iter()) {
            adapter.shutdown_all().await;
        }
    }

    pub async fn reserve_login(
        &self,
        account_id: Uuid,
        actor_id: Uuid,
        session_id: Uuid,
        idempotency_key: &str,
        kind: LoginKind,
    ) -> Result<LoginReservation, LoginCeremonyError> {
        let mut ceremonies = self.inner.ceremonies.lock().await;
        let now = Instant::now();
        ceremonies.retain(|_, ceremony| ceremony.expires_at > now);
        if let Some(ceremony) = ceremonies.get(&account_id) {
            if ceremony.actor_id == actor_id
                && ceremony.session_id == session_id
                && ceremony.idempotency_key == idempotency_key
                && ceremony.kind == kind
                && ceremony.response.is_some()
            {
                return Ok(LoginReservation::Replay(ceremony.snapshot()));
            }
            return Err(LoginCeremonyError::Active);
        }
        if ceremonies.len() >= MAX_LOGIN_CEREMONIES {
            return Err(LoginCeremonyError::Capacity);
        }
        let ceremony_id = Uuid::now_v7();
        let expires_at_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(LOGIN_CEREMONY_LIFETIME.as_secs());
        ceremonies.insert(
            account_id,
            LoginCeremony {
                ceremony_id,
                actor_id,
                session_id,
                idempotency_key: idempotency_key.to_owned(),
                kind,
                response: None,
                expires_at: now + LOGIN_CEREMONY_LIFETIME,
                expires_at_unix_seconds,
            },
        );
        Ok(LoginReservation::Reserved(ceremony_id))
    }

    pub async fn complete_login(
        &self,
        account_id: Uuid,
        ceremony_id: Uuid,
        response: LoginStart,
    ) -> Result<LoginCeremonySnapshot, LoginCeremonyError> {
        let mut ceremonies = self.inner.ceremonies.lock().await;
        let ceremony = ceremonies
            .get_mut(&account_id)
            .filter(|ceremony| ceremony.ceremony_id == ceremony_id)
            .ok_or(LoginCeremonyError::NotFound)?;
        ceremony.response = Some(response);
        Ok(ceremony.snapshot())
    }

    pub async fn login_ceremony(
        &self,
        account_id: Uuid,
        actor_id: Uuid,
        session_id: Uuid,
    ) -> Option<LoginCeremonySnapshot> {
        let mut ceremonies = self.inner.ceremonies.lock().await;
        if ceremonies
            .get(&account_id)
            .is_some_and(|ceremony| ceremony.expires_at <= Instant::now())
        {
            ceremonies.remove(&account_id);
        }
        ceremonies
            .get(&account_id)
            .filter(|ceremony| ceremony.actor_id == actor_id && ceremony.session_id == session_id)
            .map(LoginCeremony::snapshot)
    }

    pub async fn clear_login(&self, account_id: Uuid, ceremony_id: Option<Uuid>) {
        let mut ceremonies = self.inner.ceremonies.lock().await;
        let should_remove = ceremonies.get(&account_id).is_some_and(|ceremony| {
            ceremony_id.is_none_or(|ceremony_id| ceremony.ceremony_id == ceremony_id)
        });
        if should_remove {
            ceremonies.remove(&account_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::{LoginCeremonyError, LoginReservation, SupervisorRegistry};
    use crate::{
        config::AppConfig,
        provider::adapter::{LoginKind, LoginStart},
    };

    fn registry() -> SupervisorRegistry {
        let config = AppConfig::from_values(&BTreeMap::from([(
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            "postgres://example.invalid/database".to_owned(),
        )]))
        .expect("registry test config");
        SupervisorRegistry::new(&config).expect("registry")
    }

    #[tokio::test]
    async fn login_ceremonies_replay_only_for_the_same_session_and_key() {
        let registry = registry();
        let account_id = Uuid::now_v7();
        let actor_id = Uuid::now_v7();
        let session_id = Uuid::now_v7();
        let LoginReservation::Reserved(ceremony_id) = registry
            .reserve_login(
                account_id,
                actor_id,
                session_id,
                "key",
                LoginKind::DeviceCode,
            )
            .await
            .expect("reserve")
        else {
            panic!("first login must reserve");
        };
        registry
            .complete_login(
                account_id,
                ceremony_id,
                LoginStart::DeviceCode {
                    login_id: "login-1".to_owned(),
                    verification_url: "https://example.com/device".to_owned(),
                    user_code: "ABCD-EFGH".to_owned(),
                },
            )
            .await
            .expect("complete");
        assert!(matches!(
            registry
                .reserve_login(
                    account_id,
                    actor_id,
                    session_id,
                    "key",
                    LoginKind::DeviceCode
                )
                .await,
            Ok(LoginReservation::Replay(_))
        ));
        assert!(matches!(
            registry
                .reserve_login(
                    account_id,
                    actor_id,
                    Uuid::now_v7(),
                    "other",
                    LoginKind::DeviceCode
                )
                .await,
            Err(LoginCeremonyError::Active)
        ));
        assert!(
            registry
                .login_ceremony(account_id, actor_id, Uuid::now_v7())
                .await
                .is_none()
        );
        registry.clear_login(account_id, Some(ceremony_id)).await;
        assert!(
            registry
                .login_ceremony(account_id, actor_id, session_id)
                .await
                .is_none()
        );
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
