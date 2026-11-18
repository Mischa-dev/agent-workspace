-- A2.18 (R19): an agent is one account in every workspace it is in. Its
-- token and its connections, made where it was first added, now reach each
-- workspace it is a member of (added there by someone who may, under that
-- workspace's rule on outside accounts), not only the first. What it may do
-- in each is still its membership there; the token's scopes are unchanged.

CREATE OR REPLACE FUNCTION agent_connection_can_access(connection_uuid UUID, target_project UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM external_agent_connections c
        JOIN projects target ON target.id = target_project AND target.archived_at IS NULL
        JOIN actors a ON a.id = c.actor_id AND a.status = 'active'
        JOIN organization_memberships om ON om.organization_id = target.organization_id
          AND om.actor_id = a.id AND om.enabled
        JOIN project_memberships member ON member.project_id = target.id
          AND member.actor_id = a.id AND member.enabled
        WHERE c.id = connection_uuid AND c.status = 'active'
    );
$$;

-- A connection made for such a token reaches the same workspaces.
DO $$
DECLARE
    found RECORD;
BEGIN
    FOR found IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'agent_connections'::regclass AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%reaches_all%source_connection_id%'
    LOOP
        EXECUTE format('ALTER TABLE agent_connections DROP CONSTRAINT %I', found.conname);
    END LOOP;
END
$$;
UPDATE agent_connections
   SET reaches_all = true
 WHERE source_connection_id IS NOT NULL AND is_default AND NOT reaches_all;
