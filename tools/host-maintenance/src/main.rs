#![forbid(unsafe_code)]

use std::{collections::BTreeMap, env, path::PathBuf};

use agent_workspace_host_maintenance::{
    BackupOptions, RestoreOptions, RetentionOptions, StorageReportOptions, create_backup,
    restore_backup, run_retention, storage_report, validate_backup,
};
use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut arguments = env::args().skip(1);
    let command = arguments
        .next()
        .context("maintenance command is required")?;
    let options = parse_options(arguments)?;
    match command.as_str() {
        "backup" => {
            reject_unknown(
                &options,
                &[
                    "destination",
                    "data-root",
                    "release-root",
                    "release-commit",
                    "database-name-file",
                    "pg-dump",
                    "psql",
                    "tar",
                ],
            )?;
            let result = create_backup(&BackupOptions {
                destination: path(&options, "destination")?,
                data_root: path(&options, "data-root")?,
                release_root: path(&options, "release-root")?,
                release_commit: value(&options, "release-commit")?.to_owned(),
                database_name_file: path(&options, "database-name-file")?,
                pg_dump: path(&options, "pg-dump")?,
                psql: path(&options, "psql")?,
                tar: path(&options, "tar")?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        "validate" => {
            reject_unknown(&options, &["source"])?;
            let manifest = validate_backup(&path(&options, "source")?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "valid": true,
                    "formatVersion": manifest.format_version,
                    "releaseCommit": manifest.release_commit,
                    "postgresMajor": manifest.postgres_major,
                    "migrationCount": manifest.migrations.len(),
                    "artifacts": manifest.artifacts,
                }))?
            );
        }
        "restore" => {
            reject_unknown(
                &options,
                &[
                    "source",
                    "target-data-root",
                    "target-release-root",
                    "target-database-name-file",
                    "active-database-name-file",
                    "pg-restore",
                    "psql",
                    "tar",
                    "chown",
                ],
            )?;
            let result = restore_backup(&RestoreOptions {
                source: path(&options, "source")?,
                target_data_root: path(&options, "target-data-root")?,
                target_release_root: path(&options, "target-release-root")?,
                target_database_name_file: path(&options, "target-database-name-file")?,
                active_database_name_file: path(&options, "active-database-name-file")?,
                pg_restore: path(&options, "pg-restore")?,
                psql: path(&options, "psql")?,
                tar: path(&options, "tar")?,
                chown: path(&options, "chown")?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        "storage-report" => {
            reject_unknown(
                &options,
                &[
                    "data-root",
                    "releases-root",
                    "backups-root",
                    "database-name-file",
                    "psql",
                    "data-quota-bytes",
                    "minimum-free-bytes",
                ],
            )?;
            let report = storage_report(&StorageReportOptions {
                data_root: path(&options, "data-root")?,
                releases_root: path(&options, "releases-root")?,
                backups_root: path(&options, "backups-root")?,
                database_name_file: path(&options, "database-name-file")?,
                psql: path(&options, "psql")?,
                data_quota_bytes: unsigned(&options, "data-quota-bytes")?,
                minimum_free_bytes: unsigned(&options, "minimum-free-bytes")?,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        "retention-cleanup" => {
            reject_unknown(&options, &["database-name-file", "psql", "apply"])?;
            let result = run_retention(&RetentionOptions {
                database_name_file: path(&options, "database-name-file")?,
                psql: path(&options, "psql")?,
                apply: optional_boolean(&options, "apply")?,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => bail!("unknown maintenance command"),
    }
    Ok(())
}

fn parse_options(arguments: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>> {
    let mut parsed = BTreeMap::new();
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        let name = argument
            .strip_prefix("--")
            .context("maintenance options must begin with --")?;
        if name.is_empty() || name.contains('=') {
            bail!("maintenance option names must be non-empty and separate from values");
        }
        let option_value = arguments
            .next()
            .with_context(|| format!("missing value for --{name}"))?;
        if parsed.insert(name.to_owned(), option_value).is_some() {
            bail!("duplicate maintenance option --{name}");
        }
    }
    Ok(parsed)
}

fn reject_unknown(options: &BTreeMap<String, String>, allowed: &[&str]) -> Result<()> {
    for name in options.keys() {
        if !allowed.contains(&name.as_str()) {
            bail!("unknown maintenance option --{name}");
        }
    }
    Ok(())
}

fn value<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("missing required maintenance option --{name}"))
}

fn optional_boolean(options: &BTreeMap<String, String>, name: &str) -> Result<bool> {
    match options.get(name).map(String::as_str) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => bail!("maintenance option --{name} must be true or false"),
    }
}

fn path(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(value(options, name)?))
}

fn unsigned(options: &BTreeMap<String, String>, name: &str) -> Result<u64> {
    value(options, name)?
        .parse()
        .with_context(|| format!("maintenance option --{name} must be an unsigned integer"))
}
