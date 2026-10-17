-- Outgoing webhooks (docs/plans/roadmap.md, workstream W14, X4).
--
-- A subscription belongs to the person or connected app that made it and
-- hears about one workspace's events: every project in it the owner can
-- see, or one project. Each delivery is checked again against the owner's
-- access when it is sent. The signing secret lives in `secrets` (S1), never
-- here; a delivery keeps only what it is about (IDs), and its body is built
-- when it is sent, so a removed message's text is never stored.
CREATE TABLE webhook_subscriptions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    -- NULL: every project in the workspace the owner can see at delivery.
    project_id UUID REFERENCES projects(id),
    owner_actor_id UUID NOT NULL REFERENCES actors(id),
    -- The connected app it was made through, if any.
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name = btrim(name)),
    url TEXT NOT NULL CHECK (char_length(url) BETWEEN 8 AND 500),
    event_types TEXT[] NOT NULL CHECK (cardinality(event_types) BETWEEN 1 AND 20),
    secret_id UUID REFERENCES secrets(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'paused', 'disabled')),
    disabled_reason TEXT CHECK (disabled_reason IS NULL OR char_length(disabled_reason) <= 500),
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    last_delivery_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX webhook_subscriptions_routing
    ON webhook_subscriptions (organization_id)
    WHERE state = 'active';
CREATE INDEX webhook_subscriptions_owner
    ON webhook_subscriptions (owner_actor_id, created_at DESC, id DESC);

-- One row per event per subscription; its id is the delivery id a receiver
-- sees on every retry, so it can drop a repeat.
CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY,
    subscription_id UUID NOT NULL REFERENCES webhook_subscriptions(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID REFERENCES projects(id),
    event_type TEXT NOT NULL CHECK (char_length(event_type) BETWEEN 1 AND 64),
    -- What happened, unique per subscription, such as run:<id>:completed.
    event_key TEXT NOT NULL CHECK (char_length(event_key) BETWEEN 1 AND 300),
    -- IDs of what the event is about.
    subject JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(subject) = 'object' AND octet_length(subject::text) <= 2048
    ),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (
        state IN ('pending', 'delivered', 'failed', 'skipped')
    ),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_status_code INTEGER,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    duration_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    UNIQUE (subscription_id, event_key)
);

CREATE INDEX webhook_deliveries_log
    ON webhook_deliveries (subscription_id, created_at DESC, id DESC);
CREATE INDEX webhook_deliveries_prune
    ON webhook_deliveries (created_at)
    WHERE state <> 'pending';
