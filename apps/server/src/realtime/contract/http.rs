//! The `api` side of the contract (K5.19): tickets, resume and the public
//! key a live service checks them with.
//!
//! - `GET /api/v2/live/ticket[?after=N]`: a ticket for the caller's
//!   connection, starting after `N` (default: the log's head now).
//! - `GET /api/v2/live/resume?after=N`: what the caller missed after `N`
//!   (access checked as of now), and a ticket starting where that ends;
//!   or `resync: true` when `N` is too far back to replay.
//! - `GET /api/v2/live/key`: the public keys, for an outside live service.
//!
//! Both take `documents=page.<id>,sheet.<id>` (K5.21): the ticket names
//! the room of each the caller may see now, checked here, so the live side
//! routes a document's changes and cursors without asking.

use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Query, State},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use super::ticket::{
    Audience, ConnectionKind, Signer, TICKET_LIFETIME_SECONDS, Ticket, document_name,
    parse_document,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::AppError,
    realtime::{
        documents::{DocumentRef, can_view},
        hub::{Replay, replay, subscription_for},
        subscriptions::SubscriptionSet,
        wire::MAX_WATCHED,
    },
};

/// The most events one resume answers with; `more` says to ask again.
const RESUME_EVENTS: usize = 2_000;

#[derive(Debug, Deserialize)]
pub struct AfterQuery {
    /// A log position, as a string or a number.
    after: Option<String>,
    /// Documents to watch, comma separated, each `<kind>.<id>` (K5.21).
    documents: Option<String>,
}

impl AfterQuery {
    /// The documents asked for, at most as many as a connection watches.
    fn documents(&self) -> Result<Vec<DocumentRef>, AppError> {
        let Some(list) = self.documents.as_deref().filter(|list| !list.is_empty()) else {
            return Ok(Vec::new());
        };
        let documents = list
            .split(',')
            .map(|name| parse_document(name.trim()).ok_or(AppError::Validation("documents")))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if documents.len() > MAX_WATCHED {
            return Err(AppError::Validation("documents"));
        }
        Ok(documents.into_iter().collect())
    }

    fn position(&self) -> Result<Option<i64>, AppError> {
        self.after
            .as_deref()
            .map(|text| {
                text.parse::<i64>()
                    .ok()
                    .filter(|position| *position >= 0)
                    .ok_or(AppError::Validation("after"))
            })
            .transpose()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketResponse {
    pub ticket: String,
    /// Where to open the connection.
    pub url: String,
    /// Durable events after this position come over it.
    pub position: String,
    /// Unix seconds; fetch a new ticket before then.
    pub expires_at: i64,
    /// The asked-for documents whose rooms the ticket names: those the
    /// caller may see now.
    pub documents: Vec<String>,
}

/// The documents the account may see now, of those asked for.
async fn viewable(
    state: &AppState,
    account: uuid::Uuid,
    asked: Vec<DocumentRef>,
) -> Result<Vec<DocumentRef>, AppError> {
    let mut granted = Vec::with_capacity(asked.len());
    for document in asked {
        if can_view(&state.pool, account, document).await? {
            granted.push(document);
        }
    }
    Ok(granted)
}

/// The audiences a set names: every workspace, space and restricted
/// conversation it hears, and the account itself.
pub(crate) fn audiences(set: &SubscriptionSet) -> BTreeSet<Audience> {
    set.workspaces
        .iter()
        .map(|id| Audience::Workspace(*id))
        .chain(set.spaces.iter().map(|id| Audience::Space(*id)))
        .chain(set.restricted.iter().map(|id| Audience::Conversation(*id)))
        .chain([Audience::Account(set.account)])
        .collect()
}

/// Where connections open: the live origin when one is set, else the
/// public origin, as a WebSocket address.
fn socket_url(state: &AppState) -> String {
    let origin = state
        .config
        .live_origin
        .clone()
        .unwrap_or_else(|| crate::events::public_origin(state));
    let origin = origin
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            origin
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .unwrap_or(origin);
    format!("{}/live/v1/socket", origin.trim_end_matches('/'))
}

fn issue(
    state: &AppState,
    principal: &AuthenticatedActor,
    set: &SubscriptionSet,
    documents: &[DocumentRef],
    from: i64,
    head: i64,
) -> Result<TicketResponse, AppError> {
    let mut named = audiences(set);
    named.extend(
        documents
            .iter()
            .map(|document| Audience::Document(*document)),
    );
    let signer = Signer::from_config(&state.config)?;
    let expires = chrono::Utc::now().timestamp() + TICKET_LIFETIME_SECONDS;
    let ticket = signer.sign(&Ticket {
        v: 1,
        kid: String::new(),
        account: principal.actor_id,
        session: principal.session_id,
        kind: ConnectionKind::Web,
        audiences: named,
        from,
        expires,
        head,
    });
    Ok(TicketResponse {
        ticket,
        url: socket_url(state),
        position: from.to_string(),
        expires_at: expires,
        documents: documents
            .iter()
            .map(|document| document_name(*document))
            .collect(),
    })
}

async fn head(state: &AppState) -> Result<i64, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    crate::events::head(&mut connection).await
}

pub async fn ticket(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    Query(query): Query<AfterQuery>,
) -> Result<Json<TicketResponse>, AppError> {
    // The head first: what the ticket names is read after it, so a change
    // to the account's access past it is newer than the ticket.
    let head = head(&state).await?;
    let from = query.position()?.map_or(head, |after| after.min(head));
    let set = subscription_for(&state, principal.actor_id, &principal.actor_kind).await?;
    let documents = viewable(&state, principal.actor_id, query.documents()?).await?;
    Ok(Json(issue(
        &state, &principal, &set, &documents, from, head,
    )?))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeResponse {
    /// Protocol 2 frames (`event`, `access`), in order.
    pub events: Vec<serde_json::Value>,
    /// What it read through; ask again from here when `more`.
    pub position: String,
    pub more: bool,
    /// Too far back to replay: refetch, then follow from `position`.
    pub resync: bool,
    #[serde(flatten)]
    pub ticket: TicketResponse,
}

pub async fn resume(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    Query(query): Query<AfterQuery>,
) -> Result<Json<ResumeResponse>, AppError> {
    let after = query.position()?.ok_or(AppError::Validation("after"))?;
    let asked = query.documents()?;
    let cut = head(&state).await?;
    let mut set = subscription_for(&state, principal.actor_id, &principal.actor_kind).await?;
    let mut events = Vec::new();
    let mut position = after;
    let (resync, more) = loop {
        match replay(&state, &principal, &mut set, Some(after), position).await? {
            Replay::Batch {
                frames, through, ..
            } => {
                events.extend(
                    frames
                        .iter()
                        .filter_map(|frame| serde_json::from_str::<serde_json::Value>(frame).ok()),
                );
                position = through;
                if events.len() >= RESUME_EVENTS {
                    break (false, true);
                }
            }
            Replay::Done { head, resumed } => {
                if !resumed {
                    position = head;
                }
                break (!resumed, false);
            }
        }
    };
    let documents = viewable(&state, principal.actor_id, asked).await?;
    let ticket = issue(&state, &principal, &set, &documents, position, cut)?;
    Ok(Json(ResumeResponse {
        events,
        position: position.to_string(),
        more,
        resync,
        ticket,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyResponse {
    pub keys: Vec<PublicKey>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    pub kid: String,
    pub alg: &'static str,
    /// The raw 32-byte Ed25519 key, base64url.
    pub key: String,
}

/// The public key tickets and publishes are signed with now. Public: it
/// only lets someone check a signature.
pub async fn key(State(state): State<AppState>) -> Result<Json<KeyResponse>, AppError> {
    let signer = Signer::from_config(&state.config)?;
    Ok(Json(KeyResponse {
        keys: vec![PublicKey {
            kid: signer.key_id().to_owned(),
            alg: "Ed25519",
            key: URL_SAFE_NO_PAD.encode(signer.public_key()),
        }],
    }))
}
