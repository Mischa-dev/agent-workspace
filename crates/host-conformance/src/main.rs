//! `host-conformance`: the host protocol's suite, both ways.
//!
//! ```text
//! host-conformance host -- <command that starts your host>
//! host-conformance server --server <url> --code <one-time code>
//! ```
//!
//! `host` serves a stand-in workspace on a local port and starts the host
//! with `AW_SERVER` and `AW_ENROL_CODE` set to it, then drives it through
//! the protocol. `server` acts as a host on a real server. Each prints what
//! passed and what is missing, and exits 1 when anything failed.

use std::process::ExitCode;

use agent_workspace_host_conformance::{FakeWorkspace, Report, check_host, check_server};

const USAGE: &str = "usage: host-conformance host -- <command> [args...]\n       host-conformance server --server <url> --code <code>";

fn value_of(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|at| arguments.get(at + 1))
        .cloned()
}

async fn against_host(command: &[String]) -> Result<Report, String> {
    let Some((program, arguments)) = command.split_first() else {
        return Err(USAGE.to_owned());
    };
    let fake = FakeWorkspace::start("127.0.0.1:0")
        .await
        .map_err(|error| format!("can't listen: {error}"))?;
    let mut child = tokio::process::Command::new(program)
        .args(arguments)
        .env("AW_SERVER", &fake.base)
        .env("AW_ENROL_CODE", &fake.code)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("can't start {program}: {error}"))?;
    let report = check_host(&fake).await;
    let _ = child.kill().await;
    Ok(report)
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let report = match arguments.first().map(String::as_str) {
        Some("host") => {
            let command = arguments
                .iter()
                .position(|argument| argument == "--")
                .map(|at| arguments[at + 1..].to_vec())
                .unwrap_or_default();
            against_host(&command).await
        }
        Some("server") => match (
            value_of(&arguments, "--server"),
            value_of(&arguments, "--code"),
        ) {
            (Some(server), Some(code)) => Ok(check_server(&server, &code).await),
            _ => Err(USAGE.to_owned()),
        },
        _ => Err(USAGE.to_owned()),
    };
    match report {
        Ok(report) => {
            print!("{report}");
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
