-- K1.19: members, roles and workspace administration onto access.
--
-- * "Read the audit log" (`settings.audit`) joins the SQL permission
--   catalogue, beside `policy::PERMISSIONS`: owners and admins hold it by
--   default. A custom role that manages settings keeps reading the log.
-- * Workspace roles no longer follow project roles: every current
--   workspace role is kept as it is, now set by hand.
INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES ('settings.audit', 'settings', 'workspace', false, ARRAY['owner', 'admin']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

UPDATE workspace_roles
SET permissions = array_append(permissions, 'settings.audit')
WHERE NOT built_in
  AND 'settings.manage' = ANY (permissions)
  AND NOT 'settings.audit' = ANY (permissions);

UPDATE organization_memberships
SET role_explicit = true
WHERE NOT role_explicit;
