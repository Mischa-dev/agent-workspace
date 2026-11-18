-- H1.33: the billing seam. A workspace's plan state as a merchant of
-- record last reported it, and the webhook deliveries already taken, so the
-- same one twice changes nothing. No provider is built in: with none
-- configured there is no billing route and these stay empty.

ALTER TABLE workspace_plans
    ADD COLUMN billing_provider TEXT
        CHECK (billing_provider IS NULL OR char_length(billing_provider) BETWEEN 1 AND 40),
    ADD COLUMN customer_ref TEXT
        CHECK (customer_ref IS NULL OR char_length(customer_ref) BETWEEN 1 AND 200),
    ADD COLUMN subscription_ref TEXT
        CHECK (subscription_ref IS NULL OR char_length(subscription_ref) BETWEEN 1 AND 200),
    ADD COLUMN seats INTEGER CHECK (seats IS NULL OR seats >= 0),
    ADD COLUMN period_ends_at TIMESTAMPTZ,
    -- When the state written here was read from the provider: an older
    -- reading never overwrites a newer one.
    ADD COLUMN billing_state_at TIMESTAMPTZ;

CREATE UNIQUE INDEX workspace_plans_subscription
    ON workspace_plans (billing_provider, subscription_ref)
    WHERE subscription_ref IS NOT NULL;

CREATE TABLE billing_events (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (char_length(provider) BETWEEN 1 AND 40),
    provider_event_id TEXT NOT NULL CHECK (char_length(provider_event_id) BETWEEN 1 AND 200),
    kind TEXT NOT NULL CHECK (kind IN (
        'subscription_started', 'subscription_changed', 'payment_failed',
        'payment_succeeded', 'subscription_ended'
    )),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    applied_at TIMESTAMPTZ,
    UNIQUE (provider, provider_event_id)
);
CREATE INDEX billing_events_workspace ON billing_events (workspace_id, received_at DESC);
SELECT tenancy_protect('billing_events', 'workspace_id');
