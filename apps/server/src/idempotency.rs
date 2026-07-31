use std::fmt;

use axum::http::{HeaderMap, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;
pub const MAX_IDEMPOTENCY_SCOPE_BYTES: usize = 120;

#[derive(Clone, Eq, PartialEq)]
pub struct IdempotencyKey(String);

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IdempotencyKey([redacted])")
    }
}

impl IdempotencyKey {
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, AppError> {
        let value = headers
            .get("idempotency-key")
            .and_then(|header| header.to_str().ok())
            .map(str::trim)
            .filter(|value| valid_idempotency_key(value))
            .ok_or(AppError::Validation("Idempotency-Key"))?;
        Ok(Self(value.to_owned()))
    }

    pub fn parse(value: &str) -> Result<Self, AppError> {
        let value = value.trim();
        valid_idempotency_key(value)
            .then(|| Self(value.to_owned()))
            .ok_or(AppError::Validation("Idempotency-Key"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestHash([u8; 32]);

impl RequestHash {
    pub fn for_value<T: Serialize>(value: &T) -> Result<Self, AppError> {
        let encoded = serde_json::to_vec(value)
            .map_err(|_| AppError::Internal("serialize idempotent command"))?;
        Ok(Self(Sha256::digest(encoded).into()))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone)]
pub struct IdempotencyReservation {
    actor_id: Uuid,
    scope: String,
    key: IdempotencyKey,
    request_hash: RequestHash,
}

impl IdempotencyReservation {
    pub fn actor_id(&self) -> Uuid {
        self.actor_id
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn key(&self) -> &IdempotencyKey {
        &self.key
    }

    pub fn request_hash(&self) -> RequestHash {
        self.request_hash
    }
}

#[derive(Clone)]
pub struct IdempotencyReplay {
    status: StatusCode,
    response_body: serde_json::Value,
}

impl IdempotencyReplay {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn response_body(&self) -> &serde_json::Value {
        &self.response_body
    }

    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, AppError> {
        serde_json::from_value(self.response_body.clone())
            .map_err(|_| AppError::Internal("decode stored idempotent response"))
    }
}

#[derive(Clone)]
pub enum IdempotencyOutcome {
    Reserved(IdempotencyReservation),
    Replay(IdempotencyReplay),
}

#[derive(Debug, FromRow)]
struct IdempotencyRow {
    request_hash: Vec<u8>,
    response_status: Option<i32>,
    response_body: Option<serde_json::Value>,
}

pub fn resource_bound_scope(
    namespace: &str,
    resource_id: Uuid,
    action: &str,
) -> Result<String, AppError> {
    let scope = format!("{namespace}.{}.{action}", resource_id.simple());
    validate_scope(&scope)?;
    Ok(scope)
}

pub async fn reserve(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    scope: &str,
    key: &IdempotencyKey,
    request_hash: RequestHash,
) -> Result<IdempotencyOutcome, AppError> {
    validate_scope(scope)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{actor_id}:{scope}:{}", key.as_str()))
        .execute(&mut **transaction)
        .await?;

    sqlx::query(
        "DELETE FROM idempotency_keys WHERE actor_id = $1 AND scope = $2 AND key = $3 AND expires_at <= now()",
    )
    .bind(actor_id)
    .bind(scope)
    .bind(key.as_str())
    .execute(&mut **transaction)
    .await?;

    let existing = sqlx::query_as::<_, IdempotencyRow>(
        r#"
        SELECT request_hash, response_status, response_body
        FROM idempotency_keys
        WHERE actor_id = $1 AND scope = $2 AND key = $3
          AND expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(actor_id)
    .bind(scope)
    .bind(key.as_str())
    .fetch_optional(&mut **transaction)
    .await?;

    if let Some(existing) = existing {
        if existing.request_hash.as_slice() != request_hash.as_bytes() {
            return Err(AppError::Conflict("idempotency_key", "payload mismatch"));
        }
        let status = existing
            .response_status
            .and_then(|value| u16::try_from(value).ok())
            .and_then(|value| StatusCode::from_u16(value).ok())
            .ok_or(AppError::Conflict("idempotency_key", "command in progress"))?;
        let response_body = existing
            .response_body
            .ok_or(AppError::Conflict("idempotency_key", "command in progress"))?;
        return Ok(IdempotencyOutcome::Replay(IdempotencyReplay {
            status,
            response_body,
        }));
    }

    sqlx::query(
        r#"
        INSERT INTO idempotency_keys (
            id, actor_id, scope, key, request_hash, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, now() + interval '24 hours')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(scope)
    .bind(key.as_str())
    .bind(request_hash.as_bytes().as_slice())
    .execute(&mut **transaction)
    .await?;

    Ok(IdempotencyOutcome::Reserved(IdempotencyReservation {
        actor_id,
        scope: scope.to_owned(),
        key: key.clone(),
        request_hash,
    }))
}

pub async fn complete<T: Serialize>(
    transaction: &mut Transaction<'_, Postgres>,
    reservation: &IdempotencyReservation,
    status: StatusCode,
    response_body: &T,
) -> Result<(), AppError> {
    let response_body = serde_json::to_value(response_body)
        .map_err(|_| AppError::Internal("serialize idempotent response"))?;
    let updated = sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET response_status = $5, response_body = $6
        WHERE actor_id = $1 AND scope = $2 AND key = $3
          AND request_hash = $4
          AND response_status IS NULL
          AND response_body IS NULL
          AND expires_at > now()
        "#,
    )
    .bind(reservation.actor_id)
    .bind(&reservation.scope)
    .bind(reservation.key.as_str())
    .bind(reservation.request_hash.as_bytes().as_slice())
    .bind(i32::from(status.as_u16()))
    .bind(response_body)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("complete idempotent command"));
    }
    Ok(())
}

fn valid_idempotency_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDEMPOTENCY_KEY_BYTES
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn validate_scope(scope: &str) -> Result<(), AppError> {
    let mut characters = scope.chars();
    let valid = !scope.is_empty()
        && scope.len() <= MAX_IDEMPOTENCY_SCOPE_BYTES
        && characters
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '.')
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::Internal("invalid idempotency scope"))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::{
        IdempotencyKey, IdempotencyReplay, MAX_IDEMPOTENCY_KEY_BYTES, RequestHash,
        resource_bound_scope,
    };

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Example {
        conversation_id: Uuid,
        text: String,
    }

    #[test]
    fn parses_the_existing_bounded_header_contract() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static(" message.01_retry-2 "),
        );
        let key = IdempotencyKey::from_headers(&headers).expect("valid key");
        assert_eq!(key.as_str(), "message.01_retry-2");
        assert_eq!(format!("{key:?}"), "IdempotencyKey([redacted])");

        for invalid in [
            "",
            "contains a space",
            "contains/slash",
            &"x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1),
        ] {
            assert!(IdempotencyKey::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn resource_scopes_bind_the_same_key_to_one_conversation() {
        let first = resource_bound_scope(
            "conversations",
            Uuid::parse_str("019fb8b0-0000-7000-8000-000000000001").expect("UUID"),
            "messages.create",
        )
        .expect("first scope");
        let second = resource_bound_scope(
            "conversations",
            Uuid::parse_str("019fb8b0-0000-7000-8000-000000000002").expect("UUID"),
            "messages.create",
        )
        .expect("second scope");
        assert_eq!(
            first,
            "conversations.019fb8b0000070008000000000000001.messages.create"
        );
        assert_ne!(first, second);
        assert!(resource_bound_scope("Conversations", Uuid::now_v7(), "create").is_err());
    }

    #[test]
    fn request_hash_is_stable_and_payload_sensitive() {
        let conversation_id = Uuid::now_v7();
        let first = Example {
            conversation_id,
            text: "hello".to_owned(),
        };
        let same = Example {
            conversation_id,
            text: "hello".to_owned(),
        };
        let changed = Example {
            conversation_id,
            text: "goodbye".to_owned(),
        };
        assert_eq!(
            RequestHash::for_value(&first).expect("first hash"),
            RequestHash::for_value(&same).expect("same hash")
        );
        assert_ne!(
            RequestHash::for_value(&first).expect("first hash"),
            RequestHash::for_value(&changed).expect("changed hash")
        );
    }

    #[test]
    fn stored_replays_decode_to_the_callers_typed_response() {
        let expected = Example {
            conversation_id: Uuid::now_v7(),
            text: "canonical response".to_owned(),
        };
        let replay = IdempotencyReplay {
            status: StatusCode::CREATED,
            response_body: serde_json::to_value(&expected).expect("response JSON"),
        };
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(replay.decode::<Example>().expect("typed replay"), expected);
    }
}
