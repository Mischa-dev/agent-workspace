//! The connector's configuration: everything about one installation is
//! named here, never in code (no context, machine, person or path).

use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// The workspace server, such as `https://workspace.example`.
    pub workspace_origin: String,
    /// Where the host credential and the receipts live (created 0700).
    pub state_dir: PathBuf,
    /// The host's name on the workspace's Hosts page.
    #[serde(default = "default_host_name")]
    pub host_name: String,
    pub agentos: AgentOs,
    /// The agents this host serves, by the reference each is bound with.
    pub agents: BTreeMap<String, AgentEntry>,
    /// Seconds between looks at running tasks.
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    /// A task still running after this long is stopped and its run fails.
    #[serde(default = "default_task_deadline_seconds")]
    pub task_deadline_seconds: u64,
    /// Longer answers are cut, with a note.
    #[serde(default = "default_max_reply_characters")]
    pub max_reply_characters: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOs {
    /// The Agent OS context the agents live in.
    pub context: String,
    /// The controller command, as an argument array; the request file's
    /// path is added last. Usually `["agentos", "controller", "call"]`.
    pub command: Vec<String>,
    #[serde(default = "default_controller_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentEntry {
    /// The agent's id in Agent OS.
    pub agentos_id: String,
    /// What it is called (shown on the Hosts page and in its prompt).
    pub name: String,
    /// Extra standing instructions put first in each task's prompt.
    #[serde(default)]
    pub instructions: Option<String>,
}

fn default_host_name() -> String {
    "Agent OS".to_owned()
}
fn default_poll_seconds() -> u64 {
    8
}
fn default_task_deadline_seconds() -> u64 {
    3600
}
fn default_max_reply_characters() -> usize {
    19_500
}
fn default_controller_timeout() -> u64 {
    45
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if config.agentos.command.is_empty() {
            return Err("agentos.command needs at least the program".to_owned());
        }
        if config.agents.is_empty() {
            return Err("agents lists no agent".to_owned());
        }
        if !(config.workspace_origin.starts_with("https://")
            || config.workspace_origin.starts_with("http://127.0.0.1")
            || config.workspace_origin.starts_with("http://localhost"))
        {
            return Err("workspaceOrigin must be https (or a local address)".to_owned());
        }
        Ok(config)
    }
}
