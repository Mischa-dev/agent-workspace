-- K1.8: profile agents are agent accounts of their workspace. Each gets a
-- workspace membership (the Member role, so it is in the directory with a
-- role like any agent; a hidden profile's is off), and any agent or app
-- account still without an owner belongs to the workspace it is in. Bare
-- per-project workers stay unlisted (K1.5). Runs once at the deploy; every
-- statement only touches rows still missing, so a stopped run resumes.

-- A profile agent now sits in its workspace's own space as a member as well
-- as being the worker in its project, so the old pairing only holds one
-- way: the worker role is still for profile agents alone, but a profile
-- agent may hold other roles elsewhere.
CREATE OR REPLACE FUNCTION validate_project_membership_actor_role()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    actor_kind TEXT;
BEGIN
    SELECT kind
      INTO actor_kind
      FROM actors
     WHERE id = NEW.actor_id
       FOR UPDATE;

    IF actor_kind IS NOT NULL
       AND NEW.role = 'worker'
       AND actor_kind <> 'execution_worker' THEN
        RAISE EXCEPTION 'The Worker Project role is only for profile agents'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

INSERT INTO organization_memberships (organization_id, actor_id, role, enabled)
SELECT DISTINCT ON (profile.actor_id)
       profile.organization_id, profile.actor_id, 'member', profile.hidden_at IS NULL
FROM agent_profiles profile
WHERE NOT EXISTS (
    SELECT 1 FROM organization_memberships existing
    WHERE existing.organization_id = profile.organization_id
      AND existing.actor_id = profile.actor_id
)
ORDER BY profile.actor_id, profile.created_at
ON CONFLICT DO NOTHING;

UPDATE actors actor
SET owner_workspace_id = profile.organization_id
FROM agent_profiles profile
WHERE profile.actor_id = actor.id
  AND actor.owner_workspace_id IS NULL
  AND actor.owner_account_id IS NULL;

-- Whoever made a profile is one of its agent's managers, as for any agent.
INSERT INTO account_managers (account_id, manager_account_id, workspace_id, added_by_actor_id)
SELECT DISTINCT ON (profile.actor_id)
       profile.actor_id, profile.created_by_actor_id, profile.organization_id,
       profile.created_by_actor_id
FROM agent_profiles profile
WHERE profile.created_by_actor_id IS NOT NULL
  AND profile.created_by_actor_id <> profile.actor_id
ORDER BY profile.actor_id, profile.created_at
ON CONFLICT DO NOTHING;
