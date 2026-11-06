//! S7 and S9: a person's own account. Change the password, see where
//! you're signed in and sign out elsewhere, turn on TOTP two-factor for
//! password sign-in, make personal API tokens, and keep your appearance on
//! the account so it follows you between devices.
//!
//! Everything here acts on the signed-in person only; nobody manages
//! someone else's account through these routes.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::hmac;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::USER_AGENT},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AppState,
    auth::{
        AuthenticatedActor, SessionIdentity, establish_session_in_transaction, password,
        require_allowed_origin, require_authenticated_write, respond_with_established_session,
    },
    error::{ApiJson, ApiPath, AppError},
    mcp,
    secrets::{self, DetachedSeal, WrappedKey},
};

/// How long the step between a correct password and a correct code lasts.
const CHALLENGE_MINUTES: i32 = 5;
/// Wrong codes allowed on one challenge before signing in starts again.
const CHALLENGE_ATTEMPTS: i32 = 5;
const TOTP_STEP_SECONDS: u64 = 30;
const TOTP_DIGITS: u32 = 6;
const TOTP_SECRET_BYTES: usize = 20;
const RECOVERY_CODES: usize = 10;
const ISSUER: &str = "Agent Workspace";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login/two-factor", post(login_two_factor))
        .route("/api/v1/account/password", post(change_password))
        .route("/api/v1/account/sessions", get(list_sessions))
        .route(
            "/api/v1/account/sessions/{session_id}/revoke",
            post(revoke_session),
        )
        .route(
            "/api/v1/account/sessions/revoke-others",
            post(revoke_other_sessions),
        )
        .route("/api/v1/account/security", get(security_overview))
        .route("/api/v1/account/two-factor/setup", post(two_factor_setup))
        .route(
            "/api/v1/account/two-factor/confirm",
            post(two_factor_confirm),
        )
        .route(
            "/api/v1/account/two-factor/recovery-codes",
            post(two_factor_recovery_codes),
        )
        .route(
            "/api/v1/account/two-factor/disable",
            post(two_factor_disable),
        )
        .route(
            "/api/v1/account/api-tokens",
            get(list_api_tokens).post(create_api_token),
        )
        .route(
            "/api/v1/account/api-tokens/{credential_id}/revoke",
            post(revoke_api_token),
        )
        .route(
            "/api/v1/account/appearance",
            get(read_appearance).put(write_appearance),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

// ---------------------------------------------------------------------------
// Sessions: where you're signed in

/// Note how a new session started, for the person's list of devices.
pub(crate) async fn record_sign_in_method(
    connection: &mut PgConnection,
    session_id: Uuid,
    method: &'static str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE sessions SET sign_in_method = $2 WHERE id = $1")
        .bind(session_id)
        .bind(method)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// A session that replaces another (accepting an invitation while signed
/// in) keeps how the person signed in.
pub(crate) async fn copy_session_details(
    connection: &mut PgConnection,
    from_session_id: Uuid,
    to_session_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE sessions target
        SET sign_in_method = source.sign_in_method,
            user_agent = COALESCE(target.user_agent, source.user_agent)
        FROM sessions source
        WHERE source.id = $1 AND target.id = $2
        "#,
    )
    .bind(from_session_id)
    .bind(to_session_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// The browser's description of itself, bounded, for the list of devices.
pub(crate) fn user_agent(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(USER_AGENT)?.to_str().ok()?;
    let cleaned = value
        .chars()
        .filter(|character| !character.is_control())
        .take(300)
        .collect::<String>();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    id: Uuid,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    user_agent: Option<String>,
    sign_in_method: String,
    #[sqlx(skip)]
    current: bool,
}

#[derive(Debug, Serialize)]
struct SessionsResponse {
    sessions: Vec<SessionView>,
}

async fn list_sessions(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<SessionsResponse>, AppError> {
    let mut sessions = sqlx::query_as::<_, SessionView>(
        r#"
        SELECT session.id, session.created_at, session.last_seen_at,
               LEAST(session.idle_expires_at, session.absolute_expires_at) AS expires_at,
               session.user_agent,
               COALESCE(session.sign_in_method, 'password') AS sign_in_method
        FROM sessions session
        JOIN users user_account ON user_account.id = session.user_id
        WHERE session.user_id = $1 AND session.revoked_at IS NULL
          AND session.idle_expires_at > now() AND session.absolute_expires_at > now()
          AND session.authentication_epoch = user_account.authentication_epoch
        ORDER BY session.last_seen_at DESC, session.id
        LIMIT 100
        "#,
    )
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    for session in &mut sessions {
        session.current = session.id == principal.session_id;
    }
    Ok(Json(SessionsResponse { sessions }))
}

async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(session_id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if session_id == principal.session_id {
        return Err(AppError::Conflict("session", "sign out to end this one"));
    }
    let mut transaction = state.pool.begin().await?;
    let revoked = sqlx::query(
        r#"
        UPDATE sessions SET revoked_at = now(), updated_at = now()
        WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(session_id)
    .bind(principal.user_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(AppError::NotFound("session"));
    }
    account_audit(
        &mut transaction,
        &principal,
        "account.session.revoke",
        "session",
        Some(session_id),
        serde_json::json!({}),
    )
    .await?;
    transaction.commit().await?;
    state.realtime.revoke_session(session_id);
    crate::auth::workos::revoke_provider_session(&state, session_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// End every session of `user_id` but `keep`. Returns the ones ended, for
/// the caller to drop from realtime once the transaction commits.
async fn revoke_sessions_except(
    connection: &mut PgConnection,
    user_id: Uuid,
    keep: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE sessions SET revoked_at = now(), updated_at = now()
        WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(keep)
    .fetch_all(&mut *connection)
    .await?)
}

#[derive(Debug, Serialize)]
struct RevokedResponse {
    revoked: usize,
}

async fn revoke_other_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<RevokedResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let revoked =
        revoke_sessions_except(&mut transaction, principal.user_id, principal.session_id).await?;
    account_audit(
        &mut transaction,
        &principal,
        "account.session.revoke_others",
        "session",
        None,
        serde_json::json!({ "count": revoked.len() }),
    )
    .await?;
    transaction.commit().await?;
    for session_id in &revoked {
        state.realtime.revoke_session(*session_id);
        crate::auth::workos::revoke_provider_session(&state, *session_id).await;
    }
    Ok(Json(RevokedResponse {
        revoked: revoked.len(),
    }))
}

// ---------------------------------------------------------------------------
// Password

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

impl Drop for ChangePasswordRequest {
    fn drop(&mut self) {
        self.current_password.zeroize();
        self.new_password.zeroize();
    }
}

/// Check the signed-in person's password, rate limited like sign-in.
/// `None` when the account has no password (it signs in another way).
async fn verify_current_password(
    state: &AppState,
    principal: &AuthenticatedActor,
    submitted: &mut String,
) -> Result<Option<()>, AppError> {
    let submitted = Zeroizing::new(std::mem::take(submitted));
    state.rate_limits.check_login(&principal.login_name)?;
    let stored = sqlx::query_scalar::<_, String>(
        "SELECT password_phc FROM user_password_credentials WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if !(12..=1024).contains(&submitted.len()) {
        return Err(AppError::InvalidCredentials);
    }
    let _permit = state.rate_limits.try_acquire_password_kdf()?;
    if password::verify_password(submitted.to_string(), stored).await? {
        Ok(Some(()))
    } else {
        Err(AppError::InvalidCredentials)
    }
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(mut request): ApiJson<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if !(12..=1024).contains(&request.new_password.len()) {
        return Err(AppError::Validation("new password"));
    }
    if verify_current_password(&state, &principal, &mut request.current_password)
        .await?
        .is_none()
    {
        return Err(AppError::Conflict(
            "password",
            "this account has no password",
        ));
    }
    let new_phc = {
        let _permit = state.rate_limits.try_acquire_password_kdf()?;
        password::hash_password(std::mem::take(&mut request.new_password)).await?
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE user_password_credentials
        SET password_phc = $2, version = version + 1, updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(principal.user_id)
    .bind(new_phc)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE login_challenges SET consumed_at = now() WHERE user_id = $1 AND consumed_at IS NULL")
        .bind(principal.user_id)
        .execute(&mut *transaction)
        .await?;
    // A reset link someone else asked for stops working once you choose a
    // new password yourself.
    crate::password_reset::end_live_resets(&mut transaction, principal.user_id).await?;
    let revoked =
        revoke_sessions_except(&mut transaction, principal.user_id, principal.session_id).await?;
    account_audit(
        &mut transaction,
        &principal,
        "account.password.change",
        "user",
        Some(principal.user_id),
        serde_json::json!({ "signedOutSessions": revoked.len() }),
    )
    .await?;
    transaction.commit().await?;
    for session_id in revoked {
        state.realtime.revoke_session(session_id);
        crate::auth::workos::revoke_provider_session(&state, session_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Two-factor

/// RFC 4648 base32 without padding, as authenticator apps expect.
fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u64 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u64::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    output
}

/// The code for one time step (RFC 4226 with HMAC-SHA1, as RFC 6238 and
/// every authenticator app use).
fn hotp(secret: &[u8], counter: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let digest = tag.as_ref();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    binary % 10_u32.pow(TOTP_DIGITS)
}

fn current_step() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() / TOTP_STEP_SECONDS)
        .unwrap_or(0)
}

/// The time step `code` is valid for, allowing one step of clock drift each
/// way and refusing any step at or before `last_used` (a code works once).
fn matching_step(secret: &[u8], code: &str, now: u64, last_used: Option<u64>) -> Option<u64> {
    let code = code.trim().replace(' ', "");
    if code.len() != TOTP_DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let submitted = code.parse::<u32>().ok()?;
    let mut found = None;
    for step in [now.saturating_sub(1), now, now + 1] {
        // Compare every candidate, so timing doesn't say which matched.
        let expected = hotp(secret, step);
        if bool::from(expected.to_be_bytes().ct_eq(&submitted.to_be_bytes()))
            && last_used.is_none_or(|last| step > last)
            && found.is_none()
        {
            found = Some(step);
        }
    }
    found
}

/// A QR code of `text` as a standalone SVG, black on white with its quiet
/// zone, so authenticator apps read it in either theme.
fn qr_svg(text: &str) -> Result<String, AppError> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|_| AppError::Internal("draw the two-factor QR code"))?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

fn otpauth_uri(secret_base32: &str, account_name: &str) -> String {
    let label = format!("{ISSUER}:{account_name}");
    let mut uri = url::Url::parse("otpauth://totp/").expect("static URI");
    uri.set_path(&label);
    uri.query_pairs_mut()
        .append_pair("secret", secret_base32)
        .append_pair("issuer", ISSUER)
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", "6")
        .append_pair("period", "30");
    uri.to_string()
}

#[derive(FromRow)]
pub(crate) struct StoredFactor {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_key: Vec<u8>,
    wrap_nonce: Option<Vec<u8>>,
    master_key_id: String,
    pub(crate) confirmed_at: Option<DateTime<Utc>>,
    last_used_step: Option<i64>,
}

impl StoredFactor {
    fn sealed(&self) -> Result<DetachedSeal, AppError> {
        Ok(DetachedSeal {
            ciphertext: self.ciphertext.clone(),
            nonce: self
                .nonce
                .as_slice()
                .try_into()
                .map_err(|_| AppError::Internal("two-factor nonce"))?,
            wrapped: WrappedKey {
                master_key_id: self.master_key_id.clone(),
                nonce: self
                    .wrap_nonce
                    .as_deref()
                    .map(|nonce| nonce.try_into())
                    .transpose()
                    .map_err(|_| AppError::Internal("two-factor nonce"))?,
                ciphertext: self.wrapped_key.clone(),
            },
        })
    }
}

pub(crate) async fn load_factor(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<StoredFactor>, AppError> {
    Ok(sqlx::query_as::<_, StoredFactor>(
        r#"
        SELECT ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id,
               confirmed_at, last_used_step
        FROM user_totp_factors WHERE user_id = $1
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Check a TOTP code against the person's factor and, when it matches,
/// remember its step so it can't be used again. Call inside a transaction
/// (the factor row is locked).
pub(crate) async fn accept_totp(
    connection: &mut PgConnection,
    state: &AppState,
    user_id: Uuid,
    factor: &StoredFactor,
    code: &str,
) -> Result<bool, AppError> {
    let keys = secrets::keyring(&state.config)?;
    let secret = secrets::open_detached(
        keys.as_ref(),
        &secrets::totp_aad(user_id),
        &factor.sealed()?,
    )?;
    let last_used = factor
        .last_used_step
        .and_then(|step| u64::try_from(step).ok());
    let Some(step) = matching_step(&secret, code, current_step(), last_used) else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE user_totp_factors SET last_used_step = $2, updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(i64::try_from(step).map_err(|_| AppError::Internal("time step"))?)
    .execute(&mut *connection)
    .await?;
    Ok(true)
}

/// A recovery code looks like `abcde-12345`.
fn normalize_recovery_code(code: &str) -> Option<String> {
    let cleaned = code
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    (cleaned.len() == 10).then_some(cleaned)
}

fn recovery_digest(user_id: Uuid, normalized: &str) -> [u8; 32] {
    Sha256::digest(format!("aw-recovery:v1:{user_id}:{normalized}").as_bytes()).into()
}

fn generate_recovery_codes() -> Vec<String> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut bytes = [0_u8; 10];
            OsRng.fill_bytes(&mut bytes);
            let characters = bytes
                .iter()
                .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
                .collect::<String>();
            format!("{}-{}", &characters[..5], &characters[5..])
        })
        .collect()
}

async fn replace_recovery_codes(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<Vec<String>, AppError> {
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *connection)
        .await?;
    let codes = generate_recovery_codes();
    for code in &codes {
        let normalized =
            normalize_recovery_code(code).ok_or(AppError::Internal("recovery code"))?;
        sqlx::query(
            "INSERT INTO user_recovery_codes (id, user_id, code_digest) VALUES ($1, $2, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(recovery_digest(user_id, &normalized).to_vec())
        .execute(&mut *connection)
        .await?;
    }
    Ok(codes)
}

/// Use a recovery code once. Returns whether it was one of the person's.
pub(crate) async fn accept_recovery_code(
    connection: &mut PgConnection,
    user_id: Uuid,
    code: &str,
) -> Result<bool, AppError> {
    let Some(normalized) = normalize_recovery_code(code) else {
        return Ok(false);
    };
    Ok(sqlx::query(
        r#"
        UPDATE user_recovery_codes SET used_at = now()
        WHERE user_id = $1 AND code_digest = $2 AND used_at IS NULL
        "#,
    )
    .bind(user_id)
    .bind(recovery_digest(user_id, &normalized).to_vec())
    .execute(&mut *connection)
    .await?
    .rows_affected()
        == 1)
}

/// Whether the person has two-factor on (a confirmed factor).
pub(crate) async fn two_factor_enabled(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM user_totp_factors WHERE user_id = $1 AND confirmed_at IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_one(&mut *connection)
    .await?)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeResponse {
    authenticated: bool,
    two_factor_required: bool,
    challenge: String,
}

/// After a correct password for someone with two-factor on: no session yet,
/// only a challenge to answer with a code.
pub(crate) async fn start_challenge(
    connection: &mut PgConnection,
    user_id: Uuid,
    authentication_epoch: i64,
) -> Result<Response, AppError> {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let challenge = format!("aw_2fa_{}", URL_SAFE_NO_PAD.encode(bytes));
    bytes.zeroize();
    sqlx::query("DELETE FROM login_challenges WHERE expires_at < now() - interval '1 day'")
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO login_challenges (id, user_id, token_digest, authentication_epoch, expires_at)
        VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(Sha256::digest(challenge.as_bytes()).to_vec())
    .bind(authentication_epoch)
    .bind(CHALLENGE_MINUTES)
    .execute(&mut *connection)
    .await?;
    Ok((
        StatusCode::OK,
        Json(ChallengeResponse {
            authenticated: false,
            two_factor_required: true,
            challenge,
        }),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TwoFactorLoginRequest {
    challenge: String,
    code: String,
}

impl Drop for TwoFactorLoginRequest {
    fn drop(&mut self) {
        self.challenge.zeroize();
        self.code.zeroize();
    }
}

#[derive(FromRow)]
struct ChallengeRow {
    id: Uuid,
    user_id: Uuid,
    authentication_epoch: i64,
    failed_attempts: i32,
    actor_id: Uuid,
    login_name: String,
    display_name: String,
    current_epoch: i64,
}

async fn login_two_factor(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<TwoFactorLoginRequest>,
) -> Result<Response, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    if !request.challenge.starts_with("aw_2fa_") || request.challenge.len() > 100 {
        return Err(AppError::NotFound("login challenge"));
    }
    let digest: [u8; 32] = Sha256::digest(request.challenge.as_bytes()).into();
    let mut transaction = state.pool.begin().await?;
    let challenge = sqlx::query_as::<_, ChallengeRow>(
        r#"
        SELECT challenge.id, challenge.user_id, challenge.authentication_epoch,
               challenge.failed_attempts, user_account.actor_id, user_account.login_name,
               user_account.display_name, user_account.authentication_epoch AS current_epoch
        FROM login_challenges challenge
        JOIN users user_account ON user_account.id = challenge.user_id
         AND user_account.status = 'active'
        JOIN actors actor ON actor.id = user_account.actor_id
         AND actor.status = 'active' AND actor.kind = 'human'
        WHERE challenge.token_digest = $1 AND challenge.consumed_at IS NULL
          AND challenge.expires_at > now() AND challenge.failed_attempts < $2
        FOR UPDATE OF challenge
        "#,
    )
    .bind(digest.to_vec())
    .bind(CHALLENGE_ATTEMPTS)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("login challenge"))?;
    if challenge.authentication_epoch != challenge.current_epoch {
        return Err(AppError::NotFound("login challenge"));
    }
    state.rate_limits.check_login(&challenge.login_name)?;
    let factor = load_factor(&mut transaction, challenge.user_id)
        .await?
        .filter(|factor| factor.confirmed_at.is_some())
        .ok_or(AppError::NotFound("login challenge"))?;
    let (accepted, used_recovery) = if accept_totp(
        &mut transaction,
        &state,
        challenge.user_id,
        &factor,
        &request.code,
    )
    .await?
    {
        (true, false)
    } else if accept_recovery_code(&mut transaction, challenge.user_id, &request.code).await? {
        (true, true)
    } else {
        (false, false)
    };
    if !accepted {
        sqlx::query(
            "UPDATE login_challenges SET failed_attempts = failed_attempts + 1 WHERE id = $1",
        )
        .bind(challenge.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_events (
                id, actor_id, authentication_class, action, target_kind, target_id,
                outcome, channel, metadata
            ) VALUES ($1, $2, 'password_session', 'authentication.two_factor', 'user', $3,
                      'denied', 'browser', '{}'::jsonb)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(challenge.actor_id)
        .bind(challenge.user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let _ = challenge.failed_attempts;
        return Err(AppError::InvalidCredentials);
    }
    sqlx::query("UPDATE login_challenges SET consumed_at = now() WHERE id = $1")
        .bind(challenge.id)
        .execute(&mut *transaction)
        .await?;
    let established = establish_session_in_transaction(
        &state.config,
        &headers,
        &mut transaction,
        SessionIdentity {
            user_id: challenge.user_id,
            actor_id: challenge.actor_id,
            actor_kind: "human".to_owned(),
            login_name: challenge.login_name,
            display_name: challenge.display_name,
            authentication_epoch: challenge.current_epoch,
        },
    )
    .await?;
    record_sign_in_method(
        &mut transaction,
        established.principal.session_id,
        "password",
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, session_id, authentication_class, action,
            target_kind, target_id, outcome, channel, metadata
        )
        VALUES (
            $1, $2, $3, 'password_session', 'authentication.login',
            'session', $3, 'succeeded', 'browser', $4
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(established.principal.actor_id)
    .bind(established.principal.session_id)
    .bind(serde_json::json!({ "twoFactor": if used_recovery { "recovery_code" } else { "totp" } }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    respond_with_established_session(&state, established).await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TwoFactorState {
    available: bool,
    enabled: bool,
    pending: bool,
    recovery_codes_left: i64,
    enabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecurityEvent {
    id: Uuid,
    action: String,
    occurred_at: DateTime<Utc>,
    outcome: String,
    channel: Option<String>,
    client_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecurityOverview {
    has_password: bool,
    two_factor: TwoFactorState,
    recent_events: Vec<SecurityEvent>,
}

async fn security_overview(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<SecurityOverview>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let has_password = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM user_password_credentials WHERE user_id = $1)",
    )
    .bind(principal.user_id)
    .fetch_one(&mut *connection)
    .await?;
    let factor = sqlx::query_as::<_, (Option<DateTime<Utc>>,)>(
        "SELECT confirmed_at FROM user_totp_factors WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *connection)
    .await?;
    let recovery_codes_left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(principal.user_id)
    .fetch_one(&mut *connection)
    .await?;
    let recent_events = sqlx::query_as::<_, SecurityEvent>(
        r#"
        SELECT id, action, occurred_at, outcome, channel, client_name
        FROM audit_events
        WHERE actor_id = $1
          AND (action LIKE 'authentication.%' OR action LIKE 'account.%')
        ORDER BY occurred_at DESC, id DESC
        LIMIT 20
        "#,
    )
    .bind(principal.actor_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(Json(SecurityOverview {
        has_password,
        two_factor: TwoFactorState {
            available: secrets::keyring(&state.config).is_ok(),
            enabled: factor.as_ref().is_some_and(|factor| factor.0.is_some()),
            pending: factor.as_ref().is_some_and(|factor| factor.0.is_none()),
            recovery_codes_left,
            enabled_at: factor.and_then(|factor| factor.0),
        },
        recent_events,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PasswordConfirmation {
    current_password: String,
}

impl Drop for PasswordConfirmation {
    fn drop(&mut self) {
        self.current_password.zeroize();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupResponse {
    secret: String,
    otpauth_uri: String,
    /// The same link as a QR code, drawn here so no third party sees the
    /// secret and the browser needs no QR library.
    qr_svg: String,
    issuer: &'static str,
    account_name: String,
}

async fn two_factor_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(mut request): ApiJson<PasswordConfirmation>,
) -> Result<Json<SetupResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let keys = secrets::keyring(&state.config)?;
    if verify_current_password(&state, &principal, &mut request.current_password)
        .await?
        .is_none()
    {
        return Err(AppError::Conflict(
            "two_factor",
            "two-factor protects password sign-in; this account has no password",
        ));
    }
    let mut secret = Zeroizing::new([0_u8; TOTP_SECRET_BYTES]);
    OsRng.fill_bytes(secret.as_mut());
    let sealed = secrets::seal_detached(
        keys.as_ref(),
        &secrets::totp_aad(principal.user_id),
        secret.as_ref(),
    )?;
    let mut transaction = state.pool.begin().await?;
    secrets::hold_off_rotation(&mut transaction, keys.as_ref()).await?;
    if two_factor_enabled(&mut transaction, principal.user_id).await? {
        return Err(AppError::Conflict("two_factor", "already on"));
    }
    sqlx::query(
        r#"
        INSERT INTO user_totp_factors (
            user_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id
        ) VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_id) DO UPDATE
        SET ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce,
            wrapped_key = EXCLUDED.wrapped_key, wrap_nonce = EXCLUDED.wrap_nonce,
            master_key_id = EXCLUDED.master_key_id, confirmed_at = NULL,
            last_used_step = NULL, created_at = now(), updated_at = now()
        "#,
    )
    .bind(principal.user_id)
    .bind(&sealed.ciphertext)
    .bind(sealed.nonce.as_slice())
    .bind(&sealed.wrapped.ciphertext)
    .bind(sealed.wrapped.nonce.as_ref().map(|nonce| nonce.as_slice()))
    .bind(&sealed.wrapped.master_key_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let encoded = base32_encode(secret.as_ref());
    let uri = otpauth_uri(&encoded, &principal.login_name);
    Ok(Json(SetupResponse {
        qr_svg: qr_svg(&uri)?,
        otpauth_uri: uri,
        secret: encoded,
        issuer: ISSUER,
        account_name: principal.login_name.clone(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CodeRequest {
    code: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryCodesResponse {
    recovery_codes: Vec<String>,
}

async fn two_factor_confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CodeRequest>,
) -> Result<Json<RecoveryCodesResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    state.rate_limits.check_login(&principal.login_name)?;
    let mut transaction = state.pool.begin().await?;
    let factor = load_factor(&mut transaction, principal.user_id)
        .await?
        .ok_or(AppError::NotFound("two-factor setup"))?;
    if factor.confirmed_at.is_some() {
        return Err(AppError::Conflict("two_factor", "already on"));
    }
    if !accept_totp(
        &mut transaction,
        &state,
        principal.user_id,
        &factor,
        &request.code,
    )
    .await?
    {
        return Err(AppError::Validation("code"));
    }
    sqlx::query(
        "UPDATE user_totp_factors SET confirmed_at = now(), updated_at = now() WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .execute(&mut *transaction)
    .await?;
    let recovery_codes = replace_recovery_codes(&mut transaction, principal.user_id).await?;
    account_audit(
        &mut transaction,
        &principal,
        "account.two_factor.enable",
        "user",
        Some(principal.user_id),
        serde_json::json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(RecoveryCodesResponse { recovery_codes }))
}

async fn two_factor_recovery_codes(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CodeRequest>,
) -> Result<Json<RecoveryCodesResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    state.rate_limits.check_login(&principal.login_name)?;
    let mut transaction = state.pool.begin().await?;
    let factor = load_factor(&mut transaction, principal.user_id)
        .await?
        .filter(|factor| factor.confirmed_at.is_some())
        .ok_or(AppError::NotFound("two-factor"))?;
    if !accept_totp(
        &mut transaction,
        &state,
        principal.user_id,
        &factor,
        &request.code,
    )
    .await?
    {
        return Err(AppError::Validation("code"));
    }
    let recovery_codes = replace_recovery_codes(&mut transaction, principal.user_id).await?;
    account_audit(
        &mut transaction,
        &principal,
        "account.two_factor.recovery_codes",
        "user",
        Some(principal.user_id),
        serde_json::json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(RecoveryCodesResponse { recovery_codes }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DisableRequest {
    current_password: String,
    code: String,
}

impl Drop for DisableRequest {
    fn drop(&mut self) {
        self.current_password.zeroize();
        self.code.zeroize();
    }
}

async fn two_factor_disable(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(mut request): ApiJson<DisableRequest>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if verify_current_password(&state, &principal, &mut request.current_password)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("two-factor"));
    }
    let mut transaction = state.pool.begin().await?;
    let factor = load_factor(&mut transaction, principal.user_id)
        .await?
        .ok_or(AppError::NotFound("two-factor"))?;
    if factor.confirmed_at.is_some()
        && !accept_totp(
            &mut transaction,
            &state,
            principal.user_id,
            &factor,
            &request.code,
        )
        .await?
        && !accept_recovery_code(&mut transaction, principal.user_id, &request.code).await?
    {
        return Err(AppError::Validation("code"));
    }
    sqlx::query("DELETE FROM user_totp_factors WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *transaction)
        .await?;
    account_audit(
        &mut transaction,
        &principal,
        "account.two_factor.disable",
        "user",
        Some(principal.user_id),
        serde_json::json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Personal API tokens: MCP bearer tokens for one project, acting as you

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenView {
    id: Uuid,
    project_id: Uuid,
    project_name: String,
    project_key: String,
    name: String,
    token_prefix: String,
    scopes: Vec<String>,
    status: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    version: i64,
}

const TOKEN_COLUMNS: &str = r#"
    credential.id, credential.project_id, project.name AS project_name,
    project.project_key, credential.client_name AS name, credential.token_prefix,
    credential.scopes,
    CASE WHEN credential.status = 'active' AND credential.expires_at <= now()
         THEN 'expired' ELSE credential.status END AS status,
    credential.created_at, credential.last_used_at, credential.expires_at,
    credential.revoked_at, credential.version
    FROM mcp_client_credentials credential
    JOIN projects project ON project.id = credential.project_id
"#;

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenProject {
    id: Uuid,
    name: String,
    key: String,
    can_write: bool,
}

#[derive(Debug, Serialize)]
struct TokensResponse {
    tokens: Vec<TokenView>,
    projects: Vec<TokenProject>,
}

async fn list_api_tokens(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<TokensResponse>, AppError> {
    // Tokens a person made and those an operator made for them, but not an
    // agent's: those belong to the agent's connection.
    let tokens = sqlx::query_as::<_, TokenView>(&format!(
        r#"
        SELECT {TOKEN_COLUMNS}
        WHERE credential.actor_id = $1 AND credential.external_agent_connection_id IS NULL
          AND credential.token_prefix LIKE 'aw\_mcp\_%'
        ORDER BY (credential.status = 'active' AND credential.expires_at > now()) DESC,
                 credential.created_at DESC, credential.id DESC
        LIMIT 200
        "#
    ))
    .bind(principal.actor_id)
    .fetch_all(&state.pool)
    .await?;
    let projects = sqlx::query_as::<_, TokenProject>(
        r#"
        SELECT project.id, project.name, project.project_key AS key,
               -- As issuing checks: a role that may write anything beyond seeing.
               (membership.role IN ('owner', 'admin', 'member')
                AND (membership.role_id IS NULL OR EXISTS (
                    SELECT 1 FROM unnest(workspace_role_permissions(membership.role_id)) granted
                    WHERE granted NOT IN ('projects.view', 'tasks.view', 'code.view', 'members.view')
                ))) AS can_write
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
         AND project.archived_at IS NULL
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = membership.actor_id
         AND organization_membership.enabled
        WHERE membership.actor_id = $1 AND membership.enabled
          AND membership.role IN ('owner', 'admin', 'member', 'viewer')
        ORDER BY (project.kind = 'direct'), lower(project.name), project.id
        "#,
    )
    .bind(principal.actor_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(TokensResponse { tokens, projects }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateTokenRequest {
    project_id: Uuid,
    name: String,
    scopes: Vec<String>,
    expires_in_days: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedToken {
    token: String,
    credential: TokenView,
}

fn validate_token_name(name: &str) -> Result<(), AppError> {
    let mut characters = name.chars();
    if name.is_empty()
        || name.chars().count() > 80
        || !characters.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-'))
    {
        return Err(AppError::Validation("token name"));
    }
    Ok(())
}

async fn fetch_token(pool: &sqlx::PgPool, credential_id: Uuid) -> Result<TokenView, AppError> {
    sqlx::query_as::<_, TokenView>(&format!("SELECT {TOKEN_COLUMNS} WHERE credential.id = $1"))
        .bind(credential_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("api token"))
}

async fn create_api_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreatedToken>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = request.name.trim().to_owned();
    validate_token_name(&name)?;
    let mut scopes = request.scopes.clone();
    scopes.sort();
    scopes.dedup();
    // `issue_credential` checks the person may use these scopes in the
    // project right now.
    let issued = mcp::issue_credential_as(
        &state.pool,
        mcp::IssueCredentialCommand {
            project_id: request.project_id,
            actor_id: principal.actor_id,
            client_name: name,
            scopes,
            expires_in_days: request.expires_in_days,
        },
        mcp::CredentialIssuer::Browser {
            session_id: principal.session_id,
        },
    )
    .await?;
    let credential = fetch_token(&state.pool, issued.credential_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            token: issued.token,
            credential,
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevokeTokenRequest {
    expected_version: i64,
}

async fn revoke_api_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(credential_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RevokeTokenRequest>,
) -> Result<Json<TokenView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let owner = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor_id FROM mcp_client_credentials
        WHERE id = $1 AND external_agent_connection_id IS NULL
        "#,
    )
    .bind(credential_id)
    .fetch_optional(&state.pool)
    .await?;
    if owner != Some(principal.actor_id) {
        return Err(AppError::NotFound("api token"));
    }
    mcp::revoke_credential_as(
        &state.pool,
        credential_id,
        request.expected_version,
        mcp::CredentialIssuer::Browser {
            session_id: principal.session_id,
        },
    )
    .await?;
    Ok(Json(fetch_token(&state.pool, credential_id).await?))
}

// ---------------------------------------------------------------------------
// Appearance

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppearanceResponse {
    appearance: Option<Value>,
    version: i64,
}

async fn read_appearance(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<AppearanceResponse>, AppError> {
    let (appearance, version) = sqlx::query_as::<_, (Option<Value>, i64)>(
        "SELECT appearance, appearance_version FROM users WHERE id = $1",
    )
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(AppearanceResponse {
        appearance,
        version,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppearanceRequest {
    appearance: Value,
    expected_version: i64,
}

/// The appearance is the browser's own format; the server keeps it whole
/// and only bounds it.
fn validate_appearance(value: &Value) -> Result<(), AppError> {
    let Value::Object(object) = value else {
        return Err(AppError::Validation("appearance"));
    };
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 8192) || object.len() > 32 {
        return Err(AppError::Validation("appearance"));
    }
    Ok(())
}

async fn write_appearance(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<AppearanceRequest>,
) -> Result<Json<AppearanceResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    validate_appearance(&request.appearance)?;
    let updated = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE users
        SET appearance = $2, appearance_version = appearance_version + 1, updated_at = now()
        WHERE id = $1 AND appearance_version = $3
        RETURNING appearance_version
        "#,
    )
    .bind(principal.user_id)
    .bind(&request.appearance)
    .bind(request.expected_version)
    .fetch_optional(&state.pool)
    .await?;
    let Some(version) = updated else {
        let current =
            sqlx::query_scalar::<_, i64>("SELECT appearance_version FROM users WHERE id = $1")
                .bind(principal.user_id)
                .fetch_one(&state.pool)
                .await?;
        return Err(AppError::VersionConflict {
            resource: "appearance",
            current_version: current,
        });
    };
    Ok(Json(AppearanceResponse {
        appearance: Some(request.appearance),
        version,
    }))
}

// ---------------------------------------------------------------------------
// Audit

async fn account_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, session_id, authentication_class, action,
            target_kind, target_id, outcome, channel, metadata
        ) VALUES ($1, $2, $3, 'password_session', $4, $5, $6, 'succeeded', 'browser', $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_the_rfc_6238_test_vectors() {
        // RFC 6238 appendix B, SHA-1, eight digits truncated to six.
        let secret = b"12345678901234567890";
        assert_eq!(hotp(secret, 59 / 30), 287_082);
        assert_eq!(hotp(secret, 1_111_111_109 / 30), 81_804);
        assert_eq!(hotp(secret, 1_234_567_890 / 30), 5_924);
        assert_eq!(hotp(secret, 2_000_000_000 / 30), 279_037);
    }

    #[test]
    fn a_code_works_once_and_within_one_step_of_drift() {
        let secret = b"12345678901234567890";
        let now = 1_000_000;
        let code = format!("{:06}", hotp(secret, now));
        assert_eq!(matching_step(secret, &code, now, None), Some(now));
        assert_eq!(matching_step(secret, &code, now + 1, None), Some(now));
        assert_eq!(matching_step(secret, &code, now + 2, None), None);
        assert_eq!(matching_step(secret, &code, now, Some(now)), None);
        assert_eq!(matching_step(secret, "12345", now, None), None);
        assert_eq!(matching_step(secret, "abcdef", now, None), None);
    }

    #[test]
    fn base32_round_trips_the_rfc_4648_examples() {
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_encode(b"fo"), "MZXQ");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn the_qr_code_is_a_standalone_svg_without_scripts() {
        let svg =
            qr_svg("otpauth://totp/Agent%20Workspace:mischa?secret=MZXW6YTBOI").expect("qr code");
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"), "{svg}");
        assert!(svg.contains("</svg>"));
        assert!(!svg.to_lowercase().contains("<script"));
        assert!(
            !svg.contains("MZXW6YTBOI"),
            "the secret is drawn, not written out"
        );
    }

    #[test]
    fn the_otpauth_uri_names_the_issuer_and_account() {
        let uri = otpauth_uri("MZXW6YTBOI", "mischa");
        assert!(uri.starts_with("otpauth://totp/Agent%20Workspace:mischa?secret=MZXW6YTBOI"));
        assert!(uri.contains("issuer=Agent+Workspace"));
    }

    #[test]
    fn recovery_codes_are_distinct_and_normalize() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let unique = codes.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), RECOVERY_CODES);
        for code in &codes {
            assert_eq!(code.len(), 11);
            assert_eq!(
                normalize_recovery_code(&code.to_uppercase())
                    .expect("test value")
                    .len(),
                10
            );
        }
        assert_eq!(normalize_recovery_code("short"), None);
    }

    #[test]
    fn token_names_follow_the_credential_rules() {
        assert!(validate_token_name("Laptop CLI").is_ok());
        assert!(validate_token_name(" leading").is_err());
        assert!(validate_token_name("").is_err());
        assert!(validate_token_name("emoji 🙂").is_err());
        assert!(validate_token_name(&"a".repeat(81)).is_err());
    }

    #[test]
    fn appearance_must_be_a_small_object() {
        assert!(validate_appearance(&serde_json::json!({ "theme": "indigo" })).is_ok());
        assert!(validate_appearance(&serde_json::json!(["indigo"])).is_err());
        assert!(validate_appearance(&serde_json::json!({ "x": "y".repeat(9000) })).is_err());
    }

    #[test]
    fn user_agents_are_bounded_and_clean() {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            "Mozilla/5.0\t(X11)".parse().expect("test value"),
        );
        assert_eq!(user_agent(&headers).as_deref(), Some("Mozilla/5.0(X11)"));
        headers.insert(USER_AGENT, "x".repeat(400).parse().expect("test value"));
        assert_eq!(user_agent(&headers).map(|agent| agent.len()), Some(300));
    }
}
