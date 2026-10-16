-- S1: encrypted secrets. Anything the server must use later (provider API
-- keys, webhook signing secrets, push keys) is stored here, encrypted with a
-- per-secret data key (AES-256-GCM) that is itself wrapped by a master key
-- the server reads from a file. Only ciphertext, the wrapped data key, the
-- master key's id and the last four characters are stored.
CREATE TABLE secrets (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Set for a project's own secret.
    project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    -- Set for one person's (or agent's) own secret.
    owner_actor_id UUID REFERENCES actors(id),
    -- Secrets the server makes for itself (webhook signing secrets) say so,
    -- and aren't listed with the ones people add.
    system_managed BOOLEAN NOT NULL DEFAULT false,
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120
        AND name ~ '^[a-z][a-z0-9_.:-]*$'
    ),
    description TEXT CHECK (description IS NULL OR char_length(description) <= 500),
    ciphertext BYTEA NOT NULL CHECK (octet_length(ciphertext) BETWEEN 17 AND 70000),
    nonce BYTEA NOT NULL CHECK (octet_length(nonce) = 12),
    wrapped_key BYTEA NOT NULL CHECK (octet_length(wrapped_key) BETWEEN 32 AND 512),
    wrap_nonce BYTEA CHECK (wrap_nonce IS NULL OR octet_length(wrap_nonce) = 12),
    master_key_id TEXT NOT NULL CHECK (
        char_length(master_key_id) BETWEEN 1 AND 64
        AND master_key_id ~ '^[A-Za-z0-9_.-]+$'
    ),
    last_four TEXT NOT NULL CHECK (char_length(last_four) <= 4),
    created_by_actor_id UUID REFERENCES actors(id),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (project_id IS NULL OR owner_actor_id IS NULL)
);

CREATE UNIQUE INDEX secrets_unique_name
    ON secrets (
        organization_id,
        COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid),
        COALESCE(owner_actor_id, '00000000-0000-0000-0000-000000000000'::uuid),
        name
    );

CREATE INDEX secrets_master_key ON secrets (master_key_id);

CREATE INDEX secrets_project ON secrets (project_id) WHERE project_id IS NOT NULL;

-- The wakeup webhook's signing secret moves into the store. The server moves
-- any plaintext secret left in webhook_secret at startup, once a master key
-- is configured, and then clears the column.
ALTER TABLE external_agent_connections
    ADD COLUMN webhook_secret_id UUID REFERENCES secrets(id),
    DROP CONSTRAINT external_agent_connections_webhook_pair,
    ADD CONSTRAINT external_agent_connections_webhook_pair CHECK (
        (webhook_url IS NULL)
        = (webhook_secret IS NULL AND webhook_secret_id IS NULL)
    ),
    ADD CONSTRAINT external_agent_connections_one_webhook_secret CHECK (
        webhook_secret IS NULL OR webhook_secret_id IS NULL
    );
