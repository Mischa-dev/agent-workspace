//! Where file bytes live. One interface, so the store can change without
//! touching anything that uses it: local disk under the data root, or an
//! S3-compatible object store (K5.12, `object`). New uploads go to the
//! primary store (`AGENT_WORKSPACE_FILE_STORE`); each file row names the
//! store its bytes are in (`files.store_id`), and reads follow it. Keys are
//! opaque, made only of the workspace and file IDs, and never contain a
//! machine path.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use axum::body::{Body, Bytes};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{config::AppConfig, error::AppError};

/// The folder under the data root that holds uploaded files.
const LOCAL_FOLDER: &str = "files";

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // one per request, never in a collection
pub enum FileStore {
    Local(LocalDisk),
    Object(super::object::ObjectBucket),
}

/// The local store's id in `files.store_id`.
pub const LOCAL: &str = "local";
/// The S3-compatible store's.
pub const OBJECT: &str = "object";
/// A workspace's own bucket's id starts with this (K5.20).
pub const WORKSPACE_PREFIX: &str = "ws-";

/// Who reads what a store is asked to keep. A workspace's own bucket is
/// read with keys its managers hold, so it keeps only what the workspace
/// may read; a DM's or group's files, and archives that hold them, stay in
/// the server's store (`files.members_only` holds this in the database).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readers {
    /// Anyone the workspace lets read it.
    Workspace,
    /// A DM's or group's members only.
    Members,
}

/// Files on this machine's disk, under `<data root>/files`.
#[derive(Clone, Debug)]
pub struct LocalDisk {
    root: PathBuf,
}

/// What a streamed write stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub length: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// Bytes read back from the store.
pub struct StoredBytes {
    pub body: Body,
    pub length: u64,
}

/// Opens the store a row names by its id (a history payload's), for code
/// that reads rows kept in several stores: the configured store for new
/// writes, any other by id.
#[derive(Clone)]
pub struct Stores<'a> {
    pool: &'a sqlx::PgPool,
    config: &'a AppConfig,
    /// Moved payloads read before a transaction opened, by key. When set,
    /// nothing else is read from a store (`history::read_moved_payload`).
    warmed: Option<std::collections::HashMap<String, serde_json::Value>>,
}

impl<'a> Stores<'a> {
    pub fn of(state: &'a crate::AppState) -> Self {
        Self {
            pool: &state.pool,
            config: &state.config,
            warmed: None,
        }
    }

    /// Only these payloads, already read: for use inside a transaction.
    pub fn warmed(
        state: &'a crate::AppState,
        payloads: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            warmed: Some(payloads),
            ..Self::of(state)
        }
    }

    pub(crate) fn warmed_payloads(
        &self,
    ) -> Option<&std::collections::HashMap<String, serde_json::Value>> {
        self.warmed.as_ref()
    }

    /// Where new payloads go.
    pub fn configured(&self) -> FileStore {
        FileStore::from_config(self.config)
    }

    /// The store `id` names.
    pub async fn open(&self, id: &str) -> Result<FileStore, AppError> {
        FileStore::resolve_with(self.pool, self.config, id).await
    }
}

impl FileStore {
    /// Where new uploads go.
    pub fn from_config(config: &AppConfig) -> Self {
        match (config.file_store.as_str(), &config.object_store) {
            (OBJECT, Some(object)) => {
                Self::Object(super::object::ObjectBucket::new(object.clone()))
            }
            _ => Self::local(config),
        }
    }

    fn local(config: &AppConfig) -> Self {
        Self::Local(LocalDisk {
            root: config.data_root.join(LOCAL_FOLDER),
        })
    }

    /// The store a file row names, a workspace's own bucket included.
    pub async fn resolve(state: &crate::AppState, id: &str) -> Result<Self, AppError> {
        Self::resolve_with(&state.pool, &state.config, id).await
    }

    /// [`Self::resolve`] for commands that have no full app state.
    pub async fn resolve_with(
        pool: &sqlx::PgPool,
        config: &AppConfig,
        id: &str,
    ) -> Result<Self, AppError> {
        if id.starts_with(WORKSPACE_PREFIX) {
            return super::workspace_store::open(pool, config, id)
                .await
                .map(Self::Object);
        }
        Self::named(config, id)
    }

    /// Where a workspace's new uploads go: its own bucket when it has one
    /// (K5.20) and the workspace may read what's kept, otherwise the
    /// server's primary store. Read on each upload, never held per
    /// workspace.
    pub async fn for_uploads(
        state: &crate::AppState,
        connection: &mut sqlx::PgConnection,
        workspace: uuid::Uuid,
        readers: Readers,
    ) -> Result<Self, AppError> {
        if readers == Readers::Members {
            return Ok(Self::from_config(&state.config));
        }
        match super::workspace_store::active(&state.config, connection, workspace).await? {
            Some(bucket) => Ok(Self::Object(bucket)),
            None => Ok(Self::from_config(&state.config)),
        }
    }

    /// One of this server's configured stores (not a workspace's own).
    pub fn named(config: &AppConfig, id: &str) -> Result<Self, AppError> {
        match (id, &config.object_store) {
            (LOCAL, _) => Ok(Self::local(config)),
            (OBJECT, Some(object)) => Ok(Self::Object(super::object::ObjectBucket::new(
                object.clone(),
            ))),
            _ => Err(AppError::Internal("a file's store isn't configured")),
        }
    }

    /// Every store this server is configured with.
    pub fn all(config: &AppConfig) -> Vec<Self> {
        let mut stores = vec![Self::local(config)];
        if let Some(object) = &config.object_store {
            stores.push(Self::Object(super::object::ObjectBucket::new(
                object.clone(),
            )));
        }
        stores
    }

    /// The store's name, kept with each file (`files.store_id`) so files
    /// can move between stores (K5.13).
    pub fn id(&self) -> &str {
        match self {
            Self::Local(_) => LOCAL,
            Self::Object(bucket) => bucket.id(),
        }
    }

    /// A link a client uploads straight to (K5.20); None for disk, which
    /// keeps the upload route through the server.
    pub fn presign_put(
        &self,
        key: &str,
        ttl: std::time::Duration,
        content_type: &str,
        length: u64,
        sha256: &[u8; 32],
    ) -> Result<Option<super::object::PutLink>, AppError> {
        valid(key)?;
        match self {
            Self::Local(_) => Ok(None),
            Self::Object(bucket) => bucket
                .presign_put(key, ttl, content_type, length, sha256)
                .map(Some),
        }
    }

    /// What is stored under a key, or None.
    pub async fn head(&self, key: &str) -> Result<Option<super::object::Head>, AppError> {
        valid(key)?;
        match self {
            Self::Local(disk) => match tokio::fs::metadata(disk.path(key)?).await {
                Ok(metadata) => Ok(Some(super::object::Head {
                    length: metadata.len(),
                    sha256: None,
                })),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(_) => Err(AppError::Internal("read file store metadata")),
            },
            Self::Object(bucket) => bucket.head(key).await,
        }
    }

    /// The first `count` bytes under a key.
    pub async fn read_start(&self, key: &str, count: u64) -> Result<Vec<u8>, AppError> {
        valid(key)?;
        match self {
            Self::Local(disk) => {
                use tokio::io::AsyncReadExt;
                let file = tokio::fs::File::open(disk.path(key)?)
                    .await
                    .map_err(not_found)?;
                let mut bytes = Vec::new();
                file.take(count)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| AppError::Internal("read file from the file store"))?;
                Ok(bytes)
            }
            Self::Object(bucket) => bucket.read_start(key, count).await,
        }
    }

    /// A link the store itself serves the key from, for `ttl`; None for
    /// disk, which the app serves.
    pub fn presign_get(
        &self,
        key: &str,
        ttl: std::time::Duration,
        content_type: &str,
        disposition: &str,
    ) -> Result<Option<url::Url>, AppError> {
        if !valid_key(key) {
            return Err(AppError::Internal("invalid file store key"));
        }
        match self {
            Self::Local(_) => Ok(None),
            Self::Object(bucket) => bucket
                .presign_get(key, ttl, content_type, disposition)
                .map(Some),
        }
    }

    /// Streams bytes under a key that isn't in use yet, never holding more
    /// than one chunk; past `limit` bytes it stops, keeps nothing, and
    /// answers `PayloadTooLarge`.
    pub async fn put_stream<S, E>(
        &self,
        key: &str,
        chunks: S,
        limit: u64,
    ) -> Result<Written, AppError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin,
    {
        match self {
            Self::Local(disk) => disk.put_stream(key, chunks, limit).await,
            Self::Object(bucket) => {
                valid(key)?;
                bucket.put_stream(key, chunks, limit).await
            }
        }
    }

    /// Stores bytes under a key that isn't in use yet.
    pub async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        match self {
            Self::Local(disk) => disk.put(key, bytes).await,
            Self::Object(bucket) => {
                valid(key)?;
                bucket.put(key, bytes).await
            }
        }
    }

    pub async fn get(&self, key: &str) -> Result<StoredBytes, AppError> {
        match self {
            Self::Local(disk) => disk.get(key).await,
            Self::Object(bucket) => {
                valid(key)?;
                bucket.get(key).await
            }
        }
    }

    pub async fn read_all(&self, key: &str, maximum: u64) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Local(disk) => disk.read_all(key, maximum).await,
            Self::Object(bucket) => {
                valid(key)?;
                bucket.read_all(key, maximum).await
            }
        }
    }

    /// Deletes a key. Deleting one that is already gone succeeds.
    pub async fn delete(&self, key: &str) -> Result<(), AppError> {
        match self {
            Self::Local(disk) => disk.delete(key).await,
            Self::Object(bucket) => {
                valid(key)?;
                bucket.delete(key).await
            }
        }
    }

    /// Deletes every key of a workspace (keys start with its id, H1.24).
    /// Deleting a workspace with nothing stored succeeds.
    pub async fn delete_workspace(&self, workspace: uuid::Uuid) -> Result<(), AppError> {
        match self {
            Self::Local(disk) => {
                let folder = disk.root.join(workspace.simple().to_string());
                match tokio::fs::remove_dir_all(&folder).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(AppError::Internal("workspace files could not be deleted")),
                }
            }
            Self::Object(bucket) => bucket
                .delete_prefix(&format!("{}/", workspace.simple()))
                .await
                .map_err(|_| AppError::Internal("workspace files could not be deleted")),
        }
    }
}

fn valid(key: &str) -> Result<(), AppError> {
    if valid_key(key) {
        Ok(())
    } else {
        Err(AppError::Internal("invalid file store key"))
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

    async fn put_stream<S, E>(
        &self,
        key: &str,
        mut chunks: S,
        limit: u64,
    ) -> Result<Written, AppError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin,
    {
        let path = self.path(key)?;
        let parent = path
            .parent()
            .ok_or(AppError::Internal("file store key has no folder"))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| AppError::Internal("create file store folder"))?;
        let partial = partial_path(&path);
        let result: Result<Written, AppError> = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .await
                .map_err(|_| AppError::Internal("write file to the file store"))?;
            let mut hasher = Sha256::new();
            let mut length = 0_u64;
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| AppError::Validation("body"))?;
                length = length.saturating_add(chunk.len() as u64);
                if length > limit {
                    return Err(AppError::PayloadTooLarge);
                }
                hasher.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .map_err(|_| AppError::Internal("write file to the file store"))?;
            }
            file.sync_all()
                .await
                .map_err(|_| AppError::Internal("write file to the file store"))?;
            tokio::fs::rename(&partial, &path)
                .await
                .map_err(|_| AppError::Internal("write file to the file store"))?;
            Ok(Written {
                length,
                sha256: hex::encode(hasher.finalize()),
            })
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result
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
        // Streamed: kept whole within the limit, nothing kept past it.
        let chunks = || {
            futures_util::stream::iter(
                ["hel", "lo"].map(|piece| Ok::<_, std::io::Error>(Bytes::from(piece))),
            )
        };
        let written = store
            .put_stream("aa/cc", chunks(), 5)
            .await
            .expect("stream");
        assert_eq!(written.length, 5);
        assert_eq!(written.sha256, hex::encode(Sha256::digest(b"hello")));
        assert_eq!(store.read_all("aa/cc", 10).await.expect("read"), b"hello");
        assert!(matches!(
            store.put_stream("aa/dd", chunks(), 4).await,
            Err(AppError::PayloadTooLarge)
        ));
        assert!(matches!(
            store.read_all("aa/dd", 10).await,
            Err(AppError::NotFound(_))
        ));
        let folder = std::fs::read_dir(root.path().join("aa")).expect("folder");
        assert_eq!(folder.count(), 1, "no partial file is left behind");
        assert!(matches!(
            store.read_all("aa/bb", 10).await,
            Err(AppError::NotFound(_))
        ));
    }
}
