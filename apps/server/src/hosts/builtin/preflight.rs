//! What the operator's preflight learns from the built-in host: whether
//! the harness programs it runs are there, by asking each for its version.

use std::{path::Path, process::Stdio, time::Duration};

use tokio::process::Command;

use crate::config::AppConfig;

/// Each harness program the built-in host runs, with the first line of its
/// `--version`, or `None` when it can't run.
pub async fn harness_programs(config: &AppConfig) -> Vec<(&'static str, Option<String>)> {
    let mut programs = Vec::new();
    for (name, binary) in [
        ("codex", &config.codex_binary),
        ("claude", &config.claude_binary),
    ] {
        programs.push((name, cli_version(binary).await));
    }
    programs
}

/// The first line of `<binary> --version`, or None when it can't run.
async fn cli_version(binary: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(120)
        .collect::<String>();
    Some(if line.is_empty() {
        "installed".into()
    } else {
        line
    })
}
