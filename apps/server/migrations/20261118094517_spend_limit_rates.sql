-- H1.27, spend limits per account, space and conversation.
--
-- A limit over a minute or an hour is a rate, counted in the shared
-- limiter's store: it refuses or allows, so it is a hard limit only.
ALTER TABLE spend_limits ADD CONSTRAINT spend_limits_rates_are_hard
    CHECK (period IN ('day', 'month') OR (soft_limit IS NULL AND hard_limit IS NOT NULL));

-- `admit` reads a workspace's limits on one meter for every ask a limit can
-- cover (each message, once a workspace limits messages).
CREATE INDEX spend_limits_meter ON spend_limits (workspace_id, meter);

-- When a limit last changed, for Settings › Usage.
ALTER TABLE spend_limits ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
