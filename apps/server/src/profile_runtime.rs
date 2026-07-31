use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags, fchmod, mkdirat, open, openat},
    io::Errno,
};

use crate::{error::AppError, git_runtime::absolute_runtime_path};

#[cfg(unix)]
const PRIVATE_DIRECTORY_MODE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);

pub fn resolve_path_within_data_root(
    configured_data_root: &Path,
    candidate: &Path,
) -> Result<PathBuf, AppError> {
    let data_root = normalize_absolute(&absolute_runtime_path(configured_data_root)?)?;
    let candidate = normalize_absolute(&absolute_runtime_path(candidate)?)?;
    if candidate == data_root || !candidate.starts_with(&data_root) {
        return Err(AppError::Invalid("runtime path escapes the data root"));
    }
    Ok(candidate)
}

/// Creates one provider profile beneath a configured root using descriptor-
/// relative, no-follow directory operations. The returned path is absolute
/// and uses the canonical location of the trusted ancestor above the root.
#[cfg(unix)]
pub fn prepare_private_profile_directory(
    configured_data_root: &Path,
    configured_root: &Path,
    profile_key: &str,
) -> Result<PathBuf, AppError> {
    validate_profile_key(profile_key)?;
    let normalized_data_root = normalize_absolute(&absolute_runtime_path(configured_data_root)?)?;
    let normalized_root = resolve_path_within_data_root(configured_data_root, configured_root)?;
    let relative_profile_root = normalized_root
        .strip_prefix(&normalized_data_root)
        .map_err(|_| AppError::Invalid("Codex profile root escapes the data root"))?;
    if relative_profile_root.as_os_str().is_empty() {
        return Err(AppError::Invalid(
            "Codex profile root must be beneath the data root",
        ));
    }

    let data_parent = normalized_data_root
        .parent()
        .ok_or(AppError::Invalid("unsafe data root"))?;
    let (anchor_path, relative_data_root) =
        existing_anchor_and_suffix(data_parent, &normalized_data_root)?;
    let canonical_anchor = anchor_path
        .canonicalize()
        .map_err(|_| AppError::Internal("canonicalize Codex profile ancestor"))?;
    let mut directory = open_directory(&canonical_anchor)?;
    let mut canonical_data_root = canonical_anchor;

    for component in relative_data_root.components() {
        let Component::Normal(name) = component else {
            return Err(AppError::Invalid("unsafe data root"));
        };
        directory = create_or_open_child(&directory, name)?;
        canonical_data_root.push(name);
    }
    fchmod(&directory, PRIVATE_DIRECTORY_MODE)
        .map_err(|_| AppError::Internal("protect application data root"))?;

    let mut canonical_root = canonical_data_root;
    for component in relative_profile_root.components() {
        let Component::Normal(name) = component else {
            return Err(AppError::Invalid("unsafe Codex profile root"));
        };
        directory = create_or_open_child(&directory, name)?;
        canonical_root.push(name);
    }
    fchmod(&directory, PRIVATE_DIRECTORY_MODE)
        .map_err(|_| AppError::Internal("protect Codex profile root"))?;

    let profile = create_or_open_child(&directory, OsStr::new(profile_key))?;
    fchmod(&profile, PRIVATE_DIRECTORY_MODE)
        .map_err(|_| AppError::Internal("protect Codex profile"))?;
    let profile_path = canonical_root.join(profile_key);

    // The final path check catches a rename/substitution that occurred while
    // the descriptor-relative creation sequence was in progress. Provider
    // processes run under the same non-login service account, so no untrusted
    // principal is granted concurrent write access to the containing root.
    let canonical_profile = profile_path
        .canonicalize()
        .map_err(|_| AppError::Internal("canonicalize Codex profile"))?;
    if canonical_profile != profile_path {
        return Err(AppError::Invalid("unsafe Codex profile path"));
    }

    Ok(profile_path)
}

#[cfg(not(unix))]
pub fn prepare_private_profile_directory(
    _configured_data_root: &Path,
    _configured_root: &Path,
    _profile_key: &str,
) -> Result<PathBuf, AppError> {
    Err(AppError::Internal(
        "private Codex profiles require a Unix host",
    ))
}

fn validate_profile_key(profile_key: &str) -> Result<(), AppError> {
    if profile_key.is_empty()
        || profile_key.len() > 64
        || !profile_key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(AppError::Invalid("unsafe Codex profile key"));
    }
    Ok(())
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, AppError> {
    if !path.is_absolute() {
        return Err(AppError::Invalid("Codex profile root must be absolute"));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AppError::Invalid("unsafe Codex profile root"));
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn existing_anchor_and_suffix<'a>(
    root_parent: &'a Path,
    normalized_root: &'a Path,
) -> Result<(PathBuf, PathBuf), AppError> {
    let mut anchor = root_parent.to_path_buf();
    while !anchor.exists() {
        if !anchor.pop() {
            return Err(AppError::Invalid("unsafe Codex profile root"));
        }
    }
    let suffix = normalized_root
        .strip_prefix(&anchor)
        .map_err(|_| AppError::Invalid("unsafe Codex profile root"))?
        .to_path_buf();
    Ok((anchor, suffix))
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<OwnedFd, AppError> {
    open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| AppError::Invalid("unsafe Codex profile ancestor"))
}

#[cfg(unix)]
fn create_or_open_child(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd, AppError> {
    match mkdirat(parent, name, PRIVATE_DIRECTORY_MODE) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(_) => return Err(AppError::Internal("create Codex profile directory")),
    }
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| AppError::Invalid("unsafe Codex profile directory"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::TempDir;

    use super::{prepare_private_profile_directory, resolve_path_within_data_root};

    #[test]
    fn creates_an_absolute_owner_only_profile_and_is_idempotent() {
        let temporary = TempDir::new().expect("temporary root");
        let data_root = temporary.path().join("data");
        let configured = data_root.join("profiles");
        let first = prepare_private_profile_directory(&data_root, &configured, "profile-1")
            .expect("create private profile");
        let second = prepare_private_profile_directory(&data_root, &configured, "profile-1")
            .expect("reuse private profile");

        assert!(first.is_absolute());
        assert_eq!(first, second);
        assert_eq!(
            fs::metadata(&first)
                .expect("profile metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(first.parent().expect("profile root"))
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn rejects_a_profile_symlink_without_touching_its_target() {
        let temporary = TempDir::new().expect("temporary root");
        let data_root = temporary.path().join("data");
        let configured = data_root.join("profiles");
        fs::create_dir(&data_root).expect("create data root");
        fs::create_dir(&configured).expect("create configured root");
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).expect("create outside directory");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755))
            .expect("set outside permissions");
        std::os::unix::fs::symlink(&outside, configured.join("profile-1"))
            .expect("create malicious profile symlink");

        assert!(prepare_private_profile_directory(&data_root, &configured, "profile-1").is_err());
        assert_eq!(
            fs::metadata(&outside)
                .expect("outside metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "a rejected symlink target must not be chmodded"
        );
    }

    #[test]
    fn rejects_a_symlink_inside_the_configured_root_path() {
        let temporary = TempDir::new().expect("temporary root");
        let data_root = temporary.path().join("data");
        fs::create_dir(&data_root).expect("create data root");
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).expect("create outside directory");
        std::os::unix::fs::symlink(&outside, data_root.join("profiles"))
            .expect("create malicious root symlink");

        assert!(
            prepare_private_profile_directory(
                &data_root,
                &data_root.join("profiles").join("nested"),
                "profile-1"
            )
            .is_err()
        );
        assert!(!outside.join("nested").exists());
    }

    #[test]
    fn rejects_path_like_profile_keys() {
        let temporary = TempDir::new().expect("temporary root");
        for key in ["../escape", "two/components", ".", "", "UPPER"] {
            assert!(
                prepare_private_profile_directory(
                    temporary.path(),
                    &temporary.path().join("profiles"),
                    key
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_a_profile_root_outside_the_data_root() {
        let temporary = TempDir::new().expect("temporary root");
        assert!(
            prepare_private_profile_directory(
                &temporary.path().join("data"),
                &temporary.path().join("outside"),
                "profile-1"
            )
            .is_err()
        );
    }

    #[test]
    fn resolves_only_descendants_of_the_data_root() {
        let temporary = TempDir::new().expect("temporary root");
        let data_root = temporary.path().join("data");
        let mirror = data_root.join("repositories").join("mirror.git");
        assert_eq!(
            resolve_path_within_data_root(&data_root, &mirror).expect("contained mirror"),
            mirror
        );
        assert!(resolve_path_within_data_root(&data_root, temporary.path()).is_err());
        assert!(resolve_path_within_data_root(&data_root, &data_root).is_err());
    }
}
