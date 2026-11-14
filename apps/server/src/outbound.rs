//! Every connection to an address a workspace chooses goes through here: its
//! outgoing webhooks and agent wakeups, link previews of pages its members
//! post, its model provider (`ai.base_url`) and its mail server (and later
//! host links and connected modules). A workspace's owner isn't necessarily
//! the person who runs the server, and these requests leave from inside the
//! server's network, so unless the server's operator allows private
//! addresses (`AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES=true`, for a local
//! model or mail relay, or `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS` for one
//! runner) they must go to public ones.
//!
//! The rules every client here keeps:
//! - the name is resolved when the request is made, and every address it
//!   resolves to is checked (a name that also resolves to a private address
//!   is refused, whichever comes first);
//! - the client connects only to the addresses checked, so a second DNS
//!   answer can't point it somewhere else;
//! - it follows no redirects and uses no proxy, so nothing else picks the
//!   destination;
//! - a workspace holds at most [`PER_WORKSPACE_CONCURRENCY`] of these
//!   requests at once in a process ([`permit`]), so one workspace's slow
//!   receivers can't hold every connection the server has.
//!
//! Clients for fixed addresses the operator configures (GitHub, the sign-in
//! provider, push services, a provider's own cloud) are built where they are
//! used; `tests::only_outbound_builds_clients_a_workspace_can_aim` keeps that
//! list short.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::{Host, Url};
use uuid::Uuid;

use crate::error::AppError;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Ports mail servers take submissions on: SMTP, SMTPS, submission, and the
/// alternative submission port some providers offer.
pub const MAIL_PORTS: [u16; 4] = [25, 465, 587, 2525];

/// How many outbound requests one workspace may have open at once in one
/// process.
pub const PER_WORKSPACE_CONCURRENCY: usize = 8;

/// Why an address was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// A private, loopback, link-local or otherwise non-public address, or a
    /// name that plainly stays on this machine or its network.
    Private,
    /// The name didn't resolve in time, or to anything.
    Unresolved,
}

/// Whether `ip` is on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => public_ipv6(ip),
    }
}

/// Whether an IPv4 address is on the public internet: not this-network,
/// private, loopback, shared (carrier-grade NAT, which tailnets use),
/// link-local, documentation, benchmarking, multicast or reserved.
pub(crate) fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

/// Whether an IPv6 address is on the public internet, including the IPv4
/// address some IPv6 forms carry.
pub(crate) fn public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    !ip.is_unspecified()
        && !ip.is_loopback()
        && (segments[0] & 0xfe00) != 0xfc00
        && (segments[0] & 0xffc0) != 0xfe80
        && (segments[0] & 0xff00) != 0xff00
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && ip.to_ipv4_mapped().is_none_or(public_ipv4)
        && embedded_ipv4(ip).is_none_or(public_ipv4)
}

/// The IPv4 address inside a NAT64 (64:ff9b::/96), 6to4 (2002::/16) or
/// IPv4-compatible (::a.b.c.d) address, which can reach it on hosts that
/// translate them.
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let from = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        Some(from(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(from(s[1], s[2]))
    } else if s[..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback() && !ip.is_unspecified() {
        Some(from(s[6], s[7]))
    } else {
        None
    }
}

/// Whether `host` is plainly not a public name or address, without asking
/// DNS: `localhost`, `.local` and `.internal` names, single-label names, and
/// non-public address literals.
pub fn plainly_private(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !is_public(ip);
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".local")
        || name.ends_with(".internal")
        || !name.contains('.')
}

/// The addresses to connect to for `host` and `port`, in the order DNS gave
/// them, never empty: try each in turn, as a connection by name would.
/// Unless `allow_private`, every address the name resolves to must be public
/// (a name that also resolves to a private address is refused, whichever
/// comes first).
pub async fn resolve(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<Vec<SocketAddr>, Refusal> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if allow_private || is_public(ip) {
            Ok(vec![SocketAddr::new(ip, port)])
        } else {
            Err(Refusal::Private)
        };
    }
    if !allow_private && plainly_private(host) {
        return Err(Refusal::Private);
    }
    let addresses = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| Refusal::Unresolved)?
        .map_err(|_| Refusal::Unresolved)?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(Refusal::Unresolved);
    }
    if !allow_private && addresses.iter().any(|address| !is_public(address.ip())) {
        return Err(Refusal::Private);
    }
    Ok(addresses)
}

/// How long a pinned client waits.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    /// The whole request, or none (a model's answer streams for minutes).
    pub total: Option<Duration>,
}

/// The one way to build a client for an address a workspace chose: it
/// connects to `url`'s host only at `addresses` (every one that was checked,
/// tried in turn), follows no redirect and uses no proxy.
pub fn pinned_client(
    url: &Url,
    addresses: &[SocketAddr],
    timeouts: Timeouts,
    user_agent: Option<&'static str>,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(timeouts.connect)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(total) = timeouts.total {
        builder = builder.timeout(total);
    }
    if let Some(user_agent) = user_agent {
        builder = builder.user_agent(user_agent);
    }
    if let (Some(Host::Domain(name)), false) = (url.host(), addresses.is_empty()) {
        builder = builder.resolve_to_addrs(name, addresses);
    }
    builder.build()
}

/// Resolves `url`'s host, checks every address, and builds a client pinned
/// to them.
pub async fn client_for(
    url: &Url,
    allow_private: bool,
    timeouts: Timeouts,
    user_agent: Option<&'static str>,
) -> Result<reqwest::Client, Refusal> {
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Err(Refusal::Unresolved);
    };
    let addresses = resolve(host, port, allow_private).await?;
    pinned_client(url, &addresses, timeouts, user_agent).map_err(|_| Refusal::Unresolved)
}

/// One workspace's share of outbound requests: hold it for the length of the
/// request.
pub struct Permit {
    _held: OwnedSemaphorePermit,
}

static PERMITS: LazyLock<Mutex<HashMap<Uuid, Arc<Semaphore>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Waits for one of `workspace_id`'s [`PER_WORKSPACE_CONCURRENCY`] outbound
/// slots. Workspaces with nothing in flight hold no entry, so nothing here
/// grows with the number of workspaces on the server.
pub async fn permit(workspace_id: Uuid) -> Permit {
    let semaphore = {
        let mut permits = PERMITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Forget workspaces whose requests have all finished.
        permits.retain(|_, semaphore| {
            Arc::strong_count(semaphore) > 1
                || semaphore.available_permits() < PER_WORKSPACE_CONCURRENCY
        });
        Arc::clone(
            permits
                .entry(workspace_id)
                .or_insert_with(|| Arc::new(Semaphore::new(PER_WORKSPACE_CONCURRENCY))),
        )
    };
    let held = semaphore
        .acquire_owned()
        .await
        .unwrap_or_else(|_| unreachable!("outbound semaphores are never closed"));
    Permit { _held: held }
}

// --- Webhooks and agent wakeups ---------------------------------------------

const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(8);

/// Whether `url` is one the operator trusts with plain http and private
/// addresses (`AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS`).
fn is_trusted_origin(url: &Url, trusted_origins: &[String]) -> bool {
    matches!(url.scheme(), "http" | "https")
        && trusted_origins.contains(&url.origin().ascii_serialization())
}

/// A webhook or wakeup address as saved: https to a public host, with no
/// credentials or fragment, unless the operator trusts its exact origin.
pub(crate) fn validate_webhook_url(
    value: &str,
    trusted_origins: &[String],
) -> Result<Url, AppError> {
    let url = Url::parse(value).map_err(|_| AppError::Validation("webhook URL"))?;
    let host = url.host().ok_or(AppError::Validation("webhook URL"))?;
    if value.len() > 500
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Validation("webhook URL"));
    }
    if is_trusted_origin(&url, trusted_origins) {
        return Ok(url);
    }
    let loopback = match host {
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
        Host::Domain(_) => false,
    };
    if url.scheme() != "https"
        || matches!(host, Host::Domain(name) if name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost"))
        || matches!(host, Host::Ipv4(address) if !loopback && !public_ipv4(address))
        || matches!(host, Host::Ipv6(address) if !loopback && !public_ipv6(address))
        || loopback
    {
        return Err(AppError::Validation("webhook URL"));
    }
    Ok(url)
}

/// The client for one webhook delivery or wakeup to `value`, checked again
/// now (the address it names may resolve somewhere else than when it was
/// saved) and pinned to what was checked.
pub(crate) async fn webhook_client(
    value: &str,
    trusted_origins: &[String],
) -> Result<(Url, reqwest::Client), &'static str> {
    let url =
        validate_webhook_url(value, trusted_origins).map_err(|_| "the webhook URL is unsafe")?;
    let trusted = is_trusted_origin(&url, trusted_origins);
    let client = client_for(
        &url,
        trusted,
        Timeouts {
            connect: WEBHOOK_TIMEOUT,
            total: Some(WEBHOOK_TIMEOUT),
        },
        None,
    )
    .await
    .map_err(|refusal| match refusal {
        Refusal::Private => "the webhook host resolved to an unsafe address",
        Refusal::Unresolved => "the webhook host could not be resolved",
    })?;
    Ok((url, client))
}

// --- Pages a member links to (link previews) ---------------------------------

const PAGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const PAGE_USER_AGENT: &str = "AgentWorkspace-LinkPreview/1.0";

/// What may be fetched to preview a link. Outside tests, only public
/// addresses on the standard web ports.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PagePolicy {
    allow_loopback: bool,
}

impl PagePolicy {
    pub(crate) fn public() -> Self {
        Self {
            allow_loopback: false,
        }
    }

    /// Lets a test reach a server on this machine (any port). Never used
    /// outside tests.
    #[cfg(test)]
    pub(crate) fn loopback_for_tests() -> Self {
        Self {
            allow_loopback: true,
        }
    }

    fn address_allowed(self, ip: IpAddr) -> bool {
        is_public(ip) || (self.allow_loopback && ip.is_loopback())
    }

    /// The URL's shape: http or https, no credentials, the standard port,
    /// and no host that is plainly this machine or a private address.
    pub(crate) fn check_url(self, url: &Url) -> Result<(), &'static str> {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("the link isn't a plain web address");
        }
        if !self.allow_loopback && !matches!(url.port_or_known_default(), Some(80 | 443)) {
            return Err("the link uses an unusual port");
        }
        match url.host() {
            Some(Host::Domain(name)) => {
                if !self.allow_loopback && plainly_private(name) {
                    return Err("the link points at a private host");
                }
                Ok(())
            }
            Some(Host::Ipv4(ip)) if self.address_allowed(IpAddr::V4(ip)) => Ok(()),
            Some(Host::Ipv6(ip)) if self.address_allowed(IpAddr::V6(ip)) => Ok(()),
            _ => Err("the link points at a private address"),
        }
    }

    /// A client for one request to `url`, pinned to every address its name
    /// resolved to, all of them checked.
    pub(crate) async fn client_for(self, url: &Url) -> Result<reqwest::Client, &'static str> {
        self.check_url(url)?;
        let host = url.host_str().ok_or("the link has no host")?;
        let port = url.port_or_known_default().ok_or("the link has no port")?;
        let addresses = tokio::time::timeout(PAGE_CONNECT_TIMEOUT, resolve(host, port, true))
            .await
            .map_err(|_| "the host took too long to resolve")?
            .map_err(|_| "the host could not be resolved")?;
        if addresses
            .iter()
            .any(|address| !self.address_allowed(address.ip()))
        {
            return Err("the host resolved to a private address");
        }
        pinned_client(
            url,
            &addresses,
            Timeouts {
                connect: PAGE_CONNECT_TIMEOUT,
                total: Some(PAGE_REQUEST_TIMEOUT),
            },
            Some(PAGE_USER_AGENT),
        )
        .map_err(|_| "the request couldn't be made")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_literals_that_stay_inside_are_plainly_private() {
        for private in [
            "localhost",
            "LOCALHOST.",
            "ollama",
            "box.local",
            "metadata.google.internal",
            "app.localhost",
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.10",
            "100.119.56.3",
            "169.254.169.254",
            "[::1]",
            "::ffff:127.0.0.1",
            "fd00::1",
        ] {
            assert!(plainly_private(private), "{private}");
        }
        for public in ["api.example.com", "smtp.example.org.", "93.184.216.34"] {
            assert!(!plainly_private(public), "{public}");
        }
    }

    #[test]
    fn ipv6_addresses_carrying_a_private_ipv4_are_not_public() {
        for private in [
            "64:ff9b::10.0.0.1",
            "2002:a00:1::",
            "::192.168.1.1",
            "::ffff:127.0.0.1",
        ] {
            let ip = private.parse::<Ipv6Addr>().expect("address");
            assert!(!public_ipv6(ip), "{private}");
        }
        for public in ["64:ff9b::8.8.8.8", "2002:808:808::", "2606:4700::1111"] {
            let ip = public.parse::<Ipv6Addr>().expect("address");
            assert!(public_ipv6(ip), "{public}");
        }
    }

    #[tokio::test]
    async fn private_addresses_need_the_operator_switch() {
        assert_eq!(
            resolve("127.0.0.1", 11434, false).await,
            Err(Refusal::Private)
        );
        assert_eq!(
            resolve("localhost", 11434, false).await,
            Err(Refusal::Private)
        );
        assert_eq!(resolve("[::1]", 25, false).await, Err(Refusal::Private));
        assert_eq!(
            resolve("127.0.0.1", 11434, true).await,
            Ok(vec!["127.0.0.1:11434".parse().expect("address")])
        );
        // Resolved, not just read: localhost is only reachable with the
        // switch, and every address it has is kept to try in turn.
        let local = resolve("localhost", 2525, true).await.expect("localhost");
        assert!(!local.is_empty());
        assert!(
            local
                .iter()
                .all(|address| address.ip().is_loopback() && address.port() == 2525)
        );
        assert_eq!(
            resolve("93.184.216.34", 443, false).await,
            Ok(vec!["93.184.216.34:443".parse().expect("address")])
        );
    }

    /// A name that resolves to one public and one private address is refused
    /// for webhooks and previews alike: every address is checked, not only
    /// the first (`localhost` resolves to loopback only, so a name with two
    /// answers is built from a literal pair here).
    #[test]
    fn one_private_answer_among_public_ones_is_refused() {
        let answers: [SocketAddr; 2] = [
            "93.184.216.34:443".parse().expect("public"),
            "10.0.0.7:443".parse().expect("private"),
        ];
        assert!(answers.iter().any(|address| !is_public(address.ip())));
        let policy = PagePolicy::public();
        assert!(
            answers
                .iter()
                .any(|address| !policy.address_allowed(address.ip()))
        );
    }

    #[tokio::test]
    async fn webhooks_and_previews_refuse_private_names_before_resolving() {
        assert_eq!(
            webhook_client("https://intranet/hook", &[]).await.err(),
            Some("the webhook host resolved to an unsafe address")
        );
        assert_eq!(
            webhook_client("https://printer.local/hook", &[])
                .await
                .err(),
            Some("the webhook host resolved to an unsafe address")
        );
        let url = Url::parse("https://box.internal/").expect("url");
        assert!(PagePolicy::public().client_for(&url).await.is_err());
        // The operator's trusted origin may be private, and is pinned too.
        let trusted = ["http://127.0.0.1:8732".to_owned()];
        assert!(
            webhook_client("http://127.0.0.1:8732/wake", &trusted)
                .await
                .is_ok()
        );
    }

    #[test]
    fn browser_webhooks_reject_ssrf_and_credential_vectors() {
        for value in [
            "http://example.com/hook",
            "https://localhost/hook",
            "https://127.0.0.1/hook",
            "https://169.254.169.254/latest/meta-data",
            "https://10.0.0.1/hook",
            "https://user:password@example.com/hook",
            "https://example.com/hook#secret",
        ] {
            assert!(
                validate_webhook_url(value, &[]).is_err(),
                "accepted {value}"
            );
        }
        assert!(validate_webhook_url("https://example.com/hook", &[]).is_ok());
    }

    #[test]
    fn only_operator_trusted_origins_may_use_http_and_tailnet_addresses() {
        let trusted = ["http://100.119.56.3:8732".to_owned()];
        assert!(validate_webhook_url("http://100.119.56.3:8732/wake", &trusted).is_ok());
        // Without the operator's say-so the same listener is refused.
        assert!(validate_webhook_url("http://100.119.56.3:8732/wake", &[]).is_err());
        for value in [
            // Another port, scheme or host on the same network is not trusted.
            "http://100.119.56.3:8733/wake",
            "https://100.119.56.3:8732/wake",
            "http://100.119.56.4:8732/wake",
            "http://169.254.169.254/latest/meta-data",
            // Credentials and fragments stay refused even for trusted origins.
            "http://user:secret@100.119.56.3:8732/wake",
            "http://100.119.56.3:8732/wake#secret",
        ] {
            assert!(
                validate_webhook_url(value, &trusted).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn private_and_odd_preview_targets_are_refused() {
        let checked =
            |url: &str| PagePolicy::public().check_url(&Url::parse(url).expect("test fixture"));
        for refused in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://app.localhost/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.100.100.100/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://0.0.0.0/",
            "http://example.com:8080/",
            "https://user:pass@example.com/",
            "ftp://example.com/",
            "file:///etc/passwd",
            "http://intranet/",
            "http://printer.local/",
            "http://metadata.google.internal/",
        ] {
            assert!(checked(refused).is_err(), "{refused} should be refused");
        }
        for allowed in [
            "https://example.com/page",
            "http://example.org/",
            "https://93.184.216.34/",
            "https://example.com:443/x",
        ] {
            assert!(checked(allowed).is_ok(), "{allowed} should be allowed");
        }
    }

    #[tokio::test]
    async fn a_workspace_holds_a_bounded_share_and_idle_ones_hold_none() {
        let busy = Uuid::now_v7();
        let mut held = Vec::new();
        for _ in 0..PER_WORKSPACE_CONCURRENCY {
            held.push(permit(busy).await);
        }
        // The next request for the busy workspace waits; another workspace
        // goes straight through.
        let waiting = tokio::time::timeout(Duration::from_millis(50), permit(busy)).await;
        assert!(waiting.is_err(), "a ninth request waited for a slot");
        let other = tokio::time::timeout(Duration::from_millis(50), permit(Uuid::now_v7())).await;
        assert!(other.is_ok(), "another workspace was not held up");
        drop(other);
        drop(held);
        // Once its requests finish, a workspace leaves nothing behind.
        drop(permit(Uuid::now_v7()).await);
        let permits = PERMITS.lock().expect("permits");
        assert!(!permits.contains_key(&busy));
    }

    /// Clients for addresses a workspace can aim are built only here. A
    /// client for a fixed address the operator configures is allowed where
    /// it is used, with the reason it is safe.
    #[test]
    fn only_outbound_builds_clients_a_workspace_can_aim() {
        const FIXED_ADDRESS_CLIENTS: &[(&str, &str)] = &[
            ("ai/gateway.rs", "the built-in providers' own addresses"),
            ("auth/workos.rs", "the sign-in provider's API"),
            ("github.rs", "the configured GitHub API"),
            ("provider/claude_cloud.rs", "the provider's own cloud API"),
            (
                "push.rs",
                "known push services only (push::allowed_endpoint)",
            ),
        ];
        const BUILDERS: &[&str] = &[
            "Client::builder(",
            "ClientBuilder::new(",
            "reqwest::Client::new(",
            "reqwest::Client::default(",
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut folders = vec![root.clone()];
        let mut offenders = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("read source folder") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative == "outbound.rs"
                    || FIXED_ADDRESS_CLIENTS
                        .iter()
                        .any(|(file, _)| *file == relative)
                {
                    continue;
                }
                let mut source = std::fs::read_to_string(&path).expect("read source file");
                if let Some(tests) = source.find("\n#[cfg(test)]\nmod tests") {
                    source.truncate(tests);
                }
                if BUILDERS.iter().any(|builder| source.contains(builder)) {
                    offenders.push(relative);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "build HTTP clients for addresses a workspace chooses with outbound::client_for \
             or outbound::pinned_client, or list a fixed-address client with its reason: {offenders:?}"
        );
    }
}
