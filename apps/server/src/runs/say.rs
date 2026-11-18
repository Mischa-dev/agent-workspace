//! What a run's agent says in its thread. A host reports it as a
//! `text_message` activity with `inThread: true`; the activity is kept in
//! the run's log like any other, and a job posts it in the run's
//! conversation as the run's account, through the one send path, linked to
//! the run. A run launched from a reply request answers that request with
//! its first message there (claiming it, so a channel tag gets its answer
//! thread); what it says after goes to the same place.
//!
//! The job carries ids only; the words are read from the run's log when it
//! runs (rule 10). Each message is posted once: its key is the run and the
//! activity's own id.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{launch_paths, store};
use crate::{
    AppState,
    conversations::{AppendExtras, MessageCommandActor, ReplyCompletion},
    error::AppError,
    idempotency::IdempotencyKey,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
};

pub const SAY_KIND: &str = "runs.say";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Say {
    run_id: Uuid,
    activity_id: String,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(SAY_KIND, |context: JobContext| async move {
        let say: Say = serde_json::from_value(context.job.payload.clone())
            .map_err(|_| JobError::permanent("payload"))?;
        post(&context.state, say.run_id, &say.activity_id)
            .await
            .map_err(|error| match error {
                // The thread or the account is gone, or the agent may no
                // longer post there: nothing to try again.
                AppError::NotFound(_) | AppError::Forbidden | AppError::Refused { .. } => {
                    JobError::permanent(error.log_category())
                }
                error => JobError::retry(error.log_category()),
            })
    });
}

/// Whether an activity is something the agent says in its thread.
pub fn is_said_in_thread(kind: &str, payload: &Value) -> bool {
    kind == "text_message" && payload.get("inThread").and_then(Value::as_bool) == Some(true)
}

/// [`enqueue`] for a run known by its id.
pub async fn enqueue_for(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    activity_id: &str,
) -> Result<(), AppError> {
    match store::get(&mut **transaction, run_id).await? {
        Some(run) => enqueue(transaction, &run, activity_id).await,
        None => Ok(()),
    }
}

/// Queue the post for an activity just kept, in the same transaction.
pub async fn enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    run: &store::RunRow,
    activity_id: &str,
) -> Result<(), AppError> {
    let (Some(space_id), Some(_)) = (run.space_id, run.conversation_id) else {
        return Ok(());
    };
    crate::jobs::enqueue(
        &mut **transaction,
        NewJob::new(SAY_KIND)
            .project(run.workspace_id, space_id)
            .payload(Say {
                run_id: run.id,
                activity_id: activity_id.to_owned(),
            })
            .max_attempts(10),
    )
    .await?;
    Ok(())
}

/// Post one activity's words in the run's thread.
pub async fn post(state: &AppState, run_id: Uuid, activity_id: &str) -> Result<(), AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(run) = store::get(&mut *transaction, run_id).await? else {
        return Ok(());
    };
    let (Some(space_id), Some(conversation_id)) = (run.space_id, run.conversation_id) else {
        return Ok(());
    };
    let Some(text) = super::activities::said_text(&mut *transaction, run.id, activity_id).await?
    else {
        return Ok(());
    };
    let text: String = text
        .trim()
        .chars()
        .take(crate::message_content::MAX_MESSAGE_TEXT_CHARACTERS)
        .collect();
    if text.is_empty() {
        return Ok(());
    }
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
        .bind(run.account_id)
        .fetch_one(&mut *transaction)
        .await?;
    let actor_kind = crate::launch::actor_kind(&kind).ok_or(AppError::Forbidden)?;
    // A launched run answers its request: claimed now (its first message
    // there opens a channel tag's answer thread), answered by this message.
    let mut reply = None;
    let mut target = conversation_id;
    if let Some((request_id, status)) = launch_paths::reply_of(&mut transaction, run.id).await? {
        let creator = crate::conversations::threads::ThreadCreator::plain(
            run.account_id,
            actor_kind,
            None,
            "run",
            None,
        );
        if matches!(status.as_str(), "pending" | "claimed") {
            if let Some((conversation, thread)) = crate::mcp::replies::claim_in(
                &mut transaction,
                &creator,
                request_id,
                space_id,
                run.account_id,
            )
            .await?
            {
                target = thread.unwrap_or(conversation);
                let generation = sqlx::query_scalar::<_, i32>(
                    "SELECT generation FROM reply_requests WHERE id = $1",
                )
                .bind(request_id)
                .fetch_one(&mut *transaction)
                .await?;
                reply = Some(ReplyCompletion {
                    request_id,
                    generation,
                    final_reply: true,
                });
            }
        } else if let Some(place) = sqlx::query_scalar::<_, Uuid>(
            "SELECT COALESCE(thread_id, conversation_id) FROM reply_requests WHERE id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            target = place;
        }
    }
    transaction.commit().await?;
    let content = crate::mcp::common::content_from_text(state, space_id, &text).await?;
    let key = IdempotencyKey::parse(&format!("run-say-{}-{}", run.id, short(activity_id)))
        .map_err(|_| AppError::Internal("run say key"))?;
    crate::conversations::append_message_command_with(
        state,
        MessageCommandActor {
            actor_id: run.account_id,
            actor_kind: kind,
            mcp_credential: None,
            via_grant_id: None,
            run_id: Some(run.id),
        },
        target,
        None,
        key,
        content,
        &[],
        reply,
        AppendExtras {
            agent_run_id: (target == conversation_id).then_some(run.id),
            ..AppendExtras::default()
        },
    )
    .await?;
    Ok(())
}

/// An activity id in a key: as it is when short and plain, else its digest.
fn short(activity_id: &str) -> String {
    if activity_id.len() <= 64
        && activity_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        activity_id.to_owned()
    } else {
        use sha2::Digest;
        sha2::Sha256::digest(activity_id.as_bytes())
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
