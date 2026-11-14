//! A1.18: GitHub as a code host. The rules are the ones the server already
//! used (repositories.rs for names, git_runtime.rs for push remotes); they
//! live here now.

use super::{CodeHost, RepoRef};

pub struct GitHub;

pub static GITHUB: GitHub = GitHub;

const WEB: &str = "https://github.com/";

/// An owner or repository name as GitHub allows it.
pub fn valid_component(value: &str, repository_name: bool) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte == b'-'
                || (repository_name && matches!(byte, b'_' | b'.'))
        })
}

fn clean(url: &str) -> Option<&str> {
    let url = url.trim();
    (!url.contains('@') && !url.bytes().any(|byte| byte.is_ascii_control()))
        .then_some(url)?
        .strip_prefix(WEB)
}

impl CodeHost for GitHub {
    fn kind(&self) -> &'static str {
        "github"
    }

    fn parse_url(&self, url: &str) -> Option<RepoRef> {
        let rest = clean(url)?;
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let mut parts = rest.split('/');
        let (owner, name) = (parts.next()?, parts.next()?);
        (parts.next().is_none() && valid_component(owner, false) && valid_component(name, true))
            .then(|| RepoRef {
                code_host: "github",
                full_name: format!("{owner}/{name}"),
            })
    }

    fn is_safe_remote(&self, url: &str) -> bool {
        // The same remotes git_runtime's push check allowed before the seam.
        url.starts_with(WEB)
            && url.ends_with(".git")
            && !url.contains('@')
            && !url.bytes().any(|byte| byte.is_ascii_control())
    }

    fn pull_request_by_url(&self, url: &str) -> Option<(RepoRef, i64)> {
        let rest = clean(url)?;
        let rest = rest.split(['?', '#']).next()?;
        let mut parts = rest.split('/');
        let (owner, name, pull, number) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let tail_ok = match parts.next() {
            None | Some("") => parts.next().is_none(),
            Some("files" | "commits" | "checks") => true,
            Some(_) => false,
        };
        let number = number.parse::<i64>().ok().filter(|number| *number > 0)?;
        (pull == "pull"
            && tail_ok
            && valid_component(owner, false)
            && valid_component(name, true))
        .then(|| {
            (
                RepoRef {
                    code_host: "github",
                    full_name: format!("{owner}/{name}"),
                },
                number,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeHost, GITHUB};

    /// The check git_runtime.rs made before the seam, kept here to compare.
    fn old_push_check(url: &str) -> bool {
        url.starts_with("https://github.com/")
            && url.ends_with(".git")
            && !url.contains('@')
            && !url.bytes().any(|byte| byte.is_ascii_control())
    }

    #[test]
    fn safe_remotes_are_the_ones_the_push_check_allowed() {
        for url in [
            "https://github.com/acme/app.git",
            "https://github.com/acme/app",
            "https://token@github.com/acme/app.git",
            "https://github.com/acme/app.git\n",
            "http://github.com/acme/app.git",
            "https://github.com.evil.example/acme/app.git",
            "https://gitlab.com/acme/app.git",
            "git@github.com:acme/app.git",
            "ssh://github.com/acme/app.git",
            "https://github.com/acme/app.git?x=1",
            "",
        ] {
            assert_eq!(GITHUB.is_safe_remote(url), old_push_check(url), "{url:?}");
            assert_eq!(crate::code_host::is_safe_remote(url), old_push_check(url), "{url:?}");
        }
    }

    #[test]
    fn repositories_and_pull_requests_are_read_from_their_addresses() {
        assert_eq!(
            GITHUB.parse_url("https://github.com/acme/app.git").map(|repo| repo.full_name),
            Some("acme/app".to_owned())
        );
        assert!(GITHUB.parse_url("https://github.com/acme").is_none());
        assert!(GITHUB.parse_url("https://x@github.com/acme/app").is_none());
        let (repo, number) = GITHUB
            .pull_request_by_url("https://github.com/acme/app/pull/12")
            .expect("pull request");
        assert_eq!((repo.full_name.as_str(), number), ("acme/app", 12));
        assert!(GITHUB.pull_request_by_url("https://github.com/acme/app/pull/12/files").is_some());
        assert!(GITHUB.pull_request_by_url("https://github.com/acme/app/issues/12").is_none());
        assert!(GITHUB.pull_request_by_url("https://github.com/acme/app/pull/0").is_none());
        assert!(GITHUB.pull_request_by_url("https://code.example/acme/app/pull/12").is_none());
    }
}
