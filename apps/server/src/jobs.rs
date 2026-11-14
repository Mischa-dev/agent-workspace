//! F9: one durable job queue in PostgreSQL for background work.
//!
//! Enqueue a job with [`enqueue`] (inside the transaction that decides the
//! work is needed, so the job exists exactly when that change commits).
//! Register what runs it in a [`JobRegistry`]. [`run_workers`] claims due
//! jobs with `FOR UPDATE SKIP LOCKED`, takes a lease, commits, and only then
//! runs the handler, so HTTP calls and other slow work never hold a row
//! lock. A lease that runs out (the process died) makes the job claimable
//! again; a finished attempt only counts if it still holds its lease token.
//!
//! Delivery is at least once. A job is leased to one worker at a time, but
//! a handler can crash after its outside call and before its result is
//! recorded, and then it runs again. So every handler with an outside effect
//! (a push, a launch, a webhook) records what it did in a row keyed by the
//! cause (the notification, the launch, the delivery) and checks that row
//! before acting again; where a remote API takes a deduplication key, it
//! sends one derived from the same cause.
//!
//! The outbox publisher, reply webhooks, GitHub jobs and the run scheduler
//! keep their own loops. New background work (usage polling, AI jobs,
//! notification fan-out, push delivery, the launch queue, automations,
//! outgoing webhooks) goes here instead of another hand-written loop.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{FromRow, PgExecutor, PgPool};
use tokio::{sync::watch, task::JoinSet};
use uuid::Uuid;

use crate::{AppState, error::AppError, metrics};

/// The most attempts a job gets unless it asks for a different number.
pub const DEFAULT_MAX_ATTEMPTS: i32 = 8;
const MAX_LAST_ERROR_CHARS: usize = 1000;
const MAX_UNIQUE_KEY_CHARS: usize = 300;

/// A job to put on the queue.
#[derive(Clone, Debug)]
pub struct NewJob {
    kind: &'static str,
    payload: Value,
    organization_id: Option<Uuid>,
    project_id: Option<Uuid>,
    run_at: Option<DateTime<Utc>>,
    unique_key: Option<String>,
    max_attempts: i32,
    priority: i16,
}

impl NewJob {
    /// A job of `kind`, such as `"usage.poll"`. The kind must have a handler
    /// in the registry of at least one running server, or it waits.
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            payload: Value::Object(serde_json::Map::new()),
            organization_id: None,
            project_id: None,
            run_at: None,
            unique_key: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            priority: 0,
        }
    }

    /// The job's input. Keep it to ids and small values; never a secret
    /// (store it with `secrets` and pass the secret's id).
    pub fn payload(mut self, payload: impl Serialize) -> Self {
        self.payload = serde_json::to_value(payload).unwrap_or(Value::Null);
        self
    }

    /// The workspace the job works for. Every job that touches a workspace's
    /// data says which one (principle 8).
    pub fn workspace(mut self, organization_id: Uuid) -> Self {
        self.organization_id = Some(organization_id);
        self
    }

    /// The project (and its workspace) the job works for.
    pub fn project(mut self, organization_id: Uuid, project_id: Uuid) -> Self {
        self.organization_id = Some(organization_id);
        self.project_id = Some(project_id);
        self
    }

    /// Run no earlier than `at`.
    pub fn run_at(mut self, at: DateTime<Utc>) -> Self {
        self.run_at = Some(at);
        self
    }

    /// Run no earlier than `delay` from now.
    pub fn delay(mut self, delay: Duration) -> Self {
        let delay = chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::MAX);
        self.run_at = Some(Utc::now().checked_add_signed(delay).unwrap_or(Utc::now()));
        self
    }

    /// At most one queued or running job holds this key; enqueueing another
    /// while one does is a no-op that returns [`Enqueued::Duplicate`].
    pub fn unique(mut self, key: impl Into<String>) -> Self {
        self.unique_key = Some(key.into());
        self
    }

    pub fn max_attempts(mut self, attempts: i32) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// Higher runs first among due jobs; -100 to 100, default 0.
    pub fn priority(mut self, priority: i16) -> Self {
        self.priority = priority;
        self
    }
}

/// What [`enqueue`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Enqueued {
    Created(Uuid),
    /// A queued or running job already holds the unique key.
    Duplicate,
}

/// Put a job on the queue. Pass a transaction to make the job part of the
/// change that needs it.
pub async fn enqueue<'e, E: PgExecutor<'e>>(
    executor: E,
    job: NewJob,
) -> Result<Enqueued, AppError> {
    if job.payload.as_object().is_none() {
        return Err(AppError::Validation("job payload"));
    }
    if !(1..=100).contains(&job.max_attempts) || !(-100..=100).contains(&job.priority) {
        return Err(AppError::Validation("job options"));
    }
    if job
        .unique_key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.chars().count() > MAX_UNIQUE_KEY_CHARS)
    {
        return Err(AppError::Validation("job unique key"));
    }
    if job.project_id.is_some() && job.organization_id.is_none() {
        return Err(AppError::Validation("job workspace"));
    }
    let id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO background_jobs (
            id, organization_id, project_id, kind, payload, unique_key,
            max_attempts, priority, run_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, now()))
        ON CONFLICT (unique_key)
            WHERE unique_key IS NOT NULL AND state IN ('queued', 'running')
            DO NOTHING
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(job.organization_id)
    .bind(job.project_id)
    .bind(job.kind)
    .bind(&job.payload)
    .bind(&job.unique_key)
    .bind(job.max_attempts)
    .bind(job.priority)
    .bind(job.run_at)
    .fetch_optional(executor)
    .await?;
    Ok(inserted.map_or(Enqueued::Duplicate, Enqueued::Created))
}

/// Cancel the queued (not yet running) job holding `unique_key`. Returns
/// whether one was canceled.
pub async fn cancel_queued<'e, E: PgExecutor<'e>>(
    executor: E,
    unique_key: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE background_jobs
        SET state = 'canceled', finished_at = now(), updated_at = now()
        WHERE unique_key = $1 AND state = 'queued'
        "#,
    )
    .bind(unique_key)
    .execute(executor)
    .await?
    .rows_affected()
        > 0)
}

/// A claimed job, as its handler sees it.
#[derive(Clone, Debug, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// This attempt's number, starting at 1.
    pub attempts: i32,
    pub max_attempts: i32,
    lease_token: Uuid,
}

impl Job {
    /// The payload as a typed value.
    pub fn payload_as<T: DeserializeOwned>(&self) -> Result<T, JobError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|_| JobError::permanent("the job's payload doesn't match its kind"))
    }
}

/// Why a handler didn't finish.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobError {
    /// Try again later, with backoff, until the job runs out of attempts.
    Retry(String),
    /// Retrying won't help; the job goes to the dead state now.
    Permanent(String),
}

impl JobError {
    pub fn retry(reason: impl Into<String>) -> Self {
        Self::Retry(reason.into())
    }

    pub fn permanent(reason: impl Into<String>) -> Self {
        Self::Permanent(reason.into())
    }

    fn reason(&self) -> &str {
        match self {
            Self::Retry(reason) | Self::Permanent(reason) => reason,
        }
    }
}

impl From<AppError> for JobError {
    fn from(error: AppError) -> Self {
        // Only the category: driver messages can carry bound values.
        Self::Retry(error.log_category().to_owned())
    }
}

impl From<sqlx::Error> for JobError {
    fn from(error: sqlx::Error) -> Self {
        AppError::from(error).into()
    }
}

/// What a handler is given.
#[derive(Clone)]
pub struct JobContext {
    pub state: AppState,
    pub job: Job,
}

pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;
type Handler = Arc<dyn Fn(JobContext) -> JobFuture + Send + Sync>;

struct Registration {
    handler: Handler,
    timeout: Duration,
}

/// A system job that runs every `interval`, such as retention.
#[derive(Clone, Debug)]
struct Recurring {
    kind: &'static str,
    interval: Duration,
}

/// Which handler runs each kind of job, and which system jobs recur.
#[derive(Default)]
pub struct JobRegistry {
    handlers: HashMap<&'static str, Registration>,
    recurring: Vec<Recurring>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `kind` jobs with `handler`, for at most ten minutes an attempt.
    pub fn register<F, Fut>(&mut self, kind: &'static str, handler: F) -> &mut Self
    where
        F: Fn(JobContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        self.register_with_timeout(kind, Duration::from_secs(600), handler)
    }

    /// Like [`register`](Self::register), with a different time limit per
    /// attempt. An attempt that runs over is retried.
    pub fn register_with_timeout<F, Fut>(
        &mut self,
        kind: &'static str,
        timeout: Duration,
        handler: F,
    ) -> &mut Self
    where
        F: Fn(JobContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |context| Box::pin(handler(context)));
        let previous = self
            .handlers
            .insert(kind, Registration { handler, timeout });
        assert!(previous.is_none(), "job kind {kind} registered twice");
        self
    }

    /// Also run `kind`, which must be registered, every `interval`. The
    /// queue holds at most one of it at a time, so restarts and several
    /// servers don't multiply it.
    pub fn every(&mut self, kind: &'static str, interval: Duration) -> &mut Self {
        assert!(
            self.handlers.contains_key(kind),
            "recurring job kind {kind} has no handler"
        );
        self.recurring.push(Recurring { kind, interval });
        self
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        let mut kinds = self.handlers.keys().copied().collect::<Vec<_>>();
        kinds.sort_unstable();
        kinds
    }
}

/// The registry the server runs: retention and queue upkeep. Workstreams
/// add their kinds here.
pub fn default_registry() -> JobRegistry {
    let mut registry = JobRegistry::new();
    crate::retention::register_jobs(&mut registry);
    crate::ai::register_jobs(&mut registry);
    crate::launch::register_jobs(&mut registry);
    crate::work_items::templates::register_jobs(&mut registry);
    crate::work_items::imports::register_jobs(&mut registry);
    registry.register(PRUNE_KIND, |context| async move {
        prune_finished(&context.state.pool).await?;
        Ok(())
    });
    registry.every(PRUNE_KIND, Duration::from_secs(6 * 60 * 60));
    registry.register(HEALTH_CHECK_KIND, |context| async move {
        metrics::health_check(&context.state).await?;
        Ok(())
    });
    registry.every(HEALTH_CHECK_KIND, Duration::from_secs(5 * 60));
    crate::usage::register_jobs(&mut registry);
    crate::push::register_jobs(&mut registry);
    crate::notification_email::register_jobs(&mut registry);
    crate::later::register_jobs(&mut registry);
    crate::scheduled_messages::register_jobs(&mut registry);
    crate::password_reset::register_jobs(&mut registry);
    crate::events::register_jobs(&mut registry);
    crate::webhooks::register_jobs(&mut registry);
    crate::automations::register_jobs(&mut registry);
    crate::files::register_jobs(&mut registry);
    crate::link_previews::register_jobs(&mut registry);
    crate::things::backfill::register_jobs(&mut registry);
    registry
}

const HEALTH_CHECK_KIND: &str = "ops.health_check";

const PRUNE_KIND: &str = "jobs.prune_finished";

/// How the workers run.
#[derive(Clone, Debug)]
pub struct WorkerOptions {
    /// Jobs run at once in this process.
    pub concurrency: usize,
    /// How often to look for due jobs when the queue was empty.
    pub poll_interval: Duration,
    /// How long a claim lasts before another worker may take the job. The
    /// worker renews it while the handler runs.
    pub lease: Duration,
    /// Names this process in `lease_owner`, for operators.
    pub worker_id: String,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            concurrency: 4,
            poll_interval: Duration::from_secs(2),
            lease: Duration::from_secs(60),
            worker_id: format!("server-{}", Uuid::now_v7()),
        }
    }
}

/// Run jobs until `shutdown` turns true, then let running jobs finish.
pub async fn run_workers(
    state: AppState,
    registry: Arc<JobRegistry>,
    options: WorkerOptions,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let kinds = registry
        .kinds()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut running = JoinSet::new();
    let mut last_recurring_check: Option<Instant> = None;
    loop {
        if *shutdown.borrow() {
            break;
        }
        if last_recurring_check.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
            if let Err(error) = ensure_recurring(&state.pool, &registry).await {
                tracing::warn!(
                    category = error.log_category(),
                    "recurring job check failed"
                );
            }
            last_recurring_check = Some(Instant::now());
        }
        while running.try_join_next().is_some() {}
        let free = options.concurrency.saturating_sub(running.len());
        let mut claimed_any = false;
        if free > 0 {
            match claim(&state.pool, &kinds, free, &options).await {
                Ok(jobs) => {
                    claimed_any = !jobs.is_empty();
                    for job in jobs {
                        let state = state.clone();
                        let registry = Arc::clone(&registry);
                        let lease = options.lease;
                        running.spawn(async move {
                            execute(&state, &registry, job, lease).await;
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(category = error.log_category(), "job claim failed");
                }
            }
        }
        if claimed_any && running.len() < options.concurrency {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(options.poll_interval) => {}
            Some(_) = running.join_next(), if !running.is_empty() => {}
            _ = shutdown.changed() => {}
        }
    }
    while running.join_next().await.is_some() {}
    Ok(())
}

/// Claim and run every job that is due now, one at a time, and return how
/// many ran. For tests and host commands; the server uses [`run_workers`].
pub async fn run_due(
    state: &AppState,
    registry: &JobRegistry,
    options: &WorkerOptions,
) -> Result<usize, AppError> {
    let kinds = registry
        .kinds()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut count = 0;
    loop {
        let jobs = claim(&state.pool, &kinds, 1, options).await?;
        let Some(job) = jobs.into_iter().next() else {
            return Ok(count);
        };
        execute(state, registry, job, options.lease).await;
        count += 1;
    }
}

/// Make sure each recurring system job has its next run queued.
pub async fn ensure_recurring(pool: &PgPool, registry: &JobRegistry) -> Result<(), AppError> {
    for recurring in &registry.recurring {
        ensure_next(pool, recurring).await?;
    }
    Ok(())
}

/// Queue the next run of one recurring job, `interval` after its last.
async fn ensure_next(pool: &PgPool, recurring: &Recurring) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO background_jobs (id, kind, unique_key, run_at, max_attempts)
        SELECT $1, $2::text, $3::text,
               GREATEST(
                   now(),
                   (SELECT max(finished_at) FROM background_jobs
                    WHERE kind = $2::text AND unique_key = $3::text
                      AND state IN ('succeeded', 'dead'))
                   + make_interval(secs => $4)
               ),
               3
        ON CONFLICT (unique_key)
            WHERE unique_key IS NOT NULL AND state IN ('queued', 'running')
            DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(recurring.kind)
    .bind(format!("recurring:{}", recurring.kind))
    .bind(recurring.interval.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(())
}

async fn claim(
    pool: &PgPool,
    kinds: &[String],
    limit: usize,
    options: &WorkerOptions,
) -> Result<Vec<Job>, AppError> {
    // A job whose lease ran out on its last attempt is dead, not retried. The
    // claim below also skips such rows, so one whose lease expires between the
    // two statements waits for the next pass instead of running again.
    let expired = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        UPDATE background_jobs
        SET state = 'dead', lease_token = NULL, lease_owner = NULL,
            lease_expires_at = NULL, finished_at = now(), updated_at = now(),
            last_error = 'The worker stopped before the job finished.'
        WHERE state = 'running' AND lease_expires_at < now()
          AND attempts >= max_attempts AND kind = ANY($1)
        RETURNING id, kind
        "#,
    )
    .bind(kinds)
    .fetch_all(pool)
    .await?;
    for (id, kind) in expired {
        report_dead(id, &kind, "The worker stopped before the job finished.");
    }
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    Ok(sqlx::query_as::<_, Job>(
        r#"
        WITH due AS (
            SELECT id FROM background_jobs
            WHERE kind = ANY($1)
              AND ((state = 'queued' AND run_at <= now())
                   OR (state = 'running' AND lease_expires_at < now()
                       AND attempts < max_attempts))
            ORDER BY priority DESC, run_at, id
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        UPDATE background_jobs job
        SET state = 'running', attempts = job.attempts + 1,
            lease_token = gen_random_uuid(), lease_owner = $3,
            lease_expires_at = now() + make_interval(secs => $4),
            started_at = now(), updated_at = now()
        FROM due
        WHERE job.id = due.id
        RETURNING job.id, job.kind, job.payload, job.organization_id, job.project_id,
                  job.attempts, job.max_attempts, job.lease_token
        "#,
    )
    .bind(kinds)
    .bind(limit)
    .bind(&options.worker_id)
    .bind(options.lease.as_secs_f64())
    .fetch_all(pool)
    .await?)
}

async fn execute(state: &AppState, registry: &JobRegistry, job: Job, lease: Duration) {
    let Some(registration) = registry.handlers.get(job.kind.as_str()) else {
        return;
    };
    let started = Instant::now();
    let kind = job.kind.clone();
    let context = JobContext {
        state: state.clone(),
        job: job.clone(),
    };
    // The claim may have reached this worker late, after its lease ran out
    // and another worker took the job. Confirm the lease is still ours, and
    // time renewals from this check, before any effect starts.
    let Some(confirmed_at) = confirm_lease(&state.pool, job.id, job.lease_token, lease).await
    else {
        tracing::warn!(job_id = %job.id, kind, "a claimed job's lease was gone before it started");
        return;
    };
    let handler = (registration.handler)(context);
    let renewals = renew_lease(
        state.pool.clone(),
        job.id,
        job.lease_token,
        lease,
        confirmed_at,
    );
    let result = tokio::select! {
        result = tokio::time::timeout(registration.timeout, handler) => {
            result.unwrap_or_else(|_| Err(JobError::retry("The job ran out of time.")))
        }
        () = renewals => Err(JobError::retry("The job lost its lease.")),
    };
    metrics::observe_job(&kind, started.elapsed(), result.is_ok());
    if let Err(error) = finish(&state.pool, &job, result).await {
        tracing::warn!(
            category = error.log_category(),
            job_id = %job.id,
            kind,
            "could not record a job's result; its lease will expire and it will run again"
        );
    }
    if let Some(recurring) = registry
        .recurring
        .iter()
        .find(|recurring| recurring.kind == job.kind)
        && let Err(error) = ensure_next(&state.pool, recurring).await
    {
        tracing::warn!(
            category = error.log_category(),
            kind,
            "could not queue the next run"
        );
    }
}

/// Extend the lease every third of its length. Returns when the lease is
/// lost: another worker took the job, or renewals kept failing until the
/// lease may have expired (so another worker may take it). Returning cancels
/// the handler, so two workers never run the same job at once.
/// Extend a lease this worker holds, giving up after a sixth of it.
/// `Ok(false)` means the lease is no longer this worker's.
async fn extend_lease(pool: &PgPool, id: Uuid, token: Uuid, lease: Duration) -> Result<bool, ()> {
    let extended = tokio::time::timeout(
        lease / 6,
        sqlx::query(
            r#"
            UPDATE background_jobs
            SET lease_expires_at = now() + make_interval(secs => $3), updated_at = now()
            WHERE id = $1 AND lease_token = $2 AND state = 'running'
              AND lease_expires_at > now()
            "#,
        )
        .bind(id)
        .bind(token)
        .bind(lease.as_secs_f64())
        .execute(pool),
    )
    .await;
    match extended {
        Ok(Ok(result)) => Ok(result.rows_affected() == 1),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

/// Renew a just-claimed lease once before its handler starts. Returns when
/// the renewal was sent, a safe lower bound for when the lease now ends.
async fn confirm_lease(
    pool: &PgPool,
    id: Uuid,
    token: Uuid,
    lease: Duration,
) -> Option<tokio::time::Instant> {
    let sent = tokio::time::Instant::now();
    matches!(extend_lease(pool, id, token, lease).await, Ok(true)).then_some(sent)
}

async fn renew_lease(
    pool: PgPool,
    id: Uuid,
    token: Uuid,
    lease: Duration,
    mut renewed_at: tokio::time::Instant,
) {
    loop {
        tokio::time::sleep(lease / 3).await;
        let sent = tokio::time::Instant::now();
        match extend_lease(&pool, id, token, lease).await {
            Ok(true) => renewed_at = sent,
            Ok(false) => return,
            // A database blip: keep trying while the lease still holds.
            Err(()) => {
                if !lease_still_held(renewed_at.elapsed(), lease) {
                    tracing::warn!(job_id = %id, "could not renew a job's lease in time; stopping it");
                    return;
                }
            }
        }
    }
}

/// Whether a lease last renewed `since_renewed` ago is sure to still hold
/// through one more try: a wait of a third of its length plus a renewal
/// that may take a sixth.
fn lease_still_held(since_renewed: Duration, lease: Duration) -> bool {
    since_renewed + lease / 3 + lease / 6 < lease
}

async fn finish(pool: &PgPool, job: &Job, result: Result<(), JobError>) -> Result<(), AppError> {
    match result {
        Ok(()) => {
            sqlx::query(
                r#"
                UPDATE background_jobs
                SET state = 'succeeded', lease_token = NULL, lease_owner = NULL,
                    lease_expires_at = NULL, last_error = NULL,
                    finished_at = now(), updated_at = now()
                WHERE id = $1 AND lease_token = $2
                "#,
            )
            .bind(job.id)
            .bind(job.lease_token)
            .execute(pool)
            .await?;
        }
        Err(error) => {
            let permanent = matches!(error, JobError::Permanent(_));
            let reason = bounded(error.reason());
            let dead = permanent || job.attempts >= job.max_attempts;
            let updated = sqlx::query(
                r#"
                UPDATE background_jobs
                SET state = CASE WHEN $3 THEN 'dead' ELSE 'queued' END,
                    run_at = CASE WHEN $3 THEN run_at ELSE now() + make_interval(secs => $4) END,
                    finished_at = CASE WHEN $3 THEN now() END,
                    lease_token = NULL, lease_owner = NULL, lease_expires_at = NULL,
                    last_error = $5, updated_at = now()
                WHERE id = $1 AND lease_token = $2
                "#,
            )
            .bind(job.id)
            .bind(job.lease_token)
            .bind(dead)
            .bind(backoff(job.attempts).as_secs_f64())
            .bind(&reason)
            .execute(pool)
            .await?;
            if updated.rows_affected() == 1 {
                if dead {
                    report_dead(job.id, &job.kind, &reason);
                } else {
                    if job.attempts == 3 {
                        metrics::alert(metrics::Alert {
                            key: "job_failing",
                            subject: job.kind.clone(),
                            detail: format!(
                                "Background job {} ({}) has failed three times and will keep retrying: {reason}",
                                job.id, job.kind
                            ),
                        });
                    }
                    tracing::warn!(
                        job_id = %job.id,
                        kind = job.kind,
                        attempt = job.attempts,
                        reason,
                        "background job failed; it will be retried"
                    );
                }
            }
        }
    }
    Ok(())
}

fn report_dead(id: Uuid, kind: &str, reason: &str) {
    metrics::job_dead(kind);
    metrics::alert(metrics::Alert {
        key: "job_dead",
        subject: kind.to_owned(),
        detail: format!("Background job {id} ({kind}) failed for good: {reason}"),
    });
}

/// Ten seconds after the first failure, doubling to at most an hour, with
/// up to a fifth of jitter so failures don't retry in lockstep.
pub fn backoff(attempt: i32) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1).clamp(0, 12)).unwrap_or(12);
    let base = (10_u64 << exponent).min(3600);
    let jitter = rand_jitter(base / 5);
    Duration::from_secs(base + jitter)
}

fn rand_jitter(maximum: u64) -> u64 {
    use rand_core::{OsRng, RngCore};
    if maximum == 0 {
        return 0;
    }
    OsRng.next_u64() % (maximum + 1)
}

fn bounded(reason: &str) -> String {
    let reason = reason.trim();
    let reason = if reason.is_empty() {
        "The job failed."
    } else {
        reason
    };
    reason.chars().take(MAX_LAST_ERROR_CHARS).collect()
}

/// Delete finished jobs: succeeded and canceled after a week, dead after a
/// month (long enough to look into).
pub async fn prune_finished(pool: &PgPool) -> Result<u64, AppError> {
    let mut removed = 0;
    loop {
        let batch = sqlx::query(
            r#"
            DELETE FROM background_jobs
            WHERE id IN (
                SELECT id FROM background_jobs
                WHERE (state IN ('succeeded', 'canceled') AND finished_at < now() - interval '7 days')
                   OR (state = 'dead' AND finished_at < now() - interval '30 days')
                LIMIT 5000
            )
            "#,
        )
        .execute(pool)
        .await?
        .rows_affected();
        removed += batch;
        if batch < 5000 {
            return Ok(removed);
        }
    }
}

/// Queue counts for the metrics endpoint.
#[derive(Clone, Debug, Default, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStats {
    pub queued_due: i64,
    pub queued_later: i64,
    pub running: i64,
    pub dead_last_day: i64,
    /// Seconds the oldest due job has waited.
    pub oldest_due_seconds: f64,
}

pub async fn queue_stats(pool: &PgPool) -> Result<QueueStats, AppError> {
    Ok(sqlx::query_as::<_, QueueStats>(
        r#"
        SELECT
            count(*) FILTER (WHERE state = 'queued' AND run_at <= now()) AS queued_due,
            count(*) FILTER (WHERE state = 'queued' AND run_at > now()) AS queued_later,
            count(*) FILTER (WHERE state = 'running') AS running,
            count(*) FILTER (WHERE state = 'dead' AND finished_at > now() - interval '1 day')
                AS dead_last_day,
            COALESCE(EXTRACT(EPOCH FROM now() - min(run_at)
                FILTER (WHERE state = 'queued' AND run_at <= now())), 0)::float8
                AS oldest_due_seconds
        FROM background_jobs
        WHERE state IN ('queued', 'running')
           OR (state = 'dead' AND finished_at > now() - interval '1 day')
        "#,
    )
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handler_stops_before_a_lease_it_cannot_renew_runs_out() {
        let lease = Duration::from_secs(30);
        assert!(lease_still_held(Duration::from_secs(10), lease));
        assert!(lease_still_held(Duration::from_secs(14), lease));
        assert!(!lease_still_held(Duration::from_secs(15), lease));
        assert!(!lease_still_held(Duration::from_secs(45), lease));
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let first = backoff(1).as_secs();
        assert!((10..=12).contains(&first), "{first}");
        let third = backoff(3).as_secs();
        assert!((40..=48).contains(&third), "{third}");
        let late = backoff(40).as_secs();
        assert!((3600..=4320).contains(&late), "{late}");
    }

    #[test]
    fn failure_reasons_are_bounded() {
        assert_eq!(bounded("  "), "The job failed.");
        assert_eq!(bounded(&"x".repeat(5000)).chars().count(), 1000);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn a_kind_has_one_handler() {
        let mut registry = JobRegistry::new();
        registry.register("test.once", |_| async { Ok(()) });
        registry.register("test.once", |_| async { Ok(()) });
    }
}
