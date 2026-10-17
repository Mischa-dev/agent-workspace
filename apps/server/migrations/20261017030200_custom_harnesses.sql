-- X2: harnesses a workspace registers itself: a command on the server
-- (H7's protocol) or a connected runner (an agent with a webhook, X1). Each
-- gets its own adapter kind, so accounts, the model catalog, run settings
-- and the Launch dialog treat it exactly like a built-in harness.
CREATE TABLE custom_harnesses (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    kind TEXT NOT NULL UNIQUE CHECK (kind ~ '^x_[a-z0-9_]{1,40}$'),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'
    ),
    transport TEXT NOT NULL CHECK (transport IN ('command', 'runner')),
    -- Models, options, features and limits, as the registration API checks them.
    manifest JSONB NOT NULL CHECK (
        jsonb_typeof(manifest) = 'object' AND octet_length(manifest::text) <= 65536
    ),
    -- command: the program and its argument template.
    command JSONB CHECK (
        command IS NULL OR (jsonb_typeof(command) = 'array'
                            AND octet_length(command::text) <= 16384)
    ),
    -- Names of the account owner's personal secrets handed to the command.
    secret_names TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(secret_names) <= 16),
    -- runner: the connected agent that does the work.
    runner_actor_id UUID REFERENCES actors(id),
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, id),
    UNIQUE (organization_id, name),
    CHECK ((transport = 'command') = (command IS NOT NULL)),
    CHECK ((transport = 'runner') = (runner_actor_id IS NOT NULL))
);
