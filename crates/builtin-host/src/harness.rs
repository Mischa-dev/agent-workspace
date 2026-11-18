//! The harness adapters: each speaks one harness's own protocol (a local
//! program, a cloud API) and turns it into the same events and answers.

pub mod adapter;
pub mod claude_cloud;
pub mod claude_code;
pub mod codex_app_server;
pub mod codex_cloud;
pub mod command_line;
#[cfg(test)]
pub(crate) mod contract_tests;
pub mod demo;
pub mod host;
pub mod line_process;
pub mod manifest;
pub mod run_config;
pub mod secret_fd;
pub mod shutdown;
pub mod slots;
