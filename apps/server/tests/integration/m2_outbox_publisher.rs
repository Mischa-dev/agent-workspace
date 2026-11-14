use std::time::Duration;

use agent_workspace_server::{
    outbox::{
        ConversationCreatedPayload, NewOutboxEvent, OutboxConversationKind, OutboxEventPayload,
        OutboxPublisher, claim_ready, enqueue, mark_published,
    },
    realtime::{RealtimeHub, outbox_worker_id, run_outbox_publisher},
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::{sync::watch, time::timeout};
use uuid::Uuid;

#[tokio::test]
async fn publisher_recovers_committed_rows_and_isolates_poison_events() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_outbox_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated outbox schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let conversation_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO organizations (id, name, slug)
        VALUES ($1, 'Outbox organization', $2)
        "#,
    )
    .bind(organization_id)
    .bind(format!(
        "outbox-{}",
        &organization_id.simple().to_string()[..12]
    ))
    .execute(&pool)
    .await
    .expect("insert Organization");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES ($1, $2, 'OBX', 'Outbox Project')
        "#,
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert Project");
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, name)
        VALUES ($1, $2, 'channel', 'outbox')
        "#,
    )
    .bind(conversation_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("insert Channel");

    let poison_event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type, payload
        )
        VALUES (
            $1, $2, $3, 'conversation', $3,
            'conversation.future.changed',
            jsonb_build_object('conversationId', $3::text)
        )
        "#,
    )
    .bind(poison_event_id)
    .bind(project_id)
    .bind(conversation_id)
    .execute(&pool)
    .await
    .expect("insert poison event");

    let first_valid_event_id =
        enqueue_conversation_created(&pool, project_id, conversation_id).await;
    let realtime = RealtimeHub::new(8);
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let publisher = tokio::spawn(run_outbox_publisher(
        pool.clone(),
        realtime.clone(),
        outbox_worker_id(),
        shutdown_receiver,
    ));

    wait_for_outbox_state(&pool, poison_event_id, false, true).await;
    wait_for_outbox_state(&pool, first_valid_event_id, true, false).await;
    shutdown_sender
        .send(true)
        .expect("request publisher shutdown");
    timeout(Duration::from_secs(3), publisher)
        .await
        .expect("publisher shutdown deadline")
        .expect("join publisher")
        .expect("publisher result");

    let poison_attempts =
        sqlx::query_scalar::<_, i32>("SELECT attempt_count FROM outbox_events WHERE event_id = $1")
            .bind(poison_event_id)
            .fetch_one(&pool)
            .await
            .expect("read poison attempts");
    assert_eq!(poison_attempts, 1);
    let error_summary = sqlx::query_scalar::<_, String>(
        "SELECT last_error_summary FROM outbox_events WHERE event_id = $1",
    )
    .bind(poison_event_id)
    .fetch_one(&pool)
    .await
    .expect("read poison summary");
    assert_eq!(error_summary, "outbox event is malformed");

    // A committed row left unpublished while the worker is stopped is picked
    // up by the next supervised worker without manual repair.
    let restart_event_id = enqueue_conversation_created(&pool, project_id, conversation_id).await;
    let (restart_shutdown_sender, restart_shutdown_receiver) = watch::channel(false);
    let restarted_publisher = tokio::spawn(run_outbox_publisher(
        pool.clone(),
        realtime,
        outbox_worker_id(),
        restart_shutdown_receiver,
    ));
    wait_for_outbox_state(&pool, restart_event_id, true, false).await;
    restart_shutdown_sender
        .send(true)
        .expect("request restarted publisher shutdown");
    timeout(Duration::from_secs(3), restarted_publisher)
        .await
        .expect("restarted publisher shutdown deadline")
        .expect("join restarted publisher")
        .expect("restarted publisher result");

    // Two workers racing the same ready row must produce exactly one claim.
    let competing_event_id = enqueue_conversation_created(&pool, project_id, conversation_id).await;
    let mut first_transaction = pool.begin().await.expect("begin first competing claim");
    let mut second_transaction = pool.begin().await.expect("begin second competing claim");
    let (first_claims, second_claims) = tokio::join!(
        claim_ready(&mut first_transaction, "competing-worker-a", 1, 30),
        claim_ready(&mut second_transaction, "competing-worker-b", 1, 30),
    );
    let first_claims = first_claims.expect("first competing claim");
    let second_claims = second_claims.expect("second competing claim");
    assert_eq!(
        first_claims.len() + second_claims.len(),
        1,
        "SKIP LOCKED must assign a ready row to only one publisher"
    );
    first_transaction
        .commit()
        .await
        .expect("commit first competing claim");
    second_transaction
        .commit()
        .await
        .expect("commit second competing claim");
    let (winner, claimed_event_id) = if let Some(claim) = first_claims.first() {
        ("competing-worker-a", claim.event_id())
    } else {
        (
            "competing-worker-b",
            second_claims.first().expect("one winning claim").event_id(),
        )
    };
    assert_eq!(claimed_event_id, competing_event_id);
    let mut transaction = pool.begin().await.expect("begin competing acknowledgement");
    mark_published(&mut transaction, competing_event_id, winner)
        .await
        .expect("acknowledge competing claim");
    transaction
        .commit()
        .await
        .expect("commit competing acknowledgement");

    // A process may publish successfully and die before recording that fact.
    // The replacement publisher redelivers the same stable event ID, allowing
    // a version-aware client to deduplicate before converging through HTTP.
    let redelivery_event_id =
        enqueue_conversation_created(&pool, project_id, conversation_id).await;
    let mut transaction = pool.begin().await.expect("begin pre-crash claim");
    let mut claims = claim_ready(&mut transaction, "pre-crash-worker", 1, 1)
        .await
        .expect("claim pre-crash event");
    assert_eq!(claims.len(), 1);
    let first_claim = claims.pop().expect("pre-crash claim");
    transaction.commit().await.expect("commit pre-crash claim");
    let first_envelope = first_claim.envelope().expect("first delivery envelope");
    assert_eq!(first_envelope.event_id, redelivery_event_id);
    OutboxPublisher::publish(&RealtimeHub::new(1), &first_envelope)
        .expect("publish before simulated crash");
    sqlx::query(
        "UPDATE outbox_events SET lease_expires_at = now() - interval '1 second' WHERE event_id = $1",
    )
    .bind(redelivery_event_id)
    .execute(&pool)
    .await
    .expect("expire pre-crash publication lease");

    let mut transaction = pool.begin().await.expect("begin replacement claim");
    let mut claims = claim_ready(&mut transaction, "replacement-redelivery-worker", 1, 30)
        .await
        .expect("claim event after publish crash");
    assert_eq!(claims.len(), 1);
    let second_claim = claims.pop().expect("replacement claim");
    transaction
        .commit()
        .await
        .expect("commit replacement claim");
    let second_envelope = second_claim
        .envelope()
        .expect("replacement delivery envelope");
    assert_eq!(second_envelope.event_id, first_envelope.event_id);
    assert_eq!(second_envelope.event_cursor, first_envelope.event_cursor);
    assert_eq!(
        serde_json::to_value(&second_envelope).expect("second envelope JSON"),
        serde_json::to_value(&first_envelope).expect("first envelope JSON")
    );
    OutboxPublisher::publish(&RealtimeHub::new(1), &second_envelope)
        .expect("stable duplicate publication is harmless");
    let mut transaction = pool
        .begin()
        .await
        .expect("begin redelivery acknowledgement");
    mark_published(
        &mut transaction,
        redelivery_event_id,
        "replacement-redelivery-worker",
    )
    .await
    .expect("acknowledge stable redelivery");
    transaction
        .commit()
        .await
        .expect("commit redelivery acknowledgement");
    let (published, attempts) = sqlx::query_as::<_, (bool, i32)>(
        "SELECT published_at IS NOT NULL, attempt_count FROM outbox_events WHERE event_id = $1",
    )
    .bind(redelivery_event_id)
    .fetch_one(&pool)
    .await
    .expect("read redelivered event state");
    assert!(published);
    assert_eq!(attempts, 2);

    // Simulate a worker crash after claiming the final permitted attempt.
    // The next worker must terminally fail the expired claim rather than
    // leaving an unreachable unpublished row forever.
    let final_attempt_event = NewOutboxEvent::new(
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id,
            kind: OutboxConversationKind::Channel,
            version: 1,
        }),
    )
    .expect("valid final-attempt event")
    .with_max_attempts(1)
    .expect("one permitted attempt");
    let final_attempt_event_id = final_attempt_event.event_id();
    let mut transaction = pool.begin().await.expect("begin final-attempt enqueue");
    enqueue(&mut transaction, &final_attempt_event)
        .await
        .expect("enqueue final-attempt event");
    transaction
        .commit()
        .await
        .expect("commit final-attempt event");

    let mut transaction = pool.begin().await.expect("begin crash simulation claim");
    let claims = claim_ready(&mut transaction, "crashed-worker", 1, 1)
        .await
        .expect("claim final attempt");
    assert_eq!(claims.len(), 1);
    transaction
        .commit()
        .await
        .expect("commit crash simulation claim");
    sqlx::query(
        "UPDATE outbox_events SET lease_expires_at = now() - interval '1 second' WHERE event_id = $1",
    )
    .bind(final_attempt_event_id)
    .execute(&pool)
    .await
    .expect("expire crashed final-attempt lease");

    let mut transaction = pool.begin().await.expect("begin exhausted sweep");
    let claims = claim_ready(&mut transaction, "replacement-worker", 1, 1)
        .await
        .expect("sweep exhausted claim");
    assert!(claims.is_empty());
    transaction.commit().await.expect("commit exhausted sweep");
    let exhausted = sqlx::query_as::<_, (bool, bool, Option<String>)>(
        r#"
        SELECT failed_at IS NOT NULL,
               lease_owner IS NULL AND lease_expires_at IS NULL,
               last_error_summary
        FROM outbox_events
        WHERE event_id = $1
        "#,
    )
    .bind(final_attempt_event_id)
    .fetch_one(&pool)
    .await
    .expect("read exhausted final attempt");
    assert!(exhausted.0);
    assert!(exhausted.1);
    assert_eq!(
        exhausted.2.as_deref(),
        Some("outbox publication lease expired after final attempt")
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated outbox schema");
}

async fn enqueue_conversation_created(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
) -> Uuid {
    let event = NewOutboxEvent::new(
        project_id,
        OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
            conversation_id,
            kind: OutboxConversationKind::Channel,
            version: 1,
        }),
    )
    .expect("valid typed outbox event");
    let event_id = event.event_id();
    let mut transaction = pool.begin().await.expect("begin outbox transaction");
    enqueue(&mut transaction, &event)
        .await
        .expect("enqueue outbox event");
    transaction.commit().await.expect("commit outbox event");
    event_id
}

async fn wait_for_outbox_state(
    pool: &PgPool,
    event_id: Uuid,
    expect_published: bool,
    expect_failed: bool,
) {
    timeout(Duration::from_secs(5), async {
        loop {
            let state = sqlx::query_as::<_, (bool, bool)>(
                r#"
                SELECT published_at IS NOT NULL, failed_at IS NOT NULL
                FROM outbox_events
                WHERE event_id = $1
                "#,
            )
            .bind(event_id)
            .fetch_one(pool)
            .await
            .expect("read outbox state");
            if state == (expect_published, expect_failed) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("outbox state deadline");
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect to isolated outbox schema")
}

/// P0.1: an event whose notifications or webhook and automation routing
/// fail is retried, not marked published, so its inbox rows, pushes, emails
/// and webhooks aren't lost after one transient error.
#[tokio::test]
async fn an_event_whose_notifications_fail_is_retried_not_published() {
    use agent_workspace_server::realtime::publish_ready_outbox_batch;
    use serde_json::json;

    use super::{
        mcp_admin::{create_project, setup},
        message_edits::{add_person, create_channel, say},
        notifications::drain,
    };

    let Some(setup) = setup("outbox_side_effects").await else {
        return;
    };
    let project = create_project(&setup, "OSE", "Side effects").await;
    let general = create_channel(&setup, &project, "general").await;
    let (mara, _) = add_person(&setup, &[&project], "mara").await;
    drain(&setup).await;
    let hub = RealtimeHub::new(64);
    let mention = |text: &str| {
        json!({
            "version": 1,
            "nodes": [
                {"type": "text", "text": text},
                {"type": "mention", "actorId": mara}
            ]
        })
    };
    let state = |message: String| {
        let pool = setup.pool.clone();
        async move {
            sqlx::query_as::<_, (bool, i32, Option<String>)>(
                r#"
                SELECT published_at IS NOT NULL, attempt_count, last_error_summary
                FROM outbox_events
                WHERE event_type = 'conversation.message.created'
                  AND resource_id = $1
                "#,
            )
            .bind(message.parse::<Uuid>().expect("message"))
            .fetch_one(&pool)
            .await
            .expect("message event")
        }
    };
    let retry_now = || {
        let pool = setup.pool.clone();
        async move {
            sqlx::query(
                "UPDATE outbox_events SET available_at = now() WHERE published_at IS NULL AND failed_at IS NULL",
            )
            .execute(&pool)
            .await
            .expect("make due");
        }
    };

    // Notifications can't be written: the event stays unpublished.
    let message = say(&setup, &setup.browser, &general, "m1", mention("Look ")).await;
    sqlx::query("ALTER TABLE notifications RENAME TO notifications_away")
        .execute(&setup.pool)
        .await
        .expect("rename notifications");
    publish_ready_outbox_batch(&setup.pool, &hub, "side-effects-test")
        .await
        .expect("publish");
    let (published, attempts, summary) = state(message.clone()).await;
    assert!(!published, "the event was acknowledged without its notifications");
    assert_eq!(attempts, 1);
    assert_eq!(
        summary.as_deref(),
        Some("notifications or webhook and automation routing were unavailable")
    );

    // Back again, the retry delivers exactly one notification.
    sqlx::query("ALTER TABLE notifications_away RENAME TO notifications")
        .execute(&setup.pool)
        .await
        .expect("restore notifications");
    retry_now().await;
    drain(&setup).await;
    let (published, _, _) = state(message.clone()).await;
    assert!(published);
    let delivered = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1",
    )
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("notifications");
    assert_eq!(delivered, 1);

    // The same for routing to webhooks and automations.
    let message = say(&setup, &setup.browser, &general, "m2", mention("Again ")).await;
    sqlx::query("ALTER TABLE webhook_subscriptions RENAME TO webhook_subscriptions_away")
        .execute(&setup.pool)
        .await
        .expect("rename webhook subscriptions");
    publish_ready_outbox_batch(&setup.pool, &hub, "side-effects-test")
        .await
        .expect("publish");
    let (published, attempts, _) = state(message.clone()).await;
    assert!(!published, "the event was acknowledged without its routing");
    assert_eq!(attempts, 1);
    sqlx::query("ALTER TABLE webhook_subscriptions_away RENAME TO webhook_subscriptions")
        .execute(&setup.pool)
        .await
        .expect("restore webhook subscriptions");
    retry_now().await;
    drain(&setup).await;
    let (published, _, _) = state(message).await;
    assert!(published);
}
