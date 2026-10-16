//! U3: capacity alerts. Rules per account, all on by default and editable
//! by its owner:
//!
//! - a window resets within X hours with at least Y% unused,
//! - a window is above Y% used,
//! - a limit is reached (and when it resets),
//! - banked resets are available, or one expires within X hours.
//!
//! Each fires once per window cycle (or once per banked reset), into the
//! owner's inbox through W6's notifications, and from there to their
//! devices by push (N3). Alerts belong to an account, not a project, so
//! they're filed under the owner's direct space and checked against
//! account ownership when read.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, PgExecutor, PgPool};
use uuid::Uuid;

use super::{UsageAccount, WindowSource, WindowView, load_account, ready, windows_at};
use crate::{
    AppState,
    error::AppError,
    notifications::{self, NewNotification},
};

pub mod kind {
    pub const RESET_SOON: &str = "capacity_reset_soon";
    pub const HIGH_USAGE: &str = "capacity_high_usage";
    pub const LIMIT_REACHED: &str = "capacity_limit_reached";
    pub const RESET_CREDIT: &str = "capacity_reset_credit";
    pub const ALL: &[&str] = &[RESET_SOON, HIGH_USAGE, LIMIT_REACHED, RESET_CREDIT];
}

/// Subject kind of capacity notifications: the provider account.
pub const SUBJECT_KIND: &str = "provider_account";

/// Readings this close to a fired alert's reset time are the same cycle.
const SAME_CYCLE_SECONDS: f64 = 30.0 * 60.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    ResetSoon,
    HighUsage,
    LimitReached,
    ResetCredits,
}

impl Rule {
    pub const ALL: [Self; 4] = [
        Self::ResetSoon,
        Self::HighUsage,
        Self::LimitReached,
        Self::ResetCredits,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResetSoon => "reset_soon",
            Self::HighUsage => "high_usage",
            Self::LimitReached => "limit_reached",
            Self::ResetCredits => "reset_credits",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rule| rule.as_str() == value)
    }

    /// Built-in hours and percent, when the rule has them.
    fn defaults(self) -> (Option<i16>, Option<i16>) {
        match self {
            Self::ResetSoon => (Some(3), Some(25)),
            Self::HighUsage => (None, Some(90)),
            Self::LimitReached => (None, None),
            Self::ResetCredits => (Some(48), None),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ResetSoon => "A window resets soon with capacity left to use.",
            Self::HighUsage => "A window is nearly used up.",
            Self::LimitReached => "A limit is reached, with when it resets.",
            Self::ResetCredits => "A banked reset is available or about to expire.",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub rule: Rule,
    pub description: &'static str,
    pub enabled: bool,
    pub hours: Option<i16>,
    pub percent: Option<i16>,
    pub default_hours: Option<i16>,
    pub default_percent: Option<i16>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleChange {
    pub enabled: bool,
    pub hours: Option<i16>,
    pub percent: Option<i16>,
}

#[derive(FromRow)]
struct RuleRow {
    rule: String,
    enabled: bool,
    hours: Option<i16>,
    percent: Option<i16>,
}

/// Every rule for the account, defaults filled in.
pub async fn rules<'e, E: PgExecutor<'e>>(
    executor: E,
    account_id: Uuid,
) -> Result<Vec<RuleView>, AppError> {
    let stored = sqlx::query_as::<_, RuleRow>(
        "SELECT rule, enabled, hours, percent FROM capacity_alert_rules WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_all(executor)
    .await?;
    Ok(Rule::ALL
        .into_iter()
        .map(|rule| {
            let (default_hours, default_percent) = rule.defaults();
            let row = stored.iter().find(|row| row.rule == rule.as_str());
            RuleView {
                rule,
                description: rule.description(),
                enabled: row.is_none_or(|row| row.enabled),
                hours: default_hours.and(row.and_then(|row| row.hours).or(default_hours)),
                percent: default_percent.and(row.and_then(|row| row.percent).or(default_percent)),
                default_hours,
                default_percent,
            }
        })
        .collect())
}

pub(crate) async fn set_rule(
    pool: &PgPool,
    account: &UsageAccount,
    rule: Rule,
    change: &RuleChange,
) -> Result<(), AppError> {
    let (default_hours, default_percent) = rule.defaults();
    if change.hours.is_some() && default_hours.is_none()
        || change.percent.is_some() && default_percent.is_none()
    {
        return Err(AppError::Validation("alert rule threshold"));
    }
    if change.hours.is_some_and(|hours| !(1..=168).contains(&hours))
        || change
            .percent
            .is_some_and(|percent| !(1..=100).contains(&percent))
    {
        return Err(AppError::Validation("alert rule threshold"));
    }
    sqlx::query(
        r#"
        INSERT INTO capacity_alert_rules (account_id, organization_id, rule, enabled, hours, percent)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (account_id, rule) DO UPDATE SET
            enabled = EXCLUDED.enabled, hours = EXCLUDED.hours,
            percent = EXCLUDED.percent, updated_at = now()
        "#,
    )
    .bind(account.id)
    .bind(account.organization_id)
    .bind(rule.as_str())
    .bind(change.enabled)
    .bind(change.hours)
    .bind(change.percent)
    .execute(pool)
    .await?;
    Ok(())
}

/// "45 min", "2 h", "3 d".
pub fn countdown(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let minutes = (to - from).num_minutes().max(0);
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else if minutes < 48 * 60 {
        format!("{} h", (minutes + 30) / 60)
    } else {
        format!("{} d", (minutes + 12 * 60) / (24 * 60))
    }
}

/// One alert about to fire.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub rule: Rule,
    pub kind: &'static str,
    pub subject_key: String,
    pub cycle_at: Option<DateTime<Utc>>,
    pub title: String,
    pub window: Option<WindowView>,
}

fn window_phrase(account_name: &str, window: &WindowView) -> String {
    match window.source {
        WindowSource::Reported => format!("{account_name} {} limit", window.label.to_lowercase()),
        WindowSource::Schedule => format!("{account_name} {}", window.label),
    }
}

/// What the rules say about these windows and banked resets right now.
/// Pure, so the thresholds are tested without a database.
pub fn candidates(
    account_name: &str,
    rules: &[RuleView],
    windows: &[WindowView],
    credits: &[super::ResetCreditView],
    now: DateTime<Utc>,
) -> Vec<Candidate> {
    let rule = |wanted: Rule| rules.iter().find(|view| view.rule == wanted && view.enabled);
    let mut found = Vec::new();
    for window in windows {
        let phrase = window_phrase(account_name, window);
        let resets = window.resets_at.filter(|at| *at > now);
        if let (Some(view), Some(at)) = (rule(Rule::ResetSoon), resets) {
            let hours = i64::from(view.hours.unwrap_or(3));
            let percent = view.percent.unwrap_or(25);
            let unused_enough = window
                .unused_percent
                .is_none_or(|unused| i16::try_from(unused).unwrap_or(0) >= percent);
            if at <= now + Duration::hours(hours) && unused_enough && !window.limit_reached {
                let title = match window.unused_percent {
                    Some(unused) => {
                        format!("{phrase} resets in {} · {unused}% unused", countdown(now, at))
                    }
                    None => format!("{phrase} resets in {}", countdown(now, at)),
                };
                found.push(Candidate {
                    rule: Rule::ResetSoon,
                    kind: kind::RESET_SOON,
                    subject_key: window.key.clone(),
                    cycle_at: Some(at),
                    title,
                    window: Some(window.clone()),
                });
            }
        }
        if window.limit_reached {
            if rule(Rule::LimitReached).is_some() {
                let title = match resets {
                    Some(at) => format!("{phrase} reached · resets in {}", countdown(now, at)),
                    None => format!("{phrase} reached"),
                };
                found.push(Candidate {
                    rule: Rule::LimitReached,
                    kind: kind::LIMIT_REACHED,
                    subject_key: window.key.clone(),
                    cycle_at: window.resets_at,
                    title,
                    window: Some(window.clone()),
                });
            }
        } else if let (Some(view), Some(used)) = (rule(Rule::HighUsage), window.used_percent) {
            if i16::try_from(used).unwrap_or(100) >= view.percent.unwrap_or(90) {
                let title = match resets {
                    Some(at) => format!("{phrase} is {used}% used · resets in {}", countdown(now, at)),
                    None => format!("{phrase} is {used}% used"),
                };
                found.push(Candidate {
                    rule: Rule::HighUsage,
                    kind: kind::HIGH_USAGE,
                    subject_key: window.key.clone(),
                    cycle_at: window.resets_at,
                    title,
                    window: Some(window.clone()),
                });
            }
        }
    }
    if let Some(view) = rule(Rule::ResetCredits) {
        let hours = i64::from(view.hours.unwrap_or(48));
        for credit in credits {
            match credit.expires_at {
                Some(at) if at > now && at <= now + Duration::hours(hours) => {
                    found.push(Candidate {
                        rule: Rule::ResetCredits,
                        kind: kind::RESET_CREDIT,
                        subject_key: format!("credit:{}:expiring", credit.id),
                        cycle_at: None,
                        title: format!(
                            "{account_name} banked reset expires in {}",
                            countdown(now, at)
                        ),
                        window: None,
                    });
                }
                Some(at) if at <= now => {}
                _ => found.push(Candidate {
                    rule: Rule::ResetCredits,
                    kind: kind::RESET_CREDIT,
                    subject_key: format!("credit:{}:available", credit.id),
                    cycle_at: None,
                    title: format!("{account_name} has a banked reset to spend"),
                    window: None,
                }),
            }
        }
    }
    found
}

/// Run the account's rules against its windows at `now` and notify its
/// owner of each alert that hasn't fired this cycle. Returns how many fired.
pub async fn evaluate(
    state: &AppState,
    account_id: Uuid,
    now: DateTime<Utc>,
) -> Result<usize, AppError> {
    let Some(account) = load_account(&state.pool, account_id).await? else {
        return Ok(0);
    };
    let rules = rules(&state.pool, account.id).await?;
    let windows = windows_at(&state.pool, account.id, now).await?;
    let credits = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT reset_credits FROM usage_accounts WHERE account_id = $1 AND reset_credits_available > 0",
    )
    .bind(account.id)
    .fetch_optional(&state.pool)
    .await?
    .map(|value| super::stored_credits(&value))
    .unwrap_or_default();
    let found = candidates(&account.name, &rules, &windows, &credits, now);
    if found.is_empty() {
        return Ok(0);
    }
    let mut transaction = state.pool.begin().await?;
    // One evaluation per account at a time, so a cycle fires once.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("capacity-alerts:{}", account.id))
        .execute(&mut *transaction)
        .await?;
    let Some(space) = crate::direct::space_for_member(
        &mut transaction,
        account.organization_id,
        account.owner_actor_id,
    )
    .await?
    else {
        // The owner left the workspace: nobody to tell.
        return Ok(0);
    };
    let mut fired = 0;
    for candidate in found {
        let already = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM capacity_alert_fires
                WHERE account_id = $1 AND rule = $2 AND subject_key = $3
                  AND ((cycle_at IS NULL AND $4::timestamptz IS NULL)
                       OR abs(extract(epoch FROM cycle_at - $4::timestamptz)) < $5)
            )
            "#,
        )
        .bind(account.id)
        .bind(candidate.rule.as_str())
        .bind(&candidate.subject_key)
        .bind(candidate.cycle_at)
        .bind(SAME_CYCLE_SECONDS)
        .fetch_one(&mut *transaction)
        .await?;
        if already {
            continue;
        }
        let fire_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO capacity_alert_fires (id, account_id, rule, subject_key, cycle_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(fire_id)
        .bind(account.id)
        .bind(candidate.rule.as_str())
        .bind(&candidate.subject_key)
        .bind(candidate.cycle_at)
        .execute(&mut *transaction)
        .await?;
        let preview = if candidate.rule == Rule::ResetSoon {
            let ready =
                ready::ready_tasks(&mut *transaction, account.owner_actor_id, account.id, Some(100))
                    .await?;
            match ready.tasks.len() {
                0 => None,
                1 => Some("1 ready task can use it.".to_owned()),
                count => Some(format!(
                    "{count}{} ready tasks across {} project{} can use it.",
                    if count == 100 { "+" } else { "" },
                    ready.project_count,
                    if ready.project_count == 1 { "" } else { "s" },
                )),
            }
        } else {
            None
        };
        let href = format!("/settings/account/usage?account={}", account.id);
        let detail = json!({
            "title": candidate.title,
            "preview": preview,
            "href": href,
            "place": "Usage",
            "accountId": account.id,
            "rule": candidate.rule.as_str(),
            "windowKey": candidate.window.as_ref().map(|window| window.key.clone()),
            "resetsAt": candidate.window.as_ref().and_then(|window| window.resets_at),
            "unusedPercent": candidate.window.as_ref().and_then(|window| window.unused_percent),
            "launchHref": format!("{href}&ready=1#ready-tasks"),
        });
        let notification = NewNotification {
            project_id: space,
            recipient_actor_id: account.owner_actor_id,
            kind: candidate.kind,
            subject_kind: SUBJECT_KIND,
            subject_id: account.id,
            actor_id: None,
            conversation_id: None,
            message_id: None,
            work_item_id: None,
            comment_id: None,
            agent_run_id: None,
            approval_id: None,
            pull_request_id: None,
            detail,
            group_key: format!(
                "capacity:{}:{}:{}",
                account.id,
                candidate.rule.as_str(),
                candidate.subject_key
            ),
            dedupe_key: format!("capacity:{fire_id}"),
        };
        if notifications::deliver(&mut transaction, &notification).await? {
            fired += 1;
        }
    }
    if fired > 0 {
        notifications::announce(&mut transaction, space, account.owner_actor_id).await?;
    }
    transaction.commit().await?;
    Ok(fired)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::{Rule, RuleView, candidates, countdown, kind};
    use crate::usage::{ResetCreditView, WindowSource, WindowView};

    fn defaults() -> Vec<RuleView> {
        Rule::ALL
            .into_iter()
            .map(|rule| {
                let (hours, percent) = rule.defaults();
                RuleView {
                    rule,
                    description: rule.description(),
                    enabled: true,
                    hours,
                    percent,
                    default_hours: hours,
                    default_percent: percent,
                }
            })
            .collect()
    }

    fn window(label: &str, used: u16, resets_in: Duration, reached: bool) -> WindowView {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        WindowView {
            key: format!("codex.{}", label.to_lowercase()),
            label: label.to_owned(),
            source: WindowSource::Reported,
            schedule_id: None,
            duration_minutes: Some(10080),
            used_percent: Some(used),
            unused_percent: Some(100 - used),
            resets_at: Some(now + resets_in),
            limit_reached: reached,
            read_at: Some(now),
            rolled_over: false,
        }
    }

    #[test]
    fn a_window_resetting_soon_with_capacity_left_alerts_once_per_rule() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let found = candidates(
            "Codex",
            &defaults(),
            &[window("Weekly", 62, Duration::minutes(125), false)],
            &[],
            now,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, kind::RESET_SOON);
        assert_eq!(found[0].title, "Codex weekly limit resets in 2 h · 38% unused");
    }

    #[test]
    fn thresholds_and_switches_are_respected() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        // Too far off, and too little left.
        assert!(
            candidates(
                "Codex",
                &defaults(),
                &[
                    window("Weekly", 10, Duration::hours(30), false),
                    window("Daily", 80, Duration::hours(1), false),
                ],
                &[],
                now,
            )
            .is_empty()
        );
        let found = candidates(
            "Codex",
            &defaults(),
            &[
                window("Weekly", 95, Duration::hours(30), false),
                window("Daily", 100, Duration::hours(2), true),
            ],
            &[],
            now,
        );
        let kinds = found.iter().map(|found| found.kind).collect::<Vec<_>>();
        assert_eq!(kinds, [kind::HIGH_USAGE, kind::LIMIT_REACHED]);
        assert_eq!(found[1].title, "Codex daily limit reached · resets in 2 h");
        let mut off = defaults();
        for rule in &mut off {
            rule.enabled = false;
        }
        assert!(
            candidates(
                "Codex",
                &off,
                &[window("Daily", 100, Duration::hours(2), true)],
                &[],
                now
            )
            .is_empty()
        );
    }

    #[test]
    fn banked_resets_alert_when_new_and_when_expiring() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let credit = |id: &str, expires: Option<Duration>| ResetCreditView {
            id: id.to_owned(),
            title: None,
            expires_at: expires.map(|after| now + after),
            granted_at: None,
        };
        let found = candidates(
            "Codex",
            &defaults(),
            &[],
            &[
                credit("a", None),
                credit("b", Some(Duration::hours(5))),
                credit("c", Some(Duration::days(30))),
            ],
            now,
        );
        let keys = found
            .iter()
            .map(|found| found.subject_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            ["credit:a:available", "credit:b:expiring", "credit:c:available"]
        );
        assert_eq!(found[1].title, "Codex banked reset expires in 5 h");
    }

    #[test]
    fn countdowns_read_naturally() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        assert_eq!(countdown(now, now + Duration::minutes(20)), "20 min");
        assert_eq!(countdown(now, now + Duration::minutes(150)), "3 h");
        assert_eq!(countdown(now, now + Duration::hours(80)), "3 d");
    }
}
