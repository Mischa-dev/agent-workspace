//! Packages (M1 section 3.6): a setup in one file. M1.8 brings the format,
//! its digest, the no-secrets check, templates read as packages and
//! `package_export`; preview and install arrive with M1.9.

pub mod export;
pub mod format;
pub mod ops;
pub mod secrets_check;
