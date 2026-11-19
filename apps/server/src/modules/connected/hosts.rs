//! M1.16: an agent host and its connected module. A host whose describe
//! names a module manifest (`hosts.module_manifest_url`) draws its steps as
//! that module's panels: an offer whose `start` names a panel opens it, an
//! action whose reply names a panel opens it, and a panel that finishes
//! with an `agentRef` hands it on only when that host has proposed that
//! agent here (`agent_register`) and the proposal still waits.
//!
//! A host is joined to the connected module in its workspace whose
//! manifest address is the one the host described, or else to the one
//! acting as the host's own account. Nothing else ties them: not the
//! host's issuer, not a name. Workspace knows no host or panel by name.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::panels;
use crate::error::AppError;

/// A connected module's panel to open.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModulePanelRef {
    /// Open it with module_panel_open.
    #[schemars(with = "String")]
    pub module_id: Uuid,
    pub panel: String,
}

/// The joined pair, read the same way both ways: the manifest address the
/// host described first, then the host's own account.
const JOIN_SQL: &str = "((host.module_manifest_url IS NOT NULL \
      AND app.manifest_url = host.module_manifest_url) \
     OR app.account_id = host.account_id)";

/// The connected module a host draws its steps with, in `workspace`.
pub async fn module_of_host(
    connection: &mut PgConnection,
    workspace: Uuid,
    host: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT app.module_id FROM hosts host \
         JOIN module_apps app ON app.workspace_id = $2 AND app.state = 'active' AND {JOIN_SQL} \
         WHERE host.id = $1 AND host.status = 'active' AND NOT host.builtin \
         ORDER BY (host.module_manifest_url IS NOT NULL \
                   AND app.manifest_url = host.module_manifest_url) DESC, \
                  app.created_at, app.module_id \
         LIMIT 1"
    ))
    .bind(host)
    .bind(workspace)
    .fetch_optional(connection)
    .await?)
}

/// The panel `panel` of the module `host` is joined to, when that module
/// declares it and may be drawn here; otherwise nothing opens.
pub async fn host_panel(
    connection: &mut PgConnection,
    workspace: Uuid,
    host: Uuid,
    panel: &str,
) -> Result<Option<ModulePanelRef>, AppError> {
    let Some(module) = module_of_host(connection, workspace, host).await? else {
        return Ok(None);
    };
    Ok(panels::declares(connection, workspace, module, panel)
        .await?
        .then(|| ModulePanelRef {
            module_id: module,
            panel: panel.to_owned(),
        }))
}

/// A waiting proposal an `agentRef` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vouched {
    pub proposal_id: Uuid,
    pub request_id: Option<Uuid>,
}

/// The waiting `agent_register` proposal, in `workspace`, from a host
/// joined to `module`, that `agent_ref` names: the host's own name for the
/// agent (a string) or its outside reference (`{issuer, id}`). The one
/// asked to approve it (`viewer`) comes first.
pub async fn vouch(
    connection: &mut PgConnection,
    workspace: Uuid,
    module: Uuid,
    agent_ref: &Value,
    viewer: Uuid,
) -> Result<Option<Vouched>, AppError> {
    let (by_name, issuer, id) = match agent_ref {
        Value::String(name) => (Some(name.as_str()), None, None),
        Value::Object(reference) => match (
            reference.get("issuer").and_then(Value::as_str),
            reference.get("id").and_then(Value::as_str),
        ) {
            (Some(issuer), Some(id)) => (None, Some(issuer), Some(id)),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    let found = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(&format!(
        "SELECT proposal.id, proposal.request_id FROM agent_proposals proposal \
         JOIN hosts host ON host.id = proposal.host_id AND host.status = 'active' \
              AND NOT host.builtin \
         JOIN module_apps app ON app.module_id = $2 AND app.workspace_id = proposal.workspace_id \
              AND app.state = 'active' AND {JOIN_SQL} \
         WHERE proposal.workspace_id = $1 AND proposal.status = 'proposed' \
           AND (($3::text IS NOT NULL AND proposal.host_agent_ref = $3) \
             OR ($4::text IS NOT NULL AND proposal.ref_issuer = $4 AND proposal.ref_id = $5)) \
         ORDER BY (proposal.asked_account_id = $6) DESC NULLS LAST, proposal.created_at DESC \
         LIMIT 1"
    ))
    .bind(workspace)
    .bind(module)
    .bind(by_name)
    .bind(issuer)
    .bind(id)
    .bind(viewer)
    .fetch_optional(connection)
    .await?;
    Ok(found.map(|(proposal_id, request_id)| Vouched {
        proposal_id,
        request_id,
    }))
}

/// A `done` result as it is passed on: an `agentRef` stays only beside the
/// proposal it names (`proposalId`, and `requestId` to answer it); one
/// that names no waiting proposal from the module's host is dropped.
pub async fn checked_result(
    connection: &mut PgConnection,
    workspace: Uuid,
    module: Uuid,
    viewer: Uuid,
    result: &mut Value,
) -> Result<(), AppError> {
    let Some(object) = result.as_object_mut() else {
        return Ok(());
    };
    // Only this server says which proposal it is.
    object.remove("proposalId");
    object.remove("requestId");
    let Some(agent_ref) = object.get("agentRef").cloned() else {
        return Ok(());
    };
    match vouch(connection, workspace, module, &agent_ref, viewer).await? {
        Some(vouched) => {
            object.insert(
                "proposalId".to_owned(),
                vouched.proposal_id.to_string().into(),
            );
            if let Some(request) = vouched.request_id {
                object.insert("requestId".to_owned(), request.to_string().into());
            }
        }
        None => {
            object.remove("agentRef");
        }
    }
    Ok(())
}
