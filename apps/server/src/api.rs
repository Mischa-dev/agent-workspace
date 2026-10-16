use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ApprovalCapabilities, ApprovalRow, ApprovalView, RunCapabilities, RunRow, RunView,
    },
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::{ApiJson, ApiPath, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    message_content::MessageContent,
    outbox,
    policy::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, Membership,
        MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
    provider::run_config::RunConfig,
};

const ANY_AUTHORIZED_APPROVER: &str = "any_authorized";

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::auth::router())
        .merge(crate::projects::router())
        .merge(crate::execution::router())
        .merge(crate::external_execution::router())
        .route("/api/v1/dev/bootstrap", get(development_bootstrap))
        .route("/api/v1/threads/{id}/agent-runs", post(start_agent_run))
        .route(
            "/api/v1/agent-runs/{id}/interrupt",
            post(interrupt_agent_run),
        )
        .route("/api/v1/agent-runs/{id}/retry", post(retry_agent_run))
        .route(
            "/api/v1/agent-runs/{id}/acknowledge-failure",
            post(acknowledge_run_failure),
        )
        .route(
            "/api/v1/agent-approvals/{id}/respond",
            post(respond_to_approval),
        )
        .route("/api/v1/realtime", get(crate::realtime::websocket))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevelopmentBootstrap {
    project_id: Uuid,
    channel_id: Uuid,
    thread_id: Uuid,
}

async fn development_bootstrap(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<DevelopmentBootstrap>, AppError> {
    if !state.config.environment.is_development() || !state.config.server_addr.ip().is_loopback() {
        return Err(AppError::DevelopmentAuthUnavailable);
    }

    #[derive(FromRow)]
    struct BootstrapRow {
        project_id: Uuid,
        channel_id: Uuid,
        thread_id: Uuid,
        membership_role: String,
        membership_enabled: bool,
    }

    let bootstrap = sqlx::query_as::<_, BootstrapRow>(
        r#"
        SELECT
            thread.project_id,
            parent.id AS channel_id,
            thread.id AS thread_id,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled
        FROM conversations thread
        JOIN conversations parent ON parent.id = thread.parent_channel_id
        JOIN projects p ON p.id = thread.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $1
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = thread.project_id
         AND pm.actor_id = $1
        WHERE p.archived_at IS NULL
          AND thread.kind = 'thread'
          AND thread.archived_at IS NULL
          AND parent.archived_at IS NULL
        ORDER BY (p.project_key = 'AW') DESC, thread.created_at
        LIMIT 1
        "#,
    )
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("seeded_thread"))?;
    authorize_project_action(
        &principal,
        bootstrap.project_id,
        &bootstrap.membership_role,
        bootstrap.membership_enabled,
        Action::ProjectView,
        Resource::Project(ProjectScope::thread(
            bootstrap.project_id,
            bootstrap.thread_id,
        )),
    )?;

    Ok(Json(DevelopmentBootstrap {
        project_id: bootstrap.project_id,
        channel_id: bootstrap.channel_id,
        thread_id: bootstrap.thread_id,
    }))
}

#[cfg(any())]
#[derive(Clone, Copy)]
struct RequestIdentity {
    actor_id: Uuid,
    project_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ConversationRow {
    id: Uuid,
    project_id: Uuid,
    kind: String,
    conversation_archived: bool,
    project_archived: bool,
    membership_role: String,
    membership_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartRunRequest {
    pub(crate) prompt: String,
    pub(crate) repository_id: Option<Uuid>,
    /// Launch overrides for this run (harness, model, reasoning, speed, ...).
    /// Whatever is left out comes from the account's defaults and the
    /// harness's catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) config: Option<RunConfig>,
    /// One of the caller's own provider accounts to run on; left out, their
    /// preferred profile's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider_account_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
pub(crate) struct RunSetup {
    pub(crate) repository_id: Uuid,
    pub(crate) execution_worker_id: Uuid,
    pub(crate) execution_profile_id: Uuid,
    pub(crate) organization_id: Uuid,
    pub(crate) adapter_kind: String,
    pub(crate) run_defaults: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartRunResponse {
    pub(crate) run_id: Uuid,
    pub(crate) state: RunState,
    pub(crate) repository_id: Uuid,
    pub(crate) repository_workspace_id: Option<Uuid>,
    pub(crate) branch: Option<String>,
    pub(crate) base_commit: Option<String>,
    pub(crate) queue_position: Option<i64>,
}

#[cfg(any())]
const WORKTREE_START_FAILURE_REASON: &str =
    "Worktree creation failed; inspect the retained repository workspace.";
#[cfg(any())]
const PROVIDER_START_FAILURE_REASON: &str =
    "Codex App Server could not start. The worktree was preserved.";

pub(crate) async fn start_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<StartRunRequest>,
) -> Result<(StatusCode, Json<StartRunResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let (status, response) = start_run_in(
        &state,
        &principal,
        None,
        conversation_id,
        request,
        IdempotencyKey::from_headers(&headers),
    )
    .await?;
    Ok((status, Json(response)))
}

/// Start a Codex run in a thread on the caller's own Codex profile. Shared by
/// the browser and MCP. `via_grant_id` is the connected app a person acts
/// through; runs started that way also count against the per-person cap.
pub(crate) async fn start_run_in(
    state: &AppState,
    principal: &impl RunActor,
    via_grant_id: Option<Uuid>,
    conversation_id: Uuid,
    mut request: StartRunRequest,
    // Checked after authorization, so a caller who may not start runs is
    // refused as such rather than for a missing key.
    idempotency_key: Result<IdempotencyKey, AppError>,
) -> Result<(StatusCode, StartRunResponse), AppError> {
    request.prompt = request.prompt.trim().to_owned();
    let (prompt_content, prompt, prompt_mentions) =
        MessageContent::from_plain_text(request.prompt.clone())
            .map_err(|_| AppError::Validation("Message content"))?
            .into_parts();
    debug_assert!(prompt_mentions.is_empty());
    let prompt_content = serde_json::to_value(prompt_content)
        .map_err(|_| AppError::Internal("serialize hosted prompt Message"))?;
    let conversation =
        authorized_conversation(state, principal, conversation_id, Action::WorkerInvoke).await?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived);
    }
    if conversation.conversation_archived {
        return Err(AppError::NotFound("conversation"));
    }
    if conversation.kind != "thread" {
        return Err(AppError::Invalid("agent runs require a Thread"));
    }

    let idempotency_key = idempotency_key?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = idempotency::resource_bound_scope("thread", conversation_id, "agent_run_start")?;
    let mut transaction = state.pool.begin().await?;
    guard_conversation_mutation(
        &mut transaction,
        principal,
        conversation_id,
        conversation.project_id,
        Action::WorkerInvoke,
    )
    .await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id(),
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let response: StartRunResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), response));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    if principal.is_mcp()
        && crate::runs::run_cap(&mut transaction, principal.actor_id())
            .await?
            .reached()
    {
        return Err(AppError::Conflict(
            "agent_run",
            "you already have 5 Codex runs queued or working; wait for one to finish",
        ));
    }
    if sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
            WHERE conversation_id = $1
              AND state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        )
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&mut *transaction)
    .await?
    {
        return Err(AppError::Conflict(
            "agent_run",
            "Thread already has an active run",
        ));
    }

    let launch = request.config.clone().unwrap_or_default();
    // The person's and project's run settings (S2) fill in what the launch
    // leaves out: the harness (the default environment's first), the
    // repository when there are several, and the run defaults.
    let defaults = crate::launch::run_defaults(
        &mut transaction,
        conversation.project_id,
        principal.actor_id(),
    )
    .await?;
    let mut harness = launch
        .harness
        .clone()
        .or_else(|| defaults.preferred_harness());
    let mut repository_id = request.repository_id;
    let setup = loop {
        match select_run_setup_for(
            &mut transaction,
            conversation.project_id,
            conversation_id,
            principal.actor_id(),
            repository_id,
            harness.as_deref(),
            request.provider_account_id,
        )
        .await
        {
            // No account for the default harness: any harness they have.
            Err(AppError::ExecutionProfileRequired)
                if launch.harness.is_none() && harness.is_some() =>
            {
                harness = None;
            }
            Err(AppError::RepositoryChoiceRequired { .. })
                if repository_id.is_none() && defaults.repository_id.is_some() =>
            {
                repository_id = defaults.repository_id;
            }
            result => break result?,
        }
    };
    let account_defaults = match state.supervisors.manifest(&setup.adapter_kind) {
        Some(manifest) => crate::launch::merged_defaults(&setup.run_defaults, &defaults, manifest),
        None => setup.run_defaults.clone(),
    };
    let config = crate::provider::resolve_run_config(
        state,
        setup.organization_id,
        &setup.adapter_kind,
        &account_defaults,
        &launch,
    )
    .await?;
    let bound = sqlx::query(
        r#"
        UPDATE conversations
        SET repository_id = COALESCE(repository_id, $2), updated_at = now()
        WHERE id = $1 AND project_id = $3
          AND (repository_id IS NULL OR repository_id = $2)
        "#,
    )
    .bind(conversation_id)
    .bind(setup.repository_id)
    .bind(conversation.project_id)
    .execute(&mut *transaction)
    .await?;
    if bound.rows_affected() != 1 {
        return Err(AppError::RepositoryBindingConflict);
    }

    let prompt_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text, via_oauth_grant_id
        )
        VALUES ($1, $2, $3, $4, 1, $5, $6, $7)
        "#,
    )
    .bind(prompt_message_id)
    .bind(conversation.project_id)
    .bind(conversation_id)
    .bind(principal.actor_id())
    .bind(prompt_content)
    .bind(&prompt)
    .bind(via_grant_id)
    .execute(&mut *transaction)
    .await?;
    let prompt_sequence = allocate_conversation_sequence(&mut transaction, conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind, message_id)
        VALUES ($1, $2, $3, 'message', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(prompt_sequence)
    .bind(prompt_message_id)
    .execute(&mut *transaction)
    .await?;

    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, prompt_message_id, config
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'created', $7, $8, $9, $10)
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(setup.execution_worker_id)
    .bind(principal.actor_id())
    .bind(setup.execution_profile_id)
    .bind(setup.repository_id)
    .bind(&prompt)
    .bind(serde_json::json!({
        "actor_id": principal.actor_id(),
        "project_id": conversation.project_id,
        "scope": ["repository.read", "repository.write", "provider.turn"]
    }))
    .bind(prompt_message_id)
    .bind(serde_json::to_value(&config).map_err(|_| AppError::Internal("serialize run settings"))?)
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Queued, None).await?;
    let queue_position = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM agent_runs queued
        JOIN agent_runs current ON current.id = $1
        WHERE queued.state = 'queued'
          AND (queued.created_at, queued.id) <= (current.created_at, current.id)
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut *transaction)
    .await?;
    outbox::enqueue_message_created(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        prompt_message_id,
        prompt_sequence,
        principal.actor_id(),
    )
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        run_id,
    )
    .await?;
    let response = StartRunResponse {
        run_id,
        state: RunState::Queued,
        repository_id: setup.repository_id,
        repository_workspace_id: None,
        branch: None,
        base_commit: None,
        queue_position: Some(queue_position),
    };
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::ACCEPTED,
        &response,
    )
    .await?;
    transaction.commit().await?;

    crate::scheduler::kick(state.clone());
    Ok((StatusCode::ACCEPTED, response))
}

async fn retry_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(predecessor_run_id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyCommand>,
) -> Result<(StatusCode, Json<StartRunResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let conversation_id =
        sqlx::query_scalar::<_, Uuid>("SELECT conversation_id FROM agent_runs WHERE id = $1")
            .bind(predecessor_run_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound("agent_run"))?;
    let conversation =
        authorized_conversation(&state, &principal, conversation_id, Action::WorkerInvoke).await?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived);
    }
    if conversation.conversation_archived {
        return Err(AppError::NotFound("conversation"));
    }

    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&EmptyCommand {})?;
    let scope = idempotency::resource_bound_scope("agent_run", predecessor_run_id, "retry")?;
    let mut transaction = state.pool.begin().await?;
    guard_conversation_mutation(
        &mut transaction,
        &principal,
        conversation_id,
        conversation.project_id,
        Action::WorkerInvoke,
    )
    .await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let response: StartRunResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(response)));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let (repository_id, prompt, predecessor_state, prompt_message_id, predecessor_config) =
        sqlx::query_as::<_, (Uuid, String, String, Option<Uuid>, serde_json::Value)>(
            r#"
            SELECT repository_id, prompt, state, prompt_message_id, config
              FROM agent_runs
             WHERE id = $1 AND conversation_id = $2 AND project_id = $3
             FOR UPDATE
            "#,
        )
        .bind(predecessor_run_id)
        .bind(conversation_id)
        .bind(conversation.project_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("agent_run"))?;
    if let Some(message_id) = prompt_message_id
        && sqlx::query_scalar::<_, bool>(
            "SELECT removed_at IS NOT NULL FROM messages WHERE id = $1",
        )
        .bind(message_id)
        .fetch_one(&mut *transaction)
        .await?
    {
        return Err(AppError::Conflict(
            "agent_run",
            "the prompt message was removed; start a new run",
        ));
    }
    if !matches!(
        predecessor_state.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        return Err(AppError::Conflict(
            "agent_run",
            "only a terminal Agent Run can be retried",
        ));
    }
    if sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
             WHERE conversation_id = $1
               AND state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        )
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&mut *transaction)
    .await?
    {
        return Err(AppError::Conflict(
            "agent_run",
            "Thread already has an active run",
        ));
    }
    // A retry asks for the same settings again; whatever the account or the
    // catalog no longer offers falls back on its own, and a sandbox or
    // approval policy that is gone refuses the retry rather than loosening.
    let predecessor_config = RunConfig::from_value(&predecessor_config);
    let setup = select_run_setup_for(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        principal.actor_id,
        Some(repository_id),
        predecessor_config.harness.as_deref(),
        None,
    )
    .await?;
    let config = crate::provider::resolve_stored_run_config(
        &state,
        setup.organization_id,
        &setup.adapter_kind,
        &setup.run_defaults,
        &predecessor_config,
    )
    .await?;
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, predecessor_run_id,
            state, prompt, authorization_snapshot, prompt_message_id, config
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'created', $8, $9, $10, $11)
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(setup.execution_worker_id)
    .bind(principal.actor_id)
    .bind(setup.execution_profile_id)
    .bind(repository_id)
    .bind(predecessor_run_id)
    .bind(prompt)
    .bind(serde_json::json!({
        "actor_id": principal.actor_id,
        "project_id": conversation.project_id,
        "scope": ["repository.read", "repository.write", "provider.turn"],
        "retry_of": predecessor_run_id
    }))
    .bind(prompt_message_id)
    .bind(serde_json::to_value(&config).map_err(|_| AppError::Internal("serialize run settings"))?)
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Queued, None).await?;
    let queue_position = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
          FROM agent_runs queued
          JOIN agent_runs current ON current.id = $1
         WHERE queued.state = 'queued'
           AND (queued.created_at, queued.id) <= (current.created_at, current.id)
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut *transaction)
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        run_id,
    )
    .await?;
    let response = StartRunResponse {
        run_id,
        state: RunState::Queued,
        repository_id,
        repository_workspace_id: None,
        branch: None,
        base_commit: None,
        queue_position: Some(queue_position),
    };
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::ACCEPTED,
        &response,
    )
    .await?;
    transaction.commit().await?;
    crate::scheduler::kick(state.clone());
    Ok((StatusCode::ACCEPTED, Json(response)))
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FailureAcknowledgementRequest {
    event_id: Uuid,
}

async fn acknowledge_run_failure(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(run_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<FailureAcknowledgementRequest>,
) -> Result<Json<EmptyCommand>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let conversation_id =
        sqlx::query_scalar::<_, Uuid>("SELECT conversation_id FROM agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound("agent_run"))?;
    let conversation =
        authorized_conversation(&state, &principal, conversation_id, Action::ReadStateUpdate)
            .await?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = idempotency::resource_bound_scope("agent_run", run_id, "acknowledge_failure")?;
    let mut transaction = state.pool.begin().await?;
    guard_conversation_mutation(
        &mut transaction,
        &principal,
        conversation_id,
        conversation.project_id,
        Action::ReadStateUpdate,
    )
    .await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let _: EmptyCommand = replay.decode()?;
            transaction.commit().await?;
            return Ok(Json(EmptyCommand {}));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let valid_failure = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
              FROM agent_runs run
              JOIN agent_run_events event ON event.agent_run_id = run.id
              JOIN conversation_entries entry
                ON entry.agent_run_event_id = event.id
               AND entry.conversation_id = run.conversation_id
             WHERE run.id = $1
               AND run.project_id = $2
               AND run.state = 'failed'
               AND event.id = $3
               AND event.kind = 'run_failed'
        )
        "#,
    )
    .bind(run_id)
    .bind(conversation.project_id)
    .bind(request.event_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !valid_failure {
        return Err(AppError::Conflict(
            "agent_run",
            "failure event is not acknowledgeable",
        ));
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO agent_run_acknowledgements (
            id, project_id, agent_run_id, actor_id, acknowledged_event_id
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (agent_run_id, actor_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation.project_id)
    .bind(run_id)
    .bind(principal.actor_id)
    .bind(request.event_id)
    .execute(&mut *transaction)
    .await?;
    let created_acknowledgement = inserted.rows_affected() == 1;
    if !created_acknowledgement {
        let acknowledged_event_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT acknowledged_event_id FROM agent_run_acknowledgements WHERE agent_run_id = $1 AND actor_id = $2",
        )
        .bind(run_id)
        .bind(principal.actor_id)
        .fetch_one(&mut *transaction)
        .await?;
        if acknowledged_event_id != request.event_id {
            return Err(AppError::Conflict(
                "agent_run_acknowledgement",
                "a different failure event is already acknowledged",
            ));
        }
    }
    if created_acknowledgement {
        outbox::enqueue_agent_run_updated(
            &mut transaction,
            conversation.project_id,
            conversation_id,
            run_id,
        )
        .await?;
    }
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &EmptyCommand {},
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(EmptyCommand {}))
}

pub(crate) async fn select_run_setup(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    profile_owner_actor_id: Uuid,
    explicit_repository_id: Option<Uuid>,
) -> Result<RunSetup, AppError> {
    select_run_setup_for(
        transaction,
        project_id,
        conversation_id,
        profile_owner_actor_id,
        explicit_repository_id,
        None,
        None,
    )
    .await
}

/// `select_run_setup` limited to profiles on one harness when `harness` is
/// set. Among the owner's signed-in profiles the preferred one comes first.
pub(crate) async fn select_run_setup_for(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    profile_owner_actor_id: Uuid,
    explicit_repository_id: Option<Uuid>,
    harness: Option<&str>,
    provider_account_id: Option<Uuid>,
) -> Result<RunSetup, AppError> {
    let bound_repository_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT repository_id FROM conversations WHERE id = $1 AND project_id = $2 FOR UPDATE",
    )
    .bind(conversation_id)
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;
    if bound_repository_id.is_some()
        && explicit_repository_id.is_some()
        && bound_repository_id != explicit_repository_id
    {
        return Err(AppError::RepositoryBindingConflict);
    }
    let ready_repository_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM repositories
        WHERE project_id = $1 AND enabled AND trusted
          AND archived_at IS NULL AND preparation_state = 'ready'
        ORDER BY created_at, id
        LIMIT 101
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let linked_work_item_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_conversation_links WHERE project_id = $1 AND conversation_id = $2",
    )
    .bind(project_id)
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    let linked_primary_repository_id = if linked_work_item_count == 1 {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT repository.id
            FROM work_item_conversation_links conversation_link
            JOIN work_item_repository_links repository_link
              ON repository_link.project_id = conversation_link.project_id
             AND repository_link.work_item_id = conversation_link.work_item_id
             AND repository_link.role = 'primary'
            JOIN repositories repository
              ON repository.project_id = repository_link.project_id
             AND repository.id = repository_link.repository_id
            WHERE conversation_link.project_id = $1
              AND conversation_link.conversation_id = $2
              AND repository.enabled AND repository.trusted
              AND repository.archived_at IS NULL
              AND repository.preparation_state = 'ready'
            "#,
        )
        .bind(project_id)
        .bind(conversation_id)
        .fetch_optional(&mut **transaction)
        .await?
    } else {
        None
    };
    let selected_repository_id = match bound_repository_id
        .or(explicit_repository_id)
        .or(linked_primary_repository_id)
    {
        Some(repository_id) if ready_repository_ids.contains(&repository_id) => repository_id,
        Some(_) => return Err(AppError::NotFound("repository")),
        None if ready_repository_ids.len() == 1 => ready_repository_ids[0],
        None => {
            return Err(AppError::RepositoryChoiceRequired {
                repository_ids: ready_repository_ids,
            });
        }
    };
    sqlx::query_as::<_, RunSetup>(
        r#"
        SELECT r.id AS repository_id,
               ew.id AS execution_worker_id,
               profile.id AS execution_profile_id,
               account.organization_id,
               account.adapter_kind,
               account.run_defaults
        FROM repositories r
        JOIN execution_workers ew
          ON ew.project_id = r.project_id AND ew.enabled
        JOIN project_memberships worker_membership
          ON worker_membership.project_id = ew.project_id
         AND worker_membership.actor_id = ew.actor_id
         AND worker_membership.role = 'worker'
         AND worker_membership.enabled
        JOIN actors worker_actor
          ON worker_actor.id = ew.actor_id
         AND worker_actor.kind = 'execution_worker'
         AND worker_actor.status = 'active'
        JOIN provider_connections connection
          ON connection.project_id = ew.project_id
         AND connection.id = ew.provider_connection_id
         AND connection.enabled
        JOIN execution_profiles profile
          ON profile.project_id = ew.project_id
         AND profile.owner_actor_id = $3
         AND profile.status = 'available'
         AND profile.account_state = 'authenticated'
        JOIN provider_connections profile_connection
          ON profile_connection.id = profile.provider_connection_id
         AND profile_connection.project_id = profile.project_id
         AND profile_connection.enabled
        JOIN provider_accounts account
          ON account.id = profile.provider_account_id
         AND ($4::text IS NULL OR account.adapter_kind = $4)
         AND ($5::uuid IS NULL OR account.id = $5)
        JOIN project_memberships owner_membership
          ON owner_membership.project_id = profile.project_id
         AND owner_membership.actor_id = profile.owner_actor_id
         AND owner_membership.enabled
        JOIN actors owner_actor
          ON owner_actor.id = profile.owner_actor_id
         AND owner_actor.status = 'active'
        WHERE r.project_id = $1 AND r.id = $2
          AND r.enabled AND r.trusted AND r.archived_at IS NULL
          AND r.preparation_state = 'ready'
        ORDER BY ew.created_at, ew.id, profile.preferred DESC,
                 profile.created_at, profile.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(selected_repository_id)
    .bind(profile_owner_actor_id)
    .bind(harness)
    .bind(provider_account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::ExecutionProfileRequired)
}

#[cfg(any())]
async fn legacy_start_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<StartRunRequest>,
) -> Result<Json<StartRunResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let (prompt_content, prompt, prompt_mentions) =
        MessageContent::from_plain_text(request.prompt.trim().to_owned())
            .map_err(|_| AppError::Validation("Message content"))?
            .into_parts();
    debug_assert!(prompt_mentions.is_empty());
    let prompt_content = serde_json::to_value(prompt_content)
        .map_err(|_| AppError::Internal("serialize hosted prompt Message"))?;
    let conversation =
        authorized_conversation(&state, &principal, conversation_id, Action::WorkerInvoke).await?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived);
    }
    if conversation.conversation_archived {
        return Err(AppError::NotFound("conversation"));
    }
    if conversation.kind != "thread" {
        return Err(AppError::Invalid("agent runs require a Thread"));
    }
    let identity = RequestIdentity {
        actor_id: principal.actor_id,
        project_id: conversation.project_id,
    };

    let setup = sqlx::query_as::<_, RunSetup>(
        r#"
        SELECT
            p.project_key,
            r.id AS repository_id,
            r.mirror_path,
            r.default_branch,
            ew.id AS execution_worker_id,
            ew.execution_profile_id
        FROM projects p
        JOIN execution_workers ew
          ON ew.project_id = p.id AND ew.enabled
        JOIN project_memberships worker_membership
          ON worker_membership.project_id = ew.project_id
         AND worker_membership.actor_id = ew.actor_id
         AND worker_membership.role = 'worker'
         AND worker_membership.enabled
        JOIN actors worker_actor
          ON worker_actor.id = ew.actor_id
         AND worker_actor.kind = 'execution_worker'
         AND worker_actor.status = 'active'
        JOIN provider_connections connection
          ON connection.project_id = ew.project_id
         AND connection.id = ew.provider_connection_id
         AND connection.enabled
        JOIN execution_profiles ep
          ON ep.project_id = ew.project_id
         AND ep.id = ew.execution_profile_id
         AND ep.provider_connection_id = connection.id
         AND ep.owner_actor_id = $3
         AND ep.status = 'available'
         AND ep.account_state = 'authenticated'
        JOIN project_memberships owner_membership
          ON owner_membership.project_id = ep.project_id
         AND owner_membership.actor_id = ep.owner_actor_id
         AND owner_membership.enabled
        JOIN actors owner_actor
          ON owner_actor.id = ep.owner_actor_id
         AND owner_actor.status = 'active'
        JOIN repositories r
          ON r.project_id = p.id
         AND r.enabled
         AND r.trusted
         AND r.archived_at IS NULL
         AND r.preparation_state = 'ready'
        WHERE p.id = $1
          AND r.id = COALESCE(
              $2,
              CASE
                  WHEN (SELECT count(*) FROM repositories rr
                         WHERE rr.project_id = p.id
                           AND rr.enabled
                           AND rr.trusted
                           AND rr.archived_at IS NULL
                           AND rr.preparation_state = 'ready') = 1
                  THEN r.id
                  ELSE NULL
              END
          )
        LIMIT 1
        "#,
    )
    .bind(identity.project_id)
    .bind(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT repository_id FROM conversations WHERE id = $1 AND project_id = $2",
        )
        .bind(conversation_id)
        .bind(identity.project_id)
        .fetch_one(&state.pool)
        .await?,
    )
    .bind(identity.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Conflict(
        "repository",
        "a deterministic trusted repository is required",
    ))?;

    let admission_root = state.config.data_root.clone();
    let admission_quota = state.config.data_quota_bytes;
    let admission_free = state.config.minimum_free_bytes;
    tokio::task::spawn_blocking(move || {
        crate::storage_admission::require_growth(&admission_root, admission_quota, admission_free)
    })
    .await
    .map_err(|_| AppError::Internal("join storage admission"))??;

    let mirror = git_runtime::absolute_runtime_path(PathBuf::from(&setup.mirror_path).as_path())?;
    let default_branch = setup.default_branch.clone();
    let mirror_for_resolution = mirror.clone();
    let base_commit = tokio::task::spawn_blocking(move || {
        git_runtime::resolve_base_commit(&mirror_for_resolution, &default_branch)
    })
    .await
    .map_err(|_| AppError::Internal("join base commit task"))??;

    let run_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    let worktree_id = Uuid::now_v7();
    let branch = git_runtime::unique_run_branch(&setup.project_key)?;
    let data_root = git_runtime::absolute_runtime_path(&state.config.data_root)?;
    let worktree_path =
        git_runtime::managed_worktree_path(&data_root, setup.repository_id, workspace_id)?;

    let mut transaction = state.pool.begin().await?;
    guard_conversation_mutation(
        &mut transaction,
        &principal,
        conversation_id,
        identity.project_id,
        Action::WorkerInvoke,
    )
    .await?;
    if sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_runs ar
            JOIN conversations c
              ON c.id = ar.conversation_id
             AND c.project_id = $2
            WHERE ar.conversation_id = $1
              AND ar.state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        )
        "#,
    )
    .bind(conversation_id)
    .bind(identity.project_id)
    .fetch_one(&mut *transaction)
    .await?
    {
        return Err(AppError::Conflict(
            "agent_run",
            "Thread already has an active run",
        ));
    }

    sqlx::query(
        "UPDATE conversations SET repository_id = COALESCE(repository_id, $2), updated_at = now() WHERE id = $1 AND project_id = $3",
    )
    .bind(conversation_id)
    .bind(setup.repository_id)
    .bind(identity.project_id)
    .execute(&mut *transaction)
    .await?;

    let prompt_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text
        )
        VALUES ($1, $2, $3, $4, 1, $5, $6)
        "#,
    )
    .bind(prompt_message_id)
    .bind(identity.project_id)
    .bind(conversation_id)
    .bind(identity.actor_id)
    .bind(prompt_content)
    .bind(&prompt)
    .execute(&mut *transaction)
    .await?;
    let prompt_sequence = allocate_conversation_sequence(&mut transaction, conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, message_id
        )
        VALUES ($1, $2, $3, 'message', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(prompt_sequence)
    .bind(prompt_message_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO repository_workspaces (
            id, repository_id, conversation_id, branch_name, base_commit
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(workspace_id)
    .bind(setup.repository_id)
    .bind(conversation_id)
    .bind(&branch)
    .bind(&base_commit)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO git_worktrees (
            id, repository_workspace_id, path, state
        )
        VALUES ($1, $2, $3, 'creating')
        "#,
    )
    .bind(worktree_id)
    .bind(workspace_id)
    .bind(worktree_path.to_string_lossy().as_ref())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, repository_workspace_id,
            state, prompt, authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'created', $8, $9)
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .bind(setup.execution_worker_id)
    .bind(identity.actor_id)
    .bind(setup.execution_profile_id)
    .bind(setup.repository_id)
    .bind(workspace_id)
    .bind(&prompt)
    .bind(serde_json::json!({
        "actor_id": identity.actor_id,
        "project_id": identity.project_id,
        "scope": ["repository.read", "repository.write", "provider.turn"]
    }))
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Starting, None).await?;
    outbox::enqueue_message_created(
        &mut transaction,
        identity.project_id,
        conversation_id,
        prompt_message_id,
        prompt_sequence,
        identity.actor_id,
    )
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        identity.project_id,
        conversation_id,
        run_id,
    )
    .await?;
    transaction.commit().await?;

    let mirror_for_worktree = mirror.clone();
    let worktree_for_creation = worktree_path.clone();
    let branch_for_creation = branch.clone();
    let base_for_creation = base_commit.clone();
    let creation = tokio::task::spawn_blocking(move || {
        git_runtime::create_worktree(
            &mirror_for_worktree,
            &worktree_for_creation,
            &branch_for_creation,
            &base_for_creation,
        )
    })
    .await;
    let creation = match creation {
        Ok(creation) => creation,
        Err(_) => {
            fail_worktree_start(
                &state,
                run_id,
                worktree_id,
                conversation_id,
                WORKTREE_START_FAILURE_REASON,
            )
            .await?;
            return Err(AppError::Internal("join worktree task"));
        }
    };
    if let Err(error) = creation {
        fail_worktree_start(
            &state,
            run_id,
            worktree_id,
            conversation_id,
            WORKTREE_START_FAILURE_REASON,
        )
        .await?;
        return Err(error);
    }

    let response_state = if state.config.demo_executor {
        let approval_id =
            prepare_demo_blocker(&state, identity, run_id, worktree_id, conversation_id).await?;
        tracing::info!(%run_id, %approval_id, "development Agent Run is waiting for approval");
        RunState::Blocked
    } else {
        if let Err(error) = mark_worktree_ready(&state, worktree_id).await {
            fail_worktree_start(
                &state,
                run_id,
                worktree_id,
                conversation_id,
                WORKTREE_START_FAILURE_REASON,
            )
            .await?;
            return Err(error);
        }
        let provider_start = crate::provider::start_run(
            &state,
            identity.project_id,
            run_id,
            conversation_id,
            setup.execution_profile_id,
            worktree_path.clone(),
            prompt.clone(),
        )
        .await;
        match provider_start {
            Ok(started) => {
                tracing::info!(
                    %run_id,
                    provider_session_id = %started.provider_session_id,
                    provider_thread_id = %started.provider_thread_id,
                    "hosted Codex Agent Run started"
                );
                RunState::Working
            }
            Err(error) => {
                fail_provider_start(&state, run_id, conversation_id).await?;
                return Err(error);
            }
        }
    };

    Ok(Json(StartRunResponse {
        run_id,
        state: response_state,
        repository_workspace_id: workspace_id,
        branch,
        base_commit,
        worktree_path: worktree_path.to_string_lossy().into_owned(),
    }))
}

#[cfg(any())]
async fn mark_worktree_ready(state: &AppState, worktree_id: Uuid) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE git_worktrees SET state = 'ready', last_inspected_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(worktree_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "git_worktree",
            "created worktree record is unavailable",
        ));
    }
    Ok(())
}

#[cfg(any())]
async fn fail_provider_start(
    state: &AppState,
    run_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    persist_run_start_failure(
        &mut transaction,
        run_id,
        conversation_id,
        PROVIDER_START_FAILURE_REASON,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(any())]
async fn prepare_demo_blocker(
    state: &AppState,
    identity: RequestIdentity,
    run_id: Uuid,
    worktree_id: Uuid,
    conversation_id: Uuid,
) -> Result<Uuid, AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "UPDATE git_worktrees SET state = 'ready', last_inspected_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(worktree_id)
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Working, None).await?;

    let approval_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        )
        VALUES (
            $1, $2, $3, $4, 'approval',
            'Allow the development executor to append its proof line to KNOWN_FILE.txt?',
            $5, 'owner', true
        )
        "#,
    )
    .bind(approval_id)
    .bind(run_id)
    .bind(identity.actor_id)
    .bind(format!("development-approval-{approval_id}"))
    .bind(serde_json::json!([
        {"id": "approve", "label": "Allow once"},
        {"id": "deny", "label": "Interrupt run"}
    ]))
    .execute(&mut *transaction)
    .await?;
    crate::external_events::enqueue_external_approval_requested(&mut transaction, approval_id)
        .await?;
    let event_id = insert_significant_event(
        &mut transaction,
        run_id,
        "approval_requested",
        "The run is waiting for an approval.",
        serde_json::json!({"approval_id": approval_id}),
    )
    .await?;
    insert_event_entry(&mut transaction, conversation_id, event_id).await?;
    transition_run(&mut transaction, run_id, RunState::Blocked, None).await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        identity.project_id,
        conversation_id,
        run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(approval_id)
}

#[cfg(any())]
async fn fail_worktree_start(
    state: &AppState,
    run_id: Uuid,
    worktree_id: Uuid,
    conversation_id: Uuid,
    reason: &'static str,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE git_worktrees SET state = 'failed', updated_at = now() WHERE id = $1")
        .bind(worktree_id)
        .execute(&mut *transaction)
        .await?;
    persist_run_start_failure(&mut transaction, run_id, conversation_id, reason).await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(any())]
async fn persist_run_start_failure(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    conversation_id: Uuid,
    reason: &'static str,
) -> Result<(), AppError> {
    transition_run(transaction, run_id, RunState::Failed, Some(reason)).await?;
    retain_run_workspace(transaction, run_id).await?;
    let event_id = insert_significant_event(
        transaction,
        run_id,
        "run_failed",
        reason,
        serde_json::json!({}),
    )
    .await?;
    insert_event_entry(transaction, conversation_id, event_id).await?;
    enqueue_run_updated_for_id(transaction, run_id).await?;
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApprovalResponse {
    pub(crate) decision: Option<String>,
    pub(crate) answer: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct CommandReference {
    resource_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyCommand {}

async fn respond_to_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(approval_id): ApiPath<Uuid>,
    ApiJson(response): ApiJson<ApprovalResponse>,
) -> Result<Json<ApprovalView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let external_request_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT external_execution_request_id FROM agent_approvals WHERE id = $1",
    )
    .bind(approval_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    if let Some(external_request_id) = external_request_id {
        return crate::external_execution::respond_to_sponsored_execution(
            &state,
            &headers,
            &principal,
            approval_id,
            external_request_id,
            response,
        )
        .await
        .map(Json);
    }
    let request_hash = RequestHash::for_value(&response)?;

    #[derive(FromRow)]
    struct ApprovalContext {
        run_id: Uuid,
        conversation_id: Uuid,
        project_id: Uuid,
        repository_id: Uuid,
        requesting_actor_id: Uuid,
        eligible_role: String,
        human_required: bool,
        kind: String,
        membership_role: String,
        membership_enabled: bool,
        status: String,
        responded_by_actor_id: Option<Uuid>,
        current_response: Option<serde_json::Value>,
    }

    let mut transaction = state.pool.begin().await?;
    let context = sqlx::query_as::<_, ApprovalContext>(
        r#"
        SELECT
            aa.agent_run_id AS run_id,
            ar.conversation_id,
            c.project_id,
            ar.repository_id,
            aa.requesting_actor_id,
            aa.eligible_role,
            aa.human_required,
            aa.kind,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled,
            aa.status,
            aa.responded_by_actor_id,
            aa.response AS current_response
        FROM agent_approvals aa
        JOIN agent_runs ar ON ar.id = aa.agent_run_id
        JOIN conversations c ON c.id = ar.conversation_id
        JOIN projects p ON p.id = c.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $2
        WHERE aa.id = $1
        FOR UPDATE OF aa
        FOR SHARE OF pm
        "#,
    )
    .bind(approval_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))?;
    let approval_eligibility = if context.eligible_role == ANY_AUTHORIZED_APPROVER {
        ApprovalEligibility::AnyAuthorizedMember
    } else {
        ApprovalEligibility::Role(parse_membership_role(&context.eligible_role)?)
    };
    authorize_project_action(
        &principal,
        context.project_id,
        &context.membership_role,
        context.membership_enabled,
        Action::ApprovalRespond,
        Resource::Approval {
            scope: ProjectScope::approval(
                context.project_id,
                context.conversation_id,
                context.run_id,
                context.repository_id,
                approval_id,
            ),
            requirements: ApprovalRequirements {
                requesting_actor_id: context.requesting_actor_id,
                eligibility: approval_eligibility,
                human_required: context.human_required,
                separation_of_duty: false,
            },
        },
    )?;

    let (response_json, approved) = match context.kind.as_str() {
        "approval" => {
            if response.answer.is_some()
                || !matches!(response.decision.as_deref(), Some("approve" | "deny"))
            {
                return Err(AppError::Invalid("unknown approval decision"));
            }
            let decision = response
                .decision
                .as_deref()
                .ok_or(AppError::Invalid("approval decision is required"))?;
            (
                serde_json::json!({"decision": decision}),
                decision == "approve",
            )
        }
        "question" => {
            if response.decision.is_some() {
                return Err(AppError::Invalid(
                    "question answer cannot include a decision",
                ));
            }
            let answer = response
                .answer
                .as_deref()
                .map(str::trim)
                .filter(|answer| {
                    (1..=4_000).contains(&answer.chars().count())
                        && !answer.chars().any(|character| {
                            character.is_control() && character != '\n' && character != '\t'
                        })
                })
                .ok_or(AppError::Invalid("question answer is invalid"))?;
            (serde_json::json!({"answer": answer}), true)
        }
        _ => return Err(AppError::Internal("unknown approval kind")),
    };

    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = idempotency::resource_bound_scope("approval", approval_id, "respond")?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let reference: CommandReference = replay.decode()?;
            if reference.resource_id != approval_id {
                return Err(AppError::Internal(
                    "approval idempotency reference mismatch",
                ));
            }
            transaction.commit().await?;
            return fetch_approval(&state, approval_id, context.project_id)
                .await
                .map(Json);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };

    if context.status == "resolved" {
        if context.responded_by_actor_id == Some(principal.actor_id)
            && context.current_response == Some(response_json)
        {
            idempotency::complete(
                &mut transaction,
                &reservation,
                StatusCode::OK,
                &CommandReference {
                    resource_id: approval_id,
                },
            )
            .await?;
            transaction.commit().await?;
            return fetch_approval(&state, approval_id, context.project_id)
                .await
                .map(Json);
        }
        return Err(AppError::Conflict(
            "agent_approval",
            "approval already resolved",
        ));
    }

    sqlx::query(
        r#"
        UPDATE agent_approvals
           SET status = 'resolved',
               responded_by_actor_id = $2,
               response = $3,
               responded_at = now()
         WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(approval_id)
    .bind(principal.actor_id)
    .bind(&response_json)
    .execute(&mut *transaction)
    .await?;
    let resolved_event = insert_significant_event(
        &mut transaction,
        context.run_id,
        if context.kind == "question" {
            "question_resolved"
        } else {
            "approval_resolved"
        },
        if context.kind == "question" {
            "The question was answered."
        } else {
            "The approval was resolved."
        },
        serde_json::json!({
            "approval_id": approval_id,
            "resolution": if context.kind == "question" { "answered" } else if approved { "approved" } else { "denied" },
            "responded_by_actor_id": principal.actor_id
        }),
    )
    .await?;
    insert_event_entry(&mut transaction, context.conversation_id, resolved_event).await?;

    if approved {
        transition_run(&mut transaction, context.run_id, RunState::Working, None).await?;
    } else {
        transition_run(
            &mut transaction,
            context.run_id,
            RunState::Interrupted,
            Some("The requested approval was denied."),
        )
        .await?;
        retain_run_workspace(&mut transaction, context.run_id).await?;
        crate::provider::idle_provider_session_for_terminal_run(&mut transaction, context.run_id)
            .await?;
        let interrupted_event = insert_significant_event(
            &mut transaction,
            context.run_id,
            "run_interrupted",
            "The run was interrupted after the approval was denied.",
            serde_json::json!({}),
        )
        .await?;
        insert_event_entry(&mut transaction, context.conversation_id, interrupted_event).await?;
    }
    crate::external_events::enqueue_external_approval_resolved(&mut transaction, approval_id)
        .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        context.project_id,
        context.conversation_id,
        context.run_id,
    )
    .await?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &CommandReference {
            resource_id: approval_id,
        },
    )
    .await?;
    transaction.commit().await?;
    fetch_approval(&state, approval_id, context.project_id)
        .await
        .map(Json)
}

async fn interrupt_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;

    #[derive(FromRow)]
    struct RunContext {
        conversation_id: Uuid,
        project_id: Uuid,
        repository_id: Uuid,
        membership_role: String,
        membership_enabled: bool,
        state: String,
    }

    let mut transaction = state.pool.begin().await?;
    let context = sqlx::query_as::<_, RunContext>(
        r#"
        SELECT
            ar.conversation_id,
            c.project_id,
            ar.repository_id,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled,
            ar.state
        FROM agent_runs ar
        JOIN conversations c ON c.id = ar.conversation_id
        JOIN projects p ON p.id = c.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $2
        WHERE ar.id = $1
        FOR UPDATE OF ar
        FOR SHARE OF pm
        "#,
    )
    .bind(run_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    authorize_project_action(
        &principal,
        context.project_id,
        &context.membership_role,
        context.membership_enabled,
        Action::RunInterrupt,
        Resource::Project(ProjectScope::run(
            context.project_id,
            context.conversation_id,
            run_id,
            context.repository_id,
        )),
    )?;

    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&EmptyCommand {})?;
    let scope = idempotency::resource_bound_scope("agent_run", run_id, "interrupt")?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let reference: CommandReference = replay.decode()?;
            if reference.resource_id != run_id {
                return Err(AppError::Internal(
                    "interrupt idempotency reference mismatch",
                ));
            }
            transaction.commit().await?;
            return fetch_run(&state, run_id, context.project_id)
                .await
                .map(Json);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    if matches!(
        context.state.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        if context.state != "interrupted" {
            return Err(AppError::Conflict(
                "agent_run",
                "only a non-terminal run can be interrupted",
            ));
        }
        idempotency::complete(
            &mut transaction,
            &reservation,
            StatusCode::OK,
            &CommandReference {
                resource_id: run_id,
            },
        )
        .await?;
        transaction.commit().await?;
        return fetch_run(&state, run_id, context.project_id)
            .await
            .map(Json);
    }

    transition_run(
        &mut transaction,
        run_id,
        RunState::Interrupted,
        Some("Interrupted by the requesting Actor."),
    )
    .await?;
    retain_run_workspace(&mut transaction, run_id).await?;
    crate::provider::idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
    let event_id = insert_significant_event(
        &mut transaction,
        run_id,
        "run_interrupted",
        "The run was interrupted.",
        serde_json::json!({"actor_id": principal.actor_id}),
    )
    .await?;
    insert_event_entry(&mut transaction, context.conversation_id, event_id).await?;
    sqlx::query(
        "UPDATE agent_approvals SET status = 'canceled' WHERE agent_run_id = $1 AND status = 'pending'",
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        context.project_id,
        context.conversation_id,
        run_id,
    )
    .await?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &CommandReference {
            resource_id: run_id,
        },
    )
    .await?;
    transaction.commit().await?;
    fetch_run(&state, run_id, context.project_id)
        .await
        .map(Json)
}

pub(crate) async fn insert_significant_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    kind: &str,
    summary: &str,
    payload: serde_json::Value,
) -> Result<Uuid, AppError> {
    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(event_id)
    .bind(run_id)
    .bind(format!("{kind}:{event_id}"))
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    Ok(event_id)
}

pub(crate) async fn retain_run_workspace(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn insert_event_entry(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation_id: Uuid,
    event_id: Uuid,
) -> Result<(), AppError> {
    let sequence = allocate_conversation_sequence(transaction, conversation_id).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, agent_run_event_id
        )
        VALUES ($1, $2, $3, 'worker_event', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(conversation_id)
    .bind(sequence)
    .bind(event_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn fetch_run(state: &AppState, run_id: Uuid, project_id: Uuid) -> Result<RunView, AppError> {
    let run = sqlx::query_as::<_, RunRow>(
        r#"
        SELECT
            ar.id,
            ar.state,
            ar.repository_id,
            ar.repository_workspace_id,
            ar.provider_session_id,
            ar.provider_turn_id,
            workspace.branch_name AS branch,
            ar.config,
            workspace.base_commit,
            workspace.published_commit,
            workspace.published_at::text AS published_at,
            workspace.publish_failure_category,
            pull_request_request.state AS pull_request_request_state,
            pull_request_request.failure_category AS pull_request_failure_category,
            pull_request.pull_request_number,
            pull_request.id AS pull_request_id,
            pull_request.html_url AS pull_request_url,
            pull_request.title AS pull_request_title,
            pull_request.state AS pull_request_state,
            pull_request.draft AS pull_request_draft,
            pull_request.checks_state AS pull_request_checks_state,
            pull_request.review_state AS pull_request_review_state,
            pull_request.mergeability_state AS pull_request_mergeability_state,
            pull_request.sync_state AS pull_request_sync_state,
            pull_request.sync_failure_category AS pull_request_sync_failure_category,
            pull_request.automation_state AS pull_request_automation_state,
            pull_request.automation_reason AS pull_request_automation_reason,
            CASE WHEN ar.state = 'queued' THEN (
                SELECT count(*) FROM agent_runs queued
                WHERE queued.state = 'queued'
                  AND (queued.created_at, queued.id) <= (ar.created_at, ar.id)
            ) END AS queue_position,
            ar.failure_reason,
            ar.created_at::text AS created_at,
            ar.updated_at::text AS updated_at
        FROM agent_runs ar
        JOIN conversations c
          ON c.id = ar.conversation_id
         AND c.project_id = $2
        LEFT JOIN repository_workspaces workspace
          ON workspace.id = ar.repository_workspace_id
        LEFT JOIN github_pull_request_requests pull_request_request
          ON pull_request_request.agent_run_id = ar.id
        LEFT JOIN github_pull_requests pull_request
          ON pull_request.id = pull_request_request.github_pull_request_id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    Ok(RunView {
        run,
        failure_acknowledged: false,
        capabilities: RunCapabilities {
            interrupt: false,
            retry: false,
            publish: false,
            create_pull_request: false,
            refresh_pull_request: false,
        },
    })
}

pub(crate) async fn fetch_approval(
    state: &AppState,
    approval_id: Uuid,
    project_id: Uuid,
) -> Result<ApprovalView, AppError> {
    let approval = sqlx::query_as::<_, ApprovalRow>(
        r#"
        SELECT
            aa.id,
            aa.agent_run_id,
            aa.kind,
            aa.summary,
            aa.options,
            aa.status,
            aa.response,
            aa.created_at::text AS created_at,
            ar.repository_id,
            aa.requesting_actor_id,
            aa.eligible_role,
            aa.human_required
        FROM agent_approvals aa
        JOIN agent_runs ar ON ar.id = aa.agent_run_id
        JOIN conversations c
          ON c.id = ar.conversation_id
         AND c.project_id = $2
        WHERE aa.id = $1
        "#,
    )
    .bind(approval_id)
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))?;
    Ok(ApprovalView {
        approval,
        capabilities: ApprovalCapabilities { respond: false },
    })
}

async fn guard_conversation_mutation(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &impl RunActor,
    conversation_id: Uuid,
    project_id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    #[derive(FromRow)]
    struct MutationGuard {
        kind: String,
        conversation_archived: bool,
        project_archived: bool,
        membership_role: String,
        membership_enabled: bool,
    }

    // Match conversation writes: project first, then parent, then thread.
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR SHARE")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    let parent_channel_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT parent_channel_id FROM conversations WHERE id = $1 AND project_id = $2 AND conversation_visible_to(id, $3)",
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(principal.actor_id())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    let parent_archived = if let Some(parent_channel_id) = parent_channel_id {
        !sqlx::query_scalar::<_, bool>(
            r#"
            SELECT archived_at IS NULL
            FROM conversations
            WHERE id = $1 AND project_id = $2 AND kind = 'channel'
            FOR SHARE
            "#,
        )
        .bind(parent_channel_id)
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("conversation"))?
    } else {
        false
    };

    let guard = sqlx::query_as::<_, MutationGuard>(
        r#"
        SELECT
            c.kind,
            c.archived_at IS NOT NULL AS conversation_archived,
            p.archived_at IS NOT NULL AS project_archived,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled
        FROM conversations c
        JOIN projects p
          ON p.id = c.project_id
         AND p.id = $2
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $3
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $3
         AND pm.enabled
        WHERE c.id = $1
          AND c.project_id = $2
          AND conversation_visible_to(c.id, $3)
        FOR UPDATE OF c
        FOR SHARE OF p, pm
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(principal.actor_id())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    let scope = if guard.kind == "thread" {
        ProjectScope::thread(project_id, conversation_id)
    } else {
        ProjectScope::project(project_id)
    };
    authorize_project_action(
        principal,
        project_id,
        &guard.membership_role,
        guard.membership_enabled,
        action,
        Resource::Project(scope),
    )?;
    if guard.project_archived {
        return Err(AppError::ProjectArchived);
    }
    // A visibility predicate evaluated before waiting on a lock can have an
    // older snapshot. Check again while holding the parent and thread locks.
    if !sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
        .bind(conversation_id)
        .bind(principal.actor_id())
        .fetch_one(&mut **transaction)
        .await?
    {
        return Err(AppError::NotFound("conversation"));
    }
    if guard.conversation_archived || parent_archived {
        return Err(AppError::NotFound("conversation"));
    }
    Ok(())
}

async fn authorized_conversation(
    state: &AppState,
    principal: &impl RunActor,
    conversation_id: Uuid,
    action: Action,
) -> Result<ConversationRow, AppError> {
    let conversation = sqlx::query_as::<_, ConversationRow>(
        r#"
        SELECT
            c.id,
            c.project_id,
            c.kind,
            c.parent_channel_id,
            c.name,
            (c.archived_at IS NOT NULL OR parent.archived_at IS NOT NULL)
                AS conversation_archived,
            p.archived_at IS NOT NULL AS project_archived,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled
        FROM conversations c
        LEFT JOIN conversations parent
          ON parent.project_id = c.project_id
         AND parent.id = c.parent_channel_id
        JOIN projects p ON p.id = c.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $2
         AND pm.enabled
        WHERE c.id = $1
          AND conversation_visible_to(c.id, $2)
        "#,
    )
    .bind(conversation_id)
    .bind(principal.actor_id())
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    let scope = if conversation.kind == "thread" {
        ProjectScope::thread(conversation.project_id, conversation.id)
    } else {
        ProjectScope::project(conversation.project_id)
    };
    authorize_project_action(
        principal,
        conversation.project_id,
        &conversation.membership_role,
        conversation.membership_enabled,
        action,
        Resource::Project(scope),
    )?;
    Ok(conversation)
}

fn authorize_project_action(
    principal: &impl RunActor,
    project_id: Uuid,
    membership_role: &str,
    membership_enabled: bool,
    action: Action,
    resource: Resource,
) -> Result<(), AppError> {
    let subject = Subject::new(
        principal.actor_id(),
        principal.actor_kind()?,
        true,
        Some(Membership::project(
            project_id,
            parse_membership_role(membership_role)?,
            membership_enabled,
        )),
        None,
    );
    match authorize(&subject, action, resource) {
        Decision::Allow => Ok(()),
        Decision::Deny(_) => Err(AppError::Forbidden),
    }
}

/// Whoever starts or changes a run: a browser session or an MCP caller.
pub(crate) trait RunActor {
    fn actor_id(&self) -> Uuid;
    fn actor_kind(&self) -> Result<ActorKind, AppError>;
    fn is_mcp(&self) -> bool {
        false
    }
}

impl RunActor for AuthenticatedActor {
    fn actor_id(&self) -> Uuid {
        self.actor_id
    }
    fn actor_kind(&self) -> Result<ActorKind, AppError> {
        parse_actor_kind(&self.actor_kind)
    }
}

impl RunActor for crate::runs::RunCaller {
    fn actor_id(&self) -> Uuid {
        self.actor_id
    }
    fn actor_kind(&self) -> Result<ActorKind, AppError> {
        Ok(self.actor_kind)
    }
    fn is_mcp(&self) -> bool {
        self.is_mcp()
    }
}

fn parse_actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("unknown Actor kind")),
    }
}

fn parse_membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("unknown Project membership role")),
    }
}

#[cfg(any())]
async fn enqueue_run_updated_for_id(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    let (project_id, conversation_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT conversation.project_id, run.conversation_id
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(&mut **transaction)
    .await?;
    outbox::enqueue_agent_run_updated(transaction, project_id, conversation_id, run_id).await?;
    Ok(())
}
