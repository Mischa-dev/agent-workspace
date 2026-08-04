-- M7 human membership provenance and reversible disable lifecycle.

ALTER TABLE project_memberships
    ADD COLUMN invited_by_actor_id UUID,
    ADD COLUMN accepted_invitation_id UUID UNIQUE,
    ADD COLUMN accepted_at TIMESTAMPTZ,
    ADD COLUMN disabled_by_actor_id UUID,
    ADD COLUMN disabled_at TIMESTAMPTZ,
    ADD CONSTRAINT project_memberships_inviter_fk
        FOREIGN KEY (project_id, invited_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT project_memberships_acceptance_fk
        FOREIGN KEY (accepted_invitation_id)
        REFERENCES project_invitations(id),
    ADD CONSTRAINT project_memberships_disabler_fk
        FOREIGN KEY (project_id, disabled_by_actor_id)
        REFERENCES project_memberships(project_id, actor_id),
    ADD CONSTRAINT project_memberships_acceptance_shape_check CHECK (
        (accepted_invitation_id IS NULL AND accepted_at IS NULL)
        OR (accepted_invitation_id IS NOT NULL AND accepted_at IS NOT NULL)
    );

-- Existing disabled rows predate disable provenance. Their existing update
-- timestamp is the only durable timestamp available; no Actor is invented.
UPDATE project_memberships
SET disabled_at = updated_at
WHERE NOT enabled AND disabled_at IS NULL;

CREATE FUNCTION maintain_project_membership_disable_provenance()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT NEW.enabled THEN
        NEW.disabled_at := COALESCE(NEW.disabled_at, now());
    ELSE
        NEW.disabled_by_actor_id := NULL;
        NEW.disabled_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER project_memberships_disable_provenance_trigger
BEFORE INSERT OR UPDATE OF enabled, disabled_by_actor_id, disabled_at
ON project_memberships
FOR EACH ROW
EXECUTE FUNCTION maintain_project_membership_disable_provenance();

ALTER TABLE project_memberships
    ADD CONSTRAINT project_memberships_disable_shape_check CHECK (
        (enabled AND disabled_by_actor_id IS NULL AND disabled_at IS NULL)
        OR (NOT enabled AND disabled_at IS NOT NULL)
    );

CREATE INDEX project_memberships_human_management
    ON project_memberships (project_id, enabled, role, created_at, actor_id)
    WHERE role <> 'worker';
