//! A1.11: hosts and enrolment (A1 section 3.9). A host is a system that runs
//! agents. It signs in as its own `integration` account and acts only for
//! the agent accounts people bound to it (`host_agents`); it never chooses
//! which account it acts as.
//!
//! - People: link a host with a one-time code (`create_enrolment`), bind and
//!   unbind its agents, and unlink it. A person may own a host ("my
//!   machine"); a holder of `hosts.manage` links one for a workspace.
//! - Hosts (`protocol`): enrol with the code, describe themselves, send
//!   heartbeats and the three notices.
//!
//! Until K1.22/K1.23 issue app credentials, enrolment returns an interim
//! bearer token (`awh_…`), kept only as a digest; K1 migrates it. Host
//! events (`host.*`) wait for K3.3's event log; until then the notices are
//! kept in `host_notices`.

use chrono::{DateTime, Duration, Utc};
use rand_core::{OsRng, RngCore};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    policy::{ActorKind, Permission},
    roles,
};

pub mod protocol;
pub mod routes;

/// The protocol version this server speaks, and every one it still serves.
pub const PROTOCOL: &str = "2026-11";
pub const PROTOCOLS: &[&str] = &[PROTOCOL];
/// How long an enrolment code works.
pub const CODE_MINUTES: i64 = 15;
/// Unused codes one person may have at once.
const MAX_OPEN_CODES: i64 = 10;
const CODE_PREFIX: &str = "AWH";
/// Crockford's base32: no I, L, O or U.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub(crate) const TOKEN_PREFIX: &str = "awh_";
pub(crate) const TOKEN_PREFIX_LENGTH: usize = 12;
const MAX_LABEL_CHARS: usize = 80;

/// Who owns a host: a person, or a workspace (R19).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    Account(Uuid),
    Workspace(Uuid),
}

#[derive(Clone, Debug, FromRow)]
pub struct HostRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub owner_account_id: Option<Uuid>,
    pub owner_workspace_id: Option<Uuid>,
    pub name: String,
    pub software: Option<String>,
    pub software_version: Option<String>,
    pub protocol_version: String,
    pub capabilities: Value,
    pub offers: Value,
    pub actions: Value,
    pub described_agents: Value,
    pub issuer: Option<String>,
    pub module_manifest_url: Option<String>,
    pub connection: String,
    pub status: String,
    pub builtin: bool,
    pub report_interval_seconds: i32,
    pub capacity: Value,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub enrolled_by_account_id: Uuid,
    pub revoked_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const HOST_COLUMNS: &str = r#"
    host.id, host.account_id, host.owner_account_id, host.owner_workspace_id, host.name,
    host.software, host.software_version, host.protocol_version, host.capabilities,
    host.offers, host.actions, host.described_agents, host.issuer, host.module_manifest_url,
    host.connection, host.status, host.builtin, host.report_interval_seconds, host.capacity,
    host.last_seen_at, host.enrolled_by_account_id, host.revoked_at, host.version,
    host.created_at, host.updated_at
"#;

impl HostRow {
    pub fn owner(&self) -> Owner {
        match (self.owner_account_id, self.owner_workspace_id) {
            (Some(account), _) => Owner::Account(account),
            (None, Some(workspace)) => Owner::Workspace(workspace),
            // The table's check makes one of them set.
            (None, None) => Owner::Account(self.enrolled_by_account_id),
        }
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct HostAgentRow {
    pub host_id: Uuid,
    pub account_id: Uuid,
    pub host_agent_ref: String,
    pub lineage: Option<String>,
    pub backends: Value,
    pub actions: Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const HOST_AGENT_COLUMNS: &str = r#"
    binding.host_id, binding.account_id, binding.host_agent_ref, binding.lineage,
    binding.backends, binding.actions, binding.status, binding.created_at, binding.updated_at
"#;

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OwnerView {
    /// person or workspace.
    pub kind: String,
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostAgentView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub name: Option<String>,
    pub host_agent_ref: String,
    /// active, paused or gone.
    pub status: String,
    #[schemars(with = "Vec<serde_json::Value>")]
    pub backends: Value,
    #[schemars(with = "Vec<serde_json::Value>")]
    pub actions: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    pub owner: OwnerView,
    /// This server's own runner: it can't be unlinked or given agents here.
    pub builtin: bool,
    /// active, offline or revoked.
    pub status: String,
    pub software: Option<String>,
    pub software_version: Option<String>,
    pub protocol: String,
    pub connection: String,
    #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
    pub capabilities: Value,
    /// Ways to get a new agent, in the host's own words.
    #[schemars(with = "Vec<serde_json::Value>")]
    pub offers: Value,
    #[schemars(with = "Vec<serde_json::Value>")]
    pub actions: Value,
    /// The agents the host last described, bound or not.
    #[schemars(with = "Vec<serde_json::Value>")]
    pub described_agents: Value,
    /// The agent accounts it may act for.
    pub agents: Vec<HostAgentView>,
    pub module_manifest_url: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Whether the caller may bind agents to it and unlink it.
    pub can_manage: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnrolmentCode {
    #[schemars(with = "String")]
    pub enrolment_id: Uuid,
    /// Shown once: give it to the host's own enrol step.
    pub code: String,
    pub expires_at: DateTime<Utc>,
    pub owner: OwnerView,
}

// ---------------------------------------------------------------------------
// Codes and tokens

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// A new code: `AWH-` and 80 random bits in four groups of base32.
fn new_code() -> String {
    let mut bytes = [0_u8; 10];
    OsRng.fill_bytes(&mut bytes);
    let mut bits = 0_u128;
    for byte in bytes {
        bits = (bits << 8) | u128::from(byte);
    }
    let characters = (0..16)
        .rev()
        .map(|index| char::from(CODE_ALPHABET[((bits >> (index * 5)) & 31) as usize]))
        .collect::<Vec<_>>();
    let groups = characters
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>();
    format!("{CODE_PREFIX}-{}", groups.join("-"))
}

/// The code as it was issued, from what someone typed: any case, spaces or
/// dashes anywhere, and the letters people mistake for digits.
pub(crate) fn normalize_code(typed: &str) -> Option<String> {
    let cleaned = typed
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .map(|character| match character.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect::<String>();
    let body = cleaned.strip_prefix(CODE_PREFIX).unwrap_or(&cleaned);
    // "AWH" itself survives the mapping above unchanged.
    if body.len() != 16 || !body.bytes().all(|byte| CODE_ALPHABET.contains(&byte)) {
        return None;
    }
    let groups = body
        .as_bytes()
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>();
    Some(format!("{CODE_PREFIX}-{}", groups.join("-")))
}

/// A new interim host token, its lookup prefix and its digest.
pub(crate) fn new_token() -> (String, String, [u8; 32]) {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let prefix = token[..TOKEN_PREFIX_LENGTH].to_owned();
    let token_digest = digest(&token);
    (token, prefix, token_digest)
}

// ---------------------------------------------------------------------------
// Who may do what

/// Whether `caller` manages `host`: owns it, or holds `hosts.manage` in the
/// workspace that owns it (R34). The built-in host is managed by the server.
pub async fn may_manage(
    connection: &mut PgConnection,
    caller: Uuid,
    host: &HostRow,
) -> Result<bool, AppError> {
    if host.builtin {
        return Ok(false);
    }
    Ok(match host.owner() {
        Owner::Account(owner) => owner == caller,
        Owner::Workspace(workspace) => holds_hosts_manage(connection, caller, workspace).await?,
    })
}

async fn holds_hosts_manage(
    connection: &mut PgConnection,
    caller: Uuid,
    workspace: Uuid,
) -> Result<bool, AppError> {
    match roles::standing(connection, workspace, caller).await {
        Ok(standing) => Ok(standing.may(Permission::HostsManage)),
        Err(AppError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether `caller` may hand agent `account_id` to `host`: the account is
/// an agent, and the caller holds `agents.manage` in a workspace where it is
/// a member (for a workspace's host, that workspace).
async fn may_manage_agent(
    connection: &mut PgConnection,
    caller: Uuid,
    host: &HostRow,
    account_id: Uuid,
) -> Result<bool, AppError> {
    let workspaces = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.organization_id
        FROM organization_memberships membership
        JOIN actors account ON account.id = membership.actor_id
        WHERE membership.actor_id = $1 AND membership.enabled
          AND account.kind = 'external_agent' AND account.status = 'active' -- // kind-check: mechanical (only an agent account is bound to a host)
          AND ($2::uuid IS NULL OR membership.organization_id = $2)
        ORDER BY membership.organization_id
        "#,
    )
    .bind(account_id)
    .bind(host.owner_workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    for workspace in workspaces {
        match roles::standing(connection, workspace, caller).await {
            Ok(standing) if standing.may(Permission::AgentsManage) => return Ok(true),
            Ok(_) | Err(AppError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// What a host does when it acts for one of its agents in a workspace:
/// K1.23 turns this into `Via::ActingFor`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActingFor {
    pub host_id: Uuid,
    pub host_account_id: Uuid,
    pub account_id: Uuid,
    pub workspace_id: Uuid,
}

/// The one place a host's right to act for an agent is decided: the host is
/// linked, the agent is bound to it and active, and the agent is a member of
/// the workspace. A host reaches a workspace only through such an agent.
pub async fn acting_for(
    connection: &mut PgConnection,
    host_id: Uuid,
    account_id: Uuid,
    workspace_id: Uuid,
) -> Result<Option<ActingFor>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT host.account_id
        FROM hosts host
        JOIN host_agents binding
          ON binding.host_id = host.id AND binding.account_id = $2 AND binding.status = 'active'
        JOIN organization_memberships membership
          ON membership.actor_id = binding.account_id
         AND membership.organization_id = $3 AND membership.enabled
        JOIN actors account ON account.id = binding.account_id AND account.status = 'active'
        WHERE host.id = $1 AND host.status <> 'revoked'
        "#,
    )
    .bind(host_id)
    .bind(account_id)
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?
    .map(|(host_account_id,)| ActingFor {
        host_id,
        host_account_id,
        account_id,
        workspace_id,
    }))
}

// ---------------------------------------------------------------------------
// Reading

async fn owner_view(connection: &mut PgConnection, owner: Owner) -> Result<OwnerView, AppError> {
    Ok(match owner {
        Owner::Account(id) => OwnerView {
            kind: "person".to_owned(),
            id,
            name: sqlx::query_scalar::<_, String>(
                "SELECT display_name FROM users WHERE actor_id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?
            .unwrap_or_else(|| "Someone".to_owned()),
        },
        Owner::Workspace(id) => OwnerView {
            kind: "workspace".to_owned(),
            id,
            name: sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_one(&mut *connection)
                .await?,
        },
    })
}

pub async fn get(
    connection: &mut PgConnection,
    host_id: Uuid,
) -> Result<Option<HostRow>, AppError> {
    Ok(sqlx::query_as::<_, HostRow>(&format!(
        "SELECT {HOST_COLUMNS} FROM hosts host WHERE host.id = $1"
    ))
    .bind(host_id)
    .fetch_optional(&mut *connection)
    .await?)
}

async fn get_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
) -> Result<Option<HostRow>, AppError> {
    Ok(sqlx::query_as::<_, HostRow>(&format!(
        "SELECT {HOST_COLUMNS} FROM hosts host WHERE host.id = $1 FOR UPDATE"
    ))
    .bind(host_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Whether `caller` may see `host`: they manage it, or they are in the
/// workspace that owns it.
async fn may_see(
    connection: &mut PgConnection,
    caller: Uuid,
    host: &HostRow,
) -> Result<bool, AppError> {
    Ok(match host.owner() {
        Owner::Account(owner) => owner == caller,
        Owner::Workspace(workspace) => {
            sqlx::query_scalar::<_, bool>(
                r#"
            SELECT EXISTS (
                SELECT 1 FROM organization_memberships
                WHERE organization_id = $1 AND actor_id = $2 AND enabled
            )
            "#,
            )
            .bind(workspace)
            .bind(caller)
            .fetch_one(&mut *connection)
            .await?
        }
    })
}

pub async fn view(
    connection: &mut PgConnection,
    caller: Uuid,
    host: &HostRow,
) -> Result<HostView, AppError> {
    let bindings = sqlx::query_as::<_, HostAgentRow>(&format!(
        "SELECT {HOST_AGENT_COLUMNS} FROM host_agents binding WHERE binding.host_id = $1 ORDER BY binding.created_at, binding.account_id"
    ))
    .bind(host.id)
    .fetch_all(&mut *connection)
    .await?;
    let mut agents = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let name = sqlx::query_scalar::<_, String>(
            r#"
            SELECT display_name FROM project_memberships
            WHERE actor_id = $1 ORDER BY enabled DESC, created_at LIMIT 1
            "#,
        )
        .bind(binding.account_id)
        .fetch_optional(&mut *connection)
        .await?;
        agents.push(HostAgentView {
            account_id: binding.account_id,
            name,
            host_agent_ref: binding.host_agent_ref,
            status: binding.status,
            backends: binding.backends,
            actions: binding.actions,
        });
    }
    Ok(HostView {
        id: host.id,
        name: host.name.clone(),
        owner: owner_view(connection, host.owner()).await?,
        builtin: host.builtin,
        status: host.status.clone(),
        software: host.software.clone(),
        software_version: host.software_version.clone(),
        protocol: host.protocol_version.clone(),
        connection: host.connection.clone(),
        capabilities: host.capabilities.clone(),
        offers: host.offers.clone(),
        actions: host.actions.clone(),
        described_agents: host.described_agents.clone(),
        agents,
        module_manifest_url: host.module_manifest_url.clone(),
        last_seen_at: host.last_seen_at,
        can_manage: may_manage(connection, caller, host).await?,
        version: host.version,
        created_at: host.created_at,
    })
}

/// The hosts `caller` may see: their own, and those of `workspace` (when
/// they are in it). Unlinked hosts are left out.
pub async fn list(
    connection: &mut PgConnection,
    caller: Uuid,
    workspace: Option<Uuid>,
) -> Result<Vec<HostView>, AppError> {
    let hosts = sqlx::query_as::<_, HostRow>(&format!(
        r#"
        SELECT {HOST_COLUMNS} FROM hosts host
        WHERE host.status <> 'revoked'
          AND (host.owner_account_id = $1
               OR (host.owner_workspace_id = $2 AND EXISTS (
                       SELECT 1 FROM organization_memberships membership
                       WHERE membership.organization_id = $2 AND membership.actor_id = $1
                         AND membership.enabled)))
        ORDER BY host.builtin DESC, host.created_at, host.id
        LIMIT 200
        "#
    ))
    .bind(caller)
    .bind(workspace)
    .fetch_all(&mut *connection)
    .await?;
    let mut views = Vec::with_capacity(hosts.len());
    for host in &hosts {
        views.push(view(connection, caller, host).await?);
    }
    Ok(views)
}

// ---------------------------------------------------------------------------
// People's operations

fn label(value: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > MAX_LABEL_CHARS || value.chars().any(char::is_control) {
        return Err(AppError::Refused {
            field: "label",
            message: "Keep the label to one line of 80 characters.",
        });
    }
    Ok(Some(value.to_owned()))
}

/// Make a one-time code that links a host to `owner`. A person may own a
/// host; a workspace's host needs `hosts.manage` there.
pub async fn create_enrolment(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    caller_kind: ActorKind,
    owner: Owner,
    label_text: Option<&str>,
) -> Result<EnrolmentCode, AppError> {
    let label_text = label(label_text)?;
    match owner {
        Owner::Account(account) => {
            // A host's owner is a person or a workspace (R19).
            let person = caller_kind == ActorKind::Human; // kind-check: mechanical (R19)
            if account != caller || !person {
                return Err(AppError::Refused {
                    field: "owner",
                    message: "Only a person links a host of their own.",
                });
            }
        }
        Owner::Workspace(workspace) => {
            // Someone outside the workspace hears it doesn't exist.
            let standing = roles::standing(transaction, workspace, caller).await?;
            if !standing.may(Permission::HostsManage) {
                return Err(AppError::Refused {
                    field: "workspaceId",
                    message: "Linking a host for the workspace needs the hosts.manage permission.",
                });
            }
        }
    }
    let open = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM host_enrolments
        WHERE created_by_account_id = $1 AND used_at IS NULL AND expires_at > now()
        "#,
    )
    .bind(caller)
    .fetch_one(&mut **transaction)
    .await?;
    if open >= MAX_OPEN_CODES {
        return Err(AppError::Refused {
            field: "code",
            message: "You have ten unused codes already. Use one, or wait 15 minutes.",
        });
    }
    let code = new_code();
    let expires_at = Utc::now() + Duration::minutes(CODE_MINUTES);
    let enrolment_id = Uuid::now_v7();
    let (owner_account, owner_workspace) = match owner {
        Owner::Account(account) => (Some(account), None),
        Owner::Workspace(workspace) => (None, Some(workspace)),
    };
    sqlx::query(
        r#"
        INSERT INTO host_enrolments (
            id, owner_account_id, owner_workspace_id, code_hash, label,
            created_by_account_id, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(enrolment_id)
    .bind(owner_account)
    .bind(owner_workspace)
    .bind(digest(&code).as_slice())
    .bind(label_text)
    .bind(caller)
    .bind(expires_at)
    .execute(&mut **transaction)
    .await?;
    Ok(EnrolmentCode {
        enrolment_id,
        code,
        expires_at,
        owner: owner_view(transaction, owner).await?,
    })
}

/// A host the caller manages, locked for a change. Not found when they
/// can't see it; refused when they can see it but not manage it.
async fn managed_host(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    host_id: Uuid,
) -> Result<HostRow, AppError> {
    let host = get_for_update(transaction, host_id)
        .await?
        .ok_or(AppError::NotFound("host"))?;
    if host.status == "revoked" || !may_see(transaction, caller, &host).await? {
        return Err(AppError::NotFound("host"));
    }
    if host.builtin {
        return Err(AppError::Refused {
            field: "hostId",
            message: "This server's own host can't be changed here.",
        });
    }
    if !may_manage(transaction, caller, &host).await? {
        return Err(AppError::Refused {
            field: "hostId",
            message: "Changing this host needs the hosts.manage permission in its workspace.",
        });
    }
    Ok(host)
}

fn host_agent_ref(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        return Err(AppError::Refused {
            field: "hostAgentRef",
            message: "Give the host's name for the agent: one line of at most 200 characters.",
        });
    }
    Ok(value.to_owned())
}

/// Let `host` act for agent `account_id`, which it knows as
/// `host_agent_ref`. Needs managing the host and `agents.manage` for the
/// agent (R45).
pub async fn bind(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    host_id: Uuid,
    account_id: Uuid,
    reference: &str,
) -> Result<HostView, AppError> {
    let reference = host_agent_ref(reference)?;
    let host = managed_host(transaction, caller, host_id).await?;
    if !may_manage_agent(transaction, caller, &host, account_id).await? {
        return Err(AppError::Refused {
            field: "accountId",
            message: "Binding that agent needs the agents.manage permission where it is a member.",
        });
    }
    let lineage = described_lineage(&host.described_agents, &reference);
    let taken = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM host_agents
            WHERE host_id = $1 AND host_agent_ref = $2 AND account_id <> $3
        )
        "#,
    )
    .bind(host.id)
    .bind(&reference)
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?;
    if taken {
        return Err(AppError::Refused {
            field: "hostAgentRef",
            message: "Another agent is bound to that name on this host. Unbind it first.",
        });
    }
    let described = described_agent(&host.described_agents, &reference);
    sqlx::query(
        r#"
        INSERT INTO host_agents (
            host_id, account_id, host_agent_ref, lineage, backends, actions, status,
            bound_by_account_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'active', $7)
        ON CONFLICT (host_id, account_id) DO UPDATE SET
            host_agent_ref = EXCLUDED.host_agent_ref,
            lineage = EXCLUDED.lineage,
            backends = EXCLUDED.backends,
            actions = EXCLUDED.actions,
            status = 'active',
            bound_by_account_id = EXCLUDED.bound_by_account_id,
            updated_at = now()
        "#,
    )
    .bind(host.id)
    .bind(account_id)
    .bind(&reference)
    .bind(lineage)
    .bind(
        described
            .as_ref()
            .map_or_else(|| json!([]), |agent| agent["backends"].clone()),
    )
    .bind(
        described
            .as_ref()
            .map_or_else(|| json!([]), |agent| agent["actions"].clone()),
    )
    .bind(caller)
    .execute(&mut **transaction)
    .await?;
    let host = bump(transaction, host.id).await?;
    view(transaction, caller, &host).await
}

/// Stop `host` acting for agent `account_id`. Its runs there are
/// interrupted: nobody is left to report them.
pub async fn unbind(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    host_id: Uuid,
    account_id: Uuid,
) -> Result<HostView, AppError> {
    let host = managed_host(transaction, caller, host_id).await?;
    let removed = sqlx::query("DELETE FROM host_agents WHERE host_id = $1 AND account_id = $2")
        .bind(host.id)
        .bind(account_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(AppError::NotFound("agent on this host"));
    }
    crate::runs::ops::host_lost(
        transaction,
        host.id,
        Some(account_id),
        "The agent was unbound from its host",
    )
    .await?;
    let host = bump(transaction, host.id).await?;
    view(transaction, caller, &host).await
}

/// Unlink a host: its credential stops working (the next call is `401
/// host_revoked`), its active runs are interrupted and the open requests
/// raised through it are cancelled.
pub async fn unlink(
    transaction: &mut Transaction<'_, Postgres>,
    caller: Uuid,
    host_id: Uuid,
) -> Result<HostView, AppError> {
    let host = managed_host(transaction, caller, host_id).await?;
    let host = sqlx::query_as::<_, HostRow>(&format!(
        r#"
        UPDATE hosts host
           SET status = 'revoked', revoked_at = now(), revoked_by_account_id = $2,
               version = host.version + 1, updated_at = now()
         WHERE host.id = $1
        RETURNING {HOST_COLUMNS}
        "#
    ))
    .bind(host.id)
    .bind(caller)
    .fetch_one(&mut **transaction)
    .await?;
    crate::runs::ops::host_lost(transaction, host.id, None, "The host was unlinked").await?;
    crate::requests::cancel_raised_via_host(transaction, host.id).await?;
    view(transaction, caller, &host).await
}

async fn bump(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
) -> Result<HostRow, AppError> {
    Ok(sqlx::query_as::<_, HostRow>(&format!(
        r#"
        UPDATE hosts host SET version = host.version + 1, updated_at = now()
         WHERE host.id = $1
        RETURNING {HOST_COLUMNS}
        "#
    ))
    .bind(host_id)
    .fetch_one(&mut **transaction)
    .await?)
}

/// The agent the host last described under `reference`, if any.
fn described_agent(described: &Value, reference: &str) -> Option<Value> {
    described
        .as_array()?
        .iter()
        .find(|agent| agent["hostAgentRef"].as_str() == Some(reference))
        .cloned()
}

fn described_lineage(described: &Value, reference: &str) -> Option<String> {
    described_agent(described, reference)
        .and_then(|agent| agent["ref"]["lineage"].as_str().map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_carry_80_bits_and_read_back_however_they_are_typed() {
        let code = new_code();
        assert_eq!(code.len(), "AWH-XXXX-XXXX-XXXX-XXXX".len(), "{code}");
        assert!(code.starts_with("AWH-"));
        assert_eq!(normalize_code(&code).as_deref(), Some(code.as_str()));
        let typed = code.to_lowercase().replace('-', " ");
        assert_eq!(normalize_code(&typed).as_deref(), Some(code.as_str()));
        assert_ne!(new_code(), code);
        assert_eq!(
            normalize_code("awh-0o1l-abcd-efgh-jkmn").as_deref(),
            Some("AWH-0011-ABCD-EFGH-JKMN")
        );
        assert_eq!(normalize_code("AWH-1234"), None);
        assert_eq!(normalize_code("AWH-UUUU-UUUU-UUUU-UUUU"), None);
    }

    #[test]
    fn tokens_are_found_by_prefix_and_kept_as_a_digest() {
        let (token, prefix, token_digest) = new_token();
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(prefix.len(), TOKEN_PREFIX_LENGTH);
        assert!(token.starts_with(&prefix));
        assert_eq!(token_digest, digest(&token));
    }
}
