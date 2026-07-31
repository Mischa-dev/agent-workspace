use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand, ValueEnum};
use codex_bridge_spike::{BridgeConfig, CodexBridge, safe_environment};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(about = "Pinned Codex 0.146.0 App Server compatibility spike")]
struct Cli {
    #[arg(long, env = "AGENT_WORKSPACE_CODEX_BINARY")]
    codex_binary: PathBuf,
    #[arg(long, env = "CODEX_HOME")]
    codex_home: PathBuf,
    #[arg(long)]
    worktree: PathBuf,
    #[arg(long)]
    runtime_path: PathBuf,
    #[arg(long, default_value_t = 1_048_576)]
    max_line_bytes: usize,
    #[arg(long, default_value_t = 262_144)]
    max_stderr_bytes: usize,
    #[arg(long, default_value_t = 120)]
    timeout_seconds: u64,
    #[command(subcommand)]
    command: SpikeCommand,
}

#[derive(Debug, Subcommand)]
enum SpikeCommand {
    Probe,
    Login {
        kind: LoginKind,
    },
    Exercise {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        resume_thread: Option<String>,
        #[arg(long, default_value_t = false)]
        interrupt: bool,
    },
    Logout,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LoginKind {
    Browser,
    Device,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = BridgeConfig {
        codex_binary: cli.codex_binary.clone(),
        codex_home: cli.codex_home.clone(),
        cwd: cli.worktree.clone(),
        safe_environment: safe_environment(&cli.runtime_path),
        max_line_bytes: cli.max_line_bytes,
        max_stderr_bytes: cli.max_stderr_bytes,
        request_timeout: Duration::from_secs(cli.timeout_seconds),
    };
    let mut bridge = CodexBridge::spawn(config).await?;
    bridge.initialize().await?;

    let outcome = run(&mut bridge, &cli).await;
    if let Err(error) = &outcome {
        eprintln!("bridge failed: {error}");
        let stderr = bridge.stderr_snapshot().await;
        if !stderr.is_empty() {
            eprintln!("bounded Codex stderr:\n{stderr}");
        }
    }
    bridge.shutdown().await;
    outcome
}

async fn run(bridge: &mut CodexBridge, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    match &cli.command {
        SpikeCommand::Probe => {
            let account = bridge
                .request("account/read", json!({"refreshToken": false}))
                .await?;
            let limits = bridge
                .request("account/rateLimits/read", Value::Null)
                .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "account": summarize_account(&account),
                    "rateLimitsAvailable": !limits.is_null()
                }))?
            );
        }
        SpikeCommand::Login { kind } => {
            let params = match kind {
                LoginKind::Browser => json!({
                    "type": "chatgpt",
                    "appBrand": "codex",
                    "codexStreamlinedLogin": false,
                    "useHostedLoginSuccessPage": true
                }),
                LoginKind::Device => json!({"type": "chatgptDeviceCode"}),
            };
            let login = bridge.request("account/login/start", params).await?;
            println!("{}", serde_json::to_string_pretty(&login)?);
        }
        SpikeCommand::Exercise {
            prompt,
            resume_thread,
            interrupt,
        } => {
            let thread = if let Some(thread_id) = resume_thread {
                bridge
                    .request(
                        "thread/resume",
                        json!({"threadId": thread_id, "cwd": cli.worktree}),
                    )
                    .await?
            } else {
                bridge
                    .request(
                        "thread/start",
                        json!({
                            "cwd": cli.worktree,
                            "approvalPolicy": "on-request",
                            "sandbox": "workspace-write",
                            "ephemeral": false
                        }),
                    )
                    .await?
            };
            let thread_id = thread
                .pointer("/thread/id")
                .and_then(Value::as_str)
                .ok_or("thread response omitted thread.id")?;
            let turn = bridge
                .request(
                    "turn/start",
                    json!({
                        "threadId": thread_id,
                        "cwd": cli.worktree,
                        "input": [{"type": "text", "text": prompt}]
                    }),
                )
                .await?;
            let turn_id = turn
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .ok_or("turn response omitted turn.id")?;

            let deadline = Instant::now() + Duration::from_secs(cli.timeout_seconds);
            let mut counts = BTreeMap::<String, usize>::new();
            if *interrupt {
                loop {
                    if Instant::now() >= deadline {
                        return Err("turn start event deadline exceeded".into());
                    }
                    let event = bridge.next_event().await?;
                    let Some(method) = event.get("method").and_then(Value::as_str) else {
                        continue;
                    };
                    *counts.entry(method.to_owned()).or_default() += 1;
                    if method == "turn/completed" {
                        return Err("turn completed before interrupt could be sent".into());
                    }
                    if method == "turn/started" {
                        break;
                    }
                }
                bridge
                    .request(
                        "turn/interrupt",
                        json!({"threadId": thread_id, "turnId": turn_id}),
                    )
                    .await?;
            }

            loop {
                if Instant::now() >= deadline {
                    return Err("turn event deadline exceeded".into());
                }
                let event = bridge.next_event().await?;
                if let Some(method) = event.get("method").and_then(Value::as_str) {
                    *counts.entry(method.to_owned()).or_default() += 1;
                    if method == "turn/completed" {
                        break;
                    }
                }
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "notifications": counts
                }))?
            );
        }
        SpikeCommand::Logout => {
            bridge.request("account/logout", Value::Null).await?;
            println!("logged out");
        }
    }
    Ok(())
}

fn summarize_account(account: &Value) -> Value {
    json!({
        "authenticated": !account.pointer("/account").is_none_or(Value::is_null),
        "requiresOpenaiAuth": account
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}
