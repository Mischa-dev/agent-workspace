//! AI2 and AI4: finding dependencies, blockers and duplicates, and triaging
//! new tasks.
//!
//! A run picks candidate pairs with cheap signals (one task names the
//! other's key, the titles read alike, they share a milestone and a label),
//! leaves out pairs already linked or already suggested, and asks the model
//! about them in batches. It also reads recent messages that say someone is
//! waiting or blocked, and triages tasks created since the last run.
//!
//! What a run may read depends on who is behind it:
//! - Background runs (after edits, and nightly) have no person behind them,
//!   so they read only conversations every project member can see
//!   (`conversation_visible_to(conversation, NULL)`) and only this project's
//!   tasks.
//! - A person's Analyze reads what that person can see. A suggestion drawn
//!   from something not every member can see (a private thread, a task in
//!   another project) is shown to that person alone.
//!
//! Whatever a run queues is rechecked against the reader on every read
//! (see [`super::suggestions`]).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::{
    gateway::{self, AiError, ResolvedModel, Scope, Tier},
    prompts,
    providers::fake,
    suggestions::{self, Kind, NewSuggestion, Provenance, SourceRef},
};
use crate::{
    AppState,
    error::AppError,
    jobs::{self, Enqueued, JobContext, JobError, JobRegistry, NewJob},
    policy::ActorKind,
    settings::{self, SettingsContext},
    work_item_commands::{AuditAttribution, CommandActor},
    work_item_planning::Reader,
};

pub const DETECT_KIND: &str = "ai.detect";
pub const SWEEP_KIND: &str = "ai.sweep";
pub const NIGHTLY_KIND: &str = "ai.nightly";

/// Pairs one model call judges.
const PAIRS_PER_CALL: usize = 30;
/// Tasks one triage call looks at.
const TRIAGE_PER_CALL: usize = 20;
/// The most open tasks a run loads.
const MAX_TASKS: i64 = 3000;
/// The most messages a run reads.
const MAX_MESSAGES: i64 = 80;
/// Phrases that say someone is waiting on something (case-insensitive,
/// PostgreSQL regular expression).
const WAITING_PATTERN: &str =
    r"waiting (on|for)|blocked (by|on)|depends on|can.?t start until|need[s]? .{0,40} first";

/// What started a run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// After edits: the tasks changed since the last run.
    Edit,
    /// A person asked: every open task, with what that person can see.
    Analyze,
    /// Once a day: every open task, with what every member can see.
    Nightly,
    /// Triage named tasks now (AI4).
    Triage,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Analyze => "analyze",
            Self::Nightly => "nightly",
            Self::Triage => "triage",
        }
    }

    fn origin(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Analyze => "analyze",
            Self::Nightly => "nightly",
            Self::Triage => "create",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectPayload {
    project_id: Uuid,
    run_id: Uuid,
    mode: Mode,
    actor_id: Option<Uuid>,
    #[serde(default)]
    task_ids: Vec<Uuid>,
}

pub fn register_jobs(registry: &mut JobRegistry) {
    registry.register(
        DETECT_KIND,
        |context| async move { detect_job(context).await },
    );
    registry.register(SWEEP_KIND, |context| async move {
        sweep(&context.state).await?;
        Ok(())
    });
    registry.every(SWEEP_KIND, Duration::from_secs(2 * 60));
    registry.register(NIGHTLY_KIND, |context| async move {
        nightly(&context.state).await?;
        Ok(())
    });
    registry.every(NIGHTLY_KIND, Duration::from_secs(60 * 60));
}

// ---------------------------------------------------------------------------
// Requesting runs

/// A run, as the settings and review pages show it.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: Uuid,
    pub kind: String,
    pub state: String,
    pub tasks_considered: i32,
    pub suggestions_created: i32,
    pub suggestions_applied: i32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether the person reading asked for it.
    pub mine: bool,
}

/// Queues a run, or returns the one already queued or running for the same
/// project, kind and person.
pub async fn request(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
    mode: Mode,
    actor_id: Option<Uuid>,
    task_ids: Vec<Uuid>,
) -> Result<Uuid, AppError> {
    let mut transaction = pool.begin().await?;
    if mode != Mode::Triage
        && let Some(existing) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM ai_runs
            WHERE project_id = $1 AND kind = $2
              AND requested_by_actor_id IS NOT DISTINCT FROM $3
              AND state IN ('queued', 'running')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(mode.as_str())
        .bind(actor_id)
        .fetch_optional(&mut *transaction)
        .await?
    {
        return Ok(existing);
    }
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO ai_runs (id, organization_id, project_id, kind, requested_by_actor_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(run_id)
    .bind(organization_id)
    .bind(project_id)
    .bind(mode.as_str())
    .bind(actor_id)
    .execute(&mut *transaction)
    .await?;
    let unique = match (mode, actor_id) {
        (Mode::Triage, _) => format!("{DETECT_KIND}:triage:{run_id}"),
        (_, Some(actor)) => format!("{DETECT_KIND}:{project_id}:{}:{actor}", mode.as_str()),
        (_, None) => format!("{DETECT_KIND}:{project_id}:{}", mode.as_str()),
    };
    let enqueued = jobs::enqueue(
        &mut *transaction,
        NewJob::new(DETECT_KIND)
            .project(organization_id, project_id)
            .payload(DetectPayload {
                project_id,
                run_id,
                mode,
                actor_id,
                task_ids,
            })
            .unique(unique)
            .max_attempts(3)
            .priority(if actor_id.is_some() { 10 } else { 0 }),
    )
    .await?;
    if enqueued == Enqueued::Duplicate {
        // A job without a run row: the earlier run is still going.
        transaction.rollback().await?;
        return Ok(sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id FROM ai_runs
            WHERE project_id = $1 AND kind = $2 AND state IN ('queued', 'running')
            ORDER BY created_at DESC LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(mode.as_str())
        .fetch_optional(pool)
        .await?
        .unwrap_or(run_id));
    }
    transaction.commit().await?;
    Ok(run_id)
}

/// The project's recent runs.
pub async fn recent_runs(
    pool: &PgPool,
    project_id: Uuid,
    reader: Uuid,
    limit: i64,
) -> Result<Vec<RunView>, AppError> {
    Ok(sqlx::query_as::<_, RunView>(
        r#"
        SELECT id, kind, state, tasks_considered, suggestions_created, suggestions_applied,
               error, created_at, finished_at,
               requested_by_actor_id IS NOT DISTINCT FROM $2 AS mine
        FROM ai_runs
        WHERE project_id = $1
          -- A person's run is listed for them; others see background runs.
          AND (requested_by_actor_id IS NULL OR requested_by_actor_id = $2)
        ORDER BY created_at DESC
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(reader)
    .bind(limit.clamp(1, 50))
    .fetch_all(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// The background schedule

/// Whether background detection is on for a project: a provider is chosen
/// and `ai.background_detection` is on.
async fn background_enabled(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
) -> Result<bool, AppError> {
    let mut connection = pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    Ok(effective.string("ai.provider").is_some()
        && effective.bool("ai.background_detection").unwrap_or(false))
}

/// Queues an edit run for every project with tasks changed since its last
/// run.
pub async fn sweep(state: &AppState) -> Result<usize, AppError> {
    let projects = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT project.id, project.organization_id
        FROM projects project
        LEFT JOIN ai_project_scans scan ON scan.project_id = project.id
        WHERE project.kind = 'standard' AND project.archived_at IS NULL
          AND EXISTS (
              SELECT 1 FROM work_items item
              WHERE item.project_id = project.id AND item.archived_at IS NULL
                AND item.updated_at > COALESCE(scan.last_scan_at, '-infinity'::timestamptz)
          )
        ORDER BY scan.last_scan_at NULLS FIRST
        LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    let mut queued = 0;
    for (project_id, organization_id) in projects {
        if background_enabled(&state.pool, organization_id, project_id).await? {
            request(
                &state.pool,
                organization_id,
                project_id,
                Mode::Edit,
                None,
                Vec::new(),
            )
            .await?;
            queued += 1;
        }
    }
    Ok(queued)
}

/// Queues the nightly run for every project that hasn't had one for a day.
pub async fn nightly(state: &AppState) -> Result<usize, AppError> {
    let projects = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT project.id, project.organization_id
        FROM projects project
        LEFT JOIN ai_project_scans scan ON scan.project_id = project.id
        WHERE project.kind = 'standard' AND project.archived_at IS NULL
          AND COALESCE(scan.last_nightly_at, '-infinity'::timestamptz) < now() - interval '20 hours'
          AND EXISTS (
              SELECT 1 FROM work_items item
              WHERE item.project_id = project.id AND item.archived_at IS NULL
          )
        ORDER BY scan.last_nightly_at NULLS FIRST
        LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    let mut queued = 0;
    for (project_id, organization_id) in projects {
        if background_enabled(&state.pool, organization_id, project_id).await? {
            request(
                &state.pool,
                organization_id,
                project_id,
                Mode::Nightly,
                None,
                Vec::new(),
            )
            .await?;
            queued += 1;
        }
    }
    Ok(queued)
}

// ---------------------------------------------------------------------------
// One run

async fn detect_job(context: JobContext) -> Result<(), JobError> {
    let payload: DetectPayload = context.job.payload_as()?;
    let last_attempt = context.job.attempts >= context.job.max_attempts;
    match run(&context.state, &payload).await {
        Ok(()) => Ok(()),
        Err(error) if error.retryable() && !last_attempt => Err(JobError::retry(error.message())),
        Err(error) => {
            finish_failed(&context.state.pool, payload.run_id, &error.message()).await?;
            Ok(())
        }
    }
}

async fn finish_failed(pool: &PgPool, run_id: Uuid, message: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE ai_runs SET state = 'failed', error = left($2, 500), finished_at = now()
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct ProjectRow {
    organization_id: Uuid,
    project_key: String,
    standard: bool,
    archived: bool,
    last_scan_at: Option<DateTime<Utc>>,
}

/// A task as a run sees it.
#[derive(Clone, Debug, FromRow)]
struct TaskRow {
    id: Uuid,
    project_id: Uuid,
    key: String,
    title: String,
    description: String,
    status_name: String,
    finished: bool,
    priority: String,
    type_id: Uuid,
    type_name: String,
    parent_work_item_id: Option<Uuid>,
    milestone_id: Option<Uuid>,
    milestone_name: Option<String>,
    blocked: bool,
    label_ids: Vec<Uuid>,
    label_names: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const TASK_COLUMNS: &str = r#"
    item.id, item.project_id, project.project_key || '-' || item.number AS key,
    item.title, left(item.description, 1500) AS description,
    status.name AS status_name,
    status.semantic_category IN ('completed', 'canceled') AS finished,
    item.priority, item.type_id, kind.name AS type_name, item.parent_work_item_id,
    item.milestone_id, milestone.name AS milestone_name,
    item.blocked_reason IS NOT NULL AS blocked,
    COALESCE(array(
        SELECT label.id FROM work_item_labels link
        JOIN project_labels label ON label.project_id = link.project_id AND label.id = link.label_id
        WHERE link.project_id = item.project_id AND link.work_item_id = item.id
          AND label.archived_at IS NULL
        ORDER BY label.name
    ), '{}') AS label_ids,
    COALESCE(array(
        SELECT label.name FROM work_item_labels link
        JOIN project_labels label ON label.project_id = link.project_id AND label.id = link.label_id
        WHERE link.project_id = item.project_id AND link.work_item_id = item.id
          AND label.archived_at IS NULL
        ORDER BY label.name
    ), '{}') AS label_names,
    item.created_at, item.updated_at
"#;

const TASK_FROM: &str = r#"
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    JOIN work_item_types kind ON kind.project_id = item.project_id AND kind.id = item.type_id
    LEFT JOIN project_milestones milestone
      ON milestone.project_id = item.project_id AND milestone.id = item.milestone_id
"#;

/// What one run found, before it's queued.
struct Found {
    suggestions: Vec<NewSuggestion>,
    tasks_considered: usize,
}

/// Runs detection for one queued run. Errors that say what's wrong for the
/// person (not configured, over budget) end the run as failed.
async fn run(state: &AppState, payload: &DetectPayload) -> Result<(), AiError> {
    let started = Utc::now();
    sqlx::query(
        "UPDATE ai_runs SET state = 'running', started_at = COALESCE(started_at, now()) WHERE id = $1",
    )
    .bind(payload.run_id)
    .execute(&state.pool)
    .await?;
    let Some(project) = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project.organization_id, project.project_key,
               project.kind = 'standard' AS standard,
               project.archived_at IS NOT NULL AS archived,
               scan.last_scan_at
        FROM projects project
        LEFT JOIN ai_project_scans scan ON scan.project_id = project.id
        WHERE project.id = $1
        "#,
    )
    .bind(payload.project_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(AiError::NotConfigured(
            "The project no longer exists.".to_owned(),
        ));
    };
    if !project.standard || project.archived {
        return Err(AiError::NotConfigured(
            "AI planning runs only in open projects.".to_owned(),
        ));
    }
    // A person's run stops if they lost the project (principle 9).
    if let Some(actor_id) = payload.actor_id
        && !actor_can_access(&state.pool, actor_id, payload.project_id).await?
    {
        return Err(AiError::NotConfigured(
            "The person who asked can no longer see this project.".to_owned(),
        ));
    }
    let scope = Scope {
        organization_id: project.organization_id,
        project_id: Some(payload.project_id),
        actor_id: payload.actor_id,
    };
    let resolved = gateway::resolve(state, scope, Tier::Quick).await?;
    let context = RunContext {
        state,
        scope,
        resolved: &resolved,
        payload,
        project: &project,
    };
    let found = context.find().await?;
    let mut created = Vec::new();
    for suggestion in &found.suggestions {
        if let Some(id) = suggestions::insert(&state.pool, suggestion).await? {
            created.push((id, suggestion.confidence, suggestion.audience));
        }
    }
    let applied = auto_apply(state, &project, payload.project_id, &created).await?;
    sqlx::query(
        r#"
        UPDATE ai_runs
        SET state = 'succeeded', finished_at = now(), error = NULL,
            tasks_considered = $2, suggestions_created = $3, suggestions_applied = $4
        WHERE id = $1
        "#,
    )
    .bind(payload.run_id)
    .bind(i32::try_from(found.tasks_considered).unwrap_or(i32::MAX))
    .bind(i32::try_from(created.len()).unwrap_or(i32::MAX))
    .bind(i32::try_from(applied).unwrap_or(i32::MAX))
    .execute(&state.pool)
    .await?;
    if let Some(actor_id) = payload.actor_id
        && !created.is_empty()
    {
        super::notify(
            &state.pool,
            payload.project_id,
            actor_id,
            "ai_suggestions",
            payload.run_id,
            format!(
                "AI found {} suggestion{} to review",
                created.len(),
                if created.len() == 1 { "" } else { "s" }
            ),
            format!("/projects/{}/suggestions", payload.project_id),
        )
        .await?;
    }
    // Background runs move the project's mark; a person's run doesn't, as
    // it read with that person's eyes.
    if payload.actor_id.is_none() && payload.mode != Mode::Triage {
        sqlx::query(
            r#"
            INSERT INTO ai_project_scans (project_id, organization_id, last_scan_at, last_nightly_at)
            VALUES ($1, $2, $3, CASE WHEN $4 THEN $3 END)
            ON CONFLICT (project_id) DO UPDATE
            SET last_scan_at = GREATEST(ai_project_scans.last_scan_at, EXCLUDED.last_scan_at),
                last_nightly_at = COALESCE(EXCLUDED.last_nightly_at, ai_project_scans.last_nightly_at),
                updated_at = now()
            "#,
        )
        .bind(payload.project_id)
        .bind(project.organization_id)
        .bind(started)
        .bind(payload.mode == Mode::Nightly)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

async fn actor_can_access(
    pool: &PgPool,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
            .bind(actor_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?,
    )
}

struct RunContext<'a> {
    state: &'a AppState,
    scope: Scope,
    resolved: &'a ResolvedModel,
    payload: &'a DetectPayload,
    project: &'a ProjectRow,
}

/// One candidate pair and why it was picked.
#[derive(Clone, Debug)]
struct Candidate {
    a: usize,
    b: usize,
    signals: Vec<&'static str>,
}

#[derive(Deserialize)]
struct Verdicts {
    verdicts: Vec<Verdict>,
}

#[derive(Deserialize)]
struct Verdict {
    pair: usize,
    kind: String,
    confidence: f32,
    reason: String,
}

#[derive(Deserialize)]
struct Findings {
    findings: Vec<Finding>,
}

#[derive(Deserialize)]
struct Finding {
    message: usize,
    task: String,
    kind: String,
    blocker: Option<String>,
    reason: String,
    confidence: f32,
}

#[derive(Deserialize)]
struct Triaged {
    tasks: Vec<TriagedTask>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TriagedTask {
    task: String,
    labels: Vec<String>,
    priority: Option<String>,
    #[serde(rename = "type")]
    task_type: Option<String>,
    parent: Option<String>,
    duplicate_of: Option<String>,
    confidence: f32,
    reason: String,
}

#[derive(FromRow)]
struct MessageRow {
    id: Uuid,
    conversation_id: Uuid,
    conversation: String,
    author: String,
    text: String,
    linked_task: Option<Uuid>,
    /// Every project member can see it.
    shared: bool,
}

impl RunContext<'_> {
    fn provenance(&self, prompt: &'static prompts::Prompt, model: &str) -> Provenance {
        Provenance {
            provider: self.resolved.manifest.id.to_owned(),
            model: model.to_owned(),
            prompt_id: prompt.id,
            prompt_version: prompt.version,
        }
    }

    fn suggestion(
        &self,
        task: &TaskRow,
        kind: Kind,
        other: Option<&TaskRow>,
        value: Value,
        confidence: f32,
        reason: &str,
        sources: Vec<SourceRef>,
        provenance: &Provenance,
    ) -> NewSuggestion {
        // Something only one person could see is for that person alone.
        let cross_project = other.is_some_and(|other| other.project_id != task.project_id);
        NewSuggestion {
            organization_id: self.project.organization_id,
            project_id: self.payload.project_id,
            work_item_id: task.id,
            kind,
            other: other.map(|other| (other.project_id, other.id)),
            value,
            confidence,
            reason: reason.to_owned(),
            sources,
            audience: if cross_project {
                self.payload.actor_id
            } else {
                None
            },
            origin: self.payload.mode.origin(),
            provenance: provenance.clone(),
            requested_by: self.payload.actor_id,
        }
    }

    async fn find(&self) -> Result<Found, AiError> {
        let pool = &self.state.pool;
        let tasks = sqlx::query_as::<_, TaskRow>(&format!(
            r#"
            SELECT {TASK_COLUMNS}
            {TASK_FROM}
            WHERE item.project_id = $1 AND item.archived_at IS NULL
            ORDER BY (status.semantic_category IN ('completed', 'canceled')), item.updated_at DESC
            LIMIT $2
            "#
        ))
        .bind(self.payload.project_id)
        .bind(MAX_TASKS)
        .fetch_all(pool)
        .await?;
        let since = match self.payload.mode {
            Mode::Edit => Some(
                self.project
                    .last_scan_at
                    .unwrap_or_else(|| Utc::now() - chrono::Duration::days(1)),
            ),
            _ => None,
        };
        let mut found = Found {
            suggestions: Vec::new(),
            tasks_considered: 0,
        };
        if self.payload.mode == Mode::Triage {
            let wanted = self.payload.task_ids.iter().collect::<HashSet<_>>();
            let targets = tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| wanted.contains(&task.id))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            found.tasks_considered = targets.len();
            self.triage(&tasks, &targets, &mut found).await?;
            return Ok(found);
        }
        let focus = tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| !task.finished && since.is_none_or(|since| task.updated_at > since))
            .map(|(index, _)| index)
            .take(if since.is_some() { 150 } else { 400 })
            .collect::<Vec<_>>();
        found.tasks_considered = focus.len();
        let mut extra = Vec::new();
        if !focus.is_empty() {
            let mut candidates = self.candidates(&tasks, &focus).await?;
            if self.payload.mode == Mode::Analyze {
                let (more_tasks, more) = self.cross_project(&tasks, &focus).await?;
                extra = more_tasks;
                candidates.extend(more);
            }
            let all = tasks.iter().chain(extra.iter()).collect::<Vec<_>>();
            self.judge(&all, &candidates, &mut found).await?;
        }
        self.conversations(&tasks, since, &mut found).await?;
        if let Some(since) = since {
            let new_tasks = tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| !task.finished && task.created_at > since)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            self.triage(&tasks, &new_tasks, &mut found).await?;
        }
        Ok(found)
    }

    /// Pairs already linked or already suggested (in any state but stale).
    async fn known_pairs(&self) -> Result<HashSet<(Uuid, Uuid)>, AppError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT blocking_work_item_id, blocked_work_item_id FROM work_item_dependencies
            WHERE project_id = $1 OR blocking_project_id = $1
            UNION ALL
            SELECT work_item_id, other_work_item_id FROM work_item_relations
            WHERE project_id = $1 OR other_project_id = $1
            UNION ALL
            SELECT id, parent_work_item_id FROM work_items
            WHERE project_id = $1 AND parent_work_item_id IS NOT NULL
            UNION ALL
            SELECT work_item_id, other_work_item_id FROM ai_suggestions
            WHERE project_id = $1 AND other_work_item_id IS NOT NULL AND status <> 'stale'
            "#,
        )
        .bind(self.payload.project_id)
        .fetch_all(&self.state.pool)
        .await?;
        Ok(rows.into_iter().map(|(a, b)| ordered(a, b)).collect())
    }

    async fn candidates(
        &self,
        tasks: &[TaskRow],
        focus: &[usize],
    ) -> Result<Vec<Candidate>, AppError> {
        let known = self.known_pairs().await?;
        let in_focus = focus.iter().copied().collect::<HashSet<_>>();
        let by_key = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (task.key.as_str(), index))
            .collect::<HashMap<_, _>>();
        let mut pairs: BTreeMap<(usize, usize), Vec<&'static str>> = BTreeMap::new();
        let mut add = |a: usize, b: usize, signal: &'static str| {
            if a == b || known.contains(&ordered(tasks[a].id, tasks[b].id)) {
                return;
            }
            let signals = pairs.entry((a.min(b), a.max(b))).or_default();
            if !signals.contains(&signal) {
                signals.push(signal);
            }
        };
        // One task names the other's key.
        for (index, task) in tasks.iter().enumerate() {
            for key in task_keys(&format!("{} {}", task.title, task.description)) {
                if let Some(&other) = by_key.get(key.as_str())
                    && (in_focus.contains(&index) || in_focus.contains(&other))
                {
                    add(other, index, "references");
                }
            }
        }
        // Titles that read alike, and shared milestone and labels.
        let words = tasks
            .iter()
            .map(|task| fake::words(&task.title))
            .collect::<Vec<_>>();
        for &index in focus {
            let mut alike = Vec::new();
            let mut related = Vec::new();
            for (other, task) in tasks.iter().enumerate() {
                if other == index {
                    continue;
                }
                let likeness = jaccard(&words[index], &words[other]);
                if likeness >= 0.4 {
                    alike.push((likeness, other));
                }
                let shared_labels = tasks[index]
                    .label_ids
                    .iter()
                    .filter(|label| task.label_ids.contains(label))
                    .count();
                if !task.finished
                    && shared_labels > 0
                    && tasks[index].milestone_id.is_some()
                    && tasks[index].milestone_id == task.milestone_id
                {
                    related.push((shared_labels, other));
                }
            }
            alike.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, other) in alike.into_iter().take(3) {
                add(index, other, "similar");
            }
            related.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, other) in related.into_iter().take(3) {
                add(index, other, "milestone");
            }
        }
        let limit = if self.payload.mode == Mode::Edit {
            90
        } else {
            300
        };
        let mut candidates = pairs
            .into_iter()
            .map(|((a, b), signals)| Candidate { a, b, signals })
            .collect::<Vec<_>>();
        // Strongest signals first.
        candidates.sort_by_key(|candidate| {
            std::cmp::Reverse(
                candidate.signals.len() * 10
                    + usize::from(candidate.signals.contains(&"references")) * 5,
            )
        });
        candidates.truncate(limit);
        Ok(candidates)
    }

    /// For a person's Analyze: tasks in the workspace's other projects named
    /// by key in this project's tasks, which that person can see.
    async fn cross_project(
        &self,
        tasks: &[TaskRow],
        focus: &[usize],
    ) -> Result<(Vec<TaskRow>, Vec<Candidate>), AppError> {
        let Some(actor_id) = self.payload.actor_id else {
            return Ok((Vec::new(), Vec::new()));
        };
        let mut wanted: Vec<(usize, String)> = Vec::new();
        for &index in focus {
            let task = &tasks[index];
            for key in task_keys(&format!("{} {}", task.title, task.description)) {
                if !key.starts_with(&format!("{}-", self.project.project_key)) {
                    wanted.push((index, key));
                }
            }
        }
        wanted.truncate(100);
        if wanted.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let keys = wanted
            .iter()
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>();
        let others = sqlx::query_as::<_, TaskRow>(&format!(
            r#"
            SELECT {TASK_COLUMNS}
            {TASK_FROM}
            WHERE project.organization_id = $1 AND project.id <> $2
              AND project.kind = 'standard'
              AND item.archived_at IS NULL
              AND project.project_key || '-' || item.number = ANY($3)
              AND actor_can_access_project($4, NULL, project.id)
            "#
        ))
        .bind(self.project.organization_id)
        .bind(self.payload.project_id)
        .bind(&keys)
        .bind(actor_id)
        .fetch_all(&self.state.pool)
        .await?;
        let known = self.known_pairs().await?;
        let offset = tasks.len();
        let mut candidates = Vec::new();
        for (index, key) in &wanted {
            if let Some(position) = others.iter().position(|other| &other.key == key)
                && !known.contains(&ordered(tasks[*index].id, others[position].id))
            {
                candidates.push(Candidate {
                    a: offset + position,
                    b: *index,
                    signals: vec!["references"],
                });
            }
        }
        Ok((others, candidates))
    }

    async fn judge(
        &self,
        tasks: &[&TaskRow],
        candidates: &[Candidate],
        found: &mut Found,
    ) -> Result<(), AiError> {
        for batch in candidates.chunks(PAIRS_PER_CALL) {
            let mut included = Vec::new();
            for candidate in batch {
                for index in [candidate.a, candidate.b] {
                    if !included.contains(&index) {
                        included.push(index);
                    }
                }
            }
            let input = json!({
                "tasks": included.iter().map(|&index| task_input(tasks[index])).collect::<Vec<_>>(),
                "pairs": batch.iter().enumerate().map(|(pair, candidate)| json!({
                    "pair": pair,
                    "a": tasks[candidate.a].key,
                    "b": tasks[candidate.b].key,
                    "signals": candidate.signals,
                })).collect::<Vec<_>>(),
            });
            let answer = gateway::run_with::<Verdicts>(
                self.state,
                self.scope,
                &prompts::DEPENDENCY_JUDGE,
                &input,
                Tier::Quick,
                self.resolved,
            )
            .await?;
            let provenance = self.provenance(&prompts::DEPENDENCY_JUDGE, &answer.model);
            for verdict in answer.value.verdicts {
                let Some(candidate) = batch.get(verdict.pair) else {
                    continue;
                };
                let (a, b) = (tasks[candidate.a], tasks[candidate.b]);
                // The suggestion lives on a task of this project.
                let (task, kind, other) = match verdict.kind.as_str() {
                    "a_blocks_b" => (b, Kind::Blocks, a),
                    "b_blocks_a" => (a, Kind::Blocks, b),
                    "relates_to" => (a, Kind::RelatesTo, b),
                    "duplicates" => {
                        // The newer task duplicates the older one.
                        if a.created_at >= b.created_at {
                            (a, Kind::Duplicates, b)
                        } else {
                            (b, Kind::Duplicates, a)
                        }
                    }
                    "a_parent_of_b" => (b, Kind::Parent, a),
                    "b_parent_of_a" => (a, Kind::Parent, b),
                    _ => continue,
                };
                let (task, other) = if task.project_id == self.payload.project_id {
                    (task, other)
                } else if matches!(kind, Kind::RelatesTo) {
                    (other, task)
                } else {
                    // A link stored on another project's task isn't this
                    // project's to suggest.
                    continue;
                };
                if kind == Kind::Parent && other.project_id != task.project_id {
                    continue;
                }
                let mut sources = vec![
                    SourceRef::Task { id: task.id },
                    SourceRef::Task { id: other.id },
                ];
                sources.extend(candidate.signals.iter().map(|name| SourceRef::Signal {
                    name: (*name).to_owned(),
                }));
                found.suggestions.push(self.suggestion(
                    task,
                    kind,
                    Some(other),
                    json!({}),
                    verdict.confidence,
                    &verdict.reason,
                    sources,
                    &provenance,
                ));
            }
        }
        Ok(())
    }

    /// Messages that say someone is waiting or blocked.
    async fn conversations(
        &self,
        tasks: &[TaskRow],
        since: Option<DateTime<Utc>>,
        found: &mut Found,
    ) -> Result<(), AiError> {
        let since = since.unwrap_or_else(|| Utc::now() - chrono::Duration::days(14));
        // With no person behind the run, only conversations every member can
        // see (viewer NULL). A person's run reads what they can see.
        let messages = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT message.id, message.conversation_id, conversation.name AS conversation,
                   COALESCE(membership.display_name, 'Someone') AS author,
                   left(message.plain_text, 1200) AS text,
                   (
                       SELECT link.work_item_id FROM work_item_links link
                       JOIN work_items item ON item.id = link.work_item_id
                       WHERE link.project_id = $1
                         AND item.archived_at IS NULL
                         AND ((link.target_kind = 'conversation' AND link.target_id = conversation.id)
                           OR (link.target_kind = 'message' AND link.target_id = message.id))
                       ORDER BY link.created_at
                       LIMIT 1
                   ) AS linked_task,
                   conversation_visible_to(conversation.id, NULL) AS shared
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            LEFT JOIN project_memberships membership
              ON membership.project_id = conversation.project_id
             AND membership.actor_id = message.author_actor_id
            WHERE conversation.project_id = $1
              AND conversation.archived_at IS NULL
              AND message.removed_at IS NULL
              AND message.created_at > $2
              AND message.plain_text ~* $3
              AND conversation_visible_to(conversation.id, $4)
            ORDER BY message.created_at DESC
            LIMIT $5
            "#,
        )
        .bind(self.payload.project_id)
        .bind(since)
        .bind(WAITING_PATTERN)
        .bind(self.payload.actor_id)
        .bind(MAX_MESSAGES)
        .fetch_all(&self.state.pool)
        .await?;
        if messages.is_empty() {
            return Ok(());
        }
        let by_id = tasks
            .iter()
            .map(|task| (task.id, task))
            .collect::<HashMap<_, _>>();
        let by_key = tasks
            .iter()
            .map(|task| (task.key.as_str(), task))
            .collect::<HashMap<_, _>>();
        // The tasks the messages name or are linked to, and the open ones
        // most recently changed, so the model can match "the login API".
        let mut named = HashSet::new();
        for message in &messages {
            for key in task_keys(&message.text) {
                if let Some(task) = by_key.get(key.as_str()) {
                    named.insert(task.id);
                }
            }
            if let Some(task) = message.linked_task {
                named.insert(task);
            }
        }
        let task_list = tasks
            .iter()
            .filter(|task| named.contains(&task.id))
            .chain(
                tasks
                    .iter()
                    .filter(|task| !task.finished && !named.contains(&task.id))
                    .take(80),
            )
            .map(|task| json!({"key": task.key, "title": task.title, "status": task.status_name}))
            .collect::<Vec<_>>();
        let input = json!({
            "messages": messages.iter().enumerate().map(|(index, message)| json!({
                "index": index,
                "conversation": message.conversation,
                "author": message.author,
                "text": message.text,
                "linkedTask": message.linked_task.and_then(|id| by_id.get(&id)).map(|task| task.key.clone()),
            })).collect::<Vec<_>>(),
            "tasks": task_list,
        });
        let answer = gateway::run_with::<Findings>(
            self.state,
            self.scope,
            &prompts::CONVERSATION_BLOCKERS,
            &input,
            Tier::Quick,
            self.resolved,
        )
        .await?;
        let provenance = self.provenance(&prompts::CONVERSATION_BLOCKERS, &answer.model);
        for finding in answer.value.findings {
            let (Some(message), Some(task)) = (
                messages.get(finding.message),
                by_key.get(finding.task.as_str()),
            ) else {
                continue;
            };
            if task.finished {
                continue;
            }
            let message_source = SourceRef::Message {
                message_id: message.id,
                conversation_id: message.conversation_id,
            };
            let audience = if message.shared {
                None
            } else {
                self.payload.actor_id
            };
            if !message.shared && audience.is_none() {
                // Never from a conversation not every member can see,
                // unless a person asked and it's for them alone.
                continue;
            }
            let mut suggestion = match (finding.kind.as_str(), finding.blocker.as_deref()) {
                ("blocks", Some(blocker)) => {
                    let Some(blocker) = by_key.get(blocker) else {
                        continue;
                    };
                    if blocker.id == task.id
                        || self
                            .known_pairs()
                            .await?
                            .contains(&ordered(task.id, blocker.id))
                    {
                        continue;
                    }
                    self.suggestion(
                        task,
                        Kind::Blocks,
                        Some(blocker),
                        json!({}),
                        finding.confidence,
                        &finding.reason,
                        vec![
                            message_source,
                            SourceRef::Task { id: task.id },
                            SourceRef::Task { id: blocker.id },
                        ],
                        &provenance,
                    )
                }
                ("blocked_reason", _) => {
                    if task.blocked {
                        continue;
                    }
                    self.suggestion(
                        task,
                        Kind::BlockedReason,
                        None,
                        json!({"reason": suggestions::clean_reason(&finding.reason)}),
                        finding.confidence,
                        &finding.reason,
                        vec![message_source, SourceRef::Task { id: task.id }],
                        &provenance,
                    )
                }
                _ => continue,
            };
            suggestion.audience = suggestion.audience.or(audience);
            suggestion.origin = "conversation";
            found.suggestions.push(suggestion);
        }
        Ok(())
    }

    /// AI4: labels, priority, type, parent and duplicates for new tasks.
    async fn triage(
        &self,
        tasks: &[TaskRow],
        targets: &[usize],
        found: &mut Found,
    ) -> Result<(), AiError> {
        if targets.is_empty() {
            return Ok(());
        }
        let labels = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM project_labels WHERE project_id = $1 AND archived_at IS NULL ORDER BY name LIMIT 200",
        )
        .bind(self.payload.project_id)
        .fetch_all(&self.state.pool)
        .await?;
        let types = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM work_item_types WHERE project_id = $1 AND enabled ORDER BY position",
        )
        .bind(self.payload.project_id)
        .fetch_all(&self.state.pool)
        .await?;
        let by_key = tasks
            .iter()
            .map(|task| (task.key.as_str(), task))
            .collect::<HashMap<_, _>>();
        let words = tasks
            .iter()
            .map(|task| fake::words(&task.title))
            .collect::<Vec<_>>();
        for batch in targets.chunks(TRIAGE_PER_CALL) {
            let mut similar = Vec::new();
            for &index in batch {
                let mut alike = tasks
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != index)
                    .map(|(other, _)| (jaccard(&words[index], &words[other]), other))
                    .filter(|(likeness, _)| *likeness >= 0.3)
                    .collect::<Vec<_>>();
                alike.sort_by(|a, b| b.0.total_cmp(&a.0));
                for (_, other) in alike.into_iter().take(4) {
                    similar.push(json!({
                        "key": tasks[other].key,
                        "title": tasks[other].title,
                        "forTask": tasks[index].key,
                    }));
                }
            }
            let input = json!({
                "newTasks": batch.iter().map(|&index| {
                    let task = &tasks[index];
                    json!({
                        "key": task.key,
                        "title": task.title,
                        "description": task.description,
                        "type": task.type_name,
                        "priority": task.priority,
                    })
                }).collect::<Vec<_>>(),
                "labels": labels.iter().map(|(_, name)| name).collect::<Vec<_>>(),
                "types": types.iter().map(|(_, name)| name).collect::<Vec<_>>(),
                "priorities": ["no_priority", "low", "medium", "high", "urgent"],
                "similar": similar,
            });
            let answer = gateway::run_with::<Triaged>(
                self.state,
                self.scope,
                &prompts::TRIAGE,
                &input,
                Tier::Quick,
                self.resolved,
            )
            .await?;
            let provenance = self.provenance(&prompts::TRIAGE, &answer.model);
            for triaged in answer.value.tasks {
                let Some(task) = by_key.get(triaged.task.as_str()).copied() else {
                    continue;
                };
                if !batch.iter().any(|&index| tasks[index].id == task.id) {
                    continue;
                }
                let own = vec![SourceRef::Task { id: task.id }];
                for name in &triaged.labels {
                    if let Some((label_id, label_name)) = labels
                        .iter()
                        .find(|(_, label)| label.eq_ignore_ascii_case(name))
                        && !task.label_ids.contains(label_id)
                    {
                        found.suggestions.push(self.suggestion(
                            task,
                            Kind::Label,
                            None,
                            json!({"labelId": label_id, "name": label_name}),
                            triaged.confidence,
                            &triaged.reason,
                            own.clone(),
                            &provenance,
                        ));
                    }
                }
                if let Some(priority) = triaged.priority.as_deref()
                    && task.priority == "no_priority"
                    && priority != "no_priority"
                    && crate::work_item_commands::validate_priority(priority).is_ok()
                {
                    found.suggestions.push(self.suggestion(
                        task,
                        Kind::Priority,
                        None,
                        json!({"priority": priority}),
                        triaged.confidence,
                        &triaged.reason,
                        own.clone(),
                        &provenance,
                    ));
                }
                if let Some(name) = triaged.task_type.as_deref()
                    && let Some((type_id, type_name)) = types
                        .iter()
                        .find(|(_, kind)| kind.eq_ignore_ascii_case(name))
                    && *type_id != task.type_id
                {
                    found.suggestions.push(self.suggestion(
                        task,
                        Kind::Type,
                        None,
                        json!({"typeId": type_id, "name": type_name}),
                        triaged.confidence,
                        &triaged.reason,
                        own.clone(),
                        &provenance,
                    ));
                }
                for (key, kind) in [
                    (triaged.parent.as_deref(), Kind::Parent),
                    (triaged.duplicate_of.as_deref(), Kind::Duplicates),
                ] {
                    let Some(other) = key.and_then(|key| by_key.get(key)).copied() else {
                        continue;
                    };
                    if other.id == task.id
                        || (kind == Kind::Parent && task.parent_work_item_id.is_some())
                    {
                        continue;
                    }
                    found.suggestions.push(self.suggestion(
                        task,
                        kind,
                        Some(other),
                        json!({}),
                        triaged.confidence,
                        &triaged.reason,
                        vec![
                            SourceRef::Task { id: task.id },
                            SourceRef::Task { id: other.id },
                        ],
                        &provenance,
                    ));
                }
            }
        }
        Ok(())
    }
}

fn task_input(task: &TaskRow) -> Value {
    json!({
        "key": task.key,
        "title": task.title,
        "description": task.description,
        "status": task.status_name,
        "milestone": task.milestone_name,
        "labels": task.label_names,
    })
}

fn ordered(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b { (a, b) } else { (b, a) }
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.intersection(b).count();
    shared as f64 / (a.len() + b.len() - shared) as f64
}

/// Task keys (`ACME-12`) in a text: a capital letter, one to nine capitals
/// or digits, a dash and a number, standing alone.
pub fn task_keys(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let starts_word = index == 0 || !word_byte(bytes[index - 1]);
        if starts_word && bytes[index].is_ascii_uppercase() {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_uppercase() || bytes[end].is_ascii_digit())
            {
                end += 1;
            }
            let prefix_len = end - index;
            if (2..=10).contains(&prefix_len) && end < bytes.len() && bytes[end] == b'-' {
                let mut digits_end = end + 1;
                while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
                    digits_end += 1;
                }
                let digits = digits_end - end - 1;
                let ends_word = digits_end == bytes.len() || !word_byte(bytes[digits_end]);
                if (1..=12).contains(&digits) && ends_word {
                    let key = &text[index..digits_end];
                    if !keys.iter().any(|known: &String| known == key) {
                        keys.push(key.to_owned());
                    }
                    index = digits_end;
                    continue;
                }
            }
            index = end;
            continue;
        }
        index += 1;
    }
    keys
}

/// Letters, digits and any non-ASCII byte count as part of a word.
fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

// ---------------------------------------------------------------------------
// Applying on its own

/// The person a background change acts as: whoever turned on
/// `ai.auto_apply_suggestions` for the project or workspace.
async fn auto_apply_actor(
    pool: &PgPool,
    organization_id: Uuid,
    project_id: Uuid,
) -> Result<Option<(Uuid, f32)>, AppError> {
    let mut connection = pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: None,
        },
        &BTreeMap::new(),
    )
    .await?;
    if !effective.bool("ai.auto_apply_suggestions").unwrap_or(false) {
        return Ok(None);
    }
    let Some(resolved) = effective.get("ai.auto_apply_suggestions") else {
        return Ok(None);
    };
    let Some(actor) = resolved
        .layers
        .iter()
        .find(|layer| settings::Source::from(layer.scope) == resolved.source)
        .and_then(|layer| layer.updated_by_actor_id)
    else {
        return Ok(None);
    };
    let threshold = effective
        .value::<i64>("ai.auto_apply_min_confidence")
        .unwrap_or(90)
        .clamp(50, 100);
    Ok(Some((actor, threshold as f32 / 100.0)))
}

/// The command actor for a change the server makes for `actor_id`, or none
/// when that actor no longer exists or is disabled.
pub async fn background_actor(
    pool: &PgPool,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<CommandActor>, AppError> {
    let Some(kind) = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM actors WHERE id = $1 AND status = 'active'",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let actor_kind: ActorKind = crate::conversations::parse_actor_kind(&kind)?;
    Ok(Some(CommandActor {
        actor_id,
        actor_kind,
        project_id,
        audit: AuditAttribution::Mcp {
            authentication_class: "ai_background",
            request_id: Uuid::now_v7().to_string(),
            client_name: "Agent Workspace AI".to_owned(),
            mcp_credential_id: None,
            oauth_grant_id: None,
            external_agent_connection_id: None,
        },
    }))
}

/// Applies new suggestions at or over the confidence the project set, as
/// the person who turned the switch on, rechecking that they still belong
/// and can see every source (principle 9). Suggestions for one person only
/// are never applied on their own. Returns how many were applied.
async fn auto_apply(
    state: &AppState,
    project: &ProjectRow,
    project_id: Uuid,
    created: &[(Uuid, f32, Option<Uuid>)],
) -> Result<usize, AppError> {
    if created.is_empty() {
        return Ok(0);
    }
    let Some((actor_id, threshold)) =
        auto_apply_actor(&state.pool, project.organization_id, project_id).await?
    else {
        return Ok(0);
    };
    let Some(actor) = background_actor(&state.pool, actor_id, project_id).await? else {
        return Ok(0);
    };
    let reader = Reader {
        actor_id,
        connection_id: None,
    };
    let mut applied = 0;
    for &(suggestion_id, confidence, audience) in created {
        if audience.is_some() || confidence < threshold {
            continue;
        }
        let mut transaction = state.pool.begin().await?;
        match suggestions::accept(&mut transaction, &actor, reader, suggestion_id, "applied")
            .await?
        {
            Ok(()) => {
                transaction.commit().await?;
                applied += 1;
            }
            // Left for people to decide: the actor lost access, a source
            // went out of their sight, or the change was refused.
            Err(_) => transaction.rollback().await?,
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_keys_stand_alone() {
        assert_eq!(
            task_keys("After ACME-12 and (WEB-3), not xACME-4 or ACME-5x; ACME-12 again. A-1 AB-"),
            vec!["ACME-12".to_owned(), "WEB-3".to_owned()]
        );
        assert_eq!(
            task_keys("W10-7 é ÉACME-1 R2D2-99"),
            vec!["W10-7", "R2D2-99"]
        );
    }

    #[test]
    fn likeness_is_shared_words_over_all_words() {
        let a = fake::words("Login page for the app");
        let b = fake::words("Login page");
        assert!(jaccard(&a, &b) > 0.3);
        assert_eq!(jaccard(&a, &HashSet::new()), 0.0);
    }
}
