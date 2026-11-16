-- K1.29: unclaimed accounts and claim links.
--
-- * users.claimed_at: when the person first took the account as theirs.
--   Every existing sign-in is claimed (the default fills it), and so is
--   every new one an older server makes; `accounts::create_person` leaves
--   it NULL only for an account made for someone who hasn't signed in yet.
-- * account_claims: one-time links that let that person claim it. Only the
--   token's digest is stored, with a short prefix to find the row. A claim
--   belongs to the workspace whose admin made it (ruling R1's names).

ALTER TABLE users ADD COLUMN claimed_at TIMESTAMPTZ DEFAULT now();

CREATE TABLE account_claims (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    token_prefix TEXT NOT NULL UNIQUE CHECK (
        char_length(token_prefix) = 16 AND token_prefix ~ '^aw_clm_[A-Za-z0-9_-]{9}$'
    ),
    token_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);

CREATE INDEX account_claims_account ON account_claims (account_id);
CREATE INDEX account_claims_workspace ON account_claims (workspace_id, created_at DESC);
