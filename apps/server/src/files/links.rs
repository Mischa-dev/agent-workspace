//! Signed, short-lived download links (K5.12): a file handed to an agent,
//! a host or a script as a URL it can fetch without the reader's
//! credential. `/api/v1/files/links/{token}`, where the token names the
//! file, which of its bytes, when it stops working and who it was made for,
//! signed under a key derived from the secrets keyring ("file-links").
//!
//! Opening a link checks the signature and the time, then the reader's
//! access exactly as reading the file would (`readable_file`), so a link
//! stops working the moment the file is removed or the reader loses access.
//! The bytes stream from the app, or the request is redirected to the
//! store's own presigned link when it has one.

use std::time::Duration;

use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use super::{FileRow, FileStore, Reader, readable_file, served_content_type};
use crate::{
    AppState,
    error::{ApiPath, AppError},
    secrets,
};

/// How long a link works unless asked otherwise.
pub const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);
/// The longest a link works.
pub const MAX_TTL: Duration = Duration::from_secs(60 * 60);
const PURPOSE: &str = "file-links";
const VERSION: u8 = 1;
const MAC_BYTES: usize = 32;

/// Which bytes of a file a link serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Original,
    Thumbnail,
}

/// What a token says, once its signature checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Claims {
    file: Uuid,
    variant: Variant,
    reader: Reader,
    expires: i64,
}

/// A link to `file_id` for `reader`, working for `ttl` (at most an hour).
/// The caller has checked the reader may read the file now; the link
/// checks again each time it is opened.
pub(crate) fn link(
    state: &AppState,
    reader: Reader,
    file_id: Uuid,
    variant: Variant,
    ttl: Duration,
) -> Result<(String, chrono::DateTime<Utc>), AppError> {
    let keys = secrets::keyring(&state.config)?;
    let expires = Utc::now()
        + chrono::Duration::from_std(ttl.min(MAX_TTL)).unwrap_or(chrono::Duration::minutes(15));
    let (key_id, key) = keys.derived(None, PURPOSE)?;
    let token = sign(
        &key_id,
        &key[..],
        &Claims {
            file: file_id,
            variant,
            reader,
            expires: expires.timestamp(),
        },
    );
    let origin = crate::events::public_origin(state);
    Ok((format!("{origin}/api/v1/files/links/{token}"), expires))
}

fn sign(key_id: &str, key: &[u8], claims: &Claims) -> String {
    let mut payload = vec![VERSION];
    payload.push(match claims.variant {
        Variant::Original => 0,
        Variant::Thumbnail => 1,
    });
    payload.extend_from_slice(claims.file.as_bytes());
    payload.extend_from_slice(&claims.expires.to_be_bytes());
    match claims.reader {
        Reader::Browser { actor_id } => {
            payload.push(0);
            payload.extend_from_slice(actor_id.as_bytes());
        }
        Reader::Mcp {
            actor_id,
            connection_id,
            confined_project,
        } => {
            payload.push(1);
            payload.extend_from_slice(actor_id.as_bytes());
            for optional in [connection_id, confined_project] {
                match optional {
                    Some(id) => {
                        payload.push(1);
                        payload.extend_from_slice(id.as_bytes());
                    }
                    None => payload.push(0),
                }
            }
        }
    }
    payload.extend_from_slice(key_id.as_bytes());
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(&payload);
    payload.extend_from_slice(&mac.finalize().into_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

/// Reads a token; None for anything malformed, forged or expired.
fn open(token: &str, key: impl Fn(&str) -> Option<Vec<u8>>, now: i64) -> Option<Claims> {
    if token.len() > 512 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
    let (payload, signature) = bytes.split_at_checked(bytes.len().checked_sub(MAC_BYTES)?)?;
    let mut cursor = Cursor(payload);
    if cursor.byte()? != VERSION {
        return None;
    }
    let variant = match cursor.byte()? {
        0 => Variant::Original,
        1 => Variant::Thumbnail,
        _ => return None,
    };
    let file = cursor.uuid()?;
    let expires = i64::from_be_bytes(cursor.take(8)?.try_into().ok()?);
    let reader = match cursor.byte()? {
        0 => Reader::Browser {
            actor_id: cursor.uuid()?,
        },
        1 => {
            let actor_id = cursor.uuid()?;
            let mut optional = || match cursor.byte()? {
                0 => Some(None),
                1 => Some(Some(cursor.uuid()?)),
                _ => None,
            };
            let connection_id = optional()?;
            let confined_project = optional()?;
            Reader::Mcp {
                actor_id,
                connection_id,
                confined_project,
            }
        }
        _ => return None,
    };
    let key_id = std::str::from_utf8(cursor.0).ok()?;
    let key = key(key_id)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
    mac.update(payload);
    mac.verify_slice(signature).ok()?;
    (expires > now).then_some(Claims {
        file,
        variant,
        reader,
        expires,
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn uuid(&mut self) -> Option<Uuid> {
        Uuid::from_slice(self.take(16)?).ok()
    }
}

/// `GET /api/v1/files/links/{token}`: no session or credential; the token
/// is the credential, and access is checked again now.
pub(super) async fn open_link(
    State(state): State<AppState>,
    ApiPath(token): ApiPath<String>,
) -> Result<Response, AppError> {
    let keys = secrets::keyring(&state.config)?;
    let claims = open(
        &token,
        |key_id| {
            keys.derived(Some(key_id), PURPOSE)
                .ok()
                .map(|(_, key)| key.to_vec())
        },
        Utc::now().timestamp(),
    )
    .ok_or(AppError::NotFound("file"))?;
    let row = readable_file(&state.pool, claims.reader, claims.file).await?;
    let (key, content_type) = match (claims.variant, &row.thumbnail_key) {
        (Variant::Original, _) => (row.storage_key.as_str(), served_content_type(&row)),
        (Variant::Thumbnail, Some(thumbnail)) if row.is_inline_image() => {
            (thumbnail.as_str(), thumbnail_type(&row))
        }
        (Variant::Thumbnail, _) => return Err(AppError::NotFound("file")),
    };
    // Always a download: the link opens outside the app, and never as a
    // page on the app's own origin.
    let disposition = super::http::content_disposition(&row.name, true);
    let store = FileStore::named(&state.config, &row.store_id)?;
    let remaining = u64::try_from(claims.expires - Utc::now().timestamp()).unwrap_or(1);
    if let Some(url) = store.presign_get(
        key,
        Duration::from_secs(remaining.clamp(1, MAX_TTL.as_secs())),
        content_type,
        &disposition,
    )? {
        let mut response = Redirect::to(url.as_str()).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(response);
    }
    let mut response =
        super::http::serve(&state, &row.store_id, key, content_type, &row.name, true).await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn thumbnail_type(row: &FileRow) -> &'static str {
    if row.content_type == "image/jpeg" {
        "image/jpeg"
    } else {
        "image/png"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(reader: Reader, expires: i64) -> Claims {
        Claims {
            file: Uuid::now_v7(),
            variant: Variant::Original,
            reader,
            expires,
        }
    }

    #[test]
    fn a_token_opens_only_as_signed_and_in_time() {
        let key = [7_u8; 32];
        let keys = |id: &str| (id == "k1").then(|| key.to_vec());
        let mcp = Reader::Mcp {
            actor_id: Uuid::now_v7(),
            connection_id: Some(Uuid::now_v7()),
            confined_project: None,
        };
        for reader in [
            mcp,
            Reader::Browser {
                actor_id: Uuid::now_v7(),
            },
        ] {
            let signed = claims(reader, 1_000);
            let token = sign("k1", &key, &signed);
            assert_eq!(open(&token, keys, 999), Some(signed));
            assert_eq!(open(&token, keys, 1_000), None, "expired");
            // Any change to the token breaks it.
            let mut bytes = URL_SAFE_NO_PAD.decode(&token).expect("token");
            for index in [2, 20, bytes.len() - 1] {
                bytes[index] ^= 1;
                assert_eq!(open(&URL_SAFE_NO_PAD.encode(&bytes), keys, 999), None);
                bytes[index] ^= 1;
            }
            // Signed under a key the keyring doesn't have, or another key.
            assert_eq!(open(&sign("k2", &key, &signed), keys, 999), None);
            assert_eq!(open(&sign("k1", &[8; 32], &signed), keys, 999), None);
        }
        assert_eq!(open("not a token", keys, 0), None);
        assert_eq!(open("", keys, 0), None);
    }
}
