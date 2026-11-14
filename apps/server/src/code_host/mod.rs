//! A1.18: the code-host seam (A1 section 3.14), part 1. What the rest of the
//! server needs from wherever a repository lives (GitHub today), behind one
//! trait, so another code host is an implementation rather than a rewrite.
//!
//! Part 1 holds what callers use now: naming a repository from its address,
//! reading a pull request's address, and deciding which remotes a branch
//! may be pushed to. Short-lived repository tokens join when hosts receive
//! `start` commands (A1.12), and opening and reading pull requests when
//! their callers move behind the seam (A1.19).

pub mod github;

/// A repository as its code host names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoRef {
    /// Which code host: `github`.
    pub code_host: &'static str,
    /// `owner/name`, as the code host writes it.
    pub full_name: String,
}

/// A code host: where repositories, branches and pull requests live.
pub trait CodeHost: Send + Sync {
    /// The key stored in `repositories.code_host`.
    fn kind(&self) -> &'static str;
    /// The repository a web or clone address names, if it is one of this
    /// code host's.
    fn parse_url(&self, url: &str) -> Option<RepoRef>;
    /// Whether a branch may be pushed to this remote: one of this code
    /// host's repositories over https, with no credentials or control
    /// characters in the address.
    fn is_safe_remote(&self, url: &str) -> bool;
    /// The repository and number a pull request's address names.
    fn pull_request_by_url(&self, url: &str) -> Option<(RepoRef, i64)>;
}

/// Every code host this server knows.
pub fn all() -> [&'static dyn CodeHost; 1] {
    [&github::GITHUB]
}

/// The code host stored as `kind`.
pub fn for_kind(kind: &str) -> Option<&'static dyn CodeHost> {
    all().into_iter().find(|host| host.kind() == kind)
}

/// Whether some code host accepts `url` as a remote to push to.
pub fn is_safe_remote(url: &str) -> bool {
    all().into_iter().any(|host| host.is_safe_remote(url))
}

/// The code host, repository and number a pull request's address names.
pub fn pull_request_by_url(url: &str) -> Option<(RepoRef, i64)> {
    all()
        .into_iter()
        .find_map(|host| host.pull_request_by_url(url))
}
