//! Agent sessions (A3): the session an agent works in for a thread, so the
//! thread can link to it ("Codex Cloud · Open session") and a runner resumes
//! it instead of starting cold. One row per agent per thread, whichever way
//! the work started: a harness run records its cloud task here, and an
//! external runner binds its own Claude Code or Codex session with
//! `session_bind`. Kept apart from Agent Runs and provider sessions, as
//! AGENTS.md requires.

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/conversations/{conversation_id}/agent-sessions",
        get(list_sessions),
    )
}

/// What a runner or a thread header needs to know about a session.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub agent_actor_id: Uuid,
    /// The agent's name in the project.
    pub agent_name: String,
    /// "harness_run" or "runner".
    pub source: String,
    /// What runs it, such as codex_cloud, claude_code or codex.
    pub harness: String,
    /// The harness's name for people, such as "Codex Cloud".
    #[sqlx(default)]
    pub harness_name: String,
    /// The runner's or harness's own reference, such as a task or session
    /// ID; resume this one.
    pub external_ref: String,
    /// A page people can open to watch the session.
    pub url: Option<String>,
    /// active, idle, ended or cancelled.
    pub state: String,
    pub agent_run_id: Option<Uuid>,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const SESSION_SELECT: &str = r#"
    SELECT session.id, session.conversation_id, session.agent_actor_id,
           COALESCE(membership.display_name, 'Agent') AS agent_name,
           session.source, session.harness, session.external_ref, session.url,
           session.state, session.agent_run_id, session.last_activity_at,
           session.created_at
    FROM agent_sessions session
    LEFT JOIN project_memberships membership
      ON membership.project_id = session.project_id
     AND membership.actor_id = session.agent_actor_id
"#;

fn named(state: &AppState, mut session: SessionView) -> SessionView {
    session.harness_name = harness_name(state, &session.harness);
    session
}

/// A name people know a harness by, for ones the server runs and the usual
/// runner kinds.
pub(crate) fn harness_name(state: &AppState, harness: &str) -> String {
    if let Some(manifest) = state.supervisors.manifest(harness) {
        return manifest.display_name.clone();
    }
    match harness {
        "claude_code" => "Claude Code".to_owned(),
        "codex" => "Codex".to_owned(),
        other => other.replace('_', " "),
    }
}

/// The sessions in a conversation the caller can see, newest first.
pub(crate) async fn sessions_in(
    state: &AppState,
    actor_id: Uuid,
    conversation_id: Uuid,
) -> Result<Vec<SessionView>, AppError> {
    let rows = sqlx::query_as::<_, SessionView>(&format!(
        r#"{SESSION_SELECT}
        WHERE session.conversation_id = $1
          AND conversation_visible_to(session.conversation_id, $2)
        ORDER BY session.last_activity_at DESC, session.id
        LIMIT 20
        "#
    ))
    .bind(conversation_id)
    .bind(actor_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(|row| named(state, row)).collect())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionList {
    sessions: Vec<SessionView>,
}

async fn list_sessions(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
) -> Result<Json<SessionList>, AppError> {
    let visible = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM conversations conversation
            JOIN projects project ON project.id = conversation.project_id
            JOIN organization_memberships membership
              ON membership.organization_id = project.organization_id
             AND membership.actor_id = $2
             AND membership.enabled
            WHERE conversation.id = $1
              AND conversation_visible_to(conversation.id, $2)
        )
        "#,
    )
    .bind(conversation_id)
    .bind(principal.actor_id)
    .fetch_one(&state.pool)
    .await?;
    if !visible {
        return Err(AppError::NotFound("conversation"));
    }
    Ok(Json(SessionList {
        sessions: sessions_in(&state, principal.actor_id, conversation_id).await?,
    }))
}

/// The session an agent holds in a conversation, for its claim result and
/// wakeups, so it resumes rather than starting cold.
pub(crate) async fn session_for(
    executor: impl PgExecutor<'_>,
    conversation_id: Uuid,
    agent_actor_id: Uuid,
) -> Result<Option<SessionView>, AppError> {
    Ok(sqlx::query_as::<_, SessionView>(&format!(
        r#"{SESSION_SELECT}
        WHERE session.conversation_id = $1 AND session.agent_actor_id = $2
        "#
    ))
    .bind(conversation_id)
    .bind(agent_actor_id)
    .fetch_optional(executor)
    .await?)
}

/// What binding a session needs.
pub(crate) struct Binding<'a> {
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub agent_actor_id: Uuid,
    pub source: &'static str,
    pub harness: &'a str,
    pub external_ref: &'a str,
    pub url: Option<&'a str>,
    pub state: &'a str,
    pub agent_run_id: Option<Uuid>,
    pub reply_request_id: Option<Uuid>,
}

/// Checks a session reference and link before they are stored.
pub(crate) fn valid_reference(external_ref: &str, url: Option<&str>) -> Result<(), AppError> {
    if external_ref.is_empty()
        || external_ref.chars().count() > 200
        || external_ref.chars().any(char::is_control)
    {
        return Err(AppError::Validation("externalRef"));
    }
    if let Some(url) = url
        && (url.len() > 2000
            || !url.starts_with("https://")
            || url.len() <= "https://".len()
            || url.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        return Err(AppError::Validation("url"));
    }
    Ok(())
}

/// Creates or moves the agent's session in the conversation. The caller
/// checked that the agent may work there.
pub(crate) async fn bind(
    transaction: &mut Transaction<'_, Postgres>,
    binding: Binding<'_>,
) -> Result<Uuid, AppError> {
    valid_reference(binding.external_ref, binding.url)?;
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO agent_sessions (
            id, organization_id, project_id, conversation_id, agent_actor_id, source,
            harness, external_ref, url, state, agent_run_id, reply_request_id
        )
        SELECT $1, project.organization_id, project.id, $3, $4, $5, $6, $7, $8, $9, $10, $11
        FROM projects project WHERE project.id = $2
        ON CONFLICT (conversation_id, agent_actor_id) DO UPDATE
        SET source = EXCLUDED.source,
            harness = EXCLUDED.harness,
            external_ref = EXCLUDED.external_ref,
            url = COALESCE(EXCLUDED.url,
                           CASE WHEN agent_sessions.external_ref = EXCLUDED.external_ref
                                THEN agent_sessions.url END),
            state = EXCLUDED.state,
            agent_run_id = COALESCE(EXCLUDED.agent_run_id, agent_sessions.agent_run_id),
            reply_request_id = COALESCE(EXCLUDED.reply_request_id, agent_sessions.reply_request_id),
            last_activity_at = now(),
            updated_at = now()
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(binding.project_id)
    .bind(binding.conversation_id)
    .bind(binding.agent_actor_id)
    .bind(binding.source)
    .bind(binding.harness)
    .bind(binding.external_ref)
    .bind(binding.url)
    .bind(binding.state)
    .bind(binding.agent_run_id)
    .bind(binding.reply_request_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// A harness run's turn started: its thread now has the run's session (the
/// execution worker's), pointing at the turn.
pub(crate) async fn record_run_turn(
    state: &AppState,
    run_id: Uuid,
    turn_id: &str,
    url: Option<&str>,
) -> Result<(), AppError> {
    let Some((project_id, conversation_id, worker_actor_id, harness)) =
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
            r#"
            SELECT run.project_id, run.conversation_id, worker.actor_id,
                   COALESCE(run.config ->> 'harness', account.adapter_kind)
            FROM agent_runs run
            JOIN execution_workers worker ON worker.id = run.execution_worker_id
            JOIN execution_profiles profile ON profile.id = run.execution_profile_id
            JOIN provider_accounts account ON account.id = profile.provider_account_id
            WHERE run.id = $1
            "#,
        )
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
    else {
        return Ok(());
    };
    // Only turns people can open somewhere get a session row; a local turn
    // is already the run card.
    let Some(url) = url else {
        return Ok(());
    };
    let reference = turn_id.chars().take(200).collect::<String>();
    if valid_reference(&reference, Some(url)).is_err() {
        return Ok(());
    }
    let mut transaction = state.pool.begin().await?;
    bind(
        &mut transaction,
        Binding {
            project_id,
            conversation_id,
            agent_actor_id: worker_actor_id,
            source: "harness_run",
            harness: &harness,
            external_ref: &reference,
            url: Some(url),
            state: "active",
            agent_run_id: Some(run_id),
            reply_request_id: None,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// A run ended: its session goes idle (it can be resumed by the next run).
pub(crate) async fn idle_run_session(
    executor: impl PgExecutor<'_>,
    run_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE agent_sessions SET state = 'idle', updated_at = now()
        WHERE agent_run_id = $1 AND state = 'active'
        "#,
    )
    .bind(run_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// A person pressed Stop on a request: the session working on it is
/// cancelled too.
pub(crate) async fn cancel_for_request(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE agent_sessions session
        SET state = 'cancelled', updated_at = now()
        FROM reply_requests request
        WHERE request.id = $1
          AND session.agent_actor_id = request.agent_actor_id
          AND session.conversation_id = COALESCE(request.thread_id, request.conversation_id)
          AND session.state = 'active'
        "#,
    )
    .bind(request_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
