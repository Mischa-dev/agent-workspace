-- N3: Web Push subscriptions, one per device and person. The endpoint and
-- its keys are a capability (anyone holding them can push to the device),
-- so they are stored only as an encrypted secret (S1); this row keeps what
-- the device list shows: a label, the push service's host, and delivery
-- health.

CREATE TABLE push_subscriptions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    secret_id UUID NOT NULL REFERENCES secrets(id),
    -- sha256 of the endpoint, so subscribing the same device again replaces
    -- its row instead of adding one.
    endpoint_hash TEXT NOT NULL CHECK (endpoint_hash ~ '^[0-9a-f]{64}$'),
    service_host TEXT NOT NULL CHECK (char_length(service_host) BETWEEN 1 AND 253),
    label TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80 AND label !~ '[[:cntrl:]]'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_sent_at TIMESTAMPTZ,
    last_failure_at TIMESTAMPTZ,
    failure_count INTEGER NOT NULL DEFAULT 0 CHECK (failure_count >= 0),
    UNIQUE (organization_id, actor_id, endpoint_hash)
);

CREATE INDEX push_subscriptions_actor ON push_subscriptions (actor_id, organization_id, created_at);

-- One row per notification occurrence and device that was sent, so a
-- retried delivery job skips the devices that already got it.
CREATE TABLE push_deliveries (
    subscription_id UUID NOT NULL REFERENCES push_subscriptions(id) ON DELETE CASCADE,
    delivery_key TEXT NOT NULL CHECK (char_length(delivery_key) BETWEEN 1 AND 300),
    sent_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (subscription_id, delivery_key)
);

CREATE INDEX push_deliveries_sent ON push_deliveries (sent_at);

-- N4: per group of kinds and channel. A missing row means the built-in
-- default for that pair.
CREATE TABLE notification_channel_preferences (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    kind_group TEXT NOT NULL CHECK (kind_group ~ '^[a-z][a-z_]{0,39}$'),
    channel TEXT NOT NULL CHECK (channel IN ('in_app', 'push', 'email')),
    enabled BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, actor_id, kind_group, channel)
);

-- A project muted for one group of kinds ('all' for every kind).
CREATE TABLE notification_project_mutes (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    actor_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    kind_group TEXT NOT NULL CHECK (kind_group ~ '^[a-z][a-z_]{0,39}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_id, project_id, kind_group)
);
