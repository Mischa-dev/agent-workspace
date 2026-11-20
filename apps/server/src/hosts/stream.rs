//! A1.17: a reply's words as they arrive, from any host. The run's own
//! credential sends them; the server shows them live to the sockets of the
//! run's conversation on every instance and keeps nothing (the finished
//! reply is sent as a message, the only place its words are stored).

use agent_workspace_host_protocol::{MAX_STREAM_DELTA_BYTES, StreamDelta};
use serde_json::json;
use uuid::Uuid;

use super::protocol::HostError;
use crate::{AppState, realtime::RealtimeEvent, runs};

/// Shows one piece of a run's streamed reply. Refused for another run's
/// credential, a run that has ended, or a run with no conversation.
pub async fn delta(
    state: &AppState,
    token: &str,
    run_id: Uuid,
    delta: StreamDelta,
) -> Result<(), HostError> {
    // The run's own token (K1.24's narrow token bound to the run).
    let holder = crate::access::tokens::resolve(&state.pool, token)
        .await?
        .filter(|holder| holder.run_id == Some(run_id))
        .ok_or(HostError::Unauthorized)?;
    if delta.delta.is_empty() || delta.delta.len() > MAX_STREAM_DELTA_BYTES {
        return Err(HostError::Invalid {
            field: "delta",
            message: format!("Send 1 to {MAX_STREAM_DELTA_BYTES} bytes at a time."),
        });
    }
    let run = runs::store::get(crate::db::scoped(&state.pool), run_id)
        .await?
        .ok_or(HostError::Unauthorized)?;
    if runs::RunState::terminal_keys().contains(&run.state.as_str()) {
        return Err(HostError::Invalid {
            field: "runId",
            message: "The run has ended.".to_owned(),
        });
    }
    let Some(conversation_id) = run.conversation_id else {
        return Err(HostError::Invalid {
            field: "runId",
            message: "The run has no conversation to show its reply in.".to_owned(),
        });
    };
    crate::realtime::ephemeral::stream_delta(
        state,
        RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: holder.space_id,
            conversation_id: Some(conversation_id),
            payload: json!({
                "message_id": delta.stream_id,
                "agent_run_id": run_id,
                "delta": delta.delta,
            }),
        },
    )
    .await;
    Ok(())
}
