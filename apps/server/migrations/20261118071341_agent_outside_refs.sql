-- K1.7: which agent elsewhere an agent account is. Accounts are global
-- (R19), and so is this: one active reference per account, and no two
-- accounts hold the same issuer and id at once. A host's proposed rebind
-- waits as `pending_rebind` until a manager accepts it.
CREATE TABLE agent_outside_refs (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES actors(id),
    -- Who issued the outside id, as the host or platform names itself.
    issuer TEXT NOT NULL CHECK (char_length(issuer) BETWEEN 1 AND 200),
    external_id TEXT NOT NULL CHECK (char_length(external_id) BETWEEN 1 AND 200),
    -- Earlier pairs, newest first: [{"issuer", "externalId", "until"}].
    lineage JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(lineage) = 'array' AND octet_length(lineage::text) <= 16000),
    status TEXT NOT NULL CHECK (status IN ('active', 'pending_rebind', 'detached')),
    -- The host that proposed a pending rebind; not identity.
    host_id UUID,
    attached_by_actor_id UUID REFERENCES actors(id),
    attached_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    detached_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0)
);

CREATE UNIQUE INDEX agent_outside_refs_one_active
    ON agent_outside_refs (account_id) WHERE status = 'active';
CREATE UNIQUE INDEX agent_outside_refs_one_pending
    ON agent_outside_refs (account_id) WHERE status = 'pending_rebind';
CREATE UNIQUE INDEX agent_outside_refs_identity
    ON agent_outside_refs (issuer, external_id) WHERE status = 'active';
