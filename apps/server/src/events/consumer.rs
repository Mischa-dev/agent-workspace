//! Consumers of the log (K3 3.3.4): each reads `events` after its own
//! cursor and handles a batch in one transaction with the cursor's advance,
//! so its database effects and its cursor commit together (at least once,
//! and exactly once for database effects). Outside effects (a webhook POST,
//! a push) never happen in `handle`: it enqueues a job keyed by the event.
//!
//! One runner per consumer at a time, in any process: until K5.2's
//! `consumer:<name>` leases, each batch locks the consumer's own
//! `event_consumers` row with `FOR UPDATE SKIP LOCKED` (pooler-safe, never
//! a session advisory lock). A failing batch rolls back and is retried;
//! after three failures the runner goes event by event, and an event that
//! fails five times is parked in `event_consumer_failures` and passed, so
//! one bad event never stops a consumer for long and is never forgotten. A
//! sweep retries parked events.

use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::watch;

use super::{LOGGED_EVENT_COLUMNS, LoggedEvent};
use crate::error::AppError;

/// The most events one batch reads.
pub const BATCH: i64 = 500;
/// Batch failures before the runner goes event by event.
const BATCH_ATTEMPTS: u32 = 3;
/// Failures of one event before it is parked.
const EVENT_ATTEMPTS: u32 = 5;

/// Why a handler didn't finish; its text is kept (cut to 500 characters)
/// with a parked event, so it must carry no content.
#[derive(Debug, Clone)]
pub struct ConsumerError(pub String);

impl From<AppError> for ConsumerError {
    fn from(error: AppError) -> Self {
        Self(error.log_category().to_owned())
    }
}

impl From<sqlx::Error> for ConsumerError {
    fn from(error: sqlx::Error) -> Self {
        AppError::from(error).into()
    }
}

#[async_trait]
pub trait Consumer: Send + Sync {
    /// Its cursor's name.
    fn name(&self) -> &'static str;
    /// The types it wants (None: all); the reader filters in SQL.
    fn types(&self) -> Option<&'static [&'static str]> {
        None
    }
    /// Where a new cursor starts: the position before the first event it
    /// should see. Default: the log's head when the cursor is made, so a
    /// new consumer doesn't replay history.
    fn starts_at_head(&self) -> bool {
        true
    }
    /// Handle a batch inside `transaction`; the runtime advances the cursor
    /// in the same transaction.
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        batch: &[LoggedEvent],
    ) -> Result<(), ConsumerError>;
}

/// What filters a read of the log.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventFilter<'a> {
    pub types: Option<&'a [&'a str]>,
}

/// Events after `after`, oldest first, that pass `filter`. For consumers
/// and the tail only: it answers no account, so readers that do re-check
/// access.
pub async fn read(
    connection: &mut sqlx::PgConnection,
    filter: &EventFilter<'_>,
    after: i64,
    limit: i64,
) -> Result<Vec<LoggedEvent>, AppError> {
    let types = filter
        .types
        .map(|types| types.iter().map(|ty| (*ty).to_owned()).collect::<Vec<_>>());
    Ok(sqlx::query_as::<_, LoggedEvent>(&format!(
        "SELECT {LOGGED_EVENT_COLUMNS} FROM events \
         WHERE position > $1 AND ($3::text[] IS NULL OR type = ANY($3)) \
         ORDER BY position LIMIT $2"
    ))
    .bind(after)
    .bind(limit.clamp(1, 5000))
    .bind(types)
    .fetch_all(&mut *connection)
    .await?)
}

/// What one round did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Round {
    pub handled: usize,
    pub parked: usize,
    /// Another runner held the cursor.
    pub busy: bool,
}

/// A consumer's runner: its in-memory failure counts between rounds.
pub struct Runner<C: Consumer> {
    consumer: C,
    batch_failures: u32,
    /// After repeated batch failures: event by event up to this position.
    one_by_one_until: Option<i64>,
    event_failures: HashMap<i64, u32>,
}

impl<C: Consumer> Runner<C> {
    pub fn new(consumer: C) -> Self {
        Self {
            consumer,
            batch_failures: 0,
            one_by_one_until: None,
            event_failures: HashMap::new(),
        }
    }

    pub fn consumer(&self) -> &C {
        &self.consumer
    }

    /// One batch (or, after repeated failures, one event).
    pub async fn round(&mut self, pool: &PgPool) -> Result<Round, AppError> {
        let one_by_one = self.one_by_one_until.is_some();
        let mut transaction = pool.begin().await?;
        let Some(cursor) = lock_cursor(&mut transaction, &self.consumer).await? else {
            return Ok(Round {
                busy: true,
                ..Round::default()
            });
        };
        let limit = if one_by_one { 1 } else { BATCH };
        let batch = read(
            &mut transaction,
            &EventFilter {
                types: self.consumer.types(),
            },
            cursor,
            limit,
        )
        .await?;
        let Some(last) = batch.last().map(|event| event.position) else {
            transaction.commit().await?;
            self.batch_failures = 0;
            return Ok(Round::default());
        };
        match self.consumer.handle(&mut transaction, &batch).await {
            Ok(()) => {
                advance(&mut transaction, self.consumer.name(), last).await?;
                transaction.commit().await?;
                self.event_failures.remove(&last);
                self.passed(last);
                Ok(Round {
                    handled: batch.len(),
                    ..Round::default()
                })
            }
            Err(error) => {
                transaction.rollback().await?;
                if !one_by_one {
                    self.batch_failures += 1;
                    if self.batch_failures >= BATCH_ATTEMPTS {
                        self.one_by_one_until = Some(last);
                    }
                    return Ok(Round::default());
                }
                let failures = self.event_failures.entry(last).or_default();
                *failures += 1;
                if *failures < EVENT_ATTEMPTS {
                    return Ok(Round::default());
                }
                self.event_failures.remove(&last);
                park(pool, &self.consumer, last, &error).await?;
                self.passed(last);
                Ok(Round {
                    parked: 1,
                    ..Round::default()
                })
            }
        }
    }

    /// The cursor passed `position`: back to batches once past the batch
    /// that failed.
    fn passed(&mut self, position: i64) {
        match self.one_by_one_until {
            Some(until) if position < until => {}
            _ => {
                self.one_by_one_until = None;
                self.batch_failures = 0;
            }
        }
    }

    /// Retry parked events that are due; each success is resolved, each
    /// failure waits longer.
    pub async fn sweep(&self, pool: &PgPool) -> Result<usize, AppError> {
        let due = sqlx::query_scalar::<_, i64>(
            "SELECT event_position FROM event_consumer_failures \
             WHERE consumer = $1 AND resolved_at IS NULL AND next_attempt_at <= now() \
             ORDER BY event_position LIMIT 50",
        )
        .bind(self.consumer.name())
        .fetch_all(pool)
        .await?;
        let mut resolved = 0;
        for position in due {
            let mut transaction = pool.begin().await?;
            let events = read(&mut transaction, &EventFilter::default(), position - 1, 1).await?;
            let outcome = match events.first() {
                Some(event) if event.position == position => {
                    self.consumer.handle(&mut transaction, &events).await
                }
                // Gone (retention): nothing left to do.
                _ => Ok(()),
            };
            match outcome {
                Ok(()) => {
                    sqlx::query(
                        "UPDATE event_consumer_failures SET resolved_at = now() \
                         WHERE consumer = $1 AND event_position = $2",
                    )
                    .bind(self.consumer.name())
                    .bind(position)
                    .execute(&mut *transaction)
                    .await?;
                    transaction.commit().await?;
                    resolved += 1;
                }
                Err(error) => {
                    transaction.rollback().await?;
                    sqlx::query(
                        "UPDATE event_consumer_failures \
                         SET attempts = attempts + 1, last_error = left($3, 500), \
                             next_attempt_at = now() + make_interval(secs => least(3600, 30 * 2 ^ least(attempts, 7))) \
                         WHERE consumer = $1 AND event_position = $2",
                    )
                    .bind(self.consumer.name())
                    .bind(position)
                    .bind(&error.0)
                    .execute(pool)
                    .await?;
                }
            }
        }
        Ok(resolved)
    }
}

/// The cursor, locked for this transaction; None when another runner holds
/// it. A new cursor starts at the head (or the beginning).
async fn lock_cursor<C: Consumer>(
    transaction: &mut Transaction<'_, Postgres>,
    consumer: &C,
) -> Result<Option<i64>, AppError> {
    sqlx::query(
        "INSERT INTO event_consumers (name, position) \
         SELECT $1, CASE WHEN $2 THEN next_position - 1 ELSE 0 END FROM event_position_counter \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(consumer.name())
    .bind(consumer.starts_at_head())
    .execute(&mut **transaction)
    .await?;
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT position FROM event_consumers WHERE name = $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(consumer.name())
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn advance(
    transaction: &mut Transaction<'_, Postgres>,
    name: &str,
    position: i64,
) -> Result<(), AppError> {
    sqlx::query("UPDATE event_consumers SET position = $2, updated_at = now() WHERE name = $1")
        .bind(name)
        .bind(position)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// Park the event at `position` and move the cursor past it.
async fn park<C: Consumer>(
    pool: &PgPool,
    consumer: &C,
    position: i64,
    error: &ConsumerError,
) -> Result<(), AppError> {
    let mut transaction = pool.begin().await?;
    if lock_cursor(&mut transaction, consumer).await?.is_none() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO event_consumer_failures \
             (consumer, event_position, attempts, last_error, next_attempt_at) \
         VALUES ($1, $2, $3, left($4, 500), now() + interval '1 minute') \
         ON CONFLICT (consumer, event_position) DO UPDATE \
         SET attempts = event_consumer_failures.attempts + EXCLUDED.attempts, \
             last_error = EXCLUDED.last_error, next_attempt_at = EXCLUDED.next_attempt_at, \
             resolved_at = NULL",
    )
    .bind(consumer.name())
    .bind(position)
    .bind(i32::try_from(EVENT_ATTEMPTS).unwrap_or(5))
    .bind(&error.0)
    .execute(&mut *transaction)
    .await?;
    advance(&mut transaction, consumer.name(), position).await?;
    transaction.commit().await?;
    Ok(())
}

/// Run a consumer until shutdown: a round whenever there may be work, a
/// sweep every minute, a pause when idle or failing. Errors are logged and
/// retried; they never stop the server.
pub async fn run_consumer<C: Consumer>(
    pool: PgPool,
    consumer: C,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut runner = Runner::new(consumer);
    let mut last_sweep = tokio::time::Instant::now();
    loop {
        if *shutdown.borrow() {
            return;
        }
        let pause = match runner.round(&pool).await {
            Ok(round) if round.handled > 0 || round.parked > 0 => Duration::ZERO,
            Ok(round) if round.busy => Duration::from_secs(2),
            Ok(_) if runner.batch_failures > 0 => backoff(runner.batch_failures),
            Ok(_) => super::sequencer::POLL,
            Err(error) => {
                tracing::warn!(
                    consumer = runner.consumer.name(),
                    error_category = error.log_category(),
                    "event consumer round failed"
                );
                Duration::from_secs(1)
            }
        };
        if last_sweep.elapsed() > Duration::from_secs(60) {
            last_sweep = tokio::time::Instant::now();
            if let Err(error) = runner.sweep(&pool).await {
                tracing::warn!(
                    consumer = runner.consumer.name(),
                    error_category = error.log_category(),
                    "event consumer sweep failed"
                );
            }
        }
        if pause.is_zero() {
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep(pause) => {}
            _ = shutdown.changed() => {}
        }
    }
}

fn backoff(failures: u32) -> Duration {
    Duration::from_millis(250 * 2_u64.pow(failures.min(6)))
}
