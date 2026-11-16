-- A1.9 (build 2G): which server process drives a built-in run, and until
-- when. A stopgap until K5.3's single-holder `builtin-host` lease: the
-- process that starts a turn takes the run's lease and renews it while the
-- turn is driven; recovery takes only runs whose lease is free, lapsed or
-- its own, so a second process (or a rolling deploy) never takes another
-- process's live runs. Removed when K5.3 lands. Both columns are empty on
-- every existing row, which recovery treats as it does today.
ALTER TABLE agent_runs
    ADD COLUMN driver_instance_id UUID,
    ADD COLUMN driver_lease_expires_at TIMESTAMPTZ,
    ADD CONSTRAINT agent_runs_driver_lease_pair
        CHECK ((driver_instance_id IS NULL) = (driver_lease_expires_at IS NULL));

-- Runs whose driver stopped renewing: what the lapsed-lease recovery reads.
CREATE INDEX agent_runs_driver_lease_lapsed
    ON agent_runs (driver_lease_expires_at)
    WHERE driver_instance_id IS NOT NULL
      AND state IN ('starting', 'working', 'blocked', 'recovering');
