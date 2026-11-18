//! `budget_list` and `budget_set` (A2.14): a workspace's budgets over its
//! `spend_limits`, for whoever manages its settings. Use is someone's
//! activity, so only they read it; the one refused is told why in the
//! refusal.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::AppError,
    metering::{self, DEFAULT_MESSAGE_RATE, MESSAGES_METER, METERS, MeterKind, Scope},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<BudgetSet>().add::<BudgetList>();
}

/// What a budget is on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// The whole workspace.
    Workspace,
    /// One person, agent or app.
    Account,
    /// One space.
    Space,
    /// One conversation.
    Conversation,
}

impl BudgetScope {
    const fn key(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Account => "account",
            Self::Space => "space",
            Self::Conversation => "conversation",
        }
    }
}

/// How long a budget counts over.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPeriod {
    /// The last minute.
    Minute,
    /// The last hour.
    Hour,
    /// The day (UTC).
    Day,
    /// The month (UTC).
    Month,
}

impl BudgetPeriod {
    const fn key(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Month => "month",
        }
    }
}

/// One budget.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BudgetView {
    /// Null for the default message rate, until someone sets it.
    #[schemars(with = "Option<String>")]
    pub id: Option<Uuid>,
    /// workspace, account, space or conversation.
    pub scope: String,
    /// The account, space or conversation; null for the workspace.
    #[schemars(with = "Option<String>")]
    pub scope_id: Option<Uuid>,
    /// Each account in the scope on its own (minute and hour budgets).
    pub each_account: bool,
    pub meter: String,
    /// What the meter counts, in words.
    pub meter_name: String,
    /// count, micros (millionths of the currency), or tokens.
    pub unit: String,
    /// minute, hour, day or month.
    pub period: String,
    /// Past this, whoever set it is told.
    pub soft_limit: Option<i64>,
    /// Past this, more is refused (pushes wait for the next period).
    pub hard_limit: Option<i64>,
    /// Use so far in the period; for each account on its own, the account
    /// asked about (or the caller).
    pub used: i64,
    #[schemars(with = "Option<String>")]
    pub set_by: Option<Uuid>,
    pub version: Option<i64>,
    /// The default message rate, not a budget anyone set.
    pub is_default: bool,
}

#[derive(sqlx::FromRow)]
struct BudgetRow {
    id: Uuid,
    scope_kind: String,
    scope_id: Uuid,
    per_account: bool,
    meter: String,
    period: String,
    soft_limit: Option<i64>,
    hard_limit: Option<i64>,
    set_by_actor_id: Uuid,
    version: i64,
}

const SELECT: &str = "SELECT id, scope_kind, scope_id, per_account, meter, period, soft_limit, \
    hard_limit, set_by_actor_id, version FROM spend_limits";

/// A meter's words and unit, for a counted meter.
fn meter_of(id: &str) -> Option<&'static metering::Meter> {
    METERS
        .iter()
        .find(|meter| meter.id == id && meter.kind == MeterKind::Counter)
}

async fn view(
    connection: &mut sqlx::PgConnection,
    workspace: Uuid,
    row: BudgetRow,
    account: Uuid,
) -> Result<BudgetView, AppError> {
    let scope = match row.scope_kind.as_str() {
        "account" => Scope::Account(row.scope_id),
        "space" => Scope::Space(row.scope_id),
        "conversation" => Scope::Conversation(row.scope_id),
        _ => Scope::Workspace,
    };
    let used = if row.per_account {
        metering::budget_used(
            connection,
            workspace,
            &row.meter,
            Scope::Account(account),
            Some(scope),
            &row.period,
        )
        .await?
    } else {
        metering::budget_used(connection, workspace, &row.meter, scope, None, &row.period).await?
    };
    let meter = meter_of(&row.meter);
    Ok(BudgetView {
        id: Some(row.id),
        scope_id: (row.scope_kind != BudgetScope::Workspace.key()).then_some(row.scope_id),
        scope: row.scope_kind,
        each_account: row.per_account,
        meter_name: meter.map_or_else(|| row.meter.clone(), |meter| meter.name.to_owned()),
        unit: meter.map_or("count", |meter| meter.unit).to_owned(),
        meter: row.meter,
        period: row.period,
        soft_limit: row.soft_limit,
        hard_limit: row.hard_limit,
        used,
        set_by: Some(row.set_by_actor_id),
        version: Some(row.version),
        is_default: false,
    })
}

/// Set, change or remove a budget.
pub struct BudgetSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetSetInput {
    /// What the budget is on.
    pub scope: BudgetScope,
    /// The account, space or conversation; left out for the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<IdRef>,
    /// Each account in the scope on its own (minute and hour only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub each_account: bool,
    /// What it counts: operations, writes, messages, deliveries,
    /// reported_cost_micros, reported_tokens, or another counted meter
    /// budget_list names.
    pub meter: String,
    pub period: BudgetPeriod,
    /// Past this, you are told once a period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit: Option<i64>,
    /// Past this, more is refused (pushes wait for the next period).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_limit: Option<i64>,
    /// Leave both limits out to remove the budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for BudgetSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BudgetSetOutput {
    /// The budget as it is now; null once removed.
    pub budget: Option<BudgetView>,
}

impl Operation for BudgetSet {
    const NAME: &'static str = "budget_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set a budget";
    const DESCRIPTION: &'static str = "Set, change or remove a budget: how much the workspace, \
        an account, a space or a conversation may use of a meter in a minute, hour, day or \
        month. Past the soft limit its setter is told; past the hard limit more is refused, the \
        same for people and agents. Leave both limits out to remove it.";
    const EMITS: &'static [&'static str] = &["budgets.changed"];

    type Input = BudgetSetInput;
    type Output = BudgetSetOutput;

    async fn run(call: &mut OpCall<'_>, input: BudgetSetInput) -> Result<BudgetSetOutput, OpError> {
        let account = call.account_id();
        let workspace = call.ctx.workspace_id;
        if meter_of(&input.meter).is_none() {
            return Err(OpError::invalid(
                "meter",
                "Name a counted meter, such as messages or operations: budget_list lists them.",
            ));
        }
        let short = matches!(input.period, BudgetPeriod::Minute | BudgetPeriod::Hour);
        if input.each_account && !short {
            return Err(OpError::invalid(
                "eachAccount",
                "Each account on its own works for minute and hour budgets.",
            ));
        }
        for (field, limit) in [
            ("softLimit", input.soft_limit),
            ("hardLimit", input.hard_limit),
        ] {
            if limit.is_some_and(|limit| limit < 0) {
                return Err(OpError::invalid(field, "A limit is 0 or more."));
            }
        }
        if let (Some(soft), Some(hard)) = (input.soft_limit, input.hard_limit)
            && soft > hard
        {
            return Err(OpError::invalid(
                "softLimit",
                "The soft limit is at most the hard limit.",
            ));
        }
        let tx = call.tx().await?;
        let scope_id = match (input.scope, input.scope_id) {
            (BudgetScope::Workspace, None) => workspace,
            (BudgetScope::Workspace, Some(_)) => {
                return Err(OpError::invalid(
                    "scopeId",
                    "A budget on the workspace names nothing else.",
                ));
            }
            (_, None) => {
                return Err(OpError::invalid(
                    "scopeId",
                    "Name the account, space or conversation.",
                ));
            }
            (scope, Some(id)) => {
                let id = id.id();
                // In this workspace, and seen by the caller: anything else
                // is not found.
                let sql = match scope {
                    BudgetScope::Account => {
                        "SELECT EXISTS (SELECT 1 FROM organization_memberships \
                         WHERE organization_id = $1 AND actor_id = $2 AND enabled)"
                    }
                    BudgetScope::Space => {
                        "SELECT EXISTS (SELECT 1 FROM projects WHERE organization_id = $1 \
                         AND id = $2 AND actor_can_access_project($3, NULL, id))"
                    }
                    _ => {
                        "SELECT EXISTS (SELECT 1 FROM conversations conversation \
                         JOIN projects project ON project.id = conversation.project_id \
                         WHERE project.organization_id = $1 AND conversation.id = $2 \
                           AND conversation_visible_to(conversation.id, $3))"
                    }
                };
                let found = sqlx::query_scalar::<_, bool>(sql)
                    .bind(workspace)
                    .bind(id)
                    .bind(account)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
                if !found {
                    return Err(OpError::not_found(match scope {
                        BudgetScope::Account => "That account",
                        BudgetScope::Space => "That space",
                        _ => "That conversation",
                    }));
                }
                id
            }
        };
        let existing = sqlx::query_as::<_, BudgetRow>(&format!(
            "{SELECT} WHERE workspace_id = $1 AND scope_kind = $2 AND scope_id = $3 \
             AND per_account = $4 AND meter = $5 AND period = $6 FOR UPDATE"
        ))
        .bind(workspace)
        .bind(input.scope.key())
        .bind(scope_id)
        .bind(input.each_account)
        .bind(&input.meter)
        .bind(input.period.key())
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if let Some(expected) = input.expected_version
            && existing.as_ref().map(|row| row.version) != Some(expected)
        {
            return Err(AppError::VersionConflict {
                resource: "budget",
                current_version: existing.as_ref().map_or(0, |row| row.version),
            }
            .into());
        }
        let removing = input.soft_limit.is_none() && input.hard_limit.is_none();
        let id = match (&existing, removing) {
            (Some(existing), true) => {
                sqlx::query("DELETE FROM spend_limits WHERE id = $1")
                    .bind(existing.id)
                    .execute(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
                None
            }
            (None, true) => {
                return Err(OpError::invalid(
                    "hardLimit",
                    "Give a soft or a hard limit, or both.",
                ));
            }
            (existing, false) => {
                let id = existing.as_ref().map_or_else(Uuid::now_v7, |row| row.id);
                sqlx::query(
                    r#"
                    INSERT INTO spend_limits (
                        id, workspace_id, scope_kind, scope_id, per_account, meter, period,
                        soft_limit, hard_limit, set_by_actor_id
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (id) DO UPDATE SET
                        soft_limit = EXCLUDED.soft_limit, hard_limit = EXCLUDED.hard_limit,
                        set_by_actor_id = EXCLUDED.set_by_actor_id,
                        version = spend_limits.version + 1
                    "#,
                )
                .bind(id)
                .bind(workspace)
                .bind(input.scope.key())
                .bind(scope_id)
                .bind(input.each_account)
                .bind(&input.meter)
                .bind(input.period.key())
                .bind(input.soft_limit)
                .bind(input.hard_limit)
                .bind(account)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
                Some(id)
            }
        };
        let space = crate::spaces::workspace::ensure(tx, workspace).await?;
        crate::outbox::enqueue_typed(
            tx,
            space,
            crate::outbox::OutboxEventPayload::BudgetsChanged(
                crate::outbox::BudgetsChangedPayload {
                    workspace_id: workspace,
                    project_id: space,
                },
            ),
        )
        .await?;
        let budget = match id {
            Some(id) => {
                let row = sqlx::query_as::<_, BudgetRow>(&format!("{SELECT} WHERE id = $1"))
                    .bind(id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
                Some(view(tx, workspace, row, account).await?)
            }
            None => None,
        };
        let audited = id
            .or_else(|| existing.as_ref().map(|row| row.id))
            .unwrap_or(workspace);
        call.audit_target("budget", audited, json!({ "removed": removing }));
        Ok(BudgetSetOutput { budget })
    }
}

/// The workspace's budgets.
pub struct BudgetList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BudgetListInput {
    /// Only the budgets that apply to this account: on it, and on each
    /// account on its own. Use is counted for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for BudgetListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// A meter a budget can count.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BudgetMeter {
    pub id: String,
    pub name: String,
    pub unit: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BudgetListOutput {
    pub budgets: Vec<BudgetView>,
    /// The meters a budget can count.
    pub meters: Vec<BudgetMeter>,
}

impl Operation for BudgetList {
    const NAME: &'static str = "budget_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Budgets";
    const DESCRIPTION: &'static str = "The workspace's budgets, each with its use so far, and \
        the meters a budget can count. With account, only those that apply to that account, \
        with its use.";

    type Input = BudgetListInput;
    type Output = BudgetListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: BudgetListInput,
    ) -> Result<BudgetListOutput, OpError> {
        let caller = call.account_id();
        let workspace = call.ctx.workspace_id;
        let account = input.account.map(IdRef::id);
        let tx = call.tx().await?;
        // A budget on a conversation or space is listed only to those who
        // can see it.
        let rows = sqlx::query_as::<_, BudgetRow>(&format!(
            "{SELECT} WHERE workspace_id = $1 \
               AND ($2::uuid IS NULL OR (scope_kind = 'account' AND scope_id = $2) OR per_account) \
               AND (scope_kind <> 'conversation' OR conversation_visible_to(scope_id, $3)) \
               AND (scope_kind <> 'space' OR actor_can_access_project($3, NULL, scope_id)) \
             ORDER BY created_at, id LIMIT 500"
        ))
        .bind(workspace)
        .bind(account)
        .bind(caller)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let counted_for = account.unwrap_or(caller);
        let has_rate = rows.iter().any(|row| {
            row.scope_kind == BudgetScope::Workspace.key()
                && row.per_account
                && row.meter == MESSAGES_METER
                && row.period == "minute"
        });
        let mut budgets = Vec::with_capacity(rows.len() + 1);
        if !has_rate {
            let meter = meter_of(MESSAGES_METER);
            budgets.push(BudgetView {
                id: None,
                scope: BudgetScope::Workspace.key().to_owned(),
                scope_id: None,
                each_account: true,
                meter: MESSAGES_METER.to_owned(),
                meter_name: meter.map_or(MESSAGES_METER, |meter| meter.name).to_owned(),
                unit: "count".to_owned(),
                period: "minute".to_owned(),
                soft_limit: None,
                hard_limit: Some(DEFAULT_MESSAGE_RATE),
                used: metering::budget_used(
                    tx,
                    workspace,
                    MESSAGES_METER,
                    Scope::Account(counted_for),
                    Some(Scope::Workspace),
                    "minute",
                )
                .await?,
                set_by: None,
                version: None,
                is_default: true,
            });
        }
        for row in rows {
            budgets.push(view(tx, workspace, row, counted_for).await?);
        }
        Ok(BudgetListOutput {
            budgets,
            meters: METERS
                .iter()
                .filter(|meter| meter.kind == MeterKind::Counter)
                .map(|meter| BudgetMeter {
                    id: meter.id.to_owned(),
                    name: meter.name.to_owned(),
                    unit: meter.unit.to_owned(),
                })
                .collect(),
        })
    }
}
