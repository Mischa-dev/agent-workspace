//! HTTP routes for AI planning: the AI and models settings, the review
//! queue, readiness and the ranking, the dependency graph, plan imports and
//! summaries. MCP calls the same functions (see `mcp/ai_tools.rs`).

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    detect::{self, Mode},
    gateway::{self, AiError, Scope, Tier},
    plans::{self, CreateRefusal, Draft, Source},
    prompts,
    ready::{self, GraphFilter, RankFilter, RankScope},
    suggestions::{self, Filter, Kind, NewSuggestion, Provenance, Refusal, SourceRef},
    summaries::{self, SummaryRefusal},
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    policy::Action,
    settings::{self, SettingsContext},
    work_item_commands::{AuditAttribution, CommandActor, ProjectMembership, authorize_mutation},
    work_item_planning::Reader,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/ai",
            get(workspace_status),
        )
        .route(
            "/api/v1/organizations/{organization_id}/ai/test",
            post(test_workspace),
        )
        .route("/api/v1/projects/{project_id}/ai", get(project_status))
        .route("/api/v1/projects/{project_id}/ai/test", post(test_project))
        .route("/api/v1/projects/{project_id}/ai/analyze", post(analyze))
        .route(
            "/api/v1/projects/{project_id}/ai/suggestions",
            get(list_suggestions),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/accept",
            post(accept),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/suggestions/{suggestion_id}/dismiss",
            post(dismiss),
        )
        .route("/api/v1/projects/{project_id}/ai/ready", get(ranking))
        .route("/api/v1/projects/{project_id}/ai/graph", get(graph))
        .route(
            "/api/v1/projects/{project_id}/ai/plans",
            get(list_plans).post(create_plan),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/plans/{import_id}",
            get(plan).put(save_plan),
        )
        .route(
            "/api/v1/projects/{project_id}/ai/plans/{import_id}/commit",
            post(commit_plan),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/readiness",
            get(readiness),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/triage",
            post(triage),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/criteria",
            post(criteria),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/ai/summary",
            post(task_summary),
        )
        .route(
            "/api/v1/conversations/{id}/ai/summary",
            post(thread_summary),
        )
}

// ---------------------------------------------------------------------------
// Access

/// A person's place in a project, checked for one action.
struct Member {
    organization_id: Uuid,
    actor: CommandActor,
    can_edit: bool,
}

impl Member {
    fn reader(&self) -> Reader {
        Reader {
            actor_id: self.actor.actor_id,
            connection_id: None,
        }
    }

    fn scope(&self) -> Scope {
        Scope {
            organization_id: self.organization_id,
            project_id: Some(self.actor.project_id),
            actor_id: Some(self.actor.actor_id),
        }
    }
}

/// Checks `principal` may do `action` in a standard project, else "not
/// found" (not a member, or the hidden Messages space) or "forbidden".
async fn member(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<Member, AppError> {
    let Some((organization_id, archived_at, role, enabled)) =
        sqlx::query_as::<_, (Uuid, Option<String>, String, bool)>(
            r#"
            SELECT project.organization_id, project.archived_at::text, membership.role,
                   membership.enabled
            FROM projects project
            JOIN project_memberships membership
              ON membership.project_id = project.id AND membership.actor_id = $2
            JOIN organization_memberships organization_membership
              ON organization_membership.organization_id = project.organization_id
             AND organization_membership.actor_id = $2 AND organization_membership.enabled
            JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
            WHERE project.id = $1 AND project.kind = 'standard'
            "#,
        )
        .bind(project_id)
        .bind(principal.actor_id)
        .fetch_optional(&state.pool)
        .await?
    else {
        return Err(AppError::NotFound("project"));
    };
    let actor = CommandActor {
        actor_id: principal.actor_id,
        actor_kind: crate::conversations::parse_actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id: None,
        },
    };
    let membership = ProjectMembership {
        project_id,
        project_archived_at: archived_at,
        role,
        enabled,
    };
    if !authorize_mutation(&actor, &membership, Action::WorkItemView, None)?.is_allowed() {
        return Err(AppError::NotFound("project"));
    }
    let can_edit = membership.project_archived_at.is_none()
        && authorize_mutation(&actor, &membership, Action::WorkItemEdit, None)?.is_allowed();
    if action != Action::WorkItemView {
        if membership.project_archived_at.is_some() {
            return Err(AppError::ProjectArchived);
        }
        if !authorize_mutation(&actor, &membership, action, None)?.is_allowed() {
            return Err(AppError::Forbidden);
        }
    }
    Ok(Member {
        organization_id,
        actor,
        can_edit,
    })
}

/// A write: CSRF and rate limit, then [`member`].
async fn writer(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<Member, AppError> {
    require_authenticated_write(state, headers, principal)?;
    member(state, principal, project_id, action).await
}

fn error_response(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message.into(), "details": null}})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Settings and status

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageRow {
    model: String,
    provider: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_micros: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DayRow {
    day: String,
    calls: i64,
    cost_micros: i64,
}

async fn workspace_status(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<Value>, AppError> {
    settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    let providers = gateway::offered_providers(state.config.environment);
    // Which providers have a workspace key; never the key.
    let mut keys = BTreeMap::new();
    for manifest in &providers {
        let found = crate::secrets::find(
            &state.pool,
            crate::secrets::SecretOwner::Workspace { organization_id },
            &super::providers::secret_name(manifest.id),
        )
        .await?;
        keys.insert(manifest.id, found.is_some());
    }
    let configured = gateway::resolve(
        &state,
        Scope {
            organization_id,
            project_id: None,
            actor_id: None,
        },
        Tier::Quick,
    )
    .await;
    let budget = gateway::budget_usd(&state.pool, organization_id).await?;
    let spent = gateway::spent_this_month_micros(&state.pool, organization_id).await?;
    let by_model = sqlx::query_as::<_, (String, String, i64, i64, i64, i64)>(
        r#"
        SELECT model, provider, count(*), COALESCE(sum(input_tokens), 0)::bigint,
               COALESCE(sum(output_tokens), 0)::bigint, COALESCE(sum(cost_micros), 0)::bigint
        FROM ai_usage
        WHERE organization_id = $1 AND created_at >= date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
        GROUP BY model, provider
        ORDER BY 6 DESC
        LIMIT 20
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(
        |(model, provider, calls, input_tokens, output_tokens, cost_micros)| UsageRow {
            model,
            provider,
            calls,
            input_tokens,
            output_tokens,
            cost_micros,
        },
    )
    .collect::<Vec<_>>();
    let by_day = sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT to_char(date_trunc('day', created_at AT TIME ZONE 'UTC'), 'YYYY-MM-DD'), count(*),
               COALESCE(sum(cost_micros), 0)::bigint
        FROM ai_usage
        WHERE organization_id = $1 AND created_at > now() - interval '30 days'
        GROUP BY 1 ORDER BY 1
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(day, calls, cost_micros)| DayRow {
        day,
        calls,
        cost_micros,
    })
    .collect::<Vec<_>>();
    Ok(Json(json!({
        "providers": providers,
        "workspaceKeys": keys,
        "configured": configured.as_ref().is_ok(),
        "problem": configured.as_ref().err().map(AiError::message),
        "provider": configured.as_ref().ok().map(|resolved| resolved.manifest.id),
        "quickModel": configured.as_ref().ok().map(|resolved| resolved.model.clone()),
        "budgetUsd": budget,
        "spentMicros": spent,
        "usageByModel": by_model,
        "usageByDay": by_day,
    })))
}

/// The provider, models and key a project's AI calls would use.
async fn project_status(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<Value>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    let scope = Scope {
        actor_id: None,
        ..member.scope()
    };
    let quick = gateway::resolve(&state, scope, Tier::Quick).await;
    let deep = gateway::resolve(&state, scope, Tier::Deep).await;
    let mut connection = state.pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id: member.organization_id,
            project_id: Some(project_id),
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    let runs = detect::recent_runs(&state.pool, project_id, principal.actor_id, 10).await?;
    let pending = suggestions::list(
        &mut connection,
        member.reader(),
        project_id,
        &Filter {
            limit: 1,
            ..Filter::default()
        },
    )
    .await?
    .pending_count;
    Ok(Json(json!({
        "organizationId": member.organization_id,
        "configured": quick.is_ok(),
        "problem": quick.as_ref().err().map(AiError::message),
        "provider": quick.as_ref().ok().map(|resolved| json!({
            "id": resolved.manifest.id,
            "displayName": resolved.manifest.display_name,
        })),
        "quickModel": quick.as_ref().ok().map(|resolved| resolved.model.clone()),
        "deepModel": deep.as_ref().ok().map(|resolved| resolved.model.clone()),
        "keySource": quick.as_ref().ok().and_then(|resolved| resolved.key.map(|(_, source)| source)),
        "backgroundDetection": effective.bool("ai.background_detection").unwrap_or(false),
        "autoApply": effective.bool("ai.auto_apply_suggestions").unwrap_or(false),
        "autoApplyMinConfidence": effective.value::<i64>("ai.auto_apply_min_confidence").unwrap_or(90),
        "pendingSuggestions": pending,
        "runs": runs,
        "canEdit": member.can_edit,
    })))
}

/// Pings the configured model with a tiny prompt, so a person knows the key
/// and model work. Costs a few tokens.
async fn ping(state: &AppState, scope: Scope) -> Response {
    let started = std::time::Instant::now();
    match gateway::run::<Value>(state, scope, &prompts::PING, &json!({}), Tier::Quick).await {
        Ok(answer) => Json(json!({
            "ok": true,
            "provider": answer.provider,
            "model": answer.model,
            "latencyMs": started.elapsed().as_millis() as u64,
        }))
        .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn test_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    Ok(ping(
        &state,
        Scope {
            organization_id,
            project_id: None,
            actor_id: Some(principal.actor_id),
        },
    )
    .await)
}

async fn test_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemEdit,
    )
    .await?;
    Ok(ping(&state, member.scope()).await)
}

// ---------------------------------------------------------------------------
// Detection and the review queue

async fn analyze(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemEdit,
    )
    .await?;
    if let Err(error) = gateway::resolve(&state, member.scope(), Tier::Quick).await {
        return Ok(error.into_response());
    }
    let run_id = detect::request(
        &state.pool,
        member.organization_id,
        project_id,
        Mode::Analyze,
        Some(principal.actor_id),
        Vec::new(),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"runId": run_id}))).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SuggestionQuery {
    status: Option<String>,
    work_item_id: Option<Uuid>,
    /// Comma-separated kinds.
    kind: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

pub(crate) fn parse_kinds(value: Option<&str>) -> Result<Vec<Kind>, AppError> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(|kind| Kind::parse(kind).ok_or(AppError::Validation("kind")))
        .collect()
}

async fn list_suggestions(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SuggestionQuery>,
) -> Result<Json<Value>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    let mut connection = state.pool.acquire().await?;
    let page = suggestions::list(
        &mut connection,
        member.reader(),
        project_id,
        &Filter {
            status: query.status,
            work_item_id: query.work_item_id,
            kinds: parse_kinds(query.kind.as_deref())?,
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await?;
    let mut body = serde_json::to_value(page).map_err(|_| AppError::Internal("suggestions"))?;
    body["canEdit"] = json!(member.can_edit);
    Ok(Json(body))
}

fn decision_refusal(refusal: Refusal) -> Response {
    match refusal {
        Refusal::NotFound => refusal_response(StatusCode::NOT_FOUND, "not_found", &refusal),
        Refusal::AlreadyDecided(_) => {
            refusal_response(StatusCode::CONFLICT, "already_decided", &refusal)
        }
        Refusal::Forbidden => refusal_response(StatusCode::FORBIDDEN, "forbidden", &refusal),
        Refusal::Refused(_) => refusal_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "suggestion_refused",
            &refusal,
        ),
    }
}

fn refusal_response(status: StatusCode, code: &str, refusal: &Refusal) -> Response {
    error_response(status, code, refusal.message())
}

async fn decide(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    suggestion_id: Uuid,
    accept: bool,
) -> Result<Response, AppError> {
    let member = writer(state, headers, principal, project_id, Action::WorkItemEdit).await?;
    let mut actor = member.actor.clone();
    actor.audit = AuditAttribution::Browser {
        session_id: principal.session_id,
        request_id: headers
            .get(crate::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128)
            .map(str::to_owned),
    };
    let mut transaction = state.pool.begin().await?;
    let outcome = if accept {
        suggestions::accept(
            &mut transaction,
            &actor,
            member.reader(),
            suggestion_id,
            "accepted",
        )
        .await?
    } else {
        suggestions::dismiss(&mut transaction, &actor, member.reader(), suggestion_id).await?
    };
    if let Err(refusal) = outcome {
        transaction.rollback().await?;
        return Ok(decision_refusal(refusal));
    }
    transaction.commit().await?;
    let mut connection = state.pool.acquire().await?;
    let view =
        suggestions::get(&mut connection, member.reader(), project_id, suggestion_id).await?;
    Ok(Json(json!({"suggestion": view})).into_response())
}

async fn accept(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, suggestion_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    decide(
        &state,
        &headers,
        &principal,
        project_id,
        suggestion_id,
        true,
    )
    .await
}

async fn dismiss(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, suggestion_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    decide(
        &state,
        &headers,
        &principal,
        project_id,
        suggestion_id,
        false,
    )
    .await
}

async fn task_in_project(
    state: &AppState,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_items WHERE project_id = $1 AND id = $2 AND archived_at IS NULL)",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_one(&state.pool)
    .await?)
}

async fn triage(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemEdit,
    )
    .await?;
    if !task_in_project(&state, project_id, task_id).await? {
        return Err(AppError::NotFound("work_item"));
    }
    if let Err(error) = gateway::resolve(&state, member.scope(), Tier::Quick).await {
        return Ok(error.into_response());
    }
    let run_id = detect::request(
        &state.pool,
        member.organization_id,
        project_id,
        Mode::Triage,
        Some(principal.actor_id),
        vec![task_id],
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"runId": run_id}))).into_response())
}

#[derive(Deserialize)]
struct CriteriaAnswer {
    criteria: Vec<String>,
}

/// Drafts acceptance criteria for a task now, as a suggestion to accept.
pub(crate) async fn draft_criteria(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    task_id: Uuid,
) -> Result<Result<Uuid, Response>, AppError> {
    let Some((key, title, description, type_name)) =
        sqlx::query_as::<_, (String, String, String, String)>(
            r#"
            SELECT project.project_key || '-' || item.number, item.title,
                   left(item.description, 6000), kind.name
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_types kind ON kind.project_id = item.project_id AND kind.id = item.type_id
            WHERE item.project_id = $1 AND item.id = $2 AND item.archived_at IS NULL
            "#,
        )
        .bind(project_id)
        .bind(task_id)
        .fetch_optional(&state.pool)
        .await?
    else {
        return Err(AppError::NotFound("work_item"));
    };
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT text FROM work_item_acceptance_criteria WHERE project_id = $1 AND work_item_id = $2 ORDER BY position",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_all(&state.pool)
    .await?;
    let scope = Scope {
        organization_id,
        project_id: Some(project_id),
        actor_id: Some(actor_id),
    };
    let answer = match gateway::run::<CriteriaAnswer>(
        state,
        scope,
        &prompts::CRITERIA,
        &json!({
            "task": {"key": key, "title": title, "description": description, "type": type_name},
            "existing": existing,
        }),
        Tier::Quick,
    )
    .await
    {
        Ok(answer) => answer,
        Err(error) => return Ok(Err(error.into_response())),
    };
    // A new draft replaces an open one for the same task.
    sqlx::query(
        r#"
        UPDATE ai_suggestions SET status = 'stale', updated_at = now()
        WHERE project_id = $1 AND work_item_id = $2 AND kind = 'criteria' AND status = 'pending'
        "#,
    )
    .bind(project_id)
    .bind(task_id)
    .execute(&state.pool)
    .await?;
    let criteria = answer
        .value
        .criteria
        .into_iter()
        .map(|criterion| criterion.trim().chars().take(1000).collect::<String>())
        .filter(|criterion| !criterion.is_empty())
        .take(6)
        .collect::<Vec<_>>();
    let inserted = suggestions::insert(
        &state.pool,
        &NewSuggestion {
            organization_id,
            project_id,
            work_item_id: task_id,
            kind: Kind::Criteria,
            other: None,
            value: json!({"criteria": criteria}),
            confidence: 1.0,
            reason: "Drafted on request.".to_owned(),
            sources: vec![SourceRef::Task { id: task_id }],
            audience: None,
            origin: "request",
            provenance: Provenance {
                provider: answer.provider.to_owned(),
                model: answer.model,
                prompt_id: answer.prompt_id,
                prompt_version: answer.prompt_version,
            },
            requested_by: Some(actor_id),
        },
    )
    .await?;
    match inserted {
        Some(id) => Ok(Ok(id)),
        None => Ok(Err(error_response(
            StatusCode::CONFLICT,
            "suggestion_exists",
            "Drafted criteria for this task were dismissed before.",
        ))),
    }
}

async fn criteria(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemEdit,
    )
    .await?;
    let id = match draft_criteria(
        &state,
        member.organization_id,
        project_id,
        principal.actor_id,
        task_id,
    )
    .await?
    {
        Ok(id) => id,
        Err(response) => return Ok(response),
    };
    let mut connection = state.pool.acquire().await?;
    let view = suggestions::get(&mut connection, member.reader(), project_id, id).await?;
    Ok((StatusCode::CREATED, Json(json!({"suggestion": view}))).into_response())
}

// ---------------------------------------------------------------------------
// Readiness, ranking and the graph

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RankQuery {
    scope: Option<RankScope>,
    milestone_id: Option<Uuid>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn ranking(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<RankQuery>,
) -> Result<Json<ready::Ranking>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    Ok(Json(
        ready::rank(
            &state.pool,
            member.reader(),
            project_id,
            &RankFilter {
                scope: query.scope.unwrap_or_default(),
                milestone_id: query.milestone_id,
                limit: query.limit.unwrap_or(50),
                offset: query.offset.unwrap_or(0),
            },
        )
        .await?,
    ))
}

async fn readiness(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<ready::Readiness>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    ready::readiness_of(&state.pool, member.reader(), project_id, task_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("work_item"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GraphQuery {
    milestone_id: Option<Uuid>,
    include_finished: Option<bool>,
}

async fn graph(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<GraphQuery>,
) -> Result<Json<Value>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    let graph = ready::graph(
        &state.pool,
        member.reader(),
        project_id,
        &GraphFilter {
            milestone_id: query.milestone_id,
            include_finished: query.include_finished.unwrap_or(false),
        },
    )
    .await?;
    let mut body = serde_json::to_value(graph).map_err(|_| AppError::Internal("graph"))?;
    body["canEdit"] = json!(member.can_edit);
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// Plans

async fn list_plans(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<Value>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    Ok(Json(json!({
        "plans": plans::list(&state.pool, project_id).await?,
        "canEdit": member.can_edit,
    })))
}

async fn plan(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    let plan = plans::get(&state.pool, project_id, import_id)
        .await?
        .ok_or(AppError::NotFound("plan"))?;
    Ok(Json(json!({"plan": plan, "canEdit": member.can_edit})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreatePlanRequest {
    title: String,
    source: Source,
}

fn create_refusal(refusal: CreateRefusal) -> Response {
    match refusal {
        CreateRefusal::Invalid(message) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            message,
        ),
        CreateRefusal::NotFound(message) => {
            error_response(StatusCode::NOT_FOUND, "not_found", message)
        }
        CreateRefusal::Ai(error) => error.into_response(),
    }
}

async fn create_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreatePlanRequest>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemCreate,
    )
    .await?;
    match plans::create(
        &state,
        member.organization_id,
        &member.actor,
        &request.title,
        request.source,
    )
    .await?
    {
        Ok(plan) => Ok((StatusCode::CREATED, Json(json!({"plan": plan}))).into_response()),
        Err(refusal) => Ok(create_refusal(refusal)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavePlanRequest {
    expected_version: i64,
    title: Option<String>,
    draft: Draft,
}

fn edit_refusal(refusal: plans::EditRefusal) -> Response {
    let status = match &refusal {
        plans::EditRefusal::NotFound => StatusCode::NOT_FOUND,
        plans::EditRefusal::WrongState(_) | plans::EditRefusal::VersionConflict(_) => {
            StatusCode::CONFLICT
        }
        plans::EditRefusal::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let code = match &refusal {
        plans::EditRefusal::NotFound => "not_found",
        plans::EditRefusal::WrongState(_) => "plan_not_editable",
        plans::EditRefusal::VersionConflict(_) => "version_conflict",
        plans::EditRefusal::Invalid(_) => "validation_failed",
    };
    error_response(status, code, refusal.message())
}

async fn save_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<SavePlanRequest>,
) -> Result<Response, AppError> {
    writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemCreate,
    )
    .await?;
    if let Err(refusal) = plans::update_draft(
        &state.pool,
        project_id,
        import_id,
        request.expected_version,
        request.title.as_deref(),
        &request.draft,
    )
    .await?
    {
        return Ok(edit_refusal(refusal));
    }
    let plan = plans::get(&state.pool, project_id, import_id).await?;
    Ok(Json(json!({"plan": plan})).into_response())
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitPlanRequest {
    expected_version: i64,
}

async fn commit_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, import_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CommitPlanRequest>,
) -> Result<Response, AppError> {
    let member = writer(
        &state,
        &headers,
        &principal,
        project_id,
        Action::WorkItemCreate,
    )
    .await?;
    if !member.can_edit {
        return Err(AppError::Forbidden);
    }
    if let Err(refusal) = plans::commit(
        &state.pool,
        member.organization_id,
        project_id,
        import_id,
        request.expected_version,
        principal.actor_id,
    )
    .await?
    {
        return Ok(edit_refusal(refusal));
    }
    let plan = plans::get(&state.pool, project_id, import_id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"plan": plan}))).into_response())
}

// ---------------------------------------------------------------------------
// Summaries

fn summary_response(outcome: Result<summaries::Summary, SummaryRefusal>) -> Response {
    match outcome {
        Ok(summary) => Json(json!({"summary": summary})).into_response(),
        Err(SummaryRefusal::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "That wasn't found, or you can't see it.",
        ),
        Err(SummaryRefusal::Empty) => Json(json!({"summary": {
            "lines": ["Nothing has happened here yet."],
            "outcome": null,
            "status": null,
            "entries": 0,
            "provider": "",
            "model": "",
        }}))
        .into_response(),
        Err(SummaryRefusal::Ai(error)) => error.into_response(),
    }
}

async fn task_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, task_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    Ok(summary_response(
        summaries::task(
            &state,
            member.organization_id,
            project_id,
            principal.actor_id,
            None,
            task_id,
        )
        .await,
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThreadSummaryRequest {
    since: Option<DateTime<Utc>>,
}

async fn thread_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ThreadSummaryRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let Some(project_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM conversations WHERE id = $1 AND conversation_visible_to(id, $2)",
    )
    .bind(conversation_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(AppError::NotFound("conversation"));
    };
    let member = member(&state, &principal, project_id, Action::WorkItemView).await?;
    Ok(summary_response(
        summaries::thread(
            &state,
            member.organization_id,
            project_id,
            principal.actor_id,
            None,
            conversation_id,
            request.since,
        )
        .await,
    ))
}
