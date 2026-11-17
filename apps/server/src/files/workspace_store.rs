//! A workspace's own bucket (K5.20, ruling R23): an owner points the
//! workspace's new uploads at an S3-compatible bucket of its own. The
//! access keys are a secret in the keyring, opened only to sign a request;
//! the row holds where the bucket is and which secret has its keys. The
//! server checks a bucket with a put, read and delete of a probe object
//! before using it, reaching it only at a public address (the outbound
//! guard applies, unlike for the operator's own store).
//!
//! Setting another bucket, or none, retires the current one: files already
//! in it keep naming it (`ws-<id>`) and are read from it until the owner
//! moves them (`files-move`, one workspace at a time).

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, PgPool};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    object::ObjectBucket,
    store::{FileStore, WORKSPACE_PREFIX},
};
use crate::{
    AppState,
    config::{AppConfig, ObjectStoreConfig},
    error::AppError,
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

#[derive(Debug, FromRow)]
struct Row {
    id: Uuid,
    endpoint: String,
    bucket: String,
    region: String,
    prefix: String,
    path_style: bool,
    secret_id: Uuid,
    verified_at: chrono::DateTime<chrono::Utc>,
}

const COLUMNS: &str = "id, endpoint, bucket, region, prefix, path_style, \
                       secret_id, verified_at";

/// What anyone who manages the workspace's settings sees: never the keys.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStoreView {
    /// The id files in it are stored under (`ws-` and its id).
    pub store_id: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    pub path_style: bool,
    /// When the server last checked it could write, read and delete there.
    pub verified_at: chrono::DateTime<chrono::Utc>,
}

impl Row {
    fn view(&self) -> WorkspaceStoreView {
        WorkspaceStoreView {
            store_id: store_id(self.id),
            endpoint: self.endpoint.clone(),
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            prefix: self.prefix.clone(),
            path_style: self.path_style,
            verified_at: self.verified_at,
        }
    }
}

fn store_id(id: Uuid) -> String {
    format!("{WORKSPACE_PREFIX}{}", id.simple())
}

fn location(
    endpoint: &str,
    bucket: &str,
    region: &str,
    prefix: &str,
    path_style: bool,
) -> Result<ObjectStoreConfig, AppError> {
    Ok(ObjectStoreConfig {
        endpoint: Url::parse(endpoint)
            .map_err(|_| AppError::Internal("a workspace bucket's address"))?,
        bucket: bucket.to_owned(),
        region: region.to_owned(),
        prefix: prefix.to_owned(),
        path_style,
        // Unused: a workspace bucket's keys are held, not read from files.
        access_key_id_path: std::path::PathBuf::new(),
        secret_access_key_path: std::path::PathBuf::new(),
    })
}

/// The two keys, as one secret.
#[derive(serde::Deserialize, Serialize)]
struct Keys {
    access_key_id: String,
    secret_access_key: String,
}

fn secret_name(id: Uuid) -> String {
    format!("file_store:{}", id.simple())
}

async fn bucket_of(
    config: &AppConfig,
    connection: &mut PgConnection,
    row: &Row,
) -> Result<ObjectBucket, AppError> {
    let keyring = secrets::keyring(config)?;
    let value = secrets::reveal(
        &mut *connection,
        keyring.as_ref(),
        row.secret_id,
        "files.store",
    )
    .await?;
    let keys = serde_json::from_str::<Keys>(value.expose())
        .map_err(|_| AppError::Internal("a workspace bucket's keys"))?;
    Ok(ObjectBucket::workspace(
        store_id(row.id),
        location(
            &row.endpoint,
            &row.bucket,
            &row.region,
            &row.prefix,
            row.path_style,
        )?,
        keys.access_key_id,
        Zeroizing::new(keys.secret_access_key),
        config.allow_private_addresses,
    ))
}

/// The workspace's bucket new uploads go to, when it has one.
pub(crate) async fn active(
    config: &AppConfig,
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Option<ObjectBucket>, AppError> {
    let row = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM workspace_stores WHERE workspace_id = $1 AND retired_at IS NULL"
    ))
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?;
    match row {
        Some(row) => bucket_of(config, connection, &row).await.map(Some),
        None => Ok(None),
    }
}

/// The bucket a file row names (`ws-<id>`), retired or not.
pub(crate) async fn open(
    pool: &PgPool,
    config: &AppConfig,
    store: &str,
) -> Result<ObjectBucket, AppError> {
    let id = store
        .strip_prefix(WORKSPACE_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or(AppError::Internal("a file's store isn't configured"))?;
    let mut connection = crate::db::scoped_tx(pool).await?;
    let row = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM workspace_stores WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::Internal("a file's store isn't configured"))?;
    let bucket = bucket_of(config, &mut connection, &row).await?;
    connection.commit().await?;
    Ok(bucket)
}

/// Every bucket a workspace has had, for deleting its files with it.
pub async fn all_of(state: &AppState, workspace: Uuid) -> Result<Vec<FileStore>, AppError> {
    let ids =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM workspace_stores WHERE workspace_id = $1")
            .bind(workspace)
            .fetch_all(crate::db::scoped(&state.pool))
            .await?;
    let mut stores = Vec::with_capacity(ids.len());
    for id in ids {
        stores.push(FileStore::Object(
            open(&state.pool, &state.config, &store_id(id)).await?,
        ));
    }
    Ok(stores)
}

/// The workspace's current bucket, as it may be shown.
pub(crate) async fn current(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<Option<WorkspaceStoreView>, AppError> {
    Ok(sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM workspace_stores WHERE workspace_id = $1 AND retired_at IS NULL"
    ))
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?
    .map(|row| row.view()))
}

/// A bucket an owner asked for.
pub(crate) struct NewBucket<'a> {
    pub endpoint: &'a str,
    pub bucket: &'a str,
    pub region: &'a str,
    pub prefix: &'a str,
    pub path_style: bool,
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

/// Why a bucket was refused, in words for the owner (never a key).
pub(crate) enum Refused {
    Field(&'static str, &'static str),
    Unreachable,
    Other(AppError),
}

impl From<AppError> for Refused {
    fn from(error: AppError) -> Self {
        Self::Other(error)
    }
}

fn checked(bucket: &NewBucket<'_>) -> Result<(Url, String), Refused> {
    let url = Url::parse(bucket.endpoint.trim()).map_err(|_| {
        Refused::Field(
            "endpoint",
            "Give the bucket's address, such as https://s3.example.com.",
        )
    })?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || bucket.endpoint.len() > 300
    {
        return Err(Refused::Field(
            "endpoint",
            "Give the store's address alone (https and a host, no path or keys), such as https://s3.example.com.",
        ));
    }
    let name = bucket.bucket.trim();
    if !(2..=63).contains(&name.len())
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
        || !name.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(Refused::Field(
            "bucket",
            "Give the bucket's name: lowercase letters, digits, dots and dashes.",
        ));
    }
    let region = bucket.region.trim();
    if region.is_empty()
        || region.len() > 40
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(Refused::Field(
            "region",
            "Give the region, such as us-east-1 (or auto).",
        ));
    }
    let prefix = bucket.prefix.trim().trim_start_matches('/');
    if prefix.len() > 199
        || prefix.contains("..")
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
    {
        return Err(Refused::Field(
            "prefix",
            "Letters, digits, dashes, dots and slashes, up to 199.",
        ));
    }
    let prefix = if prefix.is_empty() || prefix.ends_with('/') {
        prefix.to_owned()
    } else {
        format!("{prefix}/")
    };
    if bucket.access_key_id.trim().is_empty() || bucket.access_key_id.len() > 200 {
        return Err(Refused::Field(
            "accessKeyId",
            "Give the bucket's access key id.",
        ));
    }
    if bucket.secret_access_key.trim().is_empty() || bucket.secret_access_key.len() > 200 {
        return Err(Refused::Field(
            "secretAccessKey",
            "Give the bucket's secret access key.",
        ));
    }
    Ok((url, prefix))
}

/// A bucket that passed its probe, ready to record.
pub(crate) struct Probed {
    id: Uuid,
    endpoint: String,
    bucket: String,
    region: String,
    prefix: String,
    path_style: bool,
    keys: Zeroizing<String>,
}

/// Checks a bucket by writing, reading and deleting a probe object. Run it
/// before any transaction opens: it waits on the store.
pub(crate) async fn probe(
    state: &AppState,
    workspace: Uuid,
    bucket: NewBucket<'_>,
) -> Result<Probed, Refused> {
    let (url, prefix) = checked(&bucket)?;
    let id = Uuid::now_v7();
    let endpoint = url.as_str().trim_end_matches('/').to_owned();
    let config = location(
        &endpoint,
        bucket.bucket.trim(),
        bucket.region.trim(),
        &prefix,
        bucket.path_style,
    )?;
    let candidate = ObjectBucket::workspace(
        store_id(id),
        config,
        bucket.access_key_id.trim().to_owned(),
        Zeroizing::new(bucket.secret_access_key.trim().to_owned()),
        state.config.allow_private_addresses,
    );
    let probe = format!("{}/probe-{}", workspace.simple(), id.simple());
    let body = b"agent-workspace probe";
    let probed = async {
        candidate.put(&probe, body).await?;
        let read = candidate.read_all(&probe, 1024).await;
        let deleted = candidate.delete(&probe).await;
        if read? != body {
            return Err(AppError::Internal("the probe came back changed"));
        }
        deleted
    }
    .await;
    if probed.is_err() {
        return Err(Refused::Unreachable);
    }
    let keys = serde_json::to_string(&Keys {
        access_key_id: bucket.access_key_id.trim().to_owned(),
        secret_access_key: bucket.secret_access_key.trim().to_owned(),
    })
    .map_err(|_| AppError::Internal("encode a bucket's keys"))?;
    Ok(Probed {
        id,
        endpoint,
        bucket: bucket.bucket.trim().to_owned(),
        region: bucket.region.trim().to_owned(),
        prefix,
        path_style: bucket.path_style,
        keys: Zeroizing::new(keys),
    })
}

/// Makes a probed bucket where the workspace's new uploads go, retiring the
/// one before; its keys go into the keyring.
pub(crate) async fn record(
    state: &AppState,
    connection: &mut PgConnection,
    workspace: Uuid,
    actor: Uuid,
    probed: Probed,
) -> Result<WorkspaceStoreView, AppError> {
    let keyring = secrets::keyring(&state.config)?;
    let secret = secrets::put(
        &mut *connection,
        keyring.as_ref(),
        NewSecret {
            owner: SecretOwner::Workspace {
                organization_id: workspace,
            },
            name: &secret_name(probed.id),
            description: Some("The access keys of the workspace's own file bucket."),
            value: &SecretValue::new(probed.keys.as_str()),
            actor_id: Some(actor),
            system_managed: true,
        },
    )
    .await?;
    retire(connection, workspace).await?;
    let row = sqlx::query_as::<_, Row>(&format!(
        r#"
        INSERT INTO workspace_stores
            (id, workspace_id, endpoint, bucket, region, prefix, path_style, secret_id,
             verified_at, set_by_actor_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), $9)
        RETURNING {COLUMNS}
        "#
    ))
    .bind(probed.id)
    .bind(workspace)
    .bind(&probed.endpoint)
    .bind(&probed.bucket)
    .bind(&probed.region)
    .bind(&probed.prefix)
    .bind(probed.path_style)
    .bind(secret.id)
    .bind(actor)
    .fetch_one(&mut *connection)
    .await?;
    Ok(row.view())
}

/// New uploads go back to the server's own store. Files already in the
/// workspace's bucket stay there, and are still read from it.
pub(crate) async fn retire(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<bool, AppError> {
    let retired = sqlx::query(
        "UPDATE workspace_stores SET retired_at = now() WHERE workspace_id = $1 AND retired_at IS NULL",
    )
    .bind(workspace)
    .execute(&mut *connection)
    .await?;
    Ok(retired.rows_affected() > 0)
}

/// The workspace a bucket (`ws-<id>`) belongs to.
pub(crate) async fn workspace_of(pool: &PgPool, store: &str) -> Result<Option<Uuid>, AppError> {
    let Some(id) = store
        .strip_prefix(WORKSPACE_PREFIX)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return Ok(None);
    };
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM workspace_stores WHERE id = $1")
            .bind(id)
            .fetch_optional(crate::db::scoped(pool))
            .await?,
    )
}
