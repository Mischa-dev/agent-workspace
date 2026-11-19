//! An account's own stream of the log (K5.9), for agents' event streams
//! and long-polls (A2 owns the endpoints): what the account may hear,
//! from a position, then live.
//!
//! The stream is the log read with a cursor, so any instance can serve it
//! and there is no switch between catching up and live to lose or repeat
//! an event across: this instance's tail only says when to read again (a
//! second's poll covers an instance whose wake is off). What the account
//! may hear is worked out at the start and again on every access change
//! about it, as a socket's is (K5.8). Presence and typing it may see come
//! along as they happen; a heartbeat every 15 seconds keeps idle streams
//! open.

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use futures_util::Stream;
use tokio::sync::broadcast::{Receiver, error::RecvError};
use uuid::Uuid;

use super::{
    hub::{LiveState, RealtimeDelivery, access_changed, live_frames, subscription_for},
    subscriptions::{Hearing, Route, RouteAudience, SubscriptionSet},
    wire::RESUME_LIMIT,
};
use crate::{AppState, error::AppError, events::LoggedEvent};

/// How often an idle stream says it is still there.
pub const HEARTBEAT: Duration = Duration::from_secs(15);
/// Read again this often even when nothing says to.
const POLL: Duration = Duration::from_secs(1);
const BATCH: i64 = 500;

/// Whose stream, and which of their events it carries.
#[derive(Clone)]
pub struct FollowSpec {
    pub account_id: Uuid,
    /// The account's kind, for the realtime policy's role rule.
    pub actor_kind: String,
    /// Narrows what the account may hear (A2: event kinds, reasons).
    pub filter: Arc<dyn Fn(&LoggedEvent) -> bool + Send + Sync>,
    /// Which of its workspaces the stream carries (R19); every workspace
    /// the account is in when None. One cursor walks them all in the log's
    /// one order, and each event names its workspace.
    pub workspaces: Option<HashSet<Uuid>>,
    /// Whether it carries DMs and groups too (the direct scope, R20).
    pub direct: bool,
}

impl FollowSpec {
    /// Every workspace the account is in, and its DMs and groups.
    pub fn everything(
        account_id: Uuid,
        actor_kind: impl Into<String>,
        filter: Arc<dyn Fn(&LoggedEvent) -> bool + Send + Sync>,
    ) -> Self {
        Self {
            account_id,
            actor_kind: actor_kind.into(),
            filter,
            workspaces: None,
            direct: true,
        }
    }

    /// Whether the stream asked for this event's scope: the direct scope's
    /// (a DM or group, or anything K4.23 files under its id) only when it
    /// includes DMs, any other only when it names its workspace.
    fn asked_for(&self, event: &LoggedEvent, route: &Route, set: &SubscriptionSet) -> bool {
        let direct = event.workspace_id == crate::events::DIRECT_SCOPE_ID
            || matches!(
                route.audience,
                RouteAudience::Conversation { root, restricted: true } if set.direct.contains(&root)
            );
        if direct {
            self.direct
        } else {
            self.workspaces
                .as_ref()
                .is_none_or(|workspaces| workspaces.contains(&event.workspace_id))
        }
    }
}

#[derive(Clone, Debug)]
pub enum FollowItem {
    Event(Arc<LoggedEvent>),
    /// A presence or typing frame, as protocol 2 sends it.
    Live(String),
    Heartbeat,
    /// Too far behind to catch up from the log: refetch, then follow from
    /// `position`. The stream ends after it.
    Resync {
        position: i64,
    },
}

/// The account's events after `after`, then live, until dropped.
pub fn follow(
    state: AppState,
    spec: FollowSpec,
    after: i64,
) -> impl Stream<Item = Result<FollowItem, AppError>> + Send {
    let follower = Follower {
        receiver: state.realtime.subscribe(),
        state,
        spec,
        cursor: after,
        set: None,
        pending: VecDeque::new(),
        heartbeat: tokio::time::Instant::now() + HEARTBEAT,
        done: false,
    };
    futures_util::stream::unfold(follower, |mut follower| async move {
        let item = follower.next().await?;
        Some((item, follower))
    })
}

struct Follower {
    state: AppState,
    spec: FollowSpec,
    receiver: Receiver<RealtimeDelivery>,
    cursor: i64,
    /// None until the first read works it out.
    set: Option<SubscriptionSet>,
    pending: VecDeque<FollowItem>,
    heartbeat: tokio::time::Instant,
    done: bool,
}

impl Follower {
    async fn next(&mut self) -> Option<Result<FollowItem, AppError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                if matches!(item, FollowItem::Resync { .. }) {
                    self.done = true;
                }
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            if self.set.is_none() {
                if let Err(error) = self.start().await {
                    self.done = true;
                    return Some(Err(error));
                }
                continue;
            }
            match self.read().await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
            self.wait().await;
        }
    }

    /// Works out the set and checks the start isn't too far back.
    async fn start(&mut self) -> Result<(), AppError> {
        self.set = Some(subscription_for(&self.state, self.spec.account_id).await?);
        let mut connection = crate::db::scoped_tx(&self.state.pool).await?;
        let head = crate::events::head(&mut connection).await?;
        if head - self.cursor > RESUME_LIMIT || self.cursor < 0 {
            self.pending
                .push_back(FollowItem::Resync { position: head });
        }
        Ok(())
    }

    /// Reads the next batch; true when it found anything.
    async fn read(&mut self) -> Result<bool, AppError> {
        let events = {
            let mut connection = crate::db::scoped_tx(&self.state.pool).await?;
            crate::events::tail(&mut connection, self.cursor, BATCH).await?
        };
        if events.is_empty() {
            return Ok(false);
        }
        for event in events {
            self.cursor = event.position;
            if self.heard(&event).await? {
                self.pending.push_back(FollowItem::Event(Arc::new(event)));
            }
        }
        self.heartbeat = tokio::time::Instant::now() + HEARTBEAT;
        Ok(true)
    }

    async fn heard(&mut self, event: &LoggedEvent) -> Result<bool, AppError> {
        let account = self.spec.account_id;
        let route = Route::of_logged(event);
        let set = self.set.as_ref().expect("set worked out");
        let changed = access_changed(event) == Some(account)
            || route.as_ref().is_some_and(|route| set.changed_by(route));
        if changed {
            self.set = Some(subscription_for(&self.state, account).await?);
        }
        let Some(route) = route else {
            return Ok(false);
        };
        let set = self.set.as_ref().expect("set worked out");
        if !self.spec.asked_for(event, &route, set) {
            return Ok(false);
        }
        let heard = match set.hears(&route) {
            Hearing::Yes => true,
            Hearing::No => false,
            Hearing::IfBoardVisible(board) => {
                crate::whiteboards::visible_to(&self.state.pool, board, account).await?
            }
        };
        Ok(heard && (self.spec.filter)(event))
    }

    /// Until the tail says there is more, a live frame arrives, a second
    /// passes or a heartbeat is due.
    async fn wait(&mut self) {
        let poll = tokio::time::sleep(POLL);
        tokio::pin!(poll);
        loop {
            tokio::select! {
                delivery = self.receiver.recv() => match delivery {
                    Ok(RealtimeDelivery::Live(frame)) => {
                        let set = self.set.as_ref().expect("set worked out");
                        let frames = live_frames(
                            &frame,
                            set,
                            &LiveState::default(),
                            Uuid::nil(),
                            self.spec.account_id,
                        );
                        if frames.is_empty() {
                            continue;
                        }
                        self.pending.extend(frames.into_iter().map(FollowItem::Live));
                        return;
                    }
                    Ok(RealtimeDelivery::Durable { .. } | RealtimeDelivery::AccessChanged(_))
                    | Err(RecvError::Lagged(_)) => return,
                    Ok(_) => continue,
                    Err(RecvError::Closed) => {
                        self.done = true;
                        return;
                    }
                },
                () = &mut poll => return,
                () = tokio::time::sleep_until(self.heartbeat) => {
                    self.heartbeat = tokio::time::Instant::now() + HEARTBEAT;
                    self.pending.push_back(FollowItem::Heartbeat);
                    return;
                }
            }
        }
    }
}
