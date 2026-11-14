//! A1.11: what a host calls (A1 section 3.9): enrol with a one-time code,
//! describe itself, send heartbeats and notices. Every call but enrol
//! carries the host's bearer token; an unlinked host gets `401
//! host_revoked`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{
    HOST_COLUMNS, HostRow, OwnerView, PROTOCOL, PROTOCOLS, TOKEN_PREFIX, TOKEN_PREFIX_LENGTH,
    digest, new_token, normalize_code, owner_view,
};
use crate::error::AppError;
use crate::runs::activities::redact_value;

const MAX_DESCRIBED_AGENTS: usize = 200;
const MAX_LIST_ITEMS: usize = 50;
const MAX_HEARTBEAT_RUNS: usize = 500;

/// Why a host's call failed, as the host reads it.
#[derive(Debug)]
pub enum HostError {
    /// No such token (401 `unauthorized`).
    Unauthorized,
    /// The host was unlinked (401 `host_revoked`).
    Revoked,
    /// The code is wrong, used or expired (401 `enrolment_refused`).
    CodeRefused,
    /// The host speaks no version this server serves.
    ProtocolUnsupported,
    /// A field is wrong: which, and what to do.
    Invalid {
        field: &'static str,
        message: String,
    },
    /// The agent named isn't bound to this host.
    AgentUnknown,
    App(AppError),
}

impl From<AppError> for HostError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for HostError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

fn invalid(field: &'static str, message: impl Into<String>) -> HostError {
    HostError::Invalid {
        field,
        message: message.into(),
    }
}

/// The host a token belongs to. A revoked host is told so.
pub async fn authenticate(pool: &PgPool, token: &str) -> Result<HostRow, HostError> {
    if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        return Err(HostError::Unauthorized);
    }
    let Some(prefix) = token.get(..TOKEN_PREFIX_LENGTH) else {
        return Err(HostError::Unauthorized);
    };
    let row = sqlx::query_as::<_, (Vec<u8>, Uuid)>(
        "SELECT interim_token_digest, id FROM hosts WHERE interim_token_prefix = $1",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await?;
    let Some((stored, host_id)) = row else {
        return Err(HostError::Unauthorized);
    };
    let candidate = digest(token);
    if stored.len() != candidate.len() || !bool::from(stored.as_slice().ct_eq(&candidate)) {
        return Err(HostError::Unauthorized);
    }
    let mut connection = pool.acquire().await?;
    let host = super::get(&mut connection, host_id)
        .await?
        .ok_or(HostError::Unauthorized)?;
    if host.status == "revoked" {
        return Err(HostError::Revoked);
    }
    Ok(host)
}

/// The version a call says it speaks: absent means the current one.
pub fn check_protocol(header: Option<&str>) -> Result<(), HostError> {
    match header {
        None => Ok(()),
        Some(version) if PROTOCOLS.contains(&version.trim()) => Ok(()),
        Some(_) => Err(HostError::ProtocolUnsupported),
    }
}

/// A host's own text (a name, a reference, a reason) as it is kept:
/// secrets removed (`run_log::redact`), one line.
fn one_line(value: &str, max: usize, field: &'static str) -> Result<String, HostError> {
    let value = crate::run_log::redact(value.trim());
    let value = value.as_str();
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            format!("Give {field} as one line of at most {max} characters."),
        ));
    }
    Ok(value.to_owned())
}

fn optional_line(
    value: Option<&str>,
    max: usize,
    field: &'static str,
) -> Result<Option<String>, HostError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| one_line(value, max, field))
        .transpose()
}

fn https(value: Option<&str>, field: &'static str) -> Result<Option<String>, HostError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = crate::run_log::redact(value);
    let value = value.as_str();
    let ok = value.len() <= 1000
        && value.starts_with("https://")
        && value.len() > "https://".len()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control());
    if !ok {
        return Err(invalid(
            field,
            format!("Give {field} as an https:// address."),
        ));
    }
    Ok(Some(value.to_owned()))
}

/// A JSON value no bigger than `max` bytes.
fn bounded(value: &Value, max: usize, field: &'static str) -> Result<(), HostError> {
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > max) {
        return Err(invalid(
            field,
            format!("Keep {field} under {} KiB.", max / 1024),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- enrol

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSoftware {
    pub name: String,
    pub software: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrolRequest {
    pub code: String,
    pub host: HostSoftware,
    #[serde(default)]
    pub protocol_versions: Vec<String>,
    pub connection: Option<String>,
    #[serde(default)]
    pub push: Option<Value>,
    #[serde(default)]
    pub host_token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    /// `bearer` until K1 issues client credentials.
    pub kind: &'static str,
    /// Shown once; send it as `Authorization: Bearer <token>`.
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrolled {
    pub host_id: Uuid,
    pub owner: OwnerView,
    pub protocol: &'static str,
    pub credential: Credential,
    pub push_signing_secret: Option<String>,
}

/// Turn a one-time code into a linked host with its own account and
/// credential. The code works once.
pub async fn enrol(pool: &PgPool, request: EnrolRequest) -> Result<Enrolled, HostError> {
    let Some(code) = normalize_code(&request.code) else {
        return Err(HostError::CodeRefused);
    };
    if !request.protocol_versions.is_empty()
        && !request
            .protocol_versions
            .iter()
            .any(|version| PROTOCOLS.contains(&version.as_str()))
    {
        return Err(HostError::ProtocolUnsupported);
    }
    match request.connection.as_deref().unwrap_or("dial_out") {
        "dial_out" => {}
        "push" => {
            return Err(invalid(
                "connection",
                "This server takes dial_out hosts for now; push comes with the command channel.",
            ));
        }
        _ => {
            return Err(invalid(
                "connection",
                "Give connection as dial_out or push.",
            ));
        }
    }
    if request.push.as_ref().is_some_and(|push| !push.is_null()) || request.host_token.is_some() {
        return Err(invalid(
            "push",
            "Leave push and hostToken out: this server takes dial_out hosts for now.",
        ));
    }
    let name = one_line(&request.host.name, 80, "host.name")?;
    let software = optional_line(request.host.software.as_deref(), 80, "host.software")?;
    let version = optional_line(request.host.version.as_deref(), 40, "host.version")?;

    let mut transaction = pool.begin().await?;
    let enrolment = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<Uuid>, Uuid)>(
        r#"
        SELECT id, owner_account_id, owner_workspace_id, created_by_account_id
        FROM host_enrolments
        WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(digest(&code).as_slice())
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((enrolment_id, owner_account, owner_workspace, created_by)) = enrolment else {
        return Err(HostError::CodeRefused);
    };
    let account_id = Uuid::now_v7();
    let host_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'integration', 'active')")
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
    let (token, prefix, token_digest) = new_token();
    let host = sqlx::query_as::<_, HostRow>(&format!(
        r#"
        INSERT INTO hosts AS host (
            id, account_id, owner_account_id, owner_workspace_id, name, software,
            software_version, protocol_version, connection, interim_token_prefix,
            interim_token_digest, enrolled_by_account_id, last_seen_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'dial_out', $9, $10, $11, now())
        RETURNING {HOST_COLUMNS}
        "#
    ))
    .bind(host_id)
    .bind(account_id)
    .bind(owner_account)
    .bind(owner_workspace)
    .bind(&name)
    .bind(software)
    .bind(version)
    .bind(PROTOCOL)
    .bind(&prefix)
    .bind(token_digest.as_slice())
    .bind(created_by)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("UPDATE host_enrolments SET used_at = now(), host_id = $2 WHERE id = $1")
        .bind(enrolment_id)
        .bind(host_id)
        .execute(&mut *transaction)
        .await?;
    let owner = owner_view(&mut transaction, host.owner()).await?;
    transaction.commit().await?;
    Ok(Enrolled {
        host_id,
        owner,
        protocol: PROTOCOL,
        credential: Credential {
            kind: "bearer",
            token,
        },
        push_signing_secret: None,
    })
}

// ---------------------------------------------------------------- describe

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeSoftware {
    pub name: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeModule {
    pub manifest_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeRequest {
    pub software: Option<DescribeSoftware>,
    pub issuer: Option<String>,
    pub module: Option<DescribeModule>,
    #[serde(default)]
    pub capabilities: Map<String, Value>,
    pub report_interval_seconds: Option<i32>,
    #[serde(default)]
    pub offers: Vec<Value>,
    #[serde(default)]
    pub actions: Vec<Value>,
    #[serde(default)]
    pub agents: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Described {
    pub host_id: Uuid,
    pub protocol: &'static str,
    pub deprecated: bool,
    /// Described agents not bound to an account yet: a person binds them.
    pub unbound_agents: Vec<String>,
}

/// An offer `{id, title, summary?, start}` or action `{id, label, needs,
/// danger?}`, kept as the host wrote it once its shape holds (R21: the
/// server never knows what one means).
fn listed(items: &[Value], field: &'static str, label_key: &str) -> Result<Value, HostError> {
    if items.len() > MAX_LIST_ITEMS {
        return Err(invalid(
            field,
            format!("List at most {MAX_LIST_ITEMS} {field}."),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        let id = item["id"].as_str().unwrap_or_default();
        let label = item[label_key].as_str().unwrap_or_default();
        if id.is_empty() || id.len() > 64 || label.trim().is_empty() || label.chars().count() > 80 {
            return Err(invalid(
                field,
                format!(
                    "Each of {field} needs an id (at most 64 characters) and a {label_key} (at most 80)."
                ),
            ));
        }
        if !seen.insert(id.to_owned()) {
            return Err(invalid(field, format!("Each of {field} needs its own id.")));
        }
        if field == "actions" || field.ends_with(".actions") {
            match item["needs"].as_str() {
                Some("confirm" | "panel") => {}
                _ => {
                    return Err(invalid(field, "An action's needs is confirm or panel."));
                }
            }
        }
    }
    let value = Value::Array(items.to_vec());
    bounded(&value, 32 * 1024, field)?;
    Ok(redact_value(value))
}

/// One described agent, cut to what the server keeps.
fn described_agent(agent: &Value) -> Result<Value, HostError> {
    let reference = agent["hostAgentRef"]
        .as_str()
        .ok_or_else(|| invalid("agents", "Each agent needs its hostAgentRef."))?;
    let reference = one_line(reference, 200, "agents.hostAgentRef")?;
    let name = optional_line(agent["name"].as_str(), 80, "agents.name")?;
    let backends = agent["backends"].as_array().cloned().unwrap_or_default();
    if backends.len() > MAX_LIST_ITEMS {
        return Err(invalid("agents.backends", "List at most 50 backends."));
    }
    let backends = backends
        .iter()
        .map(|backend| {
            let id = backend["id"].as_str().unwrap_or_default();
            if id.is_empty() || id.len() > 64 {
                return Err(invalid("agents.backends", "Each backend needs an id."));
            }
            let hidden = backend["hidden"].as_bool().unwrap_or(false);
            Ok(json!({
                "id": id,
                // A hidden backend's name is never kept (R21).
                "name": if hidden { Value::Null } else {
                    backend["name"].as_str().map(|name| name.chars().take(80).collect::<String>()).into()
                },
                "hidden": hidden,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    bounded(
        &Value::Array(backends.clone()),
        16 * 1024,
        "agents.backends",
    )?;
    let actions = listed(
        agent["actions"].as_array().map_or(&[][..], Vec::as_slice),
        "agents.actions",
        "label",
    )?;
    let reference_block = &agent["ref"];
    let outside_ref = if reference_block.is_object() {
        json!({
            "issuer": reference_block["issuer"].as_str().map(|value| value.chars().take(500).collect::<String>()),
            "id": reference_block["id"].as_str().map(|value| value.chars().take(200).collect::<String>()),
            "lineage": reference_block["lineage"].as_str().map(|value| value.chars().take(200).collect::<String>()),
        })
    } else {
        Value::Null
    };
    // Names and references are the host's text: secrets in them go.
    Ok(json!({
        "hostAgentRef": reference,
        "name": name,
        "ref": redact_value(outside_ref),
        "backends": redact_value(Value::Array(backends)),
        "actions": actions,
    }))
}

/// Store what the host says it is and can do.
pub async fn describe(
    pool: &PgPool,
    host: &HostRow,
    request: DescribeRequest,
) -> Result<Described, HostError> {
    let software = request.software.as_ref();
    let software_name = optional_line(
        software.and_then(|software| software.name.as_deref()),
        80,
        "software.name",
    )?;
    let software_version = optional_line(
        software.and_then(|software| software.version.as_deref()),
        40,
        "software.version",
    )?;
    let issuer = https(request.issuer.as_deref(), "issuer")?;
    let manifest = https(
        request
            .module
            .as_ref()
            .and_then(|module| module.manifest_url.as_deref()),
        "module.manifestUrl",
    )?;
    let capabilities = Value::Object(request.capabilities);
    bounded(&capabilities, 32 * 1024, "capabilities")?;
    let capabilities = redact_value(capabilities);
    let interval = request
        .report_interval_seconds
        .unwrap_or(host.report_interval_seconds);
    if !(15..=3600).contains(&interval) {
        return Err(invalid(
            "reportIntervalSeconds",
            "Give reportIntervalSeconds between 15 and 3600.",
        ));
    }
    let offers = listed(&request.offers, "offers", "title")?;
    let actions = listed(&request.actions, "actions", "label")?;
    if request.agents.len() > MAX_DESCRIBED_AGENTS {
        return Err(invalid("agents", "Describe at most 200 agents."));
    }
    let agents = request
        .agents
        .iter()
        .map(described_agent)
        .collect::<Result<Vec<_>, _>>()?;
    let mut references = std::collections::BTreeSet::new();
    for agent in &agents {
        if !references.insert(
            agent["hostAgentRef"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        ) {
            return Err(invalid("agents", "Each agent needs its own hostAgentRef."));
        }
    }
    let described = Value::Array(agents.clone());
    bounded(&described, 128 * 1024, "agents")?;

    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE hosts
           SET software = COALESCE($2, software), software_version = COALESCE($3, software_version),
               issuer = $4, module_manifest_url = $5, capabilities = $6,
               report_interval_seconds = $7, offers = $8, actions = $9, described_agents = $10,
               last_seen_at = now(),
               status = CASE WHEN status = 'offline' THEN 'active' ELSE status END,
               version = version + 1, updated_at = now()
         WHERE id = $1
        "#,
    )
    .bind(host.id)
    .bind(software_name)
    .bind(software_version)
    .bind(issuer)
    .bind(manifest)
    .bind(&capabilities)
    .bind(interval)
    .bind(&offers)
    .bind(&actions)
    .bind(&described)
    .execute(&mut *transaction)
    .await?;
    let bound = sqlx::query_scalar::<_, String>(
        "SELECT host_agent_ref FROM host_agents WHERE host_id = $1",
    )
    .bind(host.id)
    .fetch_all(&mut *transaction)
    .await?;
    let mut unbound_agents = Vec::new();
    for agent in &agents {
        let reference = agent["hostAgentRef"].as_str().unwrap_or_default();
        if bound.iter().any(|bound| bound == reference) {
            sqlx::query(
                r#"
                UPDATE host_agents
                   SET backends = $3, actions = $4,
                       lineage = COALESCE(lineage, $5), updated_at = now()
                 WHERE host_id = $1 AND host_agent_ref = $2
                "#,
            )
            .bind(host.id)
            .bind(reference)
            .bind(&agent["backends"])
            .bind(&agent["actions"])
            .bind(agent["ref"]["lineage"].as_str())
            .execute(&mut *transaction)
            .await?;
        } else {
            unbound_agents.push(reference.to_owned());
        }
    }
    transaction.commit().await?;
    Ok(Described {
        host_id: host.id,
        protocol: PROTOCOL,
        deprecated: false,
        unbound_agents,
    })
}

// ---------------------------------------------------------------- heartbeat

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatAgent {
    pub host_agent_ref: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRun {
    pub run_id: Uuid,
    pub host_run_ref: Option<String>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatRequest {
    pub at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub capacity: Option<Value>,
    #[serde(default)]
    pub agents: Vec<HeartbeatAgent>,
    #[serde(default)]
    pub runs: Vec<HeartbeatRun>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatReply {
    pub protocol: &'static str,
    pub deprecated: bool,
    /// The host hasn't described itself yet.
    pub describe_wanted: bool,
    /// How many of the listed runs are this host's.
    pub runs_renewed: u64,
}

/// The host is alive: it is back online, its agents' statuses are noted,
/// and the runs it lists (its own only) have their liveness renewed.
pub async fn heartbeat(
    pool: &PgPool,
    host: &HostRow,
    request: HeartbeatRequest,
) -> Result<HeartbeatReply, HostError> {
    let capacity = match request.capacity {
        None | Some(Value::Null) => None,
        Some(value @ Value::Object(_)) => {
            bounded(&value, 4096, "capacity")?;
            Some(redact_value(value))
        }
        Some(_) => return Err(invalid("capacity", "Give capacity as an object.")),
    };
    if request.agents.len() > MAX_DESCRIBED_AGENTS || request.runs.len() > MAX_HEARTBEAT_RUNS {
        return Err(invalid(
            "runs",
            "List at most 200 agents and 500 runs in one heartbeat.",
        ));
    }
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE hosts
           SET last_seen_at = now(), capacity = COALESCE($2, capacity),
               status = CASE WHEN status = 'offline' THEN 'active' ELSE status END
         WHERE id = $1
        "#,
    )
    .bind(host.id)
    .bind(capacity)
    .execute(&mut *transaction)
    .await?;
    for agent in &request.agents {
        let status = match agent.status.as_str() {
            "active" | "paused" => agent.status.as_str(),
            _ => continue,
        };
        sqlx::query(
            r#"
            UPDATE host_agents SET status = $3, updated_at = now()
             WHERE host_id = $1 AND host_agent_ref = $2 AND status <> 'gone' AND status <> $3
            "#,
        )
        .bind(host.id)
        // Kept references have their secrets removed (`one_line`).
        .bind(crate::run_log::redact(agent.host_agent_ref.trim()))
        .bind(status)
        .execute(&mut *transaction)
        .await?;
    }
    let run_ids = request
        .runs
        .iter()
        .map(|run| run.run_id)
        .collect::<Vec<_>>();
    let runs_renewed =
        crate::runs::liveness::renew_for_host(&mut transaction, host.id, &run_ids).await?;
    transaction.commit().await?;
    Ok(HeartbeatReply {
        protocol: PROTOCOL,
        deprecated: false,
        describe_wanted: host.software.is_none() && host.capabilities == json!({}),
        runs_renewed,
    })
}

// ---------------------------------------------------------------- notices

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeRequest {
    pub kind: String,
    pub host_agent_ref: String,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub new_host_agent_ref: Option<String>,
    #[serde(rename = "ref", default)]
    pub reference: Option<Value>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeReply {
    pub notice_id: Uuid,
    /// applied, waiting (kept until outside references arrive, K1.7) or
    /// request_opened (the agent's managers decide).
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub request_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interrupted_run_ids: Vec<Uuid>,
}

async fn bound_agent(
    transaction: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    reference: &str,
) -> Result<(Uuid, Option<String>), HostError> {
    sqlx::query_as::<_, (Uuid, Option<String>)>(
        r#"
        SELECT account_id, lineage FROM host_agents
        WHERE host_id = $1 AND host_agent_ref = $2 AND status <> 'gone'
        FOR UPDATE
        "#,
    )
    .bind(host_id)
    .bind(reference)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(HostError::AgentUnknown)
}

/// One of the three notices: an agent's profile changed, its reference
/// changed, or it was retired. Each needs the agent bound to this host.
pub async fn notice(
    pool: &PgPool,
    host: &HostRow,
    request: NoticeRequest,
) -> Result<NoticeReply, HostError> {
    let reference = one_line(&request.host_agent_ref, 200, "hostAgentRef")?;
    let mut transaction = pool.begin().await?;
    let (account_id, lineage) = bound_agent(&mut transaction, host.id, &reference).await?;
    let mut request_ids = Vec::new();
    let mut interrupted_run_ids = Vec::new();
    let (outcome, body) = match request.kind.as_str() {
        "agent_profile_changed" => {
            let profile = request
                .profile
                .as_ref()
                .filter(|profile| profile.is_object())
                .ok_or_else(|| invalid("profile", "Send the agent's profile as an object."))?;
            let name = one_line(
                profile["name"].as_str().unwrap_or_default(),
                80,
                "profile.name",
            )?;
            // The name people see for the agent in each project; the rest of
            // the profile waits for K1's account profile.
            sqlx::query(
                "UPDATE project_memberships SET display_name = $2, updated_at = now() WHERE actor_id = $1",
            )
            .bind(account_id)
            .bind(&name)
            .execute(&mut *transaction)
            .await?;
            bounded(profile, 8 * 1024, "profile")?;
            ("applied", json!({"profile": redact_value(profile.clone())}))
        }
        "agent_ref_changed" => {
            let new_reference = one_line(
                request.new_host_agent_ref.as_deref().unwrap_or_default(),
                200,
                "newHostAgentRef",
            )?;
            // As described agents' references are kept: secrets removed.
            let outside_ref = redact_value(request.reference.clone().unwrap_or(Value::Null));
            let new_lineage = outside_ref["lineage"].as_str().map(str::to_owned);
            let body = json!({
                "newHostAgentRef": new_reference,
                "ref": outside_ref,
            });
            bounded(&body, 8 * 1024, "ref")?;
            if lineage.is_some() && lineage == new_lineage {
                // Same lineage: the same agent, moved or re-keyed. The
                // binding follows now; K1.7 replays this notice for the
                // account's outside reference.
                let taken = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM host_agents WHERE host_id = $1 AND host_agent_ref = $2 AND account_id <> $3)",
                )
                .bind(host.id)
                .bind(&new_reference)
                .bind(account_id)
                .fetch_one(&mut *transaction)
                .await?;
                if taken {
                    return Err(invalid(
                        "newHostAgentRef",
                        "Another agent on this host is bound to that reference.",
                    ));
                }
                sqlx::query(
                    "UPDATE host_agents SET host_agent_ref = $3, updated_at = now() WHERE host_id = $1 AND account_id = $2",
                )
                .bind(host.id)
                .bind(account_id)
                .bind(&new_reference)
                .execute(&mut *transaction)
                .await?;
                ("waiting", body)
            } else {
                // A different lineage may be a different agent: the
                // account's managers decide; the binding stays as it was.
                request_ids = crate::requests::open_for_host_agent(
                    &mut transaction,
                    host.id,
                    host.account_id,
                    account_id,
                    &host.name,
                    &reference,
                    &new_reference,
                )
                .await?;
                ("request_opened", body)
            }
        }
        "agent_retired" => {
            let reason = optional_line(request.reason.as_deref(), 200, "reason")?;
            sqlx::query(
                "UPDATE host_agents SET status = 'gone', updated_at = now() WHERE host_id = $1 AND account_id = $2",
            )
            .bind(host.id)
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
            interrupted_run_ids = crate::runs::ops::host_lost(
                &mut transaction,
                host.id,
                Some(account_id),
                "The host retired this agent",
            )
            .await?;
            ("applied", json!({"reason": reason}))
        }
        _ => {
            return Err(invalid(
                "kind",
                "Give kind as agent_profile_changed, agent_ref_changed or agent_retired.",
            ));
        }
    };
    let notice_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO host_notices (id, host_id, account_id, kind, host_agent_ref, body, outcome)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(notice_id)
    .bind(host.id)
    .bind(account_id)
    .bind(&request.kind)
    .bind(&reference)
    .bind(&body)
    .bind(outcome)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE hosts SET last_seen_at = now() WHERE id = $1")
        .bind(host.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(NoticeReply {
        notice_id,
        outcome,
        request_ids,
        interrupted_run_ids,
    })
}
