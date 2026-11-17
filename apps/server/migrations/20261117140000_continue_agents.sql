-- K1.14b (chat): "Let agents continue" (`conversations.continue_agents`)
-- in the SQL permission catalogue, beside `access::catalogue::PERMISSIONS`.
-- Giving the agents in a group another round was for people only; now a
-- role decides it, held by default by owners, admins and members. Custom
-- roles (and stored limits on apps acting for someone) that hold its parent
-- `channels.post` get it, so a role saved before means the same after.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('conversations.continue_agents', 'channels', 'space', false,
        ARRAY['owner', 'admin', 'member']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

UPDATE workspace_roles
SET permissions = permissions || ARRAY['conversations.continue_agents']::text[]
WHERE NOT built_in
  AND 'channels.post' = ANY(permissions)
  AND NOT ('conversations.continue_agents' = ANY(permissions));

UPDATE workspace_roles
SET agent_limit = agent_limit || ARRAY['conversations.continue_agents']::text[]
WHERE agent_limit IS NOT NULL
  AND 'channels.post' = ANY(agent_limit)
  AND NOT ('conversations.continue_agents' = ANY(agent_limit));

UPDATE organization_memberships
SET agent_limit = agent_limit || ARRAY['conversations.continue_agents']::text[]
WHERE agent_limit IS NOT NULL
  AND 'channels.post' = ANY(agent_limit)
  AND NOT ('conversations.continue_agents' = ANY(agent_limit));
