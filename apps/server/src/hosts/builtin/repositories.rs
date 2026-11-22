//! A1.18: the repositories on the built-in host's disk. Each registered
//! repository has a mirror there, and each run a worktree beside it. The
//! host clones and refreshes mirrors, checks a run's branch before it is
//! published, pushes it, and removes a repository's bytes when its
//! workspace goes.
//!
//! The server never does that work itself: it asks the workspace's
//! built-in host with an `action` command
//! ([`agent_workspace_host_protocol::repository`]) and reads the answer,
//! as it asks about accounts ([`super::accounts`]). When this process is
//! the host the question is carried out at once through the protocol in
//! process; otherwise it waits for the host. A git token the host needs is
//! handed over in memory, or, for a host that runs elsewhere, sealed beside
//! its command and taken when the host fetches it, so no row keeps it.
//!
//! - [`prepare`], [`Publication::check`], [`Publication::push`] and
//!   [`remove`] are the server's half.
//! - [`answer`] is the host's half, run from its command loop, under the
//!   host's own data root.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use agent_workspace_builtin_host::{
    git::{self, PublishPush, RegisteredRepository, RepositoryPreparationMode},
    runtime::commands::{self as host_commands, refused},
};
use agent_workspace_host_protocol::{
    Ack, AckOutcome, Command, CommandKind,
    repository::{
        MirrorSpec, PublicationSpec, REFUSED_FAILED, REFUSED_INVALID, REPOSITORY_ACTION,
        RepositoryAnswer, RepositoryQuestion,
    },
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    hosts::commands::{self, NewCommand},
};

/// How long a clone, a refresh or a push the host has taken may take.
const LONG_WORK: Duration = Duration::from_secs(15 * 60);
/// How long a branch check or a removal the host has taken may take.
const SHORT_WORK: Duration = Duration::from_secs(2 * 60);
/// The key a git token is handed over under. It is never stored in a
/// command's body.
const TOKEN_KEY: &str = "gitToken";

/// Where a repository's mirror lives on this server's own disk, recorded
/// when it is registered. The built-in host keeps its mirrors under its own
/// root, whatever this says.
pub fn mirror_path(data_root: &Path, repository_id: Uuid) -> Result<PathBuf, AppError> {
    Ok(git::expected_repository_mirror_path(
        data_root,
        repository_id,
    )?)
}

/// The workspace a space belongs to, whose built-in host has its
/// repositories.
pub async fn workspace_of_space(
    connection: &mut sqlx::PgConnection,
    space_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(space_id)
        .fetch_optional(connection)
        .await?
        .ok_or(AppError::NotFound("project"))
}

// ---------------------------------------------------------------- asking

/// What the host made of a question.
enum Asked {
    Answered(RepositoryAnswer),
    /// It refused, or didn't answer in time (`unanswered`).
    Failed(String),
}

/// Ask the built-in host of `workspace_id` about one of its repositories,
/// handing it `token` when the question needs one.
async fn ask(
    state: &AppState,
    workspace_id: Uuid,
    question: RepositoryQuestion,
    token: Option<SecretString>,
) -> Result<Asked, AppError> {
    let here = super::accounts::host_is_here(state);
    let sealing = match (&token, here) {
        (Some(_), false) => Some(crate::secrets::keyring(&state.config).map_err(|_| {
            AppError::Conflict(
                "repository",
                "a separate built-in host needs the server's secrets key to be handed a git token",
            )
        })?),
        _ => None,
    };
    let budget = match question {
        RepositoryQuestion::Prepare { .. } | RepositoryQuestion::Push { .. } => LONG_WORK,
        RepositoryQuestion::CheckPublication { .. } | RepositoryQuestion::Remove { .. } => {
            SHORT_WORK
        }
    };
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let host_id = super::assistant::builtin_host_id(&mut transaction, workspace_id)
        .await?
        .ok_or(AppError::Conflict(
            "repository",
            "this workspace has no built-in host",
        ))?;
    let command = commands::issue(
        &mut transaction,
        NewCommand {
            workspace_id,
            host_id,
            run_id: None,
            request_id: None,
            kind: CommandKind::Action,
            start_key: None,
            body: json!({"action": REPOSITORY_ACTION, "input": question}),
        },
    )
    .await?;
    if let (Some(keys), Some(token)) = (&sealing, &token) {
        seal_token(
            &mut transaction,
            keys.as_ref(),
            workspace_id,
            command.id,
            token,
        )
        .await?;
    }
    if here {
        commands::take_here(&mut transaction, command.id).await?;
    }
    transaction.commit().await?;
    let ack = if here {
        let client = super::transport::client(state);
        let mut handed = serde_json::Map::new();
        if let Some(token) = &token {
            handed.insert(
                TOKEN_KEY.into(),
                Value::String(token.expose_secret().to_owned()),
            );
        }
        crate::tenancy::system(
            "runs.host",
            host_commands::carry_out_one(&super::driver::host(state), &client, command.id, handed),
        )
        .await
        .ok()
    } else {
        super::accounts::wait_for(state, command.id, budget).await?
    };
    commands::forget_answered(&state.pool, command.id).await?;
    Ok(match ack {
        Some(ack) => read_ack(ack),
        None => Asked::Failed("unanswered".to_owned()),
    })
}

fn read_ack(ack: Ack) -> Asked {
    match ack.outcome {
        Some(AckOutcome::Done | AckOutcome::Accepted) => ack
            .result
            .and_then(|result| serde_json::from_value::<RepositoryAnswer>(result).ok())
            .map_or_else(
                || Asked::Failed("invalid_response".to_owned()),
                Asked::Answered,
            ),
        _ => Asked::Failed(ack.reason.unwrap_or_else(|| REFUSED_FAILED.to_owned())),
    }
}

/// What binds a sealed git token to its command.
fn token_aad(command_id: Uuid, workspace_id: Uuid) -> Vec<u8> {
    format!("aw-host-command-token:v1:{command_id}:{workspace_id}").into_bytes()
}

async fn seal_token(
    connection: &mut sqlx::PgConnection,
    keys: &dyn crate::secrets::KeyEncryption,
    workspace_id: Uuid,
    command_id: Uuid,
    token: &SecretString,
) -> Result<(), AppError> {
    let sealed = crate::secrets::seal_detached(
        keys,
        &token_aad(command_id, workspace_id),
        token.expose_secret().as_bytes(),
    )?;
    crate::secrets::hold_off_rotation(&mut *connection, keys).await?;
    sqlx::query(
        r#"
        INSERT INTO host_command_secrets
            (command_id, workspace_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(command_id)
    .bind(workspace_id)
    .bind(&sealed.ciphertext)
    .bind(sealed.nonce.as_slice())
    .bind(&sealed.wrapped.ciphertext)
    .bind(sealed.wrapped.nonce.as_ref().map(|nonce| nonce.as_slice()))
    .bind(&sealed.wrapped.master_key_id)
    .execute(connection)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct SealedToken {
    workspace_id: Uuid,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_key: Vec<u8>,
    wrap_nonce: Option<Vec<u8>>,
    master_key_id: String,
}

/// What the host's fetch of a repository question adds that no row keeps:
/// the git token sealed beside it, taken once.
pub async fn attach_secret(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    command_id: Uuid,
    body: &Value,
    command: &mut serde_json::Map<String, Value>,
) -> Result<(), AppError> {
    if body.get("action").and_then(Value::as_str) != Some(REPOSITORY_ACTION) {
        return Ok(());
    }
    let Some(row) = sqlx::query_as::<_, SealedToken>(
        r#"
        DELETE FROM host_command_secrets
         WHERE command_id = $1
        RETURNING workspace_id, ciphertext, nonce, wrapped_key, wrap_nonce, master_key_id
        "#,
    )
    .bind(command_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(());
    };
    let keys = crate::secrets::keyring(&state.config)?;
    let broken = || AppError::Internal("sealed git token");
    let sealed = crate::secrets::DetachedSeal {
        ciphertext: row.ciphertext,
        nonce: row.nonce.as_slice().try_into().map_err(|_| broken())?,
        wrapped: crate::secrets::WrappedKey {
            master_key_id: row.master_key_id,
            nonce: row
                .wrap_nonce
                .as_deref()
                .map(|nonce| nonce.try_into())
                .transpose()
                .map_err(|_| broken())?,
            ciphertext: row.wrapped_key,
        },
    };
    let opened = crate::secrets::open_detached(
        keys.as_ref(),
        &token_aad(command_id, row.workspace_id),
        &sealed,
    )?;
    let token = String::from_utf8(opened.to_vec()).map_err(|_| broken())?;
    command.insert(TOKEN_KEY.into(), Value::String(token));
    Ok(())
}

// --------------------------------------------------------- what is asked

/// A repository's mirror, as registered.
pub struct Mirror {
    pub repository_id: Uuid,
    pub clone_url: String,
    pub default_branch: String,
    /// Its mirror came from an older layout and is only checked, never
    /// fetched again.
    pub imported: bool,
}

/// Have the host clone or refresh a repository's mirror (an imported one
/// is checked instead), with a short-lived token from its code host.
/// Whether the mirror is ready; an error, worth trying again later, when
/// the host has no room or didn't answer.
pub async fn prepare(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    mirror: Mirror,
) -> Result<bool, AppError> {
    let token = if mirror.imported {
        None
    } else {
        match crate::github::repository_git_token(state, project_id, mirror.repository_id).await {
            Ok(token) => Some(token),
            Err(_) => return Ok(false),
        }
    };
    let repository_id = mirror.repository_id;
    let question = RepositoryQuestion::Prepare {
        mirror: MirrorSpec {
            repository_id,
            clone_url: mirror.clone_url,
            default_branch: mirror.default_branch,
        },
        imported: mirror.imported,
    };
    match ask(state, workspace_id, question, token).await? {
        Asked::Answered(RepositoryAnswer::Prepared { ready }) => Ok(ready),
        Asked::Answered(RepositoryAnswer::NoRoom) => Err(AppError::Conflict(
            "repository",
            "the built-in host's disk has no room for it yet",
        )),
        Asked::Answered(_) => Ok(false),
        Asked::Failed(reason) => {
            tracing::warn!(%repository_id, reason, "the host didn't prepare a repository");
            Err(AppError::Conflict(
                "repository",
                "the built-in host didn't prepare it; try again",
            ))
        }
    }
}

/// Before migrating from the old layout: move a mirror kept at `legacy`
/// into its place under the data root. This runs once, at a deploy, on the
/// machine whose disk holds both.
pub async fn adopt_legacy_mirror(
    data_root: &Path,
    mirror: Mirror,
    legacy: PathBuf,
) -> Result<(), AppError> {
    let data_root = data_root.to_path_buf();
    let registered = RegisteredRepository {
        id: mirror.repository_id,
        clone_url: mirror.clone_url,
        default_branch: mirror.default_branch,
        mirror_path: git::expected_repository_mirror_path(&data_root, mirror.repository_id)?,
    };
    tokio::task::spawn_blocking(move || git::adopt_legacy_mirror(&data_root, &registered, &legacy))
        .await
        .map_err(|_| AppError::Internal("join legacy mirror adoption"))??;
    Ok(())
}

/// Have the host remove a repository's mirror and worktrees. Missing pieces
/// are fine.
pub async fn remove(
    state: &AppState,
    workspace_id: Uuid,
    repository_id: Uuid,
) -> Result<(), AppError> {
    match ask(
        state,
        workspace_id,
        RepositoryQuestion::Remove { repository_id },
        None,
    )
    .await?
    {
        Asked::Answered(RepositoryAnswer::Removed) => Ok(()),
        _ => Err(AppError::Conflict(
            "repository",
            "the built-in host didn't remove the repository; try again",
        )),
    }
}

/// A run's worktree, as stored, to publish from.
pub struct Publication<'a> {
    pub workspace_id: Uuid,
    pub repository_id: Uuid,
    pub repository_workspace_id: Uuid,
    pub worktree_path: &'a str,
    pub branch: &'a str,
    pub base_commit: &'a str,
}

/// What the host found when it looked at a run's branch.
pub enum PublicationCheck {
    /// The stored worktree isn't where this host keeps it.
    Moved,
    /// The branch can't be published as it is.
    Refused(AppError),
    /// It can, at this commit.
    At(String),
}

impl Publication<'_> {
    fn spec(&self) -> PublicationSpec {
        PublicationSpec {
            repository_id: self.repository_id,
            repository_workspace_id: self.repository_workspace_id,
            worktree_path: self.worktree_path.to_owned(),
            branch: self.branch.to_owned(),
            base_commit: self.base_commit.to_owned(),
        }
    }

    /// Whether the run's branch is clean and its own, from its base, and
    /// the commit it holds. An error when the host couldn't say.
    pub async fn check(&self, state: &AppState) -> Result<PublicationCheck, AppError> {
        let question = RepositoryQuestion::CheckPublication {
            publication: self.spec(),
        };
        match ask(state, self.workspace_id, question, None).await? {
            Asked::Answered(RepositoryAnswer::Moved) => Ok(PublicationCheck::Moved),
            Asked::Answered(RepositoryAnswer::At { commit }) => Ok(PublicationCheck::At(commit)),
            Asked::Answered(RepositoryAnswer::Refused { why }) => Ok(PublicationCheck::Refused(
                git::publication_refusal(&why).into(),
            )),
            Asked::Answered(_) | Asked::Failed(_) => Err(AppError::Conflict(
                "repository_workspace",
                "the built-in host didn't check the run's branch; try again",
            )),
        }
    }

    /// Have the host push `intended_commit` to the run's branch on
    /// `remote_url` (which its code host checked is one of its own) with
    /// `token`. Whether git reported success; the caller confirms with the
    /// code host either way.
    pub async fn push(
        &self,
        state: &AppState,
        intended_commit: &str,
        remote_url: &str,
        remote_is_safe: bool,
        token: SecretString,
    ) -> Result<bool, AppError> {
        let question = RepositoryQuestion::Push {
            publication: self.spec(),
            intended_commit: intended_commit.to_owned(),
            remote_url: remote_url.to_owned(),
            remote_is_safe,
        };
        Ok(matches!(
            ask(state, self.workspace_id, question, Some(token)).await?,
            Asked::Answered(RepositoryAnswer::Pushed { ok: true })
        ))
    }
}

// --------------------------------------------------------------- the host

/// Whether `command` is a repository question.
pub fn is_repository_action(command: &Command) -> bool {
    command.detail.get("action").and_then(Value::as_str) == Some(REPOSITORY_ACTION)
}

/// Carry out a repository question under this host's own data root.
pub async fn answer(state: &AppState, command: &Command) -> Ack {
    let Some(question) = command
        .detail
        .get("input")
        .cloned()
        .and_then(|input| serde_json::from_value::<RepositoryQuestion>(input).ok())
    else {
        return refused(REFUSED_INVALID, "That repository question can't be read.");
    };
    let token = command
        .detail
        .get(TOKEN_KEY)
        .and_then(Value::as_str)
        .map(|token| SecretString::from(token.to_owned()));
    if question.needs_token() && token.is_none() {
        return refused(REFUSED_FAILED, "No git token came with the question.");
    }
    match carry_out(state, question, token).await {
        Ok(answer) => {
            let mut ack = Ack::new(AckOutcome::Done);
            ack.result = serde_json::to_value(answer).ok();
            ack
        }
        Err(error) => {
            tracing::warn!(error = %error, "a repository question failed");
            refused(REFUSED_FAILED, "The repository work failed.")
        }
    }
}

async fn carry_out(
    state: &AppState,
    question: RepositoryQuestion,
    token: Option<SecretString>,
) -> Result<RepositoryAnswer, AppError> {
    let data_root = state.config.data_root.clone();
    match question {
        RepositoryQuestion::Prepare { mirror, imported } => {
            let quota = state.config.data_quota_bytes;
            let free = state.config.minimum_free_bytes;
            let root = data_root.clone();
            let admitted = tokio::task::spawn_blocking(move || {
                crate::storage_admission::require_growth(&root, quota, free)
            })
            .await
            .map_err(|_| AppError::Internal("join storage admission"))?;
            if admitted.is_err() {
                return Ok(RepositoryAnswer::NoRoom);
            }
            let registered = RegisteredRepository {
                id: mirror.repository_id,
                mirror_path: git::expected_repository_mirror_path(
                    &data_root,
                    mirror.repository_id,
                )?,
                clone_url: mirror.clone_url,
                default_branch: mirror.default_branch,
            };
            let prepared = tokio::task::spawn_blocking(move || match token {
                Some(token) if !imported => git::prepare_repository_with_token(
                    data_root.as_path(),
                    &registered,
                    RepositoryPreparationMode::RefreshOrClone,
                    &token,
                ),
                _ => git::prepare_repository(
                    data_root.as_path(),
                    &registered,
                    RepositoryPreparationMode::VerifyImportedMirror,
                ),
            })
            .await
            .map_err(|_| AppError::Internal("join repository preparation"))?;
            Ok(RepositoryAnswer::Prepared {
                ready: prepared.is_ok(),
            })
        }
        RepositoryQuestion::CheckPublication { publication } => {
            let Some((mirror, worktree)) = paths_here(&data_root, &publication)? else {
                return Ok(RepositoryAnswer::Moved);
            };
            let inspection = tokio::task::spawn_blocking(move || {
                git::inspect_publishable_worktree(
                    &mirror,
                    &worktree,
                    &publication.branch,
                    &publication.base_commit,
                )
            })
            .await
            .map_err(|_| AppError::Internal("join Git publication inspection"))?;
            Ok(match inspection {
                Ok(inspection) => RepositoryAnswer::At {
                    commit: inspection.head_commit,
                },
                Err(agent_workspace_builtin_host::RuntimeError::Conflict(_, why)) => {
                    RepositoryAnswer::Refused {
                        why: why.to_owned(),
                    }
                }
                Err(error) => return Err(error.into()),
            })
        }
        RepositoryQuestion::Push {
            publication,
            intended_commit,
            remote_url,
            remote_is_safe,
        } => {
            let Some((mirror, worktree)) = paths_here(&data_root, &publication)? else {
                return Ok(RepositoryAnswer::Pushed { ok: false });
            };
            let Some(token) = token else {
                return Ok(RepositoryAnswer::Pushed { ok: false });
            };
            let pushed = tokio::task::spawn_blocking(move || {
                git::push_publishable_commit(PublishPush {
                    data_root: &data_root,
                    mirror: &mirror,
                    worktree: &worktree,
                    expected_branch: &publication.branch,
                    base_commit: &publication.base_commit,
                    intended_commit: &intended_commit,
                    remote_url: &remote_url,
                    remote_is_safe,
                    token: &token,
                })
            })
            .await
            .map_err(|_| AppError::Internal("join credentialed Git push"))?;
            Ok(RepositoryAnswer::Pushed { ok: pushed.is_ok() })
        }
        RepositoryQuestion::Remove { repository_id } => {
            git::remove_repository_data(&data_root, repository_id).await?;
            Ok(RepositoryAnswer::Removed)
        }
    }
}

/// The mirror and worktree this host keeps for a run, when the worktree
/// the server stored is that one.
fn paths_here(
    data_root: &Path,
    publication: &PublicationSpec,
) -> Result<Option<(PathBuf, PathBuf)>, AppError> {
    let mirror = git::expected_repository_mirror_path(data_root, publication.repository_id)?;
    let worktree = git::managed_worktree_path(
        data_root,
        publication.repository_id,
        publication.repository_workspace_id,
    )?;
    Ok((Path::new(&publication.worktree_path) == worktree).then_some((mirror, worktree)))
}
