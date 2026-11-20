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

pub mod backfill;
#[cfg(test)]
mod ratchet;
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
    // A credential presented before its account is known (an OAuth code,
    // refresh or access token, a client or run credential): the row its
    // digest names says whose it is, and the lookup checks that account's
    // standing before the request takes the account's scope.
    ("auth.credential", &["mcp.rs", "mcp/oauth.rs"]),
    // A sign-in revoking the session its cookie still names: the revoke
    // stages `session.revoked` in a workspace of that session's account,
    // which the anonymous sign-in can't see (`widen_to_system`, one
    // statement keyed by the cookie's digest).
    ("auth.rotate", &["auth/mod.rs"]),
    // The built-in host's scheduler, recovery and host jobs: they claim runs
    // of every workspace, and check each against its run's space.
    (
        "runs.host",
        &[
            "server.rs",
            "scheduler.rs",
            "recovery.rs",
            "launch.rs",
            "runs/liveness.rs",
            "runs/usage.rs",
            "harness_host.rs",
            "provider.rs",
            "hosts/commands.rs",
            "hosts/a2a.rs",
        ],
    ),
    // GitHub's own job queue, claimed across workspaces like the job queue.
    ("github.jobs", &["server.rs", "github.rs"]),
    // The reply worker: expires reply leases and marks unresponsive agents
    // in every workspace.
    ("replies.sweep", &["server.rs", "replies.rs"]),
    // Reloading every workspace's registered harnesses on each instance.
    ("harnesses.reload", &["harnesses.rs"]),
    // Commands an operator runs on the host, and the server's own start:
    // preflight before migrating, a password or master key rotation, the
    // default plan.
    (
        "server.operator",
        &[
            "bootstrap.rs",
            "migration_preflight.rs",
            "secrets.rs",
            "metering/mod.rs",
            "mcp.rs",
            "main.rs",
        ],
    ),
    // A live connection outlives any one request's list of workspaces: its
    // reads check each frame against its account (`conversation_visible_to`,
    // a document's own check), like the tail that feeds it.
    (
        "realtime.connections",
        &[
            "realtime/hub.rs",
            "realtime/contract/live.rs",
            "realtime/follow.rs",
            "realtime/ephemeral.rs",
        ],
    ),
    // A whole-server backup or restore: every workspace's rows.
    ("backup", &["backup.rs"]),
    // First run (H1.14): the setup code, and the first account and
    // workspace, made before anyone has a scope (in the browser, or by the
    // bootstrap commands on the host).
    ("server.first_run", &["setup.rs", "bootstrap.rs", "seed.rs"]),
    // A billing provider's delivery (H1.33): the workspace is known only
    // once the provider names it.
    ("billing.webhook", &["billing/mod.rs"]),
    // A signed GitHub delivery: its installation, and so its workspace, is
    // known only once it is read.
    ("github.webhook", &["github.rs"]),
    // An OpenID Connect sign-in (H1.18): accounts are server-wide, and the
    // workspace a verified domain joins is known only once it is read.
    ("auth.oidc", &["auth/oidc.rs"]),
    // Claiming due jobs of every workspace, and finishing them.
    ("jobs.claim", &["jobs.rs"]),
    // Jobs queued for no workspace (the server's own recurring work).
    ("jobs.server", &["jobs.rs"]),
    // Pruning finished jobs and counting the queue for the Server page.
    ("jobs.queue", &["jobs.rs"]),
    // An import (H1.10) checks the archive's ids against every workspace,
    // and fills a workspace that has no members' scope yet.
    ("workspace.import", &["export/import.rs"]),
    // Taking, renewing and releasing a server-wide maintenance lease.
    ("maintenance.lease", &["maintenance_lease.rs"]),
    // The event log's own loops: the sequencer, the tail that feeds
    // sockets and the ordered consumers, which read every workspace's
    // events (each reader filters per socket or per subscription).
    (
        "events.log",
        &[
            "events/sequencer.rs",
            "realtime/tail.rs",
            "events/consumer.rs",
            "events/upkeep.rs",
            "server.rs",
        ],
    ),
    // Retention: old deliveries, worktrees and usage windows of every
    // workspace.
    ("retention", &["retention.rs"]),
    // Requests of every workspace that ran out of time.
    ("requests.expire", &["requests/mod.rs"]),
    // Memberships of every workspace whose time has come.
    ("memberships.expire", &["memberships/expiry.rs"]),
    // Statuses of every account whose time to clear has come.
    ("statuses.clear", &["statuses.rs"]),
    // Files nothing holds any more, unfinished uploads and old link
    // previews, in every workspace.
    (
        "files.sweep",
        &["files.rs", "files/upload.rs", "link_previews.rs"],
    ),
    // The server's own disk watermark: the bytes every workspace's files
    // hold together (a sum, never a row), in the caller's own transaction
    // (`widen_to_system`).
    ("files.quota", &["files.rs"]),
    // A link someone was given (an invitation, an account claim, a setup
    // link, a public form or report link, a password reset or email
    // check): its token names the one row it opens, and the handler acts
    // only on what that row says.
    (
        "auth.link",
        &[
            "workspace.rs",
            "invitations.rs",
            "accounts/claims.rs",
            "password_reset.rs",
            "bug_reports.rs",
            "agent_setup_links.rs",
            "spaces/invite_links.rs",
            "github/connect.rs",
            "push/receipts.rs",
            "push/actions.rs",
        ],
    ),
    // A signed incoming-hook delivery: its hook, and so its workspace, is
    // known only once it is read.
    ("hooks.incoming", &["incoming_hooks/mod.rs"]),
    // The module gate: which workspace or space a route acts in, and that
    // workspace's switches, read before the caller is known (only a caller
    // who can see the place learns that a module is off there).
    ("modules.gate", &["modules/gate.rs"]),
    // A host's own calls (A1): it carries out commands for its agents'
    // runs in every workspace they work in, and each route checks the
    // command or run against the host it was written for.
    ("hosts.protocol", &["hosts/routes.rs"]),
    // Moving files between stores: a whole server's, or one workspace's
    // into its own bucket.
    ("files.move", &["files/migrate.rs"]),
    // Redeeming a connected module's one-time install code (M1.12), before
    // the app's workspace is known: one lookup by the code's digest.
    ("modules.install_code", &["modules/connected/mod.rs"]),
    // An event a connected module sends in (M1.13), before its workspace
    // is known: one lookup of the install by its id.
    ("modules.events_in", &["modules/connected/http.rs"]),
    // A connected module redeeming a frame's launch token (M1.15), before
    // its workspace is known: one lookup of the install by its id.
    ("modules.frame_redeem", &["modules/connected/http.rs"]),
    // Whether an agent is also in another workspace (A2.15's scope screen
    // says yes or no, never which, R19): one count across workspaces the
    // viewer may not be in.
    ("agents.scope", &["external_agents/scope.rs"]),
    // An operator suspending or resuming a workspace from the command line
    // (H1.24), before any account is known.
    ("workspace.operator", &["workspace/lifecycle.rs"]),
    // Deleting a workspace: other workspaces' rows that point at its rows
    // let go of them (a request raised through its host), in the deletion's
    // own transaction (`widen_to_system`).
    ("workspace.delete", &["workspace/lifecycle.rs"]),
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
                account
                    .map(|account| account.to_string())
                    .unwrap_or_default()
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

/// One task's scope: task-local (each request or job has its own), so it is
/// not process state; the lock lets authentication narrow it mid-request.
type TaskScope = Arc<Mutex<Tenancy>>;

tokio::task_local! {
    static TENANCY: TaskScope;
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
    signed_in_as(Tenancy::Member {
        account,
        workspaces: None,
    });
}

/// Authentication found a caller whose scope isn't an account's
/// workspaces: a run's credential (its run's workspace, as its agent) or a
/// host (server-wide, under a registered reason). Like [`signed_in`], it
/// changes only an anonymous scope.
pub fn signed_in_as(scope: Tenancy) {
    if let Tenancy::System(reason) = &scope {
        debug_assert!(
            SYSTEM_REASONS.iter().any(|(known, _)| known == reason),
            "unregistered tenancy reason {reason}"
        );
    }
    let _ = TENANCY.try_with(|cell| {
        let mut tenancy = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *tenancy == Tenancy::Anonymous {
            *tenancy = scope;
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

/// Widens the transaction in hand, and the rest of the work in hand, to a
/// workspace it is making (for `owner`), so the new workspace's own rows
/// (its roles, plan and the owner's membership) are inside the scope as
/// they go in: a member's scope gains it, a workspace or anonymous scope
/// becomes the owner's member scope over both, and a server-wide one stays.
pub async fn widen_to_new_workspace(
    connection: &mut sqlx::PgConnection,
    workspace: Uuid,
    owner: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        SELECT set_config('app.tenancy', CASE split_part(current, ':', 1)
            WHEN 's' THEN current
            WHEN 'm' THEN current
                || CASE WHEN split_part(current, '|', 2) = '' THEN '' ELSE ',' END
                || $1::text
            WHEN 'w' THEN 'm:' || $2::text || '|' || split_part(current, '|', 2) || ',' || $1::text
            ELSE 'm:' || $2::text || '|' || $1::text
        END, true)
        FROM (SELECT coalesce(current_setting('app.tenancy', true), '') AS current) setting
        "#,
    )
    .bind(workspace)
    .bind(owner)
    .execute(&mut *connection)
    .await?;
    let _ = TENANCY.try_with(|cell| {
        let mut tenancy = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Tenancy::Member {
            workspaces: Some(workspaces),
            ..
        } = &mut *tenancy
            && !workspaces.contains(&workspace)
        {
            workspaces.push(workspace);
        }
    });
    Ok(())
}

/// Turns the transaction in hand into server-wide work under `reason` (one
/// of [`SYSTEM_REASONS`], registered for the calling file) until
/// [`restore`] puts back the scope this returns: for the one step of a
/// workspace's own work that must reach other workspaces' rows in the same
/// transaction, such as a deletion letting go of their references to it.
pub async fn widen_to_system(
    connection: &mut sqlx::PgConnection,
    reason: &'static str,
) -> Result<String, sqlx::Error> {
    debug_assert!(
        SYSTEM_REASONS.iter().any(|(known, _)| *known == reason),
        "unregistered tenancy reason {reason}"
    );
    let previous = sqlx::query_scalar::<_, String>(
        "SELECT coalesce(current_setting('app.tenancy', true), '')",
    )
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query("SELECT set_config('app.tenancy', $1, true)")
        .bind(Tenancy::System(reason).setting())
        .execute(&mut *connection)
        .await?;
    Ok(previous)
}

/// Puts back the scope [`widen_to_system`] returned.
pub async fn restore(
    connection: &mut sqlx::PgConnection,
    previous: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.tenancy', $1, true)")
        .bind(previous)
        .execute(&mut *connection)
        .await?;
    Ok(())
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
                for opener in [
                    "tenancy::system(\"",
                    "Tenancy::System(\"",
                    " system(\"",
                    "widen_to_system(&mut *connection, \"",
                ] {
                    for (index, _) in source.match_indices(opener) {
                        let rest = &source[index + opener.len()..];
                        let reason = &rest[..rest.find('"').unwrap_or(0)];
                        match SYSTEM_REASONS.iter().find(|(known, _)| *known == reason) {
                            None => problems.push(format!("{relative}: unregistered {reason}")),
                            Some((_, files)) if !files.contains(&relative.as_str()) => {
                                problems.push(format!("{relative}: {reason} isn't allowed here"))
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
