-- AWD-63: a one-time link that sets an agent up in the app it runs in.
--
-- A person who manages agents makes a link for one connection and the app
-- it runs in. Making it issues the agent a fresh token, which is kept
-- encrypted in the secrets store (secret_id), never in this table. The
-- first fetch of the link within 30 minutes returns the token with setup
-- steps for that app, marks the link claimed and deletes the secret. Only
-- the code's SHA-256 is stored.
CREATE TABLE agent_setup_links (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES external_agent_connections(id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    secret_id UUID REFERENCES secrets(id) ON DELETE SET NULL,
    app TEXT NOT NULL CHECK (app IN ('claude_code', 'codex', 'other')),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    expires_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agent_setup_links_short_lived CHECK (expires_at <= created_at + interval '1 hour')
);

-- Links still waiting, to purge when they lapse or the agent's token changes.
CREATE INDEX agent_setup_links_open ON agent_setup_links (connection_id, expires_at)
    WHERE claimed_at IS NULL;
