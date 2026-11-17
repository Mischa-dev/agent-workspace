//! The log's side effects (K3.9, K3.4): a batch whose notifications or
//! routing fail is retried, not passed, so its inbox rows, pushes, emails
//! and webhooks aren't lost after one transient error; once it works, each
//! lands exactly once.

use serde_json::json;
use uuid::Uuid;

use super::{
    mcp_admin::{create_project, setup},
    message_edits::{add_person, create_channel, say},
    notifications::drain,
};

/// Whether a log consumer's cursor is still before the message's event.
async fn behind(pool: &sqlx::PgPool, consumer: &str, message: &str) -> bool {
    let message = message.parse::<Uuid>().expect("message id");
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT COALESCE((SELECT position FROM event_consumers WHERE name = $1), 0)
             < (SELECT position FROM events WHERE type = 'message.created' AND thing_id = $2)
        "#,
    )
    .bind(consumer)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("cursor and event")
}

#[tokio::test]
async fn a_batch_whose_side_effects_fail_is_retried_not_passed() {
    let Some(setup) = setup("log_side_effects").await else {
        return;
    };
    let project = create_project(&setup, "OSE", "Side effects").await;
    let general = create_channel(&setup, &project, "general").await;
    let (mara, _) = add_person(&setup, &[&project], "mara").await;
    drain(&setup).await;
    let mention = |text: &str| {
        json!({
            "version": 1,
            "nodes": [
                {"type": "text", "text": text},
                {"type": "mention", "actorId": mara}
            ]
        })
    };

    // Notifications can't be written: the notifier stays before the event.
    let message = say(&setup, &setup.browser, &general, "m1", mention("Look ")).await;
    sqlx::query("ALTER TABLE notifications RENAME TO notifications_away")
        .execute(&setup.pool)
        .await
        .expect("rename notifications");
    for _ in 0..3 {
        agent_workspace_server::events::catch_up(&setup.pool)
            .await
            .expect("catch up");
    }
    assert!(
        behind(&setup.pool, "notifications", &message).await,
        "the notifier passed the event without its notifications"
    );

    // Back again, the retry delivers exactly one notification.
    sqlx::query("ALTER TABLE notifications_away RENAME TO notifications")
        .execute(&setup.pool)
        .await
        .expect("restore notifications");
    drain(&setup).await;
    drain(&setup).await;
    assert!(!behind(&setup.pool, "notifications", &message).await);
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
    agent_workspace_server::events::catch_up(&setup.pool)
        .await
        .expect("catch up");
    assert!(
        behind(&setup.pool, "router", &message).await,
        "the router passed the event without routing it"
    );
    sqlx::query("ALTER TABLE webhook_subscriptions_away RENAME TO webhook_subscriptions")
        .execute(&setup.pool)
        .await
        .expect("restore webhook subscriptions");
    drain(&setup).await;
    assert!(!behind(&setup.pool, "router", &message).await);
}
