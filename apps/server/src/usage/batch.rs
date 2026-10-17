//! U4: use it before it resets. A capacity alert carries a suggested batch:
//! the ready tasks across the owner's projects that can run on the alert's
//! account, ranked by W10's readiness ranking and sized to the capacity the
//! account has left. One tap launches it; Review opens it to edit first.
//!
//! A project can run on the account when the owner has a signed-in profile
//! for the account there, may launch runs there, and the project's default
//! environment runs on the account's harness and didn't fail its last
//! check. Each task launches in its own project's environment with that
//! project's defaults, but on the alert's account, through W7's launch
//! queue, which checks every launch again as the person who queued it when
//! it starts (principle 9).

use std::collections::{BTreeMap, HashSet};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{UsageAccount, WindowView, alerts::countdown, history, owned_account, windows_at};
use crate::{
    AppState,
    ai::ready::{self as ranking, RankFilter, RankScope},
    auth::{AuthenticatedActor, require_authenticated_write},
    environments::{self, EnvironmentRow},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::IdempotencyKey,
    launch::{self, LaunchCaller, LaunchOverrides, LaunchRequest, LaunchView, StartWhen},
    policy::{Action, ActorKind},
    work_item_planning::Reader,
};

/// The most tasks a batch suggests or launches.
pub const MAX_BATCH: usize = 20;
/// The most projects one batch looks at.
const MAX_PROJECTS: i64 = 50;
/// Ready tasks listed beyond the batch, for Review to add.
const MAX_MORE: usize = 30;
/// Ranked tasks read per project.
const PER_PROJECT: usize = 40;
/// Until an account has history, a run is taken to use this much of a
/// window.
const DEFAULT_PERCENT_PER_RUN: f64 = 10.0;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub key: String,
    pub title: String,
    pub priority: String,
    pub score: i64,
    pub unblocks: usize,
    /// Why it ranks where it does, in words.
    pub reasons: Vec<String>,
    pub environment_name: String,
    pub href: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedProject {
    pub project_id: Uuid,
    pub project_name: String,
    /// What to change so its tasks can join, in words.
    pub reason: String,
    pub href: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    pub slots: i64,
    pub busy: i64,
    /// The window the batch is sized for: the soonest to reset with
    /// capacity left, or the one the alert was about.
    pub window_key: Option<String>,
    pub window_label: Option<String>,
    pub unused_percent: Option<u16>,
    pub resets_at: Option<DateTime<Utc>>,
    /// How much of the window one run uses, from past cycles when known.
    pub percent_per_run: Option<f64>,
    pub size: usize,
    /// How the size was chosen, in words.
    pub summary: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub account_id: Uuid,
    pub account_name: String,
    pub harness: String,
    pub harness_name: String,
    pub capacity: Capacity,
    /// The suggested batch, most important first.
    pub tasks: Vec<BatchTask>,
    /// Other ready tasks, for Review to add.
    pub more: Vec<BatchTask>,
    /// Projects the account serves whose tasks can't join, and why.
    pub skipped: Vec<SkippedProject>,
}

/// A project whose tasks can run on the account.
struct Place {
    project_id: Uuid,
    project_name: String,
    environment: EnvironmentRow,
}

struct Places {
    runnable: Vec<Place>,
    skipped: Vec<SkippedProject>,
}

fn settings_href(project_id: Uuid) -> String {
    format!("/settings/projects/{project_id}/environments")
}

/// Where the account can run for `viewer`, and where it can't and why.
async fn places(
    state: &AppState,
    viewer: Uuid,
    account: &UsageAccount,
) -> Result<Places, AppError> {
    let projects = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT DISTINCT project.id, project.name
        FROM execution_profiles profile
        JOIN projects project ON project.id = profile.project_id
        WHERE profile.provider_account_id = $1 AND profile.owner_actor_id = $2
          AND profile.status <> 'disabled'
          AND project.archived_at IS NULL AND project.kind = 'standard'
          AND actor_can_access_project($2, NULL, project.id)
        ORDER BY project.name, project.id
        LIMIT $3
        "#,
    )
    .bind(account.id)
    .bind(viewer)
    .bind(MAX_PROJECTS)
    .fetch_all(&state.pool)
    .await?;
    let mut result = Places {
        runnable: Vec::new(),
        skipped: Vec::new(),
    };
    for (project_id, project_name) in projects {
        let skip = |reason: String| SkippedProject {
            project_id,
            project_name: project_name.clone(),
            reason,
            href: settings_href(project_id),
        };
        if environments::authorize_project(
            &state.pool,
            viewer,
            ActorKind::Human,
            project_id,
            Action::WorkerInvoke,
        )
        .await
        .is_err()
        {
            result
                .skipped
                .push(skip("You can't launch runs in this project.".to_owned()));
            continue;
        }
        if launch::launch_account(&state.pool, project_id, viewer, None, Some(account.id))
            .await?
            .is_none()
        {
            result.skipped.push(skip(
                "This account isn't signed in and switched on for this project.".to_owned(),
            ));
            continue;
        }
        let defaults = {
            let mut connection = state.pool.acquire().await?;
            launch::run_defaults(&mut connection, project_id, viewer).await?
        };
        let Some(environment) = defaults.environment else {
            result.skipped.push(skip(
                "It has no default environment. Choose one in its settings.".to_owned(),
            ));
            continue;
        };
        if environment.harness != account.adapter_kind {
            let name = state
                .supervisors
                .manifest(&environment.harness)
                .map_or_else(
                    || environment.harness.clone(),
                    |manifest| manifest.display_name.clone(),
                );
            result.skipped.push(skip(format!(
                "Its default environment, {}, runs on {name}.",
                environment.name
            )));
            continue;
        }
        if environment.last_check_ok == Some(false) {
            result.skipped.push(skip(format!(
                "Its default environment, {}, failed its last check.",
                environment.name
            )));
            continue;
        }
        result.runnable.push(Place {
            project_id,
            project_name,
            environment,
        });
    }
    Ok(result)
}

/// The window to size for: the one named, else the soonest to reset that
/// still has capacity and a known reset.
fn sizing_window<'a>(
    windows: &'a [WindowView],
    window_key: Option<&str>,
    now: DateTime<Utc>,
) -> Option<&'a WindowView> {
    if let Some(key) = window_key
        && let Some(window) = windows.iter().find(|window| window.key == key)
    {
        return Some(window);
    }
    windows
        .iter()
        .filter(|window| window.unused_percent.is_some() && !window.limit_reached)
        .filter(|window| window.resets_at.is_some_and(|at| at > now))
        .min_by_key(|window| window.resets_at)
}

/// How many tasks fit: as many as the account's slots can start before the
/// reset (about one run an hour each), and as many as the unused share of
/// the window covers at the account's usual use per run. Never more than
/// [`MAX_BATCH`].
fn size_for(
    slots: i64,
    busy: i64,
    window: Option<&WindowView>,
    percent_per_run: Option<f64>,
    now: DateTime<Utc>,
) -> (usize, String) {
    let slots = slots.max(1);
    let Some(window) = window else {
        let free = usize::try_from((slots - busy).max(1))
            .unwrap_or(1)
            .min(MAX_BATCH);
        return (
            free,
            format!(
                "Sized to {free} free run slot{}; this account doesn't report its usage.",
                plural(free)
            ),
        );
    };
    if window.limit_reached || window.unused_percent == Some(0) {
        return (
            0,
            format!(
                "The {} limit is reached, so nothing would start.",
                window.label.to_lowercase()
            ),
        );
    }
    let per_run = percent_per_run.unwrap_or(DEFAULT_PERCENT_PER_RUN).max(0.5);
    let unused = f64::from(window.unused_percent.unwrap_or(100));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let by_use = (unused / per_run).ceil().max(1.0) as usize;
    let hours = window
        .resets_at
        .map_or(1, |at| ((at - now).num_minutes() + 59) / 60)
        .max(1);
    let by_time = usize::try_from(slots.saturating_mul(hours)).unwrap_or(MAX_BATCH);
    let size = by_use.min(by_time).clamp(1, MAX_BATCH);
    let left = window
        .unused_percent
        .map_or_else(String::new, |unused| format!("{unused}% left of "));
    let when = window.resets_at.map_or_else(String::new, |at| {
        format!(", resetting in {}", countdown(now, at))
    });
    let usual = if percent_per_run.is_some() {
        format!("about {per_run:.0}% a run")
    } else {
        format!("about {per_run:.0}% a run until there's history")
    };
    (
        size,
        format!(
            "{left}the {} window{when}, at {usual}, on {slots} run slot{}.",
            window.label.to_lowercase(),
            plural(usize::try_from(slots).unwrap_or(1)),
        ),
    )
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The suggested batch for the account's owner.
pub(crate) async fn suggest(
    state: &AppState,
    viewer: Uuid,
    account: &UsageAccount,
    window_key: Option<&str>,
) -> Result<Batch, AppError> {
    let now = Utc::now();
    let places = places(state, viewer, account).await?;
    let project_ids = places
        .runnable
        .iter()
        .map(|place| place.project_id)
        .collect::<Vec<_>>();
    // Tasks already waiting in a launch queue or running aren't suggested again.
    let busy_tasks = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT work_item_id FROM launches
        WHERE project_id = ANY($1) AND state = 'queued' AND work_item_id IS NOT NULL
        "#,
    )
    .bind(&project_ids)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let mut ranked = Vec::new();
    for place in &places.runnable {
        let ranking = ranking::rank(
            &state.pool,
            Reader {
                actor_id: viewer,
                connection_id: None,
            },
            place.project_id,
            &RankFilter {
                scope: RankScope::Agents,
                limit: PER_PROJECT,
                ..RankFilter::default()
            },
        )
        .await?;
        ranked.extend(
            ranking
                .tasks
                .into_iter()
                .filter(|task| !busy_tasks.contains(&task.id))
                .map(|task| BatchTask {
                    id: task.id,
                    project_id: place.project_id,
                    project_name: place.project_name.clone(),
                    key: task.key,
                    title: task.title,
                    priority: task.priority,
                    score: task.score,
                    unblocks: task.unblocks,
                    reasons: task.reasons,
                    environment_name: place.environment.name.clone(),
                    href: task.href,
                }),
        );
    }
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.key.cmp(&b.key)));

    let windows = windows_at(&state.pool, account.id, now).await?;
    let window = sizing_window(&windows, window_key, now);
    let percent_per_run = match window {
        Some(window) => history::percent_per_run(&state.pool, account.id, &window.key).await?,
        None => None,
    };
    let busy = launch::account_load(&state.pool, account.id).await?;
    let slots = i64::from(account.max_concurrent_runs);
    let (size, summary) = size_for(slots, busy, window, percent_per_run, now);
    let more = ranked.split_off(size.min(ranked.len()));
    Ok(Batch {
        account_id: account.id,
        account_name: account.name.clone(),
        harness: account.adapter_kind.clone(),
        harness_name: state
            .supervisors
            .manifest(&account.adapter_kind)
            .map_or_else(
                || account.adapter_kind.clone(),
                |manifest| manifest.display_name.clone(),
            ),
        capacity: Capacity {
            slots,
            busy,
            window_key: window.map(|window| window.key.clone()),
            window_label: window.map(|window| window.label.clone()),
            unused_percent: window.and_then(|window| window.unused_percent),
            resets_at: window.and_then(|window| window.resets_at),
            percent_per_run,
            size,
            summary,
        },
        tasks: ranked,
        more: more.into_iter().take(MAX_MORE).collect(),
        skipped: places.skipped,
    })
}

// ---------------------------------------------------------------------------
// Launching

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchPick {
    pub project_id: Uuid,
    pub work_item_id: Uuid,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchLaunchRequest {
    /// The tasks to launch, as Review left them. Left out, the suggested
    /// batch as it stands now.
    #[serde(default)]
    pub tasks: Option<Vec<BatchPick>>,
    /// The window the alert was about, to size the suggestion for.
    #[serde(default)]
    pub window_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectLaunches {
    pub project_id: Uuid,
    pub project_name: String,
    pub environment_name: String,
    pub launches: Vec<LaunchView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFailure {
    pub project_id: Uuid,
    pub project_name: Option<String>,
    pub tasks: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchResult {
    pub launched: usize,
    pub projects: Vec<ProjectLaunches>,
    pub failed: Vec<ProjectFailure>,
}

/// Queue a launch for every task, grouped by project, each in its project's
/// default environment on this account. A project that can't take its
/// tasks is reported; the others still launch.
pub(crate) async fn launch_batch(
    state: &AppState,
    caller: &LaunchCaller,
    account: &UsageAccount,
    request: BatchLaunchRequest,
    idempotency_key: &IdempotencyKey,
) -> Result<BatchResult, AppError> {
    let empty = BatchResult {
        launched: 0,
        projects: Vec::new(),
        failed: Vec::new(),
    };
    let suggested = request.tasks.is_none();
    let picks = match request.tasks {
        Some(picks) => picks,
        None => suggest(
            state,
            caller.actor_id,
            account,
            request.window_key.as_deref(),
        )
        .await?
        .tasks
        .into_iter()
        .map(|task| BatchPick {
            project_id: task.project_id,
            work_item_id: task.id,
        })
        .collect::<Vec<_>>(),
    };
    // Nothing suggested (or a repeated tap after the batch was queued).
    if picks.is_empty() && suggested {
        return Ok(empty);
    }
    if picks.is_empty() || picks.len() > launch::MAX_TASKS_PER_LAUNCH {
        return Err(AppError::Validation("tasks"));
    }
    let places = places(state, caller.actor_id, account).await?;
    // Keep the order the batch was ranked in, project by project.
    let mut order = Vec::new();
    let mut by_project: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for pick in picks {
        let tasks = by_project.entry(pick.project_id).or_default();
        if tasks.is_empty() {
            order.push(pick.project_id);
        }
        if !tasks.contains(&pick.work_item_id) {
            tasks.push(pick.work_item_id);
        }
    }
    let mut result = empty;
    for project_id in order {
        let tasks = by_project.remove(&project_id).unwrap_or_default();
        let Some(place) = places
            .runnable
            .iter()
            .find(|place| place.project_id == project_id)
        else {
            let skipped = places
                .skipped
                .iter()
                .find(|skipped| skipped.project_id == project_id);
            result.failed.push(ProjectFailure {
                project_id,
                project_name: skipped.map(|skipped| skipped.project_name.clone()),
                tasks: tasks.len(),
                reason: skipped.map_or_else(
                    || "This account doesn't run tasks in that project.".to_owned(),
                    |skipped| skipped.reason.clone(),
                ),
            });
            continue;
        };
        let count = tasks.len();
        let launched = launch::launch_tasks(
            state,
            caller,
            project_id,
            LaunchRequest {
                work_item_ids: tasks,
                environment_id: Some(place.environment.id),
                overrides: LaunchOverrides::default(),
                provider_account_id: Some(account.id),
                prompt: None,
                start_when: StartWhen::Now,
                start_at: None,
                queue: true,
                dialog: false,
            },
            "queue",
            Ok(idempotency_key.clone()),
        )
        .await;
        match launched {
            Ok(launched) => {
                result.launched += launched.launches.len();
                result.projects.push(ProjectLaunches {
                    project_id,
                    project_name: place.project_name.clone(),
                    environment_name: place.environment.name.clone(),
                    launches: launched.launches,
                });
            }
            Err(error) if is_retryable(&error) => return Err(error),
            Err(error) => result.failed.push(ProjectFailure {
                project_id,
                project_name: Some(place.project_name.clone()),
                tasks: count,
                reason: launch::failure_reason(&error),
            }),
        }
    }
    Ok(result)
}

fn is_retryable(error: &AppError) -> bool {
    matches!(error, AppError::Database(_))
}

// ---------------------------------------------------------------------------
// HTTP

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SuggestQuery {
    window_key: Option<String>,
}

pub(super) async fn suggest_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SuggestQuery>,
) -> Result<Json<Batch>, AppError> {
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    Ok(Json(
        suggest(
            &state,
            principal.actor_id,
            &account,
            query.window_key.as_deref(),
        )
        .await?,
    ))
}

pub(super) async fn launch_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<BatchLaunchRequest>,
) -> Result<(StatusCode, Json<BatchResult>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let account = owned_account(&state.pool, principal.actor_id, account_id).await?;
    let caller = LaunchCaller::browser(&principal, &headers)?;
    let result = launch_batch(&state, &caller, &account, request, &key).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::{MAX_BATCH, size_for, sizing_window};
    use crate::usage::{WindowSource, WindowView};

    fn window(key: &str, unused: u16, resets_in: Duration, reached: bool) -> WindowView {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        WindowView {
            key: key.to_owned(),
            label: "5-hour".to_owned(),
            source: WindowSource::Reported,
            schedule_id: None,
            duration_minutes: Some(300),
            used_percent: Some(100 - unused),
            unused_percent: Some(unused),
            resets_at: Some(now + resets_in),
            limit_reached: reached,
            read_at: Some(now),
            rolled_over: false,
        }
    }

    #[test]
    fn the_batch_is_sized_by_capacity_left_and_time() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        // 40% left at 10% a run is 4 runs; 2 slots for 2 hours allow 4.
        let left = window("primary", 40, Duration::hours(2), false);
        assert_eq!(size_for(2, 0, Some(&left), None, now).0, 4);
        // One slot and one hour left caps it at 1.
        let soon = window("primary", 40, Duration::minutes(50), false);
        assert_eq!(size_for(1, 0, Some(&soon), None, now).0, 1);
        // History says a run uses 5%: 8 runs, but never past the cap.
        assert_eq!(size_for(8, 0, Some(&left), Some(5.0), now).0, 8);
        let lots = window("weekly", 90, Duration::days(2), false);
        assert_eq!(size_for(8, 0, Some(&lots), Some(1.0), now).0, MAX_BATCH);
        // A reached limit starts nothing.
        let full = window("primary", 0, Duration::hours(1), true);
        assert_eq!(size_for(2, 0, Some(&full), None, now).0, 0);
        // No reported usage: the free slots.
        assert_eq!(size_for(3, 1, None, None, now).0, 2);
    }

    #[test]
    fn the_soonest_window_with_capacity_is_the_one_sized_for() {
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let windows = vec![
            window("weekly", 60, Duration::days(3), false),
            window("primary", 30, Duration::hours(2), false),
            window("full", 0, Duration::minutes(10), true),
        ];
        assert_eq!(sizing_window(&windows, None, now).unwrap().key, "primary");
        assert_eq!(
            sizing_window(&windows, Some("weekly"), now).unwrap().key,
            "weekly"
        );
    }
}
