//! A harness child process that speaks JSON lines (Claude Code's
//! stream-json, the command-line harness protocol): one process per turn,
//! its stdout read into a channel so waiting for the next line can be
//! cancelled, its stderr drained and dropped (it may echo what the agent
//! ran), and the whole process group killed when the turn is dropped.

use std::process::Stdio;

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    sync::mpsc,
};

use super::adapter::HarnessError;

/// The longest stdout line read; a longer one ends the turn.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

pub enum Line {
    Json(Value),
    /// Not JSON; ignored by the protocols but counted.
    Text,
    /// The process closed stdout (or wrote a line that was too long).
    Closed,
}

pub struct LineProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<Line>,
    closed: bool,
}

impl LineProcess {
    /// Spawns `command` with piped stdio in its own process group.
    pub fn spawn(mut command: tokio::process::Command) -> Result<Self, HarnessError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| HarnessError::Transport(Box::new(error)))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or(HarnessError::InvalidResponse)?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = [0_u8; 8192];
                while matches!(stderr.read(&mut sink).await, Ok(read) if read > 0) {}
            });
        }
        let (sender, lines) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                let read = (&mut reader)
                    .take(MAX_LINE_BYTES as u64 + 1)
                    .read_until(b'\n', &mut buffer)
                    .await;
                let line = match read {
                    Ok(0) | Err(_) => Line::Closed,
                    Ok(_) if buffer.len() > MAX_LINE_BYTES => Line::Closed,
                    Ok(_) => match serde_json::from_slice::<Value>(trim(&buffer)) {
                        Ok(value) => Line::Json(value),
                        Err(_) => Line::Text,
                    },
                };
                let closed = matches!(line, Line::Closed);
                if sender.send(line).await.is_err() || closed {
                    return;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
            closed: false,
        })
    }

    /// Writes one JSON line to the process.
    pub async fn send(&mut self, message: &Value) -> Result<(), HarnessError> {
        let stdin = self.stdin.as_mut().ok_or(HarnessError::InvalidResponse)?;
        let mut line = serde_json::to_vec(message).map_err(|_| HarnessError::InvalidResponse)?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|error| HarnessError::Transport(Box::new(error)))?;
        stdin
            .flush()
            .await
            .map_err(|error| HarnessError::Transport(Box::new(error)))
    }

    /// Closes the process's stdin, its cue that no more input comes.
    pub fn close_input(&mut self) {
        self.stdin = None;
    }

    /// The next line. Cancel-safe: a dropped call loses nothing.
    pub async fn next(&mut self) -> Line {
        if self.closed {
            return Line::Closed;
        }
        let line = self.lines.recv().await.unwrap_or(Line::Closed);
        if matches!(line, Line::Closed) {
            self.closed = true;
        }
        line
    }

    /// Kills the process group now.
    pub async fn kill(&mut self) {
        self.stdin = None;
        if let Some(pid) = self.child.id().and_then(|pid| i32::try_from(pid).ok())
            && let Some(group) = rustix::process::Pid::from_raw(pid)
        {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = self.child.kill().await;
    }
}

fn trim(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |index| index + 1);
    &bytes[..end]
}
