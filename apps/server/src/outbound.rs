//! Connections to addresses a workspace chooses: its model provider
//! (`ai.base_url`) and its mail server. A workspace's owner isn't
//! necessarily the person who runs the server, and these requests leave from
//! inside the server's network, so unless the server's operator allows
//! private addresses (`AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES=true`, for a
//! local model or mail relay) they must go to public ones. The name is
//! resolved when the request is made, and the connection goes only to the
//! addresses that were checked, so a second DNS answer can't point it
//! somewhere else.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use crate::replies::{public_ipv4, public_ipv6};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Ports mail servers take submissions on: SMTP, SMTPS, submission, and the
/// alternative submission port some providers offer.
pub const MAIL_PORTS: [u16; 4] = [25, 465, 587, 2525];

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
            "100.64.0.10",
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
}
