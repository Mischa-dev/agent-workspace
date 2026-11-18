//! `aw-agentos-connector`: runs Agent OS agents as a host on Agent Workspace.
//!
//! ```text
//! aw-agentos-connector --config connector.json enrol <one-time code>
//! aw-agentos-connector --config connector.json check
//! aw-agentos-connector --config connector.json run
//! ```

use agent_workspace_agent_os_connector::{
    Connector, client,
    config::Config,
    controller::{CommandController, source_revision},
    enrol, serve,
};

fn usage() -> ! {
    eprintln!("usage: aw-agentos-connector --config <file> (enrol <code> | check | run)");
    std::process::exit(2);
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("aw-agentos-connector: {message}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some("--config"), Some(path)) = (args.first().map(String::as_str), args.get(1)) else {
        usage()
    };
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| fail(format!("reading {path}: {error}")));
    let config = Config::parse(&text).unwrap_or_else(|error| fail(format!("{path}: {error}")));
    let scratch = config.state_dir.join("requests");
    if let Err(error) = agent_workspace_agent_os_connector::receipts::create_private_dir(&scratch) {
        fail(format!("creating {}: {error}", scratch.display()));
    }
    let controller = CommandController::new(config.agentos.clone(), scratch);
    match args.get(2).map(String::as_str) {
        Some("enrol") => {
            let Some(code) = args.get(3) else { usage() };
            enrol(&config, code)
                .await
                .unwrap_or_else(|error| fail(error));
            eprintln!("aw-agentos-connector: enrolled; start it with `run`");
        }
        Some("check") => {
            let revision = source_revision(&controller)
                .await
                .unwrap_or_else(|error| fail(error));
            let client = client(&config).unwrap_or_else(|error| fail(error));
            // Reads nothing new (no wait, past every command): only proves
            // the credential works.
            client
                .commands(i64::MAX, 0)
                .await
                .unwrap_or_else(|error| fail(format!("the workspace didn't answer: {error:?}")));
            println!("connected; Agent OS answers at revision {revision}");
        }
        Some("run") => {
            let client = client(&config).unwrap_or_else(|error| fail(error));
            let connector = Connector::new(config, controller).unwrap_or_else(|error| fail(error));
            let stop = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            if let Err(error) = serve(client, connector, stop).await {
                fail(format!("stopped: {error:?}"));
            }
        }
        _ => usage(),
    }
}
