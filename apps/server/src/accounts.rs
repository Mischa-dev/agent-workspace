//! Accounts (K1): every person, agent and app is one kind of thing, an
//! account, global to the server and joining workspaces by membership
//! (ruling R19). This module is where an account's kind is read and where
//! the mechanics that differ by kind live (how it signs in, how it is
//! told). Nothing here answers "may it": that is `access`.
//!
//! K1.5 adds `actors.account_kind` (kept in step with the older `kind` by a
//! trigger while old code still writes `kind`), the owner columns, and the
//! one parser that replaces the copies each module had.

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{error::AppError, policy::ActorKind};

/// The four kinds of account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Person,
    Agent,
    App,
    System,
}

impl AccountKind {
    /// The one parser: accepts the stored `account_kind` values and the
    /// older `kind` ones.
    pub fn parse(stored: &str) -> Result<Self, AppError> {
        match stored {
            "person" | "human" => Ok(Self::Person),
            "agent" | "external_agent" | "execution_worker" => Ok(Self::Agent),
            "app" | "integration" => Ok(Self::App),
            "system" => Ok(Self::System),
            _ => Err(AppError::Internal("stored account kind")),
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Agent => "agent",
            Self::App => "app",
            Self::System => "system",
        }
    }

    pub const fn of(kind: ActorKind) -> Self {
        match kind {
            ActorKind::Human => Self::Person,
            ActorKind::ExternalAgent | ActorKind::ExecutionWorker => Self::Agent,
            ActorKind::Integration => Self::App,
            ActorKind::System => Self::System,
        }
    }

    /// Whether accounts of this kind have an owner (an account or a
    /// workspace). Structural, not a permission.
    pub const fn has_owner(self) -> bool {
        matches!(self, Self::Agent | Self::App)
    }
}

/// The older five-way kind, parsed once here from its stored value
/// (`actors.kind`). Every module that read it with its own `match` calls
/// this instead.
pub fn actor_kind(stored: &str) -> Result<ActorKind, AppError> {
    match stored {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

/// The stored value of the older kind.
pub const fn actor_kind_key(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Human => "human",
        ActorKind::ExternalAgent => "external_agent",
        ActorKind::ExecutionWorker => "execution_worker",
        ActorKind::Integration => "integration",
        ActorKind::System => "system",
    }
}

/// Who an agent or app account belongs to (ruling R19, R45): an account
/// (any kind, the chain ending at a person or a workspace) or a workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    Account(Uuid),
    Workspace(Uuid),
}

/// An account as `access` and the mechanics read it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Account {
    pub id: Uuid,
    pub kind: AccountKind,
    pub active: bool,
    /// Some for agents and apps only.
    pub owner: Option<Owner>,
}

impl Account {
    /// People get inbox cards and push. A mechanic, never a permission.
    pub const fn has_inbox(&self) -> bool {
        matches!(self.kind, AccountKind::Person)
    }

    /// Agents get events through deliveries. A mechanic, never a permission.
    pub const fn receives_deliveries(&self) -> bool {
        matches!(self.kind, AccountKind::Agent)
    }
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: Uuid,
    account_kind: String,
    status: String,
    owner_account_id: Option<Uuid>,
    owner_workspace_id: Option<Uuid>,
}

/// Loads one account.
pub async fn load(connection: &mut PgConnection, id: Uuid) -> Result<Option<Account>, AppError> {
    let Some(row) = sqlx::query_as::<_, AccountRow>(
        "SELECT id, account_kind, status, owner_account_id, owner_workspace_id \
         FROM actors WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(Account {
        id: row.id,
        kind: AccountKind::parse(&row.account_kind)?,
        active: row.status == "active",
        owner: match (row.owner_account_id, row.owner_workspace_id) {
            (Some(account), _) => Some(Owner::Account(account)),
            (None, Some(workspace)) => Some(Owner::Workspace(workspace)),
            (None, None) => None,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_new_values_parse_to_one_kind() {
        for (stored, kind) in [
            ("human", AccountKind::Person),
            ("person", AccountKind::Person),
            ("external_agent", AccountKind::Agent),
            ("execution_worker", AccountKind::Agent),
            ("agent", AccountKind::Agent),
            ("integration", AccountKind::App),
            ("app", AccountKind::App),
            ("system", AccountKind::System),
        ] {
            assert_eq!(AccountKind::parse(stored).expect(stored), kind);
        }
        assert!(AccountKind::parse("robot").is_err());
        for kind in [
            ActorKind::Human,
            ActorKind::ExternalAgent,
            ActorKind::ExecutionWorker,
            ActorKind::Integration,
            ActorKind::System,
        ] {
            assert_eq!(actor_kind(actor_kind_key(kind)).expect("kind"), kind);
            assert_eq!(
                AccountKind::parse(actor_kind_key(kind)).expect("kind"),
                AccountKind::of(kind)
            );
        }
    }
}
