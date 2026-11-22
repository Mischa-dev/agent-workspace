//! Git names and addresses that travel between Workspace and a host (a
//! repository's default branch, an environment's branch, a run's branch, a
//! clone address): which names are safe to hand to git, and an address
//! with no credentials in it. Workspace checks them when they are saved; a
//! host checks them again before it uses one.

/// Whether `value` is a branch or tag name safe to pass to git: plain
/// letters, digits and `-_./`, no component that is empty, hidden or a
/// lock file, and none of the forms git reads as something else (`@`,
/// `..`, `@{`, a leading `-`).
pub fn is_safe_ref(value: &str) -> bool {
    !(value.is_empty()
        || value.len() > 255
        || value == "@"
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        || value.split('/').any(|component| {
            component.is_empty() || component.starts_with('.') || component.ends_with(".lock")
        }))
}

/// A clone address without any user name or token in it
/// (`https://token@host/x.git` and `git@host:x.git` lose the part before
/// the `@`). Credentials never travel in a command.
pub fn without_user_info(value: &str) -> String {
    match value.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = match rest.find('/') {
                Some(index) => rest.split_at(index),
                None => (rest, ""),
            };
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!("{scheme}://{host}{path}")
        }
        // scp-style "git@host:owner/name.git"
        None => value.rsplit('@').next().unwrap_or(value).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_safe_ref, without_user_info};

    #[test]
    fn addresses_lose_their_credentials() {
        assert_eq!(
            without_user_info("https://x-access-token:abc@example.com/o/r.git"),
            "https://example.com/o/r.git"
        );
        assert_eq!(
            without_user_info("git@example.com:o/r.git"),
            "example.com:o/r.git"
        );
        assert_eq!(
            without_user_info("https://example.com/o/r.git"),
            "https://example.com/o/r.git"
        );
    }

    #[test]
    fn only_plain_names_are_safe() {
        for safe in ["main", "release/1.2", "feature_x-y", "v1.0"] {
            assert!(is_safe_ref(safe), "{safe}");
        }
        for unsafe_ref in [
            "", "@", "-main", "/main", "main/", "main.", "a//b", "a..b", "a@{1}", "a b", "a/.b",
            "a/b.lock", "a;b",
        ] {
            assert!(!is_safe_ref(unsafe_ref), "{unsafe_ref}");
        }
        assert!(!is_safe_ref(&"a".repeat(256)));
    }
}
