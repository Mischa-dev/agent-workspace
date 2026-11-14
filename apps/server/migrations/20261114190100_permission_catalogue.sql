-- K1.3: the permission catalogue as rows, and one complete SQL role check.
--
-- P0.8 fixed the SQL helpers that read only a custom role's list (finding 4)
-- by naming the base roles in Rust. Here the catalogue becomes data SQL can
-- read too, so one function answers "does this membership's role hold this
-- key" exactly as `policy::RoleGrant::stored` does: the built-in role holds
-- it by default, and a custom role, if any, kept it (required keys always
-- kept; owners always hold everything their base does).
--
-- * permission_catalogue: one row per key, kept equal to `policy::PERMISSIONS`
--   by the integration test `roles_and_access::catalogue_matches_sql`. A
--   slice that adds a key adds its row in its own migration. `thing_kind`
--   and `level` are filled by K1.9. Global: the catalogue is the server's,
--   the same for every workspace.
-- * role_allows(role, role_id, key): the check. `custom_role_allows` stays
--   for its callers until K1.20 moves them to `access::sql_filter`.

CREATE TABLE permission_catalogue (
    key TEXT PRIMARY KEY CHECK (key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    module TEXT NOT NULL CHECK (char_length(module) BETWEEN 1 AND 40),
    -- NULL for a capability (K1.9).
    thing_kind TEXT,
    level TEXT CHECK (level IN ('see', 'comment', 'suggest', 'edit_own', 'edit', 'manage')),
    scope TEXT NOT NULL CHECK (scope IN ('space', 'workspace', 'both')),
    required BOOLEAN NOT NULL DEFAULT false,
    -- The built-in roles that hold it by default.
    default_for TEXT[] NOT NULL DEFAULT '{}'
        CHECK (default_for <@ ARRAY['owner', 'admin', 'member', 'viewer', 'guest']::text[]),
    CHECK ((thing_kind IS NULL) = (level IS NULL))
);

INSERT INTO permission_catalogue (key, module, scope, required, default_for)
VALUES
    ('projects.view', 'projects', 'space', true, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('projects.create', 'projects', 'workspace', false, ARRAY['owner', 'admin']::text[]),
    ('projects.manage', 'projects', 'space', false, ARRAY['owner', 'admin']::text[]),
    ('tasks.view', 'tasks', 'space', false, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('tasks.create', 'tasks', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('tasks.edit', 'tasks', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('tasks.delete', 'tasks', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('tasks.workflow', 'tasks', 'space', false, ARRAY['owner', 'admin']::text[]),
    ('channels.post', 'channels', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('channels.create', 'channels', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('channels.moderate', 'channels', 'space', false, ARRAY['owner', 'admin']::text[]),
    ('pages.edit', 'pages', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('whiteboards.edit', 'whiteboards', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('collections.edit', 'collections', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('collections.build', 'collections', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('sheets.edit', 'sheets', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('runs.launch', 'runs', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('runs.approve', 'runs', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('runs.interrupt', 'runs', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('runs.accounts', 'runs', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('code.view', 'code', 'space', false, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('code.publish', 'code', 'space', false, ARRAY['owner', 'admin', 'member']::text[]),
    ('code.manage', 'code', 'space', false, ARRAY['owner', 'admin']::text[]),
    ('agents.manage', 'agents', 'space', false, ARRAY['owner', 'admin']::text[]),
    ('members.view', 'members', 'both', false, ARRAY['owner', 'admin']::text[]),
    ('directory.view', 'members', 'workspace', false, ARRAY['owner', 'admin', 'member', 'viewer']::text[]),
    ('members.invite', 'members', 'both', false, ARRAY['owner', 'admin']::text[]),
    ('members.manage', 'members', 'both', false, ARRAY['owner', 'admin']::text[]),
    ('roles.manage', 'members', 'workspace', false, ARRAY['owner', 'admin']::text[]),
    ('settings.manage', 'settings', 'both', false, ARRAY['owner', 'admin']::text[])
ON CONFLICT (key) DO UPDATE
SET module = EXCLUDED.module, scope = EXCLUDED.scope, required = EXCLUDED.required,
    default_for = EXCLUDED.default_for;

-- Whether a membership whose role is `role` (and custom role `role_id`,
-- NULL for the built-in one) holds `permission`. A worker holds nothing;
-- an unknown key is held by nobody.
CREATE FUNCTION role_allows(role TEXT, role_id UUID, permission TEXT)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE((
        SELECT role = ANY(catalogue.default_for)
           AND (role_id IS NULL
                OR role = 'owner'
                OR catalogue.required
                OR COALESCE(permission = ANY(workspace_role_permissions(role_id)), false))
        FROM permission_catalogue catalogue
        WHERE catalogue.key = permission
    ), false)
$$;
