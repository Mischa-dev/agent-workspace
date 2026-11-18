//! Status as operations (C2.15): `status_set`, `status_delete` and
//! `status_list`. The web, agents (as the tools of the same names), the v2
//! API and automations all run these. An app acting for a person sets that
//! person's status; an agent sets its own through its connection; someone
//! who manages an agent may set the agent's.

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{StatusInput, StatusView};
use crate::{
    access,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<StatusSet>()
        .add::<StatusDelete>()
        .add::<StatusList>()
        .add::<AgentPause>()
        .add::<AgentResume>();
}

/// Whose status a call changes: the caller's own, or an account they manage.
async fn account_for(call: &mut OpCall<'_>, account: Option<IdRef>) -> Result<Uuid, OpError> {
    let own = call.account_id();
    let Some(account) = account.map(IdRef::id).filter(|account| *account != own) else {
        return Ok(own);
    };
    let caller = call.ctx.caller.clone();
    let workspace_id = call.ctx.workspace_id;
    let tx = call.tx().await?;
    let member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
    )
    .bind(workspace_id)
    .bind(account)
    .fetch_one(&mut **tx)
    .await
    .map_err(crate::error::AppError::from)?;
    if !member {
        return Err(OpError::not_found("That account"));
    }
    match access::may_manage_account(tx, &caller, account).await? {
        Ok(_) => Ok(account),
        Err(access::Denied::NotFound) => Err(OpError::not_found("That account")),
        Err(_) => Err(OpError::invalid(
            "account",
            "You can set only your own status, or that of an agent you manage.",
        )),
    }
}

fn time(value: Option<&str>, field: &'static str) -> Result<Option<DateTime<Utc>>, OpError> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value.trim())
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| OpError::invalid(field, "Pass a time such as 2026-11-15T17:00:00Z."))
        })
        .transpose()
}

/// Set a status.
pub struct StatusSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusSetInput {
    /// One line, up to 120 characters ("In a meeting", "Reviewing the
    /// release notes").
    #[serde(default, alias = "line", skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// One emoji, such as 🌴.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// available, working, asleep, paused or unresponsive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// When it clears by itself (an ISO 8601 time).
    #[serde(default, alias = "until", skip_serializing_if = "Option::is_none")]
    pub clear_at: Option<String>,
    /// Away until this time (an ISO 8601 time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_until: Option<String>,
    /// true: a line about work in this workspace, shown only here. Left
    /// out, it is your status everywhere you are a member.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace_only: bool,
    /// Someone else's status: an agent you manage. Left out, your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for StatusSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for StatusSet {
    const NAME: &'static str = "status_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set status";
    const DESCRIPTION: &'static str = "Set the line and emoji shown next to your name (or an \
        agent's you manage), replacing the one there: text, emoji, state, clearAt for when it \
        clears by itself, awayUntil. Left out, it shows in every workspace you are in; with \
        workspaceOnly it is a line about work here and shows only here. Clear it with \
        status_delete.";
    const EMITS: &'static [&'static str] = &["account.status.changed"];

    type Input = StatusSetInput;
    type Output = StatusView;

    async fn run(call: &mut OpCall<'_>, input: StatusSetInput) -> Result<StatusView, OpError> {
        let now = Utc::now();
        let status = StatusInput {
            text: input.text,
            emoji: input.emoji,
            state: input.state,
            clear_at: time(input.clear_at.as_deref(), "clearAt")?,
            away_until: time(input.away_until.as_deref(), "awayUntil")?,
        }
        .checked(now)
        .map_err(|refused| OpError::invalid(refused.field, refused.message))?;
        let account = account_for(call, input.account).await?;
        let workspace = input.workspace_only.then_some(call.ctx.workspace_id);
        let setter = call.account_id();
        let here = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let view = super::set(tx, account, setter, workspace, &status).await?;
        super::announce_where_shown(tx, account, input.workspace_only.then_some(here)).await?;
        call.audit_target(
            "account",
            account,
            json!({ "workspace_only": input.workspace_only }),
        );
        Ok(view)
    }
}

/// Clear a status.
pub struct StatusDelete;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusDeleteInput {
    /// true: clear the line about work in this workspace. Left out, the
    /// status shown everywhere.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub workspace_only: bool,
    /// An agent you manage. Left out, your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for StatusDeleteInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusCleared {
    /// False when there was nothing to clear.
    pub cleared: bool,
}

impl Operation for StatusDelete {
    const NAME: &'static str = "status_delete";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Clear status";
    const DESCRIPTION: &'static str = "Clear your status (or an agent's you manage): the one \
        shown everywhere, or with workspaceOnly the line about work in this workspace.";
    const EMITS: &'static [&'static str] = &["account.status.changed"];

    type Input = StatusDeleteInput;
    type Output = StatusCleared;

    async fn run(
        call: &mut OpCall<'_>,
        input: StatusDeleteInput,
    ) -> Result<StatusCleared, OpError> {
        let account = account_for(call, input.account).await?;
        let here = call.ctx.workspace_id;
        let workspace = input.workspace_only.then_some(here);
        let tx = call.tx().await?;
        let cleared = super::clear(tx, account, workspace).await?;
        if cleared {
            super::announce_where_shown(tx, account, input.workspace_only.then_some(here)).await?;
        }
        call.audit_target(
            "account",
            account,
            json!({ "workspace_only": input.workspace_only }),
        );
        Ok(StatusCleared { cleared })
    }
}

/// Read statuses.
pub struct StatusList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusListInput {
    /// The accounts (people or agents) whose status to show; up to 200.
    pub accounts: Vec<IdRef>,
}

impl Targeted for StatusListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusListOutput {
    /// Each account's status here: its own one, and its line about work in
    /// this workspace. Accounts with none, or not in this workspace, are
    /// left out.
    pub statuses: Vec<StatusView>,
}

impl Operation for StatusList {
    const NAME: &'static str = "status_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List statuses";
    const DESCRIPTION: &'static str = "Show the statuses of people and agents in this workspace: \
        each one's own status and its line about work here.";

    type Input = StatusListInput;
    type Output = StatusListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: StatusListInput,
    ) -> Result<StatusListOutput, OpError> {
        if input.accounts.len() > super::MAX_ACCOUNTS {
            return Err(OpError::invalid(
                "accounts",
                "Ask for up to 200 accounts at a time.",
            ));
        }
        let accounts = input
            .accounts
            .iter()
            .map(|account| account.id())
            .collect::<Vec<_>>();
        let workspace_id = call.ctx.workspace_id;
        let connection = &mut **call.tx().await?;
        let shown = super::shown_in(connection, workspace_id, &accounts).await?;
        let mut statuses = shown.into_values().flatten().collect::<Vec<_>>();
        statuses.sort_by_key(|status| (status.account_id, status.workspace_only));
        Ok(StatusListOutput { statuses })
    }
}

/// Pause or resume an account (A2.10).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPauseInput {
    /// The agent (or any account you manage): its id or a link to it.
    pub account: IdRef,
    /// true: in every workspace it is in. Left out, only in this one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub everywhere: bool,
    /// Why, in a line people see (up to 200 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Targeted for AgentPauseInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PauseView {
    #[schemars(with = "String")]
    pub account: Uuid,
    /// "paused" or "resumed".
    pub status: &'static str,
    /// False when it already was.
    pub changed: bool,
}

/// What a pause or resume tells: the account itself (addressed, so it
/// hears it even while paused) and the screens showing its status.
async fn told(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    ty: &'static crate::events::catalog::EventType,
    account: Uuid,
    by: Uuid,
    here: Uuid,
    everywhere: bool,
) -> Result<(), OpError> {
    crate::events::emit(
        tx,
        crate::events::NewEvent {
            id: Uuid::now_v7(),
            ty,
            workspace_id: Some(here),
            space_id: None,
            thing: Some((account, "account")),
            access_thing: None,
            version: None,
            changes: &[],
            audience: crate::events::Audience::Account(account),
            payload: json!({ "accountId": account, "everywhere": everywhere }),
            actor: Some(by),
            legacy_type: None,
        },
    )
    .await?;
    super::announce(tx, account, here, !everywhere).await?;
    Ok(())
}

pub struct AgentPause;

impl Operation for AgentPause {
    const NAME: &'static str = "agent_pause";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Pause agent";
    const DESCRIPTION: &'static str = "Pause an agent you manage, here or everywhere: until \
        resumed it is asked nothing new, hears only about its own pause, its connections and \
        stopped replies, and its changes do nothing (each answers ok with status \"paused\"). \
        What it would have heard waits and arrives in order when it is resumed.";
    const EMITS: &'static [&'static str] = &["agent.paused", "account.status.changed"];

    type Input = AgentPauseInput;
    type Output = PauseView;

    async fn run(call: &mut OpCall<'_>, input: AgentPauseInput) -> Result<PauseView, OpError> {
        let reason = match input.reason.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(reason)
                if reason.chars().count() <= 200 && !reason.chars().any(char::is_control) =>
            {
                Some(reason.to_owned())
            }
            Some(_) => {
                return Err(OpError::invalid(
                    "reason",
                    "Say why in one line of up to 200 characters.",
                ));
            }
        };
        if input.account.id() == call.account_id() {
            return Err(OpError::invalid(
                "account",
                "You can't pause yourself; someone who manages you can.",
            ));
        }
        let account = account_for(call, Some(input.account)).await?;
        let by = call.account_id();
        let here = call.ctx.workspace_id;
        let workspace = (!input.everywhere).then_some(here);
        let tx = call.tx().await?;
        let before = super::paused_in(tx, account).await?;
        let changed = !before.contains(&workspace);
        super::pause(tx, account, workspace, by, reason.as_deref()).await?;
        if changed {
            told(
                tx,
                &crate::events::catalog::AGENT_PAUSED,
                account,
                by,
                here,
                input.everywhere,
            )
            .await?;
        }
        call.audit_target(
            "account",
            account,
            json!({ "everywhere": input.everywhere }),
        );
        Ok(PauseView {
            account,
            status: "paused",
            changed,
        })
    }
}

pub struct AgentResume;

impl Operation for AgentResume {
    const NAME: &'static str = "agent_resume";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Resume agent";
    const DESCRIPTION: &'static str = "Lift an agent's pause, here or (everywhere) the one \
        set for every workspace. What it missed arrives in order.";
    const EMITS: &'static [&'static str] = &["agent.resumed", "account.status.changed"];

    type Input = AgentPauseInput;
    type Output = PauseView;

    async fn run(call: &mut OpCall<'_>, input: AgentPauseInput) -> Result<PauseView, OpError> {
        let account = account_for(call, Some(input.account)).await?;
        let by = call.account_id();
        let here = call.ctx.workspace_id;
        let workspace = (!input.everywhere).then_some(here);
        let tx = call.tx().await?;
        let changed = super::resume(tx, account, workspace).await?;
        if changed {
            told(
                tx,
                &crate::events::catalog::AGENT_RESUMED,
                account,
                by,
                here,
                input.everywhere,
            )
            .await?;
        }
        call.audit_target(
            "account",
            account,
            json!({ "everywhere": input.everywhere }),
        );
        Ok(PauseView {
            account,
            status: "resumed",
            changed,
        })
    }
}
