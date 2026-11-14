//! Events: the one ordered log of everything that changed (K3 3.3), and
//! the routing of workspace events to webhooks and automations (`route`).
//!
//! A change writes its event in its own transaction with [`emit`]; the
//! sequencer (`sequencer`) gives committed events their positions, dense and
//! in commit order, so a reader that follows positions never skips one.
//! Consumers (`consumer`) read the log with their own cursor; the live tail
//! (K5) reads it with [`tail`]. Events carry ids, kinds and the keys of what
//! changed, never text (AGENTS.md rule 10): readers fetch content live with
//! their own access.
//!
//! Until every writer emits its own events, `outbox::enqueue` mirrors each
//! outbox event into the log with the same id (`legacy`), so every outbox
//! writer is in the log without touching its call site.

pub mod catalog;
pub mod legacy;
mod route;

pub use catalog::{EventClass, EventType};
pub use route::*;

use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// Who is acting in this transaction, for every event it emits: the
/// account, the credential (and so the app) it came through, who asked for
/// it, the run it is part of and the caller's request id. K2's runner sets
/// it for operations; older write paths set it themselves.
#[derive(Clone, Debug, Default)]
pub struct EventContext {
    pub actor: Option<Uuid>,
    /// The connected app's OAuth grant, until K1's credentials (K1.22).
    pub via_credential: Option<Uuid>,
    pub requested_by: Option<Uuid>,
    pub run: Option<Uuid>,
    pub request_id: Option<String>,
}

impl EventContext {
    /// An account acting, possibly through a connected app.
    pub fn actor(actor: Uuid, via_credential: Option<Uuid>) -> Self {
        Self {
            actor: Some(actor),
            via_credential,
            ..Self::default()
        }
    }
}

/// Stores the context in transaction-local settings (`set_config(..., true)`
/// only, ruling R24 rule 3), so shared command code and SQL triggers read it
/// without new arguments. `aw.via_grant` is the existing setting
/// `current_via_grant()` reads (migration 0042).
pub async fn set_context(
    transaction: &mut Transaction<'_, Postgres>,
    context: &EventContext,
) -> Result<(), AppError> {
    let text = |value: Option<Uuid>| value.map(|value| value.to_string()).unwrap_or_default();
    sqlx::query(
        r#"
        SELECT set_config('aw.actor', $1, true),
               CASE WHEN $2 <> '' THEN set_config('aw.via_grant', $2, true) END,
               set_config('aw.requested_by', $3, true),
               set_config('aw.run', $4, true),
               set_config('aw.request_id', $5, true)
        "#,
    )
    .bind(text(context.actor))
    .bind(text(context.via_credential))
    .bind(text(context.requested_by))
    .bind(text(context.run))
    .bind(
        context
            .request_id
            .as_deref()
            .map(|id| id.chars().take(128).collect::<String>())
            .unwrap_or_default(),
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Names `actor` as who acts in this transaction unless an edge (the
/// operation runner, the MCP wrapper) already said, for shared command code
/// that knows its actor but not the edge.
pub async fn ensure_actor(connection: &mut PgConnection, actor: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "SELECT set_config('aw.actor', $1, true) \
         WHERE COALESCE(current_setting('aw.actor', true), '') = ''",
    )
    .bind(actor.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Who may hear an event (ruling R8). Exactly one per event; the log's
/// columns follow from it, and K5 routes live updates on them with no query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Audience {
    /// Every member who can see the workspace.
    Workspace,
    /// Members of the event's space.
    Space,
    /// A conversation's readers: `root` is the channel a thread is in (or
    /// the conversation itself); `restricted` when only its members may read
    /// it (a DM, a group, a private channel or a thread in one).
    Conversation { root: Uuid, restricted: bool },
    /// The root and restriction of this conversation, looked up as the
    /// event is staged.
    ConversationOf(Uuid),
    /// One account only: its notifications, its read marks, a wake-up.
    Account(Uuid),
    /// Whoever may see the access thing (else the thing) now, checked per
    /// event: workspace boards today.
    Thing,
}

impl Audience {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Space => "space",
            Self::Conversation { .. } | Self::ConversationOf(_) => "conversation",
            Self::Account(_) => "account",
            Self::Thing => "thing",
        }
    }
}

/// One event to stage.
#[derive(Clone, Debug)]
pub struct NewEvent<'a> {
    pub id: Uuid,
    pub ty: &'static EventType,
    /// The workspace; looked up from `space_id` when not given.
    pub workspace_id: Option<Uuid>,
    pub space_id: Option<Uuid>,
    pub thing: Option<(Uuid, &'a str)>,
    pub access_thing: Option<Uuid>,
    /// The thing's version after this change, when it has one.
    pub version: Option<i64>,
    /// Keys of what changed ("status", "title", a field id); at most 64.
    pub changes: &'a [&'a str],
    pub audience: Audience,
    /// Ids, keys and small non-text values only; at most 8 KiB.
    pub payload: serde_json::Value,
    /// Who acted, when the transaction's context doesn't say.
    pub actor: Option<Uuid>,
    /// The outbox's name for it, while the web still reads that.
    pub legacy_type: Option<&'static str>,
}

impl<'a> NewEvent<'a> {
    /// The common case: a fact about one thing, heard by its space.
    pub fn thing(ty: &'static EventType, space_id: Uuid, kind: &'a str, id: Uuid) -> Self {
        Self {
            id: Uuid::now_v7(),
            ty,
            workspace_id: None,
            space_id: Some(space_id),
            thing: Some((id, kind)),
            access_thing: None,
            version: None,
            changes: &[],
            audience: Audience::Space,
            payload: serde_json::json!({}),
            actor: None,
            legacy_type: None,
        }
    }

    pub fn payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn audience(mut self, audience: Audience) -> Self {
        self.audience = audience;
        self
    }

    pub fn version(mut self, version: i64) -> Self {
        self.version = Some(version);
        self
    }

    pub fn changes(mut self, changes: &'a [&'a str]) -> Self {
        self.changes = changes;
        self
    }

    pub fn access_thing(mut self, id: Uuid) -> Self {
        self.access_thing = Some(id);
        self
    }

    pub fn actor(mut self, actor: Uuid) -> Self {
        self.actor = Some(actor);
        self
    }

    pub fn workspace(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }
}

/// The largest payload an event may carry.
pub const MAX_PAYLOAD_BYTES: usize = 8192;

/// Stages one event in the change's transaction; the sequencer gives it a
/// position once the transaction commits, and a rollback leaves nothing.
pub async fn emit(
    transaction: &mut Transaction<'_, Postgres>,
    event: NewEvent<'_>,
) -> Result<Uuid, AppError> {
    if !event.payload.is_object()
        || serde_json::to_vec(&event.payload).map_or(true, |bytes| bytes.len() > MAX_PAYLOAD_BYTES)
    {
        return Err(AppError::Internal("event payload is not a small object"));
    }
    if event.changes.len() > 64 {
        return Err(AppError::Internal("an event names at most 64 changes"));
    }
    if event.workspace_id.is_none() && event.space_id.is_none() {
        return Err(AppError::Internal("an event names its workspace or space"));
    }
    let (conversation, root, restricted, account) = match event.audience {
        Audience::Conversation { root, restricted } => {
            (Some(root), Some(root), Some(restricted), None)
        }
        Audience::ConversationOf(conversation) => (Some(conversation), None, None, None),
        Audience::Account(account) => (None, None, None, Some(account)),
        _ => (None, None, None, None),
    };
    let changes = event
        .changes
        .iter()
        .map(|change| (*change).to_owned())
        .collect::<Vec<_>>();
    let (thing_id, thing_kind) = event
        .thing
        .map_or((None, None), |(id, kind)| (Some(id), Some(kind.to_owned())));
    // One statement: the workspace from the space, and a conversation's
    // root and restriction (the rule `conversation_visible_to` reads), are
    // looked up as the row goes in. Who acted comes from the transaction's
    // context, else from the event.
    sqlx::query(
        r#"
        INSERT INTO event_staging (
            id, workspace_id, space_id, thing_id, thing_kind, access_thing_id,
            thing_version, type, class, changes, audience, conversation_id,
            conversation_restricted, audience_account_id, actor_account_id,
            via_credential_id, requested_by_account_id, run_id, request_id,
            legacy_type, payload
        )
        SELECT $1, COALESCE($2, project.organization_id), $3, $4, $5, $6,
               $7, $8, $9, $10, $11,
               CASE WHEN $11 = 'conversation'
                    THEN COALESCE($13, conversation.parent_channel_id, conversation.id) END,
               CASE WHEN $11 = 'conversation'
                    THEN COALESCE($14, COALESCE(parent.visibility, conversation.visibility) <> 'project') END,
               $15,
               COALESCE(NULLIF(current_setting('aw.actor', true), '')::uuid, $16),
               NULLIF(current_setting('aw.via_grant', true), '')::uuid,
               NULLIF(current_setting('aw.requested_by', true), '')::uuid,
               NULLIF(current_setting('aw.run', true), '')::uuid,
               NULLIF(current_setting('aw.request_id', true), ''),
               $17, $18
        FROM (SELECT 1) AS one
        LEFT JOIN projects project ON project.id = $3
        -- visibility: only the root and its restriction are read, to route the event
        LEFT JOIN conversations conversation ON conversation.id = $12
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        "#,
    )
    .bind(event.id)
    .bind(event.workspace_id)
    .bind(event.space_id)
    .bind(thing_id)
    .bind(thing_kind)
    .bind(event.access_thing)
    .bind(event.version)
    .bind(event.ty.name)
    .bind(event.ty.class.key())
    .bind(changes)
    .bind(event.audience.key())
    .bind(conversation)
    .bind(root)
    .bind(restricted)
    .bind(account)
    .bind(event.actor)
    .bind(event.legacy_type)
    .bind(&event.payload)
    .execute(&mut **transaction)
    .await?;
    Ok(event.id)
}

/// One event in the log, with its position.
#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggedEvent {
    pub position: i64,
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub space_id: Option<Uuid>,
    pub thing_id: Option<Uuid>,
    pub thing_kind: Option<String>,
    pub access_thing_id: Option<Uuid>,
    pub thing_version: Option<i64>,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub event_type: String,
    pub class: String,
    pub changes: Vec<String>,
    pub audience: String,
    pub conversation_id: Option<Uuid>,
    pub conversation_restricted: Option<bool>,
    pub audience_account_id: Option<Uuid>,
    pub actor_account_id: Option<Uuid>,
    pub via_credential_id: Option<Uuid>,
    pub requested_by_account_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub request_id: Option<String>,
    pub legacy_type: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The columns `LoggedEvent` reads, for queries over `events`.
pub(crate) const LOGGED_EVENT_COLUMNS: &str = "position, id, workspace_id, space_id, thing_id, \
    thing_kind, access_thing_id, thing_version, type, class, changes, audience, conversation_id, \
    conversation_restricted, audience_account_id, actor_account_id, via_credential_id, \
    requested_by_account_id, run_id, request_id, legacy_type, payload, created_at";

/// Unfiltered read by position, for the live tail and the consumers only
/// (K5.4, K3.3). Never answers an account: readers that do re-check access.
pub async fn tail(
    connection: &mut PgConnection,
    after: i64,
    limit: i64,
) -> Result<Vec<LoggedEvent>, AppError> {
    let rows = sqlx::query_as::<_, LoggedEvent>(&format!(
        "SELECT {LOGGED_EVENT_COLUMNS} FROM events WHERE position > $1 ORDER BY position LIMIT $2"
    ))
    .bind(after)
    .bind(limit.clamp(1, 5000))
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows)
}

/// The newest position in the log (0 when it is empty).
pub async fn head(connection: &mut PgConnection) -> Result<i64, AppError> {
    let next = sqlx::query_scalar::<_, i64>("SELECT next_position FROM event_position_counter")
        .fetch_one(&mut *connection)
        .await?;
    Ok(next - 1)
}
