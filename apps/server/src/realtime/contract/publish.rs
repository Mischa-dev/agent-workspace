//! The database side of the contract (K5.19): what is published, to which
//! audiences. Every live instance of this server feeds its own router from
//! its tail and the ephemeral bus; with `AGENT_WORKSPACE_LIVE_URL` one
//! `work` instance (holding `live.publisher`) sends the log there instead,
//! and the instance that makes an ephemeral frame sends it too.

use std::time::Duration;

use tokio::sync::watch;

use super::{
    live::{Publish, PublishBody, SIGNATURE_HEADER},
    ticket::{Audience, Signer},
};
use crate::{
    AppState,
    cluster::{Cluster, Lease, LeaseName, while_held},
    error::AppError,
    events::LoggedEvent,
    outbox::OutboxEnvelope,
    realtime::{
        ephemeral::EphemeralFrame,
        hub::{access_changed, checked_delivery, contract_frame},
        subscriptions::{Route, RouteAudience},
        wire,
    },
};

/// How often the publisher reads the log without a wake.
const POLL: Duration = Duration::from_millis(250);
const BATCH: i64 = 500;

/// Who hears an event routed by `route`: the audience a ticket names for
/// the same readers. A board's events (`thing`) go to its space, or to its
/// workspace when it has none, which is exactly who may see a board.
pub(crate) fn audiences_of(route: &Route) -> Vec<Audience> {
    let space = || route.space.map(Audience::Space).into_iter().collect();
    match route.audience {
        RouteAudience::Workspace => vec![Audience::Workspace(route.workspace)],
        RouteAudience::Space => space(),
        RouteAudience::Conversation {
            root,
            restricted: true,
        } => vec![Audience::Conversation(root)],
        RouteAudience::Conversation {
            restricted: false, ..
        } => space(),
        RouteAudience::Account(account) => vec![Audience::Account(account)],
        RouteAudience::Thing(_) => route.space.map_or_else(
            || vec![Audience::Workspace(route.workspace)],
            |space| vec![Audience::Space(space)],
        ),
    }
}

/// Whose tickets an event makes stale: the connections whose set it may
/// change (what `SubscriptionSet::changed_by` asks of each).
fn stale_audiences(route: &Route) -> Vec<Audience> {
    if !route.changes_access() {
        return Vec::new();
    }
    let mut audiences = route
        .about_account
        .map(Audience::Account)
        .into_iter()
        .collect::<Vec<_>>();
    match route.audience {
        RouteAudience::Conversation {
            root,
            restricted: true,
        } => {
            audiences.push(Audience::Workspace(route.workspace));
            audiences.push(Audience::Conversation(root));
        }
        RouteAudience::Space | RouteAudience::Workspace
            if route.event_type != "space.member.changed" =>
        {
            audiences.push(Audience::Workspace(route.workspace));
            audiences.extend(route.space.map(Audience::Space));
        }
        _ => {}
    }
    audiences
}

/// What a logged event publishes: its frame for its audiences, then, when
/// it changes who may hear what, `ticket.stale` for those it may change.
pub(crate) fn of_logged(event: &LoggedEvent) -> Vec<Publish> {
    if let Some(account) = access_changed(event) {
        return vec![Publish::Stale {
            audiences: vec![Audience::Account(account)],
        }];
    }
    let Some(route) = Route::of_logged(event) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    if let Some(envelope) = OutboxEnvelope::from_logged(event)
        && let Ok(delivery) = checked_delivery(&envelope, route.clone())
        && let Ok(frame) = contract_frame(&delivery)
    {
        let audiences = envelope.recipient_actor_id.map_or_else(
            || audiences_of(&route),
            |account| vec![Audience::Account(account)],
        );
        items.push(Publish::Frame {
            audiences,
            position: Some(event.position),
            except_account: None,
            frame,
        });
    }
    let stale = stale_audiences(&route);
    if !stale.is_empty() {
        items.push(Publish::Stale { audiences: stale });
    }
    items
}

/// What an ephemeral frame publishes. Awareness and document changes go to
/// the document's room (K5.21); a cache invalidation stays on the bus.
pub(crate) fn of_ephemeral(frame: &EphemeralFrame) -> Vec<Publish> {
    match frame {
        EphemeralFrame::Presence {
            account,
            workspaces,
            online,
        } => workspaces
            .iter()
            .map(|workspace| Publish::Frame {
                audiences: vec![Audience::Workspace(*workspace)],
                position: None,
                except_account: Some(*account),
                frame: wire::presence(*workspace, *account, *online),
            })
            .collect(),
        EphemeralFrame::Typing {
            space,
            root,
            restricted,
            conversation,
            account,
            until,
        } => vec![Publish::Frame {
            audiences: vec![if *restricted {
                Audience::Conversation(*root)
            } else {
                Audience::Space(*space)
            }],
            position: None,
            except_account: Some(*account),
            frame: wire::typing(*conversation, *account, until),
        }],
        EphemeralFrame::StreamDelta {
            space,
            conversation,
            payload,
            route: Some((root, restricted)),
        } => vec![Publish::Frame {
            audiences: vec![if *restricted {
                Audience::Conversation(*root)
            } else {
                Audience::Space(*space)
            }],
            position: None,
            except_account: None,
            frame: wire::stream_delta(*conversation, payload),
        }],
        EphemeralFrame::SessionRevoked { session } => {
            vec![Publish::Revoke { session: *session }]
        }
        // Each watcher drops its own connection's cursor by `connection`.
        EphemeralFrame::Awareness {
            document,
            connection,
            account,
            state,
        } => vec![Publish::Frame {
            audiences: vec![Audience::Document(*document)],
            position: None,
            except_account: None,
            frame: wire::awareness(*document, *connection, *account, state),
        }],
        EphemeralFrame::DocumentUpdate {
            document,
            generation,
            version,
            update,
        } => vec![Publish::Frame {
            audiences: vec![Audience::Document(*document)],
            position: None,
            except_account: None,
            frame: wire::document_update(*document, *generation, *version, update),
        }],
        EphemeralFrame::StreamDelta { route: None, .. } | EphemeralFrame::Invalidate { .. } => {
            Vec::new()
        }
    }
}

/// Sends items to the outside live service, signed.
pub(crate) struct Remote {
    endpoint: url::Url,
    client: reqwest::Client,
    signer: Signer,
}

impl Remote {
    pub(crate) async fn connect(state: &AppState) -> Result<Option<Self>, AppError> {
        let Some(base) = state.config.live_url.as_ref() else {
            return Ok(None);
        };
        let endpoint = base
            .join("live/v1/publish")
            .map_err(|_| AppError::Internal("the live service's address"))?;
        // The operator's own service: a private address is expected.
        let client = crate::outbound::client_for(
            &endpoint,
            true,
            crate::outbound::Timeouts {
                connect: Duration::from_secs(2),
                total: Some(Duration::from_secs(5)),
            },
            None,
        )
        .await
        .map_err(|_| AppError::Internal("reach the live service"))?;
        Ok(Some(Self {
            endpoint,
            client,
            signer: Signer::from_config(&state.config)?,
        }))
    }

    pub(crate) async fn send(&self, items: Vec<Publish>) -> Result<(), AppError> {
        if items.is_empty() {
            return Ok(());
        }
        let body = serde_json::to_vec(&PublishBody {
            sent_at: chrono::Utc::now().timestamp(),
            items,
        })
        .map_err(|_| AppError::Internal("encode a publish"))?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(SIGNATURE_HEADER, self.signer.sign_body(&body))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| AppError::Internal("reach the live service"))?;
        if !response.status().is_success() {
            return Err(AppError::Internal("the live service refused a publish"));
        }
        Ok(())
    }
}

/// An ephemeral frame to the outside live service, when there is one. A
/// failure is logged: the frame is only a shortcut.
pub(crate) async fn send_ephemeral(state: &AppState, frame: &EphemeralFrame) {
    if state.config.live_url.is_none() {
        return;
    }
    let items = of_ephemeral(frame);
    if items.is_empty() {
        return;
    }
    let sent = match Remote::connect(state).await {
        Ok(Some(remote)) => remote.send(items).await,
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    };
    if let Err(error) = sent {
        tracing::warn!(
            error_category = error.log_category(),
            "ephemeral frame not sent to the live service"
        );
    }
}

/// The publisher's cursor, a row among the log's consumers: a new holder
/// carries on where the last one stopped, so the live service never misses
/// a position.
const CURSOR: &str = "live.publisher";

/// On a `work` instance with an outside live service: whoever holds
/// `live.publisher` sends the log there in position order, carrying on
/// from the cursor (the first holder starts at the head; connections
/// further back resume through `api`).
pub async fn run(
    state: AppState,
    cluster: Cluster,
    wake: std::sync::Arc<tokio::sync::Notify>,
    shutdown: watch::Receiver<bool>,
) {
    let holder = cluster.clone();
    while_held(cluster, LeaseName::LivePublisher, shutdown, move |lease| {
        let state = state.clone();
        let cluster = holder.clone();
        let wake = std::sync::Arc::clone(&wake);
        async move {
            crate::tenancy::system(
                "events.log",
                publish_while_held(&state, &cluster, &wake, &lease),
            )
            .await
        }
    })
    .await;
}

async fn publish_while_held(
    state: &AppState,
    cluster: &Cluster,
    wake: &tokio::sync::Notify,
    lease: &Lease,
) -> Result<(), AppError> {
    let Some(remote) = Remote::connect(state).await? else {
        lease.lost.cancelled().await;
        return Ok(());
    };
    let mut cursor = {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        sqlx::query(
            "INSERT INTO event_consumers (name, position) \
             SELECT $1, next_position - 1 FROM event_position_counter \
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(CURSOR)
        .execute(&mut *transaction)
        .await?;
        let position =
            sqlx::query_scalar::<_, i64>("SELECT position FROM event_consumers WHERE name = $1")
                .bind(CURSOR)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        position
    };
    loop {
        if lease.lost.is_cancelled() {
            return Ok(());
        }
        let events = {
            let mut connection = crate::db::scoped_tx(&state.pool).await?;
            crate::events::tail(&mut connection, cursor, BATCH).await?
        };
        if let Some(last) = events.last().map(|event| event.position) {
            let items = events.iter().flat_map(of_logged).collect::<Vec<_>>();
            if let Err(error) = remote.send(items).await {
                tracing::warn!(
                    error_category = error.log_category(),
                    "live service publish failed; retrying"
                );
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    () = lease.lost.cancelled() => {}
                }
                continue;
            }
            let mut transaction = crate::db::scoped_tx(&state.pool).await?;
            cluster.fence(&mut transaction, lease).await?;
            sqlx::query(
                "UPDATE event_consumers SET position = $2, updated_at = now() WHERE name = $1",
            )
            .bind(CURSOR)
            .bind(last)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            cursor = last;
            continue;
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(POLL) => {}
            () = lease.lost.cancelled() => {}
        }
    }
}
