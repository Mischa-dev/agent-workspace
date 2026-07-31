use std::{fs::OpenOptions, io::Write, path::PathBuf};

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{
        ApprovalCapabilities, ApprovalRow, ApprovalView, RunCapabilities, RunRow, RunView,
    },
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::{ApiJson, ApiPath, AppError},
    git_runtime,
    message_content::MessageContent,
    outbox,
    policy::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, Membership,
        MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::auth::router())
        .merge(crate::projects::router())
        .merge(crate::execution::router())
        .route("/api/v1/dev/bootstrap", get(development_bootstrap))
        .route("/api/v1/threads/{id}/agent-runs", post(start_agent_run))
        .route(
            "/api/v1/agent-runs/{id}/interrupt",
            post(interrupt_agent_run),
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunRequest {
    prompt: String,
}

#[derive(Debug, FromRow)]
struct RunSetup {
    project_key: String,
    repository_id: Uuid,
    mirror_path: String,
    default_branch: String,
    execution_worker_id: Uuid,
    execution_profile_id: Uuid,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRunResponse {
    run_id: Uuid,
    state: RunState,
    repository_workspace_id: Uuid,
    branch: String,
    base_commit: String,
    worktree_path: String,
}

const WORKTREE_START_FAILURE_REASON: &str =
    "Worktree creation failed; inspect the retained repository workspace.";
const PROVIDER_START_FAILURE_REASON: &str =
    "Codex App Server could not start. The worktree was preserved.";

async fn start_agent_run(
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
    let worktree_path = data_root
        .join("worktrees")
        .join(setup.repository_id.to_string())
        .join(run_id.to_string());

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
            kind, summary, options
        )
        VALUES (
            $1, $2, $3, $4, 'approval',
            'Allow the development executor to append its proof line to KNOWN_FILE.txt?',
            $5
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

#[derive(Deserialize)]
struct ApprovalResponse {
    decision: String,
}

async fn respond_to_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(approval_id): ApiPath<Uuid>,
    ApiJson(response): ApiJson<ApprovalResponse>,
) -> Result<Json<ApprovalView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if !matches!(response.decision.as_str(), "approve" | "deny") {
        return Err(AppError::Invalid("unknown approval decision"));
    }

    #[derive(FromRow)]
    struct ApprovalContext {
        run_id: Uuid,
        conversation_id: Uuid,
        project_id: Uuid,
        repository_id: Uuid,
        requesting_actor_id: Uuid,
        eligible_role: String,
        human_required: bool,
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
            pm.role AS membership_role,
            pm.enabled AS membership_enabled,
            aa.status,
            aa.responded_by_actor_id,
            aa.response AS current_response
        FROM agent_approvals aa
        JOIN agent_runs ar ON ar.id = aa.agent_run_id
        JOIN conversations c ON c.id = ar.conversation_id
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
    let eligible_role = parse_membership_role(&context.eligible_role)?;
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
                eligibility: ApprovalEligibility::Role(eligible_role),
                human_required: context.human_required,
                separation_of_duty: false,
            },
        },
    )?;

    let response_json = serde_json::json!({"decision": response.decision});
    if context.status == "resolved" {
        if context.responded_by_actor_id == Some(principal.actor_id)
            && context.current_response == Some(response_json)
        {
            transaction.rollback().await?;
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
        "approval_resolved",
        "The approval was resolved.",
        serde_json::json!({
            "approval_id": approval_id,
            "decision": response.decision,
            "responded_by_actor_id": principal.actor_id
        }),
    )
    .await?;
    insert_event_entry(&mut transaction, context.conversation_id, resolved_event).await?;

    if response.decision == "approve" {
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
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        context.project_id,
        context.conversation_id,
        context.run_id,
    )
    .await?;
    transaction.commit().await?;

    if response.decision == "approve" && state.config.demo_executor {
        let background_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) =
                complete_demo_run(background_state, context.run_id, context.conversation_id).await
            {
                tracing::error!(
                    error_category = error.log_category(),
                    run_id = %context.run_id,
                    "development run completion failed"
                );
            }
        });
    }
    fetch_approval(&state, approval_id, context.project_id)
        .await
        .map(Json)
}

async fn complete_demo_run(
    state: AppState,
    run_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    #[derive(FromRow)]
    struct DemoRunContext {
        project_id: Uuid,
        worktree_path: String,
    }

    let context = sqlx::query_as::<_, DemoRunContext>(
        r#"
        SELECT c.project_id, gw.path AS worktree_path
        FROM agent_runs ar
        JOIN conversations c
          ON c.id = ar.conversation_id
         AND c.id = $2
        JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
        JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
        WHERE ar.id = $1 AND gw.state = 'ready'
        "#,
    )
    .bind(run_id)
    .bind(conversation_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Conflict(
        "git_worktree",
        "ready worktree not found",
    ))?;
    let worktree_path = context.worktree_path;

    let proof_worktree_path = worktree_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let known_file = PathBuf::from(proof_worktree_path).join("KNOWN_FILE.txt");
        let mut file = OpenOptions::new()
            .append(true)
            .open(known_file)
            .map_err(|_| AppError::Internal("open known proof file"))?;
        writeln!(file, "M0 walking skeleton browser proof.")
            .map_err(|_| AppError::Internal("write known proof file"))
    })
    .await
    .map_err(|_| AppError::Internal("join proof write task"))??;

    let dirty = tokio::task::spawn_blocking(move || {
        crate::git_runtime::worktree_is_dirty(PathBuf::from(worktree_path).as_path())
    })
    .await
    .map_err(|_| AppError::Internal("join worktree inspection task"))??;

    let worker_actor_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT ew.actor_id FROM agent_runs ar JOIN execution_workers ew ON ew.id = ar.execution_worker_id WHERE ar.id = $1",
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let mut transaction = state.pool.begin().await?;
    let file_event = insert_significant_event(
        &mut transaction,
        run_id,
        "file_change",
        "Updated KNOWN_FILE.txt in the isolated worktree.",
        serde_json::json!({"path": "KNOWN_FILE.txt", "operation": "modified"}),
    )
    .await?;
    insert_event_entry(&mut transaction, conversation_id, file_event).await?;
    sqlx::query(
        r#"
        UPDATE git_worktrees gw
        SET state = CASE WHEN $2 THEN 'dirty' ELSE 'ready' END,
            dirty = $2,
            last_inspected_at = now(),
            updated_at = now()
        FROM repository_workspaces rw, agent_runs ar
        WHERE ar.id = $1
          AND rw.id = ar.repository_workspace_id
          AND gw.repository_workspace_id = rw.id
        "#,
    )
    .bind(run_id)
    .bind(dirty)
    .execute(&mut *transaction)
    .await?;

    let message_id = Uuid::now_v7();
    let (message_content, message_text, message_mentions) = MessageContent::from_plain_text(
        "The development walking skeleton completed its isolated file change and preserved the worktree.",
    )
    .map_err(|_| AppError::Internal("validate development Worker Message"))?
    .into_parts();
    debug_assert!(message_mentions.is_empty());
    let message_content = serde_json::to_value(message_content)
        .map_err(|_| AppError::Internal("serialize development Worker Message"))?;
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text, agent_run_id
        )
        VALUES ($1, $2, $3, $4, 1, $5, $6, $7)
        "#,
    )
    .bind(message_id)
    .bind(context.project_id)
    .bind(conversation_id)
    .bind(worker_actor_id)
    .bind(message_content)
    .bind(&message_text)
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    let message_sequence =
        allocate_conversation_sequence(&mut transaction, conversation_id).await?;
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
    .bind(message_sequence)
    .bind(message_id)
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Completed, None).await?;
    retain_run_workspace(&mut transaction, run_id).await?;
    let completed_event = insert_significant_event(
        &mut transaction,
        run_id,
        "run_completed",
        "The run completed successfully.",
        serde_json::json!({}),
    )
    .await?;
    insert_event_entry(&mut transaction, conversation_id, completed_event).await?;
    outbox::enqueue_message_created(
        &mut transaction,
        context.project_id,
        conversation_id,
        message_id,
        message_sequence,
        worker_actor_id,
    )
    .await?;
    outbox::enqueue_agent_run_updated(
        &mut transaction,
        context.project_id,
        conversation_id,
        run_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
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
    }

    let mut transaction = state.pool.begin().await?;
    let context = sqlx::query_as::<_, RunContext>(
        r#"
        SELECT
            ar.conversation_id,
            c.project_id,
            ar.repository_id,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled
        FROM agent_runs ar
        JOIN conversations c ON c.id = ar.conversation_id
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
    transaction.commit().await?;
    fetch_run(&state, run_id, context.project_id)
        .await
        .map(Json)
}

async fn insert_significant_event(
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

async fn retain_run_workspace(
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

async fn insert_event_entry(
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
            ar.failure_reason,
            ar.created_at::text AS created_at,
            ar.updated_at::text AS updated_at
        FROM agent_runs ar
        JOIN conversations c
          ON c.id = ar.conversation_id
         AND c.project_id = $2
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
        capabilities: RunCapabilities { interrupt: false },
    })
}

async fn fetch_approval(
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
    principal: &AuthenticatedActor,
    conversation_id: Uuid,
    project_id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    #[derive(FromRow)]
    struct MutationGuard {
        kind: String,
        parent_channel_id: Option<Uuid>,
        conversation_archived: bool,
        project_archived: bool,
        membership_role: String,
        membership_enabled: bool,
    }

    let guard = sqlx::query_as::<_, MutationGuard>(
        r#"
        SELECT
            c.kind,
            c.parent_channel_id,
            c.archived_at IS NOT NULL AS conversation_archived,
            p.archived_at IS NOT NULL AS project_archived,
            pm.role AS membership_role,
            pm.enabled AS membership_enabled
        FROM conversations c
        JOIN projects p
          ON p.id = c.project_id
         AND p.id = $2
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $3
        WHERE c.id = $1
          AND c.project_id = $2
        FOR SHARE OF c, p, pm
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .bind(principal.actor_id)
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
    let parent_archived = if let Some(parent_channel_id) = guard.parent_channel_id {
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
    if guard.conversation_archived || parent_archived {
        return Err(AppError::NotFound("conversation"));
    }
    Ok(())
}

async fn authorized_conversation(
    state: &AppState,
    principal: &AuthenticatedActor,
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
        JOIN project_memberships pm
          ON pm.project_id = c.project_id
         AND pm.actor_id = $2
        WHERE c.id = $1
        "#,
    )
    .bind(conversation_id)
    .bind(principal.actor_id)
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
    principal: &AuthenticatedActor,
    project_id: Uuid,
    membership_role: &str,
    membership_enabled: bool,
    action: Action,
    resource: Resource,
) -> Result<(), AppError> {
    let subject = Subject::new(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
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
