//! K3.2 and K3.3: the event log. A change writes its event in its own
//! transaction; the sequencer gives committed events dense positions in
//! commit order, and a reader that follows positions never skips one.

use serde_json::json;
use uuid::Uuid;

use super::{
    message_edits::create_channel,
    support::{
        app::tool,
        world::{Setup, WorldBuilder},
    },
};

/// Sending a message stages one `message.created` with who sent it and the
/// app they sent it through; a rolled-back transaction stages nothing.
#[tokio::test]
async fn a_message_stages_one_event_with_its_actor_and_app() {
    let Some(world) = WorldBuilder::default()
        .project("LOG")
        .connected_app("helper")
        .build("event_log_message")
        .await
    else {
        return;
    };
    let setup = Setup {
        app: world.app.clone(),
        browser: super::support::app::login(&world.app).await,
        organization_id: world.organization,
        pool: world.pool().clone(),
        cleanup: None,
    };
    let channel = create_channel(&setup, world.project("LOG"), "general").await;
    let helper = &world.apps["helper"];
    let sent = tool(
        &world.app,
        &helper.token,
        &helper.session,
        "message_send",
        json!({"conversationId": channel, "text": "hello log", "clientRequestId": "log-1"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let message = sent["value"]["messageId"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("message id");

    let staged = sqlx::query_as::<
        _,
        (
            String,
            Option<Uuid>,
            Option<Uuid>,
            String,
            Option<Uuid>,
            Option<bool>,
            Uuid,
            Option<String>,
            serde_json::Value,
        ),
    >(
        r#"
        SELECT type, actor_account_id, via_credential_id, audience, conversation_id,
               conversation_restricted, workspace_id, legacy_type, payload
        FROM event_staging WHERE thing_id = $1
        "#,
    )
    .bind(message)
    .fetch_all(world.pool())
    .await
    .expect("staged events");
    assert_eq!(staged.len(), 1, "{staged:?}");
    let (ty, actor, via, audience, conversation, restricted, workspace, legacy, payload) =
        staged.into_iter().next().expect("one");
    assert_eq!(ty, "message.created");
    assert_eq!(actor, Some(helper.person));
    let grant = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT via_oauth_grant_id FROM messages WHERE id = $1",
    )
    .bind(message)
    .fetch_one(world.pool())
    .await
    .expect("message");
    assert!(grant.is_some(), "the message records its app");
    assert_eq!(via, grant, "the event names the app it came through");
    assert_eq!(audience, "conversation");
    assert_eq!(conversation, Some(channel.parse().expect("channel")));
    assert_eq!(restricted, Some(false));
    assert_eq!(workspace, world.organization);
    assert_eq!(legacy.as_deref(), Some("conversation.message.created"));
    assert!(
        !payload.to_string().contains("hello log"),
        "an event never carries message text: {payload}"
    );

    // A transaction that rolls back leaves no event behind.
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(world.pool())
        .await
        .expect("count");
    let mut transaction = world.pool().begin().await.expect("begin");
    agent_workspace_server::events::emit(
        &mut transaction,
        agent_workspace_server::events::NewEvent::thing(
            &agent_workspace_server::events::catalog::TASK_UPDATED,
            world.project("LOG").parse().expect("project"),
            "task",
            Uuid::now_v7(),
        ),
    )
    .await
    .expect("emit");
    transaction.rollback().await.expect("rollback");
    let after = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(world.pool())
        .await
        .expect("count");
    assert_eq!(before, after);
    drop(setup);
    world.finish().await;
}

// --- K3.3: the sequencer, cursors and consumers ---------------------------

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use agent_workspace_server::events::{
    self, LoggedEvent, NewEvent, catalog,
    consumer::{Consumer, ConsumerError, Runner},
    sequencer,
};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};

/// A database with one project to hang events on.
async fn log_db(name: &str) -> Option<(super::support::db::Db, Uuid)> {
    let db = super::support::db::db(name).await?;
    let owner = agent_workspace_server::bootstrap::bootstrap_admin(
        &db.pool,
        agent_workspace_server::bootstrap::BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Log".to_owned()),
            organization_slug: Some(format!("log-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        super::agent_interface::PASSWORD.to_owned(),
    )
    .await
    .expect("owner");
    let app = super::support::app::test_app(db.pool.clone(), &db.url);
    let browser = super::support::app::login(&app).await;
    let (status, project) = super::support::app::send(
        &app,
        browser.request(
            "POST",
            "/api/v1/projects",
            Some("log-project"),
            json!({"organizationId": owner.organization_id, "key": "LOG", "name": "Log"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{project}");
    let project = project["id"]
        .as_str()
        .expect("project")
        .parse()
        .expect("id");
    // Making the project staged events of its own; start from an empty log.
    sqlx::query("DELETE FROM event_staging")
        .execute(&db.pool)
        .await
        .expect("clear");
    Some((db, project))
}

/// Stage one event about a fresh thing in its own transaction.
async fn stage(transaction: &mut Transaction<'_, Postgres>, project: Uuid) -> Uuid {
    events::emit(
        transaction,
        NewEvent::thing(&catalog::TASK_UPDATED, project, "task", Uuid::now_v7()),
    )
    .await
    .expect("emit")
}

async fn stage_one(pool: &PgPool, project: Uuid) -> Uuid {
    let mut transaction = pool.begin().await.expect("begin");
    let id = stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");
    id
}

async fn positions(pool: &PgPool) -> Vec<(i64, Uuid)> {
    sqlx::query_as::<_, (i64, Uuid)>("SELECT position, id FROM events ORDER BY position")
        .fetch_all(pool)
        .await
        .expect("events")
}

/// (a) Two transactions that stage in one order and commit in the other
/// are read in commit order, and a reader following positions skips none.
#[tokio::test]
async fn events_take_positions_in_commit_order() {
    let Some((db, project)) = log_db("k3_3_commit_order").await else {
        return;
    };
    let pool = &db.pool;
    let mut first = pool.begin().await.expect("first");
    let early = stage(&mut first, project).await;
    let late = stage_one(pool, project).await;
    sequencer::step(pool).await.expect("step");
    // A reader at the head sees the second transaction's event alone.
    let mut connection = pool.acquire().await.expect("connection");
    let seen = events::tail(&mut connection, 0, 100).await.expect("tail");
    assert_eq!(
        seen.iter().map(|event| event.id).collect::<Vec<_>>(),
        [late]
    );
    let cursor = seen[0].position;
    first.commit().await.expect("commit the first");
    sequencer::step(pool).await.expect("step");
    let after = events::tail(&mut connection, cursor, 100)
        .await
        .expect("tail");
    assert_eq!(
        after.iter().map(|event| event.id).collect::<Vec<_>>(),
        [early]
    );
    assert_eq!(after[0].position, cursor + 1, "positions are dense");
    let staged = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(pool)
        .await
        .expect("count");
    assert_eq!(staged, 0);
    drop(connection);
    db.finish().await;
}

/// (c) Two sequencers in two pools never give out a position twice.
#[tokio::test]
async fn two_sequencers_never_share_a_position() {
    let Some((db, project)) = log_db("k3_3_two_sequencers").await else {
        return;
    };
    let other = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db.url)
        .await
        .expect("second pool");
    let pools = [db.pool.clone(), other.clone()];
    let mut writers = Vec::new();
    for writer in 0..4 {
        let pool = pools[writer % 2].clone();
        writers.push(tokio::spawn(async move {
            for _ in 0..25 {
                stage_one(&pool, project).await;
            }
        }));
    }
    let mut sequencers = Vec::new();
    for pool in pools.clone() {
        sequencers.push(tokio::spawn(async move {
            for _ in 0..200 {
                sequencer::step(&pool).await.expect("step");
                tokio::task::yield_now().await;
            }
        }));
    }
    for writer in writers {
        writer.await.expect("writer");
    }
    for sequencer in sequencers {
        sequencer.await.expect("sequencer");
    }
    while sequencer::step(&db.pool).await.expect("step") > 0 {}
    let all = positions(&db.pool).await;
    assert_eq!(all.len(), 100);
    for (index, (position, _)) in all.iter().enumerate() {
        assert_eq!(*position, index as i64 + 1, "dense and unique");
    }
    other.close().await;
    db.finish().await;
}

/// (e) With no wake at all (the pooler case), the loop's poll alone gives a
/// new event its position, and a reader sees it, within one interval.
#[tokio::test]
async fn the_sequencer_works_on_its_poll_alone() {
    let Some((db, project)) = log_db("k3_3_poll_alone").await else {
        return;
    };
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let looping = tokio::spawn(sequencer::run(
        db.pool.clone(),
        Arc::new(tokio::sync::Notify::new()),
        receiver,
    ));
    let id = stage_one(&db.pool, project).await;
    let deadline = tokio::time::Instant::now() + sequencer::POLL * 4;
    loop {
        if positions(&db.pool)
            .await
            .iter()
            .any(|(_, seen)| *seen == id)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not sequenced within a poll"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = shutdown.send(true);
    looping.await.expect("loop stops");
    db.finish().await;
}

/// (d) Retention lets signals go after 7 days and keeps facts.
#[tokio::test]
async fn retention_deletes_old_signals_and_keeps_facts() {
    let Some((db, project)) = log_db("k3_3_retention").await else {
        return;
    };
    let pool = &db.pool;
    let mut transaction = pool.begin().await.expect("begin");
    let signal = events::emit(
        &mut transaction,
        NewEvent::thing(
            &catalog::TASK_PLACEMENT_CHANGED,
            project,
            "task",
            Uuid::now_v7(),
        ),
    )
    .await
    .expect("signal");
    let fact = stage(&mut transaction, project).await;
    transaction.commit().await.expect("commit");
    // Staged eight days ago.
    sqlx::query("UPDATE event_staging SET created_at = now() - interval '8 days'")
        .execute(pool)
        .await
        .expect("age");
    sequencer::step(pool).await.expect("step");
    let gone = events::upkeep::apply_retention(pool)
        .await
        .expect("retention");
    assert_eq!(gone, 1);
    let left = positions(pool)
        .await
        .into_iter()
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    assert_eq!(left, [fact]);
    assert!(!left.contains(&signal));
    // The partition job has nothing to add on a new log.
    assert!(
        !events::upkeep::ensure_partitions(pool)
            .await
            .expect("partitions")
    );
    db.finish().await;
}

/// A consumer that fails on the events it's told to.
struct Picky {
    bad: Uuid,
    fail_bad: Arc<std::sync::atomic::AtomicBool>,
    handled: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Consumer for Picky {
    fn name(&self) -> &'static str {
        "test.picky"
    }

    fn starts_at_head(&self) -> bool {
        false
    }

    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        batch: &[LoggedEvent],
    ) -> Result<(), ConsumerError> {
        // A database effect in the same transaction as the cursor.
        sqlx::query("SELECT 1").execute(&mut **transaction).await?;
        if self.fail_bad.load(Ordering::SeqCst) && batch.iter().any(|event| event.id == self.bad) {
            return Err(ConsumerError("this one is bad".to_owned()));
        }
        self.handled.fetch_add(batch.len(), Ordering::SeqCst);
        Ok(())
    }
}

/// (b) A batch that keeps failing goes event by event; the event that
/// keeps failing is parked and passed; the sweep retries it later.
#[tokio::test]
async fn a_failing_event_is_parked_and_swept_not_lost() {
    let Some((db, project)) = log_db("k3_3_parking").await else {
        return;
    };
    let pool = &db.pool;
    stage_one(pool, project).await;
    let bad = stage_one(pool, project).await;
    stage_one(pool, project).await;
    sequencer::step(pool).await.expect("step");
    let fail_bad = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let handled = Arc::new(AtomicUsize::new(0));
    let mut runner = Runner::new(Picky {
        bad,
        fail_bad: Arc::clone(&fail_bad),
        handled: Arc::clone(&handled),
    });
    let mut parked = 0;
    for _ in 0..20 {
        let round = runner.round(pool).await.expect("round");
        parked += round.parked;
        if parked > 0 && round.handled == 0 && round.parked == 0 {
            break;
        }
    }
    assert_eq!(parked, 1);
    assert_eq!(
        handled.load(Ordering::SeqCst),
        2,
        "the good events were handled once"
    );
    let cursor = sqlx::query_scalar::<_, i64>(
        "SELECT position FROM event_consumers WHERE name = 'test.picky'",
    )
    .fetch_one(pool)
    .await
    .expect("cursor");
    assert_eq!(cursor, 3, "the consumer moved past the bad event");
    let (position, attempts) = sqlx::query_as::<_, (i64, i32)>(
        "SELECT event_position, attempts FROM event_consumer_failures WHERE consumer = 'test.picky'",
    )
    .fetch_one(pool)
    .await
    .expect("parked");
    assert_eq!((position, attempts), (2, 5));

    // Once it's due and works, the sweep resolves it.
    fail_bad.store(false, Ordering::SeqCst);
    assert_eq!(runner.sweep(pool).await.expect("sweep"), 0, "not due yet");
    sqlx::query("UPDATE event_consumer_failures SET next_attempt_at = now()")
        .execute(pool)
        .await
        .expect("due");
    assert_eq!(runner.sweep(pool).await.expect("sweep"), 1);
    assert_eq!(handled.load(Ordering::SeqCst), 3);
    db.finish().await;
}
