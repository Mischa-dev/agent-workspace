-- U5: usage over time, for the usage page.
--
-- usage_windows keeps only each window's latest reading. These keep its
-- history: a sample at most every few minutes (for the chart), and one row
-- per window cycle with the most it reached before resetting (for "how much
-- went unused"). Both belong to the account's workspace and are pruned by
-- the usage.poll_due job.

CREATE TABLE usage_window_samples (
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    window_key TEXT NOT NULL CHECK (char_length(window_key) BETWEEN 1 AND 64),
    used_percent SMALLINT NOT NULL CHECK (used_percent BETWEEN 0 AND 100),
    resets_at TIMESTAMPTZ,
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, window_key, sampled_at)
);

CREATE INDEX usage_window_samples_age ON usage_window_samples (sampled_at);

-- One cycle of one window: from when it last reset until resets_at.
CREATE TABLE usage_window_cycles (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES provider_accounts(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    window_key TEXT NOT NULL CHECK (char_length(window_key) BETWEEN 1 AND 64),
    duration_minutes INTEGER CHECK (duration_minutes IS NULL OR duration_minutes BETWEEN 1 AND 525600),
    resets_at TIMESTAMPTZ NOT NULL,
    -- The most it reached, and the latest reading, before the reset.
    peak_used_percent SMALLINT NOT NULL CHECK (peak_used_percent BETWEEN 0 AND 100),
    last_used_percent SMALLINT NOT NULL CHECK (last_used_percent BETWEEN 0 AND 100),
    first_read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_read_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX usage_window_cycles_account
    ON usage_window_cycles (account_id, window_key, resets_at DESC);
CREATE INDEX usage_window_cycles_age ON usage_window_cycles (resets_at);
