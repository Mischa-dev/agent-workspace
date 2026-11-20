//! Cycles (T1.24): a space's work in fixed-length periods.
//!
//! A space turns cycles on in its settings (length, the weekday they start
//! on, how many are made ahead, whether unfinished work rolls over). The
//! daily job (`run_due`) keeps the running cycle and that many after it
//! made, closes each cycle that has ended (with a last snapshot), moves its
//! unfinished tasks into the next one when roll-over is on, and counts the
//! running cycle's tasks once a day for its burn-up chart. Days are UTC
//! dates; a cycle is `[starts_on, ends_on)`.
//!
//! The job acts for whoever last changed the settings: roll-over is a task
//! update as them, refused (the tasks stay put) once they can no longer edit
//! tasks in the space.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    policy::{Action, Decision},
    work_item_commands::{
        self, AuditAttribution, AuditVersionMetadata, CommandActor, TaskFieldChanges,
        UpdateWorkItemCommand, UpdateWorkItemOutcome,
    },
};

/// The job that plans spaces' cycles.
pub const PLAN_KIND: &str = "tasks.cycles";
/// Spaces one run plans at most; the next run takes the rest.
const PLAN_BATCH: i64 = 50;
/// Ended cycles `list` shows, most recent first.
pub const ENDED_LISTED: i64 = 6;
/// Characters a cycle's name has at most (the CHECK).
pub const MAX_NAME_CHARACTERS: usize = 80;

/// What a refused cycle call says: the input field and what to do.
pub type Refusal = (&'static str, String);

// ---------------------------------------------------------------------------
// Settings

#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    /// Days in a cycle, from 7 to 56.
    pub length_days: i32,
    /// The weekday a cycle starts on: 1 is Monday, 7 is Sunday.
    pub starts_on_weekday: i16,
    /// Cycles kept made after the running one, from 1 to 6.
    pub upcoming: i32,
    /// Whether a cycle's unfinished tasks move into the next when it ends.
    pub roll_over: bool,
    /// 0 until the space's settings are first written.
    pub version: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            length_days: 14,
            starts_on_weekday: 1,
            upcoming: 2,
            roll_over: true,
            version: 0,
        }
    }
}

/// A change to a space's settings: what's left out stays.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Days in a cycle, from 7 to 56. Cycles already made keep theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_days: Option<i32>,
    /// The weekday cycles start on: 1 is Monday, 7 is Sunday. Takes effect
    /// for the first cycle; later ones follow on from the one before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_on_weekday: Option<i16>,
    /// How many cycles after the running one are kept made, from 1 to 6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upcoming: Option<i32>,
    /// Whether a cycle's unfinished tasks move into the next when it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll_over: Option<bool>,
}

impl SettingsChange {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.length_days.is_none()
            && self.starts_on_weekday.is_none()
            && self.upcoming.is_none()
            && self.roll_over.is_none()
    }
}

const SETTINGS_COLUMNS: &str =
    "enabled, length_days, starts_on_weekday, upcoming, roll_over, version";

/// A space's cycle settings; the defaults (off) when it has none.
pub async fn settings(connection: &mut PgConnection, space_id: Uuid) -> Result<Settings, AppError> {
    Ok(sqlx::query_as::<_, Settings>(&format!(
        "SELECT {SETTINGS_COLUMNS} FROM cycle_settings WHERE space_id = $1"
    ))
    .bind(space_id)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or_default())
}

/// Writes a space's settings as `actor`, who the job acts for from now on.
/// An expected version that isn't the current one is refused. Turning
/// cycles on makes them at once.
pub async fn write_settings(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    expected_version: Option<i64>,
    change: &SettingsChange,
    today: NaiveDate,
) -> Result<Result<Settings, Refusal>, AppError> {
    let current = sqlx::query_as::<_, Settings>(&format!(
        "SELECT {SETTINGS_COLUMNS} FROM cycle_settings WHERE space_id = $1 FOR UPDATE"
    ))
    .bind(actor.project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_default();
    if let Some(expected) = expected_version
        && expected != current.version
    {
        return Ok(Err((
            "expectedVersion",
            format!(
                "The cycle settings changed since you read them (now version {}). Read them again with cycle_list and retry.",
                current.version
            ),
        )));
    }
    if change
        .length_days
        .is_some_and(|days| !(7..=56).contains(&days))
    {
        return Ok(Err((
            "lengthDays",
            "A cycle is 7 to 56 days long.".to_owned(),
        )));
    }
    if change
        .starts_on_weekday
        .is_some_and(|day| !(1..=7).contains(&day))
    {
        return Ok(Err((
            "startsOnWeekday",
            "Pass a weekday from 1 (Monday) to 7 (Sunday).".to_owned(),
        )));
    }
    if change
        .upcoming
        .is_some_and(|count| !(1..=6).contains(&count))
    {
        return Ok(Err((
            "upcoming",
            "Keep 1 to 6 cycles made after the running one.".to_owned(),
        )));
    }
    let written = sqlx::query_as::<_, Settings>(&format!(
        r#"
        INSERT INTO cycle_settings (space_id, workspace_id, enabled, length_days,
            starts_on_weekday, upcoming, roll_over, updated_by_actor_id)
        SELECT project.id, project.organization_id, $2, $3, $4, $5, $6, $7
        FROM projects project WHERE project.id = $1
        ON CONFLICT (space_id) DO UPDATE
        SET enabled = EXCLUDED.enabled, length_days = EXCLUDED.length_days,
            starts_on_weekday = EXCLUDED.starts_on_weekday, upcoming = EXCLUDED.upcoming,
            roll_over = EXCLUDED.roll_over, updated_by_actor_id = EXCLUDED.updated_by_actor_id,
            version = cycle_settings.version + 1, updated_at = now(),
            planned_on = CASE WHEN cycle_settings.enabled THEN cycle_settings.planned_on END
        RETURNING {SETTINGS_COLUMNS}
        "#
    ))
    .bind(actor.project_id)
    .bind(change.enabled.unwrap_or(current.enabled))
    .bind(change.length_days.unwrap_or(current.length_days))
    .bind(
        change
            .starts_on_weekday
            .unwrap_or(current.starts_on_weekday),
    )
    .bind(change.upcoming.unwrap_or(current.upcoming))
    .bind(change.roll_over.unwrap_or(current.roll_over))
    .bind(actor.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if written.enabled {
        plan(transaction, actor.project_id, today, Some(actor)).await?;
    }
    Ok(Ok(written))
}

// ---------------------------------------------------------------------------
// Cycles

#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Cycle {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub number: i32,
    /// Its own name; empty when it has none.
    pub name: String,
    /// What to call it: its name, else "Cycle N".
    #[sqlx(skip)]
    pub title: String,
    /// Its first day.
    #[schemars(with = "String")]
    pub starts_on: NaiveDate,
    /// The day after its last.
    #[schemars(with = "String")]
    pub ends_on: NaiveDate,
    /// upcoming, running or ended.
    #[sqlx(skip)]
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub completed_at: Option<DateTime<Utc>>,
    pub version: i64,
    /// Its tasks now, leaving out canceled ones.
    pub scope: i64,
    pub started: i64,
    pub completed: i64,
    /// One count a day while it ran, oldest first: its burn-up.
    #[sqlx(skip)]
    pub snapshots: Vec<Snapshot>,
}

#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(skip)]
    pub cycle_id: Uuid,
    #[schemars(with = "String")]
    pub on_date: NaiveDate,
    pub scope: i32,
    pub started: i32,
    pub completed: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_points: Option<f64>,
}

/// What a cycle is called: its name, else "Cycle N".
pub fn title(number: i32, name: &str) -> String {
    if name.is_empty() {
        format!("Cycle {number}")
    } else {
        name.to_owned()
    }
}

/// The space's cycles that haven't ended and its last few that have, in
/// order, with their task counts and daily snapshots.
pub async fn list(
    connection: &mut PgConnection,
    space_id: Uuid,
    today: NaiveDate,
) -> Result<Vec<Cycle>, AppError> {
    fetch(connection, space_id, today, None).await
}

/// `list`, or the one cycle named.
async fn fetch(
    connection: &mut PgConnection,
    space_id: Uuid,
    today: NaiveDate,
    only: Option<Uuid>,
) -> Result<Vec<Cycle>, AppError> {
    let mut cycles = sqlx::query_as::<_, Cycle>(
        r#"
        SELECT cycle.id, cycle.number, cycle.name, cycle.starts_on, cycle.ends_on,
               cycle.completed_at, cycle.version,
               count(task.id) FILTER (WHERE status.semantic_category <> 'canceled') AS scope,
               count(task.id) FILTER (WHERE status.semantic_category = 'started') AS started,
               count(task.id) FILTER (WHERE status.semantic_category = 'completed') AS completed
        FROM cycles cycle
        LEFT JOIN task_rows task
          ON task.project_id = cycle.space_id AND task.cycle_id = cycle.id
         AND task.archived_at IS NULL
        LEFT JOIN work_item_statuses status
          ON status.project_id = task.project_id AND status.id = task.status_id
        WHERE cycle.space_id = $1
          AND CASE WHEN $3::uuid IS NOT NULL THEN cycle.id = $3
              ELSE cycle.completed_at IS NULL
               OR cycle.id IN (SELECT ended.id FROM cycles ended
                               WHERE ended.space_id = $1 AND ended.completed_at IS NOT NULL
                               ORDER BY ended.number DESC LIMIT $2)
              END
        GROUP BY cycle.id
        ORDER BY cycle.number
        "#,
    )
    .bind(space_id)
    .bind(ENDED_LISTED)
    .bind(only)
    .fetch_all(&mut *connection)
    .await?;
    let ids = cycles.iter().map(|cycle| cycle.id).collect::<Vec<_>>();
    let snapshots = sqlx::query_as::<_, Snapshot>(
        "SELECT cycle_id, on_date, scope, started, completed, scope_points, completed_points \
         FROM cycle_snapshots WHERE cycle_id = ANY($1) ORDER BY cycle_id, on_date",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?;
    for cycle in &mut cycles {
        cycle.title = title(cycle.number, &cycle.name);
        cycle.state = if cycle.completed_at.is_some() || cycle.ends_on <= today {
            "ended"
        } else if cycle.starts_on <= today {
            "running"
        } else {
            "upcoming"
        }
        .to_owned();
        cycle.snapshots = snapshots
            .iter()
            .filter(|snapshot| snapshot.cycle_id == cycle.id)
            .cloned()
            .collect();
    }
    Ok(cycles)
}

/// Whether a task can be put in this cycle: one of the space's that hasn't
/// been closed.
pub async fn is_open(
    connection: &mut PgConnection,
    space_id: Uuid,
    cycle_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM cycles WHERE space_id = $1 AND id = $2 AND completed_at IS NULL)",
    )
    .bind(space_id)
    .bind(cycle_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// The cycle a caller names: "none" (`Some(None)`), "current" (the running
/// one), "next", its number, its ID or its name. None when there's no such
/// open cycle.
pub async fn reference(
    connection: &mut PgConnection,
    space_id: Uuid,
    value: &str,
    today: NaiveDate,
) -> Result<Option<Option<Uuid>>, AppError> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(Some(None));
    }
    let lower = value.to_lowercase();
    let number = lower
        .strip_prefix("cycle ")
        .unwrap_or(&lower)
        .parse::<i32>()
        .ok();
    let found = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM cycles
        WHERE space_id = $1 AND completed_at IS NULL
          AND CASE $2
                WHEN 'current' THEN starts_on <= $4 AND ends_on > $4
                WHEN 'next' THEN starts_on > $4
                ELSE id::text = $2 OR number = $3 OR lower(name) = $2
              END
        ORDER BY id::text = $2 DESC, COALESCE(number = $3, false) DESC, number
        LIMIT 1
        "#,
    )
    .bind(space_id)
    .bind(&lower)
    .bind(number)
    .bind(today)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(found.map(Some))
}

/// One of the space's cycles, ended or not, by number ("3" or "Cycle 3"),
/// ID or name.
pub async fn reference_any(
    connection: &mut PgConnection,
    space_id: Uuid,
    value: &str,
) -> Result<Option<Option<Uuid>>, AppError> {
    let lower = value.trim().to_lowercase();
    let number = lower
        .strip_prefix("cycle ")
        .unwrap_or(&lower)
        .parse::<i32>()
        .ok();
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM cycles WHERE space_id = $1 \
         AND (id::text = $2 OR number = $3 OR lower(name) = $2) \
         ORDER BY id::text = $2 DESC, COALESCE(number = $3, false) DESC, number DESC LIMIT 1",
    )
    .bind(space_id)
    .bind(&lower)
    .bind(number)
    .fetch_optional(&mut *connection)
    .await?
    .map(Some))
}

/// Renames one of the space's cycles; an empty name goes back to "Cycle N".
pub async fn rename(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    cycle_id: Uuid,
    name: &str,
) -> Result<Result<Cycle, Refusal>, AppError> {
    let name = name.trim();
    if name.chars().count() > MAX_NAME_CHARACTERS || name.chars().any(char::is_control) {
        return Ok(Err((
            "name",
            format!("A cycle's name is one line of at most {MAX_NAME_CHARACTERS} characters."),
        )));
    }
    let renamed = sqlx::query_scalar::<_, Uuid>(
        "UPDATE cycles SET name = $3, version = version + 1 \
         WHERE space_id = $1 AND id = $2 RETURNING id",
    )
    .bind(space_id)
    .bind(cycle_id)
    .bind(name)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(id) = renamed else {
        return Ok(Err((
            "cycle",
            "This space has no such cycle (cycle_list lists them).".to_owned(),
        )));
    };
    fetch(transaction, space_id, Utc::now().date_naive(), Some(id))
        .await?
        .pop()
        .map(Ok)
        .ok_or(AppError::Internal("a renamed cycle is gone"))
}

// ---------------------------------------------------------------------------
// Planning

/// What one planning pass did, for its caller and tests.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Planned {
    pub made: usize,
    pub ended: usize,
    pub rolled_over: usize,
}

/// The first day of the cycle holding `day`, for cycles starting on ISO
/// weekday `weekday`.
fn aligned(day: NaiveDate, weekday: i16) -> NaiveDate {
    let today = i64::from(day.weekday().number_from_monday());
    let back = (today - i64::from(weekday)).rem_euclid(7);
    day - Duration::days(back)
}

/// Brings a space's cycles up to `today`: closes the ended ones (rolling
/// their unfinished tasks into the running one when roll-over is on and
/// `actor` may), makes the running one and the upcoming ones, and counts
/// the running one's tasks for today. Does nothing while cycles are off.
pub async fn plan(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    today: NaiveDate,
    actor: Option<&CommandActor>,
) -> Result<Planned, AppError> {
    let mut planned = Planned::default();
    let Some(settings) = sqlx::query_as::<_, Settings>(&format!(
        "SELECT {SETTINGS_COLUMNS} FROM cycle_settings WHERE space_id = $1 AND enabled FOR UPDATE"
    ))
    .bind(space_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(planned);
    };

    // Close what has ended, with a last count on its last day.
    let ended = sqlx::query_as::<_, (Uuid, i32, NaiveDate)>(
        "SELECT id, number, ends_on FROM cycles \
         WHERE space_id = $1 AND completed_at IS NULL AND ends_on <= $2 \
         ORDER BY number FOR UPDATE",
    )
    .bind(space_id)
    .bind(today)
    .fetch_all(&mut **transaction)
    .await?;
    for (cycle_id, number, ends_on) in &ended {
        snapshot(transaction, *cycle_id, *ends_on - Duration::days(1)).await?;
        sqlx::query("UPDATE cycles SET completed_at = now(), version = version + 1 WHERE id = $1")
            .bind(cycle_id)
            .execute(&mut **transaction)
            .await?;
        cycle_event(
            transaction,
            &crate::events::catalog::CYCLE_ENDED,
            space_id,
            *cycle_id,
            *number,
        )
        .await?;
    }
    planned.ended = ended.len();

    // Make the running cycle and the upcoming ones, keeping the cadence:
    // after a gap (cycles were off), whole periods are skipped.
    let length = Duration::days(i64::from(settings.length_days));
    let last = sqlx::query_as::<_, (i32, NaiveDate)>(
        "SELECT number, ends_on FROM cycles WHERE space_id = $1 ORDER BY number DESC LIMIT 1",
    )
    .bind(space_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let (mut number, mut starts_on) = match last {
        None => (1, aligned(today, settings.starts_on_weekday)),
        Some((number, ends_on)) => {
            let mut starts_on = ends_on;
            if starts_on + length <= today {
                let periods = (today - starts_on).num_days() / length.num_days();
                starts_on += Duration::days(periods * length.num_days());
            }
            (number + 1, starts_on)
        }
    };
    let mut ahead = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM cycles WHERE space_id = $1 AND ends_on > $2",
    )
    .bind(space_id)
    .bind(today)
    .fetch_one(&mut **transaction)
    .await?;
    while ahead < 1 + i64::from(settings.upcoming) {
        sqlx::query(
            "INSERT INTO cycles (id, workspace_id, space_id, number, starts_on, ends_on) \
             SELECT $1, project.organization_id, project.id, $3, $4, $5 \
             FROM projects project WHERE project.id = $2",
        )
        .bind(Uuid::now_v7())
        .bind(space_id)
        .bind(number)
        .bind(starts_on)
        .bind(starts_on + length)
        .execute(&mut **transaction)
        .await?;
        planned.made += 1;
        number += 1;
        starts_on += length;
        ahead += 1;
    }

    let running = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT id, number FROM cycles WHERE space_id = $1 AND completed_at IS NULL \
         AND starts_on <= $2 AND ends_on > $2 ORDER BY number LIMIT 1",
    )
    .bind(space_id)
    .bind(today)
    .fetch_optional(&mut **transaction)
    .await?;

    // Roll unfinished work into the running cycle.
    if settings.roll_over
        && !ended.is_empty()
        && let (Some(actor), Some((running_id, _))) = (actor, running)
    {
        let ended_ids = ended.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
        planned.rolled_over = roll_over(transaction, actor, &ended_ids, running_id).await?;
    }

    if let Some((running_id, running_number)) = running {
        let first = snapshot(transaction, running_id, today).await?;
        if first {
            cycle_event(
                transaction,
                &crate::events::catalog::CYCLE_STARTED,
                space_id,
                running_id,
                running_number,
            )
            .await?;
        }
    }
    sqlx::query("UPDATE cycle_settings SET planned_on = $2 WHERE space_id = $1")
        .bind(space_id)
        .bind(today)
        .execute(&mut **transaction)
        .await?;
    Ok(planned)
}

/// Counts a cycle's tasks for a day, replacing that day's count. True when
/// it is the cycle's first.
async fn snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    cycle_id: Uuid,
    on_date: NaiveDate,
) -> Result<bool, AppError> {
    let first = !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM cycle_snapshots WHERE cycle_id = $1)",
    )
    .bind(cycle_id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO cycle_snapshots (cycle_id, workspace_id, on_date, scope, started,
            completed, scope_points, completed_points)
        SELECT cycle.id, cycle.workspace_id, $2,
               count(task.id) FILTER (WHERE status.semantic_category <> 'canceled')::int,
               count(task.id) FILTER (WHERE status.semantic_category = 'started')::int,
               count(task.id) FILTER (WHERE status.semantic_category = 'completed')::int,
               sum(task.estimate) FILTER (WHERE status.semantic_category <> 'canceled'),
               sum(task.estimate) FILTER (WHERE status.semantic_category = 'completed')
        FROM cycles cycle
        LEFT JOIN task_rows task
          ON task.project_id = cycle.space_id AND task.cycle_id = cycle.id
         AND task.archived_at IS NULL
        LEFT JOIN work_item_statuses status
          ON status.project_id = task.project_id AND status.id = task.status_id
        WHERE cycle.id = $1
        GROUP BY cycle.id, cycle.workspace_id
        ON CONFLICT (cycle_id, on_date) DO UPDATE
        SET scope = EXCLUDED.scope, started = EXCLUDED.started,
            completed = EXCLUDED.completed, scope_points = EXCLUDED.scope_points,
            completed_points = EXCLUDED.completed_points
        "#,
    )
    .bind(cycle_id)
    .bind(on_date)
    .execute(&mut **transaction)
    .await?;
    Ok(first)
}

/// Tells the space a cycle started or ended, naming it only.
async fn cycle_event(
    transaction: &mut Transaction<'_, Postgres>,
    ty: &'static crate::events::EventType,
    space_id: Uuid,
    cycle_id: Uuid,
    number: i32,
) -> Result<(), AppError> {
    let mut event = crate::events::NewEvent::thing(ty, space_id, "space", space_id)
        .payload(json!({ "cycleId": cycle_id, "number": number }));
    // A cycle isn't a thing of its own: the event is about the space.
    event.thing = None;
    crate::events::emit(transaction, event).await?;
    Ok(())
}

/// Moves the unfinished tasks of ended cycles into `into`, as `actor`. A
/// task that changed meanwhile is moved at its new version; one that can't
/// be moved stays.
async fn roll_over(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    ended: &[Uuid],
    into: Uuid,
) -> Result<usize, AppError> {
    let unfinished = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT task.id FROM task_rows task
        JOIN work_item_statuses status
          ON status.project_id = task.project_id AND status.id = task.status_id
        WHERE task.project_id = $1 AND task.cycle_id = ANY($2) AND task.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
        ORDER BY task.id
        "#,
    )
    .bind(actor.project_id)
    .bind(ended)
    .fetch_all(&mut **transaction)
    .await?;
    let mut moved = 0;
    for task_id in unfinished {
        let Some(current) =
            work_item_commands::lock_work_item(transaction, actor.project_id, task_id).await?
        else {
            continue;
        };
        let expected_version = current.version;
        let outcome = work_item_commands::update_locked_work_item(
            transaction,
            actor,
            current,
            UpdateWorkItemCommand {
                expected_version,
                title: None,
                description: None,
                type_id: None,
                status_id: None,
                priority: None,
                assignee_actor_id: None,
                parent_work_item_id: None,
                board_placement: None,
                labels: None,
                fields: TaskFieldChanges {
                    cycle_id: Some(Some(into)),
                    ..TaskFieldChanges::default()
                },
                field_values: None,
                summary: "Rolled over into the next cycle",
                audit_action: "work_item.cycle_rolled_over",
                audit_version_metadata: AuditVersionMetadata::Include,
            },
        )
        .await?;
        if matches!(outcome, UpdateWorkItemOutcome::Applied { .. }) {
            moved += 1;
        }
    }
    Ok(moved)
}

// ---------------------------------------------------------------------------
// The job

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(PLAN_KIND, |context: crate::jobs::JobContext| async move {
        run_due(&context.state.pool, Utc::now().date_naive(), context.job.id).await?;
        Ok(())
    });
    registry.every(PLAN_KIND, std::time::Duration::from_secs(60 * 60));
}

/// Plans each space with cycles on that hasn't been planned `today`, each
/// in its own transaction. Returns how many it planned.
pub async fn run_due(
    pool: &sqlx::PgPool,
    today: NaiveDate,
    job_id: Uuid,
) -> Result<usize, AppError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT space_id, updated_by_actor_id FROM cycle_settings \
         WHERE enabled AND (planned_on IS NULL OR planned_on < $1) \
         ORDER BY planned_on NULLS FIRST, space_id LIMIT $2",
    )
    .bind(today)
    .bind(PLAN_BATCH)
    .fetch_all(crate::db::scoped(pool))
    .await?;
    for (space_id, actor_id) in &due {
        let mut transaction = crate::db::scoped_tx(pool).await?;
        let actor = acting_actor(&mut transaction, *space_id, *actor_id, job_id).await?;
        plan(&mut transaction, *space_id, today, actor.as_ref()).await?;
        transaction.commit().await?;
    }
    Ok(due.len())
}

/// Who the job acts as for a space: whoever last wrote its settings, while
/// they can still edit its tasks.
async fn acting_actor(
    transaction: &mut Transaction<'_, Postgres>,
    space_id: Uuid,
    actor_id: Uuid,
    job_id: Uuid,
) -> Result<Option<CommandActor>, AppError> {
    let Some(membership) =
        work_item_commands::lock_membership(transaction, actor_id, space_id).await?
    else {
        return Ok(None);
    };
    if membership.project_archived_at.is_some() {
        return Ok(None);
    }
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
        .bind(actor_id)
        .fetch_one(&mut **transaction)
        .await?;
    let actor = CommandActor {
        actor_id,
        actor_kind: crate::conversations::parse_actor_kind(&kind)?,
        project_id: space_id,
        audit: AuditAttribution::Job {
            job_id,
            source: "cycle",
        },
    };
    Ok(
        match work_item_commands::authorize_mutation(
            &actor,
            &membership,
            Action::WorkItemEdit,
            None,
        )? {
            Decision::Allow => Some(actor),
            Decision::Deny(_) => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_start_on_their_weekday() {
        // 2026-11-18 is a Wednesday.
        let wednesday = NaiveDate::from_ymd_opt(2026, 11, 18).expect("date");
        assert_eq!(
            aligned(wednesday, 1),
            NaiveDate::from_ymd_opt(2026, 11, 16).expect("date")
        );
        assert_eq!(aligned(wednesday, 3), wednesday);
        assert_eq!(
            aligned(wednesday, 4),
            NaiveDate::from_ymd_opt(2026, 11, 12).expect("date")
        );
        assert_eq!(
            aligned(wednesday, 7),
            NaiveDate::from_ymd_opt(2026, 11, 15).expect("date")
        );
    }

    #[test]
    fn a_cycle_without_a_name_is_numbered() {
        assert_eq!(title(3, ""), "Cycle 3");
        assert_eq!(title(3, "Launch"), "Launch");
    }
}
