-- M6 Project-scoped local Codex MCP credentials.
--
-- Bearer material is never persisted. The host-local command emits a token
-- once and stores only its SHA-256 digest plus a non-secret lookup prefix.

CREATE TABLE mcp_client_credentials (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    client_name TEXT NOT NULL CHECK (
        char_length(client_name) BETWEEN 1 AND 80
        AND client_name ~ '^[A-Za-z0-9][A-Za-z0-9._ -]*$'
    ),
    token_prefix TEXT NOT NULL UNIQUE CHECK (
        char_length(token_prefix) BETWEEN 8 AND 24
        AND token_prefix ~ '^aw_mcp_[A-Za-z0-9_-]+$'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    scopes TEXT[] NOT NULL CHECK (
        cardinality(scopes) BETWEEN 1 AND 2
        AND scopes <@ ARRAY['read', 'write']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    ),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    replaced_by_credential_id UUID UNIQUE REFERENCES mcp_client_credentials(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (expires_at > created_at),
    CHECK (
        (status = 'active' AND revoked_at IS NULL)
        OR (status = 'revoked' AND revoked_at IS NOT NULL)
    )
);

CREATE INDEX mcp_client_credentials_actor_project
    ON mcp_client_credentials (actor_id, project_id, created_at DESC);

CREATE INDEX mcp_client_credentials_active_expiry
    ON mcp_client_credentials (expires_at)
    WHERE status = 'active';

ALTER TABLE audit_events
    ADD COLUMN channel TEXT CHECK (
        channel IS NULL OR channel IN ('browser', 'mcp', 'host_cli', 'system')
    ),
    ADD COLUMN client_name TEXT CHECK (
        client_name IS NULL OR char_length(client_name) BETWEEN 1 AND 80
    ),
    ADD COLUMN mcp_credential_id UUID REFERENCES mcp_client_credentials(id);

CREATE INDEX audit_events_mcp_credential
    ON audit_events (mcp_credential_id, occurred_at DESC)
    WHERE mcp_credential_id IS NOT NULL;
