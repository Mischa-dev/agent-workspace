//! Metering as operations: the workspace's plan and its use of every meter
//! (H1.25), and its spend limits (H1.27), for whoever manages its settings.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{METERS, MeterKind, Period, Rate, Scope, UsageReport, find_meter};
use crate::{
    error::AppError,
    events::{Audience, NewEvent, catalog},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// The most spend limits one workspace keeps.
const MAX_LIMITS: i64 = 200;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<WorkspaceUsageGet>()
        .add::<SpendLimitList>()
        .add::<SpendLimitSet>();
}

/// See the workspace's plan and usage.
pub struct WorkspaceUsageGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceUsageGetInput {}

impl Targeted for WorkspaceUsageGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceUsageGet {
    const NAME: &'static str = "workspace_usage_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See the workspace's usage";
    const DESCRIPTION: &'static str = "The workspace's plan and its use of each meter: people \
        and stored bytes now, and this month's counted use (emails, model calls on the server's \
        key, automation runs, form submissions and the rest), each with the workspace's limit \
        where it has one and when the count resets.";

    type Input = WorkspaceUsageGetInput;
    type Output = UsageReport;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceUsageGetInput,
    ) -> Result<UsageReport, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        Ok(super::usage_for(tx, workspace).await?)
    }
}

// ---------------------------------------------------------------------------
// Spend limits (H1.27)

/// One spend limit, as Settings › Usage shows it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendLimitView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// `workspace`, `account`, `space` or `conversation`.
    pub scope: String,
    /// The account, space or conversation; none for the workspace.
    #[schemars(with = "Option<String>")]
    pub scope_id: Option<Uuid>,
    /// Its name as the reader may see it.
    pub scope_name: String,
    /// Each account inside the scope is limited on its own.
    pub per_account: bool,
    pub meter: String,
    pub meter_name: String,
    pub unit: String,
    /// `minute`, `hour`, `day` or `month` (UTC).
    pub period: String,
    pub soft_limit: Option<i64>,
    pub hard_limit: Option<i64>,
    /// Used this day or month at this scope; none for a minute or an hour.
    pub used: Option<i64>,
    pub version: i64,
}

/// What `spend_limit_set` changed.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendLimitSetOutput {
    /// The limit as set; none when removed.
    pub limit: Option<SpendLimitView>,
}

/// A meter a spend limit can be set on.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LimitableMeter {
    pub id: &'static str,
    pub name: &'static str,
    pub unit: &'static str,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpendLimits {
    pub limits: Vec<SpendLimitView>,
    /// The meters a limit can be set on.
    pub meters: Vec<LimitableMeter>,
}

#[derive(sqlx::FromRow)]
struct LimitRow {
    id: Uuid,
    scope_kind: String,
    scope_id: Uuid,
    scope_name: Option<String>,
    per_account: bool,
    meter: String,
    period: String,
    soft_limit: Option<i64>,
    hard_limit: Option<i64>,
    version: i64,
}

async fn limits(call: &mut OpCall<'_>) -> Result<SpendLimits, OpError> {
    let workspace = call.ctx.workspace_id;
    let (account, connection) = (call.account_id(), call.ctx.caller.connection_id);
    let tx = call.tx().await?;
    // Names as the reader may see them: a conversation they can't read is
    // named only as one.
    let rows = sqlx::query_as::<_, LimitRow>(
        r#"
        SELECT spend.id, spend.scope_kind, spend.scope_id, spend.per_account, spend.meter,
               spend.period, spend.soft_limit, spend.hard_limit, spend.version,
               CASE spend.scope_kind
                   WHEN 'account' THEN (
                       SELECT membership.display_name FROM project_memberships membership
                       JOIN spaces own ON own.id = membership.project_id
                        AND own.workspace_id = spend.workspace_id AND own.kind = $4
                       WHERE membership.actor_id = spend.scope_id LIMIT 1)
                   WHEN 'space' THEN (
                       SELECT space.name FROM spaces space
                       WHERE space.id = spend.scope_id
                         AND actor_can_access_project($2, $3, space.id))
                   WHEN 'conversation' THEN (
                       SELECT CASE WHEN conversation.kind IN ('dm', 'group') THEN 'A direct conversation'
                                   ELSE conversation.name END
                       FROM conversations conversation
                       WHERE conversation.id = spend.scope_id
                         AND conversation_visible_to(conversation.id, $2))
               END AS scope_name
        FROM spend_limits spend
        WHERE spend.workspace_id = $1
        ORDER BY spend.meter, spend.scope_kind, spend.period, spend.id
        "#,
    )
    .bind(workspace)
    .bind(account)
    .bind(connection)
    .bind(crate::spaces::SpaceKind::Workspace.stored())
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let now = chrono::Utc::now();
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(meter) = find_meter(&row.meter) else {
            continue;
        };
        let scope = match row.scope_kind.as_str() {
            "account" => Scope::Account(row.scope_id),
            "space" => Scope::Space(row.scope_id),
            "conversation" => Scope::Conversation(row.scope_id),
            _ => Scope::Workspace,
        };
        let used = match Period::parse(&row.period) {
            Some(period) if !row.per_account && meter.kind == MeterKind::Counter => Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT quantity FROM usage_counters WHERE workspace_id = $1 AND meter = $2 \
                     AND period = $3 AND period_start = $4 AND scope_kind = $5 AND scope_id = $6",
                )
                .bind(workspace)
                .bind(meter.id)
                .bind(&row.period)
                .bind(period.bounds(now).0)
                .bind(scope.kind())
                .bind(scope.id(workspace))
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?
                .unwrap_or(0),
            ),
            _ => None,
        };
        let scope_name = match scope {
            Scope::Workspace => "The whole workspace".to_owned(),
            Scope::Account(_) => row.scope_name.unwrap_or_else(|| "An account".to_owned()),
            Scope::Space(_) => row.scope_name.unwrap_or_else(|| "A space".to_owned()),
            Scope::Conversation(_) => row
                .scope_name
                .unwrap_or_else(|| "A conversation you can't open".to_owned()),
        };
        views.push(SpendLimitView {
            id: row.id,
            scope_id: (scope != Scope::Workspace).then_some(row.scope_id),
            scope: row.scope_kind,
            scope_name,
            per_account: row.per_account,
            meter: row.meter,
            meter_name: meter.name.to_owned(),
            unit: meter.unit.to_owned(),
            period: row.period,
            soft_limit: row.soft_limit,
            hard_limit: row.hard_limit,
            used,
            version: row.version,
        });
    }
    Ok(SpendLimits {
        limits: views,
        meters: METERS
            .iter()
            .filter(|meter| meter.kind == MeterKind::Counter)
            .map(|meter| LimitableMeter {
                id: meter.id,
                name: meter.name,
                unit: meter.unit,
            })
            .collect(),
    })
}

/// See the workspace's spend limits.
pub struct SpendLimitList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpendLimitListInput {}

impl Targeted for SpendLimitListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for SpendLimitList {
    const NAME: &'static str = "spend_limit_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List spend limits";
    const DESCRIPTION: &'static str = "The workspace's spend limits, with what each has used \
        this day or month, and the meters a limit can be set on.";

    type Input = SpendLimitListInput;
    type Output = SpendLimits;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SpendLimitListInput,
    ) -> Result<SpendLimits, OpError> {
        limits(call).await
    }
}

/// Set or remove a spend limit.
pub struct SpendLimitSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpendLimitSetInput {
    /// `workspace`, `account`, `space` or `conversation`.
    pub scope: String,
    /// Its id or link; left out for the workspace. A thread counts as its
    /// channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<IdRef>,
    /// A meter `spend_limit_list` names, such as `messages`.
    pub meter: String,
    /// `minute` or `hour` (hard limits only), `day` or `month`.
    pub period: String,
    /// Limit each account in the scope on its own (minute or hour only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_account: Option<bool>,
    /// Past this, settings managers are told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit: Option<i64>,
    /// Past this, asks are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_limit: Option<i64>,
    /// Remove the limit with this scope, meter, period and perAccount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,
}

impl Targeted for SpendLimitSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// The scope's id, checked to be in the workspace and in the caller's
/// reach; a thread is its channel.
async fn scope_of(
    call: &mut OpCall<'_>,
    kind: &str,
    id: Option<Uuid>,
) -> Result<(&'static str, Uuid), OpError> {
    let workspace = call.ctx.workspace_id;
    let (account, connection) = (call.account_id(), call.ctx.caller.connection_id);
    let kind = match kind {
        "workspace" => return Ok((Scope::Workspace.kind(), workspace)),
        "account" => "account",
        "space" => "space",
        "conversation" => "conversation",
        _ => {
            return Err(OpError::invalid(
                "scope",
                "A limit's scope is workspace, account, space or conversation.",
            ));
        }
    };
    let Some(id) = id else {
        return Err(OpError::invalid(
            "scopeId",
            "Say which account, space or conversation.",
        ));
    };
    let tx = call.tx().await?;
    let found = match kind {
        "account" => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT actor_id FROM organization_memberships \
             WHERE organization_id = $1 AND actor_id = $2",
            )
            .bind(workspace)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
        }
        "space" => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM spaces WHERE id = $2 AND workspace_id = $1 \
               AND actor_can_access_project($3, $4, id)",
            )
            .bind(workspace)
            .bind(id)
            .bind(account)
            .bind(connection)
            .fetch_optional(&mut **tx)
            .await
        }
        _ => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT CASE WHEN conversation.kind = 'thread' \
                         THEN COALESCE(conversation.parent_channel_id, conversation.id) \
                         ELSE conversation.id END \
             FROM conversations conversation \
             JOIN spaces space ON space.id = conversation.project_id \
             WHERE conversation.id = $2 AND space.workspace_id = $1 \
               AND conversation_visible_to(conversation.id, $3) \
               AND actor_can_access_project($3, $4, conversation.project_id)",
            )
            .bind(workspace)
            .bind(id)
            .bind(account)
            .bind(connection)
            .fetch_optional(&mut **tx)
            .await
        }
    }
    .map_err(AppError::from)?;
    match found {
        Some(id) => Ok((kind, id)),
        None => Err(OpError::not_found(match kind {
            "account" => "That account",
            "space" => "That space",
            _ => "That conversation",
        })),
    }
}

impl Operation for SpendLimitSet {
    const NAME: &'static str = "spend_limit_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Set a spend limit";
    const DESCRIPTION: &'static str = "Set, change or remove a spend limit on the workspace, an \
        account, a space or a conversation: a meter, a period (UTC) and a soft limit (settings \
        managers are told) or a hard one (refused with limit_reached).";
    const EMITS: &'static [&'static str] = &["workspace.spend_limits_changed"];

    type Input = SpendLimitSetInput;
    type Output = SpendLimitSetOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpendLimitSetInput,
    ) -> Result<SpendLimitSetOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        let meter = find_meter(input.meter.trim())
            .filter(|meter| meter.kind == MeterKind::Counter)
            .ok_or(OpError::invalid(
                "meter",
                "That meter takes no spend limit: spend_limit_list names the ones that do.",
            ))?;
        let period = input.period.trim();
        let rate = Rate::parse(period).is_some();
        if !rate && Period::parse(period).is_none() {
            return Err(OpError::invalid(
                "period",
                "A period is minute, hour, day or month.",
            ));
        }
        let per_account = input.per_account.unwrap_or(false);
        if per_account && !rate {
            return Err(OpError::invalid(
                "perAccount",
                "A limit on each account is for a minute or an hour.",
            ));
        }
        let remove = input.remove == Some(true);
        if !remove {
            if [input.soft_limit, input.hard_limit]
                .iter()
                .flatten()
                .any(|value| *value < 0)
            {
                return Err(OpError::invalid("hardLimit", "A limit is 0 or more."));
            }
            if input.soft_limit.is_none() && input.hard_limit.is_none() {
                return Err(OpError::invalid(
                    "hardLimit",
                    "Give a soft limit, a hard limit or both, or remove: true.",
                ));
            }
            if rate && (input.soft_limit.is_some() || input.hard_limit.is_none()) {
                return Err(OpError::invalid(
                    "hardLimit",
                    "A minute or an hour takes a hard limit only.",
                ));
            }
        }
        let (kind, scope_id) =
            scope_of(call, input.scope.trim(), input.scope_id.map(IdRef::id)).await?;
        let tx = call.tx().await?;
        let changed = if remove {
            sqlx::query_scalar::<_, Uuid>(
                "DELETE FROM spend_limits WHERE workspace_id = $1 AND scope_kind = $2 \
                 AND scope_id = $3 AND per_account = $4 AND meter = $5 AND period = $6 \
                 RETURNING id",
            )
            .bind(workspace)
            .bind(kind)
            .bind(scope_id)
            .bind(per_account)
            .bind(meter.id)
            .bind(period)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?
        } else {
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM spend_limits WHERE workspace_id = $1",
            )
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_LIMITS {
                return Err(OpError::invalid(
                    "meter",
                    "This workspace has 200 spend limits already; remove one first.",
                ));
            }
            Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO spend_limits AS spend
                        (id, workspace_id, scope_kind, scope_id, per_account, meter, period,
                         soft_limit, hard_limit, set_by_actor_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (workspace_id, scope_kind, scope_id, per_account, meter, period)
                    DO UPDATE SET soft_limit = EXCLUDED.soft_limit,
                                  hard_limit = EXCLUDED.hard_limit,
                                  set_by_actor_id = EXCLUDED.set_by_actor_id,
                                  version = spend.version + 1,
                                  updated_at = now()
                    RETURNING id
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(workspace)
                .bind(kind)
                .bind(scope_id)
                .bind(per_account)
                .bind(meter.id)
                .bind(period)
                .bind(input.soft_limit)
                .bind(input.hard_limit)
                .bind(account)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?,
            )
        };
        if let Some(id) = changed {
            crate::events::emit(
                &mut **tx,
                NewEvent {
                    id: Uuid::now_v7(),
                    ty: &catalog::WORKSPACE_SPEND_LIMITS_CHANGED,
                    workspace_id: Some(workspace),
                    space_id: None,
                    thing: None,
                    access_thing: None,
                    version: None,
                    changes: &[],
                    audience: Audience::Workspace,
                    payload: json!({ "spendLimitId": id, "removed": remove }),
                    actor: None,
                    legacy_type: None,
                },
            )
            .await?;
            call.audit_target(
                "spend_limit",
                id,
                json!({ "meter": meter.id, "period": period, "scope": kind, "removed": remove }),
            );
        } else {
            call.audited_by_command();
        }
        let limit = match changed.filter(|_| !remove) {
            Some(id) => limits(call)
                .await?
                .limits
                .into_iter()
                .find(|limit| limit.id == id),
            None => None,
        };
        Ok(SpendLimitSetOutput { limit })
    }
}
