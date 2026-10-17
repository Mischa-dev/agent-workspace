//! `preflight`: what the release script runs with a new build, against the live
//! service configuration and database, before it switches `current` to it.
//!
//! It only reads. It loads the configuration (which already refuses a keyring
//! inside a Git checkout), opens the secrets keyring, looks for the harness
//! CLIs, says whether email is set up and how the last nightly backup went,
//! checks free disk, and compares the build's migrations with the ones the
//! database has run. Settings > Workspace > Server shows the same report. It never prints a configuration value or secret.

use std::{path::Path, process::Stdio, time::Duration};

use sqlx::PgPool;
use tokio::process::Command;

use crate::{config::AppConfig, secrets, storage_admission};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<(Level, String)>,
}

impl Report {
    fn push(&mut self, level: Level, line: impl Into<String>) {
        self.lines.push((level, line.into()));
    }

    pub fn failed(&self) -> bool {
        self.lines.iter().any(|(level, _)| *level == Level::Fail)
    }

    pub fn print(&self) {
        for (level, line) in &self.lines {
            let tag = match level {
                Level::Ok => "ok  ",
                Level::Warn => "warn",
                Level::Fail => "FAIL",
            };
            println!("{tag} {line}");
        }
    }
}

pub async fn run(config: &AppConfig, pool: &PgPool) -> anyhow::Result<Report> {
    let mut report = Report::default();
    report.push(Level::Ok, "configuration loads");

    match &config.secrets_key_path {
        Some(path) => match secrets::read_keyring(path) {
            Ok(keyring) => report.push(
                Level::Ok,
                format!("secrets keyring opens ({} key(s))", keyring.key_ids().len()),
            ),
            Err(_) => report.push(
                Level::Fail,
                "secrets keyring is configured but can't be read by the service user",
            ),
        },
        None if config.environment.is_development() => report.push(
            Level::Ok,
            "development build: keyring is created on first use",
        ),
        None => report.push(
            Level::Warn,
            "no secrets keyring (AGENT_WORKSPACE_SECRETS_KEY_PATH): two-factor, AI provider keys, \
             push and webhook secrets won't work; see docs/operations/secret-rotation.md",
        ),
    }

    for (name, binary) in [
        ("codex", &config.codex_binary),
        ("claude", &config.claude_binary),
    ] {
        match cli_version(binary).await {
            Some(version) => report.push(Level::Ok, format!("{name} CLI: {version}")),
            None => report.push(
                Level::Warn,
                format!(
                    "{name} CLI not found or not runnable; {name} runs on this server won't start"
                ),
            ),
        }
    }

    report.push(
        Level::Ok,
        if config.github.is_some() {
            "GitHub App is configured"
        } else {
            "GitHub App isn't configured (projects can't connect repositories from GitHub)"
        },
    );
    report.push(
        Level::Ok,
        format!(
            "{} trusted webhook origin(s)",
            config.webhook_trusted_origins.len()
        ),
    );

    let mail_workspaces = if migrations_table_exists(pool).await?
        && sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('workspace_mail_settings') IS NOT NULL",
        )
        .fetch_one(pool)
        .await?
    {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspace_mail_settings WHERE enabled")
            .fetch_one(pool)
            .await?
    } else {
        0
    };
    report.push(
        Level::Ok,
        match mail_workspaces {
            0 => "email isn't set up (no password reset emails or email notifications); an owner \
                  sets it in Settings > Workspace > Email"
                .to_owned(),
            count => format!("{count} workspace(s) send email"),
        },
    );
    let (level, line) = crate::server_status::backup_line(&config.data_root);
    report.push(level, line);

    match storage_admission::inspect(
        &config.data_root,
        config.data_quota_bytes,
        config.minimum_free_bytes,
    ) {
        Ok(storage) if storage.admitted => report.push(
            Level::Ok,
            format!("{} GiB free for app data", storage.available_bytes >> 30),
        ),
        Ok(storage) => report.push(
            Level::Fail,
            format!(
                "app data is over its quota or the disk is nearly full ({} GiB free)",
                storage.available_bytes >> 30
            ),
        ),
        Err(_) => report.push(Level::Warn, "couldn't measure the app data folder"),
    }

    let known: Vec<(i64, Vec<u8>, String)> = sqlx::migrate!()
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| {
            (
                migration.version,
                migration.checksum.to_vec(),
                migration.description.to_string(),
            )
        })
        .collect();
    let applied: Vec<(i64, Vec<u8>)> = if migrations_table_exists(pool).await? {
        sqlx::query_as(
            "SELECT version, checksum FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(pool)
        .await?
    } else {
        Vec::new()
    };
    let plan = compare_migrations(&known, &applied);
    if !plan.unknown.is_empty() {
        report.push(
            Level::Fail,
            format!(
                "the database ran {} migration(s) this build doesn't have (first: {}); it is older \
                 than what's deployed, so don't switch to it",
                plan.unknown.len(),
                plan.unknown[0]
            ),
        );
    }
    if !plan.changed.is_empty() {
        report.push(
            Level::Fail,
            format!(
                "{} migration(s) changed after they ran (first: {}); the server would refuse to start",
                plan.changed.len(),
                plan.changed[0]
            ),
        );
    }
    match plan.pending.as_slice() {
        [] => report.push(Level::Ok, "no migrations to apply"),
        [only] => report.push(Level::Ok, format!("1 migration to apply: {only}")),
        [first, .., last] => report.push(
            Level::Ok,
            format!(
                "{} migrations to apply, {first} through {last}; back up the database first",
                plan.pending.len()
            ),
        ),
    }
    Ok(report)
}

async fn migrations_table_exists(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await
}

/// The first line of `<binary> --version`, or None when it can't run.
async fn cli_version(binary: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(120)
        .collect::<String>();
    Some(if line.is_empty() {
        "installed".into()
    } else {
        line
    })
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct MigrationPlan {
    /// In this build, not yet run: "<version> <description>".
    pub pending: Vec<String>,
    /// Run with a different checksum than this build's copy.
    pub changed: Vec<String>,
    /// Run, but missing from this build (the build is older than the database).
    pub unknown: Vec<String>,
}

pub fn compare_migrations(
    known: &[(i64, Vec<u8>, String)],
    applied: &[(i64, Vec<u8>)],
) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    for (version, checksum, description) in known {
        match applied.iter().find(|(applied, _)| applied == version) {
            None => plan.pending.push(format!("{version} {description}")),
            Some((_, ran)) if ran != checksum => {
                plan.changed.push(format!("{version} {description}"))
            }
            Some(_) => {}
        }
    }
    for (version, _) in applied {
        if !known.iter().any(|(known, _, _)| known == version) {
            plan.unknown.push(version.to_string());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(version: i64, checksum: u8) -> (i64, Vec<u8>, String) {
        (version, vec![checksum], format!("m{version}"))
    }

    #[test]
    fn sorts_migrations_into_pending_changed_and_unknown() {
        let plan = compare_migrations(
            &[known(1, 1), known(2, 2), known(3, 3)],
            &[(1, vec![1]), (2, vec![9]), (7, vec![7])],
        );
        assert_eq!(plan.pending, vec!["3 m3"]);
        assert_eq!(plan.changed, vec!["2 m2"]);
        assert_eq!(plan.unknown, vec!["7"]);
    }

    #[test]
    fn an_up_to_date_database_has_nothing_to_do() {
        let plan = compare_migrations(&[known(1, 1)], &[(1, vec![1])]);
        assert_eq!(plan, MigrationPlan::default());
    }
}
