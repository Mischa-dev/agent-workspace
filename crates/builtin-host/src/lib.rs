//! The built-in host (A1.16): what runs agents on this machine. The harness
//! adapters, the run worktrees and mirrors they work in, and the private
//! account homes they sign in with. It knows nothing of the server's
//! database; what it needs from the server (stored keys, a repository's
//! token) it asks through [`harness::host::HarnessHost`].

pub mod assistant;
pub mod config;
pub mod error;
pub mod git;
pub mod harness;
pub mod profile;
pub mod runtime;
pub mod steps;

pub use config::HarnessConfig;
pub use error::RuntimeError;
