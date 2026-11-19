//! M1.12: connected modules. An outside app becomes a module by publishing
//! a manifest at its own address (M1 plan 3.7); nothing it supplies runs in
//! this server or in the app's pages.
//!
//! - An admin previews the manifest (fetched through `outbound.rs`), then
//!   installs it, approving some or all of the scopes it asks for. The
//!   install makes the module (kind `connected`, switched on), an app
//!   account in the workspace with a role holding exactly the approved
//!   permissions, the two signing secrets (sealed in `secrets`), and the
//!   `module_apps` row. Nothing is fetched while a transaction is open.
//! - A job sends a one-time install code to the manifest's install address,
//!   signed with the outbound secret. The app exchanges the code once at
//!   `POST /api/v1/connected-modules/token` for its account's secret and the
//!   signing secrets, so it proves it controls the manifest's address before
//!   it gets anything. It then signs in like any account (the secret, or an
//!   access token from `/oauth/token`'s client_credentials grant).
//! - Re-fetching applies a narrower or equal manifest at once; one that asks
//!   for new scopes or frame origins waits for approval, and scopes it no
//!   longer asks for are dropped at once.
//! - Revoking disables the account (its next call is refused), revokes its
//!   secrets, deletes the signing secrets, and switches the module off and
//!   archives it, keeping what it made.
//!
//! Every call the app makes is checked against its account's current role,
//! as anyone's is, so a scope that wasn't approved is refused everywhere.

pub mod blocks;
pub mod files;
pub mod http;
pub mod manifest;
pub mod ops;
pub mod panels;
pub mod remote;
pub mod things;

use std::{collections::BTreeSet, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use url::Url;
use uuid::Uuid;

use self::manifest::{Checked, Manifest, Problem, Scope, ScopeRequest};
use crate::{
    AppState,
    access::catalogue::{Permission, PermissionSet},
    error::AppError,
    jobs::{JobContext, JobError, JobRegistry, NewJob},
    ops::{OpError, error::ConflictReason},
    policy::MembershipRole,
};

/// The job that sends an install its one-time code.
pub const HANDSHAKE_KIND: &str = "connected_module.handshake";
const USER_AGENT: &str = concat!("agent-workspace-modules/", env!("CARGO_PKG_VERSION"));
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const CODE_PREFIX: &str = "awmi_";
const CODE_MINUTES: i64 = 15;
/// How long the app's account secret lives before it rotates it.
const SECRET_DAYS: i32 = crate::access::credentials::MAX_DAYS;

// ------------------------------------------------------------------ fetching

/// A manifest fetched and checked.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub address: Url,
    pub checked: Checked,
    pub digest: String,
}

/// A client for an address an app chose: public addresses only, unless the
/// operator trusts that origin or allows private addresses (never on a
/// hosted server).
pub(crate) async fn client(
    state: &AppState,
    url: &Url,
    timeout: Duration,
) -> Result<reqwest::Client, crate::outbound::Refusal> {
    let trusted = state
        .config
        .webhook_trusted_origins
        .contains(&url.origin().ascii_serialization());
    crate::outbound::client_for(
        url,
        trusted || state.config.allow_private_addresses,
        crate::outbound::Timeouts {
            connect: Duration::from_secs(5),
            total: Some(timeout),
        },
        Some(USER_AGENT),
    )
    .await
}

fn problems_error(problems: Vec<Problem>) -> OpError {
    let first = problems
        .first()
        .map(|problem| problem.path.clone())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "manifest".to_owned());
    let listed = problems
        .iter()
        .take(6)
        .map(|problem| {
            if problem.path.is_empty() {
                problem.message.clone()
            } else {
                format!("{}: {}", problem.path, problem.message)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    OpError::Invalid {
        field: first.into(),
        message: format!("The manifest isn't usable. {listed}").into(),
    }
}

/// Fetches and checks the manifest at `address`. Never inside a
/// transaction (rule 14).
pub async fn fetch(state: &AppState, address: &str) -> Result<Fetched, OpError> {
    let trusted = &state.config.webhook_trusted_origins;
    let url = manifest::manifest_address(address, trusted).ok_or(OpError::invalid(
        "url",
        "Give the manifest's https address, with no user name, password or #fragment.",
    ))?;
    let client = client(state, &url, FETCH_TIMEOUT)
        .await
        .map_err(|refusal| match refusal {
            crate::outbound::Refusal::Private => OpError::invalid(
                "url",
                "That address is on a private network this server doesn't reach.",
            ),
            crate::outbound::Refusal::Unresolved => {
                OpError::invalid("url", "That address can't be found.")
            }
        })?;
    let mut response = client
        .get(url.clone())
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| OpError::invalid("url", "The address didn't answer."))?;
    if !response.status().is_success() {
        return Err(OpError::Invalid {
            field: "url".into(),
            message: format!("The address answered {}.", response.status().as_u16()).into(),
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OpError::invalid("url", "The answer was cut off."))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > manifest::MAX_BYTES {
            return Err(OpError::invalid(
                "url",
                "The manifest is larger than 64 KB.",
            ));
        }
    }
    let allow_http = trusted.contains(&url.origin().ascii_serialization());
    let checked = manifest::check(&bytes, &url, allow_http).map_err(problems_error)?;
    Ok(Fetched {
        address: url,
        checked,
        digest: manifest::digest(&bytes),
    })
}

// ------------------------------------------------------------------ views

/// A scope the manifest asks for, as an admin reads it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    /// A permission key, or `events:<type>`.
    pub scope: String,
    /// What it lets the app do, in this server's words.
    pub label: String,
    /// The app's reason.
    pub why: String,
    /// The install can't leave it out.
    pub required: bool,
    /// This server has it; one it doesn't can't be approved.
    pub known: bool,
    pub approved: bool,
}

fn scope_view(request: &ScopeRequest, scope: &Scope, approved: &BTreeSet<String>) -> ScopeView {
    let label = match scope {
        Scope::Permission(permission) => permission.info().label.to_owned(),
        Scope::Events(event) => format!("Hear {event} events"),
        Scope::Unknown => "Not something this server has".to_owned(),
    };
    ScopeView {
        scope: request.scope.clone(),
        label,
        why: request.why.clone(),
        required: request.required,
        known: *scope != Scope::Unknown,
        approved: approved.contains(&request.scope),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub name: String,
    pub verb: String,
    pub permission: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelSummary {
    pub id: String,
    pub slot: String,
    pub title: String,
}

/// What a manifest adds, for a preview or a pending change.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSummary {
    /// The module's key here (`publisher.module`).
    pub key: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: Option<String>,
    /// The name its account will have.
    pub account_name: String,
    pub scopes: Vec<ScopeView>,
    pub kinds: Vec<String>,
    pub operations: Vec<OperationSummary>,
    pub events: Vec<String>,
    pub panels: Vec<PanelSummary>,
    pub frame_origins: Vec<String>,
    /// SHA-256 of the manifest served: pass it to install or approve to be
    /// sure nothing changed since you looked.
    pub digest: String,
}

fn summary(manifest: &Manifest, digest: &str, approved: &BTreeSet<String>) -> ManifestSummary {
    ManifestSummary {
        key: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        publisher: manifest
            .publisher
            .as_ref()
            .map(|publisher| publisher.name.clone()),
        account_name: manifest.account.name.clone(),
        scopes: manifest
            .scopes
            .iter()
            .map(|request| scope_view(request, &Scope::read(&request.scope), approved))
            .collect(),
        kinds: manifest
            .kinds
            .iter()
            .map(|kind| kind.kind.clone())
            .collect(),
        operations: manifest
            .operations
            .iter()
            .map(|operation| OperationSummary {
                name: operation.name.clone(),
                verb: operation.verb.clone(),
                permission: operation.permission.clone(),
            })
            .collect(),
        events: manifest
            .events
            .emits
            .iter()
            .map(|event| event.event_type.clone())
            .collect(),
        panels: manifest
            .panels
            .iter()
            .map(|panel| PanelSummary {
                id: panel.id.clone(),
                slot: panel.slot.clone(),
                title: panel.title.clone(),
            })
            .collect(),
        frame_origins: manifest
            .frame_origins
            .iter()
            .map(|origin| origin.trim_end_matches('/').to_owned())
            .collect(),
        digest: digest.to_owned(),
    }
}

/// A connected module as its admins see it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedModuleView {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    /// `pending` until the app exchanges its install code, then `active`;
    /// `revoked` for good.
    pub state: String,
    /// The app's own account here.
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub manifest_url: String,
    pub base_url: String,
    #[serde(flatten)]
    pub manifest: ManifestSummary,
    /// A re-fetched manifest that asks for more, waiting for approval
    /// (connected_module_approve).
    pub pending: Option<ManifestSummary>,
    #[schemars(with = "String")]
    pub approved_by: Uuid,
    pub approved_at: DateTime<Utc>,
    /// When the install code was last sent to the app.
    pub install_sent_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// For a file source: what its conformance check found,
    /// `{checkedAt, problems}`; none until it has run.
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub file_check: Option<Value>,
}

// ------------------------------------------------------------------ rows

#[derive(Clone, Debug, FromRow)]
pub(crate) struct AppRow {
    pub module_id: Uuid,
    pub workspace_id: Uuid,
    pub account_id: Uuid,
    pub role_id: Uuid,
    pub manifest_url: String,
    pub base_url: String,
    pub manifest: Value,
    pub manifest_digest: String,
    pub pending_manifest: Option<Value>,
    pub pending_digest: Option<String>,
    pub scopes_approved: Vec<String>,
    pub outbound_secret_id: Option<Uuid>,
    pub inbound_secret_id: Option<Uuid>,
    pub credential_id: Option<Uuid>,
    pub approved_by_actor_id: Uuid,
    pub approved_at: DateTime<Utc>,
    pub install_sent_at: Option<DateTime<Utc>>,
    pub state: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub module_key: String,
    pub file_check: Option<Value>,
}

const APP_COLUMNS: &str = "app.module_id, app.workspace_id, app.account_id, app.role_id, \
    app.manifest_url, app.base_url, app.manifest, app.manifest_digest, app.pending_manifest, \
    app.pending_digest, app.scopes_approved, app.outbound_secret_id, app.inbound_secret_id, \
    app.credential_id, app.approved_by_actor_id, app.approved_at, app.install_sent_at, \
    app.state, app.revoked_at, module.module_key, app.file_check";

impl AppRow {
    pub fn manifest(&self) -> Result<Manifest, AppError> {
        serde_json::from_value(self.manifest.clone())
            .map_err(|_| AppError::Internal("stored connected module manifest"))
    }

    fn pending(&self) -> Result<Option<Manifest>, AppError> {
        self.pending_manifest
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| AppError::Internal("stored connected module manifest"))
    }

    pub fn view(&self) -> Result<ConnectedModuleView, AppError> {
        let approved = self.scopes_approved.iter().cloned().collect();
        let pending = match (self.pending()?, &self.pending_digest) {
            (Some(manifest), Some(digest)) => Some(summary(&manifest, digest, &approved)),
            _ => None,
        };
        Ok(ConnectedModuleView {
            module_id: self.module_id,
            state: self.state.clone(),
            account_id: self.account_id,
            manifest_url: self.manifest_url.clone(),
            base_url: self.base_url.clone(),
            manifest: summary(&self.manifest()?, &self.manifest_digest, &approved),
            pending,
            approved_by: self.approved_by_actor_id,
            approved_at: self.approved_at,
            install_sent_at: self.install_sent_at,
            revoked_at: self.revoked_at,
            file_check: self.file_check.clone(),
        })
    }
}

pub(crate) async fn get(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    module_id: Uuid,
    lock: bool,
) -> Result<Option<AppRow>, AppError> {
    Ok(sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM module_apps app \
         JOIN modules module ON module.id = app.module_id \
         WHERE app.workspace_id = $1 AND app.module_id = $2{}",
        if lock { " FOR UPDATE OF app" } else { "" }
    ))
    .bind(workspace_id)
    .bind(module_id)
    .fetch_optional(&mut *connection)
    .await?)
}

pub(crate) async fn list(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    revoked: bool,
) -> Result<Vec<AppRow>, AppError> {
    Ok(sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM module_apps app \
         JOIN modules module ON module.id = app.module_id \
         WHERE app.workspace_id = $1 AND ($2 OR app.state <> 'revoked') \
         ORDER BY app.created_at DESC LIMIT 200"
    ))
    .bind(workspace_id)
    .bind(revoked)
    .fetch_all(&mut *connection)
    .await?)
}

// ------------------------------------------------------------------ scopes

/// The scopes to approve, as keys in the manifest's order and as the
/// permissions they give: `chosen` (all the known ones the manifest asks
/// for when not given), every required one included.
pub(crate) fn approve_scopes(
    manifest: &Manifest,
    chosen: Option<&[String]>,
) -> Result<(Vec<String>, PermissionSet), OpError> {
    let requested = manifest
        .scopes
        .iter()
        .map(|request| (request, Scope::read(&request.scope)))
        .collect::<Vec<_>>();
    let chosen: BTreeSet<&str> = match chosen {
        Some(list) => {
            for scope in list {
                match requested
                    .iter()
                    .find(|(request, _)| request.scope == *scope)
                {
                    None => {
                        return Err(OpError::Invalid {
                            field: "scopes".into(),
                            message: format!(
                                "The manifest doesn't ask for {scope}: approve only what it asks for."
                            )
                            .into(),
                        });
                    }
                    Some((_, Scope::Unknown)) => {
                        return Err(OpError::Invalid {
                            field: "scopes".into(),
                            message: format!("{scope} isn't something this server has.").into(),
                        });
                    }
                    Some(_) => {}
                }
            }
            list.iter().map(String::as_str).collect()
        }
        None => requested
            .iter()
            .filter(|(_, scope)| *scope != Scope::Unknown)
            .map(|(request, _)| request.scope.as_str())
            .collect(),
    };
    for (request, scope) in &requested {
        if request.required && *scope == Scope::Unknown {
            return Err(OpError::Invalid {
                field: "scopes".into(),
                message: format!(
                    "The app needs {}, which this server doesn't have, so it can't be installed here.",
                    request.scope
                )
                .into(),
            });
        }
        if request.required && !chosen.contains(request.scope.as_str()) {
            return Err(OpError::Invalid {
                field: "scopes".into(),
                message: format!(
                    "The app needs {}: approve every required scope, or don't install it.",
                    request.scope
                )
                .into(),
            });
        }
    }
    let mut keys = Vec::new();
    let mut permissions = PermissionSet::EMPTY;
    for (request, scope) in &requested {
        if !chosen.contains(request.scope.as_str()) {
            continue;
        }
        keys.push(request.scope.clone());
        if let Scope::Permission(permission) = scope {
            permissions = permissions.with(*permission);
        }
    }
    Ok((keys, permissions))
}

fn permissions_of(keys: &[String]) -> PermissionSet {
    keys.iter()
        .filter_map(|key| Permission::from_key(key))
        .fold(PermissionSet::EMPTY, PermissionSet::with)
}

/// What `actor` may approve, and the base the app's role takes.
async fn may_approve(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    actor_id: Uuid,
    permissions: PermissionSet,
) -> Result<MembershipRole, OpError> {
    let standing = crate::roles::standing(transaction, workspace_id, actor_id).await?;
    Ok(crate::roles::may_grant_app(standing, permissions)?)
}

// ------------------------------------------------------------------ install

fn handle_stem(key: &str) -> String {
    let stem: String = key
        .chars()
        .map(|c| if c == '.' || c == '_' { '-' } else { c })
        .take(28)
        .collect();
    stem.trim_end_matches('-').to_owned()
}

async fn free_handle(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    key: &str,
) -> Result<String, AppError> {
    let stem = handle_stem(key);
    let taken = sqlx::query_scalar::<_, String>(
        "SELECT lower(handle) FROM organization_actor_profiles \
         WHERE organization_id = $1 AND lower(handle) LIKE $2 || '%'",
    )
    .bind(workspace_id)
    .bind(&stem)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(std::iter::once(stem.clone())
        .chain((2..1000).map(|n| format!("{stem}-{n}")))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| format!("app-{}", &Uuid::now_v7().simple().to_string()[..20])))
}

async fn seal(
    state: &AppState,
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &'static str,
    actor_id: Uuid,
) -> Result<Uuid, AppError> {
    let keys = crate::secrets::keyring(&state.config).map_err(AppError::from)?;
    let mut random = [0_u8; 32];
    OsRng.fill_bytes(&mut random);
    let value =
        crate::secrets::SecretValue::new(agent_workspace_host_protocol::new_push_secret(random));
    Ok(crate::secrets::put(
        transaction,
        keys.as_ref(),
        crate::secrets::NewSecret {
            owner: crate::secrets::SecretOwner::Workspace {
                organization_id: workspace_id,
            },
            name,
            description: Some(description),
            value: &value,
            actor_id: Some(actor_id),
            system_managed: true,
        },
    )
    .await?
    .id)
}

/// Installs a fetched manifest with the scopes `chosen` (every known one
/// when not given). Returns the new module's id.
pub(crate) async fn install(
    state: &AppState,
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    actor_id: Uuid,
    fetched: &Fetched,
    chosen: Option<&[String]>,
) -> Result<Uuid, OpError> {
    let manifest = &fetched.checked.manifest;
    let (approved, permissions) = approve_scopes(manifest, chosen)?;
    let base = may_approve(transaction, workspace_id, actor_id, permissions).await?;

    let module_id = Uuid::now_v7();
    let description: String = manifest.description.chars().take(500).collect();
    let made = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO modules (id, workspace_id, module_key, kind, name, description, created_by_actor_id)
        VALUES ($1, $2, $3, 'connected', $4, $5, $6)
        ON CONFLICT (workspace_id, module_key) WHERE archived_at IS NULL DO NOTHING
        RETURNING id
        "#,
    )
    .bind(module_id)
    .bind(workspace_id)
    .bind(&manifest.id)
    .bind(&manifest.name)
    .bind(&description)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    if made.is_none() {
        return Err(OpError::Conflict {
            reason: ConflictReason::State {
                code: "module_exists",
            },
            message: format!("A module named {} is already here.", manifest.id).into(),
            current: None,
        });
    }
    crate::modules::state::write_switch(
        transaction,
        workspace_id,
        crate::modules::state::SwitchScope::Workspace,
        &manifest.id,
        true,
        actor_id,
    )
    .await?;

    let role_id = crate::roles::insert_app_role(
        transaction,
        workspace_id,
        &manifest.name,
        base,
        permissions,
        actor_id,
    )
    .await?;
    let account_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO actors \
             (id, kind, account_kind, status, owner_workspace_id, created_by_actor_id) \
         VALUES ($1, 'integration', 'app', 'active', $2, $3)",
    )
    .bind(account_id)
    .bind(workspace_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    let handle = free_handle(transaction, workspace_id, &manifest.id).await?;
    sqlx::query(
        "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(&manifest.account.name)
    .bind(&handle)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, role_id, role_explicit) \
         VALUES ($1, $2, $3, $4, true)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(base.key())
    .bind(role_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    crate::invitations::engine::join_workspace_space(transaction, workspace_id, account_id).await?;

    let outbound = seal(
        state,
        transaction,
        workspace_id,
        &format!("connected_module_outbound:{module_id}"),
        "Signs what Workspace sends a connected module.",
        actor_id,
    )
    .await?;
    let inbound = seal(
        state,
        transaction,
        workspace_id,
        &format!("connected_module_inbound:{module_id}"),
        "Checks what a connected module sends Workspace.",
        actor_id,
    )
    .await?;
    let stored = serde_json::to_value(manifest)
        .map_err(|_| AppError::Internal("connected module manifest"))?;
    sqlx::query(
        r#"
        INSERT INTO module_apps (
            module_id, workspace_id, account_id, role_id, manifest_url, base_url, manifest,
            manifest_digest, manifest_fetched_at, frame_origins, scopes_requested,
            scopes_approved, outbound_secret_id, inbound_secret_id, approved_by_actor_id,
            approved_at, state
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), $9, $10, $11, $12, $13, $14, now(), 'pending')
        "#,
    )
    .bind(module_id)
    .bind(workspace_id)
    .bind(account_id)
    .bind(role_id)
    .bind(fetched.address.as_str())
    .bind(fetched.checked.base_url.as_str())
    .bind(&stored)
    .bind(&fetched.digest)
    .bind(fetched.checked.frame_origins())
    .bind(fetched.checked.scope_keys())
    .bind(&approved)
    .bind(outbound)
    .bind(inbound)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    things::register_kinds(transaction, workspace_id, module_id, manifest).await?;
    send_code_later(transaction, workspace_id, module_id).await?;
    announce(transaction, workspace_id, module_id).await?;
    Ok(module_id)
}

async fn send_code_later(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<(), AppError> {
    crate::jobs::enqueue(
        &mut **transaction,
        NewJob::new(HANDSHAKE_KIND)
            .payload(json!({ "moduleId": module_id }))
            .workspace(workspace_id)
            .unique(format!("{HANDSHAKE_KIND}:{module_id}"))
            .max_attempts(6),
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ refresh and approve

/// What a re-fetch did.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Refreshed {
    /// The same manifest.
    Unchanged,
    /// Applied: it asks for nothing new.
    Applied,
    /// It asks for more: waiting for approval (scopes it dropped are
    /// dropped already).
    Pending,
}

fn origins(manifest: &Manifest) -> BTreeSet<String> {
    manifest
        .frame_origins
        .iter()
        .map(|origin| origin.trim_end_matches('/').to_owned())
        .collect()
}

/// Applies `fetched` to an installed module, or holds it for approval.
pub(crate) async fn refresh(
    transaction: &mut Transaction<'static, Postgres>,
    row: &AppRow,
    fetched: &Fetched,
    actor_id: Uuid,
) -> Result<Refreshed, OpError> {
    live(row)?;
    let new = &fetched.checked.manifest;
    if new.id != row.module_key {
        return Err(OpError::invalid(
            "manifest",
            "The manifest now names another module. Install that one on its own.",
        ));
    }
    if fetched.digest == row.manifest_digest {
        sqlx::query(
            "UPDATE module_apps SET manifest_fetched_at = now(), pending_manifest = NULL, \
             pending_digest = NULL, updated_at = now() WHERE module_id = $1",
        )
        .bind(row.module_id)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        return Ok(Refreshed::Unchanged);
    }
    let old = row.manifest()?;
    let old_scopes: BTreeSet<&str> = old.scopes.iter().map(|s| s.scope.as_str()).collect();
    let new_scopes: BTreeSet<&str> = new.scopes.iter().map(|s| s.scope.as_str()).collect();
    let wider = new_scopes.iter().any(|scope| !old_scopes.contains(scope))
        || origins(new)
            .iter()
            .any(|origin| !origins(&old).contains(origin));
    // Scopes the app no longer asks for go at once, whatever waits.
    let kept: Vec<String> = row
        .scopes_approved
        .iter()
        .filter(|scope| new_scopes.contains(scope.as_str()))
        .cloned()
        .collect();
    if kept.len() != row.scopes_approved.len() {
        set_scopes(transaction, row, &kept, actor_id).await?;
    }
    let stored =
        serde_json::to_value(new).map_err(|_| AppError::Internal("connected module manifest"))?;
    if wider {
        sqlx::query(
            "UPDATE module_apps SET pending_manifest = $2, pending_digest = $3, \
             manifest_fetched_at = now(), version = version + 1, updated_at = now() \
             WHERE module_id = $1",
        )
        .bind(row.module_id)
        .bind(&stored)
        .bind(&fetched.digest)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        announce(transaction, row.workspace_id, row.module_id).await?;
        return Ok(Refreshed::Pending);
    }
    apply_manifest(transaction, row, fetched).await?;
    announce(transaction, row.workspace_id, row.module_id).await?;
    Ok(Refreshed::Applied)
}

async fn apply_manifest(
    transaction: &mut Transaction<'static, Postgres>,
    row: &AppRow,
    fetched: &Fetched,
) -> Result<(), OpError> {
    let manifest = &fetched.checked.manifest;
    let stored = serde_json::to_value(manifest)
        .map_err(|_| AppError::Internal("connected module manifest"))?;
    sqlx::query(
        r#"
        UPDATE module_apps
        SET manifest = $2, manifest_digest = $3, base_url = $4, frame_origins = $5,
            scopes_requested = $6, pending_manifest = NULL, pending_digest = NULL,
            manifest_fetched_at = now(), version = version + 1, updated_at = now()
        WHERE module_id = $1
        "#,
    )
    .bind(row.module_id)
    .bind(&stored)
    .bind(&fetched.digest)
    .bind(fetched.checked.base_url.as_str())
    .bind(fetched.checked.frame_origins())
    .bind(fetched.checked.scope_keys())
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    let description: String = manifest.description.chars().take(500).collect();
    sqlx::query(
        "UPDATE modules SET name = $2, description = $3, revision = revision + 1, \
         updated_at = now() WHERE id = $1",
    )
    .bind(row.module_id)
    .bind(&manifest.name)
    .bind(&description)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    things::register_kinds(transaction, row.workspace_id, row.module_id, manifest).await
}

/// Sets the approved scopes and the role that holds them.
async fn set_scopes(
    transaction: &mut Transaction<'static, Postgres>,
    row: &AppRow,
    scopes: &[String],
    actor_id: Uuid,
) -> Result<(), OpError> {
    let permissions = permissions_of(scopes);
    let base = crate::roles::app_role_base(permissions).ok_or(OpError::invalid(
        "scopes",
        "An app can't be given what only owners hold.",
    ))?;
    crate::roles::set_app_role(
        transaction,
        row.workspace_id,
        row.role_id,
        base,
        permissions,
        actor_id,
    )
    .await?;
    sqlx::query(
        "UPDATE module_apps SET scopes_approved = $2, version = version + 1, updated_at = now() \
         WHERE module_id = $1",
    )
    .bind(row.module_id)
    .bind(scopes)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    Ok(())
}

/// Approves the pending manifest, if any, and sets the approved scopes to
/// `chosen` (every known one the manifest asks for when not given).
/// `digest`, when given, must be the manifest being approved.
pub(crate) async fn approve(
    transaction: &mut Transaction<'static, Postgres>,
    row: &AppRow,
    chosen: Option<&[String]>,
    digest: Option<&str>,
    actor_id: Uuid,
) -> Result<(), OpError> {
    live(row)?;
    let (manifest, current_digest) = match (row.pending()?, &row.pending_digest) {
        (Some(pending), Some(digest)) => (pending, digest.clone()),
        _ => (row.manifest()?, row.manifest_digest.clone()),
    };
    if digest.is_some_and(|given| given != current_digest) {
        return Err(OpError::Conflict {
            reason: ConflictReason::State {
                code: "manifest_changed",
            },
            message: "The manifest changed since you looked at it. Open the module again.".into(),
            current: None,
        });
    }
    let (approved, permissions) = approve_scopes(&manifest, chosen)?;
    may_approve(transaction, row.workspace_id, actor_id, permissions).await?;
    if row.pending_manifest.is_some() {
        sqlx::query(
            r#"
            UPDATE module_apps
            SET manifest = pending_manifest, manifest_digest = pending_digest, base_url = $2,
                frame_origins = $3, scopes_requested = $4, pending_manifest = NULL,
                pending_digest = NULL, version = version + 1, updated_at = now()
            WHERE module_id = $1
            "#,
        )
        .bind(row.module_id)
        .bind(&manifest.base_url)
        .bind(origins(&manifest).into_iter().collect::<Vec<_>>())
        .bind(
            manifest
                .scopes
                .iter()
                .map(|scope| scope.scope.clone())
                .collect::<Vec<_>>(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        let description: String = manifest.description.chars().take(500).collect();
        sqlx::query(
            "UPDATE modules SET name = $2, description = $3, revision = revision + 1, \
             updated_at = now() WHERE id = $1",
        )
        .bind(row.module_id)
        .bind(&manifest.name)
        .bind(&description)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        things::register_kinds(transaction, row.workspace_id, row.module_id, &manifest).await?;
    }
    set_scopes(transaction, row, &approved, actor_id).await?;
    sqlx::query(
        "UPDATE module_apps SET approved_by_actor_id = $2, approved_at = now() WHERE module_id = $1",
    )
    .bind(row.module_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    announce(transaction, row.workspace_id, row.module_id).await?;
    Ok(())
}

fn live(row: &AppRow) -> Result<(), OpError> {
    if row.state == "revoked" {
        return Err(OpError::Conflict {
            reason: ConflictReason::State { code: "revoked" },
            message: "This connected module was revoked. Install it again to use it.".into(),
            current: None,
        });
    }
    Ok(())
}

// ------------------------------------------------------------------ revoke

/// Revokes an install: the account is disabled and its secrets revoked
/// (its next call is refused), the signing secrets deleted, and the module
/// switched off and archived, keeping what it made.
pub(crate) async fn revoke(
    transaction: &mut Transaction<'static, Postgres>,
    row: &AppRow,
    actor_id: Uuid,
) -> Result<(), OpError> {
    live(row)?;
    sqlx::query("UPDATE actors SET status = 'disabled', updated_at = now() WHERE id = $1")
        .bind(row.account_id)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
    for credential in crate::access::credentials::list_for(transaction, row.account_id).await? {
        if credential.status == "active" {
            crate::access::credentials::revoke(transaction, credential.id).await?;
        }
    }
    for secret in [row.outbound_secret_id, row.inbound_secret_id]
        .into_iter()
        .flatten()
    {
        crate::secrets::delete(&mut **transaction, secret).await?;
    }
    crate::modules::state::write_switch(
        transaction,
        row.workspace_id,
        crate::modules::state::SwitchScope::Workspace,
        &row.module_key,
        false,
        actor_id,
    )
    .await?;
    sqlx::query(
        "UPDATE modules SET archived_at = now(), archived_parts = 'keep', \
         revision = revision + 1, updated_at = now() WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(row.module_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    sqlx::query(
        r#"
        UPDATE module_apps
        SET state = 'revoked', revoked_at = now(), revoked_by_actor_id = $2,
            install_code_digest = NULL, install_code_expires_at = NULL,
            pending_manifest = NULL, pending_digest = NULL,
            version = version + 1, updated_at = now()
        WHERE module_id = $1
        "#,
    )
    .bind(row.module_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    crate::modules::gate::forget_everywhere(transaction, row.workspace_id).await?;
    announce(transaction, row.workspace_id, row.module_id).await?;
    Ok(())
}

/// Tells the workspace's other tabs, agents and webhooks that a connected
/// module changed (installed, re-read, approved or revoked); they read it
/// again with their own access.
async fn announce(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<(), AppError> {
    let mut event = crate::events::NewEvent::thing(
        &crate::events::catalog::MODULE_CONNECTED,
        workspace_id,
        "module",
        module_id,
    )
    .audience(crate::events::Audience::Workspace);
    event.space_id = None;
    crate::events::emit(transaction, event.workspace(workspace_id)).await?;
    Ok(())
}

// ------------------------------------------------------------------ re-reading manifests

/// Looks for connected modules whose manifest wasn't read in a day, and
/// queues a re-read of each.
pub const REFETCH_SWEEP_KIND: &str = "connected_module.refetch_sweep";
/// Re-reads one connected module's manifest, as `connected_module_refresh`
/// does: a narrower one applies, a wider one waits for approval.
pub const REFETCH_KIND: &str = "connected_module.refetch";
const REFETCH_EVERY: Duration = Duration::from_secs(60 * 60);
const REFETCH_AFTER_HOURS: i32 = 24;
const REFETCH_BATCH: i64 = 50;

/// Queues the due re-reads (the oldest first, a batch an hour).
pub(crate) async fn queue_due_refetches(state: &AppState) -> Result<usize, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let due = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT module_id, workspace_id FROM module_apps \
         WHERE revoked_at IS NULL \
           AND manifest_fetched_at < now() - make_interval(hours => $1) \
         ORDER BY manifest_fetched_at LIMIT $2",
    )
    .bind(REFETCH_AFTER_HOURS)
    .bind(REFETCH_BATCH)
    .fetch_all(&mut *transaction)
    .await?;
    for (module_id, workspace_id) in &due {
        crate::jobs::enqueue(
            &mut *transaction,
            NewJob::new(REFETCH_KIND)
                .payload(json!({ "moduleId": module_id }))
                .workspace(*workspace_id)
                .unique(format!("{REFETCH_KIND}:{module_id}"))
                .max_attempts(3),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(due.len())
}

/// Re-reads one module's manifest. The fetch happens outside any
/// transaction; a module that can't be reached keeps what it has and is
/// tried again the next day.
async fn refetch(state: &AppState, workspace_id: Uuid, module_id: Uuid) -> Result<(), JobError> {
    let Some(address) = crate::db::scoped_one(&state.pool, async |connection| {
        get(connection, workspace_id, module_id, false).await
    })
    .await?
    .filter(|row| row.revoked_at.is_none())
    .map(|row| row.manifest_url) else {
        return Ok(());
    };
    let fetched = match fetch(state, &address).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(
                module_id = %module_id,
                outcome = error.outcome(),
                "connected module manifest re-read failed"
            );
            sqlx::query("UPDATE module_apps SET manifest_fetched_at = now() WHERE module_id = $1")
                .bind(module_id)
                .execute(crate::db::scoped(&state.pool))
                .await?;
            return Ok(());
        }
    };
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(row) = get(&mut transaction, workspace_id, module_id, true).await? else {
        return Ok(());
    };
    if row.revoked_at.is_some() {
        return Ok(());
    }
    // Scopes it no longer asks for are dropped in the name of whoever
    // approved it last.
    let approver = row.approved_by_actor_id;
    match refresh(&mut transaction, &row, &fetched, approver).await {
        Ok(_) => {}
        // A manifest that now names another module is left for a person
        // to look at (connected_module_refresh says why).
        Err(OpError::Invalid { .. }) => {
            transaction.rollback().await?;
            return Ok(());
        }
        Err(error) => return Err(JobError::retry(error.outcome())),
    }
    transaction.commit().await?;
    Ok(())
}

// ------------------------------------------------------------------ the install code

fn code_digest(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

/// Sends the app a fresh one-time code at its install address, signed with
/// the outbound secret. The code is made and stored in a short transaction
/// of its own; the call is made after it commits.
async fn handshake(state: &AppState, workspace_id: Uuid, module_id: Uuid) -> Result<(), JobError> {
    let mut random = [0_u8; 32];
    OsRng.fill_bytes(&mut random);
    let code = format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(random));
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(row) = get(&mut transaction, workspace_id, module_id, true).await? else {
        return Ok(());
    };
    if row.state != "pending" {
        return Ok(());
    }
    let Some(outbound) = row.outbound_secret_id else {
        return Ok(());
    };
    let keys = crate::secrets::keyring(&state.config).map_err(AppError::from)?;
    let secret = crate::secrets::reveal(
        &mut *transaction,
        keys.as_ref(),
        outbound,
        "connected module install",
    )
    .await?;
    let expires = Utc::now() + chrono::Duration::minutes(CODE_MINUTES);
    sqlx::query(
        "UPDATE module_apps SET install_code_digest = $2, install_code_expires_at = $3, \
         install_sent_at = now(), updated_at = now() WHERE module_id = $1",
    )
    .bind(module_id)
    .bind(code_digest(&code).as_slice())
    .bind(expires)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let manifest = row.manifest()?;
    let base = Url::parse(&row.base_url).map_err(|_| JobError::permanent("base url"))?;
    let install = base
        .join(&manifest.install)
        .map_err(|_| JobError::permanent("install url"))?;
    let body = json!({
        "format": "agent-workspace-install/1",
        "code": code,
        "expiresAt": expires,
        "module": { "id": module_id, "key": row.module_key },
        "workspace": { "id": workspace_id },
        "exchange": format!(
            "{}/api/v1/connected-modules/token",
            public_origin(state).trim_end_matches('/')
        ),
    })
    .to_string();
    let id = format!("msg_{}", Uuid::now_v7().simple());
    let timestamp = Utc::now().timestamp();
    let signature =
        agent_workspace_host_protocol::sign_push(secret.expose(), &id, timestamp, &body)
            .ok_or_else(|| JobError::permanent("signing secret"))?;
    let client = client(state, &install, FETCH_TIMEOUT)
        .await
        .map_err(|_| JobError::retry("the install address can't be reached"))?;
    let response = client
        .post(install)
        .header("content-type", "application/json")
        .header("webhook-id", &id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signature)
        .body(body)
        .send()
        .await
        .map_err(|_| JobError::retry("the install address didn't answer"))?;
    if !response.status().is_success() {
        return Err(JobError::retry(format!(
            "the install address answered {}",
            response.status().as_u16()
        )));
    }
    Ok(())
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register_with_timeout(
        HANDSHAKE_KIND,
        Duration::from_secs(60),
        |context: JobContext| async move {
            let module_id = context
                .job
                .payload
                .get("moduleId")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .ok_or_else(|| JobError::permanent("payload"))?;
            let workspace_id = context
                .job
                .organization_id
                .ok_or_else(|| JobError::permanent("workspace"))?;
            handshake(&context.state, workspace_id, module_id).await
        },
    );
    registry.register_with_timeout(
        files::FILE_CHECK_KIND,
        Duration::from_secs(150),
        |context: JobContext| async move {
            let module_id = context
                .job
                .payload
                .get("moduleId")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .ok_or_else(|| JobError::permanent("payload"))?;
            let workspace_id = context
                .job
                .organization_id
                .ok_or_else(|| JobError::permanent("workspace"))?;
            match files::check_and_keep(&context.state, workspace_id, module_id).await? {
                Some(_) => Ok(()),
                // It may not hold its signing secret yet: ask again later.
                None => Err(JobError::retry("the file source didn't answer")),
            }
        },
    );
    registry.register(REFETCH_SWEEP_KIND, |context: JobContext| async move {
        queue_due_refetches(&context.state).await?;
        Ok(())
    });
    registry.every(REFETCH_SWEEP_KIND, REFETCH_EVERY);
    registry.register_with_timeout(
        REFETCH_KIND,
        Duration::from_secs(60),
        |context: JobContext| async move {
            let module_id = context
                .job
                .payload
                .get("moduleId")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .ok_or_else(|| JobError::permanent("payload"))?;
            let workspace_id = context
                .job
                .organization_id
                .ok_or_else(|| JobError::permanent("workspace"))?;
            refetch(&context.state, workspace_id, module_id).await
        },
    );
}

/// Where people and apps reach this server.
fn public_origin(state: &AppState) -> String {
    state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://{}", state.config.server_addr))
}

/// What the app gets for its install code, once.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchanged {
    pub module_id: Uuid,
    pub workspace_id: Uuid,
    pub account_id: Uuid,
    /// The account's secret: a bearer for the API and MCP, or the
    /// client_secret of `/oauth/token`'s client_credentials grant.
    pub client_secret: String,
    pub token_url: String,
    /// Workspace signs what it sends the app with this (Standard Webhooks).
    pub outbound_signing_secret: String,
    /// The app signs what it sends Workspace with this.
    pub inbound_signing_secret: String,
}

/// Redeems an install code: once, before it lapses, for a module still
/// waiting for it. A second exchange (a resent code) replaces the account's
/// secret.
pub async fn exchange(state: &AppState, code: &str) -> Result<Option<Exchanged>, AppError> {
    if !code.starts_with(CODE_PREFIX) || code.len() > 100 {
        return Ok(None);
    }
    let digest = code_digest(code);
    let found = crate::tenancy::system(
        "modules.install_code",
        crate::db::scoped_one(&state.pool, async |connection| {
            Ok::<_, AppError>(
                sqlx::query_as::<_, (Uuid, Uuid)>(
                    "SELECT module_id, workspace_id FROM module_apps \
                 WHERE install_code_digest = $1 AND install_code_expires_at > now() \
                   AND state = 'pending'",
                )
                .bind(digest.as_slice())
                .fetch_optional(&mut *connection)
                .await?,
            )
        }),
    )
    .await?;
    let Some((module_id, workspace_id)) = found else {
        return Ok(None);
    };
    crate::tenancy::scoped(
        crate::tenancy::Tenancy::Workspace {
            account: None,
            workspace: workspace_id,
        },
        redeem(state, workspace_id, module_id, digest),
    )
    .await
}

async fn redeem(
    state: &AppState,
    workspace_id: Uuid,
    module_id: Uuid,
    digest: [u8; 32],
) -> Result<Option<Exchanged>, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let Some(row) = get(&mut transaction, workspace_id, module_id, true).await? else {
        return Ok(None);
    };
    let current = sqlx::query_scalar::<_, bool>(
        "SELECT install_code_digest = $2 AND install_code_expires_at > now() \
         FROM module_apps WHERE module_id = $1",
    )
    .bind(module_id)
    .bind(digest.as_slice())
    .fetch_one(&mut *transaction)
    .await?;
    let (Some(outbound), Some(inbound)) = (row.outbound_secret_id, row.inbound_secret_id) else {
        return Ok(None);
    };
    if !current || row.state != "pending" {
        return Ok(None);
    }
    if let Some(previous) = row.credential_id {
        crate::access::credentials::revoke(&mut transaction, previous).await?;
    }
    let home = crate::spaces::workspace::ensure(&mut transaction, workspace_id).await?;
    let made = crate::access::credentials::create_secret(
        &mut transaction,
        crate::access::credentials::NewSecret {
            account_id: row.account_id,
            home_space: home,
            client_name: &format!("{} module", row.module_key),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            reach: crate::access::credentials::CredentialReach::Account,
            days: SECRET_DAYS,
            connection_id: None,
            created_by: row.approved_by_actor_id,
            request_id: None,
        },
    )
    .await?;
    let secret = made
        .secret
        .ok_or(AppError::Internal("connected module secret"))?;
    let keys = crate::secrets::keyring(&state.config).map_err(AppError::from)?;
    let outbound = crate::secrets::reveal(
        &mut *transaction,
        keys.as_ref(),
        outbound,
        "connected module exchange",
    )
    .await?;
    let inbound = crate::secrets::reveal(
        &mut *transaction,
        keys.as_ref(),
        inbound,
        "connected module exchange",
    )
    .await?;
    sqlx::query(
        "UPDATE module_apps SET state = 'active', credential_id = $2, install_code_digest = NULL, \
         install_code_expires_at = NULL, version = version + 1, updated_at = now() \
         WHERE module_id = $1",
    )
    .bind(module_id)
    .bind(made.credential.id)
    .execute(&mut *transaction)
    .await?;
    // A file source runs the conformance suite once it can sign.
    if row.manifest()?.file_source.is_some() {
        crate::jobs::enqueue(
            &mut *transaction,
            NewJob::new(files::FILE_CHECK_KIND)
                .payload(json!({ "moduleId": module_id }))
                .workspace(workspace_id)
                .unique(format!("{}:{module_id}", files::FILE_CHECK_KIND))
                .max_attempts(6),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(Some(Exchanged {
        module_id,
        workspace_id,
        account_id: row.account_id,
        client_secret: secret,
        token_url: format!("{}/oauth/token", public_origin(state).trim_end_matches('/')),
        outbound_signing_secret: outbound.expose().to_owned(),
        inbound_signing_secret: inbound.expose().to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        serde_json::from_value(manifest::tests::sample("https://builds.example.com"))
            .expect("manifest")
    }

    #[test]
    fn required_scopes_go_in_and_unknown_ones_stay_out() {
        let mut manifest = manifest();
        let (all, permissions) = approve_scopes(&manifest, None).expect("everything");
        assert_eq!(all, ["members.view", "tasks.edit", "events:task.updated"]);
        assert!(permissions.contains(Permission::TasksEdit));

        let (some, permissions) =
            approve_scopes(&manifest, Some(&["members.view".to_owned()])).expect("required only");
        assert_eq!(some, ["members.view"]);
        assert!(!permissions.contains(Permission::TasksEdit));

        assert!(approve_scopes(&manifest, Some(&["tasks.edit".to_owned()])).is_err());
        assert!(approve_scopes(&manifest, Some(&["roles.manage".to_owned()])).is_err());

        manifest.scopes.push(ScopeRequest {
            scope: "tasks.fly".to_owned(),
            why: "To fly.".to_owned(),
            required: false,
        });
        let (all, _) = approve_scopes(&manifest, None).expect("unknown left out");
        assert!(!all.contains(&"tasks.fly".to_owned()));
        assert!(approve_scopes(&manifest, Some(&["tasks.fly".to_owned()])).is_err());
        manifest.scopes.last_mut().expect("scope").required = true;
        assert!(approve_scopes(&manifest, None).is_err());
    }

    #[test]
    fn handles_follow_the_key() {
        assert_eq!(handle_stem("acme.builds"), "acme-builds");
        assert_eq!(
            handle_stem("a_very_long_publisher.and_module_name"),
            "a-very-long-publisher-and-mo"
        );
    }
}
