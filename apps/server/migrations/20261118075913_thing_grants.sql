-- K1.32: sharing one thing (a page, for now) with an account of its
-- workspace or by a link. A share opens that thing to read and nothing else
-- in its space. A link keeps only its digest; its secret is shown once.
CREATE TABLE thing_grants (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    thing_kind TEXT NOT NULL CHECK (thing_kind IN ('page')),
    thing_id UUID NOT NULL,
    grantee_account_id UUID REFERENCES actors(id),
    link_prefix TEXT CHECK (link_prefix IS NULL OR char_length(link_prefix) = 12),
    link_digest BYTEA CHECK (link_digest IS NULL OR octet_length(link_digest) = 32),
    level TEXT NOT NULL DEFAULT 'see' CHECK (level IN ('see')),
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    granted_by_account_id UUID NOT NULL REFERENCES actors(id),
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (num_nonnulls(grantee_account_id, link_digest) = 1),
    CHECK ((link_digest IS NULL) = (link_prefix IS NULL)),
    CHECK (link_digest IS NULL OR expires_at IS NOT NULL)
);

CREATE INDEX thing_grants_thing ON thing_grants (thing_kind, thing_id) WHERE revoked_at IS NULL;
CREATE UNIQUE INDEX thing_grants_one_per_account
    ON thing_grants (thing_kind, thing_id, grantee_account_id)
    WHERE revoked_at IS NULL AND grantee_account_id IS NOT NULL;
CREATE INDEX thing_grants_link ON thing_grants (link_prefix) WHERE link_prefix IS NOT NULL;
CREATE UNIQUE INDEX thing_grants_request
    ON thing_grants (granted_by_account_id, request_id) WHERE request_id IS NOT NULL;
CREATE INDEX thing_grants_workspace ON thing_grants (workspace_id);

SELECT tenancy_protect('thing_grants', 'workspace_id');
