use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt;
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::{
    error::AppError,
    profile_runtime::{prepare_private_profile_directory, resolve_path_within_data_root},
};

const MAX_GIT_ERROR_BYTES: usize = 8 * 1024;
const MAX_GIT_OUTPUT_BYTES: usize = 1024 * 1024;
const GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(90);
#[cfg(not(test))]
const MAX_CONCURRENT_GIT_PROCESSES: usize = 4;
#[cfg(test)]
const MAX_CONCURRENT_GIT_PROCESSES: usize = 64;
static ACTIVE_GIT_PROCESSES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
pub struct RegisteredRepository {
    pub id: Uuid,
    pub clone_url: String,
    pub default_branch: String,
    pub mirror_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRepository {
    pub base_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeInspection {
    pub dirty: bool,
    pub branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishInspection {
    pub branch: String,
    pub head_commit: String,
}

pub struct PublishPush<'a> {
    pub data_root: &'a Path,
    pub mirror: &'a Path,
    pub worktree: &'a Path,
    pub expected_branch: &'a str,
    pub base_commit: &'a str,
    pub intended_commit: &'a str,
    pub remote_url: &'a str,
    pub token: &'a SecretString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryPreparationMode {
    RefreshOrClone,
    VerifyImportedMirror,
}

pub fn expected_repository_mirror_path(
    data_root: &Path,
    repository_id: Uuid,
) -> Result<PathBuf, AppError> {
    let data_root = absolute_runtime_path(data_root)?;
    let repository_root = prepare_private_profile_directory(
        &data_root,
        &data_root.join("repositories"),
        &repository_id.to_string(),
    )?;
    Ok(repository_root.join("mirror.git"))
}

pub fn managed_worktree_path(
    data_root: &Path,
    repository_id: Uuid,
    workspace_id: Uuid,
) -> Result<PathBuf, AppError> {
    let data_root = absolute_runtime_path(data_root)?;
    let repository_root = prepare_private_profile_directory(
        &data_root,
        &data_root.join("worktrees"),
        &repository_id.to_string(),
    )?;
    let candidate = repository_root.join(workspace_id.to_string());
    let canonical_data_root = data_root
        .canonicalize()
        .map_err(|_| AppError::Internal("canonicalize worktree data root"))?;
    resolve_path_within_data_root(&canonical_data_root, &candidate)
}

pub fn prepare_repository(
    data_root: &Path,
    repository: &RegisteredRepository,
    mode: RepositoryPreparationMode,
) -> Result<PreparedRepository, AppError> {
    let data_root = absolute_runtime_path(data_root)?;
    let repository_root = prepare_private_profile_directory(
        &data_root,
        &data_root.join("repositories"),
        &repository.id.to_string(),
    )?;
    let expected_mirror = repository_root.join("mirror.git");
    if repository.mirror_path != expected_mirror {
        return Err(AppError::Conflict(
            "repository",
            "stored mirror path does not match the runtime root",
        ));
    }
    let _lock = repository_lock(&repository_root)?;
    if repository.mirror_path.exists() {
        validate_existing_mirror(&repository.mirror_path, &repository.clone_url)?;
    } else if mode == RepositoryPreparationMode::VerifyImportedMirror {
        return Err(AppError::Conflict(
            "repository",
            "imported mirror is not available",
        ));
    } else {
        run_git([
            OsString::from("clone"),
            OsString::from("--mirror"),
            OsString::from("--no-local"),
            OsString::from("--config"),
            OsString::from("core.hooksPath=/dev/null"),
            OsString::from(&repository.clone_url),
            repository.mirror_path.as_os_str().to_owned(),
        ])?;
        validate_existing_mirror(&repository.mirror_path, &repository.clone_url)?;
    }
    if mode == RepositoryPreparationMode::RefreshOrClone {
        run_git([
            OsString::from("--git-dir"),
            repository.mirror_path.as_os_str().to_owned(),
            OsString::from("remote"),
            OsString::from("update"),
            OsString::from("--prune"),
            OsString::from("origin"),
        ])?;
    }
    let base_commit = resolve_base_commit(&repository.mirror_path, &repository.default_branch)?;
    Ok(PreparedRepository { base_commit })
}

pub fn inspect_prepared_repository(
    data_root: &Path,
    repository: &RegisteredRepository,
) -> Result<PreparedRepository, AppError> {
    let expected_mirror = expected_repository_mirror_path(data_root, repository.id)?;
    if repository.mirror_path != expected_mirror {
        return Err(AppError::Conflict(
            "repository",
            "stored mirror path does not match the runtime root",
        ));
    }
    validate_existing_mirror(&repository.mirror_path, &repository.clone_url)?;
    let base_commit = resolve_base_commit(&repository.mirror_path, &repository.default_branch)?;
    Ok(PreparedRepository { base_commit })
}

pub fn adopt_legacy_mirror(
    data_root: &Path,
    repository: &RegisteredRepository,
    legacy_mirror: &Path,
) -> Result<PreparedRepository, AppError> {
    let data_root = absolute_runtime_path(data_root)?;
    let legacy_mirror = resolve_path_within_data_root(&data_root, legacy_mirror)?;
    validate_existing_mirror(&legacy_mirror, &repository.clone_url)?;
    let repository_root = prepare_private_profile_directory(
        &data_root,
        &data_root.join("repositories"),
        &repository.id.to_string(),
    )?;
    let expected_mirror = repository_root.join("mirror.git");
    if repository.mirror_path != expected_mirror {
        return Err(AppError::Conflict(
            "repository",
            "canonical mirror path does not match the runtime root",
        ));
    }
    let _lock = repository_lock(&repository_root)?;
    if !expected_mirror.exists() {
        run_git([
            OsString::from("clone"),
            OsString::from("--mirror"),
            OsString::from("--no-local"),
            legacy_mirror.as_os_str().to_owned(),
            expected_mirror.as_os_str().to_owned(),
        ])?;
        run_git([
            OsString::from("--git-dir"),
            expected_mirror.as_os_str().to_owned(),
            OsString::from("remote"),
            OsString::from("set-url"),
            OsString::from("origin"),
            OsString::from(&repository.clone_url),
        ])?;
    }
    validate_existing_mirror(&expected_mirror, &repository.clone_url)?;
    let base_commit = resolve_base_commit(&expected_mirror, &repository.default_branch)?;
    Ok(PreparedRepository { base_commit })
}

pub fn unique_run_branch(project_key: &str) -> Result<String, AppError> {
    let slug = project_key.to_ascii_lowercase();
    if slug.is_empty()
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(AppError::Invalid("unsafe project key"));
    }
    Ok(format!("codex/run-{slug}-{}", Uuid::now_v7()))
}

pub fn resolve_base_commit(mirror: &Path, branch: &str) -> Result<String, AppError> {
    validate_mirror(mirror)?;
    validate_ref(branch)?;
    let reference = format!("refs/heads/{branch}^{{commit}}");
    let output = run_git([
        "--git-dir",
        &mirror.to_string_lossy(),
        "rev-parse",
        "--verify",
        &reference,
    ])?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Conflict(
            "repository",
            "base commit could not be resolved",
        ));
    }
    Ok(commit)
}

pub fn create_worktree(
    mirror: &Path,
    worktree: &Path,
    branch: &str,
    base_commit: &str,
) -> Result<(), AppError> {
    validate_mirror(mirror)?;
    validate_branch(branch)?;
    if !worktree.is_absolute() || worktree.exists() {
        return Err(AppError::Invalid("unsafe or existing worktree path"));
    }
    let parent = worktree
        .parent()
        .ok_or(AppError::Invalid("worktree has no parent"))?;
    fs::create_dir_all(parent).map_err(|_| AppError::Internal("create worktree parent"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| AppError::Internal("canonicalize worktree parent"))?;
    if worktree.parent() != Some(canonical_parent.as_path()) {
        return Err(AppError::Invalid("worktree parent is not canonical"));
    }

    let repository_root = mirror
        .parent()
        .ok_or(AppError::Invalid("mirror has no repository root"))?;
    let _lock = repository_lock(repository_root)?;
    run_git([
        "--git-dir",
        &mirror.to_string_lossy(),
        "worktree",
        "add",
        "-b",
        branch,
        &worktree.to_string_lossy(),
        base_commit,
    ])?;
    Ok(())
}

pub fn inspect_worktree(mirror: &Path, worktree: &Path) -> Result<WorktreeInspection, AppError> {
    validate_mirror(mirror)?;
    if !worktree.is_absolute() || !worktree.is_dir() {
        return Err(AppError::Conflict("git_worktree", "worktree is not ready"));
    }
    let common_dir = run_git([
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("rev-parse"),
        OsString::from("--git-common-dir"),
    ])?;
    let common_dir = String::from_utf8(common_dir.stdout)
        .map_err(|_| AppError::Conflict("git_worktree", "Git metadata is invalid"))?;
    let common_dir = PathBuf::from(common_dir.trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        worktree.join(common_dir)
    }
    .canonicalize()
    .map_err(|_| AppError::Conflict("git_worktree", "Git metadata is missing"))?;
    let expected = mirror
        .canonicalize()
        .map_err(|_| AppError::Conflict("repository", "mirror is not ready"))?;
    if common_dir != expected {
        return Err(AppError::Conflict(
            "git_worktree",
            "worktree belongs to another repository",
        ));
    }
    let branch = run_git([
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("symbolic-ref"),
        OsString::from("--short"),
        OsString::from("HEAD"),
    ])?;
    let branch = String::from_utf8(branch.stdout)
        .map_err(|_| AppError::Conflict("git_worktree", "Git branch is invalid"))?
        .trim()
        .to_owned();
    validate_branch(&branch)?;
    Ok(WorktreeInspection {
        dirty: worktree_is_dirty(worktree)?,
        branch,
    })
}

pub fn inspect_publishable_worktree(
    mirror: &Path,
    worktree: &Path,
    expected_branch: &str,
    base_commit: &str,
) -> Result<PublishInspection, AppError> {
    validate_branch(expected_branch)?;
    validate_commit(base_commit)?;
    let repository_root = mirror
        .parent()
        .ok_or(AppError::Invalid("mirror has no repository root"))?;
    let _lock = repository_lock(repository_root)?;
    let inspection = inspect_worktree(mirror, worktree)?;
    if inspection.dirty {
        return Err(AppError::Conflict(
            "git_worktree",
            "dirty worktree is retained; commit intended changes through the Thread before publishing",
        ));
    }
    if inspection.branch != expected_branch {
        return Err(AppError::Conflict(
            "git_worktree",
            "worktree branch does not match its durable workspace",
        ));
    }
    let head = run_git([
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("rev-parse"),
        OsString::from("--verify"),
        OsString::from("HEAD^{commit}"),
    ])?;
    let head_commit = String::from_utf8(head.stdout)
        .map_err(|_| AppError::Conflict("git_worktree", "HEAD commit is invalid"))?
        .trim()
        .to_owned();
    validate_commit(&head_commit)?;
    if head_commit == base_commit {
        return Err(AppError::Conflict(
            "git_worktree",
            "workspace has no commit ahead of its recorded base",
        ));
    }
    run_git([
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("merge-base"),
        OsString::from("--is-ancestor"),
        OsString::from(base_commit),
        OsString::from(&head_commit),
    ])
    .map_err(|_| {
        AppError::Conflict(
            "git_worktree",
            "workspace HEAD is not descended from its recorded base",
        )
    })?;
    Ok(PublishInspection {
        branch: inspection.branch,
        head_commit,
    })
}

pub fn push_publishable_commit(request: PublishPush<'_>) -> Result<(), AppError> {
    let inspection = inspect_publishable_worktree(
        request.mirror,
        request.worktree,
        request.expected_branch,
        request.base_commit,
    )?;
    if inspection.head_commit != request.intended_commit {
        return Err(AppError::Conflict(
            "git_worktree",
            "workspace HEAD changed after publication was queued",
        ));
    }
    if !request.remote_url.starts_with("https://github.com/")
        || !request.remote_url.ends_with(".git")
        || request.remote_url.contains('@')
        || request
            .remote_url
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(AppError::Invalid("unsafe GitHub remote"));
    }
    let askpass = TemporaryAskpass::create(request.data_root)?;
    let refspec = format!(
        "{}:refs/heads/{}",
        request.intended_commit, request.expected_branch
    );
    let _process_permit = GitProcessPermit::try_acquire()?;
    let mut command = Command::new("/usr/bin/git");
    command
        .args([
            OsStr::new("-C"),
            request.worktree.as_os_str(),
            OsStr::new("push"),
            OsStr::new("--porcelain"),
            OsStr::new(request.remote_url),
            OsStr::new(&refspec),
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "4")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        .env("GIT_CONFIG_KEY_1", "credential.helper")
        .env("GIT_CONFIG_VALUE_1", "")
        .env("GIT_CONFIG_KEY_2", "credential.useHttpPath")
        .env("GIT_CONFIG_VALUE_2", "true")
        .env("GIT_CONFIG_KEY_3", "http.followRedirects")
        .env("GIT_CONFIG_VALUE_3", "false")
        .env("GIT_ASKPASS", askpass.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "/bin/false")
        .env(
            "AGENT_WORKSPACE_GIT_PASSWORD",
            request.token.expose_secret(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| AppError::Internal("start credentialed Git push"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(AppError::Internal("capture Git push stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(AppError::Internal("capture Git push stderr"))?;
    let stdout_reader = thread::spawn(move || bounded_read(stdout, MAX_GIT_OUTPUT_BYTES));
    let stderr_reader = thread::spawn(move || bounded_read(stderr, MAX_GIT_ERROR_BYTES));
    let deadline = Instant::now() + GIT_NETWORK_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| AppError::Internal("wait for credentialed Git push"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Conflict("repository", "Git push timed out"));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| AppError::Internal("join Git push stdout reader"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| AppError::Internal("join Git push stderr reader"))??;
    if status.success() {
        return Ok(());
    }
    tracing::warn!(
        stdout_bytes = stdout.len(),
        stderr_bytes = stderr.len(),
        "bounded credentialed Git push failed"
    );
    Err(AppError::Conflict("repository", "Git push failed"))
}

pub fn remove_clean_worktree(mirror: &Path, worktree: &Path) -> Result<(), AppError> {
    let inspection = inspect_worktree(mirror, worktree)?;
    if inspection.dirty {
        return Err(AppError::Conflict(
            "git_worktree",
            "dirty worktree is retained",
        ));
    }
    let repository_root = mirror
        .parent()
        .ok_or(AppError::Invalid("mirror has no repository root"))?;
    let _lock = repository_lock(repository_root)?;
    run_git([
        OsString::from("--git-dir"),
        mirror.as_os_str().to_owned(),
        OsString::from("worktree"),
        OsString::from("remove"),
        worktree.as_os_str().to_owned(),
    ])?;
    prune_worktrees_locked(mirror)
}

pub fn remove_published_worktree(
    mirror: &Path,
    worktree: &Path,
    expected_branch: &str,
    published_commit: &str,
) -> Result<(), AppError> {
    validate_branch(expected_branch)?;
    validate_commit(published_commit)?;
    validate_mirror(mirror)?;
    let repository_root = mirror
        .parent()
        .ok_or(AppError::Invalid("mirror has no repository root"))?;
    let _lock = repository_lock(repository_root)?;
    let inspection = inspect_worktree(mirror, worktree)?;
    if inspection.dirty {
        return Err(AppError::Conflict(
            "git_worktree",
            "dirty worktree is retained",
        ));
    }
    if inspection.branch != expected_branch {
        return Err(AppError::Conflict(
            "git_worktree",
            "worktree branch does not match its durable workspace",
        ));
    }
    let head = run_git([
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("rev-parse"),
        OsString::from("--verify"),
        OsString::from("HEAD^{commit}"),
    ])?;
    let head = String::from_utf8(head.stdout)
        .map_err(|_| AppError::Conflict("git_worktree", "HEAD commit is invalid"))?;
    if head.trim() != published_commit {
        return Err(AppError::Conflict(
            "git_worktree",
            "unpushed worktree is retained",
        ));
    }
    run_git([
        OsString::from("--git-dir"),
        mirror.as_os_str().to_owned(),
        OsString::from("worktree"),
        OsString::from("remove"),
        worktree.as_os_str().to_owned(),
    ])?;
    prune_worktrees_locked(mirror)
}

pub fn repair_and_prune_worktrees(mirror: &Path, worktree: Option<&Path>) -> Result<(), AppError> {
    validate_mirror(mirror)?;
    let repository_root = mirror
        .parent()
        .ok_or(AppError::Invalid("mirror has no repository root"))?;
    let _lock = repository_lock(repository_root)?;
    if let Some(worktree) = worktree {
        if !worktree.is_absolute() {
            return Err(AppError::Invalid("unsafe worktree path"));
        }
        run_git([
            OsString::from("--git-dir"),
            mirror.as_os_str().to_owned(),
            OsString::from("worktree"),
            OsString::from("repair"),
            worktree.as_os_str().to_owned(),
        ])?;
    }
    prune_worktrees_locked(mirror)
}

pub fn absolute_runtime_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|_| AppError::Internal("resolve runtime path"))
}

pub fn worktree_is_dirty(worktree: &Path) -> Result<bool, AppError> {
    if !worktree.is_absolute() || !worktree.is_dir() {
        return Err(AppError::Conflict("git_worktree", "worktree is not ready"));
    }
    let output = run_git([
        "-C",
        &worktree.to_string_lossy(),
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
    ])?;
    Ok(!output.stdout.is_empty())
}

fn validate_mirror(mirror: &Path) -> Result<(), AppError> {
    if !mirror.is_absolute() || !mirror.is_dir() {
        return Err(AppError::Conflict("repository", "mirror is not ready"));
    }
    Ok(())
}

fn validate_existing_mirror(mirror: &Path, expected_origin: &str) -> Result<(), AppError> {
    validate_mirror(mirror)?;
    let origin = run_git([
        OsString::from("--git-dir"),
        mirror.as_os_str().to_owned(),
        OsString::from("remote"),
        OsString::from("get-url"),
        OsString::from("origin"),
    ])?;
    let origin = String::from_utf8(origin.stdout)
        .map_err(|_| AppError::Conflict("repository", "origin is invalid"))?;
    if origin.trim() != expected_origin {
        return Err(AppError::Conflict(
            "repository",
            "mirror origin does not match registration",
        ));
    }
    Ok(())
}

fn validate_ref(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 255
        || value == "@"
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        || value.split('/').any(|component| {
            component.is_empty() || component.starts_with('.') || component.ends_with(".lock")
        })
    {
        return Err(AppError::Invalid("unsafe Git reference"));
    }
    Ok(())
}

pub fn validate_registered_ref(value: &str) -> Result<(), AppError> {
    validate_ref(value)
}

fn validate_branch(value: &str) -> Result<(), AppError> {
    if !value.starts_with("codex/run-") {
        return Err(AppError::Invalid("unsafe Git branch"));
    }
    validate_ref(value)
}

fn validate_commit(value: &str) -> Result<(), AppError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Invalid("unsafe Git commit"));
    }
    Ok(())
}

fn repository_lock(repository_root: &Path) -> Result<RepositoryLock, AppError> {
    if !repository_root.is_absolute() || !repository_root.is_dir() {
        return Err(AppError::Invalid("unsafe repository root"));
    }
    let path = repository_root.join("metadata.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| AppError::Internal("open repository metadata lock"))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| AppError::Internal("protect repository metadata lock"))?;
    file.lock_exclusive()
        .map_err(|_| AppError::Internal("lock repository metadata"))?;
    Ok(RepositoryLock { file })
}

fn prune_worktrees_locked(mirror: &Path) -> Result<(), AppError> {
    run_git([
        OsString::from("--git-dir"),
        mirror.as_os_str().to_owned(),
        OsString::from("worktree"),
        OsString::from("prune"),
        OsString::from("--expire=now"),
    ])?;
    Ok(())
}

fn run_git<I, S>(arguments: I) -> Result<Output, AppError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let _process_permit = GitProcessPermit::try_acquire()?;
    let mut command = Command::new("/usr/bin/git");
    command
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        .env("GIT_CONFIG_KEY_1", "credential.helper")
        .env("GIT_CONFIG_VALUE_1", "")
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_SSH_COMMAND", "/bin/false")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| AppError::Internal("start Git"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(AppError::Internal("capture Git stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(AppError::Internal("capture Git stderr"))?;
    let stdout_reader = thread::spawn(move || bounded_read(stdout, MAX_GIT_OUTPUT_BYTES));
    let stderr_reader = thread::spawn(move || bounded_read(stderr, MAX_GIT_ERROR_BYTES));
    let status = child
        .wait()
        .map_err(|_| AppError::Internal("wait for Git"))?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| AppError::Internal("join Git stdout reader"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| AppError::Internal("join Git stderr reader"))??;
    let output = Output {
        status,
        stdout,
        stderr,
    };
    if output.status.success() {
        return Ok(output);
    }
    let stderr_bytes = output.stderr.len().min(MAX_GIT_ERROR_BYTES);
    tracing::warn!(
        stderr_bytes,
        truncated = output.stderr.len() > MAX_GIT_ERROR_BYTES,
        "bounded Git command failed"
    );
    Err(AppError::Conflict("repository", "Git operation failed"))
}

struct GitProcessPermit;

impl GitProcessPermit {
    fn try_acquire() -> Result<Self, AppError> {
        reserve_process_slot(&ACTIVE_GIT_PROCESSES, MAX_CONCURRENT_GIT_PROCESSES)
            .then_some(())
            .ok_or_else(|| AppError::rate_limited(1))?;
        Ok(Self)
    }
}

fn reserve_process_slot(counter: &AtomicUsize, limit: usize) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < limit).then_some(active + 1)
        })
        .is_ok()
}

impl Drop for GitProcessPermit {
    fn drop(&mut self) {
        let previous = ACTIVE_GIT_PROCESSES.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

fn bounded_read(mut input: impl Read, maximum: usize) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::with_capacity(maximum.min(16 * 1024));
    input
        .by_ref()
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::Conflict("repository", "Git output could not be read"))?;
    if bytes.len() > maximum {
        return Err(AppError::Conflict(
            "repository",
            "Git output exceeded its bound",
        ));
    }
    Ok(bytes)
}

struct TemporaryAskpass {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryAskpass {
    fn create(data_root: &Path) -> Result<Self, AppError> {
        let key = Uuid::now_v7().simple().to_string();
        let directory =
            prepare_private_profile_directory(data_root, &data_root.join("github/askpass"), &key)?;
        let path = directory.join("git-askpass");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o700)
            .open(&path)
            .map_err(|_| AppError::Internal("create Git askpass helper"))?;
        file.write_all(
            b"#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' 'x-access-token' ;;\n  *Password*) printf '%s\\n' \"$AGENT_WORKSPACE_GIT_PASSWORD\" ;;\n  *) exit 1 ;;\nesac\n",
        )
        .map_err(|_| AppError::Internal("write Git askpass helper"))?;
        file.sync_all()
            .map_err(|_| AppError::Internal("sync Git askpass helper"))?;
        file.set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| AppError::Internal("protect Git askpass helper"))?;
        Ok(Self { directory, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryAskpass {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}

struct RepositoryLock {
    file: fs::File,
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RegisteredRepository, RepositoryPreparationMode, TemporaryAskpass, create_worktree,
        expected_repository_mirror_path, inspect_publishable_worktree, inspect_worktree,
        managed_worktree_path, prepare_repository, remove_clean_worktree,
        remove_published_worktree, repair_and_prune_worktrees, reserve_process_slot,
        unique_run_branch, validate_branch, validate_ref, worktree_is_dirty,
    };
    use std::{
        fs, os::unix::fs::PermissionsExt, path::Path, process::Command, sync::atomic::AtomicUsize,
        thread,
    };
    use uuid::Uuid;

    #[test]
    fn rejects_unsafe_refs_and_branches() {
        for value in [
            "-main",
            "main..other",
            "main@{1}",
            "white space",
            r"bad\ref",
            "main~1",
            "main:other",
            "refs/.hidden",
            "main.lock",
        ] {
            assert!(validate_ref(value).is_err(), "{value}");
        }
        assert!(validate_branch("main").is_err());
        assert!(validate_branch("codex/run-aw-safe").is_ok());
    }

    #[test]
    fn git_process_slots_reject_one_over_the_limit() {
        let counter = AtomicUsize::new(0);
        assert!(reserve_process_slot(&counter, 2));
        assert!(reserve_process_slot(&counter, 2));
        assert!(!reserve_process_slot(&counter, 2));
    }

    #[test]
    fn creates_collision_safe_branch_names() {
        let first = unique_run_branch("AW").expect("branch");
        let second = unique_run_branch("AW").expect("branch");
        assert!(first.starts_with("codex/run-aw-"));
        assert_ne!(first, second);
    }

    #[test]
    fn rejects_missing_worktrees_during_inspection() {
        assert!(worktree_is_dirty(Path::new("/definitely/missing/worktree")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn askpass_helper_is_owner_only_contains_no_token_and_is_removed() {
        let temporary = tempfile::tempdir().expect("temporary askpass runtime root");
        let data_root = temporary.path().join("data");
        let helper = TemporaryAskpass::create(&data_root).expect("create askpass helper");
        let helper_path = helper.path().to_path_buf();
        let directory = helper.directory.clone();
        let contents = fs::read_to_string(&helper_path).expect("read askpass helper");
        assert!(contents.contains("AGENT_WORKSPACE_GIT_PASSWORD"));
        assert!(!contents.contains("opaque-test-token"));
        assert_eq!(
            fs::metadata(&helper_path)
                .expect("askpass helper metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        drop(helper);
        assert!(!helper_path.exists());
        assert!(!directory.exists());
    }

    #[cfg(unix)]
    #[test]
    fn prepares_canonical_mirror_and_preserves_dirty_parallel_worktrees() {
        let temporary = tempfile::tempdir().expect("temporary Git runtime root");
        let data_root = temporary.path().join("data");
        let source = temporary.path().join("source");
        fs::create_dir(&source).expect("create source repository");
        git(&source, ["init", "-b", "main"]);
        fs::write(source.join("known.txt"), "known\n").expect("write known file");
        git(&source, ["add", "known.txt"]);
        git(
            &source,
            [
                "-c",
                "user.name=Agent Workspace",
                "-c",
                "user.email=agent-workspace@example.invalid",
                "commit",
                "-m",
                "baseline",
            ],
        );

        let repository_id = Uuid::now_v7();
        let mirror = expected_repository_mirror_path(&data_root, repository_id)
            .expect("expected mirror path");
        let repository = RegisteredRepository {
            id: repository_id,
            clone_url: source.to_string_lossy().into_owned(),
            default_branch: "main".to_owned(),
            mirror_path: mirror.clone(),
        };
        let first_preparation = prepare_repository(
            &data_root,
            &repository,
            RepositoryPreparationMode::RefreshOrClone,
        )
        .expect("prepare canonical mirror");
        let second_preparation = prepare_repository(
            &data_root,
            &repository,
            RepositoryPreparationMode::RefreshOrClone,
        )
        .expect("reuse canonical mirror");
        assert_eq!(first_preparation, second_preparation);
        assert_eq!(
            fs::metadata(mirror.parent().expect("repository root"))
                .expect("repository root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let workspaces = [Uuid::now_v7(), Uuid::now_v7()];
        let mut workers = Vec::new();
        for workspace_id in workspaces {
            let data_root = data_root.clone();
            let mirror = mirror.clone();
            let base_commit = first_preparation.base_commit.clone();
            workers.push(thread::spawn(move || {
                let worktree = managed_worktree_path(&data_root, repository_id, workspace_id)
                    .expect("managed worktree path");
                let branch = unique_run_branch("aw").expect("unique branch");
                create_worktree(&mirror, &worktree, &branch, &base_commit)
                    .expect("create linked worktree");
                (worktree, branch)
            }));
        }
        let created = workers
            .into_iter()
            .map(|worker| worker.join().expect("join worktree creator"))
            .collect::<Vec<_>>();
        assert_ne!(created[0].0, created[1].0);
        assert_ne!(created[0].1, created[1].1);
        for (worktree, branch) in &created {
            let inspection = inspect_worktree(&mirror, worktree).expect("inspect worktree");
            assert!(!inspection.dirty);
            assert_eq!(&inspection.branch, branch);
        }

        let no_commit = inspect_publishable_worktree(
            &mirror,
            &created[1].0,
            &created[1].1,
            &first_preparation.base_commit,
        );
        assert!(no_commit.is_err());
        fs::write(created[1].0.join("published.txt"), "publish\n").expect("write publish fixture");
        git(&created[1].0, ["add", "published.txt"]);
        git(
            &created[1].0,
            [
                "-c",
                "user.name=Agent Workspace",
                "-c",
                "user.email=agent-workspace@example.invalid",
                "commit",
                "-m",
                "publish fixture",
            ],
        );
        let publishable = inspect_publishable_worktree(
            &mirror,
            &created[1].0,
            &created[1].1,
            &first_preparation.base_commit,
        )
        .expect("inspect publishable worktree");
        assert_eq!(publishable.branch, created[1].1);
        assert_ne!(publishable.head_commit, first_preparation.base_commit);

        fs::write(created[0].0.join("dirty.txt"), "retain\n").expect("dirty worktree");
        assert!(remove_clean_worktree(&mirror, &created[0].0).is_err());
        assert!(created[0].0.exists(), "dirty worktree must be preserved");
        fs::remove_file(created[0].0.join("dirty.txt")).expect("clean temporary worktree");
        remove_clean_worktree(&mirror, &created[0].0).expect("remove clean worktree");
        assert!(!created[0].0.exists());
        repair_and_prune_worktrees(&mirror, Some(&created[1].0)).expect("repair and prune");
        assert!(
            remove_published_worktree(
                &mirror,
                &created[1].0,
                &created[1].1,
                &first_preparation.base_commit,
            )
            .is_err(),
            "a clean but unpushed commit must be retained"
        );
        assert!(created[1].0.exists());
        remove_published_worktree(
            &mirror,
            &created[1].0,
            &created[1].1,
            &publishable.head_commit,
        )
        .expect("remove verified published worktree");
        assert!(!created[1].0.exists());
    }

    #[cfg(unix)]
    #[test]
    fn verifies_an_imported_private_mirror_without_network_credentials() {
        let temporary = tempfile::tempdir().expect("temporary Git runtime root");
        let data_root = temporary.path().join("data");
        let source = temporary.path().join("source");
        fs::create_dir(&source).expect("create source repository");
        git(&source, ["init", "-b", "main"]);
        fs::write(source.join("known.txt"), "known\n").expect("write known file");
        git(&source, ["add", "known.txt"]);
        git(
            &source,
            [
                "-c",
                "user.name=Agent Workspace",
                "-c",
                "user.email=agent-workspace@example.invalid",
                "commit",
                "-m",
                "baseline",
            ],
        );

        let repository_id = Uuid::now_v7();
        let mirror = expected_repository_mirror_path(&data_root, repository_id)
            .expect("expected mirror path");
        git(
            temporary.path(),
            [
                "clone",
                "--mirror",
                "--no-local",
                source.to_str().expect("source path"),
                mirror.to_str().expect("mirror path"),
            ],
        );
        let clone_url = "https://github.com/Mischa-dev/private-fixture.git";
        git(
            temporary.path(),
            [
                "--git-dir",
                mirror.to_str().expect("mirror path"),
                "remote",
                "set-url",
                "origin",
                clone_url,
            ],
        );
        let repository = RegisteredRepository {
            id: repository_id,
            clone_url: clone_url.to_owned(),
            default_branch: "main".to_owned(),
            mirror_path: mirror,
        };

        let prepared = prepare_repository(
            &data_root,
            &repository,
            RepositoryPreparationMode::VerifyImportedMirror,
        )
        .expect("verify imported mirror without fetching");
        assert_eq!(prepared.base_commit.len(), 40);

        let missing_id = Uuid::now_v7();
        let missing = RegisteredRepository {
            id: missing_id,
            clone_url: clone_url.to_owned(),
            default_branch: "main".to_owned(),
            mirror_path: expected_repository_mirror_path(&data_root, missing_id)
                .expect("missing mirror path"),
        };
        assert!(
            prepare_repository(
                &data_root,
                &missing,
                RepositoryPreparationMode::VerifyImportedMirror,
            )
            .is_err(),
            "verification mode must not clone a missing private mirror"
        );
    }

    #[cfg(unix)]
    fn git<const N: usize>(repository: &Path, arguments: [&str; N]) {
        let output = Command::new("/usr/bin/git")
            .current_dir(repository)
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/dev/null")
            .output()
            .expect("run fixture Git command");
        assert!(output.status.success());
    }
}
