//! The run service (A1). It owns the run record every agent gets, whoever
//! runs it: `runs`, `run_activities`, `run_usage` and `usage_reports`
//! (`state` for where a run is, `store` for its rows). The built-in host
//! keeps its detail in `agent_runs` and writes its runs' records
//! (`builtin`) and steps (`steps`) itself.
//!
//! It also holds what the run, approval and GitHub job cores share between
//! the browser and MCP (docs/plans/mcp-tools.md, Phase 4): who is calling,
//! the built-in run states that count as active, and the per-person cap on
//! agent runs started through MCP.
//!
//! Whoever calls, the cores bind the execution profile to the caller (or the
//! sponsor of an approved run request); no tool ever takes a profile,
//! sponsor or worker argument.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthenticatedActor,
    error::AppError,
    jobs::JobRegistry,
    mcp::McpPrincipal,
    policy::ActorKind,
    work_item_commands::{AuditAttribution, CommandActor},
};

pub mod activities;
pub mod brief;
pub mod builtin;
pub mod credentials;
pub mod finished;
#[cfg(test)]
mod guards;
pub mod hosted;
pub mod launch_paths;
pub mod liveness;
pub mod operations;
pub mod ops;
pub mod pages;
pub mod reads;
pub mod say;
pub mod sql;
pub mod state;
pub mod steps;
pub mod store;
pub mod tool_ops;
pub mod usage;
pub mod views;

pub use state::{Liveness, RunState};

pub fn register_jobs(registry: &mut JobRegistry) {
    liveness::register_jobs(registry);
    usage::register_jobs(registry);
    say::register_jobs(registry);
}

/// Built-in run states (`agent_runs.state`) that hold (or wait for) harness time. One list for starting a
/// run, the cap, archiving a project, repository counts and run_list's
/// "active" filter. It matches `RunState::is_terminal` being false.
pub const ACTIVE_RUN_STATES: [&str; 6] = [
    "created",
    "queued",
    "starting",
    "working",
    "blocked",
    "recovering",
];

/// The most active runs one person may have when starting, retrying or
/// approving a run request through MCP. The web app has no cap.
pub const MCP_ACTIVE_RUN_CAP: i64 = 5;

/// The space a run is in, by the run's id: one row of `project_id`. For a
/// check that refuses a write before the handler looks (H1.24).
pub const SPACE_OF_RUN: &str = "SELECT conversation.project_id FROM agent_runs run \
     JOIN conversations conversation ON conversation.id = run.conversation_id \
     -- visibility: the space only, to refuse a write; the handler checks access\n \
     WHERE run.id = $1";

/// The runs in `workspace` that haven't finished, locked, with where each
/// lives: what suspending or deleting the workspace stops (H1.24).
pub(crate) async fn active_in_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
) -> Result<Vec<crate::hosts::builtin::http::RunToStop>, AppError> {
    sqlx::query_as::<_, crate::hosts::builtin::http::RunToStop>(
        r#"
        SELECT run.id AS run_id, conversation.project_id, run.conversation_id
        FROM agent_runs run
        JOIN conversations conversation ON conversation.id = run.conversation_id
        JOIN projects project ON project.id = conversation.project_id
        -- visibility: the workspace's own runs, stopped by the workspace's lifecycle
        WHERE project.organization_id = $1 AND run.state = ANY($2)
        ORDER BY run.id
        FOR UPDATE OF run
        "#,
    )
    .bind(workspace)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_all(&mut **transaction)
    .await
    .map_err(AppError::from)
}

/// Who is calling a shared run, approval or GitHub job core.
#[derive(Clone, Debug)]
pub struct RunCaller {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    /// The connected app a person acts through; `None` for the browser and
    /// for agents, who act as themselves.
    pub via_grant_id: Option<Uuid>,
    pub audit: AuditAttribution,
}

impl RunCaller {
    /// A signed-in browser session. `request_id` is the request's
    /// `x-request-id`, for the audit row.
    pub fn browser(
        actor: &AuthenticatedActor,
        request_id: Option<String>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            actor_id: actor.actor_id,
            actor_kind: crate::accounts::actor_kind(&actor.actor_kind)?,
            via_grant_id: None,
            audit: AuditAttribution::Browser {
                session_id: actor.session_id,
                request_id,
            },
        })
    }

    /// An MCP call. `request_id` goes in the audit log (the clientRequestId,
    /// already checked to fit there).
    pub fn mcp(principal: &McpPrincipal, request_id: String) -> Self {
        Self {
            actor_id: principal.actor_id,
            actor_kind: principal.actor_kind,
            via_grant_id: principal.via_grant_id(),
            audit: AuditAttribution::Mcp {
                authentication_class: principal.authentication_class,
                request_id,
                client_name: principal.client_name.clone(),
                mcp_credential_id: principal.mcp_credential_id,
                oauth_grant_id: principal.oauth_grant_id,
                external_agent_connection_id: principal.external_agent_connection_id,
            },
        }
    }

    /// The caller acting in one project, for task commands and
    /// `record_audit_for`.
    pub fn command_actor(&self, project_id: Uuid) -> CommandActor {
        CommandActor {
            actor_id: self.actor_id,
            actor_kind: self.actor_kind,
            project_id,
            audit: self.audit.clone(),
        }
    }

    pub fn is_mcp(&self) -> bool {
        matches!(self.audit, AuditAttribution::Mcp { .. })
    }
}

/// The person's active runs, counted under a lock that lasts until the
/// transaction ends, so two calls at the cap can't both start one. A run
/// counts against whoever's provider account it uses: the sponsor of an
/// approved run request, otherwise the person who started it.
#[derive(Clone, Debug)]
pub struct RunCapStatus {
    pub active: i64,
    /// Active runs the person can still see, newest first (at most the cap),
    /// for naming them. A run in a thread they've since lost counts but
    /// isn't named.
    pub visible_run_ids: Vec<Uuid>,
}

impl RunCapStatus {
    pub fn reached(&self) -> bool {
        self.active >= MCP_ACTIVE_RUN_CAP
    }
}

/// Call inside the transaction that inserts the run, after the idempotency
/// reservation (a replay never reaches the cap).
pub async fn run_cap(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
) -> Result<RunCapStatus, AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('run_cap:' || $1::text, 0))")
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    let active = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM agent_runs
        WHERE COALESCE(sponsoring_actor_id, requesting_actor_id) = $1
          AND state = ANY($2)
        "#,
    )
    .bind(actor_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .fetch_one(&mut **transaction)
    .await?;
    let visible_run_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM agent_runs
        WHERE COALESCE(sponsoring_actor_id, requesting_actor_id) = $1
          AND state = ANY($2)
          AND conversation_visible_to(conversation_id, $1)
        ORDER BY created_at DESC, id DESC
        LIMIT $3
        "#,
    )
    .bind(actor_id)
    .bind(&ACTIVE_RUN_STATES[..])
    .bind(MCP_ACTIVE_RUN_CAP)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(RunCapStatus {
        active,
        visible_run_ids,
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ACTIVE_RUN_STATES;
    use crate::domain::RunState;

    #[test]
    fn active_states_are_exactly_the_states_that_are_not_terminal() {
        let active: Vec<String> = RunState::ALL
            .into_iter()
            .filter(|state| !state.is_terminal())
            .map(|state| state.to_string())
            .collect();
        assert_eq!(active, ACTIVE_RUN_STATES);
        for state in ACTIVE_RUN_STATES {
            assert!(!RunState::from_str(state).expect("state").is_terminal());
        }
    }
}
