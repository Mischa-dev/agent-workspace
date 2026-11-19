-- K5.14: provider sign-ins in progress, shared by every instance, so the
-- page's next poll (or a cancel) can land on any of them. A row lives for
-- the sign-in's ten minutes at most; it holds the address and code the
-- person is shown, never a key or token.
CREATE TABLE provider_login_ceremonies (
    account_id UUID PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    ceremony_id UUID NOT NULL UNIQUE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    session_id UUID NOT NULL,
    idempotency_key TEXT NOT NULL CHECK (char_length(idempotency_key) BETWEEN 1 AND 500),
    kind TEXT NOT NULL CHECK (kind IN ('browser', 'device_code', 'api_key')),
    response JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX provider_login_ceremonies_expiry ON provider_login_ceremonies (expires_at);
SELECT tenancy_protect('provider_login_ceremonies', 'workspace_id');
