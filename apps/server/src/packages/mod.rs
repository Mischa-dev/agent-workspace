//! Packages (M1 section 3.6): a setup in one file. M1.8 brings the format,
//! its digest, the no-secrets check, templates read as packages and
//! `package_export`; M1.9 preview, install, list and removal.

pub mod export;
pub mod format;
pub mod install;
pub mod ops;
pub mod secrets_check;
