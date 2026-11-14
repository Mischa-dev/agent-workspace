-- K3.3 (build 1E): cursors for the event log's consumers (docs/plans/master/
-- plans/K3-things-links-events.md 3.3.4). Server plumbing, on the list of
-- global tables: a consumer reads every workspace's events in one stream.
--
-- * event_consumers: how far each consumer has handled the log. Until K5.2's
--   `consumer:<name>` leases, a runner locks its own row for each batch
--   (FOR UPDATE SKIP LOCKED: pooler-safe, never a session advisory lock).
-- * event_consumer_failures: events a consumer moved past after they kept
--   failing. A sweep retries them; nothing is forgotten.

CREATE TABLE event_consumers (
    name TEXT PRIMARY KEY CHECK (name ~ '^[a-z][a-z0-9_.:#-]{0,79}$'),
    -- The last position fully handled.
    position BIGINT NOT NULL CHECK (position >= 0),
    lease_owner TEXT,
    lease_expires_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE event_consumer_failures (
    consumer TEXT NOT NULL REFERENCES event_consumers(name) ON DELETE CASCADE,
    event_position BIGINT NOT NULL,
    attempts INTEGER NOT NULL CHECK (attempts > 0),
    last_error TEXT NOT NULL CHECK (char_length(last_error) <= 500),
    next_attempt_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_position)
);
CREATE INDEX event_consumer_failures_due
    ON event_consumer_failures (consumer, next_attempt_at) WHERE resolved_at IS NULL;
