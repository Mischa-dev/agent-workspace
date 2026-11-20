//! An S3-compatible object store (K5.12): any service that speaks the S3
//! API (AWS, R2, MinIO, Backblaze, Garage). Requests are signed here with
//! AWS Signature Version 4 and sent through `outbound`, so no other HTTP
//! client is built. The endpoint is the operator's, never a workspace's.
//!
//! Uploads that fit in one part are one `PUT`; larger ones stream as a
//! multipart upload, holding one part at a time. Downloads stream, and can
//! be handed out as presigned links the store serves itself.

use std::{fmt::Write as _, path::Path, time::Duration};

use axum::body::{Body, Bytes};
use chrono::{DateTime, Utc};
use futures_util::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::{Method, StatusCode};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use super::store::{StoredBytes, Written};
use crate::{config::ObjectStoreConfig, error::AppError, outbound};

/// The longest a request that moves no file body may take: a look, a
/// delete, a listing, or starting, finishing or abandoning an upload.
const CONTROL_DEADLINE: Duration = Duration::from_secs(60);
/// The longest a request that moves at most a part's bytes may take.
const PART_DEADLINE: Duration = Duration::from_secs(300);
/// The most of a control answer read: a listing of a thousand keys is
/// well under this.
const CONTROL_BYTES: usize = 4 << 20;
/// Bytes held before an upload becomes multipart; also each part's size.
/// S3 needs parts of at least 5 MiB, except the last.
const PART_BYTES: usize = 8 * 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

#[derive(Clone, Debug)]
pub struct ObjectBucket {
    /// The store's name in `files.store_id`.
    id: String,
    /// Where the bucket is. For a workspace's own bucket the key paths in
    /// it are unused: its keys are held (`keys`).
    config: ObjectStoreConfig,
    keys: Keys,
    /// The operator's own store may be on a private address; a workspace's
    /// may not (the outbound address guard applies to it).
    private_ok: bool,
}

/// Where the access keys come from.
#[derive(Clone)]
enum Keys {
    /// The operator's secret files, read on each use.
    Files,
    /// A workspace's own bucket's, opened from the secrets keyring.
    Held(std::sync::Arc<Credentials>),
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Files => "Files",
            Self::Held(_) => "Held",
        })
    }
}

struct Credentials {
    access_key: String,
    secret_key: Zeroizing<String>,
}

/// What a client sends with a direct upload: the address, and the headers
/// the signature covers.
#[derive(Clone, Debug)]
pub struct PutLink {
    pub url: Url,
    pub headers: Vec<(String, String)>,
}

/// What the store says it holds under a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub length: u64,
    /// The SHA-256 the store checked on upload (base64), when it keeps one.
    pub sha256: Option<String>,
}

/// One request, signed.
struct Signed {
    url: Url,
    headers: Vec<(String, String)>,
}

impl ObjectBucket {
    /// The operator's store (`AGENT_WORKSPACE_S3_*`).
    pub fn new(config: ObjectStoreConfig) -> Self {
        Self {
            id: super::store::OBJECT.to_owned(),
            config,
            keys: Keys::Files,
            private_ok: true,
        }
    }

    /// A workspace's own bucket, with its keys. It is reached only at a
    /// public address unless the operator allows private ones.
    pub fn workspace(
        id: String,
        config: ObjectStoreConfig,
        access_key: String,
        secret_key: Zeroizing<String>,
        private_ok: bool,
    ) -> Self {
        Self {
            id,
            config,
            keys: Keys::Held(std::sync::Arc::new(Credentials {
                access_key,
                secret_key,
            })),
            private_ok,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// A link a client uploads `key` to directly, for `ttl`: exactly
    /// `length` bytes of `content_type` whose SHA-256 is `sha256`. The
    /// signature covers all three, so the store refuses anything else.
    pub fn presign_put(
        &self,
        key: &str,
        ttl: Duration,
        content_type: &str,
        length: u64,
        sha256: &[u8; 32],
    ) -> Result<PutLink, AppError> {
        use base64::Engine as _;
        let credentials = self.credentials()?;
        let mut url = self.object_url(Some(key))?;
        let now = Utc::now();
        let checksum = base64::engine::general_purpose::STANDARD.encode(sha256);
        let headers = vec![
            ("content-length".to_owned(), length.to_string()),
            ("content-type".to_owned(), content_type.to_owned()),
            ("host".to_owned(), host_header(&url)),
            ("x-amz-checksum-sha256".to_owned(), checksum),
        ];
        let signed_headers = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let mut query = vec![
            ("X-Amz-Algorithm".to_owned(), "AWS4-HMAC-SHA256".to_owned()),
            (
                "X-Amz-Credential".to_owned(),
                format!(
                    "{}/{}",
                    credentials.access_key,
                    scope(&now, &self.config.region)
                ),
            ),
            ("X-Amz-Date".to_owned(), amz_date(&now)),
            (
                "X-Amz-Expires".to_owned(),
                ttl.as_secs().clamp(1, 604_800).to_string(),
            ),
            ("X-Amz-SignedHeaders".to_owned(), signed_headers),
        ];
        let signature = signature(
            &credentials,
            &self.config.region,
            &now,
            "PUT",
            url.path(),
            &query,
            &headers,
            UNSIGNED,
        );
        query.push(("X-Amz-Signature".to_owned(), signature));
        url.set_query(Some(&canonical_query(&query)));
        Ok(PutLink {
            url,
            headers: headers
                .into_iter()
                .filter(|(name, _)| name != "host" && name != "content-length")
                .collect(),
        })
    }

    /// What the store holds under `key`, or None when nothing is there.
    pub async fn head(&self, key: &str) -> Result<Option<Head>, AppError> {
        let response = self
            .send(
                Method::HEAD,
                key,
                &[],
                &[("x-amz-checksum-mode".to_owned(), "ENABLED".to_owned())],
                Bytes::new(),
                Some(CONTROL_DEADLINE),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = expect(response, "look at a file").await?;
        let length = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(AppError::Internal("the object store gave no length"))?;
        let sha256 = response
            .headers()
            .get("x-amz-checksum-sha256")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Some(Head { length, sha256 }))
    }

    /// The first `count` bytes under `key` (fewer when it is shorter).
    pub async fn read_start(&self, key: &str, count: u64) -> Result<Vec<u8>, AppError> {
        let range = format!("bytes=0-{}", count.saturating_sub(1));
        let response = self
            .send(
                Method::GET,
                key,
                &[],
                &[("range".to_owned(), range)],
                Bytes::new(),
                Some(body_deadline(count)),
            )
            .await?;
        let mut response = expect(response, "read a file").await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AppError::Internal("read a file from the object store"))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 >= count {
                break;
            }
        }
        bytes.truncate(usize::try_from(count).unwrap_or(usize::MAX));
        Ok(bytes)
    }

    pub async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), AppError> {
        let response = self
            .send(
                Method::PUT,
                key,
                &[],
                &[],
                Bytes::copy_from_slice(bytes),
                Some(body_deadline(bytes.len() as u64)),
            )
            .await?;
        expect(response, "store a file").await.map(drop)
    }

    pub async fn put_stream<S, E>(
        &self,
        key: &str,
        mut chunks: S,
        limit: u64,
    ) -> Result<Written, AppError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin,
    {
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut part = Vec::with_capacity(PART_BYTES);
        let mut upload: Option<Multipart> = None;
        let result: Result<(), AppError> = async {
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| AppError::Validation("body"))?;
                length = length.saturating_add(chunk.len() as u64);
                if length > limit {
                    return Err(AppError::PayloadTooLarge);
                }
                hasher.update(&chunk);
                part.extend_from_slice(&chunk);
                if part.len() >= PART_BYTES {
                    let multipart = match upload.as_mut() {
                        Some(multipart) => multipart,
                        None => upload.insert(self.start_multipart(key).await?),
                    };
                    let full = std::mem::replace(&mut part, Vec::with_capacity(PART_BYTES));
                    self.upload_part(key, multipart, full).await?;
                }
            }
            match upload.as_mut() {
                None => self.put(key, &part).await,
                Some(multipart) => {
                    if !part.is_empty() {
                        let last = std::mem::take(&mut part);
                        self.upload_part(key, multipart, last).await?;
                    }
                    self.finish_multipart(key, multipart).await
                }
            }
        }
        .await;
        if let Err(error) = result {
            if let Some(multipart) = &upload {
                self.abort_multipart(key, multipart).await;
            }
            return Err(error);
        }
        Ok(Written {
            length,
            sha256: hex::encode(hasher.finalize()),
        })
    }

    pub async fn get(&self, key: &str) -> Result<StoredBytes, AppError> {
        // Streamed on to its reader, at the reader's pace: no deadline.
        let response = self
            .send(Method::GET, key, &[], &[], Bytes::new(), None)
            .await?;
        let response = expect(response, "read a file").await?;
        let length = response.content_length().unwrap_or(0);
        let body = futures_util::stream::unfold(Some(response), |response| async move {
            let mut response = response?;
            match response.chunk().await {
                Ok(Some(chunk)) => Some((Ok(chunk), Some(response))),
                Ok(None) => None,
                Err(error) => Some((Err(std::io::Error::other(error)), None)),
            }
        });
        Ok(StoredBytes {
            body: Body::from_stream(body),
            length,
        })
    }

    pub async fn read_all(&self, key: &str, maximum: u64) -> Result<Vec<u8>, AppError> {
        let response = self
            .send(
                Method::GET,
                key,
                &[],
                &[],
                Bytes::new(),
                Some(body_deadline(maximum)),
            )
            .await?;
        let mut response = expect(response, "read a file").await?;
        if response
            .content_length()
            .is_some_and(|length| length > maximum)
        {
            return Err(AppError::PayloadTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AppError::Internal("read a file from the object store"))?
        {
            if (bytes.len() + chunk.len()) as u64 > maximum {
                return Err(AppError::PayloadTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Deleting a key that is already gone succeeds.
    pub async fn delete(&self, key: &str) -> Result<(), AppError> {
        let response = self
            .send(
                Method::DELETE,
                key,
                &[],
                &[],
                Bytes::new(),
                Some(CONTROL_DEADLINE),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        expect(response, "delete a file").await.map(drop)
    }

    /// Deletes every key under `prefix` (a workspace's, H1.24).
    pub async fn delete_prefix(&self, prefix: &str) -> Result<(), AppError> {
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("list-type".to_owned(), "2".to_owned()),
                (
                    "prefix".to_owned(),
                    format!("{}{prefix}", self.config.prefix),
                ),
            ];
            if let Some(token) = &token {
                query.push(("continuation-token".to_owned(), token.clone()));
            }
            let response = self
                .send_raw(
                    Method::GET,
                    None,
                    &query,
                    &[],
                    Bytes::new(),
                    Some(CONTROL_DEADLINE),
                )
                .await?;
            let listing = control_text(
                expect(response, "list files").await?,
                "list the object store",
            )
            .await?;
            for key in xml_values(&listing, "Key") {
                let Some(key) = key.strip_prefix(&self.config.prefix) else {
                    continue;
                };
                self.delete(key).await?;
            }
            token = xml_values(&listing, "NextContinuationToken")
                .into_iter()
                .next()
                .filter(|_| {
                    xml_values(&listing, "IsTruncated")
                        .first()
                        .map(String::as_str)
                        == Some("true")
                });
            if token.is_none() {
                return Ok(());
            }
        }
    }

    /// A link the store serves `key` from until `ttl` passes, telling the
    /// browser the type and whether to show or save it.
    pub fn presign_get(
        &self,
        key: &str,
        ttl: Duration,
        content_type: &str,
        disposition: &str,
    ) -> Result<Url, AppError> {
        let credentials = self.credentials()?;
        let mut url = self.object_url(Some(key))?;
        let now = Utc::now();
        let scope = scope(&now, &self.config.region);
        let mut query = vec![
            ("X-Amz-Algorithm".to_owned(), "AWS4-HMAC-SHA256".to_owned()),
            (
                "X-Amz-Credential".to_owned(),
                format!("{}/{scope}", credentials.access_key),
            ),
            ("X-Amz-Date".to_owned(), amz_date(&now)),
            (
                "X-Amz-Expires".to_owned(),
                ttl.as_secs().clamp(1, 604_800).to_string(),
            ),
            ("X-Amz-SignedHeaders".to_owned(), "host".to_owned()),
            (
                "response-content-disposition".to_owned(),
                disposition.to_owned(),
            ),
            ("response-content-type".to_owned(), content_type.to_owned()),
        ];
        let host = host_header(&url);
        let signature = signature(
            &credentials,
            &self.config.region,
            &now,
            "GET",
            url.path(),
            &query,
            &[("host".to_owned(), host)],
            UNSIGNED,
        );
        query.push(("X-Amz-Signature".to_owned(), signature));
        url.set_query(Some(&canonical_query(&query)));
        Ok(url)
    }

    async fn start_multipart(&self, key: &str) -> Result<Multipart, AppError> {
        let response = self
            .send(
                Method::POST,
                key,
                &[("uploads".to_owned(), String::new())],
                &[],
                Bytes::new(),
                Some(CONTROL_DEADLINE),
            )
            .await?;
        let body = control_text(
            expect(response, "start an upload").await?,
            "start an upload to the object store",
        )
        .await?;
        let upload_id = xml_values(&body, "UploadId")
            .into_iter()
            .next()
            .ok_or(AppError::Internal("start an upload to the object store"))?;
        Ok(Multipart {
            upload_id,
            parts: Vec::new(),
        })
    }

    async fn upload_part(
        &self,
        key: &str,
        multipart: &mut Multipart,
        bytes: Vec<u8>,
    ) -> Result<(), AppError> {
        let number = multipart.parts.len() + 1;
        let deadline = body_deadline(bytes.len() as u64);
        let response = self
            .send(
                Method::PUT,
                key,
                &[
                    ("partNumber".to_owned(), number.to_string()),
                    ("uploadId".to_owned(), multipart.upload_id.clone()),
                ],
                &[],
                Bytes::from(bytes),
                Some(deadline),
            )
            .await?;
        let response = expect(response, "upload a part").await?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .ok_or(AppError::Internal("upload a part to the object store"))?
            .to_owned();
        multipart.parts.push(etag);
        Ok(())
    }

    async fn finish_multipart(&self, key: &str, multipart: &Multipart) -> Result<(), AppError> {
        let mut body = String::from("<CompleteMultipartUpload>");
        for (index, etag) in multipart.parts.iter().enumerate() {
            let _ = write!(
                body,
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                index + 1,
                xml_escape(etag)
            );
        }
        body.push_str("</CompleteMultipartUpload>");
        let response = self
            .send(
                Method::POST,
                key,
                &[("uploadId".to_owned(), multipart.upload_id.clone())],
                &[("content-type".to_owned(), "application/xml".to_owned())],
                Bytes::from(body),
                Some(CONTROL_DEADLINE),
            )
            .await?;
        // A failed completion can still answer 200, with an error inside.
        let text = control_text(
            expect(response, "finish an upload").await?,
            "finish an upload to the object store",
        )
        .await?;
        if text.contains("<Error>") {
            return Err(AppError::Internal("finish an upload to the object store"));
        }
        Ok(())
    }

    async fn abort_multipart(&self, key: &str, multipart: &Multipart) {
        let _ = self
            .send(
                Method::DELETE,
                key,
                &[("uploadId".to_owned(), multipart.upload_id.clone())],
                &[],
                Bytes::new(),
                Some(CONTROL_DEADLINE),
            )
            .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: Method,
        key: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        body: Bytes,
        deadline: Option<Duration>,
    ) -> Result<reqwest::Response, AppError> {
        self.send_raw(method, Some(key), query, headers, body, deadline)
            .await
    }

    /// One signed request. `deadline` bounds it from connecting until its
    /// answer's body is read; a download streamed on to a reader has none.
    async fn send_raw(
        &self,
        method: Method,
        key: Option<&str>,
        query: &[(String, String)],
        headers: &[(String, String)],
        body: Bytes,
        deadline: Option<Duration>,
    ) -> Result<reqwest::Response, AppError> {
        let credentials = self.credentials()?;
        let signed = self.sign(
            &credentials,
            &method,
            key,
            query,
            headers,
            &body,
            Utc::now(),
        )?;
        let client = outbound::client_for(
            &signed.url,
            self.private_ok,
            outbound::Timeouts {
                connect: Duration::from_secs(10),
                total: deadline,
            },
            None,
        )
        .await
        .map_err(|_| AppError::Internal("reach the object store"))?;
        let mut request = client.request(method, signed.url);
        for (name, value) in signed.headers {
            if name != "host" {
                request = request.header(name, value);
            }
        }
        request
            .body(body)
            .send()
            .await
            .map_err(|_| AppError::Internal("reach the object store"))
    }

    #[allow(clippy::too_many_arguments)]
    fn sign(
        &self,
        credentials: &Credentials,
        method: &Method,
        key: Option<&str>,
        query: &[(String, String)],
        extra: &[(String, String)],
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Signed, AppError> {
        let mut url = self.object_url(key)?;
        let payload = if body.is_empty() {
            EMPTY_SHA256.to_owned()
        } else {
            hex::encode(Sha256::digest(body))
        };
        let mut headers = vec![
            ("host".to_owned(), host_header(&url)),
            ("x-amz-content-sha256".to_owned(), payload.clone()),
            ("x-amz-date".to_owned(), amz_date(&now)),
        ];
        headers.extend(extra.iter().cloned());
        headers.sort();
        let signature = signature(
            credentials,
            &self.config.region,
            &now,
            method.as_str(),
            url.path(),
            query,
            &headers,
            &payload,
        );
        let signed_headers = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        headers.push((
            "authorization".to_owned(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={signed_headers}, Signature={signature}",
                credentials.access_key,
                scope(&now, &self.config.region),
            ),
        ));
        if !query.is_empty() {
            url.set_query(Some(&canonical_query(query)));
        }
        Ok(Signed { url, headers })
    }

    /// The object's address: `endpoint/bucket/prefix+key` path-style, else
    /// `bucket.endpoint/prefix+key`; the bucket itself when `key` is None.
    fn object_url(&self, key: Option<&str>) -> Result<Url, AppError> {
        let config = &self.config;
        let mut url = config.endpoint.clone();
        let object = key.map(|key| format!("{}{key}", config.prefix));
        let path = if config.path_style {
            match &object {
                Some(object) => format!("/{}/{}", config.bucket, uri_encode(object, false)),
                None => format!("/{}/", config.bucket),
            }
        } else {
            let host = format!(
                "{}.{}",
                config.bucket,
                url.host_str()
                    .ok_or(AppError::Internal("object store address"))?
            );
            url.set_host(Some(&host))
                .map_err(|_| AppError::Internal("object store address"))?;
            match &object {
                Some(object) => format!("/{}", uri_encode(object, false)),
                None => "/".to_owned(),
            }
        };
        url.set_path(&path);
        Ok(url)
    }

    /// Read each time: the files are small, and a rotated key is picked up
    /// without a restart.
    fn credentials(&self) -> Result<Credentials, AppError> {
        if let Keys::Held(held) = &self.keys {
            return Ok(Credentials {
                access_key: held.access_key.clone(),
                secret_key: held.secret_key.clone(),
            });
        }
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .map(|value| value.trim().to_owned())
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or(AppError::Internal("read the object store's keys"))
        };
        Ok(Credentials {
            access_key: read(&self.config.access_key_id_path)?,
            secret_key: Zeroizing::new(read(&self.config.secret_access_key_path)?),
        })
    }
}

struct Multipart {
    upload_id: String,
    parts: Vec<String>,
}

async fn expect(
    response: reqwest::Response,
    doing: &'static str,
) -> Result<reqwest::Response, AppError> {
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::NotFound("file"));
    }
    if !status.is_success() {
        tracing::warn!(status = status.as_u16(), doing, "object store refused");
        return Err(AppError::Internal("the object store refused"));
    }
    Ok(response)
}

/// A control answer's text (a listing, an upload's id, a completion), read
/// up to `CONTROL_BYTES`: a store that answers with more is refused, not
/// held in memory.
async fn control_text(
    mut response: reqwest::Response,
    failure: &'static str,
) -> Result<String, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > CONTROL_BYTES as u64)
    {
        return Err(AppError::Internal(failure));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::Internal(failure))?
    {
        if bytes.len() + chunk.len() > CONTROL_BYTES {
            return Err(AppError::Internal(failure));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| AppError::Internal(failure))
}

/// How long a request moving `bytes` may take: a part's deadline, or longer
/// for more, at no less than 64 KiB a second.
fn body_deadline(bytes: u64) -> Duration {
    PART_DEADLINE.max(Duration::from_secs(bytes / (64 * 1024)))
}

fn amz_date(now: &DateTime<Utc>) -> String {
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

fn scope(now: &DateTime<Utc>, region: &str) -> String {
    format!("{}/{region}/s3/aws4_request", now.format("%Y%m%d"))
}

fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// SigV4's encoding: every byte but unreserved ones as %XX, and `/` kept in
/// a path.
fn uri_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (byte == b'/' && !encode_slash)
        {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs = query
        .iter()
        .map(|(name, value)| (uri_encode(name, true), uri_encode(value, true)))
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[allow(clippy::too_many_arguments)]
fn signature(
    credentials: &Credentials,
    region: &str,
    now: &DateTime<Utc>,
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &[(String, String)],
    payload: &str,
) -> String {
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let request = format!(
        "{method}\n{path}\n{}\n{canonical_headers}\n{signed_headers}\n{payload}",
        canonical_query(query)
    );
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date(now),
        scope(now, region),
        hex::encode(Sha256::digest(request.as_bytes()))
    );
    let date = now.format("%Y%m%d").to_string();
    let key = [date.as_str(), region, "s3", "aws4_request"].iter().fold(
        format!("AWS4{}", credentials.secret_key.as_str()).into_bytes(),
        |key, part| hmac(&key, part.as_bytes()),
    );
    hex::encode(hmac(&key, to_sign.as_bytes()))
}

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// The text of every `<tag>` in a small S3 answer. The answers read here
/// (an upload id, a listing's keys) carry no nested tags of the same name.
fn xml_values(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        values.push(xml_unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    values
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn example() -> Credentials {
        Credentials {
            access_key: "AKIAIOSFODNN7EXAMPLE".to_owned(),
            secret_key: Zeroizing::new("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_owned()),
        }
    }

    /// AWS's published example of a presigned GET (SigV4 query signing).
    #[test]
    fn presigning_matches_the_published_example() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let query = [
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
            (
                "X-Amz-Credential",
                "AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request",
            ),
            ("X-Amz-Date", "20130524T000000Z"),
            ("X-Amz-Expires", "86400"),
            ("X-Amz-SignedHeaders", "host"),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()));
        assert_eq!(
            signature(
                &example(),
                "us-east-1",
                &now,
                "GET",
                "/test.txt",
                &query,
                &[(
                    "host".to_owned(),
                    "examplebucket.s3.amazonaws.com".to_owned()
                )],
                UNSIGNED,
            ),
            "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    /// AWS's published example of a signed GET with a range header.
    #[test]
    fn header_signing_matches_the_published_example() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let headers = [
            ("host", "examplebucket.s3.amazonaws.com"),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY_SHA256),
            ("x-amz-date", "20130524T000000Z"),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()));
        assert_eq!(
            signature(
                &example(),
                "us-east-1",
                &now,
                "GET",
                "/test.txt",
                &[],
                &headers,
                EMPTY_SHA256,
            ),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn keys_and_queries_are_encoded_once() {
        assert_eq!(uri_encode("a b/c+d", false), "a%20b/c%2Bd");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        let query = [("uploads", ""), ("b", "x y")].map(|(n, v)| (n.to_owned(), v.to_owned()));
        assert_eq!(canonical_query(&query), "b=x%20y&uploads=");
        assert_eq!(
            xml_values("<R><Key>a&amp;b</Key><Key>c</Key></R>", "Key"),
            ["a&b", "c"]
        );
    }
}
