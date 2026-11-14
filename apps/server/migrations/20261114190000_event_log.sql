-- K3.2 (build 1E): the event log. Every change writes one event in its own
-- transaction; a sequencer gives committed events their positions, so a
-- reader that follows positions never skips one (docs/plans/master/plans/
-- K3-things-links-events.md 3.3, ruling R8: one log, one envelope, one
-- sequencer, K5 reads it).
--
-- * event_staging: writers insert here, inside the change's transaction. It
--   has the log's columns, not a JSON body, so a malformed event fails the
--   write that made it instead of stopping the sequencer for everyone.
-- * event_position_counter: one row; the sequencer locks it, so positions are
--   handed out by one transaction at a time, dense and in commit order. A
--   counter row (not transaction ids) survives a dump and restore.
-- * events: the log, partitioned by position, never updated (only retention
--   deletes). Payloads carry ids, keys of what changed and small non-text
--   values: never message, page or cell text (AGENTS.md rule 10).
-- * realtime_channel / wake_event_log / event_staging_wake: the wake. NOTIFY
--   is an optimisation (ruling R24 rule 3): every reader also polls.
--
-- Server-level plumbing: rows carry their workspace_id (rule 11), and
-- workspace_id / space_id are plain columns, not foreign keys, so a deleted
-- workspace's history can still be read by the retention and export jobs.

CREATE SEQUENCE event_staging_ordinal AS BIGINT;

CREATE TABLE event_staging (
    id UUID PRIMARY KEY,
    -- Order of staging: within one transaction it is the order events were
    -- emitted. Positions come from the sequencer, in this order.
    ordinal BIGINT NOT NULL DEFAULT nextval('event_staging_ordinal'),
    staged_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    workspace_id UUID NOT NULL,
    space_id UUID,
    thing_id UUID,
    thing_kind TEXT CHECK (thing_kind IS NULL OR thing_kind ~ '^[a-z][a-z0-9_]{1,39}$'),
    access_thing_id UUID,
    thing_version BIGINT,
    type TEXT NOT NULL CHECK (type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$' AND char_length(type) <= 80),
    class TEXT NOT NULL CHECK (class IN ('fact', 'signal')),
    changes TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(changes) <= 64),
    audience TEXT NOT NULL CHECK (audience IN ('workspace', 'space', 'conversation', 'account', 'thing')),
    conversation_id UUID,
    conversation_restricted BOOLEAN,
    audience_account_id UUID,
    actor_account_id UUID,
    via_credential_id UUID,
    requested_by_account_id UUID,
    run_id UUID,
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) <= 128),
    legacy_type TEXT CHECK (legacy_type IS NULL OR char_length(legacy_type) <= 120),
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 8192),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((audience = 'space') <= (space_id IS NOT NULL)),
    CHECK ((audience = 'conversation') = (conversation_id IS NOT NULL AND conversation_restricted IS NOT NULL)),
    CHECK ((audience = 'account') = (audience_account_id IS NOT NULL)),
    CHECK ((audience = 'thing') <= (COALESCE(access_thing_id, thing_id) IS NOT NULL))
);
CREATE INDEX event_staging_order ON event_staging (ordinal);

CREATE TABLE event_position_counter (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    next_position BIGINT NOT NULL CHECK (next_position > 0)
);
INSERT INTO event_position_counter (singleton, next_position) VALUES (true, 1);

CREATE TABLE events (
    position BIGINT NOT NULL,
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    space_id UUID,
    thing_id UUID,
    thing_kind TEXT,
    access_thing_id UUID,
    thing_version BIGINT,
    type TEXT NOT NULL CHECK (type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    class TEXT NOT NULL CHECK (class IN ('fact', 'signal')),
    changes TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(changes) <= 64),
    audience TEXT NOT NULL CHECK (audience IN ('workspace', 'space', 'conversation', 'account', 'thing')),
    conversation_id UUID,
    conversation_restricted BOOLEAN,
    audience_account_id UUID,
    actor_account_id UUID,
    via_credential_id UUID,
    requested_by_account_id UUID,
    run_id UUID,
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) <= 128),
    legacy_type TEXT,
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 8192),
    created_at TIMESTAMPTZ NOT NULL,
    CHECK ((audience = 'space') <= (space_id IS NOT NULL)),
    CHECK ((audience = 'conversation') = (conversation_id IS NOT NULL AND conversation_restricted IS NOT NULL)),
    CHECK ((audience = 'account') = (audience_account_id IS NOT NULL)),
    CHECK ((audience = 'thing') <= (COALESCE(access_thing_id, thing_id) IS NOT NULL)),
    PRIMARY KEY (position)
) PARTITION BY RANGE (position);
CREATE TABLE events_p0 PARTITION OF events FOR VALUES FROM (1) TO (10000001);
CREATE TABLE events_default PARTITION OF events DEFAULT;
CREATE INDEX events_workspace ON events (workspace_id, position);
CREATE INDEX events_space ON events (space_id, position) WHERE space_id IS NOT NULL;
CREATE INDEX events_thing ON events (thing_id, position) WHERE thing_id IS NOT NULL;
CREATE INDEX events_audience ON events (audience_account_id, position) WHERE audience_account_id IS NOT NULL;
CREATE INDEX events_id ON events (id);
CREATE INDEX events_actor ON events (actor_account_id, position) WHERE actor_account_id IS NOT NULL;
CREATE INDEX events_conversation ON events (conversation_id, position) WHERE conversation_id IS NOT NULL;
CREATE INDEX events_type ON events (type, position);

-- The log is append-only: nothing changes an event once it has a position.
CREATE FUNCTION events_refuse_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'events are never updated' USING ERRCODE = 'check_violation';
END $$;
CREATE TRIGGER events_immutable BEFORE UPDATE ON events
    FOR EACH ROW EXECUTE FUNCTION events_refuse_update();

-- The wake. Channel names carry the schema, so tests that share a database
-- in separate schemas don't wake each other, and stay under PostgreSQL's
-- 63-byte limit.
CREATE FUNCTION realtime_channel(suffix TEXT) RETURNS TEXT LANGUAGE sql STABLE AS $$
    SELECT 'aw_' || left(md5(current_schema()), 12) || '_' || suffix
$$;
CREATE FUNCTION wake_event_log() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify(realtime_channel('log'), TG_TABLE_NAME);   -- delivered at commit
    RETURN NULL;
END $$;
CREATE TRIGGER event_staging_wake AFTER INSERT ON event_staging
    FOR EACH STATEMENT EXECUTE FUNCTION wake_event_log();
