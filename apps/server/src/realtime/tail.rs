//! The tail (K5 3.4): each instance follows the log from the head it saw
//! at start (there's nobody to replay to) and hands every event, once, to
//! whoever listens here. It reads on a wake and on a poll, at most one read
//! at a time and 20 a second, 500 events a read; if it falls more than
//! 10,000 positions behind it jumps to the head and says `Resync`.
//!
//! K5.4 runs it in shadow: it counts what it sees (position, lag, gaps),
//! and the sockets are still fed by the outbox publisher until K5.5. A gap
//! in positions is a bug (the sequencer makes them dense), so it is counted
//! and logged, never skipped silently.

use std::{sync::Arc, time::Duration};

use sqlx::PgPool;
use tokio::{
    sync::{Notify, broadcast, watch},
    time::Instant,
};

use crate::{
    error::AppError,
    events::{self, LoggedEvent},
};

/// Events per read.
pub const READ: i64 = 500;
/// The fastest the tail reads (20 a second).
const MIN_GAP: Duration = Duration::from_millis(50);
/// The safety poll when no wake comes.
pub const POLL: Duration = Duration::from_millis(250);
/// How far behind before the tail jumps to the head.
pub const JUMP_AFTER: i64 = 10_000;

/// What the tail hands on.
#[derive(Clone, Debug)]
pub enum Tailed {
    Event(Arc<LoggedEvent>),
    /// The tail skipped ahead: refetch rather than wait for a replay.
    Resync,
}

/// Where the tail's events go; subscribe for a receiver.
pub type TailSender = broadcast::Sender<Tailed>;

pub fn channel() -> TailSender {
    broadcast::channel(4096).0
}

/// Follow the log after `start` (the head when the instance started)
/// until shutdown. Errors are logged and retried.
pub async fn run(
    pool: PgPool,
    start: i64,
    wake: Arc<Notify>,
    sender: TailSender,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut after = start;
    let mut last_read = Instant::now() - MIN_GAP;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let pause = match step(&pool, &mut after, &sender).await {
            Ok(read) if read >= READ as usize => Duration::ZERO,
            Ok(_) => POLL,
            Err(error) => {
                tracing::warn!(
                    error_category = error.log_category(),
                    "event tail read failed"
                );
                Duration::from_secs(1)
            }
        };
        // At most one read per MIN_GAP, however often it is woken.
        let wait_until = last_read + MIN_GAP;
        last_read = Instant::now();
        if !pause.is_zero() {
            tokio::select! {
                () = wake.notified() => {}
                () = tokio::time::sleep(pause) => {}
                _ = shutdown.changed() => {}
            }
        }
        tokio::time::sleep_until(wait_until).await;
    }
}

/// One read: returns how many events it handed on.
async fn step(pool: &PgPool, after: &mut i64, sender: &TailSender) -> Result<usize, AppError> {
    let mut connection = pool.acquire().await?;
    let head = events::head(&mut connection).await?;
    let position = *after;
    if head - position > JUMP_AFTER {
        tracing::warn!(
            behind = head - position,
            "event tail fell behind; jumping to the head"
        );
        let _ = sender.send(Tailed::Resync);
        *after = head;
        crate::metrics::tail_moved(head, 0, 0);
        return Ok(0);
    }
    let batch = events::tail(&mut connection, position, READ).await?;
    drop(connection);
    let mut expected = position + 1;
    let mut gaps = 0;
    for event in &batch {
        if event.position != expected {
            gaps += 1;
            tracing::error!(
                expected,
                found = event.position,
                "the event log has a gap in positions"
            );
        }
        expected = event.position + 1;
        let _ = sender.send(Tailed::Event(Arc::new(event.clone())));
    }
    let reached = batch.last().map_or(position, |event| event.position);
    *after = reached;
    crate::metrics::tail_moved(reached, (head - reached).max(0), gaps);
    Ok(batch.len())
}
