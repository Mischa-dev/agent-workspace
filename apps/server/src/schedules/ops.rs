//! Follow-up operations (A2.11): an account sets, changes and lists the
//! times it wants to be told again.

use chrono::{DateTime, TimeDelta, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// Open follow-ups one account may keep in a workspace.
const MAX_ACTIVE: i64 = 200;
/// How far ahead the first time may be.
const MAX_AHEAD_DAYS: i64 = 366;
const MAX_NOTE_CHARACTERS: usize = 500;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ScheduleList>()
        .add::<ScheduleCreate>()
        .add::<ScheduleUpdate>();
}

/// One follow-up, as its account sees it.
#[derive(Debug, Serialize, JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Your own words for it.
    pub note: String,
    /// The kind of thing it is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thing_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub thing_id: Option<Uuid>,
    /// Repeats this often; none: once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<i32>,
    /// active, done (a one-time follow-up that fired) or cancelled.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_due_at: Option<String>,
    pub times_due: i32,
    pub version: i64,
    pub created_at: String,
}

const VIEW_COLUMNS: &str = "id, note, thing_kind, thing_id, every_minutes, state, \
    to_char(next_due_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS next_due_at, \
    to_char(last_due_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS last_due_at, \
    times_due, version, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at";

async fn load(
    call: &mut OpCall<'_>,
    id: Uuid,
    lock: bool,
) -> Result<Option<ScheduleView>, OpError> {
    let account = call.account_id();
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    Ok(sqlx::query_as::<_, ScheduleView>(&format!(
        "SELECT {VIEW_COLUMNS} FROM agent_schedules \
         WHERE id = $1 AND account_id = $2 AND workspace_id = $3{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .bind(account)
    .bind(workspace)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?)
}

/// When it is first due: at a time, in some minutes, or one interval from
/// now. `None` when nothing says.
fn first_due(
    at: Option<&str>,
    in_minutes: Option<i64>,
    every_minutes: Option<i32>,
) -> Result<Option<DateTime<Utc>>, OpError> {
    let now = Utc::now();
    let when = match (at, in_minutes) {
        (Some(_), Some(_)) => {
            return Err(OpError::invalid("at", "Give at or inMinutes, not both."));
        }
        (Some(at), None) => Some(
            DateTime::parse_from_rfc3339(at)
                .map_err(|_| OpError::invalid("at", "Give a time like 2026-11-18T09:00:00Z."))?
                .with_timezone(&Utc),
        ),
        (None, Some(minutes)) => {
            if !(1..=MAX_AHEAD_DAYS * 24 * 60).contains(&minutes) {
                return Err(OpError::invalid(
                    "inMinutes",
                    "Give 1 minute to a year of minutes.",
                ));
            }
            Some(now + TimeDelta::minutes(minutes))
        }
        (None, None) => every_minutes.map(|every| now + TimeDelta::minutes(i64::from(every))),
    };
    if let Some(when) = when {
        if when <= now {
            return Err(OpError::invalid("at", "Give a time ahead of now."));
        }
        if when > now + TimeDelta::days(MAX_AHEAD_DAYS) {
            return Err(OpError::invalid("at", "Give a time within a year."));
        }
    }
    Ok(when)
}

fn check_every(every: Option<i32>) -> Result<(), OpError> {
    match every {
        Some(every) if !(5..=525_600).contains(&every) => Err(OpError::invalid(
            "everyMinutes",
            "Repeat every 5 minutes to every year (525600).",
        )),
        _ => Ok(()),
    }
}

fn check_note(note: Option<&str>) -> Result<String, OpError> {
    let note = note.unwrap_or_default().trim();
    if note.chars().count() > MAX_NOTE_CHARACTERS || note.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "note",
            "Keep the note to one line of at most 500 characters.",
        ));
    }
    Ok(note.to_owned())
}

// --- schedule_list -----------------------------------------------------------

pub struct ScheduleList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleListInput {
    /// Include done and cancelled ones (the latest 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_finished: Option<bool>,
}

impl Targeted for ScheduleListInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleListView {
    pub schedules: Vec<ScheduleView>,
}

impl Operation for ScheduleList {
    const NAME: &'static str = "schedule_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List follow-ups";
    const DESCRIPTION: &'static str = "The follow-ups you set in this workspace with \
        schedule_create, soonest first: what each is about, when it is next due and how often \
        it repeats.";

    type Input = ScheduleListInput;
    type Output = ScheduleListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ScheduleListInput,
    ) -> Result<ScheduleListView, OpError> {
        let account = call.account_id();
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let schedules = sqlx::query_as::<_, ScheduleView>(&format!(
            "SELECT {VIEW_COLUMNS} FROM agent_schedules \
             WHERE account_id = $1 AND workspace_id = $2 AND ($3 OR state = 'active') \
             ORDER BY state <> 'active', next_due_at NULLS LAST, updated_at DESC, id \
             LIMIT 200"
        ))
        .bind(account)
        .bind(workspace)
        .bind(input.include_finished.unwrap_or(false))
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(ScheduleListView { schedules })
    }
}

// --- schedule_create ---------------------------------------------------------

pub struct ScheduleCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleCreateInput {
    /// Your own words for why (one line, at most 500 characters). Only you
    /// see it; schedule.due carries ids, so read it with schedule_list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A thing you can see that it is about: a task, a conversation, a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<IdRef>,
    /// When it is first due (RFC 3339, such as 2026-11-18T09:00:00Z).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Or: in this many minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_minutes: Option<i64>,
    /// Repeat this often (5 to 525600 minutes). Left out: once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<i32>,
}

impl Targeted for ScheduleCreateInput {
    fn target(&self) -> Target {
        match &self.about {
            Some(about) => Target::thing("thing", about.id()),
            None => Target::Account,
        }
    }
}

impl Operation for ScheduleCreate {
    const NAME: &'static str = "schedule_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set a follow-up";
    const DESCRIPTION: &'static str = "Ask to be told again later: at a time (at), in some \
        minutes (inMinutes), or every so many minutes (everyMinutes), optionally about one \
        thing you can see (about). When it is due you hear schedule.due, with its id and the \
        thing's, through event_list or your connection. For something a person should be \
        reminded of, use later_save.";
    const EMITS: &'static [&'static str] = &["schedule.changed"];

    type Input = ScheduleCreateInput;
    type Output = ScheduleView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ScheduleCreateInput,
    ) -> Result<ScheduleView, OpError> {
        check_every(input.every_minutes)?;
        let note = check_note(input.note.as_deref())?;
        let Some(due) = first_due(input.at.as_deref(), input.in_minutes, input.every_minutes)?
        else {
            return Err(OpError::invalid(
                "at",
                "Say when: at, inMinutes or everyMinutes.",
            ));
        };
        let about = match &input.about {
            Some(about) => {
                let id = about.id();
                let kind = crate::history::ops::visible_kind(call, id).await?;
                Some((id, kind))
            }
            None => None,
        };
        let account = call.account_id();
        let workspace = call.ctx.workspace_id;
        let space = call.ctx.space_id;
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM agent_schedules \
             WHERE account_id = $1 AND workspace_id = $2 AND state = 'active'",
        )
        .bind(account)
        .bind(workspace)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if active >= MAX_ACTIVE {
            return Err(OpError::invalid(
                "at",
                "You have 200 follow-ups here already; cancel some with schedule_update.",
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO agent_schedules (
                id, workspace_id, space_id, account_id, thing_id, thing_kind, note,
                every_minutes, next_due_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(id)
        .bind(workspace)
        .bind(about.as_ref().and(space))
        .bind(account)
        .bind(about.as_ref().map(|(id, _)| *id))
        .bind(about.as_ref().map(|(_, kind)| kind.as_str()))
        .bind(&note)
        .bind(input.every_minutes)
        .bind(due)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        super::queue(tx, workspace, id, due).await?;
        super::changed(tx, workspace, account, id).await?;
        call.audit_target(
            "schedule",
            id,
            json!({ "repeats": input.every_minutes.is_some(), "about": about.is_some() }),
        );
        load(call, id, false).await?.ok_or(OpError::Unavailable)
    }
}

// --- schedule_update ---------------------------------------------------------

pub struct ScheduleUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleUpdateInput {
    pub schedule_id: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Move when it is next due (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Or: in this many minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_minutes: Option<i64>,
    /// Repeat this often; 0 makes it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<i32>,
    /// true cancels it: it won't be due again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for ScheduleUpdateInput {
    fn target(&self) -> Target {
        Target::Account
    }
}

impl Operation for ScheduleUpdate {
    const NAME: &'static str = "schedule_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "agents";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change a follow-up";
    const DESCRIPTION: &'static str = "Change one of your follow-ups: its note, when it is next \
        due (at or inMinutes), how often it repeats (everyMinutes, 0 for once), or cancel it \
        (cancel: true).";
    const EMITS: &'static [&'static str] = &["schedule.changed"];

    type Input = ScheduleUpdateInput;
    type Output = ScheduleView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ScheduleUpdateInput,
    ) -> Result<ScheduleView, OpError> {
        let id = input.schedule_id.id();
        let found = load(call, id, true)
            .await?
            .ok_or(OpError::not_found("That follow-up"))?;
        if input
            .expected_version
            .is_some_and(|expected| expected != found.version)
        {
            return Err(AppError::VersionConflict {
                resource: "schedule",
                current_version: found.version,
            }
            .into());
        }
        let every = match input.every_minutes {
            Some(0) => None,
            Some(every) => {
                check_every(Some(every))?;
                Some(every)
            }
            None => found.every_minutes,
        };
        let note = match input.note.as_deref() {
            Some(note) => check_note(Some(note))?,
            None => found.note.clone(),
        };
        let cancel = input.cancel.unwrap_or(false);
        let moved = first_due(input.at.as_deref(), input.in_minutes, None)?;
        if found.state != "active" && !cancel && moved.is_none() {
            return Err(OpError::invalid(
                "at",
                "This follow-up is finished; give it a new time (at or inMinutes) to use it again.",
            ));
        }
        let account = call.account_id();
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let next: Option<DateTime<Utc>> = if cancel {
            None
        } else if let Some(moved) = moved {
            Some(moved)
        } else {
            sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT next_due_at FROM agent_schedules WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?
        };
        sqlx::query(
            r#"
            UPDATE agent_schedules
            SET note = $2, every_minutes = $3, next_due_at = $4,
                state = CASE WHEN $5 THEN 'cancelled' ELSE 'active' END,
                version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&note)
        .bind(every)
        .bind(next)
        .bind(cancel)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if let (Some(moved), false) = (moved, cancel) {
            super::queue(tx, workspace, id, moved).await?;
        }
        super::changed(tx, workspace, account, id).await?;
        call.audit_target(
            "schedule",
            id,
            json!({ "cancelled": cancel, "moved": moved.is_some() }),
        );
        load(call, id, false).await?.ok_or(OpError::Unavailable)
    }
}
