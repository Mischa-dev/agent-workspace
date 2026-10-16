//! S1: encrypted secrets.
//!
//! Anything the server must use later (provider API keys, webhook signing
//! secrets, push keys) is stored with envelope encryption: each secret gets
//! its own random data key, the value is sealed with AES-256-GCM under that
//! key, and the data key is sealed under the master key. Both seals bind the
//! secret's id and workspace as associated data, so a row copied onto
//! another secret doesn't open. The master key comes from a [`KeyEncryption`]
//! implementation: a keyring file today ([`FileKeyring`]), a key service
//! later, without touching callers.
//!
//! Secrets are write-only. The API returns a secret's name, its last four
//! characters, who added it and when it was last used, never the value.
//! The value comes back only through [`reveal`], which takes a `purpose`
//! for the log line it writes (the purpose, never the value), and returns a
//! [`SecretValue`] whose `Debug` is redacted and which isn't `Serialize`.
//! Pass it only to the adapter call that needs it; never into a child
//! process environment, a log, an event payload or a database row.
//!
//! The keyring file holds one key per line, `aw-secrets-v1 <id> <base64>`,
//! the first being the one new secrets use. `secret-key-rotate` adds a new
//! first key, re-seals every data key under it, and drops keys nothing uses.

use std::{
    fmt,
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, PgConnection, PgExecutor, PgPool};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    config::AppConfig,
    error::{ApiJson, ApiPath, AppError},
};

const KEYRING_HEADER: &str = "aw-secrets-v1";
/// The most a secret may hold.
pub const MAX_SECRET_BYTES: usize = 64 * 1024;
/// Secret writes hold this advisory lock shared; key rotation takes it
/// exclusively while it retires old master keys.
const ROTATION_LOCK: (i32, i32) = (470127786, 2);
/// Held for a whole rotation, so two rotations never interleave.
const ONE_ROTATION_LOCK: (i32, i32) = (470127786, 3);

// ---------------------------------------------------------------------------
// Values

/// A secret's plaintext. Its `Debug` is redacted, it isn't `Serialize`, and
/// its memory is wiped when dropped.
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// The plaintext, for the one call that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The last four characters people see, or nothing for short values
    /// (where four characters would give too much away).
    fn last_four(&self) -> String {
        let count = self.0.chars().count();
        if count < 12 {
            return String::new();
        }
        self.0.chars().skip(count - 4).collect()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

// ---------------------------------------------------------------------------
// Master keys

/// A data key sealed under a master key.
#[derive(Clone, Debug)]
pub struct WrappedKey {
    pub master_key_id: String,
    /// Empty when the key service carries its own nonce.
    pub nonce: Option<[u8; NONCE_LEN]>,
    pub ciphertext: Vec<u8>,
}

/// What seals and opens data keys. A file keyring now; a key service
/// (a SaaS would use one) implements the same three calls.
pub trait KeyEncryption: Send + Sync {
    /// The key new secrets are sealed with.
    fn active_key_id(&self) -> &str;
    fn wrap(&self, data_key: &[u8; 32], aad: &[u8]) -> Result<WrappedKey, SecretError>;
    fn unwrap(&self, wrapped: &WrappedKey, aad: &[u8]) -> Result<Zeroizing<[u8; 32]>, SecretError>;
    /// Whether this key id can still be opened.
    fn has_key(&self, master_key_id: &str) -> bool;
}

#[derive(Debug, thiserror::Error, Clone, Copy, Eq, PartialEq)]
pub enum SecretError {
    #[error("encrypted secrets aren't set up on this server")]
    NotConfigured,
    #[error("the secrets keyring can't be read")]
    KeyringUnreadable,
    #[error("the secret was sealed with a master key the keyring no longer has")]
    UnknownMasterKey,
    #[error("the secret couldn't be opened")]
    Corrupt,
}

impl From<SecretError> for AppError {
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::NotConfigured => AppError::SecretsNotConfigured,
            SecretError::KeyringUnreadable => AppError::Internal("secrets keyring"),
            SecretError::UnknownMasterKey => AppError::Internal("secret master key"),
            SecretError::Corrupt => AppError::Internal("secret ciphertext"),
        }
    }
}

/// Master keys read from an owner-only file.
pub struct FileKeyring {
    keys: Vec<(String, Zeroizing<[u8; 32]>)>,
}

impl FileKeyring {
    pub fn parse(contents: &str) -> Result<Self, SecretError> {
        let mut keys = Vec::new();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(KEYRING_HEADER), Some(id), Some(encoded), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(SecretError::KeyringUnreadable);
            };
            if !valid_key_id(id) || keys.iter().any(|(existing, _)| existing == id) {
                return Err(SecretError::KeyringUnreadable);
            }
            let mut decoded = Zeroizing::new(
                STANDARD
                    .decode(encoded)
                    .map_err(|_| SecretError::KeyringUnreadable)?,
            );
            let key: [u8; 32] = decoded
                .as_slice()
                .try_into()
                .map_err(|_| SecretError::KeyringUnreadable)?;
            decoded.zeroize();
            keys.push((id.to_owned(), Zeroizing::new(key)));
        }
        if keys.is_empty() {
            return Err(SecretError::KeyringUnreadable);
        }
        Ok(Self { keys })
    }

    /// A keyring with one new random key.
    pub fn generate() -> Self {
        Self {
            keys: vec![(new_key_id(), random_key())],
        }
    }

    /// The same keyring with a new active key in front.
    pub fn with_new_active_key(&self) -> Self {
        let mut keys = vec![(new_key_id(), random_key())];
        keys.extend(
            self.keys
                .iter()
                .map(|(id, key)| (id.clone(), Zeroizing::new(**key))),
        );
        Self { keys }
    }

    /// Only the keys in `keep`, in order; the active key always stays.
    pub fn retain(&self, keep: &[String]) -> Self {
        Self {
            keys: self
                .keys
                .iter()
                .enumerate()
                .filter(|(index, (id, _))| *index == 0 || keep.contains(id))
                .map(|(_, (id, key))| (id.clone(), Zeroizing::new(**key)))
                .collect(),
        }
    }

    pub fn key_ids(&self) -> Vec<String> {
        self.keys.iter().map(|(id, _)| id.clone()).collect()
    }

    /// The file's contents. Handle as secret material.
    pub fn serialize(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::from(
            "# Agent Workspace secrets keyring. The first key seals new secrets.\n\
             # Back this file up apart from database dumps; never commit it.\n",
        ));
        for (id, key) in &self.keys {
            out.push_str(KEYRING_HEADER);
            out.push(' ');
            out.push_str(id);
            out.push(' ');
            out.push_str(&STANDARD.encode(key.as_slice()));
            out.push('\n');
        }
        out
    }

    fn key(&self, id: &str) -> Option<&[u8; 32]> {
        self.keys
            .iter()
            .find(|(existing, _)| existing == id)
            .map(|(_, key)| &**key)
    }
}

impl KeyEncryption for FileKeyring {
    fn active_key_id(&self) -> &str {
        &self.keys[0].0
    }

    fn wrap(&self, data_key: &[u8; 32], aad: &[u8]) -> Result<WrappedKey, SecretError> {
        let (id, master) = &self.keys[0];
        let (nonce, ciphertext) = seal(master, aad, data_key)?;
        Ok(WrappedKey {
            master_key_id: id.clone(),
            nonce: Some(nonce),
            ciphertext,
        })
    }

    fn unwrap(&self, wrapped: &WrappedKey, aad: &[u8]) -> Result<Zeroizing<[u8; 32]>, SecretError> {
        let master = self
            .key(&wrapped.master_key_id)
            .ok_or(SecretError::UnknownMasterKey)?;
        let nonce = wrapped.nonce.ok_or(SecretError::Corrupt)?;
        let opened = open(master, &nonce, aad, &wrapped.ciphertext)?;
        let key: [u8; 32] = opened
            .as_slice()
            .try_into()
            .map_err(|_| SecretError::Corrupt)?;
        Ok(Zeroizing::new(key))
    }

    fn has_key(&self, master_key_id: &str) -> bool {
        self.key(master_key_id).is_some()
    }
}

fn valid_key_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn new_key_id() -> String {
    format!("k{}", Utc::now().format("%Y%m%d%H%M%S"))
        + &format!("-{:04x}", OsRng.next_u32() & 0xffff)
}

fn random_key() -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0_u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    key
}

fn seal(
    key: &[u8; 32],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), SecretError> {
    let sealing =
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| SecretError::Corrupt)?);
    let mut nonce = [0_u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let mut in_out = plaintext.to_vec();
    sealing
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut in_out,
        )
        .map_err(|_| SecretError::Corrupt)?;
    Ok((nonce, in_out))
}

fn open(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    let opening =
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| SecretError::Corrupt)?);
    let mut in_out = Zeroizing::new(ciphertext.to_vec());
    let length = opening
        .open_in_place(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(aad),
            &mut in_out,
        )
        .map_err(|_| SecretError::Corrupt)?
        .len();
    in_out.truncate(length);
    Ok(in_out)
}

fn aad(secret_id: Uuid, organization_id: Uuid) -> Vec<u8> {
    format!("aw-secret:v1:{secret_id}:{organization_id}").into_bytes()
}

/// The keyring this server uses: the configured file, read again when it
/// changes; in development without one, a keyring created on first use.
pub fn keyring(config: &AppConfig) -> Result<Arc<dyn KeyEncryption>, SecretError> {
    let path = match &config.secrets_key_path {
        Some(path) => path.clone(),
        None if config.environment.is_development() => development_keyring_path()?,
        None => return Err(SecretError::NotConfigured),
    };
    load_cached(&path)
}

fn development_keyring_path() -> Result<PathBuf, SecretError> {
    let directory = std::env::current_dir()
        .map_err(|_| SecretError::KeyringUnreadable)?
        .join(".agent-workspace-secrets");
    let path = directory.join("dev.keyring");
    if !path.exists() {
        std::fs::create_dir_all(&directory).map_err(|_| SecretError::KeyringUnreadable)?;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
        // Losing a race to another process is fine: keep whichever file won.
        match write_new_keyring(&path, &FileKeyring::generate()) {
            Ok(()) => {}
            Err(_) if path.exists() => {}
            Err(_) => return Err(SecretError::KeyringUnreadable),
        }
    }
    Ok(path)
}

type CachedKeyring = (PathBuf, SystemTime, Arc<FileKeyring>);

fn load_cached(path: &Path) -> Result<Arc<dyn KeyEncryption>, SecretError> {
    static CACHE: OnceLock<Mutex<Option<CachedKeyring>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|_| SecretError::KeyringUnreadable)?;
    let mut cache = cache.lock().map_err(|_| SecretError::KeyringUnreadable)?;
    if let Some((cached_path, cached_modified, keyring)) = cache.as_ref()
        && cached_path == path
        && *cached_modified == modified
    {
        return Ok(keyring.clone());
    }
    let keyring = Arc::new(read_keyring(path)?);
    *cache = Some((path.to_owned(), modified, keyring.clone()));
    Ok(keyring)
}

pub fn read_keyring(path: &Path) -> Result<FileKeyring, SecretError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SecretError::KeyringUnreadable)?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(SecretError::KeyringUnreadable);
    }
    let contents =
        Zeroizing::new(std::fs::read_to_string(path).map_err(|_| SecretError::KeyringUnreadable)?);
    FileKeyring::parse(&contents)
}

/// Write a keyring to a file that must not exist yet (owner-only).
pub fn write_new_keyring(path: &Path, keyring: &FileKeyring) -> std::io::Result<()> {
    // Write beside it and hard-link into place, so a reader never sees a
    // half-written file and an existing keyring is never replaced.
    let temporary = path.with_extension(format!("partial-{}", Uuid::now_v7().simple()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(keyring.serialize().as_bytes())?;
        file.sync_all()?;
        std::fs::hard_link(&temporary, path)?;
        std::fs::remove_file(&temporary)?;
        sync_parent(path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Make a rename or link in `path`'s directory survive a power loss: the
/// database must never hold data keys under a master key the file lost.
fn sync_parent(path: &Path) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::File::open(parent)?.sync_all()
}

/// Replace an existing keyring file atomically (owner-only).
pub fn replace_keyring(path: &Path, keyring: &FileKeyring) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("partial-{}", Uuid::now_v7().simple()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(keyring.serialize().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

// ---------------------------------------------------------------------------
// The store

/// Whose secret it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretOwner {
    /// The workspace's own, such as its Anthropic key.
    Workspace { organization_id: Uuid },
    /// One project's.
    Project {
        organization_id: Uuid,
        project_id: Uuid,
    },
    /// One person's (or agent's) within a workspace.
    Actor {
        organization_id: Uuid,
        actor_id: Uuid,
    },
}

impl SecretOwner {
    pub fn organization_id(self) -> Uuid {
        match self {
            Self::Workspace { organization_id }
            | Self::Project {
                organization_id, ..
            }
            | Self::Actor {
                organization_id, ..
            } => organization_id,
        }
    }

    fn project_id(self) -> Option<Uuid> {
        match self {
            Self::Project { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    fn actor_id(self) -> Option<Uuid> {
        match self {
            Self::Actor { actor_id, .. } => Some(actor_id),
            _ => None,
        }
    }
}

/// What anyone may see about a secret.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretMetadata {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    pub owner_actor_id: Option<Uuid>,
    /// The last four characters, or empty for short values.
    pub last_four: String,
    pub created_by_actor_id: Option<Uuid>,
    pub created_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub version: i64,
}

const METADATA_COLUMNS: &str = r#"
    secret.id, secret.name, secret.description, secret.organization_id,
    secret.project_id, secret.owner_actor_id, secret.last_four,
    secret.created_by_actor_id,
    COALESCE(
        (SELECT display_name FROM organization_actor_profiles
         WHERE organization_id = secret.organization_id
           AND actor_id = secret.created_by_actor_id),
        (SELECT display_name FROM users WHERE actor_id = secret.created_by_actor_id)
    ) AS created_by_name,
    secret.created_at, secret.updated_at, secret.last_used_at, secret.version
"#;

/// A secret to store.
pub struct NewSecret<'a> {
    pub owner: SecretOwner,
    /// Lower case, such as `anthropic_api_key` or `webhook:<id>`.
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub value: &'a SecretValue,
    /// Who set it; `None` for the server itself.
    pub actor_id: Option<Uuid>,
    /// Made by the server for its own use, and left out of listings.
    pub system_managed: bool,
}

pub fn validate_name(name: &str) -> Result<(), AppError> {
    let mut characters = name.chars();
    if !(1..=120).contains(&name.len())
        || !characters.next().is_some_and(|c| c.is_ascii_lowercase())
        || !characters.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | ':' | '-')
        })
    {
        return Err(AppError::Validation("secret name"));
    }
    Ok(())
}

/// Store a secret, replacing the value of one with the same owner and name
/// (with a new data key). Returns what may be shown about it.
pub async fn put(
    connection: &mut PgConnection,
    keys: &dyn KeyEncryption,
    secret: NewSecret<'_>,
) -> Result<SecretMetadata, AppError> {
    validate_name(secret.name)?;
    if secret.value.expose().is_empty() || secret.value.expose().len() > MAX_SECRET_BYTES {
        return Err(AppError::Validation("secret value"));
    }
    if secret
        .description
        .is_some_and(|description| description.chars().count() > 500)
    {
        return Err(AppError::Validation("secret description"));
    }
    let mut transaction = connection.begin().await?;
    let connection = &mut *transaction;
    // Hold off a rotation that's retiring keys until this write commits, and
    // refuse a write sealed under a key a finished rotation already retired
    // (this server read the keyring before it changed; a retry reloads it).
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1, $2)")
        .bind(ROTATION_LOCK.0)
        .bind(ROTATION_LOCK.1)
        .execute(&mut *connection)
        .await?;
    let retired = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM secret_master_keys_retired WHERE master_key_id = $1)",
    )
    .bind(keys.active_key_id())
    .fetch_one(&mut *connection)
    .await?;
    if retired {
        return Err(AppError::Conflict("secret", "master key rotated"));
    }
    let organization_id = secret.owner.organization_id();
    let existing = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM secrets
        WHERE organization_id = $1
          AND project_id IS NOT DISTINCT FROM $2
          AND owner_actor_id IS NOT DISTINCT FROM $3
          AND name = $4
        FOR UPDATE
        "#,
    )
    .bind(organization_id)
    .bind(secret.owner.project_id())
    .bind(secret.owner.actor_id())
    .bind(secret.name)
    .fetch_optional(&mut *connection)
    .await?;
    let id = existing.unwrap_or_else(Uuid::now_v7);
    let sealed = seal_value(keys, id, organization_id, secret.value)?;
    let last_four = secret.value.last_four();
    if existing.is_some() {
        sqlx::query(
            r#"
            UPDATE secrets
            SET ciphertext = $2, nonce = $3, wrapped_key = $4, wrap_nonce = $5,
                master_key_id = $6, last_four = $7,
                description = COALESCE($8, description),
                updated_by_actor_id = $9, updated_at = now(), version = version + 1
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&sealed.ciphertext)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.wrapped.ciphertext)
        .bind(sealed.wrapped.nonce.as_ref().map(|nonce| nonce.as_slice()))
        .bind(&sealed.wrapped.master_key_id)
        .bind(&last_four)
        .bind(secret.description)
        .bind(secret.actor_id)
        .execute(&mut *connection)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO secrets (
                id, organization_id, project_id, owner_actor_id, system_managed, name,
                description, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id,
                last_four, created_by_actor_id, updated_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)
            "#,
        )
        .bind(id)
        .bind(organization_id)
        .bind(secret.owner.project_id())
        .bind(secret.owner.actor_id())
        .bind(secret.system_managed)
        .bind(secret.name)
        .bind(secret.description)
        .bind(&sealed.ciphertext)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.wrapped.ciphertext)
        .bind(sealed.wrapped.nonce.as_ref().map(|nonce| nonce.as_slice()))
        .bind(&sealed.wrapped.master_key_id)
        .bind(&last_four)
        .bind(secret.actor_id)
        .execute(&mut *connection)
        .await?;
    }
    let stored = metadata(&mut *connection, id)
        .await?
        .ok_or(AppError::Internal("stored secret"))?;
    transaction.commit().await?;
    Ok(stored)
}

struct Sealed {
    ciphertext: Vec<u8>,
    nonce: [u8; NONCE_LEN],
    wrapped: WrappedKey,
}

fn seal_value(
    keys: &dyn KeyEncryption,
    id: Uuid,
    organization_id: Uuid,
    value: &SecretValue,
) -> Result<Sealed, SecretError> {
    let associated = aad(id, organization_id);
    let data_key = random_key();
    let (nonce, ciphertext) = seal(&data_key, &associated, value.expose().as_bytes())?;
    let wrapped = keys.wrap(&data_key, &associated)?;
    Ok(Sealed {
        ciphertext,
        nonce,
        wrapped,
    })
}

#[derive(FromRow)]
struct StoredSecret {
    id: Uuid,
    organization_id: Uuid,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_key: Vec<u8>,
    wrap_nonce: Option<Vec<u8>>,
    master_key_id: String,
}

fn open_stored(
    keys: &dyn KeyEncryption,
    stored: &StoredSecret,
) -> Result<SecretValue, SecretError> {
    let associated = aad(stored.id, stored.organization_id);
    let wrapped = WrappedKey {
        master_key_id: stored.master_key_id.clone(),
        nonce: stored
            .wrap_nonce
            .as_deref()
            .map(|nonce| nonce.try_into().map_err(|_| SecretError::Corrupt))
            .transpose()?,
        ciphertext: stored.wrapped_key.clone(),
    };
    let data_key = keys.unwrap(&wrapped, &associated)?;
    let nonce: [u8; NONCE_LEN] = stored
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| SecretError::Corrupt)?;
    let plaintext = open(&data_key, &nonce, &associated, &stored.ciphertext)?;
    let text = std::str::from_utf8(&plaintext).map_err(|_| SecretError::Corrupt)?;
    Ok(SecretValue::new(text))
}

/// Decrypt a secret for the one call that needs it, and note that it was
/// used. `purpose` (such as `"webhook.sign"`) goes to the log; the value
/// never does.
/// Open a secret for one use. Takes any executor, so a caller already inside
/// a transaction reveals through it instead of waiting for a second
/// connection.
pub async fn reveal<'e, E: PgExecutor<'e>>(
    executor: E,
    keys: &dyn KeyEncryption,
    secret_id: Uuid,
    purpose: &'static str,
) -> Result<SecretValue, AppError> {
    let stored = sqlx::query_as::<_, StoredSecret>(
        r#"
        UPDATE secrets SET last_used_at = now()
        WHERE id = $1
        RETURNING id, organization_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id
        "#,
    )
    .bind(secret_id)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("secret"))?;
    let value = open_stored(keys, &stored)?;
    tracing::debug!(secret_id = %secret_id, purpose, "secret used");
    Ok(value)
}

/// [`reveal`] by owner and name; `None` when there is no such secret.
pub async fn reveal_named(
    pool: &PgPool,
    keys: &dyn KeyEncryption,
    owner: SecretOwner,
    name: &str,
    purpose: &'static str,
) -> Result<Option<SecretValue>, AppError> {
    let Some(id) = find(pool, owner, name).await? else {
        return Ok(None);
    };
    reveal(pool, keys, id, purpose).await.map(Some)
}

/// The id of the secret with this owner and name.
pub async fn find<'e, E: PgExecutor<'e>>(
    executor: E,
    owner: SecretOwner,
    name: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM secrets
        WHERE organization_id = $1
          AND project_id IS NOT DISTINCT FROM $2
          AND owner_actor_id IS NOT DISTINCT FROM $3
          AND name = $4
        "#,
    )
    .bind(owner.organization_id())
    .bind(owner.project_id())
    .bind(owner.actor_id())
    .bind(name)
    .fetch_optional(executor)
    .await?)
}

pub async fn metadata<'e, E: PgExecutor<'e>>(
    executor: E,
    secret_id: Uuid,
) -> Result<Option<SecretMetadata>, AppError> {
    Ok(sqlx::query_as::<_, SecretMetadata>(&format!(
        "SELECT {METADATA_COLUMNS} FROM secrets secret WHERE secret.id = $1"
    ))
    .bind(secret_id)
    .fetch_optional(executor)
    .await?)
}

/// The secrets people added for this owner, by name.
pub async fn list<'e, E: PgExecutor<'e>>(
    executor: E,
    owner: SecretOwner,
) -> Result<Vec<SecretMetadata>, AppError> {
    Ok(sqlx::query_as::<_, SecretMetadata>(&format!(
        r#"
        SELECT {METADATA_COLUMNS} FROM secrets secret
        WHERE secret.organization_id = $1
          AND secret.project_id IS NOT DISTINCT FROM $2
          AND secret.owner_actor_id IS NOT DISTINCT FROM $3
          AND NOT secret.system_managed
        ORDER BY secret.name
        "#
    ))
    .bind(owner.organization_id())
    .bind(owner.project_id())
    .bind(owner.actor_id())
    .fetch_all(executor)
    .await?)
}

/// Delete a secret by id. Returns whether it existed.
pub async fn delete<'e, E: PgExecutor<'e>>(executor: E, secret_id: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(secret_id)
        .execute(executor)
        .await?
        .rows_affected()
        == 1)
}

/// Re-seal every data key under `keys`' active master key. Values and their
/// data keys don't change, so this is safe to run while the server serves.
pub async fn rewrap_all(pool: &PgPool, keys: &dyn KeyEncryption) -> Result<u64, AppError> {
    let mut rewrapped = 0;
    loop {
        let mut transaction = pool.begin().await?;
        let batch = sqlx::query_as::<_, StoredSecret>(
            r#"
            SELECT id, organization_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id
            FROM secrets WHERE master_key_id <> $1
            ORDER BY id LIMIT 200
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(keys.active_key_id())
        .fetch_all(&mut *transaction)
        .await?;
        if batch.is_empty() {
            transaction.commit().await?;
            return Ok(rewrapped);
        }
        rewrapped += rewrap_rows(&mut transaction, keys, &batch).await?;
        transaction.commit().await?;
    }
}

async fn rewrap_rows(
    connection: &mut PgConnection,
    keys: &dyn KeyEncryption,
    rows: &[StoredSecret],
) -> Result<u64, AppError> {
    for stored in rows {
        let associated = aad(stored.id, stored.organization_id);
        let wrapped = WrappedKey {
            master_key_id: stored.master_key_id.clone(),
            nonce: stored
                .wrap_nonce
                .as_deref()
                .map(|nonce| nonce.try_into().map_err(|_| SecretError::Corrupt))
                .transpose()?,
            ciphertext: stored.wrapped_key.clone(),
        };
        let data_key = keys.unwrap(&wrapped, &associated)?;
        let rewrapped_key = keys.wrap(&data_key, &associated)?;
        sqlx::query(
            "UPDATE secrets SET wrapped_key = $2, wrap_nonce = $3, master_key_id = $4 WHERE id = $1",
        )
        .bind(stored.id)
        .bind(&rewrapped_key.ciphertext)
        .bind(rewrapped_key.nonce.as_ref().map(|nonce| nonce.as_slice()))
        .bind(&rewrapped_key.master_key_id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(rows.len() as u64)
}

/// The master key ids secrets still use.
pub async fn master_keys_in_use(pool: &PgPool) -> Result<Vec<String>, AppError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT DISTINCT master_key_id FROM secrets ORDER BY 1")
            .fetch_all(pool)
            .await?,
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationResult {
    pub active_key_id: String,
    pub rewrapped_secrets: u64,
    pub retired_key_ids: Vec<String>,
}

/// `secret-key-rotate`: put a new master key first in the keyring file,
/// re-seal every data key under it, then drop keys nothing uses. Safe to
/// re-run after an interruption.
pub async fn rotate_master_key(
    pool: &PgPool,
    config: &AppConfig,
) -> Result<RotationResult, AppError> {
    let path = config
        .secrets_key_path
        .clone()
        .ok_or(AppError::SecretsNotConfigured)?;
    // On a connection of its own, outside the pool, so a pool of one still
    // has room for the rotation's work; closing it releases the lock.
    let mut one_rotation = PgConnection::connect_with(&pool.connect_options()).await?;
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(ONE_ROTATION_LOCK.0)
        .bind(ONE_ROTATION_LOCK.1)
        .execute(&mut one_rotation)
        .await?;
    let current = read_keyring(&path)?;
    let rotated = current.with_new_active_key();
    replace_keyring(&path, &rotated).map_err(|_| AppError::Internal("write secrets keyring"))?;
    let mut rewrapped_secrets = rewrap_all(pool, &rotated).await?;
    // Writers that read the keyring before it changed may still be sealing
    // under an old key. Wait for them, re-seal what they stored, and mark
    // the old keys retired so a later write under one is refused.
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(ROTATION_LOCK.0)
        .bind(ROTATION_LOCK.1)
        .execute(&mut *transaction)
        .await?;
    let stragglers = sqlx::query_as::<_, StoredSecret>(
        r#"
        SELECT id, organization_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id
        FROM secrets WHERE master_key_id <> $1
        FOR UPDATE
        "#,
    )
    .bind(rotated.active_key_id())
    .fetch_all(&mut *transaction)
    .await?;
    rewrapped_secrets += rewrap_rows(&mut transaction, &rotated, &stragglers).await?;
    let in_use =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT master_key_id FROM secrets ORDER BY 1")
            .fetch_all(&mut *transaction)
            .await?;
    let trimmed = rotated.retain(&in_use);
    let retired_key_ids = rotated
        .key_ids()
        .into_iter()
        .filter(|id| !trimmed.key_ids().contains(id))
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO secret_master_keys_retired (master_key_id)
        SELECT unnest($1::text[])
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(&retired_key_ids)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    replace_keyring(&path, &trimmed).map_err(|_| AppError::Internal("write secrets keyring"))?;
    let _ = one_rotation.close().await;
    Ok(RotationResult {
        active_key_id: trimmed.active_key_id().to_owned(),
        rewrapped_secrets,
        retired_key_ids,
    })
}

// ---------------------------------------------------------------------------
// Webhook signing secrets (moved out of external_agent_connections)

/// The store name of a connection's wakeup signing secret.
pub fn webhook_secret_name(connection_id: Uuid) -> String {
    format!("webhook:{connection_id}")
}

/// Move plaintext webhook signing secrets left from before the store into
/// it, and clear the old column. Runs at startup; does nothing without a
/// keyring, so plaintext stays usable until one is configured.
pub async fn migrate_plaintext_webhook_secrets(
    pool: &PgPool,
    config: &AppConfig,
) -> Result<u64, AppError> {
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM external_agent_connections WHERE webhook_secret IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    if pending == 0 {
        return Ok(0);
    }
    let keys = match keyring(config) {
        Ok(keys) => keys,
        Err(SecretError::NotConfigured) => {
            tracing::warn!(
                pending,
                "agent webhook secrets are still stored in plaintext; set AGENT_WORKSPACE_SECRETS_KEY_PATH to encrypt them"
            );
            return Ok(0);
        }
        Err(error) => return Err(error.into()),
    };
    let mut moved = 0;
    loop {
        let mut transaction = pool.begin().await?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
            r#"
            SELECT connection.id, project.organization_id, connection.project_id,
                   connection.webhook_secret
            FROM external_agent_connections connection
            JOIN projects project ON project.id = connection.project_id
            WHERE connection.webhook_secret IS NOT NULL
            ORDER BY connection.id LIMIT 100
            FOR UPDATE OF connection SKIP LOCKED
            "#,
        )
        .fetch_all(&mut *transaction)
        .await?;
        if rows.is_empty() {
            transaction.commit().await?;
            return Ok(moved);
        }
        for (connection_id, organization_id, project_id, plaintext) in rows {
            let value = SecretValue::new(plaintext);
            let stored = put(
                &mut transaction,
                keys.as_ref(),
                NewSecret {
                    owner: SecretOwner::Project {
                        organization_id,
                        project_id,
                    },
                    name: &webhook_secret_name(connection_id),
                    description: Some("Signs wakeups sent to this agent's webhook."),
                    value: &value,
                    actor_id: None,
                    system_managed: true,
                },
            )
            .await?;
            sqlx::query(
                "UPDATE external_agent_connections SET webhook_secret = NULL, webhook_secret_id = $2 WHERE id = $1",
            )
            .bind(connection_id)
            .bind(stored.id)
            .execute(&mut *transaction)
            .await?;
            moved += 1;
        }
        transaction.commit().await?;
    }
}

/// Store a new signing secret for a connection's webhook and return its
/// id. The caller sets `webhook_secret_id` in the same transaction.
pub async fn put_webhook_secret(
    connection: &mut PgConnection,
    config: &AppConfig,
    connection_id: Uuid,
    value: &SecretValue,
) -> Result<Uuid, AppError> {
    let keys = keyring(config)?;
    let (organization_id, project_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT project.organization_id, project.id
        FROM external_agent_connections connection
        JOIN projects project ON project.id = connection.project_id
        WHERE connection.id = $1
        "#,
    )
    .bind(connection_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("external agent connection"))?;
    let stored = put(
        connection,
        keys.as_ref(),
        NewSecret {
            owner: SecretOwner::Project {
                organization_id,
                project_id,
            },
            name: &webhook_secret_name(connection_id),
            description: Some("Signs wakeups sent to this agent's webhook."),
            value,
            actor_id: None,
            system_managed: true,
        },
    )
    .await?;
    Ok(stored.id)
}

/// Delete a connection's webhook signing secret, if it has one. Call after
/// clearing `webhook_secret_id`.
pub async fn delete_webhook_secret(
    connection: &mut PgConnection,
    connection_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM secrets secret
        USING external_agent_connections connection, projects project
        WHERE connection.id = $1 AND project.id = connection.project_id
          AND secret.organization_id = project.organization_id
          AND secret.project_id = connection.project_id
          AND secret.owner_actor_id IS NULL
          AND secret.name = $2
          AND connection.webhook_secret_id IS DISTINCT FROM secret.id
        "#,
    )
    .bind(connection_id)
    .bind(webhook_secret_name(connection_id))
    .execute(connection)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP: people manage the workspace's, a project's, and their own secrets.

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/secrets",
            get(list_workspace),
        )
        .route(
            "/api/v1/organizations/{organization_id}/secrets/{name}",
            axum::routing::put(put_workspace).delete(delete_workspace),
        )
        .route(
            "/api/v1/organizations/{organization_id}/personal-secrets",
            get(list_personal),
        )
        .route(
            "/api/v1/organizations/{organization_id}/personal-secrets/{name}",
            axum::routing::put(put_personal).delete(delete_personal),
        )
        .route("/api/v1/projects/{project_id}/secrets", get(list_project))
        .route(
            "/api/v1/projects/{project_id}/secrets/{name}",
            axum::routing::put(put_project).delete(delete_project),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutSecretRequest {
    value: String,
    #[serde(default)]
    description: Option<String>,
}

impl fmt::Debug for PutSecretRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PutSecretRequest")
            .field("value", &"[REDACTED]")
            .field("description", &self.description)
            .finish()
    }
}

impl Drop for PutSecretRequest {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretListResponse {
    secrets: Vec<SecretMetadata>,
    /// Whether this server has a keyring, so new secrets can be saved.
    configured: bool,
}

async fn list_workspace(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<SecretListResponse>, AppError> {
    let owner =
        crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id)
            .await
            .map(|()| SecretOwner::Workspace { organization_id })?;
    respond_list(&state, owner).await
}

async fn list_project(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<SecretListResponse>, AppError> {
    let organization_id =
        crate::settings::authorize_project_manage(&state.pool, &principal, project_id).await?;
    respond_list(
        &state,
        SecretOwner::Project {
            organization_id,
            project_id,
        },
    )
    .await
}

async fn list_personal(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<SecretListResponse>, AppError> {
    crate::settings::authorize_personal(&state.pool, &principal, organization_id).await?;
    respond_list(
        &state,
        SecretOwner::Actor {
            organization_id,
            actor_id: principal.actor_id,
        },
    )
    .await
}

async fn respond_list(
    state: &AppState,
    owner: SecretOwner,
) -> Result<Json<SecretListResponse>, AppError> {
    Ok(Json(SecretListResponse {
        secrets: list(&state.pool, owner).await?,
        configured: keyring(&state.config).is_ok(),
    }))
}

async fn put_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, name)): ApiPath<(Uuid, String)>,
    ApiJson(request): ApiJson<PutSecretRequest>,
) -> Result<Json<SecretMetadata>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    store_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Workspace { organization_id },
        &name,
        request,
    )
    .await
}

async fn put_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, name)): ApiPath<(Uuid, String)>,
    ApiJson(request): ApiJson<PutSecretRequest>,
) -> Result<Json<SecretMetadata>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let organization_id =
        crate::settings::authorize_project_manage(&state.pool, &principal, project_id).await?;
    store_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Project {
            organization_id,
            project_id,
        },
        &name,
        request,
    )
    .await
}

async fn put_personal(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, name)): ApiPath<(Uuid, String)>,
    ApiJson(request): ApiJson<PutSecretRequest>,
) -> Result<Json<SecretMetadata>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    crate::settings::authorize_personal(&state.pool, &principal, organization_id).await?;
    store_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Actor {
            organization_id,
            actor_id: principal.actor_id,
        },
        &name,
        request,
    )
    .await
}

async fn store_from_request(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    owner: SecretOwner,
    name: &str,
    request: PutSecretRequest,
) -> Result<Json<SecretMetadata>, AppError> {
    validate_name(name)?;
    if name.starts_with("webhook:") {
        return Err(AppError::Validation("secret name"));
    }
    let keys = keyring(&state.config)?;
    let value = SecretValue::new(request.value.trim().to_owned());
    let description = request
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty());
    let mut transaction = state.pool.begin().await?;
    let stored = put(
        &mut transaction,
        keys.as_ref(),
        NewSecret {
            owner,
            name,
            description,
            value: &value,
            actor_id: Some(principal.actor_id),
            system_managed: false,
        },
    )
    .await?;
    crate::settings::audit(
        &mut transaction,
        headers,
        principal,
        owner.project_id(),
        "secret.saved",
        "secret",
        Some(stored.id),
        serde_json::json!({
            "organizationId": owner.organization_id(),
            "name": stored.name,
            "version": stored.version,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(stored))
}

async fn delete_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, name)): ApiPath<(Uuid, String)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    delete_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Workspace { organization_id },
        &name,
    )
    .await
}

async fn delete_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, name)): ApiPath<(Uuid, String)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let organization_id =
        crate::settings::authorize_project_manage(&state.pool, &principal, project_id).await?;
    delete_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Project {
            organization_id,
            project_id,
        },
        &name,
    )
    .await
}

async fn delete_personal(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, name)): ApiPath<(Uuid, String)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    crate::settings::authorize_personal(&state.pool, &principal, organization_id).await?;
    delete_from_request(
        &state,
        &headers,
        &principal,
        SecretOwner::Actor {
            organization_id,
            actor_id: principal.actor_id,
        },
        &name,
    )
    .await
}

async fn delete_from_request(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    owner: SecretOwner,
    name: &str,
) -> Result<StatusCode, AppError> {
    validate_name(name)?;
    let mut transaction = state.pool.begin().await?;
    let id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM secrets
        WHERE organization_id = $1
          AND project_id IS NOT DISTINCT FROM $2
          AND owner_actor_id IS NOT DISTINCT FROM $3
          AND name = $4 AND NOT system_managed
        FOR UPDATE
        "#,
    )
    .bind(owner.organization_id())
    .bind(owner.project_id())
    .bind(owner.actor_id())
    .bind(name)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("secret"))?;
    delete(&mut *transaction, id).await?;
    crate::settings::audit(
        &mut transaction,
        headers,
        principal,
        owner.project_id(),
        "secret.deleted",
        "secret",
        Some(id),
        serde_json::json!({ "organizationId": owner.organization_id(), "name": name }),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_bind_their_identity() {
        let keyring = FileKeyring::generate();
        let id = Uuid::now_v7();
        let organization = Uuid::now_v7();
        let value = SecretValue::new("sk-ant-api03-abcdefghijklmnop");
        let sealed = seal_value(&keyring, id, organization, &value).expect("seal");
        assert!(
            !sealed
                .ciphertext
                .windows(8)
                .any(|window| window == b"abcdefgh")
        );
        let stored = StoredSecret {
            id,
            organization_id: organization,
            ciphertext: sealed.ciphertext.clone(),
            nonce: sealed.nonce.to_vec(),
            wrapped_key: sealed.wrapped.ciphertext.clone(),
            wrap_nonce: sealed.wrapped.nonce.map(|nonce| nonce.to_vec()),
            master_key_id: sealed.wrapped.master_key_id.clone(),
        };
        assert_eq!(
            open_stored(&keyring, &stored).expect("open").expose(),
            value.expose()
        );
        // The same ciphertext on another secret's row doesn't open.
        let moved = StoredSecret {
            id: Uuid::now_v7(),
            ..stored
        };
        assert_eq!(
            open_stored(&keyring, &moved).map(|_| ()),
            Err(SecretError::Corrupt)
        );
    }

    #[test]
    fn keyrings_round_trip_and_rotate() {
        let first = FileKeyring::generate();
        let parsed = FileKeyring::parse(&first.serialize()).expect("parse");
        assert_eq!(parsed.key_ids(), first.key_ids());
        let rotated = first.with_new_active_key();
        assert_eq!(rotated.key_ids().len(), 2);
        assert_ne!(rotated.active_key_id(), first.active_key_id());
        let data_key = random_key();
        let wrapped = first.wrap(&data_key, b"aad").expect("wrap");
        // The rotated keyring still opens keys sealed with the old one.
        assert_eq!(
            *rotated.unwrap(&wrapped, b"aad").expect("unwrap"),
            *data_key
        );
        let trimmed = rotated.retain(&[]);
        assert_eq!(trimmed.key_ids(), vec![rotated.active_key_id().to_owned()]);
        assert!(FileKeyring::parse("aw-secrets-v1 k1 notbase64").is_err());
        assert!(FileKeyring::parse("# nothing\n").is_err());
    }

    #[test]
    fn secret_values_never_print() {
        let value = SecretValue::new("sk-ant-api03-supersecretvalue");
        assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
        assert_eq!(value.last_four(), "alue");
        assert_eq!(SecretValue::new("short").last_four(), "");
        let request = PutSecretRequest {
            value: "sk-ant-api03-supersecretvalue".to_owned(),
            description: None,
        };
        assert!(!format!("{request:?}").contains("supersecret"));
    }

    #[test]
    fn names_are_checked() {
        assert!(validate_name("anthropic_api_key").is_ok());
        assert!(validate_name("webhook:0199").is_ok());
        assert!(validate_name("Anthropic").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("1password").is_err());
    }
}
