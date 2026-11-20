//! A1.25: the built-in host's assistant, an agent that needs no setup. On a
//! workspace with a model provider (and its key) set, the directory offers
//! it; adding it is one step, and it joins with the role the person picks.
//! It is bound to the built-in host under a name starting [`AGENT_REF_PREFIX`], so a DM or a tag
//! launches a run there like any host's agent.
//!
//! Its turns think with the workspace's own model through the server's
//! model gateway (`POST /api/v1/host/runs/{run}/model`, the run's
//! credential): the key stays in the server's secrets store and never
//! reaches the host, a child process, a command or the browser. Each call
//! is counted on the run and in `ai_usage`, so the workspace's monthly
//! model budget covers it. The assistant acts through the agent tools as
//! itself, with the same run credential, like any agent.
//!
//! With no provider or key, the offer says what to set and adds nothing.

use agent_workspace_host_protocol::{
    Ack, AckOutcome, Command, MAX_MODEL_MESSAGES, MAX_MODEL_TOOLS, ModelReply, ModelTurn,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    ai::gateway::{self, AiError, Scope, Tier},
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    runs::ops::{self, Reporter, UsageInput},
};

/// How the built-in host's own names for its assistants start: each is
/// `assistant:` and the proposal that added it.
pub const AGENT_REF_PREFIX: &str = "assistant:";
/// The offer's id in `agent_offer_list`.
pub const OFFER_ID: &str = "assistant";
pub const DEFAULT_NAME: &str = "Assistant";
pub const SUMMARY: &str = "Answers messages and mentions and works with tasks and pages, on this \
    workspace's model. Nothing to set up.";

/// The built-in host's name for one assistant.
pub fn agent_ref(proposal_id: Uuid) -> String {
    format!("{AGENT_REF_PREFIX}{proposal_id}")
}

/// Whether a built-in host's name for an agent is an assistant's.
pub fn is_assistant_ref(reference: &str) -> bool {
    reference.starts_with(AGENT_REF_PREFIX)
}

/// The assistant offer as `agent_offer_list` shows it: ready to add, or
/// what to set first.
pub async fn offer(state: &AppState, workspace_id: Uuid) -> Result<Value, AppError> {
    let ready = gateway::resolve(
        state,
        Scope {
            organization_id: workspace_id,
            project_id: None,
            actor_id: None,
        },
        Tier::Deep,
    )
    .await;
    let mut offer = json!({
        "id": OFFER_ID,
        "title": DEFAULT_NAME,
        "summary": SUMMARY,
        "start": {"adds": true},
    });
    match ready {
        Ok(_) => {}
        Err(AiError::NotConfigured(message)) => {
            offer["unavailable"] = json!(format!(
                "{message} The assistant thinks with this workspace's model."
            ));
        }
        Err(AiError::App(error)) => return Err(error),
        Err(other) => offer["unavailable"] = json!(other.message()),
    }
    Ok(offer)
}

/// Whether the workspace can run the assistant now: what to set when not.
pub async fn unavailable(state: &AppState, workspace_id: Uuid) -> Result<Option<String>, AppError> {
    Ok(offer(state, workspace_id).await?["unavailable"]
        .as_str()
        .map(str::to_owned))
}

/// The workspace's built-in host row.
pub async fn builtin_host_id(
    connection: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM hosts WHERE owner_workspace_id = $1 AND builtin AND status <> 'revoked' \
         ORDER BY created_at LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A free handle like `assistant`, `assistant-2`, ... in the workspace.
pub async fn free_handle(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    wanted: &str,
) -> Result<String, AppError> {
    for attempt in 1..=50 {
        let handle = if attempt == 1 {
            wanted.to_owned()
        } else {
            format!("{wanted}-{attempt}")
        };
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
             WHERE organization_id = $1 AND lower(handle) = lower($2))",
        )
        .bind(workspace_id)
        .bind(&handle)
        .fetch_one(&mut **transaction)
        .await?;
        if !taken {
            return Ok(handle);
        }
    }
    Err(AppError::Validation("handle"))
}

/// Why a model call was turned away, as the host protocol answers it.
pub enum ModelRefusal {
    Unauthorized,
    NotAssistant,
    Invalid(&'static str, String),
    Ai(AiError),
    App(AppError),
}

impl From<sqlx::Error> for ModelRefusal {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<AppError> for ModelRefusal {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl IntoResponse for ModelRefusal {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Send the run's credential; it ends with the run.".to_owned(),
            ),
            Self::NotAssistant => (
                StatusCode::FORBIDDEN,
                "not_assistant",
                "Only the built-in assistant's runs think with the workspace's model.".to_owned(),
            ),
            Self::Invalid(field, message) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(
                        json!({"error": "invalid", "code": "invalid", "field": field,
                                "message": message}),
                    ),
                )
                    .into_response();
            }
            Self::Ai(error) => return error.into_response(),
            Self::App(error) => return error.into_response(),
        };
        (
            status,
            Json(json!({"error": code, "code": code, "message": message})),
        )
            .into_response()
    }
}

/// One model turn for an assistant run, by the run's credential: the
/// workspace's model answers, and its tokens go on the run and in
/// `ai_usage`.
pub async fn model_turn(
    state: &AppState,
    token: &str,
    run_id: Uuid,
    turn: ModelTurn,
) -> Result<ModelReply, ModelRefusal> {
    let holder = crate::access::tokens::resolve(&state.pool, token)
        .await?
        .filter(|holder| holder.run_id == Some(run_id))
        .ok_or(ModelRefusal::Unauthorized)?;
    let (workspace_id, space_id) = {
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let found = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            r#"
            SELECT run.workspace_id, run.space_id
            FROM access_tokens credential
            JOIN runs run ON run.id = credential.run_id
            JOIN hosts host ON host.id = credential.host_id AND host.builtin
            JOIN host_agents binding
              ON binding.host_id = credential.host_id
             AND binding.account_id = credential.account_id
             AND binding.status = 'active' AND starts_with(binding.host_agent_ref, $2)
            WHERE credential.id = $1
            "#,
        )
        .bind(holder.token_id)
        .bind(AGENT_REF_PREFIX)
        .fetch_optional(&mut *connection)
        .await?;
        found.ok_or(ModelRefusal::NotAssistant)?
    };
    if turn.messages.is_empty() || turn.messages.len() > MAX_MODEL_MESSAGES {
        return Err(ModelRefusal::Invalid(
            "messages",
            format!("Send 1 to {MAX_MODEL_MESSAGES} messages."),
        ));
    }
    if turn.tools.len() > MAX_MODEL_TOOLS {
        return Err(ModelRefusal::Invalid(
            "tools",
            format!("Offer at most {MAX_MODEL_TOOLS} tools."),
        ));
    }
    let answer = gateway::chat(
        state,
        Scope {
            organization_id: workspace_id,
            project_id: space_id,
            actor_id: Some(holder.account_id),
        },
        &turn,
    )
    .await
    .map_err(ModelRefusal::Ai)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let recorded = ops::record_usage(
        &mut transaction,
        Reporter::Account {
            account_id: holder.account_id,
        },
        UsageInput {
            report_key: format!("model-{}", Uuid::now_v7()),
            run_id: Some(run_id),
            window: None,
            space_id: space_id.unwrap_or(workspace_id),
            model: answer.reply.model.clone(),
            input_tokens: i64::try_from(answer.reply.input_tokens).unwrap_or(i64::MAX),
            output_tokens: i64::try_from(answer.reply.output_tokens).unwrap_or(i64::MAX),
            cached_input_tokens: 0,
            cost: (answer.cost_micros > 0).then(|| answer.cost_micros as f64 / 1_000_000.0),
            currency: (answer.cost_micros > 0).then(|| "USD".to_owned()),
        },
    )
    .await;
    match recorded {
        Ok(_) => transaction.commit().await.map_err(AppError::from)?,
        // The run ended while the model answered: what it cost is in
        // `ai_usage`; the run takes no more.
        Err(error) => {
            tracing::info!(%run_id, ?error, "an assistant turn's usage wasn't added to its run");
        }
    }
    Ok(answer.reply)
}

/// The job that carries out one assistant run.
pub const TURN_KIND: &str = "hosts.assistant_turn";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Turn {
    run_id: Uuid,
    command_id: Uuid,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        TURN_KIND,
        std::time::Duration::from_secs(20 * 60),
        |context: JobContext| async move {
            let turn: Turn = serde_json::from_value(context.job.payload.clone())
                .map_err(|_| JobError::permanent("payload"))?;
            carry_out(&context.state, turn.run_id, turn.command_id)
                .await
                .map_err(|error| JobError::permanent(error.log_category()))
        },
    );
}

/// Whether a start is for one of the built-in host's assistants.
pub fn is_assistant_start(command: &Command) -> bool {
    command
        .detail
        .get("agent")
        .and_then(|agent| agent.get("hostAgentRef"))
        .and_then(Value::as_str)
        .is_some_and(is_assistant_ref)
}

/// The built-in host took an assistant's start: its turn runs as a job
/// (it thinks for a while; the command pump goes on). Once only, by run.
pub async fn accept_start(state: &AppState, command: &Command) -> Ack {
    let Some(run_id) = command.run_id else {
        let mut ack = Ack::new(AckOutcome::Refused);
        ack.reason = Some("invalid".to_owned());
        ack.message = Some("A start needs its run.".to_owned());
        return ack;
    };
    let queued = async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let workspace_id =
            sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM runs WHERE id = $1")
                .bind(run_id)
                .fetch_one(&mut *transaction)
                .await?;
        crate::jobs::enqueue(
            &mut *transaction,
            NewJob::new(TURN_KIND)
                .workspace(workspace_id)
                .payload(Turn {
                    run_id,
                    command_id: command.command_id,
                })
                .unique(format!("assistant.turn:{run_id}"))
                .max_attempts(1),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(())
    }
    .await;
    match queued {
        Ok(()) => Ack::new(AckOutcome::Accepted),
        Err(error) => {
            tracing::warn!(%run_id, error_category = error.log_category(), "an assistant's start wasn't queued");
            Ack::new(AckOutcome::Uncertain)
        }
    }
}

/// One assistant run, as the built-in host carries it out: its start is
/// fetched again for a fresh credential, and the crate's assistant does
/// the rest through the protocol.
async fn carry_out(state: &AppState, run_id: Uuid, command_id: Uuid) -> Result<(), AppError> {
    let client = super::transport::client(state);
    let Ok(command) = client.command(command_id).await else {
        // The run ended before its turn came: nothing to do.
        return Ok(());
    };
    let Some(credential) = command.workspace_credential.as_ref() else {
        return Ok(());
    };
    let name = sqlx::query_scalar::<_, String>(
        r#"
        SELECT COALESCE(profile.display_name, 'Assistant')
        FROM runs run
        LEFT JOIN organization_actor_profiles profile
          ON profile.organization_id = run.workspace_id AND profile.actor_id = run.account_id
        WHERE run.id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .unwrap_or_else(|| DEFAULT_NAME.to_owned());
    let reporter = client.reporter(run_id, credential);
    let start =
        agent_workspace_builtin_host::assistant::AssistantStart::from_detail(name, &command.detail);
    let outcome = agent_workspace_builtin_host::assistant::run(&reporter, &start).await;
    tracing::info!(%run_id, ?outcome, "an assistant run ended");
    Ok(())
}
