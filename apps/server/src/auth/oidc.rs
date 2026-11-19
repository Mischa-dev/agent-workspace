//! Plain OpenID Connect sign-in (H1.18): one issuer the operator
//! configures (`AGENT_WORKSPACE_OIDC_*`), found by discovery, with PKCE and
//! a nonce, its ID token checked against the issuer's published keys.
//!
//! An identity is the issuer's `sub`, linked to one account here
//! (`oidc_identities`). The first time it signs in it links to the account
//! whose verified email matches, to the signed-in account when someone
//! links it from Account settings, or, where the operator allows it, to a
//! new account. An email the issuer hasn't verified never links and never
//! makes an account. A person whose verified email is in the configured
//! domain joins the configured workspace.
//!
//! A sign-in's state lives in the database (`oidc_login_attempts`), bound
//! to the browser by a cookie, and is taken once; the PKCE verifier and the
//! nonce are derived from it under the client secret, so neither is stored.
//! Nothing is cached in the process: every start and callback reads the
//! issuer's discovery document and keys again.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    SessionIdentity, append_cookie, cookie_value, establish_session_in_transaction, hash_secret,
    load_authenticated_actor, normalize_login_name, random_token, require_allowed_origin,
    require_authenticated_write, respond_with_established_session,
};
use crate::{
    AppState,
    config::{Environment, OidcConfig},
    error::{ApiQuery, AppError},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/oidc/start", get(start))
        .route("/api/v1/auth/oidc/link", post(link))
        .route("/auth/oidc/callback", get(callback))
}

fn config(state: &AppState) -> Result<&OidcConfig, AppError> {
    state
        .config
        .oidc
        .as_ref()
        .ok_or(AppError::NotFound("OpenID Connect sign-in"))
}

fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}/auth/oidc/callback", public_origin(state))
}

fn cookie_name(state: &AppState) -> &'static str {
    if state.config.environment == Environment::Development {
        "aw_oidc_login"
    } else {
        "__Host-aw_oidc_login"
    }
}

fn browser_cookie(state: &AppState, value: &str, expire: bool) -> String {
    let secure = if state.config.environment == Environment::Development {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
        cookie_name(state),
        if expire { 0 } else { 600 },
    )
}

/// A value derived from the sign-in's state under the client secret: the
/// PKCE verifier and the nonce, so neither needs storing.
fn derived(config: &OidcConfig, purpose: &str, login_state: &str) -> Result<String, AppError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(config.client_secret.expose().as_bytes())
        .map_err(|_| AppError::Internal("sign-in key"))?;
    mac.update(b"agent-workspace/oidc/");
    mac.update(purpose.as_bytes());
    mac.update(b"/v1:");
    mac.update(login_state.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// What the issuer publishes about itself.
#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    state: &AppState,
    address: &str,
    form: Option<&[(&str, &str)]>,
) -> Result<T, AppError> {
    let url = url::Url::parse(address).map_err(|_| AppError::Internal("issuer address"))?;
    let local = state.config.environment == Environment::Development
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if url.scheme() != "https" && !local {
        return Err(AppError::Internal("issuer address must be https"));
    }
    let client = crate::outbound::client_for(
        &url,
        state.config.allow_private_addresses || local,
        crate::outbound::Timeouts {
            connect: std::time::Duration::from_secs(5),
            total: Some(std::time::Duration::from_secs(10)),
        },
        None,
    )
    .await
    .map_err(|_| AppError::Internal("issuer unreachable"))?;
    let request = match form {
        Some(form) => client.post(url).form(form),
        None => client.get(url),
    };
    let response = request
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| AppError::Internal("issuer unreachable"))?;
    if !response.status().is_success() {
        return Err(AppError::InvalidCredentials);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| AppError::Internal("issuer unreachable"))?;
    if bytes.len() > 256 * 1024 {
        return Err(AppError::Internal("issuer answer too large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| AppError::InvalidCredentials)
}

async fn discover(state: &AppState, config: &OidcConfig) -> Result<Discovery, AppError> {
    let found: Discovery = fetch_json(
        state,
        &format!("{}/.well-known/openid-configuration", config.issuer),
        None,
    )
    .await?;
    if found.issuer.trim_end_matches('/') != config.issuer {
        return Err(AppError::Internal("issuer names another issuer"));
    }
    Ok(found)
}

#[derive(Deserialize)]
struct StartQuery {
    #[serde(rename = "returnTo")]
    return_to: Option<String>,
}

/// Where the web app goes after signing in: a path on this site only.
fn safe_return(value: Option<&str>) -> Option<String> {
    value
        .filter(|path| {
            path.len() <= 2048
                && path.starts_with('/')
                && !path.starts_with("//")
                && !path.starts_with("/\\")
                && path.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .map(str::to_owned)
}

/// `GET /auth/oidc/start`: off to the issuer to sign in.
async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<StartQuery>,
) -> Result<Response, AppError> {
    let address = crate::rate_limit::client_address(&state.config, &headers);
    state.rate_limits.check_login_sender(address).await?;
    let url = begin(&state, None, safe_return(query.return_to.as_deref())).await?;
    let (location, cookie) = url;
    let mut response = Redirect::to(&location).into_response();
    append_cookie(response.headers_mut(), cookie)?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Serialize)]
struct LinkResponse {
    url: String,
}

/// `POST /api/v1/auth/oidc/link`: link the issuer's identity to the
/// signed-in account; answers with where to send the browser.
async fn link(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    require_allowed_origin(&state.config, &headers)?;
    let principal = load_authenticated_actor(&state, &headers)
        .await?
        .ok_or(AppError::AuthenticationRequired)?;
    require_authenticated_write(&state, &headers, &principal).await?;
    let (url, cookie) = begin(&state, Some(principal.session_id), None).await?;
    let mut response = Json(LinkResponse { url }).into_response();
    append_cookie(response.headers_mut(), cookie)?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Stores a sign-in's state and returns the issuer's address for it and
/// the browser's cookie.
async fn begin(
    state: &AppState,
    link_session: Option<Uuid>,
    return_to: Option<String>,
) -> Result<(String, String), AppError> {
    let config = config(state)?;
    let discovery = discover(state, config).await?;
    let login_state = random_token();
    let browser = random_token();
    let verifier = Zeroizing::new(derived(config, "pkce", &login_state)?);
    let nonce = derived(config, "nonce", &login_state)?;
    let mut url = url::Url::parse(&discovery.authorization_endpoint)
        .map_err(|_| AppError::Internal("issuer authorization address"))?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", config.client_id.as_str()),
        ("response_type", "code"),
        ("scope", "openid email profile"),
        ("redirect_uri", &redirect_uri(state)),
        ("state", &login_state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        (
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        ),
    ]);
    if link_session.is_some() {
        url.query_pairs_mut().append_pair("prompt", "login");
    }
    crate::tenancy::system("auth.oidc", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        sqlx::query("DELETE FROM oidc_login_attempts WHERE expires_at < now()")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO oidc_login_attempts \
             (state_hash, browser_hash, link_session_id, return_to, expires_at) \
             VALUES ($1, $2, $3, $4, now() + interval '10 minutes')",
        )
        .bind(hash_secret(&login_state).to_vec())
        .bind(hash_secret(&browser).to_vec())
        .bind(link_session)
        .bind(return_to)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(())
    })
    .await?;
    Ok((url.to_string(), browser_cookie(state, &browser, false)))
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Attempt {
    link_session_id: Option<Uuid>,
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: String,
}

/// The ID token's claims this server reads.
#[derive(Deserialize)]
struct Claims {
    sub: String,
    nonce: Option<String>,
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    name: Option<String>,
    preferred_username: Option<String>,
}

impl Claims {
    /// The email, only when the issuer says it verified it.
    fn verified_email(&self) -> Option<String> {
        let verified = match &self.email_verified {
            Some(serde_json::Value::Bool(value)) => *value,
            // Some issuers send it as a string.
            Some(serde_json::Value::String(value)) => value == "true",
            _ => false,
        };
        self.email
            .as_deref()
            .map(|email| email.trim().to_lowercase())
            .filter(|email| verified && email.len() <= 320 && email.contains('@'))
    }
}

/// Why a sign-in didn't finish, as the sign-in page says it.
enum Failure {
    /// No account here for this identity, and none may be made.
    Unlinked,
    /// The identity belongs to another account.
    LinkedElsewhere,
    Other,
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<Callback>,
) -> Response {
    let mut linking = false;
    match finish(&state, &headers, query, &mut linking).await {
        Ok(response) => response,
        Err(failure) => {
            let reason = match failure {
                Failure::Unlinked => "unlinked",
                Failure::LinkedElsewhere => "linked_elsewhere",
                Failure::Other => "1",
            };
            let page = if linking {
                "/settings/account/profile"
            } else {
                "/sign-in"
            };
            let mut response = Redirect::to(&format!("{page}?oidcError={reason}")).into_response();
            let _ = append_cookie(response.headers_mut(), browser_cookie(&state, "", true));
            response
        }
    }
}

/// A sign-in that failed for a reason the person can't act on: logged for
/// the operator (the error names what failed, never a token), shown as a
/// plain failure.
fn other(error: impl std::fmt::Display) -> Failure {
    tracing::warn!(%error, "single sign-on didn't finish");
    Failure::Other
}

async fn finish(
    state: &AppState,
    headers: &HeaderMap,
    query: Callback,
    linking: &mut bool,
) -> Result<Response, Failure> {
    let config = config(state).map_err(other)?;
    let login_state = Zeroizing::new(
        query
            .state
            .filter(|value| value.len() <= 256)
            .ok_or(Failure::Other)?,
    );
    let browser = cookie_value(headers, cookie_name(state))
        .filter(|value| value.len() <= 256)
        .ok_or(Failure::Other)?;
    // Taken once: a second callback with the same state finds nothing.
    let attempt = crate::tenancy::system("auth.oidc", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let attempt = sqlx::query_as::<_, Attempt>(
            "DELETE FROM oidc_login_attempts \
             WHERE state_hash = $1 AND browser_hash = $2 AND expires_at > now() \
             RETURNING link_session_id, return_to",
        )
        .bind(hash_secret(&login_state).to_vec())
        .bind(hash_secret(&browser).to_vec())
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(attempt)
    })
    .await
    .map_err(other)?
    .ok_or(Failure::Other)?;
    *linking = attempt.link_session_id.is_some();
    let code = Zeroizing::new(
        query
            .code
            .filter(|value| value.len() <= 4096)
            .ok_or(Failure::Other)?,
    );
    let discovery = discover(state, config).await.map_err(other)?;
    let verifier = Zeroizing::new(derived(config, "pkce", &login_state).map_err(other)?);
    let redirect = redirect_uri(state);
    let tokens: Tokens = fetch_json(
        state,
        &discovery.token_endpoint,
        Some(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.expose()),
            ("code_verifier", verifier.as_str()),
        ]),
    )
    .await
    .map_err(other)?;
    let claims = verify_id_token(state, config, &discovery, &tokens.id_token)
        .await
        .map_err(other)?;
    let expected_nonce = derived(config, "nonce", &login_state).map_err(other)?;
    if claims.nonce.as_deref() != Some(expected_nonce.as_str()) {
        return Err(Failure::Other);
    }
    if claims.sub.is_empty() || claims.sub.len() > 255 {
        return Err(Failure::Other);
    }
    let established = crate::tenancy::system("auth.oidc", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let user_id = match resolve(&mut transaction, config, &claims, &attempt).await? {
            Ok(user_id) => user_id,
            Err(failure) => return Ok(Err(failure)),
        };
        let row = sqlx::query_as::<_, (Uuid, String, String, i64)>(
            "SELECT person.actor_id, person.login_name, person.display_name, \
                 person.authentication_epoch \
             FROM users person JOIN actors account ON account.id = person.actor_id \
             WHERE person.id = $1 AND person.status = 'active' AND account.status = 'active' \
             FOR SHARE OF person, account",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((actor_id, login_name, display_name, authentication_epoch)) = row else {
            return Ok(Err(Failure::Other));
        };
        let established = establish_session_in_transaction(
            &state.config,
            headers,
            &mut transaction,
            SessionIdentity {
                user_id,
                actor_id,
                actor_kind: "human".into(), // kind-check: mechanical (a person signing in with a browser)
                login_name,
                display_name,
                authentication_epoch,
            },
        )
        .await?;
        crate::account_security::record_sign_in_method(
            &mut transaction,
            established.principal.session_id,
            "oidc",
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(Ok(established))
    })
    .await
    .map_err(other)??;
    let location = if attempt.link_session_id.is_some() {
        "/settings/account/profile".to_owned()
    } else {
        // Through the sign-in page: the session cookie is SameSite=Strict, so
        // it isn't sent on a redirect chain that began at the issuer.
        let back = attempt.return_to.unwrap_or_else(|| "/home".to_owned());
        format!(
            "/sign-in?returnTo={}",
            url::form_urlencoded::byte_serialize(back.as_bytes()).collect::<String>()
        )
    };
    let mut response = respond_with_established_session(state, established)
        .await
        .map_err(other)?;
    *response.status_mut() = StatusCode::SEE_OTHER;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).map_err(|_| Failure::Other)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(response.headers_mut(), browser_cookie(state, "", true)).map_err(other)?;
    Ok(response)
}

/// Checks the ID token's signature against the issuer's published keys,
/// its issuer, audience and expiry.
async fn verify_id_token(
    state: &AppState,
    config: &OidcConfig,
    discovery: &Discovery,
    token: &str,
) -> Result<Claims, AppError> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| AppError::InvalidCredentials)?;
    let algorithm = header.alg;
    if !matches!(
        algorithm,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
    ) {
        return Err(AppError::InvalidCredentials);
    }
    let keys: JwkSet = fetch_json(state, &discovery.jwks_uri, None).await?;
    let key = match header.kid.as_deref() {
        Some(kid) => keys.find(kid),
        None if keys.keys.len() == 1 => keys.keys.first(),
        None => None,
    }
    .ok_or(AppError::InvalidCredentials)?;
    let key = DecodingKey::from_jwk(key).map_err(|_| AppError::InvalidCredentials)?;
    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[config.issuer.as_str(), discovery.issuer.as_str()]);
    validation.set_audience(&[config.client_id.as_str()]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = 60;
    jsonwebtoken::decode::<Claims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|_| AppError::InvalidCredentials)
}

/// The account this identity signs in to, linking or making one as the
/// rules say.
async fn resolve(
    transaction: &mut Transaction<'_, Postgres>,
    config: &OidcConfig,
    claims: &Claims,
    attempt: &Attempt,
) -> Result<Result<Uuid, Failure>, AppError> {
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM oidc_identities WHERE issuer = $1 AND subject = $2 FOR UPDATE",
    )
    .bind(&config.issuer)
    .bind(&claims.sub)
    .fetch_optional(&mut **transaction)
    .await?;
    let linking = match attempt.link_session_id {
        Some(session) => Some(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT session.user_id FROM sessions session \
                 JOIN users person ON person.id = session.user_id \
                 WHERE session.id = $1 AND session.revoked_at IS NULL \
                   AND session.absolute_expires_at > now() AND session.idle_expires_at > now() \
                   AND session.authentication_epoch = person.authentication_epoch \
                   AND person.status = 'active'",
            )
            .bind(session)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(AppError::AuthenticationRequired)?,
        ),
        None => None,
    };
    if let Some(existing) = existing {
        if linking.is_some_and(|user| user != existing) {
            return Ok(Err(Failure::LinkedElsewhere));
        }
        return Ok(Ok(existing));
    }
    let email = claims.verified_email();
    let user_id = if let Some(user) = linking {
        user
    } else if let Some(user) = match &email {
        Some(email) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT address.user_id FROM user_email_addresses address \
                 JOIN users person ON person.id = address.user_id \
                 WHERE lower(address.address) = $1 AND address.verified_at IS NOT NULL \
                   AND person.status = 'active'",
            )
            .bind(email)
            .fetch_optional(&mut **transaction)
            .await?
        }
        None => None,
    } {
        user
    } else if config.allow_signup
        && let Some(email) = &email
    {
        new_person(transaction, claims, email).await?
    } else {
        return Ok(Err(Failure::Unlinked));
    };
    sqlx::query(
        "INSERT INTO oidc_identities (issuer, subject, user_id, email) VALUES ($1, $2, $3, $4)",
    )
    .bind(&config.issuer)
    .bind(&claims.sub)
    .bind(user_id)
    .bind(email.as_deref())
    .execute(&mut **transaction)
    .await?;
    if let (Some(email), Some((domain, workspace))) = (&email, &config.join) {
        let in_domain = email
            .rsplit_once('@')
            .is_some_and(|(_, at)| at.eq_ignore_ascii_case(domain));
        if in_domain {
            join(transaction, user_id, workspace).await?;
        }
    }
    Ok(Ok(user_id))
}

/// A new person for a verified email the issuer vouched for, with a login
/// name from the issuer's preferred name or the email, made unique.
async fn new_person(
    transaction: &mut Transaction<'_, Postgres>,
    claims: &Claims,
    email: &str,
) -> Result<Uuid, AppError> {
    let wanted = claims
        .preferred_username
        .as_deref()
        .and_then(|name| normalize_login_name(name).ok())
        .or_else(|| {
            let local = email.split('@').next().unwrap_or_default();
            let cleaned = local
                .chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
                })
                .collect::<String>();
            normalize_login_name(cleaned.trim_matches(|c: char| !c.is_ascii_alphanumeric())).ok()
        })
        .unwrap_or_else(|| "person".to_owned());
    let mut login = wanted.clone();
    for suffix in 2..1000 {
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE login_name = $1)",
        )
        .bind(&login)
        .fetch_one(&mut **transaction)
        .await?;
        if !taken {
            break;
        }
        login = format!("{}{suffix}", wanted.chars().take(58).collect::<String>());
    }
    let name = claims
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| {
            !name.is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control)
        })
        .unwrap_or(login.as_str())
        .to_owned();
    // One way to create a person (K1.29).
    let person = crate::accounts::create_person(
        transaction,
        crate::accounts::NewPerson {
            login_name: &login,
            display_name: &name,
            default_workspace: None,
            created_by: None,
            claimed: true,
        },
    )
    .await?;
    // The issuer verified the address; it is theirs here too.
    sqlx::query(
        "INSERT INTO user_email_addresses (user_id, address, verified_at) VALUES ($1, $2, now()) \
         ON CONFLICT DO NOTHING",
    )
    .bind(person.user_id)
    .bind(email)
    .execute(&mut **transaction)
    .await?;
    Ok(person.user_id)
}

/// Joins the configured workspace, by its address, as a member.
async fn join(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    workspace_slug: &str,
) -> Result<(), AppError> {
    let Some(workspace) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM organizations WHERE slug = $1 AND lifecycle = 'active'",
    )
    .bind(workspace_slug)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(());
    };
    let (account, name, login) = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT actor_id, display_name, login_name FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut **transaction)
    .await?;
    crate::invitations::engine::admit(
        transaction,
        crate::invitations::engine::Admission {
            workspace_id: workspace,
            account_id: account,
            role: "member",
            role_id: None,
            display_name: &name,
            handle: &login,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE users SET default_organization_id = COALESCE(default_organization_id, $2) \
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(workspace)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
