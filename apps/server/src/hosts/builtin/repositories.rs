//! A1.18: the repositories on the built-in host's disk. Each registered
//! repository has a mirror there, and each run a worktree beside it. The
//! host clones and refreshes mirrors, checks a run's branch before it is
//! published, pushes it, and removes a repository's bytes when its
//! workspace goes. The server's own modules ask here; they never run git.

use std::path::{Path, PathBuf};

use agent_workspace_builtin_host::git::{
    self, PublishPush, RegisteredRepository, RepositoryPreparationMode,
};
use secrecy::SecretString;
use uuid::Uuid;

use crate::{AppState, error::AppError};

/// Where a repository's mirror lives on the host's disk.
pub fn mirror_path(data_root: &Path, repository_id: Uuid) -> Result<PathBuf, AppError> {
    Ok(git::expected_repository_mirror_path(
        data_root,
        repository_id,
    )?)
}

/// A repository's mirror, as registered.
pub struct Mirror {
    pub repository_id: Uuid,
    pub clone_url: String,
    pub default_branch: String,
    pub mirror_path: PathBuf,
    /// Its mirror came from an older layout and is only checked, never
    /// fetched again.
    pub imported: bool,
}

impl Mirror {
    fn registered(&self) -> RegisteredRepository {
        RegisteredRepository {
            id: self.repository_id,
            clone_url: self.clone_url.clone(),
            default_branch: self.default_branch.clone(),
            mirror_path: self.mirror_path.clone(),
        }
    }
}

/// Clone or refresh a repository's mirror (an imported one is checked
/// instead), with a short-lived token from its code host. Whether the
/// mirror is ready.
pub async fn prepare(state: &AppState, project_id: Uuid, mirror: Mirror) -> bool {
    let registered = mirror.registered();
    let data_root = state.config.data_root.clone();
    if mirror.imported {
        return matches!(
            tokio::task::spawn_blocking(move || {
                git::prepare_repository(
                    data_root.as_path(),
                    &registered,
                    RepositoryPreparationMode::VerifyImportedMirror,
                )
            })
            .await,
            Ok(Ok(_))
        );
    }
    let Ok(token) =
        crate::github::repository_git_token(state, project_id, mirror.repository_id).await
    else {
        return false;
    };
    matches!(
        tokio::task::spawn_blocking(move || {
            git::prepare_repository_with_token(
                data_root.as_path(),
                &registered,
                RepositoryPreparationMode::RefreshOrClone,
                &token,
            )
        })
        .await,
        Ok(Ok(_))
    )
}

/// Before migrating from the old layout: move a mirror kept at `legacy`
/// into its place under the data root (`mirror.mirror_path`).
pub async fn adopt_legacy_mirror(
    data_root: &Path,
    mirror: Mirror,
    legacy: PathBuf,
) -> Result<(), AppError> {
    let data_root = data_root.to_path_buf();
    let registered = mirror.registered();
    tokio::task::spawn_blocking(move || git::adopt_legacy_mirror(&data_root, &registered, &legacy))
        .await
        .map_err(|_| AppError::Internal("join legacy mirror adoption"))??;
    Ok(())
}

/// Remove a repository's mirror and worktrees. Missing pieces are fine.
pub async fn remove(data_root: &Path, repository_id: Uuid) -> Result<(), AppError> {
    git::remove_repository_data(data_root, repository_id).await?;
    Ok(())
}

/// A run's worktree, as stored, to publish from.
pub struct Publication<'a> {
    pub repository_id: Uuid,
    pub repository_workspace_id: Uuid,
    pub mirror_path: &'a str,
    pub worktree_path: &'a str,
    pub branch: &'a str,
    pub base_commit: &'a str,
}

/// What the host found when it looked at a run's branch.
pub enum PublicationCheck {
    /// The stored paths aren't where this host keeps them.
    Moved,
    /// The branch can't be published as it is.
    Refused(AppError),
    /// It can, at this commit.
    At(String),
}

impl Publication<'_> {
    fn paths(&self, data_root: &Path) -> Result<Option<(PathBuf, PathBuf)>, AppError> {
        let mirror = mirror_path(data_root, self.repository_id)?;
        let worktree = git::managed_worktree_path(
            data_root,
            self.repository_id,
            self.repository_workspace_id,
        )?;
        Ok(
            (Path::new(self.mirror_path) == mirror && Path::new(self.worktree_path) == worktree)
                .then_some((mirror, worktree)),
        )
    }

    /// Whether the run's branch is clean and its own, from its base, and
    /// the commit it holds.
    pub async fn check(&self, state: &AppState) -> Result<PublicationCheck, AppError> {
        let Some((mirror, worktree)) = self.paths(&state.config.data_root)? else {
            return Ok(PublicationCheck::Moved);
        };
        let branch = self.branch.to_owned();
        let base = self.base_commit.to_owned();
        let inspection = tokio::task::spawn_blocking(move || {
            git::inspect_publishable_worktree(&mirror, &worktree, &branch, &base)
        })
        .await
        .map_err(|_| AppError::Internal("join Git publication inspection"))?;
        Ok(match inspection {
            Ok(inspection) => PublicationCheck::At(inspection.head_commit),
            Err(error) => PublicationCheck::Refused(error.into()),
        })
    }

    /// Push `intended_commit` to the run's branch on `remote_url` (which
    /// its code host checked is one of its own) with `token`. Whether git
    /// reported success; the caller confirms with the code host either way.
    pub async fn push(
        &self,
        state: &AppState,
        intended_commit: &str,
        remote_url: &str,
        remote_is_safe: bool,
        token: SecretString,
    ) -> Result<bool, AppError> {
        let Some((mirror, worktree)) = self.paths(&state.config.data_root)? else {
            return Ok(false);
        };
        let data_root = state.config.data_root.clone();
        let branch = self.branch.to_owned();
        let base = self.base_commit.to_owned();
        let intended = intended_commit.to_owned();
        let remote_url = remote_url.to_owned();
        let pushed = tokio::task::spawn_blocking(move || {
            git::push_publishable_commit(PublishPush {
                data_root: &data_root,
                mirror: &mirror,
                worktree: &worktree,
                expected_branch: &branch,
                base_commit: &base,
                intended_commit: &intended,
                remote_url: &remote_url,
                remote_is_safe,
                token: &token,
            })
        })
        .await
        .map_err(|_| AppError::Internal("join credentialed Git push"))?;
        Ok(pushed.is_ok())
    }
}
