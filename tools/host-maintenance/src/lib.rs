#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const BACKUP_FORMAT_VERSION: u32 = 1;
pub const ARCHIVE_FORMAT: &str = "agent-workspace-private-host-v1";
pub const EXCLUDED_TABLE_DATA: &[&str] = &[
    "public.sessions",
    "public.user_password_credentials",
    "public.mcp_client_credentials",
    "public.mcp_oauth_clients",
    "public.mcp_oauth_authorization_codes",
    "public.mcp_oauth_grants",
    "public.mcp_oauth_access_tokens",
    "public.mcp_oauth_refresh_tokens",
];
pub const INCLUDED_DATA_ENTRIES: &[&str] =
    &["artifacts", "attachments", "repositories", "worktrees"];
pub const REBUILDABLE_CACHE_ENTRIES: &[&str] = &["repositories"];
pub const PRESERVED_UNIQUE_ENTRIES: &[&str] = &["artifacts", "attachments", "worktrees"];
pub const RESTORED_DATA_OWNER: &str = "agent-workspace:agent-workspace";
pub const SUPPORTED_DATA_ROOT: &str = "/var/lib/agent-workspace";
pub const RETENTION_POLICY_VERSION: u32 = 1;
pub const SUPPORTED_RELEASES_ROOT: &str = "/opt/agent-workspace/releases";
pub const SUPPORTED_BACKUPS_ROOT: &str = "/var/backups/agent-workspace";

const MAX_SECRET_FILE_BYTES: u64 = 8 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 250_000;

#[derive(Clone, Debug)]
pub struct BackupOptions {
    pub destination: PathBuf,
    pub data_root: PathBuf,
    pub release_root: PathBuf,
    pub release_commit: String,
    pub database_name_file: PathBuf,
    pub pg_dump: PathBuf,
    pub psql: PathBuf,
    pub tar: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RestoreOptions {
    pub source: PathBuf,
    pub target_data_root: PathBuf,
    pub target_release_root: PathBuf,
    pub target_database_name_file: PathBuf,
    pub active_database_name_file: PathBuf,
    pub pg_restore: PathBuf,
    pub psql: PathBuf,
    pub tar: PathBuf,
    pub chown: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StorageReportOptions {
    pub data_root: PathBuf,
    pub releases_root: PathBuf,
    pub backups_root: PathBuf,
    pub database_name_file: PathBuf,
    pub psql: PathBuf,
    pub data_quota_bytes: u64,
    pub minimum_free_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct RetentionOptions {
    pub database_name_file: PathBuf,
    pub psql: PathBuf,
    pub apply: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format_version: u32,
    pub archive_format: String,
    pub created_at_unix_seconds: u64,
    pub release_commit: String,
    pub postgres_major: u32,
    pub source_layout: SourceLayout,
    pub excluded_table_data: Vec<String>,
    pub migrations: Vec<MigrationRecord>,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLayout {
    pub data_layout: String,
    pub release_layout: String,
    pub included_data_entries: Vec<String>,
    pub rebuildable_cache_entries: Vec<String>,
    pub preserved_unique_entries: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecord {
    pub version: i64,
    pub checksum_sha384: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub destination: PathBuf,
    pub release_commit: String,
    pub postgres_major: u32,
    pub migration_count: usize,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub source: PathBuf,
    pub target_data_root: PathBuf,
    pub target_release_root: PathBuf,
    pub release_commit: String,
    pub credential_rows_restored: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCategory {
    pub category: String,
    pub allocated_bytes: u64,
    pub regular_files: u64,
    pub directories: u64,
    pub skipped_symlinks: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    pub policy_version: u32,
    pub postgres_bytes: u64,
    pub filesystem_available_bytes: u64,
    pub data_quota_bytes: u64,
    pub minimum_free_bytes: u64,
    pub growth_admitted: bool,
    pub rejection_categories: Vec<String>,
    pub categories: Vec<StorageCategory>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionCategory {
    pub category: String,
    #[serde(alias = "eligible_rows")]
    pub eligible_rows: u64,
    #[serde(alias = "affected_rows")]
    pub affected_rows: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionResult {
    pub policy_version: u32,
    pub mode: String,
    pub locked: bool,
    pub categories: Vec<RetentionCategory>,
}

pub fn create_backup(options: &BackupOptions) -> Result<BackupResult> {
    validate_release_commit(&options.release_commit)?;
    let data_root = validate_source_root(&options.data_root, "data root")?;
    ensure!(
        data_root == Path::new(SUPPORTED_DATA_ROOT),
        "backup data root does not match the supported Red Hat layout"
    );
    let release_root = validate_source_root(&options.release_root, "release root")?;
    ensure!(
        release_root.file_name().and_then(OsStr::to_str) == Some(options.release_commit.as_str()),
        "release root must be named for the exact release commit"
    );
    ensure!(
        data_root != release_root,
        "data root and release root must be distinct"
    );
    for executable in [&options.pg_dump, &options.psql, &options.tar] {
        validate_executable(executable)?;
    }
    let database_name = read_database_name_file(&options.database_name_file)?;
    let destination = validate_new_destination(
        &options.destination,
        &[data_root.as_path(), release_root.as_path()],
    )?;
    let staging = staging_path(&destination)?;
    fs::create_dir(&staging)
        .with_context(|| format!("create backup staging directory {}", staging.display()))?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure backup staging directory {}", staging.display()))?;
    let mut cleanup = CleanupDirectory::new(staging.clone());

    let database_dump = staging.join("database.dump");
    create_database_dump(&options.pg_dump, database_name.as_str(), &database_dump)?;
    let migrations = read_migrations(&options.psql, database_name.as_str())?;
    ensure!(
        !migrations.is_empty(),
        "database has no successful migrations"
    );
    let postgres_major = read_postgres_major(&options.psql, database_name.as_str())?;

    let data_archive = staging.join("data.tar");
    create_data_archive(&options.tar, &data_root, &data_archive)?;
    let release_archive = staging.join("release.tar");
    create_release_archive(&options.tar, &release_root, &release_archive)?;

    let artifacts = [database_dump, data_archive, release_archive]
        .iter()
        .map(|path| artifact_record(path))
        .collect::<Result<Vec<_>>>()?;
    let manifest = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        archive_format: ARCHIVE_FORMAT.to_owned(),
        created_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock precedes Unix epoch")?
            .as_secs(),
        release_commit: options.release_commit.clone(),
        postgres_major,
        source_layout: SourceLayout {
            data_layout: "agent-workspace-data-root-v1".to_owned(),
            release_layout: "immutable-release-v1".to_owned(),
            included_data_entries: INCLUDED_DATA_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect(),
            rebuildable_cache_entries: REBUILDABLE_CACHE_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect(),
            preserved_unique_entries: PRESERVED_UNIQUE_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect(),
        },
        excluded_table_data: EXCLUDED_TABLE_DATA
            .iter()
            .map(|table| (*table).to_owned())
            .collect(),
        migrations,
        artifacts: artifacts.clone(),
    };
    write_manifest(&staging.join("manifest.json"), &manifest)?;
    sync_directory(&staging)?;
    fs::rename(&staging, &destination).with_context(|| {
        format!(
            "publish backup directory {} as {}",
            staging.display(),
            destination.display()
        )
    })?;
    sync_directory(
        destination
            .parent()
            .context("backup destination has no parent")?,
    )?;
    cleanup.commit();

    Ok(BackupResult {
        destination,
        release_commit: manifest.release_commit,
        postgres_major,
        migration_count: manifest.migrations.len(),
        artifacts,
    })
}

pub fn validate_backup(source: &Path) -> Result<BackupManifest> {
    let source = validate_existing_owner_directory(source, "backup source")?;
    let manifest_path = source.join("manifest.json");
    ensure_regular_file(&manifest_path, "backup manifest")?;
    let metadata = fs::metadata(&manifest_path)
        .with_context(|| format!("inspect backup manifest {}", manifest_path.display()))?;
    ensure!(
        metadata.len() <= 1024 * 1024,
        "backup manifest is too large"
    );
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("read backup manifest {}", manifest_path.display()))?;
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest_bytes).context("parse backup manifest")?;
    validate_manifest(&manifest)?;

    for artifact in &manifest.artifacts {
        let path = source.join(&artifact.file);
        ensure_regular_file(&path, "backup artifact")?;
        let observed = artifact_record(&path)?;
        ensure!(
            observed.bytes == artifact.bytes && observed.sha256 == artifact.sha256,
            "backup artifact checksum mismatch for {}",
            artifact.file
        );
    }
    Ok(manifest)
}

pub fn storage_report(options: &StorageReportOptions) -> Result<StorageReport> {
    ensure!(
        options.data_quota_bytes >= 1024 * 1024 * 1024,
        "data quota must be at least one GiB"
    );
    ensure!(
        options.minimum_free_bytes >= 1024 * 1024 * 1024,
        "minimum free space must be at least one GiB"
    );
    let data_root = validate_source_root(&options.data_root, "data root")?;
    let releases_root = validate_source_root(&options.releases_root, "releases root")?;
    let backups_root = validate_source_root(&options.backups_root, "backups root")?;
    ensure!(
        data_root == Path::new(SUPPORTED_DATA_ROOT)
            && releases_root == Path::new(SUPPORTED_RELEASES_ROOT)
            && backups_root == Path::new(SUPPORTED_BACKUPS_ROOT),
        "storage report roots do not match the supported Red Hat layout"
    );
    validate_executable(&options.psql)?;
    let database_name = read_database_name_file(&options.database_name_file)?;
    let postgres_bytes: u64 = run_psql(
        &options.psql,
        database_name.as_str(),
        "SELECT pg_database_size(current_database())",
    )?
    .trim()
    .parse()
    .context("parse PostgreSQL database size")?;

    let categories = vec![
        scan_storage_category("data_total", &data_root)?,
        scan_storage_category("attachments", &data_root.join("attachments"))?,
        scan_storage_category("artifacts", &data_root.join("artifacts"))?,
        scan_storage_category("repository_mirrors", &data_root.join("repositories"))?,
        scan_storage_category("worktrees", &data_root.join("worktrees"))?,
        scan_storage_category("provider_profiles", &data_root.join("codex/profiles"))?,
        scan_storage_category("temporary_files", &data_root.join("tmp"))?,
        scan_storage_category("immutable_releases", &releases_root)?,
        scan_storage_category("backup_archives", &backups_root)?,
    ];
    let data_bytes = categories
        .first()
        .context("data storage category is missing")?
        .allocated_bytes;
    let filesystem = rustix::fs::statvfs(&data_root).context("inspect data filesystem")?;
    let filesystem_available_bytes = filesystem.f_bavail.saturating_mul(filesystem.f_frsize);
    let mut rejection_categories = Vec::new();
    if data_bytes >= options.data_quota_bytes {
        rejection_categories.push("data_quota_reached".to_owned());
    }
    if filesystem_available_bytes <= options.minimum_free_bytes {
        rejection_categories.push("minimum_free_space_reached".to_owned());
    }

    Ok(StorageReport {
        policy_version: RETENTION_POLICY_VERSION,
        postgres_bytes,
        filesystem_available_bytes,
        data_quota_bytes: options.data_quota_bytes,
        minimum_free_bytes: options.minimum_free_bytes,
        growth_admitted: rejection_categories.is_empty(),
        rejection_categories,
        categories,
    })
}

pub fn run_retention(options: &RetentionOptions) -> Result<RetentionResult> {
    validate_executable(&options.psql)?;
    let database_name = read_database_name_file(&options.database_name_file)?;
    let query = if options.apply {
        retention_apply_query()
    } else {
        retention_report_query()
    };
    let encoded = run_psql(&options.psql, database_name.as_str(), query)?;
    let categories: Vec<RetentionCategory> =
        serde_json::from_str(encoded.trim()).context("parse retention summary")?;
    ensure!(
        categories.len() == 7,
        "retention summary did not contain every policy category"
    );
    ensure!(
        categories
            .iter()
            .all(|category| category.affected_rows <= category.eligible_rows),
        "retention summary reported more affected than eligible rows"
    );
    Ok(RetentionResult {
        policy_version: RETENTION_POLICY_VERSION,
        mode: if options.apply { "apply" } else { "dry_run" }.to_owned(),
        locked: options.apply,
        categories,
    })
}

fn retention_report_query() -> &'static str {
    "SELECT json_agg(row_to_json(summary) ORDER BY summary.category)::text FROM (VALUES \
    ('expired_idempotency', (SELECT count(*) FROM idempotency_keys WHERE expires_at <= now() - interval '1 day'), 0::bigint), \
    ('expired_invitations', (SELECT count(*) FROM project_invitations WHERE status = 'active' AND expires_at <= now()), 0::bigint), \
    ('oauth_authorization_codes', (SELECT count(*) FROM mcp_oauth_authorization_codes WHERE (used_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '1 day'), 0::bigint), \
    ('oauth_access_tokens', (SELECT count(*) FROM mcp_oauth_access_tokens WHERE (revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '7 days'), 0::bigint), \
    ('oauth_refresh_tokens', (SELECT count(*) FROM mcp_oauth_refresh_tokens WHERE (used_at IS NOT NULL OR revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '30 days'), 0::bigint), \
    ('published_outbox', (SELECT count(*) FROM outbox_events WHERE published_at <= now() - interval '30 days'), 0::bigint), \
    ('completed_temporary_jobs', (SELECT count(*) FROM github_jobs WHERE state IN ('completed', 'exhausted') AND updated_at <= now() - interval '30 days'), 0::bigint) \
    ) AS summary(category, eligible_rows, affected_rows)"
}

fn retention_apply_query() -> &'static str {
    "BEGIN; SELECT pg_advisory_xact_lock(470127786, 1); WITH \
    expired_idempotency AS (DELETE FROM idempotency_keys WHERE expires_at <= now() - interval '1 day' RETURNING 1), \
    expired_invitations AS (UPDATE project_invitations SET status='revoked', revoked_at=now(), version=version+1, updated_at=now() WHERE status='active' AND expires_at <= now() RETURNING 1), \
    oauth_authorization_codes AS (DELETE FROM mcp_oauth_authorization_codes WHERE (used_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '1 day' RETURNING 1), \
    oauth_access_tokens AS (DELETE FROM mcp_oauth_access_tokens WHERE (revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '7 days' RETURNING 1), \
    refresh_eligible AS (SELECT id FROM mcp_oauth_refresh_tokens WHERE (used_at IS NOT NULL OR revoked_at IS NOT NULL OR expires_at <= now()) AND created_at <= now() - interval '30 days' FOR UPDATE), \
    refresh_links AS (UPDATE mcp_oauth_refresh_tokens SET rotated_to_token_id=NULL WHERE rotated_to_token_id IN (SELECT id FROM refresh_eligible) RETURNING 1), \
    oauth_refresh_tokens AS (DELETE FROM mcp_oauth_refresh_tokens WHERE id IN (SELECT id FROM refresh_eligible) RETURNING 1), \
    published_outbox AS (DELETE FROM outbox_events WHERE published_at <= now() - interval '30 days' RETURNING 1), \
    completed_temporary_jobs AS (DELETE FROM github_jobs WHERE state IN ('completed', 'exhausted') AND updated_at <= now() - interval '30 days' RETURNING 1), \
    summary AS (SELECT * FROM (VALUES \
      ('expired_idempotency', (SELECT count(*) FROM expired_idempotency), (SELECT count(*) FROM expired_idempotency)), \
      ('expired_invitations', (SELECT count(*) FROM expired_invitations), (SELECT count(*) FROM expired_invitations)), \
      ('oauth_authorization_codes', (SELECT count(*) FROM oauth_authorization_codes), (SELECT count(*) FROM oauth_authorization_codes)), \
      ('oauth_access_tokens', (SELECT count(*) FROM oauth_access_tokens), (SELECT count(*) FROM oauth_access_tokens)), \
      ('oauth_refresh_tokens', (SELECT count(*) FROM oauth_refresh_tokens), (SELECT count(*) FROM oauth_refresh_tokens)), \
      ('published_outbox', (SELECT count(*) FROM published_outbox), (SELECT count(*) FROM published_outbox)), \
      ('completed_temporary_jobs', (SELECT count(*) FROM completed_temporary_jobs), (SELECT count(*) FROM completed_temporary_jobs)) \
    ) AS value(category, eligible_rows, affected_rows)) \
    SELECT json_agg(row_to_json(summary) ORDER BY summary.category)::text FROM summary; COMMIT;"
}

pub fn restore_backup(options: &RestoreOptions) -> Result<RestoreResult> {
    let source = validate_existing_owner_directory(&options.source, "backup source")?;
    let manifest = validate_backup(&source)?;
    for executable in [
        &options.pg_restore,
        &options.psql,
        &options.tar,
        &options.chown,
    ] {
        validate_executable(executable)?;
    }
    let active_database_name = read_database_name_file(&options.active_database_name_file)?;
    let target_database_name = read_database_name_file(&options.target_database_name_file)?;
    ensure!(
        active_database_name.as_str() != target_database_name.as_str(),
        "restore target database must differ from the active database"
    );
    let active_database_identity = database_identity(&options.psql, active_database_name.as_str())?;
    let target_database_identity = database_identity(&options.psql, target_database_name.as_str())?;
    ensure!(
        active_database_identity != target_database_identity,
        "restore target database resolves to the active database"
    );
    ensure!(
        read_postgres_major(&options.psql, target_database_name.as_str())?
            == manifest.postgres_major,
        "restore target PostgreSQL major is incompatible with the backup"
    );
    ensure_database_empty(&options.psql, target_database_name.as_str())?;

    let target_data_root = validate_new_destination(&options.target_data_root, &[&source])?;
    let target_release_root =
        validate_new_destination(&options.target_release_root, &[&source, &target_data_root])?;
    ensure!(
        !path_contains(&target_data_root, &target_release_root)
            && !path_contains(&target_release_root, &target_data_root),
        "restore data and release roots must not contain one another"
    );
    validate_archive(&options.tar, &source.join("data.tar"))?;
    validate_archive(&options.tar, &source.join("release.tar"))?;

    fs::create_dir(&target_data_root)
        .with_context(|| format!("create restore data root {}", target_data_root.display()))?;
    fs::set_permissions(&target_data_root, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure restore data root {}", target_data_root.display()))?;
    fs::create_dir(&target_release_root).with_context(|| {
        format!(
            "create restore release root {}",
            target_release_root.display()
        )
    })?;
    fs::set_permissions(&target_release_root, fs::Permissions::from_mode(0o755)).with_context(
        || {
            format!(
                "secure restore release root {}",
                target_release_root.display()
            )
        },
    )?;

    restore_database(
        &options.pg_restore,
        &options.psql,
        target_database_name.as_str(),
        &source.join("database.dump"),
    )?;
    ensure!(
        read_migrations(&options.psql, target_database_name.as_str())? == manifest.migrations,
        "restored migration history does not match the backup manifest"
    );
    reset_restored_credentials(&options.psql, target_database_name.as_str())?;
    extract_archive(&options.tar, &source.join("data.tar"), &target_data_root)?;
    extract_archive(
        &options.tar,
        &source.join("release.tar"),
        &target_release_root,
    )?;
    assign_restored_data_owner(&options.chown, &target_data_root)?;
    let credential_rows_restored =
        credential_row_count(&options.psql, target_database_name.as_str())?;
    ensure!(
        credential_rows_restored == 0,
        "credential-bearing rows were present after restore"
    );

    Ok(RestoreResult {
        source,
        target_data_root,
        target_release_root,
        release_commit: manifest.release_commit,
        credential_rows_restored,
    })
}

fn validate_manifest(manifest: &BackupManifest) -> Result<()> {
    ensure!(
        manifest.format_version == BACKUP_FORMAT_VERSION,
        "unsupported backup format version"
    );
    ensure!(
        manifest.archive_format == ARCHIVE_FORMAT,
        "unsupported archive format"
    );
    validate_release_commit(&manifest.release_commit)?;
    ensure!(
        manifest.postgres_major == 18,
        "unsupported PostgreSQL major"
    );
    let expected_exclusions = EXCLUDED_TABLE_DATA
        .iter()
        .map(|table| (*table).to_owned())
        .collect::<Vec<_>>();
    ensure!(
        manifest.excluded_table_data == expected_exclusions,
        "backup credential-table exclusion set is incomplete"
    );
    ensure!(
        manifest.source_layout.included_data_entries
            == INCLUDED_DATA_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect::<Vec<_>>(),
        "backup data-entry set is unsupported"
    );
    ensure!(
        manifest.source_layout.rebuildable_cache_entries
            == REBUILDABLE_CACHE_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect::<Vec<_>>(),
        "backup rebuildable-cache classification is unsupported"
    );
    ensure!(
        manifest.source_layout.preserved_unique_entries
            == PRESERVED_UNIQUE_ENTRIES
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect::<Vec<_>>(),
        "backup preserved-data classification is unsupported"
    );
    ensure!(!manifest.migrations.is_empty(), "backup has no migrations");
    let mut previous_version = None;
    for migration in &manifest.migrations {
        ensure!(migration.version > 0, "migration version is invalid");
        ensure!(
            migration.checksum_sha384.len() == 96
                && migration
                    .checksum_sha384
                    .chars()
                    .all(|value| value.is_ascii_hexdigit()),
            "migration checksum is invalid"
        );
        if let Some(previous_version) = previous_version {
            ensure!(
                migration.version > previous_version,
                "migration versions are not strictly ordered"
            );
        }
        previous_version = Some(migration.version);
    }
    let expected_files = BTreeSet::from([
        "data.tar".to_owned(),
        "database.dump".to_owned(),
        "release.tar".to_owned(),
    ]);
    let observed_files = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.file.clone())
        .collect::<BTreeSet<_>>();
    ensure!(
        observed_files == expected_files,
        "backup artifact set is invalid"
    );
    for artifact in &manifest.artifacts {
        ensure!(
            safe_manifest_file(&artifact.file),
            "backup artifact path is unsafe"
        );
        ensure!(
            artifact.sha256.len() == 64
                && artifact
                    .sha256
                    .chars()
                    .all(|value| value.is_ascii_hexdigit()),
            "backup artifact checksum is invalid"
        );
    }
    Ok(())
}

fn validate_release_commit(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit()),
        "release commit must be a full hexadecimal Git object ID"
    );
    Ok(())
}

fn validate_source_root(path: &Path, label: &str) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "{label} must be absolute");
    reject_symlink_components(path)?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolve {label} {}", path.display()))?;
    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("inspect {label} {}", canonical.display()))?;
    ensure!(metadata.is_dir(), "{label} must be a directory");
    Ok(canonical)
}

fn validate_existing_owner_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = validate_source_root(path, label)?;
    let mode = fs::metadata(&canonical)
        .with_context(|| format!("inspect {label} permissions"))?
        .permissions()
        .mode();
    ensure!(
        mode & 0o077 == 0,
        "{label} must not grant group or other access"
    );
    Ok(canonical)
}

fn validate_new_destination(path: &Path, sources: &[&Path]) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "destination path must be absolute");
    ensure!(
        path.file_name().is_some(),
        "destination path has no final component"
    );
    ensure!(!path.exists(), "destination already exists");
    let parent = path.parent().context("destination has no parent")?;
    reject_symlink_components(parent)?;
    let parent = parent
        .canonicalize()
        .with_context(|| format!("resolve destination parent {}", parent.display()))?;
    let mode = fs::metadata(&parent)
        .with_context(|| format!("inspect destination parent {}", parent.display()))?
        .permissions()
        .mode();
    ensure!(
        mode & 0o077 == 0,
        "destination parent must not grant group or other access"
    );
    let candidate = parent.join(path.file_name().context("destination has no name")?);
    for source in sources {
        ensure!(
            !path_contains(source, &candidate) && !path_contains(&candidate, source),
            "destination must be outside every captured source"
        );
    }
    Ok(candidate)
}

fn validate_executable(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "maintenance executable paths must be absolute"
    );
    reject_symlink_components(path)?;
    ensure_regular_file(path, "maintenance executable")
}

fn ensure_regular_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{label} must be a regular file"
    );
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        resolved.push(component.as_os_str());
        match fs::symlink_metadata(&resolved) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "symlink path component is not accepted: {}",
                resolved.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect path component {}", resolved.display()));
            }
        }
    }
    Ok(())
}

fn read_secret_file(path: &Path) -> Result<Zeroizing<String>> {
    ensure!(path.is_absolute(), "secret file path must be absolute");
    reject_symlink_components(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect secret file {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "secret file must be regular"
    );
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "secret file must not grant group or other access"
    );
    ensure!(
        metadata.len() <= MAX_SECRET_FILE_BYTES,
        "secret file is too large"
    );
    let mut value = Zeroizing::new(String::new());
    File::open(path)
        .with_context(|| format!("open secret file {}", path.display()))?
        .take(MAX_SECRET_FILE_BYTES + 1)
        .read_to_string(&mut value)
        .with_context(|| format!("read secret file {}", path.display()))?;
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    ensure!(!value.is_empty(), "secret file is empty");
    ensure!(
        !value.contains(['\n', '\r', '\0']),
        "secret file must contain exactly one bounded value"
    );
    Ok(value)
}

fn read_database_name_file(path: &Path) -> Result<Zeroizing<String>> {
    let value = read_secret_file(path)?;
    ensure!(
        value.len() <= 63
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "maintenance database file must contain only a PostgreSQL database name"
    );
    Ok(value)
}

fn create_database_dump(program: &Path, database_url: &str, output: &Path) -> Result<()> {
    let mut args = vec![
        OsString::from("--format=custom"),
        OsString::from("--no-owner"),
        OsString::from("--no-privileges"),
    ];
    for table in EXCLUDED_TABLE_DATA {
        args.push(OsString::from(format!("--exclude-table-data={table}")));
    }
    run_to_file(
        program,
        &args,
        database_url,
        output,
        "create PostgreSQL dump",
    )
}

fn read_migrations(program: &Path, database_url: &str) -> Result<Vec<MigrationRecord>> {
    let query = "SELECT version, encode(checksum, 'hex') FROM _sqlx_migrations WHERE success ORDER BY version";
    let output = run_psql(program, database_url, query)?;
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (version, checksum) = line
                .split_once('|')
                .context("migration query returned an invalid row")?;
            Ok(MigrationRecord {
                version: version.parse().context("parse migration version")?,
                checksum_sha384: checksum.to_owned(),
            })
        })
        .collect()
}

fn read_postgres_major(program: &Path, database_url: &str) -> Result<u32> {
    let output = run_psql(
        program,
        database_url,
        "SELECT current_setting('server_version_num')",
    )?;
    let version: u32 = output.trim().parse().context("parse PostgreSQL version")?;
    Ok(version / 10_000)
}

fn database_identity(program: &Path, database_url: &str) -> Result<String> {
    let output = run_psql(
        program,
        database_url,
        "SELECT current_setting('server_version_num') || ':' || current_database()",
    )?;
    let identity = output.trim();
    ensure!(
        !identity.is_empty(),
        "PostgreSQL database identity is empty"
    );
    Ok(identity.to_owned())
}

fn create_data_archive(program: &Path, data_root: &Path, output: &Path) -> Result<()> {
    let mut args = vec![
        OsString::from("--format=pax"),
        OsString::from("--create"),
        OsString::from("--file"),
        output.as_os_str().to_owned(),
        OsString::from("--directory"),
        data_root.as_os_str().to_owned(),
    ];
    let mut included = 0_usize;
    for entry in INCLUDED_DATA_ENTRIES {
        let path = data_root.join(entry);
        if !path.exists() {
            continue;
        }
        ensure_no_symlinks(&path)?;
        args.push(OsString::from(entry));
        included += 1;
    }
    if included == 0 {
        args.push(OsString::from("--files-from"));
        args.push(OsString::from("/dev/null"));
    }
    run_status(program, &args, None, "create application data archive")
}

fn create_release_archive(program: &Path, release_root: &Path, output: &Path) -> Result<()> {
    ensure_no_symlinks(release_root)?;
    let args = vec![
        OsString::from("--format=pax"),
        OsString::from("--create"),
        OsString::from("--file"),
        output.as_os_str().to_owned(),
        OsString::from("--directory"),
        release_root.as_os_str().to_owned(),
        OsString::from("."),
    ];
    run_status(program, &args, None, "create immutable release archive")
}

fn ensure_no_symlinks(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0_usize;
    while let Some(path) = pending.pop() {
        entries += 1;
        ensure!(
            entries <= MAX_ARCHIVE_ENTRIES,
            "captured source has too many entries"
        );
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect captured path {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "captured sources must not contain symlinks"
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)
                .with_context(|| format!("read captured directory {}", path.display()))?
            {
                pending.push(entry.context("read captured directory entry")?.path());
            }
        }
    }
    Ok(())
}

fn scan_storage_category(category: &str, root: &Path) -> Result<StorageCategory> {
    if !root.exists() {
        return Ok(StorageCategory {
            category: category.to_owned(),
            allocated_bytes: 0,
            regular_files: 0,
            directories: 0,
            skipped_symlinks: 0,
        });
    }
    reject_symlink_components(root)?;
    let mut pending = vec![root.to_path_buf()];
    let mut allocated_bytes = 0_u64;
    let mut regular_files = 0_u64;
    let mut directories = 0_u64;
    let mut skipped_symlinks = 0_u64;
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect storage category {category}"))?;
        if metadata.file_type().is_symlink() {
            skipped_symlinks = skipped_symlinks.saturating_add(1);
            continue;
        } else if metadata.is_dir() {
            directories = directories.saturating_add(1);
            for entry in
                fs::read_dir(&path).with_context(|| format!("read storage category {category}"))?
            {
                pending.push(
                    entry
                        .with_context(|| format!("read storage category entry {category}"))?
                        .path(),
                );
            }
        } else if metadata.is_file() {
            regular_files = regular_files.saturating_add(1);
            allocated_bytes = allocated_bytes.saturating_add(metadata.blocks().saturating_mul(512));
        } else {
            bail!("storage category contains an unsupported filesystem entry");
        }
    }
    Ok(StorageCategory {
        category: category.to_owned(),
        allocated_bytes,
        regular_files,
        directories,
        skipped_symlinks,
    })
}

fn artifact_record(path: &Path) -> Result<ArtifactRecord> {
    let mut file =
        File::open(path).with_context(|| format!("open backup artifact {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hash backup artifact {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes = bytes.saturating_add(read as u64);
    }
    Ok(ArtifactRecord {
        file: path
            .file_name()
            .and_then(OsStr::to_str)
            .context("backup artifact filename is not UTF-8")?
            .to_owned(),
        bytes,
        sha256: hex::encode(digest.finalize()),
    })
}

fn write_manifest(path: &Path, manifest: &BackupManifest) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create backup manifest {}", path.display()))?;
    let mut encoded = serde_json::to_vec_pretty(manifest).context("serialize backup manifest")?;
    encoded.push(b'\n');
    file.write_all(&encoded)
        .with_context(|| format!("write backup manifest {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync backup manifest {}", path.display()))
}

fn run_psql(program: &Path, database_url: &str, query: &str) -> Result<String> {
    let args = vec![
        OsString::from("--no-psqlrc"),
        OsString::from("--quiet"),
        OsString::from("--tuples-only"),
        OsString::from("--no-align"),
        OsString::from("--field-separator=|"),
        OsString::from("--command"),
        OsString::from(query),
    ];
    let output = run_output(program, &args, Some(database_url), "query PostgreSQL")?;
    String::from_utf8(output.stdout).context("PostgreSQL query returned non-UTF-8 output")
}

fn run_status(
    program: &Path,
    args: &[OsString],
    database_url: Option<&str>,
    operation: &str,
) -> Result<()> {
    let output = run_output(program, args, database_url, operation)?;
    ensure!(
        output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES
            && output.stderr.len() <= MAX_COMMAND_OUTPUT_BYTES,
        "{operation} output exceeded the bounded limit"
    );
    Ok(())
}

fn run_output(
    program: &Path,
    args: &[OsString],
    database_url: Option<&str>,
    operation: &str,
) -> Result<Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8");
    if let Some(database_url) = database_url {
        command.env("PGDATABASE", database_url);
    }
    let output = command
        .output()
        .with_context(|| format!("start {operation}"))?;
    ensure!(output.status.success(), "{operation} failed");
    ensure!(
        output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES
            && output.stderr.len() <= MAX_COMMAND_OUTPUT_BYTES,
        "{operation} output exceeded the bounded limit"
    );
    Ok(output)
}

fn run_to_file(
    program: &Path,
    args: &[OsString],
    database_url: &str,
    destination: &Path,
    operation: &str,
) -> Result<()> {
    let destination_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination)
        .with_context(|| format!("create command output {}", destination.display()))?;
    let child = Command::new(program)
        .args(args)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("PGDATABASE", database_url)
        .stdout(Stdio::from(destination_file))
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {operation}"))?;
    let output = child
        .wait_with_output()
        .with_context(|| format!("wait for {operation}"))?;
    ensure!(output.status.success(), "{operation} failed");
    ensure!(
        output.stderr.len() <= MAX_COMMAND_OUTPUT_BYTES,
        "{operation} output exceeded the bounded limit"
    );
    Ok(())
}

fn run_from_file(
    program: &Path,
    args: &[OsString],
    database_url: &str,
    source: &Path,
    operation: &str,
) -> Result<()> {
    let source_file =
        File::open(source).with_context(|| format!("open command input {}", source.display()))?;
    let output = Command::new(program)
        .args(args)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("PGDATABASE", database_url)
        .stdin(Stdio::from(source_file))
        .output()
        .with_context(|| format!("start {operation}"))?;
    ensure!(output.status.success(), "{operation} failed");
    ensure!(
        output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES
            && output.stderr.len() <= MAX_COMMAND_OUTPUT_BYTES,
        "{operation} output exceeded the bounded limit"
    );
    Ok(())
}

fn ensure_database_empty(program: &Path, database_url: &str) -> Result<()> {
    let query = "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%'";
    let output = run_psql(program, database_url, query)?;
    let count: u64 = output
        .trim()
        .parse()
        .context("parse restore target object count")?;
    ensure!(count == 0, "restore target database is not empty");
    Ok(())
}

fn restore_database(program: &Path, psql: &Path, database_url: &str, dump: &Path) -> Result<()> {
    for section in ["pre-data", "data"] {
        restore_database_section(program, database_url, dump, section)?;
    }
    // Credential rows are intentionally absent from ordinary backups. Audit
    // history remains durable, but its optional credential correlations must
    // be cleared before post-data foreign keys are installed.
    let _ = run_psql(
        psql,
        database_url,
        "UPDATE audit_events SET session_id = NULL, mcp_credential_id = NULL, mcp_oauth_grant_id = NULL WHERE session_id IS NOT NULL OR mcp_credential_id IS NOT NULL OR mcp_oauth_grant_id IS NOT NULL",
    )?;
    restore_database_section(program, database_url, dump, "post-data")
}

fn restore_database_section(
    program: &Path,
    database_url: &str,
    dump: &Path,
    section: &str,
) -> Result<()> {
    let args = vec![
        // pg_restore requires an explicit destination selector. An empty
        // dbname delegates connection resolution to libpq's cleared
        // PGDATABASE environment without copying the database name into argv.
        OsString::from("--dbname="),
        OsString::from("--exit-on-error"),
        OsString::from("--no-owner"),
        OsString::from("--no-privileges"),
        OsString::from(format!("--section={section}")),
    ];
    run_from_file(
        program,
        &args,
        database_url,
        dump,
        "restore PostgreSQL dump",
    )
}

fn reset_restored_credentials(program: &Path, database_url: &str) -> Result<()> {
    let query = "BEGIN; UPDATE users SET authentication_epoch=authentication_epoch+1; UPDATE project_invitations SET status='revoked', revoked_at=now(), version=version+1, updated_at=now() WHERE status='active'; UPDATE execution_profiles SET status='authentication_required', account_state='logged_out', version=version+1, updated_at=now(); COMMIT;";
    let _ = run_psql(program, database_url, query)?;
    Ok(())
}

fn credential_row_count(program: &Path, database_url: &str) -> Result<u64> {
    let query = "SELECT (SELECT count(*) FROM sessions) + (SELECT count(*) FROM user_password_credentials) + (SELECT count(*) FROM mcp_client_credentials) + (SELECT count(*) FROM mcp_oauth_clients) + (SELECT count(*) FROM mcp_oauth_authorization_codes) + (SELECT count(*) FROM mcp_oauth_grants) + (SELECT count(*) FROM mcp_oauth_access_tokens) + (SELECT count(*) FROM mcp_oauth_refresh_tokens)";
    run_psql(program, database_url, query)?
        .trim()
        .parse()
        .context("parse restored credential-row count")
}

fn validate_archive(program: &Path, archive: &Path) -> Result<()> {
    let args = vec![
        OsString::from("--list"),
        OsString::from("--file"),
        archive.as_os_str().to_owned(),
    ];
    let output = run_output(program, &args, None, "list backup archive")?;
    let listing = String::from_utf8(output.stdout).context("archive listing is not UTF-8")?;
    ensure!(
        listing.lines().count() <= MAX_ARCHIVE_ENTRIES,
        "archive has too many entries"
    );
    for entry in listing.lines().filter(|entry| !entry.is_empty()) {
        ensure!(safe_archive_entry(entry), "archive contains an unsafe path");
    }
    Ok(())
}

fn extract_archive(program: &Path, archive: &Path, destination: &Path) -> Result<()> {
    let args = vec![
        OsString::from("--extract"),
        OsString::from("--file"),
        archive.as_os_str().to_owned(),
        OsString::from("--directory"),
        destination.as_os_str().to_owned(),
        OsString::from("--no-same-owner"),
        OsString::from("--no-same-permissions"),
    ];
    run_status(program, &args, None, "extract backup archive")
}

fn assign_restored_data_owner(program: &Path, target_data_root: &Path) -> Result<()> {
    let args = vec![
        OsString::from("--recursive"),
        OsString::from("--no-dereference"),
        OsString::from(RESTORED_DATA_OWNER),
        target_data_root.as_os_str().to_owned(),
    ];
    run_status(
        program,
        &args,
        None,
        "assign restored application data owner",
    )
}

fn safe_archive_entry(value: &str) -> bool {
    if value == "." || value == "./" {
        return true;
    }
    let trimmed = value.strip_prefix("./").unwrap_or(value);
    !trimmed.is_empty()
        && !Path::new(trimmed).is_absolute()
        && Path::new(trimmed)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn safe_manifest_file(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && Path::new(value).components().count() == 1
}

fn path_contains(parent: &Path, candidate: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

fn staging_path(destination: &Path) -> Result<PathBuf> {
    let parent = destination.parent().context("destination has no parent")?;
    let name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .context("destination name is not UTF-8")?;
    let staging = parent.join(format!(".{name}.partial-{}", std::process::id()));
    ensure!(!staging.exists(), "backup staging path already exists");
    Ok(staging)
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory {} for sync", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

struct CleanupDirectory {
    path: PathBuf,
    committed: bool,
}

impl CleanupDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for CleanupDirectory {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
    };

    use tempfile::TempDir;

    use super::{
        ARCHIVE_FORMAT, ArtifactRecord, BACKUP_FORMAT_VERSION, BackupManifest, EXCLUDED_TABLE_DATA,
        INCLUDED_DATA_ENTRIES, MigrationRecord, PRESERVED_UNIQUE_ENTRIES,
        REBUILDABLE_CACHE_ENTRIES, SourceLayout, create_release_archive, read_database_name_file,
        read_secret_file, restore_database, retention_apply_query, retention_report_query,
        run_from_file, run_to_file, safe_archive_entry, scan_storage_category, validate_archive,
        validate_backup, validate_manifest, validate_new_destination,
    };

    fn fixture_manifest() -> BackupManifest {
        BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            archive_format: ARCHIVE_FORMAT.to_owned(),
            created_at_unix_seconds: 1,
            release_commit: "a".repeat(40),
            postgres_major: 18,
            source_layout: SourceLayout {
                data_layout: "agent-workspace-data-root-v1".to_owned(),
                release_layout: "immutable-release-v1".to_owned(),
                included_data_entries: INCLUDED_DATA_ENTRIES
                    .iter()
                    .map(|entry| (*entry).to_owned())
                    .collect(),
                rebuildable_cache_entries: REBUILDABLE_CACHE_ENTRIES
                    .iter()
                    .map(|entry| (*entry).to_owned())
                    .collect(),
                preserved_unique_entries: PRESERVED_UNIQUE_ENTRIES
                    .iter()
                    .map(|entry| (*entry).to_owned())
                    .collect(),
            },
            excluded_table_data: EXCLUDED_TABLE_DATA
                .iter()
                .map(|table| (*table).to_owned())
                .collect(),
            migrations: vec![MigrationRecord {
                version: 1,
                checksum_sha384: "b".repeat(96),
            }],
            artifacts: vec![
                ArtifactRecord {
                    file: "data.tar".to_owned(),
                    bytes: 0,
                    sha256: "c".repeat(64),
                },
                ArtifactRecord {
                    file: "database.dump".to_owned(),
                    bytes: 0,
                    sha256: "d".repeat(64),
                },
                ArtifactRecord {
                    file: "release.tar".to_owned(),
                    bytes: 0,
                    sha256: "e".repeat(64),
                },
            ],
        }
    }

    #[test]
    fn storage_categories_count_allocated_files_without_following_symlinks() {
        let temporary_parent = std::env::temp_dir()
            .canonicalize()
            .expect("canonical temporary root");
        let directory = TempDir::new_in(temporary_parent).expect("storage fixture");
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).expect("nested directory");
        fs::write(nested.join("artifact"), b"bounded").expect("fixture file");
        let report = scan_storage_category("fixture", directory.path()).expect("scan storage");
        assert_eq!(report.category, "fixture");
        assert_eq!(report.regular_files, 1);
        assert_eq!(report.directories, 2);
        assert!(report.allocated_bytes >= 512);

        symlink(nested.join("artifact"), directory.path().join("link")).expect("fixture symlink");
        let report = scan_storage_category("fixture", directory.path()).expect("skip symlink");
        assert_eq!(report.skipped_symlinks, 1);
        assert_eq!(report.regular_files, 1);
    }

    #[test]
    fn retention_is_dry_run_by_default_and_apply_is_locked() {
        let report = retention_report_query();
        assert!(!report.contains("DELETE FROM"));
        assert!(!report.contains("UPDATE project_invitations"));
        assert!(report.contains("expired_idempotency"));
        assert!(report.contains("completed_temporary_jobs"));

        let apply = retention_apply_query();
        assert!(apply.contains("pg_advisory_xact_lock"));
        assert!(apply.contains("DELETE FROM idempotency_keys"));
        assert!(apply.contains("UPDATE project_invitations"));
        assert!(apply.contains("rotated_to_token_id=NULL"));
        assert!(!apply.contains("DELETE FROM audit_events"));
        assert!(!apply.contains("DELETE FROM messages"));
        assert!(!apply.contains("DELETE FROM agent_runs"));

        let decoded: Vec<super::RetentionCategory> = serde_json::from_str(
            r#"[{"category":"expired_idempotency","eligible_rows":2,"affected_rows":0}]"#,
        )
        .expect("decode PostgreSQL retention summary");
        assert_eq!(decoded[0].eligible_rows, 2);
        assert_eq!(decoded[0].affected_rows, 0);
        let encoded = serde_json::to_string(&decoded).expect("encode public retention summary");
        assert!(encoded.contains("\"eligibleRows\":2"));
        assert!(!encoded.contains("eligible_rows"));
    }

    fn temporary_directory() -> TempDir {
        TempDir::new_in(
            std::env::current_dir()
                .expect("current directory")
                .canonicalize()
                .expect("canonical current directory"),
        )
        .expect("temporary directory")
    }

    #[test]
    fn manifest_requires_exact_credential_exclusions_and_artifacts() {
        let manifest = fixture_manifest();
        assert!(validate_manifest(&manifest).is_ok());

        let mut incomplete = manifest.clone();
        incomplete.excluded_table_data.pop();
        assert!(validate_manifest(&incomplete).is_err());

        let mut misclassified = manifest.clone();
        misclassified
            .source_layout
            .rebuildable_cache_entries
            .push("worktrees".to_owned());
        assert!(validate_manifest(&misclassified).is_err());

        let mut extra_artifact = manifest;
        extra_artifact.artifacts.push(ArtifactRecord {
            file: "unexpected".to_owned(),
            bytes: 0,
            sha256: "f".repeat(64),
        });
        assert!(validate_manifest(&extra_artifact).is_err());
    }

    #[test]
    fn destination_must_be_new_owner_only_and_outside_sources() {
        let temporary = temporary_directory();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("secure temporary directory");
        let source = temporary.path().join("source");
        fs::create_dir(&source).expect("source directory");

        let sibling = temporary.path().join("backup");
        assert_eq!(
            validate_new_destination(&sibling, &[source.as_path()])
                .expect("valid sibling destination"),
            temporary
                .path()
                .canonicalize()
                .expect("canonical temporary directory")
                .join("backup")
        );
        assert!(validate_new_destination(&source.join("backup"), &[source.as_path()]).is_err());
        fs::create_dir(&sibling).expect("existing destination");
        assert!(validate_new_destination(&sibling, &[source.as_path()]).is_err());

        let real_parent = temporary.path().join("real-parent");
        fs::create_dir(&real_parent).expect("real destination parent");
        fs::set_permissions(&real_parent, fs::Permissions::from_mode(0o700))
            .expect("secure real destination parent");
        let linked_parent = temporary.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).expect("linked destination parent");
        assert!(
            validate_new_destination(&linked_parent.join("backup"), &[source.as_path()]).is_err()
        );
    }

    #[test]
    fn secret_file_rejects_permissions_symlinks_and_multiple_values() {
        let temporary = temporary_directory();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("secure temporary directory");
        let secret = temporary.path().join("database-url");
        fs::write(&secret, "postgres://example\n").expect("secret fixture");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600))
            .expect("secure secret fixture");
        assert_eq!(
            read_secret_file(&secret).expect("valid secret").as_str(),
            "postgres://example"
        );

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644))
            .expect("make fixture unsafe");
        assert!(read_secret_file(&secret).is_err());
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600))
            .expect("restore secret mode");
        fs::write(&secret, "first\nsecond\n").expect("multiple values fixture");
        assert!(read_secret_file(&secret).is_err());

        let target = temporary.path().join("target");
        fs::write(&target, "value").expect("symlink target");
        let link = temporary.path().join("link");
        symlink(&target, &link).expect("secret symlink");
        assert!(read_secret_file(&link).is_err());
    }

    #[test]
    fn database_name_file_rejects_urls_and_credentials() {
        let temporary = temporary_directory();
        let target = temporary.path().join("database-name");
        fs::write(&target, "agent_workspace_restore").expect("database name fixture");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("secure database name fixture");
        assert_eq!(
            read_database_name_file(&target)
                .expect("valid database name")
                .as_str(),
            "agent_workspace_restore"
        );

        fs::write(&target, "postgres://user:password@example/database")
            .expect("credential URL fixture");
        assert!(read_database_name_file(&target).is_err());
    }

    #[test]
    fn archive_paths_reject_absolute_and_parent_traversal() {
        assert!(safe_archive_entry("./"));
        assert!(safe_archive_entry("./worktrees/example/file"));
        assert!(safe_archive_entry("repositories/mirror.git/HEAD"));
        assert!(!safe_archive_entry("/etc/passwd"));
        assert!(!safe_archive_entry("../escape"));
        assert!(!safe_archive_entry("worktrees/../../escape"));
    }

    #[test]
    fn release_archive_root_entry_validates() {
        let temporary = temporary_directory();
        let release = temporary.path().join("release");
        fs::create_dir(&release).expect("release fixture");
        fs::write(release.join("artifact"), b"exact release").expect("release artifact");
        let archive = temporary.path().join("release.tar");
        create_release_archive(Path::new("/usr/bin/tar"), &release, &archive)
            .expect("create release archive");
        validate_archive(Path::new("/usr/bin/tar"), &archive).expect("validate release archive");
    }

    #[test]
    fn validation_detects_artifact_tampering() {
        let temporary = temporary_directory();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("secure backup fixture");
        for file in ["data.tar", "database.dump", "release.tar"] {
            fs::write(temporary.path().join(file), []).expect("artifact fixture");
        }
        let mut manifest = fixture_manifest();
        let empty_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        for artifact in &mut manifest.artifacts {
            artifact.sha256 = empty_sha.to_owned();
        }
        fs::write(
            temporary.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
        )
        .expect("manifest fixture");
        assert!(validate_backup(temporary.path()).is_ok());

        fs::write(temporary.path().join("data.tar"), b"tampered").expect("tamper artifact");
        assert!(validate_backup(temporary.path()).is_err());
    }

    #[test]
    fn manifest_fixture_paths_are_plain_filenames() {
        for artifact in fixture_manifest().artifacts {
            assert_eq!(
                Path::new(&artifact.file)
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some(artifact.file.as_str())
            );
        }
    }

    #[test]
    fn command_streaming_keeps_host_paths_outside_the_child() {
        let temporary = temporary_directory();
        let producer = temporary.path().join("producer");
        fs::write(&producer, "#!/bin/sh\nprintf 'database-dump'\n").expect("producer fixture");
        fs::set_permissions(&producer, fs::Permissions::from_mode(0o700))
            .expect("producer executable");
        let dump = temporary.path().join("database.dump");
        run_to_file(&producer, &[], "fixture", &dump, "produce fixture")
            .expect("stream command output");
        assert_eq!(
            fs::read(&dump).expect("read streamed output"),
            b"database-dump"
        );

        let consumer = temporary.path().join("consumer");
        fs::write(
            &consumer,
            "#!/bin/sh\nvalue=$(/bin/cat)\n[ \"$value\" = 'database-dump' ]\n",
        )
        .expect("consumer fixture");
        fs::set_permissions(&consumer, fs::Permissions::from_mode(0o700))
            .expect("consumer executable");
        run_from_file(
            &consumer,
            &[OsString::from("--ignored-by-fixture")],
            "fixture",
            &dump,
            "consume fixture",
        )
        .expect("stream command input");
    }

    #[test]
    fn restore_selects_the_environment_database_and_streams_the_dump() {
        let temporary = temporary_directory();
        let order = temporary.path().join("restore-order");
        let consumer = temporary.path().join("pg-restore-fixture");
        fs::write(
            &consumer,
            format!(
                "#!/bin/sh\n[ \"$PGDATABASE\" = 'target_database' ]\n[ \"$1\" = '--dbname=' ]\n[ \"$2\" = '--exit-on-error' ]\n[ \"$3\" = '--no-owner' ]\n[ \"$4\" = '--no-privileges' ]\ncase \"$5\" in --section=*) ;; *) exit 1 ;; esac\n[ \"$(/bin/cat)\" = 'database-dump' ]\nprintf '%s\\n' \"${{5#--section=}}\" >> '{}'\n",
                order.display()
            ),
        )
        .expect("restore consumer fixture");
        fs::set_permissions(&consumer, fs::Permissions::from_mode(0o700))
            .expect("restore consumer executable");
        let dump = temporary.path().join("database.dump");
        fs::write(&dump, b"database-dump").expect("restore dump fixture");
        let psql = temporary.path().join("psql-fixture");
        fs::write(
            &psql,
            format!(
                "#!/bin/sh\n[ \"$PGDATABASE\" = 'target_database' ]\n[ \"$5\" = '--command' ]\n[ \"$6\" = 'UPDATE audit_events SET session_id = NULL, mcp_credential_id = NULL, mcp_oauth_grant_id = NULL WHERE session_id IS NOT NULL OR mcp_credential_id IS NOT NULL OR mcp_oauth_grant_id IS NOT NULL' ]\nprintf 'sanitize\\n' >> '{}'\n",
                order.display()
            ),
        )
        .expect("psql fixture");
        fs::set_permissions(&psql, fs::Permissions::from_mode(0o700))
            .expect("psql fixture executable");

        restore_database(&consumer, &psql, "target_database", &dump)
            .expect("restore uses the environment-selected database");
        assert_eq!(
            fs::read_to_string(order).expect("restore order"),
            "pre-data\ndata\nsanitize\npost-data\n"
        );
    }

    #[test]
    fn red_hat_restore_wrapper_forwards_standard_input() {
        let wrapper = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/redhat/agent-workspace-pg-restore");
        let source = fs::read_to_string(wrapper).expect("read Red Hat restore wrapper");
        assert!(source.contains("/usr/bin/podman exec\n  --interactive\n"));
    }
}
