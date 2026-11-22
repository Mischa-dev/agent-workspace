//! A1.18 follow-up: what a server asks the built-in host about the
//! repositories on its disk, and what the host answers. Each registered
//! repository has a mirror there, and each run a worktree beside it. The
//! server writes the question as an `action` command
//! (`{"action": "repository", "input": RepositoryQuestion}`), the host does
//! the git work under its own data root and answers in the acknowledgement's
//! `result`. A refusal's `reason` is a word, such as [`REFUSED_INVALID`].
//!
//! The server names a repository and a run's worktree by id; where they
//! live is the host's own business. The worktree path the server stored is
//! passed only so the host can say it isn't where it keeps it (`Moved`).
//!
//! Nothing secret travels in a command: a short-lived git token is handed
//! over when the host fetches the command, and is kept in no row.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The `action` a repository question is written as.
pub const REPOSITORY_ACTION: &str = "repository";

/// The question couldn't be read, or named something unsafe.
pub const REFUSED_INVALID: &str = "invalid";
/// The git work failed.
pub const REFUSED_FAILED: &str = "failed";

/// A repository's mirror, as registered.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorSpec {
    pub repository_id: Uuid,
    /// The clone address, with no credentials in it.
    pub clone_url: String,
    pub default_branch: String,
}

/// A run's worktree, as the server stored it, to publish from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationSpec {
    pub repository_id: Uuid,
    pub repository_workspace_id: Uuid,
    /// Where the host said the run's worktree is.
    pub worktree_path: String,
    pub branch: String,
    pub base_commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum RepositoryQuestion {
    /// Clone or refresh the mirror with the git token handed over with the
    /// command. A mirror that came from an older layout (`imported`) is
    /// only checked, and needs no token.
    Prepare {
        mirror: MirrorSpec,
        #[serde(default)]
        imported: bool,
    },
    /// Whether the run's branch is clean and its own, from its base, and
    /// the commit it holds.
    CheckPublication { publication: PublicationSpec },
    /// Push `intended_commit` to the run's branch on `remote_url` with the
    /// git token handed over with the command. `remote_is_safe` says the
    /// code host checked the address is one of its own.
    #[serde(rename_all = "camelCase")]
    Push {
        publication: PublicationSpec,
        intended_commit: String,
        remote_url: String,
        remote_is_safe: bool,
    },
    /// The repository is gone: remove its mirror and worktrees. Missing
    /// pieces are fine.
    #[serde(rename_all = "camelCase")]
    Remove { repository_id: Uuid },
}

impl RepositoryQuestion {
    /// Whether the host needs a git token handed over to answer.
    pub fn needs_token(&self) -> bool {
        match self {
            Self::Prepare { imported, .. } => !imported,
            Self::Push { .. } => true,
            Self::CheckPublication { .. } | Self::Remove { .. } => false,
        }
    }
}

/// The host's answer, in the acknowledgement's `result`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum RepositoryAnswer {
    /// Whether the mirror is ready.
    Prepared { ready: bool },
    /// The stored worktree path isn't where this host keeps it.
    Moved,
    /// The branch can't be published as it is, and why, in a sentence.
    Refused { why: String },
    /// It can, at this commit.
    At { commit: String },
    /// Whether git reported the push went through. The server confirms
    /// with the code host either way.
    Pushed { ok: bool },
    /// Removed (or already gone).
    Removed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_and_answers_read_back() {
        let question = RepositoryQuestion::Push {
            publication: PublicationSpec {
                repository_id: Uuid::nil(),
                repository_workspace_id: Uuid::nil(),
                worktree_path: "/w".into(),
                branch: "aw/run".into(),
                base_commit: "abc".into(),
            },
            intended_commit: "def".into(),
            remote_url: "https://example.test/r.git".into(),
            remote_is_safe: true,
        };
        let value = serde_json::to_value(&question).expect("write the question");
        assert_eq!(value["ask"], "push");
        assert_eq!(value["intendedCommit"], "def");
        assert_eq!(
            serde_json::from_value::<RepositoryQuestion>(value).expect("read the question"),
            question
        );
        assert!(question.needs_token());
        assert!(
            !RepositoryQuestion::Prepare {
                mirror: MirrorSpec {
                    repository_id: Uuid::nil(),
                    clone_url: "https://example.test/r.git".into(),
                    default_branch: "main".into(),
                },
                imported: true,
            }
            .needs_token()
        );
        let answer = serde_json::to_value(RepositoryAnswer::At {
            commit: "abc".into(),
        })
        .expect("write the answer");
        assert_eq!(answer, serde_json::json!({"answer": "at", "commit": "abc"}));
    }
}
