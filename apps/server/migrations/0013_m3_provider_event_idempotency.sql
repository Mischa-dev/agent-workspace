-- Provider notification replay must converge on one durable timeline artifact.
-- Existing M0/M1 data contains at most one Worker Message per Agent Run; fail
-- the migration rather than silently choosing among duplicates if that
-- invariant was ever violated.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM messages
         WHERE agent_run_id IS NOT NULL
         GROUP BY agent_run_id
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'Agent Run has multiple Worker Messages';
    END IF;
END;
$$;

CREATE UNIQUE INDEX messages_one_worker_message_per_agent_run
    ON messages (agent_run_id)
    WHERE agent_run_id IS NOT NULL;

ALTER TABLE agent_approvals
    ADD COLUMN provider_response_claimed_at TIMESTAMPTZ,
    ADD COLUMN provider_response_delivered_at TIMESTAMPTZ,
    ADD CONSTRAINT agent_approvals_provider_response_shape_check CHECK (
        provider_response_delivered_at IS NULL
        OR (
            provider_response_claimed_at IS NOT NULL
            AND status IN ('resolved', 'canceled')
        )
    );
