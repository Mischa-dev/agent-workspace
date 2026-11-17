-- Test fixture: every agent account holding a role with `pages.edit` moves
-- to a copy of that role, "R, suggests": R's permissions without
-- `pages.edit` (and a few capabilities), with `pages.suggest` and
-- `pages.edit_own`. Tests whose agent should suggest on other people's
-- pages run it after their setup. Selecting by account kind is fine here:
-- it only builds the test's roles.

CREATE TEMPORARY TABLE suggest_roles ON COMMIT DROP AS
SELECT source.id AS source_id,
       gen_random_uuid() AS role_id,
       source.organization_id,
       left(source.key, 31) || '-suggests' AS key,
       CASE WHEN source.base_role = 'owner' THEN 'admin' ELSE source.base_role END AS base_role,
       left(source.name, 50) || ', suggests' AS name,
       ARRAY(
           SELECT DISTINCT held
           FROM unnest(
               source.effective_permissions || ARRAY['pages.suggest', 'pages.edit_own']
           ) AS held
           WHERE held NOT IN (
               'pages.edit', 'runs.launch', 'runs.accounts', 'replies.steer',
               'conversations.members', 'requests.answer_sensitive', 'webhooks.manage'
           )
           ORDER BY held
       ) AS permissions,
       source.agent_limit,
       source.defaults
FROM workspace_roles source
WHERE 'pages.edit' = ANY(source.effective_permissions)
  AND (
      EXISTS (
          SELECT 1 FROM organization_memberships membership
          JOIN actors account ON account.id = membership.actor_id
          WHERE membership.role_id = source.id
            AND account.kind IN ('external_agent', 'execution_worker')
      )
      OR EXISTS (
          SELECT 1 FROM project_memberships membership
          JOIN actors account ON account.id = membership.actor_id
          WHERE membership.role_id = source.id
            AND account.kind IN ('external_agent', 'execution_worker')
      )
  );

INSERT INTO workspace_roles (
    id, organization_id, key, built_in, base_role, name, description, permissions,
    agent_limit, defaults
)
SELECT role_id, organization_id, key, false, base_role, name,
       'Edits to pages others made are suggestions.',
       permissions, agent_limit, defaults
FROM suggest_roles
ON CONFLICT DO NOTHING;

UPDATE organization_memberships membership
SET role_id = suggest.role_id, role_explicit = true, version = membership.version + 1, updated_at = now()
FROM suggest_roles suggest
JOIN workspace_roles made ON made.id = suggest.role_id
JOIN actors account ON true
WHERE membership.role_id = suggest.source_id
  AND account.id = membership.actor_id
  AND account.kind IN ('external_agent', 'execution_worker');

UPDATE project_memberships membership
SET role_id = suggest.role_id, version = membership.version + 1, updated_at = now()
FROM suggest_roles suggest
JOIN workspace_roles made ON made.id = suggest.role_id
JOIN actors account ON true
WHERE membership.role_id = suggest.source_id
  AND account.id = membership.actor_id
  AND account.kind IN ('external_agent', 'execution_worker');
