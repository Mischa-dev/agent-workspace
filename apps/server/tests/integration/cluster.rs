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

// K5.4: positions, the wake and the tail.

/// A project to hang events on, made through A's HTTP.
async fn log_project(cluster: &crate::cluster_support::Cluster) -> Uuid {
    let a = cluster.instance(0);
    let browser = a.login("owner").await;
    let (status, project) = a
        .send(browser.request(
            "POST",
            "/api/v1/projects",
            Some("cluster-log"),
            json!({"organizationId": cluster.organization_id, "key": "LOG", "name": "Log"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
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
            Ok(Ok(Tailed::Event(event))) => seen.push((event.position, event.id)),
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
        assert_eq!(seen.len(), written.len(), "each event once");
        assert_dense(&seen);
    }
    cluster.finish().await;
}

#[tokio::test]
async fn tail_survives_losing_the_listener() {
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
        "SELECT state, (SELECT count(*) FROM agent_run_events \
                        WHERE agent_run_id = run.id AND kind = 'recovery') \
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
            "/api/v1/projects",
            Some("k5-5-project"),
            json!({"organizationId": cluster.organization_id, "key": "WORK", "name": "Work"}),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
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
            if frame["type"] == "conversation.message.created"
                && frame["conversationId"] == channel.to_string()
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
    let frame = frame_of(&mut on_b, "message.delta", Duration::from_secs(2))
        .await
        .expect("the delta reached B's socket");
    assert_eq!(
        frame["payload"]["message_id"],
        message.to_string(),
        "{frame}"
    );
    assert_eq!(frame["payload"]["delta"], "Hel", "{frame}");
    cluster.finish().await;
}

#[tokio::test]
async fn module_switch_is_seen_everywhere() {
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
            "PUT",
            &format!("/api/v1/organizations/{}/modules", cluster.organization_id),
            None,
            json!({"switches": {"tasks": false}}),
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
