//! Packages (M1 section 3.6): a setup in one file. M1.8 brings the format,
//! its digest, the no-secrets check, templates read as packages and
//! `package_export`; M1.9 preview, install, list and removal; M1.11 the
//! built-in packages, the zip form, the directory and templates used as
//! packages; M1.17 the shapes a workspace starts from.

pub mod apply;
pub mod builtin;
pub mod directory;
pub mod directory_ops;
pub mod export;
pub mod format;
pub mod install;
pub mod ops;
pub mod secrets_check;
pub mod source;
pub mod zipped;
