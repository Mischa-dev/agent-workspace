//! The one read path (A2 3.5): every transport reads a connection's events
//! with [`read_batch`], which re-checks access per event
//! (`delivery_visible`) and drops what is no longer true (a reply request
//! already answered), so poll, stream and webhook agree.

use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{
    catalogue,
    connections::Subscription,
    reason::{self, Facts, Level, Reason},
};
use crate::{
    error::AppError,
    events::{LOGGED_EVENT_COLUMNS, LoggedEvent},
    policy::{AgentLimits, Permission},
};

/// The most log rows one read looks at; a read that finds nothing for the
/// connection still moves its cursor past them.
pub const SCAN: i64 = 500;
/// The most events one read returns.
pub const MAX_LIMIT: i64 = 100;

/// Where a read looks.
#[derive(Clone, Debug)]
pub struct Scope {
    pub recipient: Uuid,
    pub workspaces: Vec<Uuid>,
    /// An older credential confined to one space.
    pub space: Option<Uuid>,
    /// The delegation limit of an app reading for a person: an event is
    /// heard only where the app may read what it is about.
    pub limits: Option<AgentLimits>,
}

/// One event for the connection, and why.
#[derive(Clone, Debug)]
pub struct Delivered {
    pub event: LoggedEvent,
    pub reason: Reason,
}

/// What one read found.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub delivered: Vec<Delivered>,
    /// The position the next read starts after.
    pub next: i64,
    /// More may be waiting: read again now.
    pub has_more: bool,
}

#[derive(FromRow)]
struct Row {
    #[sqlx(flatten)]
    event: LoggedEvent,
    visible: bool,
    relevant: bool,
    addressed: bool,
    own: bool,
    following: bool,
    raiser: bool,
    for_you: bool,
    required: bool,
    direct: bool,
}

/// Events after `after` for the connection: at most `limit`, in log order.
pub async fn read_batch(
    connection: &mut PgConnection,
    subscriptions: &[Subscription],
    scope: &Scope,
    after: i64,
    limit: i64,
) -> Result<Batch, AppError> {
    let limit = usize::try_from(limit.clamp(1, MAX_LIMIT)).unwrap_or(1);
    if scope.workspaces.is_empty() {
        return Ok(Batch {
            next: after,
            ..Batch::default()
        });
    }
    let columns = LOGGED_EVENT_COLUMNS
        .split(", ")
        .map(|column| format!("e.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    // The head before the scan: every event up to it is already in the log
    // (the sequencer numbers and moves the head in one transaction), so a
    // scan that reaches its end has seen all of them.
    let head = crate::events::head(&mut *connection).await?;
    let run_followed = crate::runs::sql::run_followed_by("e.thing_id", "$1");
    let rows = sqlx::query_as::<_, Row>(&format!(
        r#"
        SELECT {columns},
               delivery_visible($1, e) AS visible,
               delivery_still_relevant($1, e) AS relevant,
               COALESCE(e.audience_account_id = $1, false) AS addressed,
               COALESCE(e.actor_account_id = $1, false) AS own,
               CASE e.thing_kind::text
                   WHEN 'task' THEN EXISTS (
                           SELECT 1 FROM work_item_watchers w
                           WHERE w.work_item_id = e.thing_id AND w.actor_id = $1)
                       OR EXISTS (
                           SELECT 1 FROM task_rows t
                           WHERE t.id = e.thing_id AND t.assignee_actor_id = $1)
                   WHEN 'run' THEN {run_followed}
                   ELSE false
               END AS following,
               COALESCE(e.thing_kind::text = 'request' AND EXISTS (
                   SELECT 1 FROM requests r
                   WHERE r.id = e.thing_id AND r.raised_by_account_id = $1), false) AS raiser,
               COALESCE(e.type = 'request.raised'
                   AND request_answerable_by(e.thing_id, $1), false) AS for_you,
               COALESCE(e.payload ->> 'required' = 'true', false) AS required,
               -- visibility: only the kind is read, to honour includes_dms
               COALESCE(e.conversation_id IS NOT NULL AND EXISTS (
                   SELECT 1 FROM conversations c
                   WHERE c.id = e.conversation_id AND c.kind IN ('dm', 'group')), false) AS direct
        FROM events e
        WHERE e.position > $2
          AND e.workspace_id = ANY($3)
          AND e.class = 'fact'
          AND e.type = ANY($4)
          AND (e.audience_account_id IS NULL OR e.audience_account_id = $1)
          AND ($5::uuid IS NULL OR e.space_id = $5)
        ORDER BY e.position
        LIMIT $6
        "#
    ))
    .bind(scope.recipient)
    .bind(after)
    .bind(&scope.workspaces)
    .bind(catalogue::kinds())
    .bind(scope.space)
    .bind(SCAN)
    .fetch_all(&mut *connection)
    .await?;
    let scanned = rows.len();
    let mut batch = Batch {
        next: after,
        ..Batch::default()
    };
    for (index, row) in rows.into_iter().enumerate() {
        batch.next = row.event.position;
        if let Some(reason) = accept(&row, subscriptions)
            && scope
                .limits
                .as_ref()
                .is_none_or(|limits| within_limits(limits, &row.event))
        {
            batch.delivered.push(Delivered {
                event: row.event,
                reason,
            });
            if batch.delivered.len() == limit {
                batch.has_more = index + 1 < scanned || scanned == SCAN as usize;
                return Ok(batch);
            }
        }
    }
    // Nothing more for this connection in what was read: the cursor still
    // moves past it, and past the rest of the log up to the head when the
    // scan reached its end, so a connection nothing is for stays caught up.
    batch.has_more = scanned == SCAN as usize;
    if !batch.has_more {
        batch.next = batch.next.max(head);
    }
    Ok(batch)
}

/// Whether an app held to `limits` may read what `event` is about: the
/// permission to see its space (and, for a task or a pull request, to see
/// those), in the space where the app's person holds it or else their
/// workspace, as a policy check under the same limit would find it.
fn within_limits(limits: &AgentLimits, event: &LoggedEvent) -> bool {
    let allowed = event
        .space_id
        .and_then(|space| limits.projects.get(&space))
        .or_else(|| limits.organizations.get(&event.workspace_id))
        .copied()
        .unwrap_or(crate::policy::PermissionSet::EMPTY);
    let family = catalogue::get(&event.event_type).map(|entry| entry.family);
    let needed: &[Permission] = match family {
        Some("task") => &[Permission::ProjectsView, Permission::TasksView],
        Some("pull_request") => &[Permission::ProjectsView, Permission::CodeView],
        _ => &[Permission::ProjectsView],
    };
    needed
        .iter()
        .all(|permission| allowed.contains(*permission))
}

/// Whether any subscription wants the row, and why.
fn accept(row: &Row, subscriptions: &[Subscription]) -> Option<Reason> {
    if !row.visible || !row.relevant {
        return None;
    }
    let kind = row.event.event_type.as_str();
    let reason = reason::decide(
        kind,
        Facts {
            addressed: row.addressed,
            own: row.own,
            following: row.following,
            raiser: row.raiser,
            for_you: row.for_you,
            required: row.required,
        },
    );
    subscriptions
        .iter()
        .any(|subscription| wants(subscription, row, reason))
        .then_some(reason)
}

fn wants(subscription: &Subscription, row: &Row, reason: Reason) -> bool {
    let event = &row.event;
    if reason == Reason::Own {
        if !subscription.echoes {
            return false;
        }
    } else if reason.level() == Level::Addressed {
        // What is addressed to the account is always heard.
        return true;
    }
    if subscription
        .workspace_ids
        .as_ref()
        .is_some_and(|ids| !ids.contains(&event.workspace_id))
    {
        return false;
    }
    if row.direct && !subscription.includes_dms {
        return false;
    }
    if !subscription
        .kinds
        .iter()
        .any(|pattern| catalogue::matches(pattern, &event.event_type))
    {
        return false;
    }
    if let Some(spaces) = &subscription.space_ids
        && !event.space_id.is_some_and(|space| spaces.contains(&space))
    {
        return false;
    }
    if let Some(conversations) = &subscription.conversation_ids
        && ![event.conversation_id, event.access_thing_id]
            .into_iter()
            .flatten()
            .any(|id| conversations.contains(&id))
    {
        return false;
    }
    let wanted = Level::parse(&subscription.min_reason).unwrap_or(Level::Following);
    reason == Reason::Own || reason.level() >= wanted
}
