-- U2: tokens and cost. One row per run and model, added to as the harness
-- reports usage, and priced from the workspace's model catalog when the
-- run records it. A harness that reports its own cost keeps that beside it,
-- for models the catalog has no price for.
CREATE TABLE agent_run_usage (
    agent_run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    provider_account_id UUID REFERENCES provider_accounts(id) ON DELETE SET NULL,
    adapter_kind TEXT NOT NULL CHECK (adapter_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    usage_date DATE NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    -- The estimate from the catalog's prices, in `currency`.
    estimated_cost DOUBLE PRECISION CHECK (estimated_cost IS NULL OR estimated_cost >= 0),
    currency TEXT CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    -- What the harness itself said the run cost, in US dollars.
    reported_cost_usd DOUBLE PRECISION CHECK (reported_cost_usd IS NULL OR reported_cost_usd >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_run_id, model)
);

CREATE INDEX agent_run_usage_project_day
    ON agent_run_usage (organization_id, project_id, usage_date);
CREATE INDEX agent_run_usage_account_day
    ON agent_run_usage (provider_account_id, usage_date);
