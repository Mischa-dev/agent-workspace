//! Shared integration fixtures: the app, its databases, the callers that
//! reach it (a person, an agent, a connected app) and worlds built from
//! them. New tests start here; the older files re-export what moved.
//! Later workstreams build on this, so not every helper has a caller yet.
#![allow(dead_code)]

pub(crate) mod app;
pub(crate) mod callers;
pub(crate) mod db;
pub(crate) mod live;
pub(crate) mod requests;
pub(crate) mod test_host;
pub(crate) mod two_workspaces;
pub(crate) mod world;

/// Every change notice `outbox::enqueue` staged in the log, whether the
/// sequencer has given it a position yet or not, with the columns tests ask
/// about. Staged notices are numbered after the log's head in staging
/// order, which is the order they will get. Use it with an alias:
/// `FROM {NOTICES} notice`.
pub(crate) const NOTICES: &str = "(SELECT id AS event_id, legacy_type AS event_type, \
    space_id AS project_id, \
    CASE WHEN legacy_type = 'conversation.member.removed' THEN NULL \
         ELSE (payload ->> 'conversationId')::uuid END AS conversation_id, \
    CASE WHEN legacy_type = 'conversation.read_state.updated' THEN audience_account_id \
         ELSE COALESCE(thing_id, audience_account_id) END AS resource_id, \
    audience, payload, created_at, \
    event_cursor \
    FROM (SELECT position AS event_cursor, id, legacy_type, space_id, thing_id, \
                 audience_account_id, audience, payload, created_at FROM events \
          UNION ALL \
          SELECT (SELECT next_position FROM event_position_counter) + ordinal, id, legacy_type, \
                 space_id, thing_id, audience_account_id, audience, payload, created_at \
          FROM event_staging) logged \
    WHERE legacy_type IS NOT NULL)";
