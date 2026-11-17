//! `minimal-host`: link it with a one-time code from the workspace's host
//! settings and leave it running.
//!
//! ```text
//! AW_SERVER=https://work.example AW_ENROL_CODE=<code> minimal-host
//! ```
//!
//! It keeps nothing on disk: started again, it needs a new code.

use agent_workspace_minimal_host::{MinimalHost, enrol, serve};

#[tokio::main]
async fn main() {
    let server = std::env::var("AW_SERVER").unwrap_or_else(|_| "http://127.0.0.1:3100".to_owned());
    let Ok(code) = std::env::var("AW_ENROL_CODE") else {
        eprintln!("minimal-host: set AW_ENROL_CODE to a one-time code from the workspace");
        std::process::exit(2);
    };
    let name = std::env::var("AW_HOST_NAME").unwrap_or_else(|_| "Minimal host".to_owned());
    let client = match enrol(&server, &code, &name).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("minimal-host: enrolling failed: {error:?}");
            std::process::exit(1);
        }
    };
    eprintln!("minimal-host: enrolled with {server}");
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(error) = serve(MinimalHost::new(client), stop).await {
        eprintln!("minimal-host: stopped: {error:?}");
        std::process::exit(1);
    }
}
