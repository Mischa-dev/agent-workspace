//! `delivery-contract`: the delivery contract's suite against a server.
//!
//! ```text
//! AW_TOKEN=<agent token> delivery-contract server --origin <url> [--workspace <id or slug>]
//! delivery-contract fixtures
//! ```
//!
//! `server` reads the server as the account the token signs in as: the
//! catalogue, a poll, reading to the end, a refused cursor and a held
//! long-poll, checking every event against the fixtures. With
//! `--workspace` it also sets a one-time follow-up a minute ahead
//! (`schedule_create`) and checks that its `schedule.due` wakes a waiting
//! long-poll, streams with its cursor as the id, and doesn't repeat on
//! resume. `fixtures` checks the bundled webhook fixtures verify. Each
//! prints what passed and what didn't, and exits 1 when anything failed.

use std::process::ExitCode;

use agent_workspace_delivery_contract::{
    Report, check_server,
    webhook::{check_webhook, fixture_push},
};

const USAGE: &str = "usage: AW_TOKEN=<token> delivery-contract server --origin <url> [--workspace <id or slug>]\n       delivery-contract fixtures";

fn value_of(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|at| arguments.get(at + 1))
        .cloned()
}

/// Sets a one-time follow-up a minute ahead, which comes due as an event
/// addressed to the token's account.
async fn follow_up(origin: &str, token: &str, workspace: &str) -> Result<(), String> {
    let key = format!(
        "delivery-contract-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    );
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v2/w/{workspace}/ops/schedule_create",
            origin.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "inMinutes": 1 }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        Err(format!(
            "{status} {}",
            response.text().await.unwrap_or_default()
        ))
    }
}

fn fixtures_report() -> Report {
    let mut report = Report::default();
    let (secret, push) = fixture_push();
    let stamped = push
        .header("webhook-timestamp")
        .and_then(|at| at.parse().ok());
    report
        .checks
        .extend(check_webhook(&secret, &push, stamped).checks);
    report
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let report = match arguments.first().map(String::as_str) {
        Some("server") => {
            let origin = value_of(&arguments, "--origin");
            let token = std::env::var("AW_TOKEN")
                .ok()
                .filter(|token| !token.is_empty());
            match (origin, token) {
                (Some(origin), Some(token)) => {
                    let workspace = value_of(&arguments, "--workspace");
                    let stimulus = workspace.map(|workspace| {
                        let origin = origin.clone();
                        let token = token.clone();
                        async move || {
                            println!("Set a follow-up a minute ahead; waiting for it.");
                            follow_up(&origin, &token, &workspace).await
                        }
                    });
                    Ok(check_server(&origin, &token, stimulus).await)
                }
                _ => Err(USAGE.to_owned()),
            }
        }
        Some("fixtures") => Ok(fixtures_report()),
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
