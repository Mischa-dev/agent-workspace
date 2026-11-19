//! Who is calling, through which edge, and what an operation's `run` sees.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{OpError, Target};
use crate::{AppState, policy::ActorKind};

/// The account making a call: K1's `access::Caller` (K1.2 replaced the
/// stopgap struct with this re-export).
pub use crate::access::Caller;

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
    /// [`super::Operation::ACROSS_WORKSPACES`].
    pub across_workspaces: bool,
    /// The call is a preview (`PREVIEW` operations): the dispatcher rolls
    /// its transaction back, and a wrapped command that writes on its own
    /// connection must only preview too.
    pub preview: bool,
}

/// One running call: the state, the context, and the transaction the
/// dispatcher began for it.
pub struct OpCall<'a> {
    pub state: &'a AppState,
    pub ctx: &'a OpContext,
    pub(super) tx: Option<Transaction<'static, Postgres>>,
    pub(super) audit: Option<AuditFacts>,
    pub(super) replayed: bool,
    pub(super) start_queued_runs: bool,
    /// An `Idempotency::Command` operation made this call retry-safe
    /// (`key_honoured`).
    pub(super) keyed: bool,
    /// The command wrote the write's audit row itself (K2.7's message
    /// commands keep theirs), so dispatch adds none.
    pub(super) audited: bool,
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

    /// The write queued a run: once it commits, the scheduler looks at the
    /// queue now rather than at its next pass.
    pub fn start_queued_runs(&mut self) {
        self.start_queued_runs = true;
    }

    /// For commands that keep their own idempotency (`Idempotency::Command`):
    /// the answer was a replay.
    pub fn mark_replayed(&mut self, replayed: bool) {
        self.replayed |= replayed;
    }

    /// For commands that keep their own idempotency (`Idempotency::Command`):
    /// a retry of this call changes nothing more, because the command
    /// deduplicated it (on the caller's request id, or on a key in the
    /// input such as `clientRunId`). Only then does the answer say
    /// `replaySafe: true` (ruling R52).
    pub fn key_honoured(&mut self) {
        self.keyed = true;
    }

    /// The command wrote this write's audit row itself (or changed
    /// nothing), so dispatch writes none: one row per write.
    pub fn audited_by_command(&mut self) {
        self.audited = true;
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
            confined_project: ctx.caller.confined_space,
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
            run_id: None,
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

impl From<&OpContext> for crate::events::Caller {
    /// Webhooks', rules' and forms' own caller, for an operation that makes
    /// them (a built module's parts, M1.6). Dispatch already checked the
    /// credential reaches the space and may write.
    fn from(ctx: &OpContext) -> Self {
        let (channel, authentication_class, client_name) = match &ctx.edge {
            Edge::Web { .. } => ("browser", "password_session", None),
            Edge::Mcp {
                authentication_class,
                client_name,
                ..
            } => ("mcp", *authentication_class, Some(client_name.clone())),
            Edge::PublicApi {
                authentication_class,
                client_name,
                ..
            } => ("api", *authentication_class, Some(client_name.clone())),
            Edge::Automation { .. } => ("automation", "automation", None),
            Edge::Job { .. } | Edge::Proposal { .. } => ("system", "background_job", None),
        };
        Self {
            actor_id: ctx.caller.account_id,
            actor_kind: ctx.caller.kind,
            session_id: match ctx.edge {
                Edge::Web { session_id } => Some(session_id),
                _ => None,
            },
            via_grant_id: ctx.caller.via_grant_id,
            external_agent_connection_id: ctx.caller.connection_id,
            token_project_id: ctx.caller.confined_space,
            can_write: ctx.caller.can_write,
            channel,
            authentication_class,
            client_name: client_name.map(|name| name.chars().take(80).collect()),
        }
    }
}

/// The stored key of an account kind (`actors.kind`).
pub fn actor_kind_key(kind: ActorKind) -> &'static str {
    crate::accounts::actor_kind_key(kind)
}

/// An account kind from its stored key (`accounts::actor_kind`).
pub fn actor_kind_of(key: &str) -> Option<ActorKind> {
    crate::accounts::actor_kind(key).ok()
}
