use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use fs2::FileExt;
use thiserror::Error;
use uuid::Uuid;

const OUTPUT_LIMIT: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct GitWorktrees {
    git_binary: PathBuf,
    mirror: PathBuf,
    worktree_root: PathBuf,
}

#[derive(Debug)]
pub struct Workspace {
    pub branch: String,
    pub path: PathBuf,
    pub base_commit: String,
}

#[derive(Debug, Error)]
pub enum GitWorktreeError {
    #[error("unsafe branch component: {0}")]
    UnsafeBranch(String),
    #[error("unsafe worktree path: {0}")]
    UnsafePath(PathBuf),
    #[error("worktree has uncommitted changes: {0}")]
    Dirty(PathBuf),
    #[error("Git command failed: {command}; stderr: {stderr}")]
    Command { command: String, stderr: String },
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git returned non-UTF-8 output")]
    NonUtf8,
}

impl GitWorktrees {
    pub fn clone_mirror(
        git_binary: impl Into<PathBuf>,
        source: &Path,
        mirror: &Path,
        worktree_root: &Path,
    ) -> Result<Self, GitWorktreeError> {
        require_absolute(mirror)?;
        require_absolute(worktree_root)?;
        if mirror.exists() || worktree_root.exists() {
            return Err(GitWorktreeError::UnsafePath(
                if mirror.exists() {
                    mirror
                } else {
                    worktree_root
                }
                .to_path_buf(),
            ));
        }

        let git_binary = git_binary.into();
        run(
            &git_binary,
            [
                OsString::from("clone"),
                OsString::from("--mirror"),
                source.as_os_str().to_owned(),
                mirror.as_os_str().to_owned(),
            ],
        )?;
        fs::create_dir_all(worktree_root)?;

        Ok(Self {
            git_binary,
            mirror: mirror.to_path_buf(),
            worktree_root: worktree_root.to_path_buf(),
        })
    }

    pub fn open(
        git_binary: impl Into<PathBuf>,
        mirror: PathBuf,
        worktree_root: PathBuf,
    ) -> Result<Self, GitWorktreeError> {
        require_absolute(&mirror)?;
        require_absolute(&worktree_root)?;
        Ok(Self {
            git_binary: git_binary.into(),
            mirror,
            worktree_root,
        })
    }

    pub fn create(
        &self,
        project_slug: &str,
        base_ref: &str,
    ) -> Result<Workspace, GitWorktreeError> {
        validate_component(project_slug)?;
        validate_ref(base_ref)?;
        let _lock = self.lock()?;
        let branch = self.unique_branch(project_slug)?;
        let path = self.worktree_root.join(branch.replace('/', "--"));
        self.validate_target(&path)?;
        let base_commit = self.rev_parse(base_ref)?;
        self.git_dir([
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("-b"),
            OsString::from(&branch),
            path.as_os_str().to_owned(),
            OsString::from(&base_commit),
        ])?;
        Ok(Workspace {
            branch,
            path,
            base_commit,
        })
    }

    pub fn status_porcelain(&self, worktree: &Path) -> Result<Vec<u8>, GitWorktreeError> {
        self.validate_existing_worktree(worktree)?;
        let output = run(
            &self.git_binary,
            [
                OsString::from("-C"),
                worktree.as_os_str().to_owned(),
                OsString::from("status"),
                OsString::from("--porcelain=v1"),
                OsString::from("-z"),
                OsString::from("--untracked-files=all"),
            ],
        )?;
        Ok(output.stdout)
    }

    pub fn remove_clean(&self, worktree: &Path) -> Result<(), GitWorktreeError> {
        self.validate_existing_worktree(worktree)?;
        if !self.status_porcelain(worktree)?.is_empty() {
            return Err(GitWorktreeError::Dirty(worktree.to_path_buf()));
        }
        let _lock = self.lock()?;
        self.git_dir([
            OsString::from("worktree"),
            OsString::from("remove"),
            worktree.as_os_str().to_owned(),
        ])?;
        self.prune_locked()
    }

    pub fn repair_and_prune(&self, worktree: Option<&Path>) -> Result<(), GitWorktreeError> {
        let _lock = self.lock()?;
        if let Some(worktree) = worktree {
            self.validate_target(worktree)?;
            self.git_dir([
                OsString::from("worktree"),
                OsString::from("repair"),
                worktree.as_os_str().to_owned(),
            ])?;
        }
        self.prune_locked()
    }

    pub fn list_porcelain(&self) -> Result<String, GitWorktreeError> {
        let output = self.git_dir([
            OsString::from("worktree"),
            OsString::from("list"),
            OsString::from("--porcelain"),
        ])?;
        String::from_utf8(output.stdout).map_err(|_| GitWorktreeError::NonUtf8)
    }

    fn unique_branch(&self, project_slug: &str) -> Result<String, GitWorktreeError> {
        for _ in 0..8 {
            let candidate = format!("codex/run-{project_slug}-{}", Uuid::now_v7());
            let result = Command::new(&self.git_binary)
                .args([
                    OsStr::new("--git-dir"),
                    self.mirror.as_os_str(),
                    OsStr::new("show-ref"),
                    OsStr::new("--verify"),
                    OsStr::new("--quiet"),
                    OsStr::new(&format!("refs/heads/{candidate}")),
                ])
                .status()?;
            if !result.success() {
                return Ok(candidate);
            }
        }
        Err(GitWorktreeError::Command {
            command: "generate collision-safe branch".to_owned(),
            stderr: "eight UUIDv7 branch collisions".to_owned(),
        })
    }

    fn rev_parse(&self, reference: &str) -> Result<String, GitWorktreeError> {
        let output = self.git_dir([
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from(format!("{reference}^{{commit}}")),
        ])?;
        String::from_utf8(output.stdout)
            .map_err(|_| GitWorktreeError::NonUtf8)
            .map(|value| value.trim().to_owned())
    }

    fn git_dir<I>(&self, args: I) -> Result<Output, GitWorktreeError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut all = vec![
            OsString::from("--git-dir"),
            self.mirror.as_os_str().to_owned(),
        ];
        all.extend(args);
        run(&self.git_binary, all)
    }

    fn lock(&self) -> Result<RepositoryLock, GitWorktreeError> {
        let path = self.mirror.join("agent-workspace.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock_exclusive()?;
        Ok(RepositoryLock { file })
    }

    fn validate_existing_worktree(&self, path: &Path) -> Result<(), GitWorktreeError> {
        self.validate_target(path)?;
        if !path.is_dir() {
            return Err(GitWorktreeError::UnsafePath(path.to_path_buf()));
        }
        Ok(())
    }

    fn validate_target(&self, path: &Path) -> Result<(), GitWorktreeError> {
        require_absolute(path)?;
        let root = self.worktree_root.canonicalize()?;
        let candidate = if path.exists() {
            path.canonicalize()?
        } else {
            let parent = path
                .parent()
                .ok_or_else(|| GitWorktreeError::UnsafePath(path.to_path_buf()))?;
            parent.canonicalize()?.join(
                path.file_name()
                    .ok_or_else(|| GitWorktreeError::UnsafePath(path.to_path_buf()))?,
            )
        };
        if candidate.parent() != Some(root.as_path()) {
            return Err(GitWorktreeError::UnsafePath(path.to_path_buf()));
        }
        Ok(())
    }

    fn prune_locked(&self) -> Result<(), GitWorktreeError> {
        self.git_dir([
            OsString::from("worktree"),
            OsString::from("prune"),
            OsString::from("--expire"),
            OsString::from("now"),
        ])?;
        Ok(())
    }
}

struct RepositoryLock {
    file: File,
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn validate_component(value: &str) -> Result<(), GitWorktreeError> {
    if value.is_empty()
        || value.len() > 48
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
    {
        return Err(GitWorktreeError::UnsafeBranch(value.to_owned()));
    }
    Ok(())
}

fn validate_ref(value: &str) -> Result<(), GitWorktreeError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value.contains('\\')
        || value.chars().any(char::is_whitespace)
    {
        return Err(GitWorktreeError::UnsafeBranch(value.to_owned()));
    }
    Ok(())
}

fn require_absolute(path: &Path) -> Result<(), GitWorktreeError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(GitWorktreeError::UnsafePath(path.to_path_buf()))
    }
}

fn run<I>(git_binary: &Path, args: I) -> Result<Output, GitWorktreeError>
where
    I: IntoIterator<Item = OsString>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let output = Command::new(git_binary)
        .args(&args)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "1")
        .env("HOME", "/dev/null")
        .output()?;
    if output.status.success() {
        return Ok(output);
    }
    Err(GitWorktreeError::Command {
        command: render_command(git_binary, &args),
        stderr: bounded(&output.stderr),
    })
}

fn render_command(git_binary: &Path, args: &[OsString]) -> String {
    std::iter::once(git_binary.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn bounded(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(OUTPUT_LIMIT)]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        process::Command,
        sync::{Arc, Barrier},
        thread,
    };

    use tempfile::TempDir;

    use super::{GitWorktreeError, GitWorktrees};

    fn fixture() -> (TempDir, GitWorktrees) {
        let temp = tempfile::tempdir().expect("temporary root");
        let source = temp.path().join("source");
        fs::create_dir(&source).expect("source directory");
        git(&source, ["init", "-b", "main"]);
        git(&source, ["config", "user.name", "M0 Test"]);
        git(&source, ["config", "user.email", "m0@example.invalid"]);
        fs::write(source.join("known.txt"), "baseline\n").expect("fixture content");
        git(&source, ["add", "known.txt"]);
        git(&source, ["commit", "-m", "baseline"]);

        let worktrees = GitWorktrees::clone_mirror(
            Path::new("/usr/bin/git"),
            &source,
            &temp.path().join("mirror.git"),
            &temp.path().join("worktrees"),
        )
        .expect("mirror clone");
        (temp, worktrees)
    }

    fn git<const N: usize>(directory: &Path, args: [&str; N]) {
        let status = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/dev/null")
            .status()
            .expect("run fixture Git");
        assert!(status.success());
    }

    #[test]
    fn rejects_unsafe_branch_and_path_inputs() {
        let (_temp, worktrees) = fixture();
        assert!(matches!(
            worktrees.create("../escape", "main"),
            Err(GitWorktreeError::UnsafeBranch(_))
        ));
        assert!(matches!(
            worktrees.create("safe", "--upload-pack=bad"),
            Err(GitWorktreeError::UnsafeBranch(_))
        ));
        assert!(matches!(
            worktrees.remove_clean(Path::new("/tmp/not-our-worktree")),
            Err(GitWorktreeError::UnsafePath(_))
        ));
    }

    #[test]
    fn creates_two_concurrent_linked_worktrees_with_unique_branches() {
        let (_temp, worktrees) = fixture();
        let worktrees = Arc::new(worktrees);
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let worktrees = Arc::clone(&worktrees);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                worktrees.create("parallel", "main")
            }));
        }
        barrier.wait();
        let first = handles
            .remove(0)
            .join()
            .expect("first thread")
            .expect("first worktree");
        let second = handles
            .remove(0)
            .join()
            .expect("second thread")
            .expect("second worktree");

        assert_ne!(first.branch, second.branch);
        assert_eq!(first.base_commit, second.base_commit);
        assert!(first.path.join("known.txt").is_file());
        assert!(second.path.join("known.txt").is_file());
        let listing = worktrees.list_porcelain().expect("list worktrees");
        assert!(listing.contains(&first.path.to_string_lossy().to_string()));
        assert!(listing.contains(&second.path.to_string_lossy().to_string()));
    }

    #[test]
    fn preserves_dirty_worktree_then_removes_when_clean() {
        let (_temp, worktrees) = fixture();
        let workspace = worktrees.create("dirty", "main").expect("worktree");
        fs::write(workspace.path.join("known.txt"), "changed\n").expect("dirty file");

        assert!(matches!(
            worktrees.remove_clean(&workspace.path),
            Err(GitWorktreeError::Dirty(_))
        ));
        assert!(workspace.path.is_dir());
        git(&workspace.path, ["restore", "known.txt"]);

        worktrees
            .remove_clean(&workspace.path)
            .expect("clean removal");
        assert!(!workspace.path.exists());
        worktrees.repair_and_prune(None).expect("prune metadata");
    }
}
