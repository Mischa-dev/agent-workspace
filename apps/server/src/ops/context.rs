//! Who is calling, through which edge, and what an operation's `run` sees.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{OpError, Target};
use crate::{
    AppState,
    policy::{ActorKind, AgentLimits},
};

/// The account making a call, as the edge resolved it. This is the stopgap
/// for K1's `access::Caller` (same fields and names); K1.2 replaces the
/// struct with a re-export.
#[derive(Clone, Debug)]
pub struct Caller {
    pub account_id: Uuid,
    /// For mechanics only (how it signs in, how it is told). Never a
    /// permission: the conventions guard refuses kind checks in `ops.rs`.
    pub kind: ActorKind,
    /// The connected app a person acts through, recorded on what they write.
    pub via_grant_id: Option<Uuid>,
    /// What the credential allows: reads, writes.
    pub can_read: bool,
    pub can_write: bool,
    /// An agent's connection, whose rules decide the spaces it reaches.
    pub connection_id: Option<Uuid>,
    /// An older space-scoped credential that stays in one space.
    pub confined_space: Option<Uuid>,
    /// What an app acting for a person may do (their agent limit).
    pub limits: Option<AgentLimits>,
    /// The account a call is made for (a run's requester), when not the
    /// caller's own.
    pub requested_by: Option<Uuid>,
    pub run_id: Option<Uuid>,
}

impl Caller {
    /// A person in the web app: full access as themselves.
    pub fn person(account_id: Uuid, kind: ActorKind) -> Self {
        Self {
            account_id,
            kind,
            via_grant_id: None,
            can_read: true,
            can_write: true,
            connection_id: None,
            confined_space: None,
            limits: None,
            requested_by: None,
            run_id: None,
        }
    }

    /// The caller behind an MCP or public API credential.
    pub fn of_principal(principal: &crate::mcp::McpPrincipal, limits: Option<AgentLimits>) -> Self {
        Self {
            account_id: principal.actor_id,
            kind: principal.actor_kind,
            via_grant_id: principal.via_grant_id(),
            can_read: principal.can_read(),
            can_write: principal.can_write(),
            connection_id: principal.external_agent_connection_id,
            // kind-check: mechanical (an older person's credential is tied
            // to the one project it was made in; agents reach by connection)
            confined_space: (principal.actor_kind == ActorKind::Human && !principal.account_wide)
                .then_some(principal.project_id),
            limits,
            requested_by: None,
            run_id: None,
        }
    }
}

/// The edge a call arrived through: recorded on the audit row.
#[derive(Clone, Debug)]
pub enum Edge {
    Web {
        session_id: Uuid,
    },
    Mcp {
        credential_id: Option<Uuid>,
        oauth_grant_id: Option<Uuid>,
        connection_id: Option<Uuid>,
        client_name: String,
        authentication_class: &'static str,
    },
    PublicApi {
        credential_id: Option<Uuid>,
        oauth_grant_id: Option<Uuid>,
        connection_id: Option<Uuid>,
        client_name: String,
        authentication_class: &'static str,
    },
    Automation {
        rule_id: Uuid,
        run_id: Uuid,
    },
    Job {
        job_id: Uuid,
        source: &'static str,
    },
    Proposal {
        proposal_id: Uuid,
        accepted_by: Uuid,
    },
}

impl Edge {
    pub fn of_mcp(principal: &crate::mcp::McpPrincipal) -> Self {
        Self::Mcp {
            credential_id: principal.mcp_credential_id,
            oauth_grant_id: principal.oauth_grant_id,
            connection_id: principal.external_agent_connection_id,
            client_name: principal.client_name.clone(),
            authentication_class: principal.authentication_class,
        }
    }

    pub fn of_public_api(principal: &crate::mcp::McpPrincipal) -> Self {
        Self::PublicApi {
            credential_id: principal.mcp_credential_id,
            oauth_grant_id: principal.oauth_grant_id,
            connection_id: principal.external_agent_connection_id,
            client_name: principal.client_name.clone(),
            authentication_class: principal.authentication_class,
        }
    }

    /// The edge's short name: `web`, `mcp`, `api`, `automation`, `job`,
    /// `proposal`.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Web { .. } => "web",
            Self::Mcp { .. } => "mcp",
            Self::PublicApi { .. } => "api",
            Self::Automation { .. } => "automation",
            Self::Job { .. } => "job",
            Self::Proposal { .. } => "proposal",
        }
    }
}

/// What `run` sees. Built by `dispatch`; the target in it has been located
/// and checked.
#[derive(Clone, Debug)]
pub struct OpContext {
    pub caller: Caller,
    pub edge: Edge,
    /// `x-request-id`, for logs.
    pub trace_id: Option<String>,
    pub workspace_id: Uuid,
    /// Today's project id.
    pub space_id: Option<Uuid>,
    pub target: Target,
    /// Set for every write.
    pub request_id: Option<String>,
    pub op: &'static str,
}

/// One running call: the state, the context, and the transaction the
/// dispatcher began for it.
pub struct OpCall<'a> {
    pub state: &'a AppState,
    pub ctx: &'a OpContext,
    pub(super) tx: Option<Transaction<'static, Postgres>>,
    pub(super) audit: Option<AuditFacts>,
    pub(super) replayed: bool,
}

/// What the audit row says the write was about, when not the target.
#[derive(Clone, Debug)]
pub struct AuditFacts {
    pub kind: &'static str,
    pub id: Uuid,
    pub metadata: serde_json::Value,
}

impl OpCall<'_> {
    /// The call's transaction, begun on first use with the tenancy scope
    /// set. Writes the dispatcher reserved a request id for already have it.
    pub async fn tx(&mut self) -> Result<&mut Transaction<'static, Postgres>, OpError> {
        if self.tx.is_none() {
            self.tx = Some(super::dispatch::begin(self.state, self.ctx).await?);
        }
        self.tx.as_mut().ok_or(OpError::Unavailable)
    }

    /// Say what the audit row is about (the thing made, for a create).
    pub fn audit_target(&mut self, kind: &'static str, id: Uuid, metadata: serde_json::Value) {
        self.audit = Some(AuditFacts { kind, id, metadata });
    }

    /// For commands that keep their own idempotency (`Idempotency::Command`):
    /// the answer was a replay.
    pub fn mark_replayed(&mut self, replayed: bool) {
        self.replayed |= replayed;
    }

    /// The account the call acts as.
    pub fn account_id(&self) -> Uuid {
        self.ctx.caller.account_id
    }
}

// Adapters, so wrapped domain functions need no change in the first step.

impl From<&OpContext> for crate::memory::Writer {
    fn from(ctx: &OpContext) -> Self {
        Self {
            actor_id: ctx.caller.account_id,
            actor_kind: ctx.caller.kind,
            via_grant_id: ctx.caller.via_grant_id,
        }
    }
}

impl From<&OpContext> for crate::whiteboards::Caller {
    fn from(ctx: &OpContext) -> Self {
        Self {
            actor_id: ctx.caller.account_id,
            actor_kind: ctx.caller.kind,
            via_grant_id: ctx.caller.via_grant_id,
            connection_id: ctx.caller.connection_id,
            confined_project: ctx.caller.confined_space,
            channel: match ctx.edge {
                Edge::Web { .. } => "browser",
                _ => "mcp",
            },
            authentication_class: match ctx.edge {
                Edge::Web { .. } => "password_session",
                Edge::Mcp {
                    authentication_class,
                    ..
                }
                | Edge::PublicApi {
                    authentication_class,
                    ..
                } => authentication_class,
                _ => "background_job",
            },
            session_id: match ctx.edge {
                Edge::Web { session_id } => Some(session_id),
                _ => None,
            },
        }
    }
}

impl From<&OpContext> for crate::conversations::MessageCommandActor {
    fn from(ctx: &OpContext) -> Self {
        Self {
            actor_id: ctx.caller.account_id,
            actor_kind: actor_kind_key(ctx.caller.kind).to_owned(),
            mcp_credential: match (&ctx.edge, ctx.caller.connection_id) {
                (
                    Edge::Mcp {
                        credential_id: Some(credential_id),
                        ..
                    }
                    | Edge::PublicApi {
                        credential_id: Some(credential_id),
                        ..
                    },
                    Some(connection_id),
                ) => Some(crate::conversations::MessageMcpCredential {
                    credential_id: *credential_id,
                    connection_id,
                }),
                _ => None,
            },
            via_grant_id: ctx.caller.via_grant_id,
        }
    }
}

impl From<&OpContext> for crate::work_item_commands::CommandActor {
    fn from(ctx: &OpContext) -> Self {
        use crate::work_item_commands::AuditAttribution;
        Self {
            actor_id: ctx.caller.account_id,
            actor_kind: ctx.caller.kind,
            project_id: ctx.space_id.unwrap_or_default(),
            audit: match &ctx.edge {
                Edge::Web { session_id } => AuditAttribution::Browser {
                    session_id: *session_id,
                    request_id: ctx.request_id.clone(),
                },
                Edge::Mcp {
                    credential_id,
                    oauth_grant_id,
                    connection_id,
                    client_name,
                    authentication_class,
                } => AuditAttribution::Mcp {
                    authentication_class,
                    request_id: ctx.request_id.clone().unwrap_or_default(),
                    client_name: client_name.clone(),
                    mcp_credential_id: *credential_id,
                    oauth_grant_id: *oauth_grant_id,
                    external_agent_connection_id: *connection_id,
                },
                Edge::PublicApi {
                    credential_id,
                    oauth_grant_id,
                    connection_id,
                    client_name,
                    authentication_class,
                } => AuditAttribution::PublicApi {
                    authentication_class,
                    request_id: ctx.request_id.clone().unwrap_or_default(),
                    client_name: client_name.clone(),
                    mcp_credential_id: *credential_id,
                    oauth_grant_id: *oauth_grant_id,
                    external_agent_connection_id: *connection_id,
                },
                Edge::Automation { rule_id, run_id } => AuditAttribution::Automation {
                    rule_id: *rule_id,
                    run_id: *run_id,
                },
                Edge::Job { job_id, source } => AuditAttribution::Job {
                    job_id: *job_id,
                    source,
                },
                Edge::Proposal {
                    proposal_id,
                    accepted_by,
                } => AuditAttribution::Proposal {
                    proposal_id: *proposal_id,
                    accepted_by: *accepted_by,
                },
            },
        }
    }
}

/// The stored key of an account kind (`actors.kind`).
pub fn actor_kind_key(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Human => "human",
        ActorKind::ExternalAgent => "external_agent",
        ActorKind::ExecutionWorker => "execution_worker",
        ActorKind::Integration => "integration",
        ActorKind::System => "system",
    }
}

/// An account kind from its stored key.
pub fn actor_kind_of(key: &str) -> Option<ActorKind> {
    [
        ActorKind::Human,
        ActorKind::ExternalAgent,
        ActorKind::ExecutionWorker,
        ActorKind::Integration,
        ActorKind::System,
    ]
    .into_iter()
    .find(|kind| actor_kind_key(*kind) == key)
}
