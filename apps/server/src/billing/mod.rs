//! The billing seam (H1.33): the hook a merchant of record plugs into, and
//! nothing behind it. A merchant of record handles checkout, tax, invoices
//! and card data; this server never sees a card. No provider is built in,
//! so no billing route exists: the first provider is a module under
//! `billing/` implementing [`MerchantOfRecord`], with its JSON kept there,
//! and mounts [`router`].
//!
//! In: the provider's signed webhook names a subscription. The delivery is
//! stored once by its id (`billing_events`), so a replay changes nothing;
//! the subscription's current state is read from the provider, never from
//! the delivery, and written by [`crate::metering::apply_billing_state`],
//! so deliveries arriving out of order end in the provider's current state.
//! Checkout, the customer portal, seat sync, trials and dunning come with
//! the hosted business (H1.34) and add to the trait then.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{AppState, error::AppError};

/// What a provider's webhook said happened. Only the subscription it names
/// is trusted, and only to look it up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BillingEvent {
    /// The provider's id for this delivery: a replay carries the same one.
    pub provider_event_id: String,
    pub kind: EventKind,
    pub subscription_ref: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    SubscriptionStarted,
    SubscriptionChanged,
    PaymentFailed,
    PaymentSucceeded,
    SubscriptionEnded,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionStarted => "subscription_started",
            Self::SubscriptionChanged => "subscription_changed",
            Self::PaymentFailed => "payment_failed",
            Self::PaymentSucceeded => "payment_succeeded",
            Self::SubscriptionEnded => "subscription_ended",
        }
    }
}

/// Where a subscription stands, as the provider says now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionState {
    Trialing,
    Active,
    /// A payment failed and the provider is retrying.
    PastDue,
    /// Over: the workspace goes back to the free plan, keeping everything.
    Ended,
}

/// A subscription's current state, read from the provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub subscription_ref: String,
    pub customer_ref: String,
    /// The workspace it pays for, as given at checkout.
    pub workspace_id: Uuid,
    /// One of this server's plan ids.
    pub plan_id: String,
    pub state: SubscriptionState,
    pub seats: u32,
    pub period_ends_at: Option<DateTime<Utc>>,
    pub trial_ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    /// The delivery isn't signed by the provider.
    #[error("the delivery's signature doesn't match")]
    Signature,
    /// A delivery this seam doesn't act on; it is acknowledged and dropped.
    #[error("a delivery that changes no subscription")]
    Ignored,
    /// The provider couldn't be asked; it will deliver again.
    #[error("the billing provider couldn't be reached")]
    Unavailable,
}

/// A merchant of record. Provider-specific JSON stays in its module.
#[async_trait]
pub trait MerchantOfRecord: Send + Sync {
    /// Its id, stored with each delivery and subscription.
    fn id(&self) -> &'static str;
    /// Checks a webhook delivery's signature and reads what it names.
    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<BillingEvent, BillingError>;
    /// A subscription as the provider holds it now; `None` if it has none.
    async fn subscription(
        &self,
        subscription_ref: &str,
    ) -> Result<Option<Subscription>, BillingError>;
}

/// The webhook route for `provider`: `POST /api/v1/billing/webhook`. Not
/// mounted while no provider is configured.
pub fn router(provider: Arc<dyn MerchantOfRecord>) -> Router<AppState> {
    Router::new().route(
        "/api/v1/billing/webhook",
        post(
            move |State(state): State<AppState>, headers: HeaderMap, body: Bytes| {
                let provider = provider.clone();
                async move { receive(&state, provider.as_ref(), &headers, &body).await }
            },
        ),
    )
}

/// Takes one delivery: checked, looked up, stored once, applied.
pub async fn receive(
    state: &AppState,
    provider: &dyn MerchantOfRecord,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<StatusCode, AppError> {
    let event = match provider.verify_webhook(headers, body) {
        Ok(event) => event,
        Err(BillingError::Ignored) => return Ok(StatusCode::OK),
        Err(BillingError::Signature) => return Err(AppError::Invalid("billing signature")),
        Err(BillingError::Unavailable) => {
            return Err(AppError::Internal("billing provider unavailable"));
        }
    };
    // Read before any transaction: no transaction waits on the provider.
    let fetched_at = Utc::now();
    let Some(subscription) = provider
        .subscription(&event.subscription_ref)
        .await
        .map_err(|_| AppError::Internal("billing provider unavailable"))?
    else {
        return Ok(StatusCode::OK);
    };
    if subscription.subscription_ref != event.subscription_ref {
        return Err(AppError::Internal(
            "billing provider answered for another subscription",
        ));
    }
    crate::tenancy::system("billing.webhook", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        let known = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1)",
        )
        .bind(subscription.workspace_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !known {
            // A workspace deleted since: nothing to change.
            return Ok(StatusCode::OK);
        }
        let stored = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO billing_events (id, workspace_id, provider, provider_event_id, kind) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (provider, provider_event_id) DO NOTHING RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(subscription.workspace_id)
        .bind(provider.id())
        .bind(&event.provider_event_id)
        .bind(event.kind.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(stored) = stored else {
            // The same delivery again: already taken.
            return Ok(StatusCode::OK);
        };
        crate::metering::apply_billing_state(
            &mut transaction,
            provider.id(),
            &subscription,
            fetched_at,
        )
        .await?;
        sqlx::query("UPDATE billing_events SET applied_at = now() WHERE id = $1")
            .bind(stored)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(StatusCode::OK)
    })
    .await
}
