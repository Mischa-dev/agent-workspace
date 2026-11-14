//! K5: any number of servers. Two instances on one database act as one: a
//! sign-in on one works on the other, a write on one reads on the other,
//! and a lease is held by one of them at a time.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use agent_workspace_server::{cluster::LeaseName, error::AppError};
use axum::http::StatusCode;
use futures_util::future::join_all;
use serde_json::{Value, json};

use crate::cluster_support::{cluster, cluster_with};

#[tokio::test]
async fn sign_in_on_one_instance_works_on_the_other() {
    let Some(cluster) = cluster("k5_1_sign_in", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    for instance in &cluster.instances {
        let (status, session) = instance
            .send(browser.request("GET", "/api/v1/auth/session", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{}: {session}", instance.name);
        assert!(session.to_string().contains("owner"), "{session}");
    }
    cluster.finish().await;
}

#[tokio::test]
async fn write_on_one_read_on_the_other() {
    let Some(mut cluster) = cluster("k5_1_write_read", 2).await else {
        return;
    };
    let a = cluster.instance(0);
    let b = cluster.instance(1);
    let browser = a.login("owner").await;
    let (status, project) = a
        .send(browser.request(
            "POST",
            "/api/v1/projects",
            Some("cluster-project"),
            json!({"organizationId": cluster.organization_id, "key": "WORK", "name": "Work"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project = project["id"].as_str().expect("project").to_owned();
    let (status, task) = a
        .send(browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/work-items"),
            Some("cluster-task"),
            json!({"title": "Written on A"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().expect("task").to_owned();
    let (status, read) = b
        .send(browser.request(
            "GET",
            &format!("/api/v1/projects/{project}/work-items/{task}"),
            None,
            Value::Null,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert!(read.to_string().contains("Written on A"), "{read}");

    // A stops as a crash would; B carries on serving.
    cluster.instance_mut(0).abort();
    let b = cluster.instance(1);
    let (status, _) = b
        .send(browser.request("GET", "/api/v1/auth/session", None, Value::Null))
        .await;
    assert_eq!(status, StatusCode::OK);
    cluster.finish().await;
}

// K5.2: instances and leases, with a two-second time to live.

const SHORT_LEASES: &[(&str, &str)] = &[("AGENT_WORKSPACE_LEASE_SECONDS", "2")];

#[tokio::test]
async fn lease_is_held_once_under_contention() {
    let Some(cluster) = cluster_with("k5_2_contention", 2, SHORT_LEASES).await else {
        return;
    };
    let name = LeaseName::Consumer("contended");
    let attempts = cluster.instances.iter().flat_map(|instance| {
        let leases = instance.cluster().clone();
        let name = name.clone();
        (0..50).map(move |_| {
            let leases = leases.clone();
            let name = name.clone();
            async move {
                let lease = leases.try_acquire(name).await.expect("try to acquire");
                lease.map(|lease| (leases.instance_id(), lease.token))
            }
        })
    });
    let taken = join_all(attempts)
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let holders = taken
        .iter()
        .map(|(holder, _)| *holder)
        .collect::<HashSet<_>>();
    assert_eq!(holders.len(), 1, "one instance holds it: {taken:?}");
    assert!(taken.iter().all(|(_, token)| *token == 1), "{taken:?}");
    let holder = cluster
        .instances
        .iter()
        .position(|instance| holders.contains(&instance.cluster().instance_id()))
        .expect("holder");
    assert!(cluster.instance(holder).cluster().holds(&name));
    assert!(!cluster.instance(1 - holder).cluster().holds(&name));
    cluster.finish().await;
}

#[tokio::test]
async fn lease_moves_after_holder_stops() {
    let Some(mut cluster) = cluster_with("k5_2_moves", 2, SHORT_LEASES).await else {
        return;
    };
    let name = LeaseName::Consumer("moving");
    let first = cluster
        .instance(0)
        .cluster()
        .try_acquire(name.clone())
        .await
        .expect("acquire")
        .expect("A takes it");
    let b = cluster.instance(1).cluster().clone();
    assert!(b.try_acquire(name.clone()).await.expect("try").is_none());
    let crashed = Instant::now();
    cluster.instance_mut(0).abort();
    let second = loop {
        if let Some(lease) = b.try_acquire(name.clone()).await.expect("try") {
            break lease;
        }
        assert!(
            crashed.elapsed() < Duration::from_secs(10),
            "B never took it"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(second.token, first.token + 1);
    // Not before A's last renewal ran out.
    assert!(
        crashed.elapsed() >= Duration::from_millis(500),
        "{:?}",
        crashed.elapsed()
    );
    cluster.finish().await;
}

#[tokio::test]
async fn released_lease_is_taken_at_once() {
    let Some(cluster) = cluster_with("k5_2_release", 2, SHORT_LEASES).await else {
        return;
    };
    let name = LeaseName::Consumer("handed-over");
    let a = cluster.instance(0).cluster();
    let b = cluster.instance(1).cluster();
    let lease = a
        .try_acquire(name.clone())
        .await
        .expect("acquire")
        .expect("A takes it");
    assert!(b.try_acquire(name.clone()).await.expect("try").is_none());
    a.release(&lease).await.expect("release");
    assert!(lease.lost.is_cancelled());
    let released = Instant::now();
    let taken = b
        .try_acquire(name.clone())
        .await
        .expect("try")
        .expect("B takes it at once");
    assert!(released.elapsed() < Duration::from_millis(500));
    assert_eq!(taken.token, lease.token + 1);
    cluster.finish().await;
}

#[tokio::test]
async fn fenced_write_fails_with_a_stale_token() {
    let Some(mut cluster) = cluster_with("k5_2_fence", 2, SHORT_LEASES).await else {
        return;
    };
    let name = LeaseName::Consumer("fenced");
    let a = cluster.instance(0).cluster().clone();
    let b = cluster.instance(1).cluster().clone();
    let stale = a
        .try_acquire(name.clone())
        .await
        .expect("acquire")
        .expect("A takes it");
    // While A holds it, its fence passes.
    let mut transaction = cluster.pool.begin().await.expect("transaction");
    a.fence(&mut transaction, &stale)
        .await
        .expect("A still holds it");
    transaction.rollback().await.expect("rollback");
    // A stops renewing (as a crash would) and B takes over.
    cluster.instance_mut(0).abort();
    let started = Instant::now();
    let current = loop {
        if let Some(lease) = b.try_acquire(name.clone()).await.expect("try") {
            break lease;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "B never took it"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let mut transaction = cluster.pool.begin().await.expect("transaction");
    let refused = a.fence(&mut transaction, &stale).await;
    assert!(matches!(refused, Err(AppError::LeaseLost)), "{refused:?}");
    transaction.rollback().await.expect("rollback");
    let mut transaction = cluster.pool.begin().await.expect("transaction");
    b.fence(&mut transaction, &current)
        .await
        .expect("B holds it");
    transaction.rollback().await.expect("rollback");
    cluster.finish().await;
}

#[tokio::test]
async fn self_fencing_cancels_work_when_renewal_stops() {
    let Some(cluster) = cluster_with("k5_2_self_fence", 2, SHORT_LEASES).await else {
        return;
    };
    let name = LeaseName::Consumer("self-fenced");
    let a = cluster.instance(0).cluster().clone();
    let b = cluster.instance(1).cluster().clone();
    let lease = a
        .try_acquire(name.clone())
        .await
        .expect("acquire")
        .expect("A takes it");
    // Hold A's instance row so its heartbeat can't renew.
    let mut blocker = cluster.pool.begin().await.expect("transaction");
    sqlx::query("SELECT 1 FROM server_instances WHERE id = $1 FOR UPDATE")
        .bind(a.instance_id().0)
        .execute(&mut *blocker)
        .await
        .expect("lock A's row");
    let started = Instant::now();
    let lost = tokio::spawn({
        let lost = lease.lost.clone();
        async move {
            lost.cancelled().await;
            Instant::now()
        }
    });
    let taken = loop {
        if b.try_acquire(name.clone()).await.expect("try").is_some() {
            break Instant::now();
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "B never took it"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let lost = tokio::time::timeout(Duration::from_secs(5), lost)
        .await
        .expect("A gave the lease up")
        .expect("join");
    assert!(lost < taken, "A stopped its work before B took over");
    assert!(!a.holds(&name));
    blocker.rollback().await.expect("rollback");
    cluster.finish().await;
}
