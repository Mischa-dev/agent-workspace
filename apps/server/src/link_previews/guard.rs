//! The SSRF guard for link preview fetches: which URLs the server may
//! request, and a client pinned to the address that was checked.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use url::{Host, Url};

use crate::replies::{public_ipv4, public_ipv6};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const USER_AGENT: &str =
    "AgentWorkspace-LinkPreview/1.0 (+https://github.com/Mischa-dev/agent-workspace)";

/// What the guard lets through. Outside tests, only public addresses on the
/// standard web ports.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    allow_loopback: bool,
}

impl Policy {
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
        match ip {
            IpAddr::V4(ip) => public_ipv4(ip) || (self.allow_loopback && ip.is_loopback()),
            IpAddr::V6(ip) => public_ipv6(ip) || (self.allow_loopback && ip.is_loopback()),
        }
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
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                if !self.allow_loopback
                    && (name == "localhost"
                        || name.ends_with(".localhost")
                        || name.ends_with(".local")
                        || name.ends_with(".internal")
                        || !name.contains('.'))
                {
                    return Err("the link points at a private host");
                }
                Ok(())
            }
            Some(Host::Ipv4(ip)) if self.address_allowed(IpAddr::V4(ip)) => Ok(()),
            Some(Host::Ipv6(ip)) if self.address_allowed(IpAddr::V6(ip)) => Ok(()),
            _ => Err("the link points at a private address"),
        }
    }

    /// A client for one request to `url`: the name is resolved once, every
    /// address it gives must be allowed, and the client connects only to the
    /// address checked (a second DNS answer can't redirect it). It follows no
    /// redirects and uses no proxy, so nothing else picks the destination.
    pub(crate) async fn client_for(self, url: &Url) -> Result<reqwest::Client, &'static str> {
        self.check_url(url)?;
        let host = url.host_str().ok_or("the link has no host")?;
        let port = url.port_or_known_default().ok_or("the link has no port")?;
        let lookup_host = host.trim_start_matches('[').trim_end_matches(']');
        let addresses = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::lookup_host((lookup_host, port)),
        )
        .await
        .map_err(|_| "the host took too long to resolve")?
        .map_err(|_| "the host could not be resolved")?
        .collect::<Vec<SocketAddr>>();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| !self.address_allowed(address.ip()))
        {
            return Err("the host resolved to a private address");
        }
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(USER_AGENT);
        if matches!(url.host(), Some(Host::Domain(_))) {
            builder = builder.resolve(host, addresses[0]);
        }
        builder.build().map_err(|_| "the request couldn't be made")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(url: &str) -> Result<(), &'static str> {
        Policy::public().check_url(&Url::parse(url).unwrap())
    }

    #[test]
    fn private_and_odd_targets_are_refused() {
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
}
