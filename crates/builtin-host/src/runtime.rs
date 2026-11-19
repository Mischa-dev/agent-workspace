//! The built-in host's runtime: how it talks to the server about the runs
//! it carries out. Everything here goes through the public host protocol.

pub mod commands;
pub mod reply;
pub mod report;
pub mod requests;
pub mod turn;
pub mod windows;
