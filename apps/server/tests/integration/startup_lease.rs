//! H1.15: the startup work runs under the `server.startup` lease. Two
//! servers started at once against one database: one does the work, the
//! other waits for it and then finds nothing left to do; neither runs it
//! beside the other.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_workspace_server::startup;

#[tokio::test]
async fn two_servers_starting_at_once_take_turns() {
    let Some(db) = super::support::db::db("h1_15_startup").await else {
        return;
    };
    let pool = db.pool.clone();
    let running = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let waited = Arc::new(AtomicUsize::new(0));
    let start = || {
        let (pool, running, most, done, waited) = (
            pool.clone(),
            Arc::clone(&running),
            Arc::clone(&most),
            Arc::clone(&done),
            Arc::clone(&waited),
        );
        async move {
            startup::under_lease(
                &pool,
                Duration::from_secs(60),
                || {
                    waited.fetch_add(1, Ordering::SeqCst);
                },
                async {
                    // The work: only the first finds anything to do.
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    if done.load(Ordering::SeqCst) == 0 {
                        tokio::time::sleep(Duration::from_secs(7)).await;
                        done.fetch_add(1, Ordering::SeqCst);
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(())
                },
            )
            .await
        }
    };
    let (first, second) = tokio::join!(start(), start());
    first.expect("first start");
    second.expect("second start");
    assert_eq!(most.load(Ordering::SeqCst), 1, "never side by side");
    assert_eq!(done.load(Ordering::SeqCst), 1, "the work ran once");
    assert_eq!(waited.load(Ordering::SeqCst), 1, "the other one waited");

    // Held by a process that stopped: the lease runs out, then is taken.
    let holder = uuid::Uuid::now_v7();
    sqlx::query(
        "UPDATE leases SET holder_instance_id = $1, expires_at = now() + interval '1 hour' \
         WHERE name = $2",
    )
    .bind(holder)
    .bind(startup::LEASE)
    .execute(&pool)
    .await
    .expect("held elsewhere");
    let refused = startup::under_lease(&pool, Duration::from_secs(1), || {}, async {
        Ok::<_, anyhow::Error>(())
    })
    .await;
    assert!(refused.is_err(), "a waiter past its limit exits");
    db.finish().await;
}
