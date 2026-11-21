//! How a host describes a harness it runs (its manifest: models, options,
//! features) and the run configuration a launch resolves against it. Both
//! sides read these: the host to run, the server to offer and check.
pub mod manifest;
pub mod registered;
pub mod report;
pub mod run_config;
