use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};

const CLIENT_NAME: &str = "agent_workspace";

#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub codex_binary: PathBuf,
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub safe_environment: BTreeMap<String, String>,
    pub max_line_bytes: usize,
    pub max_stderr_bytes: usize,
    pub request_timeout: Duration,
}

impl BridgeConfig {
    pub fn validate(&self) -> Result<(), BridgeError> {
        for (label, path) in [
            ("codex binary", self.codex_binary.as_path()),
            ("CODEX_HOME", self.codex_home.as_path()),
            ("working directory", self.cwd.as_path()),
        ] {
            if !path.is_absolute() {
                return Err(BridgeError::InvalidConfig(format!(
                    "{label} must be an absolute path"
                )));
            }
        }
        if self.max_line_bytes == 0 || self.max_stderr_bytes == 0 || self.request_timeout.is_zero()
        {
            return Err(BridgeError::InvalidConfig(
                "limits and request timeout must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("invalid bridge configuration: {0}")]
    InvalidConfig(String),
    #[error("failed to start Codex App Server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Codex App Server transport failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Codex App Server returned malformed JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Codex App Server line exceeded {limit} bytes")]
    OversizedLine { limit: usize },
    #[error("Codex App Server request timed out")]
    Timeout,
    #[error("Codex App Server closed stdout")]
    Closed,
    #[error("Codex App Server rejected request: {0}")]
    Rpc(Value),
    #[error("Codex App Server response did not include a result")]
    MissingResult,
}

pub struct CodexBridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
    next_id: u64,
    max_line_bytes: usize,
    request_timeout: Duration,
    notifications: Vec<Value>,
}

impl CodexBridge {
    pub async fn spawn(config: BridgeConfig) -> Result<Self, BridgeError> {
        config.validate()?;

        let mut command = Command::new(&config.codex_binary);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .current_dir(&config.cwd)
            .env_clear()
            .env("CODEX_HOME", &config.codex_home)
            .env("PWD", &config.cwd)
            .envs(&config.safe_environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }

        let mut child = command.spawn().map_err(BridgeError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::InvalidConfig("Codex stdin was not piped".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::InvalidConfig("Codex stdout was not piped".to_owned()))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| BridgeError::InvalidConfig("Codex stderr was not piped".to_owned()))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_task =
            spawn_bounded_stderr(stderr_pipe, Arc::clone(&stderr), config.max_stderr_bytes);

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
            stderr_task,
            next_id: 1,
            max_line_bytes: config.max_line_bytes,
            request_timeout: config.request_timeout,
            notifications: Vec::new(),
        })
    }

    pub async fn initialize(&mut self) -> Result<Value, BridgeError> {
        let response = self
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": CLIENT_NAME,
                        "title": "Agent Workspace",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "experimentalApi": false
                    }
                }),
            )
            .await?;
        self.notify("initialized", Value::Null).await?;
        Ok(response)
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(&json!({"id": id, "method": method, "params": params}))
            .await?;

        timeout(self.request_timeout, self.read_until_response(id))
            .await
            .map_err(|_| BridgeError::Timeout)?
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), BridgeError> {
        self.write_json(&json!({"method": method, "params": params}))
            .await
    }

    pub async fn next_event(&mut self) -> Result<Value, BridgeError> {
        let message = self.next_raw_event().await?;
        if message.get("id").is_some() && message.get("method").is_some() {
            self.respond_to_server_request(&message).await?;
        }
        Ok(redact_fixture(message))
    }

    pub async fn next_raw_event(&mut self) -> Result<Value, BridgeError> {
        timeout(self.request_timeout, self.read_message())
            .await
            .map_err(|_| BridgeError::Timeout)?
    }

    pub async fn respond(&mut self, id: Value, result: Value) -> Result<(), BridgeError> {
        self.write_json(&json!({"id": id, "result": result})).await
    }

    pub fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }

    pub async fn stderr_snapshot(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().await).into_owned()
    }

    pub async fn shutdown(mut self) {
        let process_id = self.child.id();
        if let Some(process_id) = process_id {
            #[cfg(unix)]
            {
                if let Ok(raw_pid) = i32::try_from(process_id) {
                    let _ = killpg(Pid::from_raw(raw_pid), Signal::SIGTERM);
                }
            }
        }

        if timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
        self.stderr_task.abort();
    }

    async fn read_until_response(&mut self, id: u64) -> Result<Value, BridgeError> {
        loop {
            let message = self.read_message().await?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(BridgeError::Rpc(redact_value(error.clone())));
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or(BridgeError::MissingResult);
            }

            if message.get("id").is_some() && message.get("method").is_some() {
                self.respond_to_server_request(&message).await?;
            } else {
                self.notifications.push(redact_fixture(message));
            }
        }
    }

    async fn read_message(&mut self) -> Result<Value, BridgeError> {
        let mut bytes = Vec::with_capacity(self.max_line_bytes.min(8 * 1024));
        loop {
            let (consumed, complete) = {
                let available = self.stdout.fill_buf().await?;
                if available.is_empty() {
                    if bytes.is_empty() {
                        return Err(BridgeError::Closed);
                    }
                    break;
                }
                let newline = available.iter().position(|byte| *byte == b'\n');
                let consumed = newline.map_or(available.len(), |index| index + 1);
                if bytes.len().saturating_add(consumed) > self.max_line_bytes {
                    return Err(BridgeError::OversizedLine {
                        limit: self.max_line_bytes,
                    });
                }
                bytes.extend_from_slice(&available[..consumed]);
                (consumed, newline.is_some())
            };
            self.stdout.consume(consumed);
            if complete {
                break;
            }
        }
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        decode_line(&bytes, self.max_line_bytes)
    }

    async fn write_json(&mut self, value: &Value) -> Result<(), BridgeError> {
        let mut encoded = serde_json::to_vec(value)?;
        if encoded.len() > self.max_line_bytes {
            return Err(BridgeError::OversizedLine {
                limit: self.max_line_bytes,
            });
        }
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn respond_to_server_request(&mut self, request: &Value) -> Result<(), BridgeError> {
        let id = request
            .get("id")
            .cloned()
            .ok_or(BridgeError::MissingResult)?;
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                json!({"decision": "accept"})
            }
            "item/tool/requestUserInput" => {
                let mut answers = serde_json::Map::new();
                if let Some(questions) = request
                    .pointer("/params/questions")
                    .and_then(Value::as_array)
                {
                    for question in questions {
                        if let Some(question_id) = question.get("id").and_then(Value::as_str) {
                            answers
                                .insert(question_id.to_owned(), json!({"answers": ["approved"]}));
                        }
                    }
                }
                json!({"answers": answers})
            }
            _ => {
                self.write_json(&json!({
                    "id": id,
                    "error": {"code": -32601, "message": "unsupported server request"}
                }))
                .await?;
                return Ok(());
            }
        };
        self.write_json(&json!({"id": id, "result": result})).await
    }
}

fn spawn_bounded_stderr(
    stderr_pipe: tokio::process::ChildStderr,
    buffer: Arc<Mutex<Vec<u8>>>,
    max_bytes: usize,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr_pipe);
        let mut chunk = vec![0_u8; 4096];
        loop {
            let read = match tokio::io::AsyncReadExt::read(&mut reader, &mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let mut target = buffer.lock().await;
            let remaining = max_bytes.saturating_sub(target.len());
            target.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    })
}

pub fn safe_environment(path: &Path) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_owned(), path.to_string_lossy().into_owned());
    environment.insert("LANG".to_owned(), "C.UTF-8".to_owned());
    environment.insert("LC_ALL".to_owned(), "C.UTF-8".to_owned());
    environment
}

pub fn redact_fixture(value: Value) -> Value {
    let mut value = redact_value(value);
    for pointer in [
        "/params/msg/message",
        "/params/message",
        "/params/item/content",
        "/result/authUrl",
        "/result/userCode",
        "/result/verificationUrl",
    ] {
        if let Some(slot) = value.pointer_mut(pointer) {
            *slot = Value::String("[REDACTED]".to_owned());
        }
    }
    value
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| {
                    let lowered = key.to_ascii_lowercase();
                    if [
                        "token",
                        "apikey",
                        "api_key",
                        "authorization",
                        "cookie",
                        "secret",
                        "content",
                        "message",
                        "output",
                        "command",
                        "path",
                        "cwd",
                        "url",
                        "usercode",
                    ]
                    .iter()
                    .any(|needle| lowered.contains(needle))
                    {
                        (key, Value::String("[REDACTED]".to_owned()))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        other => other,
    }
}

fn decode_line(bytes: &[u8], max_line_bytes: usize) -> Result<Value, BridgeError> {
    if bytes.len() > max_line_bytes {
        return Err(BridgeError::OversizedLine {
            limit: max_line_bytes,
        });
    }
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf, time::Duration};

    use serde_json::json;

    use super::{BridgeConfig, BridgeError, decode_line, redact_fixture};

    #[test]
    fn rejects_relative_runtime_paths() {
        let config = BridgeConfig {
            codex_binary: PathBuf::from("codex"),
            codex_home: PathBuf::from("/tmp/profile"),
            cwd: PathBuf::from("/tmp/worktree"),
            safe_environment: BTreeMap::new(),
            max_line_bytes: 1024,
            max_stderr_bytes: 1024,
            request_timeout: Duration::from_secs(1),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn fixture_redaction_removes_secrets_and_content() {
        let value = json!({
            "method": "item/completed",
            "params": {
                "token": "secret-token",
                "message": "private reasoning",
                "nested": {"authorization": "Bearer secret"}
            }
        });
        let redacted = redact_fixture(value);
        let encoded = redacted.to_string();
        assert!(!encoded.contains("secret-token"));
        assert!(!encoded.contains("private reasoning"));
        assert!(!encoded.contains("Bearer secret"));
        assert!(encoded.contains("[REDACTED]"));
    }

    #[test]
    fn malformed_json_is_rejected() {
        assert!(matches!(
            decode_line(b"{not-json}", 1024),
            Err(BridgeError::Json(_))
        ));
    }

    #[test]
    fn oversized_jsonl_line_is_rejected_before_parsing() {
        assert!(matches!(
            decode_line(br#"{"method":"turn/completed"}"#, 8),
            Err(BridgeError::OversizedLine { limit: 8 })
        ));
    }
}
