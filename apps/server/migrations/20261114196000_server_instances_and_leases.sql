-- K5.2 (build 1E): the server's instances and the leases they hold
-- (docs/plans/master/plans/K5-any-number-of-servers.md 3.2). Both are
-- server-level, not workspace rows, so they carry no `workspace_id`; they
-- are on the list of global tables.
--
-- Every time here is the database's clock, so clock skew between machines
-- doesn't matter. A lease's row outlives its holder (no foreign key), so its
-- `token` keeps growing: each takeover by another instance adds one, and a
-- write that must happen only under a lease checks the row in its own
-- transaction (`FOR SHARE`).

CREATE TABLE server_instances (
    id UUID PRIMARY KEY CHECK (substring(id::text FROM 15 FOR 1) = '7'),
    roles TEXT[] NOT NULL CHECK (
        cardinality(roles) BETWEEN 1 AND 3 AND roles <@ ARRAY['api', 'live', 'work']::text[]
    ),
    label TEXT CHECK (label IS NULL OR char_length(label) BETWEEN 1 AND 80),
    release TEXT NOT NULL CHECK (char_length(release) BETWEEN 1 AND 80),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    draining_at TIMESTAMPTZ,
    stopped_at TIMESTAMPTZ
);
CREATE INDEX server_instances_live ON server_instances (heartbeat_at) WHERE stopped_at IS NULL;

CREATE TABLE leases (
    name TEXT PRIMARY KEY CHECK (
        char_length(name) <= 160 AND name ~ '^[a-z][a-z0-9_.-]*(:[a-z0-9_.-]{1,120})?$'
    ),
    -- No foreign key: the row must outlive its holder so `token` keeps increasing.
    holder_instance_id UUID NOT NULL,
    token BIGINT NOT NULL CHECK (token > 0),
    acquired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at >= acquired_at)
);
CREATE INDEX leases_holder ON leases (holder_instance_id);
