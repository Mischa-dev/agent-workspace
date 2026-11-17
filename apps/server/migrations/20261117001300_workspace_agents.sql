-- K1.6: agents belong to the workspace, not a project (K1 §4.7).
--
-- * external_agent_connections.workspace_id (added with row security) is
--   the workspace the connection is in, indexed for listing. `project_id`
--   stays, meaning where the agent was first added; an agent added at
--   workspace level is first added to the workspace's own space.
-- * The connection and its credentials no longer hang off a project
--   membership: their foreign keys into project_memberships go, so removing
--   an agent from the project it was first added to leaves them whole.
--   `project_id` stays NOT NULL (an agent made at workspace level is first
--   added to the workspace's own space), so `UNIQUE (project_id, actor_id)`
--   still holds and no existing row has to change.
-- * agent_connection_can_access: an active account, an active connection,
--   an enabled membership of the connection's workspace and an enabled
--   membership of the target. The "anchor" membership of the first project
--   is no longer required (the visible change), and the account's kind is
--   not read (ruling R45).
-- * `validate_external_agent_connection` is dropped: it capped agents at
--   Member or Viewer and required a person to create them, both decided by
--   kind. What an agent is given is checked when it is given (the role is
--   within the giver's). The `runtime_kind = 'hermes'` CHECK the plan names
--   was already a format check (migration 0037).
-- * `external_agent_connections_name_manager` (K1.4's stopgap) is dropped:
--   `accounts::create_agent` names the creator as a manager itself.

CREATE INDEX external_agent_connections_workspace_listing
    ON external_agent_connections (workspace_id, status, created_at, id);

DO $$
DECLARE
    found RECORD;
BEGIN
    FOR found IN
        SELECT conrelid::regclass::text AS table_name, conname
        FROM pg_constraint
        WHERE conrelid IN ('external_agent_connections'::regclass, 'mcp_client_credentials'::regclass)
          AND contype = 'f'
          AND confrelid = 'project_memberships'::regclass
    LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', found.table_name, found.conname);
    END LOOP;
END
$$;

DROP TRIGGER IF EXISTS external_agent_connections_validate ON external_agent_connections;
DROP FUNCTION IF EXISTS validate_external_agent_connection();

DROP TRIGGER IF EXISTS external_agent_connections_name_manager ON external_agent_connections;
DROP FUNCTION IF EXISTS external_agent_connections_name_manager();

CREATE OR REPLACE FUNCTION agent_connection_can_access(connection_uuid UUID, target_project UUID)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM external_agent_connections c
        JOIN projects target ON target.id = target_project
          AND target.organization_id = c.workspace_id AND target.archived_at IS NULL
        JOIN actors a ON a.id = c.actor_id AND a.status = 'active'
        JOIN organization_memberships om ON om.organization_id = c.workspace_id
          AND om.actor_id = a.id AND om.enabled
        JOIN project_memberships member ON member.project_id = target.id
          AND member.actor_id = a.id AND member.enabled
        WHERE c.id = connection_uuid AND c.status = 'active'
    );
$$;
