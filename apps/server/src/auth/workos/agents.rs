//! WorkOS tokens authenticate named agents. Local grants still bound every call.
use super::{client, config, public_origin};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    config::WorkOsConfig,
    error::{ApiJson, ApiPath, AppError},
    mcp::{McpPrincipal, exact_bearer_token, resolve_bearer_principal},
    policy::ActorKind,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth.md", get(auth_document))
        .route("/api/v1/agents/token", post(mint))
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
            get(list).post(attach),
        )
        .route(
            "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos/{binding_id}",
            axum::routing::delete(revoke),
        )
}

fn domain(config: &WorkOsConfig) -> Result<&str, AppError> {
    config
        .authkit_domain
        .as_deref()
        .ok_or(AppError::NotFound("WorkOS agent authentication"))
}
fn valid_id(id: &str, prefix: &str) -> bool {
    id.starts_with(prefix)
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
async fn api(
    config: &WorkOsConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, AppError> {
    let mut request = client()?
        .request(method, format!("{}{path}", config.api_base))
        .bearer_auth(config.api_key.expose());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| AppError::Internal("WorkOS agent service unavailable"))?;
    if !response.status().is_success() {
        // Provider bodies may contain credentials. Never log or forward them.
        return Err(if response.status().is_client_error() {
            AppError::InvalidCredentials
        } else {
            AppError::Internal("WorkOS agent service unavailable")
        });
    }
    response
        .json()
        .await
        .map_err(|_| AppError::Internal("WorkOS agent response"))
}

#[derive(Clone, Deserialize)]
struct Claims {
    sub: String,
    org_id: String,
    exp: u64,
    iat: u64,
    jti: String,
    #[serde(default)]
    sub_profile: Option<String>,
    #[serde(default)]
    agent_blueprint_id: Option<String>,
    #[serde(default)]
    sid: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    act: Option<Value>,
}
fn verify_claims(config: &WorkOsConfig, token: &str, jwks: &JwkSet) -> Option<Claims> {
    let header = decode_header(token).ok()?;
    if header.alg != Algorithm::RS256 {
        return None;
    }
    let key = jwks.find(header.kid.as_deref()?)?;
    let key = DecodingKey::from_jwk(key).ok()?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[config.authkit_domain.as_deref()?]);
    // Only this application can receive these tokens, never arbitrary resources.
    validation.set_audience(&[&config.client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "sub", "iat"]);
    validation.leeway = 0;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(token, &key, &validation).ok()?.claims;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if claims.iat > now + 30
        || claims.exp <= now
        || claims.jti.is_empty()
        || !valid_id(&claims.org_id, "org_")
    {
        return None;
    }
    if let Some(blueprint) = &claims.agent_blueprint_id {
        if claims.sub_profile.as_deref() != Some("ai_agent")
            || claims.act.is_some()
            || !valid_id(blueprint, "agent_blueprint_")
            || !valid_id(&claims.sub, "agent_")
            || !claims.sid.as_deref().is_some_and(|id| {
                valid_id(id, "agent_session_") || valid_id(id, "agent_instance_session_")
            })
        {
            return None;
        }
    } else if !valid_id(&claims.sub, "agent_reg_")
        || claims
            .act
            .as_ref()
            .and_then(|a| a.get("sub"))
            .and_then(Value::as_str)
            .is_none()
    {
        return None;
    }
    Some(claims)
}
async fn verified_token(config: &WorkOsConfig, token: &str) -> Result<Option<Claims>, AppError> {
    domain(config)?;
    if token.len() > 16 * 1024 {
        return Ok(None);
    }
    let response = client()?
        .get(format!("{}/sso/jwks/{}", config.api_base, config.client_id))
        .send()
        .await
        .map_err(|_| AppError::Internal("WorkOS signing keys unavailable"))?;
    if !response.status().is_success() {
        return Err(AppError::Internal("WorkOS signing keys unavailable"));
    }
    let jwks: JwkSet = response
        .json()
        .await
        .map_err(|_| AppError::Internal("WorkOS signing keys"))?;
    let Some(claims) = verify_claims(config, token, &jwks) else {
        return Ok(None);
    };
    let checked = if let Some(blueprint) = &claims.agent_blueprint_id {
        let session = api(
            config,
            reqwest::Method::GET,
            &format!(
                "/agents/sessions/{}",
                claims.sid.as_deref().unwrap_or_default()
            ),
            None,
        )
        .await?;
        let instance = api(
            config,
            reqwest::Method::GET,
            &format!("/agents/instances/{}", claims.sub),
            None,
        )
        .await?;
        session["status"] == "active"
            && session["agent_instance_id"] == claims.sub
            && instance["id"] == claims.sub
            && instance["organization_id"] == claims.org_id
            && instance["agent_blueprint_id"] == *blueprint
            && instance["type"] == "autonomous"
    } else {
        if !config.agent_registration_enabled {
            return Ok(None);
        }
        let validated = api(
            config,
            reqwest::Method::POST,
            "/agents/credentials/validate",
            Some(json!({
                "type":"access_token", "credential":token, "audience":config.client_id
            })),
        )
        .await?;
        let registration = api(
            config,
            reqwest::Method::GET,
            &format!("/agents/registrations/{}", claims.sub),
            None,
        )
        .await?;
        validated["valid"] == true
            && validated["registration_id"] == claims.sub
            && registration["status"] == "verified"
            && registration["organization_id"] == claims.org_id
            && registration["agent_identity"]["userland_user_id"]
                == claims
                    .act
                    .as_ref()
                    .and_then(|a| a.get("sub"))
                    .cloned()
                    .unwrap_or(Value::Null)
    };
    Ok(checked.then_some(claims))
}

#[derive(FromRow)]
struct Grant {
    credential_id: Uuid,
    actor_id: Uuid,
    project_id: Uuid,
    connection_id: Uuid,
    client_name: String,
    scopes: Vec<String>,
}
async fn grant(
    state: &AppState,
    config: &WorkOsConfig,
    claims: &Claims,
) -> Result<Option<McpPrincipal>, AppError> {
    let subject = claims.agent_blueprint_id.as_deref().unwrap_or(&claims.sub);
    let kind = if claims.agent_blueprint_id.is_some() {
        "blueprint"
    } else {
        "registration"
    };
    let row = sqlx::query_as::<_, Grant>(
        r#"
        SELECT c.id AS credential_id, c.actor_id, c.project_id,
               connection.id AS connection_id, c.client_name, c.scopes
        FROM workos_agent_bindings b
        JOIN workos_organizations o USING (client_id, organization_id)
        JOIN mcp_client_credentials c ON c.id = b.credential_id
        JOIN external_agent_connections connection ON connection.id = b.connection_id
        JOIN projects p ON p.id = connection.project_id AND p.organization_id = b.organization_id
        WHERE b.client_id = $1 AND b.subject_id = $2 AND b.kind = $3 AND b.revoked_at IS NULL
          AND o.workos_organization_id = $4
          AND c.external_agent_connection_id = connection.id
          AND c.actor_id = connection.actor_id AND c.project_id = connection.project_id
          AND c.status = 'active' AND c.revoked_at IS NULL AND c.expires_at > now()
          AND c.scopes = connection.scopes
          AND agent_connection_can_access(connection.id, connection.project_id)
    "#,
    )
    .bind(&config.client_id)
    .bind(subject)
    .bind(kind)
    .bind(&claims.org_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(row) = &row {
        // The agent's last check-in, throttled as for OAuth-connected agents.
        sqlx::query("UPDATE external_agent_connections SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')")
            .bind(row.connection_id)
            .execute(&state.pool)
            .await?;
    }
    Ok(row
        .map(|row| {
            let granted: Vec<&str> = if kind == "blueprint" {
                claims.permissions.iter().map(String::as_str).collect()
            } else {
                claims.scope.split_ascii_whitespace().collect()
            };
            McpPrincipal {
                credential_id: row.credential_id,
                mcp_credential_id: Some(row.credential_id),
                oauth_grant_id: None,
                actor_id: row.actor_id,
                actor_kind: ActorKind::ExternalAgent,
                project_id: row.project_id,
                external_agent_connection_id: Some(row.connection_id),
                client_name: row.client_name,
                scopes: row
                    .scopes
                    .into_iter()
                    .filter(|scope| {
                        // The token audience is this server, so the bare local
                        // scope names are accepted alongside `workspace:*`.
                        granted.contains(&format!("workspace:{scope}").as_str())
                            || granted.contains(&scope.as_str())
                    })
                    .collect(),
                authentication_class: "workos_agent",
                account_wide: false,
                project_explicit: false,
            }
        })
        .filter(|principal| principal.can_read()))
}
pub(crate) async fn resolve_token(
    state: &AppState,
    token: &str,
) -> Result<Option<McpPrincipal>, AppError> {
    let Some(config) = &state.config.workos else {
        return Ok(None);
    };
    if config.authkit_domain.is_none() {
        return Ok(None);
    }
    let claims = match verified_token(config, token).await {
        Ok(Some(claims)) => claims,
        Ok(None) | Err(AppError::InvalidCredentials) => return Ok(None),
        Err(error) => return Err(error),
    };
    grant(state, config, &claims).await
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct BindingView {
    id: Uuid,
    kind: String,
    subject_id: String,
    revoked_at: Option<String>,
}
async fn list(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project, connection)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    let mut tx = state.pool.begin().await?;
    crate::external_agents::authorize_identity_binding(&mut tx, &principal, project, connection)
        .await?;
    let bindings = sqlx::query_as::<_, BindingView>("SELECT id,kind,subject_id,revoked_at::text FROM workos_agent_bindings WHERE connection_id=$1 ORDER BY created_at")
        .bind(connection).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({
        "enabled": state.config.workos.as_ref().is_some_and(|c| c.authkit_domain.is_some()),
        "registrationEnabled": state.config.workos.as_ref().is_some_and(|c| c.authkit_domain.is_some() && c.agent_registration_enabled),
        "bindings": bindings,
    })))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Attach {
    registration_id: Option<String>,
}
async fn attach(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project, connection)): ApiPath<(Uuid, Uuid)>,
    ApiJson(input): ApiJson<Attach>,
) -> Result<Json<BindingView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let config = config(&state)?;
    domain(config)?;
    let mut tx = state.pool.begin().await?;
    let org = crate::external_agents::authorize_identity_binding(
        &mut tx, &principal, project, connection,
    )
    .await?;
    // Hold the local grant locks across provider operations so removal cannot race attachment.
    let (credential, scopes, name): (Uuid, Vec<String>, String) = sqlx::query_as(
        "SELECT id,scopes,client_name FROM mcp_client_credentials WHERE external_agent_connection_id=$1 AND status='active' AND revoked_at IS NULL AND expires_at>now() ORDER BY created_at DESC LIMIT 1 FOR SHARE")
        .bind(connection).fetch_optional(&mut *tx).await?.ok_or(AppError::InvalidCredentials)?;
    let (kind, subject) = if let Some(registration_id) = input.registration_id {
        if !config.agent_registration_enabled || !valid_id(&registration_id, "agent_reg_") {
            return Err(AppError::Validation("agent registration"));
        }
        let registration = api(
            config,
            reqwest::Method::GET,
            &format!("/agents/registrations/{registration_id}"),
            None,
        )
        .await?;
        let user: Option<String> = sqlx::query_scalar(
            "SELECT subject_id FROM workos_identities WHERE client_id=$1 AND user_id=$2",
        )
        .bind(&config.client_id)
        .bind(principal.user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if registration["status"] != "verified"
            || user.as_deref().is_none()
            || registration["agent_identity"]["userland_user_id"].as_str() != user.as_deref()
        {
            return Err(AppError::Forbidden);
        }
        let provider_org = registration["organization_id"]
            .as_str()
            .filter(|v| valid_id(v, "org_"))
            .ok_or(AppError::InvalidCredentials)?;
        bind_org(&mut tx, config, org, provider_org).await?;
        ("registration", registration_id)
    } else {
        let provider_org = ensure_org(&mut tx, config, org, principal.user_id).await?;
        let prior: Option<String> = sqlx::query_scalar("SELECT subject_id FROM workos_agent_bindings WHERE client_id=$1 AND connection_id=$2 AND kind='blueprint' ORDER BY created_at DESC LIMIT 1")
            .bind(&config.client_id).bind(connection).fetch_optional(&mut *tx).await?;
        let permissions: Vec<String> = scopes.iter().map(|s| format!("workspace:{s}")).collect();
        let body = json!({"name":format!("{name} {connection}"), "permissions":permissions,
            "invocable_by":{"role_slugs":[],"organization_ids":[provider_org]}, "invocable_as":{"instance_types":["autonomous"]},
            "session_settings":{"max_age_seconds":86400,"access_token_ttl_seconds":300,"refresh_token_ttl_seconds":3600}});
        let blueprint = if let Some(id) = prior {
            api(
                config,
                reqwest::Method::PATCH,
                &format!("/agents/blueprints/{id}"),
                Some(body),
            )
            .await?
        } else {
            api(
                config,
                reqwest::Method::POST,
                "/agents/blueprints",
                Some(body),
            )
            .await?
        };
        let id = blueprint["id"]
            .as_str()
            .filter(|id| valid_id(id, "agent_blueprint_"))
            .ok_or(AppError::Internal("WorkOS blueprint response"))?;
        ("blueprint", id.to_owned())
    };
    let id = Uuid::now_v7();
    sqlx::query("UPDATE workos_agent_bindings SET revoked_at=now() WHERE client_id=$1 AND connection_id=$2 AND kind=$3 AND revoked_at IS NULL")
        .bind(&config.client_id).bind(connection).bind(kind).execute(&mut *tx).await?;
    // A revoked subject may be rebound only to its original connection.
    let row = sqlx::query_as::<_, BindingView>(r#"
        INSERT INTO workos_agent_bindings(id,client_id,organization_id,connection_id,credential_id,kind,subject_id,created_by_actor_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8)
        ON CONFLICT(client_id,subject_id) DO UPDATE SET credential_id=excluded.credential_id, revoked_at=NULL
        WHERE workos_agent_bindings.connection_id=excluded.connection_id AND workos_agent_bindings.organization_id=excluded.organization_id
        RETURNING id,kind,subject_id,revoked_at::text
    "#).bind(id).bind(&config.client_id).bind(org).bind(connection).bind(credential).bind(kind).bind(&subject).bind(principal.actor_id)
        .fetch_optional(&mut *tx).await?.ok_or(AppError::Conflict("agent identity", "already attached"))?;
    audit(
        &mut tx,
        &principal,
        project,
        connection,
        "workos_agent.attached",
        &subject,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}
async fn bind_org(
    tx: &mut Transaction<'_, Postgres>,
    config: &WorkOsConfig,
    org: Uuid,
    provider_org: &str,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO workos_organizations(client_id,organization_id,workos_organization_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(&config.client_id).bind(org).bind(provider_org).execute(&mut **tx).await?;
    let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workos_organizations WHERE client_id=$1 AND organization_id=$2 AND workos_organization_id=$3)")
        .bind(&config.client_id).bind(org).bind(provider_org).fetch_one(&mut **tx).await?;
    if !matches {
        return Err(AppError::Conflict(
            "WorkOS organization",
            "identity belongs to a different workspace",
        ));
    }
    Ok(())
}
async fn ensure_org(
    tx: &mut Transaction<'_, Postgres>,
    config: &WorkOsConfig,
    org: Uuid,
    user: Uuid,
) -> Result<String, AppError> {
    // Serialize provisioning for all projects in this organization.
    let name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(org)
        .fetch_one(&mut **tx)
        .await?;
    let existing: Option<String> = sqlx::query_scalar("SELECT workos_organization_id FROM workos_organizations WHERE client_id=$1 AND organization_id=$2")
        .bind(&config.client_id).bind(org).fetch_optional(&mut **tx).await?;
    let id = if let Some(id) = existing {
        id
    } else {
        let response = client()?
            .post(format!("{}/organizations", config.api_base))
            .bearer_auth(config.api_key.expose())
            .header("Idempotency-Key", format!("workspace-org-{org}"))
            .json(&json!({"name":name,"external_id":org.to_string()}))
            .send()
            .await
            .map_err(|_| AppError::Internal("WorkOS organization creation"))?;
        if !response.status().is_success() {
            return Err(AppError::Internal("WorkOS organization creation"));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| AppError::Internal("WorkOS organization response"))?;
        let id = value["id"]
            .as_str()
            .filter(|id| valid_id(id, "org_"))
            .ok_or(AppError::Internal("WorkOS organization response"))?
            .to_owned();
        bind_org(tx, config, org, &id).await?;
        id
    };
    if let Some(subject) = sqlx::query_scalar::<_, String>(
        "SELECT subject_id FROM workos_identities WHERE client_id=$1 AND user_id=$2",
    )
    .bind(&config.client_id)
    .bind(user)
    .fetch_optional(&mut **tx)
    .await?
    {
        // Existing memberships return conflict; inspect rather than suppressing errors.
        let memberships = api(
            config,
            reqwest::Method::GET,
            &format!(
                "/user_management/organization_memberships?user_id={subject}&organization_id={id}"
            ),
            None,
        )
        .await?;
        if memberships["data"]
            .as_array()
            .is_some_and(|rows| rows.is_empty())
        {
            api(
                config,
                reqwest::Method::POST,
                "/user_management/organization_memberships",
                Some(json!({"user_id":subject,"organization_id":id})),
            )
            .await?;
        }
    }
    Ok(id)
}
async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project, connection, binding)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut tx = state.pool.begin().await?;
    crate::external_agents::authorize_identity_binding(&mut tx, &principal, project, connection)
        .await?;
    let subject: String = sqlx::query_scalar("UPDATE workos_agent_bindings SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1 AND connection_id=$2 RETURNING subject_id")
        .bind(binding).bind(connection).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound("agent identity"))?;
    audit(
        &mut tx,
        &principal,
        project,
        connection,
        "workos_agent.revoked",
        &subject,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"revoked":true})))
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project: Uuid,
    connection: Uuid,
    action: &str,
    subject: &str,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO audit_events(id,project_id,actor_id,authentication_class,action,target_kind,target_id,channel,outcome,metadata) VALUES($1,$2,$3,'browser_session',$4,'external_agent_connection',$5,'browser','succeeded',$6)")
        .bind(Uuid::now_v7()).bind(project).bind(principal.actor_id).bind(action).bind(connection).bind(json!({"workos_subject_id":subject})).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Mint {
    blueprint_id: Option<String>,
    refresh_token: Option<String>,
}
async fn mint(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(input): ApiJson<Mint>,
) -> Result<Response, AppError> {
    let config = config(&state)?;
    domain(config)?;
    state.rate_limits.check_mcp_auth(
        input
            .refresh_token
            .as_deref()
            .or_else(|| exact_bearer_token(&headers))
            .unwrap_or("invalid"),
    )?;
    let (blueprint, body) = if let Some(refresh) = input.refresh_token {
        let blueprint = input
            .blueprint_id
            .filter(|id| valid_id(id, "agent_blueprint_"))
            .ok_or(AppError::InvalidCredentials)?;
        if refresh.is_empty() || refresh.len() > 4096 {
            return Err(AppError::InvalidCredentials);
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workos_agent_bindings WHERE client_id=$1 AND subject_id=$2 AND revoked_at IS NULL)")
            .bind(&config.client_id).bind(&blueprint).fetch_one(&state.pool).await?;
        if !active {
            return Err(AppError::InvalidCredentials);
        }
        (blueprint, json!({"type":"refresh","refresh_token":refresh}))
    } else {
        // Only possession of this agent's existing credential can bootstrap its
        // WorkOS session. A human cookie or another agent cannot choose authorship.
        if !exact_bearer_token(&headers).is_some_and(|t| t.starts_with("aw_ext_")) {
            return Err(AppError::InvalidCredentials);
        }
        let principal = resolve_bearer_principal(&state, &headers)
            .await?
            .ok_or(AppError::InvalidCredentials)?;
        let connection = principal
            .external_agent_connection_id
            .ok_or(AppError::Forbidden)?;
        let (blueprint,org):(String,String)=sqlx::query_as("SELECT b.subject_id,o.workos_organization_id FROM workos_agent_bindings b JOIN workos_organizations o USING(client_id,organization_id) WHERE b.client_id=$1 AND b.connection_id=$2 AND b.credential_id=$3 AND b.kind='blueprint' AND b.revoked_at IS NULL")
            .bind(&config.client_id).bind(connection).bind(principal.credential_id).fetch_optional(&state.pool).await?.ok_or(AppError::InvalidCredentials)?;
        (
            blueprint,
            json!({"type":"autonomous","organization_id":org}),
        )
    };
    let tokens = api(
        config,
        reqwest::Method::POST,
        &format!("/agents/blueprints/{blueprint}/tokens"),
        Some(body),
    )
    .await?;
    let token = tokens["access_token"]
        .as_str()
        .ok_or(AppError::Internal("WorkOS token response"))?;
    let claims = verified_token(config, token)
        .await?
        .ok_or(AppError::InvalidCredentials)?;
    if claims.agent_blueprint_id.as_deref() != Some(&blueprint)
        || grant(&state, config, &claims).await?.is_none()
    {
        return Err(AppError::InvalidCredentials);
    }
    // Forward only documented fields and prevent caches from retaining credentials.
    Ok(([(header::CACHE_CONTROL,"no-store"),(header::PRAGMA,"no-cache")],Json(json!({
        "access_token":tokens["access_token"],"refresh_token":tokens["refresh_token"],
        "token_type":"Bearer","expires_in":tokens["expires_in"],"blueprint_id":blueprint,
        "agent_instance_id":tokens["agent_instance_id"],"agent_instance_session_id":tokens["agent_instance_session_id"]
    }))).into_response())
}
async fn auth_document(State(state): State<AppState>) -> Result<Response, AppError> {
    let origin = public_origin(&state);
    let mut text = format!(
        "# Agent Workspace authentication\n\nMCP endpoint: {origin}/api/v1/mcp\n\nAgents have their own accounts and author messages as themselves. An administrator adds an agent in Agents, then adds that same account to each project it may access. Tokens never create project membership.\n\n## Existing agent credentials\n\nUse the one-time agent token from Agents as `Authorization: Bearer <token>`. Keep credentials in your runtime's secret store, never in messages or source files.\n\n## WorkOS agent tokens\n\nAn administrator enables WorkOS tokens for your existing agent in Agents. Bootstrap by POSTing an empty JSON object to `{origin}/api/v1/agents/token` with your existing agent bearer credential. The response contains `access_token`, `refresh_token`, `expires_in`, and `blueprint_id`. Use the access token as your MCP bearer token. Before expiry, POST `{{\"blueprintId\":\"<blueprint_id>\",\"refreshToken\":\"<refresh_token>\"}}` to the same endpoint. Refresh tokens are single-use; replace them atomically after each refresh. Sessions last at most 24 hours; bootstrap again after expiry. Disabling the agent, revoking its WorkOS binding, or rotating its bootstrap credential stops the previous binding. After rotation the administrator must enable WorkOS tokens again.\n\n## External agent registration\n\n"
    );
    if let Some(config) = &state.config.workos
        && config.agent_registration_enabled
    {
        let domain = domain(config)?;
        text.push_str(&format!("Follow the WorkOS registration instructions below using `service_auth`. The user must link their WorkOS account in Workspace Account settings. After claiming, give the administrator your non-secret registration ID (`agent_reg_...`). They attach it to your named agent in Agents. Your token becomes usable only after that attachment. Use the client ID audience `{}` when exchanging assertions; do not request a different resource audience.\n\n",config.client_id));
        let response = client()?
            .get(format!("{domain}/agent/auth.md"))
            .send()
            .await
            .map_err(|_| AppError::Internal("WorkOS auth.md unavailable"))?;
        if !response.status().is_success() {
            return Err(AppError::Internal("WorkOS auth.md unavailable"));
        }
        let body = response
            .text()
            .await
            .map_err(|_| AppError::Internal("WorkOS auth.md unavailable"))?;
        if body.len() > 128 * 1024 {
            return Err(AppError::Internal("WorkOS auth.md too large"));
        }
        text.push_str(&body);
    } else {
        text.push_str("WorkOS external registration is not enabled on this host. Ask an administrator to create an agent account and supply its own credential.\n");
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        text,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{AppConfig, SecretString},
        provider::SupervisorRegistry,
        rate_limit::RateLimiters,
        realtime::RealtimeHub,
    };
    use jsonwebtoken::{EncodingKey, Header, encode};
    use sqlx::{Executor, postgres::PgPoolOptions};
    use std::{collections::BTreeMap, sync::Arc};
    fn configuration() -> WorkOsConfig {
        WorkOsConfig {
            client_id: "client_test".into(),
            api_key: SecretString::new("sk_test"),
            allow_signup: false,
            authkit_domain: Some("https://test.authkit.app".into()),
            google_enabled: false,
            agent_registration_enabled: true,
            api_base: crate::config::WORKOS_API_BASE.into(),
        }
    }
    fn claims() -> Value {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("WorkOS agent test operation")
            .as_secs();
        json!({"iss":"https://test.authkit.app","aud":"client_test","sub":"agent_test","org_id":"org_test",
            "exp":now+300,"iat":now,"jti":"test","sub_profile":"ai_agent","sid":"agent_session_test",
            "agent_blueprint_id":"agent_blueprint_test","permissions":["workspace:read","workspace:write"]})
    }
    fn signed(value: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-only".into());
        header.typ = Some("at+jwt".into());
        encode(
            &header,
            value,
            &EncodingKey::from_rsa_pem(include_bytes!(
                "../../../tests/fixtures/workos-agent/test-only-rsa.txt"
            ))
            .expect("WorkOS agent test operation"),
        )
        .expect("WorkOS agent test operation")
    }
    #[test]
    fn agent_jwt_verification_rejects_wrong_identity_signature_audience_and_expiry() {
        let config = configuration();
        let keys: JwkSet = serde_json::from_str(include_str!(
            "../../../tests/fixtures/workos-agent/jwks.json"
        ))
        .expect("WorkOS agent test operation");
        let good = claims();
        assert!(verify_claims(&config, &signed(&good), &keys).is_some());
        for (field, value) in [
            ("iss", json!("https://other.authkit.app")),
            ("aud", json!("client_other")),
            ("exp", json!(1)),
            ("sub_profile", json!("user")),
            ("act", json!({"sub":"user_test"})),
            ("sid", json!("../../sessions")),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(
                verify_claims(&config, &signed(&bad), &keys).is_none(),
                "accepted {field}"
            );
        }
        let forged = format!(
            "{}.{}.invalid",
            signed(&good)
                .split('.')
                .next()
                .expect("WorkOS agent test operation"),
            signed(&good)
                .split('.')
                .nth(1)
                .expect("WorkOS agent test operation")
        );
        assert!(verify_claims(&config, &forged, &keys).is_none());
        let mut human = good.clone();
        human
            .as_object_mut()
            .expect("WorkOS agent test operation")
            .remove("agent_blueprint_id");
        human["sub"] = json!("user_test");
        assert!(verify_claims(&config, &signed(&human), &keys).is_none());
        let mut registration = good;
        registration
            .as_object_mut()
            .expect("WorkOS agent test operation")
            .remove("agent_blueprint_id");
        registration["sub"] = json!("agent_reg_test");
        assert!(verify_claims(&config, &signed(&registration), &keys).is_none());
        registration["act"] = json!({"sub":"user_test"});
        assert!(verify_claims(&config, &signed(&registration), &keys).is_some());
    }
    #[tokio::test]
    async fn workos_agent_grants_preserve_identity_and_fail_closed_on_local_revocation() {
        let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
            return;
        };
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("WorkOS agent test operation");
        let schema = format!("workos_agents_{}", Uuid::now_v7().simple());
        admin
            .execute(format!("CREATE SCHEMA {schema}").as_str())
            .await
            .expect("WorkOS agent test operation");
        let search_path = format!("SET search_path TO {schema},public");
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |c, _| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    c.execute(search_path.as_str()).await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("WorkOS agent test operation");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("WorkOS agent test operation");
        let root = tempfile::tempdir().expect("WorkOS agent test operation");
        let app_config = AppConfig::from_values(&BTreeMap::from([
            ("AGENT_WORKSPACE_DATABASE_URL".into(), database_url),
            (
                "AGENT_WORKSPACE_DATA_ROOT".into(),
                root.path().join("data").to_string_lossy().into_owned(),
            ),
            (
                "AGENT_WORKSPACE_CODEX_HOME_ROOT".into(),
                root.path()
                    .join("data/codex/profiles")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "AGENT_WORKSPACE_CODEX_BINARY".into(),
                "/usr/bin/false".into(),
            ),
            (
                "AGENT_WORKSPACE_WEB_ROOT".into(),
                root.path().join("web").to_string_lossy().into_owned(),
            ),
        ]))
        .expect("WorkOS agent test operation");
        let seed = crate::seed::dev_seed(&pool, &app_config)
            .await
            .expect("WorkOS agent test operation");
        let agent = crate::external_agents::provision(
            &pool,
            seed.project_id,
            seed.human_actor_id,
            "Test Agent",
            "test_agent",
        )
        .await
        .expect("WorkOS agent test operation");
        let credential: Uuid = sqlx::query_scalar(
            "SELECT id FROM mcp_client_credentials WHERE external_agent_connection_id=$1",
        )
        .bind(agent.connection_id)
        .fetch_one(&pool)
        .await
        .expect("WorkOS agent test operation");
        let config = configuration();
        let mut tx = pool.begin().await.expect("WorkOS agent test operation");
        bind_org(&mut tx, &config, seed.organization_id, "org_test")
            .await
            .expect("WorkOS agent test operation");
        tx.commit().await.expect("WorkOS agent test operation");
        sqlx::query("INSERT INTO workos_agent_bindings(id,client_id,organization_id,connection_id,credential_id,kind,subject_id,created_by_actor_id) VALUES($1,$2,$3,$4,$5,'blueprint','agent_blueprint_test',$6)")
            .bind(Uuid::now_v7()).bind(&config.client_id).bind(seed.organization_id).bind(agent.connection_id).bind(credential).bind(seed.human_actor_id).execute(&pool).await.expect("WorkOS agent test operation");
        let supervisors =
            SupervisorRegistry::new(&app_config).expect("WorkOS agent test operation");
        let state = AppState {
            config: Arc::new(app_config),
            pool: pool.clone(),
            supervisors,
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(16),
        };
        let mut token: Claims =
            serde_json::from_value(claims()).expect("WorkOS agent test operation");
        let principal = grant(&state, &config, &token)
            .await
            .expect("WorkOS agent test operation")
            .expect("WorkOS agent test operation");
        assert_eq!(principal.actor_id, agent.actor_id);
        assert!(principal.can_write());
        assert!(!principal.has_scope("events"));
        token.permissions = vec!["workspace:read".into()];
        assert!(
            !grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .expect("WorkOS agent test operation")
                .can_write()
        );
        token.permissions.clear();
        assert!(
            grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .is_none()
        );
        token.permissions = vec!["workspace:read".into()];
        token.org_id = "org_wrong".into();
        assert!(
            grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .is_none()
        );
        token.org_id = "org_test".into();
        sqlx::query("UPDATE workos_agent_bindings SET revoked_at=now()")
            .execute(&pool)
            .await
            .expect("WorkOS agent test operation");
        assert!(
            grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .is_none()
        );
        sqlx::query("UPDATE workos_agent_bindings SET revoked_at=NULL")
            .execute(&pool)
            .await
            .expect("WorkOS agent test operation");
        sqlx::query("UPDATE project_memberships SET enabled=false WHERE actor_id=$1")
            .bind(agent.actor_id)
            .execute(&pool)
            .await
            .expect("WorkOS agent test operation");
        assert!(
            grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .is_none()
        );
        sqlx::query("UPDATE project_memberships SET enabled=true WHERE actor_id=$1")
            .bind(agent.actor_id)
            .execute(&pool)
            .await
            .expect("WorkOS agent test operation");
        sqlx::query(
            "UPDATE mcp_client_credentials SET status='revoked',revoked_at=now() WHERE id=$1",
        )
        .bind(credential)
        .execute(&pool)
        .await
        .expect("WorkOS agent test operation");
        assert!(
            grant(&state, &config, &token)
                .await
                .expect("WorkOS agent test operation")
                .is_none()
        );
        pool.close().await;
        admin
            .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
            .await
            .expect("WorkOS agent test operation");
        admin.close().await;
    }
}
