//! Plans and metering (H1.25). The one place a plan, a workspace's own
//! limit or a spend limit is checked: [`admit`]. Everything that costs
//! something asks it before doing the work, and records what it used with
//! [`record`] (or in the same call, with a dedupe key).
//!
//! - **Meters** are declared in [`METERS`]: a *gauge* is read live (people,
//!   stored bytes), a *counter* adds up in `usage_events` and
//!   `usage_counters` per day and month, and an *each* limits one ask on its
//!   own (the size of one file).
//! - **Limits** come from three places, and the smallest wins: the
//!   workspace's plan (with staff overrides), the workspace's own settings
//!   where one exists (`files.workspace_quota_mb`), and `spend_limits` rows
//!   on the workspace, an account, a space or a conversation. The
//!   self-hosted plan limits nothing.
//! - **Soft, then hard.** Near a limit (80%) or over a soft one the ask is
//!   allowed with a notice, once per scope and period (`limit_notices`).
//!   Over a hard limit it is refused with a [`LimitRefusal`], which every
//!   edge shows as `limit_reached` with the numbers, when it resets and who
//!   can raise it. A hard limit on a counter locks the counter's row, so two
//!   asks can't both take the last unit.
//!
//! Nothing here ever deletes or hides anything: over a limit, new things
//! that cost are refused and everything else works.

pub mod ops;

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    error::AppError,
    settings::{self, SettingsContext},
};

/// How a meter is counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeterKind {
    /// Read live from what exists.
    Gauge,
    /// Added up from recorded usage, per day and month.
    Counter,
    /// One ask on its own (a file's size).
    Each,
}

/// One thing that is counted.
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    pub id: &'static str,
    pub kind: MeterKind,
    /// What its numbers are: `people`, `bytes`, `micros` (millionths of a
    /// dollar), `count` or `tokens`.
    pub unit: &'static str,
    /// What it counts, in words, for people and agents.
    pub name: &'static str,
}

const fn meter(id: &'static str, kind: MeterKind, unit: &'static str, name: &'static str) -> Meter {
    Meter {
        id,
        kind,
        unit,
        name,
    }
}

/// Every meter. A cost the server pays has one (R24 rule 8).
pub const METERS: &[Meter] = &[
    meter("people", MeterKind::Gauge, "people", "people"),
    meter("guests", MeterKind::Gauge, "people", "guests"),
    meter("storage_bytes", MeterKind::Gauge, "bytes", "file storage"),
    meter("file_bytes", MeterKind::Each, "bytes", "file size"),
    meter(
        "workspaces_owned",
        MeterKind::Gauge,
        "count",
        "workspaces owned",
    ),
    meter(
        "hosted_model_micros",
        MeterKind::Counter,
        "micros",
        "model calls on the server's key",
    ),
    meter(
        "agent_service_micros",
        MeterKind::Counter,
        "micros",
        "the hosted agent service",
    ),
    meter(
        "automation_heavy_runs",
        MeterKind::Counter,
        "count",
        "automation runs that call a model",
    ),
    meter("emails", MeterKind::Counter, "count", "notification emails"),
    meter("pushes", MeterKind::Counter, "count", "push notifications"),
    meter(
        "form_submissions",
        MeterKind::Counter,
        "count",
        "form submissions",
    ),
    meter("operations", MeterKind::Counter, "count", "operations"),
    meter("writes", MeterKind::Counter, "count", "writes"),
    meter("messages", MeterKind::Counter, "count", "messages"),
    meter("deliveries", MeterKind::Counter, "count", "deliveries"),
    meter(
        "reported_cost_micros",
        MeterKind::Counter,
        "micros",
        "reported run cost",
    ),
    meter(
        "reported_tokens",
        MeterKind::Counter,
        "tokens",
        "reported run tokens",
    ),
];

pub fn find_meter(id: &str) -> Option<&'static Meter> {
    METERS.iter().find(|meter| meter.id == id)
}

/// Where a limit applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Scope {
    Workspace,
    Account(#[schemars(with = "String")] Uuid),
    Space(#[schemars(with = "String")] Uuid),
    Conversation(#[schemars(with = "String")] Uuid),
}

impl Scope {
    fn kind(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Account(_) => "account",
            Self::Space(_) => "space",
            Self::Conversation(_) => "conversation",
        }
    }

    fn id(self, workspace: Uuid) -> Uuid {
        match self {
            Self::Workspace => workspace,
            Self::Account(id) | Self::Space(id) | Self::Conversation(id) => id,
        }
    }
}

/// A counter's period.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Day,
    Month,
}

impl Period {
    fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "month" => Some(Self::Month),
            _ => None,
        }
    }

    /// The period `now` falls in, and when the next one starts (UTC).
    pub fn bounds(self, now: DateTime<Utc>) -> (NaiveDate, DateTime<Utc>) {
        let today = now.date_naive();
        let (start, next) = match self {
            Self::Day => (today, today.succ_opt().unwrap_or(today)),
            Self::Month => {
                let start = today.with_day(1).unwrap_or(today);
                let next = if start.month() == 12 {
                    NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
                }
                .unwrap_or(start);
                (start, next)
            }
        };
        let resets = Utc.from_utc_datetime(&next.and_hms_opt(0, 0, 0).unwrap_or_default());
        (start, resets)
    }
}

/// Who can raise a limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    /// The workspace's plan: an owner changes plan.
    Owners,
    /// A workspace setting or spend limit: whoever manages settings.
    Admins,
}

/// Where a limit came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    Plan,
    Setting,
    SpendLimit,
}

#[derive(Clone, Copy, Debug)]
struct Limit {
    scope: Scope,
    period: Option<Period>,
    soft: Option<i64>,
    hard: Option<i64>,
    source: Source,
}

/// What is asked for.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    pub workspace_id: Uuid,
    pub meter: &'static str,
    pub quantity: i64,
    /// The account acting, the space and the conversation, where there are
    /// ones: limits on each apply too.
    pub account_id: Option<Uuid>,
    pub space_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    /// Record the usage in the same call, once per key.
    pub record: Option<Recorded<'a>>,
}

impl Ask<'_> {
    pub fn new(workspace_id: Uuid, meter: &'static str, quantity: i64) -> Self {
        Self {
            workspace_id,
            meter,
            quantity,
            account_id: None,
            space_id: None,
            conversation_id: None,
            record: None,
        }
    }
}

/// What a recorded use came from, and the key that makes it count once.
#[derive(Clone, Copy, Debug)]
pub struct Recorded<'a> {
    pub source_kind: &'static str,
    pub source_id: Option<Uuid>,
    pub dedupe_key: &'a str,
}

/// Usage of a counted meter.
#[derive(Clone, Copy, Debug)]
pub struct Usage<'a> {
    pub workspace_id: Uuid,
    pub meter: &'static str,
    pub quantity: i64,
    pub account_id: Option<Uuid>,
    pub space_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub recorded: Recorded<'a>,
}

/// A limit being neared or passed, said once per scope and period.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LimitNotice {
    pub meter: &'static str,
    pub scope: Scope,
    pub limit: i64,
    pub used: i64,
    /// `soft`: near the limit or over a soft one; nothing is refused yet.
    pub level: &'static str,
}

/// The ask may go ahead.
#[derive(Clone, Debug)]
pub enum Admission {
    Allowed,
    AllowedNearLimit { notice: LimitNotice },
}

/// Why an ask was refused: the numbers, when it resets, and who can raise
/// it. Its message is for whoever reads it, person or agent.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LimitRefusal {
    pub meter: &'static str,
    pub scope: Scope,
    pub limit: i64,
    pub used: i64,
    #[schemars(with = "Option<String>")]
    pub resets_at: Option<DateTime<Utc>>,
    pub who_can_change: Who,
    pub message: String,
}

impl LimitRefusal {
    fn new(meter: &'static Meter, limit: &Limit, value: i64, used: i64) -> Self {
        let resets_at = limit.period.map(|period| period.bounds(Utc::now()).1);
        let who = match limit.source {
            Source::Plan => Who::Owners,
            Source::Setting | Source::SpendLimit => Who::Admins,
        };
        let whose = match limit.scope {
            Scope::Workspace => "This workspace",
            Scope::Account(_) => "This account",
            Scope::Space(_) => "This space",
            Scope::Conversation(_) => "This conversation",
        };
        let amount = |value: i64| match meter.unit {
            "bytes" => format!("{} MB", value / (1024 * 1024)),
            "micros" => format!("${:.2}", value as f64 / 1_000_000.0),
            _ => value.to_string(),
        };
        let mut message = if meter.kind == MeterKind::Each {
            format!(
                "That is over the limit for {} of {}.",
                meter.name,
                amount(value)
            )
        } else {
            format!(
                "{whose} has reached its limit for {}: {} of {}.",
                meter.name,
                amount(used),
                amount(value)
            )
        };
        if resets_at.is_some() {
            message.push_str(" It resets at the start of the next ");
            message.push_str(match limit.period {
                Some(Period::Day) => "day (UTC).",
                _ => "month (UTC).",
            });
        }
        message.push_str(match who {
            Who::Owners => " An owner can change the workspace's plan.",
            Who::Admins => " Someone who manages the workspace's settings can raise it.",
        });
        Self {
            meter: meter.id,
            scope: limit.scope,
            limit: value,
            used,
            resets_at,
            who_can_change: who,
            message,
        }
    }
}

/// Reads a plan's limit for a meter: a number, or `{"soft", "hard"}`.
fn plan_limit(value: &Value) -> Option<(Option<i64>, Option<i64>)> {
    match value {
        Value::Number(number) => number.as_i64().map(|hard| (None, Some(hard))),
        Value::Object(fields) => {
            let soft = fields.get("soft").and_then(Value::as_i64);
            let hard = fields.get("hard").and_then(Value::as_i64);
            (soft.is_some() || hard.is_some()).then_some((soft, hard))
        }
        _ => None,
    }
}

/// The plan's limits for a workspace, overrides applied over them. A
/// workspace with no plan row is on the self-hosted plan, which limits
/// nothing.
async fn plan_limits(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<BTreeMap<String, Value>, AppError> {
    let row = sqlx::query_as::<_, (Value, Value)>(
        "SELECT plan.limits, chosen.overrides FROM workspace_plans chosen \
         JOIN plans plan ON plan.id = chosen.plan_id WHERE chosen.workspace_id = $1",
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((limits, overrides)) = row else {
        return Ok(BTreeMap::new());
    };
    let mut merged = match limits {
        Value::Object(fields) => fields.into_iter().collect::<BTreeMap<_, _>>(),
        _ => BTreeMap::new(),
    };
    if let Value::Object(fields) = overrides {
        merged.extend(fields);
    }
    Ok(merged)
}

/// The workspace settings that limit a meter, and what one of the
/// setting's units is in the meter's.
const SETTING_LIMITS: &[(&str, &str, i64)] =
    &[("storage_bytes", "files.workspace_quota_mb", 1024 * 1024)];

/// Every limit on `ask`'s meter that applies to it.
async fn limits_for(
    connection: &mut PgConnection,
    meter: &'static Meter,
    ask: &Ask<'_>,
) -> Result<Vec<Limit>, AppError> {
    let mut found = Vec::new();
    let plan_period = (meter.kind == MeterKind::Counter).then_some(Period::Month);
    // A per-account gauge (workspaces owned) is the acting account's.
    let plan_scope = match (meter.id, ask.account_id) {
        ("workspaces_owned", Some(account)) => Scope::Account(account),
        _ => Scope::Workspace,
    };
    if let Some((soft, hard)) = plan_limits(connection, ask.workspace_id)
        .await?
        .get(meter.id)
        .and_then(plan_limit)
    {
        found.push(Limit {
            scope: plan_scope,
            period: plan_period,
            soft,
            hard,
            source: Source::Plan,
        });
    }
    let settings_wanted = SETTING_LIMITS
        .iter()
        .filter(|(id, _, _)| *id == meter.id)
        .collect::<Vec<_>>();
    if !settings_wanted.is_empty() {
        let effective = settings::resolve(
            &mut *connection,
            SettingsContext {
                organization_id: ask.workspace_id,
                project_id: None,
                actor_id: None,
            },
            &BTreeMap::new(),
        )
        .await?;
        for (_, key, unit) in settings_wanted {
            if let Some(value) = effective.value::<i64>(key).filter(|value| *value > 0) {
                found.push(Limit {
                    scope: Scope::Workspace,
                    period: plan_period,
                    soft: None,
                    hard: Some(value.saturating_mul(*unit)),
                    source: Source::Setting,
                });
            }
        }
    }
    let rows = sqlx::query_as::<_, (String, Uuid, String, Option<i64>, Option<i64>)>(
        "SELECT scope_kind, scope_id, period, soft_limit, hard_limit FROM spend_limits \
         WHERE workspace_id = $1 AND meter = $2 AND NOT per_account \
           AND period IN ('day', 'month') \
           AND ((scope_kind = $6 AND scope_id = $1) \
             OR (scope_kind = 'account' AND scope_id = $3) \
             OR (scope_kind = 'space' AND scope_id = $4) \
             OR (scope_kind = 'conversation' AND scope_id = $5))",
    )
    .bind(ask.workspace_id)
    .bind(meter.id)
    .bind(ask.account_id)
    .bind(ask.space_id)
    .bind(ask.conversation_id)
    .bind(Scope::Workspace.kind())
    .fetch_all(&mut *connection)
    .await?;
    for (kind, id, period, soft, hard) in rows {
        let scope = match kind.as_str() {
            "account" => Scope::Account(id),
            "space" => Scope::Space(id),
            "conversation" => Scope::Conversation(id),
            _ => Scope::Workspace,
        };
        found.push(Limit {
            scope,
            period: if meter.kind == MeterKind::Counter {
                Period::parse(&period)
            } else {
                None
            },
            soft,
            hard,
            source: Source::SpendLimit,
        });
    }
    Ok(found)
}

/// A gauge's reading for a scope.
async fn gauge(
    connection: &mut PgConnection,
    meter: &'static Meter,
    workspace: Uuid,
    scope: Scope,
) -> Result<i64, AppError> {
    Ok(match (meter.id, scope) {
        // Seats are people's; agents and apps are never counted.
        ("people", Scope::Workspace) => {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM organization_memberships membership \
                 JOIN actors account ON account.id = membership.actor_id \
                 WHERE membership.organization_id = $1 AND membership.enabled \
                   AND account.kind = 'human' AND account.account_kind = 'person'", // kind-check: mechanical (people are seats; agents and apps never are)
            )
            .bind(workspace)
            .fetch_one(&mut *connection)
            .await?
        }
        ("storage_bytes", Scope::Workspace) => {
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(sum(bytes), 0)::bigint FROM ( \
                     SELECT byte_size + COALESCE(thumbnail_bytes, 0) AS bytes FROM files \
                     WHERE organization_id = $1 AND blob_deleted_at IS NULL \
                     UNION ALL \
                     SELECT image_bytes FROM link_previews \
                     WHERE organization_id = $1 AND image_key IS NOT NULL) stored",
            )
            .bind(workspace)
            .fetch_one(&mut *connection)
            .await?
        }
        ("workspaces_owned", Scope::Account(account)) => {
            // Ownership is the role's tier, read the way access reads it.
            let roles = sqlx::query_scalar::<_, String>(
                "SELECT role FROM organization_memberships WHERE actor_id = $1 AND enabled",
            )
            .bind(account)
            .fetch_all(&mut *connection)
            .await?;
            roles
                .iter()
                .filter_map(|role| crate::policy::MembershipRole::parse(role))
                .filter(|role| crate::access::Tier::from(*role) == crate::access::Tier::Owner)
                .count() as i64
        }
        // Guests come with C2; until then there are none.
        _ => 0,
    })
}

/// A counter's total for a scope and period, locking its row when `lock`.
async fn counted(
    connection: &mut PgConnection,
    meter: &'static Meter,
    workspace: Uuid,
    scope: Scope,
    period: Period,
    lock: bool,
) -> Result<i64, AppError> {
    let (start, _) = period.bounds(Utc::now());
    if lock {
        sqlx::query(
            "INSERT INTO usage_counters \
             (workspace_id, meter, period, period_start, scope_kind, scope_id) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(workspace)
        .bind(meter.id)
        .bind(period.as_str())
        .bind(start)
        .bind(scope.kind())
        .bind(scope.id(workspace))
        .execute(&mut *connection)
        .await?;
    }
    let sql = format!(
        "SELECT quantity FROM usage_counters WHERE workspace_id = $1 AND meter = $2 \
         AND period = $3 AND period_start = $4 AND scope_kind = $5 AND scope_id = $6{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    Ok(sqlx::query_scalar::<_, i64>(&sql)
        .bind(workspace)
        .bind(meter.id)
        .bind(period.as_str())
        .bind(start)
        .bind(scope.kind())
        .bind(scope.id(workspace))
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(0))
}

/// A limit with what was used and what the ask would make it.
type Decided = (Limit, i64, i64);

/// Of the limits that apply, the one that decides: a hard limit the ask
/// goes over (the smallest such), else the one it comes nearest.
fn decide(
    checked: &[(Limit, i64)],
    quantity: i64,
    kind: MeterKind,
) -> Result<Option<Decided>, Decided> {
    let after = |used: i64| {
        if kind == MeterKind::Each {
            quantity
        } else {
            used.saturating_add(quantity)
        }
    };
    let mut refused: Option<(Limit, i64, i64)> = None;
    for (limit, used) in checked {
        if let Some(hard) = limit.hard
            && after(*used) > hard
            && refused.is_none_or(|(_, smallest, _)| hard < smallest)
        {
            refused = Some((*limit, hard, *used));
        }
    }
    if let Some(refused) = refused {
        return Err(refused);
    }
    // Near a hard limit (80%), or over a soft one: the nearest of them.
    let mut near: Option<(Limit, i64, i64, f64)> = None;
    for (limit, used) in checked {
        let reached = after(*used);
        let candidates = [
            limit.soft.filter(|soft| reached > *soft),
            limit
                .hard
                .filter(|hard| *hard > 0 && reached.saturating_mul(5) >= hard.saturating_mul(4)),
        ];
        for value in candidates.into_iter().flatten() {
            let share = reached as f64 / value.max(1) as f64;
            if near.is_none_or(|(_, _, _, best)| share > best) {
                near = Some((*limit, value, *used, share));
            }
        }
    }
    Ok(near.map(|(limit, value, used, _)| (limit, value, used)))
}

/// Whether `ask` may go ahead: see the module notes. A refusal is
/// [`AppError::LimitReached`]. With `ask.record`, the usage is recorded in
/// the same transaction.
pub async fn admit(connection: &mut PgConnection, ask: Ask<'_>) -> Result<Admission, AppError> {
    let meter = find_meter(ask.meter).ok_or(AppError::Internal("an unknown meter"))?;
    let quantity = ask.quantity.max(0);
    // A thing already counted (a retry) goes ahead without counting again.
    if let Some(recorded) = ask.record {
        let seen = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM usage_events WHERE workspace_id = $1 AND dedupe_key = $2)",
        )
        .bind(ask.workspace_id)
        .bind(recorded.dedupe_key)
        .fetch_one(&mut *connection)
        .await?;
        if seen {
            return Ok(Admission::Allowed);
        }
    }
    let limits = limits_for(connection, meter, &ask).await?;
    let mut checked = Vec::with_capacity(limits.len());
    for limit in limits {
        let used = match meter.kind {
            MeterKind::Each => 0,
            MeterKind::Gauge => gauge(connection, meter, ask.workspace_id, limit.scope).await?,
            MeterKind::Counter => {
                let period = limit.period.unwrap_or(Period::Month);
                counted(
                    connection,
                    meter,
                    ask.workspace_id,
                    limit.scope,
                    period,
                    limit.hard.is_some(),
                )
                .await?
            }
        };
        checked.push((limit, used));
    }
    let decided = match decide(&checked, quantity, meter.kind) {
        Ok(decided) => decided,
        Err((limit, value, used)) => {
            note(connection, meter, ask.workspace_id, &limit, "hard").await?;
            return Err(AppError::LimitReached(Box::new(LimitRefusal::new(
                meter, &limit, value, used,
            ))));
        }
    };
    if let Some(recorded) = ask.record
        && meter.kind == MeterKind::Counter
    {
        record(
            connection,
            Usage {
                workspace_id: ask.workspace_id,
                meter: ask.meter,
                quantity,
                account_id: ask.account_id,
                space_id: ask.space_id,
                conversation_id: ask.conversation_id,
                recorded,
            },
        )
        .await?;
    }
    let Some((limit, value, used)) = decided else {
        return Ok(Admission::Allowed);
    };
    if !note(connection, meter, ask.workspace_id, &limit, "soft").await? {
        return Ok(Admission::Allowed);
    }
    Ok(Admission::AllowedNearLimit {
        notice: LimitNotice {
            meter: meter.id,
            scope: limit.scope,
            limit: value,
            used: used.saturating_add(quantity),
            level: "soft",
        },
    })
}

/// Marks a notice as given for this scope and period; true the first time.
async fn note(
    connection: &mut PgConnection,
    meter: &'static Meter,
    workspace: Uuid,
    limit: &Limit,
    level: &str,
) -> Result<bool, AppError> {
    let start = limit.period.unwrap_or(Period::Month).bounds(Utc::now()).0;
    Ok(sqlx::query(
        "INSERT INTO limit_notices (workspace_id, meter, scope_kind, scope_id, period_start, level) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(workspace)
    .bind(meter.id)
    .bind(limit.scope.kind())
    .bind(limit.scope.id(workspace))
    .bind(start)
    .bind(level)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        == 1)
}

/// Records usage of a counted meter, once per dedupe key: the ledger row
/// and the day's and month's counters for the workspace and each scope
/// named. True when it was new.
pub async fn record(connection: &mut PgConnection, usage: Usage<'_>) -> Result<bool, AppError> {
    let meter = find_meter(usage.meter)
        .filter(|meter| meter.kind == MeterKind::Counter)
        .ok_or(AppError::Internal(
            "recorded usage of a meter that isn't counted",
        ))?;
    let quantity = usage.quantity.max(0);
    let inserted = sqlx::query(
        "INSERT INTO usage_events (id, workspace_id, meter, quantity, account_id, space_id, \
             conversation_id, source_kind, source_id, dedupe_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (workspace_id, dedupe_key) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(usage.workspace_id)
    .bind(meter.id)
    .bind(quantity)
    .bind(usage.account_id)
    .bind(usage.space_id)
    .bind(usage.conversation_id)
    .bind(usage.recorded.source_kind)
    .bind(usage.recorded.source_id)
    .bind(usage.recorded.dedupe_key)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        return Ok(false);
    }
    let scopes = std::iter::once(Scope::Workspace)
        .chain(usage.account_id.map(Scope::Account))
        .chain(usage.space_id.map(Scope::Space))
        .chain(usage.conversation_id.map(Scope::Conversation));
    let now = Utc::now();
    for scope in scopes {
        for period in [Period::Day, Period::Month] {
            sqlx::query(
                "INSERT INTO usage_counters \
                 (workspace_id, meter, period, period_start, scope_kind, scope_id, quantity) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (workspace_id, meter, period, period_start, scope_kind, scope_id) \
                 DO UPDATE SET quantity = usage_counters.quantity + EXCLUDED.quantity",
            )
            .bind(usage.workspace_id)
            .bind(meter.id)
            .bind(period.as_str())
            .bind(period.bounds(now).0)
            .bind(scope.kind())
            .bind(scope.id(usage.workspace_id))
            .bind(quantity)
            .execute(&mut *connection)
            .await?;
        }
    }
    Ok(true)
}

/// Whether `account` may join `workspace` (or come back to it): a person
/// takes a seat on the `people` meter; agents and apps never do. Holds the
/// workspace's plan row first, so two joins at once can't both take the
/// last seat.
pub async fn admit_member(
    connection: &mut PgConnection,
    workspace: Uuid,
    account: Uuid,
) -> Result<(), AppError> {
    let person = sqlx::query_scalar::<_, bool>(
        "SELECT kind = 'human' AND account_kind = 'person' FROM actors WHERE id = $1", // kind-check: mechanical (people are seats; agents and apps never are)
    )
    .bind(account)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or(false);
    if !person {
        return Ok(());
    }
    sqlx::query("SELECT 1 FROM workspace_plans WHERE workspace_id = $1 FOR UPDATE")
        .bind(workspace)
        .execute(&mut *connection)
        .await?;
    admit(connection, Ask::new(workspace, "people", 1)).await?;
    Ok(())
}

/// The largest one thing may be on a per-thing meter (`file_bytes`): the
/// smallest hard limit on the workspace, if any. For checks that must
/// refuse mid-stream, before the thing's size is known.
pub async fn largest(
    connection: &mut PgConnection,
    workspace: Uuid,
    meter: &'static str,
) -> Result<Option<i64>, AppError> {
    let meter = find_meter(meter)
        .filter(|meter| meter.kind == MeterKind::Each)
        .ok_or(AppError::Internal(
            "a size limit on a meter that isn't per thing",
        ))?;
    Ok(
        limits_for(connection, meter, &Ask::new(workspace, meter.id, 0))
            .await?
            .into_iter()
            .filter(|limit| limit.scope == Scope::Workspace)
            .filter_map(|limit| limit.hard)
            .min(),
    )
}

/// Writes a subscription's state, as its merchant of record reported it at
/// `read_at`, into the workspace's plan (H1.33): the one writer of a
/// workspace's plan besides staff. An older reading never overwrites a
/// newer one, and an ended subscription that has since been replaced by
/// another changes nothing. Ended, the workspace goes back to the free
/// plan with everything kept. True when it changed the plan row.
pub async fn apply_billing_state(
    connection: &mut PgConnection,
    provider: &str,
    subscription: &crate::billing::Subscription,
    read_at: DateTime<Utc>,
) -> Result<bool, AppError> {
    use crate::billing::SubscriptionState;
    let ended = subscription.state == SubscriptionState::Ended;
    let plan = if ended {
        "free"
    } else {
        subscription.plan_id.as_str()
    };
    let known = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM plans WHERE id = $1)")
        .bind(plan)
        .fetch_one(&mut *connection)
        .await?;
    if !known {
        return Err(AppError::Validation("billing plan"));
    }
    let status = match subscription.state {
        SubscriptionState::Trialing => "trialing",
        SubscriptionState::PastDue => "past_due",
        SubscriptionState::Active | SubscriptionState::Ended => "active",
    };
    start_plan(&mut *connection, subscription.workspace_id).await?;
    let changed = sqlx::query(
        "UPDATE workspace_plans SET plan_id = $3, status = $4, source = 'billing', \
             billing_provider = $2, customer_ref = $5, \
             subscription_ref = CASE WHEN $9 THEN NULL ELSE $6 END, \
             seats = $7, period_ends_at = $8, trial_ends_at = $10, \
             billing_state_at = $11, version = version + 1, updated_at = now() \
         WHERE workspace_id = $1 \
           AND (billing_state_at IS NULL OR billing_state_at < $11) \
           AND NOT ($9 AND subscription_ref IS NOT NULL \
                    AND (subscription_ref <> $6 OR billing_provider <> $2))",
    )
    .bind(subscription.workspace_id)
    .bind(provider)
    .bind(plan)
    .bind(status)
    .bind(&subscription.customer_ref)
    .bind(&subscription.subscription_ref)
    .bind(i32::try_from(subscription.seats).unwrap_or(i32::MAX))
    .bind(subscription.period_ends_at)
    .bind(ended)
    .bind(subscription.trial_ends_at)
    .bind(read_at)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        == 1;
    Ok(changed)
}

/// The plan a new workspace starts on, written as it is made.
pub async fn start_plan(connection: &mut PgConnection, workspace: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO workspace_plans (workspace_id, plan_id) \
         SELECT $1, id FROM plans WHERE is_default ON CONFLICT (workspace_id) DO NOTHING",
    )
    .bind(workspace)
    .execute(connection)
    .await?;
    Ok(())
}

/// The plan new workspaces start on: `free` for a hosted server, else
/// `self_hosted`. Run at start by the process that migrates.
pub async fn set_default_plan(pool: &sqlx::PgPool, hosted: bool) -> Result<(), AppError> {
    let plan = if hosted { "free" } else { "self_hosted" };
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let current = sqlx::query_scalar::<_, String>("SELECT id FROM plans WHERE is_default")
        .fetch_optional(&mut *transaction)
        .await?;
    if current.as_deref() != Some(plan) {
        sqlx::query("UPDATE plans SET is_default = false WHERE is_default")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE plans SET is_default = true WHERE id = $1")
            .bind(plan)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// One meter's use this month and its limit, as Settings › Usage shows it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeterUsage {
    pub meter: &'static str,
    pub name: &'static str,
    pub kind: MeterKind,
    pub unit: &'static str,
    /// Now, for a gauge; this month so far (UTC), for a counter.
    pub used: i64,
    /// The smallest limit on the whole workspace, if any.
    pub limit: Option<i64>,
    #[schemars(with = "Option<String>")]
    pub resets_at: Option<DateTime<Utc>>,
}

/// The workspace's plan and its use of every meter.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub plan_id: String,
    pub plan_name: String,
    /// `active`, `trialing`, `past_due`, `grace` or `over_limit`.
    pub status: String,
    pub meters: Vec<MeterUsage>,
}

/// What [`UsageReport`] holds, for `workspace`.
pub async fn usage_for(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<UsageReport, AppError> {
    let (plan_id, plan_name, status) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT plan.id, plan.name, chosen.status FROM workspace_plans chosen \
         JOIN plans plan ON plan.id = chosen.plan_id WHERE chosen.workspace_id = $1",
    )
    .bind(workspace)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or_else(|| {
        (
            "self_hosted".to_owned(),
            "Self-hosted".to_owned(),
            "active".to_owned(),
        )
    });
    let (month, resets) = Period::Month.bounds(Utc::now());
    let counters = sqlx::query_as::<_, (String, i64)>(
        "SELECT meter, quantity FROM usage_counters WHERE workspace_id = $1 \
         AND period = 'month' AND period_start = $2 AND scope_kind = $3 \
         AND scope_id = $1",
    )
    .bind(workspace)
    .bind(month)
    .bind(Scope::Workspace.kind())
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let mut meters = Vec::new();
    for meter in METERS {
        if meter.kind == MeterKind::Each || meter.id == "workspaces_owned" {
            continue;
        }
        let used = match meter.kind {
            MeterKind::Gauge => gauge(connection, meter, workspace, Scope::Workspace).await?,
            _ => counters.get(meter.id).copied().unwrap_or(0),
        };
        let ask = Ask::new(workspace, meter.id, 0);
        let limit = limits_for(connection, meter, &ask)
            .await?
            .into_iter()
            .filter(|limit| limit.scope == Scope::Workspace)
            .filter(|limit| limit.period.is_none_or(|period| period == Period::Month))
            .filter_map(|limit| limit.hard.or(limit.soft))
            .min();
        meters.push(MeterUsage {
            meter: meter.id,
            name: meter.name,
            kind: meter.kind,
            unit: meter.unit,
            used,
            limit,
            resets_at: (meter.kind == MeterKind::Counter).then_some(resets),
        });
    }
    Ok(UsageReport {
        plan_id,
        plan_name,
        status,
        meters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(scope: Scope, soft: Option<i64>, hard: Option<i64>) -> Limit {
        Limit {
            scope,
            period: Some(Period::Month),
            soft,
            hard,
            source: Source::SpendLimit,
        }
    }

    #[test]
    fn the_smallest_hard_limit_decides_across_scopes() {
        let account = Scope::Account(Uuid::nil());
        let checked = [
            (limit(Scope::Workspace, None, Some(100)), 50),
            (limit(account, None, Some(60)), 50),
            (limit(Scope::Space(Uuid::nil()), None, Some(1_000)), 50),
        ];
        // 50 + 20 is over the account's 60, not the workspace's 100.
        let (refused, value, used) = decide(&checked, 20, MeterKind::Counter).expect_err("refused");
        assert_eq!((refused.scope, value, used), (account, 60, 50));
        // Over two: the smaller is named.
        let (refused, value, _) = decide(&checked, 60, MeterKind::Counter).expect_err("refused");
        assert_eq!((refused.scope, value), (account, 60));
        // Within all: allowed, with the nearest named once past 80%.
        let (near, value, _) = decide(&checked, 0, MeterKind::Counter)
            .expect("allowed")
            .expect("near the account's limit");
        assert_eq!((near.scope, value), (account, 60));
        assert!(
            decide(&checked[..1], 0, MeterKind::Counter)
                .expect("allowed")
                .is_none(),
            "half the workspace's limit is not near it"
        );
    }

    #[test]
    fn a_soft_limit_warns_and_never_refuses() {
        let checked = [(limit(Scope::Workspace, Some(10), None), 9)];
        let (near, value, _) = decide(&checked, 5, MeterKind::Counter)
            .expect("allowed")
            .expect("over the soft limit");
        assert_eq!((near.scope, value), (Scope::Workspace, 10));
    }

    #[test]
    fn an_each_limit_compares_the_ask_alone() {
        let checked = [(limit(Scope::Workspace, None, Some(25)), 1_000)];
        assert!(decide(&checked, 10, MeterKind::Each).is_ok());
        assert!(decide(&checked, 26, MeterKind::Each).is_err());
    }

    #[test]
    fn periods_reset_at_the_next_utc_day_and_month() {
        let now = Utc.with_ymd_and_hms(2026, 12, 31, 23, 0, 0).unwrap();
        let (start, resets) = Period::Month.bounds(now);
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 12, 1).expect("date"));
        assert_eq!(resets, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
        let (start, resets) = Period::Day.bounds(now);
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 12, 31).expect("date"));
        assert_eq!(resets, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn meters_are_declared_once() {
        let mut ids = METERS.iter().map(|meter| meter.id).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), METERS.len());
    }
}
