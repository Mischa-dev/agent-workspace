//! A2.19: offers, registration and actions (A2 3.9, rulings R19 and R21).
//!
//! - `agent_offer_list`: the ways to get a new agent that the hosts a
//!   person may use here publish, in the hosts' own words.
//! - A host proposes an agent it made (`POST /api/v1/host/agents/register`):
//!   a profile, its own name for it, an outside reference, and the
//!   workspace. That opens an approval request to the person who chose the
//!   offer (or the host's owner, or the workspace's owners and admins).
//! - `agent_register_decide` (or answering the request) approves it with a
//!   role, which makes the account (owned as the host is), its membership,
//!   and the host's binding to it; or refuses it.
//! - `agent_action_run` / `agent_action_get`: run one of the actions a
//!   host publishes for an agent or for itself, and read its reply.
//!
//! The server never knows what an offer or action means; ids are the
//! host's.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{HostRow, Owner as HostOwner, protocol::HostError};
use crate::{
    access::{self, Caller, Scope},
    accounts::{NewAgent, Owner, create_agent},
    error::AppError,
    invitations::engine,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// The subject of the approval request a proposal opens.
pub(crate) const REGISTER_SUBJECT: &str = "agent_register";

pub fn register_ops(registry: &mut Registry) {
    registry.add::<AgentOfferList>();
    registry.add::<AgentOfferStart>();
    registry.add::<AgentRegisterDecide>();
    registry.add::<AgentActionRun>();
    registry.add::<AgentActionGet>();
}

// ------------------------------------------------------------- the host

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterProfile {
    pub name: String,
    pub handle: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsideRef {
    pub issuer: String,
    pub id: String,
    pub lineage: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    /// The workspace the agent is for.
    pub workspace_id: Uuid,
    /// The host's own name for the agent.
    pub host_agent_ref: String,
    pub profile: RegisterProfile,
    #[serde(rename = "ref")]
    pub reference: Option<OutsideRef>,
    /// Who chose the offer, when the host knows (it is asked first).
    pub requested_by: Option<Uuid>,
    /// Retrying with the same id makes one proposal.
    pub client_request_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Registered {
    pub proposal_id: Uuid,
    /// proposed, approved, refused or expired.
    pub status: String,
    pub request_id: Option<Uuid>,
    /// The agent's account, once approved.
    pub account_id: Option<Uuid>,
}

fn invalid(field: &'static str, message: &str) -> HostError {
    HostError::Invalid {
        field,
        message: message.to_owned(),
    }
}

fn one_line(value: &str, max: usize) -> Option<String> {
    let value = crate::run_log::redact(value.trim());
    (!value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control))
        .then_some(value)
}

#[derive(Clone, Debug, FromRow)]
struct ProposalRow {
    id: Uuid,
    host_id: Uuid,
    host_agent_ref: String,
    display_name: String,
    handle: String,
    status: String,
    request_id: Option<Uuid>,
    account_id: Option<Uuid>,
}

const PROPOSAL_COLUMNS: &str =
    "id, host_id, host_agent_ref, display_name, handle, status, request_id, account_id";

impl ProposalRow {
    fn registered(&self) -> Registered {
        Registered {
            proposal_id: self.id,
            status: self.status.clone(),
            request_id: self.request_id,
            account_id: self.account_id,
        }
    }
}

/// Whether the host may offer agents in `workspace_id`: it is the
/// workspace's, or its owner is an enabled member there (R19, R21).
async fn offers_here(
    connection: &mut PgConnection,
    host: &HostRow,
    workspace_id: Uuid,
) -> Result<bool, AppError> {
    Ok(match host.owner() {
        HostOwner::Workspace(owner) => owner == workspace_id,
        HostOwner::Account(owner) => member(connection, workspace_id, owner).await?,
    })
}

async fn member(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM organization_memberships membership
            JOIN actors account ON account.id = membership.actor_id AND account.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
        )
        "#,
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// Whether `account` may add agents in `workspace_id` (`agents.create`):
/// only such a person is asked, and only their answer adds the agent.
async fn may_add_agents(
    connection: &mut PgConnection,
    workspace_id: Uuid,
    account_id: Uuid,
) -> Result<bool, AppError> {
    match crate::roles::standing(connection, workspace_id, account_id).await {
        Ok(standing) => Ok(standing.may(crate::policy::Permission::AgentsCreate)),
        Err(AppError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// A host proposes an agent. Retried with the same `clientRequestId`, it
/// gets the same proposal back.
pub async fn propose(
    state: &crate::AppState,
    host: &HostRow,
    request: RegisterRequest,
) -> Result<Registered, HostError> {
    let reference = super::host_agent_ref(&request.host_agent_ref).map_err(HostError::App)?;
    let name = one_line(&request.profile.name, 80).ok_or_else(|| {
        invalid(
            "profile.name",
            "Give the agent a name: one line of at most 80 characters.",
        )
    })?;
    let handle = match request.profile.handle.as_deref() {
        Some(handle) => crate::workspace::normalize_handle(handle).map_err(|_| {
            invalid(
                "profile.handle",
                "Use 2 to 32 lowercase letters, digits, - or _, starting with a letter or digit.",
            )
        })?,
        None => crate::workspace::normalize_handle(&name).unwrap_or_else(|_| "agent".to_owned()),
    };
    let summary = match request.profile.summary.as_deref() {
        Some(summary) => Some(one_line(summary, 280).ok_or_else(|| {
            invalid(
                "profile.summary",
                "Keep the summary to one line of 280 characters.",
            )
        })?),
        None => None,
    };
    let client_request_id = one_line(&request.client_request_id, 200).ok_or_else(|| {
        invalid(
            "clientRequestId",
            "Send a clientRequestId of at most 200 characters.",
        )
    })?;
    let outside = request
        .reference
        .as_ref()
        .map(|outside| {
            Ok::<_, HostError>((
                one_line(&outside.issuer, 200)
                    .ok_or_else(|| invalid("ref.issuer", "Give the issuer on one line."))?,
                one_line(&outside.id, 200)
                    .ok_or_else(|| invalid("ref.id", "Give the id on one line."))?,
                outside
                    .lineage
                    .as_deref()
                    .and_then(|lineage| one_line(lineage, 200)),
            ))
        })
        .transpose()?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    if let Some(found) = sqlx::query_as::<_, ProposalRow>(&format!(
        "SELECT {PROPOSAL_COLUMNS} FROM agent_proposals WHERE host_id = $1 AND client_request_id = $2"
    ))
    .bind(host.id)
    .bind(&client_request_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(AppError::from)?
    {
        transaction.commit().await.map_err(AppError::from)?;
        return Ok(found.registered());
    }
    if !offers_here(&mut transaction, host, request.workspace_id).await? {
        return Err(invalid(
            "workspaceId",
            "This host can't offer agents in that workspace: it isn't the workspace's, and its owner isn't a member there.",
        ));
    }
    // Who is asked: the person who chose the offer, else the host's owner,
    // if they may add agents here; else the workspace's owners and admins.
    let mut asked = None;
    let owner = match host.owner() {
        HostOwner::Account(owner) => Some(owner),
        HostOwner::Workspace(_) => None,
    };
    for person in [request.requested_by, owner].into_iter().flatten() {
        if member(&mut transaction, request.workspace_id, person).await?
            && may_add_agents(&mut transaction, request.workspace_id, person).await?
        {
            asked = Some(person);
            break;
        }
    }
    let proposal_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_proposals (
            id, workspace_id, host_id, host_agent_ref, display_name, handle, summary,
            ref_issuer, ref_id, ref_lineage, asked_account_id, client_request_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(proposal_id)
    .bind(request.workspace_id)
    .bind(host.id)
    .bind(&reference)
    .bind(&name)
    .bind(&handle)
    .bind(&summary)
    .bind(outside.as_ref().map(|outside| outside.0.clone()))
    .bind(outside.as_ref().map(|outside| outside.1.clone()))
    .bind(outside.as_ref().and_then(|outside| outside.2.clone()))
    .bind(asked)
    .bind(&client_request_id)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::from)?;
    let space_id = crate::spaces::workspace::find(&mut transaction, request.workspace_id).await?;
    let audience = asked.map_or_else(
        || json!({"roles": ["owner", "admin"]}),
        |person| json!({"accounts": [person]}),
    );
    let raised = crate::requests::open(
        &mut transaction,
        crate::requests::Origin {
            account_id: host.account_id,
            workspace_id: request.workspace_id,
            space_id,
            run_id: None,
            host_request_ref: None,
            conversation_id: None,
            work_item_id: None,
            subject: Some((REGISTER_SUBJECT, proposal_id)),
            field: "workspaceId",
        },
        &json!({
            "kind": "approval",
            "title": "Add an agent a host made?",
            "detail": match &summary {
                Some(summary) => format!("@{handle} ({name}), from {}: {summary}", host.name),
                None => format!("@{handle} ({name}), from {}.", host.name),
            },
            "options": [
                {"id": "approve", "label": "Add it", "style": "approve"},
                {"id": "deny", "label": "Don't add it", "style": "deny"},
            ],
            "allowText": false,
            "audience": audience,
            "expiresInSeconds": 7 * 24 * 3600,
        }),
    )
    .await
    .map_err(|error| match error {
        crate::requests::OpenError::App(error) => HostError::App(error),
        crate::requests::OpenError::Refused { message, .. } => HostError::Invalid {
            field: "workspaceId",
            message: message.to_string(),
        },
    })?;
    sqlx::query("UPDATE agent_proposals SET request_id = $2 WHERE id = $1")
        .bind(proposal_id)
        .bind(raised.request_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::from)?;
    crate::requests::announce_raised(&mut transaction, raised.request_id).await?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok(Registered {
        proposal_id,
        status: "proposed".to_owned(),
        request_id: Some(raised.request_id),
        account_id: None,
    })
}

// --------------------------------------------------------------- decide

/// Approve or refuse a proposal: on approval, the account (owned as the
/// host is), its membership with the role, and the host's binding.
async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    proposal_id: Uuid,
    workspace_id: Uuid,
    approve: bool,
    role_id: Option<Uuid>,
) -> Result<ProposalRow, OpError> {
    let proposal = sqlx::query_as::<_, ProposalRow>(&format!(
        "SELECT {PROPOSAL_COLUMNS} FROM agent_proposals WHERE id = $1 AND workspace_id = $2 FOR UPDATE"
    ))
    .bind(proposal_id)
    .bind(workspace_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That proposal"))?;
    if proposal.status != "proposed" {
        return Ok(proposal);
    }
    if !approve {
        return settle(transaction, proposal.id, "refused", None).await;
    }
    let host = super::get(transaction, proposal.host_id)
        .await?
        .filter(|host| host.status != "revoked")
        .ok_or_else(|| {
            OpError::invalid(
                "proposal",
                "That host was unlinked; it can't run the agent.",
            )
        })?;
    let (role, role_id) = engine::agent_role(transaction, workspace_id, role_id)
        .await?
        .ok_or_else(|| OpError::not_found("That role"))?;
    if engine::is_ownership(&role) {
        return Err(OpError::invalid(
            "roleId",
            "Ownership is handed over with Transfer ownership, not given on arrival.",
        ));
    }
    if !engine::issuer_may_grant(
        transaction,
        caller,
        Scope::Workspace(workspace_id),
        &role,
        role_id,
    )
    .await?
    {
        return Err(OpError::invalid(
            "roleId",
            "You can only give a role within what you hold yourself.",
        ));
    }
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_actor_profiles \
         WHERE organization_id = $1 AND lower(handle) = lower($2))",
    )
    .bind(workspace_id)
    .bind(&proposal.handle)
    .fetch_one(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    if taken {
        return Err(OpError::invalid(
            "proposal",
            "Someone here already has that agent's handle. Ask the host to propose it with another.",
        ));
    }
    let owner = match host.owner() {
        HostOwner::Account(owner) => Owner::Account(owner),
        HostOwner::Workspace(owner) => Owner::Workspace(owner),
    };
    let account_id = create_agent(
        transaction,
        NewAgent {
            workspace_id,
            role: &role,
            role_id,
            owner,
            created_by: caller.account_id,
        },
    )
    .await
    .map_err(AppError::from)?;
    sqlx::query(
        "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(&proposal.display_name)
    .bind(&proposal.handle)
    .execute(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    engine::join_workspace_space(transaction, workspace_id, account_id).await?;
    super::write_binding(
        transaction,
        &host,
        account_id,
        &proposal.host_agent_ref,
        caller.account_id,
    )
    .await?;
    super::bump(transaction, host.id).await?;
    settle(transaction, proposal.id, "approved", Some(account_id)).await
}

async fn settle(
    transaction: &mut Transaction<'_, Postgres>,
    proposal_id: Uuid,
    status: &str,
    account_id: Option<Uuid>,
) -> Result<ProposalRow, OpError> {
    Ok(sqlx::query_as::<_, ProposalRow>(&format!(
        "UPDATE agent_proposals SET status = $2, account_id = $3, settled_at = now() \
         WHERE id = $1 RETURNING {PROPOSAL_COLUMNS}"
    ))
    .bind(proposal_id)
    .bind(status)
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(AppError::from)?)
}

/// The proposal's request was answered or ended: approve it with the
/// workspace's role for new agents, or close it.
pub(crate) async fn settled(
    transaction: &mut Transaction<'_, Postgres>,
    row: &crate::requests::RequestRow,
    by: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(proposal_id) = row.subject_id else {
        return Ok(());
    };
    let approved = row.status == "answered"
        && row
            .answer
            .as_ref()
            .and_then(|answer| answer.get("optionId"))
            .and_then(Value::as_str)
            == Some("approve");
    if approved
        && let Some(by) = by
        && may_add_agents(transaction, row.workspace_id, by).await?
    {
        let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
            .bind(by)
            .fetch_one(&mut **transaction)
            .await?;
        let caller = Caller::person(by, crate::accounts::actor_kind(&kind)?);
        return match decide(
            transaction,
            &caller,
            proposal_id,
            row.workspace_id,
            true,
            None,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(OpError::Invalid { .. }) => Err(AppError::Validation("proposal")),
            Err(OpError::NotFound { .. }) => Err(AppError::NotFound("proposal")),
            Err(_) => Err(AppError::Internal("approve proposal")),
        };
    }
    let status = match row.status.as_str() {
        "expired" => "expired",
        _ => "refused",
    };
    sqlx::query(
        "UPDATE agent_proposals SET status = $2, settled_at = now() WHERE id = $1 AND status = 'proposed'",
    )
    .bind(proposal_id)
    .bind(status)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub struct AgentRegisterDecide;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRegisterDecideInput {
    /// The proposal: its id, from the request.
    pub proposal: IdRef,
    /// approve or refuse.
    pub decision: String,
    /// Its role here. Default: the workspace's role for new agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<IdRef>,
}

impl Targeted for AgentRegisterDecideInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    #[schemars(with = "String")]
    pub proposal_id: Uuid,
    /// proposed, approved, refused or expired.
    pub status: String,
    #[schemars(with = "Option<String>")]
    pub account_id: Option<Uuid>,
}

impl Operation for AgentRegisterDecide {
    const NAME: &'static str = "agent_register_decide";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["host.agent_registered"];
    const TITLE: &'static str = "Approve an agent a host made";
    const DESCRIPTION: &'static str = "Approve or refuse an agent a host proposed, with its role \
        here.";

    type Input = AgentRegisterDecideInput;
    type Output = ProposalView;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentRegisterDecideInput,
    ) -> Result<ProposalView, OpError> {
        let approve = match input.decision.as_str() {
            "approve" => true,
            "refuse" => false,
            _ => return Err(OpError::invalid("decision", "Say approve or refuse.")),
        };
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let proposal = decide(
            tx,
            &caller,
            input.proposal.id(),
            workspace_id,
            approve,
            input.role_id.map(IdRef::id),
        )
        .await?;
        if proposal.status != "proposed" {
            crate::requests::withdraw_about(tx, REGISTER_SUBJECT, &[proposal.id]).await?;
        }
        call.audit_target(
            "agent_proposal",
            proposal.id,
            json!({"status": proposal.status}),
        );
        Ok(ProposalView {
            proposal_id: proposal.id,
            status: proposal.status,
            account_id: proposal.account_id,
        })
    }
}

// --------------------------------------------------------------- offers

pub struct AgentOfferList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOfferListInput {}

impl Targeted for AgentOfferListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    #[schemars(with = "String")]
    pub host_id: Uuid,
    pub host_name: String,
    /// The offer as the host wrote it: `{id, title, summary?, start}`.
    #[schemars(with = "serde_json::Map<String, Value>")]
    pub offer: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OffersView {
    pub offers: Vec<Offer>,
}

impl Operation for AgentOfferList {
    const NAME: &'static str = "agent_offer_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Ways to get a new agent";
    const DESCRIPTION: &'static str = "The offers of the hosts you may use here: yours and this \
        workspace's.";

    type Input = AgentOfferListInput;
    type Output = OffersView;

    async fn run(
        call: &mut OpCall<'_>,
        _input: AgentOfferListInput,
    ) -> Result<OffersView, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let hosts = sqlx::query_as::<_, HostRow>(&format!(
            r#"
            SELECT {columns} FROM hosts host
            WHERE host.status = 'active' AND NOT host.builtin
              AND (host.owner_workspace_id = $1 OR host.owner_account_id = $2)
            ORDER BY host.created_at, host.id
            LIMIT 100
            "#,
            columns = super::HOST_COLUMNS
        ))
        .bind(workspace_id)
        .bind(caller)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let offers = hosts
            .iter()
            .flat_map(|host| {
                host.offers
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|offer| Offer {
                        host_id: host.id,
                        host_name: host.name.clone(),
                        offer: offer.clone(),
                    })
            })
            .collect();
        Ok(OffersView { offers })
    }
}

/// A host the caller may get agents from here: the workspace's or their
/// own, linked, not the built-in one.
async fn offering_host(
    connection: &mut PgConnection,
    host_id: Uuid,
    workspace_id: Uuid,
    caller: Uuid,
) -> Result<Option<HostRow>, AppError> {
    Ok(super::get(connection, host_id).await?.filter(|host| {
        host.status == "active"
            && !host.builtin
            && match host.owner() {
                HostOwner::Workspace(owner) => owner == workspace_id,
                HostOwner::Account(owner) => owner == caller,
            }
    }))
}

/// The page an offer starts on, when it names one on the host's own
/// origin (its issuer, approved at enrolment). Anything else is not opened.
fn offer_page(host: &HostRow, offer: &Value, command_id: Uuid) -> Option<String> {
    let page = offer["start"]["page"].as_str()?;
    let page = url::Url::parse(page).ok()?;
    let issuer = url::Url::parse(host.issuer.as_deref()?).ok()?;
    if page.scheme() != "https" || page.origin() != issuer.origin() {
        return None;
    }
    let mut page = page;
    page.query_pairs_mut()
        .append_pair("command", &command_id.to_string());
    Some(page.into())
}

pub struct AgentOfferStart;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOfferStartInput {
    /// The host whose offer it is (from agent_offer_list).
    pub host: IdRef,
    /// The offer's id, as the host published it.
    pub offer_id: String,
}

impl Targeted for AgentOfferStartInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OfferStarted {
    /// The command the host was sent; read it with agent_action_get.
    #[schemars(with = "String")]
    pub command_id: Uuid,
    /// The host's page to carry on at, when the offer names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl Operation for AgentOfferStart {
    const NAME: &'static str = "agent_offer_start";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = Some(Perm::AgentsCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["host.action_run"];
    const TITLE: &'static str = "Start a host's offer";
    const DESCRIPTION: &'static str = "Tell a host you chose one of its offers. It sets the \
        agent up, then proposes it here for you to approve.";

    type Input = AgentOfferStartInput;
    type Output = OfferStarted;

    async fn run(
        call: &mut OpCall<'_>,
        input: AgentOfferStartInput,
    ) -> Result<OfferStarted, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let caller = call.account_id();
        let tx = call.tx().await?;
        let host = offering_host(tx, input.host.id(), workspace_id, caller)
            .await?
            .ok_or_else(|| OpError::not_found("That host"))?;
        let offer = host
            .offers
            .as_array()
            .and_then(|offers| {
                offers
                    .iter()
                    .find(|offer| offer["id"] == input.offer_id.as_str())
            })
            .cloned()
            .ok_or_else(|| OpError::invalid("offerId", "That host publishes no such offer."))?;
        let command = super::commands::issue(
            tx,
            super::commands::NewCommand {
                workspace_id,
                host_id: host.id,
                run_id: None,
                request_id: None,
                kind: agent_workspace_host_protocol::CommandKind::Action,
                start_key: None,
                body: json!({
                    "offer": input.offer_id,
                    "workspaceId": workspace_id,
                    "requestedBy": caller,
                }),
            },
        )
        .await?;
        mark_invoker(tx, command.id, caller).await?;
        call.audit_target("host", host.id, json!({"offer": offer["id"]}));
        Ok(OfferStarted {
            command_id: command.id,
            page: offer_page(&host, &offer, command.id),
        })
    }
}

/// Who ran an action: only they read its reply.
async fn mark_invoker(
    transaction: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    account_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("UPDATE host_commands SET invoked_by_account_id = $2 WHERE id = $1")
        .bind(command_id)
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

// -------------------------------------------------------------- actions

pub struct AgentActionRun;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActionRunInput {
    /// The agent whose action it is; or give `host`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
    /// A host, for one of its own actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<IdRef>,
    /// The action's id, as the host published it.
    pub action_id: String,
    /// What the action's panel asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub input: Option<Value>,
}

impl Targeted for AgentActionRunInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    /// Read its reply with agent_action_get.
    #[schemars(with = "String")]
    pub command_id: Uuid,
    /// pending, fetched, acked, uncertain, failed or expired.
    pub state: String,
    /// The host's reply (a result or a panel), once it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub reply: Option<Value>,
}

fn declared(actions: &Value, action_id: &str) -> bool {
    actions
        .as_array()
        .is_some_and(|actions| actions.iter().any(|action| action["id"] == action_id))
}

impl Operation for AgentActionRun {
    const NAME: &'static str = "agent_action_run";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["host.action_run"];
    const TITLE: &'static str = "Run a host's action";
    const DESCRIPTION: &'static str = "Run an action a host publishes for an agent you manage, or \
        for a host you manage.";

    type Input = AgentActionRunInput;
    type Output = ActionView;

    async fn run(call: &mut OpCall<'_>, input: AgentActionRunInput) -> Result<ActionView, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let caller = call.ctx.caller.clone();
        let tx = call.tx().await?;
        let input_value = input.input.unwrap_or_else(|| json!({}));
        if serde_json::to_vec(&input_value).map_or(true, |bytes| bytes.len() > 16 * 1024) {
            return Err(OpError::invalid("input", "Keep the input under 16 KB."));
        }
        let (host_id, reference) = match (input.account, input.host) {
            (Some(account), None) => {
                let account = account.id();
                if access::may_manage_account(tx, &caller, account)
                    .await?
                    .is_err()
                {
                    return Err(OpError::not_found("That agent"));
                }
                let binding = sqlx::query_as::<_, (Uuid, String, Value)>(
                    r#"
                    SELECT binding.host_id, binding.host_agent_ref, binding.actions
                    FROM host_agents binding
                    JOIN hosts host ON host.id = binding.host_id AND host.status <> 'revoked'
                    WHERE binding.account_id = $1 AND binding.status = 'active' AND NOT host.builtin
                    ORDER BY host.last_seen_at DESC NULLS LAST LIMIT 1
                    "#,
                )
                .bind(account)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?
                .ok_or_else(|| OpError::invalid("account", "No host runs that agent."))?;
                if !declared(&binding.2, &input.action_id) {
                    return Err(OpError::invalid(
                        "actionId",
                        "Its host publishes no such action.",
                    ));
                }
                (binding.0, Some(binding.1))
            }
            (None, Some(host)) => {
                let host = super::get(tx, host.id())
                    .await?
                    .filter(|host| host.status != "revoked")
                    .ok_or_else(|| OpError::not_found("That host"))?;
                if !super::may_manage(tx, caller.account_id, &host).await? {
                    return Err(OpError::not_found("That host"));
                }
                if !declared(&host.actions, &input.action_id) {
                    return Err(OpError::invalid(
                        "actionId",
                        "That host publishes no such action.",
                    ));
                }
                (host.id, None)
            }
            _ => {
                return Err(OpError::invalid(
                    "account",
                    "Give the agent or the host, not both.",
                ));
            }
        };
        let command = super::commands::issue(
            tx,
            super::commands::NewCommand {
                workspace_id,
                host_id,
                run_id: None,
                request_id: None,
                kind: agent_workspace_host_protocol::CommandKind::Action,
                start_key: None,
                body: json!({
                    "hostAgentRef": reference,
                    "action": input.action_id,
                    "input": input_value,
                }),
            },
        )
        .await?;
        mark_invoker(tx, command.id, caller.account_id).await?;
        call.audit_target("host", host_id, json!({"action": command.body["action"]}));
        Ok(ActionView {
            command_id: command.id,
            state: command.state,
            reply: None,
        })
    }
}

pub struct AgentActionGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActionGetInput {
    /// The commandId agent_action_run gave.
    pub command_id: IdRef,
}

impl Targeted for AgentActionGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for AgentActionGet {
    const NAME: &'static str = "agent_action_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "A host action's reply";
    const DESCRIPTION: &'static str = "Where an action you ran stands, and the host's reply.";

    type Input = AgentActionGetInput;
    type Output = ActionView;

    async fn run(call: &mut OpCall<'_>, input: AgentActionGetInput) -> Result<ActionView, OpError> {
        let workspace_id = call.ctx.workspace_id;
        let caller = call.account_id();
        let tx = call.tx().await?;
        // Only the caller's own.
        let row = sqlx::query_as::<_, (Uuid, String, Option<Value>)>(
            r#"
            SELECT command.id, command.state, command.ack
            FROM host_commands command
            WHERE command.id = $1 AND command.workspace_id = $2 AND command.kind = 'action'
              AND command.invoked_by_account_id = $3
            "#,
        )
        .bind(input.command_id.id())
        .bind(workspace_id)
        .bind(caller)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That action"))?;
        Ok(ActionView {
            command_id: row.0,
            state: row.1,
            reply: row.2,
        })
    }
}
