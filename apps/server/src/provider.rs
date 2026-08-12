use std::{path::PathBuf, time::Duration};

use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    AppState,
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::AppError,
    git_runtime::worktree_is_dirty,
    message_content::{MessageContentError, PlainTextMessageBuffer, ValidatedMessageContent},
    outbox,
    realtime::RealtimeEvent,
};

mod codex_app_server;

pub use codex_app_server::{
    AccountSummary, LoginCeremonyError, LoginCeremonySnapshot, LoginKind, LoginReservation,
    LoginStart, ProfileHealth, RateLimitSummary, RateLimitWindow, SupervisorRegistry,
};
use codex_app_server::{
    ApprovalKind, ItemKind, ProfileLease, ProviderAnswer, ProviderEvent, RecoveredTurn,
};

#[derive(Debug)]
pub struct StartedProviderRun {
    pub provider_session_id: Uuid,
    pub provider_thread_id: String,
}

#[derive(Debug)]
pub(crate) enum ProviderRecovery {
    Completed { message: String },
    Resumed { latest_turn_status: String },
    UnboundTurn,
    MismatchedTurn,
    RejectedMessage,
    Unavailable,
}

pub async fn start_run(
    state: &AppState,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    execution_profile_id: Uuid,
    worktree: PathBuf,
    prompt: String,
) -> Result<StartedProviderRun, AppError> {
    let codex_home = sqlx::query_scalar::<_, String>(
        "SELECT codex_home_path FROM execution_profiles WHERE id = $1",
    )
    .bind(execution_profile_id)
    .fetch_one(&state.pool)
    .await?;
    let codex_home = PathBuf::from(codex_home);
    let provider = state
        .supervisors
        .acquire(execution_profile_id, codex_home)
        .await;
    let mut provider = match provider {
        Ok(provider) => {
            mark_profile_healthy(state, execution_profile_id).await?;
            provider
        }
        Err(error) => {
            mark_profile_failure(state, execution_profile_id, "supervisor_unavailable").await;
            return Err(provider_error(run_id, "acquire profile supervisor", error));
        }
    };

    let existing = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, provider_thread_id
        FROM provider_sessions
        WHERE conversation_id = $1 AND execution_profile_id = $2
        "#,
    )
    .bind(conversation_id)
    .bind(execution_profile_id)
    .fetch_optional(&state.pool)
    .await?;

    let (provider_session_id, provider_thread_id) = if let Some((session_id, thread_id)) = existing
    {
        provider
            .resume_thread(&thread_id, &worktree)
            .await
            .map_err(|error| provider_error(run_id, "resume thread", error))?;
        (session_id, thread_id)
    } else {
        let thread_id = provider
            .start_thread(&worktree)
            .await
            .map_err(|error| provider_error(run_id, "start thread", error))?;
        let session_id = Uuid::now_v7();
        sqlx::query(
            r#"
                INSERT INTO provider_sessions (
                    id, conversation_id, execution_profile_id,
                    provider_thread_id, state
                )
                VALUES ($1, $2, $3, $4, 'active')
                "#,
        )
        .bind(session_id)
        .bind(conversation_id)
        .bind(execution_profile_id)
        .bind(&thread_id)
        .execute(&state.pool)
        .await?;
        (session_id, thread_id)
    };

    // Persist the provider Thread ownership before starting a turn. If the
    // process dies around the external turn/start side effect, recovery can
    // distinguish an unbound turn from a durably identified one and must not
    // mistake an older completed turn for this run.
    let mut transaction = state.pool.begin().await?;
    let bound = sqlx::query(
        r#"
        UPDATE agent_runs
        SET provider_session_id = $2, updated_at = now()
        WHERE id = $1
          AND state = 'starting'
          AND (provider_session_id IS NULL OR provider_session_id = $2)
        "#,
    )
    .bind(run_id)
    .bind(provider_session_id)
    .execute(&mut *transaction)
    .await?;
    if bound.rows_affected() != 1 {
        transaction.rollback().await?;
        idle_unowned_provider_session(state, provider_session_id).await?;
        return Err(AppError::Conflict(
            "agent_run",
            "provider Session binding is no longer available",
        ));
    }
    sqlx::query("UPDATE provider_sessions SET state = 'active', updated_at = now() WHERE id = $1")
        .bind(provider_session_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let turn_id = provider
        .start_turn(&provider_thread_id, &worktree, &prompt)
        .await
        .map_err(|error| provider_error(run_id, "start turn", error))?;

    let mut transaction = state.pool.begin().await?;
    let bound = sqlx::query(
        r#"
        UPDATE agent_runs
        SET provider_turn_id = $2, updated_at = now()
        WHERE id = $1
          AND state = 'starting'
          AND provider_session_id = $3
          AND provider_turn_id IS NULL
        "#,
    )
    .bind(run_id)
    .bind(&turn_id)
    .bind(provider_session_id)
    .execute(&mut *transaction)
    .await?;
    if bound.rows_affected() != 1 {
        transaction.rollback().await?;
        let _ = provider.interrupt_turn(&provider_thread_id, &turn_id).await;
        idle_unowned_provider_session(state, provider_session_id).await?;
        return Err(AppError::Conflict(
            "agent_run",
            "provider turn binding is no longer available",
        ));
    }
    transition_run(&mut transaction, run_id, RunState::Working, None).await?;
    outbox::enqueue_agent_run_updated(&mut transaction, project_id, conversation_id, run_id)
        .await?;
    transaction.commit().await?;

    let background_state = state.clone();
    let thread_for_task = provider_thread_id.clone();
    tokio::spawn(async move {
        if let Err(error) = drive_turn(
            background_state.clone(),
            provider,
            project_id,
            run_id,
            conversation_id,
            thread_for_task,
            turn_id,
        )
        .await
        {
            tracing::error!(
                error_category = error.log_category(),
                %run_id,
                "hosted Codex turn failed"
            );
            let _ = fail_run(
                &background_state,
                project_id,
                run_id,
                conversation_id,
                "The hosted Codex turn failed. The worktree was preserved.",
            )
            .await;
        }
    });

    Ok(StartedProviderRun {
        provider_session_id,
        provider_thread_id,
    })
}

pub(crate) async fn inspect_provider_for_recovery(
    state: &AppState,
    run_id: Uuid,
    worktree: PathBuf,
) -> ProviderRecovery {
    let provider = sqlx::query_as::<_, (String, String, Uuid, Option<String>)>(
        r#"
        SELECT ps.provider_thread_id, ep.codex_home_path, ep.id,
               ar.provider_turn_id
        FROM agent_runs ar
        JOIN provider_sessions ps ON ps.id = ar.provider_session_id
        JOIN execution_profiles ep ON ep.id = ar.execution_profile_id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await;
    let Ok(Some((provider_thread_id, codex_home, execution_profile_id, provider_turn_id))) =
        provider
    else {
        return ProviderRecovery::Unavailable;
    };
    let Some(provider_turn_id) = provider_turn_id else {
        return ProviderRecovery::UnboundTurn;
    };

    let provider = state
        .supervisors
        .acquire(execution_profile_id, PathBuf::from(codex_home))
        .await;
    let Ok(mut provider) = provider else {
        return ProviderRecovery::Unavailable;
    };
    if provider
        .resume_thread(&provider_thread_id, &worktree)
        .await
        .is_err()
    {
        return ProviderRecovery::Unavailable;
    }
    let history = provider.read_latest_turn(&provider_thread_id).await;
    let Ok(history) = history else {
        return ProviderRecovery::Unavailable;
    };
    if !recovered_turn_has_identity(&history, &provider_turn_id) {
        return ProviderRecovery::MismatchedTurn;
    }
    let latest_turn_status = history.status;
    if latest_turn_status == "completed" {
        let message = history.final_message.unwrap_or_default();
        if validated_worker_message(&message).is_err() {
            tracing::warn!(
                %run_id,
                "rejected invalid provider Message while reconciling completed turn"
            );
            return ProviderRecovery::RejectedMessage;
        }
        return ProviderRecovery::Completed { message };
    }
    ProviderRecovery::Resumed { latest_turn_status }
}

fn recovered_turn_has_identity(history: &RecoveredTurn, expected_turn_id: &str) -> bool {
    history.id.as_deref() == Some(expected_turn_id)
}

async fn mark_profile_healthy(state: &AppState, profile_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE execution_profiles
        SET last_health_at = now(), last_failure_category = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(profile_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn idle_unowned_provider_session(
    state: &AppState,
    provider_session_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_sessions session
           SET state = 'idle', updated_at = now()
         WHERE session.id = $1
           AND NOT EXISTS (
               SELECT 1
                 FROM agent_runs run
                WHERE run.provider_session_id = session.id
                  AND run.state IN (
                      'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                  )
           )
        "#,
    )
    .bind(provider_session_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn mark_profile_failure(state: &AppState, profile_id: Uuid, category: &'static str) {
    if let Err(error) = sqlx::query(
        r#"
        UPDATE execution_profiles
        SET last_health_at = now(), last_failure_category = $2,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(profile_id)
    .bind(category)
    .execute(&state.pool)
    .await
    {
        tracing::error!(
            error_type = std::any::type_name_of_val(&error),
            %profile_id,
            "failed to persist redacted Codex profile health"
        );
    }
}

pub(crate) async fn complete_recovered_run(
    state: &AppState,
    run_id: Uuid,
    message: &str,
    previous_state: &str,
    worktree_state: &str,
    dirty: bool,
) -> Result<(), AppError> {
    let (project_id, conversation_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
            SELECT c.project_id, ar.conversation_id
            FROM agent_runs ar
            JOIN conversations c ON c.id = ar.conversation_id
            WHERE ar.id = $1
            "#,
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let mut transaction = state.pool.begin().await?;
    transition_run(&mut transaction, run_id, RunState::Working, None).await?;
    outbox::enqueue_agent_run_updated(&mut transaction, project_id, conversation_id, run_id)
        .await?;
    transaction.commit().await?;
    complete_run(
        state,
        project_id,
        run_id,
        conversation_id,
        Uuid::now_v7(),
        message,
        Some(RecoveryCompletion {
            previous_state,
            worktree_state,
            dirty,
        }),
    )
    .await
}

struct RecoveryCompletion<'a> {
    previous_state: &'a str,
    worktree_state: &'a str,
    dirty: bool,
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    state: AppState,
    mut provider: ProfileLease,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    provider_thread_id: String,
    turn_id: String,
) -> Result<(), AppError> {
    let mut message = PlainTextMessageBuffer::default();
    let message_stream_id = Uuid::now_v7();
    let execution_profile_id = provider.profile_id();
    loop {
        tokio::select! {
            event = provider.next_event() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if !state.supervisors.is_shutting_down() {
                            sleep(Duration::from_millis(100)).await;
                        }
                        if state.supervisors.is_shutting_down() {
                            provider.shutdown_process().await;
                            return Ok(());
                        }
                        return Err(provider_error(run_id, "read event", error));
                    }
                };
                match event {
                    ProviderEvent::Request(request) => handle_provider_request(
                        &state,
                        &mut provider,
                        project_id,
                        run_id,
                        conversation_id,
                        request,
                    ).await?,
                    ProviderEvent::AgentMessageDelta(delta) => {
                        if let Err(error) = message.push(&delta) {
                            provider.shutdown_process().await;
                            return Err(provider_message_error(run_id, error));
                        }
                        state.realtime.publish(RealtimeEvent {
                            event_id: Uuid::now_v7(),
                            kind: "message.delta",
                            project_id,
                            conversation_id: Some(conversation_id),
                            payload: json!({
                                "message_id": message_stream_id,
                                "agent_run_id": run_id,
                                "delta": delta
                            }),
                        });
                    }
                    ProviderEvent::ItemStarted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        persist_event(
                            &state,
                            project_id,
                            run_id,
                            conversation_id,
                            SignificantProviderEvent {
                                key: &event.key,
                                kind: "command_started",
                                summary: "Codex started a command in the isolated worktree.",
                                payload: json!({}),
                            },
                        ).await?;
                    }
                    ProviderEvent::ItemCompleted(event)
                        if event.kind == ItemKind::CommandExecution => {
                        persist_event(
                            &state,
                            project_id,
                            run_id,
                            conversation_id,
                            SignificantProviderEvent {
                                key: &event.key,
                                kind: "command_completed",
                                summary: "Codex completed a command in the isolated worktree.",
                                payload: json!({}),
                            },
                        ).await?;
                    }
                    ProviderEvent::ItemCompleted(event) if event.kind == ItemKind::FileChange => {
                        persist_event(
                            &state,
                            project_id,
                            run_id,
                            conversation_id,
                            SignificantProviderEvent {
                                key: &event.key,
                                kind: "file_change",
                                summary: "Codex completed a file change in the isolated worktree.",
                                payload: json!({}),
                            },
                        ).await?;
                    }
                    ProviderEvent::TurnCompleted { completed } => {
                        if !completed {
                            return Err(AppError::Conflict(
                                "provider",
                                "Codex turn did not complete successfully",
                            ));
                        }
                        complete_run(
                            &state,
                            project_id,
                            run_id,
                            conversation_id,
                            message_stream_id,
                            message.as_str(),
                            None,
                        ).await?;
                        return Ok(());
                    }
                    ProviderEvent::Error => {
                        return Err(AppError::Conflict("provider", "Codex reported an error"));
                    }
                    ProviderEvent::ItemStarted(_)
                    | ProviderEvent::ItemCompleted(_)
                    | ProviderEvent::Ignored => {}
                }
            }
            () = sleep(Duration::from_millis(500)) => {
                if state.supervisors.is_shutting_down() {
                    provider.shutdown_process().await;
                    return Ok(());
                }
                if state.supervisors.profile_is_stopping(execution_profile_id).await {
                    provider.shutdown_process().await;
                    fail_run(
                        &state,
                        project_id,
                        run_id,
                        conversation_id,
                        "The Execution Profile was stopped. The worktree was preserved.",
                    ).await?;
                    return Ok(());
                }
                let run_state = sqlx::query_scalar::<_, String>(
                    "SELECT state FROM agent_runs WHERE id = $1",
                )
                .bind(run_id)
                .fetch_one(&state.pool)
                .await?;
                if run_state == "interrupted" {
                    let _ = provider.interrupt_turn(&provider_thread_id, &turn_id).await;
                    return Ok(());
                }
            }
            () = state.supervisors.shutdown_requested() => {
                provider.shutdown_process().await;
                return Ok(());
            }
        }
    }
}

async fn handle_provider_request(
    state: &AppState,
    provider: &mut ProfileLease,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    request: codex_app_server::ProviderRequest,
) -> Result<(), AppError> {
    if request.kind == ApprovalKind::Unsupported {
        provider
            .answer_request(request, ProviderAnswer::Deny)
            .await
            .map_err(|error| {
                provider_error(run_id, "reject unsupported provider request", error)
            })?;
        return Ok(());
    }
    let kind = match request.kind {
        ApprovalKind::Question => "question",
        ApprovalKind::Approval => "approval",
        ApprovalKind::Unsupported => unreachable!("unsupported requests return above"),
    };
    let provider_request_id = request.durable_id();
    let summary = request.summary().to_owned();
    let options = request.public_options();
    let mut transaction = state.pool.begin().await?;
    let inserted_approval_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options
        )
        SELECT $1, $2, ar.requesting_actor_id, $3, $4, $5, $6
        FROM agent_runs ar WHERE ar.id = $2
        ON CONFLICT (agent_run_id, provider_request_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(run_id)
    .bind(&provider_request_id)
    .bind(kind)
    .bind(&summary)
    .bind(options)
    .fetch_optional(&mut *transaction)
    .await?;
    let approval_id = match inserted_approval_id {
        Some(approval_id) => {
            crate::external_events::enqueue_external_approval_requested(
                &mut transaction,
                approval_id,
            )
            .await?;
            transition_run(&mut transaction, run_id, RunState::Blocked, None).await?;
            let event_id = insert_event(
                &mut transaction,
                run_id,
                &format!("provider:{kind}:requested:{provider_request_id}"),
                if kind == "question" {
                    "question_requested"
                } else {
                    "approval_requested"
                },
                &summary,
                json!({"approval_id": approval_id}),
            )
            .await?
            .ok_or(AppError::Internal("new approval event was not inserted"))?;
            insert_entry(&mut transaction, conversation_id, event_id).await?;
            outbox::enqueue_agent_run_updated(
                &mut transaction,
                project_id,
                conversation_id,
                run_id,
            )
            .await?;
            approval_id
        }
        None => sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM agent_approvals WHERE agent_run_id = $1 AND provider_request_id = $2",
        )
        .bind(run_id)
        .bind(&provider_request_id)
        .fetch_one(&mut *transaction)
        .await?,
    };
    transaction.commit().await?;

    loop {
        sleep(Duration::from_millis(300)).await;
        if state.supervisors.is_shutting_down() {
            let _ = provider.answer_request(request, ProviderAnswer::Deny).await;
            provider.shutdown_process().await;
            return Ok(());
        }
        if state
            .supervisors
            .profile_is_stopping(provider.profile_id())
            .await
        {
            let _ = provider.answer_request(request, ProviderAnswer::Deny).await;
            provider.shutdown_process().await;
            fail_run(
                state,
                project_id,
                run_id,
                conversation_id,
                "The Execution Profile was stopped. The worktree was preserved.",
            )
            .await?;
            return Ok(());
        }
        #[derive(FromRow)]
        struct Resolution {
            run_state: String,
            status: String,
            response: Option<Value>,
        }
        let resolution = sqlx::query_as::<_, Resolution>(
            r#"
            SELECT ar.state AS run_state, aa.status, aa.response
            FROM agent_approvals aa
            JOIN agent_runs ar ON ar.id = aa.agent_run_id
            WHERE aa.id = $1
            "#,
        )
        .bind(approval_id)
        .fetch_one(&state.pool)
        .await?;
        if resolution.run_state == "interrupted" {
            provider
                .answer_request(request, ProviderAnswer::Deny)
                .await
                .map_err(|error| provider_error(run_id, "decline interrupted request", error))?;
            return Ok(());
        }
        if resolution.status != "resolved" {
            continue;
        }
        let claimed = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE agent_approvals
               SET provider_response_claimed_at = now()
             WHERE id = $1
               AND status = 'resolved'
               AND provider_response_claimed_at IS NULL
            RETURNING id
            "#,
        )
        .bind(approval_id)
        .fetch_optional(&state.pool)
        .await?;
        if claimed.is_none() {
            return Ok(());
        }
        let answer = match resolution.response.as_ref() {
            Some(response) if kind == "question" => response
                .get("answer")
                .and_then(Value::as_str)
                .map(|answer| ProviderAnswer::Text(answer.to_owned()))
                .ok_or(AppError::Internal("resolved question omitted its answer"))?,
            Some(response)
                if response.get("decision").and_then(Value::as_str) == Some("approve") =>
            {
                ProviderAnswer::Approve
            }
            _ => ProviderAnswer::Deny,
        };
        provider
            .answer_request(request, answer)
            .await
            .map_err(|error| provider_error(run_id, "answer provider request", error))?;
        sqlx::query(
            "UPDATE agent_approvals SET provider_response_delivered_at = now() WHERE id = $1",
        )
        .bind(approval_id)
        .execute(&state.pool)
        .await?;
        return Ok(());
    }
}

async fn complete_run(
    state: &AppState,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    worker_message_id: Uuid,
    message: &str,
    recovery: Option<RecoveryCompletion<'_>>,
) -> Result<(), AppError> {
    let validated_message =
        validated_worker_message(message).map_err(|error| provider_message_error(run_id, error))?;
    let worktree_path = sqlx::query_scalar::<_, String>(
        r#"
        SELECT gw.path
        FROM agent_runs ar
        JOIN repository_workspaces rw ON rw.id = ar.repository_workspace_id
        JOIN git_worktrees gw ON gw.repository_workspace_id = rw.id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let dirty = tokio::task::spawn_blocking(move || {
        worktree_is_dirty(PathBuf::from(worktree_path).as_path())
    })
    .await
    .map_err(|_| AppError::Internal("join worktree inspection task"))??;

    let worker_actor_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT ew.actor_id FROM agent_runs ar
        JOIN execution_workers ew ON ew.id = ar.execution_worker_id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(&state.pool)
    .await?;
    let mut transaction = state.pool.begin().await?;
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
    if let Some(validated_message) = validated_message {
        let (content, plain_text, mentioned_actor_ids) = validated_message.into_parts();
        debug_assert!(mentioned_actor_ids.is_empty());
        let content = serde_json::to_value(content)
            .map_err(|_| AppError::Internal("serialize provider Worker Message"))?;
        sqlx::query(
            r#"
            INSERT INTO messages (
                id, project_id, conversation_id, author_actor_id,
                content_version, content, plain_text, agent_run_id
            )
            VALUES ($1, $2, $3, $4, 1, $5, $6, $7)
            ON CONFLICT (agent_run_id) WHERE agent_run_id IS NOT NULL DO NOTHING
            "#,
        )
        .bind(worker_message_id)
        .bind(project_id)
        .bind(conversation_id)
        .bind(worker_actor_id)
        .bind(content)
        .bind(plain_text)
        .bind(run_id)
        .execute(&mut *transaction)
        .await?;
        let persisted_message_id =
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM messages WHERE agent_run_id = $1")
                .bind(run_id)
                .fetch_one(&mut *transaction)
                .await?;
        let existing_entry = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM conversation_entries WHERE message_id = $1)",
        )
        .bind(persisted_message_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !existing_entry {
            let sequence =
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
            .bind(sequence)
            .bind(persisted_message_id)
            .execute(&mut *transaction)
            .await?;
            outbox::enqueue_message_created(
                &mut transaction,
                project_id,
                conversation_id,
                persisted_message_id,
                sequence,
                worker_actor_id,
            )
            .await?;
        }
    }
    transition_run(&mut transaction, run_id, RunState::Completed, None).await?;
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
    let event_id = insert_event(
        &mut transaction,
        run_id,
        "provider:run:completed",
        "run_completed",
        "The hosted Codex turn completed.",
        json!({}),
    )
    .await?;
    if let Some(event_id) = event_id {
        insert_entry(&mut transaction, conversation_id, event_id).await?;
    }
    if let Some(recovery) = recovery {
        sqlx::query(
            "UPDATE agent_approvals SET status = 'canceled' WHERE agent_run_id = $1 AND status = 'pending'",
        )
        .bind(run_id)
        .execute(&mut *transaction)
        .await?;
        let recovery_event = insert_event(
            &mut transaction,
            run_id,
            "startup-recovery:completed",
            "recovery",
            "Service restarted during a run. Provider history proved that the turn completed, so the durable run was reconciled as completed.",
            json!({
                "previous_state": recovery.previous_state,
                "result": "completed",
                "provider_resumed": true,
                "worktree_preserved": true,
                "worktree_state": recovery.worktree_state,
                "dirty": recovery.dirty
            }),
        )
        .await?;
        if let Some(recovery_event) = recovery_event {
            insert_entry(&mut transaction, conversation_id, recovery_event).await?;
        }
    }
    outbox::enqueue_agent_run_updated(&mut transaction, project_id, conversation_id, run_id)
        .await?;
    transaction.commit().await?;
    Ok(())
}

struct SignificantProviderEvent<'a> {
    key: &'a str,
    kind: &'a str,
    summary: &'a str,
    payload: Value,
}

async fn persist_event(
    state: &AppState,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    event: SignificantProviderEvent<'_>,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    if let Some(event_id) = insert_event(
        &mut transaction,
        run_id,
        event.key,
        event.kind,
        event.summary,
        event.payload,
    )
    .await?
    {
        insert_entry(&mut transaction, conversation_id, event_id).await?;
        outbox::enqueue_agent_run_updated(&mut transaction, project_id, conversation_id, run_id)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    event_key: &str,
    kind: &str,
    summary: &str,
    payload: Value,
) -> Result<Option<Uuid>, AppError> {
    let event_id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (agent_run_id, event_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(event_id)
    .bind(run_id)
    .bind(event_key)
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(inserted)
}

async fn insert_entry(
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

async fn fail_run(
    state: &AppState,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    reason: &'static str,
) -> Result<(), AppError> {
    let current = sqlx::query_scalar::<_, String>("SELECT state FROM agent_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&state.pool)
        .await?;
    if matches!(
        current.as_str(),
        "completed" | "failed" | "canceled" | "interrupted"
    ) {
        return Ok(());
    }
    let mut transaction = state.pool.begin().await?;
    transition_run(&mut transaction, run_id, RunState::Failed, Some(reason)).await?;
    sqlx::query(
        r#"
        UPDATE repository_workspaces rw
        SET state = 'retained', updated_at = now()
        FROM agent_runs ar
        WHERE ar.id = $1 AND rw.id = ar.repository_workspace_id
        "#,
    )
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    idle_provider_session_for_terminal_run(&mut transaction, run_id).await?;
    let event_id = insert_event(
        &mut transaction,
        run_id,
        "provider:run:failed",
        "run_failed",
        reason,
        json!({}),
    )
    .await?;
    if let Some(event_id) = event_id {
        insert_entry(&mut transaction, conversation_id, event_id).await?;
    }
    outbox::enqueue_agent_run_updated(&mut transaction, project_id, conversation_id, run_id)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn idle_provider_session_for_terminal_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_sessions session
           SET state = 'idle', updated_at = now()
          FROM agent_runs run
         WHERE run.id = $1
           AND run.provider_session_id = session.id
           AND run.state IN ('completed', 'failed', 'canceled', 'interrupted')
           AND NOT EXISTS (
               SELECT 1
                 FROM agent_runs other
                WHERE other.provider_session_id = session.id
                  AND other.id <> run.id
                  AND other.state IN (
                      'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
                  )
           )
        "#,
    )
    .bind(run_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn provider_error(run_id: Uuid, operation: &'static str, error: impl std::fmt::Debug) -> AppError {
    tracing::error!(
        %run_id,
        operation,
        error_type = std::any::type_name_of_val(&error),
        "Codex App Server operation failed"
    );
    AppError::Conflict("provider", "Codex App Server operation failed")
}

fn validated_worker_message(
    message: &str,
) -> Result<Option<ValidatedMessageContent>, MessageContentError> {
    let mut buffer = PlainTextMessageBuffer::default();
    buffer.push(message)?;
    if buffer.as_str().trim().is_empty() {
        return Ok(None);
    }
    buffer.into_validated().map(Some)
}

fn provider_message_error(run_id: Uuid, error: MessageContentError) -> AppError {
    tracing::warn!(
        %run_id,
        validation_error = ?error,
        "Codex Message output violated the shared Message boundary"
    );
    AppError::Conflict("provider", "Codex Message output is invalid")
}

#[cfg(test)]
mod tests {
    use super::{recovered_turn_has_identity, validated_worker_message};
    use crate::message_content::{MAX_MESSAGE_TEXT_CHARACTERS, MessageContentError};
    use crate::provider::codex_app_server::RecoveredTurn;

    #[test]
    fn worker_messages_share_unicode_and_control_boundaries() {
        let exact = "🙂".repeat(MAX_MESSAGE_TEXT_CHARACTERS);
        let validated = validated_worker_message(&exact)
            .expect("exact scalar-value boundary")
            .expect("non-empty Worker Message");
        assert_eq!(validated.plain_text(), exact);
        assert_eq!(
            validated_worker_message(&format!("{exact}🙂")),
            Err(MessageContentError::TextTooLong)
        );
        assert_eq!(
            validated_worker_message("safe\u{0007}unsafe"),
            Err(MessageContentError::UnsupportedControlCharacter)
        );
        assert!(
            validated_worker_message(" \n\t ")
                .expect("safe whitespace")
                .is_none()
        );
    }

    #[test]
    fn recovery_requires_the_exact_durable_provider_turn_identity() {
        let history = RecoveredTurn {
            id: Some("turn-current".to_owned()),
            status: "completed".to_owned(),
            final_message: Some("proof".to_owned()),
        };
        assert!(recovered_turn_has_identity(&history, "turn-current"));
        assert!(!recovered_turn_has_identity(&history, "turn-older"));
        assert!(!recovered_turn_has_identity(
            &RecoveredTurn {
                id: None,
                status: "missing".to_owned(),
                final_message: None,
            },
            "turn-current"
        ));
    }
}
