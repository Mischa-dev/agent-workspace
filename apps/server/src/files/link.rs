//! Short-lived file links (A2.12): `file_link_get` hands an agent, an app or a
//! script a URL it can fetch a file from with a plain `GET`, for five
//! minutes, without its credential. The link names who it was made for and
//! is signed with that account's own default connection key, so nothing is
//! stored and rotating nothing is needed; it is checked again when fetched,
//! as that account may read the file then (a conversation it has left
//! refuses it).

use axum::{Router, extract::State, response::Response, routing::get};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use hmac::{Hmac, Mac};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use super::{Reader, readable_file, served_content_type};
use crate::{
    AppState,
    error::{ApiPath, AppError},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

const PREFIX: &str = "fl1.";
const MAC_BYTES: usize = 16;
/// How long a link works.
pub const LIFETIME_SECONDS: i64 = 300;

/// Who a link is for and until when: the reader it was checked as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Claims {
    file_id: Uuid,
    account_id: Uuid,
    connection_id: Option<Uuid>,
    confined_space: Option<Uuid>,
    expires: i64,
}

impl Claims {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(72);
        bytes.extend_from_slice(self.file_id.as_bytes());
        bytes.extend_from_slice(self.account_id.as_bytes());
        bytes.extend_from_slice(self.connection_id.unwrap_or_default().as_bytes());
        bytes.extend_from_slice(self.confined_space.unwrap_or_default().as_bytes());
        bytes.extend_from_slice(&self.expires.to_be_bytes());
        bytes
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 72 {
            return None;
        }
        let id = |at: usize| Uuid::from_slice(&bytes[at..at + 16]).ok();
        let optional = |at: usize| id(at).filter(|id| !id.is_nil());
        Some(Self {
            file_id: id(0)?,
            account_id: id(16)?,
            connection_id: optional(32),
            confined_space: optional(48),
            expires: i64::from_be_bytes(bytes[64..72].try_into().ok()?),
        })
    }

    fn reader(&self) -> Reader {
        if self.connection_id.is_none() && self.confined_space.is_none() {
            Reader::Browser {
                actor_id: self.account_id,
            }
        } else {
            Reader::Mcp {
                actor_id: self.account_id,
                connection_id: self.connection_id,
                confined_project: self.confined_space,
            }
        }
    }
}

fn mac(key: &[u8], payload: &[u8]) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(b"file-link");
    mac.update(payload);
    Some(mac)
}

fn encode(key: &[u8], claims: &Claims) -> Option<String> {
    let payload = claims.bytes();
    let signature = mac(key, &payload)?.finalize().into_bytes();
    Some(format!(
        "{PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(&signature[..MAC_BYTES])
    ))
}

/// The claims a token makes, unchecked: who to look the key up for.
fn unverified(token: &str) -> Option<(Claims, Vec<u8>, Vec<u8>)> {
    if token.len() > 256 {
        return None;
    }
    let (payload, signature) = token.strip_prefix(PREFIX)?.split_once('.')?;
    let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    (signature.len() == MAC_BYTES).then_some(())?;
    Some((Claims::parse(&payload)?, payload, signature))
}

fn verify(key: &[u8], payload: &[u8], signature: &[u8]) -> bool {
    mac(key, payload).is_some_and(|mac| mac.verify_truncated_left(signature).is_ok())
}

/// The key a link for `account_id` is signed with: its default
/// connection's own key.
async fn key_of(state: &AppState, account_id: Uuid) -> Result<Vec<u8>, AppError> {
    crate::db::scoped_one(&state.pool, async |connection| {
        Ok(
            crate::delivery::connections::default_for(connection, account_id)
                .await?
                .cursor_key,
        )
    })
    .await
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/file-links/{token}", get(fetch))
}

/// The file a link names, if the link is good and its account may still
/// read the file. Anything else is `not_found`, whatever the reason.
async fn fetch(
    State(state): State<AppState>,
    ApiPath(token): ApiPath<String>,
) -> Result<Response, AppError> {
    let gone = AppError::NotFound("file");
    let Some((claims, payload, signature)) = unverified(&token) else {
        return Err(gone);
    };
    if claims.expires < Utc::now().timestamp() {
        return Err(gone);
    }
    let key = match key_of(&state, claims.account_id).await {
        Ok(key) => key,
        Err(AppError::NotFound(_)) => return Err(gone),
        Err(error) => return Err(error),
    };
    if !verify(&key, &payload, &signature) {
        return Err(gone);
    }
    let row = readable_file(&state.pool, claims.reader(), claims.file_id).await?;
    super::http::serve(
        &state,
        &row.storage_key,
        served_content_type(&row),
        &row.name,
        !row.is_inline_image(),
    )
    .await
}

pub fn register_ops(registry: &mut Registry) {
    registry.add::<FileLink>();
}

/// `file_link_get`: a link to fetch one file you can see.
pub struct FileLink;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileLinkInput {
    /// The file: its id from a message's or task's attachments, or
    /// file_list.
    pub file_id: IdRef,
}

impl Targeted for FileLinkInput {
    fn target(&self) -> Target {
        Target::thing("file", self.file_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileLinkView {
    /// Fetch it with a plain GET, no credential needed, until `expiresAt`.
    /// Don't share it: whoever holds it can read the file until then.
    pub url: String,
    #[schemars(with = "String")]
    pub expires_at: DateTime<Utc>,
    pub name: String,
    pub content_type: String,
    pub size: i64,
}

impl Operation for FileLink {
    const NAME: &'static str = "file_link_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Link to a file";
    const DESCRIPTION: &'static str = "A link to a file you can see: a plain GET, no \
        credential, for five minutes. For files too large for file_read.";

    type Input = FileLinkInput;
    type Output = FileLinkView;

    async fn run(call: &mut OpCall<'_>, input: FileLinkInput) -> Result<FileLinkView, OpError> {
        let caller = &call.ctx.caller;
        let file_id = input.file_id.id();
        let delegated = caller.limits.is_some() || caller.via_grant_id.is_some();
        let claims = Claims {
            file_id,
            account_id: caller.account_id,
            connection_id: caller.connection_id,
            confined_space: caller.confined_space,
            expires: (Utc::now() + TimeDelta::seconds(LIFETIME_SECONDS)).timestamp(),
        };
        let reader = if delegated && claims.connection_id.is_none() {
            // An app acting for a person reads as an MCP caller does.
            Reader::Mcp {
                actor_id: claims.account_id,
                connection_id: None,
                confined_project: claims.confined_space,
            }
        } else {
            claims.reader()
        };
        let row = crate::policy::within_agent_limits(
            caller.limits.clone(),
            readable_file(&call.state.pool, reader, file_id),
        )
        .await
        .map_err(|error| match error {
            AppError::NotFound(_) => OpError::not_found("That file"),
            other => other.into(),
        })?;
        let key = key_of(call.state, caller.account_id).await?;
        let token = encode(&key, &claims).ok_or(AppError::Internal("sign file link"))?;
        let expires_at = DateTime::from_timestamp(claims.expires, 0).unwrap_or_else(Utc::now);
        Ok(FileLinkView {
            url: format!(
                "{}/api/v1/file-links/{token}",
                crate::events::public_origin(call.state).trim_end_matches('/')
            ),
            expires_at,
            name: row.name,
            content_type: row.content_type,
            size: row.byte_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> Claims {
        Claims {
            file_id: Uuid::now_v7(),
            account_id: Uuid::now_v7(),
            connection_id: Some(Uuid::now_v7()),
            confined_space: None,
            expires: 1_900_000_000,
        }
    }

    #[test]
    fn a_link_carries_its_reader_and_is_signed() {
        let key = [7_u8; 32];
        let made = claims();
        let token = encode(&key, &made).expect("token");
        assert!(token.starts_with(PREFIX));
        let (read, payload, signature) = unverified(&token).expect("parses");
        assert_eq!(read, made);
        assert!(verify(&key, &payload, &signature));
        assert!(
            !verify(&[8_u8; 32], &payload, &signature),
            "another key refuses it"
        );
    }

    #[test]
    fn a_changed_link_is_refused() {
        let key = [7_u8; 32];
        let token = encode(&key, &claims()).expect("token");
        let (mut read, _, signature) = unverified(&token).expect("parses");
        read.file_id = Uuid::now_v7();
        assert!(!verify(&key, &read.bytes(), &signature));
        assert!(unverified("fl1.short.x").is_none());
        assert!(unverified(&"a".repeat(300)).is_none());
    }
}
