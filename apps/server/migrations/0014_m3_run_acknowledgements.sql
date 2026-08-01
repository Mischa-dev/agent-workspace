-- Per-Actor failure acknowledgement stores the cause that was actually shown.
-- Clients cannot clear another Actor's red attention or acknowledge an event
-- that was not durably attached to the Agent Run timeline.

CREATE TABLE agent_run_acknowledgements (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    agent_run_id UUID NOT NULL,
    actor_id UUID NOT NULL,
    acknowledged_event_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_run_id, actor_id),
    FOREIGN KEY (project_id, agent_run_id)
        REFERENCES agent_runs(project_id, id),
    FOREIGN KEY (project_id, actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    FOREIGN KEY (acknowledged_event_id)
        REFERENCES agent_run_events(id)
);

CREATE INDEX agent_run_acknowledgements_actor_lookup
    ON agent_run_acknowledgements (project_id, actor_id, agent_run_id);

CREATE OR REPLACE FUNCTION enforce_agent_run_acknowledgement_scope()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM agent_run_events event
         WHERE event.id = NEW.acknowledged_event_id
           AND event.agent_run_id = NEW.agent_run_id
           AND event.kind = 'run_failed'
    ) THEN
        RAISE EXCEPTION 'Acknowledgement requires this Agent Run failure event'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_run_acknowledgements_enforce_scope
BEFORE INSERT OR UPDATE
ON agent_run_acknowledgements
FOR EACH ROW
EXECUTE FUNCTION enforce_agent_run_acknowledgement_scope();
