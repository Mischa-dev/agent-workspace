use axum::{
    Form, Json, Router,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
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
    auth::{AuthenticatedActor, require_csrf},
    error::{ApiJson, AppError},
};

use super::{MCP_PATH, McpPrincipal};

const TOKEN_BYTES: usize = 32;
const MAX_CLIENT_NAME: usize = 80;
const MAX_REDIRECT_URI: usize = 512;
const MAX_STATE: usize = 512;
const MAX_TOKEN: usize = 128;

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
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
}

fn public_origin(state: &AppState) -> String {
    format!("http://{}", state.config.server_addr)
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
    Json(ProtectedResourceMetadata {
        resource: format!("{origin}{MCP_PATH}"),
        authorization_servers: vec![origin],
        scopes_supported: vec!["read", "write"],
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

async fn register_client(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<RegistrationRequest>,
) -> Result<(StatusCode, Json<RegistrationResponse>), OAuthError> {
    validate_client_name(&request.client_name)?;
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > 8 {
        return Err(OAuthError::invalid_client_metadata(
            "one to eight loopback redirect URIs are required",
        ));
    }
    for redirect_uri in &request.redirect_uris {
        validate_loopback_redirect(redirect_uri)?;
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
    let client_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mcp_oauth_clients")
        .fetch_one(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    if client_count >= 1_024 {
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

#[derive(FromRow)]
struct ProjectChoice {
    id: Uuid,
    name: String,
    project_key: String,
}

async fn authorize(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Html<String>, OAuthError> {
    let request = parse_authorize_query(raw_query)?;
    let (client, scopes) = validate_authorize_request(&state, &request).await?;
    let projects = project_choices(&state, principal.actor_id).await?;
    if projects.is_empty() {
        return Err(OAuthError::access_denied(
            "no active Project membership is available",
        ));
    }
    let csrf = csrf_cookie(&headers)
        .ok_or_else(|| OAuthError::access_denied("the browser session is missing CSRF state"))?;
    let scope = scopes.join(" ");
    let project_controls = if projects.len() == 1 {
        format!(
            "<input type=hidden name=project_id value=\"{}\"><p><strong>Project:</strong> {} ({})</p>",
            projects[0].id,
            escape_html(&projects[0].name),
            escape_html(&projects[0].project_key)
        )
    } else {
        let choices = projects.iter().map(|project| format!("<label><input required type=radio name=project_id value=\"{}\"> {} ({})</label><br>", project.id, escape_html(&project.name), escape_html(&project.project_key))).collect::<String>();
        format!("<fieldset><legend>Choose one Project</legend>{choices}</fieldset>")
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>Authorize Codex</title></head><body><main><h1>Authorize {}</h1><p>This local Codex client requests <strong>{}</strong> access through Agent Workspace MCP. Calls are attributed to {}.</p>{}<form method=post action=/oauth/authorize>{}<button name=decision value=approve type=submit>Authorize</button><button name=decision value=deny type=submit>Cancel</button></form></main></body></html>",
        escape_html(&client.client_name),
        escape_html(&scope),
        escape_html(&principal.display_name),
        project_controls,
        hidden_authorize_fields(&request, &scope, &csrf),
    );
    Ok(Html(html))
}

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
    project_id: String,
    csrf_token: String,
    decision: String,
}

async fn consent(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    mut headers: HeaderMap,
    Form(form): Form<ConsentRequest>,
) -> Result<Response, OAuthError> {
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
    let (client, scopes) = validate_authorize_request(&state, &request).await?;
    if form.decision != "approve" {
        return redirect_authorization_error(
            &request.redirect_uri,
            &request.state,
            "access_denied",
        );
    }
    let project_id = form
        .project_id
        .parse::<Uuid>()
        .map_err(|_| OAuthError::access_denied("invalid Project selection"))?;
    let membership = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(
            SELECT 1 FROM projects p
            JOIN project_memberships pm ON pm.project_id = p.id
            JOIN actors a ON a.id = pm.actor_id
            WHERE p.id = $1 AND pm.actor_id = $2 AND p.archived_at IS NULL
              AND pm.enabled AND a.kind = 'human' AND a.status = 'active'
        )"#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;
    if !membership {
        return Err(OAuthError::access_denied(
            "the selected Project is not available",
        ));
    }

    let (code, prefix, digest) = generate_secret("aw_code_");
    sqlx::query(
        r#"INSERT INTO mcp_oauth_authorization_codes (
            id, client_id, project_id, actor_id, redirect_uri, scopes,
            code_prefix, code_digest, code_challenge
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
    )
    .bind(Uuid::now_v7())
    .bind(client.id)
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(&request.redirect_uri)
    .bind(&scopes)
    .bind(prefix)
    .bind(digest.to_vec())
    .bind(&request.code_challenge)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;

    let mut redirect = Url::parse(&request.redirect_uri)
        .map_err(|_| OAuthError::invalid_request("invalid redirect URI"))?;
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &request.state);
    Ok(Redirect::to(redirect.as_str()).into_response())
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
    validate_loopback_redirect(&request.redirect_uri)?;
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
    Form(request): Form<TokenRequest>,
) -> Result<Response, OAuthError> {
    let response = match request.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, request).await?,
        "refresh_token" => refresh(&state, request).await?,
        _ => return Err(OAuthError::unsupported_grant("unsupported grant type")),
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

#[derive(FromRow)]
struct CodeRow {
    id: Uuid,
    client_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    redirect_uri: String,
    scopes: Vec<String>,
    code_digest: Vec<u8>,
    code_challenge: String,
}

async fn exchange_code(
    state: &AppState,
    request: TokenRequest,
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
               code.redirect_uri, code.scopes, code.code_digest, code.code_challenge
        FROM mcp_oauth_authorization_codes code
        JOIN mcp_oauth_clients client ON client.id = code.client_id
        JOIN projects project ON project.id = code.project_id
        JOIN actors actor ON actor.id = code.actor_id
        JOIN project_memberships membership
          ON membership.project_id = code.project_id AND membership.actor_id = code.actor_id
        WHERE code.code_prefix = $1 AND code.used_at IS NULL AND code.expires_at > now()
          AND client.status = 'active' AND project.archived_at IS NULL
          AND actor.kind = 'human' AND actor.status = 'active' AND membership.enabled
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
    let grant_id = Uuid::now_v7();
    sqlx::query("INSERT INTO mcp_oauth_grants (id, client_id, project_id, actor_id, scopes) VALUES ($1,$2,$3,$4,$5)")
        .bind(grant_id).bind(client_id).bind(row.project_id).bind(row.actor_id).bind(&row.scopes)
        .execute(&mut *transaction).await.map_err(AppError::from)?;
    let response = issue_token_pair(&mut transaction, grant_id, &row.scopes).await?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok(response)
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

async fn refresh(state: &AppState, request: TokenRequest) -> Result<TokenResponse, OAuthError> {
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
        JOIN project_memberships membership
          ON membership.project_id = grant_row.project_id AND membership.actor_id = grant_row.actor_id
        WHERE token.token_prefix = $1 AND token.expires_at > now()
          AND grant_row.revoked_at IS NULL
          AND client.status = 'active' AND project.archived_at IS NULL
          AND actor.kind = 'human' AND actor.status = 'active' AND membership.enabled
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
    if row.used_at.is_some() || row.revoked_at.is_some() {
        revoke_grant_family(&mut transaction, row.grant_id, true).await?;
        transaction.commit().await.map_err(AppError::from)?;
        return Err(OAuthError::invalid_grant(
            "refresh token reuse revoked the grant family",
        ));
    }
    sqlx::query("UPDATE mcp_oauth_refresh_tokens SET used_at = now() WHERE id = $1")
        .bind(row.id)
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
    Ok(response)
}

async fn issue_token_pair(
    transaction: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    scopes: &[String],
) -> Result<TokenResponse, OAuthError> {
    let (access_token, access_prefix, access_digest) = generate_secret("aw_oauth_");
    let (refresh_token, refresh_prefix, refresh_digest) = generate_secret("aw_refresh_");
    sqlx::query("INSERT INTO mcp_oauth_access_tokens (id, grant_id, token_prefix, token_digest) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7()).bind(grant_id).bind(access_prefix).bind(access_digest.to_vec())
        .execute(&mut **transaction).await.map_err(AppError::from)?;
    sqlx::query("INSERT INTO mcp_oauth_refresh_tokens (id, grant_id, token_prefix, token_digest) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7()).bind(grant_id).bind(refresh_prefix).bind(refresh_digest.to_vec())
        .execute(&mut **transaction).await.map_err(AppError::from)?;
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: 3600,
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
               grant_row.project_id, client.client_name, grant_row.scopes, access.token_digest
        FROM mcp_oauth_access_tokens access
        JOIN mcp_oauth_grants grant_row ON grant_row.id = access.grant_id
        JOIN mcp_oauth_clients client ON client.id = grant_row.client_id
        JOIN projects project ON project.id = grant_row.project_id
        JOIN actors actor ON actor.id = grant_row.actor_id
        JOIN project_memberships membership
          ON membership.project_id = grant_row.project_id AND membership.actor_id = grant_row.actor_id
        WHERE access.token_prefix = $1 AND access.revoked_at IS NULL
          AND access.expires_at > now() AND grant_row.revoked_at IS NULL
          AND client.status = 'active' AND project.archived_at IS NULL
          AND actor.kind = 'human' AND actor.status = 'active' AND membership.enabled
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
    Ok(Some(McpPrincipal {
        credential_id: row.grant_id,
        mcp_credential_id: None,
        oauth_grant_id: Some(row.grant_id),
        actor_id: row.actor_id,
        project_id: row.project_id,
        client_name: row.client_name,
        scopes: row.scopes,
        authentication_class: "mcp_oauth",
    }))
}

async fn project_choices(
    state: &AppState,
    actor_id: Uuid,
) -> Result<Vec<ProjectChoice>, OAuthError> {
    sqlx::query_as::<_, ProjectChoice>(
        r#"
        SELECT project.id, project.name, project.project_key
        FROM projects project
        JOIN project_memberships membership ON membership.project_id = project.id
        WHERE membership.actor_id = $1 AND membership.enabled AND project.archived_at IS NULL
        ORDER BY project.name, project.id LIMIT 100
    "#,
    )
    .bind(actor_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)
    .map_err(Into::into)
}

fn parse_scopes(scope: Option<&str>) -> Result<Vec<String>, OAuthError> {
    let scope = scope.unwrap_or("read");
    if scope.is_empty() || scope.len() > 32 {
        return Err(OAuthError::invalid_scope("scope is invalid"));
    }
    let mut values = scope.split(' ').map(str::to_owned).collect::<Vec<_>>();
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

fn validate_loopback_redirect(value: &str) -> Result<(), OAuthError> {
    if value.len() > MAX_REDIRECT_URI {
        return Err(OAuthError::invalid_client_metadata(
            "redirect URI is too long",
        ));
    }
    let url = Url::parse(value)
        .map_err(|_| OAuthError::invalid_client_metadata("redirect URI is invalid"))?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(OAuthError::invalid_client_metadata(
            "only exact HTTP loopback redirects with an explicit port are allowed",
        ));
    }
    Ok(())
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

fn csrf_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        matches!(name, "agent_workspace_csrf" | "__Host-agent_workspace_csrf")
            .then(|| value.to_owned())
    })
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
    validate_loopback_redirect(redirect_uri)?;
    let mut redirect = Url::parse(redirect_uri)
        .map_err(|_| OAuthError::invalid_request("invalid redirect URI"))?;
    redirect
        .query_pairs_mut()
        .append_pair("error", error)
        .append_pair("state", state);
    Ok(Redirect::to(redirect.as_str()).into_response())
}

#[derive(Debug)]
pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: &'static str,
}

impl OAuthError {
    fn new(status: StatusCode, error: &'static str, description: &'static str) -> Self {
        Self {
            status,
            error,
            description,
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
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(validate_loopback_redirect("http://127.0.0.1:1455/callback").is_ok());
        assert!(validate_loopback_redirect("http://localhost:2345/oauth/callback").is_ok());
        for unsafe_uri in [
            "https://127.0.0.1:1455/callback",
            "http://example.com:1455/callback",
            "http://127.0.0.1/callback",
            "http://user@127.0.0.1:1455/callback",
            "http://127.0.0.1:1455/callback#code",
            "http://127.0.0.1:1455/callback?next=evil",
        ] {
            assert!(
                validate_loopback_redirect(unsafe_uri).is_err(),
                "{unsafe_uri}"
            );
        }
    }

    #[test]
    fn pkce_and_scope_validation_are_fail_closed() {
        assert!(validate_code_challenge(&"a".repeat(43)).is_ok());
        assert!(validate_code_challenge(&"a".repeat(42)).is_err());
        assert_eq!(
            parse_scopes(Some("read write")).expect("scopes"),
            vec!["read", "write"]
        );
        assert!(parse_scopes(Some("write")).is_err());
        assert!(parse_scopes(Some("read admin")).is_err());
        assert!(secret_lookup("aw_oauth_aaaaaaaéaaaa", "aw_oauth_").is_err());
    }

    #[test]
    fn consent_html_escapes_untrusted_values() {
        assert_eq!(
            escape_html("<script x='y'>&\""),
            "&lt;script x=&#39;y&#39;&gt;&amp;&quot;"
        );
    }
}
