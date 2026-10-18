//! Where file bytes live. One interface, so the store can change without
//! touching anything that uses it: local disk under the data root today, an
//! S3-compatible object store later (add a variant here and a setting in
//! `config`). Keys are opaque, made only of the workspace and file IDs, and
//! never contain a machine path.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use axum::body::Body;
use tokio::io::AsyncWriteExt;

use crate::{config::AppConfig, error::AppError};

/// The folder under the data root that holds uploaded files.
const LOCAL_FOLDER: &str = "files";

#[derive(Clone, Debug)]
pub enum FileStore {
    Local(LocalDisk),
}

/// Files on this machine's disk, under `<data root>/files`.
#[derive(Clone, Debug)]
pub struct LocalDisk {
    root: PathBuf,
}

/// Bytes read back from the store.
pub struct StoredBytes {
    pub body: Body,
    pub length: u64,
}

impl FileStore {
    pub fn from_config(config: &AppConfig) -> Self {
        Self::Local(LocalDisk {
            root: config.data_root.join(LOCAL_FOLDER),
        })
    }

    /// Stores bytes under a key that isn't in use yet.
    pub async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        match self {
            Self::Local(disk) => disk.put(key, bytes).await,
        }
    }

    pub async fn get(&self, key: &str) -> Result<StoredBytes, AppError> {
        match self {
            Self::Local(disk) => disk.get(key).await,
        }
    }

    pub async fn read_all(&self, key: &str, maximum: u64) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Local(disk) => disk.read_all(key, maximum).await,
        }
    }

    /// Deletes a key. Deleting one that is already gone succeeds.
    pub async fn delete(&self, key: &str) -> Result<(), AppError> {
        match self {
            Self::Local(disk) => disk.delete(key).await,
        }
    }
}

/// Keys are written by the server alone; this refuses anything that could
/// step outside the store's folder anyway.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 200
        && key.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'a'..=b'z'))
        })
}

impl LocalDisk {
    fn path(&self, key: &str) -> Result<PathBuf, AppError> {
        if !valid_key(key) {
            return Err(AppError::Internal("invalid file store key"));
        }
        Ok(self.root.join(key))
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        let path = self.path(key)?;
        let parent = path
            .parent()
            .ok_or(AppError::Internal("file store key has no folder"))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| AppError::Internal("create file store folder"))?;
        // Write to a temporary name, then rename, so a crash never leaves a
        // half-written file under the real key.
        let partial = partial_path(&path);
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
            tokio::fs::rename(&partial, &path).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(AppError::Internal("write file to the file store"));
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<StoredBytes, AppError> {
        let path = self.path(key)?;
        let file = tokio::fs::File::open(&path).await.map_err(not_found)?;
        let length = file
            .metadata()
            .await
            .map_err(|_| AppError::Internal("read file store metadata"))?
            .len();
        Ok(StoredBytes {
            body: Body::from_stream(tokio_util::io::ReaderStream::new(file)),
            length,
        })
    }

    async fn read_all(&self, key: &str, maximum: u64) -> Result<Vec<u8>, AppError> {
        let path = self.path(key)?;
        let length = tokio::fs::metadata(&path).await.map_err(not_found)?.len();
        if length > maximum {
            return Err(AppError::PayloadTooLarge);
        }
        tokio::fs::read(&path).await.map_err(not_found)
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        let path = self.path(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AppError::Internal("delete file from the file store")),
        }
    }
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".partial-{}", uuid::Uuid::now_v7().simple()));
    path.with_file_name(name)
}

fn not_found(error: std::io::Error) -> AppError {
    if error.kind() == ErrorKind::NotFound {
        AppError::NotFound("file")
    } else {
        AppError::Internal("read file from the file store")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn keys_cannot_leave_the_store() {
        assert!(valid_key("0192/0192aa-bb/original"));
        assert!(valid_key("ab/cd.thumb"));
        assert!(!valid_key("../etc/passwd"));
        assert!(!valid_key("a//b"));
        assert!(!valid_key("/abs"));
        assert!(!valid_key("a/./b"));
        assert!(!valid_key("A/B"));
    }

    #[tokio::test]
    async fn local_disk_round_trips_and_deletes() {
        let root = TempDir::new().expect("root");
        let store = FileStore::Local(LocalDisk {
            root: root.path().to_path_buf(),
        });
        store.put("aa/bb", b"hello").await.expect("put");
        assert_eq!(store.read_all("aa/bb", 10).await.expect("read"), b"hello");
        assert!(matches!(
            store.read_all("aa/bb", 2).await,
            Err(AppError::PayloadTooLarge)
        ));
        store.delete("aa/bb").await.expect("delete");
        store.delete("aa/bb").await.expect("delete twice");
        assert!(matches!(
            store.read_all("aa/bb", 10).await,
            Err(AppError::NotFound(_))
        ));
    }
}
