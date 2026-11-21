//! The built-in host's run routes and the cores the browser, MCP and
//! automations share: start, retry, stop and dismissing a failure (A1.14
//! moved them here from `api.rs`).

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    domain::{RunState, allocate_conversation_sequence},
    error::{ApiJson, ApiPath, AppError},
    harness::run_config::RunConfig,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    message_content::MessageContent,
    outbox,
    policy::{Action, ActorKind, RoleGrant},
};

#[derive(Debug, FromRow)]
struct ConversationRow {
    project_id: Uuid,
    kind: String,
    conversation_archived: bool,
    project_archived: bool,
    membership_role: String,
    membership_role_permissions: Option<Vec<String>>,
    membership_group_permissions: Option<Vec<String>>,
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
    /// The agent profile (A9) that does the work and posts its replies;
    /// left out, the project's default agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_profile_id: Option<Uuid>,
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

pub(crate) async fn start_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<StartRunRequest>,
) -> Result<(StatusCode, Json<StartRunResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
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
    // K4.17: a DM's thread lives in no space, so it has no repositories or
    // run settings to start a run with; runs start in a space's threads.
    if crate::spaces::kind::holds_messages(conversation.project_id) {
        return Err(AppError::Invalid("agent runs start in a space's threads"));
    }

    let idempotency_key = idempotency_key?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = idempotency::resource_bound_scope("thread", conversation_id, "agent_run_start")?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    guard_conversation_mutation(
        &mut transaction,
        principal,
        conversation_id,
        conversation.project_id,
        Action::WorkerInvoke,
    )
    .await?;
    // H1.24: no run starts in a suspended or deleting workspace, and a
    // suspension waits for this one to commit.
    crate::workspace::lifecycle::hold_open_for_space(
        &mut transaction,
        conversation.project_id,
        "run_start",
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
    // Through an app, the per-person cap applies (one rule for every start).
    crate::hosts::builtin::admit(
        &mut transaction,
        conversation_id,
        principal.is_mcp().then(|| principal.actor_id()),
    )
    .await?;
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
    let worker_id = match request.agent_profile_id {
        Some(profile_id) => Some(
            crate::agent_profiles::profile_worker(
                &mut transaction,
                conversation.project_id,
                profile_id,
            )
            .await?,
        ),
        None => None,
    };
    let setup = loop {
        match select_run_setup_for(
            &mut transaction,
            conversation.project_id,
            conversation_id,
            principal.actor_id(),
            repository_id,
            harness.as_deref(),
            request.provider_account_id,
            worker_id,
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
    let config =
        crate::hosts::builtin::resolve_config_with(state, &defaults, &setup, &launch).await?;
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

    let created = crate::hosts::builtin::create(
        &mut transaction,
        crate::hosts::builtin::NewRun {
            project_id: conversation.project_id,
            conversation_id,
            setup: &setup,
            requested_by: principal.actor_id(),
            sponsored_by: None,
            external_execution_request_id: None,
            predecessor_run_id: None,
            prompt: &prompt,
            prompt_message_id: Some(prompt_message_id),
            authorization: serde_json::json!({
                "actor_id": principal.actor_id(),
                "project_id": conversation.project_id,
                "scope": ["repository.read", "repository.write", "provider.turn"]
            }),
            config: &config,
            // A person's app or script is in this call when its agent limit
            // is set: if the run waits in the queue, it starts only within
            // that limit as it is then (`scheduler::requester_refusal`).
            made_through_app: made_through_app(principal, conversation.project_id),
        },
    )
    .await?;
    let (run_id, queue_position) = (created.run_id, created.queue_position);
    outbox::enqueue_message_created(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        prompt_message_id,
        prompt_sequence,
        principal.actor_id(),
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

pub(crate) async fn retry_agent_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(predecessor_run_id): ApiPath<Uuid>,
    ApiJson(_request): ApiJson<EmptyCommand>,
) -> Result<(StatusCode, Json<StartRunResponse>), AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let (status, response) = retry_run_in(
        &state,
        &principal,
        predecessor_run_id,
        IdempotencyKey::from_headers(&headers),
    )
    .await?;
    Ok((status, Json(response)))
}

/// Start a terminal run again with the same prompt, repository and
/// settings, as `principal`: the browser's Retry and MCP's run_retry.
pub(crate) async fn retry_run_in(
    state: &AppState,
    principal: &impl RunActor,
    predecessor_run_id: Uuid,
    // Checked after authorization, as in `start_run_in`.
    idempotency_key: Result<IdempotencyKey, AppError>,
) -> Result<(StatusCode, StartRunResponse), AppError> {
    let conversation_id =
        sqlx::query_scalar::<_, Uuid>("SELECT conversation_id FROM agent_runs WHERE id = $1")
            .bind(predecessor_run_id)
            .fetch_optional(crate::db::scoped(&state.pool))
            .await?
            .ok_or(AppError::NotFound("agent_run"))?;
    let conversation =
        authorized_conversation(state, principal, conversation_id, Action::WorkerInvoke).await?;
    if conversation.project_archived {
        return Err(AppError::ProjectArchived);
    }
    if conversation.conversation_archived {
        return Err(AppError::NotFound("conversation"));
    }

    let idempotency_key = idempotency_key?;
    let request_hash = RequestHash::for_value(&EmptyCommand {})?;
    let scope = idempotency::resource_bound_scope("agent_run", predecessor_run_id, "retry")?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    guard_conversation_mutation(
        &mut transaction,
        principal,
        conversation_id,
        conversation.project_id,
        Action::WorkerInvoke,
    )
    .await?;
    // H1.24: no run starts in a suspended or deleting workspace, and a
    // suspension waits for this one to commit.
    crate::workspace::lifecycle::hold_open_for_space(
        &mut transaction,
        conversation.project_id,
        "run_start",
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
    let (
        repository_id,
        prompt,
        predecessor_state,
        prompt_message_id,
        predecessor_config,
        predecessor_worker_id,
    ) = sqlx::query_as::<_, (Uuid, String, String, Option<Uuid>, serde_json::Value, Uuid)>(
        r#"
            SELECT repository_id, prompt, state, prompt_message_id, config, execution_worker_id
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
    // Through an app, a retry counts toward the same cap as a start.
    crate::hosts::builtin::admit(
        &mut transaction,
        conversation_id,
        principal.is_mcp().then(|| principal.actor_id()),
    )
    .await?;
    // A retry asks for the same settings again; whatever the account or the
    // catalog no longer offers falls back on its own, and a sandbox or
    // approval policy that is gone refuses the retry rather than loosening.
    let predecessor_config = RunConfig::from_value(&predecessor_config);
    let setup = select_run_setup_for(
        &mut transaction,
        conversation.project_id,
        conversation_id,
        principal.actor_id(),
        Some(repository_id),
        predecessor_config.harness.as_deref(),
        None,
        // A retry stays with the agent that ran it (a profile, A9).
        Some(predecessor_worker_id),
    )
    .await?;
    let config = crate::model_catalog::resolve_stored_run_config(
        state,
        setup.organization_id,
        &setup.adapter_kind,
        &setup.run_defaults,
        &predecessor_config,
    )
    .await?;
    let created = crate::hosts::builtin::create(
        &mut transaction,
        crate::hosts::builtin::NewRun {
            project_id: conversation.project_id,
            conversation_id,
            setup: &setup,
            requested_by: principal.actor_id(),
            sponsored_by: None,
            external_execution_request_id: None,
            predecessor_run_id: Some(predecessor_run_id),
            prompt: &prompt,
            prompt_message_id,
            authorization: serde_json::json!({
                "actor_id": principal.actor_id(),
                "project_id": conversation.project_id,
                "scope": ["repository.read", "repository.write", "provider.turn"],
                "retry_of": predecessor_run_id
            }),
            config: &config,
            // As for a start: through a person's app, a queued retry starts
            // only within that app's agent limit as it is then.
            made_through_app: made_through_app(principal, conversation.project_id),
        },
    )
    .await?;
    let (run_id, queue_position) = (created.run_id, created.queue_position);
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
    Ok((StatusCode::ACCEPTED, response))
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
        None,
    )
    .await
}

/// `select_run_setup` limited to profiles on one harness when `harness` is
/// set. Among the owner's signed-in profiles the preferred one comes first.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn select_run_setup_for(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    conversation_id: Uuid,
    profile_owner_actor_id: Uuid,
    explicit_repository_id: Option<Uuid>,
    harness: Option<&str>,
    provider_account_id: Option<Uuid>,
    // One execution worker (an agent profile's, A9); left out, the
    // project's default agent, never a profile's.
    worker_id: Option<Uuid>,
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
        "SELECT count(*) FROM task_threads WHERE project_id = $1 AND conversation_id = $2",
    )
    .bind(project_id)
    .bind(conversation_id)
    .fetch_one(&mut **transaction)
    .await?;
    let linked_primary_repository_id = if linked_work_item_count == 1 {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT repository.id
            FROM task_threads conversation_link
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
         AND CASE WHEN $6::uuid IS NULL
                  THEN NOT EXISTS (
                      SELECT 1 FROM agent_profiles agent_profile
                      WHERE agent_profile.execution_worker_id = ew.id
                  )
                  ELSE ew.id = $6 END
        JOIN project_memberships worker_membership
          ON worker_membership.project_id = ew.project_id
         AND worker_membership.actor_id = ew.actor_id
         AND worker_membership.role = $7
         AND worker_membership.enabled
        JOIN actors worker_actor
          ON worker_actor.id = ew.actor_id
         AND worker_actor.kind = 'execution_worker' -- // kind-check: mechanical (the execution worker's own account)
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
    .bind(worker_id)
    .bind(crate::policy::MembershipRole::Worker.key())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::ExecutionProfileRequired)
}

#[derive(Clone, Deserialize, Serialize)]
struct CommandReference {
    resource_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmptyCommand {}

/// Stop a run that hasn't finished, as `principal`: the browser's Stop and
/// MCP's run_stop. Returns the run's project. Stopping one already stopped
/// changes nothing; a finished one is a conflict.
pub(crate) async fn interrupt_run_in(
    state: &AppState,
    principal: &impl RunActor,
    run_id: Uuid,
    idempotency_key: Result<IdempotencyKey, AppError>,
) -> Result<Uuid, AppError> {
    #[derive(FromRow)]
    struct RunContext {
        conversation_id: Uuid,
        project_id: Uuid,
        membership_role: String,
        membership_role_permissions: Option<Vec<String>>,
        membership_group_permissions: Option<Vec<String>>,
        membership_enabled: bool,
        state: String,
    }

    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let context = sqlx::query_as::<_, RunContext>(
        r#"
        SELECT
            ar.conversation_id,
            c.project_id,
            pm.role AS membership_role,
            workspace_role_permissions(pm.role_id) AS membership_role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS membership_group_permissions,
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
          AND conversation_visible_to(ar.conversation_id, $2)
        FOR UPDATE OF ar
        FOR SHARE OF pm
        "#,
    )
    .bind(run_id)
    .bind(principal.actor_id())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;
    authorize_project_action(
        principal,
        context.project_id,
        &context.membership_role,
        context.membership_role_permissions.as_deref(),
        context.membership_group_permissions.as_deref(),
        context.membership_enabled,
        Action::RunInterrupt,
    )?;

    let idempotency_key = idempotency_key?;
    let request_hash = RequestHash::for_value(&EmptyCommand {})?;
    let scope = idempotency::resource_bound_scope("agent_run", run_id, "interrupt")?;
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
            let reference: CommandReference = replay.decode()?;
            if reference.resource_id != run_id {
                return Err(AppError::Internal(
                    "interrupt idempotency reference mismatch",
                ));
            }
            transaction.commit().await?;
            return Ok(context.project_id);
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
        return Ok(context.project_id);
    }

    // A1.16: a run the built-in host has started is stopped by its host,
    // as any host's is: the run's record asks it to (cancel_requested) and
    // the host hears a cancel command, stops the turn and confirms. One
    // still waiting to start is stopped here at once; no host has it yet.
    let started = !matches!(context.state.as_str(), "created" | "queued");
    if started {
        crate::runs::ops::request_cancel(&mut transaction, principal.actor_id(), run_id, None)
            .await
            .map_err(|error| match error {
                crate::runs::ops::RunOpError::App(error) => error,
                _ => AppError::Conflict("agent_run", "only a non-terminal run can be interrupted"),
            })?;
    } else {
        interrupt_run_unchecked(
            &mut transaction,
            RunToStop {
                run_id,
                project_id: context.project_id,
                conversation_id: context.conversation_id,
            },
            crate::runs::state::STOPPED_BY_REQUESTER,
            serde_json::json!({"actor_id": principal.actor_id()}),
        )
        .await?;
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
    if started && state.config.builtin_host && state.config.builtin_host_remote.is_none() {
        // The built-in host hears the cancel now, not at its next tick,
        // when it runs in this process. Otherwise (its own program, or
        // another instance) its next look does.
        if let Err(error) = crate::hosts::builtin::driver::hear_now(
            state,
            run_id,
            &[agent_workspace_host_protocol::CommandKind::Cancel],
        )
        .await
        {
            tracing::warn!(%run_id, error_category = error.log_category(), "the stop waits for the host's next look");
        }
    }
    Ok(context.project_id)
}

/// A run to stop, with where it lives.
#[derive(Clone, Copy, Debug, FromRow)]
pub(crate) struct RunToStop {
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
}

/// Interrupt a run that hasn't finished: its state (which its driver
/// watches, and stops the harness's turn on), its workspace kept, its
/// open requests withdrawn and open screens told. The caller has
/// decided it may, and holds the run's row.
async fn interrupt_run_unchecked(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run: RunToStop,
    reason: &str,
    payload: serde_json::Value,
) -> Result<(), AppError> {
    let run_id = run.run_id;
    crate::hosts::builtin::transition(transaction, run_id, RunState::Interrupted, Some(reason))
        .await?;
    retain_run_workspace(transaction, run_id).await?;
    crate::provider::idle_provider_session_for_terminal_run(transaction, run_id).await?;
    let event_id = crate::runs::steps::record_new(
        transaction,
        run_id,
        "run_interrupted",
        "The run was interrupted.",
        payload,
    )
    .await?;
    crate::runs::steps::show_in_thread(transaction, run.conversation_id, event_id).await?;
    outbox::enqueue_agent_run_updated(transaction, run.project_id, run.conversation_id, run_id)
        .await?;
    Ok(())
}

/// Interrupt every run in `workspace` that hasn't finished (H1.24): a
/// suspended workspace, or one being deleted, runs nothing. Inside the
/// caller's transaction, which must not hold the workspace's row: stopping
/// a run writes to its thread, which run starts lock before the workspace.
pub(crate) async fn interrupt_workspace_runs(
    connection: &mut sqlx::PgConnection,
    workspace: Uuid,
    why: &'static str,
) -> Result<usize, AppError> {
    use sqlx::Connection;
    let mut transaction = connection.begin().await?;
    let runs = crate::runs::active_in_workspace(&mut transaction, workspace).await?;
    for run in &runs {
        interrupt_run_unchecked(
            &mut transaction,
            *run,
            "Stopped because the workspace was suspended or is being deleted.",
            serde_json::json!({ "reason": why }),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(runs.len())
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

async fn guard_conversation_mutation(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &impl RunActor,
    conversation_id: Uuid,
    project_id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    #[derive(FromRow)]
    struct MutationGuard {
        conversation_archived: bool,
        project_archived: bool,
        membership_role: String,
        membership_role_permissions: Option<Vec<String>>,
        membership_group_permissions: Option<Vec<String>>,
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
            workspace_role_permissions(pm.role_id) AS membership_role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS membership_group_permissions,
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
    authorize_project_action(
        principal,
        project_id,
        &guard.membership_role,
        guard.membership_role_permissions.as_deref(),
        guard.membership_group_permissions.as_deref(),
        guard.membership_enabled,
        action,
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
            workspace_role_permissions(pm.role_id) AS membership_role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS membership_group_permissions,
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
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    authorize_project_action(
        principal,
        conversation.project_id,
        &conversation.membership_role,
        conversation.membership_role_permissions.as_deref(),
        conversation.membership_group_permissions.as_deref(),
        conversation.membership_enabled,
        action,
    )?;
    Ok(conversation)
}

pub(crate) fn authorize_project_action(
    principal: &impl RunActor,
    project_id: Uuid,
    membership_role: &str,
    membership_role_permissions: Option<&[String]>,
    membership_group_permissions: Option<&[String]>,
    membership_enabled: bool,
    action: Action,
) -> Result<(), AppError> {
    let grant = RoleGrant::stored(membership_role, membership_role_permissions)?.with_groups(
        crate::access::group_permissions(membership_group_permissions),
    );
    if crate::access::membership_allows(
        principal.actor_id(),
        project_id,
        grant,
        membership_enabled,
        action.permission(),
    ) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Whoever starts or changes a run: a browser session or an MCP caller.
/// Whether a person's app or script is in this call: through MCP, or under
/// an agent limit set for this request (`scheduler::requester_refusal`).
fn made_through_app(principal: &impl RunActor, project_id: Uuid) -> bool {
    matches!(principal.actor_kind(), Ok(ActorKind::Human))
        && (principal.made_through_app()
            || crate::policy::current_agent_limit(
                principal.actor_id(),
                crate::policy::MembershipScope::Project(project_id),
            )
            .is_some())
}

pub(crate) trait RunActor {
    fn actor_id(&self) -> Uuid;
    fn actor_kind(&self) -> Result<ActorKind, AppError>;
    fn is_mcp(&self) -> bool {
        false
    }
    /// Started for something a person's app set going earlier (a queued
    /// launch), outside the app's own call. A run started inside such a
    /// call is recognized by its agent limit (`start_run_in`).
    fn made_through_app(&self) -> bool {
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

pub(crate) fn parse_actor_kind(value: &str) -> Result<ActorKind, AppError> {
    crate::accounts::actor_kind(value)
}
