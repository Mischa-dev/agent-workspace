-- K1.12: suggesting instead of editing is a permission, not an account kind.
--
-- Until now the server made an agent's edits to a page someone else made a
-- suggestion, let agents decide suggestions only on their own pages, and
-- refused agents a few capabilities by hand. Those rules are gone; roles
-- say it instead (`pages.suggest`, `pages.edit_own`, `access::write_mode`).
-- So that no live agent can do more than it did the moment the rules go
-- (K1 section 4.5), each role R held by an agent account that holds
-- `pages.edit` gets a copy, "R, suggests": R's permissions without
-- `pages.edit` and the capabilities refused to agents by hand, with
-- `pages.suggest` and `pages.edit_own`. Those agents' memberships move to
-- it. An owner gives any agent plain Member (or more) to give it a person's
-- access. This is the only step that selects by account kind, and it runs
-- once. New agents get `access.default_agent_role_id`, unset by default:
-- the role new people get (ruling R5).

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
       'Made for existing agents when suggesting became a permission: edits to pages others made are suggestions.',
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
