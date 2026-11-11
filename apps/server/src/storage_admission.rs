use std::{fs, os::unix::fs::MetadataExt, path::Path};

use crate::error::AppError;

const MAX_ACCOUNTED_ENTRIES: usize = 250_000;
/// Folders directly under the data root whose bytes are counted elsewhere:
/// uploaded files are counted from the database (`files::require_room`), so
/// many small files can't run the walk into its entry limit.
const ACCOUNTED_ELSEWHERE: &[&str] = &["files"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageAdmission {
    pub allocated_bytes: u64,
    pub available_bytes: u64,
    pub admitted: bool,
}

pub fn inspect(
    data_root: &Path,
    quota_bytes: u64,
    minimum_free_bytes: u64,
) -> Result<StorageAdmission, AppError> {
    inspect_with_metadata(data_root, quota_bytes, minimum_free_bytes, |path| {
        fs::symlink_metadata(path)
    })
}

fn inspect_with_metadata(
    data_root: &Path,
    quota_bytes: u64,
    minimum_free_bytes: u64,
    metadata_for: impl Fn(&Path) -> std::io::Result<fs::Metadata>,
) -> Result<StorageAdmission, AppError> {
    let data_root = if data_root.is_absolute() {
        data_root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| AppError::Internal("resolve storage admission root"))?
            .join(data_root)
    };
    let mut pending = vec![data_root.clone()];
    let mut entries = 0_usize;
    let mut allocated_bytes = 0_u64;
    while let Some(path) = pending.pop() {
        entries = entries.saturating_add(1);
        if entries > MAX_ACCOUNTED_ENTRIES {
            return Err(AppError::Conflict(
                "storage",
                "storage accounting limit reached",
            ));
        }
        let metadata = match metadata_for(&path) {
            Ok(metadata) => metadata,
            Err(error) if path != data_root && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(_) => return Err(AppError::Internal("inspect storage admission root")),
        };
        if metadata.file_type().is_symlink() {
            if path == data_root {
                return Err(AppError::Conflict(
                    "storage",
                    "storage root must not be a symlink",
                ));
            }
            // Provider profiles and project-local tool caches may contain
            // symlinks. Admission must never traverse them, but their mere
            // presence cannot block every unrelated disk-growing operation.
            // Registered destination roots still reject symlink path
            // components before writes are attempted.
            continue;
        }
        if metadata.is_dir() {
            let children = match fs::read_dir(&path) {
                Ok(children) => children,
                Err(error) if path != data_root && error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(_) => return Err(AppError::Internal("read storage admission root")),
            };
            for child in children {
                let child = child
                    .map_err(|_| AppError::Internal("read storage admission entry"))?
                    .path();
                if path == data_root
                    && child
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| ACCOUNTED_ELSEWHERE.contains(&name))
                {
                    continue;
                }
                pending.push(child);
            }
        } else if metadata.is_file() {
            allocated_bytes = allocated_bytes.saturating_add(metadata.blocks().saturating_mul(512));
        } else {
            return Err(AppError::Conflict(
                "storage",
                "storage root contains an unsupported entry",
            ));
        }
    }
    let filesystem = rustix::fs::statvfs(&data_root)
        .map_err(|_| AppError::Internal("inspect storage filesystem"))?;
    let available_bytes = filesystem.f_bavail.saturating_mul(filesystem.f_frsize);
    Ok(StorageAdmission {
        allocated_bytes,
        available_bytes,
        admitted: allocated_bytes < quota_bytes && available_bytes > minimum_free_bytes,
    })
}

pub fn require_growth(
    data_root: &Path,
    quota_bytes: u64,
    minimum_free_bytes: u64,
) -> Result<(), AppError> {
    if inspect(data_root, quota_bytes, minimum_free_bytes)?.admitted {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "storage",
            "disk growth is blocked by the configured watermark",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn quota_blocks_growth_without_blocking_inspection() {
        let root = TempDir::new().expect("storage root");
        fs::write(root.path().join("artifact"), vec![0_u8; 8192]).expect("artifact");
        let report = inspect(root.path(), 1, 1).expect("storage report");
        assert!(!report.admitted);
        assert!(report.allocated_bytes > 0);
        assert!(require_growth(root.path(), 1, 1).is_err());
    }

    #[test]
    fn symlinks_are_skipped_without_following_targets() {
        let root = TempDir::new().expect("storage root");
        let outside = TempDir::new().expect("outside root");
        fs::write(outside.path().join("large"), vec![0_u8; 8192]).expect("outside artifact");
        fs::write(root.path().join("inside"), vec![0_u8; 4096]).expect("inside artifact");
        symlink(outside.path(), root.path().join("outside")).expect("symlink");

        let report = inspect(root.path(), u64::MAX, 1).expect("storage report");
        let inside_blocks = fs::metadata(root.path().join("inside"))
            .expect("inside metadata")
            .blocks()
            .saturating_mul(512);
        assert_eq!(report.allocated_bytes, inside_blocks);
        assert!(report.admitted);
    }

    #[test]
    fn a_symlinked_root_cannot_hide_its_usage() {
        let directory = TempDir::new().expect("directory");
        let data = directory.path().join("data");
        fs::create_dir(&data).expect("data");
        fs::write(data.join("artifact"), vec![0_u8; 8192]).expect("artifact");
        let linked = directory.path().join("linked");
        symlink(&data, &linked).expect("root symlink");
        assert!(
            inspect(&linked, 1, 0).is_err(),
            "a symlink must not report zero usage"
        );
    }

    #[test]
    fn a_child_removed_during_the_walk_does_not_block_admission() {
        for remove_after_metadata in [false, true] {
            let root = TempDir::new().expect("root");
            let child = root.path().join("removed");
            fs::create_dir(&child).expect("child");
            fs::write(child.join("artifact"), b"gone").expect("artifact");
            let report = inspect_with_metadata(root.path(), u64::MAX, 0, |path| {
                if path == child {
                    let metadata = remove_after_metadata.then(|| fs::symlink_metadata(path));
                    fs::remove_dir_all(path).expect("concurrent removal");
                    return metadata.unwrap_or_else(|| fs::symlink_metadata(path));
                }
                fs::symlink_metadata(path)
            })
            .expect("a removed descendant is no longer using space");
            assert!(report.admitted);
            assert_eq!(report.allocated_bytes, 0);
        }
        let missing = TempDir::new().expect("root");
        let path = missing.path().to_owned();
        missing.close().expect("remove root");
        assert!(
            inspect(&path, u64::MAX, 0).is_err(),
            "a missing root still fails"
        );
    }
}
