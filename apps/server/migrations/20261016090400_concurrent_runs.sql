-- H8: an account may run several turns at once. The limit comes from the
-- account (bounded by its adapter's manifest in code); the host-wide cap
-- applies only to runs on the local host, so cloud runs never wait on it.

DROP INDEX agent_runs_one_active_turn_per_profile;

ALTER TABLE provider_accounts
    ADD COLUMN max_concurrent_runs SMALLINT NOT NULL DEFAULT 2 CHECK (
        max_concurrent_runs BETWEEN 1 AND 16
    );

ALTER TABLE execution_profiles DROP COLUMN max_active_turns;

CREATE INDEX agent_runs_active_by_profile
    ON agent_runs (execution_profile_id)
    WHERE state IN ('starting', 'working', 'blocked', 'recovering');
