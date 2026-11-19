-- H1.25: plans and metering. A plan is data: its limits per meter. Every
-- workspace is on one plan (`workspace_plans`); a self-hosted server's is
-- `self_hosted`, which limits nothing, so only a workspace's own settings
-- and spend limits apply. Counted meters append to `usage_events`, once
-- per dedupe key, and add to `usage_counters` for each period and scope in
-- the same transaction; gauges (people, stored bytes) are read live.
-- `metering::admit` is the one place a limit is checked.
CREATE TABLE plans (
    id TEXT PRIMARY KEY CHECK (id ~ '^[a-z][a-z0-9_]{0,40}$'),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    -- Per meter: a number (a hard limit) or {"soft": n, "hard": n}.
    limits JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(limits) = 'object' AND octet_length(limits::text) <= 8192),
    -- What the plan includes beyond limits: sso, scim, audit retention.
    operations JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(operations) = 'object' AND octet_length(operations::text) <= 8192),
    public BOOLEAN NOT NULL DEFAULT false,
    -- The plan a new workspace starts on; exactly one.
    is_default BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX plans_one_default ON plans (is_default) WHERE is_default;

INSERT INTO plans (id, name, limits, operations, public, is_default) VALUES
    ('self_hosted', 'Self-hosted', '{}', '{"sso": true, "scim": true}', false, true),
    ('free', 'Free',
     '{"people": 10, "guests": 10, "storage_bytes": 2147483648, "file_bytes": 26214400,
       "hosted_model_micros": 0, "automation_heavy_runs": 1000, "emails": 3000,
       "form_submissions": 1000, "workspaces_owned": 3}',
     '{}', true, false);

CREATE TABLE workspace_plans (
    workspace_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    plan_id TEXT NOT NULL REFERENCES plans(id),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'trialing', 'past_due', 'grace', 'over_limit')),
    -- Staff grants over the plan's limits, in the plan's shape.
    overrides JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(overrides) = 'object' AND octet_length(overrides::text) <= 8192),
    trial_ends_at TIMESTAMPTZ,
    grace_ends_at TIMESTAMPTZ,
    source TEXT NOT NULL DEFAULT 'default'
        CHECK (source IN ('default', 'billing', 'staff', 'import')),
    version BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
SELECT tenancy_protect('workspace_plans', 'workspace_id');

-- Every workspace already here starts on the default plan.
INSERT INTO workspace_plans (workspace_id, plan_id)
SELECT organization.id, plan.id
FROM organizations organization, plans plan
WHERE plan.is_default
ON CONFLICT (workspace_id) DO NOTHING;

-- The ledger for counted meters: append-only, once per dedupe key.
CREATE TABLE usage_events (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL CHECK (meter ~ '^[a-z][a-z0-9_.]{0,63}$'),
    quantity BIGINT NOT NULL CHECK (quantity >= 0),
    account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    space_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL CHECK (source_kind ~ '^[a-z][a-z0-9_.]{0,63}$'),
    source_id UUID,
    dedupe_key TEXT NOT NULL CHECK (char_length(dedupe_key) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, dedupe_key)
);
CREATE INDEX usage_events_meter_time ON usage_events (workspace_id, meter, created_at);
SELECT tenancy_protect('usage_events', 'workspace_id');

-- Totals per period and scope, kept with the ledger. `scope_id` is the
-- workspace's id for scope 'workspace'.
CREATE TABLE usage_counters (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL CHECK (meter ~ '^[a-z][a-z0-9_.]{0,63}$'),
    period TEXT NOT NULL CHECK (period IN ('day', 'month')),
    period_start DATE NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace', 'account', 'space', 'conversation')),
    scope_id UUID NOT NULL,
    quantity BIGINT NOT NULL DEFAULT 0 CHECK (quantity >= 0),
    PRIMARY KEY (workspace_id, meter, period, period_start, scope_kind, scope_id)
);
SELECT tenancy_protect('usage_counters', 'workspace_id');

-- A workspace's own limits on a scope inside it, and A2's budgets
-- (ruling R13). `per_account` applies the limit to each account in the
-- scope on its own, for minute and hour periods only.
CREATE TABLE spend_limits (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace', 'account', 'space', 'conversation')),
    scope_id UUID NOT NULL,
    per_account BOOLEAN NOT NULL DEFAULT false,
    meter TEXT NOT NULL CHECK (meter ~ '^[a-z][a-z0-9_.]{0,63}$'),
    period TEXT NOT NULL CHECK (period IN ('minute', 'hour', 'day', 'month')),
    soft_limit BIGINT CHECK (soft_limit IS NULL OR soft_limit >= 0),
    hard_limit BIGINT CHECK (hard_limit IS NULL OR hard_limit >= 0),
    set_by_actor_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, scope_kind, scope_id, per_account, meter, period),
    CHECK (NOT per_account OR period IN ('minute', 'hour')),
    CHECK (soft_limit IS NOT NULL OR hard_limit IS NOT NULL)
);
SELECT tenancy_protect('spend_limits', 'workspace_id');

-- One notice per scope, period and level: near a limit (soft) and at it.
CREATE TABLE limit_notices (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL CHECK (meter ~ '^[a-z][a-z0-9_.]{0,63}$'),
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace', 'account', 'space', 'conversation')),
    scope_id UUID NOT NULL,
    period_start DATE NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('soft', 'hard')),
    notified_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, meter, scope_kind, scope_id, period_start, level)
);
SELECT tenancy_protect('limit_notices', 'workspace_id');
