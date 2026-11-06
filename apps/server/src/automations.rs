//! X5: automation rules.
//!
//! A rule belongs to one project and has a trigger, conditions and up to
//! five actions:
//!
//! - triggers: an event from [`crate::events`] (a task is created, a task
//!   starts matching the conditions, a run finishes, checks fail on a linked
//!   pull request, a message is posted), a schedule, or a capacity window
//!   (one of the maker's provider accounts resets soon with capacity left);
//! - conditions: a saved view's filters, or the same list query inline,
//!   that the task an event is about must match (and, for schedules and
//!   capacity windows, that pick the tasks to act on);
//! - actions: launch with the project's run defaults, set a field, notify,
//!   post a message.
//!
//! A rule acts as the person, app or agent that made it. Every firing, and
//! every action in it, first checks that actor's access now; a rule that
//! fails the check is turned off with the reason, and its owner (or the
//! project's admins, when the owner left) is told why. Field changes and
//! messages go through the same MCP tools an agent would call, as the
//! maker, confined to the rule's project, so they're checked exactly like
//! the maker doing it by hand.
//!
//! Firings are rows in `automation_runs`, keyed by what caused them, and
//! run as `automations.fire` jobs, so an event seen twice fires a rule
//! once. A rule that fires more than 30 times in an hour is turned off: it's
//! almost certainly triggering itself.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgExecutor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    events::{self, Caller, Event, Subject},
    idempotency::IdempotencyKey,
    jobs::{self, JobContext, JobError, JobRegistry, NewJob},
    launch::{self, LaunchCaller, LaunchRequest},
    mcp::{self, McpPrincipal},
    notifications::{self, NewNotification},
    policy::{Action as PolicyAction, ActorKind, AgentLimits, with_agent_limits},
    work_items::{
        templates::{Frequency, Schedule},
        views,
    },
};

pub const FIRE_KIND: &str = "automations.fire";
const SCHEDULE_KIND: &str = "automations.schedule_sweep";
const MAX_RULES_PER_PROJECT: i64 = 100;
/// Firings in an hour that turn a rule off.
const MAX_FIRINGS_PER_HOUR: i64 = 30;
/// Tasks a scheduled or capacity rule acts on at once, at most.
const MAX_TARGET_TASKS: i64 = 50;
const MAX_TEXT_CHARACTERS: usize = 4_000;
pub const NOTIFICATION_KIND: &str = "automation";
pub const DISABLED_KIND: &str = "automation_disabled";
const SUBJECT_KIND: &str = "automation_rule";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/automations",
            get(list_route).post(create_route),
        )
        .route(
            "/api/v1/projects/{project_id}/automations/{rule_id}",
            axum::routing::patch(update_route).delete(delete_route),
        )
        .route(
            "/api/v1/projects/{project_id}/automations/{rule_id}/run",
            post(run_now_route),
        )
        .route(
            "/api/v1/projects/{project_id}/automations/{rule_id}/runs",
            get(runs_route),
        )
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(FIRE_KIND, |context: JobContext| async move {
        let payload: FirePayload = context.job.payload_as()?;
        fire(&context.state, payload.run_id)
            .await
            .map_err(|error| JobError::retry(error.log_category()))
    });
    registry.register(SCHEDULE_KIND, |context: JobContext| async move {
        sweep_schedules(&context.state)
            .await
            .map_err(|error| JobError::retry(error.log_category()))?;
        Ok(())
    });
    registry.every(SCHEDULE_KIND, Duration::from_secs(60));
}

// ---------------------------------------------------------------------------
// Rules as JSON

/// What makes a rule fire.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Trigger {
    /// A task is created (and matches the conditions, if any).
    TaskCreated {},
    /// A task starts matching the conditions: fires when it enters them,
    /// not on every change while it matches.
    TaskMatches {},
    /// A run finishes. outcomes narrows it to completed, failed, canceled
    /// or interrupted; empty means any.
    RunFinished {
        #[serde(default)]
        outcomes: Vec<String>,
    },
    /// Checks fail on a pull request linked to a task or run.
    PullRequestChecksFailed {},
    /// Someone else posts a message, in one conversation or any the maker
    /// can see. contains narrows it to messages with that text.
    MessagePosted {
        #[serde(default)]
        #[schemars(with = "Option<String>")]
        conversation_id: Option<Uuid>,
        #[serde(default)]
        contains: Option<String>,
    },
    /// On a schedule, in the maker's time zone.
    Schedule {
        /// daily, weekly or monthly.
        frequency: String,
        /// Every this many days, weeks or months (1 to 52).
        #[serde(default = "one")]
        every: u32,
        /// Weekly: ISO weekdays, 1 (Monday) to 7 (Sunday).
        #[serde(default)]
        weekdays: Vec<u32>,
        /// Monthly: the day of the month.
        #[serde(default)]
        month_day: Option<u32>,
        /// Local time, HH:MM.
        time: String,
        /// An IANA time zone such as Europe/London.
        timezone: String,
    },
    /// One of the maker's provider accounts resets soon with capacity left
    /// (the capacity alert's "resets soon" rule). Launches use that account.
    CapacityWindow {
        /// Only this account; left out, any of the maker's.
        #[serde(default)]
        #[schemars(with = "Option<String>")]
        provider_account_id: Option<Uuid>,
        /// Tasks to launch at most (1 to 10, default 3).
        #[serde(default)]
        max_tasks: Option<u8>,
    },
}

fn one() -> u32 {
    1
}

impl Trigger {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TaskCreated {} => "task_created",
            Self::TaskMatches {} => "task_matches",
            Self::RunFinished { .. } => "run_finished",
            Self::PullRequestChecksFailed {} => "pull_request_checks_failed",
            Self::MessagePosted { .. } => "message_posted",
            Self::Schedule { .. } => "schedule",
            Self::CapacityWindow { .. } => "capacity_window",
        }
    }

    /// Schedules and capacity windows aren't about one task: their
    /// conditions pick the tasks.
    fn picks_tasks(&self) -> bool {
        matches!(self, Self::Schedule { .. } | Self::CapacityWindow { .. })
    }

    fn schedule(&self) -> Option<(Schedule, chrono_tz::Tz)> {
        let Self::Schedule {
            frequency,
            every,
            weekdays,
            month_day,
            time,
            timezone,
        } = self
        else {
            return None;
        };
        let frequency = match frequency.as_str() {
            "daily" => Frequency::Daily,
            "weekly" => Frequency::Weekly,
            "monthly" => Frequency::Monthly,
            _ => return None,
        };
        let time = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
        let timezone = timezone.parse::<chrono_tz::Tz>().ok()?;
        Some((
            Schedule {
                frequency,
                every: *every,
                weekdays: weekdays.clone(),
                month_day: *month_day,
                time,
                starts_on: chrono::NaiveDate::from_ymd_opt(2000, 1, 3)?,
            },
            timezone,
        ))
    }
}

/// When the next scheduled firing is due, after `after`.
fn next_run_after(trigger: &Trigger, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (schedule, timezone) = trigger.schedule()?;
    let mut local = after.with_timezone(&timezone).naive_local();
    // A local time skipped by a clock change moves on to the next slot.
    for _ in 0..4 {
        let next = schedule.next_after(local)?;
        if let Some(at) = timezone.from_local_datetime(&next).earliest() {
            return Some(at.with_timezone(&Utc));
        }
        local = next;
    }
    None
}

/// The tasks a rule is about: a saved view's filters, and any inline ones
/// on top.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conditions {
    /// A saved view in the project: the maker's own or a shared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub view_id: Option<Uuid>,
    /// List query parameters as the task list's URL has them (status,
    /// priority, assignee, label, q, ...). "me" is the rule's maker.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filters: BTreeMap<String, String>,
}

impl Conditions {
    fn is_empty(&self) -> bool {
        self.view_id.is_none() && self.filters.is_empty()
    }
}

/// Who a notify action tells.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipients {
    /// The rule's maker.
    Owner,
    /// The task's assignee.
    Assignee,
    /// The project's owners and admins.
    ProjectAdmins,
}

/// What a rule does. Text may use {{task.key}}, {{task.title}},
/// {{task.url}}, {{task.status}}, {{run.state}}, {{pr.number}},
/// {{pr.title}}, {{pr.url}}, {{rule.name}} and {{event}}.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Launch an agent on the task (or the matching tasks) with the
    /// project's run defaults. On a pull request with no task, starts a run
    /// in the thread of the run that opened it.
    Launch {
        /// An environment in the project; left out, the project's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<String>")]
        environment_id: Option<Uuid>,
        /// The prompt, for one task; left out, the launch template's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// An agent profile to launch instead of the run defaults: its
        /// harness, model settings, instructions and environment.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<String>")]
        agent_profile_id: Option<Uuid>,
    },
    /// Change a field on the task (or the matching tasks).
    SetField {
        /// status (by name), priority, assignee ("me", "none", a handle),
        /// add_label or remove_label.
        field: String,
        value: String,
    },
    /// Put a notification in people's inboxes.
    Notify {
        recipients: Recipients,
        message: String,
    },
    /// Post a message in a channel, thread, DM or group, as the maker.
    PostMessage {
        #[schemars(with = "String")]
        conversation_id: Uuid,
        text: String,
    },
}

impl Action {
    fn kind(&self) -> &'static str {
        match self {
            Self::Launch { .. } => "launch",
            Self::SetField { .. } => "set_field",
            Self::Notify { .. } => "notify",
            Self::PostMessage { .. } => "post_message",
        }
    }
}

const SET_FIELDS: &[&str] = &[
    "status",
    "priority",
    "assignee",
    "add_label",
    "remove_label",
];
const RUN_OUTCOMES: &[&str] = &["completed", "failed", "canceled", "interrupted"];

fn clean_text(value: &str, field: &'static str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_TEXT_CHARACTERS
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation(field));
    }
    Ok(value.to_owned())
}

fn normalize_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("name"));
    }
    Ok(name.to_owned())
}

/// Checks the parts of a rule that don't need the database.
fn check_shape(
    trigger: &mut Trigger,
    conditions: &Conditions,
    actions: &mut [Action],
) -> Result<(), AppError> {
    match trigger {
        Trigger::RunFinished { outcomes } => {
            outcomes.sort();
            outcomes.dedup();
            if outcomes
                .iter()
                .any(|outcome| !RUN_OUTCOMES.contains(&outcome.as_str()))
            {
                return Err(AppError::Validation("trigger.outcomes"));
            }
        }
        Trigger::MessagePosted { contains, .. } => {
            if let Some(text) = contains {
                let text = text.trim();
                if text.is_empty() || text.chars().count() > 200 {
                    return Err(AppError::Validation("trigger.contains"));
                }
                *contains = Some(text.to_owned());
            }
        }
        Trigger::Schedule {
            every,
            weekdays,
            month_day,
            frequency,
            ..
        } => {
            weekdays.sort_unstable();
            weekdays.dedup();
            if !(1..=52).contains(every) {
                return Err(AppError::Validation("trigger.every"));
            }
            match frequency.as_str() {
                "weekly"
                    if weekdays.is_empty() || weekdays.iter().any(|d| !(1..=7).contains(d)) =>
                {
                    return Err(AppError::Validation("trigger.weekdays"));
                }
                "monthly" if !month_day.is_some_and(|day| (1..=31).contains(&day)) => {
                    return Err(AppError::Validation("trigger.monthDay"));
                }
                "daily" | "weekly" | "monthly" => {}
                _ => return Err(AppError::Validation("trigger.frequency")),
            }
            if trigger.schedule().is_none() {
                return Err(AppError::Validation("trigger.schedule"));
            }
        }
        Trigger::CapacityWindow { max_tasks, .. } => {
            if max_tasks.is_some_and(|count| !(1..=10).contains(&count)) {
                return Err(AppError::Validation("trigger.maxTasks"));
            }
        }
        Trigger::TaskCreated {} | Trigger::PullRequestChecksFailed {} => {}
        Trigger::TaskMatches {} => {
            if conditions.is_empty() {
                return Err(AppError::Invalid(
                    "a task-matches rule needs conditions: a saved view or filters",
                ));
            }
        }
    }
    if actions.is_empty() || actions.len() > 5 {
        return Err(AppError::Validation("actions"));
    }
    for action in actions.iter_mut() {
        match action {
            Action::Launch { prompt, .. } => {
                if let Some(text) = prompt {
                    *prompt = Some(clean_text(text, "actions.prompt")?);
                }
            }
            Action::SetField { field, value } => {
                if !SET_FIELDS.contains(&field.as_str()) {
                    return Err(AppError::Validation("actions.field"));
                }
                let trimmed = value.trim();
                if trimmed.is_empty() || trimmed.chars().count() > 200 {
                    return Err(AppError::Validation("actions.value"));
                }
                *value = trimmed.to_owned();
            }
            Action::Notify { message, .. } => *message = clean_text(message, "actions.message")?,
            Action::PostMessage { text, .. } => *text = clean_text(text, "actions.text")?,
        }
        let needs_tasks = matches!(action, Action::Launch { .. } | Action::SetField { .. });
        if needs_tasks && trigger.picks_tasks() && conditions.is_empty() {
            return Err(AppError::Invalid(
                "a scheduled or capacity rule that launches or changes tasks needs conditions to pick them",
            ));
        }
        if matches!(
            action,
            Action::Notify {
                recipients: Recipients::Assignee,
                ..
            }
        ) && matches!(trigger, Trigger::MessagePosted { .. })
        {
            return Err(AppError::Invalid("a message has no assignee to notify"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, FromRow)]
struct RuleRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    owner_actor_id: Uuid,
    owner_name: Option<String>,
    via_oauth_grant_id: Option<Uuid>,
    /// Made through MCP or the public API by a person's app or script.
    made_through_app: bool,
    external_agent_connection_id: Option<Uuid>,
    name: String,
    enabled: bool,
    trigger: Value,
    view_id: Option<Uuid>,
    view_name: Option<String>,
    conditions: Value,
    actions: Value,
    next_run_at: Option<DateTime<Utc>>,
    disabled_reason: Option<String>,
    last_fired_at: Option<DateTime<Utc>>,
    fire_count: i64,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const RULE_SELECT: &str = r#"
    SELECT rule.id, rule.organization_id, rule.project_id, rule.owner_actor_id,
           COALESCE(
               person.display_name,
               (SELECT membership.display_name FROM project_memberships membership
                WHERE membership.project_id = rule.project_id
                  AND membership.actor_id = rule.owner_actor_id)
           ) AS owner_name,
           rule.via_oauth_grant_id, rule.made_through_app, rule.external_agent_connection_id, rule.name,
           rule.enabled, rule.trigger, rule.view_id, view.name AS view_name,
           rule.conditions, rule.actions, rule.next_run_at, rule.disabled_reason,
           rule.last_fired_at, rule.fire_count, rule.version, rule.created_at, rule.updated_at
    FROM automation_rules rule
    LEFT JOIN users person ON person.actor_id = rule.owner_actor_id
    LEFT JOIN saved_views view ON view.project_id = rule.project_id AND view.id = rule.view_id
"#;

impl RuleRow {
    fn trigger(&self) -> Result<Trigger, AppError> {
        serde_json::from_value(self.trigger.clone())
            .map_err(|_| AppError::Internal("stored automation trigger is invalid"))
    }

    fn actions(&self) -> Result<Vec<Action>, AppError> {
        serde_json::from_value(self.actions.clone())
            .map_err(|_| AppError::Internal("stored automation actions are invalid"))
    }

    fn conditions(&self) -> Conditions {
        Conditions {
            view_id: self.view_id,
            filters: views::query_params(&self.conditions),
        }
    }

    fn who(&self) -> String {
        self.owner_name
            .clone()
            .unwrap_or_else(|| "Its maker".to_owned())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub trigger: Value,
    pub conditions: Conditions,
    /// The saved view's name, when the conditions use one that still exists.
    pub view_name: Option<String>,
    pub actions: Value,
    pub owner_actor_id: Uuid,
    pub owner_name: Option<String>,
    pub via_app: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    /// Why the server turned it off.
    pub disabled_reason: Option<String>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub fire_count: i64,
    pub version: i64,
    /// The caller made it, so may change it (it acts as its maker).
    pub can_edit: bool,
    /// The caller may turn it off or delete it (its maker or an admin).
    pub can_manage: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn view(row: RuleRow, caller: &Caller, admin: bool) -> RuleView {
    let mine = row.owner_actor_id == caller.actor_id;
    let conditions = row.conditions();
    RuleView {
        id: row.id,
        project_id: row.project_id,
        name: row.name,
        enabled: row.enabled,
        trigger: row.trigger,
        conditions,
        view_name: row.view_name,
        actions: row.actions,
        owner_actor_id: row.owner_actor_id,
        owner_name: row.owner_name,
        via_app: row.via_oauth_grant_id.is_some(),
        next_run_at: row.next_run_at,
        disabled_reason: row.disabled_reason,
        last_fired_at: row.last_fired_at,
        fire_count: row.fire_count,
        version: row.version,
        can_edit: mine && caller.can_write,
        can_manage: (mine || admin) && caller.can_write,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleList {
    pub rules: Vec<RuleView>,
    /// The caller may make rules here.
    pub can_create: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: Uuid,
    pub trigger_summary: String,
    pub subject: Value,
    /// pending, succeeded, partial, failed or skipped.
    pub state: String,
    pub detail: Option<String>,
    /// One entry per action: {kind, ok, detail}.
    pub results: Value,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPage {
    pub runs: Vec<RunView>,
}

// ---------------------------------------------------------------------------
// Access

/// Whether the caller may see the project's rules, and administer it.
async fn reader_access(
    pool: &PgPool,
    caller: &Caller,
    project_id: Uuid,
) -> Result<(bool, bool), AppError> {
    if !caller.reaches(project_id) {
        return Err(AppError::NotFound("project"));
    }
    if !sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, $2, $3)")
        .bind(caller.actor_id)
        .bind(caller.external_agent_connection_id)
        .bind(project_id)
        .fetch_one(pool)
        .await?
    {
        return Err(AppError::NotFound("project"));
    }
    let can_edit = crate::memory::project_access(
        pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        PolicyAction::WorkItemEdit,
    )
    .await
    .is_ok();
    let admin = caller.actor_kind == ActorKind::Human
        && crate::memory::project_access(
            pool,
            caller.actor_id,
            caller.actor_kind,
            project_id,
            PolicyAction::ProjectManage,
        )
        .await
        .is_ok();
    Ok((can_edit && caller.can_write, admin))
}

/// Database checks: the view, conversation and environment belong to the
/// project and the maker can use them.
async fn check_references(
    state: &AppState,
    project_id: Uuid,
    maker: Uuid,
    conditions: &Conditions,
    trigger: &Trigger,
    actions: &[Action],
) -> Result<(), AppError> {
    let pool = &state.pool;
    if let Some(view_id) = conditions.view_id {
        let found = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM saved_views
                WHERE project_id = $1 AND id = $2
                  AND (owner_actor_id IS NULL OR owner_actor_id = $3)
            )
            "#,
        )
        .bind(project_id)
        .bind(view_id)
        .bind(maker)
        .fetch_one(pool)
        .await?;
        if !found {
            return Err(AppError::NotFound("view"));
        }
    }
    if !conditions.filters.is_empty() {
        views::check_view_query(pool, project_id, maker, &conditions.filters).await?;
    }
    let mut conversations = Vec::new();
    if let Trigger::MessagePosted {
        conversation_id: Some(id),
        ..
    } = trigger
    {
        conversations.push(*id);
    }
    for action in actions {
        match action {
            Action::PostMessage {
                conversation_id, ..
            } => conversations.push(*conversation_id),
            Action::Launch {
                environment_id,
                agent_profile_id,
                ..
            } => {
                if let Some(id) = environment_id {
                    crate::environments::active_environment(pool, project_id, *id)
                        .await?
                        .ok_or(AppError::NotFound("environment"))?;
                }
                if let Some(id) = agent_profile_id {
                    let enabled = sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS (SELECT 1 FROM agent_profiles WHERE id = $1 AND project_id = $2 AND enabled)",
                    )
                    .bind(id)
                    .bind(project_id)
                    .fetch_one(pool)
                    .await?;
                    if !enabled {
                        return Err(AppError::NotFound("agent profile"));
                    }
                }
            }
            _ => {}
        }
    }
    for conversation_id in conversations {
        let visible = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations conversation
                WHERE conversation.id = $1 AND conversation.project_id = $2
                  AND conversation_visible_to(conversation.id, $3)
            )
            "#,
        )
        .bind(conversation_id)
        .bind(project_id)
        .bind(maker)
        .fetch_one(pool)
        .await?;
        if !visible {
            return Err(AppError::NotFound("conversation"));
        }
    }
    Ok(())
}

async fn fetch(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    rule_id: Uuid,
) -> Result<RuleRow, AppError> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "{RULE_SELECT} WHERE rule.project_id = $1 AND rule.id = $2"
    ))
    .bind(project_id)
    .bind(rule_id)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("automation"))
}

// ---------------------------------------------------------------------------
// Core, shared by the browser and MCP

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRule {
    pub name: String,
    pub trigger: Trigger,
    #[serde(default)]
    pub conditions: Conditions,
    pub actions: Vec<Action>,
    /// Start it off; left out, it's on.
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRule {
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub conditions: Option<Conditions>,
    #[serde(default)]
    pub actions: Option<Vec<Action>>,
    /// On or off. Admins may turn anyone's rule off; only its maker can
    /// turn it on, since it acts as them.
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub(crate) async fn list(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
) -> Result<RuleList, AppError> {
    let (can_create, admin) = reader_access(&state.pool, caller, project_id).await?;
    let rows = sqlx::query_as::<_, RuleRow>(&format!(
        "{RULE_SELECT} WHERE rule.project_id = $1 ORDER BY rule.created_at, rule.id LIMIT 200"
    ))
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(RuleList {
        rules: rows
            .into_iter()
            .map(|row| view(row, caller, admin))
            .collect(),
        can_create,
    })
}

pub(crate) async fn create(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    mut request: CreateRule,
) -> Result<RuleView, AppError> {
    let (can_create, admin) = reader_access(&state.pool, caller, project_id).await?;
    if !can_create {
        return Err(AppError::Forbidden);
    }
    let name = normalize_name(&request.name)?;
    check_shape(
        &mut request.trigger,
        &request.conditions,
        &mut request.actions,
    )?;
    check_references(
        state,
        project_id,
        caller.actor_id,
        &request.conditions,
        &request.trigger,
        &request.actions,
    )
    .await?;
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&state.pool)
            .await?;
    let enabled = request.enabled.unwrap_or(true);
    let next_run_at = enabled
        .then(|| next_run_after(&request.trigger, Utc::now()))
        .flatten();
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("automations:{project_id}"))
        .execute(&mut *transaction)
        .await?;
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM automation_rules WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
    if count >= MAX_RULES_PER_PROJECT {
        return Err(AppError::Conflict(
            "automation",
            "this project has 100 rules; delete one first",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO automation_rules (
            id, organization_id, project_id, owner_actor_id, via_oauth_grant_id, made_through_app,
            external_agent_connection_id, name, enabled, trigger_kind, trigger,
            view_id, conditions, actions, next_run_at
        ) VALUES ($1, $2, $3, $4, $5, $15, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(project_id)
    .bind(caller.actor_id)
    .bind(caller.via_grant_id)
    .bind(caller.external_agent_connection_id)
    .bind(&name)
    .bind(enabled)
    .bind(request.trigger.kind())
    .bind(serde_json::to_value(&request.trigger).map_err(|_| AppError::Validation("trigger"))?)
    .bind(request.conditions.view_id)
    .bind(json!(request.conditions.filters))
    .bind(serde_json::to_value(&request.actions).map_err(|_| AppError::Validation("actions"))?)
    .bind(next_run_at)
    .bind(through_app(caller))
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        organization_id,
        Some(project_id),
        "automation.created",
        "automation_rule",
        id,
        json!({ "trigger": request.trigger.kind(), "actions": request.actions.iter().map(Action::kind).collect::<Vec<_>>() }),
    )
    .await?;
    transaction.commit().await?;
    Ok(view(
        fetch(&state.pool, project_id, id).await?,
        caller,
        admin,
    ))
}

pub(crate) async fn update(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    rule_id: Uuid,
    request: UpdateRule,
) -> Result<RuleView, AppError> {
    let (can_edit, admin) = reader_access(&state.pool, caller, project_id).await?;
    if !caller.can_write {
        return Err(AppError::Forbidden);
    }
    let current = fetch(&state.pool, project_id, rule_id).await?;
    let mine = current.owner_actor_id == caller.actor_id;
    let only_turning_off = request.name.is_none()
        && request.trigger.is_none()
        && request.conditions.is_none()
        && request.actions.is_none()
        && request.enabled == Some(false);
    if !(mine && can_edit || admin && only_turning_off) {
        return Err(if mine || admin {
            AppError::Invalid("only its maker can change this rule, since it acts as them")
        } else {
            AppError::Forbidden
        });
    }
    if current.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "automation",
            current_version: current.version,
        });
    }
    let name = match &request.name {
        Some(name) => normalize_name(name)?,
        None => current.name.clone(),
    };
    let mut trigger = request
        .trigger
        .clone()
        .map_or_else(|| current.trigger(), Ok)?;
    let conditions = request
        .conditions
        .clone()
        .unwrap_or_else(|| current.conditions());
    let mut actions = request
        .actions
        .clone()
        .map_or_else(|| current.actions(), Ok)?;
    let enabled = request.enabled.unwrap_or(current.enabled);
    if !only_turning_off {
        check_shape(&mut trigger, &conditions, &mut actions)?;
        check_references(
            state,
            project_id,
            caller.actor_id,
            &conditions,
            &trigger,
            &actions,
        )
        .await?;
    }
    let next_run_at = enabled
        .then(|| next_run_after(&trigger, Utc::now()))
        .flatten();
    let mut transaction = state.pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE automation_rules
        SET name = $3, trigger_kind = $4, trigger = $5, view_id = $6, conditions = $7,
            actions = $8, enabled = $9, next_run_at = $10,
            disabled_reason = CASE WHEN $9 THEN NULL ELSE disabled_reason END,
            via_oauth_grant_id = CASE WHEN $12 THEN $13 ELSE via_oauth_grant_id END,
            made_through_app = CASE WHEN $12 THEN $14 ELSE made_through_app END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $11
        "#,
    )
    .bind(project_id)
    .bind(rule_id)
    .bind(&name)
    .bind(trigger.kind())
    .bind(serde_json::to_value(&trigger).map_err(|_| AppError::Validation("trigger"))?)
    .bind(conditions.view_id)
    .bind(json!(conditions.filters))
    .bind(serde_json::to_value(&actions).map_err(|_| AppError::Validation("actions"))?)
    .bind(enabled)
    .bind(next_run_at)
    .bind(request.expected_version)
    // The maker editing it through another app moves it onto that app.
    .bind(mine && !only_turning_off)
    .bind(caller.via_grant_id)
    .bind(through_app(caller))
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::VersionConflict {
            resource: "automation",
            current_version: current.version + 1,
        });
    }
    events::audit(
        &mut *transaction,
        caller,
        current.organization_id,
        Some(project_id),
        "automation.updated",
        "automation_rule",
        rule_id,
        json!({ "enabled": enabled }),
    )
    .await?;
    transaction.commit().await?;
    Ok(view(
        fetch(&state.pool, project_id, rule_id).await?,
        caller,
        admin,
    ))
}

pub(crate) async fn delete(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    rule_id: Uuid,
) -> Result<(), AppError> {
    let (_, admin) = reader_access(&state.pool, caller, project_id).await?;
    let current = fetch(&state.pool, project_id, rule_id).await?;
    if !caller.can_write || !(current.owner_actor_id == caller.actor_id || admin) {
        return Err(AppError::Forbidden);
    }
    let mut transaction = state.pool.begin().await?;
    sqlx::query("DELETE FROM automation_rules WHERE project_id = $1 AND id = $2")
        .bind(project_id)
        .bind(rule_id)
        .execute(&mut *transaction)
        .await?;
    events::audit(
        &mut *transaction,
        caller,
        current.organization_id,
        Some(project_id),
        "automation.deleted",
        "automation_rule",
        rule_id,
        json!({ "name": current.name }),
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunNow {
    /// Try it on this task, as if the trigger had fired for it.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub work_item_id: Option<Uuid>,
}

/// Fires a rule now, for trying it out. It still acts as its maker, with
/// the same checks.
pub(crate) async fn run_now(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    rule_id: Uuid,
    request: RunNow,
) -> Result<RunView, AppError> {
    let (can_edit, _) = reader_access(&state.pool, caller, project_id).await?;
    let rule = fetch(&state.pool, project_id, rule_id).await?;
    if !can_edit || rule.owner_actor_id != caller.actor_id {
        return Err(AppError::Forbidden);
    }
    if !rule.enabled {
        return Err(AppError::Invalid("turn the rule on first"));
    }
    if let Some(work_item_id) = request.work_item_id {
        let found = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_items WHERE project_id = $1 AND id = $2)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .fetch_one(&state.pool)
        .await?;
        if !found {
            return Err(AppError::NotFound("work item"));
        }
    }
    let subject = Subject {
        work_item_id: request.work_item_id,
        ..Subject::default()
    };
    let mut transaction = state.pool.begin().await?;
    let run_id = queue_fire(
        &mut transaction,
        &rule,
        &format!("manual:{}", Uuid::now_v7()),
        "Run by hand",
        &subject,
    )
    .await?
    .ok_or(AppError::Internal("manual automation run collided"))?;
    transaction.commit().await?;
    fire(state, run_id).await?;
    run_view(&state.pool, run_id).await
}

pub(crate) async fn runs(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    rule_id: Uuid,
) -> Result<RunPage, AppError> {
    reader_access(&state.pool, caller, project_id).await?;
    fetch(&state.pool, project_id, rule_id).await?;
    let runs = sqlx::query_as::<_, RunView>(
        r#"
        SELECT id, trigger_summary, subject, state, detail, results, created_at, finished_at
        FROM automation_runs WHERE rule_id = $1
        ORDER BY created_at DESC, id DESC LIMIT 50
        "#,
    )
    .bind(rule_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(RunPage { runs })
}

async fn run_view(pool: &PgPool, run_id: Uuid) -> Result<RunView, AppError> {
    Ok(sqlx::query_as::<_, RunView>(
        r#"
        SELECT id, trigger_summary, subject, state, detail, results, created_at, finished_at
        FROM automation_runs WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// Triggers

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FirePayload {
    run_id: Uuid,
}

/// Records one firing and queues its job. None when this cause already
/// fired the rule.
async fn queue_fire(
    transaction: &mut Transaction<'_, Postgres>,
    rule: &RuleRow,
    dedupe_key: &str,
    summary: &str,
    subject: &Subject,
) -> Result<Option<Uuid>, AppError> {
    let run_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO automation_runs (id, rule_id, project_id, dedupe_key, trigger_summary, subject)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (rule_id, dedupe_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(rule.id)
    .bind(rule.project_id)
    .bind(dedupe_key.chars().take(300).collect::<String>())
    .bind(summary.chars().take(300).collect::<String>())
    .bind(serde_json::to_value(subject).unwrap_or_else(|_| json!({})))
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(run_id) = run_id {
        jobs::enqueue(
            &mut **transaction,
            NewJob::new(FIRE_KIND)
                .project(rule.organization_id, rule.project_id)
                .payload(FirePayload { run_id })
                .unique(format!("automation-run:{run_id}")),
        )
        .await?;
    }
    Ok(run_id)
}

fn summary_for(event: &Event) -> String {
    match event.event_type.as_str() {
        "task.created" => "A task was created".to_owned(),
        "task.updated" => "A task started matching".to_owned(),
        "run.finished" => format!(
            "A run {}",
            event.subject.state.as_deref().unwrap_or("finished")
        ),
        "pull_request.checks_failed" => "Checks failed on a pull request".to_owned(),
        "message.created" => "A message was posted".to_owned(),
        other => other.to_owned(),
    }
}

/// The trigger kinds an event can fire.
fn kinds_for(event_type: &str) -> &'static [&'static str] {
    match event_type {
        "task.created" => &["task_created", "task_matches"],
        "task.updated" => &["task_matches"],
        "run.finished" => &["run_finished"],
        "pull_request.checks_failed" => &["pull_request_checks_failed"],
        "message.created" => &["message_posted"],
        _ => &[],
    }
}

/// Whether the task matches the rule's conditions, as its maker reads them.
/// Err when the conditions can't be read any more (the view is gone).
async fn task_matches(
    pool: &PgPool,
    rule: &RuleRow,
    work_item_id: Uuid,
) -> Result<Result<bool, &'static str>, AppError> {
    let Some(params) = condition_params(pool, rule).await? else {
        return Ok(Err(
            "Turned off because the saved view its conditions use was deleted.",
        ));
    };
    if params.is_empty() {
        return Ok(Ok(true));
    }
    match views::matching_tasks(
        pool,
        rule.project_id,
        &params,
        rule.owner_actor_id,
        Some(&[work_item_id]),
        1,
    )
    .await
    {
        Ok(found) => Ok(Ok(!found.is_empty())),
        Err(AppError::Validation(_) | AppError::Invalid(_)) => Ok(Err(
            "Turned off because its conditions can't be read any more.",
        )),
        Err(error) => Err(error),
    }
}

/// The view's query with the inline filters on top. None when the view is
/// gone.
async fn condition_params(
    pool: &PgPool,
    rule: &RuleRow,
) -> Result<Option<BTreeMap<String, String>>, AppError> {
    let mut params = BTreeMap::new();
    if let Some(view_id) = rule.view_id {
        let Some(query) = sqlx::query_scalar::<_, Value>(
            r#"
            SELECT query FROM saved_views
            WHERE project_id = $1 AND id = $2
              AND (owner_actor_id IS NULL OR owner_actor_id = $3)
            "#,
        )
        .bind(rule.project_id)
        .bind(view_id)
        .bind(rule.owner_actor_id)
        .fetch_optional(pool)
        .await?
        else {
            return Ok(None);
        };
        params = views::query_params(&query);
    }
    params.extend(views::query_params(&rule.conditions));
    Ok(Some(params))
}

/// Hands an event to the project's rules. Called by the events router.
pub async fn on_event(state: &AppState, event: &Event) -> Result<usize, AppError> {
    let kinds = kinds_for(&event.event_type);
    if kinds.is_empty() {
        return Ok(0);
    }
    let pool = &state.pool;
    let rules = sqlx::query_as::<_, RuleRow>(&format!(
        "{RULE_SELECT} WHERE rule.project_id = $1 AND rule.enabled AND rule.trigger_kind = ANY($2) ORDER BY rule.created_at, rule.id"
    ))
    .bind(event.project_id)
    .bind(kinds.iter().map(|kind| (*kind).to_owned()).collect::<Vec<_>>())
    .fetch_all(pool)
    .await?;
    let mut fired = 0;
    for rule in rules {
        let Ok(trigger) = rule.trigger() else {
            continue;
        };
        let subject = &event.subject;
        // A maker who lost access turns the rule off instead of it acting
        // for them.
        if let Err(reason) = acting_access(pool, &rule).await? {
            disable(state, &rule, &reason).await?;
            continue;
        }
        // Only what its maker could see fires it, and for a rule made through
        // an app, only what that app's current limit lets it see.
        if !crate::webhooks::visible_to(pool, rule.owner_actor_id, rule.project_id, subject).await?
        {
            continue;
        }
        if let Some(limits) = app_limits(pool, &rule).await?
            && limit_hides(
                &limits,
                rule.project_id,
                subject.work_item_id.is_some(),
                subject.pull_request_id.is_some(),
            )
        {
            continue;
        }
        let wanted = match &trigger {
            Trigger::TaskCreated {} => event.event_type == "task.created",
            Trigger::TaskMatches {} => true,
            Trigger::RunFinished { outcomes } => {
                outcomes.is_empty()
                    || subject
                        .state
                        .as_ref()
                        .is_some_and(|state| outcomes.contains(state))
            }
            Trigger::PullRequestChecksFailed {} => true,
            Trigger::MessagePosted {
                conversation_id,
                contains,
            } => {
                event.actor_id != Some(rule.owner_actor_id)
                    && conversation_id.is_none_or(|only| subject.conversation_id == Some(only))
                    && match contains {
                        Some(text) => message_contains(pool, subject, text).await?,
                        None => true,
                    }
            }
            Trigger::Schedule { .. } | Trigger::CapacityWindow { .. } => false,
        };
        if !wanted {
            continue;
        }
        let conditions = rule.conditions();
        let matched = match subject.work_item_id {
            Some(work_item_id) => match task_matches(pool, &rule, work_item_id).await? {
                Ok(matched) => matched,
                Err(reason) => {
                    disable(state, &rule, reason).await?;
                    continue;
                }
            },
            None => conditions.is_empty(),
        };
        let mut dedupe = event.key.clone();
        if let (Trigger::TaskMatches {}, Some(work_item_id)) = (&trigger, subject.work_item_id) {
            // Fires when the task enters the conditions; forgets it when it
            // leaves, so it fires again if it comes back.
            if !matched {
                sqlx::query(
                    "DELETE FROM automation_rule_matches WHERE rule_id = $1 AND work_item_id = $2",
                )
                .bind(rule.id)
                .bind(work_item_id)
                .execute(pool)
                .await?;
                continue;
            }
            let entered = sqlx::query(
                r#"
                INSERT INTO automation_rule_matches (rule_id, work_item_id) VALUES ($1, $2)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(rule.id)
            .bind(work_item_id)
            .execute(pool)
            .await?
            .rows_affected()
                == 1;
            if !entered {
                continue;
            }
            dedupe = format!("matches:{work_item_id}:{}", event.key);
        } else if !matched {
            continue;
        }
        let mut transaction = pool.begin().await?;
        if queue_fire(
            &mut transaction,
            &rule,
            &dedupe,
            &summary_for(event),
            subject,
        )
        .await?
        .is_some()
        {
            fired += 1;
        }
        transaction.commit().await?;
    }
    Ok(fired)
}

async fn message_contains(pool: &PgPool, subject: &Subject, text: &str) -> Result<bool, AppError> {
    let Some(message_id) = subject.message_id else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM messages
            WHERE id = $1 AND removed_at IS NULL
              AND strpos(lower(plain_text), lower($2)) > 0
        )
        "#,
    )
    .bind(message_id)
    .bind(text)
    .fetch_one(pool)
    .await?)
}

/// Queues the scheduled rules that are due.
async fn sweep_schedules(state: &AppState) -> Result<usize, AppError> {
    let mut transaction = state.pool.begin().await?;
    let due = sqlx::query_as::<_, RuleRow>(&format!(
        r#"{RULE_SELECT}
        WHERE rule.id IN (
            SELECT id FROM automation_rules
            WHERE enabled AND trigger_kind = 'schedule' AND next_run_at <= now()
            ORDER BY next_run_at LIMIT 100
            FOR UPDATE SKIP LOCKED
        )"#
    ))
    .fetch_all(&mut *transaction)
    .await?;
    let now = Utc::now();
    let mut queued = 0;
    for rule in &due {
        let Some(slot) = rule.next_run_at else {
            continue;
        };
        let next = rule
            .trigger()
            .ok()
            .and_then(|trigger| next_run_after(&trigger, now.max(slot)));
        sqlx::query("UPDATE automation_rules SET next_run_at = $2 WHERE id = $1")
            .bind(rule.id)
            .bind(next)
            .execute(&mut *transaction)
            .await?;
        // A slot missed by more than a day (the server was down) is skipped
        // rather than run late.
        if now - slot > chrono::Duration::days(1) {
            continue;
        }
        if queue_fire(
            &mut transaction,
            rule,
            &format!("schedule:{}", slot.to_rfc3339()),
            "On its schedule",
            &Subject::default(),
        )
        .await?
        .is_some()
        {
            queued += 1;
        }
    }
    transaction.commit().await?;
    Ok(queued)
}

/// A capacity "resets soon" alert fired for one of `owner`'s provider
/// accounts: queue their capacity-window rules. Called by the alert
/// evaluation after it commits; `cycle` names the window cycle so a rule
/// fires once per cycle.
pub async fn on_capacity_window(
    state: &AppState,
    owner: Uuid,
    provider_account_id: Uuid,
    cycle: &str,
) -> Result<usize, AppError> {
    let mut transaction = state.pool.begin().await?;
    let rules = sqlx::query_as::<_, RuleRow>(&format!(
        r#"{RULE_SELECT}
        WHERE rule.owner_actor_id = $1 AND rule.enabled AND rule.trigger_kind = 'capacity_window'
          AND (rule.trigger->>'providerAccountId' IS NULL
               OR rule.trigger->>'providerAccountId' = $2::text)
        ORDER BY rule.created_at, rule.id"#
    ))
    .bind(owner)
    .bind(provider_account_id)
    .fetch_all(&mut *transaction)
    .await?;
    let mut queued = 0;
    for rule in &rules {
        let subject = json!({ "providerAccountId": provider_account_id });
        let run_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO automation_runs (id, rule_id, project_id, dedupe_key, trigger_summary, subject)
            VALUES ($1, $2, $3, $4, 'A provider account resets soon with capacity left', $5)
            ON CONFLICT (rule_id, dedupe_key) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(rule.id)
        .bind(rule.project_id)
        .bind(format!("capacity:{provider_account_id}:{cycle}").chars().take(300).collect::<String>())
        .bind(subject)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(run_id) = run_id {
            jobs::enqueue(
                &mut *transaction,
                NewJob::new(FIRE_KIND)
                    .project(rule.organization_id, rule.project_id)
                    .payload(FirePayload { run_id })
                    .unique(format!("automation-run:{run_id}")),
            )
            .await?;
            queued += 1;
        }
    }
    transaction.commit().await?;
    Ok(queued)
}

// ---------------------------------------------------------------------------
// Firing

/// Who a rule acts as, checked now.
#[derive(Clone, Debug)]
struct Acting {
    actor_id: Uuid,
    actor_kind: ActorKind,
    via_grant_id: Option<Uuid>,
    connection_id: Option<Uuid>,
    /// An agent's live credential, which its tools are checked against.
    credential_id: Option<Uuid>,
}

#[derive(FromRow)]
struct AccessRow {
    kind: String,
    in_workspace: bool,
    can_access: bool,
    app_gone: bool,
    project_archived: bool,
    credential_id: Option<Uuid>,
}

/// Checks the rule's maker can still do what it does. Err carries the
/// reason it's turned off.
async fn acting_access(pool: &PgPool, rule: &RuleRow) -> Result<Result<Acting, String>, AppError> {
    let row = sqlx::query_as::<_, AccessRow>(
        r#"
        SELECT actor.kind,
               actor.status = 'active' AND EXISTS (
                   SELECT 1 FROM organization_memberships membership
                   WHERE membership.organization_id = $2 AND membership.actor_id = $1
                     AND membership.enabled
               ) AS in_workspace,
               actor_can_access_project($1, $4, $3) AS can_access,
               $5::uuid IS NOT NULL AND NOT EXISTS (
                   SELECT 1 FROM mcp_oauth_grants grant_row
                   WHERE grant_row.id = $5 AND grant_row.revoked_at IS NULL
               ) AS app_gone,
               EXISTS (
                   SELECT 1 FROM projects WHERE id = $3 AND archived_at IS NOT NULL
               ) AS project_archived,
               (SELECT credential.id FROM mcp_client_credentials credential
                WHERE $4::uuid IS NOT NULL
                  AND credential.external_agent_connection_id = $4
                  AND credential.actor_id = $1
                  AND credential.status = 'active' AND credential.revoked_at IS NULL
                  AND credential.expires_at > now()
                  AND credential.scopes @> ARRAY['write']::text[]
                ORDER BY credential.created_at DESC LIMIT 1) AS credential_id
        FROM actors actor WHERE actor.id = $1
        "#,
    )
    .bind(rule.owner_actor_id)
    .bind(rule.organization_id)
    .bind(rule.project_id)
    .bind(rule.external_agent_connection_id)
    .bind(rule.via_oauth_grant_id)
    .fetch_one(pool)
    .await?;
    let who = rule.who();
    let actor_kind = match row.kind.as_str() {
        "human" => ActorKind::Human,
        "external_agent" => ActorKind::ExternalAgent,
        _ => {
            return Ok(Err(format!(
                "Turned off because {who} can't run automations."
            )));
        }
    };
    if row.project_archived {
        return Ok(Err("Turned off because the project is archived.".to_owned()));
    }
    if !row.in_workspace {
        return Ok(Err(format!(
            "Turned off because {who}, who made it, is no longer in this workspace. Rules act as their maker; someone else can make it again."
        )));
    }
    if !row.can_access {
        return Ok(Err(format!(
            "Turned off because {who}, who made it, can no longer work in this project."
        )));
    }
    if row.app_gone {
        return Ok(Err(
            "Turned off because the app it was made through was disconnected.".to_owned(),
        ));
    }
    if actor_kind == ActorKind::ExternalAgent && row.credential_id.is_none() {
        return Ok(Err(format!(
            "Turned off because {who} has no active connection that can make changes."
        )));
    }
    let can_edit = crate::memory::project_access(
        pool,
        rule.owner_actor_id,
        actor_kind,
        rule.project_id,
        PolicyAction::WorkItemEdit,
    )
    .await
    .is_ok();
    if !can_edit {
        return Ok(Err(format!(
            "Turned off because {who}, who made it, can no longer make changes in this project."
        )));
    }
    Ok(Ok(Acting {
        actor_id: rule.owner_actor_id,
        actor_kind,
        via_grant_id: rule.via_oauth_grant_id,
        connection_id: rule.external_agent_connection_id,
        credential_id: row.credential_id,
    }))
}

/// Turns a rule off and tells its maker, or the project's admins when the
/// maker can't be told in the project.
async fn disable(state: &AppState, rule: &RuleRow, reason: &str) -> Result<bool, AppError> {
    let mut transaction = state.pool.begin().await?;
    let turned_off = sqlx::query(
        r#"
        UPDATE automation_rules
        SET enabled = false, disabled_reason = $2, next_run_at = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1 AND enabled
        "#,
    )
    .bind(rule.id)
    .bind(reason)
    .execute(&mut *transaction)
    .await?
    .rows_affected()
        == 1;
    if !turned_off {
        return Ok(false);
    }
    let owner_is_person = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM actors actor
            JOIN users person ON person.actor_id = actor.id AND person.status = 'active'
            JOIN organization_memberships membership
              ON membership.actor_id = actor.id AND membership.organization_id = $2
             AND membership.enabled
            WHERE actor.id = $1 AND actor.status = 'active'
        )
        "#,
    )
    .bind(rule.owner_actor_id)
    .bind(rule.organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    let project_name = sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1")
        .bind(rule.project_id)
        .fetch_one(&mut *transaction)
        .await?;
    let mut recipients: Vec<(Uuid, Uuid)> = Vec::new();
    if owner_is_person {
        let in_project =
            sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
                .bind(rule.owner_actor_id)
                .bind(rule.project_id)
                .fetch_one(&mut *transaction)
                .await?;
        // A maker who lost the project hears about it in their own space.
        let place = if in_project {
            Some(rule.project_id)
        } else {
            crate::direct::space_for_member(
                &mut transaction,
                rule.organization_id,
                rule.owner_actor_id,
            )
            .await?
        };
        if let Some(place) = place {
            recipients.push((rule.owner_actor_id, place));
        }
    }
    if recipients.is_empty() {
        let admins = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT membership.actor_id FROM project_memberships membership
            WHERE membership.project_id = $1 AND membership.enabled
              AND membership.role IN ('owner', 'admin')
              AND actor_can_access_project(membership.actor_id, NULL, $1)
            ORDER BY membership.actor_id LIMIT 20
            "#,
        )
        .bind(rule.project_id)
        .fetch_all(&mut *transaction)
        .await?;
        recipients.extend(admins.into_iter().map(|admin| (admin, rule.project_id)));
    }
    let mut announced = Vec::new();
    for (recipient, place) in recipients {
        let notification = NewNotification {
            project_id: place,
            recipient_actor_id: recipient,
            kind: DISABLED_KIND,
            subject_kind: SUBJECT_KIND,
            subject_id: rule.id,
            actor_id: None,
            conversation_id: None,
            message_id: None,
            work_item_id: None,
            comment_id: None,
            agent_run_id: None,
            approval_id: None,
            pull_request_id: None,
            detail: json!({
                "title": format!("Automation \u{201c}{}\u{201d} was turned off", rule.name),
                "preview": reason,
                "href": format!("/settings/projects/{}/automations", rule.project_id),
                "place": project_name,
                "ruleId": rule.id,
            }),
            group_key: format!("automation-disabled:{}", rule.id),
            dedupe_key: format!("automation-disabled:{}:{}", rule.id, Uuid::now_v7()),
        };
        if notifications::deliver(&mut transaction, &notification).await? {
            announced.push((place, recipient));
        }
    }
    for (place, recipient) in announced {
        notifications::announce(&mut transaction, place, recipient).await?;
    }
    transaction.commit().await?;
    Ok(true)
}

#[derive(FromRow)]
struct DueRun {
    rule_id: Uuid,
    project_id: Uuid,
    state: String,
    subject: Value,
    trigger_summary: String,
}

async fn finish(
    pool: &PgPool,
    run_id: Uuid,
    state: &str,
    detail: Option<&str>,
    results: &[Value],
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE automation_runs
        SET state = $2, detail = $3, results = $4, finished_at = now()
        WHERE id = $1 AND state = 'pending'
        "#,
    )
    .bind(run_id)
    .bind(state)
    .bind(detail.map(|text| text.chars().take(500).collect::<String>()))
    .bind(Value::Array(results.to_vec()))
    .execute(pool)
    .await?;
    Ok(())
}

/// Runs one firing: checks its maker's access, then each action, checking
/// again before each.
async fn fire(state: &AppState, run_id: Uuid) -> Result<(), AppError> {
    let pool = &state.pool;
    let Some(due) = sqlx::query_as::<_, DueRun>(
        "SELECT rule_id, project_id, state, subject, trigger_summary FROM automation_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };
    if due.state != "pending" {
        return Ok(());
    }
    let Ok(rule) = fetch(pool, due.project_id, due.rule_id).await else {
        return Ok(());
    };
    if !rule.enabled {
        return finish(pool, run_id, "skipped", Some("The rule is off."), &[]).await;
    }
    let recent = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM automation_runs
        WHERE rule_id = $1 AND id <> $2 AND state <> 'skipped'
          AND created_at > now() - interval '1 hour'
        "#,
    )
    .bind(rule.id)
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if recent >= MAX_FIRINGS_PER_HOUR {
        let reason = "Turned off because it fired more than 30 times in an hour, which usually means it sets itself off. Check its trigger and actions, then turn it back on.";
        disable(state, &rule, reason).await?;
        return finish(pool, run_id, "skipped", Some(reason), &[]).await;
    }
    let (trigger, actions) = match (rule.trigger(), rule.actions()) {
        (Ok(trigger), Ok(actions)) => (trigger, actions),
        _ => {
            let reason = "Turned off because it couldn't be read.";
            disable(state, &rule, reason).await?;
            return finish(pool, run_id, "skipped", Some(reason), &[]).await;
        }
    };
    let subject: Subject = serde_json::from_value(due.subject.clone()).unwrap_or_default();
    let provider_account_id = due
        .subject
        .get("providerAccountId")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<Uuid>().ok());
    let context = Context::load(state, &rule, &subject, &due.trigger_summary).await?;
    // A rule made through an app skips what that app's current limit hides:
    // the task or pull request the context loaded (a run's task, say) would
    // otherwise reach its messages through placeholders. Nothing is sent
    // before this.
    if let Some(limits) = app_limits(pool, &rule).await?
        && limit_hides(
            &limits,
            rule.project_id,
            context.work_item_id.is_some(),
            context.pull_request_id.is_some() || context.pull_request.is_some(),
        )
    {
        return finish(
            pool,
            run_id,
            "skipped",
            Some("Skipped: the app it was made through can't see what this is about."),
            &[],
        )
        .await;
    }
    let mut results = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        // A rule made through an app acts within that app's limits when it
        // fires, as the tool call that made it did.
        let limits = app_limits(pool, &rule).await?;
        let acting = match with_limits(limits.clone(), acting_access(pool, &rule)).await? {
            Ok(acting) => acting,
            Err(reason) => {
                disable(state, &rule, &reason).await?;
                let state_name = if results.is_empty() {
                    "skipped"
                } else {
                    "partial"
                };
                return finish(pool, run_id, state_name, Some(&reason), &results).await;
            }
        };
        let outcome = with_limits(
            limits,
            run_action(
                state,
                &rule,
                &trigger,
                &acting,
                &context,
                provider_account_id,
                run_id,
                index,
                action,
            ),
        )
        .await;
        results.push(match outcome {
            Ok(detail) => json!({ "kind": action.kind(), "ok": true, "detail": detail }),
            Err(error) => json!({ "kind": action.kind(), "ok": false, "detail": error }),
        });
    }
    let succeeded = results
        .iter()
        .filter(|result| result["ok"] == json!(true))
        .count();
    let state_name = if succeeded == results.len() {
        "succeeded"
    } else if succeeded == 0 {
        "failed"
    } else {
        "partial"
    };
    sqlx::query(
        "UPDATE automation_rules SET last_fired_at = now(), fire_count = fire_count + 1 WHERE id = $1",
    )
    .bind(rule.id)
    .execute(pool)
    .await?;
    finish(pool, run_id, state_name, None, &results).await
}

/// What an action's text can mention, and what it acts on.
struct Context {
    work_item_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    pull_request_id: Option<Uuid>,
    variables: Vec<(&'static str, String)>,
    assignee: Option<Uuid>,
    pull_request: Option<PullRequestRow>,
}

#[derive(Clone, FromRow)]
struct PullRequestRow {
    pull_request_number: i64,
    title: String,
    html_url: String,
    head_ref: String,
}

impl Context {
    async fn load(
        state: &AppState,
        rule: &RuleRow,
        subject: &Subject,
        summary: &str,
    ) -> Result<Self, AppError> {
        let origin = events::public_origin(state);
        let mut variables = vec![
            ("rule.name", rule.name.clone()),
            ("event", summary.to_owned()),
        ];
        let mut assignee = None;
        let mut work_item_id = subject.work_item_id;
        if work_item_id.is_none()
            && let Some(run_id) = subject.agent_run_id
        {
            work_item_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT work_item_id FROM launches WHERE agent_run_id = $1 AND project_id = $2 LIMIT 1",
            )
            .bind(run_id)
            .bind(rule.project_id)
            .fetch_optional(&state.pool)
            .await?;
        }
        if let Some(id) = work_item_id
            && let Some(task) = crate::webhooks::task_data(state, rule.project_id, id).await?
        {
            let text = |key: &str| {
                task.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            variables.push(("task.key", text("key")));
            variables.push(("task.title", text("title")));
            variables.push(("task.status", text("status")));
            variables.push((
                "task.url",
                format!("{origin}/projects/{}/work-items/{id}", rule.project_id),
            ));
            assignee = task
                .get("assigneeActorId")
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok());
        }
        if let Some(run_state) = &subject.state
            && subject.agent_run_id.is_some()
        {
            variables.push(("run.state", run_state.clone()));
        }
        let pull_request = match subject.pull_request_id {
            Some(id) => {
                sqlx::query_as::<_, PullRequestRow>(
                    r#"
                    SELECT pull_request_number, title, html_url, head_ref
                    FROM github_pull_requests WHERE project_id = $1 AND id = $2
                    "#,
                )
                .bind(rule.project_id)
                .bind(id)
                .fetch_optional(&state.pool)
                .await?
            }
            None => None,
        };
        if let Some(pr) = &pull_request {
            variables.push(("pr.number", pr.pull_request_number.to_string()));
            variables.push(("pr.title", pr.title.clone()));
            variables.push(("pr.url", pr.html_url.clone()));
        }
        Ok(Self {
            work_item_id,
            agent_run_id: subject.agent_run_id,
            pull_request_id: subject.pull_request_id,
            variables,
            assignee,
            pull_request,
        })
    }

    /// Fills in {{variables}}; unknown ones become empty.
    fn render(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str(&rest[start..]);
                return out;
            };
            let name = after[..end].trim();
            if let Some((_, value)) = self.variables.iter().find(|(key, _)| *key == name) {
                out.push_str(value);
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out.chars().take(MAX_TEXT_CHARACTERS).collect()
    }
}

/// The tasks an action works on: the event's task, or for schedules and
/// capacity windows the tasks matching the conditions.
async fn target_tasks(
    pool: &PgPool,
    rule: &RuleRow,
    trigger: &Trigger,
    context: &Context,
    limit: i64,
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    if !trigger.picks_tasks() {
        return Ok(Ok(context.work_item_id.into_iter().collect()));
    }
    let Some(params) = condition_params(pool, rule).await? else {
        return Ok(Err("Its saved view was deleted.".to_owned()));
    };
    if params.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    Ok(Ok(views::matching_tasks(
        pool,
        rule.project_id,
        &params,
        rule.owner_actor_id,
        None,
        limit,
    )
    .await?))
}

/// The maker, as an MCP caller confined to the rule's project, for actions
/// that go through the agent tools.
fn principal_for(rule: &RuleRow, acting: &Acting) -> McpPrincipal {
    McpPrincipal {
        credential_id: acting.credential_id.unwrap_or(rule.id),
        mcp_credential_id: acting.credential_id,
        oauth_grant_id: acting.via_grant_id,
        actor_id: acting.actor_id,
        actor_kind: acting.actor_kind,
        project_id: rule.project_id,
        external_agent_connection_id: acting.connection_id,
        client_name: format!("Automation: {}", rule.name)
            .chars()
            .take(80)
            .collect(),
        scopes: vec!["read".to_owned(), "write".to_owned()],
        authentication_class: "automation",
        account_wide: false,
        project_explicit: true,
    }
}

async fn call_tool(
    state: &AppState,
    rule: &RuleRow,
    acting: &Acting,
    tool: &str,
    arguments: Value,
) -> Result<Value, String> {
    let result = mcp::call_tool_direct(state, principal_for(rule, acting), tool, arguments)
        .await
        .map_err(|error| match error {
            mcp::DirectCallError::UnknownTool => "the tool is missing".to_owned(),
            mcp::DirectCallError::InvalidArguments(message) => message,
            mcp::DirectCallError::Refused(error) => error.to_string(),
        })?;
    match result.get("outcome").and_then(Value::as_str) {
        None | Some("ok" | "applied") => Ok(result),
        Some(_) => Err(result
            .get("message")
            .or_else(|| result.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("the change was refused")
            .chars()
            .take(300)
            .collect()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_action(
    state: &AppState,
    rule: &RuleRow,
    trigger: &Trigger,
    acting: &Acting,
    context: &Context,
    provider_account_id: Option<Uuid>,
    run_id: Uuid,
    index: usize,
    action: &Action,
) -> Result<String, String> {
    let database = |error: AppError| error.to_string();
    match action {
        Action::Launch {
            environment_id,
            prompt,
            agent_profile_id,
        } => {
            let limit = match trigger {
                Trigger::CapacityWindow { max_tasks, .. } => i64::from(max_tasks.unwrap_or(3)),
                _ => 10,
            };
            let tasks = target_tasks(&state.pool, rule, trigger, context, limit)
                .await
                .map_err(database)??;
            let caller = LaunchCaller {
                actor_id: acting.actor_id,
                actor_kind: acting.actor_kind,
                via_grant_id: acting.via_grant_id,
                mcp: acting.actor_kind == ActorKind::ExternalAgent,
                session_id: None,
                request_id: Some(format!("automation:{run_id}")),
            };
            let idempotency = IdempotencyKey::parse(&format!("automation-{run_id}-{index}"));
            let checks_prompt = context.pull_request.as_ref().map(|pr| {
                format!(
                    "Checks failed on pull request #{} \u{201c}{}\u{201d} ({}), branch {}. Find out why from the failing checks, fix it on that branch and push.",
                    pr.pull_request_number, pr.title, pr.html_url, pr.head_ref
                )
            });
            let prompt = prompt
                .as_deref()
                .map(|text| context.render(text))
                .or_else(|| {
                    matches!(trigger, Trigger::PullRequestChecksFailed {})
                        .then(|| checks_prompt.clone())
                        .flatten()
                });
            if tasks.is_empty() {
                // A pull request with no task: start a run in the thread of
                // the run that opened it.
                if let (Some(run_id), Some(prompt)) = (context.agent_run_id, prompt) {
                    let (conversation_id, repository_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
                        "SELECT conversation_id, repository_id FROM agent_runs WHERE id = $1",
                    )
                    .bind(run_id)
                    .fetch_one(&state.pool)
                    .await
                    .map_err(|error| database(error.into()))?;
                    let (_, started) = crate::api::start_run_in(
                        state,
                        &caller,
                        acting.via_grant_id,
                        conversation_id,
                        crate::api::StartRunRequest {
                            prompt,
                            repository_id: Some(repository_id),
                            config: None,
                            provider_account_id,
                            agent_profile_id: *agent_profile_id,
                        },
                        idempotency,
                    )
                    .await
                    .map_err(database)?;
                    let _ = started;
                    return Ok("Started a run in the pull request's thread.".to_owned());
                }
                return Err("There was no task to launch on.".to_owned());
            }
            let single = tasks.len() == 1;
            let result = launch::launch_tasks(
                state,
                &caller,
                rule.project_id,
                LaunchRequest {
                    work_item_ids: tasks.clone(),
                    environment_id: *environment_id,
                    overrides: Default::default(),
                    provider_account_id,
                    prompt: if single { prompt } else { None },
                    start_when: Default::default(),
                    start_at: None,
                    queue: false,
                    dialog: false,
                    agent_profile_id: *agent_profile_id,
                },
                "automation",
                idempotency,
            )
            .await
            .map_err(database)?;
            // A rule a person's app or script made: its launches start only
            // within that app's limit when the queue drains.
            if rule.made_through_app && acting.actor_kind == ActorKind::Human {
                let ids = result
                    .launches
                    .iter()
                    .map(|view| view.id)
                    .collect::<Vec<_>>();
                sqlx::query("UPDATE launches SET made_through_app = true WHERE id = ANY($1)")
                    .bind(&ids)
                    .execute(&state.pool)
                    .await
                    .map_err(|error| database(error.into()))?;
            }
            Ok(format!(
                "Launched {} task{}.",
                result.launches.len(),
                if result.launches.len() == 1 { "" } else { "s" }
            ))
        }
        Action::SetField { field, value } => {
            let tasks = target_tasks(&state.pool, rule, trigger, context, MAX_TARGET_TASKS)
                .await
                .map_err(database)??;
            if tasks.is_empty() {
                return Err("There was no task to change.".to_owned());
            }
            let mut changed = 0;
            let mut last_error = None;
            for task in &tasks {
                let version = sqlx::query_scalar::<_, i64>(
                    "SELECT version FROM work_items WHERE project_id = $1 AND id = $2",
                )
                .bind(rule.project_id)
                .bind(task)
                .fetch_one(&state.pool)
                .await
                .map_err(|error| database(error.into()))?;
                let mut arguments = json!({
                    "taskId": task.to_string(),
                    "expectedVersion": version,
                    "clientRequestId": format!("automation-{run_id}-{index}-{task}"),
                });
                let key = match field.as_str() {
                    "add_label" => "addLabels",
                    "remove_label" => "removeLabels",
                    other => other,
                };
                arguments[key] = if key.ends_with("Labels") {
                    json!([value])
                } else {
                    json!(value)
                };
                match call_tool(state, rule, acting, "task_update", arguments).await {
                    Ok(_) => changed += 1,
                    Err(error) => last_error = Some(error),
                }
            }
            match last_error {
                Some(error) if changed == 0 => Err(error),
                Some(error) => Ok(format!(
                    "Changed {changed} of {} tasks; {error}",
                    tasks.len()
                )),
                None => Ok(format!(
                    "Changed {changed} task{}.",
                    if changed == 1 { "" } else { "s" }
                )),
            }
        }
        Action::Notify {
            recipients,
            message,
        } => notify(state, rule, context, run_id, index, *recipients, message)
            .await
            .map_err(database)?,
        Action::PostMessage {
            conversation_id,
            text,
        } => {
            call_tool(
                state,
                rule,
                acting,
                "message_send",
                json!({
                    "conversationId": conversation_id.to_string(),
                    "text": context.render(text),
                    "clientRequestId": format!("automation-{run_id}-{index}"),
                }),
            )
            .await?;
            Ok("Posted the message.".to_owned())
        }
    }
}

async fn notify(
    state: &AppState,
    rule: &RuleRow,
    context: &Context,
    run_id: Uuid,
    index: usize,
    recipients: Recipients,
    message: &str,
) -> Result<Result<String, String>, AppError> {
    let candidates: Vec<Uuid> = match recipients {
        Recipients::Owner => vec![rule.owner_actor_id],
        Recipients::Assignee => context.assignee.into_iter().collect(),
        Recipients::ProjectAdmins => {
            sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT actor_id FROM project_memberships
                WHERE project_id = $1 AND enabled AND role IN ('owner', 'admin')
                ORDER BY actor_id LIMIT 20
                "#,
            )
            .bind(rule.project_id)
            .fetch_all(&state.pool)
            .await?
        }
    };
    let mut transaction = state.pool.begin().await?;
    let mut told = Vec::new();
    for recipient in candidates {
        // Only people who can see the project hear about it.
        let can_see =
            sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
                .bind(recipient)
                .bind(rule.project_id)
                .fetch_one(&mut *transaction)
                .await?;
        if !can_see {
            continue;
        }
        let rendered = context.render(message);
        let notification = NewNotification {
            project_id: rule.project_id,
            recipient_actor_id: recipient,
            kind: NOTIFICATION_KIND,
            subject_kind: if context.work_item_id.is_some() {
                "work_item"
            } else {
                SUBJECT_KIND
            },
            subject_id: context.work_item_id.unwrap_or(rule.id),
            actor_id: None,
            conversation_id: None,
            message_id: None,
            work_item_id: context.work_item_id,
            comment_id: None,
            agent_run_id: context.agent_run_id,
            approval_id: None,
            pull_request_id: context.pull_request_id,
            detail: json!({
                "title": rendered.chars().take(200).collect::<String>(),
                "preview": format!("From the automation \u{201c}{}\u{201d}", rule.name),
                "href": format!("/settings/projects/{}/automations", rule.project_id),
                "ruleId": rule.id,
            }),
            group_key: format!("automation:{}:{run_id}:{index}", rule.id),
            dedupe_key: format!("automation:{run_id}:{index}"),
        };
        if notifications::deliver(&mut transaction, &notification).await? {
            told.push(recipient);
        }
    }
    for recipient in &told {
        notifications::announce(&mut transaction, rule.project_id, *recipient).await?;
    }
    transaction.commit().await?;
    Ok(if told.is_empty() {
        Err("Nobody to notify.".to_owned())
    } else {
        Ok(format!(
            "Notified {} {}.",
            told.len(),
            if told.len() == 1 { "person" } else { "people" }
        ))
    })
}

// ---------------------------------------------------------------------------
// Browser routes

async fn list_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<RuleList>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(list(&state, &caller, project_id).await?))
}

async fn create_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateRule>,
) -> Result<(StatusCode, Json<RuleView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(create(&state, &caller, project_id, request).await?),
    ))
}

async fn update_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, rule_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateRule>,
) -> Result<Json<RuleView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(
        update(&state, &caller, project_id, rule_id, request).await?,
    ))
}

async fn delete_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, rule_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    delete(&state, &caller, project_id, rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_now_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, rule_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<RunNow>,
) -> Result<Json<RunView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(
        run_now(&state, &caller, project_id, rule_id, request).await?,
    ))
}

async fn runs_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, rule_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<RunPage>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(runs(&state, &caller, project_id, rule_id).await?))
}

/// The limits of the app a rule was made through, for a person's rule made
/// through one: the person's role there cut down by the role's and their own
/// agent limit, read again each time so a narrowed limit applies at once.
/// None for a rule made in the browser or by an agent acting as itself.
async fn app_limits(pool: &PgPool, rule: &RuleRow) -> Result<Option<AgentLimits>, AppError> {
    if !rule.made_through_app {
        return Ok(None);
    }
    let mut connection = pool.acquire().await?;
    let is_human = sqlx::query_scalar::<_, bool>("SELECT kind = 'human' FROM actors WHERE id = $1")
        .bind(rule.owner_actor_id)
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(false);
    if !is_human {
        return Ok(None);
    }
    Ok(Some(
        crate::roles::app_access(&mut connection, rule.owner_actor_id)
            .await?
            .limits,
    ))
}

/// Whether an app's limits keep it from what a rule is about in
/// `project_id`: a task without "see tasks", or a pull request without "see
/// code".
fn limit_hides(limits: &AgentLimits, project_id: Uuid, task: bool, pull_request: bool) -> bool {
    use crate::policy::{Permission, PermissionSet};
    let allowed = limits
        .projects
        .get(&project_id)
        .copied()
        .unwrap_or(PermissionSet::EMPTY);
    (task && !allowed.contains(Permission::TasksView))
        || (pull_request && !allowed.contains(Permission::CodeView))
}

/// Runs `call` within `limits`, if there are any.
async fn with_limits<F: std::future::Future>(limits: Option<AgentLimits>, call: F) -> F::Output {
    match limits {
        Some(limits) => with_agent_limits(limits, call).await,
        None => call.await,
    }
}

/// Whether a person is making or changing a rule through an app or script
/// (MCP or the public API), whose agent limit the rule then keeps.
fn through_app(caller: &Caller) -> bool {
    caller.actor_kind == ActorKind::Human && caller.channel == "mcp"
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{Action, Conditions, Recipients, Trigger, check_shape, next_run_after};

    fn notify() -> Action {
        Action::Notify {
            recipients: Recipients::Owner,
            message: " Ready: {{task.key}} ".to_owned(),
        }
    }

    #[test]
    fn rules_parse_from_tagged_json() {
        let trigger: Trigger = serde_json::from_value(serde_json::json!({
            "kind": "run_finished", "outcomes": ["failed"]
        }))
        .expect("trigger");
        assert_eq!(trigger.kind(), "run_finished");
        let action: Action = serde_json::from_value(serde_json::json!({
            "kind": "post_message",
            "conversationId": "0190a0a0-0000-7000-8000-000000000000",
            "text": "hi"
        }))
        .expect("action");
        assert!(matches!(action, Action::PostMessage { .. }));
        assert!(serde_json::from_value::<Trigger>(serde_json::json!({"kind": "never"})).is_err());
    }

    #[test]
    fn shapes_are_checked() {
        let mut actions = vec![notify()];
        check_shape(
            &mut Trigger::TaskCreated {},
            &Conditions::default(),
            &mut actions,
        )
        .expect("valid rule");
        assert!(
            matches!(&actions[0], Action::Notify { message, .. } if message == "Ready: {{task.key}}")
        );
        // Matching needs conditions to match against.
        assert!(
            check_shape(
                &mut Trigger::TaskMatches {},
                &Conditions::default(),
                &mut actions
            )
            .is_err()
        );
        // A schedule that launches needs conditions to pick tasks.
        let mut schedule = Trigger::Schedule {
            frequency: "daily".to_owned(),
            every: 1,
            weekdays: vec![],
            month_day: None,
            time: "09:00".to_owned(),
            timezone: "Europe/London".to_owned(),
        };
        let mut launch = vec![Action::Launch {
            environment_id: None,
            prompt: None,
            agent_profile_id: None,
        }];
        assert!(check_shape(&mut schedule, &Conditions::default(), &mut launch).is_err());
        let conditions = Conditions {
            view_id: None,
            filters: [("status".to_owned(), "Todo".to_owned())].into(),
        };
        check_shape(&mut schedule, &conditions, &mut launch).expect("valid rule");
        // A profile is checked against the project in check_references.
        let mut profile = vec![Action::Launch {
            environment_id: None,
            prompt: None,
            agent_profile_id: Some(uuid::Uuid::nil()),
        }];
        check_shape(&mut Trigger::TaskCreated {}, &conditions, &mut profile)
            .expect("a launch may name an agent profile");
        let mut bad_zone = Trigger::Schedule {
            frequency: "daily".to_owned(),
            every: 1,
            weekdays: vec![],
            month_day: None,
            time: "09:00".to_owned(),
            timezone: "Mars/Olympus".to_owned(),
        };
        assert!(check_shape(&mut bad_zone, &conditions, &mut [notify()]).is_err());
        assert!(check_shape(&mut Trigger::TaskCreated {}, &conditions, &mut []).is_err());
    }

    #[test]
    fn schedules_run_at_local_time() {
        let trigger = Trigger::Schedule {
            frequency: "weekly".to_owned(),
            every: 1,
            weekdays: vec![1],
            month_day: None,
            time: "09:00".to_owned(),
            timezone: "America/New_York".to_owned(),
        };
        // Saturday 17 October 2026, noon UTC: next is Monday 19th, 09:00 in
        // New York, which is 13:00 UTC (daylight time).
        let after = Utc
            .with_ymd_and_hms(2026, 10, 17, 12, 0, 0)
            .single()
            .expect("valid time");
        assert_eq!(
            next_run_after(&trigger, after),
            Some(Utc.with_ymd_and_hms(2026, 10, 19, 13, 0, 0).unwrap())
        );
    }
}
