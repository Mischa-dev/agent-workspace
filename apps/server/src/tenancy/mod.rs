//! Tenancy: which workspace every row belongs to ([`tables`]), and the scope
//! each request, job and transaction runs in (H1.20).
//!
//! A task-local [`Tenancy`] says whose rows the work in hand may touch:
//! - a signed-in account of any kind gets [`Tenancy::Member`] over the
//!   workspaces it belongs to now (the browser session, an MCP or public API
//!   credential);
//! - a job gets [`Tenancy::Workspace`] for the workspace it was queued for;
//! - server-wide work names a registered reason with [`system`];
//! - anything before authentication is [`Tenancy::Anonymous`].
//!
//! [`crate::db::scoped_tx`] and [`crate::db::scoped_one`] carry the scope
//! into PostgreSQL as `app.tenancy`, set with `set_config(.., true)` (the
//! same as `SET LOCAL`) as part of the transaction's `BEGIN`, never on the
//! session, so a transaction-mode pooler can't hand one request's scope to
//! the next (R24 rule 3). Row rules (H1.21, H1.22) read that one setting;
//! there is no separate `app.workspace_id`.
//!
//! Until those rules exist nothing is refused: this is observe mode. A
//! scoped transaction begun with no scope at all is logged once per call
//! site (target `agent_workspace::tenancy`) and counted in metrics.

pub mod tables;

use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use uuid::Uuid;

/// Whose rows the work in hand may touch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tenancy {
    /// A signed-in account of any kind: the workspaces it belongs to now.
    /// `None` until first needed, so a request that never opens a scoped
    /// transaction never looks them up.
    Member {
        account: Uuid,
        workspaces: Option<Vec<Uuid>>,
    },
    /// One workspace: a job, an import, or a request narrowed to the
    /// workspace in its path (K2's `/api/v2/w/{workspace}`), with the
    /// account acting if there is one.
    Workspace {
        account: Option<Uuid>,
        workspace: Uuid,
    },
    /// Server-wide work that reads across workspaces, under a reason from
    /// [`SYSTEM_REASONS`].
    System(&'static str),
    /// Before authentication: only global tables.
    Anonymous,
}

/// Every reason server-wide work may give, and the source files that may
/// give it. `tests::system_scopes_use_registered_reasons_in_their_files`
/// fails on any other.
pub const SYSTEM_REASONS: &[(&str, &[&str])] = &[
    // Looking up an account's workspaces, before its scope can be set.
    ("auth.lookup", &["tenancy/mod.rs"]),
    // Claiming due jobs of every workspace, and finishing them.
    ("jobs.claim", &["jobs.rs"]),
    // Jobs queued for no workspace (the server's own recurring work).
    ("jobs.server", &["jobs.rs"]),
];

impl Tenancy {
    /// The value of `app.tenancy` for this scope: `m:<account>|<ws>,<ws>`,
    /// `w:<account>|<ws>` (no account: `w:|<ws>`), `s:<reason>` or `a`.
    /// Made only of hex digits, hyphens and the reason's letters, so it is
    /// safe inside the quoted literal it is sent in.
    pub fn setting(&self) -> String {
        match self {
            Self::Member {
                account,
                workspaces,
            } => {
                let mut workspaces = workspaces.clone().unwrap_or_default();
                workspaces.sort_unstable();
                workspaces.dedup();
                format!(
                    "m:{account}|{}",
                    workspaces
                        .iter()
                        .map(Uuid::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            Self::Workspace { account, workspace } => format!(
                "w:{}|{workspace}",
                account.map(|account| account.to_string()).unwrap_or_default()
            ),
            Self::System(reason) => format!("s:{reason}"),
            Self::Anonymous => "a".to_owned(),
        }
    }

    /// The scope's kind, for logs and metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Member { .. } => "member",
            Self::Workspace { .. } => "workspace",
            Self::System(_) => "system",
            Self::Anonymous => "anonymous",
        }
    }
}

tokio::task_local! {
    static TENANCY: Arc<Mutex<Tenancy>>;
}

/// Runs `future` in `tenancy`'s scope.
pub async fn scoped<F: Future>(tenancy: Tenancy, future: F) -> F::Output {
    TENANCY.scope(Arc::new(Mutex::new(tenancy)), future).await
}

/// Runs `future` as server-wide work under `reason`, which must be one of
/// [`SYSTEM_REASONS`].
pub fn system<F: Future>(reason: &'static str, future: F) -> impl Future<Output = F::Output> {
    debug_assert!(
        SYSTEM_REASONS.iter().any(|(known, _)| *known == reason),
        "unregistered tenancy reason {reason}"
    );
    scoped(Tenancy::System(reason), future)
}

/// The scope the current task runs in, if any.
pub fn current() -> Option<Tenancy> {
    TENANCY
        .try_with(|cell| {
            cell.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .ok()
}

/// Authentication found the account behind the request: an anonymous
/// request scope becomes that account's member scope. A scope already set
/// (a job, a narrower workspace scope, another account) is left alone.
pub fn signed_in(account: Uuid) {
    let _ = TENANCY.try_with(|cell| {
        let mut tenancy = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *tenancy == Tenancy::Anonymous {
            *tenancy = Tenancy::Member {
                account,
                workspaces: None,
            };
        }
    });
}

/// The member scope's workspaces once looked up, kept for the rest of the
/// request.
fn remember_workspaces(account: Uuid, found: &[Uuid]) {
    let _ = TENANCY.try_with(|cell| {
        let mut tenancy = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Tenancy::Member {
            account: current,
            workspaces,
        } = &mut *tenancy
            && *current == account
        {
            *workspaces = Some(found.to_vec());
        }
    });
}

/// The current scope with a member's workspaces filled in: looked up the
/// first time a request needs them (under `auth.lookup`, since the scope
/// can't be set before they are known), then remembered.
pub(crate) async fn resolved(pool: &sqlx::PgPool) -> Result<Option<Tenancy>, sqlx::Error> {
    let Some(tenancy) = current() else {
        return Ok(None);
    };
    let Tenancy::Member {
        account,
        workspaces: None,
    } = tenancy
    else {
        return Ok(Some(tenancy));
    };
    let lookup = Tenancy::System("auth.lookup").setting();
    let mut transaction = pool
        .begin_with(format!(
            "BEGIN; SELECT set_config('app.tenancy', '{lookup}', true)"
        ))
        .await?;
    let workspaces = sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM organization_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(account)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    remember_workspaces(account, &workspaces);
    Ok(Some(Tenancy::Member {
        account,
        workspaces: Some(workspaces),
    }))
}

/// Middleware: every request runs in its own scope, anonymous until
/// authentication names the account ([`signed_in`]). The request's log
/// lines carry the kind of scope it ended with and the account (an id,
/// never a name).
pub async fn scope_requests(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let cell = Arc::new(Mutex::new(Tenancy::Anonymous));
    let response = TENANCY.scope(Arc::clone(&cell), next.run(request)).await;
    let ended = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let span = tracing::Span::current();
    span.record("tenancy", ended.kind());
    if let Tenancy::Member { account, .. } = ended {
        span.record("account_id", tracing::field::display(account));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_serialise_to_the_one_setting_row_rules_read() {
        let account = Uuid::parse_str("01930000-0000-7000-8000-000000000001").expect("uuid");
        let a = Uuid::parse_str("01930000-0000-7000-8000-00000000000a").expect("uuid");
        let b = Uuid::parse_str("01930000-0000-7000-8000-00000000000b").expect("uuid");
        assert_eq!(
            Tenancy::Member {
                account,
                workspaces: Some(vec![b, a, b]),
            }
            .setting(),
            format!("m:{account}|{a},{b}")
        );
        assert_eq!(
            Tenancy::Member {
                account,
                workspaces: Some(Vec::new()),
            }
            .setting(),
            format!("m:{account}|")
        );
        assert_eq!(
            Tenancy::Workspace {
                account: Some(account),
                workspace: a,
            }
            .setting(),
            format!("w:{account}|{a}")
        );
        assert_eq!(
            Tenancy::Workspace {
                account: None,
                workspace: a,
            }
            .setting(),
            format!("w:|{a}")
        );
        assert_eq!(Tenancy::System("jobs.claim").setting(), "s:jobs.claim");
        assert_eq!(Tenancy::Anonymous.setting(), "a");
        // Nothing that could close the quoted literal it travels in.
        for (reason, _) in SYSTEM_REASONS {
            assert!(
                reason
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "{reason}"
            );
        }
    }

    #[tokio::test]
    async fn signing_in_turns_only_an_anonymous_scope_into_a_member_one() {
        let account = Uuid::now_v7();
        assert_eq!(current(), None);
        signed_in(account);
        assert_eq!(current(), None, "no scope, nothing to change");
        let seen = scoped(Tenancy::Anonymous, async {
            signed_in(account);
            current()
        })
        .await;
        assert_eq!(
            seen,
            Some(Tenancy::Member {
                account,
                workspaces: None,
            })
        );
        let workspace = Uuid::now_v7();
        let job = Tenancy::Workspace {
            account: None,
            workspace,
        };
        let seen = scoped(job.clone(), async {
            signed_in(account);
            current()
        })
        .await;
        assert_eq!(seen, Some(job));
        let seen = system("jobs.claim", async { current() }).await;
        assert_eq!(seen, Some(Tenancy::System("jobs.claim")));
    }

    /// `system("reason", ..)` and `Tenancy::System("reason")` name a
    /// registered reason, in a file the reason lists.
    #[test]
    fn system_scopes_use_registered_reasons_in_their_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut folders = vec![root.clone()];
        let mut problems = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("read source folder") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut source = std::fs::read_to_string(&path).expect("read source file");
                if let Some(tests) = source.find("\n#[cfg(test)]\nmod tests") {
                    source.truncate(tests);
                }
                for opener in ["tenancy::system(\"", "Tenancy::System(\"", " system(\""] {
                    for (index, _) in source.match_indices(opener) {
                        let rest = &source[index + opener.len()..];
                        let reason = &rest[..rest.find('"').unwrap_or(0)];
                        match SYSTEM_REASONS.iter().find(|(known, _)| *known == reason) {
                            None => problems.push(format!("{relative}: unregistered {reason}")),
                            Some((_, files)) if !files.contains(&relative.as_str()) => problems
                                .push(format!("{relative}: {reason} isn't allowed here")),
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
