-- K1.4: one rule for managing an agent (K1 plan, sections 3.2 and 3.6;
-- rulings R19 and R34).
--
-- A caller may manage an agent or app account when they are its owning
-- account, or hold `agents.manage_any` in its owning workspace, or are named
-- in `account_managers` for it and hold `agents.manage` in a workspace the
-- account is a member of (`access::may_manage_account`).
--
-- Backfill: each agent and app gets as managers the people who made its
-- connections and every enabled owner and admin of its owning workspace
-- today, the people who pass the web rule now. Who can manage which agent
-- stays the same, except that the OAuth and connected-apps gap closes.
--
-- Triggers added (ruling R16): `external_agent_connections_name_manager`
-- names a connection's creator as a manager of its agent, for the code
-- that still makes connections directly. Removed in K1.6, when
-- `accounts::create_agent` names the managers itself.

CREATE TABLE account_managers (
    account_id UUID NOT NULL REFERENCES actors(id),
    manager_account_id UUID NOT NULL REFERENCES actors(id),
    -- Where the naming happened (audit); naming is global (R19), so it is
    -- nullable for managers the owning person names.
    workspace_id UUID REFERENCES organizations(id) ON DELETE SET NULL,
    added_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, manager_account_id),
    CHECK (account_id <> manager_account_id)
);

CREATE INDEX account_managers_manager ON account_managers (manager_account_id, workspace_id);

-- The connections' creators.
INSERT INTO account_managers (account_id, manager_account_id, workspace_id, added_by_actor_id)
SELECT DISTINCT ON (connection.actor_id, connection.created_by_actor_id)
       connection.actor_id, connection.created_by_actor_id, project.organization_id,
       connection.created_by_actor_id
FROM external_agent_connections connection
JOIN projects project ON project.id = connection.project_id
JOIN actors creator ON creator.id = connection.created_by_actor_id
WHERE connection.actor_id <> connection.created_by_actor_id
ORDER BY connection.actor_id, connection.created_by_actor_id, connection.created_at
ON CONFLICT DO NOTHING;

-- Every enabled owner and admin (a person) of the owning workspace.
INSERT INTO account_managers (account_id, manager_account_id, workspace_id)
SELECT account.id, membership.actor_id, account.owner_workspace_id
FROM actors account
JOIN organization_memberships membership
  ON membership.organization_id = account.owner_workspace_id
 AND membership.enabled
 AND membership.role IN ('owner', 'admin')
JOIN actors manager ON manager.id = membership.actor_id AND manager.account_kind = 'person'
WHERE account.account_kind IN ('agent', 'app')
ON CONFLICT DO NOTHING;

CREATE FUNCTION external_agent_connections_name_manager()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO account_managers (account_id, manager_account_id, workspace_id, added_by_actor_id)
    SELECT NEW.actor_id, NEW.created_by_actor_id, project.organization_id, NEW.created_by_actor_id
    FROM projects project
    WHERE project.id = NEW.project_id AND NEW.actor_id <> NEW.created_by_actor_id
    ON CONFLICT DO NOTHING;
    RETURN NULL;
END;
$$;

CREATE TRIGGER external_agent_connections_name_manager
AFTER INSERT ON external_agent_connections
FOR EACH ROW EXECUTE FUNCTION external_agent_connections_name_manager();

-- The new permission, held by the owner starting role only.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('agents.manage_any', 'agents', 'workspace', false, ARRAY['owner']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;
