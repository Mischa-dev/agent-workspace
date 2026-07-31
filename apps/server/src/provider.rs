use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use codex_bridge_spike::{BridgeConfig, CodexBridge};
use serde_json::{Value, json};
use sqlx::FromRow;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    AppState,
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::AppError,
    git_runtime::worktree_is_dirty,
    realtime::RealtimeEvent,
};

const MAX_MESSAGE_BYTES: usize = 200_000;

#[derive(Debug)]
pub struct StartedProviderRun {
    pub provider_session_id: Uuid,
    pub provider_thread_id: String,
}

#[derive(Debug)]
pub(crate) enum ProviderRecovery {
    Completed { message: String },
    Resumed { latest_turn_status: String },
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
    let mut bridge = spawn_initialized_bridge(state, codex_home, worktree.clone(), run_id).await?;

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
        bridge
            .request(
                "thread/resume",
                json!({"threadId": thread_id, "cwd": worktree}),
            )
            .await
            .map_err(|error| provider_error(run_id, "resume thread", error))?;
        (session_id, thread_id)
    } else {
        let thread = bridge
            .request(
                "thread/start",
                json!({
                    "cwd": worktree,
                    "approvalPolicy": "on-request",
                    "sandbox": "workspace-write",
                    "ephemeral": false
                }),
            )
            .await
            .map_err(|error| provider_error(run_id, "start thread", error))?;
        let thread_id = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or(AppError::Internal("provider omitted thread id"))?
            .to_owned();
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

    let turn = bridge
        .request(
            "turn/start",
            json!({
                "threadId": provider_thread_id,
                "cwd": worktree,
                "input": [{"type": "text", "text": prompt}]
            }),
        )
        .await
        .map_err(|error| provider_error(run_id, "start turn", error))?;
    let turn_id = turn
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .ok_or(AppError::Internal("provider omitted turn id"))?
        .to_owned();

    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE agent_runs
        SET provider_session_id = $2, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(provider_session_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE provider_sessions SET state = 'active', updated_at = now() WHERE id = $1")
        .bind(provider_session_id)
        .execute(&mut *transaction)
        .await?;
    transition_run(&mut transaction, run_id, RunState::Working, None).await?;
    transaction.commit().await?;

    let background_state = state.clone();
    let thread_for_task = provider_thread_id.clone();
    tokio::spawn(async move {
        if let Err(error) = drive_turn(
            background_state.clone(),
            bridge,
            project_id,
            run_id,
            conversation_id,
            provider_session_id,
            thread_for_task,
            turn_id,
        )
        .await
        {
            tracing::error!(?error, %run_id, "hosted Codex turn failed");
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

async fn spawn_initialized_bridge(
    state: &AppState,
    codex_home: PathBuf,
    worktree: PathBuf,
    run_id: Uuid,
) -> Result<CodexBridge, AppError> {
    let mut environment = BTreeMap::new();
    let codex_parent = state
        .config
        .codex_binary
        .parent()
        .ok_or(AppError::Internal("Codex binary has no parent"))?;
    environment.insert(
        "PATH".to_owned(),
        format!("{}:/usr/local/bin:/usr/bin:/bin", codex_parent.display()),
    );
    environment.insert("LANG".to_owned(), "C.UTF-8".to_owned());
    environment.insert("LC_ALL".to_owned(), "C.UTF-8".to_owned());
    let config = BridgeConfig {
        codex_binary: state.config.codex_binary.clone(),
        codex_home,
        cwd: worktree.clone(),
        safe_environment: environment,
        max_line_bytes: 1_048_576,
        max_stderr_bytes: 262_144,
        request_timeout: Duration::from_secs(120),
    };
    let mut bridge = CodexBridge::spawn(config)
        .await
        .map_err(|error| provider_error(run_id, "spawn", error))?;
    bridge
        .initialize()
        .await
        .map_err(|error| provider_error(run_id, "initialize", error))?;
    Ok(bridge)
}

pub(crate) async fn inspect_provider_for_recovery(
    state: &AppState,
    run_id: Uuid,
    worktree: PathBuf,
) -> ProviderRecovery {
    let provider = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT ps.provider_thread_id, ep.codex_home_path
        FROM agent_runs ar
        JOIN provider_sessions ps ON ps.id = ar.provider_session_id
        JOIN execution_profiles ep ON ep.id = ar.execution_profile_id
        WHERE ar.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await;
    let Ok(Some((provider_thread_id, codex_home))) = provider else {
        return ProviderRecovery::Unavailable;
    };

    let bridge =
        spawn_initialized_bridge(state, PathBuf::from(codex_home), worktree.clone(), run_id).await;
    let Ok(mut bridge) = bridge else {
        return ProviderRecovery::Unavailable;
    };
    if bridge
        .request(
            "thread/resume",
            json!({"threadId": provider_thread_id, "cwd": worktree}),
        )
        .await
        .is_err()
    {
        bridge.shutdown().await;
        return ProviderRecovery::Unavailable;
    }
    let history = bridge
        .request(
            "thread/read",
            json!({"threadId": provider_thread_id, "includeTurns": true}),
        )
        .await;
    bridge.shutdown().await;
    let Ok(history) = history else {
        return ProviderRecovery::Unavailable;
    };
    let Some(latest_turn) = history
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.last())
    else {
        return ProviderRecovery::Resumed {
            latest_turn_status: "missing".to_owned(),
        };
    };
    let latest_turn_status = latest_turn
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    if latest_turn_status == "completed" {
        let message = latest_turn
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().rev().find_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("agentMessage"))
                        .then(|| item.get("text").and_then(Value::as_str))
                        .flatten()
                })
            })
            .map(|message| bounded_message(message, MAX_MESSAGE_BYTES))
            .unwrap_or_default();
        return ProviderRecovery::Completed { message };
    }
    ProviderRecovery::Resumed { latest_turn_status }
}

pub(crate) async fn complete_recovered_run(
    state: &AppState,
    run_id: Uuid,
    message: &str,
) -> Result<(), AppError> {
    let (project_id, conversation_id, provider_session_id) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            r#"
            SELECT c.project_id, ar.conversation_id, ar.provider_session_id
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
    transaction.commit().await?;
    complete_run(
        state,
        project_id,
        run_id,
        conversation_id,
        provider_session_id,
        message,
    )
    .await
}

fn bounded_message(message: &str, limit: usize) -> String {
    let mut end = message.len().min(limit);
    while !message.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    message[..end].to_owned()
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    state: AppState,
    mut bridge: CodexBridge,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    provider_session_id: Uuid,
    provider_thread_id: String,
    turn_id: String,
) -> Result<(), AppError> {
    let mut message = String::new();
    let message_stream_id = Uuid::now_v7();
    loop {
        tokio::select! {
            event = bridge.next_raw_event() => {
                let event = event.map_err(|error| provider_error(run_id, "read event", error))?;
                if event.get("id").is_some() && event.get("method").is_some() {
                    handle_provider_request(
                        &state,
                        &mut bridge,
                        project_id,
                        run_id,
                        conversation_id,
                        event,
                    ).await?;
                    continue;
                }
                let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
                match method {
                    "item/agentMessage/delta" => {
                        if let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) {
                            let remaining = MAX_MESSAGE_BYTES.saturating_sub(message.len());
                            let mut end = delta.len().min(remaining);
                            while !delta.is_char_boundary(end) {
                                end = end.saturating_sub(1);
                            }
                            let bounded = &delta[..end];
                            message.push_str(bounded);
                            state.realtime.publish(RealtimeEvent {
                                event_id: Uuid::now_v7(),
                                kind: "message.delta",
                                project_id,
                                conversation_id: Some(conversation_id),
                                payload: json!({
                                    "message_id": message_stream_id,
                                    "delta": bounded
                                }),
                            });
                        }
                    }
                    "item/started" => {
                        let item_type = event.pointer("/params/item/type").and_then(Value::as_str);
                        if item_type == Some("commandExecution") {
                            persist_event(
                                &state,
                                run_id,
                                conversation_id,
                                "command_started",
                                "Codex started a command in the isolated worktree.",
                                json!({}),
                            ).await?;
                        }
                    }
                    "item/completed" => {
                        let item_type = event.pointer("/params/item/type").and_then(Value::as_str);
                        match item_type {
                            Some("commandExecution") => {
                                persist_event(
                                    &state,
                                    run_id,
                                    conversation_id,
                                    "command_completed",
                                    "Codex completed a command in the isolated worktree.",
                                    json!({}),
                                ).await?;
                            }
                            Some("fileChange") => {
                                persist_event(
                                    &state,
                                    run_id,
                                    conversation_id,
                                    "file_change",
                                    "Codex completed a file change in the isolated worktree.",
                                    json!({}),
                                ).await?;
                            }
                            _ => {}
                        }
                    }
                    "turn/completed" => {
                        if event.pointer("/params/turn/status").and_then(Value::as_str)
                            != Some("completed")
                        {
                            bridge.shutdown().await;
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
                            provider_session_id,
                            &message,
                        ).await?;
                        bridge.shutdown().await;
                        return Ok(());
                    }
                    "error" => {
                        bridge.shutdown().await;
                        return Err(AppError::Conflict("provider", "Codex reported an error"));
                    }
                    _ => {}
                }
            }
            () = sleep(Duration::from_millis(500)) => {
                let run_state = sqlx::query_scalar::<_, String>(
                    "SELECT state FROM agent_runs WHERE id = $1",
                )
                .bind(run_id)
                .fetch_one(&state.pool)
                .await?;
                if run_state == "interrupted" {
                    let _ = bridge.request(
                        "turn/interrupt",
                        json!({"threadId": provider_thread_id, "turnId": turn_id}),
                    ).await;
                    bridge.shutdown().await;
                    return Ok(());
                }
            }
        }
    }
}

async fn handle_provider_request(
    state: &AppState,
    bridge: &mut CodexBridge,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    request: Value,
) -> Result<(), AppError> {
    let request_id = request
        .get("id")
        .cloned()
        .ok_or(AppError::Internal("provider request omitted id"))?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = if method == "item/tool/requestUserInput" {
        "question"
    } else {
        "approval"
    };
    let approval_id = Uuid::now_v7();
    let provider_request_id = request_id.to_string();
    let summary = if kind == "question" {
        "Codex needs an answer before it can continue."
    } else {
        "Codex needs permission before it can continue."
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options
        )
        SELECT $1, $2, ar.requesting_actor_id, $3, $4, $5, $6
        FROM agent_runs ar WHERE ar.id = $2
        "#,
    )
    .bind(approval_id)
    .bind(run_id)
    .bind(&provider_request_id)
    .bind(kind)
    .bind(summary)
    .bind(json!([
        {"id": "approve", "label": "Allow once"},
        {"id": "deny", "label": "Deny"}
    ]))
    .execute(&mut *transaction)
    .await?;
    transition_run(&mut transaction, run_id, RunState::Blocked, None).await?;
    let event_id = insert_event(
        &mut transaction,
        run_id,
        if kind == "question" {
            "question_requested"
        } else {
            "approval_requested"
        },
        summary,
        json!({"approval_id": approval_id}),
    )
    .await?;
    insert_entry(&mut transaction, conversation_id, event_id).await?;
    transaction.commit().await?;
    publish(state, project_id, conversation_id, run_id);

    loop {
        sleep(Duration::from_millis(300)).await;
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
            bridge
                .respond(request_id, json!({"decision": "decline"}))
                .await
                .map_err(|error| provider_error(run_id, "decline interrupted request", error))?;
            return Ok(());
        }
        if resolution.status != "resolved" {
            continue;
        }
        let approved = resolution
            .response
            .as_ref()
            .and_then(|value| value.get("decision"))
            .and_then(Value::as_str)
            == Some("approve");
        let result = if kind == "question" {
            let mut answers = serde_json::Map::new();
            if let Some(questions) = request
                .pointer("/params/questions")
                .and_then(Value::as_array)
            {
                for question in questions {
                    if let Some(id) = question.get("id").and_then(Value::as_str) {
                        answers.insert(
                            id.to_owned(),
                            json!({"answers": [if approved { "approved" } else { "denied" }]}),
                        );
                    }
                }
            }
            json!({"answers": answers})
        } else {
            json!({"decision": if approved { "accept" } else { "decline" }})
        };
        bridge
            .respond(request_id, result)
            .await
            .map_err(|error| provider_error(run_id, "answer provider request", error))?;
        return Ok(());
    }
}

async fn complete_run(
    state: &AppState,
    project_id: Uuid,
    run_id: Uuid,
    conversation_id: Uuid,
    provider_session_id: Uuid,
    message: &str,
) -> Result<(), AppError> {
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
    if !message.is_empty() {
        let message_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO messages (
                id, conversation_id, author_actor_id, content,
                plain_text, agent_run_id
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(worker_actor_id)
        .bind(json!({"version": 1, "nodes": [{"type": "text", "text": message}]}))
        .bind(message)
        .bind(run_id)
        .execute(&mut *transaction)
        .await?;
        let sequence = allocate_conversation_sequence(&mut transaction, conversation_id).await?;
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
        .bind(message_id)
        .execute(&mut *transaction)
        .await?;
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
    sqlx::query("UPDATE provider_sessions SET state = 'idle', updated_at = now() WHERE id = $1")
        .bind(provider_session_id)
        .execute(&mut *transaction)
        .await?;
    let event_id = insert_event(
        &mut transaction,
        run_id,
        "run_completed",
        "The hosted Codex turn completed.",
        json!({}),
    )
    .await?;
    insert_entry(&mut transaction, conversation_id, event_id).await?;
    transaction.commit().await?;
    publish(state, project_id, conversation_id, run_id);
    Ok(())
}

async fn persist_event(
    state: &AppState,
    run_id: Uuid,
    conversation_id: Uuid,
    kind: &str,
    summary: &str,
    payload: Value,
) -> Result<(), AppError> {
    let mut transaction = state.pool.begin().await?;
    let event_id = insert_event(&mut transaction, run_id, kind, summary, payload).await?;
    insert_entry(&mut transaction, conversation_id, event_id).await?;
    transaction.commit().await?;
    let project_id =
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM conversations WHERE id = $1")
            .bind(conversation_id)
            .fetch_one(&state.pool)
            .await?;
    publish(state, project_id, conversation_id, run_id);
    Ok(())
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    kind: &str,
    summary: &str,
    payload: Value,
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
    let event_id = insert_event(&mut transaction, run_id, "run_failed", reason, json!({})).await?;
    insert_entry(&mut transaction, conversation_id, event_id).await?;
    transaction.commit().await?;
    publish(state, project_id, conversation_id, run_id);
    Ok(())
}

fn publish(state: &AppState, project_id: Uuid, conversation_id: Uuid, run_id: Uuid) {
    state.realtime.publish(RealtimeEvent {
        event_id: Uuid::now_v7(),
        kind: "timeline.updated",
        project_id,
        conversation_id: Some(conversation_id),
        payload: json!({"run_id": run_id}),
    });
}

fn provider_error(run_id: Uuid, operation: &'static str, error: impl std::fmt::Debug) -> AppError {
    tracing::error!(%run_id, operation, ?error, "Codex App Server operation failed");
    AppError::Conflict("provider", "Codex App Server operation failed")
}
