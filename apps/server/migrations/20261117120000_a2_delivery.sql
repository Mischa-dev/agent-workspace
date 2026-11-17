-- A2.1 to A2.3 and A2.5 (Lane E): one way to tell an agent what happened.
--
-- Agents hear the event log (K3), read for them with their access re-checked
-- per event, through one cursor, by poll, long-poll, stream or webhook. This
-- replaces the per-connection agent event feed (`external_agent_events` and
-- its counters) and the reply wakeup queue (`agent_webhook_deliveries`): the
-- writers that filled them emit addressed events instead, and the webhook
-- settings move from `external_agent_connections` to the new connections.
--
-- * agent_connections: how one account is reached and told. Like the
--   account, it is server-wide (R19); its home workspace keeps its secrets;
--   it reaches every workspace the account is in (`reaches_all`) or only
--   its home. A connection
--   made for an older project credential (`source_connection_id`) reaches
--   its home only, as that credential does. Stream and poll keep no
--   per-event state; `read_cursor` is where the client last read (health),
--   `push_cursor` where the pusher has queued deliveries up to.
-- * connection_signing_secrets: a webhook's signing secrets, by reference to
--   the secrets store; a retiring one signs alongside the new one for a day.
-- * event_subscriptions: what a connection hears beyond what is addressed to
--   it (always heard). No subscription: what is addressed to it and what it
--   follows (tasks it watches or is assigned, runs it asked for).
-- * deliveries: webhook pushes only, one per connection and event.
--
-- Converted once: each connection with a webhook keeps it, its format
-- (`typed` becomes `typed_v1`, `inbox` becomes `inbox_v1`) and its secret.
-- Wakeups still queued are not carried over: what they woke for is still in
-- the agent's inbox (`reply_claim`). A webhook whose secret predates the
-- secrets store (none on a server that has started since it came) stops
-- pushing until it is saved again.

CREATE TABLE agent_connections (
    id UUID PRIMARY KEY,
    -- Where it was made: its signing secrets are kept there.
    home_workspace_id UUID NOT NULL REFERENCES organizations(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    reaches_all BOOLEAN NOT NULL DEFAULT false,
    source_connection_id UUID UNIQUE REFERENCES external_agent_connections(id),
    is_default BOOLEAN NOT NULL DEFAULT false,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    webhook_url TEXT CHECK (
        webhook_url IS NULL OR (char_length(webhook_url) <= 500 AND webhook_url ~ '^https?://')
    ),
    webhook_format TEXT NOT NULL DEFAULT 'standard'
        CHECK (webhook_format IN ('standard', 'typed_v1', 'inbox_v1')),
    cursor_key BYTEA NOT NULL
        DEFAULT (uuid_send(gen_random_uuid()) || uuid_send(gen_random_uuid())),
    push_cursor BIGINT NOT NULL DEFAULT 0 CHECK (push_cursor >= 0),
    read_cursor BIGINT NOT NULL DEFAULT 0 CHECK (read_cursor >= 0),
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'push_disabled', 'removed')),
    disabled_reason TEXT CHECK (disabled_reason IS NULL OR char_length(disabled_reason) <= 300),
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    first_failure_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    last_delivered_at TIMESTAMPTZ,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    client_request_id TEXT CHECK (char_length(client_request_id) BETWEEN 1 AND 200),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (NOT (is_default AND reaches_all AND source_connection_id IS NOT NULL))
);
CREATE INDEX agent_connections_account ON agent_connections (account_id) WHERE state <> 'removed';
CREATE INDEX agent_connections_home ON agent_connections (home_workspace_id);
CREATE INDEX agent_connections_push ON agent_connections (account_id)
    WHERE webhook_url IS NOT NULL AND state = 'active';
-- One default per account for credentials that reach every workspace.
CREATE UNIQUE INDEX agent_connections_one_default ON agent_connections (account_id)
    WHERE is_default AND source_connection_id IS NULL AND state <> 'removed';
CREATE UNIQUE INDEX agent_connections_request
    ON agent_connections (created_by_account_id, client_request_id)
    WHERE client_request_id IS NOT NULL;

CREATE TABLE connection_signing_secrets (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    secret_id UUID NOT NULL REFERENCES secrets(id),
    state TEXT NOT NULL CHECK (state IN ('active', 'retiring')),
    retire_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((state = 'retiring') = (retire_at IS NOT NULL))
);
CREATE UNIQUE INDEX connection_signing_secrets_one_active
    ON connection_signing_secrets (connection_id) WHERE state = 'active';
CREATE INDEX connection_signing_secrets_retiring
    ON connection_signing_secrets (retire_at) WHERE state = 'retiring';

CREATE TABLE event_subscriptions (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    -- 'task.assigned', 'task.*' or '*'.
    kinds TEXT[] NOT NULL CHECK (
        cardinality(kinds) BETWEEN 1 AND 64
        AND array_position(kinds, NULL) IS NULL
    ),
    -- NULL: every workspace the connection reaches.
    workspace_ids UUID[] CHECK (workspace_ids IS NULL OR cardinality(workspace_ids) BETWEEN 1 AND 100),
    space_ids UUID[] CHECK (space_ids IS NULL OR cardinality(space_ids) BETWEEN 1 AND 100),
    conversation_ids UUID[]
        CHECK (conversation_ids IS NULL OR cardinality(conversation_ids) BETWEEN 1 AND 100),
    -- Direct messages and groups (ruling R20).
    includes_dms BOOLEAN NOT NULL DEFAULT true,
    -- What the account did itself.
    echoes BOOLEAN NOT NULL DEFAULT false,
    min_reason TEXT NOT NULL DEFAULT 'following'
        CHECK (min_reason IN ('addressed', 'following', 'visible')),
    set_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX event_subscriptions_connection ON event_subscriptions (connection_id);

-- workspace_id is the event's workspace (fairness, metering, row rules).
CREATE TABLE deliveries (
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    event_position BIGINT NOT NULL,
    event_id UUID NOT NULL,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    event_type TEXT NOT NULL CHECK (char_length(event_type) <= 80),
    reason TEXT NOT NULL CHECK (char_length(reason) <= 40),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'delivered', 'failed', 'skipped')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    last_status INTEGER,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    elapsed_ms INTEGER,
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (connection_id, event_position)
);
CREATE INDEX deliveries_due ON deliveries (next_attempt_at) WHERE state = 'pending';
CREATE INDEX deliveries_workspace ON deliveries (workspace_id);
CREATE INDEX deliveries_finished ON deliveries (created_at) WHERE state <> 'pending';
SELECT tenancy_protect('deliveries', 'workspace_id');

-- Whether `recipient` may hear `ev` now: every transport asks this once per
-- event, so all three agree. The audience decides who could ever hear it;
-- the thing access is decided on (a conversation, a request, a run, a
-- board) is checked as it is now. An inactive account hears nothing.
CREATE FUNCTION delivery_thing_visible(thing UUID, recipient UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN EXISTS (SELECT 1 FROM conversations c WHERE c.id = thing)
            THEN conversation_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM requests r WHERE r.id = thing)
            THEN request_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM runs r WHERE r.id = thing)
            THEN run_visible_to(thing, recipient)
        WHEN EXISTS (SELECT 1 FROM whiteboards w WHERE w.id = thing)
            THEN whiteboard_visible_to(thing, recipient)
        ELSE true
    END
$$;

CREATE FUNCTION delivery_visible(recipient UUID, ev events)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (SELECT 1 FROM actors a WHERE a.id = recipient AND a.status = 'active')
       AND (ev.audience_account_id IS NULL OR ev.audience_account_id = recipient)
       AND CASE ev.audience
           -- The conversation rule alone: a DM stays its participants'
           -- (R20), a channel needs a reader of its space.
           WHEN 'conversation' THEN
               conversation_visible_to(COALESCE(ev.access_thing_id, ev.conversation_id), recipient)
           WHEN 'thing' THEN delivery_thing_visible(COALESCE(ev.access_thing_id, ev.thing_id), recipient)
           ELSE EXISTS (
                   SELECT 1 FROM organization_memberships m
                   WHERE m.organization_id = ev.workspace_id AND m.actor_id = recipient
                     AND m.enabled)
               AND (ev.space_id IS NULL OR space_readable_by(ev.space_id, recipient))
               AND (ev.access_thing_id IS NULL
                    OR delivery_thing_visible(ev.access_thing_id, recipient))
       END
$$;

-- Whether what an event says is still true for `recipient`: a wake-up for a
-- reply request only while the request waits for it, a stop only once it
-- was stopped. Every transport drops what is no longer true.
CREATE FUNCTION delivery_still_relevant(recipient UUID, ev events)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN ev.type IN ('reply.requested', 'reply.followup') THEN EXISTS (
            SELECT 1 FROM reply_requests r
            WHERE r.id = ev.thing_id AND r.agent_actor_id = recipient AND r.status = 'pending')
        WHEN ev.type = 'reply.cancelled' THEN EXISTS (
            SELECT 1 FROM reply_requests r
            WHERE r.id = ev.thing_id AND r.agent_actor_id = recipient AND r.status = 'cancelled')
        ELSE true
    END
$$;

-- Converted once (see the header).
INSERT INTO agent_connections (
    id, home_workspace_id, account_id, reaches_all, source_connection_id, is_default,
    name, webhook_url, webhook_format, push_cursor, read_cursor, state, disabled_reason,
    created_by_account_id, created_at, updated_at
)
SELECT gen_random_uuid(), connection.workspace_id, connection.actor_id, false,
       connection.id, true, connection.connection_name, connection.webhook_url,
       CASE connection.wakeup_format WHEN 'inbox' THEN 'inbox_v1' ELSE 'typed_v1' END,
       counter.next_position - 1, counter.next_position - 1,
       CASE WHEN connection.webhook_url IS NOT NULL AND connection.webhook_secret_id IS NULL
            THEN 'push_disabled' ELSE 'active' END,
       CASE WHEN connection.webhook_url IS NOT NULL AND connection.webhook_secret_id IS NULL
            THEN 'Save the webhook again to get a signing secret.' END,
       connection.created_by_actor_id, connection.created_at, now()
FROM external_agent_connections connection, event_position_counter counter
WHERE connection.status <> 'removed';

INSERT INTO connection_signing_secrets (id, connection_id, secret_id, state)
SELECT gen_random_uuid(), delivery.id, connection.webhook_secret_id, 'active'
FROM agent_connections delivery
JOIN external_agent_connections connection ON connection.id = delivery.source_connection_id
WHERE connection.webhook_secret_id IS NOT NULL;

DROP TABLE agent_webhook_deliveries;
DROP TABLE external_agent_events;
DROP TABLE external_agent_event_counters;
DROP FUNCTION external_event_visible_to(UUID, TEXT, UUID);
ALTER TABLE external_agent_connections
    DROP CONSTRAINT external_agent_connections_webhook_pair,
    DROP CONSTRAINT external_agent_connections_one_webhook_secret,
    DROP COLUMN webhook_url,
    DROP COLUMN webhook_secret,
    DROP COLUMN webhook_secret_id,
    DROP COLUMN wakeup_format,
    DROP COLUMN last_wakeup_at,
    DROP COLUMN last_wakeup_error;

-- The old event retention job's queued run; delivery retention replaces it.
DELETE FROM background_jobs WHERE kind = 'retention.external_events';
