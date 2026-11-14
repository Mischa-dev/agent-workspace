//! W19: a person's email address, and "Forgot password?" by email.
//!
//! An address gets mail only once its owner opens the confirmation link,
//! so nobody can point resets or notifications at someone else's inbox.
//!
//! A reset link is single use, lasts 30 minutes, and dies as soon as the
//! password changes or the person signs out everywhere (it is tied to the
//! account's authentication epoch). Only a digest of it is stored, and it
//! exists only inside the job that emails it. Asking for one always gets
//! the same answer, whether or not the account exists, and the lookup runs
//! in that job, not in the request. Someone with two-factor on also needs
//! a code to reset, so a mailbox alone can't take over the account. A reset
//! signs the person out everywhere.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    auth::{AuthenticatedActor, password, require_allowed_origin, require_authenticated_write},
    error::{ApiJson, AppError},
    jobs::{self, JobError, JobRegistry, NewJob},
    mail::{self, Message},
};

const RESET_KIND: &str = "auth.password_reset_email";
const VERIFY_KIND: &str = "account.email_confirmation";
const RESET_PREFIX: &str = "aw_reset_";
const VERIFY_PREFIX: &str = "aw_email_";
pub const RESET_MINUTES: i32 = 30;
const VERIFY_HOURS: i32 = 24;
/// Wrong two-factor codes allowed on one link before it stops working.
const RESET_ATTEMPTS: i32 = 5;
/// At most one reset email a minute, and five an hour, per account.
const RESETS_PER_HOUR: i64 = 5;

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(RESET_KIND, |context| async move {
        let job: ResetJob = context.job.payload_as()?;
        send_reset(&context.state, &job.login).await
    });
    registry.register(VERIFY_KIND, |context| async move {
        let job: VerifyJob = context.job.payload_as()?;
        send_confirmation(&context.state, job.user_id).await
    });
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/account/email",
            get(read_email_http)
                .put(set_email_http)
                .delete(remove_email_http),
        )
        .route("/api/v1/account/email/resend", post(resend_http))
        .route("/api/v1/auth/verify-email", post(verify_http))
        .route(
            "/api/v1/auth/password-reset",
            get(reset_available_http).post(request_reset_http),
        )
        .route(
            "/api/v1/auth/password-reset/inspect",
            post(inspect_reset_http),
        )
        .route(
            "/api/v1/auth/password-reset/complete",
            post(complete_reset_http),
        )
}

fn token(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes));
    bytes.zeroize();
    token
}

pub fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn well_formed(token: &str, prefix: &str) -> bool {
    token.starts_with(prefix)
        && token.len() <= 100
        && token[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

// ---------------------------------------------------------------------------
// The person's address

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmailView {
    address: Option<String>,
    verified: bool,
    /// When the last confirmation link went out, while unconfirmed.
    confirmation_sent_at: Option<DateTime<Utc>>,
    /// Whether one of the person's workspaces can send mail.
    mail_available: bool,
}

#[derive(FromRow)]
struct AddressRow {
    address: String,
    verified_at: Option<DateTime<Utc>>,
    verification_sent_at: Option<DateTime<Utc>>,
}

async fn mail_available_for(pool: &PgPool, actor_id: Uuid) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM organization_memberships membership
            JOIN workspace_mail_settings mail
              ON mail.organization_id = membership.organization_id AND mail.enabled
            WHERE membership.actor_id = $1 AND membership.enabled
        )
        "#,
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await?)
}

async fn email_view(pool: &PgPool, principal: &AuthenticatedActor) -> Result<EmailView, AppError> {
    let row = sqlx::query_as::<_, AddressRow>(
        "SELECT address, verified_at, verification_sent_at FROM user_email_addresses WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .fetch_optional(pool)
    .await?;
    let mail_available = mail_available_for(pool, principal.actor_id).await?;
    Ok(match row {
        Some(row) => EmailView {
            address: Some(row.address),
            verified: row.verified_at.is_some(),
            confirmation_sent_at: row
                .verified_at
                .is_none()
                .then_some(row.verification_sent_at)
                .flatten(),
            mail_available,
        },
        None => EmailView {
            address: None,
            verified: false,
            confirmation_sent_at: None,
            mail_available,
        },
    })
}

async fn read_email_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<EmailView>, AppError> {
    Ok(Json(email_view(&state.pool, &principal).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetEmailRequest {
    address: String,
}

async fn queue_confirmation(
    connection: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<(), AppError> {
    jobs::enqueue(
        &mut *connection,
        NewJob::new(VERIFY_KIND)
            .payload(VerifyJob { user_id })
            .unique(format!("{VERIFY_KIND}:{user_id}:{}", Uuid::now_v7()))
            .max_attempts(4),
    )
    .await?;
    Ok(())
}

async fn set_email_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<SetEmailRequest>,
) -> Result<Json<EmailView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let address =
        mail::normalize_address(&request.address).ok_or(AppError::Validation("address"))?;
    if !mail_available_for(&state.pool, principal.actor_id).await? {
        return Err(AppError::Conflict("email", "mail isn't set up"));
    }
    let mut transaction = state.pool.begin().await?;
    let current = sqlx::query_as::<_, AddressRow>(
        "SELECT address, verified_at, verification_sent_at FROM user_email_addresses WHERE user_id = $1 FOR UPDATE",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if current
        .as_ref()
        .is_some_and(|row| row.address == address && row.verified_at.is_some())
    {
        transaction.commit().await?;
        return Ok(Json(email_view(&state.pool, &principal).await?));
    }
    if current
        .as_ref()
        .and_then(|row| row.verification_sent_at)
        .is_some_and(|at| Utc::now() - at < chrono::Duration::seconds(30))
    {
        return Err(AppError::rate_limited(30));
    }
    sqlx::query(
        r#"
        INSERT INTO user_email_addresses (user_id, address, verification_sent_at)
        VALUES ($1, $2, now())
        ON CONFLICT (user_id) DO UPDATE SET
            address = EXCLUDED.address, verified_at = NULL, verification_digest = NULL,
            verification_expires_at = NULL, verification_sent_at = now(), updated_at = now()
        "#,
    )
    .bind(principal.user_id)
    .bind(&address)
    .execute(&mut *transaction)
    .await?;
    end_live_resets(&mut transaction, principal.user_id).await?;
    queue_confirmation(&mut transaction, principal.user_id).await?;
    audit(
        &mut transaction,
        Some(&principal),
        principal.actor_id,
        principal.user_id,
        "account.email.change",
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(email_view(&state.pool, &principal).await?))
}

async fn remove_email_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("DELETE FROM user_email_addresses WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *transaction)
        .await?;
    end_live_resets(&mut transaction, principal.user_id).await?;
    audit(
        &mut transaction,
        Some(&principal),
        principal.actor_id,
        principal.user_id,
        "account.email.remove",
    )
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

async fn resend_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<EmailView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query_as::<_, AddressRow>(
        "SELECT address, verified_at, verification_sent_at FROM user_email_addresses WHERE user_id = $1 FOR UPDATE",
    )
    .bind(principal.user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("email"))?;
    if row.verified_at.is_some() {
        return Err(AppError::Conflict("email", "already confirmed"));
    }
    if row
        .verification_sent_at
        .is_some_and(|at| Utc::now() - at < chrono::Duration::seconds(30))
    {
        return Err(AppError::rate_limited(30));
    }
    sqlx::query("UPDATE user_email_addresses SET verification_sent_at = now() WHERE user_id = $1")
        .bind(principal.user_id)
        .execute(&mut *transaction)
        .await?;
    queue_confirmation(&mut transaction, principal.user_id).await?;
    transaction.commit().await?;
    Ok(Json(email_view(&state.pool, &principal).await?))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyJob {
    user_id: Uuid,
}

/// Make a fresh confirmation link (replacing any earlier one) and email it.
async fn send_confirmation(state: &AppState, user_id: Uuid) -> Result<(), JobError> {
    let row = sqlx::query_as::<_, (String, Uuid, String)>(
        r#"
        SELECT address.address, user_account.actor_id, user_account.display_name
        FROM user_email_addresses address
        JOIN users user_account ON user_account.id = address.user_id AND user_account.status = 'active'
        WHERE address.user_id = $1 AND address.verified_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((address, actor_id, display_name)) = row else {
        return Ok(());
    };
    let Some(transport) = mail::transport_for_actor(state, actor_id).await? else {
        return Ok(());
    };
    let link_token = token(VERIFY_PREFIX);
    let updated = sqlx::query(
        r#"
        UPDATE user_email_addresses
        SET verification_digest = $3,
            verification_expires_at = now() + make_interval(hours => $4)
        WHERE user_id = $1 AND address = $2 AND verified_at IS NULL
        "#,
    )
    .bind(user_id)
    .bind(&address)
    .bind(digest(&link_token).to_vec())
    .bind(VERIFY_HOURS)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(());
    }
    let origin = crate::events::public_origin(state);
    mail::send(
        state,
        &transport,
        &Message {
            to: address,
            subject: "Confirm your email address".to_owned(),
            text: format!(
                "Hi {display_name},\n\nOpen this link to confirm this is your email address for Agent Workspace at {origin}:\n\n{origin}/verify-email#token={link_token}\n\nThe link works for {VERIFY_HOURS} hours. Once confirmed, you can get notifications here and reset your password by email.\n\nIf you didn't ask for this, ignore this email; nothing changes.\n"
            ),
        },
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenRequest {
    token: String,
}

impl Drop for TokenRequest {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedResponse {
    address: String,
}

/// Opened from the confirmation email; the link is the proof, so no
/// session is needed (the person may open it on another device).
async fn verify_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<TokenRequest>,
) -> Result<Json<VerifiedResponse>, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    if !well_formed(&request.token, VERIFY_PREFIX) {
        return Err(AppError::NotFound("confirmation link"));
    }
    state.rate_limits.check_invitation(&request.token)?;
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        r#"
        SELECT address.user_id, user_account.actor_id, address.address
        FROM user_email_addresses address
        JOIN users user_account ON user_account.id = address.user_id AND user_account.status = 'active'
        WHERE address.verification_digest = $1 AND address.verified_at IS NULL
          AND address.verification_expires_at > now()
        FOR UPDATE OF address
        "#,
    )
    .bind(digest(&request.token).to_vec())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("confirmation link"))?;
    let (user_id, actor_id, address) = row;
    let taken = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM user_email_addresses
            WHERE lower(address) = lower($1) AND verified_at IS NOT NULL AND user_id <> $2
        )
        "#,
    )
    .bind(&address)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    if taken {
        return Err(AppError::Conflict(
            "email",
            "another account confirmed this address",
        ));
    }
    sqlx::query(
        r#"
        UPDATE user_email_addresses
        SET verified_at = now(), verification_digest = NULL, verification_expires_at = NULL,
            updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    audit(
        &mut transaction,
        None,
        actor_id,
        user_id,
        "account.email.confirm",
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(VerifiedResponse { address }))
}

// ---------------------------------------------------------------------------
// Password reset

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetAvailability {
    /// Whether this server can email a reset link. Without it, the
    /// server's operator resets passwords.
    available: bool,
}

async fn reset_available_http(
    State(state): State<AppState>,
) -> Result<Json<ResetAvailability>, AppError> {
    Ok(Json(ResetAvailability {
        available: mail::any_configured(&state.pool).await?,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetRequest {
    /// A login name or a confirmed email address.
    login: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetJob {
    login: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetAccepted {
    accepted: bool,
}

/// The lower-case login name or address, or None for anything that can't
/// be either.
fn normalize_reset_key(value: &str) -> Option<String> {
    let value = value.trim();
    if value.contains('@') {
        mail::normalize_address(value).map(|address| address.to_lowercase())
    } else {
        crate::auth::normalize_login_name(value).ok()
    }
}

async fn request_reset_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ResetRequest>,
) -> Result<(StatusCode, Json<ResetAccepted>), AppError> {
    require_allowed_origin(&state.config, &headers)?;
    // The same answer for every well-formed request; who exists is looked
    // up later, in the job.
    if let Some(login) = normalize_reset_key(&request.login) {
        state.rate_limits.check_password_reset(
            crate::rate_limit::client_address(&state.config, &headers),
            &login,
        )?;
        jobs::enqueue(
            &state.pool,
            NewJob::new(RESET_KIND)
                .payload(ResetJob { login })
                .max_attempts(4)
                .priority(5),
        )
        .await?;
    } else if request.login.trim().is_empty() {
        return Err(AppError::Validation("login"));
    }
    Ok((StatusCode::ACCEPTED, Json(ResetAccepted { accepted: true })))
}

#[derive(FromRow)]
struct ResetTarget {
    user_id: Uuid,
    actor_id: Uuid,
    login_name: String,
    display_name: String,
    authentication_epoch: i64,
    address: String,
}

/// Find the account, make a link, email it. Does nothing (and says
/// nothing) for an unknown account, one without a confirmed address or a
/// password, or one that asked too often.
async fn send_reset(state: &AppState, login: &str) -> Result<(), JobError> {
    let target = sqlx::query_as::<_, ResetTarget>(
        r#"
        SELECT user_account.id AS user_id, user_account.actor_id, user_account.login_name,
               user_account.display_name, user_account.authentication_epoch, address.address
        FROM users user_account
        JOIN actors actor ON actor.id = user_account.actor_id
         AND actor.status = 'active' AND actor.kind = 'human'
        JOIN user_password_credentials credential ON credential.user_id = user_account.id
        JOIN user_email_addresses address ON address.user_id = user_account.id
         AND address.verified_at IS NOT NULL
        WHERE user_account.status = 'active'
          AND (lower(user_account.login_name) = $1 OR lower(address.address) = $1)
        LIMIT 1
        "#,
    )
    .bind(login)
    .fetch_optional(&state.pool)
    .await?;
    let Some(target) = target else {
        return Ok(());
    };
    let Some(transport) = mail::transport_for_actor(state, target.actor_id).await? else {
        return Ok(());
    };
    let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("password-reset:{}", target.user_id))
        .execute(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    let (recent, last) = sqlx::query_as::<_, (i64, Option<DateTime<Utc>>)>(
        r#"
        SELECT count(*), max(created_at) FROM password_reset_tokens
        WHERE user_id = $1 AND created_at > now() - interval '1 hour'
        "#,
    )
    .bind(target.user_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    if recent >= RESETS_PER_HOUR
        || last.is_some_and(|at| Utc::now() - at < chrono::Duration::seconds(60))
    {
        return Ok(());
    }
    // The whole server's reset emails have a ceiling too; only real sends
    // spend it, so requests for made-up accounts can't hold anyone's back.
    if let Err(seconds) = state.rate_limits.check_reset_email() {
        return Err(JobError::retry(format!(
            "too many reset emails right now; trying again in {seconds} s"
        )));
    }
    // One live link at a time: the newest.
    end_live_resets(&mut transaction, target.user_id).await?;
    let link_token = token(RESET_PREFIX);
    let link_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO password_reset_tokens (id, user_id, token_digest, authentication_epoch, expires_at)
        VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
        "#,
    )
    .bind(link_id)
    .bind(target.user_id)
    .bind(digest(&link_token).to_vec())
    .bind(target.authentication_epoch)
    .bind(RESET_MINUTES)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    audit(
        &mut transaction,
        None,
        target.actor_id,
        target.user_id,
        "authentication.password_reset.request",
    )
    .await?;
    transaction.commit().await.map_err(AppError::from)?;
    let origin = crate::events::public_origin(state);
    let sent = mail::send(
        state,
        &transport,
        &Message {
            to: target.address,
            subject: "Reset your password".to_owned(),
            text: format!(
                "Hi {},\n\nSomeone (hopefully you) asked to reset the password for {} on Agent Workspace at {origin}.\n\nOpen this link to choose a new one:\n\n{origin}/reset-password#token={link_token}\n\nThe link works once, for {RESET_MINUTES} minutes. Resetting signs you out everywhere.\n\nIf you didn't ask, ignore this email; your password stays the same.\n",
                target.display_name, target.login_name
            ),
        },
    )
    .await;
    if let Err(error) = sent {
        // The link never reached anyone: forget it, so the retry isn't
        // throttled by it and mails a fresh one.
        sqlx::query("DELETE FROM password_reset_tokens WHERE id = $1")
            .bind(link_id)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
        return Err(error.into());
    }
    Ok(())
}

/// End every reset link `user_id` still has: a new link, a password change
/// or a new or removed address each make the old ones useless.
pub(crate) async fn end_live_resets(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE password_reset_tokens SET consumed_at = now() WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct LiveReset {
    id: Uuid,
    user_id: Uuid,
    actor_id: Uuid,
    login_name: String,
    failed_attempts: i32,
}

/// The live reset for `token`, locked. Expired, used, stale-epoch and
/// disabled-account links all look the same: not found.
async fn live_reset(
    connection: &mut sqlx::PgConnection,
    token: &str,
) -> Result<LiveReset, AppError> {
    if !well_formed(token, RESET_PREFIX) {
        return Err(AppError::NotFound("reset link"));
    }
    sqlx::query_as::<_, LiveReset>(
        r#"
        SELECT reset.id, reset.user_id, user_account.actor_id, user_account.login_name,
               reset.failed_attempts
        FROM password_reset_tokens reset
        JOIN users user_account ON user_account.id = reset.user_id
         AND user_account.status = 'active'
         AND user_account.authentication_epoch = reset.authentication_epoch
        JOIN actors actor ON actor.id = user_account.actor_id
         AND actor.status = 'active' AND actor.kind = 'human'
        WHERE reset.token_digest = $1 AND reset.consumed_at IS NULL
          AND reset.expires_at > now() AND reset.failed_attempts < $2
        FOR UPDATE OF reset
        "#,
    )
    .bind(digest(token).to_vec())
    .bind(RESET_ATTEMPTS)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("reset link"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetInspection {
    login_name: String,
    two_factor_required: bool,
}

/// What the reset page needs before asking for a new password.
async fn inspect_reset_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<TokenRequest>,
) -> Result<Json<ResetInspection>, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    state.rate_limits.check_invitation(&request.token)?;
    let mut transaction = state.pool.begin().await?;
    let reset = live_reset(&mut transaction, &request.token).await?;
    let two_factor_required =
        crate::account_security::two_factor_enabled(&mut transaction, reset.user_id).await?;
    transaction.commit().await?;
    Ok(Json(ResetInspection {
        login_name: reset.login_name,
        two_factor_required,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompleteRequest {
    token: String,
    new_password: String,
    /// The authenticator or recovery code, for an account with two-factor.
    code: Option<String>,
    /// Also disconnect the apps connected as this person (for when someone
    /// else may have got in). Off by default: reconnecting every app is a
    /// chore, and the reset already signs out every browser.
    #[serde(default)]
    disconnect_apps: bool,
}

impl Drop for CompleteRequest {
    fn drop(&mut self) {
        self.token.zeroize();
        self.new_password.zeroize();
        if let Some(code) = &mut self.code {
            code.zeroize();
        }
    }
}

async fn complete_reset_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<CompleteRequest>,
) -> Result<StatusCode, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    state.rate_limits.check_invitation(&request.token)?;
    if !(12..=1024).contains(&request.new_password.len()) {
        return Err(AppError::Validation("new password"));
    }
    // Check the link before spending a password hash on it.
    {
        let mut transaction = state.pool.begin().await?;
        live_reset(&mut transaction, &request.token).await?;
        transaction.rollback().await?;
    }
    let new_phc = {
        let _permit = state.rate_limits.try_acquire_password_kdf()?;
        password::hash_password(std::mem::take(&mut request.new_password)).await?
    };
    let mut transaction = state.pool.begin().await?;
    let reset = live_reset(&mut transaction, &request.token).await?;
    if crate::account_security::two_factor_enabled(&mut transaction, reset.user_id).await? {
        let code = request.code.as_deref().map(str::trim).unwrap_or_default();
        let factor = crate::account_security::load_factor(&mut transaction, reset.user_id)
            .await?
            .filter(|factor| factor.confirmed_at.is_some())
            .ok_or(AppError::Internal("two-factor factor"))?;
        // Six digits are an authenticator code; anything else may be a
        // recovery code.
        let totp = code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit());
        let accepted = if totp {
            crate::account_security::accept_totp(
                &mut transaction,
                &state,
                reset.user_id,
                &factor,
                code,
            )
            .await?
        } else {
            !code.is_empty()
                && crate::account_security::accept_recovery_code(
                    &mut transaction,
                    reset.user_id,
                    code,
                )
                .await?
        };
        if !accepted {
            sqlx::query(
                "UPDATE password_reset_tokens SET failed_attempts = failed_attempts + 1 WHERE id = $1",
            )
            .bind(reset.id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            let _ = reset.failed_attempts;
            return Err(AppError::InvalidCredentials);
        }
    }
    sqlx::query("UPDATE password_reset_tokens SET consumed_at = now() WHERE id = $1")
        .bind(reset.id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"
        UPDATE user_password_credentials
        SET password_phc = $2, version = version + 1, updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(reset.user_id)
    .bind(new_phc)
    .execute(&mut *transaction)
    .await?;
    // Sign out everywhere: a new epoch ends every session and every other
    // link or half-finished sign-in.
    sqlx::query(
        "UPDATE users SET authentication_epoch = authentication_epoch + 1, updated_at = now() WHERE id = $1",
    )
    .bind(reset.user_id)
    .execute(&mut *transaction)
    .await?;
    let revoked = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE sessions SET revoked_at = now(), updated_at = now()
        WHERE user_id = $1 AND revoked_at IS NULL
        RETURNING id
        "#,
    )
    .bind(reset.user_id)
    .fetch_all(&mut *transaction)
    .await?;
    sqlx::query("UPDATE login_challenges SET consumed_at = now() WHERE user_id = $1 AND consumed_at IS NULL")
        .bind(reset.user_id)
        .execute(&mut *transaction)
        .await?;
    if request.disconnect_apps {
        disconnect_apps(&mut transaction, reset.actor_id).await?;
    }
    audit(
        &mut transaction,
        None,
        reset.actor_id,
        reset.user_id,
        "authentication.password_reset.complete",
    )
    .await?;
    transaction.commit().await?;
    for session_id in revoked {
        state.realtime.revoke_session(session_id);
        crate::auth::workos::revoke_provider_session(&state, session_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Revokes every app connected as `actor_id`, with every token issued to
/// it, as if each had been disconnected in Settings › Connected apps.
async fn disconnect_apps(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
) -> Result<(), AppError> {
    let grants = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE mcp_oauth_grants SET revoked_at = now()
        WHERE actor_id = $1 AND revoked_at IS NULL
        RETURNING id
        "#,
    )
    .bind(actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    if grants.is_empty() {
        return Ok(());
    }
    for table in ["mcp_oauth_access_tokens", "mcp_oauth_refresh_tokens"] {
        sqlx::query(&format!(
            "UPDATE {table} SET revoked_at = COALESCE(revoked_at, now()) WHERE grant_id = ANY($1)"
        ))
        .bind(&grants)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, authentication_class, action, target_kind, target_id,
            outcome, channel, metadata, mcp_oauth_grant_id
        )
        SELECT gen_random_uuid(), $1, 'password_session', 'connected_app.revoked',
               'mcp_oauth_grant', grant_id, 'succeeded', 'browser', '{}'::jsonb, grant_id
        FROM unnest($2::uuid[]) AS grant_id
        "#,
    )
    .bind(actor_id)
    .bind(&grants)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn audit(
    connection: &mut sqlx::PgConnection,
    principal: Option<&AuthenticatedActor>,
    actor_id: Uuid,
    user_id: Uuid,
    action: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, session_id, authentication_class, action,
            target_kind, target_id, outcome, channel, metadata
        ) VALUES ($1, $2, $3, 'password_session', $4, 'user', $5, 'succeeded', 'browser', '{}'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(principal.map(|principal| principal.session_id))
    .bind(action)
    .bind(user_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_random_and_checked_for_shape() {
        let first = token(RESET_PREFIX);
        let second = token(RESET_PREFIX);
        assert_ne!(first, second);
        assert!(well_formed(&first, RESET_PREFIX));
        assert!(!well_formed(&first, VERIFY_PREFIX));
        assert!(first.len() >= RESET_PREFIX.len() + 43);
        assert!(!well_formed("aw_reset_abc def", RESET_PREFIX));
        assert!(!well_formed(
            &format!("{RESET_PREFIX}{}", "a".repeat(200)),
            RESET_PREFIX
        ));
        assert_ne!(digest(&first), digest(&second));
    }

    #[test]
    fn reset_keys_are_login_names_or_addresses() {
        assert_eq!(normalize_reset_key(" Mischa "), Some("mischa".into()));
        assert_eq!(
            normalize_reset_key("Me@Example.com"),
            Some("me@example.com".into())
        );
        assert_eq!(normalize_reset_key("not valid!"), None);
    }
}
