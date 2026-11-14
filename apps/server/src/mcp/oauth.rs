use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use subtle::ConstantTimeEq;
use url::Url;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{
        AuthenticatedActor, cookie_value, csrf_cookie_name, require_csrf, sign_in_return,
        sign_in_return_cookie,
    },
    error::{ApiJson, AppError},
};

use super::{MCP_PATH, McpPrincipal};

const TOKEN_BYTES: usize = 32;
const MAX_CLIENT_NAME: usize = 80;
const MAX_REDIRECT_URI: usize = 512;
const MAX_STATE: usize = 512;
const MAX_TOKEN: usize = 128;
const MAX_OAUTH_BODY_BYTES: usize = 64 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource/api/v1/mcp",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route("/oauth/register", post(register_client))
        .route("/oauth/authorize", get(authorize).post(consent))
        .route("/oauth/consent.css", get(consent_stylesheet))
        .route("/connect/confirm", post(consent))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(MAX_OAUTH_BODY_BYTES))
}

pub(super) fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}

#[derive(Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    scopes_supported: Vec<&'static str>,
    bearer_methods_supported: Vec<&'static str>,
}

async fn protected_resource_metadata(
    State(state): State<AppState>,
) -> Json<ProtectedResourceMetadata> {
    let origin = public_origin(&state);
    let mut authorization_servers = vec![origin.clone()];
    let mut scopes_supported = vec!["read", "write"];
    if let Some(config) = &state.config.workos
        && config.agent_registration_enabled
        && let Some(domain) = &config.authkit_domain
    {
        authorization_servers.push(domain.clone());
        // AuthKit grants are honored only under their `workspace:` names; the
        // local authorization server accepts either spelling.
        scopes_supported = vec!["workspace:read", "workspace:write"];
    }
    Json(ProtectedResourceMetadata {
        resource: format!("{origin}{MCP_PATH}"),
        authorization_servers,
        scopes_supported,
        bearer_methods_supported: vec!["header"],
    })
}

#[derive(Serialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    revocation_endpoint: String,
    response_types_supported: Vec<&'static str>,
    grant_types_supported: Vec<&'static str>,
    code_challenge_methods_supported: Vec<&'static str>,
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
}

async fn authorization_server_metadata(
    State(state): State<AppState>,
) -> Json<AuthorizationServerMetadata> {
    let origin = public_origin(&state);
    Json(AuthorizationServerMetadata {
        issuer: origin.clone(),
        authorization_endpoint: format!("{origin}/oauth/authorize"),
        token_endpoint: format!("{origin}/oauth/token"),
        registration_endpoint: format!("{origin}/oauth/register"),
        revocation_endpoint: format!("{origin}/oauth/revoke"),
        response_types_supported: vec!["code"],
        grant_types_supported: vec!["authorization_code", "refresh_token"],
        code_challenge_methods_supported: vec!["S256"],
        token_endpoint_auth_methods_supported: vec!["none"],
        scopes_supported: vec!["read", "write"],
    })
}

#[derive(Deserialize)]
struct RegistrationRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}

#[derive(Serialize)]
struct RegistrationResponse {
    client_id: String,
    client_id_issued_at: i64,
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: &'static str,
    grant_types: Vec<&'static str>,
    response_types: Vec<&'static str>,
}

/// Registrations that no one has authorized yet, across the server.
const MAX_UNCLAIMED_CLIENTS: i64 = 1_024;

async fn register_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<RegistrationRequest>,
) -> Result<(StatusCode, Json<RegistrationResponse>), OAuthError> {
    state
        .rate_limits
        .check_oauth_registration_sender(crate::rate_limit::client_address(&state.config, &headers))
        .map_err(OAuthError::from)?;
    state
        .rate_limits
        .check_oauth_ceremony(&request.client_name)
        .map_err(OAuthError::from)?;
    request.client_name = clean_client_name(&request.client_name);
    validate_client_name(&request.client_name)?;
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > 8 {
        return Err(OAuthError::invalid_client_metadata(
            "one to eight redirect URIs are required",
        ));
    }
    for redirect_uri in &request.redirect_uris {
        validate_redirect(redirect_uri)?;
    }
    if request
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
        || request.grant_types.as_ref().is_some_and(|values| {
            values
                .iter()
                .any(|value| value != "authorization_code" && value != "refresh_token")
        })
        || request
            .response_types
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| value != "code"))
    {
        return Err(OAuthError::invalid_client_metadata(
            "only a public authorization-code client with refresh tokens is supported",
        ));
    }

    let client_id = Uuid::now_v7();
    let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT pg_advisory_xact_lock(68458762026001)")
        .execute(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    sqlx::query(
        r#"DELETE FROM mcp_oauth_clients client
           WHERE client.created_at < now() - interval '1 day'
             AND NOT EXISTS (SELECT 1 FROM mcp_oauth_authorization_codes code WHERE code.client_id = client.id)
             AND NOT EXISTS (SELECT 1 FROM mcp_oauth_grants grant_row WHERE grant_row.client_id = client.id)"#,
    )
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    // Only registrations nobody has used yet count: they're all an
    // anonymous caller can grow. Clients someone authorized are bounded by
    // the people who authorized them, and their (even revoked) grants stay
    // for history.
    let unclaimed = sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*) FROM mcp_oauth_clients client
           WHERE NOT EXISTS (SELECT 1 FROM mcp_oauth_authorization_codes code WHERE code.client_id = client.id)
             AND NOT EXISTS (SELECT 1 FROM mcp_oauth_grants grant_row WHERE grant_row.client_id = client.id)"#,
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    if unclaimed >= MAX_UNCLAIMED_CLIENTS {
        return Err(OAuthError::invalid_client_metadata(
            "the local client registration limit has been reached",
        ));
    }
    sqlx::query(
        "INSERT INTO mcp_oauth_clients (id, client_name, redirect_uris) VALUES ($1, $2, $3)",
    )
    .bind(client_id)
    .bind(&request.client_name)
    .bind(&request.redirect_uris)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(RegistrationResponse {
            client_id: client_id.to_string(),
            client_id_issued_at: chrono::Utc::now().timestamp(),
            client_name: request.client_name,
            redirect_uris: request.redirect_uris,
            token_endpoint_auth_method: "none",
            grant_types: vec!["authorization_code", "refresh_token"],
            response_types: vec!["code"],
        }),
    ))
}

#[derive(Clone, Deserialize)]
struct AuthorizeRequestFields {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
}

#[derive(Clone, Debug)]
struct AuthorizeRequest {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: Option<String>,
}

fn parse_authorize_query(raw_query: Option<String>) -> Result<AuthorizeRequest, OAuthError> {
    let raw_query = raw_query.ok_or_else(|| OAuthError::invalid_request("missing query"))?;
    let mut fields = Vec::new();
    let mut resource = None;

    for (name, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        if name == "resource" {
            match resource.as_deref() {
                Some(existing) if existing != value => {
                    return Err(OAuthError::invalid_request(
                        "conflicting resource parameters are not allowed",
                    ));
                }
                Some(_) => {}
                None => resource = Some(value.into_owned()),
            }
        } else {
            fields.push((name.into_owned(), value.into_owned()));
        }
    }

    let encoded = serde_urlencoded::to_string(fields)
        .map_err(|_| OAuthError::invalid_request("invalid authorization query"))?;
    let fields: AuthorizeRequestFields = serde_urlencoded::from_str(&encoded)
        .map_err(|_| OAuthError::invalid_request("invalid authorization query"))?;
    Ok(AuthorizeRequest {
        response_type: fields.response_type,
        client_id: fields.client_id,
        redirect_uri: fields.redirect_uri,
        scope: fields.scope,
        state: fields.state,
        code_challenge: fields.code_challenge,
        code_challenge_method: fields.code_challenge_method,
        resource,
    })
}

#[derive(FromRow)]
struct ClientRow {
    id: Uuid,
    client_name: String,
    redirect_uris: Vec<String>,
}

async fn authorize(
    State(state): State<AppState>,
    principal: Result<AuthenticatedActor, AppError>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, OAuthError> {
    let back = format!(
        "/oauth/authorize?{}",
        raw_query.as_deref().unwrap_or_default()
    );
    let request = parse_authorize_query(raw_query)?;
    state
        .rate_limits
        .check_oauth_ceremony(&request.client_id)
        .map_err(OAuthError::from)?;
    let (client, scopes) = validate_authorize_request(&state, &request).await?;
    let principal = match principal {
        Ok(principal) => principal,
        Err(AppError::AuthenticationRequired) => return sign_in_first(&state, &back),
        Err(error) => return Err(error.into()),
    };
    let csrf = csrf_cookie(&state.config, &headers)
        .ok_or_else(|| OAuthError::access_denied("the browser session is missing CSRF state"))?;
    let scope = scopes.join(" ");
    let agents = agent_choices(&state, principal.actor_id).await?;
    let client_name = escape_html(&client.client_name);
    let person = escape_html(&principal.display_name);
    let wants_write = scopes.iter().any(|scope| scope == "write");
    let agent_section = if agents.is_empty() {
        "<input type=hidden name=connect_as value=self>".to_owned()
    } else {
        let options = agents
            .iter()
            .map(|agent| {
                format!(
                    "<label class=choice><input type=radio name=connect_as value=\"agent:{}\"><span><strong>{}</strong><small>The app signs in as this agent and uses the agent's own permissions.</small></span></label>",
                    agent.connection_id,
                    escape_html(&agent.display_name),
                )
            })
            .collect::<String>();
        format!(
            "<details><summary>Connect as one of your agents instead</summary><div class=choices><label class=choice><input type=radio name=connect_as value=self checked><span><strong>You ({person})</strong><small>The usual choice for Claude Code, Codex and ChatGPT.</small></span></label>{options}</div></details>"
        )
    };
    // The site-wide `no-referrer` policy makes browsers send `Origin: null` on
    // form posts, which the consent endpoint's origin check rejects. The
    // consent page alone uses `same-origin`, so the post carries its origin.
    let html = format!(
        r#"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width, initial-scale=1"><meta name=referrer content=same-origin><title>Connect {client_name}</title><link rel=stylesheet href=/oauth/consent.css></head><body><main class=card><p class=brand>Agent Workspace</p><h1>Connect {client_name}</h1><p class=lede>{client_name} will act as you, {person}, everywhere your account can reach: your messages, projects, tasks and pages. It sees exactly what you see, nothing more.</p><form method=post action=/connect/confirm><fieldset><legend>Access</legend><div class=choices><label class=choice><input type=radio name=access value=full{full_checked}><span><strong>Full access</strong><small>Read, and make changes as you: send messages, update tasks, edit pages.</small></span></label><label class=choice><input type=radio name=access value=read{read_checked}><span><strong>Read only</strong><small>Look at everything you can see, but change nothing.</small></span></label></div></fieldset><label class=check><input type=checkbox name=show_via value=on checked><span>Label what it writes “via {client_name}”</span></label><input type=hidden name=via_choice value=1>{agent_section}{hidden}<div class=actions><button class=primary name=decision value=approve type=submit>Connect</button><button name=decision value=deny type=submit>Cancel</button></div><p class=note>Requested: {scope}. You can change the label or revoke access any time in Account settings, under Connected apps.</p></form></main></body></html>"#,
        full_checked = if wants_write { " checked" } else { "" },
        read_checked = if wants_write { "" } else { " checked" },
        scope = escape_html(&scope),
        hidden = hidden_authorize_fields(&request, &scope, &csrf),
    );
    let mut response = Html(html).into_response();
    if sign_in_return(&state.config, &headers).is_some() {
        let forget = sign_in_return_cookie(&state.config, None);
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&forget).map_err(|_| AppError::Internal("sign-in return"))?,
        );
    }
    Ok(response)
}

/// Someone signed out opened the connect screen: send them to sign in, and
/// remember the screen so every kind of sign-in brings them back to it.
fn sign_in_first(state: &AppState, back: &str) -> Result<Response, OAuthError> {
    let location = format!(
        "/sign-in?returnTo={}",
        url::form_urlencoded::byte_serialize(back.as_bytes()).collect::<String>()
    );
    let remember = sign_in_return_cookie(&state.config, Some(back));
    let header_value = |value: &str| {
        HeaderValue::from_str(value).map_err(|_| AppError::Internal("sign-in return"))
    };
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, header_value(&location)?),
            (header::SET_COOKIE, header_value(&remember)?),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response())
}

/// Styles for the consent page. The site's content security policy allows
/// only same-origin stylesheets, so they're served here rather than inline.
async fn consent_stylesheet() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        CONSENT_CSS,
    )
        .into_response()
}

const CONSENT_CSS: &str = r#":root{color-scheme:light dark;--bg:#f6f5f2;--card:#fff;--text:#1d1c1a;--muted:#6b6862;--line:#e3e0d9;--accent:#3b5bdb;--accent-text:#fff;--choice:#faf9f7}
@media (prefers-color-scheme:dark){:root{--bg:#141413;--card:#1f1e1c;--text:#ecebe7;--muted:#a19d95;--line:#34322e;--accent:#7c95f5;--accent-text:#10131f;--choice:#262522}}
*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px 16px;background:var(--bg);color:var(--text);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
.card{width:100%;max-width:520px;background:var(--card);border:1px solid var(--line);border-radius:14px;padding:28px}
.brand{margin:0 0 8px;color:var(--muted);font-size:13px;font-weight:600;letter-spacing:.02em}
h1{margin:0 0 8px;font-size:22px;line-height:1.25}.lede{margin:0 0 20px;color:var(--muted)}
fieldset{border:0;margin:0 0 16px;padding:0}legend{font-weight:600;margin-bottom:8px}
.choices{display:grid;gap:8px}.choice{display:flex;gap:10px;align-items:flex-start;padding:12px;border:1px solid var(--line);border-radius:10px;background:var(--choice);cursor:pointer}
.choice:has(input:checked){border-color:var(--accent)}.choice input{margin-top:4px}.choice span{display:grid;gap:2px}.choice small{color:var(--muted);font-size:13px}
.check{display:flex;gap:10px;align-items:center;margin:4px 0 16px}
details{margin:0 0 16px}summary{cursor:pointer;color:var(--accent);margin-bottom:8px}
.actions{display:flex;gap:8px;margin-top:8px}button{font:inherit;padding:9px 16px;border-radius:9px;border:1px solid var(--line);background:transparent;color:var(--text);cursor:pointer}
button.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-text);font-weight:600}
.note{margin:16px 0 0;color:var(--muted);font-size:13px}
"#;

#[derive(Deserialize)]
struct ConsentRequest {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: Option<String>,
    /// Older forms named one Project; it now connects the whole account.
    project_id: Option<String>,
    /// "self" or "agent:<connection id>". Older forms sent "self:<project>".
    connect_as: Option<String>,
    /// "full" or "read". Older forms used the requested scope instead.
    access: Option<String>,
    show_via: Option<String>,
    via_choice: Option<String>,
    csrf_token: String,
    decision: String,
}

async fn consent(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    mut headers: HeaderMap,
    Form(form): Form<ConsentRequest>,
) -> Result<Response, OAuthError> {
    state
        .rate_limits
        .check_oauth_ceremony(&form.client_id)
        .map_err(OAuthError::from)?;
    let header = HeaderValue::from_str(&form.csrf_token)
        .map_err(|_| OAuthError::access_denied("invalid CSRF state"))?;
    headers.insert("x-csrf-token", header);
    require_csrf(&state.config, &headers, &principal)
        .map_err(|_| OAuthError::access_denied("invalid CSRF state"))?;
    let request = AuthorizeRequest {
        response_type: form.response_type,
        client_id: form.client_id,
        redirect_uri: form.redirect_uri,
        scope: Some(form.scope),
        state: form.state,
        code_challenge: form.code_challenge,
        code_challenge_method: form.code_challenge_method,
        resource: form.resource,
    };
    let (client, requested_scopes) = validate_authorize_request(&state, &request).await?;
    if form.decision != "approve" {
        return redirect_authorization_error(
            &request.redirect_uri,
            &request.state,
            "access_denied",
        );
    }
    let scopes = match form.access.as_deref() {
        Some("full") => vec!["read".to_owned(), "write".to_owned()],
        Some("read") => vec!["read".to_owned()],
        Some(_) => return Err(OAuthError::access_denied("choose full or read-only access")),
        None => requested_scopes,
    };
    let show_via = form.via_choice.is_none() || form.show_via.as_deref() == Some("on");
    let selection = form
        .connect_as
        .as_deref()
        .map(str::to_owned)
        .or_else(|| form.project_id.as_ref().map(|_| "self".to_owned()))
        .unwrap_or_else(|| "self".to_owned());
    let (project_id, actor_id, connection_id) = if let Some(connection) =
        selection.strip_prefix("agent:")
    {
        let connection_id = connection
            .parse::<Uuid>()
            .map_err(|_| OAuthError::access_denied("invalid agent selection"))?;
        let (project_id, agent_actor_id) = managed_agent(&state, principal.actor_id, connection_id)
            .await?
            .ok_or_else(|| OAuthError::access_denied("that agent is not available"))?;
        (project_id, agent_actor_id, Some(connection_id))
    } else if selection == "self" || selection.starts_with("self:") {
        // The app acts as this person across their whole account. The grant is
        // anchored to their Messages space, which every person belongs to;
        // each call is checked against their current memberships.
        let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
        let (home, _) =
            crate::direct::space_for_actor(&mut transaction, principal.actor_id, None).await?;
        transaction.commit().await.map_err(AppError::from)?;
        (home, principal.actor_id, None)
    } else {
        return Err(OAuthError::access_denied("choose who to connect as"));
    };

    let (code, prefix, digest) = generate_secret("aw_code_");
    sqlx::query(
        r#"INSERT INTO mcp_oauth_authorization_codes (
            id, client_id, project_id, actor_id, redirect_uri, scopes,
            code_prefix, code_digest, code_challenge, external_agent_connection_id, show_via
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
    )
    .bind(Uuid::now_v7())
    .bind(client.id)
    .bind(project_id)
    .bind(actor_id)
    .bind(&request.redirect_uri)
    .bind(&scopes)
    .bind(prefix)
    .bind(digest.to_vec())
    .bind(&request.code_challenge)
    .bind(connection_id)
    .bind(show_via)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    let mut redirect = Url::parse(&request.redirect_uri)
        .map_err(|_| OAuthError::invalid_request("invalid redirect URI"))?;
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &request.state);
    Ok(redirect_to_client(&redirect))
}

async fn validate_authorize_request(
    state: &AppState,
    request: &AuthorizeRequest,
) -> Result<(ClientRow, Vec<String>), OAuthError> {
    if request.response_type != "code" || request.code_challenge_method != "S256" {
        return Err(OAuthError::unsupported(
            "authorization code with PKCE S256 is required",
        ));
    }
    validate_state(&request.state)?;
    validate_code_challenge(&request.code_challenge)?;
    validate_redirect(&request.redirect_uri)?;
    if let Some(resource) = &request.resource
        && resource != &format!("{}{MCP_PATH}", public_origin(state))
    {
        return Err(OAuthError::invalid_request(
            "resource does not match this MCP endpoint",
        ));
    }
    let client_id = request
        .client_id
        .parse::<Uuid>()
        .map_err(|_| OAuthError::invalid_client("unknown public client"))?;
    let client = sqlx::query_as::<_, ClientRow>("SELECT id, client_name, redirect_uris FROM mcp_oauth_clients WHERE id = $1 AND status = 'active'")
        .bind(client_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OAuthError::invalid_client("unknown public client"))?;
    if !client
        .redirect_uris
        .iter()
        .any(|uri| uri == &request.redirect_uri)
    {
        return Err(OAuthError::invalid_request(
            "redirect URI does not exactly match registration",
        ));
    }
    Ok((client, parse_scopes(request.scope.as_deref())?))
}

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Result<Response, OAuthError> {
    let refreshing = request.grant_type == "refresh_token";
    let address = crate::rate_limit::client_address(&state.config, &headers);
    // A refresh is charged to its app only once its token checks out (in
    // `refresh`), and a refused one to its sender, so junk refreshes can't
    // spend the budget real apps need.
    let budget = if refreshing {
        state
            .rate_limits
            .check_oauth_refresh_sender(address, &request.client_id)
    } else {
        state.rate_limits.check_oauth_ceremony(&request.client_id)
    };
    if let Err(error) = budget {
        tracing::warn!(
            grant_type = %log_grant_type(&request.grant_type),
            "an OAuth token request was over its rate limit"
        );
        return Err(OAuthError::from(error));
    }
    let outcome = match request.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, &request).await,
        "refresh_token" => refresh(&state, &request).await,
        _ => Err(OAuthError::unsupported_grant("unsupported grant type")),
    };
    let response = match outcome {
        Ok(response) => {
            state
                .rate_limits
                .note_oauth_client_success(&request.client_id);
            response
        }
        Err(error) => {
            if refreshing
                && error.status.is_client_error()
                && error.status != StatusCode::TOO_MANY_REQUESTS
            {
                state
                    .rate_limits
                    .note_oauth_refresh_failure(address, &request.client_id);
            }
            // A refused refresh is what drops a connected app's tools, so say
            // why. Only the static reason is logged, never the request.
            if error.status != StatusCode::SERVICE_UNAVAILABLE {
                tracing::warn!(
                    grant_type = %log_grant_type(&request.grant_type),
                    client_id = %log_client_id(&request.client_id),
                    error = error.error,
                    reason = error.description,
                    "an OAuth token request was refused"
                );
            }
            return Err(error);
        }
    };
    let mut result = Json(response).into_response();
    result
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    result
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(result)
}

/// The grant type as sent, for a log line: one of the known ones, or
/// "other" so a client can't write arbitrary text into the logs.
fn log_grant_type(grant_type: &str) -> &'static str {
    match grant_type {
        "authorization_code" => "authorization_code",
        "refresh_token" => "refresh_token",
        _ => "other",
    }
}

/// A client ID is a UUID the server issued and isn't secret; anything else a
/// client sends there is logged as "invalid".
fn log_client_id(client_id: &str) -> String {
    Uuid::parse_str(client_id).map_or_else(|_| "invalid".to_owned(), |id| id.to_string())
}

#[derive(FromRow)]
struct CodeRow {
    id: Uuid,
    client_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    external_agent_connection_id: Option<Uuid>,
    redirect_uri: String,
    scopes: Vec<String>,
    code_digest: Vec<u8>,
    code_challenge: String,
    show_via: bool,
}

async fn exchange_code(
    state: &AppState,
    request: &TokenRequest,
) -> Result<TokenResponse, OAuthError> {
    let client_id = parse_client_id(&request.client_id)?;
    let code = request
        .code
        .as_deref()
        .ok_or_else(|| OAuthError::invalid_grant("authorization code is required"))?;
    let redirect_uri = request
        .redirect_uri
        .as_deref()
        .ok_or_else(|| OAuthError::invalid_grant("redirect URI is required"))?;
    let verifier = request
        .code_verifier
        .as_deref()
        .ok_or_else(|| OAuthError::invalid_grant("PKCE verifier is required"))?;
    validate_verifier(verifier)?;
    let (prefix, digest) = secret_lookup(code, "aw_code_")?;
    let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
    let row = sqlx::query_as::<_, CodeRow>(
        r#"
        SELECT code.id, code.client_id, code.project_id, code.actor_id,
               code.external_agent_connection_id,
               code.redirect_uri, code.scopes, code.code_digest, code.code_challenge,
               code.show_via
        FROM mcp_oauth_authorization_codes code
        JOIN mcp_oauth_clients client ON client.id = code.client_id
        JOIN projects project ON project.id = code.project_id
        JOIN actors actor ON actor.id = code.actor_id
        LEFT JOIN users user_account ON user_account.actor_id = code.actor_id
        JOIN project_memberships membership
          ON membership.project_id = code.project_id AND membership.actor_id = code.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = code.actor_id
         AND organization_membership.enabled
        WHERE code.code_prefix = $1 AND code.used_at IS NULL AND code.expires_at > now()
          AND client.status = 'active' AND project.archived_at IS NULL
          AND actor.status = 'active' AND membership.enabled
          AND (
              (actor.kind = 'human' AND user_account.status = 'active'
               AND code.external_agent_connection_id IS NULL)
              OR
              (actor.kind = 'external_agent' AND code.external_agent_connection_id IS NOT NULL)
          )
        FOR UPDATE OF code
    "#,
    )
    .bind(prefix)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(AppError::from)?
    .filter(|row| digest_matches(&row.code_digest, &digest))
    .ok_or_else(|| {
        OAuthError::invalid_grant("authorization code is invalid, expired, or already used")
    })?;
    let expected_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if row.client_id != client_id
        || row.redirect_uri != redirect_uri
        || !bool::from(
            expected_challenge
                .as_bytes()
                .ct_eq(row.code_challenge.as_bytes()),
        )
    {
        return Err(OAuthError::invalid_grant(
            "authorization code binding is invalid",
        ));
    }
    sqlx::query("UPDATE mcp_oauth_authorization_codes SET used_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    let backing_credential_id = match row.external_agent_connection_id {
        Some(connection_id) => {
            Some(create_backing_credential(&mut transaction, &row, connection_id).await?)
        }
        None => None,
    };
    let grant_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO mcp_oauth_grants (
            id, client_id, project_id, actor_id, scopes,
            external_agent_connection_id, backing_credential_id, show_via, account_wide
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
    )
    .bind(grant_id)
    .bind(client_id)
    .bind(row.project_id)
    .bind(row.actor_id)
    .bind(&row.scopes)
    .bind(row.external_agent_connection_id)
    .bind(backing_credential_id)
    .bind(row.show_via)
    // People connect apps for their whole account; agents keep their
    // connection's own reach.
    .bind(row.external_agent_connection_id.is_none())
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    let response = issue_token_pair(&mut transaction, grant_id, &row.scopes).await?;
    transaction.commit().await.map_err(AppError::from)?;
    tracing::info!(
        grant_id = %grant_id,
        client_id = %client_id,
        "a connected app signed in and got its first tokens"
    );
    Ok(response)
}

/// An agent credential whose secret is never issued. It lets an OAuth grant
/// pass the same checks as the agent's own bearer credentials, and it is
/// revoked with them when the connection is disabled, rotated, or removed.
async fn create_backing_credential(
    transaction: &mut Transaction<'_, Postgres>,
    row: &CodeRow,
    connection_id: Uuid,
) -> Result<Uuid, OAuthError> {
    let scopes = sqlx::query_scalar::<_, Vec<String>>(
        r#"
        SELECT scopes FROM external_agent_connections
        WHERE id = $1 AND project_id = $2 AND actor_id = $3 AND status = 'active'
        FOR SHARE
        "#,
    )
    .bind(connection_id)
    .bind(row.project_id)
    .bind(row.actor_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OAuthError::invalid_grant("the agent connection is no longer active"))?;
    let (_unused_secret, prefix, digest) = generate_secret("aw_ext_");
    let credential_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO mcp_client_credentials (
            id, project_id, actor_id, client_name, token_prefix, token_digest,
            scopes, expires_at, external_agent_connection_id
        ) VALUES ($1, $2, $3, 'OAuth grant', $4, $5, $6, now() + interval '10 years', $7)
        "#,
    )
    .bind(credential_id)
    .bind(row.project_id)
    .bind(row.actor_id)
    .bind(prefix)
    .bind(digest.to_vec())
    .bind(&scopes)
    .bind(connection_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    Ok(credential_id)
}

#[derive(FromRow)]
struct RefreshRow {
    id: Uuid,
    grant_id: Uuid,
    client_id: Uuid,
    scopes: Vec<String>,
    token_digest: Vec<u8>,
    used_at: Option<chrono::DateTime<chrono::Utc>>,
    revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn refresh(state: &AppState, request: &TokenRequest) -> Result<TokenResponse, OAuthError> {
    let client_id = parse_client_id(&request.client_id)?;
    let refresh_token = request
        .refresh_token
        .as_deref()
        .ok_or_else(|| OAuthError::invalid_grant("refresh token is required"))?;
    let (prefix, digest) = secret_lookup(refresh_token, "aw_refresh_")?;
    let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
    let row = sqlx::query_as::<_, RefreshRow>(
        r#"
        SELECT token.id, token.grant_id, grant_row.client_id, grant_row.scopes,
               token.token_digest, token.used_at, token.revoked_at
        FROM mcp_oauth_refresh_tokens token
        JOIN mcp_oauth_grants grant_row ON grant_row.id = token.grant_id
        JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
        JOIN projects project ON project.id = grant_row.project_id
        JOIN actors actor ON actor.id = grant_row.actor_id
        LEFT JOIN users user_account ON user_account.actor_id = grant_row.actor_id
        LEFT JOIN external_agent_connections connection
          ON connection.id = grant_row.external_agent_connection_id
        JOIN project_memberships membership
          ON membership.project_id = grant_row.project_id AND membership.actor_id = grant_row.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = grant_row.actor_id
         AND organization_membership.enabled
        WHERE token.token_prefix = $1 AND token.expires_at > now()
          AND grant_row.revoked_at IS NULL
          AND client.status = 'active' AND project.archived_at IS NULL
          AND actor.status = 'active' AND membership.enabled
          AND (
              (actor.kind = 'human' AND user_account.status = 'active'
               AND grant_row.external_agent_connection_id IS NULL)
              OR
              (actor.kind = 'external_agent' AND connection.status = 'active')
          )
        FOR UPDATE OF token, grant_row
    "#,
    )
    .bind(prefix)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(AppError::from)?
    .filter(|row| digest_matches(&row.token_digest, &digest))
    .ok_or_else(|| OAuthError::invalid_grant("refresh token is invalid or expired"))?;
    if row.client_id != client_id {
        return Err(OAuthError::invalid_grant(
            "refresh token client binding is invalid",
        ));
    }
    // Only now, with a token that checks out, does the refresh count against
    // its app's budget. Refused here, nothing is rotated.
    state
        .rate_limits
        .check_oauth_refresh(&row.client_id.to_string())?;
    let retired_seconds = row
        .used_at
        .map(|used_at| (chrono::Utc::now() - used_at).num_seconds());
    let reused = match (row.revoked_at, row.used_at) {
        (None, None) => false,
        (None, Some(used_at)) if chrono::Utc::now() - used_at <= refresh_reuse_grace() => true,
        _ => {
            revoke_grant_family(&mut transaction, row.grant_id, true).await?;
            transaction.commit().await.map_err(AppError::from)?;
            tracing::warn!(
                grant_id = %row.grant_id,
                client_id = %row.client_id,
                retired_seconds,
                "an old refresh token was used again, so its grant was revoked"
            );
            return Err(OAuthError::invalid_grant(
                "refresh token reuse revoked the grant family",
            ));
        }
    };
    if reused {
        // A client that runs many sessions on one grant refreshes from several
        // of them at once, and a client whose refresh response was lost
        // retries with the same token. Both are honest: give this one its own
        // pair and leave the grant alone. The new refresh token joins the
        // grant's current generation, so the next rotation from any branch
        // retires it like its siblings (see below) and a stolen copy can't
        // outlive the grace period.
        let response = issue_token_pair(&mut transaction, row.grant_id, &row.scopes).await?;
        transaction.commit().await.map_err(AppError::from)?;
        tracing::info!(
            grant_id = %row.grant_id,
            client_id = %row.client_id,
            retired_seconds,
            "a refresh token was used again within the grace period"
        );
        return Ok(response);
    }
    // Rotating retires every live refresh token of the grant, not just this
    // one. Without that, a replay inside the grace period would leave a second
    // branch that rotates on its own for 30 days and never trips reuse
    // detection. Now the branches stay one family: a sibling still works for
    // the grace period (parallel sessions), and used later it revokes the
    // grant. The grant row is locked above, so rotations of one grant run one
    // at a time.
    sqlx::query(
        "UPDATE mcp_oauth_refresh_tokens SET used_at = now() WHERE grant_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(row.grant_id)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    let response = issue_token_pair(&mut transaction, row.grant_id, &row.scopes).await?;
    let new_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM mcp_oauth_refresh_tokens WHERE token_digest = $1",
    )
    .bind(Sha256::digest(response.refresh_token.as_bytes()).to_vec())
    .fetch_one(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    sqlx::query("UPDATE mcp_oauth_refresh_tokens SET rotated_to_token_id = $2 WHERE id = $1")
        .bind(row.id)
        .bind(new_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    transaction.commit().await.map_err(AppError::from)?;
    tracing::info!(
        grant_id = %row.grant_id,
        client_id = %row.client_id,
        "a connected app refreshed its tokens"
    );
    Ok(response)
}

/// How long a refresh token that was already exchanged still works. Long
/// enough for parallel sessions and retries, short enough that a stolen token
/// used later still revokes the grant.
const REFRESH_REUSE_GRACE_SECONDS: i64 = 300;

fn refresh_reuse_grace() -> chrono::Duration {
    chrono::Duration::seconds(REFRESH_REUSE_GRACE_SECONDS)
}

/// How long an access token works. Every request still looks the token up,
/// so revoking a grant or removing a person cuts it off at once; the
/// lifetime only sets how often an app refreshes. An app running many
/// sessions on one grant refreshes from several at once, so fewer refreshes
/// mean fewer chances for that to go wrong.
const ACCESS_TOKEN_TTL_SECONDS: i64 = 12 * 60 * 60;

async fn issue_token_pair(
    transaction: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    scopes: &[String],
) -> Result<TokenResponse, OAuthError> {
    let (access_token, access_prefix, access_digest) = generate_secret("aw_oauth_");
    let (refresh_token, refresh_prefix, refresh_digest) = generate_secret("aw_refresh_");
    sqlx::query("INSERT INTO mcp_oauth_access_tokens (id, grant_id, token_prefix, token_digest, expires_at) VALUES ($1,$2,$3,$4, now() + make_interval(secs => $5))")
        .bind(Uuid::now_v7()).bind(grant_id).bind(access_prefix).bind(access_digest.to_vec())
        .bind(ACCESS_TOKEN_TTL_SECONDS as f64)
        .execute(&mut **transaction).await.map_err(AppError::from)?;
    sqlx::query("INSERT INTO mcp_oauth_refresh_tokens (id, grant_id, token_prefix, token_digest) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7()).bind(grant_id).bind(refresh_prefix).bind(refresh_digest.to_vec())
        .execute(&mut **transaction).await.map_err(AppError::from)?;
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL_SECONDS,
        refresh_token,
        scope: scopes.join(" "),
    })
}

#[derive(Deserialize)]
struct RevokeRequest {
    token: String,
    client_id: String,
}

async fn revoke(
    State(state): State<AppState>,
    Form(request): Form<RevokeRequest>,
) -> Result<StatusCode, OAuthError> {
    state
        .rate_limits
        .check_oauth_ceremony(&request.client_id)
        .map_err(OAuthError::from)?;
    let client_id = parse_client_id(&request.client_id)?;
    if request.token.len() > MAX_TOKEN {
        return Ok(StatusCode::OK);
    }
    let digest = Sha256::digest(request.token.as_bytes()).to_vec();
    let grant_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT grant_row.id FROM mcp_oauth_grants grant_row
        LEFT JOIN mcp_oauth_access_tokens access ON access.grant_id = grant_row.id
        LEFT JOIN mcp_oauth_refresh_tokens refresh ON refresh.grant_id = grant_row.id
        WHERE grant_row.client_id = $1 AND (access.token_digest = $2 OR refresh.token_digest = $2)
        LIMIT 1
    "#,
    )
    .bind(client_id)
    .bind(digest)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    if let Some(grant_id) = grant_id {
        let mut transaction = state.pool.begin().await.map_err(AppError::from)?;
        revoke_grant_family(&mut transaction, grant_id, false).await?;
        transaction.commit().await.map_err(AppError::from)?;
    }
    Ok(StatusCode::OK)
}

async fn revoke_grant_family(
    transaction: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    compromised: bool,
) -> Result<(), OAuthError> {
    sqlx::query("UPDATE mcp_oauth_grants SET revoked_at = COALESCE(revoked_at, now()), compromise_detected_at = CASE WHEN $2 THEN COALESCE(compromise_detected_at, now()) ELSE compromise_detected_at END WHERE id = $1")
        .bind(grant_id).bind(compromised).execute(&mut **transaction).await.map_err(AppError::from)?;
    sqlx::query("UPDATE mcp_oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE grant_id = $1")
        .bind(grant_id).execute(&mut **transaction).await.map_err(AppError::from)?;
    sqlx::query("UPDATE mcp_oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE grant_id = $1")
        .bind(grant_id).execute(&mut **transaction).await.map_err(AppError::from)?;
    Ok(())
}

#[derive(FromRow)]
struct AccessRow {
    token_id: Uuid,
    grant_id: Uuid,
    actor_id: Uuid,
    project_id: Uuid,
    client_name: String,
    scopes: Vec<String>,
    token_digest: Vec<u8>,
    external_agent_connection_id: Option<Uuid>,
    backing_credential_id: Option<Uuid>,
    connection_scopes: Option<Vec<String>>,
    account_wide: bool,
}

/// Why an access token that `resolve_access_token` turned away doesn't work,
/// for the log line on a refused MCP request. It looks at the token's own
/// row only, so it says nothing a caller couldn't already know about a
/// token they hold, and it's never sent to the client.
pub(crate) async fn rejected_access_token_reason(
    state: &AppState,
    token: &str,
) -> Result<&'static str, AppError> {
    let Ok((prefix, digest)) = secret_lookup(token, "aw_oauth_") else {
        return Ok("malformed");
    };
    let rows = sqlx::query_as::<_, RejectedAccessRow>(
        r#"
        SELECT access.token_digest, access.expires_at <= now() AS expired,
               access.revoked_at IS NOT NULL AS token_revoked,
               grant_row.revoked_at IS NOT NULL AS grant_revoked,
               grant_row.compromise_detected_at IS NOT NULL AS reuse_detected
        FROM mcp_oauth_access_tokens access
        JOIN mcp_oauth_grants grant_row ON grant_row.id = access.grant_id
        WHERE access.token_prefix = $1
        "#,
    )
    .bind(prefix)
    .fetch_all(&state.pool)
    .await?;
    let Some(row) = rows
        .into_iter()
        .find(|row| digest_matches(&row.token_digest, &digest))
    else {
        return Ok("unknown");
    };
    Ok(if row.reuse_detected {
        "grant revoked after an old refresh token was used again"
    } else if row.grant_revoked || row.token_revoked {
        "revoked"
    } else if row.expired {
        "expired"
    } else {
        "the person, project or app is no longer allowed"
    })
}

#[derive(FromRow)]
struct RejectedAccessRow {
    token_digest: Vec<u8>,
    expired: bool,
    token_revoked: bool,
    grant_revoked: bool,
    reuse_detected: bool,
}

pub async fn resolve_access_token(
    state: &AppState,
    token: &str,
) -> Result<Option<McpPrincipal>, AppError> {
    let Ok((prefix, digest)) = secret_lookup(token, "aw_oauth_") else {
        return Ok(None);
    };
    let rows = sqlx::query_as::<_, AccessRow>(
        r#"
        SELECT access.id AS token_id, grant_row.id AS grant_id, grant_row.actor_id,
               grant_row.project_id, client.client_name, grant_row.scopes, access.token_digest,
               grant_row.external_agent_connection_id, grant_row.backing_credential_id,
               connection.scopes AS connection_scopes, grant_row.account_wide
        FROM mcp_oauth_access_tokens access
        JOIN mcp_oauth_grants grant_row ON grant_row.id = access.grant_id
        JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
        JOIN projects project ON project.id = grant_row.project_id
        JOIN actors actor ON actor.id = grant_row.actor_id
        LEFT JOIN users user_account ON user_account.actor_id = grant_row.actor_id
        LEFT JOIN external_agent_connections connection
          ON connection.id = grant_row.external_agent_connection_id
        LEFT JOIN mcp_client_credentials backing
          ON backing.id = grant_row.backing_credential_id
        LEFT JOIN project_memberships membership
          ON membership.project_id = grant_row.project_id AND membership.actor_id = grant_row.actor_id
        LEFT JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = grant_row.actor_id
         AND organization_membership.enabled
        WHERE access.token_prefix = $1 AND access.revoked_at IS NULL
          AND access.expires_at > now() AND grant_row.revoked_at IS NULL
          AND client.status = 'active' AND actor.status = 'active'
          -- An app a person connected for their whole account keeps working
          -- after they leave the project it was anchored to: every call is
          -- checked against their current memberships. Project-scoped and
          -- agent grants still need their own project.
          AND (
              (actor.kind = 'human' AND grant_row.account_wide)
              OR (project.archived_at IS NULL AND membership.enabled
                  AND organization_membership.enabled)
          )
          AND (
              (actor.kind = 'human' AND user_account.status = 'active'
               AND grant_row.external_agent_connection_id IS NULL)
              OR
              (actor.kind = 'external_agent' AND connection.status = 'active'
               AND backing.status = 'active' AND backing.expires_at > now())
          )
    "#,
    )
    .bind(prefix)
    .fetch_all(&state.pool)
    .await?;
    let Some(row) = rows
        .into_iter()
        .find(|row| digest_matches(&row.token_digest, &digest))
    else {
        return Ok(None);
    };
    sqlx::query("UPDATE mcp_oauth_access_tokens SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')")
        .bind(row.token_id).execute(&state.pool).await?;
    if let (Some(connection_id), Some(backing_credential_id), Some(connection_scopes)) = (
        row.external_agent_connection_id,
        row.backing_credential_id,
        row.connection_scopes,
    ) {
        // Signed in as an agent: behave like that agent's own credential,
        // limited to what the person consented to for this app.
        let scopes = agent_grant_scopes(connection_scopes, &row.scopes);
        sqlx::query("UPDATE external_agent_connections SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')")
            .bind(connection_id).execute(&state.pool).await?;
        return Ok(Some(McpPrincipal {
            credential_id: backing_credential_id,
            mcp_credential_id: Some(backing_credential_id),
            oauth_grant_id: Some(row.grant_id),
            actor_id: row.actor_id,
            actor_kind: crate::policy::ActorKind::ExternalAgent,
            project_id: row.project_id,
            external_agent_connection_id: Some(connection_id),
            client_name: row.client_name,
            scopes,
            authentication_class: "mcp_oauth",
            account_wide: false,
            project_explicit: false,
        }));
    }
    Ok(Some(McpPrincipal {
        credential_id: row.grant_id,
        mcp_credential_id: None,
        oauth_grant_id: Some(row.grant_id),
        actor_id: row.actor_id,
        actor_kind: crate::policy::ActorKind::Human,
        project_id: row.project_id,
        external_agent_connection_id: None,
        client_name: row.client_name,
        scopes: row.scopes,
        authentication_class: "mcp_oauth",
        account_wide: row.account_wide,
        project_explicit: false,
    }))
}

/// An agent grant can do what its connection allows, but no more than the
/// person consented to: a read-only grant drops every scope that changes
/// anything.
fn agent_grant_scopes(connection_scopes: Vec<String>, grant_scopes: &[String]) -> Vec<String> {
    let grant_writes = grant_scopes.iter().any(|scope| scope == "write");
    connection_scopes
        .into_iter()
        .filter(|scope| grant_writes || matches!(scope.as_str(), "read" | "events"))
        .collect()
}

#[derive(FromRow)]
struct AgentChoice {
    connection_id: Uuid,
    display_name: String,
}

/// Active agent connections this person may manage, and so act as: the
/// candidates are the agents in projects the person is an enabled member of,
/// and `external_agents::may_manage` decides each one (P0.6).
async fn agent_choices(state: &AppState, actor_id: Uuid) -> Result<Vec<AgentChoice>, OAuthError> {
    let candidates = sqlx::query_as::<_, AgentChoice>(
        r#"
        SELECT connection.id AS connection_id, agent_membership.display_name
        FROM external_agent_connections connection
        JOIN projects project ON project.id = connection.project_id
        JOIN project_memberships agent_membership
          ON agent_membership.project_id = connection.project_id
         AND agent_membership.actor_id = connection.actor_id
         AND agent_membership.enabled
        JOIN project_memberships manager
          ON manager.project_id = connection.project_id
         AND manager.actor_id = $1
         AND manager.enabled
        WHERE connection.status = 'active' AND project.archived_at IS NULL
        ORDER BY lower(agent_membership.display_name), connection.id
        LIMIT 100
        "#,
    )
    .bind(actor_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    let mut connection = state.pool.acquire().await.map_err(AppError::from)?;
    let mut choices = Vec::new();
    for candidate in candidates {
        if crate::external_agents::may_manage(&mut connection, actor_id, candidate.connection_id)
            .await?
            .is_some()
        {
            choices.push(candidate);
        }
    }
    Ok(choices)
}

/// The agent's Project and Actor, if this person may connect clients as it.
async fn managed_agent(
    state: &AppState,
    manager_actor_id: Uuid,
    connection_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, OAuthError> {
    let mut connection = state.pool.acquire().await.map_err(AppError::from)?;
    let Some(managed) =
        crate::external_agents::may_manage(&mut connection, manager_actor_id, connection_id)
            .await?
    else {
        return Ok(None);
    };
    let (project_id, agent_actor_id, organization_id) = (
        managed.project_id,
        managed.agent_actor_id,
        managed.organization_id,
    );
    let (role, role_permissions) = sqlx::query_as::<_, (String, Option<Vec<String>>)>(
        r#"
        SELECT role, workspace_role_permissions(role_id)
        FROM project_memberships
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(agent_actor_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(AppError::from)?;
    // Connecting as the agent hands its role to the app: only within what the
    // manager holds, as creating or re-enabling the agent requires.
    let grant = crate::policy::RoleGrant::stored(&role, role_permissions.as_deref())
        .map_err(AppError::from)?;
    let manager =
        crate::roles::standing(&mut connection, organization_id, manager_actor_id).await?;
    if !grant.permissions().is_subset_of(manager.permissions) {
        return Ok(None);
    }
    // And within the manager's role in the agent's project, where it works,
    // as giving an agent its role or credentials is
    // (external_agents::require_grantable).
    let held = crate::roles::project_holding(&mut connection, manager_actor_id, project_id).await?;
    if !held
        .is_some_and(|held| crate::roles::project_checked(grant.permissions()).is_subset_of(held))
    {
        return Ok(None);
    }
    Ok(Some((project_id, agent_actor_id)))
}

fn parse_scopes(scope: Option<&str>) -> Result<Vec<String>, OAuthError> {
    let scope = scope.unwrap_or("read");
    if scope.is_empty() || scope.len() > 32 {
        return Err(OAuthError::invalid_scope("scope is invalid"));
    }
    // `workspace:read`/`workspace:write` are the names advertised when AuthKit
    // is also an authorization server; store them under the local names.
    let mut values = scope
        .split(' ')
        .map(|value| value.strip_prefix("workspace:").unwrap_or(value).to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    if values
        .iter()
        .any(|value| value != "read" && value != "write")
        || !values.iter().any(|value| value == "read")
    {
        return Err(OAuthError::invalid_scope(
            "scope must include read and may include write",
        ));
    }
    Ok(values)
}

/// Apps name themselves freely: Claude Code registers as
/// "Claude Code (<server nickname>)". Keep the app's own name in the plain
/// characters the consent page and "via" labels allow, rather than refusing
/// the connection over punctuation.
fn clean_client_name(raw: &str) -> String {
    let raw = raw.trim();
    let base = raw
        .strip_suffix(')')
        .and_then(|rest| rest.rfind(" (").map(|index| &rest[..index]))
        .filter(|base| !base.trim().is_empty())
        .unwrap_or(raw);
    let plain: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                ' '
            }
        })
        .collect();
    let mut name = plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches(|character: char| !character.is_ascii_alphanumeric())
        .chars()
        .take(MAX_CLIENT_NAME)
        .collect::<String>();
    name.truncate(name.trim_end().len());
    if name.is_empty() {
        "Connected app".to_owned()
    } else {
        name
    }
}

fn validate_client_name(name: &str) -> Result<(), OAuthError> {
    if name.is_empty()
        || name.chars().count() > MAX_CLIENT_NAME
        || name.trim() != name
        || name.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ' '))
        })
    {
        return Err(OAuthError::invalid_client_metadata(
            "client name is invalid",
        ));
    }
    Ok(())
}

/// Local clients (Codex) use exact HTTP loopback redirects with a port.
/// Hosted clients (cloud agent products) use HTTPS. Redirects must still match
/// the registration exactly.
fn validate_redirect(value: &str) -> Result<(), OAuthError> {
    if value.len() > MAX_REDIRECT_URI {
        return Err(OAuthError::invalid_client_metadata(
            "redirect URI is too long",
        ));
    }
    let url = Url::parse(value)
        .map_err(|_| OAuthError::invalid_client_metadata("redirect URI is invalid"))?;
    let hosted = url.scheme() == "https" && url.host_str().is_some_and(|host| !host.is_empty());
    let loopback = is_loopback_redirect(&url) && url.query().is_none();
    if !(loopback || hosted)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(OAuthError::invalid_client_metadata(
            "redirects must be HTTPS, or exact HTTP loopback with an explicit port",
        ));
    }
    Ok(())
}

fn is_loopback_redirect(url: &Url) -> bool {
    url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
        && url.port().is_some()
}

/// Send the browser back to the client with a tiny page that forwards
/// itself. A plain redirect after the consent form is blocked by the site's
/// `form-action 'self'` policy, because Chrome applies it to the redirect's
/// target, including loopback clients such as Codex and Claude Code.
fn redirect_to_client(redirect: &Url) -> Response {
    let target = escape_html(redirect.as_str());
    Html(format!(
        "<!doctype html><html><head><meta charset=utf-8><meta http-equiv=refresh content=\"0; url={target}\"><title>Returning</title></head><body><main><p>Returning to the app… <a href=\"{target}\">Continue</a></p></main></body></html>"
    ))
    .into_response()
}

fn validate_state(state: &str) -> Result<(), OAuthError> {
    if state.is_empty() || state.len() > MAX_STATE || state.chars().any(char::is_control) {
        return Err(OAuthError::invalid_request("state is invalid"));
    }
    Ok(())
}

fn validate_code_challenge(challenge: &str) -> Result<(), OAuthError> {
    if challenge.len() != 43
        || challenge
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        return Err(OAuthError::invalid_request(
            "PKCE S256 challenge is invalid",
        ));
    }
    Ok(())
}

fn validate_verifier(verifier: &str) -> Result<(), OAuthError> {
    if !(43..=128).contains(&verifier.len())
        || verifier.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
        })
    {
        return Err(OAuthError::invalid_grant("PKCE verifier is invalid"));
    }
    Ok(())
}

fn generate_secret(prefix: &str) -> (String, String, [u8; 32]) {
    let mut random = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut random);
    let token = format!("{prefix}{}", URL_SAFE_NO_PAD.encode(random));
    let lookup = token.chars().take(20).collect::<String>();
    let digest = Sha256::digest(token.as_bytes()).into();
    (token, lookup, digest)
}

fn secret_lookup<'a>(
    token: &'a str,
    required_prefix: &str,
) -> Result<(&'a str, [u8; 32]), OAuthError> {
    if !token.starts_with(required_prefix)
        || token.len() > MAX_TOKEN
        || token.len() < 20
        || token
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        return Err(OAuthError::invalid_grant("token is invalid"));
    }
    Ok((&token[..20], Sha256::digest(token.as_bytes()).into()))
}

fn digest_matches(stored: &[u8], candidate: &[u8; 32]) -> bool {
    stored.len() == candidate.len() && bool::from(stored.ct_eq(candidate))
}

fn parse_client_id(value: &str) -> Result<Uuid, OAuthError> {
    value
        .parse::<Uuid>()
        .map_err(|_| OAuthError::invalid_client("unknown public client"))
}

fn csrf_cookie(config: &crate::config::AppConfig, headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, csrf_cookie_name(config))
}

fn hidden_authorize_fields(request: &AuthorizeRequest, scope: &str, csrf: &str) -> String {
    let mut fields = vec![
        ("response_type", request.response_type.as_str()),
        ("client_id", request.client_id.as_str()),
        ("redirect_uri", request.redirect_uri.as_str()),
        ("scope", scope),
        ("state", request.state.as_str()),
        ("code_challenge", request.code_challenge.as_str()),
        (
            "code_challenge_method",
            request.code_challenge_method.as_str(),
        ),
        ("csrf_token", csrf),
    ];
    if let Some(resource) = request.resource.as_deref() {
        fields.push(("resource", resource));
    }
    fields
        .into_iter()
        .map(|(name, value)| {
            format!(
                "<input type=hidden name=\"{}\" value=\"{}\">",
                name,
                escape_html(value)
            )
        })
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn redirect_authorization_error(
    redirect_uri: &str,
    state: &str,
    error: &str,
) -> Result<Response, OAuthError> {
    validate_redirect(redirect_uri)?;
    let mut redirect = Url::parse(redirect_uri)
        .map_err(|_| OAuthError::invalid_request("invalid redirect URI"))?;
    redirect
        .query_pairs_mut()
        .append_pair("error", error)
        .append_pair("state", state);
    Ok(redirect_to_client(&redirect))
}

#[derive(Debug)]
pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: &'static str,
    retry_after_seconds: Option<u64>,
}

impl OAuthError {
    fn new(status: StatusCode, error: &'static str, description: &'static str) -> Self {
        Self {
            status,
            error,
            description,
            retry_after_seconds: None,
        }
    }
    fn invalid_request(description: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", description)
    }
    fn invalid_client(description: &'static str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_client", description)
    }
    fn invalid_client_metadata(description: &'static str) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            description,
        )
    }
    fn invalid_grant(description: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_grant", description)
    }
    fn invalid_scope(description: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_scope", description)
    }
    fn unsupported(description: &'static str) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_response_type",
            description,
        )
    }
    fn unsupported_grant(description: &'static str) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            description,
        )
    }
    fn access_denied(description: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, "access_denied", description)
    }
}

impl From<AppError> for OAuthError {
    fn from(error: AppError) -> Self {
        if let AppError::RateLimited {
            retry_after_seconds,
        } = error
        {
            return Self {
                status: StatusCode::TOO_MANY_REQUESTS,
                error: "slow_down",
                description: "the local authorization attempt budget is exhausted",
                retry_after_seconds: Some(retry_after_seconds),
            };
        }
        let (database_code, database_constraint) = error.database_log_fields();
        tracing::error!(
            category = error.log_category(),
            database_code,
            database_constraint,
            "OAuth operation failed"
        );
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the authorization service is temporarily unavailable",
        )
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(serde_json::json!({ "error": self.error, "error_description": self.description })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(retry_after_seconds) = self.retry_after_seconds
            && let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn production_consent_uses_secure_csrf_cookie_when_both_names_are_present() {
        let config = crate::config::AppConfig::from_values(&BTreeMap::from([
            (
                "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                "postgres://example.invalid/database".to_owned(),
            ),
            ("AGENT_WORKSPACE_ENV".to_owned(), "production".to_owned()),
            (
                "AGENT_WORKSPACE_ALLOWED_ORIGINS".to_owned(),
                "https://workspace.example.com".to_owned(),
            ),
            (
                "AGENT_WORKSPACE_SERVER_ADDR".to_owned(),
                "0.0.0.0:3100".to_owned(),
            ),
        ]))
        .expect("production config");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "agent_workspace_csrf=stale; __Host-agent_workspace_csrf=current",
            ),
        );

        assert_eq!(csrf_cookie(&config, &headers).as_deref(), Some("current"));
    }

    fn authorize_query(resources: &[&str]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer
            .append_pair("response_type", "code")
            .append_pair("client_id", "019fbebf-120b-7540-a3fd-1248fb69dd2a")
            .append_pair("redirect_uri", "http://127.0.0.1:61169/callback")
            .append_pair("scope", "read write")
            .append_pair("state", "state")
            .append_pair("code_challenge", &"a".repeat(43))
            .append_pair("code_challenge_method", "S256");
        for resource in resources {
            serializer.append_pair("resource", resource);
        }
        serializer.finish()
    }

    #[test]
    fn authorize_query_canonicalizes_only_identical_resource_duplicates() {
        let expected = "http://127.0.0.1:3100/api/v1/mcp";
        let request = parse_authorize_query(Some(authorize_query(&[expected, expected])))
            .expect("identical resource parameters");
        assert_eq!(request.resource.as_deref(), Some(expected));

        let error = parse_authorize_query(Some(authorize_query(&[
            expected,
            "http://127.0.0.1:3100/api/v1/other",
        ])))
        .expect_err("conflicting resource parameters must fail");
        assert_eq!(error.error, "invalid_request");
    }

    #[test]
    fn redirects_are_exact_public_loopback_uris() {
        assert!(validate_redirect("http://127.0.0.1:1455/callback").is_ok());
        assert!(validate_redirect("http://localhost:2345/oauth/callback").is_ok());
        assert!(validate_redirect("https://agent.example.com/oauth/callback").is_ok());
        for unsafe_uri in [
            "http://example.com:1455/callback",
            "https://user@agent.example.com/callback",
            "http://127.0.0.1/callback",
            "http://user@127.0.0.1:1455/callback",
            "http://127.0.0.1:1455/callback#code",
            "http://127.0.0.1:1455/callback?next=evil",
        ] {
            assert!(validate_redirect(unsafe_uri).is_err(), "{unsafe_uri}");
        }
    }

    #[test]
    fn oauth_rate_limit_is_bounded_and_retryable() {
        let response = OAuthError::from(AppError::rate_limited(17)).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("17"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );
    }

    #[test]
    fn read_only_consent_keeps_an_agent_grant_read_only() {
        let connection = [
            "read",
            "write",
            "events",
            "approval_respond",
            "worker_request",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            agent_grant_scopes(connection.clone(), &["read".to_owned()]),
            vec!["read".to_owned(), "events".to_owned()],
        );
        assert_eq!(
            agent_grant_scopes(connection.clone(), &["read".to_owned(), "write".to_owned()]),
            connection,
        );
        // Consent never adds what the connection itself lacks.
        assert_eq!(
            agent_grant_scopes(
                vec!["read".to_owned()],
                &["read".to_owned(), "write".to_owned()]
            ),
            vec!["read".to_owned()],
        );
    }

    #[test]
    fn pkce_and_scope_validation_are_fail_closed() {
        assert!(validate_code_challenge(&"a".repeat(43)).is_ok());
        assert!(validate_code_challenge(&"a".repeat(42)).is_err());
        assert_eq!(
            parse_scopes(Some("read write")).expect("scopes"),
            vec!["read", "write"]
        );
        assert_eq!(
            parse_scopes(Some("workspace:read workspace:write")).expect("scopes"),
            vec!["read", "write"]
        );
        assert!(parse_scopes(Some("write")).is_err());
        assert!(parse_scopes(Some("workspace:write")).is_err());
        assert!(parse_scopes(Some("read admin")).is_err());
        assert!(secret_lookup("aw_oauth_aaaaaaaéaaaa", "aw_oauth_").is_err());
    }

    #[test]
    fn client_names_are_cleaned_instead_of_refused() {
        for (raw, clean) in [
            ("Claude Code (agent-workspace)", "Claude Code"),
            ("Codex", "Codex"),
            ("ChatGPT", "ChatGPT"),
            ("  Cursor: Agent!  ", "Cursor Agent"),
            ("(agent-workspace)", "agent-workspace"),
            ("✨ 日本語 ✨", "Connected app"),
            ("", "Connected app"),
        ] {
            let cleaned = clean_client_name(raw);
            assert_eq!(cleaned, clean, "{raw}");
            assert!(validate_client_name(&cleaned).is_ok(), "{raw}");
        }
        assert!(validate_client_name(&clean_client_name(&"a ".repeat(200))).is_ok());
    }

    #[test]
    fn consent_html_escapes_untrusted_values() {
        assert_eq!(
            escape_html("<script x='y'>&\""),
            "&lt;script x=&#39;y&#39;&gt;&amp;&quot;"
        );
    }
}
