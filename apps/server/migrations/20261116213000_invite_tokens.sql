-- K4.26: invitation links (ruling R30).
--
-- A link anyone holding it may use to join a workspace with its role, and
-- the space it names when it names one, up to `max_uses` people (no limit
-- when NULL) until it expires or is revoked. It is one of the invitation
-- engine's token families (`aw_lnk_`): only the token's digest is stored,
-- with a short prefix to find the row, and the token is shown once, to the
-- person who made it. `request_id` makes a retried create find the link it
-- already made (without its token) instead of making another. The table
-- is named for its rows, invitation tokens: `*_links` names are kept for
-- links between things (AGENTS.md rule 6).

CREATE TABLE invite_tokens (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    role_id UUID NOT NULL REFERENCES workspace_roles(id),
    token_prefix TEXT NOT NULL UNIQUE CHECK (
        char_length(token_prefix) = 16 AND token_prefix ~ '^aw_lnk_[A-Za-z0-9_-]{9}$'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) BETWEEN 1 AND 200),
    max_uses INTEGER CHECK (max_uses IS NULL OR max_uses BETWEEN 1 AND 10000),
    uses INTEGER NOT NULL DEFAULT 0 CHECK (uses >= 0 AND (max_uses IS NULL OR uses <= max_uses)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    UNIQUE (created_by_actor_id, request_id)
);

CREATE INDEX invite_tokens_workspace ON invite_tokens (workspace_id, created_at DESC);
