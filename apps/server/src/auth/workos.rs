//! WorkOS owns authentication; local actors and memberships remain authoritative.
//! Provider credentials never leave this module or enter persisted application data.
use super::{
    AuthenticatedActor, SessionIdentity, append_cookie, cookie_value,
    establish_session_in_transaction, hash_secret, load_authenticated_actor, normalize_login_name,
    random_token, require_allowed_origin, require_authenticated_write,
    respond_with_established_session,
};
use crate::{
    AppState,
    config::{Environment, WorkOsConfig},
    error::{ApiJson, ApiQuery, AppError},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/workos", get(status))
        .route("/api/v1/auth/workos/start", post(start))
        .route("/auth/workos/callback", get(callback))
        .route(
            "/api/v1/account/profile",
            get(profile).patch(update_profile),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkOsStatus {
    enabled: bool,
    signup_enabled: bool,
    linked: bool,
    email: Option<String>,
}
async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WorkOsStatus>, AppError> {
    let principal = load_authenticated_actor(&state, &headers).await?;
    let email = if let (Some(config), Some(principal)) = (&state.config.workos, principal) {
        sqlx::query_scalar::<_, String>(
            "SELECT email FROM workos_identities WHERE user_id = $1 AND client_id = $2",
        )
        .bind(principal.user_id)
        .bind(&config.client_id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };
    Ok(Json(WorkOsStatus {
        enabled: state.config.workos.is_some(),
        signup_enabled: state.config.workos.as_ref().is_some_and(|c| c.allow_signup),
        linked: email.is_some(),
        email,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    link: Option<bool>,
    username: Option<String>,
}
#[derive(Serialize)]
struct StartResponse {
    url: String,
}
fn config(state: &AppState) -> Result<&WorkOsConfig, AppError> {
    state
        .config
        .workos
        .as_ref()
        .ok_or(AppError::NotFound("WorkOS sign-in"))
}
fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}
fn browser_cookie(state: &AppState, value: &str, expire: bool) -> String {
    let secure = if state.config.environment == Environment::Development {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        cookie_name(state),
        if expire { 0 } else { 600 },
        secure
    )
}
fn cookie_name(state: &AppState) -> &'static str {
    if state.config.environment == Environment::Development {
        "aw_workos_login"
    } else {
        "__Host-aw_workos_login"
    }
}
fn verifier(config: &WorkOsConfig, state: &str) -> Result<String, AppError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(config.api_key.expose().as_bytes())
        .map_err(|_| AppError::Internal("PKCE key"))?;
    mac.update(b"agent-workspace/workos/pkce/v1:");
    mac.update(state.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}
async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<StartRequest>,
) -> Result<Response, AppError> {
    let config = config(&state)?;
    require_allowed_origin(&state.config, &headers)?;
    state.rate_limits.check_login("workos-start")?;
    let link_session_id = if request.link == Some(true) {
        let principal = load_authenticated_actor(&state, &headers)
            .await?
            .ok_or(AppError::AuthenticationRequired)?;
        require_authenticated_write(&state, &headers, &principal)?;
        Some(principal.session_id)
    } else {
        None
    };
    let username = request
        .username
        .as_deref()
        .map(normalize_login_name)
        .transpose()?;
    if username.is_some() && (!config.allow_signup || link_session_id.is_some()) {
        return Err(AppError::Forbidden);
    }
    let nonce = random_token();
    let browser = random_token();
    let pkce = Zeroizing::new(verifier(config, &nonce)?);
    let mut url = url::Url::parse("https://api.workos.com/user_management/authorize")
        .map_err(|_| AppError::Internal("WorkOS URL"))?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", config.client_id.as_str()),
        ("provider", "authkit"),
        ("response_type", "code"),
        (
            "redirect_uri",
            &format!("{}/auth/workos/callback", public_origin(&state)),
        ),
        ("state", &nonce),
        ("code_challenge_method", "S256"),
        (
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.as_bytes())),
        ),
        (
            "screen_hint",
            if username.is_some() {
                "sign-up"
            } else {
                "sign-in"
            },
        ),
    ]);
    if link_session_id.is_some() {
        url.query_pairs_mut().append_pair("prompt", "login");
    }
    sqlx::query("DELETE FROM workos_login_attempts WHERE expires_at < now()")
        .execute(&state.pool)
        .await?;
    sqlx::query("INSERT INTO workos_login_attempts (state_hash,browser_hash,link_session_id,signup_username,expires_at) VALUES ($1,$2,$3,$4,now()+interval '10 minutes')")
        .bind(hash_secret(&nonce).to_vec()).bind(hash_secret(&browser).to_vec()).bind(link_session_id).bind(username).execute(&state.pool).await?;
    let mut response_headers = HeaderMap::new();
    append_cookie(
        &mut response_headers,
        browser_cookie(&state, &browser, false),
    )?;
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((
        response_headers,
        Json(StartResponse {
            url: url.to_string(),
        }),
    )
        .into_response())
}
#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
}
#[derive(FromRow)]
struct LoginAttempt {
    link_session_id: Option<Uuid>,
    signup_username: Option<String>,
}
#[derive(Deserialize)]
struct ProviderUser {
    id: String,
    email: String,
    email_verified: bool,
    first_name: Option<String>,
    last_name: Option<String>,
}
#[derive(Deserialize)]
struct Authentication {
    user: ProviderUser,
    access_token: String,
}
#[derive(Deserialize)]
struct SessionClaims {
    sid: String,
}
fn client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::Internal("identity client"))
}
async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<Callback>,
) -> Response {
    match finish_callback(&state, &headers, query).await {
        Ok(response) => response,
        Err(_) => {
            // Never log provider errors, authorization codes, or response bodies.
            let mut response = Redirect::to("/sign-in?workosError=1").into_response();
            let _ = append_cookie(response.headers_mut(), browser_cookie(&state, "", true));
            response
        }
    }
}
async fn finish_callback(
    state: &AppState,
    headers: &HeaderMap,
    query: Callback,
) -> Result<Response, AppError> {
    let config = config(state)?;
    let nonce = Zeroizing::new(
        query
            .state
            .filter(|v| v.len() <= 256)
            .ok_or(AppError::InvalidCredentials)?,
    );
    let code = Zeroizing::new(
        query
            .code
            .filter(|v| v.len() <= 2048)
            .ok_or(AppError::InvalidCredentials)?,
    );
    let browser = cookie_value(headers, cookie_name(state))
        .filter(|v| v.len() <= 256)
        .ok_or(AppError::InvalidCredentials)?;
    let attempt=sqlx::query_as::<_,LoginAttempt>("DELETE FROM workos_login_attempts WHERE state_hash=$1 AND browser_hash=$2 AND expires_at>now() RETURNING link_session_id,signup_username")
        .bind(hash_secret(&nonce).to_vec()).bind(hash_secret(&browser).to_vec()).fetch_optional(&state.pool).await?.ok_or(AppError::InvalidCredentials)?;
    let pkce = Zeroizing::new(verifier(config, &nonce)?);
    let response=client()?.post("https://api.workos.com/user_management/authenticate")
        .json(&serde_json::json!({"client_id":config.client_id,"client_secret":config.api_key.expose(),"grant_type":"authorization_code","code":code.as_str(),"code_verifier":pkce.as_str()}))
        .send().await.map_err(|_|AppError::Internal("identity service unavailable"))?;
    if !response.status().is_success() {
        return Err(AppError::InvalidCredentials);
    }
    let raw = Zeroizing::new(
        response
            .text()
            .await
            .map_err(|_| AppError::InvalidCredentials)?,
    );
    let auth: Authentication =
        serde_json::from_str(&raw).map_err(|_| AppError::InvalidCredentials)?;
    if !auth.user.email_verified
        || !auth.user.id.starts_with("user_")
        || auth.user.id.len() > 100
        || auth.user.email.len() > 320
    {
        return Err(AppError::InvalidCredentials);
    }
    // This token came directly from the authenticated HTTPS code exchange. Only
    // extract its session reference; never accept caller-supplied JWT claims.
    let token = Zeroizing::new(auth.access_token);
    let claims: SessionClaims = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(
                token
                    .split('.')
                    .nth(1)
                    .ok_or(AppError::InvalidCredentials)?,
            )
            .map_err(|_| AppError::InvalidCredentials)?,
    )
    .map_err(|_| AppError::InvalidCredentials)?;
    if !claims.sid.starts_with("session_") || claims.sid.len() > 100 {
        return Err(AppError::InvalidCredentials);
    }
    let mut tx = state.pool.begin().await?;
    let user_id = resolve_identity(&mut tx, config, &auth.user, &attempt).await?;
    let row=sqlx::query_as::<_,(Uuid,String,String,i64)>("SELECT u.actor_id,u.login_name,u.display_name,u.authentication_epoch FROM users u JOIN actors a ON a.id=u.actor_id WHERE u.id=$1 AND u.status='active' AND a.status='active' AND a.kind='human' FOR SHARE OF u,a")
        .bind(user_id).fetch_optional(&mut *tx).await?.ok_or(AppError::Forbidden)?;
    let established = establish_session_in_transaction(
        &state.config,
        headers,
        &mut tx,
        SessionIdentity {
            user_id,
            actor_id: row.0,
            actor_kind: "human".into(),
            login_name: row.1,
            display_name: row.2,
            authentication_epoch: row.3,
        },
    )
    .await?;
    sqlx::query("UPDATE sessions SET workos_session_id=$2,workos_checked_at=now(),absolute_expires_at=now()+interval '24 hours',idle_expires_at=now()+interval '24 hours' WHERE id=$1")
        .bind(established.principal.session_id).bind(claims.sid).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_events (id,actor_id,session_id,authentication_class,action,target_kind,target_id,outcome) VALUES ($1,$2,$3,'workos_session','authentication.workos','user',$4,'succeeded')")
        .bind(Uuid::now_v7()).bind(row.0).bind(established.principal.session_id).bind(user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    let mut response = respond_with_established_session(state, established).await?;
    *response.status_mut() = StatusCode::SEE_OTHER;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_static("/settings/account"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(response.headers_mut(), browser_cookie(state, "", true))?;
    Ok(response)
}
async fn resolve_identity(
    tx: &mut Transaction<'_, Postgres>,
    config: &WorkOsConfig,
    user: &ProviderUser,
    attempt: &LoginAttempt,
) -> Result<Uuid, AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("workos:{}:{}", config.client_id, user.id))
        .execute(&mut **tx)
        .await?;
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM workos_identities WHERE client_id=$1 AND subject_id=$2",
    )
    .bind(&config.client_id)
    .bind(&user.id)
    .fetch_optional(&mut **tx)
    .await?;
    let linked = if let Some(session_id) = attempt.link_session_id {
        Some(sqlx::query_scalar::<_,Uuid>("SELECT s.user_id FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.revoked_at IS NULL AND s.absolute_expires_at>now() AND s.idle_expires_at>now() AND s.authentication_epoch=u.authentication_epoch AND u.status='active' FOR SHARE OF s,u")
            .bind(session_id).fetch_optional(&mut **tx).await?.ok_or(AppError::AuthenticationRequired)?)
    } else {
        None
    };
    if let Some(existing) = existing {
        if linked.is_some_and(|id| id != existing) {
            return Err(AppError::Forbidden);
        }
        return Ok(existing);
    }
    let user_id = if let Some(id) = linked {
        id
    } else {
        if !config.allow_signup {
            return Err(AppError::Forbidden);
        }
        let username = attempt
            .signup_username
            .as_deref()
            .ok_or(AppError::Validation("choose a username before signing up"))?;
        let id = Uuid::now_v7();
        let actor = Uuid::now_v7();
        let org = Uuid::now_v7();
        let name = format!(
            "{} {}",
            user.first_name.as_deref().unwrap_or(""),
            user.last_name.as_deref().unwrap_or("")
        );
        let name = name.trim();
        let name = if name.is_empty() { username } else { name };
        if name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err(AppError::Validation("display name"));
        }
        sqlx::query("INSERT INTO actors(id,kind) VALUES ($1,'human')")
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO users(id,actor_id,login_name,display_name) VALUES ($1,$2,$3,$4)")
            .bind(id)
            .bind(actor)
            .bind(username)
            .bind(name)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO organizations(id,name,slug) VALUES ($1,'My workspace',$2)")
            .bind(org)
            .bind(format!("workspace-{}", org.simple()))
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO organization_memberships(organization_id,actor_id,role) VALUES ($1,$2,'owner')").bind(org).bind(actor).execute(&mut **tx).await?;
        id
    };
    sqlx::query(
        "INSERT INTO workos_identities(client_id,subject_id,user_id,email) VALUES ($1,$2,$3,$4)",
    )
    .bind(&config.client_id)
    .bind(&user.id)
    .bind(user_id)
    .bind(user.email.trim().to_lowercase())
    .execute(&mut **tx)
    .await?;
    Ok(user_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    username: String,
    display_name: String,
}
async fn profile(principal: AuthenticatedActor) -> Json<Profile> {
    Json(Profile {
        username: principal.login_name,
        display_name: principal.display_name,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateProfile {
    display_name: String,
}
async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<UpdateProfile>,
) -> Result<Json<Profile>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = request.display_name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("display name"));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE users SET display_name=$2,updated_at=now() WHERE id=$1")
        .bind(principal.user_id)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE organization_actor_profiles SET display_name=$2 WHERE actor_id=$1")
        .bind(principal.actor_id)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE project_memberships SET display_name=$2,version=version+1,updated_at=now() WHERE actor_id=$1").bind(principal.actor_id).bind(name).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(Profile {
        username: principal.login_name,
        display_name: name.into(),
    }))
}

pub(super) async fn session_valid(state: &AppState, session_id: Uuid) -> Result<bool, AppError> {
    let remote = sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT s.workos_session_id,i.subject_id,(s.workos_checked_at IS NULL OR s.workos_checked_at<now()-interval '5 minutes') FROM sessions s LEFT JOIN workos_identities i ON i.user_id=s.user_id AND i.client_id=$2 WHERE s.id=$1 AND s.workos_session_id IS NOT NULL"
    ).bind(session_id).bind(state.config.workos.as_ref().map(|c| c.client_id.as_str()).unwrap_or("")).fetch_optional(&state.pool).await?;
    let Some((sid, subject, needs_check)) = remote else {
        return Ok(true);
    };
    let Some(subject) = subject else {
        return Ok(false);
    };
    let config = config(state)?;
    if !needs_check {
        return Ok(true);
    }
    let mut after = None;
    let mut valid = false;
    for _ in 0..10 {
        let mut request = client()?
            .get(format!(
                "https://api.workos.com/user_management/users/{subject}/sessions"
            ))
            .bearer_auth(config.api_key.expose())
            .query(&[("limit", "100")]);
        if let Some(ref after) = after {
            request = request.query(&[("after", after)]);
        }
        let response = request
            .send()
            .await
            .map_err(|_| AppError::Internal("identity service unavailable"))?;
        if !response.status().is_success() {
            return Err(AppError::Internal("identity service unavailable"));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|_| AppError::Internal("identity service unavailable"))?;
        if value["data"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|r| r["id"] == sid && r["status"] == "active")
        }) {
            valid = true;
            break;
        }
        after = value["list_metadata"]["after"].as_str().map(str::to_owned);
        if after.is_none() {
            break;
        }
    }
    if valid {
        sqlx::query("UPDATE sessions SET workos_checked_at=now() WHERE id=$1")
            .bind(session_id)
            .execute(&state.pool)
            .await?;
    } else {
        sqlx::query("UPDATE sessions SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1")
            .bind(session_id)
            .execute(&state.pool)
            .await?;
        state.realtime.revoke_session(session_id);
    }
    Ok(valid)
}
pub(super) async fn revoke_provider_session(state: &AppState, session_id: Uuid) {
    let Some(config) = &state.config.workos else {
        return;
    };
    let Ok(Some(Some(sid))) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT workos_session_id FROM sessions WHERE id=$1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await
    else {
        return;
    };
    if let Ok(client) = client() {
        let _ = client
            .post("https://api.workos.com/user_management/sessions/revoke")
            .bearer_auth(config.api_key.expose())
            .json(&serde_json::json!({"session_id":sid}))
            .send()
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretString;
    use sqlx::{Executor, postgres::PgPoolOptions};

    #[tokio::test]
    async fn identities_require_explicit_linking_and_signup_is_isolated() {
        let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
            return;
        };
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("WorkOS test operation");
        let schema = format!("workos_{}", Uuid::now_v7().simple());
        admin
            .execute(format!("CREATE SCHEMA {schema}").as_str())
            .await
            .expect("WorkOS test operation");
        let search_path = format!("SET search_path TO {schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    connection.execute(search_path.as_str()).await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("WorkOS test operation");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("WorkOS test operation");
        let config = WorkOsConfig {
            client_id: "client_test".into(),
            api_key: SecretString::new("sk_test"),
            allow_signup: true,
        };
        let provider = ProviderUser {
            id: "user_test_one".into(),
            email: "same@example.test".into(),
            email_verified: true,
            first_name: Some("First".into()),
            last_name: None,
        };
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        let first = resolve_identity(
            &mut tx,
            &config,
            &provider,
            &LoginAttempt {
                link_session_id: None,
                signup_username: Some("first_user".into()),
            },
        )
        .await
        .expect("WorkOS test operation");
        tx.commit().await.expect("WorkOS test operation");
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        let repeated = resolve_identity(
            &mut tx,
            &config,
            &provider,
            &LoginAttempt {
                link_session_id: None,
                signup_username: None,
            },
        )
        .await
        .expect("WorkOS test operation");
        assert_eq!(first, repeated, "provider subject preserves the local user");
        tx.commit().await.expect("WorkOS test operation");
        let second_provider = ProviderUser {
            id: "user_test_two".into(),
            ..provider
        };
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        assert!(
            resolve_identity(
                &mut tx,
                &config,
                &second_provider,
                &LoginAttempt {
                    link_session_id: None,
                    signup_username: None
                }
            )
            .await
            .is_err(),
            "same email must not auto-link an existing user"
        );
        tx.rollback().await.expect("WorkOS test operation");
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        assert!(
            resolve_identity(
                &mut tx,
                &config,
                &second_provider,
                &LoginAttempt {
                    link_session_id: None,
                    signup_username: Some("first_user".into())
                }
            )
            .await
            .is_err(),
            "usernames are unique"
        );
        tx.rollback().await.expect("WorkOS test operation");
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        let second = resolve_identity(
            &mut tx,
            &config,
            &second_provider,
            &LoginAttempt {
                link_session_id: None,
                signup_username: Some("second_user".into()),
            },
        )
        .await
        .expect("WorkOS test operation");
        assert_ne!(first, second);
        tx.commit().await.expect("WorkOS test operation");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(DISTINCT organization_id) FROM organization_memberships"
            )
            .fetch_one(&pool)
            .await
            .expect("WorkOS test operation"),
            2,
            "new accounts own separate organizations"
        );
        let sid = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,csrf_token_hash,authentication_epoch,idle_expires_at,absolute_expires_at) VALUES ($1,$2,$3,$4,1,now()+interval '1 hour',now()+interval '1 day')")
            .bind(sid).bind(second).bind(hash_secret("token").to_vec()).bind(hash_secret("csrf").to_vec()).execute(&pool).await.expect("WorkOS test operation");
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        let first_provider = ProviderUser {
            id: "user_test_one".into(),
            ..second_provider
        };
        assert!(
            resolve_identity(
                &mut tx,
                &config,
                &first_provider,
                &LoginAttempt {
                    link_session_id: Some(sid),
                    signup_username: None
                }
            )
            .await
            .is_err(),
            "cannot link someone else's provider identity"
        );
        tx.rollback().await.expect("WorkOS test operation");
        let unlinked_provider = ProviderUser {
            id: "user_test_three".into(),
            ..first_provider
        };
        sqlx::query("UPDATE sessions SET revoked_at=now() WHERE id=$1")
            .bind(sid)
            .execute(&pool)
            .await
            .expect("WorkOS test operation");
        let mut tx = pool.begin().await.expect("WorkOS test operation");
        assert!(
            resolve_identity(
                &mut tx,
                &config,
                &unlinked_provider,
                &LoginAttempt {
                    link_session_id: Some(sid),
                    signup_username: None
                }
            )
            .await
            .is_err(),
            "revoked sessions cannot finish a link"
        );
        tx.rollback().await.expect("WorkOS test operation");
        pool.close().await;
        admin
            .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
            .await
            .expect("WorkOS test operation");
    }
}
