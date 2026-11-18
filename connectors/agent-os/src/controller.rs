//! Agent OS's controller, called through its public interface: a request
//! file handed to the configured command (an argument array, never a
//! shell line), answered with an envelope on standard output.

use std::{future::Future, process::Stdio, time::Duration};

use serde_json::{Value, json};

use crate::config::AgentOs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    /// Agent OS answered and said no, with its issue code. Calling again
    /// gives the same answer.
    Rejected(String),
    /// No readable answer: the call may or may not have happened. Keep the
    /// receipt and call again with the same idempotency key.
    Uncertain(String),
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(code) => write!(formatter, "Agent OS refused: {code}"),
            Self::Uncertain(why) => write!(formatter, "Agent OS answer unknown: {why}"),
        }
    }
}

/// One controller call: an operation and its fields, answered with its
/// result.
pub trait Controller: Send + Sync {
    fn call(&self, command: Value) -> impl Future<Output = Result<Value, ControllerError>> + Send;
}

/// The controller as a local command.
pub struct CommandController {
    settings: AgentOs,
    scratch: std::path::PathBuf,
}

impl CommandController {
    pub fn new(settings: AgentOs, scratch: std::path::PathBuf) -> Self {
        Self { settings, scratch }
    }
}

impl Controller for CommandController {
    async fn call(&self, command: Value) -> Result<Value, ControllerError> {
        let request = json!({
            "protocol_version": 1,
            "context": self.settings.context,
            "command": command,
        });
        let path = self
            .scratch
            .join(format!("request-{}.json", uuid::Uuid::now_v7().simple()));
        crate::receipts::write_private(&path, &request)
            .map_err(|error| ControllerError::Uncertain(format!("writing the request: {error}")))?;
        let mut program = tokio::process::Command::new(&self.settings.command[0]);
        program
            .args(&self.settings.command[1..])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let ran = tokio::time::timeout(
            Duration::from_secs(self.settings.timeout_seconds),
            program.output(),
        )
        .await;
        let _ = std::fs::remove_file(&path);
        let output = match ran {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(ControllerError::Uncertain(error.to_string())),
            Err(_) => return Err(ControllerError::Uncertain("timed out".to_owned())),
        };
        read_envelope(output.status.success(), &output.stdout)
    }
}

/// The controller's envelope: `{"status": "ok", "result": …}` or an
/// `issue` with a `code`.
pub fn read_envelope(succeeded: bool, stdout: &[u8]) -> Result<Value, ControllerError> {
    let Ok(envelope) = serde_json::from_slice::<Value>(stdout) else {
        return Err(ControllerError::Uncertain("unreadable answer".to_owned()));
    };
    if succeeded && envelope["status"] == "ok" {
        return Ok(envelope["result"].clone());
    }
    match envelope["issue"]["code"].as_str() {
        Some(code) => Err(ControllerError::Rejected(code.to_owned())),
        None => Err(ControllerError::Uncertain(
            "no result and no issue".to_owned(),
        )),
    }
}

/// The deployment's current source revision, which a task is submitted
/// against (also how `check` proves the controller answers).
pub async fn source_revision(controller: &impl Controller) -> Result<String, ControllerError> {
    let plan = controller.call(json!({"operation": "plan"})).await?;
    plan["source_revision"]
        .as_str()
        .filter(|revision| !revision.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ControllerError::Uncertain("the plan has no source revision".to_owned()))
}
