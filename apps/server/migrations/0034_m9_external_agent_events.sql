-- M9 bounded connection-addressed invalidation events.

ALTER TABLE external_agent_connections
    ADD CONSTRAINT external_agent_connections_project_id_id_unique
        UNIQUE (project_id, id);

CREATE TABLE external_agent_event_counters (
    connection_id UUID PRIMARY KEY REFERENCES external_agent_connections(id),
    next_sequence BIGINT NOT NULL DEFAULT 1 CHECK (next_sequence > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE external_agent_events (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    kind TEXT NOT NULL CHECK (
        char_length(kind) BETWEEN 1 AND 64
        AND kind ~ '^[a-z][a-z0-9_.]*$'
    ),
    summary TEXT NOT NULL CHECK (
        char_length(summary) BETWEEN 1 AND 240
        AND summary !~ '[[:cntrl:]]'
    ),
    resource_kind TEXT NOT NULL CHECK (
        char_length(resource_kind) BETWEEN 1 AND 64
        AND resource_kind ~ '^[a-z][a-z0-9_]*$'
    ),
    resource_id UUID NOT NULL,
    canonical_path TEXT NOT NULL CHECK (
        char_length(canonical_path) BETWEEN 1 AND 500
        AND canonical_path LIKE '/%'
        AND canonical_path !~ '[[:cntrl:]]'
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 days',
    UNIQUE (connection_id, sequence),
    FOREIGN KEY (project_id, connection_id)
        REFERENCES external_agent_connections(project_id, id),
    CHECK (expires_at > created_at)
);

CREATE INDEX external_agent_events_poll
    ON external_agent_events (connection_id, sequence)
    WHERE expires_at > created_at;

CREATE INDEX external_agent_events_expiry
    ON external_agent_events (expires_at);
