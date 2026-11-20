-- K1.31: every agent and app account has its owner from the moment it is
-- made. `accounts::create_agent`, `create_app` and `ensure_worker` (and the
-- built-in host below) name the owner on insert, so the fill triggers of
-- K1.5 go, and `actors_owner` holds it: an agent or app has exactly one
-- owner unless it was retired with the workspace that owned it (disabled,
-- with none); a person or the system has none. The memberships' role-row
-- checks of K1.10 are validated too.

-- The built-in host's account belongs to its workspace, like the host.
CREATE OR REPLACE FUNCTION hosts_ensure_builtin(workspace UUID)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    account UUID;
BEGIN
    IF EXISTS (SELECT 1 FROM hosts WHERE owner_workspace_id = workspace AND builtin) THEN
        RETURN;
    END IF;
    account := gen_random_uuid();
    INSERT INTO actors (id, kind, status, owner_workspace_id)
    VALUES (account, 'integration', 'active', workspace);
    INSERT INTO hosts (
        id, account_id, owner_workspace_id, name, software, protocol_version,
        capabilities, builtin, enrolled_by_account_id
    )
    VALUES (
        gen_random_uuid(), account, workspace, 'This server', 'agent-workspace', '2026-11',
        '{"start": true, "cancel": true, "steer": true, "answer": true, "inspect": true}'::jsonb,
        true, account
    );
END
$$;

DROP TRIGGER organization_memberships_fill_account_owner ON organization_memberships;
DROP TRIGGER project_memberships_fill_account_owner ON project_memberships;
DROP FUNCTION memberships_fill_account_owner();
DROP TRIGGER hosts_fill_account_owner ON hosts;
DROP FUNCTION hosts_fill_account_owner();

-- At the deploy: any active agent or app still without an owner takes, in
-- order, its host's owner, the workspace of its first membership, or the
-- person who made it.
UPDATE actors account
SET owner_account_id = host.owner_account_id,
    owner_workspace_id = CASE WHEN host.owner_account_id IS NULL THEN host.owner_workspace_id END
FROM hosts host
WHERE host.account_id = account.id
  AND account.account_kind IN ('agent', 'app')
  AND account.owner_account_id IS NULL AND account.owner_workspace_id IS NULL;

UPDATE actors actor
SET owner_workspace_id = COALESCE(
    (SELECT membership.organization_id
     FROM organization_memberships membership
     WHERE membership.actor_id = actor.id
     ORDER BY membership.created_at
     LIMIT 1),
    (SELECT project.organization_id
     FROM project_memberships membership
     JOIN projects project ON project.id = membership.project_id
     WHERE membership.actor_id = actor.id
     ORDER BY membership.created_at
     LIMIT 1))
WHERE actor.account_kind IN ('agent', 'app')
  AND actor.owner_account_id IS NULL AND actor.owner_workspace_id IS NULL;

UPDATE actors actor
SET owner_account_id = actor.created_by_actor_id
FROM actors creator
WHERE creator.id = actor.created_by_actor_id
  AND creator.account_kind = 'person'
  AND actor.account_kind IN ('agent', 'app')
  AND actor.owner_account_id IS NULL AND actor.owner_workspace_id IS NULL;

-- Whatever is left belongs to nobody and works for nobody: retired, as a
-- deleted workspace's accounts are.
UPDATE actors
SET status = 'disabled', updated_at = now()
WHERE account_kind IN ('agent', 'app')
  AND owner_account_id IS NULL AND owner_workspace_id IS NULL
  AND status <> 'disabled';

ALTER TABLE actors ADD CONSTRAINT actors_owner CHECK (
    CASE WHEN account_kind IN ('agent', 'app')
        THEN num_nonnulls(owner_account_id, owner_workspace_id) = 1 OR status = 'disabled'
        ELSE num_nonnulls(owner_account_id, owner_workspace_id) = 0
    END
);

-- Every membership names its role row (K1.10 added the checks NOT VALID and
-- filled the rows; the trigger fills any since). Validated now.
UPDATE organization_memberships SET role_id = NULL WHERE role_id IS NULL;
UPDATE project_memberships SET role_id = NULL WHERE role_id IS NULL;
ALTER TABLE organization_memberships VALIDATE CONSTRAINT organization_memberships_role_row;
ALTER TABLE project_memberships VALIDATE CONSTRAINT project_memberships_role_row;
