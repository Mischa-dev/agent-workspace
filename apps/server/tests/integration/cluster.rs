//! K5: any number of servers. Two instances on one database act as one: a
//! sign-in on one works on the other, a write on one reads on the other,
//! a lease is held by one of them at a time, and every instance's tail
//! sees every event, in order.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use agent_workspace_server::{
    cluster::LeaseName,
    error::AppError,
    events::{self, NewEvent, catalog},
    realtime::tail::Tailed,
};
use axum::http::StatusCode;
use futures_util::future::join_all;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use tokio::sync::broadcast;
use uuid::Uuid;

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
            &format!("/api/v2/w/{}/ops/space_create", cluster.organization_id),
            Some("cluster-project"),
            json!({"key": "WORK", "name": "Work"}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    let project = project["value"].clone();
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

// K5.4: positions, the wake and the tail.

/// A project to hang events on, made through A's HTTP.
async fn log_project(cluster: &crate::cluster_support::Cluster) -> Uuid {
    let a = cluster.instance(0);
    let browser = a.login("owner").await;
    let (status, project) = a
        .send(browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/space_create", cluster.organization_id),
            Some("cluster-log"),
            json!({"key": "LOG", "name": "Log"}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    let project = project["value"].clone();
    project["id"]
        .as_str()
        .expect("project")
        .parse()
        .expect("id")
}

async fn stage(transaction: &mut Transaction<'_, Postgres>, project: Uuid) -> Uuid {
    events::emit(
        transaction,
        NewEvent::thing(&catalog::TASK_UPDATED, project, "task", Uuid::now_v7()),
    )
    .await
    .expect("emit")
}

/// What a tail hands on until it has seen every one of `ids`: (position,
/// id) in the order it came.
async fn tail_until(
    receiver: &mut broadcast::Receiver<Tailed>,
    ids: &HashSet<Uuid>,
    within: Duration,
) -> Vec<(i64, Uuid)> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + within;
    while !ids.iter().all(|id| seen.iter().any(|(_, seen)| seen == id)) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, receiver.recv()).await {
            // Only what the test staged: making its project stages its
            // own events (a membership's access signal) first.
            Ok(Ok(Tailed::Event(event))) if event.event_type == catalog::TASK_UPDATED.name => {
                seen.push((event.position, event.id));
            }
            Ok(Ok(Tailed::Event(_))) => {}
            Ok(Ok(Tailed::Resync)) => panic!("the tail jumped"),
            Ok(Err(error)) => panic!("tail: {error}"),
            Err(_) => panic!(
                "the tail saw {} of {} events within {within:?}",
                seen.len(),
                ids.len()
            ),
        }
    }
    seen
}

fn assert_dense(seen: &[(i64, Uuid)]) {
    for pair in seen.windows(2) {
        assert_eq!(pair[1].0, pair[0].0 + 1, "positions are dense and in order");
    }
}

#[tokio::test]
async fn positions_follow_commit_order() {
    let Some(cluster) = cluster("k5_4_commit_order", 2).await else {
        return;
    };
    let project = log_project(&cluster).await;
    let mut tails = [cluster.instance(0).tail(), cluster.instance(1).tail()];
    let mut first = cluster.pool.begin().await.expect("first");
    let early = stage(&mut first, project).await;
    let mut second = cluster.pool.begin().await.expect("second");
    let late = stage(&mut second, project).await;
    second.commit().await.expect("second commits first");
    // The reader sees the second as soon as it commits; the first, still
    // open, isn't there yet, and comes after it once it commits.
    let mut seen_by_a = tail_until(
        &mut tails[0],
        &HashSet::from([late]),
        Duration::from_secs(5),
    )
    .await;
    first.commit().await.expect("first commits last");
    seen_by_a.extend(
        tail_until(
            &mut tails[0],
            &HashSet::from([early]),
            Duration::from_secs(5),
        )
        .await,
    );
    let ids = HashSet::from([early, late]);
    let seen_by_b = tail_until(&mut tails[1], &ids, Duration::from_secs(5)).await;
    for seen in [seen_by_a, seen_by_b] {
        let position = |id: Uuid| seen.iter().find(|(_, seen)| *seen == id).expect("seen").0;
        assert!(position(late) < position(early), "{seen:?}");
        assert_dense(&seen);
    }
    cluster.finish().await;
}

#[tokio::test]
async fn concurrent_writers_leave_no_gaps() {
    let Some(cluster) = cluster("k5_4_no_gaps", 2).await else {
        return;
    };
    let project = log_project(&cluster).await;
    let mut tails = [cluster.instance(0).tail(), cluster.instance(1).tail()];
    let writers = (0..8).map(|writer| {
        // Half write through A's pool, half through B's.
        let pool = cluster.instance(writer % 2).state().pool.clone();
        async move {
            let mut ids = Vec::new();
            for _ in 0..100 {
                let mut transaction = pool.begin().await.expect("begin");
                ids.push(stage(&mut transaction, project).await);
                transaction.commit().await.expect("commit");
            }
            ids
        }
    });
    let written = join_all(writers)
        .await
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    assert_eq!(written.len(), 800);
    for tail in &mut tails {
        let seen = tail_until(tail, &written, Duration::from_secs(30)).await;
        // The tail may also hand on the log project's own `space.created`,
        // logged before the writers start; each written event comes once.
        let ours = seen
            .iter()
            .filter(|(_, id)| written.contains(id))
            .collect::<Vec<_>>();
        assert_eq!(ours.len(), written.len(), "each event once");
        assert_dense(&seen);
    }
    cluster.finish().await;
}

#[tokio::test]
async fn tail_survives_losing_the_listener() {
    if crate::cluster_support::pooler_run() {
        eprintln!("this test is about the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_4_listener_lost", 1).await else {
        return;
    };
    let project = log_project(&cluster).await;
    let a = cluster.instance(0);
    let name = format!("aw-listen-{}", a.cluster().instance_id().0.simple());
    let listening = |pool: sqlx::PgPool, name: String| async move {
        sqlx::query_scalar::<_, i32>("SELECT pid FROM pg_stat_activity WHERE application_name = $1")
            .bind(name)
            .fetch_optional(&pool)
            .await
            .expect("activity")
    };
    let started = Instant::now();
    let old = loop {
        if let Some(pid) = listening(cluster.pool.clone(), name.clone()).await {
            break pid;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "no listener");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(old)
        .execute(&cluster.pool)
        .await
        .expect("terminate the listener");
    let mut tail = a.tail();
    let mut transaction = cluster.pool.begin().await.expect("begin");
    let id = stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");
    tail_until(&mut tail, &HashSet::from([id]), Duration::from_secs(1)).await;
    let started = Instant::now();
    loop {
        match listening(cluster.pool.clone(), name.clone()).await {
            Some(pid) if pid != old => break,
            _ => {}
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the listener never came back"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cluster.finish().await;
}

#[tokio::test]
async fn events_arrive_with_listen_off() {
    let Some(cluster) = cluster_with(
        "k5_4_listen_off",
        2,
        &[("AGENT_WORKSPACE_DATABASE_LISTEN_URL", "off")],
    )
    .await
    else {
        return;
    };
    let project = log_project(&cluster).await;
    let mut tails = [cluster.instance(0).tail(), cluster.instance(1).tail()];
    let mut transaction = cluster.pool.begin().await.expect("begin");
    let id = stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");
    for tail in &mut tails {
        // A sequencer poll and a tail poll, with room for a slow machine.
        tail_until(tail, &HashSet::from([id]), Duration::from_secs(2)).await;
    }
    cluster.finish().await;
}

/// A run's state and how many times it was recovered.
async fn recovered(pool: &sqlx::PgPool, run: Uuid) -> (String, i64) {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT state, (SELECT count(*) FROM run_activities \
                        WHERE run_id = run.id AND payload->>'step' = 'recovery') \
         FROM agent_runs run WHERE id = $1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("run")
}

async fn until_host(instance: &crate::cluster_support::Instance) {
    let started = Instant::now();
    while !instance.cluster().holds(&LeaseName::BuiltinHost) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{} never took the host",
            instance.name
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn until_settled(pool: &sqlx::PgPool, run: Uuid) -> (String, i64) {
    let started = Instant::now();
    loop {
        let (state, count) = recovered(pool, run).await;
        if !matches!(state.as_str(), "working" | "recovering") {
            return (state, count);
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "run {run} was never recovered"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// K5.3: only the built-in host's holder recovers runs. A second instance
/// starting leaves live runs alone; when the holder dies, the next holder
/// recovers each run once.
#[tokio::test]
async fn runs_are_recovered_once_by_the_host() {
    let Some(mut cluster) = cluster_with("k5_3_recovery", 0, SHORT_LEASES).await else {
        return;
    };
    let root = tempfile::tempdir().expect("data root");
    let seed = crate::a1_runs_record::seed(&cluster.pool, &root).await;
    let left_over = crate::a1_runs_record::insert_run(
        &cluster.pool,
        &seed,
        "working",
        None,
        "From before the start",
    )
    .await;

    // The first holder recovers what was left over, once.
    cluster.start(SHORT_LEASES).await;
    until_host(cluster.instance(0)).await;
    let (_, count) = until_settled(&cluster.pool, left_over).await;
    assert_eq!(count, 1);

    // A run the holder is running; a second instance starts and leaves it.
    let live =
        crate::a1_runs_record::insert_run(&cluster.pool, &seed, "working", None, "Running on A")
            .await;
    cluster.start(SHORT_LEASES).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!cluster.instance(1).cluster().holds(&LeaseName::BuiltinHost));
    assert_eq!(
        recovered(&cluster.pool, live).await,
        ("working".to_owned(), 0),
        "B recovered a run A holds"
    );

    // The holder dies: B takes the host and recovers the run, once.
    cluster.instance_mut(0).abort();
    until_host(cluster.instance(1)).await;
    let (state, count) = until_settled(&cluster.pool, live).await;
    assert_eq!(count, 1, "{state}");
    assert_eq!(recovered(&cluster.pool, left_over).await.1, 1);
    cluster.finish().await;
}

/// K5.5: the next JSON frame on `socket` within `within`; None when the
/// socket closed or nothing came.
async fn next_frame(socket: &mut crate::cluster_support::Socket, within: Duration) -> Frame {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, socket.next()).await {
            Err(_) => return Frame::Nothing,
            Ok(Some(Ok(Message::Text(text)))) => {
                return Frame::Json(serde_json::from_str(&text).expect("frame"));
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            Ok(Some(Ok(Message::Close(_))) | None | Some(Err(_))) => return Frame::Closed,
            Ok(Some(Ok(other))) => panic!("unexpected frame {other:?}"),
        }
    }
}

#[derive(Debug)]
enum Frame {
    Json(Value),
    Closed,
    Nothing,
}

/// The first frame of `kind` within `within`, skipping others.
async fn frame_of(
    socket: &mut crate::cluster_support::Socket,
    kind: &str,
    within: Duration,
) -> Option<Value> {
    let deadline = Instant::now() + within;
    loop {
        match next_frame(socket, deadline.saturating_duration_since(Instant::now())).await {
            Frame::Json(frame) if frame["type"] == kind || frame["kind"] == kind => {
                return Some(frame);
            }
            // A durable event, inside protocol 2's `event` frame.
            Frame::Json(frame) if frame["type"] == "event" && frame["event"]["type"] == kind => {
                return Some(frame["event"].clone());
            }
            Frame::Json(_) => {}
            Frame::Closed | Frame::Nothing => return None,
        }
    }
}

/// A project WORK with a channel, made on A by the owner.
async fn work_channel(
    cluster: &crate::cluster_support::Cluster,
    browser: &crate::agent_interface::Browser,
) -> (Uuid, Uuid) {
    let a = cluster.instance(0);
    let (status, project) = a
        .send(browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/space_create", cluster.organization_id),
            Some("k5-5-project"),
            json!({"key": "WORK", "name": "Work"}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    let project = project["value"].clone();
    let project: Uuid = project["id"]
        .as_str()
        .expect("project")
        .parse()
        .expect("id");
    let (status, channel) = a
        .send(browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("k5-5-channel"),
            json!({"name": "General"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .parse()
        .expect("id");
    (project, channel)
}

async fn say(
    instance: &crate::cluster_support::Instance,
    browser: &crate::agent_interface::Browser,
    conversation: Uuid,
    key: &str,
) -> Uuid {
    let (status, sent) = instance
        .send(browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(key),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Hello"}]}}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["message"]["id"]
        .as_str()
        .expect("message")
        .parse()
        .expect("id")
}

/// A person in the cluster's workspace and `project`, signed in on A.
async fn add_person(
    cluster: &crate::cluster_support::Cluster,
    project: Uuid,
    login: &str,
) -> (Uuid, crate::agent_interface::Browser) {
    use agent_workspace_server::bootstrap::{BootstrapCommand, bootstrap_development_member};
    let pool = &cluster.pool;
    let actor = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) \
         VALUES ($1, $2, 'member')",
    )
    .bind(cluster.organization_id)
    .bind(actor)
    .execute(pool)
    .await
    .expect("workspace membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'member', $4, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(project)
    .bind(actor)
    .bind(login)
    .execute(pool)
    .await
    .expect("project membership");
    bootstrap_development_member(
        pool,
        BootstrapCommand {
            login_name: login.to_owned(),
            display_name: login.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor),
            organization_id: Some(cluster.organization_id),
        },
        crate::agent_interface::PASSWORD.to_owned(),
    )
    .await
    .expect("person");
    (actor, cluster.instance(0).login(login).await)
}

#[tokio::test]
async fn message_sent_on_one_instance_reaches_a_browser_on_the_other() {
    let Some(cluster) = cluster("k5_5_message", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let (_, channel) = work_channel(&cluster, &browser).await;
    let mut on_b = cluster.instance(1).socket(&browser).await;
    let message = say(cluster.instance(0), &browser, channel, "k5-5-hello").await;
    let frame = frame_of(
        &mut on_b,
        "conversation.message.created",
        Duration::from_secs(2),
    )
    .await
    .expect("the message reached B's socket within 2 s");
    assert_eq!(frame["conversationId"], channel.to_string(), "{frame}");
    assert_eq!(
        frame["payload"]["messageId"],
        message.to_string(),
        "{frame}"
    );
    cluster.finish().await;
}

#[tokio::test]
async fn private_conversation_events_stay_private_on_both() {
    let Some(cluster) = cluster("k5_5_private", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let (_, rook) = add_person(&cluster, project, "rook").await;
    // A person joins the workspace's direct space on their first visit.
    for browser in [&owner, &mara_browser] {
        let (status, body) = cluster
            .instance(0)
            .send(browser.request("GET", "/api/v1/direct", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let mut sockets = [
        cluster.instance(0).socket(&rook).await,
        cluster.instance(1).socket(&rook).await,
    ];
    let (status, group) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v1/direct/groups",
            Some("k5-5-group"),
            json!({"memberIds": [mara]}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let secret: Uuid = group.parse().expect("id");
    say(cluster.instance(0), &owner, secret, "k5-5-secret").await;
    // Then something Rook may see, so each socket has something to wait for.
    say(cluster.instance(0), &owner, channel, "k5-5-public").await;
    for socket in &mut sockets {
        let mut heard_public = false;
        while let Frame::Json(frame) = next_frame(socket, Duration::from_secs(2)).await {
            assert!(!frame.to_string().contains(&group), "Rook heard {frame}");
            if frame["event"]["type"] == "conversation.message.created"
                && frame["event"]["conversationId"] == channel.to_string()
            {
                heard_public = true;
                break;
            }
        }
        assert!(heard_public, "the channel's message never came");
    }
    cluster.finish().await;
}

#[tokio::test]
async fn logout_on_one_instance_closes_the_socket_on_the_other() {
    if crate::cluster_support::pooler_run() {
        eprintln!("this frame crosses instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_5_logout", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let mut on_b = cluster.instance(1).socket(&browser).await;
    let (status, body) = cluster
        .instance(0)
        .send(browser.request("POST", "/api/v1/auth/logout", None, Value::Null))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // Within a second, not at the 15 s revalidation.
    match next_frame(&mut on_b, Duration::from_secs(1)).await {
        Frame::Closed => {}
        other => panic!("B's socket stayed open: {other:?}"),
    }
    cluster.finish().await;
}

#[tokio::test]
async fn streamed_text_crosses_instances() {
    if crate::cluster_support::pooler_run() {
        eprintln!("this frame crosses instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_5_stream", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &browser).await;
    let mut on_b = cluster.instance(1).socket(&browser).await;
    let message = Uuid::now_v7();
    agent_workspace_server::realtime::ephemeral::stream_delta(
        cluster.instance(0).state(),
        agent_workspace_server::realtime::RealtimeEvent {
            event_id: Uuid::now_v7(),
            kind: "message.delta",
            project_id: project,
            conversation_id: Some(channel),
            payload: json!({"message_id": message, "agent_run_id": Uuid::now_v7(), "delta": "Hel"}),
        },
    )
    .await;
    let frame = frame_of(&mut on_b, "stream.delta", Duration::from_secs(2))
        .await
        .expect("the delta reached B's socket");
    assert_eq!(frame["messageId"], message.to_string(), "{frame}");
    assert_eq!(frame["delta"], "Hel", "{frame}");
    cluster.finish().await;
}

#[tokio::test]
async fn module_switch_is_seen_everywhere() {
    if crate::cluster_support::pooler_run() {
        eprintln!("this frame crosses instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_5_modules", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let (project, _) = work_channel(&cluster, &browser).await;
    let tasks = format!("/api/v1/projects/{project}/work-items");
    let b = cluster.instance(1);
    // B reads (and keeps) the switches first.
    let (status, body) = b
        .send(browser.request("GET", &tasks, None, Value::Null))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = cluster
        .instance(0)
        .send(browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/workspace_update", cluster.organization_id),
            Some("tasks-off"),
            json!({"projectId": "workspace", "modules": {"tasks": false}}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Well inside the 10 s the gate keeps switches for.
    let switched = Instant::now();
    loop {
        let (status, body) = b
            .send(browser.request("GET", &tasks, None, Value::Null))
            .await;
        if status == StatusCode::FORBIDDEN {
            assert_eq!(body["error"]["code"], "module_off", "{body}");
            break;
        }
        assert!(
            switched.elapsed() < Duration::from_secs(2),
            "B still serves Tasks: {status} {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cluster.finish().await;
}

// ---------------------------------------------------------------------------
// K5.6 shared limits
// ---------------------------------------------------------------------------

/// One sign-in for `login` with a wrong password: its status and its
/// Retry-After, if any.
async fn wrong_sign_in(
    instance: &crate::cluster_support::Instance,
    login: &str,
) -> (StatusCode, Option<u64>) {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("origin", "http://127.0.0.1:3100")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({"loginName": login, "password": "not the password"}))
                .expect("body"),
        ))
        .expect("request");
    let (status, headers, _) = instance.send_raw(request).await;
    let retry_after = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok()?.parse().ok());
    (status, retry_after)
}

#[tokio::test]
async fn login_limit_holds_across_instances() {
    let Some(cluster) = cluster("k5_6_login", 2).await else {
        return;
    };
    let mut reached = 0;
    for attempt in 0..10 {
        let (status, _) = wrong_sign_in(cluster.instance(attempt % 2), "owner").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {attempt}");
        reached += 1;
    }
    assert_eq!(reached, 10);
    let (status, retry_after) = wrong_sign_in(cluster.instance(0), "owner").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        retry_after.is_some_and(|seconds| (1..=60).contains(&seconds)),
        "{retry_after:?}"
    );
    cluster.finish().await;
}

#[tokio::test]
async fn write_budget_is_never_exceeded() {
    let Some(cluster) = cluster("k5_6_writes", 2).await else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let (_, channel) = work_channel(&cluster, &browser).await;
    // The write bound is what's tested, not a workspace's message rate.
    crate::support::db::lift_message_rate(&cluster.pool, &channel.to_string()).await;
    let started = Instant::now();
    let mut accepted = 0;
    let mut refused = 0;
    for batch in 0..15 {
        let sends = (0..20).map(|index| {
            let attempt = batch * 20 + index;
            let instance = cluster.instance(attempt % 2);
            let request = browser.request(
                "POST",
                &format!("/api/v1/conversations/{channel}/messages"),
                Some(&format!("k5-6-write-{attempt}")),
                json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "Hi"}]}}),
            );
            async move { instance.send(request).await.0 }
        });
        for status in join_all(sends).await {
            match status {
                StatusCode::CREATED => accepted += 1,
                StatusCode::TOO_MANY_REQUESTS => refused += 1,
                other => panic!("unexpected {other}"),
            }
        }
    }
    // The 300 went out inside one window, or the bound says nothing.
    assert!(started.elapsed() < Duration::from_secs(55));
    // The channel set-up spent a few of the 240; at most one chunk (10) per
    // other instance is left unused.
    assert!(accepted <= 240, "{accepted} writes got through");
    assert!(accepted >= 240 - 10 - 5, "only {accepted} got through");
    assert_eq!(accepted + refused, 300);
    cluster.finish().await;
}

#[tokio::test]
async fn limits_survive_a_restart() {
    let Some(mut cluster) = cluster("k5_6_restart", 1).await else {
        return;
    };
    for attempt in 0..10 {
        let (status, _) = wrong_sign_in(cluster.instance(0), "owner").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {attempt}");
    }
    cluster.instance_mut(0).stop().await;
    cluster.start(&[]).await;
    let (status, _) = wrong_sign_in(cluster.instance(1), "owner").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    cluster.finish().await;
}

// ---------------------------------------------------------------------------
// K5.17 pooler-safe database use
// ---------------------------------------------------------------------------

/// Two instances configured as a transaction-mode pooler needs (no kept
/// statements, no listener) still sign in, write, read across, hear the
/// log on a socket, and hand a lease over.
#[tokio::test]
async fn the_cluster_works_as_a_pooler_needs() {
    let Some(mut cluster) = cluster_with("k5_17_pooler", 2, &crate::cluster_support::POOLER).await
    else {
        return;
    };
    let browser = cluster.instance(0).login("owner").await;
    let (_, channel) = work_channel(&cluster, &browser).await;
    let mut on_b = cluster.instance(1).socket(&browser).await;
    let message = say(cluster.instance(0), &browser, channel, "k5-17-hello").await;
    let frame = frame_of(
        &mut on_b,
        "conversation.message.created",
        Duration::from_secs(5),
    )
    .await
    .expect("the message reached B's socket on polls");
    assert_eq!(
        frame["payload"]["messageId"],
        message.to_string(),
        "{frame}"
    );
    let (status, body) = cluster
        .instance(1)
        .send(browser.request(
            "GET",
            &format!("/api/v1/conversations/{channel}/timeline"),
            None,
            Value::Null,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains(&message.to_string()), "{body}");
    let started = Instant::now();
    let holder = loop {
        if let Some(holder) = (0..2).find(|index| {
            cluster
                .instance(*index)
                .cluster()
                .holds(&LeaseName::BuiltinHost)
        }) {
            break holder;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "nobody took the host"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    cluster.instance_mut(holder).stop().await;
    until_host(cluster.instance(1 - holder)).await;
    cluster.finish().await;
}

/// K5.7: a stopping instance says it isn't ready at once, closes its
/// sockets with 1012 so they reconnect to the other instance, and lets the
/// built-in host go so the other takes it at once.
#[tokio::test]
async fn draining_instance_moves_its_sockets() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};
    const DRAIN: &[(&str, &str)] = &[("AGENT_WORKSPACE_DRAIN_SECONDS", "1")];
    let Some(mut cluster) = cluster_with("k5_7_drain", 0, DRAIN).await else {
        return;
    };
    cluster.start(DRAIN).await;
    until_host(cluster.instance(0)).await;
    cluster.start(DRAIN).await;
    let (status, ready) = cluster.instance(0).ready().await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["roles"], json!(["api", "live", "work"]));
    assert_eq!(ready["draining"], false);

    let browser = cluster.instance(0).login("owner").await;
    let mut socket = cluster.instance(0).socket(&browser).await;
    let drainer = cluster.instance(0).drainer();
    let draining = tokio::spawn(async move { drainer.drain().await });

    // Not ready at once, while the other stays ready.
    let started = Instant::now();
    loop {
        let (status, ready) = cluster.instance(0).ready().await;
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(ready["status"], "draining");
            assert_eq!(ready["draining"], true);
            break;
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "A stayed ready while draining"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(cluster.instance(1).ready().await.0, StatusCode::OK);

    // After the drain time, its socket closes to restart.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(_)) => {}
                other => panic!("the socket ended without a close frame: {other:?}"),
            }
        }
    })
    .await
    .expect("A's socket was never closed");
    let frame = closed.expect("a close frame with a code");
    assert_eq!(frame.code, CloseCode::Restart);
    assert_eq!(frame.reason.as_str(), "server_draining");
    draining.await.expect("drain");
    // The browser reconnects to the other instance.
    let _on_b = cluster.instance(1).socket(&browser).await;

    // Stopping lets the host go; B takes it without waiting for it to run
    // out (the release wakes it, except in the run without a listener).
    cluster.instance_mut(0).stop().await;
    let stopped = Instant::now();
    until_host(cluster.instance(1)).await;
    if !crate::cluster_support::pooler_run() {
        assert!(
            stopped.elapsed() < Duration::from_secs(2),
            "B took {:?} to take the host",
            stopped.elapsed()
        );
    }
    cluster.finish().await;
}

/// K5.7: an instance with only `work` serves health checks, nothing else.
#[tokio::test]
async fn work_only_instance_serves_no_app_routes() {
    let Some(mut cluster) = cluster("k5_7_work_only", 1).await else {
        return;
    };
    cluster.start(&[("AGENT_WORKSPACE_ROLES", "work")]).await;
    let work = cluster.instance(1);
    let (status, ready) = work.ready().await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["roles"], json!(["work"]));
    let browser = cluster.instance(0).login("owner").await;
    for path in ["/api/v1/auth/session", "/api/v1/realtime", "/api/v1/mcp"] {
        let (status, _) = work
            .send(browser.request("GET", path, None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    cluster.finish().await;
}

/// K5.7: an instance without `work` runs no jobs and takes no work leases;
/// a queued job waits for an instance that has it.
#[tokio::test]
async fn api_only_instance_runs_no_jobs() {
    let api: &[(&str, &str)] = &[SHORT_LEASES[0], ("AGENT_WORKSPACE_ROLES", "api,live")];
    let Some(mut cluster) = cluster_with("k5_7_api_only", 0, SHORT_LEASES).await else {
        return;
    };
    cluster.start(api).await;
    let job = match agent_workspace_server::jobs::enqueue(
        &cluster.pool,
        agent_workspace_server::jobs::NewJob::new("jobs.prune_finished"),
    )
    .await
    .expect("enqueue")
    {
        agent_workspace_server::jobs::Enqueued::Created(id) => id,
        agent_workspace_server::jobs::Enqueued::Duplicate => panic!("no other job"),
    };
    let state = async |pool: &sqlx::PgPool| {
        sqlx::query_scalar::<_, String>("SELECT state FROM background_jobs WHERE id = $1")
            .bind(job)
            .fetch_one(pool)
            .await
            .expect("job")
    };
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(state(&cluster.pool).await, "queued");
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM leases WHERE holder_instance_id = $1 AND expires_at > now()",
    )
    .bind(cluster.instance(0).cluster().instance_id().0)
    .fetch_one(&cluster.pool)
    .await
    .expect("leases");
    assert_eq!(held, 0, "an api instance took a lease");
    // A run started through it wakes the lease holder rather than being
    // admitted here: it knows its cluster though it isn't the host.
    let known = cluster
        .instance(0)
        .state()
        .supervisors
        .cluster()
        .map(|known| known.instance_id().0);
    assert_eq!(known, Some(cluster.instance(0).cluster().instance_id().0));
    // It still serves the app.
    let browser = cluster.instance(0).login("owner").await;
    let (status, _) = cluster
        .instance(0)
        .send(browser.request("GET", "/api/v1/auth/session", None, Value::Null))
        .await;
    assert_eq!(status, StatusCode::OK);

    cluster
        .start(&[SHORT_LEASES[0], ("AGENT_WORKSPACE_ROLES", "work")])
        .await;
    let started = Instant::now();
    while state(&cluster.pool).await != "succeeded" {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the work instance never ran the job"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cluster.finish().await;
}

/// K5.7 (from #283's review): recovery's later writes, after its slow git
/// and provider inspection, are fenced by the host lease too, so a server
/// that lost the host can't overwrite what the new holder recovers.
#[tokio::test]
async fn recovery_writes_nothing_once_the_host_is_lost() {
    let Some(mut cluster) = cluster_with("k5_7_recovery_fence", 0, SHORT_LEASES).await else {
        return;
    };
    cluster.start(SHORT_LEASES).await;
    until_host(cluster.instance(0)).await;
    let lease = cluster
        .instance(0)
        .cluster()
        .held_lease(&LeaseName::BuiltinHost)
        .expect("A holds the host");
    let old_holder = cluster.instance(0).cluster().clone();
    let state = cluster.instance(0).state().clone();
    // A stalls (as a crash would look from outside); B takes the host.
    cluster.instance_mut(0).abort();
    cluster.start(SHORT_LEASES).await;
    until_host(cluster.instance(1)).await;
    // With no run to recover, only the later writes are left: each refuses.
    let result =
        agent_workspace_server::recovery::reconcile_owned(&state, &old_holder, &lease).await;
    assert!(matches!(result, Err(AppError::LeaseLost)), "{result:?}");
    cluster.finish().await;
}

/// A second project with a channel, made on A by the owner, with `people`
/// added as members.
async fn side_channel(
    cluster: &crate::cluster_support::Cluster,
    browser: &crate::agent_interface::Browser,
    people: &[Uuid],
) -> (Uuid, Uuid) {
    let a = cluster.instance(0);
    let (status, project) = a
        .send(browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/space_create", cluster.organization_id),
            Some("k5-8-side"),
            json!({"key": "SIDE", "name": "Side"}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    let project = project["value"].clone();
    let project: Uuid = project["id"]
        .as_str()
        .expect("project")
        .parse()
        .expect("id");
    for person in people {
        sqlx::query(
            "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
             SELECT $1, $2, actor_id, 'member', display_name, handle FROM project_memberships \
             WHERE actor_id = $3 LIMIT 1",
        )
        .bind(Uuid::now_v7())
        .bind(project)
        .bind(person)
        .execute(&cluster.pool)
        .await
        .expect("side membership");
    }
    let (status, channel) = a
        .send(browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/channels"),
            Some("k5-8-side-channel"),
            json!({"name": "Side"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel = channel["conversation"]["id"]
        .as_str()
        .expect("channel")
        .parse()
        .expect("id");
    (project, channel)
}

/// Messages a socket hears, by conversation, until one in `last` arrives.
async fn heard_until(socket: &mut crate::cluster_support::Socket, last: Uuid) -> Vec<Value> {
    let mut heard = Vec::new();
    loop {
        match next_frame(socket, Duration::from_secs(3)).await {
            Frame::Json(frame) => {
                let event = if frame["type"] == "event" {
                    frame["event"].clone()
                } else {
                    frame.clone()
                };
                let done = event["type"] == "conversation.message.created"
                    && event["conversationId"] == last.to_string();
                heard.push(frame);
                if done {
                    return heard;
                }
            }
            other => panic!("{other:?} before the message in {last}; heard {heard:?}"),
        }
    }
}

/// K5.8: access is decided at subscribe time and again when it changes. A
/// member taken out of a project through one instance hears nothing more
/// from it on the other, and still hears the projects they are in.
#[tokio::test]
async fn removed_member_stops_hearing_at_once() {
    let Some(cluster) = cluster("k5_8_removed", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let (_, side) = side_channel(&cluster, &owner, &[mara]).await;
    let mut on_b = cluster.instance(1).socket(&mara_browser).await;
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(mara)
    .fetch_one(&cluster.pool)
    .await
    .expect("version");
    let (status, body) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v2/ops/space_member_remove",
            Some("k5-8-disable"),
            json!({"spaceId": project, "account": mara, "expectedVersion": version}),
        ))
        .await;
    assert!(status.is_success(), "{status} {body}");
    say(cluster.instance(0), &owner, channel, "k5-8-after").await;
    say(cluster.instance(0), &owner, side, "k5-8-side").await;
    let heard = heard_until(&mut on_b, side).await;
    assert!(
        !heard
            .iter()
            .any(|frame| frame["event"]["conversationId"] == channel.to_string()),
        "Mara heard the project she left: {heard:?}"
    );
    cluster.finish().await;
}

/// K5.8: a private group made after a socket opened is heard by its
/// members at once, and by nobody else.
#[tokio::test]
async fn added_to_private_conversation_starts_hearing() {
    let Some(cluster) = cluster("k5_8_added", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let (_, rook) = add_person(&cluster, project, "rook").await;
    for browser in [&owner, &mara_browser] {
        let (status, body) = cluster
            .instance(0)
            .send(browser.request("GET", "/api/v1/direct", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let mut mara_on_b = cluster.instance(1).socket(&mara_browser).await;
    let mut rook_on_b = cluster.instance(1).socket(&rook).await;
    let (status, group) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v1/direct/groups",
            Some("k5-8-group"),
            json!({"memberIds": [mara]}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group: Uuid = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .parse()
        .expect("id");
    say(cluster.instance(0), &owner, group, "k5-8-in-group").await;
    say(cluster.instance(0), &owner, channel, "k5-8-in-channel").await;
    let heard = heard_until(&mut mara_on_b, channel).await;
    assert!(
        heard.iter().any(|frame| {
            frame["event"]["type"] == "conversation.message.created"
                && frame["event"]["conversationId"] == group.to_string()
        }),
        "Mara never heard the group she was added to: {heard:?}"
    );
    let heard = heard_until(&mut rook_on_b, channel).await;
    assert!(
        !heard
            .iter()
            .any(|frame| frame.to_string().contains(&group.to_string())),
        "Rook heard {heard:?}"
    );
    cluster.finish().await;
}

/// The `ready` frame a protocol 2 socket gets, skipping what comes first.
async fn ready_of(socket: &mut crate::cluster_support::Socket) -> (Vec<Value>, Value) {
    let mut before = Vec::new();
    loop {
        match next_frame(socket, Duration::from_secs(3)).await {
            Frame::Json(frame) if frame["type"] == "ready" => return (before, frame),
            Frame::Json(frame) => before.push(frame),
            other => panic!("{other:?} before ready; got {before:?}"),
        }
    }
}

/// K5.9: a protocol 2 socket that comes back with where it stopped is sent
/// exactly what it missed and may see, in order, then `ready` with
/// `resumed`, on whichever instance it reaches.
#[tokio::test]
async fn resume_replays_what_was_missed() {
    let Some(cluster) = cluster("k5_9_resume", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let (_, rook) = add_person(&cluster, project, "rook").await;
    for browser in [&owner, &mara_browser] {
        let (status, body) = cluster
            .instance(0)
            .send(browser.request("GET", "/api/v1/direct", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let mut first = cluster.instance(1).socket_v2(&rook, None).await;
    let (_, ready) = ready_of(&mut first).await;
    assert_eq!(ready["resumed"], false, "{ready}");
    let stopped: i64 = ready["position"]
        .as_str()
        .expect("positions are strings")
        .parse()
        .expect("position");
    drop(first);

    // While Rook is away: a group he isn't in, and three channel messages.
    let (status, group) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v1/direct/groups",
            Some("k5-9-group"),
            json!({"memberIds": [mara]}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    say(
        cluster.instance(0),
        &owner,
        group.parse().expect("id"),
        "k5-9-secret",
    )
    .await;
    let mut missed = Vec::new();
    for key in ["k5-9-one", "k5-9-two", "k5-9-three"] {
        missed.push(
            say(cluster.instance(0), &owner, channel, key)
                .await
                .to_string(),
        );
    }

    // Missed means in the log: a message still being sequenced comes live.
    cluster.sequenced().await;
    let mut back = cluster.instance(0).socket_v2(&rook, Some(stopped)).await;
    let (replayed, ready) = ready_of(&mut back).await;
    assert_eq!(ready["resumed"], true, "{ready}");
    assert_eq!(
        ready["replayed"].as_u64(),
        Some(
            replayed
                .iter()
                .filter(|frame| frame["type"] == "event")
                .count() as u64
        ),
        "{ready}"
    );
    let messages = replayed
        .iter()
        .filter(|frame| frame["event"]["type"] == "conversation.message.created")
        .map(|frame| {
            frame["event"]["payload"]["messageId"]
                .as_str()
                .expect("message")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(messages, missed, "exactly the missed messages, in order");
    assert!(
        !replayed
            .iter()
            .any(|frame| frame.to_string().contains(&group)),
        "the group's events stay private on replay: {replayed:?}"
    );
    let mut positions = replayed
        .iter()
        .filter(|frame| frame["type"] == "event")
        .map(|frame| {
            frame["position"]
                .as_str()
                .expect("position")
                .parse::<i64>()
                .expect("n")
        });
    let mut previous = stopped;
    assert!(
        positions.all(|position| {
            let ordered = position > previous;
            previous = position;
            ordered
        }),
        "in position order, after where it stopped"
    );
    // Then live, with nothing repeated.
    let live = say(cluster.instance(0), &owner, channel, "k5-9-live").await;
    let heard = heard_until(&mut back, channel).await;
    let messages = heard
        .iter()
        .filter(|frame| frame["event"]["type"] == "conversation.message.created")
        .map(|frame| frame["event"]["payload"]["messageId"].clone())
        .collect::<Vec<_>>();
    assert_eq!(messages, vec![json!(live.to_string())], "{heard:?}");

    // A position the server can't resume from (ahead of the log) asks the
    // client to refetch.
    let ahead = ready["position"]
        .as_str()
        .expect("p")
        .parse::<i64>()
        .expect("n")
        + 1_000;
    let mut lost = cluster.instance(1).socket_v2(&rook, Some(ahead)).await;
    let (replayed, ready) = ready_of(&mut lost).await;
    assert_eq!(ready["resumed"], false, "{ready}");
    assert!(replayed.is_empty(), "{replayed:?}");
    cluster.finish().await;
}

/// The next protocol 2 ephemeral frame of `kind`, or None within `within`.
async fn ephemeral_of(
    socket: &mut crate::cluster_support::Socket,
    kind: &str,
    within: Duration,
) -> Option<Value> {
    let deadline = Instant::now() + within;
    loop {
        match next_frame(socket, deadline.saturating_duration_since(Instant::now())).await {
            Frame::Json(frame) if frame["type"] == "ephemeral" && frame["kind"] == kind => {
                return Some(frame);
            }
            Frame::Json(_) => {}
            Frame::Closed | Frame::Nothing => return None,
        }
    }
}

async fn send_frame(socket: &mut crate::cluster_support::Socket, frame: Value) {
    use futures_util::SinkExt;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            frame.to_string().into(),
        ))
        .await
        .expect("client frame");
}

/// K5.10: typing on one instance shows on the other, only to those who can
/// see the conversation.
#[tokio::test]
async fn typing_on_one_instance_shows_on_the_other() {
    if crate::cluster_support::pooler_run() {
        eprintln!("typing crosses instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_10_typing", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, channel) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let (_, rook) = add_person(&cluster, project, "rook").await;
    for browser in [&owner, &mara_browser] {
        let (status, body) = cluster
            .instance(0)
            .send(browser.request("GET", "/api/v1/direct", None, Value::Null))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, group) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v1/direct/groups",
            Some("k5-10-group"),
            json!({"memberIds": [mara]}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group = group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let mut typist = cluster.instance(0).socket_v2(&owner, None).await;
    ready_of(&mut typist).await;
    let mut mara_on_b = cluster.instance(1).socket_v2(&mara_browser, None).await;
    ready_of(&mut mara_on_b).await;
    let mut rook_on_b = cluster.instance(1).socket_v2(&rook, None).await;
    ready_of(&mut rook_on_b).await;

    send_frame(
        &mut typist,
        json!({"type": "typing", "conversationId": group}),
    )
    .await;
    let typing = ephemeral_of(&mut mara_on_b, "typing", Duration::from_secs(3))
        .await
        .expect("Mara saw the owner typing in their group");
    assert_eq!(typing["conversationId"], group.as_str(), "{typing}");
    assert!(typing["until"].is_string(), "{typing}");
    // Rook sees typing in the channel (after the rate limit), never the group.
    tokio::time::sleep(Duration::from_millis(3_100)).await;
    send_frame(
        &mut typist,
        json!({"type": "typing", "conversationId": channel.to_string()}),
    )
    .await;
    let seen = ephemeral_of(&mut rook_on_b, "typing", Duration::from_secs(3))
        .await
        .expect("Rook saw typing in the channel");
    assert_eq!(
        seen["conversationId"],
        channel.to_string(),
        "Rook saw {seen}"
    );
    cluster.finish().await;
}

/// K5.10: someone is online while any instance holds one of their
/// connections, and their workspace hears when that changes; a connection
/// left behind by a stopped instance stops counting once its heartbeat
/// lapses.
#[tokio::test]
async fn presence_follows_connections_across_instances() {
    if crate::cluster_support::pooler_run() {
        eprintln!("presence frames cross instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_10_presence", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, _) = work_channel(&cluster, &owner).await;
    let (mara, mara_browser) = add_person(&cluster, project, "mara").await;
    let owner_id =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'owner'")
            .fetch_one(&cluster.pool)
            .await
            .expect("owner");
    let online = |instance: usize| {
        let pool = cluster.instance(instance).state().pool.clone();
        let workspace = cluster.organization_id;
        async move {
            crate::support::as_job(
                workspace,
                agent_workspace_server::realtime::presence::online_in(&pool, workspace),
            )
            .await
            .expect("online")
        }
    };
    let mut watcher = cluster.instance(1).socket_v2(&mara_browser, None).await;
    ready_of(&mut watcher).await;
    let mut on_a = cluster.instance(0).socket_v2(&owner, None).await;
    ready_of(&mut on_a).await;
    let came = ephemeral_of(&mut watcher, "presence", Duration::from_secs(3))
        .await
        .expect("Mara heard the owner come online");
    assert_eq!(came["accountId"], owner_id.to_string(), "{came}");
    assert_eq!(came["online"], true, "{came}");
    assert!(online(1).await.contains(&owner_id), "counted on B too");
    assert!(online(0).await.contains(&mara));

    drop(on_a);
    let went = ephemeral_of(&mut watcher, "presence", Duration::from_secs(3))
        .await
        .expect("Mara heard the owner go offline");
    assert_eq!(went["online"], false, "{went}");
    assert!(!online(1).await.contains(&owner_id));

    // A row an aborted instance never closed, its heartbeat lapsed.
    sqlx::query(
        "INSERT INTO realtime_connections (id, account_id, instance_id, opened_at, seen_at) \
         VALUES ($1, $2, $3, now() - interval '5 minutes', now() - interval '2 minutes')",
    )
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(Uuid::now_v7())
    .execute(&cluster.pool)
    .await
    .expect("stale row");
    assert!(
        !online(0).await.contains(&owner_id),
        "a lapsed heartbeat isn't online"
    );
    cluster.finish().await;
}

/// K5.10: a live document's change reaches the connections watching it,
/// and no one else: not a connection that isn't watching, and not one
/// that asked to watch a document it can't see.
#[tokio::test]
async fn document_update_reaches_watchers_only() {
    if crate::cluster_support::pooler_run() {
        eprintln!("document changes cross instances on the listener, off in the pooler run");
        return;
    }
    let Some(cluster) = cluster("k5_10_documents", 2).await else {
        return;
    };
    let owner = cluster.instance(0).login("owner").await;
    let (project, _) = work_channel(&cluster, &owner).await;
    let (mara_id, mara) = add_person(&cluster, project, "mara").await;
    let (_, rook) = add_person(&cluster, project, "rook").await;
    let (side, _) = side_channel(&cluster, &owner, &[]).await;
    let mut pages = Vec::new();
    for (space, title) in [(project, "Plan"), (side, "Side plan")] {
        let (status, page) = cluster
            .instance(0)
            .send(owner.request(
                "POST",
                &format!("/api/v1/projects/{space}/pages"),
                None,
                json!({"title": title, "body": ""}),
            ))
            .await;
        assert!(status.is_success(), "{page}");
        pages.push(
            page["id"]
                .as_str()
                .expect("page")
                .parse::<Uuid>()
                .expect("id"),
        );
    }
    let (plan, side_plan) = (pages[0], pages[1]);
    let watch = |id: Uuid| json!({"type": "watch", "document": {"kind": "page", "id": id}});
    let mut watcher = cluster.instance(1).socket_v2(&mara, None).await;
    ready_of(&mut watcher).await;
    send_frame(&mut watcher, watch(plan)).await;
    let mut idle = cluster.instance(1).socket_v2(&rook, None).await;
    ready_of(&mut idle).await;
    // Rook isn't in SIDE: watching its page is refused.
    let mut prying = cluster.instance(0).socket_v2(&rook, None).await;
    ready_of(&mut prying).await;
    send_frame(&mut prying, watch(side_plan)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    use agent_workspace_server::realtime::{
        documents::{DocumentKind, DocumentRef},
        ephemeral::{EphemeralFrame, publish},
    };
    for (id, update) in [(side_plan, "U0lERQ=="), (plan, "UExBTg==")] {
        publish(
            cluster.instance(0).state(),
            EphemeralFrame::DocumentUpdate {
                document: DocumentRef {
                    kind: DocumentKind::Page,
                    id,
                },
                generation: Uuid::now_v7(),
                version: 2,
                update: update.to_owned(),
            },
        )
        .await;
    }
    let heard = ephemeral_of(&mut watcher, "document.update", Duration::from_secs(3))
        .await
        .expect("the watcher heard the change");
    assert_eq!(heard["document"]["id"], plan.to_string(), "{heard}");
    assert_eq!(heard["update"], "UExBTg==", "{heard}");
    for socket in [&mut idle, &mut prying] {
        assert!(
            ephemeral_of(socket, "document.update", Duration::from_millis(800))
                .await
                .is_none(),
            "only watchers who may see the page hear it"
        );
    }
    // Mara leaves the project: the page she watched stops reaching her.
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project)
    .bind(mara_id)
    .fetch_one(&cluster.pool)
    .await
    .expect("version");
    let (status, body) = cluster
        .instance(0)
        .send(owner.request(
            "POST",
            "/api/v2/ops/space_member_remove",
            Some("k5-10-leave"),
            json!({"spaceId": project, "account": mara_id, "expectedVersion": version}),
        ))
        .await;
    assert!(status.is_success(), "{status} {body}");
    frame_of(&mut watcher, "access", Duration::from_secs(5))
        .await
        .expect("Mara's access changed");
    publish(
        cluster.instance(0).state(),
        EphemeralFrame::DocumentUpdate {
            document: DocumentRef {
                kind: DocumentKind::Page,
                id: plan,
            },
            generation: Uuid::now_v7(),
            version: 3,
            update: "QUZURVI=".to_owned(),
        },
    )
    .await;
    assert!(
        ephemeral_of(&mut watcher, "document.update", Duration::from_millis(800))
            .await
            .is_none(),
        "a watcher who left the project still hears its page"
    );
    cluster.finish().await;
}

/// K5.9 `follow`: an account's stream catches up from a position and goes
/// on live with no gap and no repeat while writers keep writing on both
/// instances; it carries only what the account may hear; a start too far
/// back asks for a resync.
#[tokio::test]
async fn follow_catches_up_then_goes_live_without_gaps() {
    use agent_workspace_server::realtime::follow::{FollowItem, FollowSpec, follow};
    use futures_util::StreamExt;

    let Some(cluster) = cluster("k5_9_follow", 2).await else {
        return;
    };
    let project = log_project(&cluster).await;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = $1",
    )
    .bind(project)
    .fetch_one(&cluster.pool)
    .await
    .expect("owner");
    // A project the owner has left: its events never reach the stream.
    let a = cluster.instance(0);
    let browser = a.login("owner").await;
    let (status, other) = a
        .send(browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/space_create", cluster.organization_id),
            Some("cluster-else"),
            json!({"key": "ELSE", "name": "Elsewhere"}),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    let other = other["value"].clone();
    let elsewhere: Uuid = other["id"].as_str().expect("project").parse().expect("id");
    sqlx::query("UPDATE project_memberships SET enabled = false WHERE project_id = $1")
        .bind(elsewhere)
        .execute(&cluster.pool)
        .await
        .expect("leave");
    let write = |instance: usize, project: Uuid, count: usize| {
        let pool = cluster.instance(instance).state().pool.clone();
        async move {
            let mut ids = Vec::new();
            for _ in 0..count {
                let mut transaction = pool.begin().await.expect("begin");
                ids.push(stage(&mut transaction, project).await);
                transaction.commit().await.expect("commit");
            }
            ids
        }
    };
    // Some before the stream starts, after the position it starts from.
    let start = {
        let mut connection = cluster.pool.acquire().await.expect("connection");
        events::head(&mut connection).await.expect("head")
    };
    let mut written = write(0, project, 50).await;
    let task_updates = std::sync::Arc::new(|event: &events::LoggedEvent| {
        event.event_type == catalog::TASK_UPDATED.name
    });
    let mut stream = Box::pin(follow(
        cluster.instance(1).state().clone(),
        FollowSpec::everything(owner, "human", task_updates.clone()),
        start,
    ));
    // The rest while it catches up, on both instances, with others' events
    // mixed in.
    let writers = join_all([
        write(0, project, 100),
        write(1, project, 100),
        write(0, elsewhere, 50),
    ]);
    let reader = async {
        let mut seen = Vec::new();
        while seen.len() < 250 {
            match tokio::time::timeout(Duration::from_secs(30), stream.next()).await {
                Ok(Some(Ok(FollowItem::Event(event)))) => {
                    assert_eq!(event.space_id, Some(project), "only its own project");
                    seen.push((event.position, event.id));
                }
                Ok(Some(Ok(FollowItem::Heartbeat | FollowItem::Live(_)))) => {}
                other => panic!("the stream stopped: {other:?} after {}", seen.len()),
            }
        }
        seen
    };
    let (more, seen) = tokio::join!(writers, reader);
    written.extend(more.into_iter().take(2).flatten());
    let ids = seen.iter().map(|(_, id)| *id).collect::<HashSet<_>>();
    assert_eq!(ids.len(), 250, "no repeats");
    assert_eq!(ids, written.into_iter().collect::<HashSet<_>>(), "no gaps");
    assert!(
        seen.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "in log order"
    );

    // Too far back: refetch, then follow from the head.
    let mut stale = Box::pin(follow(
        cluster.instance(0).state().clone(),
        FollowSpec::everything(owner, "human", task_updates),
        -1,
    ));
    match stale.next().await {
        Some(Ok(FollowItem::Resync { position })) => assert!(position > start),
        other => panic!("expected a resync: {other:?}"),
    }
    assert!(stale.next().await.is_none(), "the stream ends after it");
    cluster.finish().await;
}

/// K3.24: one cursor walks every workspace a stream asks for, in the log's
/// one order, each event naming its workspace; the direct scope's events
/// come only to a stream that asks for DMs and groups; and the log refuses
/// a direct-scope event a workspace or a space would hear.
#[tokio::test]
async fn one_stream_walks_several_workspaces_and_the_direct_scope() {
    use agent_workspace_server::{
        events::{Audience, DIRECT_SCOPE_ID},
        realtime::follow::{FollowItem, FollowSpec, follow},
        workspace::lifecycle::{self, NewWorkspace},
    };
    use futures_util::StreamExt;

    let Some(cluster) = cluster("k3_24_streams", 1).await else {
        return;
    };
    let project = log_project(&cluster).await;
    let first = cluster.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = $1",
    )
    .bind(project)
    .fetch_one(&cluster.pool)
    .await
    .expect("owner");
    let (second, third) = {
        let mut transaction = cluster.pool.begin().await.expect("begin");
        let mut made = Vec::new();
        for name in ["Second", "Third"] {
            let workspace = NewWorkspace {
                name: name.into(),
                slug: None,
                id: None,
            };
            made.push(
                lifecycle::create(&mut transaction, owner, workspace)
                    .await
                    .expect("workspace"),
            );
        }
        let (second, third) = (made[0], made[1]);
        transaction.commit().await.expect("commit");
        (second, third)
    };
    let start = {
        let mut connection = cluster.pool.acquire().await.expect("connection");
        events::head(&mut connection).await.expect("head")
    };
    let to = |workspace: Uuid, audience: Audience| NewEvent {
        workspace_id: Some(workspace),
        space_id: None,
        audience,
        ..NewEvent::thing(&catalog::TASK_UPDATED, project, "task", Uuid::now_v7())
    };
    // One of each, in one order: the first workspace's space, the second
    // and third workspaces, the owner's direct scope, then the first again.
    let mut transaction = cluster.pool.begin().await.expect("begin");
    let in_first = stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");
    let mut written = vec![in_first];
    for event in [
        to(second, Audience::Workspace),
        to(third, Audience::Workspace),
        to(DIRECT_SCOPE_ID, Audience::Account(owner)),
    ] {
        let mut transaction = cluster.pool.begin().await.expect("begin");
        written.push(events::emit(&mut transaction, event).await.expect("emit"));
        transaction.commit().await.expect("commit");
    }
    let mut transaction = cluster.pool.begin().await.expect("begin");
    written.push(stage(&mut transaction, project).await);
    transaction.commit().await.expect("commit");
    let last = written[4];

    let task_updates = std::sync::Arc::new(|event: &events::LoggedEvent| {
        event.event_type == catalog::TASK_UPDATED.name
    });
    let heard = |spec: FollowSpec| {
        let state = cluster.instance(0).state().clone();
        async move {
            let mut stream = Box::pin(follow(state, spec, start));
            let mut seen = Vec::new();
            loop {
                match tokio::time::timeout(Duration::from_secs(30), stream.next()).await {
                    Ok(Some(Ok(FollowItem::Event(event)))) => {
                        seen.push((event.position, event.id, event.workspace_id));
                        if event.id == last {
                            return seen;
                        }
                    }
                    Ok(Some(Ok(FollowItem::Heartbeat | FollowItem::Live(_)))) => {}
                    other => panic!("the stream stopped: {other:?} after {seen:?}"),
                }
            }
        }
    };
    let ids = |seen: &[(i64, Uuid, Uuid)]| seen.iter().map(|(_, id, _)| *id).collect::<Vec<_>>();
    let in_order = |seen: &[(i64, Uuid, Uuid)]| seen.windows(2).all(|pair| pair[0].0 < pair[1].0);

    // Two workspaces, no DMs: the first and second only, in log order.
    let two = heard(FollowSpec {
        workspaces: Some(HashSet::from([first, second])),
        direct: false,
        ..FollowSpec::everything(owner, "human", task_updates.clone())
    })
    .await;
    assert_eq!(ids(&two), vec![written[0], written[1], written[4]]);
    assert!(in_order(&two), "in log order");
    assert_eq!(
        two.iter()
            .map(|(_, _, workspace)| *workspace)
            .collect::<Vec<_>>(),
        vec![first, second, first],
        "each names its workspace"
    );

    // Everything: all three workspaces and the direct scope, one order.
    let all = heard(FollowSpec::everything(owner, "human", task_updates.clone())).await;
    assert_eq!(ids(&all), written);
    assert!(in_order(&all), "in log order");
    assert_eq!(all[3].2, DIRECT_SCOPE_ID, "the direct scope names itself");

    // DMs and the first workspace only.
    let direct = heard(FollowSpec {
        workspaces: Some(HashSet::from([first])),
        ..FollowSpec::everything(owner, "human", task_updates)
    })
    .await;
    assert_eq!(ids(&direct), vec![written[0], written[3], written[4]]);

    // Nobody's workspace or space hears the direct scope.
    for event in [
        to(DIRECT_SCOPE_ID, Audience::Workspace),
        NewEvent {
            space_id: Some(project),
            ..to(DIRECT_SCOPE_ID, Audience::Space)
        },
    ] {
        let mut transaction = cluster.pool.begin().await.expect("begin");
        let error = events::emit(&mut transaction, event)
            .await
            .expect_err("refused");
        assert!(
            format!("{error:?}").contains("event_staging_direct_scope_audience"),
            "{error:?}"
        );
        transaction.rollback().await.expect("rollback");
    }
    cluster.finish().await;
}

/// K5.14: a provider sign-in started through one instance is answered by
/// the other: its poll sees the code, a retried start replays it, a second
/// start is refused, and a cancel there ends it for both.
#[tokio::test]
async fn provider_sign_in_started_on_one_instance_completes_on_the_other() {
    use agent_workspace_host_protocol::account::LoginStarted as LoginStart;
    use agent_workspace_server::{
        harness::manifest::LoginKind,
        sign_ins::{self as ceremonies, LoginCeremonyError, LoginReservation},
    };

    let Some(cluster) = cluster("k5_14_sign_in", 2).await else {
        return;
    };
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 LIMIT 1",
    )
    .bind(cluster.organization_id)
    .fetch_one(&cluster.pool)
    .await
    .expect("owner");
    let account = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provider_accounts (id, organization_id, owner_actor_id, adapter_kind, name, home_path)
        VALUES ($1, $2, $3, 'demo', 'Demo', '/tmp/k5-14-' || $1::text)
        "#,
    )
    .bind(account)
    .bind(cluster.organization_id)
    .bind(owner)
    .execute(&cluster.pool)
    .await
    .expect("provider account");
    let (a, b) = (
        cluster.instance(0).state().pool.clone(),
        cluster.instance(1).state().pool.clone(),
    );
    let session = Uuid::now_v7();
    let reserve = |pool: sqlx::PgPool, session: Uuid, key: &'static str| async move {
        crate::support::as_member(
            owner,
            ceremonies::reserve_login(&pool, account, owner, session, key, LoginKind::DeviceCode),
        )
        .await
    };
    let Ok(LoginReservation::Reserved(ceremony)) = reserve(a.clone(), session, "start").await
    else {
        panic!("the first start reserves");
    };
    // Not answered yet: a second start, anywhere, is refused.
    assert!(matches!(
        reserve(b.clone(), session, "start").await,
        Err(LoginCeremonyError::Active)
    ));
    crate::support::as_member(
        owner,
        ceremonies::complete_login(
            &a,
            account,
            ceremony,
            LoginStart::DeviceCode {
                login_id: "login-1".to_owned(),
                verification_url: "https://example.com/device".to_owned(),
                user_code: "ABCD-EFGH".to_owned(),
            },
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("complete"));
    // The page's poll lands on B and sees the code.
    let seen = crate::support::as_member(
        owner,
        ceremonies::login_ceremony(&b, account, owner, session),
    )
    .await
    .expect("read")
    .expect("open on B");
    assert_eq!(seen.ceremony_id, ceremony);
    assert!(matches!(
        seen.response,
        Some(LoginStart::DeviceCode { ref user_code, .. }) if user_code == "ABCD-EFGH"
    ));
    // Another session doesn't see it; a retried start on B replays it.
    assert!(
        crate::support::as_member(
            owner,
            ceremonies::login_ceremony(&b, account, owner, Uuid::now_v7())
        )
        .await
        .expect("read")
        .is_none()
    );
    assert!(matches!(
        reserve(b.clone(), session, "start").await,
        Ok(LoginReservation::Replay(replayed)) if replayed.ceremony_id == ceremony
    ));
    assert!(matches!(
        reserve(b.clone(), Uuid::now_v7(), "other").await,
        Err(LoginCeremonyError::Active)
    ));
    // Cancelled on B, it is gone on A, and a new start may begin.
    crate::support::as_member(owner, ceremonies::clear_login(&b, account, Some(ceremony)))
        .await
        .expect("cancel");
    assert!(
        crate::support::as_member(
            owner,
            ceremonies::login_ceremony(&a, account, owner, session)
        )
        .await
        .expect("read")
        .is_none()
    );
    assert!(matches!(
        reserve(a.clone(), session, "again").await,
        Ok(LoginReservation::Reserved(_))
    ));
    cluster.finish().await;
}

/// K5.18: with the work loop off, a queued job, a consumer batch and a due
/// scheduled message all wait for the tick, and one tick does them all.
/// The tick answers only with its token.
#[tokio::test]
async fn ticks_drive_work_with_the_loop_off() {
    use std::os::unix::fs::PermissionsExt;
    let secrets = tempfile::tempdir().expect("secrets");
    let token_path = secrets.path().join("tick-token");
    std::fs::write(&token_path, "tick-token-for-tests\n").expect("token");
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
        .expect("owner only");
    let token_path = token_path.to_string_lossy().into_owned();
    let settings: &[(&str, &str)] = &[
        ("AGENT_WORKSPACE_WORK_LOOP", "off"),
        ("AGENT_WORKSPACE_WORK_TICK_SECRET_PATH", &token_path),
    ];
    let Some(cluster) = cluster_with("k5_18_tick", 1, settings).await else {
        return;
    };
    let pool = cluster.pool.clone();
    let a = cluster.instance(0);
    let browser = a.login("owner").await;
    let (project, channel) = work_channel(&cluster, &browser).await;

    // The consumers' cursors start at the head, then fall behind.
    events::catch_up(&pool).await.expect("catch up");
    let send_at = (chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc3339();
    let (status, scheduled) = a
        .send(browser.request(
            "POST",
            "/api/v2/ops/message_schedule",
            Some("k5-18-scheduled"),
            json!({
                "conversationId": channel,
                "content": {"version": 1, "nodes": [{"type": "text", "text": "Ticked"}]},
                "sendAt": send_at,
            }),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{scheduled}");
    let scheduled: Uuid = scheduled["value"]["scheduled"]["id"]
        .as_str()
        .expect("scheduled id")
        .parse()
        .expect("id");
    let job = match agent_workspace_server::jobs::enqueue(
        &pool,
        agent_workspace_server::jobs::NewJob::new("jobs.prune_finished"),
    )
    .await
    .expect("enqueue")
    {
        agent_workspace_server::jobs::Enqueued::Created(id) => id,
        agent_workspace_server::jobs::Enqueued::Duplicate => panic!("no other job"),
    };
    let mut transaction = pool.begin().await.expect("begin");
    stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");

    let job_state = async || {
        sqlx::query_scalar::<_, String>("SELECT state FROM background_jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .expect("job")
    };
    let sent = async || {
        sqlx::query_scalar::<_, String>("SELECT status FROM scheduled_messages WHERE id = $1")
            .bind(scheduled)
            .fetch_one(&pool)
            .await
            .expect("scheduled")
    };
    // Positions still come (the live role sequences), so the browser's
    // updates don't wait for a tick; the consumers do.
    let behind = async || {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM event_consumers \
             WHERE position < (SELECT next_position - 1 FROM event_position_counter)",
        )
        .fetch_one(&pool)
        .await
        .expect("consumers")
    };
    let started = Instant::now();
    while sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(&pool)
        .await
        .expect("staging")
        > 0
    {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "never sequenced"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(job_state().await, "queued");
    assert_eq!(sent().await, "scheduled");
    assert_eq!(behind().await, 3, "every consumer waits");

    let tick = |token: Option<&str>| {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/internal/work/tick");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request.body(axum::body::Body::empty()).expect("tick")
    };
    for token in [None, Some("wrong"), Some("")] {
        let (status, _) = a.send(tick(token)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{token:?}");
    }
    assert_eq!(job_state().await, "queued", "a refused tick does nothing");

    let (status, ticked) = a.send(tick(Some("tick-token-for-tests"))).await;
    assert_eq!(status, StatusCode::OK, "{ticked}");
    assert!(ticked["jobs"].as_u64().expect("jobs") >= 2, "{ticked}");
    assert!(ticked["events"].as_u64().expect("events") >= 1, "{ticked}");
    assert_eq!(job_state().await, "succeeded");
    assert_eq!(sent().await, "sent");
    assert_eq!(behind().await, 0, "{ticked}");
    cluster.finish().await;
}

/// K5.18: an `api` instance does nothing at start but connect, so it
/// answers within a second of starting.
#[tokio::test]
async fn api_answers_within_a_second_of_starting() {
    let Some(mut cluster) = cluster("k5_18_cold_api", 0).await else {
        return;
    };
    let started = Instant::now();
    let api = cluster.start(&[("AGENT_WORKSPACE_ROLES", "api")]).await;
    let (status, ready) = api.ready().await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
    let held = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM leases WHERE expires_at > now()")
        .fetch_one(&cluster.pool)
        .await
        .expect("leases");
    assert_eq!(held, 0, "an api instance took a lease at start");
    cluster.finish().await;
}
