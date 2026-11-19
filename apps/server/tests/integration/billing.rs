//! H1.33: the billing seam. The silent, costly failures: a webhook
//! replayed and applied twice, deliveries out of order leaving a workspace
//! on a plan the provider no longer has it on, a forged delivery taken,
//! and a billing route open on a server with no provider.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_workspace_server::{
    accounts::{NewPerson, create_person},
    billing::{
        self, BillingError, BillingEvent, EventKind, MerchantOfRecord, Subscription,
        SubscriptionState,
    },
    workspace::lifecycle::{self, NewWorkspace},
};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use serde_json::{Value, json};

use crate::support::{
    app::{app, send},
    db::db,
};

/// A provider whose subscriptions the test sets, and whose deliveries are
/// signed by a header.
#[derive(Default)]
struct Provider {
    subscriptions: Mutex<HashMap<String, Subscription>>,
}

#[async_trait]
impl MerchantOfRecord for Provider {
    fn id(&self) -> &'static str {
        "test"
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<BillingEvent, BillingError> {
        if headers
            .get("x-test-signature")
            .and_then(|value| value.to_str().ok())
            != Some("signed")
        {
            return Err(BillingError::Signature);
        }
        let body: Value = serde_json::from_slice(body).map_err(|_| BillingError::Ignored)?;
        let kind = match body["kind"].as_str() {
            Some("started") => EventKind::SubscriptionStarted,
            Some("changed") => EventKind::SubscriptionChanged,
            Some("ended") => EventKind::SubscriptionEnded,
            _ => return Err(BillingError::Ignored),
        };
        Ok(BillingEvent {
            provider_event_id: body["id"].as_str().unwrap_or_default().to_owned(),
            kind,
            subscription_ref: body["subscription"].as_str().unwrap_or_default().to_owned(),
        })
    }

    async fn subscription(&self, reference: &str) -> Result<Option<Subscription>, BillingError> {
        Ok(self
            .subscriptions
            .lock()
            .expect("lock")
            .get(reference)
            .cloned())
    }
}

fn delivery(id: &str, kind: &str, signed: bool) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/billing/webhook")
        .header("content-type", "application/json");
    if signed {
        request = request.header("x-test-signature", "signed");
    }
    request
        .body(Body::from(
            json!({ "id": id, "kind": kind, "subscription": "sub_1" }).to_string(),
        ))
        .expect("request")
}

#[tokio::test]
async fn a_delivery_counts_once_and_the_provider_s_state_wins() {
    let Some(test) = db("h1_33_billing").await else {
        return;
    };
    let pool = &test.pool;
    let (server, state) = app(&test).build();

    // No provider: no route.
    let (status, _) = send(&server, delivery("evt_0", "started", true)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "no billing route without a provider"
    );

    let mut transaction = pool.begin().await.expect("begin");
    let person = create_person(
        &mut transaction,
        NewPerson {
            login_name: "payer",
            display_name: "Payer",
            default_workspace: None,
            created_by: None,
            claimed: true,
        },
    )
    .await
    .expect("person");
    let workspace = lifecycle::create(
        &mut transaction,
        person.account_id,
        NewWorkspace {
            name: "Paying".to_owned(),
            slug: None,
            id: None,
        },
    )
    .await
    .expect("workspace");
    sqlx::query("INSERT INTO plans (id, name, limits) VALUES ('team', 'Team', '{\"people\": 50}')")
        .execute(&mut *transaction)
        .await
        .expect("a paid plan");
    transaction.commit().await.expect("commit");

    let provider = Arc::new(Provider::default());
    let billing = billing::router(provider.clone()).with_state(state.clone());
    let subscription = |plan: &str, subscription_state, seats| Subscription {
        subscription_ref: "sub_1".to_owned(),
        customer_ref: "cus_1".to_owned(),
        workspace_id: workspace,
        plan_id: plan.to_owned(),
        state: subscription_state,
        seats,
        period_ends_at: None,
        trial_ends_at: None,
    };
    let plan = || async {
        sqlx::query_as::<_, (String, String, Option<i32>, i64)>(
            "SELECT plan_id, status, seats, version FROM workspace_plans WHERE workspace_id = $1",
        )
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("plan")
    };

    // A forged delivery changes nothing.
    provider.subscriptions.lock().expect("lock").insert(
        "sub_1".to_owned(),
        subscription("team", SubscriptionState::Active, 3),
    );
    let (status, _) = send(&billing, delivery("evt_1", "started", false)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(plan().await.0, "self_hosted");

    // Signed: the workspace is on the plan the provider says.
    let (status, _) = send(&billing, delivery("evt_1", "started", true)).await;
    assert_eq!(status, StatusCode::OK);
    let (plan_id, status_word, seats, version) = plan().await;
    assert_eq!(
        (plan_id.as_str(), status_word.as_str(), seats),
        ("team", "active", Some(3))
    );

    // The same delivery again changes nothing.
    let (status, _) = send(&billing, delivery("evt_1", "started", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan().await.3, version, "a replay writes nothing");
    let stored = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM billing_events")
        .fetch_one(pool)
        .await
        .expect("events");
    assert_eq!(stored, 1);

    // Out of order: the subscription ended, then an older "changed" arrives
    // late. Each reads the provider's current state, so it stays ended.
    provider.subscriptions.lock().expect("lock").insert(
        "sub_1".to_owned(),
        subscription("team", SubscriptionState::Ended, 3),
    );
    let (status, _) = send(&billing, delivery("evt_3", "ended", true)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&billing, delivery("evt_2", "changed", true)).await;
    assert_eq!(status, StatusCode::OK);
    let (plan_id, status_word, _, _) = plan().await;
    assert_eq!(
        (plan_id.as_str(), status_word.as_str()),
        ("free", "active"),
        "ended: back on the free plan, nothing taken away"
    );
    let subscription_ref = sqlx::query_scalar::<_, Option<String>>(
        "SELECT subscription_ref FROM workspace_plans WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("reference");
    assert_eq!(subscription_ref, None);

    // A delivery for a subscription the provider doesn't have is taken and
    // dropped.
    provider.subscriptions.lock().expect("lock").clear();
    let (status, _) = send(&billing, delivery("evt_4", "changed", true)).await;
    assert_eq!(status, StatusCode::OK);

    test.finish().await;
}
