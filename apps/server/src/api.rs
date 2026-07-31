use std::{fs::OpenOptions, io::Write, path::PathBuf};

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::AppError,
    git_runtime,
    realtime::RealtimeEvent,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/dev/bootstrap", get(development_bootstrap))
        .route(
            "/api/v1/conversations/{id}/timeline",
            get(conversation_timeline),
        )
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
) -> Result<Json<DevelopmentBootstrap>, AppError> {
    let identity = development_identity(&state).await?;
    let (channel_id, thread_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT parent.id, thread.id
        FROM conversations thread
        JOIN conversations parent ON parent.id = thread.parent_channel_id
        WHERE thread.project_id = $1
          AND thread.kind = 'thread'
          AND thread.archived_at IS NULL
          AND parent.archived_at IS NULL
        ORDER BY thread.created_at
        LIMIT 1
        "#,
    )
    .bind(identity.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("seeded_thread"))?;

    Ok(Json(DevelopmentBootstrap {
        project_id: identity.project_id,
        channel_id,
        thread_id,
    }))
}

#[derive(Clone, Copy)]
struct DevelopmentIdentity {
    actor_id: Uuid,
    project_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ConversationRow {
    id: Uuid,
    project_id: Uuid,
    kind: String,
    parent_channel_id: Option<Uuid>,
    name: String,
}

#[derive(Debug, FromRow)]
struct TimelineEntryRow {
    id: Uuid,
    sequence: i64,
    kind: String,
    created_at: String,
    message_id: Option<Uuid>,
    author_actor_id: Option<Uuid>,
    author_kind: Option<String>,
    author_name: Option<String>,
    author_handle: Option<String>,
    content: Option<serde_json::Value>,
    plain_text: Option<String>,
    agent_run_event_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    event_kind: Option<String>,
    event_summary: Option<String>,
    event_payload: Option<serde_json::Value>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunView {
    id: Uuid,
    state: String,
    repository_id: Uuid,
    repository_workspace_id: Option<Uuid>,
    provider_session_id: Option<Uuid>,
    failure_reason: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalView {
    id: Uuid,
    agent_run_id: Uuid,
    kind: String,
    summary: String,
    options: serde_json::Value,
    status: String,
    response: Option<serde_json::Value>,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineResponse {
    conversation: ConversationView,
    entries: Vec<TimelineEntry>,
    active_run: Option<RunView>,
    approvals: Vec<ApprovalView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationView {
    id: Uuid,
    project_id: Uuid,
    kind: String,
    parent_channel_id: Option<Uuid>,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineEntry {
    id: Uuid,
    sequence: i64,
    kind: String,
    created_at: String,
    message: Option<MessageView>,
    worker_event: Option<WorkerEventView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    id: Uuid,
    author: ActorView,
    content: serde_json::Value,
    plain_text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActorView {
    id: Uuid,
    kind: String,
    display_name: String,
    handle: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerEventView {
    id: Uuid,
    agent_run_id: Uuid,
    kind: String,
    summary: String,
    payload: serde_json::Value,
}

async fn conversation_timeline(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<TimelineResponse>, AppError> {
    let identity = development_identity(&state).await?;
    let conversation =
        conversation_for_project(&state, conversation_id, identity.project_id).await?;

    let rows = sqlx::query_as::<_, TimelineEntryRow>(
        r#"
        SELECT
            ce.id,
            ce.sequence,
            ce.kind,
            ce.created_at::text AS created_at,
            m.id AS message_id,
            m.author_actor_id,
            a.kind AS author_kind,
            pm.display_name AS author_name,
            pm.handle AS author_handle,
            m.content,
            m.plain_text,
            are.id AS agent_run_event_id,
            are.agent_run_id,
            are.kind AS event_kind,
            are.summary AS event_summary,
            are.payload AS event_payload
        FROM conversation_entries ce
        LEFT JOIN messages m ON m.id = ce.message_id
        LEFT JOIN actors a ON a.id = m.author_actor_id
        LEFT JOIN project_memberships pm
          ON pm.actor_id = m.author_actor_id
         AND pm.project_id = $2
        LEFT JOIN agent_run_events are ON are.id = ce.agent_run_event_id
        WHERE ce.conversation_id = $1
        ORDER BY ce.sequence ASC
        LIMIT 500
        "#,
    )
    .bind(conversation_id)
    .bind(identity.project_id)
    .fetch_all(&state.pool)
    .await?;

    let active_run = sqlx::query_as::<_, RunView>(
        r#"
        SELECT
            id, state, repository_id, repository_workspace_id,
            provider_session_id, failure_reason,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM agent_runs
        WHERE conversation_id = $1
          AND state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&state.pool)
    .await?;

    let approvals = sqlx::query_as::<_, ApprovalView>(
        r#"
        SELECT
            aa.id, aa.agent_run_id, aa.kind, aa.summary, aa.options,
            aa.status, aa.response, aa.created_at::text AS created_at
        FROM agent_approvals aa
        JOIN agent_runs ar ON ar.id = aa.agent_run_id
        WHERE ar.conversation_id = $1
        ORDER BY aa.created_at ASC
        "#,
    )
    .bind(conversation_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(TimelineResponse {
        conversation: ConversationView {
            id: conversation.id,
            project_id: conversation.project_id,
            kind: conversation.kind,
            parent_channel_id: conversation.parent_channel_id,
            name: conversation.name,
        },
        entries: rows.into_iter().map(TimelineEntry::from).collect(),
        active_run,
        approvals,
    }))
}

impl From<TimelineEntryRow> for TimelineEntry {
    fn from(row: TimelineEntryRow) -> Self {
        let message = row.message_id.map(|id| MessageView {
            id,
            author: ActorView {
                id: row.author_actor_id.unwrap_or_else(Uuid::nil),
                kind: row.author_kind.unwrap_or_else(|| "unknown".to_owned()),
                display_name: row
                    .author_name
                    .unwrap_or_else(|| "Unknown actor".to_owned()),
                handle: row.author_handle.unwrap_or_else(|| "unknown".to_owned()),
            },
            content: row.content.unwrap_or(serde_json::Value::Null),
            plain_text: row.plain_text.unwrap_or_default(),
        });
        let worker_event = row.agent_run_event_id.map(|id| WorkerEventView {
            id,
            agent_run_id: row.agent_run_id.unwrap_or_else(Uuid::nil),
            kind: row.event_kind.unwrap_or_else(|| "unknown".to_owned()),
            summary: row.event_summary.unwrap_or_default(),
            payload: row.event_payload.unwrap_or_else(|| serde_json::json!({})),
        });
        Self {
            id: row.id,
            sequence: row.sequence,
            kind: row.kind,
            created_at: row.created_at,
            message,
            worker_event,
        }
    }
}

#[derive(Deserialize)]
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

async fn start_agent_run(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, AppError> {
    let identity = development_identity(&state).await?;
    let prompt = request.prompt.trim();
    if prompt.is_empty() || prompt.len() > 20_000 {
        return Err(AppError::Invalid("prompt length"));
    }
    let conversation =
        conversation_for_project(&state, conversation_id, identity.project_id).await?;
    if conversation.kind != "thread" {
        return Err(AppError::Invalid("agent runs require a Thread"));
    }

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
        JOIN execution_profiles ep
          ON ep.id = ew.execution_profile_id AND ep.status = 'available'
        JOIN repositories r
          ON r.project_id = p.id AND r.enabled AND r.trusted
        WHERE p.id = $1
          AND r.id = COALESCE(
              $2,
              CASE
                  WHEN (SELECT count(*) FROM repositories rr
                         WHERE rr.project_id = p.id AND rr.enabled AND rr.trusted) = 1
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
            "SELECT repository_id FROM conversations WHERE id = $1",
        )
        .bind(conversation_id)
        .fetch_one(&state.pool)
        .await?,
    )
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

    sqlx::query(
        "UPDATE conversations SET repository_id = COALESCE(repository_id, $2), updated_at = now() WHERE id = $1",
    )
    .bind(conversation_id)
    .bind(setup.repository_id)
    .execute(&mut *transaction)
    .await?;

    let prompt_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(prompt_message_id)
    .bind(conversation_id)
    .bind(identity.actor_id)
    .bind(serde_json::json!({
        "version": 1,
        "nodes": [{"type": "text", "text": prompt}]
    }))
    .bind(prompt)
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
    .bind(prompt)
    .bind(serde_json::json!({
        "actor_id": identity.actor_id,
        "project_id": identity.project_id,
        "scope": ["repository.read", "repository.write", "provider.turn"]
    }))
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Starting, None).await?;
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
    .await
    .map_err(|_| AppError::Internal("join worktree task"))?;
    if let Err(error) = creation {
        fail_worktree_start(&state, run_id, worktree_id).await?;
        return Err(error);
    }

    let response_state = if state.config.demo_executor {
        let approval_id =
            prepare_demo_blocker(&state, identity, run_id, worktree_id, conversation_id).await?;
        publish_timeline_update(&state, identity.project_id, conversation_id, run_id);
        tracing::info!(%run_id, %approval_id, "development Agent Run is waiting for approval");
        RunState::Blocked
    } else {
        mark_worktree_ready(&state, worktree_id).await?;
        let provider_start = crate::provider::start_run(
            &state,
            identity.project_id,
            run_id,
            conversation_id,
            setup.execution_profile_id,
            worktree_path.clone(),
            prompt.to_owned(),
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
                publish_timeline_update(&state, identity.project_id, conversation_id, run_id);
                RunState::Working
            }
            Err(error) => {
                fail_provider_start(&state, run_id).await?;
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
    sqlx::query(
        "UPDATE git_worktrees SET state = 'ready', last_inspected_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(worktree_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn fail_provider_start(state: &AppState, run_id: Uuid) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    transition_run(
        &mut transaction,
        run_id,
        RunState::Failed,
        Some("Codex App Server could not start. The worktree was preserved."),
    )
    .await?;
    retain_run_workspace(&mut transaction, run_id).await?;
    transaction.commit().await?;
    Ok(())
}

async fn prepare_demo_blocker(
    state: &AppState,
    identity: DevelopmentIdentity,
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
    transaction.commit().await?;
    Ok(approval_id)
}

async fn fail_worktree_start(
    state: &AppState,
    run_id: Uuid,
    worktree_id: Uuid,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE git_worktrees SET state = 'failed', updated_at = now() WHERE id = $1")
        .bind(worktree_id)
        .execute(&mut *transaction)
        .await?;
    transition_run(
        &mut transaction,
        run_id,
        RunState::Failed,
        Some("Worktree creation failed; inspect the retained repository workspace."),
    )
    .await?;
    retain_run_workspace(&mut transaction, run_id).await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(Deserialize)]
struct ApprovalResponse {
    decision: String,
}

async fn respond_to_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<Uuid>,
    Json(response): Json<ApprovalResponse>,
) -> Result<Json<ApprovalView>, AppError> {
    let identity = development_identity(&state).await?;
    if !matches!(response.decision.as_str(), "approve" | "deny") {
        return Err(AppError::Invalid("unknown approval decision"));
    }

    #[derive(FromRow)]
    struct ApprovalContext {
        run_id: Uuid,
        conversation_id: Uuid,
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
            aa.status,
            aa.responded_by_actor_id,
            aa.response AS current_response
        FROM agent_approvals aa
        JOIN agent_runs ar ON ar.id = aa.agent_run_id
        JOIN conversations c ON c.id = ar.conversation_id
        WHERE aa.id = $1 AND c.project_id = $2
        FOR UPDATE OF aa
        "#,
    )
    .bind(approval_id)
    .bind(identity.project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))?;

    let response_json = serde_json::json!({"decision": response.decision});
    if context.status == "resolved" {
        if context.responded_by_actor_id == Some(identity.actor_id)
            && context.current_response == Some(response_json)
        {
            transaction.rollback().await?;
            return fetch_approval(&state, approval_id).await.map(Json);
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
    .bind(identity.actor_id)
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
            "responded_by_actor_id": identity.actor_id
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
    transaction.commit().await?;
    publish_timeline_update(
        &state,
        identity.project_id,
        context.conversation_id,
        context.run_id,
    );

    if response.decision == "approve" && state.config.demo_executor {
        let background_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) =
                complete_demo_run(background_state, context.run_id, context.conversation_id).await
            {
                tracing::error!(?error, run_id = %context.run_id, "development run completion failed");
            }
        });
    }
    fetch_approval(&state, approval_id).await.map(Json)
}

async fn complete_demo_run(
    state: AppState,
    run_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let identity = development_identity(&state).await?;
    let worktree_path = sqlx::query_scalar::<_, String>(
        r#"
        SELECT gw.path
        FROM agent_runs ar
        JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
        JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
        WHERE ar.id = $1 AND gw.state = 'ready'
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Conflict(
        "git_worktree",
        "ready worktree not found",
    ))?;

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
    let message_text = "The development walking skeleton completed its isolated file change and preserved the worktree.";
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text, agent_run_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(message_id)
    .bind(conversation_id)
    .bind(worker_actor_id)
    .bind(serde_json::json!({
        "version": 1,
        "nodes": [{"type": "text", "text": message_text}]
    }))
    .bind(message_text)
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
    transaction.commit().await?;
    publish_timeline_update(&state, identity.project_id, conversation_id, run_id);
    Ok(())
}

async fn interrupt_agent_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunView>, AppError> {
    let identity = development_identity(&state).await?;
    let conversation_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT ar.conversation_id
        FROM agent_runs ar
        JOIN conversations c ON c.id = ar.conversation_id
        WHERE ar.id = $1 AND c.project_id = $2
        "#,
    )
    .bind(run_id)
    .bind(identity.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_run"))?;

    let mut transaction = state.pool.begin().await?;
    transition_run(
        &mut transaction,
        run_id,
        RunState::Interrupted,
        Some("Interrupted by the requesting Actor."),
    )
    .await?;
    retain_run_workspace(&mut transaction, run_id).await?;
    let event_id = insert_significant_event(
        &mut transaction,
        run_id,
        "run_interrupted",
        "The run was interrupted.",
        serde_json::json!({"actor_id": identity.actor_id}),
    )
    .await?;
    insert_event_entry(&mut transaction, conversation_id, event_id).await?;
    sqlx::query(
        "UPDATE agent_approvals SET status = 'canceled' WHERE agent_run_id = $1 AND status = 'pending'",
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    publish_timeline_update(&state, identity.project_id, conversation_id, run_id);
    fetch_run(&state, run_id).await.map(Json)
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

async fn fetch_run(state: &AppState, run_id: Uuid) -> Result<RunView, AppError> {
    sqlx::query_as::<_, RunView>(
        r#"
        SELECT
            id, state, repository_id, repository_workspace_id,
            provider_session_id, failure_reason,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM agent_runs WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_run"))
}

async fn fetch_approval(state: &AppState, approval_id: Uuid) -> Result<ApprovalView, AppError> {
    sqlx::query_as::<_, ApprovalView>(
        r#"
        SELECT
            id, agent_run_id, kind, summary, options, status, response,
            created_at::text AS created_at
        FROM agent_approvals WHERE id = $1
        "#,
    )
    .bind(approval_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))
}

async fn conversation_for_project(
    state: &AppState,
    conversation_id: Uuid,
    project_id: Uuid,
) -> Result<ConversationRow, AppError> {
    sqlx::query_as::<_, ConversationRow>(
        r#"
        SELECT id, project_id, kind, parent_channel_id, name
        FROM conversations
        WHERE id = $1 AND project_id = $2 AND archived_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("conversation"))
}

async fn development_identity(state: &AppState) -> Result<DevelopmentIdentity, AppError> {
    if !state.config.environment.is_development() || !state.config.server_addr.ip().is_loopback() {
        return Err(AppError::DevelopmentAuthUnavailable);
    }
    #[derive(FromRow)]
    struct IdentityRow {
        actor_id: Uuid,
        project_id: Uuid,
    }
    let identity = sqlx::query_as::<_, IdentityRow>(
        r#"
        SELECT pm.actor_id, pm.project_id
        FROM project_memberships pm
        JOIN actors a ON a.id = pm.actor_id
        JOIN projects p ON p.id = pm.project_id
        WHERE lower(pm.handle) = 'mischa'
          AND pm.role = 'owner'
          AND pm.enabled
          AND a.kind = 'human'
          AND a.status = 'active'
          AND p.project_key = 'AW'
          AND p.archived_at IS NULL
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::DevelopmentAuthUnavailable)?;
    Ok(DevelopmentIdentity {
        actor_id: identity.actor_id,
        project_id: identity.project_id,
    })
}

fn publish_timeline_update(
    state: &AppState,
    project_id: Uuid,
    conversation_id: Uuid,
    run_id: Uuid,
) {
    state.realtime.publish(RealtimeEvent {
        event_id: Uuid::now_v7(),
        kind: "timeline.updated",
        project_id,
        conversation_id: Some(conversation_id),
        payload: serde_json::json!({"run_id": run_id}),
    });
}
