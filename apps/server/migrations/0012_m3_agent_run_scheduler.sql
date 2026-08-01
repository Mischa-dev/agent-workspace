-- Durable scheduler ownership for M3 hosted Agent Runs. Queue rows remain
-- independent of physical repository work until a short lease admits them.

ALTER TABLE agent_runs
    ADD COLUMN predecessor_run_id UUID,
    ADD COLUMN scheduler_claim_id UUID,
    ADD COLUMN scheduler_claimed_at TIMESTAMPTZ,
    ADD COLUMN scheduler_lease_expires_at TIMESTAMPTZ,
    ADD CONSTRAINT agent_runs_predecessor_not_self_check CHECK (
        predecessor_run_id IS NULL OR predecessor_run_id <> id
    ),
    ADD CONSTRAINT agent_runs_scheduler_claim_shape_check CHECK (
        (
            scheduler_claim_id IS NULL
            AND scheduler_claimed_at IS NULL
            AND scheduler_lease_expires_at IS NULL
        )
        OR
        (
            scheduler_claim_id IS NOT NULL
            AND scheduler_claimed_at IS NOT NULL
            AND scheduler_lease_expires_at IS NOT NULL
            AND scheduler_lease_expires_at > scheduler_claimed_at
        )
    );

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_predecessor_project_fk
        FOREIGN KEY (project_id, predecessor_run_id)
        REFERENCES agent_runs(project_id, id);

CREATE INDEX agent_runs_scheduler_claimable
    ON agent_runs (created_at, id)
    WHERE state = 'queued';

CREATE INDEX agent_runs_scheduler_expiry
    ON agent_runs (scheduler_lease_expires_at)
    WHERE scheduler_claim_id IS NOT NULL;

CREATE OR REPLACE FUNCTION enforce_agent_run_identity()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS DISTINCT FROM OLD.project_id
       OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
       OR NEW.execution_worker_id IS DISTINCT FROM OLD.execution_worker_id
       OR NEW.requesting_actor_id IS DISTINCT FROM OLD.requesting_actor_id
       OR NEW.execution_profile_id IS DISTINCT FROM OLD.execution_profile_id
       OR NEW.repository_id IS DISTINCT FROM OLD.repository_id
       OR NEW.prompt IS DISTINCT FROM OLD.prompt
       OR NEW.authorization_snapshot IS DISTINCT FROM OLD.authorization_snapshot
       OR NEW.predecessor_run_id IS DISTINCT FROM OLD.predecessor_run_id THEN
        RAISE EXCEPTION 'Agent Run creation identity is immutable'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.repository_workspace_id IS NOT NULL
       AND NEW.repository_workspace_id IS DISTINCT FROM OLD.repository_workspace_id THEN
        RAISE EXCEPTION 'Agent Run Repository Workspace binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.provider_session_id IS NOT NULL
       AND NEW.provider_session_id IS DISTINCT FROM OLD.provider_session_id THEN
        RAISE EXCEPTION 'Agent Run Provider Session binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.provider_turn_id IS NOT NULL
       AND NEW.provider_turn_id IS DISTINCT FROM OLD.provider_turn_id THEN
        RAISE EXCEPTION 'Agent Run provider turn binding is immutable once set'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;
