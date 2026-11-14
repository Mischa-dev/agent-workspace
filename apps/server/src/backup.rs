//! Portable backup and restore (H1.16): the database and the files that
//! can't be rebuilt, in the set format the Red Hat scripts write
//! (`deploy/redhat/agent-workspace-backup`), from the server's own command
//! line, so it works in a container or on any host.
//!
//! A set is a directory named by its UTC start time (`20261114T033000Z`):
//! - `database.dump`: `pg_dump --format=custom`;
//! - `data.tar.gz`: the unrebuildable folders under the data root (uploaded
//!   files, artifacts, harness account homes);
//! - `backup-info`: format, time, migration version, server version, folders;
//! - `SHA256SUMS` over the other three.
//!
//! With a recipients file the dump and the archive are encrypted with `age`
//! as they are written (`.age`), so no plaintext copy touches the disk. The
//! keyring is backed up apart (`backup key`), encrypted to a different
//! recipient, to a different place: one identity able to open both a dump
//! and its key would defeat keeping them apart.
//!
//! Files in an object store (once K5 adds one) are not copied: the set
//! records `storage=disk` and the docs say to switch on the bucket's own
//! versioning.
//!
//! Commands (see `docs/self-host/backups.md`):
//! `backup create`, `backup schedule`, `backup key`, `restore`,
//! `restore drill`. External programs (`pg_dump`, `pg_restore`, `tar`,
//! `age`) are run with argument arrays; the database password reaches them
//! in their environment, never on a command line.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail};
use chrono::{DateTime, NaiveTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    config::AppConfig,
    server_status::{BackupRecord, backup_status_path},
};

/// The folders under the data root a backup keeps by default: uploaded
/// files and the harness account homes. Repository mirrors and worktrees are
/// left out: they are cloned again.
pub const DEFAULT_DATA_PATHS: &[&str] = &["attachments", "artifacts", "files", "codex/profiles"];
/// Sets kept by `backup schedule` whatever their age.
const KEEP_AT_LEAST: usize = 3;
/// The advisory lock a key rotation holds (`secrets.rs`): a dump taken
/// while one runs could hold secrets under two keys.
const ROTATION_LOCK: (i32, i32) = (470_127_786, 3);

/// Where `pg_dump` and its kin find the database: the URL's parts as libpq
/// environment variables, so the password is never an argument.
fn libpq_environment(database_url: &str) -> anyhow::Result<Vec<(&'static str, String)>> {
    let url = url::Url::parse(database_url).context("the database URL isn't a URL")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        bail!("the database URL must start with postgres://");
    }
    let decode =
        |value: &str| percent_decode(value).context("the database URL has a malformed escape");
    let mut environment = Vec::new();
    if let Some(host) = url.host_str().filter(|host| !host.is_empty()) {
        environment.push(("PGHOST", host.trim_matches(['[', ']']).to_owned()));
    }
    if let Some(port) = url.port() {
        environment.push(("PGPORT", port.to_string()));
    }
    if !url.username().is_empty() {
        environment.push(("PGUSER", decode(url.username())?));
    }
    if let Some(password) = url.password() {
        environment.push(("PGPASSWORD", decode(password)?));
    }
    let database = url.path().trim_start_matches('/');
    if !database.is_empty() {
        environment.push(("PGDATABASE", decode(database)?));
    }
    for (key, value) in url.query_pairs() {
        let name = match key.as_ref() {
            "sslmode" => "PGSSLMODE",
            "host" => "PGHOST",
            "port" => "PGPORT",
            "user" => "PGUSER",
            "dbname" => "PGDATABASE",
            "sslrootcert" => "PGSSLROOTCERT",
            _ => continue,
        };
        environment.push((name, value.into_owned()));
    }
    Ok(environment)
}

fn percent_decode(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value
                .get(index + 1..index + 3)
                .context("a % without two hex digits")?;
            decoded.push(u8::from_str_radix(hex, 16).context("a % without two hex digits")?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("not UTF-8")
}

/// A command for a PostgreSQL client program, aimed at `database_url`.
fn pg_command(program: &str, database_url: &str) -> anyhow::Result<Command> {
    let mut command = Command::new(program);
    for name in [
        "PGHOST",
        "PGHOSTADDR",
        "PGPORT",
        "PGUSER",
        "PGPASSWORD",
        "PGPASSFILE",
        "PGDATABASE",
        "PGSERVICE",
        "PGSSLMODE",
        "PGOPTIONS",
    ] {
        command.env_remove(name);
    }
    for (name, value) in libpq_environment(database_url)? {
        command.env(name, value);
    }
    Ok(command)
}

/// Runs `producer`, writing its output to `path`, encrypted to the
/// recipients in `recipients` when given.
fn write_stream(
    mut producer: Command,
    path: &Path,
    recipients: Option<&Path>,
) -> anyhow::Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    match recipients {
        None => {
            let status = producer
                .stdout(Stdio::from(file))
                .status()
                .with_context(|| format!("run {:?}", producer.get_program()))?;
            check_status(&producer, status)
        }
        Some(recipients) => {
            let mut child = producer
                .stdout(Stdio::piped())
                .spawn()
                .with_context(|| format!("run {:?}", producer.get_program()))?;
            let stdout = child.stdout.take().context("the producer's output")?;
            let mut age = Command::new("age");
            age.arg("--encrypt")
                .arg("--recipients-file")
                .arg(recipients)
                .stdin(Stdio::from(stdout))
                .stdout(Stdio::from(file));
            let age_status = age.status().context("run age (is it installed?)")?;
            let status = child.wait()?;
            check_status(&producer, status)?;
            check_status(&age, age_status)
        }
    }
}

fn check_status(command: &Command, status: std::process::ExitStatus) -> anyhow::Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("{:?} failed ({status})", command.get_program())
    }
}

/// `tar` over the folders of `paths` present under `data_root` (an empty
/// archive when none is), gzip-compressed, to standard output.
fn tar_command(data_root: &Path, paths: &[String]) -> Command {
    let mut tar = Command::new("tar");
    tar.arg("--create")
        .arg("--gzip")
        .arg("--file=-")
        .arg("--numeric-owner")
        .arg(format!("--directory={}", data_root.display()));
    let present = paths
        .iter()
        .filter(|path| data_root.join(path).exists())
        .collect::<Vec<_>>();
    if present.is_empty() {
        tar.arg("--files-from=/dev/null");
    } else {
        tar.arg("--");
        tar.args(present);
    }
    tar
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// What `backup create` wrote.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSet {
    pub path: PathBuf,
    pub encrypted: bool,
    pub migration_version: i64,
    pub data_paths: Vec<String>,
}

/// Options for one backup set.
#[derive(Clone, Debug)]
pub struct CreateOptions {
    /// The folder sets go in; the set is a new folder inside it.
    pub to: PathBuf,
    /// An `age` recipients file; unset, the set is not encrypted.
    pub recipients: Option<PathBuf>,
    /// Folders under the data root to keep.
    pub data_paths: Vec<String>,
}

fn set_name(now: DateTime<Utc>) -> String {
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

fn valid_set_name(name: &str) -> bool {
    name.len() == 16
        && name.char_indices().all(|(index, c)| match index {
            8 => c == 'T',
            15 => c == 'Z',
            _ => c.is_ascii_digit(),
        })
}

async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .context("connect to PostgreSQL")
}

/// Writes one backup set of the server's database and data root, and the
/// status file the Server page and `preflight` read.
pub async fn create(config: &AppConfig, options: &CreateOptions) -> anyhow::Result<CreatedSet> {
    let started_at = Utc::now();
    let result = create_set(config, options, started_at).await;
    let mut steps = BTreeMap::new();
    steps.insert(
        "database".to_owned(),
        if result.is_ok() { "ok" } else { "failed" }.to_owned(),
    );
    record_status(
        &config.data_root,
        &BackupRecord {
            started_at,
            finished_at: Utc::now(),
            ok: result.is_ok(),
            steps,
        },
    );
    result
}

async fn create_set(
    config: &AppConfig,
    options: &CreateOptions,
    started_at: DateTime<Utc>,
) -> anyhow::Result<CreatedSet> {
    for path in &options.data_paths {
        let plain = Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
        if !plain || path.is_empty() {
            bail!("data path {path} must be a plain relative folder");
        }
    }
    std::fs::create_dir_all(&options.to)
        .with_context(|| format!("create {}", options.to.display()))?;
    let name = set_name(started_at);
    let finished = options.to.join(&name);
    let work = options.to.join(format!("{name}.partial"));
    if finished.exists() || work.exists() {
        bail!("set {name} already exists");
    }
    std::fs::create_dir(&work).with_context(|| format!("create {}", work.display()))?;
    set_owner_only(&work);
    let outcome = async {
        let database_url = config.database_url.expose();
        let pool = connect(database_url).await?;
        // Recorded before the dump; migrations only move forward, so the
        // restored version is at least this.
        let migration_version = sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(max(version), 0) FROM _sqlx_migrations",
        )
        .fetch_one(&pool)
        .await
        .context("read the migration version")?;
        // Hold the key rotation lock for the length of the dump, so it sees
        // every secret before or after a rotation, never half of each.
        let mut lock = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1, $2)")
            .bind(ROTATION_LOCK.0)
            .bind(ROTATION_LOCK.1)
            .execute(&mut *lock)
            .await
            .context("take the key rotation lock")?;
        let suffix = if options.recipients.is_some() { ".age" } else { "" };
        let dump = work.join(format!("database.dump{suffix}"));
        let mut pg_dump = pg_command("pg_dump", database_url)?;
        pg_dump.arg("--format=custom").arg("--no-password");
        let recipients = options.recipients.clone();
        let dumped = {
            let dump = dump.clone();
            tokio::task::spawn_blocking(move || write_stream(pg_dump, &dump, recipients.as_deref()))
                .await
                .context("the dump stopped")?
        };
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
            .bind(ROTATION_LOCK.0)
            .bind(ROTATION_LOCK.1)
            .execute(&mut *lock)
            .await;
        drop(lock);
        pool.close().await;
        dumped.context("dump the database")?;
        if std::fs::metadata(&dump)?.len() == 0 {
            bail!("the database dump is empty");
        }

        let data_paths = options
            .data_paths
            .iter()
            .filter(|path| config.data_root.join(path).exists())
            .cloned()
            .collect::<Vec<_>>();
        let archive = work.join(format!("data.tar.gz{suffix}"));
        let tar = tar_command(&config.data_root, &options.data_paths);
        let recipients = options.recipients.clone();
        {
            let archive = archive.clone();
            tokio::task::spawn_blocking(move || write_stream(tar, &archive, recipients.as_deref()))
                .await
                .context("the archive stopped")?
                .context("archive the data folders")?;
        }

        let info = format!(
            "format=1\ncreated_utc={name}\nmigration_version={migration_version}\nrelease={}\nstorage=disk\ndata_paths={}\n",
            env!("CARGO_PKG_VERSION"),
            data_paths.join(" "),
        );
        std::fs::write(work.join("backup-info"), info)?;
        let mut sums = String::new();
        for file in [
            format!("database.dump{suffix}"),
            format!("data.tar.gz{suffix}"),
            "backup-info".to_owned(),
        ] {
            sums.push_str(&format!("{}  {file}\n", sha256_file(&work.join(&file))?));
        }
        std::fs::write(work.join("SHA256SUMS"), sums)?;
        std::fs::rename(&work, &finished)
            .with_context(|| format!("move the set into {}", finished.display()))?;
        Ok(CreatedSet {
            path: finished.clone(),
            encrypted: options.recipients.is_some(),
            migration_version,
            data_paths,
        })
    }
    .await;
    if outcome.is_err() {
        let _ = std::fs::remove_dir_all(&work);
    }
    outcome
}

fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

/// Writes the status file atomically: beside it, then renamed into place,
/// so a link planted at the status path is replaced, never followed.
fn record_status(data_root: &Path, record: &BackupRecord) {
    let path = backup_status_path(data_root);
    let temporary = data_root.join(format!(".backup-status.{}", uuid::Uuid::now_v7().simple()));
    let written = serde_json::to_vec(record)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            std::fs::create_dir_all(data_root)?;
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(&temporary, &path)
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        tracing::warn!("couldn't record the backup status");
    }
}

/// The sets in `to`, oldest first.
fn sets(to: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(to)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| valid_set_name(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Removes sets older than `keep_days`, always keeping the newest
/// [`KEEP_AT_LEAST`].
fn prune(to: &Path, keep_days: u32, now: DateTime<Utc>) -> Vec<String> {
    let names = sets(to);
    let cutoff = set_name(now - chrono::Duration::days(i64::from(keep_days)));
    let removable = names.len().saturating_sub(KEEP_AT_LEAST);
    let mut removed = Vec::new();
    for name in names.into_iter().take(removable) {
        if name < cutoff && std::fs::remove_dir_all(to.join(&name)).is_ok() {
            removed.push(name);
        }
    }
    removed
}

/// `backup schedule`: a set every day at `at` (UTC), keeping `keep_days`
/// of them, until the process is asked to stop.
pub async fn schedule(
    config: &AppConfig,
    options: &CreateOptions,
    at: NaiveTime,
    keep_days: u32,
) -> anyhow::Result<()> {
    loop {
        let now = Utc::now();
        let mut next = now.date_naive().and_time(at).and_utc();
        if next <= now {
            next += chrono::Duration::days(1);
        }
        let wait = (next - now).to_std().unwrap_or_default();
        tracing::info!(next = %next, "next backup scheduled");
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = crate::shutdown_signal() => return Ok(()),
        }
        match create(config, options).await {
            Ok(set) => tracing::info!(path = %set.path.display(), "backup written"),
            Err(error) => tracing::error!(error = %error, "backup failed"),
        }
        for removed in prune(&options.to, keep_days, Utc::now()) {
            tracing::info!(set = removed, "pruned an old backup set");
        }
    }
}

/// `backup key`: the keyring, encrypted to `recipients`, into a new set
/// folder in `to`. Refuses recipients that also open the database sets.
pub fn key(
    config: &AppConfig,
    to: &Path,
    recipients: &Path,
    database_recipients: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let keyring = config
        .secrets_key_path
        .as_ref()
        .context("no keyring is configured (AGENT_WORKSPACE_SECRETS_KEY_PATH)")?;
    if let Some(database_recipients) = database_recipients {
        let lines = |path: &Path| -> anyhow::Result<Vec<String>> {
            Ok(std::fs::read_to_string(path)?
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned)
                .collect())
        };
        let shared = lines(recipients)?
            .into_iter()
            .any(|line| lines(database_recipients).is_ok_and(|other| other.contains(&line)));
        if shared {
            bail!("the key's recipients must not also open the database backups");
        }
    }
    std::fs::create_dir_all(to)?;
    let name = set_name(Utc::now());
    let work = to.join(format!("{name}.partial"));
    let finished = to.join(&name);
    if finished.exists() || work.exists() {
        bail!("key set {name} already exists");
    }
    std::fs::create_dir(&work)?;
    set_owner_only(&work);
    let outcome = (|| {
        let mut cat = Command::new("cat");
        cat.arg("--").arg(keyring);
        write_stream(cat, &work.join("secrets-key.age"), Some(recipients))?;
        let sums = format!(
            "{}  secrets-key.age\n",
            sha256_file(&work.join("secrets-key.age"))?
        );
        std::fs::write(work.join("SHA256SUMS"), sums)?;
        std::fs::rename(&work, &finished)?;
        Ok(finished.clone())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_dir_all(&work);
    }
    outcome
}

/// A checked set: its files are the ones expected and match their sums.
struct CheckedSet {
    path: PathBuf,
    encrypted: bool,
    migration_version: i64,
}

fn check_set(path: &Path) -> anyhow::Result<CheckedSet> {
    let sums =
        std::fs::read_to_string(path.join("SHA256SUMS")).context("the set has no SHA256SUMS")?;
    let mut listed = Vec::new();
    for line in sums.lines().filter(|line| !line.trim().is_empty()) {
        let (sum, file) = line
            .split_once("  ")
            .context("a malformed SHA256SUMS line")?;
        if ![
            "database.dump",
            "database.dump.age",
            "data.tar.gz",
            "data.tar.gz.age",
            "backup-info",
        ]
        .contains(&file)
        {
            bail!("SHA256SUMS lists an unexpected file: {file}");
        }
        if sha256_file(&path.join(file))? != sum {
            bail!("{file} doesn't match its checksum");
        }
        listed.push(file.to_owned());
    }
    listed.sort();
    let encrypted = match listed
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["backup-info", "data.tar.gz", "database.dump"] => false,
        ["backup-info", "data.tar.gz.age", "database.dump.age"] => true,
        _ => bail!("the set's files are incomplete: {listed:?}"),
    };
    let info = std::fs::read_to_string(path.join("backup-info"))?;
    let migration_version = info
        .lines()
        .find_map(|line| line.strip_prefix("migration_version="))
        .and_then(|value| value.parse::<i64>().ok())
        .context("backup-info has no migration version")?;
    Ok(CheckedSet {
        path: path.to_owned(),
        encrypted,
        migration_version,
    })
}

/// Feeds a set's file (decrypted with `identity` when it is encrypted) into
/// `consumer`'s standard input.
fn read_stream(
    set: &CheckedSet,
    file: &str,
    identity: Option<&Path>,
    mut consumer: Command,
) -> anyhow::Result<()> {
    let path = set.path.join(if set.encrypted {
        format!("{file}.age")
    } else {
        file.to_owned()
    });
    if !set.encrypted {
        let input = std::fs::File::open(&path)?;
        let status = consumer
            .stdin(Stdio::from(input))
            .status()
            .with_context(|| format!("run {:?}", consumer.get_program()))?;
        return check_status(&consumer, status);
    }
    let identity = identity.context("the set is encrypted: pass --identity-file")?;
    let mut age = Command::new("age");
    age.arg("--decrypt")
        .arg("--identity")
        .arg(identity)
        .arg(&path)
        .stdout(Stdio::piped());
    let mut child = age.spawn().context("run age (is it installed?)")?;
    let stdout = child.stdout.take().context("age's output")?;
    let status = consumer
        .stdin(Stdio::from(stdout))
        .status()
        .with_context(|| format!("run {:?}", consumer.get_program()))?;
    let age_status = child.wait()?;
    check_status(&age, age_status)?;
    check_status(&consumer, status)
}

/// Whether a database holds no tables of its own.
async fn database_is_empty(pool: &PgPool) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema')",
    )
    .fetch_one(pool)
    .await?
        == 0)
}

/// What a restore or a drill found in the restored database.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub set: PathBuf,
    pub organizations: i64,
    pub projects: i64,
    pub messages: i64,
    pub migration_version: i64,
    pub recorded_migration_version: i64,
    pub failed_migrations: i64,
    pub data_entries: usize,
    pub secrets: Option<crate::secrets::ExportedRowsReport>,
    /// People's two-factor seeds, sealed with the same keyring.
    pub two_factor: Option<crate::secrets::ExportedRowsReport>,
}

/// `restore`: the set's database into an empty database and its folders
/// into an empty data root. Refuses a target that holds anything.
pub async fn restore(
    from: &Path,
    database_url: &str,
    data_root: &Path,
    identity: Option<&Path>,
) -> anyhow::Result<RestoreReport> {
    let set = check_set(from)?;
    if std::fs::read_dir(data_root).is_ok_and(|mut entries| entries.next().is_some()) {
        bail!(
            "{} isn't empty; restore into an empty data root",
            data_root.display()
        );
    }
    let pool = connect(database_url).await?;
    if !database_is_empty(&pool).await? {
        bail!("the target database isn't empty; restore into an empty one");
    }
    restore_database(&set, database_url, identity).await?;
    std::fs::create_dir_all(data_root)?;
    let mut tar = Command::new("tar");
    tar.arg("--extract")
        .arg("--gzip")
        .arg("--file=-")
        .arg("--no-same-owner")
        .arg(format!("--directory={}", data_root.display()));
    run_reader(&set, "data.tar.gz", identity, tar).await?;
    let mut report = report(&pool, &set).await?;
    report.data_entries = list_archive(&set, identity).await?;
    pool.close().await;
    Ok(report)
}

/// `restore drill`: restores the set into a spare (empty) database, reads
/// the data archive end to end, and checks counts, the migration version
/// and that `keyring` opens every restored secret and two-factor seed.
pub async fn drill(
    from: &Path,
    database_url: &str,
    identity: Option<&Path>,
    keyring: Option<&crate::secrets::FileKeyring>,
) -> anyhow::Result<RestoreReport> {
    let set = check_set(from)?;
    let pool = connect(database_url).await?;
    if !database_is_empty(&pool).await? {
        bail!("the spare database isn't empty; drill into an empty one");
    }
    restore_database(&set, database_url, identity).await?;
    let mut report = report(&pool, &set).await?;
    report.data_entries = list_archive(&set, identity).await?;
    // The spare database belongs to no workspace: one plain connection.
    let mut connection = pool.acquire().await?;
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
        "SELECT id::text, organization_id::text, master_key_id, encode(ciphertext, 'hex'), \
         encode(nonce, 'hex'), encode(wrapped_key, 'hex'), coalesce(encode(wrap_nonce, 'hex'), '') \
         FROM secrets ORDER BY id",
    )
    .fetch_all(&mut *connection)
    .await
    .context("read the restored secrets")?;
    let factors = sqlx::query_as::<_, TotpFactorRow>(
        "SELECT user_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id \
         FROM user_totp_factors ORDER BY user_id",
    )
    .fetch_all(&mut *connection)
    .await
    .context("read the restored two-factor seeds")?;
    drop(connection);
    pool.close().await;
    if rows.is_empty() && factors.is_empty() {
        return check_migrations(report);
    }
    let keyring = keyring.context(
        "secrets or two-factor seeds were restored: configure the keyring (or pass --keyring) \
         to prove it opens them",
    )?;
    if !rows.is_empty() {
        let lines = rows
            .iter()
            .map(|row| {
                [&row.0, &row.1, &row.2, &row.3, &row.4, &row.5, &row.6]
                    .map(String::as_str)
                    .join("|")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let opened = crate::secrets::verify_exported_rows(keyring, &lines)
            .map_err(|_| anyhow::anyhow!("the restored secrets are malformed"))?;
        let failed = opened.failed;
        report.secrets = Some(opened);
        if failed > 0 {
            bail!("{failed} restored secrets don't open with the keyring");
        }
    }
    if !factors.is_empty() {
        let opened = verify_totp_factors(keyring, &factors);
        let failed = opened.failed;
        report.two_factor = Some(opened);
        if failed > 0 {
            bail!("{failed} restored two-factor seeds don't open with the keyring");
        }
    }
    check_migrations(report)
}

#[derive(sqlx::FromRow)]
struct TotpFactorRow {
    user_id: uuid::Uuid,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_key: Vec<u8>,
    wrap_nonce: Option<Vec<u8>>,
    master_key_id: String,
}

impl TotpFactorRow {
    fn sealed(&self) -> Option<crate::secrets::DetachedSeal> {
        Some(crate::secrets::DetachedSeal {
            ciphertext: self.ciphertext.clone(),
            nonce: self.nonce.as_slice().try_into().ok()?,
            wrapped: crate::secrets::WrappedKey {
                master_key_id: self.master_key_id.clone(),
                nonce: match self.wrap_nonce.as_deref() {
                    Some(nonce) => Some(nonce.try_into().ok()?),
                    None => None,
                },
                ciphertext: self.wrapped_key.clone(),
            },
        })
    }
}

/// Opens each two-factor seed with `keyring` and its owner's associated
/// data, keeping only the counts.
fn verify_totp_factors(
    keyring: &crate::secrets::FileKeyring,
    factors: &[TotpFactorRow],
) -> crate::secrets::ExportedRowsReport {
    use crate::secrets::KeyEncryption;
    let mut report = crate::secrets::ExportedRowsReport::default();
    for factor in factors {
        report.checked += 1;
        if !keyring.has_key(&factor.master_key_id) {
            report.failed += 1;
            if !report.missing_key_ids.contains(&factor.master_key_id) {
                report.missing_key_ids.push(factor.master_key_id.clone());
            }
            continue;
        }
        let sealed = factor.sealed();
        let opened = sealed.is_some_and(|sealed| {
            crate::secrets::open_detached(
                keyring,
                &crate::secrets::totp_aad(factor.user_id),
                &sealed,
            )
            .is_ok()
        });
        if opened {
            report.opened += 1;
        } else {
            report.failed += 1;
        }
    }
    report
}

fn check_migrations(report: RestoreReport) -> anyhow::Result<RestoreReport> {
    if report.migration_version < report.recorded_migration_version {
        bail!("the restored migration version is older than the set recorded");
    }
    if report.failed_migrations > 0 {
        bail!("the restored database lists failed migrations");
    }
    Ok(report)
}

async fn restore_database(
    set: &CheckedSet,
    database_url: &str,
    identity: Option<&Path>,
) -> anyhow::Result<()> {
    let mut pg_restore = pg_command("pg_restore", database_url)?;
    pg_restore
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("--exit-on-error")
        .arg("--no-password")
        .arg(format!(
            "--dbname={}",
            libpq_environment(database_url)?
                .into_iter()
                .find(|(name, _)| *name == "PGDATABASE")
                .map(|(_, value)| value)
                .context("the database URL names no database")?
        ));
    run_reader(set, "database.dump", identity, pg_restore)
        .await
        .context("restore the database")
}

async fn run_reader(
    set: &CheckedSet,
    file: &'static str,
    identity: Option<&Path>,
    consumer: Command,
) -> anyhow::Result<()> {
    let set = CheckedSet {
        path: set.path.clone(),
        encrypted: set.encrypted,
        migration_version: set.migration_version,
    };
    let identity = identity.map(Path::to_path_buf);
    tokio::task::spawn_blocking(move || read_stream(&set, file, identity.as_deref(), consumer))
        .await
        .context("the restore stopped")?
}

async fn list_archive(set: &CheckedSet, identity: Option<&Path>) -> anyhow::Result<usize> {
    let listing = std::env::temp_dir().join(format!(
        "agent-workspace-archive-{}.list",
        uuid::Uuid::now_v7().simple()
    ));
    let file = std::fs::File::create(&listing)?;
    let mut tar = Command::new("tar");
    tar.arg("--list")
        .arg("--gzip")
        .arg("--file=-")
        .stdout(Stdio::from(file));
    let listed = run_reader(set, "data.tar.gz", identity, tar).await;
    let entries = std::fs::read_to_string(&listing).map(|text| text.lines().count());
    let _ = std::fs::remove_file(&listing);
    listed.context("read the data archive")?;
    Ok(entries?)
}

async fn report(pool: &PgPool, set: &CheckedSet) -> anyhow::Result<RestoreReport> {
    let count = |table: &'static str| async move {
        sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .with_context(|| format!("count {table}"))
    };
    Ok(RestoreReport {
        set: set.path.clone(),
        organizations: count("organizations").await?,
        projects: count("projects").await?,
        messages: count("messages").await?,
        migration_version: sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(max(version), 0) FROM _sqlx_migrations",
        )
        .fetch_one(pool)
        .await?,
        recorded_migration_version: set.migration_version,
        failed_migrations: sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM _sqlx_migrations WHERE NOT success",
        )
        .fetch_one(pool)
        .await?,
        data_entries: 0,
        secrets: None,
        two_factor: None,
    })
}

/// Writes `contents` to a new owner-only file (for tests and the key
/// command's callers).
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_url_reaches_client_programs_in_their_environment() {
        let environment = libpq_environment(
            "postgres://aw%20app:p%40ss@db.example:6543/agent_workspace?sslmode=require",
        )
        .expect("environment");
        let environment = environment.into_iter().collect::<BTreeMap<_, _>>();
        assert_eq!(environment["PGHOST"], "db.example");
        assert_eq!(environment["PGPORT"], "6543");
        assert_eq!(environment["PGUSER"], "aw app");
        assert_eq!(environment["PGPASSWORD"], "p@ss");
        assert_eq!(environment["PGDATABASE"], "agent_workspace");
        assert_eq!(environment["PGSSLMODE"], "require");
        let command =
            pg_command("pg_dump", "postgres://user:secret@127.0.0.1/database").expect("command");
        assert!(
            command
                .get_args()
                .all(|argument| !argument.to_string_lossy().contains("secret")),
            "the password is never an argument"
        );
        assert!(libpq_environment("mysql://host/db").is_err());
    }

    #[test]
    fn old_sets_are_pruned_but_the_newest_few_are_kept() {
        let folder = tempfile::tempdir().expect("folder");
        let now = "2026-11-14T03:30:00Z"
            .parse::<DateTime<Utc>>()
            .expect("now");
        for days in [1, 3, 10, 20, 30] {
            std::fs::create_dir(
                folder
                    .path()
                    .join(set_name(now - chrono::Duration::days(days))),
            )
            .expect("set");
        }
        std::fs::create_dir(folder.path().join("not-a-set")).expect("other folder");
        let removed = prune(folder.path(), 7, now);
        assert_eq!(removed.len(), 2, "{removed:?}");
        assert_eq!(sets(folder.path()).len(), 3);
        assert!(folder.path().join("not-a-set").exists());
        // With only a few sets, even old ones stay.
        assert!(prune(folder.path(), 0, now).is_empty());
        assert!(valid_set_name("20261114T033000Z"));
        assert!(!valid_set_name("20261114T033000Z.partial"));
    }
}
