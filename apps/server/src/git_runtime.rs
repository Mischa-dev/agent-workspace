use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use uuid::Uuid;

use crate::error::AppError;

const MAX_GIT_ERROR_BYTES: usize = 8 * 1024;

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

fn validate_ref(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
    {
        return Err(AppError::Invalid("unsafe Git reference"));
    }
    Ok(())
}

fn validate_branch(value: &str) -> Result<(), AppError> {
    if !value.starts_with("codex/run-") {
        return Err(AppError::Invalid("unsafe Git branch"));
    }
    validate_ref(value)
}

fn run_git<const N: usize>(arguments: [&str; N]) -> Result<std::process::Output, AppError> {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|_| AppError::Internal("start Git"))?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = &output.stderr[..output.stderr.len().min(MAX_GIT_ERROR_BYTES)];
    tracing::warn!(
        stderr = %String::from_utf8_lossy(stderr),
        "bounded Git command failed"
    );
    Err(AppError::Conflict("repository", "Git operation failed"))
}

#[cfg(test)]
mod tests {
    use super::{unique_run_branch, validate_branch, validate_ref, worktree_is_dirty};
    use std::path::Path;

    #[test]
    fn rejects_unsafe_refs_and_branches() {
        for value in [
            "-main",
            "main..other",
            "main@{1}",
            "white space",
            r"bad\ref",
        ] {
            assert!(validate_ref(value).is_err(), "{value}");
        }
        assert!(validate_branch("main").is_err());
        assert!(validate_branch("codex/run-aw-safe").is_ok());
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
}
