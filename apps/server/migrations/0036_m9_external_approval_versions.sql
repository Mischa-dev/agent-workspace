-- M9 optimistic decision versions for addressed approvals.

ALTER TABLE agent_approvals
    ADD COLUMN version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);

CREATE OR REPLACE FUNCTION maintain_agent_approval_decision_version()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    decision_changed BOOLEAN;
BEGIN
    decision_changed := NEW.status IS DISTINCT FROM OLD.status
        OR NEW.response IS DISTINCT FROM OLD.response
        OR NEW.responded_by_actor_id IS DISTINCT FROM OLD.responded_by_actor_id
        OR NEW.responded_at IS DISTINCT FROM OLD.responded_at;

    IF decision_changed THEN
        NEW.version := OLD.version + 1;
    ELSIF NEW.version IS DISTINCT FROM OLD.version THEN
        RAISE EXCEPTION 'agent approval version is server maintained'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER maintain_agent_approval_decision_version
BEFORE UPDATE ON agent_approvals
FOR EACH ROW EXECUTE FUNCTION maintain_agent_approval_decision_version();
