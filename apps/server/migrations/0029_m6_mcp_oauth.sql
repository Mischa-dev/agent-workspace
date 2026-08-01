-- M6 public-client OAuth authorization-code + PKCE grants for local Codex.
-- Authorization codes and bearer tokens are persisted only as SHA-256 digests.

CREATE TABLE mcp_oauth_clients (
    id UUID PRIMARY KEY,
    client_name TEXT NOT NULL CHECK (
        char_length(client_name) BETWEEN 1 AND 80
        AND client_name ~ '^[A-Za-z0-9][A-Za-z0-9._ -]*$'
    ),
    redirect_uris TEXT[] NOT NULL CHECK (cardinality(redirect_uris) BETWEEN 1 AND 8),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    CHECK (
        (status = 'active' AND revoked_at IS NULL)
        OR (status = 'revoked' AND revoked_at IS NOT NULL)
    )
);

CREATE TABLE mcp_oauth_authorization_codes (
    id UUID PRIMARY KEY,
    client_id UUID NOT NULL REFERENCES mcp_oauth_clients(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    redirect_uri TEXT NOT NULL,
    scopes TEXT[] NOT NULL CHECK (
        cardinality(scopes) BETWEEN 1 AND 2
        AND scopes <@ ARRAY['read', 'write']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    ),
    code_prefix TEXT NOT NULL UNIQUE,
    code_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(code_digest) = 32),
    code_challenge TEXT NOT NULL CHECK (char_length(code_challenge) = 43),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '5 minutes',
    used_at TIMESTAMPTZ,
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    CHECK (expires_at > created_at)
);

CREATE INDEX mcp_oauth_codes_expiry
    ON mcp_oauth_authorization_codes (expires_at)
    WHERE used_at IS NULL;

CREATE TABLE mcp_oauth_grants (
    id UUID PRIMARY KEY,
    client_id UUID NOT NULL REFERENCES mcp_oauth_clients(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    scopes TEXT[] NOT NULL CHECK (
        cardinality(scopes) BETWEEN 1 AND 2
        AND scopes <@ ARRAY['read', 'write']::TEXT[]
        AND scopes @> ARRAY['read']::TEXT[]
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    compromise_detected_at TIMESTAMPTZ,
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id)
);

CREATE INDEX mcp_oauth_grants_actor_project
    ON mcp_oauth_grants (actor_id, project_id, created_at DESC);

CREATE TABLE mcp_oauth_access_tokens (
    id UUID PRIMARY KEY,
    grant_id UUID NOT NULL REFERENCES mcp_oauth_grants(id),
    token_prefix TEXT NOT NULL UNIQUE,
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '1 hour',
    revoked_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);

CREATE INDEX mcp_oauth_access_expiry
    ON mcp_oauth_access_tokens (expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE mcp_oauth_refresh_tokens (
    id UUID PRIMARY KEY,
    grant_id UUID NOT NULL REFERENCES mcp_oauth_grants(id),
    token_prefix TEXT NOT NULL UNIQUE,
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '30 days',
    used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    rotated_to_token_id UUID UNIQUE REFERENCES mcp_oauth_refresh_tokens(id),
    CHECK (expires_at > created_at)
);

CREATE INDEX mcp_oauth_refresh_expiry
    ON mcp_oauth_refresh_tokens (expires_at)
    WHERE revoked_at IS NULL AND used_at IS NULL;

ALTER TABLE audit_events
    ADD COLUMN mcp_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id);

CREATE INDEX audit_events_mcp_oauth_grant
    ON audit_events (mcp_oauth_grant_id, occurred_at DESC)
    WHERE mcp_oauth_grant_id IS NOT NULL;
