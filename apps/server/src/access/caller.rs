//! Who is calling, resolved server-side at the edge (never from client
//! input). Moved here from `ops::context` by K1.2; `ops::Caller` re-exports
//! it.

use uuid::Uuid;

use crate::policy::{ActorKind, AgentLimits};

/// The account making a call, as the edge resolved it.
#[derive(Clone, Debug)]
pub struct Caller {
    pub account_id: Uuid,
    /// For mechanics only (how it signs in, how it is told). Never a
    /// permission: `access` never reads it to decide.
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
    /// What an app acting for this account may do (its delegation limit).
    pub limits: Option<AgentLimits>,
    /// The account a call is made for (a run's requester), when not the
    /// caller's own.
    pub requested_by: Option<Uuid>,
    pub run_id: Option<Uuid>,
}

/// The credential a call came through, as far as a decision cares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Via {
    /// The browser, or the server acting as the account itself.
    Session,
    /// A long-lived secret or an agent's own connection.
    Secret { connection_id: Option<Uuid> },
    /// An app acting for the account, held to a delegation limit.
    Delegation { grant_id: Option<Uuid> },
}

/// Which workspaces and spaces the credential reaches (ruling R19).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reach {
    /// Every workspace the account is a member of.
    AllMemberships,
    /// One space, from an older credential made in it.
    Space(Uuid),
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

    /// The caller behind an MCP or public API credential, with the
    /// delegation limit the edge loaded for it.
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

    /// The credential this call came through.
    pub fn via(&self) -> Via {
        if self.limits.is_some() || self.via_grant_id.is_some() {
            Via::Delegation {
                grant_id: self.via_grant_id,
            }
        } else if self.connection_id.is_some() {
            Via::Secret {
                connection_id: self.connection_id,
            }
        } else {
            Via::Session
        }
    }

    /// Where the credential reaches.
    pub fn reach(&self) -> Reach {
        match self.confined_space {
            Some(space) => Reach::Space(space),
            None => Reach::AllMemberships,
        }
    }
}
