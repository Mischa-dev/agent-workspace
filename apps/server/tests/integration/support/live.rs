//! Live updates the way a running server delivers them (K5.5): sockets hear
//! the event log through the tail, not the outbox publisher.

use std::sync::Arc;

use agent_workspace_server::{events, realtime::RealtimeHub};
use sqlx::PgPool;
use tokio::{
    sync::{Notify, watch},
    task::JoinHandle,
};

/// Runs the sequencer, a tail and the feed into `hub` until the sender says
/// stop (or is dropped). The tail starts after position `from`, or at the
/// log's head now when `None`, as a starting server does; start it before
/// the writes a test listens for.
pub(crate) async fn follow_log(
    pool: &PgPool,
    hub: &RealtimeHub,
    from: Option<i64>,
) -> (watch::Sender<bool>, JoinHandle<()>) {
    let start = match from {
        Some(position) => position,
        None => {
            let mut connection = pool.acquire().await.expect("connection for the log head");
            events::head(&mut connection).await.expect("log head")
        }
    };
    let (stop, stopped) = watch::channel(false);
    let sender = agent_workspace_server::realtime::tail::channel();
    let sequencer = events::sequencer::run(pool.clone(), Arc::new(Notify::new()), stopped.clone());
    let tail = agent_workspace_server::realtime::tail::run(
        pool.clone(),
        start,
        Arc::new(Notify::new()),
        sender.clone(),
        stopped.clone(),
    );
    let feed =
        agent_workspace_server::realtime::tail::feed(hub.clone(), sender.subscribe(), stopped);
    let task = tokio::spawn(async move {
        tokio::join!(sequencer, tail, feed);
    });
    (stop, task)
}
